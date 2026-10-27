// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `setup` en el motor: provisión de modelos, que `self install` y `self update`
//! ejecutan también tras escribir el recibo.
//!
//! Reúne la selección de modelos, la idempotencia por presencia del snapshot y
//! la purga de `--force-update`.
//!
//! **La selección es siempre la tabla de pines completa**: todos los modelos,
//! incluido el Base de clonado, forman parte de la instalación. No hay modelos
//! opcionales, así que no existe una selección persistida que recordar.
//!
//! **Lo que `self update` necesita, y dónde.** La **poda de revisiones
//! obsoletas** vive en este módulo: la necesita una actualización.
//!
//! **Dónde queda `HF_XET_CACHE`.** Toda la provisión pasa por
//! `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos
//! exclusiva antes de construir el cliente (`avi-store`). No
//! hay nada que hacer aquí para cumplirlo, y no se toca: si se fijara también desde
//! el motor, habría dos sitios decidiendo lo mismo.
//!
//! **La purga de `--force-update` pasa por el plan de borrado de modelos**, no por
//! las purgas ad hoc del binario. `setup` y
//! `cleanup --model` obedecen las mismas reglas de propiedad —nunca se borra en una
//! raíz compartida más que lo atribuible a la aplicación— y la misma confirmación
//! destructiva.

use crate::progress::{ProvisionStats, ProvisionView};
use crate::LifecycleError;
use avi_store::TransferObserver;
use std::path::Path;
use std::sync::Arc;

/// Opciones de `setup` que el motor necesita conocer. El resto de la superficie
/// (`--json`) se queda en el binario, que es quien parsea la CLI.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Options {
    /// `--force-update`: purga los modelos seleccionados y los vuelve a
    /// provisionar. Es una operación destructiva y por eso pide su propia
    /// confirmación.
    pub force_update: bool,
    /// `--yes`: omite la confirmación de la purga y la del tamaño pendiente.
    pub assume_yes: bool,
    /// La invocó `self install` o `self update` **después de su propio resumen**:
    /// La regla dice que entonces no vuelve a preguntar por el tamaño pendiente.
    pub called_from_lifecycle: bool,
}

impl Options {
    /// Opciones de una invocación directa de `setup` por el usuario.
    pub fn user(force_update: bool, assume_yes: bool) -> Self {
        Self {
            force_update,
            assume_yes,
            called_from_lifecycle: false,
        }
    }
}

/// Plan de lo que falta provisionar. Alimenta el resumen previo y la
/// confirmación de tamaño; la ejecución no se decide con él, porque [`provision`]
/// mira el estado real del almacén tras cada descarga.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pending {
    /// Repos de los que hay que descargar, con el nombre corto que usa el producto.
    pub models: Vec<String>,
}

impl Pending {
    /// `true` si no hay nada que hacer, que es la condición de idempotencia
    /// y la que hace que `setup` no descargue nada cuando todo está provisionado.
    pub fn is_empty(&self) -> bool {
        self.models.is_empty()
    }

    /// Tamaño estimado de la descarga pendiente, en bytes: la suma de
    /// `approx_bytes` de los modelos pendientes, medida en la revisión fijada de
    /// cada uno. Es una estimación declarada, no una cifra contable.
    pub fn estimated_bytes(&self) -> u64 {
        self.models
            .iter()
            .filter_map(|name| {
                avi_shared::paths::MODEL_REVISIONS
                    .iter()
                    .find(|pin| pin.name == name)
            })
            .map(|pin| pin.approx_bytes)
            .sum()
    }
}

/// Repos de la selección: todos los pines de la tabla, sin excepción. Es la lista
/// que comparten `setup`, la provisión de `self install` y `doctor`.
pub fn selection() -> Vec<&'static str> {
    avi_shared::paths::MODEL_REVISIONS
        .iter()
        .map(|pin| pin.name)
        .collect()
}

/// Calcula lo pendiente contra un almacén ya construido. El almacén se pasa
/// inyectado para que la función sea comprobable con un directorio de pruebas y no
/// necesite red.
pub fn pending(store: &avi_store::ModelStore) -> Pending {
    let mut out = Pending::default();
    for name in selection() {
        if !store.is_provisioned(name) {
            out.models.push(name.to_string());
        }
    }
    out
}

