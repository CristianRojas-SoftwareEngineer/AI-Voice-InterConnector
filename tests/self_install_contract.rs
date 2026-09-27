//! Contrato de `self install` cuando la provisión de modelos falla: `setup_failed` como
//! **éxito parcial**, que es el paso 11 de la instalación y el criterio 6 del plan.
//!
//! ### Por qué este archivo existe y no está en `cli_golden.rs`
//!
//! La mitad del criterio que se puede comprobar desde el motor ya la cubre
//! `crates/avi-lifecycle/tests/acceptance.rs::criterion_6_setup_failure_keeps_install`: el
//! programa queda instalado, el estado del desenlace es un fallo de provisión y
//! `Outcome::lifecycle_error()` da `setup_failed` con el 11 de la tabla cerrada. Lo que **no**
//! se puede observar desde el motor es el `reason` del sobre ni el **código de salida del
//! proceso**, y eso solo se ve ejecutando el binario.
//!
//! Y aquí no cabe en `cli_golden.rs`, por una razón que semidió en vez de supusieron: el
//! fallo de provisión se provoca con la conversión de CT2, que **invoca `python`**, y en una
//! máquina con `ctranslate2` y `transformers` instalados esa importación tarda segundos. Las
//! pruebas de presupuesto de `cli_golden.rs` —`perf_local_fast_commands_under_budget` y
//! `perf_invalid_input_rejection_fail_fast`, con techos de 1500 ms— corren **en paralelo**
//! dentro del mismo binario, así que una prueba que lance el stack de ML a la vez mide el
//! disco y la CPU ajenos y falla por el motivo equivocado. Medido: con estas dos pruebas
//! dentro de `cli_golden`, `speech synthesize --help` tardó 2435 ms contra un techo de 1500.
//!
//! En su propio binario de prueba no hay problema: `cargo test` ejecuta los targets de
//! prueba **uno a uno**, así que nada de esto se solapa con los presupuestos de `cli_golden`.
//! El nombre del archivo lo dice para que nadie las meta allí dentro "para agruparlas".
//!
//! ### Qué se afirma
//!
//! - Sin `--no-setup` y con la provisión fallida: **un solo objeto** en stdout, `status`
//!   `installed`, `reason` `setup_failed`, `models` `failed`, el `reason` del fallo de
//!   provisión **anidado** en `models_cause`, y **código de salida 11**.
//! - Con `--no-setup`: código 0, `reason` `null` y **sin** `models_cause`. El camino de
//!   éxito no cambia.
//!
//! ### Cómo se evita la red y el estado de la máquina
//!
//! - **Sin red**: se plantan en la raíz de modelos del sandbox los repos de la selección con
//!   los archivos que `is_provisioned` exige, de modo que `setup::pending` no devuelve nada
//!   que descargar. Lo único que queda es la conversión de CT2, que es local y falla porque
//!   el snapshot de pruebas no es un modelo. Que falle es independiente de la máquina: sin
//!   `python` falla por no encontrarlo, y con `python` falla porque el conversor no
//!   encuentra un modelo.
//! - **Sin `PATH`**: `--no-modify-path` en las dos, porque en Windows la integración escribe
//!   en `HKCU\Environment` y una puerta no puede tocar el entorno de quien la ejecuta.
//! - **Sin el temporal de la máquina**: la tabla de rutas no declara variable de
//!   reubicación para el directorio de temporales, así que el barrido del hijo alcanzaría
//!   el `%TEMP%`
//!   real. Se le pasan `TEMP`, `TMP` y `TMPDIR` apuntando al del sandbox, y así el barrido
//!   solo ve lo suyo.
//! - **Sin el árbol de compilación**: `self install` en modo instalación **mueve** el bundle
//!   desde el directorio del ejecutable, y ese directorio es `target/debug`. Por eso el
//!   bundle se monta alrededor de una **copia** del binario en el sandbox: instalar desde
//!   `target/debug` vaciaría el árbol de build de los archivos del manifiesto.

#![allow(clippy::disallowed_methods)]

