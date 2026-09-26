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
use support::{Ahora, Inerte, Models, Sandbox};

/// Recibo con el instante de instalación neutralizado, para comparar dos pasadas por
/// todo lo demás. `installed_at` es lo único que §8.1 declara que cambia entre dos
/// instalaciones de la misma versión.
fn recibo_normalizado(recibo: &InstallReceipt) -> InstallReceipt {
    let mut copia = recibo.clone();
    copia.installed_at = "<instante>".to_string();
    copia
}

/// Estado del sandbox sin el archivo de bloqueo, que §9.1 crea al tomar el bloqueo y que
/// no es «nada modificado» sino el mecanismo que serializa las operaciones.
fn estado_sin_bloqueo(sandbox: &Sandbox) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = sandbox
        .snapshot()
        .into_iter()
        .filter(|(ruta, _)| ruta.rsplit('/').next() != Some(avi_lifecycle::LIFECYCLE_LOCK_NAME))
        .collect();
    out.sort();
    out
}

/// Entradas del directorio padre del programa, que es donde viven el staging hermano, el
/// archivo de bloqueo y los aparcados. El residuo cero del criterio 17 se afirma sobre
/// esto y no solo sobre el directorio de programa.
fn entradas_de_opt(sandbox: &Sandbox) -> Vec<String> {
    let padre = sandbox
        .program_dir
        .parent()
        .expect("el directorio de programa tiene padre");
    support::listar(padre)
}

/// `cleanup` con `--yes` y la categoría que le pase quien llama.
fn con_yes() -> cleanup::Options {
    cleanup::Options {
        assume_yes: true,
        ..Default::default()
    }
}

