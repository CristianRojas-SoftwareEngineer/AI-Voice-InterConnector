// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Contrato de `self install` cuando la provisión de modelos falla: `setup_failed` como
//! **éxito parcial**, que es el paso 11 de la instalación: el programa queda
//! instalado y el envelope informa el fallo de provisión con salida 11.
//!
//! ### Por qué este archivo existe y no está en `cli_golden.rs`
//!
//! La comprobación desde el motor ya la cubre
//! `crates/avi-lifecycle/tests/acceptance.rs::criterion_6_setup_failure_keeps_install`: el
//! programa queda instalado, el estado del desenlace es un fallo de provisión y
//! `Outcome::lifecycle_error()` da `setup_failed` con el 11 de la tabla cerrada. Lo que **no**
//! se puede observar desde el motor es el `reason` del envelope ni el **código de salida del
//! proceso**, y eso solo se ve ejecutando el binario.
//!
//! Y aquí no cabe en `cli_golden.rs`: el fallo de provisión pasa por la descarga de un
//! modelo contra un puerto local cerrado, con los reintentos del cliente de descarga, y esa
//! espera compite con las pruebas de presupuesto de `cli_golden.rs` —
//! `perf_local_fast_commands_under_budget` y `perf_invalid_input_rejection_fail_fast`, con
//! techos de 1500 ms—, que corren **en paralelo** dentro del mismo binario.
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
//! - **Sin red real**: se escriben en la raíz de modelos del sandbox los repos de la selección
//!   con los archivos que `is_provisioned` exige, salvo `opus-mt-es-en`, y `HF_ENDPOINT`
//!   apunta a un puerto local cerrado. Lo único que queda por hacer es descargar ese pin, y
//!   falla con `network_error` de forma independiente de la máquina.
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

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

/// Captura anónima para la salida del hijo: un fichero sin nombre en el directorio, que
/// el sistema retira al soltar el último manejador, así que no queda residuo ni con
/// pánico y no existe nombre que el barrido del producto pudiera alcanzar.
fn anonymous_capture() -> std::fs::File {
    tempfile::tempfile().expect("no se pudo crear la captura anónima")
}

/// Lee la captura desde el principio: el hijo escribió por un manejador duplicado y el
/// desplazamiento compartido está al final.
fn read_capture(file: &mut std::fs::File) -> String {
    use std::io::{Read, Seek, SeekFrom};
    file.seek(SeekFrom::Start(0)).expect("rebobinar la captura");
    let mut text = String::new();
    file.read_to_string(&mut text)
        .unwrap_or_else(|e| panic!("no se pudo leer la captura: {e}"));
    text
}

/// Ruta a la DLL real del runtime de VC++ del sistema para un archivo del
/// bundle, si lo es.
///
/// Un binario enlazado con el CRT dinámico importa `VCRUNTIME140.dll` por
/// nombre, y con `--features full` el binario importa siempre `VCOMP140.dll`
/// (OpenMP de CTranslate2); el cargador las busca primero junto al ejecutable: un marcador de texto en su
/// lugar mata al hijo con imagen inválida antes de que emita nada. El staging
/// lleva las reales y no un marcador.
#[cfg(windows)]
fn system_crt_dll(relative: &str) -> Option<PathBuf> {
    const CRT: &[&str] = &[
        "vcruntime140.dll",
        "vcruntime140_1.dll",
        "msvcp140.dll",
        "msvcp140_1.dll",
        "vcomp140.dll",
    ];
    let name = relative.rsplit(['/', '\\']).next().unwrap_or(relative);
    if !CRT.iter().any(|c| c.eq_ignore_ascii_case(name)) {
        return None;
    }
    let candidate = PathBuf::from(std::env::var_os("SystemRoot")?)
        .join("System32")
        .join(name);
    candidate.is_file().then_some(candidate)
}

/// Fuera de Windows no hay CRT del sistema que copiar.
#[cfg(not(windows))]
fn system_crt_dll(_relative: &str) -> Option<PathBuf> {
    None
}

/// Puerto TCP de `127.0.0.1` reservado sin oyente: un socket enlazado sobre el
/// que nunca se llama a `listen`. Mientras la guarda viva, el SO no puede
/// asignar el puerto a otro proceso ni a otra prueba, y toda conexión a él se
/// rechaza.
struct ReservedPort {
    port: u16,
    _socket: tokio::net::TcpSocket,
}

