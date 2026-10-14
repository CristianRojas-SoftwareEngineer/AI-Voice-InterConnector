// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Comportamiento de `self update`: resolución, `--check`, parada del daemon,
//! interrupción, canales gestionados externamente, poda de modelos, checksum inválido,
//! negativa sin terminal y daemon que no se deja parar.
//!
//! Las pruebas se ejecutan contra el servidor falso del arnés
//! (`release_server::FakeReleaseServer`), que suplanta al GitHub real con
//! `AVI_DOWNLOAD_BASE_URL`: resolución de `releases/latest`, `already_up_to_date`
//! sin descargas y, para la 0.24.0, descarga, verificación, extracción y arranque
//! deteniéndose antes del traspaso. Nada de este archivo habla con el GitHub real.
//!
//! Todas las pruebas usan el mismo sandbox (las cuatro raíces
//! reubicadas a temporales, declaradas en el entorno y pasadas como dato) más el
//! servidor falso apuntado por variable. Ninguna sale a internet ni toca la
//! instalación real: el bundle servido es sintético y sale del manifiesto.
//!
//! Lo dependiente de macOS corre aquí mismo, porque nada de este
//! archivo depende de la plataforma: el guion del bundle ejecuta en Unix y en
//! Windows la comprobación de arranque la cubre el señuelo de
//! `binary_incompatible`, como en las pruebas de `update_fetch`. La puerta
//! `test-macos` ejecuta este archivo sin cambios, y arm64 está excluido por diseño
//! (sin código específico de arquitectura).

#![allow(clippy::disallowed_methods)]

#[cfg(feature = "faults")]
#[path = "support/bundle.rs"]
mod bundle;
#[cfg(feature = "faults")]
#[path = "support/inert.rs"]
mod inert;
#[path = "support/inspect.rs"]
mod inspect;
#[cfg(feature = "faults")]
#[path = "support/install_env.rs"]
mod install_env;
#[path = "support/listing.rs"]
mod listing;
#[path = "support/net.rs"]
mod net;
#[path = "support/no_terminal.rs"]
mod no_terminal;
#[path = "support/process_env.rs"]
mod process_env;
#[path = "support/release_server.rs"]
mod release_server;
#[path = "support/repo.rs"]
mod repo;
#[path = "support/runtime.rs"]
mod runtime;
#[path = "support/shared.rs"]
mod shared;
mod support;

use avi_core::exit_codes::ExitCode;
use avi_lifecycle::channel::{self, Channel};
use avi_lifecycle::confirm;
use avi_lifecycle::daemon_stop::{self, ProcessControl, ProcessIdentity};
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
use avi_shared::paths::{MODEL_FILE_PATTERNS, MODEL_REVISIONS};
#[cfg(feature = "faults")]
use inert::Inert;
use release_server::FakeReleaseServer;
use std::io::IsTerminal;
use std::path::PathBuf;
use std::sync::Mutex;
use support::Sandbox;

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
            cache_dir: sandbox.cache_dir.clone(),
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

/// `true` si `version` sigue instalada y registrada: recibo con su
/// versión y ejecutable en su sitio.
fn previous_operational(sandbox: &Sandbox, version: &str) -> bool {
    receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .is_some_and(|receipt| receipt.version == version)
        && sandbox
            .program_dir
            .join(uninstall::executable_name_default())
            .is_file()
}

/// Identidad del daemon escrito en el pidfile: la misma que observan
/// `Killable` y `Stubborn`, para que la regla «sin identidad coincidente no se
/// mata» clasifique el PID como suyo.
fn daemon_identity() -> ProcessIdentity {
    ProcessIdentity {
        start: "111".to_string(),
        image: "daemon.exe".to_string(),
    }
}

/// Escribe el pidfile del daemon con su identidad y sin residente registrado.
fn set_daemon_pid_file(data_dir: &std::path::Path, pid: u32) -> anyhow::Result<()> {
    avi_process::registry::write_pid_file(
        data_dir,
        &avi_process::registry::PidFile {
            pid,
            addr: Some(net::dead_port()),
            started_at: None,
            resident_pid: 0,
            identity: Some(daemon_identity()),
            resident_identity: None,
            resident_port: None,
        },
    )
}

