//! Criterios de aceptación del plan para `self update`: del **11** al **16**.
//!
//! Los seis criterios de este ciclo se prueban contra el servidor falso del arnés
//! (`support::FakeReleaseServer`), que suplanta al GitHub real con
//! `AVI_DOWNLOAD_BASE_URL`: resolución de `releases/latest`, `already_up_to_date`
//! sin descargas y, para la 0.24.0, descarga, verificación, extracción y arranque
//! deteniéndose antes del traspaso. La parte contra el GitHub real la ejecuta F5.
//! Además hay tres pruebas con el mismo prefijo fuera de los seis: checksum
//! inválido, negativa sin terminal y daemon que no se deja parar.
//!
//! Todas las pruebas usan el mismo sandbox que el Ciclo 1 (las cuatro raíces
//! reubicadas a temporales, declaradas en el entorno y pasadas como dato) más el
//! servidor falso apuntado por variable. Ninguna sale a internet ni toca la
//! instalación real: el bundle servido es sintético y sale del manifiesto.
//!
//! Lo dependiente de macOS del criterio 13 corre aquí mismo, porque nada de este
//! archivo depende de la plataforma: el guion del bundle ejecuta en Unix y en
//! Windows la comprobación de arranque la cubre el señuelo de
//! `binary_incompatible`, como en las pruebas de `update_fetch`. La puerta
//! `test-macos` ejecuta este archivo sin cambios, y arm64 está excluido por diseño
//! (sin código específico de arquitectura).

#![allow(clippy::disallowed_methods)]

mod support;

use avi_core::exit_codes::ExitCode;
use avi_lifecycle::channel::{self, Channel};
use avi_lifecycle::confirm;
use avi_lifecycle::daemon_stop::{self, ProcessControl};
#[cfg(feature = "faults")]
use avi_lifecycle::install;
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
#[cfg(feature = "faults")]
use avi_lifecycle::recovery;
use avi_lifecycle::setup;
use avi_lifecycle::target;
use avi_lifecycle::uninstall;
use avi_lifecycle::update;
use avi_lifecycle::update_fetch;
use avi_lifecycle::update_resolve::{self, ResolveRequest, Verdict};
use avi_lifecycle::LifecycleError;
use avi_store::{MODEL_FILE_PATTERNS, MODEL_REVISIONS};
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Mutex;
#[cfg(feature = "faults")]
use support::Inert;
use support::{FakeReleaseServer, Models, Sandbox};

/// Instalación registrada de `version` en el sandbox, sin pasar por
/// `self install`: escribe el ejecutable y el recibo. Es lo que necesitan
/// las pruebas de actualización, cuyo objeto es la actualización y no la
/// colocación.
fn install_version(sandbox: &Sandbox, version: &str) -> InstallReceipt {
    let receipt = InstallReceipt::new(
        version,
        target::host_triple(),
        Channel::Script,
        &sandbox.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.models_dir.clone(),
        },
        None,
    );
    support::write(
        &sandbox
            .program_dir
            .join(uninstall::executable_name_default()),
        &format!("binario {version}"),
    );
    receipt::write_to(&receipt, &sandbox.program_dir).expect("se escribe el recibo");
    receipt
}

/// `true` si la versión anterior sigue instalada y registrada: recibo con su
/// versión y ejecutable en su sitio.
fn previous_operational(sandbox: &Sandbox, version: &str) -> bool {
    receipt::read_from(&sandbox.program_dir)
        .ok()
        .flatten()
        .is_some_and(|receipt| receipt.version == version)
        && sandbox
            .program_dir
            .join(uninstall::executable_name_default())
            .is_file()
}

/// Control de procesos con un daemon vivo que se deja reclamar por PID: la
/// primera pregunta dice que vive y el reclamo lo apaga. Es el daemon activo del
/// criterio 13 sin proceso real.
struct Killable {
    alive: Mutex<bool>,
    kills: Mutex<u32>,
}

impl Killable {
    fn new() -> Self {
        Self {
            alive: Mutex::new(true),
            kills: Mutex::new(0),
        }
    }