impl ReservedPort {
    fn reserve() -> Self {
        let socket = tokio::net::TcpSocket::new_v4().expect("crear el socket del puerto reservado");
        socket
            .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("reservar un puerto local");
        let port = socket
            .local_addr()
            .expect("dirección del puerto reservado")
            .port();
        ReservedPort {
            port,
            _socket: socket,
        }
    }
}

/// Siembra en `data_dir` un `daemon.pid` señuelo: su dirección apunta a un
/// puerto reservado sin oyente durante toda la prueba y sus PID están fuera de
/// rango. Así una orden que consulte o detenga el daemon recibe un rechazo de
/// conexión, no cae a 127.0.0.1:8765 (el del usuario) ni barre por imagen los
/// `qwen_tts` de la máquina, porque el `resident_pid` registrado no es 0.
///
/// Las claves `identity`/`resident_identity`/`resident_port` van presentes
/// porque el esquema las exige: sin ellas el fichero es ilegible y se descarta,
/// así que el señuelo no protegería nada. Devuelve la guarda del puerto, que el
/// llamante conserva mientras use el señuelo.
fn seed_decoy_daemon_pidfile(data_dir: &Path) -> ReservedPort {
    let decoy = ReservedPort::reserve();
    let dead_pid = u32::MAX - 1;
    let fake = serde_json::json!({ "start": "0", "image": "fake-identity.exe" });
    let content = serde_json::json!({
        "pid": dead_pid,
        "addr": format!("127.0.0.1:{}", decoy.port),
        "resident_pid": dead_pid,
        "identity": fake,
        "resident_identity": fake,
        "resident_port": null,
    });
    std::fs::create_dir_all(data_dir).expect("crear directorio de datos");
    std::fs::write(data_dir.join("daemon.pid"), content.to_string())
        .expect("sembrar daemon.pid señuelo");
    decoy
}

/// Sandbox del contrato: un directorio con las raíces del hijo, un `tmp` aislado y un
/// staging con el bundle completo alrededor de una copia del binario.
/// Pin de modelo que `Sandbox::new` deja sin sembrar para provocar el fallo de descarga.
const UNSEEDED_PIN: &str = "opus-mt-es-en";

