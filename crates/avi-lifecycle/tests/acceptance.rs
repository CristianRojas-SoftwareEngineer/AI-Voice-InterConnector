// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Pruebas de aceptación del ciclo de vida: instalación idempotente, `--no-setup`,
//! desinstalación (completa, con `--keep-data`, repetida), confirmación sin terminal,
//! `--dry-run`, categorías de `cleanup` y protección de los recursos compartidos. La
//! prueba de cuarentena de macOS vive en `tests/quarantine.rs`, porque solo se puede
//! ejecutar allí.
//!
//! Todas las pruebas usan el mismo arnés (`tests/support/mod.rs`), que hace las dos
//! cosas que el aislamiento exige y que no son la misma: **declarar las cuatro
//! variables de reubicación** apuntando al sandbox, para que lo que el motor resuelve por el
//! entorno caiga dentro del sandbox, y **pasar las mismas rutas como dato** en `Env` y
//! `Roots`, que es como las recibe en producción. En Windows las Known Folders ignoran
//! `LOCALAPPDATA`, así que un sandbox que se apoyara en ellas no estaría probando nada
//! del mecanismo de reubicación.
//!
//! Ninguna prueba depende de red, de ONNX Runtime ni del motor TTS: el bundle es
//! sintético y sale de `packaging/bundle-manifest.json`. El único proceso externo que
//! aparece en todo el archivo es la descarga de `setup`, que apunta a un puerto local
//! cerrado y por eso falla sin tocar la red real.
//!
//! `setup` tiene dos pruebas: `--no-setup` no provisiona nada, y un fallo de `setup`
//! deja el programa instalado.

#![allow(clippy::disallowed_methods)]

#[path = "support/bundle.rs"]
mod bundle;
#[path = "support/inert.rs"]
mod inert;
#[path = "support/inspect.rs"]
mod inspect;
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
#[path = "support/registered.rs"]
mod registered;
#[path = "support/repo.rs"]
mod repo;
#[path = "support/runtime.rs"]
mod runtime;
#[path = "support/shared.rs"]
mod shared;
mod support;

use avi_core::exit_codes::ExitCode;
use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup;
use avi_lifecycle::install::{self, ModelsState};
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::setup;
use avi_lifecycle::uninstall;
use avi_shared::paths::{MODEL_FILE_PATTERNS, MODEL_REVISIONS};
use avi_store::ModelStore;
use inert::Inert;
use registered::Now;
use std::path::{Path, PathBuf};
use support::Sandbox;

/// Pin que la prueba de fallo de `setup` deja sin sembrar para que la descarga falle.
const UNSEEDED_PIN: &str = "opus-mt-es-en";

/// Recibo con el instante de instalación neutralizado, para comparar dos pasadas por
/// todo lo demás. `installed_at` es lo único que el esquema declara que cambia entre dos
/// instalaciones de la misma versión.
fn normalized_receipt(receipt: &InstallReceipt) -> InstallReceipt {
    let mut copy = receipt.clone();
    copy.installed_at = "<instante>".to_string();
    copy
}

/// Estado del sandbox sin el archivo de bloqueo, que la recuperación crea al tomar el
/// bloqueo y que no es «nada modificado» sino el mecanismo que serializa las operaciones.
fn state_without_lock(sandbox: &Sandbox) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = sandbox
        .snapshot()
        .into_iter()
        .filter(|(path, _)| {
            ![
                avi_lifecycle::LIFECYCLE_LOCK_NAME,
                avi_lifecycle::LIFECYCLE_GATE_NAME,
            ]
            .contains(&path.rsplit('/').next().unwrap_or_default())
        })
        .collect();
    out.sort();
    out
}

/// Entradas del directorio padre del programa, que es donde viven el staging hermano, el
/// archivo de bloqueo y los aparcados. El residuo cero de la desinstalación se afirma sobre
/// esto y no solo sobre el directorio de programa.
fn opt_entries(sandbox: &Sandbox) -> Vec<String> {
    let parent = sandbox
        .program_dir
        .parent()
        .expect("el directorio de programa tiene padre");
    listing::list(parent)
}

/// Rutas del plan de borrado, como cadenas, que es la forma en que el envelope las
/// publica.
fn paths(plan: &cleanup::DeletionPlan) -> Vec<String> {
    plan.paths()
        .iter()
        .map(|p| p.display().to_string())
        .collect()
}

/// Rutas de las entradas de un plan de desinstalación, como cadenas.
fn entry_paths(plan: &[avi_lifecycle::confirm::PlanEntry]) -> Vec<String> {
    plan.iter().map(|e| e.path.display().to_string()).collect()
}

/// Un scratch huérfano (de un proceso muerto, con un PID que el sistema no puede
/// asignar), que la regla obliga a barrer, creado junto a los recursos compartidos de
/// `seed_state`: es el contraste que demuestra que el barrido decide por la identidad del
/// propietario y no por directorio.
fn seed_orphan_scratch(sandbox: &Sandbox) -> PathBuf {
    let scratch = sandbox.scratch_root.join("4294967000-1");
    support::write(&scratch.join("clip.wav"), "temporal propio");
    scratch
}

/// `cleanup` con `--yes` y la categoría que le pase quien llama.
fn with_yes() -> cleanup::Options {
    cleanup::Options {
        assume_yes: true,
        ..Default::default()
    }
}

/// Ejecuta una categoría y comprueba que no falló nada y que borró exactamente lo que el
/// plan anuncia, que es la comprobación que un plan correcto con un ejecutor laxo no
/// superaría.
fn run_category(sandbox: &Sandbox, runtime: &tokio::runtime::Runtime, options: cleanup::Options) {
    let announced = paths(&cleanup::plan(&sandbox.roots(), &options));
    let outcome = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("la categoría se ejecuta");
    assert!(
        outcome.failed.is_empty(),
        "nada falló: {:?}",
        outcome.failed
    );
    for path in &announced {
        assert!(
            !inspect::exists(Path::new(path)),
            "lo anunciado se borró: {path}"
        );
    }
    assert_eq!(
        outcome.removed, announced,
        "el plan y la ejecución coinciden"
    );
}

/// Provisiona en el sandbox los repos de la selección, con los archivos críticos que
/// `avi-store` exige para dar cada uno por provisionado.
///
/// Es lo que permite que la provisión **no toque la red**: con los snapshots ya en su
/// sitio, la provisión no tiene repos que descargar. Cada repo de pruebas se escribe con
/// los nombres de archivo que el producto espera, no con un marcador, porque `is_provisioned` decide
/// por presencia y tamaño de esos archivos.
fn provision_selection(sandbox: &Sandbox) {
    for pin in MODEL_REVISIONS {
        let snapshot = sandbox
            .models_dir
            .join(repo::repo_dir(pin.repo))
            .join("snapshots")
            .join(pin.revision);
        match MODEL_FILE_PATTERNS.iter().find(|(n, _)| *n == pin.name) {
            Some((_, patterns)) => {
                for pattern in *patterns {
                    support::write(&snapshot.join(pattern), "pesos");
                }
            }
            // Sin patrones: basta un archivo con contenido, que es lo que
            // `is_provisioned` comprueba para los repos no acotados.
            None => {
                support::write(&snapshot.join("model.safetensors"), "pesos");
            }
        }
    }
}

// ─── Instalación idempotente ──────────────────────────────────────────────────────

/// Repetir la instalación con la misma versión termina con éxito y deja
/// el mismo estado, sin entradas de `PATH` ni bloques de perfil duplicados.
///
/// Lo que se afirma va más allá de «las dos llamadas devuelven lo mismo»: se compara el
/// recibo leído de disco con el instante neutralizado, el contenido del directorio de
/// programa antes y después de la segunda pasada, y **el número de apariciones** de la
/// entrada en el destino de la integración —el valor `Path` de la clave de prueba en
/// Windows, el bloque delimitado y el enlace en Unix—. Un duplicado es exactamente el
/// defecto que la idempotencia prohíbe, y comparar solo los desenlaces no lo vería.
#[test]
fn install_twice_is_idempotent() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("install-twice");
    sandbox.seed_env();
    let options = Sandbox::install_options();
    let runtime = runtime::runtime();

    // ── Primera instalación ─────────────────────────────────────────────────────
    let exe = sandbox.write_bundle(&sandbox.staging());
    let first = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("la primera instalación se completa");
    assert_eq!(first.status, "installed", "la primera pasada instala");
    assert!(
        first.path_changed(),
        "la primera pasada reescribe el `PATH`"
    );
    let first_receipt = normalized_receipt(&first.receipt);
    let first_content = listing::list(&sandbox.program_dir);
    assert!(
        first_content.contains(&receipt::RECEIPT_NAME.to_string()),
        "el recibo está entre lo colocado: {first_content:?}"
    );

    // ── Segunda instalación, desde el mismo bundle ──────────────────────────────
    // El bundle se repone porque la colocación **mueve** los archivos desde el origen.
    // Es también lo que hace el bootstrap, que extrae en un staging nuevo cada vez.
    let exe = sandbox.write_bundle(&sandbox.staging());
    let second = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("la segunda instalación se completa");

    assert_eq!(
        second.status, "installed",
        "la segunda pasada también termina en éxito"
    );
    assert_eq!(
        second.mode,
        install::Mode::Install,
        "sigue siendo instalación, porque el ejecutable viene del staging"
    );
    assert_eq!(
        normalized_receipt(&second.receipt),
        first_receipt,
        "el mismo estado final, con el mismo recibo"
    );
    assert_eq!(
        second.path_integration, first.path_integration,
        "la integración registrada es la misma, no el diff de esta pasada"
    );
    assert!(
        !second.path_changed(),
        "la segunda pasada no reescribe el `PATH`: la entrada ya estaba"
    );
    assert!(
        second.path_integrated(),
        "pero sigue registrada en el recibo, que es lo que permite revertirla"
    );
    assert_eq!(
        listing::list(&sandbox.program_dir),
        first_content,
        "el directorio de programa no cambia de contenido"
    );

    // El recibo de disco coincide con el que devolvió la operación.
    let on_disk = receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .expect("el recibo existe");
    assert_eq!(
        normalized_receipt(&on_disk),
        first_receipt,
        "el recibo de disco es el mismo"
    );

    // Y ahora el punto central: la integración no está duplicada.
    #[cfg(unix)]
    {
        let profile = sandbox.home.join(".profile");
        let text = std::fs::read_to_string(&profile).expect("el perfil se escribió");
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "un solo bloque delimitado tras dos instalaciones: {text}"
        );
        let link = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        let dest = std::fs::read_link(&link).expect("el enlace existe");
        assert_eq!(
            dest.parent(),
            Some(sandbox.program_dir.as_path()),
            "el enlace apunta al directorio de programa"
        );
    }
    #[cfg(windows)]
    {
        let value = avi_lifecycle::path_windows::read_path(&sandbox.registry_subkey)
            .expect("se lee el valor de la clave de prueba")
            .expect("la integración escribió el valor")
            .value;
        let entry = sandbox.bin_dir.display().to_string();
        assert_eq!(
            value.matches(&entry).count(),
            1,
            "la entrada aparece una sola vez tras dos instalaciones: {value}"
        );
    }
}