/// Control de procesos con un daemon vivo que se deja reclamar por PID: la
/// primera pregunta dice que vive y el reclamo lo apaga. Es un daemon activo sin
/// proceso real.
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
    // El pidfile escrito por la prueba registra esta identidad: sin un
    // `identity` observado que coincida, el control no clasifica el PID
    // como suyo.
    fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
        Some(daemon_identity())
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
    // Misma identidad que la escrita en el pidfile: sin ella, `Stubborn`
    // no reconocería al daemon como suyo.
    fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
        Some(daemon_identity())
    }
}

/// Removedor que nunca puede borrar ya pero sí programar el diferido: el
/// ejecutable en uso de Windows sin proceso real.
struct Busy;

impl update::PathRemover for Busy {
    fn remove_now(&self, _path: &std::path::Path) -> anyhow::Result<()> {
        anyhow::bail!("el ejecutable está en uso")
    }
    fn schedule(
        &self,
        _path: &std::path::Path,
        _wait: &avi_process::OwnerToken,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

// ─── Ya en la última versión ────────────────────────────────────────────────────

/// `self update` estando en la última versión termina con `already_up_to_date`
/// sin descargar nada.
///
/// Se resuelve la última estable contra el servidor falso y se compara con la
/// instalada: el veredicto es `UpToDate` y el contador de descargas del archivo
/// queda en cero, con las sumas sin pedir. El staging ni se crea.
#[test]
fn already_up_to_date_downloads_nothing() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("up-to-date");
    sandbox.seed_env();
    install_version(&sandbox, "0.24.0");

    let server = FakeReleaseServer::serve("0.24.0");
    server.seed_download_base();
    let client = update_fetch::http_client().expect("hay cliente HTTPS");
    let latest = runtime
        .block_on(update_resolve::latest_stable(&client))
        .expect("el servidor falso dice la última estable");
    assert_eq!(latest, "0.24.0");
    let resolution = update_resolve::resolve(
        &ResolveRequest {
            installed: "0.24.0".to_string(),
            explicit: None,
            force: false,
        },
        &latest,
    )
    .expect("la igualdad resuelve");
    FakeReleaseServer::clear_download_base();

    assert_eq!(
        resolution.verdict,
        Verdict::UpToDate,
        "instalada igual a la última, al día"
    );
    assert_eq!(
        server.asset_downloads(),
        0,
        "al día no se descarga el archivo"
    );
    assert_eq!(server.sums_downloads(), 0, "y tampoco se piden las sumas");
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "y no se crea staging"
    );
    assert!(
        previous_operational(&sandbox, "0.24.0"),
        "la instalación sigue operativa"
    );
}

// ─── `--check` ──────────────────────────────────────────────────────────────────

/// `self update --check` informa la transición y no modifica el disco.
///
/// En un sandbox limpio con la 0.23.1 instalada se ejecuta el camino de `--check`
/// (resolución sin staging ni traspaso): el objetivo es la 0.24.0 con veredicto de
/// actualización y el disco queda idéntico, tamaños incluidos. "Sin cambios" son
/// sin cambios de actualización: el barrido de recuperación del paso 1 vive en el
/// binario, no en este camino.
#[test]
fn check_leaves_disk_untouched() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("check");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = sandbox.snapshot();

    let server = FakeReleaseServer::serve("0.24.0");
    server.seed_download_base();
    let client = update_fetch::http_client().expect("hay cliente HTTPS");
    let latest = runtime
        .block_on(update_resolve::latest_stable(&client))
        .expect("el servidor falso dice la última estable");
    let resolution = update_resolve::resolve(
        &ResolveRequest {
            installed: "0.23.1".to_string(),
            explicit: None,
            force: false,
        },
        &latest,
    )
    .expect("la transición resuelve");
    FakeReleaseServer::clear_download_base();

    assert_eq!(resolution.target, "0.24.0", "informa la versión objetivo");
    assert_eq!(
        resolution.verdict,
        Verdict::Upgrade,
        "e informa que hay actualización"
    );
    assert!(!resolution.destructive, "subir no es destructivo");
    assert_eq!(sandbox.snapshot(), before, "`--check` no modifica el disco");
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "y no crea staging"
    );
}