    fn kill_count(&self) -> u32 {
        *self
            .kills
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

impl ProcessControl for Killable {
    fn pid_alive(&self, _pid: u32) -> bool {
        *self
            .alive
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
    fn kill_tree_by_pid(&self, _pid: u32) -> bool {
        *self
            .alive
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = false;
        *self
            .kills
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) += 1;
        true
    }
    fn resident_pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn sweep_resident_by_image(&self) -> bool {
        false
    }
}

/// Control con un daemon que no muere: `pid_alive` siempre afirma y el reclamo
/// nunca apaga. Es el daemon que obliga a `daemon_stop_failed` sin tocar nada.
struct Stubborn;

impl ProcessControl for Stubborn {
    fn pid_alive(&self, _pid: u32) -> bool {
        true
    }
    fn kill_tree_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn resident_pid_alive(&self, _pid: u32) -> bool {
        false
    }
    fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
        false
    }
    fn sweep_resident_by_image(&self) -> bool {
        false
    }
}

/// Removedor que nunca puede borrar ya pero sí programar el diferido: el
/// ejecutable en uso de Windows sin proceso real.
struct Busy;

impl update::PathRemover for Busy {
    fn remove_now(&self, _path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::bail!("el ejecutable está en uso")
    }
    fn schedule(&self, _path: &std::path::Path, _pid: u32) -> anyhow::Result<()> {
        Ok(())
    }
}

// ─── Criterio 11 ────────────────────────────────────────────────────────────────

/// **Criterio 11.** `self update` estando en la última versión termina con
/// `already_up_to_date` sin descargar nada.
///
/// Se resuelve la última estable contra el servidor falso y se compara con la
/// instalada: el veredicto es `UpToDate` y el contador de descargas del archivo
/// queda en cero, con las sumas sin pedir. El staging ni se crea.
#[test]
fn criterion_11_already_up_to_date_downloads_nothing() {
    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("c11");
    sandbox.seed_env();
    install_version(&sandbox, "0.24.0");

    let server = FakeReleaseServer::serve("0.24.0");
    server.seed_download_base();
    let client = update_fetch::http_client().expect("criterio 11: hay cliente HTTPS");
    let latest = runtime
        .block_on(update_resolve::latest_stable(&client))
        .expect("criterio 11: el servidor falso dice la última estable");
    assert_eq!(latest, "0.24.0");
    let resolution = update_resolve::resolve(
        &ResolveRequest {
            installed: "0.24.0".to_string(),
            explicit: None,
            force: false,
        },
        &latest,
    )
    .expect("criterio 11: la igualdad resuelve");
    FakeReleaseServer::clear_download_base();

    assert_eq!(
        resolution.verdict,
        Verdict::UpToDate,
        "criterio 11: instalada igual a la última, al día"
    );
    assert_eq!(
        server.asset_downloads(),
        0,
        "criterio 11: al día no se descarga el archivo"
    );
    assert_eq!(
        server.sums_downloads(),
        0,
        "criterio 11: y tampoco se piden las sumas"
    );
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "criterio 11: y no se crea staging"
    );
    assert!(
        previous_operational(&sandbox, "0.24.0"),
        "criterio 11: la instalación sigue operativa"
    );
}

// ─── Criterio 12 ────────────────────────────────────────────────────────────────

/// **Criterio 12.** `self update --check` informa la transición y no modifica el
/// disco.
///
/// En un sandbox limpio con la 0.23.1 instalada se ejecuta el camino de `--check`
/// (resolución sin staging ni traspaso): el objetivo es la 0.24.0 con veredicto de
/// actualización y el disco queda idéntico, tamaños incluidos. "Sin cambios" son
/// sin cambios de actualización: el barrido de recuperación del paso 1 vive en el
/// binario, no en este camino.
#[test]
fn criterion_12_check_leaves_disk_untouched() {
    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("c12");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = sandbox.snapshot();

    let server = FakeReleaseServer::serve("0.24.0");
    server.seed_download_base();
    let client = update_fetch::http_client().expect("criterio 12: hay cliente HTTPS");
    let latest = runtime
        .block_on(update_resolve::latest_stable(&client))
        .expect("criterio 12: el servidor falso dice la última estable");
    let resolution = update_resolve::resolve(
        &ResolveRequest {
            installed: "0.23.1".to_string(),
            explicit: None,
            force: false,
        },
        &latest,
    )
    .expect("criterio 12: la transición resuelve");
    FakeReleaseServer::clear_download_base();

    assert_eq!(
        resolution.target, "0.24.0",
        "criterio 12: informa la objetivo"
    );
    assert_eq!(
        resolution.verdict,
        Verdict::Upgrade,
        "criterio 12: e informa que hay actualización"
    );
    assert!(
        !resolution.destructive,
        "criterio 12: subir no es destructivo"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "criterio 12: `--check` no modifica el disco"
    );
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "criterio 12: y no crea staging"
    );
}