/// Ejecuta la purga de `--force-update` por el **plan de borrado de modelos**, de
/// modo que la operación obedezca las mismas reglas de propiedad que
/// `cleanup --model`.
///
/// Devuelve lo que se borró de verdad. Un repo ausente no se reporta: `cleanup`
/// tampoco lo hace, y distinguirlos convertiría un `--force-update` normal en una
/// lista de errores.
pub fn purge(store: &avi_store::ModelStore) -> PurgeOutcome {
    let mut outcome = PurgeOutcome::default();
    for name in selection() {
        match store.remove_hf_snapshot(name) {
            Ok(true) => outcome.snapshots.push(name.to_string()),
            Ok(false) => {}
            Err(e) => outcome.failures.push((name.to_string(), e.to_string())),
        }
    }
    // `xet` y los locks solo se tocan en la raíz exclusiva, que es exactamente lo
    // que `remove_xet_cache` y `remove_hf_locks` ya garantizan. La purga
    // no reimplementa esa regla: la usa.
    match avi_store::ModelStore::remove_xet_cache() {
        Ok(true) => outcome.xet = true,
        Ok(false) => {}
        Err(e) => outcome.failures.push(("xet".to_string(), e.to_string())),
    }
    if let Err(e) = avi_store::ModelStore::remove_hf_locks() {
        outcome.failures.push((".locks".to_string(), e.to_string()));
    }
    outcome
}

/// Qué borró la purga.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeOutcome {
    /// Repos cuyos snapshots se borraron de verdad.
    pub snapshots: Vec<String>,
    /// La caché `xet` se borró. `false` en raíz compartida, donde está prohibido.
    pub xet: bool,
    /// Lo que no se pudo borrar, con su motivo.
    pub failures: Vec<(String, String)>,
}

impl PurgeOutcome {
    /// `true` si no hubo ningún fallo. Un `--force-update` con fallos no es un
    /// error de la operación —el resto se provisiona igual— pero sí se informa.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Qué podó la poda de revisiones obsoletas.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PruneOutcome {
    /// Revisiones obsoletas borradas, como `nombre/snapshots/<hash>`.
    pub removed: Vec<String>,
    /// Lo que no se pudo borrar, con su motivo.
    pub failures: Vec<(String, String)>,
}

impl PruneOutcome {
    /// `true` si no hubo ningún fallo. Una poda con fallos no es un error de
    /// la operación —lo provisionado queda igual— pero sí se informa.
    pub fn is_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Poda tras éxito: elimina las revisiones de los repos
/// propios que están fuera del pin vigente de `MODEL_REVISIONS`.
///
/// Solo toca directorios de snapshots de repos propios, que son atribuibles a
/// la aplicación: procede tanto en la raíz exclusiva como en la compartida
/// y nunca toca `xet` ni `.locks`. Idempotente: una
/// segunda pasada no encuentra nada que borrar.
///
/// La confirmación de tamaño y `called_from_lifecycle` siguen vigentes: la
/// poda no pregunta por su cuenta, corre tras un éxito ya confirmado.
///
/// Falla si la raíz de modelos no se puede resolver.
pub fn prune_obsolete() -> std::io::Result<PruneOutcome> {
    Ok(prune_obsolete_at(&crate::models_cache_dir()?))
}

/// Núcleo comprobable de [`prune_obsolete`], con la raíz de modelos como dato.
pub fn prune_obsolete_at(models_root: &Path) -> PruneOutcome {
    let mut outcome = PruneOutcome::default();
    for pin in avi_shared::paths::MODEL_REVISIONS {
        let snapshots = models_root
            .join(format!("models--{}", pin.repo.replace('/', "--")))
            .join("snapshots");
        let live = live_snapshot_name(&snapshots, pin.revision);
        let entries = match std::fs::read_dir(&snapshots) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let child = entry.file_name().to_string_lossy().to_string();
            if live.as_deref() == Some(child.as_str()) {
                continue;
            }
            let label = format!("{}/snapshots/{child}", pin.name);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => outcome.removed.push(label),
                Err(e) => outcome.failures.push((label, e.to_string())),
            }
        }
    }
    outcome.removed.sort();
    outcome
}