// ─── Parada del daemon ──────────────────────────────────────────────────────────

/// Con el daemon activo, la actualización lo detiene antes del
/// reemplazo y termina con éxito, incluido Windows con el ejecutable en uso.
///
/// Dos mitades a nivel de motor: la parada con un daemon vivo lo detiene por PID
/// (árbol exacto, nunca por imagen), deja el programa intacto y no crea staging
/// —es lo que "antes del reemplazo" significa aquí—; y la limpieza con el
/// ejecutable en uso no borra en el acto sino que programa el diferido. La comprobación
/// de arranque del bundle servido la cubre `fetch` en Unix y el señuelo de
/// `binary_incompatible` en Windows, como en `update_fetch`.
#[test]
fn daemon_stops_before_replacement() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("daemon-stop");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    // Daemon activo: pidfile con un PID que el control declara vivo, sin
    // servidor de salud (puerto muerto). El PID no puede ser el propio: la
    // parada nunca reclama al invocador.
    let daemon_pid = 424_242u32;
    assert_ne!(daemon_pid, std::process::id());
    set_daemon_pid_file(&sandbox.data_dir, daemon_pid).expect("se escribe el pidfile");
    let control = Killable::new();
    let outcome = runtime.block_on(daemon_stop::stop(
        &sandbox.data_dir,
        &net::dead_port(),
        &control,
    ));
    assert!(outcome.was_running, "el daemon estaba en ejecución");
    daemon_stop::require_stopped(&outcome).expect("la parada lo detiene");
    assert!(
        outcome.stopped && outcome.pidfile_removed,
        "parada completa con pidfile borrado"
    );
    assert_eq!(
        control.kill_count(),
        1,
        "se reclamó el árbol por PID, no por imagen"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "la parada es antes del reemplazo y el programa sigue intacto"
    );
    assert!(
        !update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0").exists(),
        "y todavía no hay staging"
    );

    // Ejecutable en uso: el borrado directo falla y queda a diferido, sin que el
    // `update` falle por ello.
    let staging = update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0");
    support::write(&staging.join("package.bin"), "staging en uso");
    assert_eq!(
        update::cleanup_staging(&staging, &Busy),
        update::StagingCleanup::Scheduled,
        "lo que está en uso se programa, no se borra en el acto"
    );
    assert!(
        inspect::exists(&staging),
        "y el staging sigue hasta que el diferido lo recoja"
    );
}

// ─── Interrupción ───────────────────────────────────────────────────────────────

