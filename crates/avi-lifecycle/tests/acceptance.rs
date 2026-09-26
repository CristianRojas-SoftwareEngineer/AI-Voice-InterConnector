//! Criterios de aceptación de §15 que le tocan al Ciclo 1, una prueba por criterio.
//!
//! Los once criterios de este ciclo son el **2**, el **6**, el **7**, el **9** —que solo
//! se puede ejecutar en macOS y por eso vive en `tests/quarantine.rs`—, y del **17** al
//! **23**. Los nombres llevan el número del criterio, que aparece además en el mensaje de
//! cada aserción, para que el informe de ground truth mapee sin ambigüedad.
//!
//! Todas las pruebas usan el mismo arnés (`tests/support/mod.rs`), que hace las dos
//! cosas que §13 exige y que no son la misma: **declarar las cuatro variables de
//! reubicación de §7** apuntando al sandbox, para que lo que el motor resuelve por el
//! entorno caiga dentro del sandbox, y **pasar las mismas rutas como dato** en `Env` y
//! `Roots`, que es como las recibe en producción. En Windows las Known Folders ignoran
//! `LOCALAPPDATA`, así que un sandbox que se apoyara en ellas no estaría probando nada
//! del mecanismo de §7.
//!
//! Ninguna prueba depende de red, de ONNX Runtime ni del motor TTS: el bundle es
//! sintético y sale de `packaging/bundle-manifest.json`. El único proceso externo que
//! aparece en todo el archivo es el conversor de CT2 del criterio 6, y su **fallo** es
//! precisamente lo que esa prueba necesita, de modo que no depende de qué tenga
//! instalado la máquina que ejecuta.
//!
//! El criterio 6 tiene dos mitades y por eso dos pruebas: `--no-setup` no provisiona
//! nada, y un fallo de `setup` deja el programa instalado. La segunda tiene una mitad del
//! enunciado que el motor **no** cumple hoy; el doc-comment lo dice con nombre y con el
//! `reason` que falta, y el arreglo es del motor, no de la prueba.

#![allow(clippy::disallowed_methods)]

mod support;

use avi_lifecycle::channel::Channel;
use avi_lifecycle::cleanup;
use avi_lifecycle::install::{self, ModelsState};
use avi_lifecycle::receipt::{self, InstallReceipt, PathIntegration};
use avi_lifecycle::setup;
use avi_lifecycle::uninstall;
use avi_store::{ModelStore, MODEL_FILE_PATTERNS, MODEL_REVISIONS};
use std::path::{Path, PathBuf};
use support::{Inert, Models, Now, Sandbox};

/// Recibo con el instante de instalación neutralizado, para comparar dos pasadas por
/// todo lo demás. `installed_at` es lo único que §8.1 declara que cambia entre dos
/// instalaciones de la misma versión.
fn normalized_receipt(receipt: &InstallReceipt) -> InstallReceipt {
    let mut copy = receipt.clone();
    copy.installed_at = "<instante>".to_string();
    copy
}

/// Estado del sandbox sin el archivo de bloqueo, que §9.1 crea al tomar el bloqueo y que
/// no es «nada modificado» sino el mecanismo que serializa las operaciones.
fn state_without_lock(sandbox: &Sandbox) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = sandbox
        .snapshot()
        .into_iter()
        .filter(|(path, _)| path.rsplit('/').next() != Some(avi_lifecycle::LIFECYCLE_LOCK_NAME))
        .collect();
    out.sort();
    out
}

/// Entradas del directorio padre del programa, que es donde viven el staging hermano, el
/// archivo de bloqueo y los aparcados. El residuo cero del criterio 17 se afirma sobre
/// esto y no solo sobre el directorio de programa.
fn opt_entries(sandbox: &Sandbox) -> Vec<String> {
    let parent = sandbox
        .program_dir
        .parent()
        .expect("el directorio de programa tiene padre");
    support::list(parent)
}

/// `cleanup` con `--yes` y la categoría que le pase quien llama.
fn with_yes() -> cleanup::Options {
    cleanup::Options {
        assume_yes: true,
        ..Default::default()
    }
}

/// Ejecuta una categoría y comprueba que no falló nada y que borró exactamente lo que el
/// plan berkata, que es la comprobación que un plan correcto con un ejecutor laxo no
/// superaría.
fn run_category(sandbox: &Sandbox, runtime: &tokio::runtime::Runtime, options: cleanup::Options) {
    let announced = support::paths(&cleanup::plan(&sandbox.roots(), &options));
    let outcome = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("criterio 22: la categoría se ejecuta");
    assert!(
        outcome.failed.is_empty(),
        "criterio 22: nada falló: {:?}",
        outcome.failed
    );
    for path in &announced {
        assert!(
            !support::exists(Path::new(path)),
            "criterio 22: lo anunciado se borró: {path}"
        );
    }
    assert_eq!(
        outcome.removed, announced,
        "criterio 22: el plan y la ejecución coinciden"
    );
}