/// Nombre del snapshot vivo de un repo: la revisión pinneada si está
/// materializada, y si no la que resuelve el `refs/` (layout estándar de HF
/// hub, el mismo que `ModelStore::model_snapshot_path` replica). `None` si el
/// pin no está en disco: entonces toda revisión presente es obsoleta.
fn live_snapshot_name(snapshots: &Path, revision: &str) -> Option<String> {
    if snapshots.join(revision).is_dir() {
        return Some(revision.to_string());
    }
    let hash = std::fs::read_to_string(snapshots.parent()?.join("refs").join(revision)).ok()?;
    let hash = hash.trim();
    if !hash.is_empty() && snapshots.join(hash).is_dir() {
        return Some(hash.to_string());
    }
    None
}

/// Traduce un fallo de provisión al `reason` de contrato que corresponde.
///
/// `setup_failed` es el que el resumen de `self install` emite ("Programa
/// instalado, pero la provisión de modelos falló"), y `network_error` el que la
/// tabla declara para un fallo de descarga tras reintentos. La traducción vive aquí
/// para que `self install` no tenga que distinguir el origen del fallo: lo que le
/// importa es que el programa queda instalado y basta reintentar con `setup`.
pub fn map_download_failure(model: &str, cause: &anyhow::Error) -> LifecycleError {
    LifecycleError::new("network_error", format!("{model}: {cause}"))
}

/// Costura de la provisión: la descarga (red) se inyecta para poder comprobar la
/// regla sin ella.
#[allow(async_fn_in_trait)]
pub trait Provisioner {
    /// Descarga el modelo `name` al almacén y le cuenta el progreso a `observer`.
    async fn download(&self, name: &str, observer: Arc<dyn TransferObserver>)
        -> anyhow::Result<()>;
}

/// Implementación de producción: `ModelStore::ensure_downloaded`.
pub struct HubProvisioner;

impl Provisioner for HubProvisioner {
    async fn download(
        &self,
        name: &str,
        observer: Arc<dyn TransferObserver>,
    ) -> anyhow::Result<()> {
        avi_store::ModelStore::ensure_downloaded(name, observer)
            .await
            .map(|_| ())
    }
}

/// Lo que hizo [`provision`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Provisioned {
    /// Repos de la selección, en el orden en que se procesaron.
    pub provisioned: Vec<String>,
    /// Repos que hubo que descargar en esta pasada.
    pub downloaded: Vec<String>,
    /// Bytes y duración de lo descargado; `None` si no hubo nada que descargar.
    pub stats: Option<ProvisionStats>,
}

/// Fallo de [`provision`], sin traducir todavía al `reason` de contrato: cada
/// llamador conserva su propio código de error.
#[derive(Debug)]
pub enum ProvisionError {
    /// Falló la descarga de `name`.
    Download { name: String, cause: anyhow::Error },
}

/// Provisión compartida por `setup` y `self install`: descarga la selección no
/// provisionada. No confirma nada ni escribe la selección.
///
/// Es el único punto que conoce la selección pendiente, así que es quien abre
/// la vista de progreso: solo si hay algo que descargar, numerando `[i/N]` sobre
/// los pendientes, y devuelve el total descargado y su duración.
pub async fn provision(
    store: &avi_store::ModelStore,
    provisioner: &impl Provisioner,
) -> Result<Provisioned, ProvisionError> {
    let names = selection();
    let pending: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !store.is_provisioned(name))
        .collect();
    let mut out = Provisioned {
        provisioned: names.iter().map(|name| name.to_string()).collect(),
        ..Provisioned::default()
    };
    if pending.is_empty() {
        return Ok(out);
    }

    let view = ProvisionView::start(pending.len());
    for (index, name) in pending.iter().enumerate() {
        let bar = Arc::new(view.model(index + 1, name));
        provisioner
            .download(name, Arc::clone(&bar) as Arc<dyn TransferObserver>)
            .await
            .map_err(|cause| ProvisionError::Download {
                name: name.to_string(),
                cause,
            })?;
        bar.finish();
        out.downloaded.push(name.to_string());
    }
    out.stats = Some(view.finish());
    Ok(out)
}