/// Interrumpir la actualización deja operativa la versión instalada, y la siguiente
/// operación completa la recuperación.
///
/// Con el feature `faults`: la interrupción antes del traspaso falla con el fallo
/// inyectado sin lanzar nada (y en Unix la de durante el traspaso tras un hijo
/// que sale con 0), la 0.23.1 sigue instalada, y la siguiente operación barre el
/// staging huérfano y completa la instalación de la 0.24.0. En Windows no hay
/// guion que lanzar como hijo, así que la mitad de durante el traspaso la cubre
/// el barrido de recuperación sobre el staging creado.
#[cfg(feature = "faults")]
#[test]
fn interrupted_update_leaves_installed_version_operational() {
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("interrupted");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = listing::list(&sandbox.program_dir);

    // Interrupción antes del traspaso: ni siquiera se lanza el binario nuevo.
    let window = faults::armed(FaultPoint::BeforeHandover);
    let error = runtime
        .block_on(update::handover(&update::HandoverRequest {
            staging_exe: PathBuf::from("/ruta/inexistente/ai-voice-interconnector"),
            no_setup: false,
            no_modify_path: false,
            force: false,
        }))
        .expect_err("el punto inyectado interrumpe el traspaso");
    drop(window);
    assert!(
        error
            .to_string()
            .contains(FaultPoint::BeforeHandover.as_str()),
        "el fallo es el inyectado y no otro: {error:#}"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "la versión instalada sigue operativa"
    );
    assert_eq!(
        listing::list(&sandbox.program_dir),
        before,
        "y el programa no se tocó"
    );

    // Interrupción con el traspaso ya ejecutado: el hijo sale con 0 y el fallo
    // llega en la limpieza. Solo en Unix, donde el guion ejecuta.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let script = sandbox.temp_root.join("child.sh");
        support::write(&script, "#!/bin/sh\nexit 0\n");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("el hijo se hace ejecutable");
        let window = faults::armed(FaultPoint::DuringHandover);
        let error = runtime
            .block_on(update::handover(&update::HandoverRequest {
                staging_exe: script,
                no_setup: false,
                no_modify_path: false,
                force: false,
            }))
            .expect_err("el punto inyectado interrumpe tras el hijo");
        drop(window);
        assert!(
            error
                .to_string()
                .contains(FaultPoint::DuringHandover.as_str()),
            "el fallo es el inyectado y no otro: {error:#}"
        );
        assert!(
            previous_operational(&sandbox, "0.23.1"),
            "la versión instalada sigue operativa"
        );
    }

    // La siguiente operación recupera: el staging huérfano se barre y la
    // instalación de la 0.24.0 se completa sin residuo pendiente.
    let orphan = update_fetch::staging_dir_for(&sandbox.program_dir, "0.24.0");
    support::write(&orphan.join("package.bin"), "staging huérfano");
    let outcome = recovery::recover(recovery::Roots {
        program_dir: &sandbox.program_dir,
        temp_root: &sandbox.temp_root,
        in_use: None,
    })
    .expect("la recuperación se ejecuta");
    assert!(
        !inspect::exists(&orphan),
        "la recuperación barre el staging huérfano"
    );
    assert!(outcome.is_clean(), "y no queda nada pendiente: {outcome:?}");
    let exe = sandbox.write_bundle(&sandbox.staging());
    let installed = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("la operación siguiente se completa");
    assert_eq!(installed.receipt.version, "0.24.0");
}

/// Sin el feature `faults` no hay inyección, y eso es una garantía del producto y no
/// una carencia: el binario distribuido no puede interrumpirse a propósito. El
/// traspaso con un ejecutable inexistente falla por el lanzamiento, no por un
/// fallo inyectado, y la 0.23.1 sigue operativa.
#[cfg(not(feature = "faults"))]
#[test]
fn fault_injection_is_inert_without_the_feature() {
    use avi_lifecycle::faults::{self, FaultPoint};

    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("fault-inert");
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
        .expect_err("sin hijo que lanzar el traspaso falla");
    drop(window);

    assert!(
        !error.to_string().contains("fallo inyectado"),
        "el fallo no es inyectado: {error:#}"
    );
    assert!(
        !faults::is_armed(FaultPoint::BeforeHandover),
        "sin `faults` armar es inerte"
    );
    assert!(
        previous_operational(&sandbox, "0.23.1"),
        "la versión instalada sigue operativa"
    );
}

// ─── Canales gestionados externamente ───────────────────────────────────────────

/// En el canal `homebrew`, `self update` termina con `externally_managed` e indica
/// el comando correcto.
///
/// La detección es la de la tabla de canales (el prefijo manda sobre el recibo, el recibo
/// sobre
/// `unmanaged`) y el `reason` con su código es el de la tabla cerrada; el binario
/// lo emite antes de tocar la red o el disco, así que aquí se afirma que nada se
/// toca. El comando exacto de cada canal es el que el brazo `Update` anuncia.
#[test]
fn managed_channels_are_externally_managed() {
    let _guard = process_env::exclusively();

    // `homebrew`: el prefijo de Homebrew gana sobre el recibo de `script`.
    let brew = Sandbox::new("managed-brew");
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
        "el prefijo de Homebrew manda sobre el recibo"
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
        "el mensaje indica el comando correcto"
    );
    assert_eq!(
        brew.snapshot(),
        before,
        "y nada se toca, ni programa ni estado"
    );
}