/// Provisiona en el sandbox los repos de la selección, con los archivos críticos que
/// `avi-store` exige para dar cada uno por provisionado.
///
/// Es lo que permite que la provisión **no toque la red**: con los snapshots ya en su
/// sitio, `setup::pending` no devuelve repos que descargar y el único trabajo que queda
/// es la conversión de CT2, que es local. Cada repo de pruebas se planta con los nombres
/// de archivo que el producto espera, no con un marcador, porque `is_provisioned` decide
/// por presencia y tamaño de esos archivos.
fn provision_selection(sandbox: &Sandbox) {
    for (name, repo, rev) in MODEL_REVISIONS {
        if *name == setup::CLONING_MODEL {
            continue;
        }
        let snapshot = sandbox
            .models_dir
            .join(support::repo_dir(repo))
            .join("snapshots")
            .join(rev);
        match MODEL_FILE_PATTERNS.iter().find(|(n, _)| n == name) {
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

// ─── Criterio 2 ───────────────────────────────────────────────────────────────────

/// **Criterio 2.** Repetir la instalación con la misma versión termina con éxito y deja
/// el mismo estado, sin entradas de `PATH` ni bloques de perfil duplicados.
///
/// Lo que se afirma va más allá de «las dos llamadas devuelven lo mismo»: se compara el
/// recibo leído de disco con el instante neutralizado, el contenido del directorio de
/// programa antes y después de la segunda pasada, y **el número de apariciones** de la
/// entrada en el destino de la integración —el valor `Path` de la clave de prueba en
/// Windows, el bloque delimitado y el enlace en Unix—. Un duplicado es exactamente el
/// defecto que el criterio nombra, y comparar solo los desenlaces no lo vería.
#[test]
fn criterion_2_install_twice_is_idempotent() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c2");
    sandbox.seed_env();
    let options = Sandbox::install_options();
    let runtime = support::runtime();

    // ── Primera instalación ─────────────────────────────────────────────────────
    let exe = sandbox.write_bundle(&sandbox.staging);
    let first = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("criterio 2: la primera instalación se completa");
    assert_eq!(
        first.status, "installed",
        "criterio 2: la primera pasada instala"
    );
    assert!(
        first.path_changed(),
        "criterio 2: la primera pasada reescribe el `PATH`"
    );
    let first_receipt = normalized_receipt(&first.receipt);
    let first_content = support::list(&sandbox.program_dir);
    assert!(
        first_content.contains(&receipt::RECEIPT_NAME.to_string()),
        "criterio 2: el recibo está entre lo colocado: {first_content:?}"
    );

    // ── Segunda instalación, desde el mismo bundle ──────────────────────────────
    // El bundle se repone porque la colocación **mueve** los archivos desde el origen
    // (§9.3.6.2). Es también lo que hace el bootstrap de §9.2, que extrae en un staging
    // nuevo cada vez.
    let exe = sandbox.write_bundle(&sandbox.staging);
    let second = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("criterio 2: la segunda instalación se completa");

    assert_eq!(
        second.status, "installed",
        "criterio 2: la segunda pasada también termina en éxito"
    );
    assert_eq!(
        second.mode,
        install::Mode::Install,
        "criterio 2: sigue siendo instalación, porque el ejecutable viene del staging"
    );
    assert_eq!(
        normalized_receipt(&second.receipt),
        first_receipt,
        "criterio 2: el mismo estado final, con el mismo recibo"
    );
    assert_eq!(
        second.path_integration, first.path_integration,
        "criterio 2: la integración registrada es la misma, no el diff de esta pasada"
    );
    assert!(
        !second.path_changed(),
        "criterio 2: la segunda pasada no reescribe el `PATH`: la entrada ya estaba"
    );
    assert!(
        second.path_integrated(),
        "criterio 2: pero sigue registrada en el recibo, que es lo que permite revertirla"
    );
    assert_eq!(
        support::list(&sandbox.program_dir),
        first_content,
        "criterio 2: el directorio de programa no cambia de contenido"
    );

    // El recibo de disco coincide con el que devolvió la operación.
    let on_disk = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 2: se lee el recibo")
        .expect("criterio 2: el recibo existe");
    assert_eq!(
        normalized_receipt(&on_disk),
        first_receipt,
        "criterio 2: el recibo de disco es el mismo"
    );

    // Y ahora el punto del criterio: la integración no está duplicada.
    #[cfg(unix)]
    {
        let profile = sandbox.home.join(".profile");
        let text = std::fs::read_to_string(&profile).expect("criterio 2: el perfil se escribió");
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "criterio 2: un solo bloque delimitado tras dos instalaciones: {text}"
        );
        let link = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        let dest = std::fs::read_link(&link).expect("criterio 2: el enlace existe");
        assert_eq!(
            dest.parent(),
            Some(sandbox.program_dir.as_path()),
            "criterio 2: el enlace apunta al directorio de programa"
        );
    }
    #[cfg(windows)]
    {
        let value = avi_lifecycle::path_windows::read_path(&sandbox.registry_subkey)
            .expect("criterio 2: se lee el valor de la clave de prueba")
            .expect("criterio 2: la integración escribió el valor")
            .value;
        let entry = sandbox.bin_dir.display().to_string();
        assert_eq!(
            value.matches(&entry).count(),
            1,
            "criterio 2: la entrada aparece una sola vez tras dos instalaciones: {value}"
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
/// El defecto que fija esta prueba era que `final_summary` copiaba la línea del `PATH` del
/// **resumen previo** —que dice lo que se va a hacer— y añadía debajo la del **estado**. En
/// una instalación correcta el usuario leía, en el mismo bloque y en este orden:
///
/// ```text
///   PATH:      se añadirá <dir> en el PATH del usuario
///   PATH:      se añadió <dir>; abre una terminal nueva para que el comando se encuentre.
/// ```
///
/// Dos líneas para un hecho, y la primera en futuro_y la segunda en pasado: leído de un
/// tirón parece una contradicción. Afirma los **tres** estados posibles, porque el arreglo no
/// es «quitar la primera línea» sino «dejar una sola, la del estado», y cada rama del
/// `if` del resumen produce una prosa distinta que también hay que fijar.
#[test]
fn criterion_2_final_summary_reports_the_path_state_once() {
    let _guard = support::exclusively();
    let runtime = support::runtime();

    // ── Estado 1: esta pasada **reescribió** el `PATH` ────────────────────────────
    let sandbox = Sandbox::new("c2-path-nuevo");
    sandbox.seed_env();
    let exe = sandbox.write_bundle(&sandbox.staging);
    let first = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("criterio 2: la primera instalación se completa");
    assert!(
        first.path_changed(),
        "criterio 2: la primera pasada reescribe el `PATH`, que es el estado que hay que probar"
    );
    let lines = summary_lines(&first.summary);
    assert_eq!(
        lines.len(),
        1,
        "criterio 2: el resumen final dice el `PATH` una sola vez, y no dos: {:?}",
        first.summary
    );
    assert!(
        lines[0].contains("se añadió"),
        "criterio 2: y en pasado, porque esta pasada lo escribió: {}",
        lines[0]
    );
    assert!(
        !lines[0].contains("se añadirá"),
        "criterio 2: y no en futuro: el resumen final no anuncia, informa: {}",
        lines[0]
    );
    assert!(
        lines[0].contains("abre una terminal nueva"),
        "criterio 2: con la indicación de §9.3.1, que solo aparece si se acaba de escribir: {}",
        lines[0]
    );

    // ── Estado 2: la integración está en pie y esta pasada no la tocó ────────────
    let exe = sandbox.write_bundle(&sandbox.staging);
    let second = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("criterio 2: la segunda instalación se completa");
    assert!(!second.path_changed());
    let lines = summary_lines(&second.summary);
    assert_eq!(
        lines.len(),
        1,
        "criterio 2: tampoco en la repetición, que es donde se vería un segundo duplicado: {:?}",
        second.summary
    );
    assert!(
        lines[0].contains("ya estaba integrado"),
        "criterio 2: y lo dice como estado, no como plan: {}",
        lines[0]
    );

    // ── Estado 3: `--no-modify-path`, donde el plan **es** el estado ──────────────
    let s = Sandbox::new("c2-path-sin-path");
    s.seed_env();
    let exe = s.write_bundle(&s.staging);
    let without_path = runtime
        .block_on(install::install(
            &s.install_env(&exe),
            &install::Options {
                no_modify_path: true,
                ..Sandbox::install_options()
            },
            &Inert,
        ))
        .expect("criterio 2: la instalación con `--no-modify-path` se completa");
    let lines = summary_lines(&without_path.summary);
    assert_eq!(
        lines.len(),
        1,
        "criterio 2: con `--no-modify-path` también una sola línea: {:?}",
        without_path.summary
    );
    assert!(
        lines[0].contains("no se modifica"),
        "criterio 2: y aquí el texto del plan es el del estado, porque no hubo escritura: {}",
        lines[0]
    );
}

// ─── Criterio 6 ───────────────────────────────────────────────────────────────────

/// **Criterio 6, primera mitad.** Con `--no-setup` no se descarga ningún modelo.
///
/// La primera aserción es la que impide que la prueba sea vacía: se calcula lo que
/// `setup` **habría** hecho contra el mismo almacén, y tiene que haber algo. Sin esa
/// comprobación la prueba pasaría igual con un `--no-setup` que no hiciera nada, que es
/// justo el defecto que el criterio prohíbe.
///
/// Y la segunda mitad es que la raíz de modelos **sigue vacía**: no basta con que el
/// resumen diga `skipped`, tiene que no haber quedado ni un repo, ni `ct2`, ni `xet`.
#[test]
fn criterion_6_no_setup_provisions_nothing() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c6-sin-setup");
    sandbox.seed_env();

    // No vacuidad: hay modelos que provisionar si se pidiera.
    let setup_options = setup::Options::user(false, false, true);
    let pending = setup::pending(&ModelStore::new(), &setup_options);
    let selection = setup::selection(&setup_options);
    assert!(
        !pending.models.is_empty(),
        "criterio 6: hay repos que descargar, así que `--no-setup` omite trabajo real: {:?}",
        pending.models
    );
    assert_eq!(
        pending.models.len(),
        selection.len(),
        "criterio 6: la selección completa está pendiente"
    );

    let exe = sandbox.write_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &Sandbox::install_options(),
            &Inert,
        ))
        .expect("criterio 6: la instalación se completa");

    assert_eq!(outcome.status, "installed");
    assert_eq!(
        outcome.models,
        ModelsState::Skipped,
        "criterio 6: el desenlace dice que no se provisionó nada"
    );
    assert_eq!(outcome.models.as_str(), "skipped");
    assert_eq!(
        outcome.lifecycle_error(),
        None,
        "criterio 6: `--no-setup` no produce `reason` de contrato, así que sigue saliendo por \
         `Done` con código 0"
    );
    assert_eq!(
        support::list(&sandbox.models_dir),
        Vec::<String>::new(),
        "criterio 6: la raíz de modelos sigue vacía: no hay repo, ni `ct2`, ni `xet`"
    );
    assert!(
        !ModelStore::new().is_provisioned("marian-es-en"),
        "criterio 6: y nada quedó provisionado en el almacén"
    );
    assert!(
        receipt::read_from(&sandbox.program_dir)
            .expect("criterio 6: se lee el recibo")
            .is_some(),
        "criterio 6: el programa queda instalado, que es lo que §9.3 paso 11 promete"
    );
}

/// **Criterio 6, segunda mitad.** Sin `--no-setup`, un fallo de `setup` deja el programa
/// instalado y termina con `setup_failed`.
///
/// El fallo se provoca **sin red**: se provisionan en el sandbox los repos de la
/// selección, de modo que `setup::pending` no devuelve nada que descargar, y el único
/// trabajo que queda es la conversión del derivado CT2, que es local. Que la conversión
/// falle es lo que la prueba necesita, y es independiente de qué conversores tenga
/// instalados la máquina: si no hay `python`, falla por no encontrarlo; si lo hay, falla
/// porque el conversor no encuentra un modelo donde está el snapshot de pruebas.
///
/// Se afirman las **dos** mitades del criterio. La del programa instalado es la que se
/// comprueba contra el disco. La del `reason` se comprueba sobre
/// [`install::Outcome::lifecycle_error`], que es el punto único donde el motor decide el
/// `reason` de la operación: `setup_failed` con el código `11` de la tabla cerrada, que es
/// lo que §9.1 declara como **éxito parcial** y lo que el cableado convierte en código de
/// salida. Y se afirma también la separación de los dos `reason`, que es lo que mantiene
/// intacto el criterio del ciclo 2: el de la **operación** es `setup_failed` y el del
/// **fallo de provisión** viaja anidado, y en este caso es `ct2_conversion_failed`.
///
/// El resumen en texto se afirma por las dos mitades —el estado de los modelos y el aviso
/// con el motivo de la causa y la instrucción de reintentar—, porque es la otra salida de
/// la misma operación y no puede decir una cosa mientras el sobre dice otra.
#[test]
fn criterion_6_setup_failure_keeps_install() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c6-fallo");
    sandbox.seed_env();
    provision_selection(&sandbox);

    // No vacuidad del camino de fallo: queda CT2 por convertir y **nada** por descargar.
    let setup_options = setup::Options::user(false, false, true);
    let pending = setup::pending(&ModelStore::new(), &setup_options);
    assert!(
        pending.models.is_empty(),
        "criterio 6: no queda nada que descargar, así que la prueba no toca la red: {:?}",
        pending.models
    );
    assert_eq!(
        pending.ct2.len(),
        setup::CT2_PAIRS.len(),
        "criterio 6: los dos derivados CT2 están pendientes de conversión: {:?}",
        pending.ct2
    );

    let exe = sandbox.write_bundle(&sandbox.staging);
    let runtime = support::runtime();
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
        .expect("criterio 6: un fallo de `setup` no es un fallo de la instalación");

    // ── Mitad 1: el programa queda instalado ──────────────────────────────────
    assert_eq!(
        outcome.status, "installed",
        "criterio 6: la instalación termina con éxito"
    );
    let on_disk = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 6: se lee el recibo")
        .expect("criterio 6: el recibo existe: el programa quedó instalado");
    assert_eq!(on_disk.version, "0.24.0");
    for relative in &outcome.receipt.files {
        assert!(
            support::place(&sandbox.program_dir, relative).is_file(),
            "criterio 6: {relative} sigue en el directorio de programa"
        );
    }

    // El fallo es un estado del desenlace, y su `reason` es el **de la provisión**, no el
    // de la operación.
    let ModelsState::Failed { cause } = &outcome.models else {
        panic!(
            "criterio 6: se esperaba un fallo de provisión y el estado es {:?}",
            outcome.models
        );
    };
    assert_eq!(outcome.models.as_str(), "failed");
    assert_eq!(
        cause.reason, "ct2_conversion_failed",
        "criterio 6: el fallo de provisión conserva su `reason`, que es el que \
         `docs/CLI/commands/SETUP.md` publica para una conversión fallida"
    );
    assert_eq!(
        cause.exit_code, 1,
        "criterio 6: y el código genérico, porque §9.1 no le declara fila propia; el código \
         de salida del proceso es el de la operación, no este"
    );

    // ── Mitad 2: termina con `setup_failed` y el código 11 ────────────────────
    let failure = outcome
        .lifecycle_error()
        .expect("criterio 6: §9.1 declara `setup_failed` para este desenlace");
    assert_eq!(
        failure.reason, "setup_failed",
        "criterio 6: el `reason` de la operación es `setup_failed`"
    );
    assert_eq!(
        failure.exit_code, 11,
        "criterio 6: y el código es `SetupFailed = 11` de la tabla cerrada"
    );
    assert_eq!(
        contract_exit_code("setup_failed"),
        Some(11),
        "criterio 6: el cableado traduce ese `reason` al mismo entero, que es lo que evita \
         que las dos copias del 11 diverjan"
    );
    // El mensaje dice las dos cosas que §9.3 paso 11 promete: qué no se completó y que
    // basta reintentar con `setup`.
    assert!(
        failure.message.contains("no se completó"),
        "criterio 6: el mensaje dice qué no se completó: {}",
        failure.message
    );
    assert!(
        failure.message.contains("reintentar con setup"),
        "criterio 6: y que basta reintentar con `setup`, que es la otra mitad del criterio: {}",
        failure.message
    );
    assert!(
        failure.message.contains(cause.message.as_str()),
        "criterio 6: y el motivo de la causa viaja dentro, para que el `reason` anidado no se \
         pierda: {}",
        failure.message
    );
    assert!(
        failure
            .message
            .contains(&sandbox.program_dir.display().to_string()),
        "criterio 6: y dónde quedó instalado, que es lo que el usuario necesita saber: {}",
        failure.message
    );

    // El camino de éxito no se ha tocado: un `setup` que no tiene nada que hacer no produce
    // `reason` de contrato, y por eso el cableado sale por `Hecho` y con código 0. La
    // prueba de al lado lo demuestra con la provisión entera, sin atajos.

    // El resumen en texto dice lo mismo que el sobre.
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("Modelos:") && l.contains("failed")),
        "criterio 6: el resumen final dice que los modelos fallaron: {:?}",
        outcome.summary
    );
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("reintentar con setup")),
        "criterio 6: y que basta reintentar con `setup`: {:?}",
        outcome.summary
    );
    assert!(
        outcome
            .summary
            .iter()
            .any(|l| l.contains("Causa:") && l.contains("ct2_conversion_failed")),
        "criterio 6: y nombra la causa, con el `reason` anidado: {:?}",
        outcome.summary
    );

    // Y es reintentable con `setup`: el derivado sigue sin existir, así que un `setup`
    // posterior lo vuelve a pedir.
    for pair in setup::CT2_PAIRS {
        assert!(
            !avi_store::ct2_missing_files(pair).is_empty(),
            "criterio 6: el derivado CT2 de {pair} sigue pendiente, así que `setup` reintenta"
        );
    }
    assert!(
        !setup::pending(&ModelStore::new(), &setup_options)
            .ct2
            .is_empty(),
        "criterio 6: y `setup` lo ve pendiente otra vez"
    );
}

