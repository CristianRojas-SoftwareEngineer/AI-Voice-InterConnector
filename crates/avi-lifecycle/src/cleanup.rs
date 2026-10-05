//! `cleanup`: borrado del estado por categorías, con el planificador que
//! solo actúa dentro de raíces de propiedad exclusiva.
//!
//! Lo que este módulo sustituye es la aritmética ad hoc que vivía en el binario,
//! donde el plan de `--dry-run` y la ejecución
//! eran **dos** listas: el planificaba `xet` y `.locks` sin mirar si la raíz de
//! modelos era compartida, mientras el ejecutor devolvía `Ok(false)` bajo R3. El
//! resultado era que `cleanup --model --dry-run` anunciaba un borrado que no
//! ocurría, y que ninguna prueba lo detectaba porque nadie comparaba las dos
//! listas.
//!
//! **Una sola función de cálculo.** [`plan`] construye la lista de destinos con sus
//! tamaños aplicando R1 a R3, y la usan las tres cosas que la necesitan: el resumen
//! de la confirmación, la salida de `--dry-run` y la ejecución. No puede
//! haber divergencia porque no hay dos implementaciones; `plan_and_execution_agree_under_shared_root`
//! lo afirma sobre el caso que fallaba.
//!
//! **R1 a R3, en una frase cada una.** Solo se borra dentro de raíces de propiedad
//! exclusiva. El directorio de programa no lo toca `cleanup` en absoluto —eso es
//! `self uninstall`— y aparece en el plan solo como recurso compartido que se
//! conserva. En la raíz de modelos **exclusiva** el borrado es de directorio
//! entero, con los snapshots, los locks y `xet` dentro, porque
//! `xet` cuelga de ella. En la raíz **compartida** que el
//! usuario eligió con `HF_HUB_CACHE` o `HF_HOME` solo se borran los repos fijados
//! y sus locks: nunca `xet` ni el `.locks` completo, ni un repo de
//! otra herramienta (criterio 23).
//!
//! **Nada de lo que el daemon usa se borra sin pararlo antes**, y el fallo
//! de la parada es `daemon_stop_failed` con nada del plan borrado.
//!
//! **Tras `--model` la aplicación queda reintentable**: `setup` vuelve a
//! descargar lo que falte, porque la provisión no depende de nada que sobreviva al
//! borrado.

use crate::confirm::{self, Confirmation, Decision, Kind, PlanEntry};
use crate::daemon_stop::{self, ProcessControl};
use crate::receipt::InstallReceipt;
use crate::recovery::{self, Roots as RecoveryRoots, SweepPreview};
use crate::LifecycleError;
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
}

impl Category {
    /// Literal de la categoría, el que va en el envelope y en el resumen.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Model => "model",
            Self::Voices => "voices",
            Self::SyntheticSpeech => "synthetic_speech",
            Self::Config => "config",
            Self::Logs => "logs",
            Self::DaemonState => "daemon_state",
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
    /// Raíz de modelos: snapshots, locks y `xet`.
    pub models_dir: PathBuf,
    /// Directorio de temporales del sistema, para el barrido transversal.
    pub temp_root: PathBuf,
    /// `$HOME`, que R2 usa como una de las rutas que el directorio de programa nunca
    /// puede ser. Vacío donde el producto no lo necesita.
    pub home: PathBuf,
    /// `true` si la raíz de modelos es la caché HF compartida que eligió el usuario, lo
    /// que hace que R3 limite el alcance de `--model` a lo atribuible a la aplicación.
    ///
    /// Va como **dato** y no se vuelve a leer del entorno dentro del planificador por
    /// dos razones. La primera es el aislamiento de las pruebas: leer
    /// `HF_HUB_CACHE` desde el planificador hace
    /// que dos pruebas que corren en paralelo se contaminen, porque el entorno del
    /// proceso es global. La segunda es que el plan tiene que ser una función de sus
    /// entradas: si el plan dependiera del entorno, `--dry-run` podría anunciar un
    /// borrado distinto del que la ejecución hace un instante después, que es
    /// exactamente el defecto que este planificador absorbe.
    pub models_shared: bool,
}