/// Líneas del resumen que hablan del `PATH`.
fn summary_lines(summary: &[String]) -> Vec<&str> {
    summary
        .iter()
        .map(String::as_str)
        .filter(|l| l.trim_start().starts_with("PATH:"))
        .collect()
}

/// **El resumen final dice el estado del `PATH` una vez, y lo dice en el estado.**
///
/// `final_summary` no copia la línea del `PATH` del **resumen previo** —que dice lo que se
/// va a hacer— junto a la del **estado**. Si lo hiciera, en una instalación correcta el
/// usuario leería, en el mismo bloque y en este orden:
///
/// ```text
///   PATH:      se añadirá <dir> en el PATH del usuario
///   PATH:      se añadió <dir>; abre una terminal nueva para que el comando se encuentre.
/// ```
///
/// Dos líneas para un hecho, y la primera en futuro y la segunda en pasado: leído de un
/// tirón parece una contradicción. Afirma los **tres** estados posibles, porque la regla no
/// es «omitir la primera línea» sino «dejar una sola, la del estado», y cada rama del
/// `if` del resumen produce una prosa distinta que también hay que fijar.
#[test]
fn final_summary_reports_the_path_state_once() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();

    // ── Estado 1: esta pasada **reescribió** el `PATH` ────────────────────────────
    let sandbox = Sandbox::new("path-new");
    sandbox.seed_env();
    let exe = sandbox.write_bundle(&sandbox.staging());
    let first = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("la primera instalación se completa");
    assert!(
        first.path_changed(),
        "la primera pasada reescribe el `PATH`, que es el estado que hay que probar"
    );
    let lines = summary_lines(&first.summary);
    assert_eq!(
        lines.len(),
        1,
        "el resumen final dice el `PATH` una sola vez, y no dos: {:?}",
        first.summary
    );
    assert!(
        lines[0].contains("se añadió"),
        "y en pasado, porque esta pasada lo escribió: {}",
        lines[0]
    );
    assert!(
        !lines[0].contains("se añadirá"),
        "y no en futuro: el resumen final no anuncia, informa: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("abre una terminal nueva"),
        "con la indicación de terminal nueva, que solo aparece si se acaba de escribir: {}",
        lines[0]
    );

    // ── Estado 2: la integración está en pie y esta pasada no la tocó ────────────
    let exe = sandbox.write_bundle(&sandbox.staging());
    let second = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("la segunda instalación se completa");
    assert!(!second.path_changed());
    let lines = summary_lines(&second.summary);
    assert_eq!(
        lines.len(),
        1,
        "tampoco en la repetición, que es donde se vería un segundo duplicado: {:?}",
        second.summary
    );
    assert!(
        lines[0].contains("ya estaba integrado"),
        "y lo dice como estado, no como plan: {}",
        lines[0]
    );

    // ── Estado 3: `--no-modify-path`, donde el plan **es** el estado ──────────────
    let s = Sandbox::new("path-not-needed");
    s.seed_env();
    let exe = s.write_bundle(&s.staging());
    let without_path = runtime
        .block_on(install::install(
            &s.install_env(&exe),
            &install::Options {
                no_modify_path: true,
                ..Sandbox::install_options()
            },
            &Inert,
        ))
        .expect("la instalación con `--no-modify-path` se completa");
    let lines = summary_lines(&without_path.summary);
    assert_eq!(
        lines.len(),
        1,
        "con `--no-modify-path` también una sola línea: {:?}",
        without_path.summary
    );
    assert!(
        lines[0].contains("no se modifica"),
        "y aquí el texto del plan es el del estado, porque no hubo escritura: {}",
        lines[0]
    );
}

// ─── `setup` ──────────────────────────────────────────────────────────────────────

/// Con `--no-setup` no se descarga ningún modelo.
///
/// La primera aserción es la que impide que la prueba sea vacía: se calcula lo que
/// `setup` **habría** hecho contra el mismo almacén, y tiene que haber algo. Sin esa
/// comprobación la prueba pasaría igual con un `--no-setup` que no hiciera nada, que es
/// justo el defecto que la regla prohíbe.
///
/// Además, la raíz de modelos **sigue vacía**: no basta con que el resumen diga
/// `skipped`, tiene que no haber quedado ni un repo ni `xet`.
#[test]
fn no_setup_provisions_nothing() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("no-setup");
    sandbox.seed_env();

    // No vacuidad: hay modelos que provisionar si se pidiera.
    let pending = setup::pending(&ModelStore::new().unwrap());
    let selection = setup::selection();
    assert!(
        !pending.models.is_empty(),
        "hay repos que descargar, así que `--no-setup` omite trabajo real: {:?}",
        pending.models
    );
    assert_eq!(
        pending.models.len(),
        selection.len(),
        "la selección completa está pendiente"
    );

    let exe = sandbox.write_bundle(&sandbox.staging());
    let runtime = runtime::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("la instalación se completa");

    assert_eq!(outcome.status, "installed");
    assert_eq!(
        outcome.models,
        ModelsState::Skipped,
        "el desenlace dice que no se provisionó nada"
    );
    assert_eq!(outcome.models.as_str(), "skipped");
    assert_eq!(
        outcome.lifecycle_error(),
        None,
        "`--no-setup` no produce `reason` de contrato, así que sigue saliendo por \
         `Done` con código 0"
    );
    assert_eq!(
        listing::list(&sandbox.models_dir),
        Vec::<String>::new(),
        "la raíz de modelos sigue vacía: no hay repo ni `xet`"
    );
    assert!(
        !ModelStore::new().unwrap().is_provisioned("opus-mt-es-en"),
        "y nada quedó provisionado en el almacén"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir)
            .expect("se lee el recibo")
            .is_some(),
        "el programa queda instalado, que es lo que `--no-setup` promete"
    );
}

