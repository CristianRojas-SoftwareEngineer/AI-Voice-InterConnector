// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `cleanup`: borrado del estado por categorías, con el planificador que
//! solo actúa dentro de raíces de propiedad exclusiva.
//!
//! **Una sola función de cálculo.** [`plan`] construye la lista de destinos con sus
//! tamaños y la usan las tres cosas que la necesitan: el resumen de la
//! confirmación, la salida de `--dry-run` y la ejecución. El plan y la ejecución no
//! pueden divergir porque no hay dos implementaciones;
//! `shared_hf_cache_keeps_foreign_entries` lo afirma con una raíz compartida.
//!
//! **Reglas de propiedad.** Solo se borra dentro de raíces de propiedad
//! exclusiva. El directorio de programa no lo toca `cleanup` en absoluto —eso es
//! `self uninstall`— y aparece en el plan solo como recurso compartido que se
//! conserva. En la raíz de modelos **exclusiva** el borrado es de directorio
//! entero, con los snapshots, los locks y `xet` dentro, porque
//! `xet` cuelga de ella. En la raíz **compartida** que el
//! usuario eligió con `HF_HUB_CACHE` o `HF_HOME` solo se borran los repos fijados
//! y sus locks: nunca `xet` ni el `.locks` completo, ni un repo de
//! otra herramienta.
//!
//! **Nada de lo que el daemon usa se borra sin pararlo antes**, y el fallo
//! de la parada es `daemon_stop_failed` con nada del plan borrado.
//!
//! **Tras `--model` la aplicación queda reintentable**: `setup` vuelve a
//! descargar lo que falte, porque la provisión no depende de nada que sobreviva al
//! borrado.

use crate::confirm::{self, Confirmation, Decision, Kind};
use crate::daemon_stop::{self, ProcessControl};
use crate::ownership;
use crate::receipt::InstallReceipt;
use crate::recovery::{self, Roots as RecoveryRoots, SweepPreview};
use crate::LifecycleError;
use avi_shared::paths::{NodeKind, OwnershipNode};
use same_file::Handle;
use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};

/// Categoría a la que pertenece un destino, para el mensaje humano y para
/// que el envelope pueda decir qué se borró y por qué.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// `--model`: los modelos provisionados.
    Model,
    /// `--voices`: las voces que puede borrar y el arrastre de su habla.
    Voices,
    /// `--synthetic-speech`: la raíz de habla sintetizada entera.
    SyntheticSpeech,
    /// `--all`: configuración persistida.
    Config,
    /// `--all`: logs.
    Logs,
    /// `--all`: el estado de ejecución del daemon (`daemon.pid` y su fichero ready).
    DaemonState,
    /// `--all`: la hoja de temporales de la aplicación, entera.
    Scratch,
    /// `self uninstall`: la raíz de datos del usuario entera (voces, habla
    /// sintetizada, configuración, logs y estado del daemon).
    UserData,
}

impl Category {
    /// Descripción en español de la categoría, la que se muestra en el plan.
    pub fn describe(self) -> &'static str {
        match self {
            Self::Model => "modelos",
            Self::Voices => "voces",
            Self::SyntheticSpeech => "locuciones",
            Self::Config => "configuración",
            Self::Logs => "registros",
            Self::DaemonState => "estado del daemon",
            Self::Scratch => "temporales",
            Self::UserData => "datos de usuario",
        }
    }
}

/// Raíces sobre las que opera `cleanup`, como dato y no como llamada a
/// `avi-store`.
///
/// El motivo es el mismo que en [`crate::install::Env`]: las pruebas aisladas tienen
/// aisladas funcionen con las raíces reubicadas a temporales, y en Windows las Known
/// Folders ignoran `LOCALAPPDATA`, así que un sandbox que no pase las raíces por
/// parámetro no representaría nada del mecanismo que se quiere probar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Roots {
    /// Directorio de programa. `cleanup` no lo borra —eso es `self
    /// uninstall`—, pero su padre es donde vive el staging y él donde los aparcados.
    pub program_dir: PathBuf,
    /// Raíz de datos: voces, habla sintetizada, configuración, logs y pidfile.
    pub data_dir: PathBuf,
    /// Contenedor de la caché regenerable: de él cuelgan los modelos y los scratch.
    pub cache_container: PathBuf,
    /// Contenedor raíz de la aplicación (Windows: `%LOCALAPPDATA%\ai-voice-interconnector`)
    /// cuando la raíz de datos o el contenedor de caché cuelgan de él.
    pub app_container: Option<PathBuf>,
    /// Hoja de modelos exclusiva de la aplicación, `<contenedor de caché>/models`:
    /// snapshots, locks y `xet`. Es siempre propia, aunque el usuario fije después una
    /// caché HF compartida: esa es [`Roots::shared_models_dir`].
    pub models_dir: PathBuf,
    /// Raíz de los scratch por proceso, para el barrido transversal.
    pub scratch_root: PathBuf,
    /// Directorio del enlace del comando, donde también se barren temporales.
    pub bin_dir: PathBuf,
    /// `$HOME`, una de las rutas que el directorio de programa nunca puede ser. Vacío donde el producto no lo necesita.
    pub home: PathBuf,
    /// Caché HF compartida que eligió el usuario (`HF_HUB_CACHE` o `HF_HOME`) y que
    /// rige ahora, si rige. Es una raíz externa **adicional** a la hoja exclusiva: de
    /// ella `--model` solo borra los repos fijados y sus locks.
    ///
    /// Va como **dato** y no se vuelve a leer del entorno dentro del planificador por
    /// dos razones. La primera es el aislamiento de las pruebas: leer
    /// `HF_HUB_CACHE` desde el planificador hace
    /// que dos pruebas que corren en paralelo se contaminen, porque el entorno del
    /// proceso es global. La segunda es que el plan tiene que ser una función de sus
    /// entradas: si el plan dependiera del entorno, `--dry-run` podría anunciar un
    /// borrado distinto del que la ejecución hace un instante después, que es
    /// exactamente el defecto que este planificador absorbe.
    pub shared_models_dir: Option<PathBuf>,
}