// ─── Criterio 13 ────────────────────────────────────────────────────────────────

/// **Criterio 13.** Con el daemon activo, la actualización lo detiene antes del
/// reemplazo y termina con éxito, incluido Windows con el ejecutable en uso.
///
/// Dos mitades a nivel de motor: la parada con un daemon vivo lo detiene por PID
/// (árbol exacto, nunca por imagen), deja el programa intacto y no crea staging
/// —es lo que "antes del reemplazo" significa aquí—; y la limpieza con el
/// ejecutable en uso no borra ya sino que programa el diferido. La comprobación
/// de arranque del bundle servido la cubre `fetch` en Unix y el señuelo de
/// `binary_incompatible` en Windows, como en `update_fetch`.
#[test]
fn criterion_13_daemon_stops_before_replacement() {
    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("c13");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    // Daemon activo: pidfile con un PID que el control declara vivo, sin
    // servidor de salud (puerto muerto). El PID no puede ser el propio: la
    // parada nunca reclama al invocador.
    let daemon_pid = 424_242u32;
    assert_ne!(daemon_pid, std::process::id());
    daemon_stop::write_pid(&sandbox.data_dir, daemon_pid, &support::dead_port(), 0)
        .expect("criterio 13: se planta el pidfile");
    let control = Killable::new();
    let outcome = runtime.block_on(daemon_stop::stop(
        &sandbox.data_dir,
        &support::dead_port(),
        &control,
    ));
    assert!(
        outcome.was_running,
        "criterio 13: el daemon estaba en ejecución"
    );
    daemon_stop::require_stopped(&outcome).expect("criterio 13: la parada lo detiene");
    assert!(
        outcome.stopped && outcome.pidfile_removed,
        "criterio 13: parada completa con pidfile borrado"
    );
    assert_eq!(
        control.kill_count(),
        1,
        "criterio 13: se reclamó el árbol por PID, no por imagen"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "criterio 13: la parada es antes del reemplazo y el programa sigue intacto"
    );
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "criterio 13: y todavía no hay staging"
    );

    // Ejecutable en uso: el borrado directo falla y queda a diferido, sin que el
    // `update` falle por ello.
    let staging = update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0");
    support::write(&staging.join("paquete.bin"), "staging en uso");
    assert_eq!(
        update::cleanup_staging(&staging, &Busy),
        update::StagingCleanup::Scheduled,
        "criterio 13: lo en uso se programa, no se borra ya"
    );
    assert!(
        support::exists(&staging),
        "criterio 13: y el staging sigue hasta que el diferido lo recoja"
    );
}

// ─── Criterio 14 ────────────────────────────────────────────────────────────────