/// Sin `--no-setup`, un fallo de `setup` deja el programa
/// instalado y termina con `setup_failed`.
///
/// El fallo se provoca **sin red real**: se provisionan en el sandbox los repos de la
/// selección salvo uno, y `HF_ENDPOINT` apunta a un puerto local cerrado. Lo único que
/// queda por hacer es descargar ese modelo, y falla de forma independiente de la máquina.
///
/// Se afirman **dos** cosas. El programa instalado se comprueba contra el disco. El
/// `reason` se comprueba sobre [`install::Outcome::lifecycle_error`], que es el punto único
/// donde el motor decide el `reason` de la operación: `setup_failed` con el código `11` de
/// la tabla cerrada, que es lo que la tabla de reasons declara como **éxito parcial** y lo
/// que el cableado convierte en código de salida. Y se afirma también la separación de los
/// dos `reason`, que es lo que `self update` necesita: el de la **operación** es
/// `setup_failed` y el del **fallo de provisión** viaja anidado, y en este caso es
/// `network_error`.
///
/// El resumen en texto se afirma por las dos mitades —el estado de los modelos y el aviso
/// con el motivo de la causa y la instrucción de reintentar—, porque es la otra salida de
/// la misma operación y no puede decir una cosa mientras el envelope dice otra.
#[test]
fn setup_failure_keeps_install() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("setup-failure");
    sandbox.seed_env();
    provision_selection(&sandbox);

    // Un solo pin sin sembrar y el endpoint de HuggingFace en un puerto local cerrado:
    // la descarga falla sin tocar la red real.
    let unseeded = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == UNSEEDED_PIN)
        .expect("el pin sin sembrar existe");
    std::fs::remove_dir_all(sandbox.models_dir.join(repo::repo_dir(unseeded.repo)))
        .expect("se quita el snapshot del pin");
    let closed_port = std::net::TcpListener::bind("127.0.0.1:0")
        .and_then(|l| l.local_addr())
        .expect("se reserva un puerto local")
        .port();
    std::env::set_var("HF_ENDPOINT", format!("http://127.0.0.1:{closed_port}"));

    // No vacuidad del camino de fallo: solo queda por descargar ese pin.
    let pending = setup::pending(&ModelStore::new().unwrap());
    assert_eq!(
        pending.models,
        vec![UNSEEDED_PIN.to_string()],
        "solo ese modelo queda por descargar"
    );

    let exe = sandbox.write_bundle(&sandbox.staging());
    let runtime = runtime::runtime();
    let options = install::Options {
        no_setup: false,
        ..Sandbox::install_options()
    };
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("un fallo de `setup` no es un fallo de la instalación");

    // ── Mitad 1: el programa queda instalado ──────────────────────────────────
    assert_eq!(
        outcome.status, "installed",
        "la instalación termina con éxito"
    );
    let on_disk = receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .expect("el recibo existe: el programa quedó instalado");
    assert_eq!(on_disk.version, "0.24.0");
    for relative in &outcome.receipt.files {
        assert!(
            bundle::place(&sandbox.program_dir, relative).is_file(),
            "{relative} sigue en el directorio de programa"
        );
    }

    // El fallo es un estado del desenlace, y su `reason` es el **de la provisión**, no el
    // de la operación.
    let ModelsState::Failed { cause } = &outcome.models else {
        panic!(
            "se esperaba un fallo de provisión y el estado es {:?}",
            outcome.models
        );
    };
    assert_eq!(outcome.models.as_str(), "failed");
    assert_eq!(
        cause.reason, "network_error",
        "el fallo de provisión conserva su `reason`, que es el que \
         `docs/CLI/commands/SETUP.md` publica para una descarga fallida"
    );
    assert_eq!(
        ExitCode::from_reason(cause.reason).code(),
        20,
        "y el código de la tabla; el código de salida del proceso es el de la \
         operación, no este"
    );

    // ── Mitad 2: termina con `setup_failed` y el código 11 ────────────────────
    let failure = outcome
        .lifecycle_error()
        .expect("`setup_failed` es el reason de este desenlace");
    assert_eq!(
        failure.reason, "setup_failed",
        "el `reason` de la operación es `setup_failed`"
    );
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        11,
        "y el código es `SetupFailed = 11` de la tabla única"
    );
    assert_eq!(
        ExitCode::from_reason("setup_failed").code(),
        11,
        "el código de `setup_failed` sale de la tabla única, sin copia propia"
    );
    // El mensaje dice las dos cosas que promete el resumen: qué no se completó y que
    // basta reintentar con `setup`.
    assert!(
        failure.message.contains("no se completó"),
        "el mensaje dice qué no se completó: {}",
        failure.message
    );
    assert!(
        failure.message.contains("reintentar con setup"),
        "y que basta reintentar con `setup`, que es la otra mitad del aviso: {}",
        failure.message
    );
    assert!(
        failure.message.contains(cause.message.as_str()),
        "y el motivo de la causa viaja dentro, para que el `reason` anidado no se \
         pierda: {}",
        failure.message
    );
    assert!(
        failure
            .message
            .contains(&sandbox.program_dir.display().to_string()),
        "y dónde quedó instalado, que es lo que el usuario necesita saber: {}",
        failure.message
    );

    // El camino de éxito no se ha tocado: un `setup` que no tiene nada que hacer no produce
    // `reason` de contrato, y por eso el cableado sale por `Hecho` y con código 0. La
    // prueba de al lado lo demuestra con la provisión entera, sin atajos.

    // El resumen en texto dice lo mismo que el envelope.
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("Modelos:") && l.contains("failed")),
        "el resumen final dice que los modelos fallaron: {:?}",
        outcome.summary
    );
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("reintentar con setup")),
        "y que basta reintentar con `setup`: {:?}",
        outcome.summary
    );
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("Causa:") && l.contains("network_error")),
        "y nombra la causa, con el `reason` anidado: {:?}",
        outcome.summary
    );

    // Y es reintentable con `setup`: el modelo sigue sin existir, así que un `setup`
    // posterior lo vuelve a pedir.
    assert_eq!(
        setup::pending(&ModelStore::new().unwrap()).models,
        vec![UNSEEDED_PIN.to_string()],
        "y `setup` lo ve pendiente otra vez"
    );
    std::env::remove_var("HF_ENDPOINT");
}

/// **Con `setup` correcto, la operación sale limpia.**
///
/// El camino de éxito no lleva `reason`, y se demuestra con el estado que el motor
/// devuelve cuando no hay nada que provisionar:
/// `AlreadyProvisioned`, sin `reason` de contrato, que es lo que hace que el cableado salga
/// por `Hecho` con código 0 en vez de por veredicto.
///
/// El sandbox tiene los repos de la selección ya provisionados, así que no queda nada que
/// descargar.
#[test]
fn successful_provisioning_has_no_reason() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("setup-ok");
    sandbox.seed_env();
    provision_selection(&sandbox);

    // No vacuidad: `setup` no tiene nada que hacer, y eso es lo que la prueba comprueba.
    let pending = setup::pending(&ModelStore::new().unwrap());
    assert!(
        pending.is_empty(),
        "con los repos ya provisionados no queda nada pendiente: {:?}",
        pending
    );

    let exe = sandbox.write_bundle(&sandbox.staging());
    let runtime = runtime::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &install::Options {
                no_setup: false,
                ..Sandbox::install_options()
            },
            &Inert,
        ))
        .expect("la instalación se completa");

    assert_eq!(outcome.status, "installed");
    assert_eq!(
        outcome.models,
        ModelsState::AlreadyProvisioned,
        "con `setup` correcto el estado es que no había nada que hacer"
    );
    assert_eq!(
        outcome.lifecycle_error(),
        None,
        "y no hay `reason` de contrato, así que el cableado sale por `Done` con \
         código 0 y no por veredicto"
    );
    assert!(
        !outcome
            .summary
            .iter()
            .any(|l| l.contains("Causa:") || l.contains("no se completó")),
        "y el resumen no dice nada de un fallo que no ha habido: {:?}",
        outcome.summary
    );
}

// ─── Desinstalación completa ──────────────────────────────────────────────────────

/// `self uninstall --yes` elimina el programa, la integración de `PATH`
/// y el estado, sin residuo dentro de las raíces de propiedad exclusiva.
///
/// El ciclo es completo: una instalación **real** por el motor, no un recibo escrito a mano, y
/// luego la desinstalación. En Unix eso incluye el enlace y el bloque delimitado del
/// perfil, y se afirma que el resto del perfil no se toca.
///
/// En Windows la mitad de la integración se demuestra en `tests/windows_path.rs`, y no
/// aquí por un motivo concreto: `uninstall::revert_path` **no** recibe la subclave de
/// registro —revierte contra `path_windows::ENV_SUBKEY`, es decir `HKCU\Environment`—, de
/// modo que una prueba que lo invocara sobre una clave de prueba acabaría leyendo y
/// escribiendo el `PATH` real de quien ejecuta la puerta. Aquí la instalación va con
/// `--no-modify-path` y se afirma que el recibo lo dice, que es lo que hace que la
/// reversión sea un no-op en vez de una escritura.
#[test]
fn uninstall_leaves_no_residue() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("uninstall-residue");
    sandbox.seed_env();
    let runtime = runtime::runtime();
    let options = install::Options {
        // En Unix la integración se integra de verdad; en Windows se apaga, y el motivo
        // está en el doc-comment.
        no_modify_path: cfg!(windows),
        ..Sandbox::install_options()
    };

    // ── Instalar de verdad ──────────────────────────────────────────────────────
    // El perfil del sandbox tiene contenido propio **antes** de instalar, que es lo que
    // hace significativa la reversión: `remove_block` solo trunca cuando el final del
    // archivo es exactamente el bloque, así que un bloque al final de un archivo con texto
    // detrás no se quita —por diseño, para no borrar contenido del usuario— y la prueba
    // tiene que reproducir la situación real, que es un perfil que ya existía.
    let profile = sandbox.home.join(".profile");
    if cfg!(unix) {
        support::write(&profile, "# adjusting del usuario\n");
    }
    let exe = sandbox.write_bundle(&sandbox.staging());
    let installed = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("la instalación se completa");
    assert_eq!(installed.status, "installed");
    sandbox.seed_state();
    let receipt = receipt::read_from(&sandbox.program_dir)
        .expect("se lee el recibo")
        .expect("el recibo existe");

    if cfg!(unix) {
        let text = std::fs::read_to_string(&profile).expect("el perfil se lee");
        assert!(
            text.starts_with("# adjusting del usuario\n"),
            "el contenido propio está al principio y no se tocó: {text}"
        );
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "y el bloque delimitado está al final: {text}"
        );
        assert!(
            text.ends_with(avi_lifecycle::path_unix::BLOCK_END),
            "con su marcador de cierre al final del archivo: {text}"
        );
    }
    if cfg!(windows) {
        assert!(
            !receipt.path_integration.modify_path,
            "con `--no-modify-path` el recibo lo dice, y por eso no hay nada \
             que revertir contra `HKCU\\Environment`"
        );
        assert!(receipt.path_integration.registry_entry.is_none());
    }

    // ── Desinstalar ─────────────────────────────────────────────────────────────
    let outcome = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");

    assert_eq!(outcome.status, "uninstalled", "termina en éxito");
    assert!(
        outcome.failed.is_empty(),
        "nada falló: {:?}",
        outcome.failed
    );
    assert!(
        outcome.program_dir_removed,
        "el directorio de programa se borró"
    );

    // Residuo cero en las raíces de propiedad exclusiva.
    assert!(
        !inspect::exists(&sandbox.program_dir),
        "el directorio de programa no queda"
    );
    assert!(
        !inspect::exists(&sandbox.data_dir),
        "la raíz de datos no queda, voces de fábrica incluidas: el programa \
         sin el programa nadie las re-materializa"
    );
    assert!(
        !inspect::exists(&sandbox.models_dir),
        "la raíz de modelos no queda"
    );
    // El staging lleva el token de este proceso, vivo, y la desinstalación no lo barre:
    // lo borra quien lo creó, como el instalador de arranque al terminar. Antes de que
    // lo borre, lo único que queda en el padre del programa es ese staging: ni
    // aparcados ni bloqueo ni compuerta.
    let staging = sandbox.staging();
    let staging_name = format!(
        "{}/",
        staging
            .file_name()
            .expect("el staging tiene nombre")
            .to_string_lossy()
    );
    let beside_program = opt_entries(&sandbox);
    assert!(
        !beside_program.is_empty() && beside_program.iter().all(|e| e.starts_with(&staging_name)),
        "el motor no deja nada junto al programa salvo el staging de un dueño vivo: \
         {beside_program:?}"
    );
    std::fs::remove_dir_all(&staging).expect("el dueño borra su staging");
    assert_eq!(
        opt_entries(&sandbox),
        Vec::<String>::new(),
        "el padre del programa queda vacío: ni staging, ni aparcados, ni bloqueo"
    );
    assert!(
        !inspect::exists(&sandbox.cache_dir) && !inspect::exists(&sandbox.app_dir),
        "los contenedores de caché y raíz desaparecen con sus hojas, scratch incluido"
    );

    if cfg!(unix) {
        let link = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        assert!(!inspect::exists(&link), "el enlace del `PATH` se retira");
        assert!(outcome.path_reverted, "y el motor lo dice");
        let text = std::fs::read_to_string(&profile).expect("el perfil se lee");
        assert_eq!(
            text, "# adjusting del usuario\n",
            "el bloque delimitado se quita y el resto del perfil no se toca"
        );
    }
}