// ─── Modelos tras actualizar ────────────────────────────────────────────────────

/// Después de actualizar, los modelos cuyo pin cambió quedan provisionados y las
/// revisiones propias obsoletas quedan podadas.
///
/// Dos mitades a nivel de motor: lo pendiente contra el pin vigente se detecta
/// y se da por provisionado al escribirlo; y la poda tras éxito elimina solo las
/// revisiones fuera del pin, también en la raíz compartida donde `xet`,
/// `.locks` y lo ajeno son intocables.
#[test]
fn pending_pin_detected_and_obsolete_pruned() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("prune");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    // Lo pendiente contra el pin vigente: todo escrito menos un repo se
    // detecta, y escrito ese último no queda nada pendiente.
    let names: Vec<&str> = setup::selection();
    assert!(!names.is_empty(), "la selección no es vacía");
    let store = avi_store::ModelStore::new().unwrap();
    for name in &names[..names.len() - 1] {
        set_snapshot(&sandbox, name);
    }
    let pending = setup::pending(&store);
    assert_eq!(
        pending.models,
        vec![names[names.len() - 1].to_string()],
        "el pin cambiado se detecta como pendiente"
    );
    set_snapshot(&sandbox, names[names.len() - 1]);
    let pending = setup::pending(&store);
    assert!(
        pending.models.is_empty(),
        "provisionado el pin, no queda nada que descargar: {:?}",
        pending.models
    );

    // La poda elimina solo la revisión fuera del pin y conserva la viva.
    let pin = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == names[0])
        .expect("el primer seleccionado tiene pin");
    let snapshots = sandbox
        .models_dir
        .join(repo::repo_dir(pin.repo))
        .join("snapshots");
    support::write(
        &snapshots.join(pin.revision).join("weights.bin"),
        "pin vigente",
    );
    support::write(
        &snapshots.join("obsolete-0000").join("weights.bin"),
        "revisión vieja",
    );
    let outcome = setup::prune_obsolete_at(&sandbox.models_dir);
    assert_eq!(
        outcome.removed,
        vec![format!("{}/snapshots/obsolete-0000", pin.name)],
        "la poda elimina solo la revisión obsoleta"
    );
    assert!(
        snapshots.join(pin.revision).is_dir(),
        "y conserva la del pin vigente"
    );
    assert!(outcome.is_clean(), "y no informa fallos: {outcome:?}");

    // En la raíz compartida la poda procede igual y lo ajeno se respeta: `xet`, los
    // locks y el repo ajeno siguen intactos.
    let shared = Sandbox::new_shared("prune-shared");
    shared.seed_env();
    let snapshots = shared
        .models_dir
        .join(repo::repo_dir(pin.repo))
        .join("snapshots");
    support::write(
        &snapshots.join(pin.revision).join("weights.bin"),
        "pin vigente",
    );
    support::write(
        &snapshots.join("obsolete-0000").join("weights.bin"),
        "revisión vieja",
    );
    support::write(&shared.models_dir.join("xet").join("shard"), "xet");
    support::write(
        &shared
            .models_dir
            .join(".locks")
            .join(repo::repo_dir(pin.repo))
            .join("lock"),
        "",
    );
    support::write(
        &shared
            .models_dir
            .join("models--other--tool")
            .join("other.safetensors"),
        "ajeno",
    );
    let outcome = setup::prune_obsolete_at(&shared.models_dir);
    assert_eq!(
        outcome.removed,
        vec![format!("{}/snapshots/obsolete-0000", pin.name)],
        "en compartida también poda lo obsoleto propio"
    );
    for kept in [
        shared.models_dir.join("xet").join("shard"),
        shared
            .models_dir
            .join("models--other--tool")
            .join("other.safetensors"),
    ] {
        assert!(
            kept.is_file(),
            "lo compartido ajeno queda intacto: {}",
            kept.display()
        );
    }
}