impl Roots {
    /// Raíces del producto resueltas ahora, que es lo que usa el binario. El
    /// directorio de programa es el registrado, el mismo que usan `self update` y
    /// `self uninstall`, para que todas las operaciones tomen el mismo bloqueo.
    /// Falla si el recibo existe pero es ilegible o si alguna ruta del producto
    /// no se puede resolver.
    pub fn resolve() -> anyhow::Result<Self> {
        Self::with_program_dir(crate::channel::registered_program_dir()?)
    }

    /// Raíces del entorno con el directorio de programa ya decidido.
    fn with_program_dir(program_dir: PathBuf) -> anyhow::Result<Self> {
        let data_dir = crate::data_dir()?;
        let cache_container = crate::cache_container()?;
        let models_dir = cache_container.join(crate::MODELS_SUBDIR);
        Ok(Self {
            program_dir,
            app_container: windows_app_container(&data_dir, &cache_container),
            data_dir,
            cache_container,
            shared_models_dir: current_shared_models_dir(&models_dir)?,
            models_dir,
            scratch_root: crate::scratch_root()?,
            bin_dir: crate::bin_dir()?,
            home: std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .map(PathBuf::from)
                .map_err(|_| {
                    anyhow::anyhow!(
                        "no se pudo determinar el directorio del usuario: ni HOME ni USERPROFILE están definidas"
                    )
                })?,
        })
    }

    /// Raíces efectivas de la instalación registrada, con el recibo como fuente de
    /// verdad de los datos y de los modelos: los valores efectivos se registran en el
    /// recibo, así que la actualización y la desinstalación operan sobre las mismas
    /// ubicaciones aunque la variable ya no esté definida.
    pub fn from_receipt(receipt: Option<&InstallReceipt>) -> anyhow::Result<Self> {
        let mut roots = Self::with_program_dir(crate::channel::registered_install_dir(receipt)?)?;
        if let Some(receipt) = receipt {
            roots.data_dir = receipt.roots.data_dir.clone();
            roots.cache_container = receipt.roots.cache_dir.clone();
            roots.app_container = windows_app_container(&roots.data_dir, &roots.cache_container);
            // Los temporales cuelgan del mismo contenedor de caché que el recibo registra.
            roots.scratch_root = receipt
                .roots
                .cache_dir
                .join(avi_shared::paths::SCRATCH_SUBDIR);
            // El recibo registra el contenedor de caché: los modelos exclusivos son su
            // hoja `models`, y lo siguen siendo aunque el usuario fije después una
            // caché HF compartida, que es una raíz externa adicional.
            roots.models_dir = receipt.roots.cache_dir.join(crate::MODELS_SUBDIR);
            roots.shared_models_dir = current_shared_models_dir(&roots.models_dir)?;
        }
        Ok(roots)
    }

    /// Raíz de modelos que la aplicación lee ahora: la caché HF compartida si rige y,
    /// si no, la hoja exclusiva.
    pub fn effective_models_dir(&self) -> &Path {
        self.shared_models_dir
            .as_deref()
            .unwrap_or(&self.models_dir)
    }

    /// Árbol de propiedad de estas raíces, con cada padre antes que sus hijos: el
    /// contenedor raíz (si lo hay), la hoja de datos, el contenedor de caché y sus
    /// hojas de modelos y temporales. La hoja de modelos es siempre `Leaf`; la raíz
    /// compartida vigente se añade como nodo `External`.
    pub fn ownership_tree(&self) -> Vec<OwnershipNode> {
        let node = |path: &PathBuf, kind| OwnershipNode {
            path: path.clone(),
            kind,
        };
        let mut tree = Vec::new();
        if let Some(app) = &self.app_container {
            tree.push(node(app, NodeKind::Container));
        }
        tree.push(node(&self.data_dir, NodeKind::Leaf));
        tree.push(node(&self.cache_container, NodeKind::Container));
        tree.push(node(&self.models_dir, NodeKind::Leaf));
        tree.push(node(&self.scratch_root, NodeKind::Leaf));
        if let Some(shared) = &self.shared_models_dir {
            tree.push(node(shared, NodeKind::External));
        }
        tree
    }