// ─── Desinstalación con `--keep-data` ─────────────────────────────────────────────

/// `self uninstall --keep-data` conserva modelos, voces y habla
/// sintetizada.
///
/// Se afirma también el reverso, que es la mitad que suele olvidarse: la configuración,
/// los logs y el pidfile **sí** caen, porque son estado de ejecución y no datos de
/// usuario. Un `--keep-data` que conservara todo no sería una operación de desinstalación.
#[test]
fn keep_data_preserves_state() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("keep-data");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());

    // Punto de partida: los tres conjuntos están.
    assert!(inspect::exists(&sandbox.models_dir.join("xet")));
    assert!(inspect::exists(
        &sandbox.data_dir.join("voices").join("mine")
    ));
    assert!(inspect::exists(
        &sandbox.data_dir.join("speech").join("default")
    ));

    let outcome = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                keep_data: true,
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !inspect::exists(&sandbox.program_dir),
        "el programa sí se va"
    );

    // Lo que se conserva.
    assert!(
        inspect::exists(&sandbox.models_dir.join("xet"))
            && inspect::exists(
                &sandbox.models_dir.join(repo::repo_dir(
                    MODEL_REVISIONS
                        .iter()
                        .find(|pin| pin.name == "opus-mt-es-en")
                        .expect("el modelo de traducción está fijado")
                        .repo
                ))
            ),
        "los modelos se quedan: {:?}",
        outcome.preserved
    );
    for voice in ["default", "ryan", "mine"] {
        assert!(
            inspect::exists(&sandbox.data_dir.join("voices").join(voice)),
            "la voz {voice} se queda"
        );
    }
    for voice in ["default", "mine"] {
        assert!(
            inspect::exists(&sandbox.data_dir.join("speech").join(voice)),
            "la locución de {voice} se queda"
        );
    }
    for reason in ["modelos", "voces", "habla"] {
        assert!(
            outcome.preserved.iter().any(|p| p.reason.contains(reason)),
            "`{reason}` se anuncia como conservado, con su motivo: {:?}",
            outcome.preserved
        );
    }

    // Y lo que no.
    for state in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !inspect::exists(&sandbox.data_dir.join(state)),
            "`{state}` sí se borra: es estado de ejecución, no datos"
        );
    }
    assert!(
        inspect::exists(&sandbox.home.join(".cargo")),
        "y lo que no es del producto, por supuesto"
    );
}

// ─── Confirmación sin terminal ────────────────────────────────────────────────────

/// Sin terminal y sin `--yes`, toda operación destructiva termina con
/// `confirmation_required` y no borra nada.
///
/// La ausencia de terminal **no se puede fingir en el proceso**: `uninstall::run` y
/// `cleanup::run` leen `stdin_is_terminal` del stdin real con `std::io::IsTerminal`, y no
/// es un parámetro. Bajo una consola interactiva el stdin **es** una terminal, así que la
/// prueba se ejecuta en un proceso hijo del mismo binario con la entrada redirigida a la
/// null. El padre es el que exige que el hijo termine en éxito, así que el trabajo no se
/// omite nunca, y las dos operaciones destructivas se ejecutan en el hijo:
/// `self uninstall` y `cleanup --all`.
///
/// La comparación de disco ignora el archivo de bloqueo a propósito: la recuperación lo
/// crea antes del plan y de la confirmación, y «no borra nada» no es «no escribe el
/// mecanismo que serializa las operaciones».
#[test]
fn no_tty_without_yes_refuses() {
    let _guard = process_env::exclusively();

    if no_terminal::is_child_without_terminal() {
        child_without_terminal();
        return;
    }

    let sandbox = Sandbox::new("no-terminal");
    sandbox.seed_env();
    sandbox.seed_state();
    sandbox.install_registered(PathIntegration::none());
    let before = state_without_lock(&sandbox);

    let output =
        no_terminal::run_without_terminal("no_tty_without_yes_refuses", "no-terminal", &sandbox);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "el proceso sin terminal terminó con {:?}.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );

    let report_lines = no_terminal::read_report_lines(&output.stdout);
    for operation in ["uninstall", "cleanup"] {
        let line = report_lines
            .iter()
            .find(|l| l.starts_with(operation))
            .unwrap_or_else(|| {
                panic!(
                    "`{operation}` no informó de su desenlace: {report_lines:?}\n\
                     --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                )
            });
        let (reason, code) = line
            .trim_start_matches(operation)
            .trim_start_matches('=')
            .split_once('/')
            .unwrap_or_else(|| panic!("informe mal formado: {line}"));
        assert_eq!(
            reason, "confirmation_required",
            "`{operation}` sin terminal y sin `--yes` se niega"
        );
        assert_eq!(code, "2", "`{operation}` devuelve el error de uso");
    }

    // Y nada se borró. El hijo lo afirma también; aquí se comprueba contra el disco.
    assert_eq!(state_without_lock(&sandbox), before, "no se borró nada");
    assert!(
        inspect::exists(&sandbox.program_dir),
        "el directorio de programa sigue"
    );
    assert!(
        inspect::exists(&sandbox.data_dir.join("voices").join("mine")),
        "el estado de usuario sigue"
    );
    assert!(
        inspect::exists(&sandbox.models_dir.join("models--other--tool")),
        "los modelos siguen"
    );
}

/// Cuerpo del proceso hijo: ejecuta las dos operaciones destructivas sin
/// terminal y sin `--yes`, y afirma que se niegan y que no borran nada.
///
/// El sandbox se reconstruye desde la raíz y la etiqueta que le pasó el padre, y **no se
/// borra al salir**: el padre tiene que encontrar ese mismo disco para comprobarlo.
fn child_without_terminal() {
    let root = PathBuf::from(std::env::var(no_terminal::VAR_ROOT).expect("el hijo recibe la raíz"));
    let tag = std::env::var(no_terminal::VAR_TAG).expect("el hijo recibe la etiqueta");
    let mut sandbox = Sandbox::at(&tag, root, false);
    sandbox.remove_on_drop = false;
    sandbox.seed_env();
    let before = state_without_lock(&sandbox);

    let receipt = receipt::read_from(&sandbox.program_dir)
        .expect("el hijo lee el recibo")
        .expect("el recibo existe");
    let runtime = runtime::runtime();

    // 1. `self uninstall` sin `--yes`.
    let error = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options::default(),
            &Now,
            &Inert,
        ))
        .expect_err("`self uninstall` sin terminal y sin `--yes` se niega");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("el fallo es un `LifecycleError`");
    assert_eq!(failure.reason, "confirmation_required");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        2,
        "error de uso"
    );
    no_terminal::report_line(&format!(
        "uninstall={}/{}",
        failure.reason,
        ExitCode::from_reason(failure.reason).code()
    ));

    // 2. `cleanup --all` sin `--yes`.
    let error = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                all: true,
                ..Default::default()
            },
            &Inert,
        ))
        .expect_err("`cleanup` sin terminal y sin `--yes` se niega");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("el fallo es un `LifecycleError`");
    assert_eq!(failure.reason, "confirmation_required");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        2,
        "error de uso"
    );
    no_terminal::report_line(&format!(
        "cleanup={}/{}",
        failure.reason,
        ExitCode::from_reason(failure.reason).code()
    ));

    // Y el disco está intacto. Sin `--yes` no se puede haber borrado nada, y esta
    // comprobación es la que convierte la negativa en una garantía.
    assert_eq!(
        state_without_lock(&sandbox),
        before,
        "ninguna de las dos operaciones borró nada"
    );
}