/// Desenlace de `setup`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Qué borró la purga de `--force-update`. Vacío si no se pidió.
    pub purge: PurgeOutcome,
    /// Revisiones obsoletas podadas tras el éxito. Vacío si no
    /// había ninguna fuera del pin vigente.
    pub pruned: PruneOutcome,
    /// Repos de la selección, en el orden en que se procesaron. Es lo que el resumen
    /// publica como `models_provisioned`, y no distingue los que se descargaron de los
    /// que ya estaban: el contrato solo promete el conjunto disponible.
    pub provisioned: Vec<String>,
}

/// Ejecuta `setup`: purga si toca y provisiona la selección.
///
/// Es el punto de entrada que el binario cablea. La descarga vive en
/// [`provision`], compartida con `self install`; `run` añade la purga, la
/// confirmación y la poda. Lo único que se
/// queda en el binario es el envelope `--json` y la prosa, porque el parseo de la CLI y
/// el emisor no viven en este crate.
///
/// La confirmación destructiva de `--force-update` y la del tamaño pendiente se
/// aplican aquí, con el mismo módulo `confirm` que usan `cleanup` y
/// `self uninstall`, para que las tres operaciones destructivas del producto tengan una
/// sola implementación de la tabla de confirmaciones.
pub async fn run(store: &avi_store::ModelStore, options: &Options) -> anyhow::Result<Outcome> {
    let mut outcome = Outcome::default();

    // 1. `--force-update`: purga de la selección.
    if options.force_update {
        if !confirm_destructive(options)? {
            return Ok(outcome);
        }
        outcome.purge = purge(store);
        for (name, reason) in &outcome.purge.failures {
            eprintln!("  no se pudo purgar {name}: {reason}");
        }
    }

    // 2. Resumen previo y confirmación con el plan de lo pendiente; después,
    //    la provisión compartida (descarga según el estado real).
    let pending = pending(store);
    if (options.force_update || !pending.is_empty()) && !confirm_size(&pending, options)? {
        return Ok(outcome);
    }
    let provisioned = provision(store, &HubProvisioner)
        .await
        .map_err(|e| match e {
            ProvisionError::Download { name, cause } => {
                anyhow::Error::from(map_download_failure(&name, &cause))
            }
        })?;
    outcome.provisioned = provisioned.provisioned;

    // 4. Poda tras éxito: las revisiones fuera del pin vigente.
    //    Un fallo aquí no invalida lo provisionado: se informa y se sigue.
    outcome.pruned = prune_obsolete()?;
    for (name, reason) in &outcome.pruned.failures {
        eprintln!("  no se pudo podar {name}: {reason}");
    }

    Ok(outcome)
}

/// Confirmación destructiva de `--force-update`. `false` es "el usuario dijo
/// que no", que la tabla de `reason` no cuenta como error.
fn confirm_destructive(options: &Options) -> anyhow::Result<bool> {
    let summary =
        vec!["Se purgarán los modelos descargados y se volverán a descargar.".to_string()];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::Destructive,
            summary: &summary,
            entries: &[],
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        },
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )?;
    Ok(decision != crate::confirm::Decision::Cancelled)
}

/// Línea de la confirmación: cuántos modelos se descargan y su tamaño estimado.
fn download_summary(pending: &Pending) -> String {
    format!(
        "Se descargarán {} modelo(s), unos {}.",
        pending.models.len(),
        crate::human_bytes(pending.estimated_bytes())
    )
}