    /// Raíces de la recuperación: las mismas que usan `cleanup`, `self uninstall`,
    /// `self update` y `doctor`.
    pub fn recovery_roots(&self) -> RecoveryRoots<'_> {
        RecoveryRoots {
            program_dir: &self.program_dir,
            scratch_root: &self.scratch_root,
            data_dir: &self.data_dir,
            bin_dir: &self.bin_dir,
        }
    }
}

/// Contenedor raíz de la aplicación en Windows, deducido de la forma por defecto
/// `%LOCALAPPDATA%\ai-voice-interconnector\{data,cache}`: el padre de la raíz de datos
/// o del contenedor de caché cuando se llama como la aplicación y ellos tienen su
/// nombre por defecto. En Unix no hay contenedor raíz.
fn windows_app_container(data_dir: &Path, cache_container: &Path) -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    [(data_dir, "data"), (cache_container, "cache")]
        .into_iter()
        .find_map(|(dir, name)| {
            let parent = dir.parent()?;
            (dir.file_name()? == name && parent.file_name()? == crate::APP_NAME)
                .then(|| parent.to_path_buf())
        })
}

/// Opciones de `cleanup`. El parseo se queda en el binario, que es quien ve los
/// argumentos.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    pub model: bool,
    pub voices: bool,
    pub synthetic_speech: bool,
    /// `--all`: la unión de las tres categorías más configuración, logs y el estado
    /// del daemon.
    pub all: bool,
    pub dry_run: bool,
    pub assume_yes: bool,
}

impl Options {
    /// `true` si se pidió alguna categoría. Sin ninguna, la operación devuelve
    /// `usage_error` sin borrar nada.
    pub fn any_category(&self) -> bool {
        self.model || self.voices || self.synthetic_speech || self.all
    }

    /// `true` si el modelo está en el alcance, por `--model` o por `--all`.
    pub fn wants_model(&self) -> bool {
        self.model || self.all
    }

    /// `true` si las voces están en el alcance.
    pub fn wants_voices(&self) -> bool {
        self.voices || self.all
    }

    /// `true` si el habla sintetizada está en el alcance.
    pub fn wants_speech(&self) -> bool {
        self.synthetic_speech || self.all
    }
}

/// Un destino del plan, con el tamaño que hay que listar junto a la ruta.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub category: Category,
    /// Tamaño total en disco de la ruta, recursivo si es un directorio. `0` si no
    /// se puede leer: el plan nunca falla por no poder medir.
    pub size: u64,
}

/// Un recurso compartido que la operación deja intacto a propósito.
///
/// El resumen lo llama "no se tocará": el plan tiene que **decir** que no se
/// borra, porque un usuario que ve `cleanup --all` sin lista de lo que se conserva
/// no puede saber que sus modelos de otra herramienta siguen ahí.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preserved {
    pub path: PathBuf,
    /// Motivo legible: por qué las reglas de propiedad lo excluyen.
    pub reason: &'static str,
}

/// Plan de borrado: lo que se borra y lo que se conserva, sin haber tocado nada.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionPlan {
    /// Destinos a borrar, sin duplicados y en orden estable.
    pub targets: Vec<Target>,
    /// Recursos que quedan intactos, con el motivo.
    pub preserved: Vec<Preserved>,
    /// Planificación del ejecutor del árbol de propiedad: los mismos destinos más los
    /// contenedores que quedarán vacíos y se podan.
    pub ownership: ownership::Plan,
}

impl DeletionPlan {
    /// Tamaño total de lo que el plan borraría, que es lo que resume la confirmación.
    pub fn total_bytes(&self) -> u64 {
        self.targets.iter().map(|t| t.size).sum()
    }

    /// `true` si no hay nada que borrar.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }

    /// Rutas del plan en el orden en que se borran: los destinos y, después, los
    /// contenedores que quedan vacíos y se podan.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.targets
            .iter()
            .map(|t| t.path.clone())
            .chain(self.ownership.prune.iter().cloned())
            .collect()
    }
}

/// Motivo con el que se informa un contenedor que se conserva por tener entradas que
/// no son de la aplicación.
const FOREIGN_CONTAINER_REASON: &str = "contiene entradas ajenas a la aplicación";