use serde_json::Value;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Prefijo de los directorios que crea este archivo.
///
/// **Por qué no empieza por `avi`.** El producto reserva para la aplicación los prefijos
/// de temporales `avi-` y `avi_`, y el barrido decide por **`starts_with`**: compara
/// `name.starts_with(prefix)` contra `TEMP_PREFIXES` en
/// `crates/avi-lifecycle/src/recovery.rs:248`, y `TEMP_PREFIXES` es `&["avi-", "avi_"]` en
/// `crates/avi-store/src/lib.rs:660`. Un directorio con el prefijo del producto es un
/// temporal propio a todos los efectos, así que el barrido lo borra —`owner_pid` no
/// encuentra una racha de tres dígitos en el nombre y lo declara huérfano—.
///
/// Y las dos correcciones "obvias" siguen colisionando: `avi_test_*` empieza por `avi_`, y
/// `avi-test-*` empieza por `avi-`. El criterio es exactamente **no empezar por `avi-` ni por
/// `avi_`**, y da igual qué más lleve el nombre.
///
/// Aquí era doble: este archivo **ejecuta `self install`**, que es una de las operaciones
/// que barren, de modo que un prefijo colisionante exponía el sandbox de este archivo al
/// barrido de sus propios hijos y al de cualquier otro comando de ciclo de vida en paralelo.
/// La segunda mitad del invariante —que el temporal del hijo esté dentro del sandbox— está
/// en `Sandbox::nuevo`, y `sweep_never_touches_a_foreign_test_sandbox`, en
/// `crates/avi-lifecycle/tests/recovery.rs`, la comprueba contra el barrido real.
const PREFIX_SANDBOX: &str = "contract-sandbox_";
/// Prefijo del tempfile donde se captura la salida del hijo. Mismo criterio, y el motivo es
/// que un `.tmp` con prefijo del producto sería un temporal propio de pleno derecho.
const PREFIX_TMP: &str = "contract-stdout_";

/// Crea un tempfile único con semántica atómica `O_CREAT|O_EXCL`.
///
/// Es el mismo primitivo que usa `cli_golden.rs`, y está aquí duplicado a propósito: el
/// archivo no comparte utils con aquel porque aquel es un binario de prueba y este otro, y
/// un `mod` común entre `tests/*.rs` de paquetes distintos no existe sin tocar la
/// estructura de la suite.
fn open_atomic_tmp() -> (PathBuf, std::fs::File) {
    let dir = std::env::temp_dir();
    for attempt in 0..64u32 {
        let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
        let path = dir.join(format!(
            "{PREFIX_TMP}{}_{}_{}_{}",
            std::process::id(),
            attempt,
            n,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos())
                .unwrap_or_default()
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => return (path, file),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => panic!("no se pudo crear el tempfile {}: {}", path.display(), e),
        }
    }
    panic!("no se pudo crear un tempfile único tras 64 intentos");
}