/// **La otra mitad del criterio 6: con `setup` correcto, la operación sale limpia.**
///
/// Es el requisito de que el arreglo del `reason` no haya cambiado el camino de éxito, y se
/// demuestra con el estado que el motor devuelve cuando no hay nada que provisionar:
/// `AlreadyProvisioned`, sin `reason` de contrato, que es lo que hace que el cableado salga
/// por `Hecho` con código 0 en vez de por veredicto.
///
/// El sandbox tiene los repos de la selección **y** los dos derivados CT2 sanos y más
/// nuevos que sus snapshots, que es lo que `needs_reconversion` exige para no pedirlos. Por
/// eso lleva una espera de más de un segundo entre escribir el snapshot y escribir el
/// derivado: la comparación es por `mtime` y dos escrituras seguidas pueden caer en el mismo
/// tick en un sistema de ficheros de resolución gruesa, con lo que la prueba probaría el
/// reloj y no el código. Es la misma espera que ya usa `repair_from_inside_program_dir_copies_nothing`.
#[test]
fn criterion_6_successful_provisioning_has_no_reason() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c6-ok");
    sandbox.seed_env();
    provision_selection(&sandbox);

    // Los derivados, después de una espera que garantiza que su `mtime` es posterior al
    // del snapshot: `needs_reconversion` compara `ct2_time <= hf_time` y devuelve
    // `true` —o sea, hay que convertir— cuando el derivado no es más nuevo.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for pair in setup::CT2_PAIRS {
        let dir = avi_store::ct2_model_dir(pair);
        support::write(&dir.join("model.bin"), "ct2");
        support::write(&dir.join("source.spm"), "spm");
        support::write(&dir.join("target.spm"), "spm");
    }

    // No vacuidad: `setup` no tiene nada que hacer, y eso es lo que la prueba comprueba.
    let pending = setup::pending(
        &ModelStore::new(),
        &setup::Options::user(false, false, true),
    );
    assert!(
        pending.is_empty(),
        "criterio 6: con los repos y los derivados ya provisionados no queda nada pendiente: \
         {:?}",
        pending
    );
    for pair in setup::CT2_PAIRS {
        assert!(
            avi_store::ct2_missing_files(pair).is_empty(),
            "criterio 6: el derivado de {pair} está sano"
        );
    }

    let exe = sandbox.write_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &install::Options {
                no_setup: false,
                ..Sandbox::install_options()
            },
            &Inert,
        ))
        .expect("criterio 6: la instalación se completa");

    assert_eq!(outcome.status, "installed");
    assert_eq!(
        outcome.models,
        ModelsState::AlreadyProvisioned,
        "criterio 6: con `setup` correcto el estado es que no había nada que hacer"
    );
    assert_eq!(
        outcome.lifecycle_error(),
        None,
        "criterio 6: y no hay `reason` de contrato, así que el cableado sale por `Done` con \
         código 0 y no por veredicto"
    );
    assert!(
        !outcome
            .summary
            .iter()
            .any(|l| l.contains("Causa:") || l.contains("no se completó")),
        "criterio 6: y el resumen no dice nada de un fallo que no ha habido: {:?}",
        outcome.summary
    );
}