struct Sandbox {
    /// Ejecutable a invocar: la **copia** del staging, que es la que tiene el bundle alrededor.
    exe: PathBuf,
    /// Entorno del hijo. `TEMP`/`TMP`/`TMPDIR` reubican su temporal, que la tabla no permite
    /// reubicar por variable de la aplicación.
    envs: Vec<(String, String)>,
    /// Puerto al que apunta el `daemon.pid` señuelo, reservado mientras viva el sandbox.
    _decoy: ReservedPort,
    /// Dueño del directorio raíz: lo borra al soltarse el sandbox, también con pánico.
    _guard: tempfile::TempDir,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let guard = tempfile::Builder::new()
            .prefix(&format!("contract-sandbox_{tag}_"))
            .tempdir()
            .expect("crear el directorio del sandbox");
        let root = guard.path().to_path_buf();
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
        let decoy = seed_decoy_daemon_pidfile(&data);

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
            } else if let Some(system) = system_crt_dll(relative) {
                std::fs::copy(&system, &complete).expect("copiar la CRT del sistema al staging");
            } else {
                std::fs::write(&complete, format!("contenido de {relative}\n"))
                    .expect("escribir el archivo del bundle");
            }
        }

        // La selección de modelos ya provisionada, para que la provisión no toque la red.
        for pin in avi_shared::paths::MODEL_REVISIONS {
            // El pin sin sembrar es el que dispara el fallo de descarga.
            if pin.name == UNSEEDED_PIN {
                continue;
            }
            let snapshot = models
                .join(format!("models--{}", pin.repo.replace('/', "--")))
                .join("snapshots")
                .join(pin.revision);
            match avi_shared::paths::MODEL_FILE_PATTERNS
                .iter()
                .find(|(n, _)| *n == pin.name)
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

        // Endpoint de HuggingFace en un puerto local que se reserva y se libera
        // de inmediato: la descarga del pin sin sembrar debe fallar de inmediato
        // y sin tocar la red real. El puerto se libera a propósito, no se retiene:
        // en macOS, conectar contra un puerto retenido sin oyente cuelga hasta el
        // timeout TCP (~76 s por intento; con los reintentos de `hf-hub` son ~7,6 min
        // en este test), mientras que contra un puerto libre el sistema rechaza al
        // instante en todas las plataformas (medido en CI: 0,2 ms).
        let hf_port = ReservedPort::reserve().port;

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
            (
                "HF_ENDPOINT".to_string(),
                format!("http://127.0.0.1:{hf_port}"),
            ),
        ];
        envs.sort();
        Self {
            exe,
            envs,
            _decoy: decoy,
            _guard: guard,
        }
    }

    /// Ejecuta el hijo y devuelve su código de salida y su envelope.
    ///
    /// El `stderr` del hijo se captura en un temporal hermano: si la salida no
    /// es JSON, el pánico muestra el código y la cola del error en vez de un
    /// fichero vacío que no dice nada (un fallo así en CI es ciego sin esto).
    fn run(&self, args: &[&str]) -> (i32, Value) {
        let mut out_capture = anonymous_capture();
        let mut err_capture = anonymous_capture();
        let mut cmd = Command::new(&self.exe);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(
                out_capture
                    .try_clone()
                    .expect("duplicar la captura de stdout"),
            )
            .stderr(
                err_capture
                    .try_clone()
                    .expect("duplicar la captura de stderr"),
            );
        // Sin consola propia, un invocador sin consola vería abrirse y cerrarse una
        // ventana por cada ejecución del binario.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(avi_process::CREATE_NO_WINDOW);
        }
        for (k, v) in &self.envs {
            cmd.env(k, v);
        }
        let status = cmd.spawn().expect("el hijo debe arrancar").wait();
        let status = status.expect("el hijo debe terminar");
        let code = status.code().expect("el hijo debe terminar con un código");
        let stdout = read_capture(&mut out_capture);
        let stderr = read_capture(&mut err_capture);
        let tail: String = stderr
            .chars()
            .rev()
            .take(2000)
            .collect::<String>()
            .chars()
            .rev()
            .collect();
        // Integridad del ejecutable del staging: tamaño, cabecera MZ y longitud
        // de la ruta. Un hijo que muere a nivel de cargador con la salida vacía
        // no dice si la copia está trunca o lo mató algo externo.
        let (exe_len, exe_mz, path_len) = match std::fs::metadata(&self.exe) {
            Ok(meta) => {
                let mut head = [0u8; 2];
                let mz = std::fs::File::open(&self.exe)
                    .and_then(|mut f| {
                        <std::fs::File as std::io::Read>::read_exact(&mut f, &mut head)
                            .map(|()| head == *b"MZ")
                    })
                    .unwrap_or(false);
                (
                    meta.len().to_string(),
                    mz.to_string(),
                    self.exe.as_os_str().len().to_string(),
                )
            }
            Err(e) => (
                format!("sin metadatos: {e}"),
                "no".to_string(),
                String::new(),
            ),
        };
        let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!(
                "stdout no es un único objeto JSON ({e}): {stdout}\ncódigo: {code}\nstderr: {tail}\nexe: {} tamaño {exe_len} MZ {exe_mz} ruta {path_len}",
                self.exe.display()
            )
        });
        (code, json)
    }
}

/// `self install` sin `--no-setup` y con la provisión fallida: el programa queda
/// instalado, el `reason` del envelope es `setup_failed` y el código de salida es el 11 de la
/// tabla cerrada, que es lo que la tabla de reasons llama éxito parcial.
///
/// Y el `reason` del **fallo de provisión** viaja anidado en `models_cause`, que es donde un
/// consumidor lo encuentra sin perderlo: el de la operación y el de la causa son dos cosas
/// distintas, y confundirlas perdería el `network_error` de un fallo de descarga, que es el
/// `reason` del fallo de provisión.
#[test]
fn self_install_setup_failure_exits_11_with_partial_success() {
    let sandbox = Sandbox::new("setupfailed");
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
         tabla cerrada; el envelope fue {actual:?}"
    );
    assert_eq!(actual["schema_version"], Value::String("5".to_string()));
    assert_eq!(
        actual["status"],
        Value::String("installed".to_string()),
        "el paso 11: el programa queda instalado, que es la otra mitad del criterio"
    );
    assert_eq!(actual["reason"], Value::String("setup_failed".to_string()));
    assert_eq!(actual["models"], Value::String("failed".to_string()));
    assert_eq!(
        actual["models_cause"]["reason"],
        Value::String("network_error".to_string()),
        "el `reason` del fallo de provisión viaja anidado, y es el que \
         `docs/CLI/commands/SETUP.md` publica para una descarga fallida"
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
        "un éxito parcial no lleva `error`: el envelope es el del desenlace, no el de un \
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
        "y el envelope dice qué versión quedó instalada: {actual:?}"
    );
}