// ─── `--dry-run` ──────────────────────────────────────────────────────────────────

/// `--dry-run` en cualquier operación destructiva lista rutas y tamaños sin modificar el
/// disco.
///
/// Se afirma en las dos operaciones destructivas, y en cada una lo mismo: la lista tiene
/// **tamaños medidos** y no cero, el disco queda idéntico —comparado con `snapshot`,
/// tamaños incluidos— y no aparece ni el archivo de bloqueo. Y se añade la comprobación
/// que convierte la simulación en una promesa: la ejecución real borra exactamente lo que
/// la simulación anunció. Un `--dry-run` que anunciara una lista y la ejecución borrara
/// otra habría cumplido la letra de la regla y no su intención.
#[test]
fn dry_run_does_not_touch_disk() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("dry-run");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());
    let runtime = runtime::runtime();

    // ── `self uninstall --dry-run` ──────────────────────────────────────────────
    let before = sandbox.snapshot();
    let options = uninstall::Options {
        dry_run: true,
        assume_yes: true,
        ..Default::default()
    };
    let plan = uninstall::compose_plan(
        &sandbox.roots(),
        Some(&receipt),
        &sandbox.program_dir,
        &options,
    );
    let entries = plan.entries();
    assert!(
        !entries.is_empty(),
        "el plan de desinstalación no está vacío"
    );
    // Los destinos de estado llevan el tamaño recursivo de la ruta, que es la cifra que
    // la tabla pide listar. Se afirma que **coincide con la medida** y no solo que es
    // positiva, porque un cero fijo pasaría la comprobación débil y no sería un tamaño.
    for dest in &plan.state.targets {
        assert_eq!(
            dest.size,
            cleanup::path_size(&dest.path),
            "el tamaño de {} es el medido",
            dest.path.display()
        );
        assert!(
            dest.size > 0,
            "{} tiene contenido y así se anuncia",
            dest.path.display()
        );
    }
    // El directorio de programa se lista como ruta. Su cifra es la longitud de la
    // entrada —que en un directorio es 0 en Windows y el tamaño del bloque en Unix—, así
    // que afirmar que es positiva sería afirmar algo que el enunciado no pide y que la
    // plataforma decide.
    assert!(
        entries.iter().any(|e| e.path == sandbox.program_dir),
        "el directorio de programa aparece en el plan: {:?}",
        entry_paths(&entries)
    );
    assert!(
        plan.program_dir.as_ref().is_some_and(|e| e.size.is_some()),
        "y con su tamaño medido"
    );
    assert!(
        cleanup::path_size(&sandbox.program_dir) > 0,
        "el directorio de programa tiene contenido de verdad"
    );
    assert!(
        entries.iter().any(|e| e.path == sandbox.models_dir),
        "y la raíz de modelos, que es el otro destino propio"
    );

    let simulation = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &options,
            &Now,
            &Inert,
        ))
        .expect("la simulación de desinstalación se ejecuta");
    assert!(simulation.dry_run, "el desenlace lo dice");
    let planned_removed: Vec<String> = entry_paths(&entries);
    for path in &planned_removed {
        assert!(
            simulation.removed.contains(path),
            "la simulación anuncia {path}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        before,
        "`self uninstall --dry-run` no modifica el disco, ni siquiera \
         con el archivo de bloqueo"
    );

    // ── `cleanup --dry-run` ─────────────────────────────────────────────────────
    let options = cleanup::Options {
        all: true,
        dry_run: true,
        ..with_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &options);
    assert!(!plan.is_empty(), "el plan de limpieza no está vacío");
    for dest in &plan.targets {
        assert!(
            dest.size > 0,
            "{} se lista con su tamaño medido",
            dest.path.display()
        );
    }
    let announced = paths(&plan);
    let simulated = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("la simulación de limpieza se ejecuta");
    assert!(simulated.dry_run);
    for path in &announced {
        assert!(
            simulated.removed.contains(path),
            "la simulación de `cleanup` anuncia {path}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        before,
        "`cleanup --dry-run` no modifica el disco"
    );

    // ── Y ahora sí, la ejecución real borra lo mismo que se anunció ─────────────
    let real = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                all: true,
                ..with_yes()
            },
            &Inert,
        ))
        .expect("la limpieza real se ejecuta");
    let removed: Vec<String> = real
        .removed
        .iter()
        .filter(|r| announced.contains(r))
        .cloned()
        .collect();
    assert_eq!(
        removed, announced,
        "la ejecución borra exactamente lo que la simulación anunció"
    );
    assert!(
        !inspect::exists(&sandbox.models_dir),
        "y lo que se anunció borrado, está borrado"
    );
}

// ─── Desinstalación idempotente ───────────────────────────────────────────────────

/// Repetir `self uninstall` en un sistema ya limpio termina con éxito
/// (`not_installed`).
///
/// Se cubren las dos formas de «ya limpio»: la repetición sobre el mismo sandbox, que es
/// la que un usuario llega por accidente, y el sistema en el que nunca se instaló, que es
/// la que llega por un `cleanup --all` previo. En los dos casos el desenlace es **éxito**,
/// no un error: `not_installed` es un desenlace y la tabla lo clasifica así para
/// `self uninstall`.
#[test]
fn uninstall_is_idempotent() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();

    // ── Instalar, desinstalar, desinstalar ──────────────────────────────────────
    let sandbox = Sandbox::new("idempotent");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());

    let first = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la primera desinstalación se ejecuta");
    assert_eq!(first.status, "uninstalled");
    assert!(first.program_dir_removed);
    assert!(first.failed.is_empty(), "{:?}", first.failed);

    let after = sandbox.snapshot();
    let second = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(None, Channel::Unmanaged),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la segunda no es un error");
    assert_eq!(
        second.status, "not_installed",
        "repetir sobre un sistema limpio termina en éxito con `not_installed`"
    );
    assert!(
        second.removed.is_empty(),
        "y no borra nada: {:?}",
        second.removed
    );
    assert_eq!(
        sandbox.snapshot(),
        after,
        "el disco no cambia en la repetición"
    );

    // ── Y el sistema en el que nunca se instaló ────────────────────────────────
    // El arnés **no** crea la raíz de datos ni la de modelos: en un sistema donde nunca
    // se instaló, esas raíces no existen, y es su ausencia la que hace que el desenlace
    // sea `not_installed` y no una desinstalación vacía.
    let clean = Sandbox::new("idempotent-clean");
    clean.seed_env();
    assert!(!inspect::exists(&clean.program_dir));
    let uninstalled = runtime
        .block_on(uninstall::run(
            &clean.env_uninstall(None, Channel::Unmanaged),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("en un sistema sin instalación tampoco es un error");
    assert_eq!(
        uninstalled.status, "not_installed",
        "sin recibo, sin estado y sin directorio de programa"
    );
    assert!(uninstalled.removed.is_empty());
    assert!(!uninstalled.program_dir_removed);
}

// ─── Categorías de `cleanup` ──────────────────────────────────────────────────────

/// Cada categoría de `cleanup` borra solo su alcance, y `cleanup` sin
/// categoría termina con `usage_error`.
///
/// Cada categoría se **ejecuta** en su propio sandbox, no solo se planifica: el defecto
/// que se vigila es que el plan y el ejecutor no coincidan, y un plan correcto
/// con un ejecutor que borra de más solo se ve ejecutando. Y lo que sobrevive se afirma
/// contra el disco, para que «borró solo su alcance» sea una afirmación sobre el resultado
/// y no sobre una lista de control.
#[test]
fn cleanup_scope_and_usage_error() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();

    // ── El gate: sin categoría, `usage_error` y nada borrado ────────────────────
    let sandbox = Sandbox::new("cleanup-gate");
    sandbox.seed_env();
    sandbox.seed_state();
    let before = sandbox.snapshot();
    let error = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Inert,
        ))
        .expect_err("sin categoría es un error");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("el fallo declara un `reason`");
    assert_eq!(failure.reason, "usage_error");
    assert_eq!(
        ExitCode::from_reason(failure.reason).code(),
        2,
        "error de uso"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "y no borra nada, ni siquiera el barrido transversal"
    );

    // ── Cada categoría, en su sandbox ──────────────────────────────────────────
    // `--model`: la raíz de modelos entera y nada del estado.
    let s = Sandbox::new("cleanup-model");
    s.seed_env();
    s.seed_state();
    s.install_registered(PathIntegration::none());
    run_category(
        &s,
        &runtime,
        cleanup::Options {
            model: true,
            ..with_yes()
        },
    );
    assert!(!inspect::exists(&s.models_dir), "--model borra los modelos");
    assert!(
        inspect::exists(&s.data_dir.join("voices").join("mine")),
        "--model no toca las voces"
    );
    assert!(
        inspect::exists(&s.data_dir.join("config.json")),
        "--model no toca la configuración"
    );
    assert!(
        inspect::exists(&s.program_dir),
        "--model nunca toca el programa: eso es `self uninstall`"
    );

    // `--voices`: las voces de usuario y el arrastre de su habla.
    let s = Sandbox::new("cleanup-voices");
    s.seed_env();
    s.seed_state();
    s.install_registered(PathIntegration::none());
    run_category(
        &s,
        &runtime,
        cleanup::Options {
            voices: true,
            ..with_yes()
        },
    );
    assert!(
        !inspect::exists(&s.data_dir.join("voices").join("mine")),
        "--voices borra la voz de usuario"
    );
    assert!(
        !inspect::exists(&s.data_dir.join("speech").join("mine")),
        "y la locución que arrastra"
    );
    assert!(
        inspect::exists(&s.data_dir.join("voices").join("default")),
        "las voces de fábrica no se borran: van embebidas"
    );
    assert!(
        inspect::exists(&s.data_dir.join("speech").join("default")),
        "ni sus locuciones"
    );
    assert!(
        inspect::exists(&s.models_dir),
        "--voices no toca los modelos"
    );
    assert!(
        inspect::exists(&s.data_dir.join("config.json")),
        "ni la configuración, que es de --all"
    );

    // `--synthetic-speech`: la raíz de habla entera, `default` incluida.
    let s = Sandbox::new("cleanup-speech");
    s.seed_env();
    s.seed_state();
    s.install_registered(PathIntegration::none());
    run_category(
        &s,
        &runtime,
        cleanup::Options {
            synthetic_speech: true,
            ..with_yes()
        },
    );
    assert!(
        !inspect::exists(&s.data_dir.join("speech")),
        "--synthetic-speech borra la raíz de habla entera"
    );
    assert!(
        inspect::exists(&s.data_dir.join("voices").join("default")),
        "no toca las voces"
    );
    assert!(inspect::exists(&s.models_dir), "no toca los modelos");

    // `--all`: la unión más configuración, logs y estado del daemon; nunca el programa.
    let s = Sandbox::new("cleanup-all");
    s.seed_env();
    s.seed_state();
    s.install_registered(PathIntegration::none());
    run_category(
        &s,
        &runtime,
        cleanup::Options {
            all: true,
            ..with_yes()
        },
    );
    assert!(!inspect::exists(&s.models_dir), "--all borra los modelos");
    assert!(
        !inspect::exists(&s.data_dir.join("voices").join("mine")),
        "y las voces de usuario"
    );
    assert!(!inspect::exists(&s.data_dir.join("speech")));
    for state in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !inspect::exists(&s.data_dir.join(state)),
            "--all borra {state}"
        );
    }
    assert!(
        inspect::exists(&s.program_dir),
        "--all nunca borra el programa"
    );
}