/// El entero que el cableado traduce a cada `reason` con variante propia, leído de la
/// tabla que `src/main.rs` usa.
///
/// La tabla **no** es accesible desde aquí —vive en el binario—, así que se reproduce su
/// parte declarada y se comprueba contra el `exit_code` que el motor emite. Si alguien
/// cambiara uno de los dos, esta comparación falla; si cambiara el otro sin cambiar el
/// primero, la puerta de `cli_golden` lo es.
fn contract_exit_code(reason: &str) -> Option<i32> {
    match reason {
        "setup_failed" => Some(11),
        "externally_managed" => Some(12),
        "rolled_back" => Some(13),
        "path_conflict" => Some(14),
        "bundle_invalid" => Some(15),
        "daemon_stop_failed" => Some(16),
        "lifecycle_locked" => Some(17),
        "confirmation_required" | "usage_error" => Some(2),
        _ => None,
    }
}

// ─── Criterio 17 ──────────────────────────────────────────────────────────────────

/// **Criterio 17.** `self uninstall --yes` elimina el programa, la integración de `PATH`
/// y el estado, sin residuo dentro de las raíces de propiedad exclusiva.
///
/// El ciclo es completo: una instalación **real** por el motor, no un recibo plantado, y
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
fn criterion_17_uninstall_leaves_no_residue() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c17");
    sandbox.seed_env();
    let runtime = support::runtime();
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
    let exe = sandbox.write_bundle(&sandbox.staging);
    let installed = runtime
        .block_on(install::install(
            &sandbox.install_env(&exe),
            &options,
            &Inert,
        ))
        .expect("criterio 17: la instalación se completa");
    assert_eq!(installed.status, "installed");
    sandbox.seed_state();
    let receipt = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 17: se lee el recibo")
        .expect("criterio 17: el recibo existe");

    if cfg!(unix) {
        let text = std::fs::read_to_string(&profile).expect("criterio 17: el perfil se lee");
        assert!(
            text.starts_with("# adjusting del usuario\n"),
            "criterio 17: el contenido propio está al principio y no se tocó: {text}"
        );
        assert_eq!(
            text.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "criterio 17: y el bloque delimitado está al final: {text}"
        );
        assert!(
            text.ends_with(avi_lifecycle::path_unix::BLOCK_END),
            "criterio 17: con su marcador de cierre al final del archivo: {text}"
        );
    }
    if cfg!(windows) {
        assert!(
            !receipt.path_integration.modify_path,
            "criterio 17: con `--no-modify-path` el recibo lo dice, y por eso no hay nada \
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
        .expect("criterio 17: la desinstalación se ejecuta");

    assert_eq!(
        outcome.status, "uninstalled",
        "criterio 17: termina en éxito"
    );
    assert!(
        outcome.failed.is_empty(),
        "criterio 17: nada falló: {:?}",
        outcome.failed
    );
    assert!(
        outcome.program_dir_removed,
        "criterio 17: el directorio de programa se borró"
    );

    // Residuo cero en las raíces de propiedad exclusiva.
    assert!(
        !support::exists(&sandbox.program_dir),
        "criterio 17: el directorio de programa no queda"
    );
    assert!(
        !support::exists(&sandbox.data_dir),
        "criterio 17: la raíz de datos no queda, voces de fábrica incluidas: el programa \
         ya no está para re-materializarlas"
    );
    assert!(
        !support::exists(&sandbox.models_dir),
        "criterio 17: la raíz de modelos no queda"
    );
    assert_eq!(
        opt_entries(&sandbox),
        Vec::<String>::new(),
        "criterio 17: el padre del programa queda vacío: ni staging, ni aparcados, ni bloqueo"
    );
    let own_temps: Vec<String> = support::list(&sandbox.temp_root)
        .into_iter()
        .filter(|n| {
            avi_lifecycle::TEMP_PREFIXES
                .iter()
                .any(|p| n.starts_with(p))
        })
        .collect();
    assert_eq!(
        own_temps,
        Vec::<String>::new(),
        "criterio 17: ningún temporal propio sobrevive"
    );

    if cfg!(unix) {
        let link = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        assert!(
            !support::exists(&link),
            "criterio 17: el enlace del `PATH` se retira"
        );
        assert!(outcome.path_reverted, "criterio 17: y el motor lo dice");
        let text = std::fs::read_to_string(&profile).expect("criterio 17: el perfil se lee");
        assert_eq!(
            text, "# adjusting del usuario\n",
            "criterio 17: el bloque delimitado se quita y el resto del perfil no se toca"
        );
    }
}

// ─── Criterio 18 ──────────────────────────────────────────────────────────────────

/// **Criterio 18.** `self uninstall --keep-data` conserva modelos, voces y habla
/// sintetizada.
///
/// Se afirma también el reverso, que es la mitad que suele olvidarse: la configuración,
/// los logs y el pidfile **sí** caen, porque son estado de ejecución y no datos de
/// usuario. Un `--keep-data` que conservara todo no sería una operación de desinstalación.
#[test]
fn criterion_18_keep_data_preserves_state() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c18");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());

    // Punto de partida: los tres conjuntos están.
    assert!(support::exists(&sandbox.models_dir.join("xet")));
    assert!(support::exists(
        &sandbox.data_dir.join("voices").join("mia")
    ));
    assert!(support::exists(
        &sandbox.data_dir.join("speech").join("default")
    ));

    let outcome = support::runtime()
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
        .expect("criterio 18: la desinstalación se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::exists(&sandbox.program_dir),
        "criterio 18: el programa sí se va"
    );

    // Lo que se conserva.
    assert!(
        support::exists(&sandbox.models_dir.join("xet"))
            && support::exists(&sandbox.models_dir.join("ct2").join("marian-es-en")),
        "criterio 18: los modelos se quedan: {:?}",
        outcome.preserved
    );
    for voice in ["default", "ryan", "mia"] {
        assert!(
            support::exists(&sandbox.data_dir.join("voices").join(voice)),
            "criterio 18: la voz {voice} se queda"
        );
    }
    for voice in ["default", "mia"] {
        assert!(
            support::exists(&sandbox.data_dir.join("speech").join(voice)),
            "criterio 18: la locución de {voice} se queda"
        );
    }
    for reason in ["modelos", "voces", "habla"] {
        assert!(
            outcome.preserved.iter().any(|p| p.reason.contains(reason)),
            "criterio 18: `{reason}` se anuncia como conservado, con su motivo: {:?}",
            outcome.preserved
        );
    }

    // Y lo que no.
    for state in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !support::exists(&sandbox.data_dir.join(state)),
            "criterio 18: `{state}` sí se borra: es estado de ejecución, no datos"
        );
    }
    assert!(
        support::exists(&sandbox.home.join(".cargo")),
        "criterio 18: y lo que no es del producto, por supuesto"
    );
}