/// Calcula el plan de borrado. Es la **única** fuente de la lista: la usan el
/// resumen de la confirmación, `--dry-run` y la ejecución.
pub fn plan(roots: &Roots, options: &Options) -> DeletionPlan {
    let (targets, mut preserved) = select(roots, options);
    // `cleanup` nunca toca el programa ni su integración de `PATH` —eso es
    // `self uninstall`—, y decirlo es parte del plan.
    if program_dir_exists(&roots.program_dir) {
        preserved.push(Preserved {
            path: roots.program_dir.clone(),
            reason: "el programa y la integración de PATH son de `self uninstall`",
        });
    }
    finish(roots, targets, preserved, &[])
}

/// Cierra un plan a partir de los destinos ya seleccionados: planifica con el ejecutor
/// del árbol de propiedad la poda de los contenedores que quedan vacíos e informa los
/// que se conservan por entradas ajenas. `departing` son rutas que salen del disco fuera
/// de este plan y que no cuentan como contenido de un contenedor (ver
/// [`ownership::plan_with_departing`]).
pub(crate) fn finish(
    roots: &Roots,
    targets: Vec<Target>,
    mut preserved: Vec<Preserved>,
    departing: &[PathBuf],
) -> DeletionPlan {
    let removals: Vec<PathBuf> = targets.iter().map(|t| t.path.clone()).collect();
    let ownership = ownership::plan_with_departing(&roots.ownership_tree(), &removals, departing);
    for foreign in &ownership.foreign {
        preserved.push(Preserved {
            path: foreign.container.clone(),
            reason: FOREIGN_CONTAINER_REASON,
        });
    }
    preserved.sort_by_key(|p| crate::canonical_path_key(&p.path));
    DeletionPlan {
        targets,
        preserved,
        ownership,
    }
}

/// Selección de nodos de las categorías pedidas, con lo compartido que se conserva.
/// Los destinos salen sin duplicados y en orden estable.
pub(crate) fn select(roots: &Roots, options: &Options) -> (Vec<Target>, Vec<Preserved>) {
    let mut collected: Vec<Target> = Vec::new();
    let mut preserved: Vec<Preserved> = Vec::new();

    if options.wants_model() {
        collect_model(roots, &mut collected, &mut preserved);
    }
    if options.wants_voices() {
        collect_voices(roots, options, &mut collected);
    }
    if options.wants_speech() {
        push(
            &mut collected,
            roots.data_dir.join("speech"),
            Category::SyntheticSpeech,
        );
    }
    if options.all {
        // `--all` añade configuración, logs y `daemon.pid` al resto. El
        // fichero `daemon.ready` va con él porque es la otra mitad del mismo estado
        // de ejecución y dejarlo sería un residuo que `doctor` seguiría reportando.
        push(
            &mut collected,
            roots.data_dir.join("config.json"),
            Category::Config,
        );
        push(
            &mut collected,
            crate::logs_dir_in(&roots.data_dir),
            Category::Logs,
        );
        push(
            &mut collected,
            roots.data_dir.join(avi_process::registry::PID_FILE),
            Category::DaemonState,
        );
        push(
            &mut collected,
            roots.data_dir.join(avi_process::registry::READY_FILE),
            Category::DaemonState,
        );
        // La hoja de temporales cae entera con `--all`: la recuperación ya barrió a los
        // dueños muertos, y los vivos están excluidos por el bloqueo y la parada del
        // daemon. Sin esto el contenedor de caché nunca queda vacío.
        collected.extend(scratch_target(roots));
    }

    // Deduplicación por clave canónica. `BTreeMap::insert` devuelve el valor
    // **anterior**, así que `None` significa "no estaba" y es lo que se conserva: el
    // arrastre de `speech/<voz>` y la raíz `speech/` pueden describir el mismo árbol.
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    collected.retain(|t| {
        seen.insert(crate::canonical_path_key(&t.path), ())
            .is_none()
    });
    collected.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.path.cmp(&b.path))
    });
    (collected, preserved)
}

/// Destino de la hoja de temporales, si existe.
pub(crate) fn scratch_target(roots: &Roots) -> Option<Target> {
    let mut found = Vec::new();
    push(&mut found, roots.scratch_root.clone(), Category::Scratch);
    found.pop()
}