// ─── Recursos compartidos ─────────────────────────────────────────────────────────

/// Ninguna operación borra recursos compartidos: una caché HF
/// configurada por el usuario —salvo los repos propios—, `~/.cargo` o sccache.
///
/// La caché compartida se monta de verdad, con `HF_HUB_CACHE` y **sin** `AVI_CACHE_DIR`,
/// que es la única forma de que `avi-store` la reconozca como compartida: la variable de
/// reubicación de la aplicación tiene precedencia y su presencia volvería exclusiva una
/// raíz que el test declara compartida. Los casos se afirman contra el disco después de
/// la operación, no contra el plan.
#[test]
fn shared_resources_survive() {
    let _guard = process_env::exclusively();
    let runtime = runtime::runtime();

    // ── La raíz compartida se reconoce como tal ────────────────────────────────
    let sandbox = Sandbox::new_shared("shared-cleanup");
    sandbox.seed_env();
    assert!(
        avi_shared::paths::models_root_is_shared(),
        "`HF_HUB_CACHE` convierte la raíz en compartida"
    );
    assert_eq!(
        avi_shared::paths::models_cache_dir().unwrap(),
        sandbox.models_dir,
        "y `avi-shared` resuelve la misma ruta que el sandbox"
    );
    assert!(
        cleanup::is_shared_models_root(&sandbox.models_dir),
        "el planificador la reconoce como compartida"
    );
    assert!(sandbox.roots().models_shared);
    assert_eq!(
        sandbox.models_dir.file_name(),
        Some(std::ffi::OsStr::new("hub")),
        "y es el directorio `hub` del sandbox, no una ruta de la máquina"
    );

    sandbox.seed_state();
    let own_temp = seed_orphan_scratch(&sandbox);

    // Lo que es atribuible a la aplicación, y lo que nunca lo es.
    let (repo, rev) = MODEL_REVISIONS
        .iter()
        .find(|pin| pin.name == "opus-mt-es-en")
        .map(|pin| (pin.repo.to_string(), pin.revision.to_string()))
        .expect("el repo de traducción está fijado");
    let our_repo = sandbox.models_dir.join(repo::repo_dir(&repo));
    let our_snapshot = our_repo.join("snapshots").join(&rev);
    let our_lock = sandbox
        .models_dir
        .join(".locks")
        .join(repo::repo_dir(&repo));
    let foreign = sandbox.models_dir.join("models--other--tool");
    let foreign_lock = sandbox
        .models_dir
        .join(".locks")
        .join("models--other--tool");
    let xet = sandbox.models_dir.join("xet");
    let locks = sandbox.models_dir.join(".locks");
    let cargo = sandbox.home.join(".cargo");
    let sccache_home = sandbox.home.join(".cache").join("sccache");
    let sccache_temp = sandbox.temp_root.join("sccache");
    for required in [
        &our_snapshot,
        &our_lock,
        &foreign,
        &foreign_lock,
        &xet,
        &locks,
        &cargo,
        &sccache_home,
        &sccache_temp,
    ] {
        assert!(
            inspect::exists(required),
            "el punto de partida existe: {}",
            required.display()
        );
    }

    // ── `cleanup --model` ───────────────────────────────────────────────────────
    let options = cleanup::Options {
        model: true,
        ..with_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &options);
    let planned_removed = paths(&plan);

    for our in [&our_repo, &our_lock] {
        assert!(
            planned_removed.contains(&our.display().to_string()),
            "lo atribuible a la aplicación está en el plan: {planned_removed:?}"
        );
    }
    for forbidden in [&xet, &locks, &foreign, &sandbox.models_dir] {
        assert!(
            !planned_removed.contains(&forbidden.display().to_string()),
            "{} no puede estar en el plan: {planned_removed:?}",
            forbidden.display()
        );
    }
    // La protección de lo compartido no es solo no borrarlo: el plan tiene que decirlo.
    for shared in [&sandbox.models_dir, &xet, &locks] {
        assert!(
            plan.preserved
                .iter()
                .any(|p| p.path.as_path() == shared.as_path()),
            "{} se anuncia como compartido: {:?}",
            shared.display(),
            plan.preserved
        );
    }
    // Ninguna ruta del plan sale de las raíces declaradas.
    for dest in &plan.targets {
        assert!(
            dest.path.starts_with(&sandbox.models_dir),
            "el destino {} sale de la raíz de modelos",
            dest.path.display()
        );
    }

    let real = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("`cleanup --model` se ejecuta");
    assert!(real.failed.is_empty(), "{:?}", real.failed);
    assert_eq!(
        real.removed, planned_removed,
        "el plan y la ejecución coinciden bajo raíz compartida"
    );
    assert!(
        !inspect::exists(&our_snapshot) && !inspect::exists(&our_repo),
        "el repo propio sí se borró"
    );
    assert!(
        !inspect::exists(&our_lock),
        "el lock del repo propio sí se borró"
    );
    assert!(inspect::exists(&xet), "`xet` sobrevive");
    assert!(inspect::exists(&locks), "el `.locks` completo sobrevive");
    assert!(
        inspect::exists(&foreign_lock),
        "el lock de otra herramienta sobrevive"
    );
    assert!(
        inspect::exists(&foreign),
        "el repo de otra herramienta sobrevive"
    );
    assert_eq!(
        std::fs::read_to_string(foreign.join("other.safetensors")).ok(),
        Some("ajeno".to_string()),
        "con su contenido intacto"
    );
    assert!(
        inspect::exists(&sandbox.models_dir),
        "la raíz compartida no se borra entera"
    );
    assert!(
        inspect::exists(&sandbox.data_dir.join("voices").join("mine")),
        "`--model` no toca el estado de usuario"
    );
    // El barrido decide por la identidad del propietario, no por directorio.
    assert!(
        !inspect::exists(&own_temp),
        "el scratch huérfano sí se barre, y se anuncia: {:?}",
        real.swept
    );
    assert!(
        real.swept.contains(&own_temp.display().to_string()),
        "y aparece en la lista de barrido"
    );
    for shared in [&cargo, &sccache_home, &sccache_temp] {
        assert!(
            inspect::exists(shared),
            "{} sobrevive al barrido",
            shared.display()
        );
    }

    // ── `cleanup --all` ─────────────────────────────────────────────────────────
    let s = Sandbox::new_shared("shared-all");
    s.seed_env();
    s.seed_state();
    s.install_registered(PathIntegration::none());
    runtime
        .block_on(cleanup::run(
            &s.roots(),
            &cleanup::Options {
                all: true,
                ..with_yes()
            },
            &Inert,
        ))
        .expect("`cleanup --all` se ejecuta");
    assert!(
        inspect::exists(&s.models_dir.join("models--other--tool")),
        "`--all` no borra el repo ajeno"
    );
    assert!(
        inspect::exists(&s.models_dir.join("xet")) && inspect::exists(&s.models_dir.join(".locks")),
        "ni `xet` ni el `.locks` completo"
    );
    assert!(
        inspect::exists(&s.models_dir),
        "la raíz compartida sigue ahí"
    );
    assert!(
        !inspect::exists(&s.data_dir.join("voices").join("mine")),
        "y el estado de usuario, que sí es nuestro, sí cae"
    );
    assert!(
        inspect::exists(&s.program_dir),
        "el programa sobrevive a `cleanup`"
    );
    assert!(
        inspect::exists(&s.home.join(".cargo")) && inspect::exists(&s.temp_root.join("sccache")),
        "y los compartidos del entorno"
    );

    // ── `self uninstall` ────────────────────────────────────────────────────────
    let s = Sandbox::new_shared("shared-uninstall");
    s.seed_env();
    s.seed_state();
    let receipt = s.install_registered(PathIntegration::none());
    let outcome = runtime
        .block_on(uninstall::run(
            &s.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");
    assert_eq!(outcome.status, "uninstalled");
    assert!(!inspect::exists(&s.program_dir), "el programa sí se borra");
    assert!(
        !inspect::exists(&s.data_dir),
        "y la raíz de datos, que es exclusiva"
    );
    for shared in [
        s.models_dir.join("models--other--tool"),
        s.models_dir.join("xet"),
        s.models_dir.join(".locks"),
        s.models_dir.clone(),
        s.home.join(".cargo"),
        s.temp_root.join("sccache"),
    ] {
        assert!(
            inspect::exists(&shared),
            "{} sobrevive a la desinstalación",
            shared.display()
        );
    }
    assert!(
        outcome
            .preserved
            .iter()
            .any(|p| p.path == s.models_dir && p.reason.contains("compartid")),
        "y la caché compartida se anuncia como conservada: {:?}",
        outcome.preserved
    );

    // `--keep-data` no toca la caché compartida en absoluto.
    let s = Sandbox::new_shared("shared-keep");
    s.seed_env();
    s.seed_state();
    let receipt = s.install_registered(PathIntegration::none());
    runtime
        .block_on(uninstall::run(
            &s.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                keep_data: true,
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación con `--keep-data` se ejecuta");
    assert!(!inspect::exists(&s.program_dir), "el programa sí se va");
    for shared in [
        s.models_dir
            .join("models--other--tool")
            .join("other.safetensors"),
        s.models_dir.join("xet").join("shard"),
        s.models_dir
            .join(".locks")
            .join("models--other--tool")
            .join("lock"),
    ] {
        assert!(
            inspect::exists(&shared),
            "`--keep-data` no toca la caché compartida: {}",
            shared.display()
        );
    }
    assert!(
        s.snapshot()
            .iter()
            .any(|(path, _)| path.starts_with("hub/")),
        "y sigue con el contenido que tenía"
    );

    // ── El contraste: con raíz exclusiva, `xet` y `.locks` sí son nuestros ──────
    let s = Sandbox::new("exclusive-models");
    s.seed_env();
    s.seed_state();
    assert!(
        !avi_shared::paths::models_root_is_shared(),
        "sin `HF_HUB_CACHE` la raíz es exclusiva"
    );
    assert!(
        !cleanup::is_shared_models_root(&s.models_dir),
        "y el planificador lo sabe"
    );
    runtime
        .block_on(cleanup::run(
            &s.roots(),
            &cleanup::Options {
                model: true,
                ..with_yes()
            },
            &Inert,
        ))
        .expect("`--model` sobre la raíz exclusiva se ejecuta");
    assert!(
        !inspect::exists(&s.models_dir),
        "en la raíz exclusiva `--model` borra el directorio entero, `xet` y \
         `.locks` incluidos, porque son de la aplicación"
    );
}

// ─── El simulacro nunca pide confirmación ─────────────────────────────────────────

/// El simulacro de `self uninstall` nunca pide confirmación: con
/// `--dry-run` y sin `--yes` termina en éxito con `planned`, con el recibo del
/// plan y sin modificar el disco.
///
/// Sin la salida temprana del simulacro, `run` llegaría a la confirmación y, sin
/// terminal y sin `--yes`, fallaría con `confirmation_required`; que esta llamada se
/// ejecute ya demuestra que el simulacro no pregunta. El patrón de sandbox es el de las
/// pruebas de `--dry-run` de este archivo.
#[test]
fn uninstall_dry_run_needs_no_confirmation() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("dry-run-without-confirmation");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());
    let before = sandbox.snapshot();

    // Sin `--yes` a propósito: si el simulacro preguntara, esto fallaría.
    let outcome = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options {
                dry_run: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("el simulacro sin `--yes` se ejecuta sin pedir confirmación");
    assert_eq!(
        outcome.status, "planned",
        "el simulacro se lee como un plan"
    );
    assert_eq!(
        outcome.planned, outcome.removed,
        "el recibo del plan es el que se anuncia"
    );
    assert!(
        !outcome.removed.is_empty(),
        "el plan no está vacío: hay instalación registrada"
    );
    assert!(
        !outcome.path_reverted,
        "el recibo no integra el `PATH`, así que nada se prevé revertir"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "el simulacro no modifica el disco"
    );

    // Y con un recibo que dice haber integrado el `PATH`, el simulacro no promete una
    // reversión que no tocaría nada: la marca del recibo dice que se integró, no que
    // quede algo que revertir, y aquí no queda ni enlace, ni bloque, ni entrada de
    // registro.
    let mut with_path = receipt.clone();
    with_path.path_integration.modify_path = true;
    let preview = uninstall::simulate(
        &sandbox.roots(),
        Some(&with_path),
        &sandbox.program_dir,
        &sandbox.home,
        &uninstall::Options {
            dry_run: true,
            ..Default::default()
        },
    );
    assert!(
        !preview.path_reverted,
        "sin artefactos de integración la ejecución real no tocaría nada, y el \
         simulacro no dice lo contrario"
    );
    assert_eq!(
        preview.planned, preview.removed,
        "el recibo del plan es el que se anuncia"
    );
    assert_eq!(
        sandbox.snapshot(),
        before,
        "la previsión tampoco modifica el disco"
    );
}

/// El simulacro y la ejecución real publican el mismo `path_reverted`, en las dos
/// direcciones.
///
/// El valor sale de un único predicado —[`uninstall::path_would_be_reverted`], que además
/// de la marca del recibo mira si existe de verdad lo que se revertiría—, y no de dos
/// implementaciones paralelas que podían divergir. Se afirma en las dos direcciones porque
/// un plan veraz no puede mentir en ninguna:
///
/// - **Con bloque delimitado**: el simulacro dice `true` y la ejecución también, porque
///   el bloque está y se quita.
/// - **Sin nada que revertir**: el simulacro dice `false` y la ejecución también. Aquí el
///   `false` del simulacro es la mitad difícil: la marca `modify_path` del recibo es
///   `true`, así que un simulacro que la leyera afirmaría una reversión que no ocurre.
///
/// El bloque delimitado se usa porque es el artefacto de integración que existe en las dos
/// plataformas —el enlace es de Unix y la entrada de registro es de Windows—, así que la
/// equivalencia se afirma en todas.
#[test]
fn uninstall_path_reverted_matches_the_real_run() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("revert-with-block");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());

    let bin_dir = sandbox.bin_dir.clone();
    let profile = sandbox.home.join(".profile");
    let with_block = {
        // El enlace del recibo hace que `receipt_bin_dir` deduzca el `bin_dir` de su
        // directorio padre, así que el bloque se escribe con ese mismo valor.
        let link = bin_dir.join("avi-voice-interconnector");
        let mut copy = receipt.clone();
        copy.path_integration = PathIntegration::unix(link, vec![profile.clone()]);
        avi_lifecycle::path_unix::write_block(&profile, &bin_dir, &sandbox.home)
            .expect("se escribe el bloque delimitado");
        copy
    };
    let without_block = {
        let mut integration = with_block.path_integration.clone();
        // El recibo dice que integró el `PATH`, pero el bloque no está en el disco: es el
        // caso en que la marca dice una cosa y el disco otra.
        integration.profile_blocks = Some(vec![sandbox.home.join("never-existed")]);
        let mut copy = with_block.clone();
        copy.path_integration = integration;
        copy
    };

    let preview_of = |r: &InstallReceipt| {
        uninstall::simulate(
            &sandbox.roots(),
            Some(r),
            &sandbox.program_dir,
            &sandbox.home,
            &uninstall::Options {
                dry_run: true,
                assume_yes: true,
                ..Default::default()
            },
        )
    };
    assert!(
        preview_of(&with_block).path_reverted,
        "con el bloque en el disco el simulacro prevé la reversión"
    );
    assert!(
        !preview_of(&without_block).path_reverted,
        "sin bloque, aunque el recibo diga `modify_path`, no se prevé revertir nada"
    );

    // Y ahora la ejecución real, con el bloque presente: el mismo `true`.
    let preview = preview_of(&with_block);
    let real = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&with_block), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación real se ejecuta");
    assert_eq!(
        preview.path_reverted, real.path_reverted,
        "el simulacro y la ejecución real dicen lo mismo"
    );
    assert!(
        real.path_reverted,
        "y aquí los dos dicen que sí, porque el bloque estaba"
    );
    assert_eq!(
        std::fs::read_to_string(&profile).expect("el perfil se lee"),
        "",
        "el bloque delimitado se quita de verdad"
    );

    // Y en la dirección contraria, sin nada que revertir: los dos dicen `false`.
    let sandbox = Sandbox::new("revert-without-block");
    sandbox.seed_env();
    sandbox.seed_state();
    let bare = sandbox.install_registered(PathIntegration::none());
    let mut bare_integration = bare.path_integration.clone();
    bare_integration.modify_path = true;
    let bare = InstallReceipt {
        path_integration: bare_integration,
        ..bare
    };
    let preview = uninstall::simulate(
        &sandbox.roots(),
        Some(&bare),
        &sandbox.program_dir,
        &sandbox.home,
        &uninstall::Options {
            dry_run: true,
            assume_yes: true,
            ..Default::default()
        },
    );
    let real = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&bare), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Now,
            &Inert,
        ))
        .expect("la desinstalación real sin integración se ejecuta");
    assert!(
        !preview.path_reverted && !real.path_reverted,
        "la marca `modify_path` sin artefactos no invierte nada en ninguna de las dos \
         ramas: el simulacro no promete una reversión que la ejecución no hace"
    );
}