/// Sandbox del contrato: un directorio con las raíces del hijo, un `tmp` aislado y un
/// staging con el bundle completo alrededor de una copia del binario.
struct Sandbox {
    root: PathBuf,
    /// Ejecutable a invocar: la **copia** del staging, que es la que tiene el bundle alrededor.
    exe: PathBuf,
    /// Entorno del hijo. `TEMP`/`TMP`/`TMPDIR` reubican su temporal, que la tabla no permite
    /// reubicar por variable de la aplicación.
    envs: Vec<(String, String)>,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let n = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.subsec_nanos())
            .unwrap_or_default();
        let root =
            std::env::temp_dir().join(format!("{PREFIX_SANDBOX}{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let staging = root.join("opt").join(format!(
            "{}contract-{tag}",
            avi_lifecycle::STAGING_DIR_PREFIX
        ));
        let install = root.join("opt").join(avi_lifecycle::APP_NAME);
        let data = root.join("data");
        let models = root.join("models");
        let temp = root.join("tmp");
        let hf = root.join("hf");
        for dir in [&staging, &data, &models, &temp, &hf] {
            std::fs::create_dir_all(dir).expect("crear raíces del sandbox");
        }

        // El bundle: la copia del ejecutable con el nombre del manifiesto y el resto de
        // archivos obligatorios con contenido de marcador. La lista no se escribe a mano.
        let section = avi_lifecycle::manifest::target_section(avi_lifecycle::target::host_triple())
            .expect("el target del host tiene sección en el manifiesto");
        for relative in &section.required {
            let complete = staging.join(relative.replace('/', std::path::MAIN_SEPARATOR_STR));
            if let Some(parent) = complete.parent() {
                std::fs::create_dir_all(parent).expect("crear directorio del archivo del bundle");
            }
            if relative == &section.executable {
                std::fs::copy(BIN, &complete).expect("copiar el binario al staging del sandbox");
            } else {
                std::fs::write(&complete, format!("contenido de {relative}\n"))
                    .expect("escribir el archivo del bundle");
            }
        }

        // La selección de modelos ya provisionada, para que la provisión no toque la red.
        for (name, repo, rev) in avi_store::MODEL_REVISIONS {
            if *name == avi_lifecycle::setup::CLONING_MODEL {
                continue;
            }
            let snapshot = models
                .join(format!("models--{}", repo.replace('/', "--")))
                .join("snapshots")
                .join(rev);
            match avi_store::MODEL_FILE_PATTERNS
                .iter()
                .find(|(n, _)| n == name)
            {
                Some((_, patterns)) => {
                    for pattern in *patterns {
                        std::fs::create_dir_all(&snapshot).expect("crear snapshot del repo");
                        std::fs::write(snapshot.join(pattern), "pesos").expect("escribir el pin");
                    }
                }
                None => {
                    std::fs::create_dir_all(&snapshot).expect("crear snapshot del repo");
                    std::fs::write(snapshot.join("model.safetensors"), "pesos")
                        .expect("escribir los pesos del snapshot");
                }
            }
        }

        let exe = staging.join(section.executable_path());
        let value = |p: &Path| p.display().to_string();
        let mut envs = vec![
            // `AVI_CACHE_DIR` tiene precedencia sobre `HF_HUB_CACHE`, así que la
            // raíz de modelos del sandbox es la **exclusiva** de la aplicación; las
            // variables de HuggingFace se fijan igualmente para que ninguna prueba que las
            // herede del entorno de la máquina escriba fuera del sandbox.
            ("AVI_CACHE_DIR".to_string(), value(&models)),
            ("HF_HUB_CACHE".to_string(), value(&hf)),
            ("HF_HOME".to_string(), value(&hf)),
            ("AVI_INSTALL_DIR".to_string(), value(&install)),
            // En Windows la entrada del `PATH` es el propio directorio de programa.
            ("AVI_BIN_DIR".to_string(), value(&install)),
            ("AVI_DATA_DIR".to_string(), value(&data)),
            // El temporal del hijo, aislado: la tabla no declara variable de reubicación para él.
            ("TEMP".to_string(), value(&temp)),
            ("TMP".to_string(), value(&temp)),
            ("TMPDIR".to_string(), value(&temp)),
            // Puerto efímero: la parada del daemon no tiene que poder tocar nada.
            ("AVI_DAEMON_PORT".to_string(), "0".to_string()),
        ];
        envs.sort();
        Self { exe, envs, root }
    }

    /// Ejecuta el hijo y devuelve su código de salida y su sobre.
    fn run(&self, args: &[&str]) -> (i32, Value) {
        let (tmp, file) = open_atomic_tmp();
        let mut cmd = Command::new(&self.exe);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(file)
            .stderr(std::process::Stdio::null());
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        let status = cmd.spawn().expect("el hijo debe arrancar").wait();
        let status = status.expect("el hijo debe terminar");
        let stdout = std::fs::read_to_string(&tmp)
            .unwrap_or_else(|e| panic!("no se pudo leer {}: {}", tmp.display(), e));
        let _ = std::fs::remove_file(&tmp);
        let json: Value = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout no es un único objeto JSON ({e}): {stdout}"));
        (
            status.code().expect("el hijo debe terminar con un código"),
            json,
        )
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// El invariante del prefijo, desde el lado de este archivo.
///
/// Son dos porque este archivo es el peor de los dos casos: **ejecuta `self install`**, que
/// es una de las operaciones que barren, así que un prefijo colisionante exponía su sandbox
/// al barrido de sus propios hijos. El prefijo protege de eso, y la reubicación de `TEMP`,
/// `TMP` y `TMPDIR` en `Sandbox::nuevo` protege de que el barrido del hijo salga al
/// temporal de la máquina.
///
/// El criterio es `!nombre.starts_with(prefijo_del_producto)`, y no "no lleva `avi`": las
/// dos formas naturales colisionan, como demuestra el propio mensaje.
#[test]
fn contract_prefixes_do_not_collide_with_the_product_temporaries() {
    for (name, prefix) in [
        ("el sandbox", PREFIX_SANDBOX),
        ("el tempfile de salida", PREFIX_TMP),
    ] {
        for product in avi_store::TEMP_PREFIXES {
            assert!(
                !prefix.starts_with(product),
                "el prefijo de {name} (`{prefix}`) empieza por `{product}`, que el producto \
                 reserva a la aplicación: el barrido lo borraría como si fuera temporal nuestro"
            );
        }
    }
    // Las dos trampas, para que el criterio quede escrito donde se mira.
    assert!(
        "avi_test_sandbox_x".starts_with("avi_"),
        "precondición: `avi_test_*` empieza por `avi_` y por eso colisionaba"
    );
    assert!(
        "avi-test-sandbox-x".starts_with("avi-"),
        "precondición: `avi-test-*` empieza por `avi-` y por eso colisionaría"
    );

    // Y la segunda mitad: el temporal del hijo está dentro de su sandbox, así que el
    // barrido tiene el universo acotado aunque el prefijo volviera a colisionar.
    let sandbox = Sandbox::new("invarianteprefijo");
    for variable in ["TEMP", "TMP", "TMPDIR"] {
        let value = PathBuf::from(
            sandbox
                .envs
                .iter()
                .find(|(k, _)| k == variable)
                .map(|(_, v)| v.as_str())
                .unwrap_or_else(|| panic!("el sandbox debe reubicar {variable}")),
        );
        assert!(
            value.starts_with(&sandbox.root),
            "{variable} del hijo es {value:?}, fuera del sandbox {:?}: el barrido \
             alcanzaría el temporal de la máquina",
            sandbox.root
        );
    }
    assert!(
        sandbox
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(PREFIX_SANDBOX),
        "y el directorio del sandbox se llama con el prefijo declarado, que es lo que la \
         aserción de arriba comprueba"
    );
}

/// `self install` sin `--no-setup` y con la provisión fallida: el programa queda
/// instalado, el `reason` del sobre es `setup_failed` y el código de salida es el 11 de la
/// tabla cerrada, que es lo que la tabla de reasons llama éxito parcial.
///
/// Y el `reason` del **fallo de provisión** viaja anidado en `models_cause`, que es donde un
/// consumidor lo encuentra sin perderlo: el de la operación y el de la causa son dos cosas
/// distintas, y confundirlas perdería el `network_error` de un fallo de descarga, que es un
/// `reason` del ciclo 2.
#[test]
fn self_install_setup_failure_exits_11_with_partial_success() {
    let sandbox = Sandbox::new("setupfallido");
    let (code, actual) = sandbox.run(&[
        "--json",
        "self",
        "install",
        "--yes",
        // Sin `PATH`: la integración de Windows escribe en `HKCU\Environment` y una puerta
        // de CI no puede tocar el entorno de quien la ejecuta.
        "--no-modify-path",
    ]);

    assert_eq!(
        code, 11,
        "`setup_failed` es éxito parcial con código propio, `SetupFailed = 11` de la \
         tabla cerrada; el sobre fue {actual:?}"
    );
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    assert_eq!(
        actual["status"],
        Value::String("installed".to_string()),
        "el paso 11: el programa queda instalado, que es la otra mitad del criterio"
    );
    assert_eq!(actual["reason"], Value::String("setup_failed".to_string()));
    assert_eq!(actual["models"], Value::String("failed".to_string()));
    assert_eq!(
        actual["models_cause"]["reason"],
        Value::String("ct2_conversion_failed".to_string()),
        "el `reason` del fallo de provisión viaja anidado, y es el que \
         `docs/CLI/commands/SETUP.md` publica para una conversión fallida"
    );
    assert!(
        actual["models_cause"]["message"]
            .as_str()
            .is_some_and(|m| !m.is_empty()),
        "y con su mensaje, que es lo que el usuario necesita para reintentar: {:?}",
        actual["models_cause"]
    );
    assert!(
        actual.get("error").is_none(),
        "un éxito parcial no lleva `error`: el sobre es el del desenlace, no el de un \
         fallo de operación: {actual:?}"
    );

    // Y el programa está de verdad, con su recibo, que es lo que lo hace instalación
    // registrada y no un copiado.
    let program_dir = PathBuf::from(
        actual["install_dir"]
            .as_str()
            .expect("`install_dir` es una cadena"),
    );
    assert!(
        program_dir.is_dir(),
        "y el directorio de programa existe en {program_dir:?}"
    );
    assert!(
        program_dir
            .join(avi_lifecycle::receipt::RECEIPT_NAME)
            .is_file(),
        "con su recibo: `self install` lo escribió antes de provisionar, que es el orden de \
         los pasos 10 y 11"
    );
    assert!(
        actual["version"].as_str().is_some_and(|v| !v.is_empty()),
        "y el sobre dice qué versión quedó instalada: {actual:?}"
    );
}

/// El camino de éxito no cambia: con `--no-setup` se sale con 0, sin `reason` y sin
/// `models_cause`. Es la mitad del criterio 6 que el paso 11 no toca, y la que demuestra
/// que el `reason` nuevo no se ha colado en las operaciones limpias.
#[test]
fn self_install_no_setup_exits_0_without_reason() {
    let sandbox = Sandbox::new("nosetup");
    let (code, actual) = sandbox.run(&[
        "--json",
        "self",
        "install",
        "--yes",
        "--no-modify-path",
        "--no-setup",
    ]);

    assert_eq!(code, 0, "`--no-setup` sale con éxito: {actual:?}");
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    assert_eq!(actual["status"], Value::String("installed".to_string()));
    assert_eq!(actual["reason"], Value::Null, "sin `reason` en el éxito");
    assert_eq!(actual["models"], Value::String("skipped".to_string()));
    assert!(
        actual.get("models_cause").is_none(),
        "y sin `models_cause`, que solo existe si hubo fallo: {actual:?}"
    );
    assert!(actual.get("error").is_none(), "ni `error`: {actual:?}");
}