/// Alcance de `--model`.
///
/// **Hoja exclusiva**: si existe, un destino, la hoja entera. Los snapshots, los locks
/// y `xet` cuelgan de ella, así que el borrado es de directorio y no hay aritmética
/// que acertar y no queda residuo. Se borra siempre, haya o no una caché compartida
/// vigente.
///
/// **Raíz compartida vigente**: además, solo los repos fijados y sus locks. `xet` y
/// el `.locks` completo se conservan y se anuncian como compartidos.
fn collect_model(roots: &Roots, collected: &mut Vec<Target>, preserved: &mut Vec<Preserved>) {
    if roots.models_dir.is_dir() {
        push(collected, roots.models_dir.clone(), Category::Model);
    }
    let Some(models) = &roots.shared_models_dir else {
        return;
    };
    if !models.is_dir() {
        return;
    }
    for pin in avi_shared::paths::MODEL_REVISIONS {
        let repo_dir = models.join(format!("models--{}", pin.repo.replace('/', "--")));
        push(collected, repo_dir.clone(), Category::Model);
        // Los locks de cada repo propio son atribuibles a la aplicación, así que caen con su repo. El `.locks` completo no.
        push(
            collected,
            models
                .join(".locks")
                .join(format!("models--{}", pin.repo.replace('/', "--"))),
            Category::Model,
        );
    }
    // Lo que se conserva se anuncia por **regla**, no entrada por entrada: una caché
    // HF compartida puede tener cientos de repos de otras herramientas, y listarlos uno
    // a uno convertiría el resumen en ruido sin decir más que la regla. Se nombra la
    // raíz, `xet` y el `.locks` completo, que es exactamente lo que no se borra.
    preserved.push(Preserved {
        path: models.clone(),
        reason: "la raíz es una caché compartida; solo se borran los repos \
                 fijados y sus locks",
    });
    for (name, reason) in [
        (
            "xet",
            "`xet` es un subdirectorio global de la caché compartida",
        ),
        (
            ".locks",
            "el `.locks` completo nunca se borra de una raíz compartida",
        ),
    ] {
        let path = models.join(name);
        if path.exists() {
            preserved.push(Preserved { path, reason });
        }
    }
}

/// Alcance de `--voices`: las voces que el usuario puede borrar, más el arrastre de
/// su_namespace de habla.
///
/// Las voces de fábrica (`default`, `ryan`, `vivian`) van embebidas en el binario y
/// el contrato de `cleanup` las protege: `--voices` no las borra ni sus locuciones,
/// y caen solo con `--synthetic-speech` o `--all`. El arrastre de `speech/<voz>` se
/// omite cuando el habla entera ya está en el alcance, para no listar dos veces lo
/// mismo.
fn collect_voices(roots: &Roots, options: &Options, collected: &mut Vec<Target>) {
    let voice_base = roots.data_dir.join("voices");
    let entries = match std::fs::read_dir(&voice_base) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    // El namespace de habla de cada voz lo ubica el mismo almacén que usa
    // `voice remove`, de modo que los dos comandos comparten el criterio de qué
    // locuciones se van con la voz.
    let speech = avi_store::SpeechStore::at(roots.data_dir.join("speech"));
    for entry in entries.flatten() {
        if !entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_lowercase();
        if avi_store::is_factory_name(&name) {
            continue;
        }
        push(collected, entry.path(), Category::Voices);
        if !options.wants_speech() && name != "default" {
            push(collected, speech.voice_dir(&name), Category::Voices);
        }
    }
}

/// Añade `path` al plan si existe. No falla nunca: una ruta que no se puede medir se
/// añade con tamaño `0` antes que desaparecer del plan, porque un plan que omite lo
/// que no sabe medir es peor que uno que lo estima.
fn push(collected: &mut Vec<Target>, path: PathBuf, category: Category) {
    if !path.exists() && !path.is_symlink() {
        return;
    }
    let size = path_size(&path);
    collected.push(Target {
        path,
        category,
        size,
    });
}

/// Tamaño total de `path`, recursivo si es un directorio. Best-effort: un archivo que
/// no se puede abrir no detiene el plan. Un mismo archivo con varios enlaces duros
/// dentro del árbol (los blobs y snapshots de la caché de modelos en Windows) cuenta
/// una sola vez; los enlaces simbólicos no se siguen y cuentan por su propia longitud.
pub fn path_size(path: &Path) -> u64 {
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(_) => return 0,
    };
    if meta.is_file() {
        return meta.len();
    }
    if !meta.is_dir() {
        return 0;
    }
    let mut total = 0;
    let mut seen = HashSet::new();
    let mut stack = vec![path.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            match entry.metadata() {
                Ok(meta) if meta.is_dir() => stack.push(entry.path()),
                Ok(meta) if meta.is_file() => {
                    // Si no se puede abrir para identificarlo, se cuenta igualmente:
                    // sobrestimar un archivo es mejor que omitirlo del plan.
                    let first_time = match Handle::from_path(entry.path()) {
                        Ok(handle) => seen.insert(handle),
                        Err(_) => true,
                    };
                    if first_time {
                        total += meta.len();
                    }
                }
                Ok(meta) => total += meta.len(),
                Err(_) => {}
            }
        }
    }
    total
}

/// `true` si `models_dir` es la caché HF compartida que eligió el usuario.
///
/// Se decide **por identidad de ruta**, no por el valor de la variable: la hoja
/// registrada en el recibo era exclusiva en el momento de instalar, y que el usuario
/// después apunte `HF_HUB_CACHE` a otro sitio no la convierte en compartida. Es lo que
/// evita que una variable obsoleta impida borrar lo propio.
pub fn is_shared_models_root(models_dir: &Path) -> bool {
    match crate::shared_hf_root() {
        Some(shared) => crate::canonical_path_key(models_dir) == crate::canonical_path_key(&shared),
        None => false,
    }
}