/// **Criterio 14.** Interrumpir la actualización deja operativa la versión
/// anterior, y la siguiente operación completa la recuperación.
///
/// Con el feature `faults`: la interrupción antes del traspaso falla con el fallo
/// inyectado sin lanzar nada (y en Unix la de durante el traspaso tras un hijo
/// que sale con 0), la 0.23.1 sigue instalada, y la siguiente operación barre el
/// staging huérfano y completa la instalación de la 0.24.0. En Windows no hay
/// guion que lanzar como hijo, así que la mitad de durante el traspaso la cubre
/// el barrido de recuperación sobre el staging plantado.
#[cfg(feature = "faults")]
#[test]
fn criterion_14_interrupted_update_leaves_previous_operational() {
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("c14");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = sandbox.program_content();

    // Interrupción antes del traspaso: ni siquiera se lanza el binario nuevo.
    let window = faults::armed(FaultPoint::BeforeHandover);
    let error = runtime
        .block_on(update::handover(&update::HandoverRequest {
            staging_exe: PathBuf::from("/ruta/inexistente/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        }))
        .expect_err("criterio 14: el punto inyectado interrumpe el traspaso");
    drop(window);
    assert!(
        error
            .to_string()
            .contains(FaultPoint::BeforeHandover.as_str()),
        "criterio 14: el fallo es el inyectado y no otro: {error:#}"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "criterio 14: la versión anterior sigue operativa"
    );
    assert_eq!(
        sandbox.program_content(),
        before,
        "criterio 14: y el programa no se tocó"
    );

    // Interrupción con el traspaso ya ejecutado: el hijo sale con 0 y el fallo
    // llega en la limpieza. Solo en Unix, donde el guion ejecuta.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = sandbox.temp_root.join("hijo-c14.sh");
        support::write(&script, "#!/bin/sh\nexit 0\n");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("criterio 14: el hijo se hace ejecutable");
        let window = faults::armed(FaultPoint::DuringHandover);
        let error = runtime
            .block_on(update::handover(&update::HandoverRequest {
                staging_exe: script,
                no_setup: false,
                no_modify_path: false,
                force: false,
            }))
            .expect_err("criterio 14: el punto inyectado interrumpe tras el hijo");
        drop(window);
        assert!(
            error
                .to_string()
                .contains(FaultPoint::DuringHandover.as_str()),
            "criterio 14: el fallo es el inyectado y no otro: {error:#}"
        );
        assert!(
            previous_operational(&sandbox, "0.23.1"),
            "criterio 14: la versión anterior sigue operativa"
        );
    }

    // La siguiente operación recupera: el staging huérfano se barre y la
    // instalación de la 0.24.0 se completa sin residuo pendiente.
    let orphan = update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0");
    support::write(&orphan.join("paquete.bin"), "staging huérfano");
    let outcome = recovery::recover(recovery::Roots {
        program_dir: &sandbox.program_dir,
        temp_root: &sandbox.temp_root,
        in_use: None,
    })
    .expect("criterio 14: la recuperación se ejecuta");
    assert!(
        !support::exists(&orphan),
        "criterio 14: la recuperación barre el staging huérfano"
    );
    assert!(
        outcome.is_clean(),
        "criterio 14: y no queda nada pendiente: {outcome:?}"
    );
    let exe = sandbox.write_bundle(&sandbox.staging);
    let installed = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("criterio 14: la operación siguiente se completa");
    assert_eq!(installed.receipt.version, "0.24.0");
}

/// Sin el feature `faults` no hay inyección, y eso es una garantía del producto y no
/// una carencia: el binario distribuido no puede interrumpirse a propósito. El
/// traspaso con un ejecutable inexistente falla por el lanzamiento, no por un
/// fallo inyectado, y la 0.23.1 sigue operativa.
#[cfg(not(feature = "faults"))]
#[test]
fn criterion_14_fault_injection_is_inert_without_the_feature() {
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("c14-inerte");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    let window = faults::armed(FaultPoint::BeforeHandover);
    let error = runtime
        .block_on(update::handover(&update::HandoverRequest {
            staging_exe: PathBuf::from("/ruta/inexistente/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        }))
        .expect_err("criterio 14: sin hijo que lanzar el traspaso falla");
    drop(window);

    assert!(
        !error.to_string().contains("fallo inyectado"),
        "criterio 14: el fallo no es inyectado: {error:#}"
    );
    assert!(
        !faults::is_armed(FaultPoint::BeforeHandover),
        "criterio 14: sin `faults` armar es inerte"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "criterio 14: la versión anterior sigue operativa"
    );
}

// ─── Criterio 15 ────────────────────────────────────────────────────────────────

/// **Criterio 15.** En los canales `homebrew` y `dev`, `self update` termina con
/// `externally_managed` e indica el comando correcto.
///
/// La detección es la de la tabla de canales (el prefijo manda sobre el recibo, el recibo
/// sobre
/// `unmanaged`) y el `reason` con su código es el de la tabla cerrada; el binario
/// lo emite antes de tocar la red o el disco, así que aquí se afirma que nada se
/// toca. El comando exacto de cada canal es el que el brazo `Update` anuncia.
#[test]
fn criterion_15_managed_channels_are_externally_managed() {
    let _guard = support::exclusively();

    // `homebrew`: el prefijo de Homebrew gana sobre el recibo de `script`.
    let brew = Sandbox::new("c15-brew");
    brew.seed_env();
    let receipt = install_version(&brew, "0.23.1");
    let cask_exe = brew
        .root
        .join("opt")
        .join("homebrew")
        .join("Caskroom")
        .join("ai-voice-interconnector");
    let before = brew.snapshot();
    assert_eq!(
        channel::detect(&cask_exe, Some(&receipt)),
        Channel::Homebrew,
        "criterio 15: el prefijo de Homebrew manda sobre el recibo"
    );
    let failure = LifecycleError::externally_managed(
        "esta copia la gestiona Homebrew: actualiza con `brew upgrade --cask \
         ai-voice-interconnector`, y `cleanup --all` para el estado de usuario",
    );
    assert_eq!(failure.reason, "externally_managed");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        12,
        "`ExternallyManaged = 12` de la tabla única"
    );
    assert!(
        failure.message.contains("brew upgrade --cask"),
        "criterio 15: el mensaje indica el comando correcto"
    );
    assert_eq!(
        brew.snapshot(),
        before,
        "criterio 15: y nada se toca, ni programa ni estado"
    );

    // `dev`: lo declara el recibo, como `cargo xtask install` lo deja.
    let dev = Sandbox::new("c15-dev");
    dev.seed_env();
    let dev_receipt = InstallReceipt::new(
        "0.23.1",
        target::host_triple(),
        Channel::Dev,
        &dev.program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: dev.data_dir.clone(),
            cache_dir: dev.models_dir.clone(),
        },
        None,
    );
    support::write(
        &dev.program_dir.join(uninstall::executable_name_default()),
        "binario dev",
    );
    receipt::write_to(&dev_receipt, &dev.program_dir).expect("se escribe el recibo");
    let before = dev.snapshot();
    assert_eq!(
        channel::detect(
            &dev.program_dir.join(uninstall::executable_name_default()),
            Some(&dev_receipt)
        ),
        Channel::Dev,
        "criterio 15: el recibo declara el canal `dev`"
    );
    let failure = LifecycleError::externally_managed(
        "esta instalación es del canal dev: actualiza con `cargo xtask install`",
    );
    assert_eq!(failure.reason, "externally_managed");
    assert_eq!(ExitCode::from_reason(failure.reason).code(), 12);
    assert!(
        failure.message.contains("cargo xtask install"),
        "criterio 15: el mensaje indica el comando correcto"
    );
    assert_eq!(
        dev.snapshot(),
        before,
        "criterio 15: y nada se toca, ni programa ni estado"
    );
}

// ─── Criterio 16 ────────────────────────────────────────────────────────────────

/// **Criterio 16.** Después de actualizar, los modelos cuyo pin cambió quedan
/// provisionados y las revisiones propias obsoletas quedan podadas.
///
/// Tres mitades a nivel de motor: la selección guardada sobrevive al reemplazo
/// del programa (el reemplazo no toca la raíz de datos); lo pendiente contra el
/// pin vigente se detecta y se da por provisionado al plantarlo; y la poda tras
/// éxito elimina solo las revisiones fuera del pin, también en la raíz
/// compartida donde `xet`, `.locks` y lo ajeno son intocables (R3).
#[test]
fn criterion_16_selection_survives_and_obsolete_pruned() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c16");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    // La selección sobrevive al reemplazo: se guarda con clonado, se sustituye
    // el programa entero y se relee intacta.
    let mut selection = setup::SetupSelection::base();
    selection.with_voice_cloning = true;
    setup::write_selection_to(&sandbox.data_dir, &selection)
        .expect("criterio 16: la selección se guarda");
    support::write(
        &sandbox
            .program_dir
            .join(uninstall::executable_name_default()),
        "binario 0.24.0",
    );
    let back = setup::read_selection_from(&sandbox.data_dir);
    assert!(
        back.with_voice_cloning,
        "criterio 16: la selección sobrevive al reemplazo del programa"
    );

    // Lo pendiente contra el pin vigente: todo plantado menos un repo se
    // detecta, y plantado ese último no queda nada pendiente.
    let options = setup::Options::user(false, false, true);
    let names: Vec<&str> = setup::selection(&options);
    assert!(
        !names.is_empty(),
        "criterio 16: la selección base no es vacía"
    );
    let store = avi_store::ModelStore::new();
    for name in &names[..names.len() - 1] {
        plant_snapshot(&sandbox, name);
    }
    let pending = setup::pending(&store, &options);
    assert_eq!(
        pending.models,
        vec![names[names.len() - 1].to_string()],
        "criterio 16: el pin cambiado se detecta como pendiente"
    );
    plant_snapshot(&sandbox, names[names.len() - 1]);
    let pending = setup::pending(&store, &options);
    assert!(
        pending.models.is_empty(),
        "criterio 16: provisionado el pin, no queda nada que descargar: {:?}",
        pending.models
    );

    // La poda elimina solo la revisión fuera del pin y conserva la viva.
    let pin = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == names[0])
        .expect("criterio 16: el primer seleccionado tiene pin");
    let snapshots = sandbox
        .models_dir
        .join(support::repo_dir(pin.repo))
        .join("snapshots");
    support::write(
        &snapshots.join(pin.revision).join("pesos.bin"),
        "pin vigente",
    );
    support::write(
        &snapshots.join("obsoleta-0000").join("pesos.bin"),
        "revisión vieja",
    );
    let outcome = setup::prune_obsolete_at(&sandbox.models_dir);
    assert_eq!(
        outcome.removed,
        vec![format!("{}/snapshots/obsoleta-0000", pin.name)],
        "criterio 16: la poda elimina solo la revisión obsoleta"
    );
    assert!(
        snapshots.join(pin.revision).is_dir(),
        "criterio 16: y conserva la del pin vigente"
    );
    assert!(
        outcome.is_clean(),
        "criterio 16: y no informa fallos: {outcome:?}"
    );

    // En la raíz compartida la poda procede igual y R3 se respeta: `xet`, los
    // locks y el repo ajeno siguen intactos.
    let shared = Sandbox::new_with("c16-compartida", Models::Shared);
    shared.seed_env();
    let snapshots = shared
        .models_dir
        .join(support::repo_dir(pin.repo))
        .join("snapshots");
    support::write(
        &snapshots.join(pin.revision).join("pesos.bin"),
        "pin vigente",
    );
    support::write(
        &snapshots.join("obsoleta-0000").join("pesos.bin"),
        "revisión vieja",
    );
    support::write(&shared.models_dir.join("xet").join("shard"), "xet");
    support::write(
        &shared
            .models_dir
            .join(".locks")
            .join(support::repo_dir(pin.repo))
            .join("lock"),
        "",
    );
    support::write(
        &shared
            .models_dir
            .join("models--otra--herramienta")
            .join("otro.safetensors"),
        "ajeno",
    );
    let outcome = setup::prune_obsolete_at(&shared.models_dir);
    assert_eq!(
        outcome.removed,
        vec![format!("{}/snapshots/obsoleta-0000", pin.name)],
        "criterio 16: en compartida también poda lo obsoleto propio"
    );
    for kept in [
        shared.models_dir.join("xet").join("shard"),
        shared
            .models_dir
            .join("models--otra--herramienta")
            .join("otro.safetensors"),
    ] {
        assert!(
            kept.is_file(),
            "criterio 16: R3 deja lo compartido intacto: {}",
            kept.display()
        );
    }
}

/// Planta el snapshot del pin vigente de `name` con los archivos que
/// `avi-store` exige para darlo por provisionado, sin tocar la red.
fn plant_snapshot(sandbox: &Sandbox, name: &str) {
    let pin = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == name)
        .expect("el seleccionado tiene pin");
    let snapshot = sandbox
        .models_dir
        .join(support::repo_dir(pin.repo))
        .join("snapshots")
        .join(pin.revision);
    match MODEL_FILE_PATTERNS.iter().find(|(n, _)| *n == name) {
        Some((_, patterns)) => {
            for pattern in *patterns {
                support::write(&snapshot.join(pattern), "pesos");
            }
        }
        None => {
            support::write(&snapshot.join("model.safetensors"), "pesos");
        }
    }
}

// ─── Fuera de los seis: checksum, sin terminal y daemon que no para ─────────────

/// Checksum inválido: el archivo manipulado en tránsito falla con
/// `checksum_mismatch`, el staging se borra y la instalación queda intacta.
#[test]
fn criterion_update_checksum_mismatch_cleans_staging() {
    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("cuid-suma");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = sandbox.program_content();

    let server = FakeReleaseServer::serve("0.24.0");
    server.seed_download_base();
    server.serve_corrupt_asset();
    let client = update_fetch::http_client().expect("hay cliente HTTPS");
    let error = runtime
        .block_on(update_fetch::fetch(
            &client,
            &update_fetch::FetchRequest {
                program_dir: sandbox.program_dir.clone(),
                target: target::host_triple().to_string(),
                version: "0.24.0".to_string(),
            },
        ))
        .expect_err("el archivo manipulado no verifica");
    FakeReleaseServer::clear_download_base();

    assert_eq!(error.reason, "checksum_mismatch");
    assert_eq!(
        ExitCode::from_reason(error.reason).code(),
        21,
        "el 21 de la tabla única"
    );
    assert_eq!(
        server.asset_downloads(),
        1,
        "el archivo sí se descargó antes de rechazarlo"
    );
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "el staging se borra"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "la instalación queda intacta"
    );
    assert_eq!(sandbox.program_content(), before, "el programa no se tocó");
}

/// Sin terminal y sin `--yes`, lo destructivo se niega con
/// `confirmation_required`.
///
/// El `stdin` real no se puede fingir en el proceso, así que la prueba se
/// reejecuta a sí misma con la entrada redirigida a la null, como el criterio
/// 19: el hijo afirma la negativa y el padre exige su informe.
#[test]
fn criterion_update_no_terminal_refuses_destructive() {
    let _guard = support::exclusively();

    if support::is_child_without_terminal() {
        child_destructive_without_terminal();
        return;
    }

    let sandbox = Sandbox::new("cuid-sinterminal");
    let output =
        support::run_without_terminal("criterion_update_no_terminal_refuses_destructive", &sandbox);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "el proceso sin terminal terminó con {:?}.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
    let lines = support::read_report_lines(&output.stdout);
    assert_eq!(
        lines,
        vec!["destructive=confirmation_required/2"],
        "sin terminal y sin `--yes` lo destructivo se niega con el 2 de la tabla de reasons"
    );
}

/// Rollo del proceso hijo: la confirmación destructiva de la actualización sin
/// terminal y sin `--yes` se niega, con el `stdin` real del proceso como dato.
fn child_destructive_without_terminal() {
    let stdin = std::io::stdin();
    let summary = vec!["actualización 0.23.1 → 0.24.0".to_string()];
    let error = confirm::confirm(
        &confirm::Confirmation {
            kind: confirm::Kind::Destructive,
            summary: &summary,
            entries: &[],
            assume_yes: false,
            dry_run: false,
            stdin_is_terminal: stdin.is_terminal(),
        },
        &mut stdin.lock(),
        &mut std::io::stderr(),
    )
    .expect_err("sin terminal y sin `--yes` lo destructivo se niega");
    let failure = error
        .downcast_ref::<LifecycleError>()
        .expect("la negativa viaja como `LifecycleError`");
    assert_eq!(failure.reason, "confirmation_required");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        2,
        "error de uso"
    );
    support::report_line(&format!(
        "destructive={}/{}",
        failure.reason,
        ExitCode::from_reason(failure.reason).code()
    ));
}