/// El plan no promete un directorio de programa que la desinstalación no permite borrar.
///
/// Un directorio de programa no es borrable si es `$HOME`, un ancestro suyo, una raíz de
/// unidad o una raíz del producto; en ese caso la ejecución real lo conserva. El plan debe
/// tomar la misma decisión que la ejecución: si decidiera por su cuenta (por ejemplo con
/// `program_dir.is_dir()`) listaría el directorio como destino, y `self uninstall
/// --dry-run` anunciaría un borrado que la operación no hace, que es la propiedad que un
/// plan veraz no puede tener.
///
/// Se afirma en las dos ramas: el simulacro no lo lista como destino y la ejecución real
/// tampoco lo borra, y las dos lo anuncian como `preserved` con el motivo del rechazo.
///
/// El caso elegido es `$HOME` como directorio de programa, con el recibo escrito dentro:
/// es un caso de rechazo que además **no** cuelga de una raíz que el plan ya borra entera,
/// así que el directorio sobrevive de verdad a la operación. Un directorio de programa
/// dentro de la raíz de datos no serviría para afirmar la conservación: la desinstalación
/// borra esa raíz completa, con él dentro.
#[test]
fn uninstall_plan_does_not_promise_a_program_dir_it_refuses_to_delete() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("program-rejected");
    sandbox.seed_env();
    sandbox.seed_state();
    let program_dir = sandbox.home.clone();
    let receipt = InstallReceipt::new(
        "0.24.0",
        avi_lifecycle::target::host_triple(),
        Channel::Script,
        &program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.cache_dir.clone(),
        },
        None,
    );
    receipt::write_to(&receipt, &program_dir).expect("se escribe el recibo");
    let roots = sandbox.roots();
    assert!(
        !uninstall::program_dir_is_removable(&roots, &program_dir),
        "este directorio de programa no es borrable, que es la premisa de la prueba"
    );

    let options = uninstall::Options {
        assume_yes: true,
        ..Default::default()
    };
    let plan = uninstall::compose_plan(&roots, Some(&receipt), &program_dir, &options);
    assert!(
        plan.program_dir.is_none(),
        "el plan no lo ofrece como destino: la ejecución real no lo borraría"
    );
    assert!(
        !plan.entries().iter().any(|e| e.path == program_dir),
        "y no aparece en las entradas del plan, que es lo que se anuncia y se borra"
    );
    assert!(
        plan.preserved
            .iter()
            .any(|p| p.path == program_dir && p.reason == uninstall::PROGRAM_DIR_PRESERVED_REASON),
        "el plan lo conserva con el motivo del rechazo, que es lo que hace la ejecución real: \
         {:?}",
        plan.preserved
    );

    // El simulacro hereda la misma decisión, porque sale del mismo planificador.
    let preview = uninstall::simulate(
        &roots,
        Some(&receipt),
        &program_dir,
        &sandbox.home,
        &options,
    );
    assert_eq!(preview.planned, preview.removed, "el simulacro es el plan");
    assert!(
        !preview
            .removed
            .iter()
            .any(|p| p == &program_dir.display().to_string()),
        "y no anuncia el borrado que no ocurriría"
    );

    // Y la ejecución real lo conserva, con el `status` habitual.
    let real = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &options,
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");
    assert_eq!(
        real.status, "uninstalled",
        "la exclusión del directorio de programa es preservación deliberada, no un fallo"
    );
    assert!(
        !real.program_dir_removed && real.program_dir_kept.is_none(),
        "y ni `program_dir_removed` ni `program_dir_kept` describen esta conservación"
    );
    assert!(
        inspect::exists(&program_dir),
        "el directorio de programa sigue en su sitio"
    );
    assert!(
        real.preserved
            .iter()
            .any(|p| p.path == program_dir && p.reason == uninstall::PROGRAM_DIR_PRESERVED_REASON),
        "la ejecución real también lo anuncia como conservado"
    );
    assert!(
        !real
            .removed
            .iter()
            .any(|p| p == &program_dir.display().to_string()),
        "y no lo cuenta entre lo borrado"
    );
    assert_eq!(
        real.planned, preview.planned,
        "el plan que ve el usuario y el que se ejecutó son el mismo"
    );
}