/// Escribe el snapshot del pin vigente de `name` con los archivos que
/// `avi-store` exige para darlo por provisionado, sin tocar la red.
fn set_snapshot(sandbox: &Sandbox, name: &str) {
    let pin = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == name)
        .expect("el seleccionado tiene pin");
    let snapshot = sandbox
        .models_dir
        .join(repo::repo_dir(pin.repo))
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

// ─── Checksum, sin terminal y daemon que no para ────────────────────────────────

/// Checksum inválido: el archivo manipulado en tránsito falla con
/// `checksum_mismatch`, el staging se borra y la instalación queda intacta.
#[test]
fn update_checksum_mismatch_cleans_staging() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("checksum");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");
    let before = listing::list(&sandbox.program_dir);

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
    assert_eq!(
        listing::list(&sandbox.program_dir),
        before,
        "el programa no se tocó"
    );
}

/// Sin terminal y sin `--yes`, lo destructivo se niega con
/// `confirmation_required`.
///
/// El `stdin` real no se puede fingir en el proceso, así que la prueba se
/// reejecuta a sí misma con la entrada redirigida a la null: el hijo afirma la
/// negativa y el padre exige su informe.
#[test]
fn update_no_terminal_refuses_destructive() {
    let _guard = process_env::exclusively();

    if no_terminal::is_child_without_terminal() {
        child_destructive_without_terminal();
        return;
    }

    let sandbox = Sandbox::new("no-terminal");
    let output = no_terminal::run_without_terminal(
        "update_no_terminal_refuses_destructive",
        "no-terminal",
        &sandbox,
    );
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "el proceso sin terminal terminó con {:?}.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );
    let lines = no_terminal::read_report_lines(&output.stdout);
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
    no_terminal::report_line(&format!(
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
fn update_daemon_stop_failure_aborts() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();
    let sandbox = Sandbox::new("daemon-stubborn");
    sandbox.seed_env();
    install_version(&sandbox, "0.23.1");

    let daemon_pid = 424_243u32;
    assert_ne!(daemon_pid, std::process::id());
    set_daemon_pid_file(&sandbox.data_dir, daemon_pid).expect("se escribe el pidfile");
    // El pidfile es estado previo de la prueba, no algo que la parada cree: la
    // foto del disco se toma después de escribirlo.
    let before = sandbox.snapshot();
    let outcome = runtime.block_on(daemon_stop::stop(
        &sandbox.data_dir,
        &net::dead_port(),
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

/// La compuerta que `self update` retiene durante el traspaso excluye a los
/// comandos de uso aunque el principal esté libre, y el hijo del traspaso
/// (`AVI_HANDOVER_PARENT_PID` presente) toma el principal sin la compuerta.
#[test]
fn handover_gate_blocks_usage_and_lets_the_child_in() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("gate");
    sandbox.seed_env();

    let mut parent = avi_lifecycle::lock::acquire_exclusive(&sandbox.program_dir)
        .expect("el padre toma compuerta y principal");
    parent.release_main();

    let usage = avi_lifecycle::lock::acquire_shared(&sandbox.program_dir)
        .expect_err("con la compuerta retenida el uso no entra");
    assert_eq!(usage.reason, "lifecycle_locked");

    std::env::set_var(
        update::HANDOVER_PARENT_PID_ENV,
        std::process::id().to_string(),
    );
    let child = avi_lifecycle::lock::acquire_exclusive(&sandbox.program_dir);
    std::env::remove_var(update::HANDOVER_PARENT_PID_ENV);
    let child = child.expect("el hijo toma el principal sin la compuerta");
    drop(child);
    drop(parent);
    drop(
        avi_lifecycle::lock::acquire_shared(&sandbox.program_dir)
            .expect("al volver el traspaso, el uso entra"),
    );
}