/// Daemon que no se deja parar: `daemon_stop_failed` sin tocar nada.
///
/// Con el daemon vivo y el reclamo inerte, la parada no se completa y quien
/// llama aborta antes de descargar: el programa sigue intacto y no hay staging.
#[test]
fn criterion_update_daemon_stop_failure_aborts() {
    let _guard = support::exclusively();
    let runtime = support::runtime();
    let sandbox = Sandbox::new("cuid-daemon");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    let daemon_pid = 424_243u32;
    assert_ne!(daemon_pid, std::process::id());
    daemon_stop::write_pid(&sandbox.data_dir, daemon_pid, &support::dead_port(), 0)
        .expect("se planta el pidfile");
    // El pidfile es estado previo de la prueba, no algo que la parada cree: la
    // foto del disco se toma después de plantarlo.
    let before = sandbox.snapshot();
    let outcome = runtime.block_on(daemon_stop::stop(
        &sandbox.data_dir,
        &support::dead_port(),
        &Stubborn,
    ));
    assert!(outcome.was_running, "el daemon estaba en ejecución");
    let error = daemon_stop::require_stopped(&outcome)
        .expect_err("la parada que no completa es `daemon_stop_failed`");
    assert_eq!(error.reason, "daemon_stop_failed");
    assert_eq!(
        ExitCode::from_reason(error.reason).code(),
        16,
        "el 16 de la tabla única"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "sin parada no se toca nada: el programa sigue intacto y no hay staging"
    );
}