/// Un directorio de programa que no es borrable por vivir dentro de una raíz del
/// producto **no** se anuncia como conservado cuando esa raíz es un destino del
/// plan, porque la operación sí se lo lleva.
///
/// «No lo borro yo» no es «no se borra». Sin esta distinción, con
/// `AVI_INSTALL_DIR` dentro de la raíz de datos —una disposición posible— el
/// comando anunciaría «no se tocará» sobre un directorio que el borrado de la
/// raíz entera se lleva: la misma afirmación falsa que el envelope del plan y el
/// resumen en texto tienen que evitar.
#[test]
fn uninstall_announces_a_program_dir_swallowed_by_its_root() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("program-inside-root");
    sandbox.seed_env();
    sandbox.seed_state();
    // Directorio de programa dentro de la raíz de datos: no es borrable por sí mismo
    // porque cuelga de una raíz del producto, y la desinstalación borra esa raíz completa.
    let program_dir = sandbox.data_dir.join("bin").join("ai-voice-interconnector");
    std::fs::create_dir_all(&program_dir).expect("se crea el directorio anidado");
    let receipt = InstallReceipt::new(
        "0.24.0",
        avi_lifecycle::target::host_triple(),
        Channel::Script,
        &program_dir,
        vec![uninstall::executable_name_default()],
        PathIntegration::none(),
        receipt::Roots {
            data_dir: sandbox.data_dir.clone(),
            cache_dir: sandbox.cache_dir.clone(),
        },
        None,
    );
    receipt::write_to(&receipt, &program_dir).expect("se escribe el recibo");
    let roots = sandbox.roots();
    assert!(
        !uninstall::program_dir_is_removable(&roots, &program_dir),
        "este directorio de programa no es borrable, que es la premisa de la prueba"
    );

    let options = uninstall::Options {
        assume_yes: true,
        ..Default::default()
    };
    let plan = uninstall::compose_plan(&roots, Some(&receipt), &program_dir, &options);
    assert_eq!(
        plan.program_dir_swallowed_by.as_deref(),
        Some(sandbox.data_dir.as_path()),
        "el plan registra que la raíz de datos se lo lleva"
    );
    assert!(
        !plan.preserved.iter().any(|p| p.path == program_dir),
        "y no lo anuncia como conservado, porque no se conserva: {:?}",
        plan.preserved
    );

    // El simulacro y la ejecución real coinciden, y los dos lo borran.
    let preview = uninstall::simulate(
        &roots,
        Some(&receipt),
        &program_dir,
        &sandbox.home,
        &options,
    );
    let real = runtime::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &options,
            &Now,
            &Inert,
        ))
        .expect("la desinstalación se ejecuta");
    assert_eq!(
        preview.planned, real.planned,
        "el plan que ve el usuario y el que se ejecutó son el mismo"
    );
    assert!(
        !real.preserved.iter().any(|p| p.path == program_dir),
        "la ejecución real tampoco lo anuncia como conservado"
    );
    assert!(
        !inspect::exists(&program_dir),
        "y el directorio desaparece con la raíz, como dice el plan"
    );
}

/// Un directorio de programa que no existe no se anuncia como conservado: no hay nada
/// que conservar. Vale igual para el plan que para el simulacro.
#[test]
fn uninstall_plan_does_not_preserve_a_missing_program_dir() {
    let _guard = process_env::exclusively();
    let sandbox = Sandbox::new("program-missing");
    sandbox.seed_env();
    sandbox.seed_state();
    let program_dir = sandbox.home.join("no-existe");
    let roots = sandbox.roots();
    let options = uninstall::Options {
        assume_yes: true,
        ..Default::default()
    };

    let plan = uninstall::compose_plan(&roots, None, &program_dir, &options);
    assert!(plan.program_dir.is_none());
    assert!(
        !plan.preserved.iter().any(|p| p.path == program_dir),
        "un directorio ausente no se conserva: {:?}",
        plan.preserved
    );
    let preview = uninstall::simulate(&roots, None, &program_dir, &sandbox.home, &options);
    assert!(!preview.preserved.iter().any(|p| p.path == program_dir));
}