/// Caché HF compartida que rige ahora, distinta de la hoja exclusiva `leaf`, o `None`
/// si no rige ninguna. La resuelve [`Roots`] una sola vez y el resto del motor la recibe
/// como dato: leer el entorno dentro del planificador haría que el plan dependiera de
/// algo que puede cambiar entre la simulación y la ejecución.
///
/// Si la variable apunta justo a la hoja exclusiva, esa ruta es de la aplicación por
/// definición y no se trata como externa.
fn current_shared_models_dir(leaf: &Path) -> anyhow::Result<Option<PathBuf>> {
    let effective = crate::models_cache_dir()?;
    let shared = is_shared_models_root(&effective)
        && crate::canonical_path_key(&effective) != crate::canonical_path_key(leaf);
    Ok(shared.then_some(effective))
}

fn program_dir_exists(program_dir: &Path) -> bool {
    program_dir.is_dir()
}

/// Desenlace de `cleanup`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// `cleanup_complete`, `planned` o `cancelled`.
    pub status: &'static str,
    /// Rutas borradas en esta ejecución, en el orden del plan.
    pub removed: Vec<String>,
    /// Recibo del plan: lo que se borraría, en el orden del plan.
    pub planned: Vec<String>,
    /// Del barrido transversal, cuando lo hubo.
    pub swept: Vec<String>,
    /// Lo que quedó sin poder borrar, con el motivo. No es un fallo: se considera
    /// un archivo en uso que recoge el borrado diferido.
    pub kept: Vec<String>,
    /// Directorios del barrido que existen y no se pudieron listar: lo que contienen
    /// queda sin recoger. En la ejecución real también están en `kept`.
    pub unreadable: Vec<String>,
    /// Recursos compartidos que se conservan, con el motivo.
    pub preserved: Vec<Preserved>,
    pub dry_run: bool,
    /// `true` si había un daemon en ejecución y lo paró esta operación.
    pub daemon_was_running: bool,
    /// Destinos del plan que fallaron al borrarse.
    pub failed: Vec<String>,
    /// Duración del barrido, la del reloj que mostró las fases. La anuncia el
    /// handler en su mensaje final; en la simulación y en la cancelación es
    /// cero, porque no hubo barrido que medir.
    pub elapsed: std::time::Duration,
}

/// Ejecuta `cleanup`.
///
/// El orden es el de las reglas de borrado: sin categoría no se borra nada; la
/// simulación imprime el plan y no toca el disco, ni siquiera el bloqueo ni el
/// barrido, porque `--dry-run` no puede dejar ni el archivo de bloqueo detrás; y en la
/// ejecución real la recuperación y el barrido van **antes** del plan, para que el
/// plan que el usuario ve y confirma sea el que queda después del barrido y no una
/// lista que el barrido va a invalidar.
pub async fn run(
    roots: &Roots,
    options: &Options,
    control: &dyn ProcessControl,
) -> anyhow::Result<Outcome> {
    run_with_progress(roots, options, control, false).await
}