impl Roots {
    /// Raíces del producto resueltas ahora, que es lo que usa el binario.
    pub fn resolve() -> Self {
        let models_dir = crate::models_cache_dir();
        Self {
            program_dir: crate::install_dir(),
            data_dir: crate::data_dir(),
            models_shared: is_shared_models_root(&models_dir),
            models_dir,
            temp_root: std::env::temp_dir(),
            home: std::env::var("HOME")
                .or_else(|_| std::env::var("USERPROFILE"))
                .map(PathBuf::from)
                .unwrap_or_default(),
        }
    }

    /// Raíces efectivas de la instalación registrada, con el recibo como fuente de
    /// verdad de los datos y de los modelos: los valores efectivos se registran en el
    /// recibo, así que la actualización y la desinstalación operan sobre las mismas
    /// ubicaciones aunque la variable ya no esté definida.
    pub fn from_receipt(receipt: Option<&InstallReceipt>) -> Self {
        let mut roots = Self::resolve();
        if let Some(receipt) = receipt {
            roots.program_dir = receipt.install_dir.clone();
            roots.data_dir = receipt.roots.data_dir.clone();
            roots.models_dir = receipt.roots.cache_dir.clone();
        }
        roots
    }

    /// `RecoveryRoots` con el staging en uso a `None`: `cleanup` no instala nada, así
    /// que todo staging hermano es huérfano por definición.
    pub fn recovery_roots(&self) -> RecoveryRoots<'_> {
        RecoveryRoots {
            program_dir: &self.program_dir,
            temp_root: &self.temp_root,
            in_use: None,
        }
    }

    /// Archivo de bloqueo, hermano del directorio de programa. Es un dato y no
    /// la constante de `lock::lock_path()` porque el directorio de programa puede ser
    /// el **registrado** y no el de la convención de rutas.
    pub fn lock_path(&self) -> PathBuf {
        crate::lock::lock_path_for(&self.program_dir)
    }
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
/// El resumen lo llama "no se tocará" y R3 lo exige: el plan tiene que **decir** que no se
/// borra, porque un usuario que ve `cleanup --all` sin lista de lo que se conserva
/// no puede saber que sus modelos de otra herramienta siguen ahí.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preserved {
    pub path: PathBuf,
    /// Motivo legible: por qué R1 o R3 lo excluyen.
    pub reason: &'static str,
}

/// Plan de borrado: lo que se borra y lo que se conserva, sin haber tocado nada.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletionPlan {
    /// Destinos a borrar, sin duplicados y en orden estable.
    pub targets: Vec<Target>,
    /// Recursos que quedan intactos, con el motivo.
    pub preserved: Vec<Preserved>,
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

    /// Rutas del plan en el orden en que se borran.
    pub fn paths(&self) -> Vec<PathBuf> {
        self.targets.iter().map(|t| t.path.clone()).collect()
    }
}

/// Calcula el plan de borrado. Es la **única** fuente de la lista: la usan el
/// resumen de la confirmación, `--dry-run` y la ejecución.
pub fn plan(roots: &Roots, options: &Options) -> DeletionPlan {
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
        // fichero `daemon.ready` va con él porque es el otro mitad del mismo estado
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
            roots.data_dir.join(crate::daemon_stop::PID_FILE),
            Category::DaemonState,
        );
        push(
            &mut collected,
            roots.data_dir.join(crate::daemon_stop::READY_FILE),
            Category::DaemonState,
        );
    }

    // `cleanup` nunca toca el programa ni su integración de `PATH` —eso es
    // `self uninstall`—, y decirlo es parte del plan.
    if program_dir_exists(&roots.program_dir) {
        preserved.push(Preserved {
            path: roots.program_dir.clone(),
            reason: "el programa y la integración de PATH son de `self uninstall`",
        });
    }

    // Deduplicación por clave canónica. `BTreeMap::insert` devuelve el valor
    // **anterior**, así que `None` significa "no estaba" y es lo que se conserva: el
    // arrastre de `speech/<voz>` y la raíz `speech/` pueden describir el mismo árbol.
    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    collected.retain(|t| {
        seen.insert(crate::canonical_path_key(&t.path), ())
            .is_none()
    });
    preserved.sort_by_key(|p| crate::canonical_path_key(&p.path));
    collected.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.path.cmp(&b.path))
    });

    DeletionPlan {
        targets: collected,
        preserved,
    }
}