/// El camino de éxito no cambia: con `--no-setup` se sale con 0, sin `reason` y sin
/// `models_cause`. Es el camino que el paso 11 no toca, y el que demuestra
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
    assert_eq!(actual["schema_version"], Value::String("5".to_string()));
    assert_eq!(actual["status"], Value::String("installed".to_string()));
    assert_eq!(actual["reason"], Value::Null, "sin `reason` en el éxito");
    assert_eq!(actual["models"], Value::String("skipped".to_string()));
    assert!(
        actual.get("models_cause").is_none(),
        "y sin `models_cause`, que solo existe si hubo fallo: {actual:?}"
    );
    assert!(actual.get("error").is_none(), "ni `error`: {actual:?}");

    // El bundle estaba en un staging ajeno (sin el token de este proceso), así que
    // `self install` lo copió a un staging propio junto al programa y lo borró al
    // terminar: ningún staging de `self install` queda, ni nada del bootstrap en el
    // temporal.
    let value_of = |key: &str| {
        PathBuf::from(
            sandbox
                .envs
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, v)| v.clone())
                .expect("el sandbox define la variable"),
        )
    };
    let names = |dir: PathBuf| -> Vec<String> {
        std::fs::read_dir(dir)
            .expect("leer el directorio")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect()
    };
    let parent = value_of("AVI_INSTALL_DIR")
        .parent()
        .expect("el programa tiene padre")
        .to_path_buf();
    let own_copies: Vec<String> = names(parent)
        .into_iter()
        .filter(|n| n.ends_with("-self-install"))
        .collect();
    assert!(
        own_copies.is_empty(),
        "el staging propio de `self install` se borra: {own_copies:?}"
    );
    let temp_left: Vec<String> = names(value_of("TEMP"))
        .into_iter()
        .filter(|n| n.starts_with("ai-voice-interconnector-install"))
        .collect();
    assert!(
        temp_left.is_empty(),
        "sin temporal del bootstrap: {temp_left:?}"
    );
}

/// Un recibo ilegible hace fallar cada comando que lo necesita, con el `reason` del propio
/// comando y un mensaje que nombra el fichero. `Ok(None)` del recibo significa solo
/// «ausente»: un fichero corrupto jamás se trata como si no existiera.
#[test]
fn unreadable_receipt_fails_lifecycle_commands_naming_the_file() {
    let sandbox = Sandbox::new("badreceipt");
    let install_dir = PathBuf::from(
        sandbox
            .envs
            .iter()
            .find(|(k, _)| k == "AVI_INSTALL_DIR")
            .map(|(_, v)| v.clone())
            .expect("el sandbox define AVI_INSTALL_DIR"),
    );
    std::fs::create_dir_all(&install_dir).expect("crear el directorio de programa");
    let receipt = install_dir.join(avi_lifecycle::receipt::RECEIPT_NAME);
    std::fs::write(&receipt, "esto no es JSON").expect("escribir el recibo corrupto");

    let cases: [(&[&str], &str); 7] = [
        (
            &[
                "--json",
                "self",
                "install",
                "--yes",
                "--no-modify-path",
                "--no-setup",
            ],
            "self_install_failed",
        ),
        (
            &["--json", "self", "update", "--check"],
            "self_update_failed",
        ),
        (
            &["--json", "self", "uninstall", "--yes"],
            "self_uninstall_failed",
        ),
        (&["--json", "doctor"], "doctor_failed"),
        (
            &["--json", "cleanup", "--all", "--dry-run"],
            "lifecycle_failed",
        ),
        (
            &[
                "--json",
                "translate",
                "--text",
                "hola",
                "--source-language",
                "es",
                "--target-language",
                "en",
            ],
            "lifecycle_failed",
        ),
        (&["--json", "speech", "list"], "lifecycle_failed"),
    ];
    for (args, reason) in cases {
        let (code, envelope) = sandbox.run(args);
        assert_eq!(code, 1, "`{args:?}` sale con 1: {envelope:?}");
        assert_eq!(
            envelope["reason"],
            Value::String(reason.to_string()),
            "`{args:?}` falla con el reason del comando: {envelope:?}"
        );
        let message = envelope["error"].as_str().unwrap_or_default();
        assert!(
            message.contains(avi_lifecycle::receipt::RECEIPT_NAME),
            "`{args:?}`: el mensaje nombra el recibo: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&receipt).expect("el recibo sigue en disco"),
            "esto no es JSON",
            "`{args:?}` no toca un recibo ilegible"
        );
    }
}