/// Igual que [`run`], pero con la frontera del indicador.
///
/// `progress` dice si la CLI quiere un indicador de fase. La librería lo abre
/// **después** de la confirmación interna —la frontera está entre `plan` y el
/// barrido—, de modo que el prompt de la confirmación nunca comparte línea con
/// una fase. Si el usuario cancela, el indicador nunca se abrió y por eso la
/// librería no lo cierra; si confirma, lo cierra al terminar el barrido.
pub async fn run_with_progress(
    roots: &Roots,
    options: &Options,
    control: &dyn ProcessControl,
    progress: bool,
) -> anyhow::Result<Outcome> {
    // Sin categoría, `usage_error` sin borrar nada. Antes de
    // cualquier otra cosa, incluido el bloqueo.
    if !options.any_category() {
        return Err(LifecycleError::usage_error(
            "cleanup requiere al menos una categoría: --model, --voices, \
             --synthetic-speech o --all",
        )
        .into());
    }

    // Ninguna operación pide elevación.
    let report = crate::privileges::ensure_per_user()?;
    if let Some(warning) = &report.warning {
        eprintln!("{warning}");
    }

    if options.dry_run {
        // El simulacro también anuncia la fase: la confirmación no ocurre —el
        // simulacro no pregunta nada—, así que el indicador puede abrirse sin
        // que un prompt quede pegado a él.
        let mut indicator = progress.then(crate::progress::PhaseProgress::new);
        if let Some(indicator) = indicator.as_mut() {
            indicator.phase("cleaning", "Limpiando datos…");
            // La línea se cede antes del plan: el plan se imprime por stderr y
            // no puede quedar pegado a la fase.
            indicator.close_line();
        }
        let outcome = simulate(roots, options);
        if let Some(indicator) = indicator {
            indicator.close();
        }
        return Ok(outcome);
    }

    // Recuperación y bloqueo. El barrido transversal va aquí dentro,
    // que es lo que significa "cualquier invocación barre además…".
    let lock = crate::lock::acquire_exclusive(&roots.program_dir)?;
    let recovery = recovery::recover(roots.recovery_roots())?;

    let plan = plan(roots, options);

    // `cleanup` es destructiva: el plan con categorías y tamaños, el mismo que
    // muestra la simulación, y confirmación `[s/N]`.
    let decision = confirm(&plan, options)?;
    if decision == Decision::Cancelled {
        // Recibo del plan aunque se cancele: lo que se habría borrado.
        let planned: Vec<String> = plan
            .paths()
            .iter()
            .map(|p| p.display().to_string())
            .collect();
        return Ok(Outcome {
            status: "cancelled",
            planned,
            preserved: plan.preserved,
            ..Outcome::default()
        });
    }

    // La frontera: el indicador nace **después** de la confirmación, porque el
    // prompt no puede quedar pegado a una fase. La cancelación ya salió, así que
    // desde aquí el barrido siempre ocurre.
    let mut indicator = progress.then(crate::progress::PhaseProgress::new);
    if let Some(indicator) = indicator.as_mut() {
        indicator.phase("cleaning", "Limpiando datos…");
    }

    // Antes de borrar recursos que el daemon usa, se detiene el daemon. Si no
    // se detiene, `daemon_stop_failed` y nada del plan se borra.
    let daemon = daemon_stop::stop(&roots.data_dir, &daemon_stop::default_addr(), control).await;
    if let Err(failure) = daemon_stop::require_stopped(&daemon) {
        if let Some(indicator) = indicator.as_mut() {
            indicator.close_line();
        }
        return Err(failure.into());
    }

    let report = ownership::execute(&roots.ownership_tree(), &plan.ownership);
    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for path in &report.removed {
        if let Some(target) = plan.targets.iter().find(|t| &t.path == path) {
            // La línea se cede antes del aviso de ruta: el aviso es del producto
            // y no puede quedar pegado a la fase.
            if let Some(indicator) = indicator.as_mut() {
                indicator.close_line();
            }
            eprintln!("  {}: {}", target.category.describe(), path.display());
        }
        removed.push(path.display().to_string());
    }
    removed.extend(report.pruned.iter().map(|p| p.display().to_string()));
    for (path, e) in &report.failed {
        if let Some(indicator) = indicator.as_mut() {
            indicator.close_line();
        }
        eprintln!("  no se pudo borrar {}: {e}", path.display());
        failed.push(path.display().to_string());
    }

    drop(lock);

    // Recibo del plan: la lista completa que `plan` calculó, aunque algún
    // destino haya fallado y no esté en `removed`.
    let planned: Vec<String> = plan
        .paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    // El cierre no imprime estado: el mensaje final lo compone quien cierra.
    // La duración que devuelve es la del barrido, y la anuncia el handler.
    let elapsed = match indicator {
        Some(indicator) => indicator.close(),
        None => std::time::Duration::ZERO,
    };
    Ok(Outcome {
        status: "cleanup_complete",
        elapsed,
        removed,
        planned,
        swept: recovery
            .removed()
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        kept: recovery
            .kept
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        unreadable: recovery
            .unreadable
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        preserved: plan.preserved,
        dry_run: false,
        daemon_was_running: daemon.was_running,
        failed,
    })
}

/// Simulación: imprime el plan, incluido el barrido que se haría, y no
/// modifica el disco.
///
/// El barrido **no** se ejecuta aquí aunque la regla diga "cualquier invocación
/// barre además…": lo que no puede coexistir con una simulación es modificar el disco.
/// Lo que sí hace es anunciarlo con la misma decisión del barrido real, de modo que
/// lo que dice el `--dry-run` es lo que ocurriría.
pub fn simulate(roots: &Roots, options: &Options) -> Outcome {
    let plan = plan(roots, options);
    let preview: SweepPreview = recovery::preview(roots.recovery_roots());
    for line in plan_lines(&plan, Some(&preview)) {
        eprintln!("{line}");
    }
    eprintln!("{}", confirm::simulation_notice());
    // `removed` son las categorías y `swept` el barrido transversal, **en las dos
    // ramas**: mezclar el barrido dentro de `removed` en la simulación y no en la
    // ejecución haría que `--dry-run` y la real no se pudieran comparar, que es
    // justamente la propiedad que el planificador único garantiza.
    // `planned` es el recibo de `removed` en el simulacro, para que el envelope se
    // lea como un plan.
    let removed: Vec<String> = plan
        .paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    let planned = removed.clone();
    Outcome {
        status: "planned",
        removed,
        planned,
        swept: preview
            .all()
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        kept: preview
            .undecided()
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        unreadable: preview
            .unreadable
            .iter()
            .map(|(path, _)| path.display().to_string())
            .collect(),
        preserved: plan.preserved,
        dry_run: true,
        daemon_was_running: false,
        failed: Vec::new(),
        // La simulación no barre: no hay duración que anunciar.
        elapsed: std::time::Duration::ZERO,
    }
}