// ─── Criterio 19 ──────────────────────────────────────────────────────────────────

/// **Criterio 19.** Sin terminal y sin `--yes`, toda operación destructiva termina con
/// `confirmation_required` y no borra nada.
///
/// La ausencia de terminal **no se puede fingir en el proceso**: `uninstall::run` y
/// `cleanup::run` leen `stdin_is_terminal` del stdin real con `std::io::IsTerminal`, y no
/// es un parámetro. Bajo una consola interactiva el stdin **es** una terminal, así que la
/// prueba se ejecuta en un proceso hijo del mismo binario con la entrada redirigida a la
/// null. El padre es el que exige que el hijo termine en éxito, así que el trabajo no se
/// omite nunca, y las dos operaciones destructivas del alcance del ciclo se ejecutan en el
/// hijo: `self uninstall` y `cleanup --all`.
///
/// La comparación de disco ignora el archivo de bloqueo a propósito: §9.1 lo crea en el
/// paso 1, antes del plan y de la confirmación, y «no borra nada» no es «no escribe el
/// mecanismo que serializa las operaciones».
#[test]
fn criterion_19_no_tty_without_yes_refuses() {
    let _guard = support::exclusively();

    if support::is_child_without_terminal() {
        child_without_terminal();
        return;
    }

    let sandbox = Sandbox::new("c19");
    sandbox.seed_env();
    sandbox.seed_state();
    sandbox.install_registered(PathIntegration::none());
    let before = state_without_lock(&sandbox);

    let output = support::run_without_terminal("criterion_19_no_tty_without_yes_refuses", &sandbox);
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    assert!(
        output.status.success(),
        "criterio 19: el proceso sin terminal terminó con {:?}.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        output.status.code()
    );

    let report_lines = support::read_report_lines(&output.stdout);
    for operation in ["uninstall", "cleanup"] {
        let line = report_lines
            .iter()
            .find(|l| l.starts_with(operation))
            .unwrap_or_else(|| {
                panic!(
                    "criterio 19: `{operation}` no informó de su desenlace: {report_lines:?}\n\
                     --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                )
            });
        let (reason, code) = line
            .trim_start_matches(operation)
            .trim_start_matches('=')
            .split_once('/')
            .unwrap_or_else(|| panic!("criterio 19: informe mal formado: {line}"));
        assert_eq!(
            reason, "confirmation_required",
            "criterio 19: `{operation}` sin terminal y sin `--yes` se niega"
        );
        assert_eq!(
            code, "2",
            "criterio 19: `{operation}` devuelve el error de uso de §9.1"
        );
    }

    // Y nada se borró. El hijo lo afirma también; aquí se comprueba contra el disco.
    assert_eq!(
        state_without_lock(&sandbox),
        before,
        "criterio 19: no se borró nada"
    );
    assert!(
        support::exists(&sandbox.program_dir),
        "criterio 19: el directorio de programa sigue"
    );
    assert!(
        support::exists(&sandbox.data_dir.join("voices").join("mia")),
        "criterio 19: el estado de usuario sigue"
    );
    assert!(
        support::exists(&sandbox.models_dir.join("models--otra--herramienta")),
        "criterio 19: los modelos siguen"
    );
}

/// Rollo del proceso hijo: ejecuta las dos operaciones destructivas del alcance sin
/// terminal y sin `--yes`, y afirma que se niegan y que no borran nada.
///
/// El sandbox se reconstruye desde la raíz y la etiqueta que le pasó el padre, y **no se
/// borra al salir**: el padre tiene que encontrar ese mismo disco para comprobarlo.
fn child_without_terminal() {
    let root = PathBuf::from(
        std::env::var(support::VAR_ROOT).expect("criterio 19: el hijo recibe la raíz"),
    );
    let tag = std::env::var(support::VAR_TAG).expect("criterio 19: el hijo recibe la etiqueta");
    let sandbox = Sandbox::from_root(&tag, root, Models::Exclusive).borrowed();
    sandbox.seed_env();
    let before = state_without_lock(&sandbox);

    let receipt = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 19: el hijo lee el recibo")
        .expect("criterio 19: el recibo existe");
    let runtime = support::runtime();

    // 1. `self uninstall` sin `--yes`.
    let error = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &uninstall::Options::default(),
            &Now,
            &Inert,
        ))
        .expect_err("criterio 19: `self uninstall` sin terminal y sin `--yes` se niega");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 19: el fallo es un `LifecycleError`");
    assert_eq!(failure.reason, "confirmation_required");
    assert_eq!(failure.exit_code, 2, "criterio 19: error de uso (§9.1)");
    support::report_line(&format!(
        "uninstall={}/{}",
        failure.reason, failure.exit_code
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
        .expect_err("criterio 19: `cleanup` sin terminal y sin `--yes` se niega");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 19: el fallo es un `LifecycleError`");
    assert_eq!(failure.reason, "confirmation_required");
    assert_eq!(failure.exit_code, 2, "criterio 19: error de uso (§9.1)");
    support::report_line(&format!("cleanup={}/{}", failure.reason, failure.exit_code));

    // Y el disco está intacto. Sin `--yes` no se puede haber borrado nada, y esta
    // comprobación es la que convierte la negativa en una garantía.
    assert_eq!(
        state_without_lock(&sandbox),
        before,
        "criterio 19: ninguna de las dos operaciones borró nada"
    );
}

// ─── Criterio 20 ──────────────────────────────────────────────────────────────────

/// **Criterio 20.** `--dry-run` en cualquier operación destructiva lista rutas y tamaños
/// sin modificar el disco.
///
/// Se afirma en las dos operaciones del alcance, y en cada una lo mismo: la lista tiene
/// **tamaños medidos** y no cero, el disco queda idéntico —comparado con `snapshot`,
/// tamaños incluidos— y no aparece ni el archivo de bloqueo. Y se añade la comprobación
/// que convierte la simulación en una promesa: la ejecución real borra exactamente lo que
/// la simulación anunció. Un `--dry-run` que anunciara una lista y la ejecución borrara
/// otra habría cumplido la letra del criterio y no su intención.
#[test]
fn criterion_20_dry_run_does_not_touch_disk() {
    let _guard = support::exclusively();
    let sandbox = Sandbox::new("c20");
    sandbox.seed_env();
    sandbox.seed_state();
    let receipt = sandbox.install_registered(PathIntegration::none());
    let runtime = support::runtime();

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
        "criterio 20: el plan de desinstalación no está vacío"
    );
    // Los destinos de estado llevan el tamaño recursivo de la ruta, que es la cifra que
    // §9.1 pide listar. Se afirma que **coincide con la medida** y no solo que es
    // positiva, porque un cero fijo pasaría la comprobación débil y no sería un tamaño.
    for dest in &plan.state.targets {
        assert_eq!(
            dest.size,
            cleanup::path_size(&dest.path),
            "criterio 20: el tamaño de {} es el medido",
            dest.path.display()
        );
        assert!(
            dest.size > 0,
            "criterio 20: {} tiene contenido y así se anuncia",
            dest.path.display()
        );
    }
    // El directorio de programa se lista como ruta. Su cifra es la longitud de la
    // entrada —que en un directorio es 0 en Windows y el tamaño del bloque en Unix—, así
    // que afirmar que es positiva sería afirmar algo que el enunciado no pide y que la
    // plataforma decide.
    assert!(
        entries.iter().any(|e| e.path == sandbox.program_dir),
        "criterio 20: el directorio de programa aparece en el plan: {:?}",
        support::entries(&entries)
    );
    assert!(
        plan.program_dir.as_ref().is_some_and(|e| e.size.is_some()),
        "criterio 20: y con su tamaño medido"
    );
    assert!(
        cleanup::path_size(&sandbox.program_dir) > 0,
        "criterio 20: el directorio de programa tiene contenido de verdad"
    );
    assert!(
        entries.iter().any(|e| e.path == sandbox.models_dir),
        "criterio 20: y la raíz de modelos, que es el otro destino propio"
    );

    let simulation = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&receipt), Channel::Script),
            &options,
            &Now,
            &Inert,
        ))
        .expect("criterio 20: la simulación de desinstalación se ejecuta");
    assert!(simulation.dry_run, "criterio 20: el desenlace lo dice");
    let planned_removed: Vec<String> = support::entries(&entries);
    for path in &planned_removed {
        assert!(
            simulation.removed.contains(path),
            "criterio 20: la simulación anuncia {path}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        before,
        "criterio 20: `self uninstall --dry-run` no modifica el disco, ni siquiera \
         con el archivo de bloqueo"
    );

    // ── `cleanup --dry-run` ─────────────────────────────────────────────────────
    let options = cleanup::Options {
        all: true,
        dry_run: true,
        ..with_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &options);
    assert!(
        !plan.is_empty(),
        "criterio 20: el plan de limpieza no está vacío"
    );
    for dest in &plan.targets {
        assert!(
            dest.size > 0,
            "criterio 20: {} se lista con su tamaño medido",
            dest.path.display()
        );
    }
    let announced = support::paths(&plan);
    let simulated = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("criterio 20: la simulación de limpieza se ejecuta");
    assert!(simulated.dry_run);
    for path in &announced {
        assert!(
            simulated.removed.contains(path),
            "criterio 20: la simulación de `cleanup` anuncia {path}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        before,
        "criterio 20: `cleanup --dry-run` no modifica el disco"
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
        .expect("criterio 20: la limpieza real se ejecuta");
    let removed: Vec<String> = real
        .removed
        .iter()
        .filter(|r| announced.contains(r))
        .cloned()
        .collect();
    assert_eq!(
        removed, announced,
        "criterio 20: la ejecución borra exactamente lo que la simulación anunció"
    );
    assert!(
        !support::exists(&sandbox.models_dir),
        "criterio 20: y lo que se anunció borrado, está borrado"
    );
}

// ─── Criterio 21 ──────────────────────────────────────────────────────────────────

/// **Criterio 21.** Repetir `self uninstall` en un sistema ya limpio termina con éxito
/// (`not_installed`).
///
/// Se cubren las dos formas de «ya limpio»: la repetición sobre el mismo sandbox, que es
/// la que un usuario llega por accidente, y el sistema en el que nunca se instaló, que es
/// la que llega por un `cleanup --all` previo. En los dos casos el desenlace es **éxito**,
/// no un error: `not_installed` es un desenlace y §9.1 lo clasifica así para
/// `self uninstall`.
#[test]
fn criterion_21_uninstall_is_idempotent() {
    let _guard = support::exclusively();
    let runtime = support::runtime();

    // ── Instalar, desinstalar, desinstalar ──────────────────────────────────────
    let sandbox = Sandbox::new("c21");
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
        .expect("criterio 21: la primera desinstalación se ejecuta");
    assert_eq!(first.status, "uninstalled");
    assert!(first.program_dir_removed);
    assert!(first.failed.is_empty(), "criterio 21: {:?}", first.failed);

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
        .expect("criterio 21: la segunda no es un error");
    assert_eq!(
        second.status, "not_installed",
        "criterio 21: repetir sobre un sistema limpio termina en éxito con `not_installed`"
    );
    assert!(
        second.removed.is_empty(),
        "criterio 21: y no borra nada: {:?}",
        second.removed
    );
    assert_eq!(
        sandbox.snapshot(),
        after,
        "criterio 21: el disco no cambia en la repetición"
    );

    // ── Y el sistema en el que nunca se instaló ────────────────────────────────
    // El arnés **no** crea la raíz de datos ni la de modelos: en un sistema donde nunca
    // se instaló, esas raíces no existen, y es su ausencia la que hace que el desenlace
    // sea `not_installed` y no una desinstalación vacía.
    let clean = Sandbox::new("c21-limpio");
    clean.seed_env();
    assert!(!support::exists(&clean.program_dir));
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
        .expect("criterio 21: en un sistema sin instalación tampoco es un error");
    assert_eq!(
        uninstalled.status, "not_installed",
        "criterio 21: sin recibo, sin estado y sin directorio de programa"
    );
    assert!(uninstalled.removed.is_empty());
    assert!(!uninstalled.program_dir_removed);
}

// ─── Criterio 22 ──────────────────────────────────────────────────────────────────

/// **Criterio 22.** Cada categoría de `cleanup` borra solo su alcance, y `cleanup` sin
/// categoría termina con `usage_error`.
///
/// Cada categoría se **ejecuta** en su propio sandbox, no solo se planifica: el defecto
/// que el criterio vigila es que el plan y el ejecutor no coincidan, y un plan correcto
/// con un ejecutor que borra de más solo se ve ejecutando. Y lo que sobrevive se afirma
/// contra el disco, para que «borró solo su alcance» sea una afirmación sobre el resultado
/// y no sobre una lista de control.
#[test]
fn criterion_22_cleanup_scope_and_usage_error() {
    let _guard = support::exclusively();
    let runtime = support::runtime();

    // ── El gate: sin categoría, `usage_error` y nada borrado ────────────────────
    let sandbox = Sandbox::new("c22-gate");
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
        .expect_err("criterio 22: sin categoría es un error");
    let failure = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 22: el fallo declara un `reason`");
    assert_eq!(failure.reason, "usage_error");
    assert_eq!(failure.exit_code, 2, "criterio 22: error de uso (§9.1)");
    assert_eq!(
        sandbox.snapshot(),
        before,
        "criterio 22: y no borra nada, ni siquiera el barrido de §9.6"
    );

    // ── Cada categoría, en su sandbox ──────────────────────────────────────────
    // `--model`: la raíz de modelos entera (D3) y nada del estado.
    let s = Sandbox::new("c22-modelo");
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
    assert!(
        !support::exists(&s.models_dir),
        "criterio 22: --model borra los modelos"
    );
    assert!(
        support::exists(&s.data_dir.join("voices").join("mia")),
        "criterio 22: --model no toca las voces"
    );
    assert!(
        support::exists(&s.data_dir.join("config.json")),
        "criterio 22: --model no toca la configuración"
    );
    assert!(
        support::exists(&s.program_dir),
        "criterio 22: --model nunca toca el programa: eso es `self uninstall`"
    );

    // `--voices`: las voces de usuario y el arrastre de su habla.
    let s = Sandbox::new("c22-voces");
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
        !support::exists(&s.data_dir.join("voices").join("mia")),
        "criterio 22: --voices borra la voz de usuario"
    );
    assert!(
        !support::exists(&s.data_dir.join("speech").join("mia")),
        "criterio 22: y la locución que arrastra"
    );
    assert!(
        support::exists(&s.data_dir.join("voices").join("default")),
        "criterio 22: las voces de fábrica no se borran: van embebidas"
    );
    assert!(
        support::exists(&s.data_dir.join("speech").join("default")),
        "criterio 22: ni sus locuciones"
    );
    assert!(
        support::exists(&s.models_dir),
        "criterio 22: --voices no toca los modelos"
    );
    assert!(
        support::exists(&s.data_dir.join("config.json")),
        "criterio 22: ni la configuración, que es de --all"
    );

    // `--synthetic-speech`: la raíz de habla entera, `default` incluida.
    let s = Sandbox::new("c22-habla");
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
        !support::exists(&s.data_dir.join("speech")),
        "criterio 22: --synthetic-speech borra la raíz de habla entera"
    );
    assert!(
        support::exists(&s.data_dir.join("voices").join("default")),
        "criterio 22: no toca las voces"
    );
    assert!(
        support::exists(&s.models_dir),
        "criterio 22: no toca los modelos"
    );

    // `--all`: la unión más configuración, logs y estado del daemon; nunca el programa.
    let s = Sandbox::new("c22-todo");
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
    assert!(
        !support::exists(&s.models_dir),
        "criterio 22: --all borra los modelos"
    );
    assert!(
        !support::exists(&s.data_dir.join("voices").join("mia")),
        "criterio 22: y las voces de usuario"
    );
    assert!(!support::exists(&s.data_dir.join("speech")));
    for state in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !support::exists(&s.data_dir.join(state)),
            "criterio 22: --all borra {state}"
        );
    }
    assert!(
        support::exists(&s.program_dir),
        "criterio 22: --all nunca borra el programa"
    );
}