/// Confirmación del tamaño pendiente de la provisión: es no destructiva, así que sin
/// terminal
/// procede, y `--yes` la omite. Cuando la invoca `self install` después de su propio
/// resumen, `called_from_lifecycle` la omite también.
fn confirm_size(pending: &Pending, options: &Options) -> anyhow::Result<bool> {
    if options.called_from_lifecycle {
        return Ok(true);
    }
    if pending.estimated_bytes() == 0 {
        return Ok(true);
    }
    let summary = vec![download_summary(pending)];
    let decision = crate::confirm::confirm(
        &crate::confirm::Confirmation {
            kind: crate::confirm::Kind::NonDestructive,
            summary: &summary,
            entries: &[],
            assume_yes: options.assume_yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&std::io::stdin()),
        },
        &mut std::io::stdin().lock(),
        &mut std::io::stderr(),
    )?;
    Ok(decision != crate::confirm::Decision::Cancelled)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file, ENV_LOCK};
    use avi_shared::paths::{CLONING_MODEL, MODEL_FILE_PATTERNS, MODEL_REVISIONS};
    use std::path::PathBuf;
    use std::sync::MutexGuard;

    /// Provisionador simulado: la descarga escribe el snapshot pinneado, sin red,
    /// y le cuenta al observador un total y un completado iguales a lo escrito.
    pub(crate) struct FakeProvisioner;

    impl Provisioner for FakeProvisioner {
        async fn download(
            &self,
            name: &str,
            observer: Arc<dyn TransferObserver>,
        ) -> anyhow::Result<()> {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .ok_or_else(|| anyhow::anyhow!("sin pin: {name}"))?;
            let dir = fake_snapshot(&repo.replace('/', "--"), revision);
            let written = crate::test_support::snapshot(&dir)
                .iter()
                .map(|(_, size)| size)
                .sum();
            observer.update(written, written);
            Ok(())
        }
    }

    /// La provisión devuelve el tamaño y la duración de lo descargado, y no
    /// abre la vista ni devuelve estadísticas cuando todo estaba provisionado.
    #[test]
    fn provision_reports_stats_only_when_it_downloads() {
        let (_guard, _root) = cache_relocated("setup-provision-stats");
        let store = avi_store::ModelStore::new().unwrap();
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime de la prueba");

        let first = runtime
            .block_on(provision(&store, &FakeProvisioner))
            .expect("la provisión simulada se completa");
        assert_eq!(first.downloaded.len(), selection().len());
        let stats = first.stats.expect("hubo descarga, hay estadísticas");
        assert!(stats.bytes > 0, "{stats:?}");

        let second = runtime
            .block_on(provision(&store, &FakeProvisioner))
            .expect("la segunda pasada se completa");
        assert!(second.downloaded.is_empty());
        assert_eq!(second.provisioned.len(), selection().len());
        assert_eq!(second.stats, None);
    }

    /// Fija `AVI_CACHE_DIR` a un directorio de pruebas y devuelve el guard que lo
    /// restaura. `avi-store` resuelve la raíz de modelos por esa variable, así que
    /// sin ella la prueba tocaría la caché de quien la ejecuta.
    pub(crate) fn cache_relocated(
        tag: &str,
    ) -> (MutexGuard<'static, ()>, crate::test_support::Scratch) {
        let guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = scratch(tag);
        std::env::set_var("AVI_CACHE_DIR", &*dir);
        (guard, dir)
    }

    /// Crea un snapshot falso del repo `repo` en la revisión fijada.
    ///
    /// Lleva `config.json`, que es lo que `is_provisioned` exige para un repo sin
    /// `MODEL_FILE_PATTERNS` —"algún fichero con tamaño > 0"—, y los ficheros que el
    /// gate de parakeet exige. Deliberadamente **no** se crea
    /// `model.safetensors` ni `pytorch_model.bin`.
    pub(crate) fn fake_snapshot(repo: &str, revision: &str) -> PathBuf {
        let dir = crate::models_cache_dir()
            .unwrap()
            .join(format!("models--{repo}"))
            .join("snapshots")
            .join(revision);
        write_file(&dir.join("config.json"), "{}");
        write_file(&dir.join("model.txt"), "pesos");
        // Se crean los patrones de todos los modelos aunque cada uno solo aplique a su
        // repo: las pruebas crean snapshots de un repo cada vez.
        for (_, patterns) in MODEL_FILE_PATTERNS {
            for pattern in *patterns {
                write_file(&dir.join(pattern), "artefacto");
            }
        }
        dir
    }

    /// La confirmación anuncia la suma de los tamaños fijados en la tabla de
    /// pines, en escala decimal: 5 865 044 386 bytes los 6 modelos de la
    /// selección, el Base de clonado y DPDFNet incluidos.
    #[test]
    fn confirmation_announces_the_pinned_sizes() {
        let all = Pending {
            models: selection().into_iter().map(String::from).collect(),
        };
        assert_eq!(all.estimated_bytes(), 5_865_044_386);
        assert_eq!(
            download_summary(&all),
            "Se descargarán 6 modelo(s), unos 5,9 GB."
        );
    }

    /// La selección es la tabla de pines completa: el modelo Base de clonado
    /// siempre forma parte de ella.
    #[test]
    fn selection_is_every_pin_including_the_cloning_model() {
        let names = selection();
        assert_eq!(names.len(), MODEL_REVISIONS.len());
        assert!(names.contains(&CLONING_MODEL));
    }

    /// La provisión no hace trabajo cuando todo está ya provisionado:
    /// `pending` queda vacío y no se toca el disco. Es la idempotencia de `setup`,
    /// que es lo que permite que `self install` lo invoque sin preguntar.
    #[test]
    fn provisioning_is_idempotent() {
        let (_guard, _root) = cache_relocated("setup-idempotent");
        let store = avi_store::ModelStore::new().unwrap();

        // 1. Sin nada provisionado, todo está pendiente.
        let initial = pending(&store);
        assert_eq!(initial.models.len(), selection().len());
        assert!(!initial.is_empty(), "nada provisionado, todo pendiente");
        assert!(initial.estimated_bytes() > 0, "hay tamaño que anunciar");

        // 2. Se provisiona todo lo de la selección: `pending` queda vacío.
        for name in selection() {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .expect("todo repo pinneado tiene revisión");
            fake_snapshot(&repo.replace('/', "--"), revision);
        }
        let after = pending(&store);
        assert!(
            after.is_empty(),
            "con todo provisionado no queda nada pendiente: {after:?}"
        );
        assert_eq!(after.estimated_bytes(), 0, "y no hay tamaño que anunciar");

        // 3. Volver a calcular no cambia nada: la función es un inspeccionador.
        assert_eq!(pending(&store), after);
    }

    /// Sin el modelo Base provisionado, `pending` lo reporta: no es opcional.
    #[test]
    fn missing_cloning_model_is_pending() {
        let (_guard, _root) = cache_relocated("setup-cloning-pending");
        for pin in MODEL_REVISIONS {
            if pin.name != CLONING_MODEL {
                fake_snapshot(&pin.repo.replace('/', "--"), pin.revision);
            }
        }
        let store = avi_store::ModelStore::new().unwrap();
        assert_eq!(pending(&store).models, vec![CLONING_MODEL.to_string()]);
    }

    /// Con todos los pines de la tabla provisionados no queda nada pendiente: los
    /// modelos de traducción se publican ya convertidos, así que ningún derivado
    /// local falta por producir.
    #[test]
    fn nothing_is_pending_when_every_pin_is_provisioned() {
        let (_guard, _root) = cache_relocated("setup-all-pinned");
        for pin in MODEL_REVISIONS {
            fake_snapshot(&pin.repo.replace('/', "--"), pin.revision);
        }
        let store = avi_store::ModelStore::new().unwrap();
        let plan = pending(&store);
        assert!(
            plan.is_empty(),
            "con todos los pines sembrados no queda nada: {plan:?}"
        );
    }

    /// La purga de `--force-update` borra la selección entera, el modelo Base de
    /// clonado incluido.
    #[test]
    fn force_update_purges_the_whole_selection() {
        let (_guard, _root) = cache_relocated("setup-purge");
        let store = avi_store::ModelStore::new().unwrap();

        fake_snapshot(
            "Qwen--Qwen3-TTS-12Hz-0.6B-Base",
            "5d83992436eae1d760afd27aff78a71d676296fc",
        );
        let clone_dir = crate::models_cache_dir()
            .unwrap()
            .join("models--Qwen--Qwen3-TTS-12Hz-0.6B-Base");
        assert!(clone_dir.exists(), "el snapshot del modelo de clonado está");

        let outcome = purge(&store);
        assert!(
            outcome.snapshots.contains(&CLONING_MODEL.to_string()),
            "el modelo de clonado se purga: {:?}",
            outcome.snapshots
        );
        assert!(!clone_dir.exists(), "y su snapshot desaparece de verdad");
        assert!(outcome.is_clean(), "no hubo fallos: {:?}", outcome.failures);
    }

    /// Con toda la selección provisionada, `run` se completa sin red y sin
    /// descargar nada.
    #[test]
    fn run_with_everything_provisioned_needs_no_network() {
        let (_guard, _root) = cache_relocated("setup-run-offline");
        let store = avi_store::ModelStore::new().unwrap();
        for name in selection() {
            let (repo, revision) = avi_store::ModelStore::revision_of(name)
                .expect("todo repo pinneado tiene revisión");
            fake_snapshot(&repo.replace('/', "--"), revision);
        }

        let outcome = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("runtime de la prueba")
            .block_on(run(&store, &Options::user(false, true)))
            .expect("el setup sin red se completa");
        assert_eq!(outcome.provisioned.len(), selection().len());
    }

    /// La poda elimina justo las revisiones fuera del pin: la pinneada se
    /// queda, la obsoleta se va (incluso con el pin por `refs/`), los
    /// ficheros sueltos se ignoran, y una segunda pasada no encuentra nada.
    #[test]
    fn prune_removes_only_obsolete_revisions() {
        let root = scratch("setup-prune");
        let (repo, revision) =
            avi_store::ModelStore::revision_of("qwen3-tts-0.6b").expect("el modelo base tiene pin");
        let snapshots = root
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots");
        let old = "abc123abc123abc123abc123abc123abc123abcd";
        assert_ne!(old, revision, "la fixture obsoleta no es el pin");
        write_file(&snapshots.join(revision).join("config.json"), "{}");
        write_file(&snapshots.join(old).join("config.json"), "{}");
        write_file(&snapshots.join("README.txt"), "no es un snapshot");

        // Pin por `refs/`: el hash vivo se conserva y el resto se poda.
        let (translation_repo, translation_rev) =
            avi_store::ModelStore::revision_of("opus-mt-es-en").expect("opus-mt-es-en tiene pin");
        let translation_snaps = root
            .join(format!("models--{}", translation_repo.replace('/', "--")))
            .join("snapshots");
        let live_hash = "def456def456def456def456def456def456def4";
        let stale_hash = "0011220011220011220011220011220011220011";
        write_file(
            &root
                .join(format!("models--{}", translation_repo.replace('/', "--")))
                .join("refs")
                .join(translation_rev),
            live_hash,
        );
        write_file(&translation_snaps.join(live_hash).join("config.json"), "{}");
        write_file(
            &translation_snaps.join(stale_hash).join("config.json"),
            "{}",
        );

        let outcome = prune_obsolete_at(&root);
        assert_eq!(
            outcome.removed,
            vec![
                format!("opus-mt-es-en/snapshots/{stale_hash}"),
                format!("qwen3-tts-0.6b/snapshots/{old}"),
            ],
            "poda exacta: solo lo obsoleto, en orden"
        );
        assert!(outcome.is_clean(), "sin fallos: {:?}", outcome.failures);
        assert!(snapshots.join(revision).is_dir(), "el pin se queda");
        assert!(
            translation_snaps.join(live_hash).is_dir(),
            "el vivo por `refs/` se queda"
        );
        assert!(!snapshots.join(old).exists(), "lo obsoleto se va");
        assert!(
            snapshots.join("README.txt").is_file(),
            "los ficheros sueltos se ignoran"
        );

        let again = prune_obsolete_at(&root);
        assert!(
            again.removed.is_empty() && again.is_clean(),
            "idempotente: la segunda pasada no encuentra nada"
        );
    }

    /// En raíz compartida la poda solo toca los snapshots propios: `xet` y
    /// `.locks` son de todos y no se borran —la poda ni los contempla.
    #[test]
    fn prune_shared_root_keeps_unattributable() {
        let root = scratch("setup-prune-shared");
        let (repo, revision) =
            avi_store::ModelStore::revision_of("parakeet-tdt-v3").expect("parakeet tiene pin");
        let snapshots = root
            .join(format!("models--{}", repo.replace('/', "--")))
            .join("snapshots");
        let old = "9998887776665554443332221110009998887776";
        write_file(
            &snapshots.join(revision).join("encoder-model.int8.onnx"),
            "x",
        );
        write_file(&snapshots.join(old).join("encoder-model.int8.onnx"), "x");
        write_file(&root.join("xet").join("sentinel"), "caché global");
        write_file(&root.join(".locks").join("sentinel"), "locks globales");

        let outcome = prune_obsolete_at(&root);
        assert_eq!(
            outcome.removed,
            vec![format!("parakeet-tdt-v3/snapshots/{old}")],
            "solo el snapshot obsoleto propio"
        );
        assert!(outcome.is_clean());
        assert!(
            root.join("xet").join("sentinel").is_file(),
            "`xet` no se toca en raíz compartida"
        );
        assert!(
            root.join(".locks").join("sentinel").is_file(),
            "ni `.locks`"
        );
    }
}