/// Aplica la confirmación con el plan ya calculado. En la ejecución el barrido
/// ya ocurrió, así que el plan no lo anuncia.
fn confirm(plan: &DeletionPlan, options: &Options) -> anyhow::Result<Decision> {
    let summary = plan_lines(plan, None);
    let stdin = std::io::stdin();
    let mut entry = stdin.lock();
    confirm::confirm(
        &Confirmation {
            kind: Kind::Destructive,
            summary: &summary,
            entries: &[],
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
        },
        &mut entry,
        &mut std::io::stderr(),
    )
}

/// Plan legible: la única composición del plan de `cleanup`, que imprimen igual la
/// simulación y la confirmación. Cada destino aparece una vez, con su categoría en
/// español y su tamaño; después, el barrido que se haría y los directorios que no
/// se pueden listar (solo en la simulación, que recibe `preview`) y lo que se
/// conserva.
pub(crate) fn plan_lines(plan: &DeletionPlan, preview: Option<&SweepPreview>) -> Vec<String> {
    let mut out = Vec::new();
    if plan.is_empty() {
        out.push("No hay nada que limpiar.".to_string());
    } else {
        out.push(format!(
            "Se borrarán {} ruta(s), {} en total:",
            plan.targets.len(),
            crate::human_bytes(plan.total_bytes())
        ));
        for target in &plan.targets {
            out.push(format!(
                "  {}: {} ({})",
                target.category.describe(),
                target.path.display(),
                crate::human_bytes(target.size)
            ));
        }
    }
    if let Some(preview) = preview {
        for path in preview.all() {
            out.push(format!("  barrido {}", path.display()));
        }
        for (path, cause) in &preview.unreadable {
            out.push(format!("  no se puede listar {}: {cause}", path.display()));
        }
    }
    for item in &plan.preserved {
        out.push(format!(
            "  no se tocará {}: {}",
            item.path.display(),
            item.reason
        ));
    }
    out
}

#[cfg(test)]
mod path_size_tests {
    use super::*;
    use crate::test_support::{scratch, write_file};

    #[test]
    fn hard_linked_file_is_counted_once() {
        let root = scratch("path-size-hardlink");
        write_file(&root.join("blobs/abc"), "0123456789");
        std::fs::create_dir_all(root.join("snapshots/rev")).expect("se crea el snapshot");
        std::fs::hard_link(root.join("blobs/abc"), root.join("snapshots/rev/model.bin"))
            .expect("se crea el enlace duro");
        write_file(&root.join("other.txt"), "12345");

        assert_eq!(
            path_size(&root),
            15,
            "el blob y su enlace duro suman una vez, más el archivo independiente"
        );
    }
}

#[cfg(test)]
mod plan_lines_tests {
    use super::*;

    const ALL: [Category; 8] = [
        Category::Model,
        Category::Voices,
        Category::SyntheticSpeech,
        Category::Config,
        Category::Logs,
        Category::DaemonState,
        Category::Scratch,
        Category::UserData,
    ];

    #[test]
    fn categories_describe_in_spanish() {
        let described: Vec<&str> = ALL.iter().map(|c| c.describe()).collect();
        assert_eq!(
            described,
            [
                "modelos",
                "voces",
                "locuciones",
                "configuración",
                "registros",
                "estado del daemon",
                "temporales",
                "datos de usuario",
            ]
        );
    }

    /// Cada destino sale una sola vez, con su categoría en español y su tamaño, y
    /// ninguna línea lleva un literal interno en inglés.
    #[test]
    fn plan_lines_list_each_path_once_with_its_category() {
        let plan = DeletionPlan {
            targets: ALL
                .iter()
                .enumerate()
                .map(|(i, category)| Target {
                    path: PathBuf::from(format!("/raiz/destino-{i}")),
                    category: *category,
                    size: 1500,
                })
                .collect(),
            preserved: vec![Preserved {
                path: PathBuf::from("/raiz/programa"),
                reason: "el programa y la integración de PATH son de `self uninstall`",
            }],
            ownership: ownership::Plan::default(),
        };
        let lines = plan_lines(&plan, None);
        assert_eq!(lines[0], "Se borrarán 8 ruta(s), 12,0 KB en total:");
        for (i, category) in ALL.iter().enumerate() {
            let path = format!("/raiz/destino-{i}");
            let shown = PathBuf::from(&path).display().to_string();
            assert_eq!(
                lines.iter().filter(|l| l.contains(&shown)).count(),
                1,
                "{lines:?}"
            );
            assert!(lines.contains(&format!("  {}: {shown} (1,5 KB)", category.describe())));
        }
        for literal in [
            "model ",
            "voices",
            "synthetic_speech",
            "config ",
            "logs",
            "daemon_state",
            "scratch",
        ] {
            assert!(
                lines.iter().all(|l| !l.contains(literal)),
                "{literal}: {lines:?}"
            );
        }
    }
}