// ─── Criterio 23 ──────────────────────────────────────────────────────────────────

/// **Criterio 23.** Ninguna operación borra recursos compartidos: una caché HF
/// configurada por el usuario —salvo los repos propios—, `~/.cargo` o sccache.
///
/// La caché compartida se monta de verdad, con `HF_HUB_CACHE` y **sin** `AVI_CACHE_DIR`,
/// que es la única forma de que `avi-store` la reconozca como compartida: la variable de
/// reubicación de la aplicación tiene precedencia y su presencia volvería exclusiva una
/// raíz que el test declara compartida. Los casos se afirman contra el disco después de
/// la operación, no contra el plan.
///
/// Rutas que se declaran aquí porque las usan todas las afirmaciones del caso 1 a 7.
#[test]
fn criterion_23_shared_resources_survive() {
    let _guard = support::exclusively();
    let runtime = support::runtime();

    // ── La raíz compartida se reconoce como tal ────────────────────────────────
    let sandbox = Sandbox::new_with("c23", Models::Shared);
    sandbox.seed_env();
    assert!(
        avi_store::models_root_is_shared(),
        "criterio 23: `HF_HUB_CACHE` convierte la raíz en compartida"
    );
    assert_eq!(
        avi_store::models_cache_dir(),
        sandbox.models_dir,
        "criterio 23: y `avi-store` resuelve la misma ruta que el sandbox"
    );
    assert!(
        cleanup::is_shared_models_root(&sandbox.models_dir),
        "criterio 23: el planificador la reconoce como compartida"
    );
    assert!(sandbox.roots().models_shared);
    assert_eq!(
        sandbox.models_dir.file_name(),
        Some(std::ffi::OsStr::new("hub")),
        "criterio 23: y es el directorio `hub` del sandbox, no una ruta de la máquina"
    );

    sandbox.seed_state();
    let own_temp = sandbox.seed_own_temp();

    // Lo que R3 declara atribuible a la aplicación, y lo que nunca lo es.
    let (repo, rev) = MODEL_REVISIONS
        .iter()
        .find(|(n, _, _)| *n == "marian-es-en")
        .map(|(_, r, v)| (r.to_string(), v.to_string()))
        .expect("criterio 23: el repo de traducción está fijado");
    let our_repo = sandbox.models_dir.join(support::repo_dir(&repo));
    let our_snapshot = our_repo.join("snapshots").join(&rev);
    let our_lock = sandbox
        .models_dir
        .join(".locks")
        .join(support::repo_dir(&repo));
    let foreign = sandbox.models_dir.join("models--otra--herramienta");
    let foreign_lock = sandbox
        .models_dir
        .join(".locks")
        .join("models--otra--herramienta");
    let xet = sandbox.models_dir.join("xet");
    let locks = sandbox.models_dir.join(".locks");
    let ct2 = sandbox.models_dir.join("ct2");
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
        &ct2,
        &cargo,
        &sccache_home,
        &sccache_temp,
    ] {
        assert!(
            support::exists(required),
            "criterio 23: el punto de partida existe: {}",
            required.display()
        );
    }

    // ── Casos 1 a 7: `cleanup --model` ──────────────────────────────────────────
    let options = cleanup::Options {
        model: true,
        ..with_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &options);
    let planned_removed = support::paths(&plan);

    for (case, our) in [("1", &our_repo), ("2", &our_lock), ("3", &ct2)] {
        assert!(
            planned_removed.contains(&our.display().to_string()),
            "criterio 23, caso {case}: lo atribuible a la aplicación está en el plan: {planned_removed:?}"
        );
    }
    for (case, forbidden) in [
        ("4", &xet),
        ("5", &locks),
        ("6", &foreign),
        ("6", &sandbox.models_dir),
    ] {
        assert!(
            !planned_removed.contains(&forbidden.display().to_string()),
            "criterio 23, caso {case}: {} no puede estar en el plan: {planned_removed:?}",
            forbidden.display()
        );
    }
    // Caso 7: R3 no solo protege, el plan tiene que decirlo.
    for shared in [&sandbox.models_dir, &xet, &locks] {
        assert!(
            plan.preserved
                .iter()
                .any(|p| p.path.as_path() == shared.as_path()),
            "criterio 23, caso 7: {} se anuncia como compartido: {:?}",
            shared.display(),
            plan.preserved
        );
    }
    // R1: ninguna ruta del plan sale de las raíces declaradas.
    for dest in &plan.targets {
        assert!(
            dest.path.starts_with(&sandbox.models_dir),
            "criterio 23: R1, el destino {} sale de la raíz de modelos",
            dest.path.display()
        );
    }

    let real = runtime
        .block_on(cleanup::run(&sandbox.roots(), &options, &Inert))
        .expect("criterio 23: `cleanup --model` se ejecuta");
    assert!(real.failed.is_empty(), "criterio 23: {:?}", real.failed);
    assert_eq!(
        real.removed, planned_removed,
        "criterio 23: el plan y la ejecución coinciden bajo raíz compartida"
    );
    assert!(
        !support::exists(&our_snapshot) && !support::exists(&our_repo),
        "criterio 23, caso 1: el repo propio sí se borró"
    );
    assert!(
        !support::exists(&our_lock),
        "criterio 23, caso 2: el lock del repo propio sí se borró"
    );
    assert!(
        !support::exists(&ct2),
        "criterio 23, caso 3: el derivado `ct2` sí se borró"
    );
    assert!(
        support::exists(&xet),
        "criterio 23, caso 4: `xet` sobrevive"
    );
    assert!(
        support::exists(&locks),
        "criterio 23, caso 5: el `.locks` completo sobrevive"
    );
    assert!(
        support::exists(&foreign_lock),
        "criterio 23, caso 5: el lock de otra herramienta sobrevive"
    );
    assert!(
        support::exists(&foreign),
        "criterio 23, caso 6: el repo de otra herramienta sobrevive"
    );
    assert_eq!(
        std::fs::read_to_string(foreign.join("otro.safetensors")).ok(),
        Some("ajeno".to_string()),
        "criterio 23, caso 6: con su contenido intacto"
    );
    assert!(
        support::exists(&sandbox.models_dir),
        "criterio 23, caso 6: la raíz compartida no se borra entera"
    );
    assert!(
        support::exists(&sandbox.data_dir.join("voices").join("mia")),
        "criterio 23: `--model` no toca el estado de usuario"
    );
    // Caso 11: el barrido es selectivo por prefijo, no por directorio.
    assert!(
        !support::exists(&own_temp),
        "criterio 23, caso 11: el temporal propio sí se barre, y se anuncia: {:?}",
        real.swept
    );
    assert!(
        real.swept.contains(&own_temp.display().to_string()),
        "criterio 23, caso 11: y aparece en la lista de barrido"
    );
    for shared in [&cargo, &sccache_home, &sccache_temp] {
        assert!(
            support::exists(shared),
            "criterio 23, caso 11: {} sobrevive al barrido",
            shared.display()
        );
    }

    // ── Caso 8: `cleanup --all` ────────────────────────────────────────────────
    let s = Sandbox::new_with("c23-all", Models::Shared);
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
        .expect("criterio 23, caso 8: `cleanup --all` se ejecuta");
    assert!(
        support::exists(&s.models_dir.join("models--otra--herramienta")),
        "criterio 23, caso 8: `--all` no borra el repo ajeno"
    );
    assert!(
        support::exists(&s.models_dir.join("xet")) && support::exists(&s.models_dir.join(".locks")),
        "criterio 23, caso 8: ni `xet` ni el `.locks` completo"
    );
    assert!(
        support::exists(&s.models_dir),
        "criterio 23, caso 8: la raíz compartida sigue ahí"
    );
    assert!(
        !support::exists(&s.data_dir.join("voices").join("mia")),
        "criterio 23, caso 8: y el estado de usuario, que sí es nuestro, sí cae"
    );
    assert!(
        support::exists(&s.program_dir),
        "criterio 23, caso 8: el programa sobrevive a `cleanup`"
    );
    assert!(
        support::exists(&s.home.join(".cargo")) && support::exists(&s.temp_root.join("sccache")),
        "criterio 23, caso 11: y los compartidos del entorno"
    );

    // ── Casos 9 y 10: `self uninstall` ──────────────────────────────────────────
    let s = Sandbox::new_with("c23-uninstall", Models::Shared);
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
        .expect("criterio 23, caso 9: la desinstalación se ejecuta");
    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::exists(&s.program_dir),
        "criterio 23, caso 9: el programa sí se borra"
    );
    assert!(
        !support::exists(&s.data_dir),
        "criterio 23, caso 9: y la raíz de datos, que es exclusiva"
    );
    for shared in [
        s.models_dir.join("models--otra--herramienta"),
        s.models_dir.join("xet"),
        s.models_dir.join(".locks"),
        s.models_dir.clone(),
        s.home.join(".cargo"),
        s.temp_root.join("sccache"),
    ] {
        assert!(
            support::exists(&shared),
            "criterio 23, caso 9: {} sobrevive a la desinstalación",
            shared.display()
        );
    }
    assert!(
        outcome
            .preserved
            .iter()
            .any(|p| p.path == s.models_dir && p.reason.contains("compartid")),
        "criterio 23, caso 9: y la caché compartida se anuncia como conservada: {:?}",
        outcome.preserved
    );

    // Caso 10: `--keep-data` no toca la caché compartida en absoluto.
    let s = Sandbox::new_with("c23-keep", Models::Shared);
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
        .expect("criterio 23, caso 10: la desinstalación con `--keep-data` se ejecuta");
    assert!(
        !support::exists(&s.program_dir),
        "criterio 23, caso 10: el programa sí se va"
    );
    for shared in [
        s.models_dir
            .join("models--otra--herramienta")
            .join("otro.safetensors"),
        s.models_dir.join("xet").join("shard"),
        s.models_dir
            .join(".locks")
            .join("models--otra--herramienta")
            .join("lock"),
    ] {
        assert!(
            support::exists(&shared),
            "criterio 23, caso 10: `--keep-data` no toca la caché compartida: {}",
            shared.display()
        );
    }
    assert!(
        s.snapshot()
            .iter()
            .any(|(path, _)| path.starts_with("hub/")),
        "criterio 23, caso 10: y sigue con el contenido que tenía"
    );

    // ── El contraste: con raíz exclusiva, `xet` y `.locks` sí son nuestros ──────
    let s = Sandbox::new("c23-exclusiva");
    s.seed_env();
    s.seed_state();
    assert!(
        !avi_store::models_root_is_shared(),
        "criterio 23: sin `HF_HUB_CACHE` la raíz es exclusiva"
    );
    assert!(
        !cleanup::is_shared_models_root(&s.models_dir),
        "criterio 23: y el planificador lo sabe"
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
        .expect("criterio 23: `--model` sobre la raíz exclusiva se ejecuta");
    assert!(
        !support::exists(&s.models_dir),
        "criterio 23: en la raíz exclusiva `--model` borra el directorio entero, `xet` y \
         `.locks` incluidos, porque son de la aplicación"
    );
}