/// Alcance de `--model`.
///
/// **Raíz exclusiva**: un único destino, la raíz entera. Los snapshots, los locks y
/// `xet` cuelgan de ella, así que el borrado es de directorio y no hay aritmética
/// que acertar. Es el residuo cero del criterio 17.
///
/// **Raíz compartida**: solo los repos fijados y sus locks. `xet` y
/// el `.locks` completo se conservan y se anuncian como compartidos (R3).
fn collect_model(roots: &Roots, collected: &mut Vec<Target>, preserved: &mut Vec<Preserved>) {
    let models = &roots.models_dir;
    if !models.is_dir() {
        return;
    }
    if !roots.models_shared {
        push(collected, models.clone(), Category::Model);
        return;
    }
    for pin in avi_store::MODEL_REVISIONS {
        let repo_dir = models.join(format!("models--{}", pin.repo.replace('/', "--")));
        push(collected, repo_dir.clone(), Category::Model);
        // Los locks de cada repo propio son atribuibles a la aplicación (R3 los
        // nombra explícitamente), así que caen con su repo. El `.locks` completo no.
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
    // raíz, `xet` y el `.locks` completo, que es exactamente lo que R3 excluye.
    preserved.push(Preserved {
        path: models.clone(),
        reason: "R3: la raíz es una caché compartida; solo se borran los repos \
                 fijados y sus locks",
    });
    preserved.push(Preserved {
        path: models.join("xet"),
        reason: "R3: `xet` es un subdirectorio global de la caché compartida",
    });
    preserved.push(Preserved {
        path: models.join(".locks"),
        reason: "R3: el `.locks` completo nunca se borra de una raíz compartida",
    });
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
            push(
                collected,
                roots.data_dir.join("speech").join(&name),
                Category::Voices,
            );
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

/// `true` si la raíz de modelos es la caché HF compartida que eligió el usuario.
///
/// Se decide **por identidad de ruta**, no por el valor de la variable: la raíz
/// registrada en el recibo era exclusiva en el momento de instalar, y que el usuario
/// después apunte `HF_HUB_CACHE` a otro sitio no convierte el registro
/// registrado en compartido. Es lo que evita que un recibo viejo autorice borrar lo
/// ajeno o, al revés, que una variable obsoleta impida borrar lo propio.
///
/// La resuelve [`Roots::resolve`] una sola vez y el resto del motor la recibe como dato:
/// leer el entorno dentro del planificador haría que el plan dependiera de algo que
/// puede cambiar entre la simulación y la ejecución.
pub fn is_shared_models_root(models_dir: &Path) -> bool {
    match avi_store::shared_hf_root() {
        Some(shared) => crate::canonical_path_key(models_dir) == crate::canonical_path_key(&shared),
        None => false,
    }
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
    /// Recursos compartidos que se conservan, con el motivo.
    pub preserved: Vec<Preserved>,
    pub dry_run: bool,
    /// `true` si había un daemon en ejecución y lo paró esta operación.
    pub daemon_was_running: bool,
    /// Destinos del plan que fallaron al borrarse.
    pub failed: Vec<String>,
}

impl Outcome {
    /// `true` si la operación se ejecutó y no dejó destinos sin borrar.
    pub fn is_complete(&self) -> bool {
        self.status == "cleanup_complete" && self.failed.is_empty()
    }
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
        return Ok(simulate(roots, options));
    }

    // Recuperación y bloqueo. El barrido transversal va aquí dentro,
    // que es lo que significa "cualquier invocación barre además…".
    let lock_path = roots.lock_path();
    let lock = crate::lock::acquire_at(&lock_path)?;
    let recovery = recovery::recover(roots.recovery_roots())?;

    let plan = plan(roots, options);

    // `cleanup` es destructiva. Lista con tamaños y confirmación `[s/N]`.
    let entries: Vec<PlanEntry> = plan
        .targets
        .iter()
        .map(|t| PlanEntry {
            path: t.path.clone(),
            size: Some(t.size),
        })
        .collect();
    let decision = confirm(&plan, &entries, options)?;
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

    // Antes de borrar recursos que el daemon usa, se detiene el daemon. Si no
    // se detiene, `daemon_stop_failed` y nada del plan se borra.
    let daemon = daemon_stop::stop(&roots.data_dir, &daemon_stop::default_addr(), control).await;
    daemon_stop::require_stopped(&daemon)?;

    let mut removed = Vec::new();
    let mut failed = Vec::new();
    for target in &plan.targets {
        match remove_path(&target.path) {
            Ok(()) => {
                eprintln!("  {} {}", target.category.as_str(), target.path.display());
                removed.push(target.path.display().to_string());
            }
            Err(e) => {
                eprintln!("  no se pudo borrar {}: {e}", target.path.display());
                failed.push(target.path.display().to_string());
            }
        }
    }

    drop(lock);

    // Recibo del plan: la lista completa que `plan` calculó, aunque algún
    // destino haya fallado y no esté en `removed`.
    let planned: Vec<String> = plan
        .paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    Ok(Outcome {
        status: "cleanup_complete",
        removed,
        planned,
        swept: recovery
            .removed_parked
            .iter()
            .chain(recovery.removed_stagings.iter())
            .chain(recovery.removed_temporaries.iter())
            .map(|p| p.display().to_string())
            .collect(),
        kept: recovery
            .kept
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
    eprintln!(
        "Simulación: se muestra lo que {} borraría, no se ha modificado nada.",
        crate::APP_NAME
    );
    for target in &plan.targets {
        eprintln!(
            "  {} {} ({})",
            target.category.as_str(),
            target.path.display(),
            crate::human_bytes(target.size)
        );
    }
    for path in preview.all() {
        eprintln!("  barrido {}", path.display());
    }
    for item in &plan.preserved {
        eprintln!("  no se tocará {}: {}", item.path.display(), item.reason);
    }
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
            .temporaries_kept
            .iter()
            .map(|p| p.display().to_string())
            .collect(),
        preserved: plan.preserved,
        dry_run: true,
        daemon_was_running: false,
        failed: Vec::new(),
    }
}

/// Aplica la confirmación con el plan ya calculado.
fn confirm(
    plan: &DeletionPlan,
    entries: &[PlanEntry],
    options: &Options,
) -> anyhow::Result<Decision> {
    let summary = summary(plan);
    let stdin = std::io::stdin();
    let mut entry = stdin.lock();
    confirm::confirm(
        &Confirmation {
            kind: Kind::Destructive,
            summary: &summary,
            entries,
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
        },
        &mut entry,
        &mut std::io::stderr(),
    )
}

/// Resumen legible del plan, que es la lista de rutas con tamaños que ve el usuario.
fn summary(plan: &DeletionPlan) -> Vec<String> {
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
                "  {} {} ({})",
                target.category.as_str(),
                target.path.display(),
                crate::human_bytes(target.size)
            ));
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

/// Borra una ruta del plan, sea archivo o directorio.
///
/// `NotFound` **es** éxito: el plan dice lo que tiene que dejar de existir, y si ya no
/// existe el objetivo está cumplido. No es un caso teórico —la parada del daemon borra
/// el pidfile cuando no había daemon vivo, y el pidfile es un destino de `--all`, así
/// que sin este tratamiento el mismo plan se anunciaría como fallido en una máquina limpia.
fn remove_path(path: &Path) -> std::io::Result<()> {
    let outcome = if path.is_dir() && !path.is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match outcome {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
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
        let _ = std::fs::remove_dir_all(&root);
    }
}