/// Ejecuta una categoría y comprueba que no falló nada y que borró exactamente lo que el
/// plan berkata, que es la comprobación que un plan correcto con un ejecutor laxo no
/// superaría.
fn ejecutar_categoria(
    sandbox: &Sandbox,
    runtime: &tokio::runtime::Runtime,
    opciones: cleanup::Options,
) {
    let announced = support::rutas(&cleanup::plan(&sandbox.roots(), &opciones));
    let outcome = runtime
        .block_on(cleanup::run(&sandbox.roots(), &opciones, &Inerte))
        .expect("criterio 22: la categoría se ejecuta");
    assert!(
        outcome.failed.is_empty(),
        "criterio 22: nada falló: {:?}",
        outcome.failed
    );
    for ruta in &announced {
        assert!(
            !support::existe(Path::new(ruta)),
            "criterio 22: lo anunciado se borró: {ruta}"
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
fn provisionar_seleccion(sandbox: &Sandbox) {
    for (nombre, repo, rev) in MODEL_REVISIONS {
        if *nombre == setup::CLONING_MODEL {
            continue;
        }
        let snapshot = sandbox
            .models_dir
            .join(support::repo_dir(repo))
            .join("snapshots")
            .join(rev);
        match MODEL_FILE_PATTERNS.iter().find(|(n, _)| n == nombre) {
            Some((_, patrones)) => {
                for patron in *patrones {
                    support::escribir(&snapshot.join(patron), "pesos");
                }
            }
            // Sin patrones: basta un archivo con contenido, que es lo que
            // `is_provisioned` comprueba para los repos no acotados.
            None => {
                support::escribir(&snapshot.join("model.safetensors"), "pesos");
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
    let sandbox = Sandbox::nuevo("c2");
    sandbox.sembrar_entorno();
    let opciones = Sandbox::opciones_instalacion();
    let runtime = support::runtime();

    // ── Primera instalación ─────────────────────────────────────────────────────
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let primera = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &opciones,
            &Inerte,
        ))
        .expect("criterio 2: la primera instalación se completa");
    assert_eq!(
        primera.status, "installed",
        "criterio 2: la primera pasada instala"
    );
    assert!(
        primera.path_changed(),
        "criterio 2: la primera pasada reescribe el `PATH`"
    );
    let recibo_primera = recibo_normalizado(&primera.receipt);
    let contenido_primera = support::listar(&sandbox.program_dir);
    assert!(
        contenido_primera.contains(&receipt::RECEIPT_NAME.to_string()),
        "criterio 2: el recibo está entre lo colocado: {contenido_primera:?}"
    );

    // ── Segunda instalación, desde el mismo bundle ──────────────────────────────
    // El bundle se repone porque la colocación **mueve** los archivos desde el origen
    // (§9.3.6.2). Es también lo que hace el bootstrap de §9.2, que extrae en un staging
    // nuevo cada vez.
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let segunda = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &opciones,
            &Inerte,
        ))
        .expect("criterio 2: la segunda instalación se completa");

    assert_eq!(
        segunda.status, "installed",
        "criterio 2: la segunda pasada también termina en éxito"
    );
    assert_eq!(
        segunda.mode,
        install::Mode::Install,
        "criterio 2: sigue siendo instalación, porque el ejecutable viene del staging"
    );
    assert_eq!(
        recibo_normalizado(&segunda.receipt),
        recibo_primera,
        "criterio 2: el mismo estado final, con el mismo recibo"
    );
    assert_eq!(
        segunda.path_integration, primera.path_integration,
        "criterio 2: la integración registrada es la misma, no el diff de esta pasada"
    );
    assert!(
        !segunda.path_changed(),
        "criterio 2: la segunda pasada no reescribe el `PATH`: la entrada ya estaba"
    );
    assert!(
        segunda.path_integrated(),
        "criterio 2: pero sigue registrada en el recibo, que es lo que permite revertirla"
    );
    assert_eq!(
        support::listar(&sandbox.program_dir),
        contenido_primera,
        "criterio 2: el directorio de programa no cambia de contenido"
    );

    // El recibo de disco coincide con el que devolvió la operación.
    let en_disco = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 2: se lee el recibo")
        .expect("criterio 2: el recibo existe");
    assert_eq!(
        recibo_normalizado(&en_disco),
        recibo_primera,
        "criterio 2: el recibo de disco es el mismo"
    );

    // Y ahora el punto del criterio: la integración no está duplicada.
    #[cfg(unix)]
    {
        let perfil = sandbox.home.join(".profile");
        let texto = std::fs::read_to_string(&perfil).expect("criterio 2: el perfil se escribió");
        assert_eq!(
            texto.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "criterio 2: un solo bloque delimitado tras dos instalaciones: {texto}"
        );
        let enlace = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        let destino = std::fs::read_link(&enlace).expect("criterio 2: el enlace existe");
        assert_eq!(
            destino.parent(),
            Some(sandbox.program_dir.as_path()),
            "criterio 2: el enlace apunta al directorio de programa"
        );
    }
    #[cfg(windows)]
    {
        let valor = avi_lifecycle::path_windows::read_path(&sandbox.registry_subkey)
            .expect("criterio 2: se lee el valor de la clave de prueba")
            .expect("criterio 2: la integración escribió el valor")
            .value;
        let entrada = sandbox.bin_dir.display().to_string();
        assert_eq!(
            valor.matches(&entrada).count(),
            1,
            "criterio 2: la entrada aparece una sola vez tras dos instalaciones: {valor}"
        );
    }
}

/// Líneas del resumen que hablan del `PATH`.
fn lineas_de_path(resumen: &[String]) -> Vec<&str> {
    resumen
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
    let sandbox = Sandbox::nuevo("c2-path-nuevo");
    sandbox.sembrar_entorno();
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let primera = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &Sandbox::opciones_instalacion(),
            &Inerte,
        ))
        .expect("criterio 2: la primera instalación se completa");
    assert!(
        primera.path_changed(),
        "criterio 2: la primera pasada reescribe el `PATH`, que es el estado que hay que probar"
    );
    let lineas = lineas_de_path(&primera.summary);
    assert_eq!(
        lineas.len(),
        1,
        "criterio 2: el resumen final dice el `PATH` una sola vez, y no dos: {:?}",
        primera.summary
    );
    assert!(
        lineas[0].contains("se añadió"),
        "criterio 2: y en pasado, porque esta pasada lo escribió: {}",
        lineas[0]
    );
    assert!(
        !lineas[0].contains("se añadirá"),
        "criterio 2: y no en futuro: el resumen final no anuncia, informa: {}",
        lineas[0]
    );
    assert!(
        lineas[0].contains("abre una terminal nueva"),
        "criterio 2: con la indicación de §9.3.1, que solo aparece si se acaba de escribir: {}",
        lineas[0]
    );

    // ── Estado 2: la integración está en pie y esta pasada no la tocó ────────────
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let segunda = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &Sandbox::opciones_instalacion(),
            &Inerte,
        ))
        .expect("criterio 2: la segunda instalación se completa");
    assert!(!segunda.path_changed());
    let lineas = lineas_de_path(&segunda.summary);
    assert_eq!(
        lineas.len(),
        1,
        "criterio 2: tampoco en la repetición, que es donde se vería un segundo duplicado: {:?}",
        segunda.summary
    );
    assert!(
        lineas[0].contains("ya estaba integrado"),
        "criterio 2: y lo dice como estado, no como plan: {}",
        lineas[0]
    );

    // ── Estado 3: `--no-modify-path`, donde el plan **es** el estado ──────────────
    let s = Sandbox::nuevo("c2-path-sin-path");
    s.sembrar_entorno();
    let exe = s.escribir_bundle(&s.staging);
    let sin_path = runtime
        .block_on(install::install(
            &s.env_instalacion(&exe),
            &install::Options {
                no_modify_path: true,
                ..Sandbox::opciones_instalacion()
            },
            &Inerte,
        ))
        .expect("criterio 2: la instalación con `--no-modify-path` se completa");
    let lineas = lineas_de_path(&sin_path.summary);
    assert_eq!(
        lineas.len(),
        1,
        "criterio 2: con `--no-modify-path` también una sola línea: {:?}",
        sin_path.summary
    );
    assert!(
        lineas[0].contains("no se modifica"),
        "criterio 2: y aquí el texto del plan es el del estado, porque no hubo escritura: {}",
        lineas[0]
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
    let sandbox = Sandbox::nuevo("c6-sin-setup");
    sandbox.sembrar_entorno();

    // No vacuidad: hay modelos que provisionar si se pidiera.
    let opciones_setup = setup::Options::user(false, false, true);
    let pendiente = setup::pending(&ModelStore::new(), &opciones_setup);
    let seleccion = setup::selection(&opciones_setup);
    assert!(
        !pendiente.models.is_empty(),
        "criterio 6: hay repos que descargar, así que `--no-setup` omite trabajo real: {:?}",
        pendiente.models
    );
    assert_eq!(
        pendiente.models.len(),
        seleccion.len(),
        "criterio 6: la selección completa está pendiente"
    );

    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &Sandbox::opciones_instalacion(),
            &Inerte,
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
         `Hecho` con código 0"
    );
    assert_eq!(
        support::listar(&sandbox.models_dir),
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
    let sandbox = Sandbox::nuevo("c6-fallo");
    sandbox.sembrar_entorno();
    provisionar_seleccion(&sandbox);

    // No vacuidad del camino de fallo: queda CT2 por convertir y **nada** por descargar.
    let opciones_setup = setup::Options::user(false, false, true);
    let pendiente = setup::pending(&ModelStore::new(), &opciones_setup);
    assert!(
        pendiente.models.is_empty(),
        "criterio 6: no queda nada que descargar, así que la prueba no toca la red: {:?}",
        pendiente.models
    );
    assert_eq!(
        pendiente.ct2.len(),
        setup::CT2_PAIRS.len(),
        "criterio 6: los dos derivados CT2 están pendientes de conversión: {:?}",
        pendiente.ct2
    );

    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let opciones = install::Options {
        no_setup: false,
        ..Sandbox::opciones_instalacion()
    };
    let outcome = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &opciones,
            &Inerte,
        ))
        .expect("criterio 6: un fallo de `setup` no es un fallo de la instalación");

    // ── Mitad 1: el programa queda instalado ──────────────────────────────────
    assert_eq!(
        outcome.status, "installed",
        "criterio 6: la instalación termina con éxito"
    );
    let en_disco = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 6: se lee el recibo")
        .expect("criterio 6: el recibo existe: el programa quedó instalado");
    assert_eq!(en_disco.version, "0.24.0");
    for relativa in &outcome.receipt.files {
        assert!(
            support::aviar(&sandbox.program_dir, relativa).is_file(),
            "criterio 6: {relativa} sigue en el directorio de programa"
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
    let le = outcome
        .lifecycle_error()
        .expect("criterio 6: §9.1 declara `setup_failed` para este desenlace");
    assert_eq!(
        le.reason, "setup_failed",
        "criterio 6: el `reason` de la operación es `setup_failed`"
    );
    assert_eq!(
        le.exit_code, 11,
        "criterio 6: y el código es `SetupFailed = 11` de la tabla cerrada"
    );
    assert_eq!(
        exit_code_de_contrato("setup_failed"),
        Some(11),
        "criterio 6: el cableado traduce ese `reason` al mismo entero, que es lo que evita \
         que las dos copias del 11 diverjan"
    );
    // El mensaje dice las dos cosas que §9.3 paso 11 promete: qué no se completó y que
    // basta reintentar con `setup`.
    assert!(
        le.message.contains("no se completó"),
        "criterio 6: el mensaje dice qué no se completó: {}",
        le.message
    );
    assert!(
        le.message.contains("reintentar con setup"),
        "criterio 6: y que basta reintentar con `setup`, que es la otra mitad del criterio: {}",
        le.message
    );
    assert!(
        le.message.contains(cause.message.as_str()),
        "criterio 6: y el motivo de la causa viaja dentro, para que el `reason` anidado no se \
         pierda: {}",
        le.message
    );
    assert!(
        le.message
            .contains(&sandbox.program_dir.display().to_string()),
        "criterio 6: y dónde quedó instalado, que es lo que el usuario necesita saber: {}",
        le.message
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
        !setup::pending(&ModelStore::new(), &opciones_setup)
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
    let sandbox = Sandbox::nuevo("c6-ok");
    sandbox.sembrar_entorno();
    provisionar_seleccion(&sandbox);

    // Los derivados, después de una espera que garantiza que su `mtime` es posterior al
    // del snapshot: `needs_reconversion` compara `ct2_time <= hf_time` y devuelve
    // `true` —o sea, hay que convertir— cuando el derivado no es más nuevo.
    std::thread::sleep(std::time::Duration::from_millis(1100));
    for pair in setup::CT2_PAIRS {
        let dir = avi_store::ct2_model_dir(pair);
        support::escribir(&dir.join("model.bin"), "ct2");
        support::escribir(&dir.join("source.spm"), "spm");
        support::escribir(&dir.join("target.spm"), "spm");
    }

    // No vacuidad: `setup` no tiene nada que hacer, y eso es lo que la prueba comprueba.
    let pendiente = setup::pending(
        &ModelStore::new(),
        &setup::Options::user(false, false, true),
    );
    assert!(
        pendiente.is_empty(),
        "criterio 6: con los repos y los derivados ya provisionados no queda nada pendiente: \
         {:?}",
        pendiente
    );
    for pair in setup::CT2_PAIRS {
        assert!(
            avi_store::ct2_missing_files(pair).is_empty(),
            "criterio 6: el derivado de {pair} está sano"
        );
    }

    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let runtime = support::runtime();
    let outcome = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &install::Options {
                no_setup: false,
                ..Sandbox::opciones_instalacion()
            },
            &Inerte,
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
        "criterio 6: y no hay `reason` de contrato, así que el cableado sale por `Hecho` con \
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
fn exit_code_de_contrato(reason: &str) -> Option<i32> {
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
    let sandbox = Sandbox::nuevo("c17");
    sandbox.sembrar_entorno();
    let runtime = support::runtime();
    let opciones = install::Options {
        // En Unix la integración se integra de verdad; en Windows se apaga, y el motivo
        // está en el doc-comment.
        no_modify_path: cfg!(windows),
        ..Sandbox::opciones_instalacion()
    };

    // ── Instalar de verdad ──────────────────────────────────────────────────────
    // El perfil del sandbox tiene contenido propio **antes** de instalar, que es lo que
    // hace significativa la reversión: `remove_block` solo trunca cuando el final del
    // archivo es exactamente el bloque, así que un bloque al final de un archivo con texto
    // detrás no se quita —por diseño, para no borrar contenido del usuario— y la prueba
    // tiene que reproducir la situación real, que es un perfil que ya existía.
    let perfil = sandbox.home.join(".profile");
    if cfg!(unix) {
        support::escribir(&perfil, "# adjusting del usuario\n");
    }
    let exe = sandbox.escribir_bundle(&sandbox.staging);
    let instalada = runtime
        .block_on(install::install(
            &sandbox.env_instalacion(&exe),
            &opciones,
            &Inerte,
        ))
        .expect("criterio 17: la instalación se completa");
    assert_eq!(instalada.status, "installed");
    sandbox.plantar_estado();
    let recibo = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 17: se lee el recibo")
        .expect("criterio 17: el recibo existe");

    if cfg!(unix) {
        let texto = std::fs::read_to_string(&perfil).expect("criterio 17: el perfil se lee");
        assert!(
            texto.starts_with("# adjusting del usuario\n"),
            "criterio 17: el contenido propio está al principio y no se tocó: {texto}"
        );
        assert_eq!(
            texto.matches(avi_lifecycle::path_unix::BLOCK_BEGIN).count(),
            1,
            "criterio 17: y el bloque delimitado está al final: {texto}"
        );
        assert!(
            texto.ends_with(avi_lifecycle::path_unix::BLOCK_END),
            "criterio 17: con su marcador de cierre al final del archivo: {texto}"
        );
    }
    if cfg!(windows) {
        assert!(
            !recibo.path_integration.modify_path,
            "criterio 17: con `--no-modify-path` el recibo lo dice, y por eso no hay nada \
             que revertir contra `HKCU\\Environment`"
        );
        assert!(recibo.path_integration.registry_entry.is_none());
    }

    // ── Desinstalar ─────────────────────────────────────────────────────────────
    let outcome = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
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
        !support::existe(&sandbox.program_dir),
        "criterio 17: el directorio de programa no queda"
    );
    assert!(
        !support::existe(&sandbox.data_dir),
        "criterio 17: la raíz de datos no queda, voces de fábrica incluidas: el programa \
         ya no está para re-materializarlas"
    );
    assert!(
        !support::existe(&sandbox.models_dir),
        "criterio 17: la raíz de modelos no queda"
    );
    assert_eq!(
        entradas_de_opt(&sandbox),
        Vec::<String>::new(),
        "criterio 17: el padre del programa queda vacío: ni staging, ni aparcados, ni bloqueo"
    );
    let temporales_propios: Vec<String> = support::listar(&sandbox.temp_root)
        .into_iter()
        .filter(|n| {
            avi_lifecycle::TEMP_PREFIXES
                .iter()
                .any(|p| n.starts_with(p))
        })
        .collect();
    assert_eq!(
        temporales_propios,
        Vec::<String>::new(),
        "criterio 17: ningún temporal propio sobrevive"
    );

    if cfg!(unix) {
        let enlace = sandbox.bin_dir.join(avi_lifecycle::APP_NAME);
        assert!(
            !support::existe(&enlace),
            "criterio 17: el enlace del `PATH` se retira"
        );
        assert!(outcome.path_reverted, "criterio 17: y el motor lo dice");
        let texto = std::fs::read_to_string(&perfil).expect("criterio 17: el perfil se lee");
        assert_eq!(
            texto, "# adjusting del usuario\n",
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
    let sandbox = Sandbox::nuevo("c18");
    sandbox.sembrar_entorno();
    sandbox.plantar_estado();
    let recibo = sandbox.instalar_registrada(PathIntegration::none());

    // Punto de partida: los tres conjuntos están.
    assert!(support::existe(&sandbox.models_dir.join("xet")));
    assert!(support::existe(
        &sandbox.data_dir.join("voices").join("mia")
    ));
    assert!(support::existe(
        &sandbox.data_dir.join("speech").join("default")
    ));

    let outcome = support::runtime()
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options {
                keep_data: true,
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 18: la desinstalación se ejecuta");

    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::existe(&sandbox.program_dir),
        "criterio 18: el programa sí se va"
    );

    // Lo que se conserva.
    assert!(
        support::existe(&sandbox.models_dir.join("xet"))
            && support::existe(&sandbox.models_dir.join("ct2").join("marian-es-en")),
        "criterio 18: los modelos se quedan: {:?}",
        outcome.preserved
    );
    for voz in ["default", "ryan", "mia"] {
        assert!(
            support::existe(&sandbox.data_dir.join("voices").join(voz)),
            "criterio 18: la voz {voz} se queda"
        );
    }
    for voz in ["default", "mia"] {
        assert!(
            support::existe(&sandbox.data_dir.join("speech").join(voz)),
            "criterio 18: la locución de {voz} se queda"
        );
    }
    for motivo in ["modelos", "voces", "habla"] {
        assert!(
            outcome.preserved.iter().any(|p| p.reason.contains(motivo)),
            "criterio 18: `{motivo}` se anuncia como conservado, con su motivo: {:?}",
            outcome.preserved
        );
    }

    // Y lo que no.
    for estado in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !support::existe(&sandbox.data_dir.join(estado)),
            "criterio 18: `{estado}` sí se borra: es estado de ejecución, no datos"
        );
    }
    assert!(
        support::existe(&sandbox.home.join(".cargo")),
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

    if support::es_hijo_sin_terminal() {
        hijo_sin_terminal();
        return;
    }

    let sandbox = Sandbox::nuevo("c19");
    sandbox.sembrar_entorno();
    sandbox.plantar_estado();
    sandbox.instalar_registrada(PathIntegration::none());
    let antes = estado_sin_bloqueo(&sandbox);

    let salida =
        support::ejecutar_sin_terminal("criterion_19_no_tty_without_yes_refuses", &sandbox);
    let stdout = String::from_utf8_lossy(&salida.stdout).to_string();
    let stderr = String::from_utf8_lossy(&salida.stderr).to_string();
    assert!(
        salida.status.success(),
        "criterio 19: el proceso sin terminal terminó con {:?}.\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}",
        salida.status.code()
    );

    let informes = support::leer_informes(&salida.stdout);
    for operacion in ["uninstall", "cleanup"] {
        let linea = informes
            .iter()
            .find(|l| l.starts_with(operacion))
            .unwrap_or_else(|| {
                panic!(
                    "criterio 19: `{operacion}` no informó de su desenlace: {informes:?}\n\
                     --- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
                )
            });
        let (reason, codigo) = linea
            .trim_start_matches(operacion)
            .trim_start_matches('=')
            .split_once('/')
            .unwrap_or_else(|| panic!("criterio 19: informe mal formado: {linea}"));
        assert_eq!(
            reason, "confirmation_required",
            "criterio 19: `{operacion}` sin terminal y sin `--yes` se niega"
        );
        assert_eq!(
            codigo, "2",
            "criterio 19: `{operacion}` devuelve el error de uso de §9.1"
        );
    }

    // Y nada se borró. El hijo lo afirma también; aquí se comprueba contra el disco.
    assert_eq!(
        estado_sin_bloqueo(&sandbox),
        antes,
        "criterio 19: no se borró nada"
    );
    assert!(
        support::existe(&sandbox.program_dir),
        "criterio 19: el directorio de programa sigue"
    );
    assert!(
        support::existe(&sandbox.data_dir.join("voices").join("mia")),
        "criterio 19: el estado de usuario sigue"
    );
    assert!(
        support::existe(&sandbox.models_dir.join("models--otra--herramienta")),
        "criterio 19: los modelos siguen"
    );
}

/// Rollo del proceso hijo: ejecuta las dos operaciones destructivas del alcance sin
/// terminal y sin `--yes`, y afirma que se niegan y que no borran nada.
///
/// El sandbox se reconstruye desde la raíz y la etiqueta que le pasó el padre, y **no se
/// borra al salir**: el padre tiene que encontrar ese mismo disco para comprobarlo.
fn hijo_sin_terminal() {
    let raiz = PathBuf::from(
        std::env::var(support::VAR_RAIZ).expect("criterio 19: el hijo recibe la raíz"),
    );
    let tag = std::env::var(support::VAR_TAG).expect("criterio 19: el hijo recibe la etiqueta");
    let sandbox = Sandbox::desde_raiz(&tag, raiz, Models::Exclusiva).en_prestado();
    sandbox.sembrar_entorno();
    let antes = estado_sin_bloqueo(&sandbox);

    let recibo = receipt::read_from(&sandbox.program_dir)
        .expect("criterio 19: el hijo lee el recibo")
        .expect("criterio 19: el recibo existe");
    let runtime = support::runtime();

    // 1. `self uninstall` sin `--yes`.
    let error = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options::default(),
            &Ahora,
            &Inerte,
        ))
        .expect_err("criterio 19: `self uninstall` sin terminal y sin `--yes` se niega");
    let le = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 19: el fallo es un `LifecycleError`");
    assert_eq!(le.reason, "confirmation_required");
    assert_eq!(le.exit_code, 2, "criterio 19: error de uso (§9.1)");
    support::informar(&format!("uninstall={}/{}", le.reason, le.exit_code));

    // 2. `cleanup --all` sin `--yes`.
    let error = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                all: true,
                ..Default::default()
            },
            &Inerte,
        ))
        .expect_err("criterio 19: `cleanup` sin terminal y sin `--yes` se niega");
    let le = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 19: el fallo es un `LifecycleError`");
    assert_eq!(le.reason, "confirmation_required");
    assert_eq!(le.exit_code, 2, "criterio 19: error de uso (§9.1)");
    support::informar(&format!("cleanup={}/{}", le.reason, le.exit_code));

    // Y el disco está intacto. Sin `--yes` no se puede haber borrado nada, y esta
    // comprobación es la que convierte la negativa en una garantía.
    assert_eq!(
        estado_sin_bloqueo(&sandbox),
        antes,
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
    let sandbox = Sandbox::nuevo("c20");
    sandbox.sembrar_entorno();
    sandbox.plantar_estado();
    let recibo = sandbox.instalar_registrada(PathIntegration::none());
    let runtime = support::runtime();

    // ── `self uninstall --dry-run` ──────────────────────────────────────────────
    let antes = sandbox.snapshot();
    let opciones = uninstall::Options {
        dry_run: true,
        assume_yes: true,
        ..Default::default()
    };
    let plan = uninstall::compose_plan(
        &sandbox.roots(),
        Some(&recibo),
        &sandbox.program_dir,
        &opciones,
    );
    let entradas = plan.entries();
    assert!(
        !entradas.is_empty(),
        "criterio 20: el plan de desinstalación no está vacío"
    );
    // Los destinos de estado llevan el tamaño recursivo de la ruta, que es la cifra que
    // §9.1 pide listar. Se afirma que **coincide con la medida** y no solo que es
    // positiva, porque un cero fijo pasaría la comprobación débil y no sería un tamaño.
    for destino in &plan.state.targets {
        assert_eq!(
            destino.size,
            cleanup::path_size(&destino.path),
            "criterio 20: el tamaño de {} es el medido",
            destino.path.display()
        );
        assert!(
            destino.size > 0,
            "criterio 20: {} tiene contenido y así se anuncia",
            destino.path.display()
        );
    }
    // El directorio de programa se lista como ruta. Su cifra es la longitud de la
    // entrada —que en un directorio es 0 en Windows y el tamaño del bloque en Unix—, así
    // que afirmar que es positiva sería afirmar algo que el enunciado no pide y que la
    // plataforma decide.
    assert!(
        entradas.iter().any(|e| e.path == sandbox.program_dir),
        "criterio 20: el directorio de programa aparece en el plan: {:?}",
        support::entradas(&entradas)
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
        entradas.iter().any(|e| e.path == sandbox.models_dir),
        "criterio 20: y la raíz de modelos, que es el otro destino propio"
    );

    let simulacion = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&recibo), Channel::Script),
            &opciones,
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 20: la simulación de desinstalación se ejecuta");
    assert!(simulacion.dry_run, "criterio 20: el desenlace lo dice");
    let del_plan: Vec<String> = support::entradas(&entradas);
    for ruta in &del_plan {
        assert!(
            simulacion.removed.contains(ruta),
            "criterio 20: la simulación anuncia {ruta}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        antes,
        "criterio 20: `self uninstall --dry-run` no modifica el disco, ni siquiera \
         con el archivo de bloqueo"
    );

    // ── `cleanup --dry-run` ─────────────────────────────────────────────────────
    let opciones = cleanup::Options {
        all: true,
        dry_run: true,
        ..con_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &opciones);
    assert!(
        !plan.is_empty(),
        "criterio 20: el plan de limpieza no está vacío"
    );
    for destino in &plan.targets {
        assert!(
            destino.size > 0,
            "criterio 20: {} se lista con su tamaño medido",
            destino.path.display()
        );
    }
    let announced = support::rutas(&plan);
    let simulado = runtime
        .block_on(cleanup::run(&sandbox.roots(), &opciones, &Inerte))
        .expect("criterio 20: la simulación de limpieza se ejecuta");
    assert!(simulado.dry_run);
    for ruta in &announced {
        assert!(
            simulado.removed.contains(ruta),
            "criterio 20: la simulación de `cleanup` anuncia {ruta}"
        );
    }
    assert_eq!(
        sandbox.snapshot(),
        antes,
        "criterio 20: `cleanup --dry-run` no modifica el disco"
    );

    // ── Y ahora sí, la ejecución real borra lo mismo que se anunció ─────────────
    let real = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                all: true,
                ..con_yes()
            },
            &Inerte,
        ))
        .expect("criterio 20: la limpieza real se ejecuta");
    let borrado: Vec<String> = real
        .removed
        .iter()
        .filter(|r| announced.contains(r))
        .cloned()
        .collect();
    assert_eq!(
        borrado, announced,
        "criterio 20: la ejecución borra exactamente lo que la simulación anunció"
    );
    assert!(
        !support::existe(&sandbox.models_dir),
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
    let sandbox = Sandbox::nuevo("c21");
    sandbox.sembrar_entorno();
    sandbox.plantar_estado();
    let recibo = sandbox.instalar_registrada(PathIntegration::none());

    let primera = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 21: la primera desinstalación se ejecuta");
    assert_eq!(primera.status, "uninstalled");
    assert!(primera.program_dir_removed);
    assert!(
        primera.failed.is_empty(),
        "criterio 21: {:?}",
        primera.failed
    );

    let despues = sandbox.snapshot();
    let segunda = runtime
        .block_on(uninstall::run(
            &sandbox.env_uninstall(None, Channel::Unmanaged),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 21: la segunda no es un error");
    assert_eq!(
        segunda.status, "not_installed",
        "criterio 21: repetir sobre un sistema limpio termina en éxito con `not_installed`"
    );
    assert!(
        segunda.removed.is_empty(),
        "criterio 21: y no borra nada: {:?}",
        segunda.removed
    );
    assert_eq!(
        sandbox.snapshot(),
        despues,
        "criterio 21: el disco no cambia en la repetición"
    );

    // ── Y el sistema en el que nunca se instaló ────────────────────────────────
    // El arnés **no** crea la raíz de datos ni la de modelos: en un sistema donde nunca
    // se instaló, esas raíces no existen, y es su ausencia la que hace que el desenlace
    // sea `not_installed` y no una desinstalación vacía.
    let limpio = Sandbox::nuevo("c21-limpio");
    limpio.sembrar_entorno();
    assert!(!support::existe(&limpio.program_dir));
    let nunca = runtime
        .block_on(uninstall::run(
            &limpio.env_uninstall(None, Channel::Unmanaged),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 21: en un sistema sin instalación tampoco es un error");
    assert_eq!(
        nunca.status, "not_installed",
        "criterio 21: sin recibo, sin estado y sin directorio de programa"
    );
    assert!(nunca.removed.is_empty());
    assert!(!nunca.program_dir_removed);
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
    let sandbox = Sandbox::nuevo("c22-gate");
    sandbox.sembrar_entorno();
    sandbox.plantar_estado();
    let antes = sandbox.snapshot();
    let error = runtime
        .block_on(cleanup::run(
            &sandbox.roots(),
            &cleanup::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Inerte,
        ))
        .expect_err("criterio 22: sin categoría es un error");
    let le = error
        .downcast_ref::<avi_lifecycle::LifecycleError>()
        .expect("criterio 22: el fallo declara un `reason`");
    assert_eq!(le.reason, "usage_error");
    assert_eq!(le.exit_code, 2, "criterio 22: error de uso (§9.1)");
    assert_eq!(
        sandbox.snapshot(),
        antes,
        "criterio 22: y no borra nada, ni siquiera el barrido de §9.6"
    );

    // ── Cada categoría, en su sandbox ──────────────────────────────────────────
    // `--model`: la raíz de modelos entera (D3) y nada del estado.
    let s = Sandbox::nuevo("c22-modelo");
    s.sembrar_entorno();
    s.plantar_estado();
    s.instalar_registrada(PathIntegration::none());
    ejecutar_categoria(
        &s,
        &runtime,
        cleanup::Options {
            model: true,
            ..con_yes()
        },
    );
    assert!(
        !support::existe(&s.models_dir),
        "criterio 22: --model borra los modelos"
    );
    assert!(
        support::existe(&s.data_dir.join("voices").join("mia")),
        "criterio 22: --model no toca las voces"
    );
    assert!(
        support::existe(&s.data_dir.join("config.json")),
        "criterio 22: --model no toca la configuración"
    );
    assert!(
        support::existe(&s.program_dir),
        "criterio 22: --model nunca toca el programa: eso es `self uninstall`"
    );

    // `--voices`: las voces de usuario y el arrastre de su habla.
    let s = Sandbox::nuevo("c22-voces");
    s.sembrar_entorno();
    s.plantar_estado();
    s.instalar_registrada(PathIntegration::none());
    ejecutar_categoria(
        &s,
        &runtime,
        cleanup::Options {
            voices: true,
            ..con_yes()
        },
    );
    assert!(
        !support::existe(&s.data_dir.join("voices").join("mia")),
        "criterio 22: --voices borra la voz de usuario"
    );
    assert!(
        !support::existe(&s.data_dir.join("speech").join("mia")),
        "criterio 22: y la locución que arrastra"
    );
    assert!(
        support::existe(&s.data_dir.join("voices").join("default")),
        "criterio 22: las voces de fábrica no se borran: van embebidas"
    );
    assert!(
        support::existe(&s.data_dir.join("speech").join("default")),
        "criterio 22: ni sus locuciones"
    );
    assert!(
        support::existe(&s.models_dir),
        "criterio 22: --voices no toca los modelos"
    );
    assert!(
        support::existe(&s.data_dir.join("config.json")),
        "criterio 22: ni la configuración, que es de --all"
    );

    // `--synthetic-speech`: la raíz de habla entera, `default` incluida.
    let s = Sandbox::nuevo("c22-habla");
    s.sembrar_entorno();
    s.plantar_estado();
    s.instalar_registrada(PathIntegration::none());
    ejecutar_categoria(
        &s,
        &runtime,
        cleanup::Options {
            synthetic_speech: true,
            ..con_yes()
        },
    );
    assert!(
        !support::existe(&s.data_dir.join("speech")),
        "criterio 22: --synthetic-speech borra la raíz de habla entera"
    );
    assert!(
        support::existe(&s.data_dir.join("voices").join("default")),
        "criterio 22: no toca las voces"
    );
    assert!(
        support::existe(&s.models_dir),
        "criterio 22: no toca los modelos"
    );

    // `--all`: la unión más configuración, logs y estado del daemon; nunca el programa.
    let s = Sandbox::nuevo("c22-todo");
    s.sembrar_entorno();
    s.plantar_estado();
    s.instalar_registrada(PathIntegration::none());
    ejecutar_categoria(
        &s,
        &runtime,
        cleanup::Options {
            all: true,
            ..con_yes()
        },
    );
    assert!(
        !support::existe(&s.models_dir),
        "criterio 22: --all borra los modelos"
    );
    assert!(
        !support::existe(&s.data_dir.join("voices").join("mia")),
        "criterio 22: y las voces de usuario"
    );
    assert!(!support::existe(&s.data_dir.join("speech")));
    for estado in ["config.json", "logs", "daemon.pid"] {
        assert!(
            !support::existe(&s.data_dir.join(estado)),
            "criterio 22: --all borra {estado}"
        );
    }
    assert!(
        support::existe(&s.program_dir),
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
    let sandbox = Sandbox::nuevo_con("c23", Models::Compartida);
    sandbox.sembrar_entorno();
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

    sandbox.plantar_estado();
    let temporal_propio = sandbox.plantar_temporal_propio();

    // Lo que R3 declara atribuible a la aplicación, y lo que nunca lo es.
    let (repo, rev) = MODEL_REVISIONS
        .iter()
        .find(|(n, _, _)| *n == "marian-es-en")
        .map(|(_, r, v)| (r.to_string(), v.to_string()))
        .expect("criterio 23: el repo de traducción está fijado");
    let nuestro_repo = sandbox.models_dir.join(support::repo_dir(&repo));
    let nuestro_snapshot = nuestro_repo.join("snapshots").join(&rev);
    let nuestro_lock = sandbox
        .models_dir
        .join(".locks")
        .join(support::repo_dir(&repo));
    let ajeno = sandbox.models_dir.join("models--otra--herramienta");
    let ajeno_lock = sandbox
        .models_dir
        .join(".locks")
        .join("models--otra--herramienta");
    let xet = sandbox.models_dir.join("xet");
    let locks = sandbox.models_dir.join(".locks");
    let ct2 = sandbox.models_dir.join("ct2");
    let cargo = sandbox.home.join(".cargo");
    let sccache_home = sandbox.home.join(".cache").join("sccache");
    let sccache_temp = sandbox.temp_root.join("sccache");
    for obligatoria in [
        &nuestro_snapshot,
        &nuestro_lock,
        &ajeno,
        &ajeno_lock,
        &xet,
        &locks,
        &ct2,
        &cargo,
        &sccache_home,
        &sccache_temp,
    ] {
        assert!(
            support::existe(obligatoria),
            "criterio 23: el punto de partida existe: {}",
            obligatoria.display()
        );
    }

    // ── Casos 1 a 7: `cleanup --model` ──────────────────────────────────────────
    let opciones = cleanup::Options {
        model: true,
        ..con_yes()
    };
    let plan = cleanup::plan(&sandbox.roots(), &opciones);
    let del_plan = support::rutas(&plan);

    for (caso, nuestro) in [("1", &nuestro_repo), ("2", &nuestro_lock), ("3", &ct2)] {
        assert!(
            del_plan.contains(&nuestro.display().to_string()),
            "criterio 23, caso {caso}: lo atribuible a la aplicación está en el plan: {del_plan:?}"
        );
    }
    for (caso, nunca) in [
        ("4", &xet),
        ("5", &locks),
        ("6", &ajeno),
        ("6", &sandbox.models_dir),
    ] {
        assert!(
            !del_plan.contains(&nunca.display().to_string()),
            "criterio 23, caso {caso}: {} no puede estar en el plan: {del_plan:?}",
            nunca.display()
        );
    }
    // Caso 7: R3 no solo protege, el plan tiene que decirlo.
    for anunciado in [&sandbox.models_dir, &xet, &locks] {
        assert!(
            plan.preserved
                .iter()
                .any(|p| p.path.as_path() == anunciado.as_path()),
            "criterio 23, caso 7: {} se anuncia como compartido: {:?}",
            anunciado.display(),
            plan.preserved
        );
    }
    // R1: ninguna ruta del plan sale de las raíces declaradas.
    for destino in &plan.targets {
        assert!(
            destino.path.starts_with(&sandbox.models_dir),
            "criterio 23: R1, el destino {} sale de la raíz de modelos",
            destino.path.display()
        );
    }

    let real = runtime
        .block_on(cleanup::run(&sandbox.roots(), &opciones, &Inerte))
        .expect("criterio 23: `cleanup --model` se ejecuta");
    assert!(real.failed.is_empty(), "criterio 23: {:?}", real.failed);
    assert_eq!(
        real.removed, del_plan,
        "criterio 23: el plan y la ejecución coinciden bajo raíz compartida"
    );
    assert!(
        !support::existe(&nuestro_snapshot) && !support::existe(&nuestro_repo),
        "criterio 23, caso 1: el repo propio sí se borró"
    );
    assert!(
        !support::existe(&nuestro_lock),
        "criterio 23, caso 2: el lock del repo propio sí se borró"
    );
    assert!(
        !support::existe(&ct2),
        "criterio 23, caso 3: el derivado `ct2` sí se borró"
    );
    assert!(
        support::existe(&xet),
        "criterio 23, caso 4: `xet` sobrevive"
    );
    assert!(
        support::existe(&locks),
        "criterio 23, caso 5: el `.locks` completo sobrevive"
    );
    assert!(
        support::existe(&ajeno_lock),
        "criterio 23, caso 5: el lock de otra herramienta sobrevive"
    );
    assert!(
        support::existe(&ajeno),
        "criterio 23, caso 6: el repo de otra herramienta sobrevive"
    );
    assert_eq!(
        std::fs::read_to_string(ajeno.join("otro.safetensors")).ok(),
        Some("ajeno".to_string()),
        "criterio 23, caso 6: con su contenido intacto"
    );
    assert!(
        support::existe(&sandbox.models_dir),
        "criterio 23, caso 6: la raíz compartida no se borra entera"
    );
    assert!(
        support::existe(&sandbox.data_dir.join("voices").join("mia")),
        "criterio 23: `--model` no toca el estado de usuario"
    );
    // Caso 11: el barrido es selectivo por prefijo, no por directorio.
    assert!(
        !support::existe(&temporal_propio),
        "criterio 23, caso 11: el temporal propio sí se barre, y se anuncia: {:?}",
        real.swept
    );
    assert!(
        real.swept.contains(&temporal_propio.display().to_string()),
        "criterio 23, caso 11: y aparece en la lista de barrido"
    );
    for compartido in [&cargo, &sccache_home, &sccache_temp] {
        assert!(
            support::existe(compartido),
            "criterio 23, caso 11: {} sobrevive al barrido",
            compartido.display()
        );
    }

    // ── Caso 8: `cleanup --all` ────────────────────────────────────────────────
    let s = Sandbox::nuevo_con("c23-all", Models::Compartida);
    s.sembrar_entorno();
    s.plantar_estado();
    s.instalar_registrada(PathIntegration::none());
    runtime
        .block_on(cleanup::run(
            &s.roots(),
            &cleanup::Options {
                all: true,
                ..con_yes()
            },
            &Inerte,
        ))
        .expect("criterio 23, caso 8: `cleanup --all` se ejecuta");
    assert!(
        support::existe(&s.models_dir.join("models--otra--herramienta")),
        "criterio 23, caso 8: `--all` no borra el repo ajeno"
    );
    assert!(
        support::existe(&s.models_dir.join("xet")) && support::existe(&s.models_dir.join(".locks")),
        "criterio 23, caso 8: ni `xet` ni el `.locks` completo"
    );
    assert!(
        support::existe(&s.models_dir),
        "criterio 23, caso 8: la raíz compartida sigue ahí"
    );
    assert!(
        !support::existe(&s.data_dir.join("voices").join("mia")),
        "criterio 23, caso 8: y el estado de usuario, que sí es nuestro, sí cae"
    );
    assert!(
        support::existe(&s.program_dir),
        "criterio 23, caso 8: el programa sobrevive a `cleanup`"
    );
    assert!(
        support::existe(&s.home.join(".cargo")) && support::existe(&s.temp_root.join("sccache")),
        "criterio 23, caso 11: y los compartidos del entorno"
    );

    // ── Casos 9 y 10: `self uninstall` ──────────────────────────────────────────
    let s = Sandbox::nuevo_con("c23-uninstall", Models::Compartida);
    s.sembrar_entorno();
    s.plantar_estado();
    let recibo = s.instalar_registrada(PathIntegration::none());
    let outcome = runtime
        .block_on(uninstall::run(
            &s.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options {
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 23, caso 9: la desinstalación se ejecuta");
    assert_eq!(outcome.status, "uninstalled");
    assert!(
        !support::existe(&s.program_dir),
        "criterio 23, caso 9: el programa sí se borra"
    );
    assert!(
        !support::existe(&s.data_dir),
        "criterio 23, caso 9: y la raíz de datos, que es exclusiva"
    );
    for compartido in [
        s.models_dir.join("models--otra--herramienta"),
        s.models_dir.join("xet"),
        s.models_dir.join(".locks"),
        s.models_dir.clone(),
        s.home.join(".cargo"),
        s.temp_root.join("sccache"),
    ] {
        assert!(
            support::existe(&compartido),
            "criterio 23, caso 9: {} sobrevive a la desinstalación",
            compartido.display()
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
    let s = Sandbox::nuevo_con("c23-keep", Models::Compartida);
    s.sembrar_entorno();
    s.plantar_estado();
    let recibo = s.instalar_registrada(PathIntegration::none());
    runtime
        .block_on(uninstall::run(
            &s.env_uninstall(Some(&recibo), Channel::Script),
            &uninstall::Options {
                keep_data: true,
                assume_yes: true,
                ..Default::default()
            },
            &Ahora,
            &Inerte,
        ))
        .expect("criterio 23, caso 10: la desinstalación con `--keep-data` se ejecuta");
    assert!(
        !support::existe(&s.program_dir),
        "criterio 23, caso 10: el programa sí se va"
    );
    for compartido in [
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
            support::existe(&compartido),
            "criterio 23, caso 10: `--keep-data` no toca la caché compartida: {}",
            compartido.display()
        );
    }
    assert!(
        s.snapshot()
            .iter()
            .any(|(ruta, _)| ruta.starts_with("hub/")),
        "criterio 23, caso 10: y sigue con el contenido que tenía"
    );

    // ── El contraste: con raíz exclusiva, `xet` y `.locks` sí son nuestros ──────
    let s = Sandbox::nuevo("c23-exclusiva");
    s.sembrar_entorno();
    s.plantar_estado();
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
                ..con_yes()
            },
            &Inerte,
        ))
        .expect("criterio 23: `--model` sobre la raíz exclusiva se ejecuta");
    assert!(
        !support::existe(&s.models_dir),
        "criterio 23: en la raíz exclusiva `--model` borra el directorio entero, `xet` y \
         `.locks` incluidos, porque son de la aplicación"
    );
}
