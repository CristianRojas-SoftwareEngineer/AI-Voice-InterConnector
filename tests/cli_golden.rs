//! Harness de tests dorados del CLI (Tarea 8 del desbloqueo de Fase 0).
//!
//! Invoca el binario compilado con argumentos fijos y compara `stdout` (JSON) y el
//! código de salida contra fixtures en `tests/golden/`, replicando el contrato que
//! cubrían los scripts Python eliminados: `schema_version == "3"` (vía
//! `avi_core::json_emitter`) y los códigos de salida de `avi_core::exit_codes`.
//!
//! Se ubica como test de integración del paquete raíz (y no dentro de `src/main.rs`)
//! porque capturar `stdout` + exit code con fidelidad exige ejecutar el binario real,
//! y `CARGO_BIN_EXE_*` solo está disponible para tests de integración.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::Value;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

/// Serializa los tests que mutan estado compartido del almacén (cleanup borra
/// snapshots HF + data_dir; los tests TTS dependen de esa provisión). Sin este
/// lock, `cargo test` los corre en paralelo dentro del mismo binario y cleanup
/// puede borrar el estado que un test TTS está verificando (carrera intra-binario).
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Carga una fixture dorada desde `tests/golden/`.
fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("fixture {} no es JSON válido: {}", name, e))
}

/// Ejecuta el binario con `args` y envs extra, devolviendo (código de salida, stdout
/// parseado a JSON). Las envs se inyectan vía `.env()` en el `Command` hijo: los tests
/// que necesitan aislar el estado del sandbox (p. ej. `LOCALAPPDATA`/`HF_*` en
/// `uninstall_force_no_se_auto_mata`) las pasan por aquí; `run_json` delega sin envs.
///
/// Usa un *tempfile* (`Stdio::File`) en vez de `Command::output()` (que captura `stdout`
/// vía un **pipe** con `bInheritHandle=TRUE`). El comando `daemon start` lanza el daemon
/// hijo (y este, a su vez, `qwen_tts.exe` vendido/precompilado) que heredan el pipe del
/// test: `output()` no retorna hasta que **todos** los holders del write-end lo cierran —
/// es decir, hasta el graceful shutdown del daemon (~10 s) — colgando el E2E en timeout
/// (exit 124). El fix de `spawn_background` (`CREATE_NO_HANDLE_INHERIT` + `Stdio::null`)
/// es necesario pero insuficiente: el binario vendido no respeta `creation_flags` y Rust
/// std decide `bInheritHandles` de forma independiente al flag. Al redirigir `stdout` a
/// un tempfile **no hay pipe** para heredar: `spawn()`+`wait()` retorna en cuanto el CLI
/// termina (~1.3 s tras `daemon start` con el bind-first).
///
/// Patrón equivalente al del legacy Python: el daemon no comparte I/O (pipe) con el
/// proceso que lo lanza.
fn run_json_env(args: &[&str], envs: &[(&str, &str)]) -> (i32, Value) {
    let (tmp, file) = open_atomic_tmp();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(file)
        .stderr(std::process::Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("el binario debe ejecutarse");
    let status = child.wait().expect("el proceso debe terminar");
    let stdout = std::fs::read_to_string(&tmp)
        .unwrap_or_else(|e| panic!("no se pudo leer tempfile {}: {}", tmp.display(), e));
    let _ = std::fs::remove_file(&tmp);
    let code = status
        .code()
        .expect("el proceso debe terminar con un código");
    let json: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout no es JSON válido ({}): {:?}", e, stdout));
    (code, json)
}

/// Ejecuta el binario con `args` (sin envs extra). Delega en [`run_json_env`].
fn run_json(args: &[&str]) -> (i32, Value) {
    run_json_env(args, &[])
}

/// Crea un tempfile único con semántica atómica `O_CREAT|O_EXCL` (`create_new`).
///
/// Corrección estructural sin sobreingeniería: la causa no es el contenido del
/// daemon sino el primitivo de FS. `File::create` trunca y no es atómico: con
/// `SystemTime` de resolución gruesa en Windows + `cargo test` paralelo, dos
/// hilos generan el mismo `tmp` y el `remove_file` de uno borra el de otro
/// -> `read_to_string` falla con `NotFound (os 2)` solo en `win/server-2022`.
/// Se reemplaza por creación atómica `O_CREAT|O_EXCL` (`create_new(true)`):
/// el SO garantiza exclusión y el bucle reintenta solo en colisión, sin
/// depender de `ThreadId`/`sleep`/`retry` sintomático. Es el mismo coste que
/// `tempfile::NamedTempFile` pero sin añadir dependencia.
fn open_atomic_tmp() -> (PathBuf, std::fs::File) {
    let mut attempts = 0;
    loop {
        let candidate = std::env::temp_dir().join(format!(
            "cli_golden_{}_{}_{}.out",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 10 {
                    panic!("no se pudo crear tempfile único tras 10 intentos: {}", e);
                }
                continue;
            }
            Err(e) => panic!("no se pudo crear tempfile {}: {}", candidate.display(), e),
        }
    }
}

/// Modelo Parakeet TDT v3 presente. Los binarios bajo `models/` están
/// gitignoreados: en un checkout limpio (CI) los E2E que los requieren se
/// saltan con aviso; en desarrollo corren completos. Solo se compila con
/// `native-stt`: sin el feature el binario no transcribe, así que los E2E que
/// dependen de él se gatean por feature (no solo por presencia de modelo).
#[cfg(feature = "native-stt")]
fn parakeet_model_disponible() -> bool {
    std::path::Path::new("models/parakeet-tdt-v3").exists()
}

/// Modelo CT2 es→en presente (mismo criterio de skip que el Parakeet). Solo se
/// compila con `native-translation`: sin el feature el binario no traduce, así
/// que el E2E que lo usa se gatea por feature (no solo por presencia de modelo).
#[cfg(feature = "native-translation")]
fn ct2_model_disponible() -> bool {
    std::path::Path::new("models/ct2/opus-mt-es-en/model.bin").exists()
}

#[test]
fn version_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "version"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_version.json"));
}

// Requiere `native-stt`: sin el motor Parakeet el binario responde
// `stt_unsupported`, por lo que el contrato de transcripción solo aplica con el
// feature activo (en CI featureless no se compila).
#[cfg(feature = "native-stt")]
#[test]
fn speech_transcribe_con_audio_cumple_contrato() {
    if !parakeet_model_disponible() {
        eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (models/ gitignoreado)");
        return;
    }
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--audio",
        "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(actual["source"], Value::String("es-latam".to_string()));
    let text = actual["text"].as_str().expect("`text` debe ser un string");
    assert!(!text.is_empty(), "`text` no debe estar vacío");
}

#[test]
fn speech_transcribe_sin_audio_ni_mic_sale_con_codigo_2() {
    let output = Command::new(BIN)
        .args([
            "--json",
            "speech",
            "transcribe",
            "--source-language",
            "es-latam",
        ])
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    assert_eq!(
        code, 2,
        "omitir --audio y --mic debe mapear a ExitCode::InvalidInput"
    );
}

#[test]
fn daemon_status_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "daemon", "status"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_daemon_status.json"));
}

#[test]
fn cleanup_coincide_con_fixture() {
    let _guard = STATE_LOCK.lock().unwrap();
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_cleanup.json"));
}

/// Regresión del self-kill de `uninstall --force` en Windows (v0.18.10–v0.18.25):
/// el fallback `taskkill /F /IM ai-voice-interconnector.exe` mataba al propio CLI
/// (daemon y CLI comparten la imagen del binario) antes de borrar PATH/install_dir,
/// retornando `exit 1` sin tocar nada. La corrección sustituyó ese fallback por un
/// kill por **PID** leído de `daemon.pid` (con guarda `pid != process::id()`). Si
/// reapareciera el kill por imagen, el binario bajo test moriría por `taskkill` y
/// `status.code()` devolvería `None`, provocando el fallo del test (regresión en CI
/// `test-windows`).
///
/// Sandbox aislado: `LOCALAPPDATA` (honrada en `handle_uninstall` para el
/// `install_dir`) y `HF_HUB_CACHE`/`HF_HOME` (honradas por `hf_cache_dir` en
/// `avi-store`) apuntan a un directorio temporal propio, con un `install_dir` falso
/// como marcador. El nombre del sandbox no empieza por `avi_` ni
/// `ai-voice-interconnector-install-`, ajeno al barrido de temp huérfano de
/// `uninstall`. Si hubiera un daemon real activo en 127.0.0.1:8765, el test se salta
/// con aviso: `cargo test` no debe detener el daemon del usuario (esa ruta la cubre
/// el E2E manual).
#[cfg(windows)]
#[test]
fn uninstall_force_no_se_auto_mata() {
    let _guard = STATE_LOCK.lock().unwrap();

    // (a) Skip si hay daemon real activo (no detenerlo desde `cargo test`).
    let (_, status) = run_json(&["--json", "daemon", "status"]);
    if status["daemon"] == Value::String("running".to_string()) {
        eprintln!("[uninstall] skip: daemon activo en 127.0.0.1:8765");
        return;
    }

    // (b) Sandbox con install_dir falso (marcador) y caches HF aisladas.
    let sandbox = std::env::temp_dir().join(format!(
        "uninstall_sandbox_{}_{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::SeqCst),
    ));
    let local = sandbox.join("LocalAppData");
    let programs = local.join("Programs/ai-voice-interconnector");
    std::fs::create_dir_all(&programs)
        .unwrap_or_else(|e| panic!("no se pudo crear install_dir falso: {}", e));
    std::fs::write(programs.join("ai-voice-interconnector.exe"), b"marker")
        .unwrap_or_else(|e| panic!("no se pudo escribir el marcador: {}", e));
    let hf = sandbox.join("hf");
    std::fs::create_dir_all(&hf).unwrap();

    // (c) uninstall --force contra el sandbox aislado.
    let (code, actual) = run_json_env(
        &["--json", "uninstall", "--force"],
        &[
            ("LOCALAPPDATA", local.to_str().unwrap()),
            ("HF_HUB_CACHE", hf.to_str().unwrap()),
            ("HF_HOME", hf.to_str().unwrap()),
        ],
    );

    // (d) Invariante crítico: no auto-muerte, contrato JSON intacto, install_dir borrado.
    assert_eq!(
        code, 0,
        "uninstall --force no debe auto-matarse: {}",
        actual
    );
    assert_eq!(actual["status"], Value::String("uninstalled".to_string()));
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert!(
        !programs.exists(),
        "el install_dir del sandbox debe borrarse tras uninstall"
    );

    // (e) Limpieza del sandbox.
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn voice_list_respeta_el_contrato_de_envelope() {
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato (envelope + presencia de `default`).
    let (code, actual) = run_json(&["--json", "voice", "list"]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    let voices = actual["voices"]
        .as_array()
        .expect("`voices` debe ser un array");
    assert!(
        voices
            .iter()
            .any(|v| v == &Value::String("default".to_string())),
        "debe listarse la voz de fábrica `default`"
    );
}

#[test]
fn translate_texto_vacio_sale_con_codigo_2() {
    // Entrada inválida → ExitCode::InvalidInput (2), con el envelope de error del CLI.
    let (code, actual) = run_json(&["--json", "translate", "--text", ""]);
    assert_eq!(code, 2, "texto vacío debe mapear a ExitCode::InvalidInput");
    assert_eq!(actual, fixture("cli_translate_empty.json"));
}

// Requiere `native-translation`: sin el motor CT2 el binario responde
// `translation_unsupported`, por lo que este contrato solo aplica con el feature
// activo (en CI featureless no se compila).
#[cfg(feature = "native-translation")]
#[test]
fn translate_es_a_en_produce_traduccion() {
    if !ct2_model_disponible() {
        eprintln!("[translate] skip: sin modelo CT2 es→en (models/ gitignoreado)");
        return;
    }
    // El texto traducido depende del motor real; se verifican invariantes de
    // contrato (mismo patrón que `speech_transcribe_con_audio_cumple_contrato`).
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Hola, ¿cómo estás?",
        "--from",
        "es",
        "--to",
        "en",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(actual["source"], Value::String("es".to_string()));
    assert_eq!(actual["target"], Value::String("en".to_string()));
    let translated = actual["translated"]
        .as_str()
        .expect("`translated` debe ser un string");
    assert!(!translated.is_empty(), "`translated` no debe estar vacío");
}

#[test]
fn translate_passthrough_mismo_idioma_devuelve_texto_intacto() {
    // Passthrough: origen == destino tras normalizar → texto intacto.
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Hola",
        "--from",
        "es",
        "--to",
        "es",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["translated"], Value::String("Hola".to_string()));
}

#[test]
fn translate_par_no_soportado_sale_con_codigo_2() {
    // Par no soportado → ExitCode::InvalidInput (2), ruta de validación pura
    // sin depender de ningún modelo.
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Bonjour",
        "--from",
        "fr",
        "--to",
        "de",
    ]);
    assert_eq!(
        code, 2,
        "par no soportado debe mapear a ExitCode::InvalidInput"
    );
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(
        actual["reason"],
        Value::String("unsupported_language_pair".to_string())
    );
}

// ─── Golden TTS (Fase 5, Tarea 11) ───────────────────────────────────

mod tts {
    use super::*;
    // El trait STT solo se necesita para el cálculo de WER real (native-stt).
    #[cfg(feature = "native-stt")]
    use avi_core::engine::SttEngine;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Ruta del binario del motor Qwen3-TTS (override o vendored).
    fn tts_binario() -> Option<PathBuf> {
        if let Ok(b) = std::env::var("QWEN3_TTS_BIN") {
            let p = PathBuf::from(b);
            if !p.as_os_str().is_empty() {
                return Some(p);
            }
        }
        let vendored = PathBuf::from("vendor/qwen3-tts/qwen_tts.exe");
        if vendored.is_file() {
            return Some(vendored);
        }
        None
    }

    /// Pesos del modelo Qwen3-TTS 0.6B presentes.
    fn tts_pesos() -> bool {
        Path::new("vendor/qwen3-tts/qwen3-tts-0.6b").is_dir()
    }

    /// Estado de provisión VERIFICADO AHORA (no cacheado): `doctor` consulta los
    /// snapshots HF vigentes. Si falta, corre `setup` una sola vez bajo lock
    /// (evita descargas paralelas) y re-verifica. No se cachea el resultado
    /// porque `cleanup_coincide_con_fixture` puede borrar la provisión en otro
    /// hilo entre tests: un caché obsoleto hacía que tests TTS posteriores a
    /// cleanup confiaran en estado ya eliminado (`model_missing`).
    fn tts_modelo_registrado() -> bool {
        static SETUP_LOCK: Mutex<()> = Mutex::new(());
        let doctor_ok = || {
            Command::new(BIN)
                .args(["doctor"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if doctor_ok() {
            return true;
        }
        let _guard = SETUP_LOCK.lock().unwrap();
        if doctor_ok() {
            return true;
        }
        matches!(
            Command::new(BIN).args(["setup"]).output(),
            Ok(o) if o.status.success()
        )
    }

    /// Provisto = modelo registrado + binario + pesos.
    fn tts_provisioned() -> bool {
        tts_modelo_registrado() && tts_binario().is_some() && tts_pesos()
    }

    /// El clonado de voz exige el modelo Base del motor (graft ICL); el modelo
    /// CustomVoice vendorizado (`qwen3-tts-0.6b/`) no sirve para clonado. El
    /// modelo Base se provisiona vía `ModelStore` snapshot HF o directorio vendored
    /// (`qwen3-tts-0.6b-base/`, `config.json: "tts_model_type": "base"`).
    fn tts_clone_provisioned() -> bool {
        if !tts_provisioned() {
            return false;
        }
        if avi_store::ModelStore::new().is_provisioned("qwen3-tts-0.6b-base") {
            return true;
        }
        let config = Path::new("vendor/qwen3-tts/qwen3-tts-0.6b-base/config.json");
        match std::fs::read_to_string(config) {
            Ok(c) => c.contains("\"tts_model_type\": \"base\""),
            Err(_) => false,
        }
    }

    /// Mutex global para serializar los tests TTS pesados (el motor residente
    /// ocupa el puerto 8766 y cada corrida consume ~2.7 GB de RAM). Un fallo de
    /// un test no debe envenenar el lock de los demás.
    static TTS_LOCK: Mutex<()> = Mutex::new(());

    fn lock_tts() -> std::sync::MutexGuard<'static, ()> {
        TTS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Etiqueta/voz única por corrida (el oráculo normaliza a minúsculas).
    fn etiqueta_unica(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("reloj del sistema")
            .as_nanos();
        format!("{}{}_{}", prefix, nanos, std::process::id())
    }

    /// El WAV producido debe ser PCM s16le mono 24 kHz con muestras (spec del motor).
    /// Solo lo usan los E2E de síntesis que verifican WER real (native-stt).
    #[cfg(feature = "native-stt")]
    fn wav_valido_24k(path: &Path) {
        let reader = hound::WavReader::open(path)
            .unwrap_or_else(|e| panic!("WAV ilegible en {}: {}", path.display(), e));
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 24_000, "muestreo del motor: 24 kHz");
        assert_eq!(spec.channels, 1, "mono");
        assert_eq!(spec.bits_per_sample, 16, "16-bit PCM");
        assert!(reader.duration() > 0, "no puede estar vacío");
    }

    /// WER (por palabras normalizadas, Levenshtein) del WAV frente al texto
    /// fuente, vía Parakeet TDT v3 (ort/ONNX Runtime).
    #[cfg(feature = "native-stt")]
    fn wer_vs_texto(path: &Path, texto: &str) -> f64 {
        let pcm = avi_audio::load_wav_16k_mono_pcm(path.to_string_lossy().as_ref())
            .unwrap_or_else(|e| panic!("no se pudo cargar {} a 16k: {}", path.display(), e));
        let engine = avi_stt::ParakeetEngine::new("models/parakeet-tdt-v3")
            .expect("el modelo Parakeet TDT v3 debe existir");
        let transcrito = engine
            .transcribe(&pcm, Some("es"))
            .expect("la transcripción no debe fallar");
        let a = normalizar(&transcrito);
        let b = normalizar(texto);
        if b.is_empty() {
            return 1.0;
        }
        let d = levenshtein(&a, &b);
        d as f64 / b.len() as f64
    }

    /// Palabras minúsculas sin diacríticos ni puntuación (señal de habla
    /// limpia). El plegado de diacríticos es manual para no depender de
    /// `unicode-normalization`.
    #[cfg(feature = "native-stt")]
    fn normalizar(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| match c {
                'á' | 'ä' => 'a',
                'é' | 'ë' => 'e',
                'í' | 'ï' => 'i',
                'ó' | 'ö' => 'o',
                'ú' | 'ü' => 'u',
                'ñ' => 'n',
                c if c.is_ascii_alphanumeric() => c,
                _ => ' ',
            })
            .collect::<String>()
            .split_whitespace()
            .map(|w| w.to_string())
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Distancia de Levenshtein entre secuencias de palabras.
    #[cfg(feature = "native-stt")]
    fn levenshtein(a: &[String], b: &[String]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut cur = vec![i + 1; b.len() + 1];
            for (j, y) in b.iter().enumerate() {
                cur[j + 1] = if x == y {
                    prev[j]
                } else {
                    1 + prev[j].min(cur[j]).min(prev[j + 1])
                };
            }
            prev = cur;
        }
        prev[b.len()]
    }

    /// Solo lo usan los E2E de reproducción/dub que dependen de STT (native-stt).
    #[cfg(feature = "native-stt")]
    fn hay_dispositivo_audio() -> bool {
        match avi_audio::get_devices_json() {
            Ok(devs) => !devs.is_empty(),
            Err(_) => false,
        }
    }

    // ─── synthesize ─────────────────────────────────────────────────────

    /// Éxito con `--label`: exit 0, WAV persistido en `speech/`, envelope y
    /// WER ≤ 0.25 frente al texto fuente. La verificación de WER exige el motor
    /// Parakeet (native-stt); sin el feature no se compila (en CI featureless los
    /// modelos tampoco están, así que no se pierde cobertura).
    #[cfg(feature = "native-stt")]
    #[test]
    fn synthesize_exito_con_label() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (models/ gitignoreado)");
            return;
        }
        let _guard = lock_tts();
        let label = etiqueta_unica("golden");
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "Hola, este es un mensaje de prueba para la verificación.",
            "--voice",
            "default",
            "--label",
            &label,
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(
            audio_path,
            "Hola, este es un mensaje de prueba para la verificación.",
        );
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
        let _ = avi_store::SpeechStore::new().remove("default", &label);
    }

    /// Gate WER texto corto — disparador exacto de H1 (Tarea 6).
    ///
    /// Cubre el caso que la E2E `test-windows-e2e` sintetizaba sin veredicto:
    /// texto de 2-4 palabras (`"Hola mundo"`) con voz `default` (preset ryan).
    /// Verifica `WAV 24kHz mono 16-bit` y `WER ≤ 0.25` vía Parakeet (`native-stt`),
    /// mismo patrón que `synthesize_exito_con_label` (11 palabras): requiere
    /// `tts_provisioned()` + `parakeet_model_disponible()`, usa `wav_valido_24k`
    /// y `wer_vs_texto`, falla la E2E/gate si `WER > 0.25`.
    #[cfg(feature = "native-stt")]
    #[test]
    fn synthesize_exito_texto_corto_wer_gate() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (models/ gitignoreado)");
            return;
        }
        let _guard = lock_tts();
        let texto_corto = "Hola mundo";
        let label = etiqueta_unica("golden_corto");
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            texto_corto,
            "--voice",
            "default",
            "--label",
            &label,
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(audio_path, texto_corto);
        assert!(
            wer <= 0.25,
            "WER texto corto '{}' = {} debe ser ≤ 0.25 (disparador H1)",
            texto_corto, wer
        );
        let _ = avi_store::SpeechStore::new().remove("default", &label);
    }

    #[test]
    fn synthesize_texto_vacio_sale_con_2() {
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "",
            "--label",
            "x",
        ]);
        assert_eq!(code, 2, "texto vacío → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("empty_text".to_string()));
    }

    #[test]
    fn synthesize_voz_inexistente_sale_con_3() {
        let _guard = STATE_LOCK.lock().unwrap();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--voice",
            "voz_inexistente_xyz",
            "--label",
            "x",
        ]);
        assert_eq!(
            code, 3,
            "voz inexistente → ExitCode::NotFound (reason={:?})",
            actual["reason"]
        );
        assert_eq!(
            actual["reason"],
            Value::String("voice_not_found".to_string())
        );
    }

    /// Colisión de `--label` sin `--force` → 6. El almacén se fabrica con un
    /// sidecar + WAV mínimo (sin síntesis real).
    #[test]
    fn synthesize_colision_label_sale_con_6() {
        let _guard = STATE_LOCK.lock().unwrap();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let label = etiqueta_unica("colision");
        let wav_min = {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 24_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
                w.write_sample(0i16).unwrap();
                w.finalize().unwrap();
            }
            cursor.into_inner()
        };
        let src = std::env::temp_dir().join(format!("{}_min.wav", label));
        std::fs::write(&src, &wav_min).unwrap();
        let store = avi_store::SpeechStore::new();
        store
            .save("default", &label, "fabricado", &src)
            .expect("el sidecar fabricado debe guardarse");
        let _ = std::fs::remove_file(&src);
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--label",
            &label,
        ]);
        assert_eq!(
            code, 6,
            "colisión de etiqueta → ExitCode::StateConflict (reason={:?})",
            actual["reason"]
        );
        assert_eq!(actual["reason"], Value::String("label_exists".to_string()));
        let _ = store.remove("default", &label);
    }

    // ─── say ───────────────────────────────────────────────────────────

    // Verifica WER real vía Parakeet (native-stt); sin el feature no se compila.
    #[cfg(feature = "native-stt")]
    #[test]
    fn say_exito_reproduce() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !hay_dispositivo_audio() {
            eprintln!("[tts] skip: sin dispositivo de salida de audio");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (models/ gitignoreado)");
            return;
        }
        let _guard = lock_tts();
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "say",
            "--text",
            "Hola, esto es una prueba de reproduccion.",
            "--voice",
            "default",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("reproduced".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(audio_path, "Hola, esto es una prueba de reproduccion.");
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn say_texto_vacio_sale_con_2() {
        let (code, actual) = run_json(&["--json", "speech", "say", "--text", ""]);
        assert_eq!(code, 2, "texto vacío → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("empty_text".to_string()));
    }

    // ─── dub ───────────────────────────────────────────────────────────

    /// Passthrough es→es con `--audio`: exit 0, WAV válido y WER ≤ 0.25 frente
    /// al texto transcrito (el pipeline devuelve `text`). El dub arranca por STT,
    /// así que exige `native-stt`; sin el feature no se compila.
    #[cfg(feature = "native-stt")]
    #[test]
    fn dub_audio_passthrough_es_es() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !Path::new("models/parakeet-tdt-v3").exists() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3");
            return;
        }
        if !hay_dispositivo_audio() {
            eprintln!("[tts] skip: sin dispositivo de salida de audio");
            return;
        }
        let _guard = lock_tts();
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "dub",
            "--audio",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
            "--from",
            "es",
            "--to",
            "es",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        wav_valido_24k(audio_path);
        let texto = actual["text"].as_str().expect("text debe existir");
        let wer = wer_vs_texto(audio_path, texto);
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn dub_archivo_inexistente_sale_con_3() {
        let (code, actual) = run_json(&["--json", "speech", "dub", "--audio", "no-existe.wav"]);
        assert_eq!(code, 3, "archivo inexistente → ExitCode::NotFound");
        assert_eq!(
            actual["reason"],
            Value::String("audio_not_found".to_string())
        );
    }

    // ─── voice clone ───────────────────────────────────────────────────

    #[test]
    fn voice_clone_exito() {
        let _state = STATE_LOCK.lock().unwrap();
        if !tts_clone_provisioned() {
            eprintln!(
                "[tts] skip: el clonado exige el modelo Base del motor Qwen3-TTS \
                 (usa setup --with-base)"
            );
            return;
        }
        let _guard = lock_tts();
        let name = etiqueta_unica("clon");
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            &name,
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["name"], Value::String(name.clone()));
        assert_eq!(actual["precomputed"], Value::Bool(false));
        let speech = actual["speech"].as_str().expect("speech debe existir");
        let qvoice = Path::new(speech);
        assert!(qvoice.is_file(), "reference.qvoice debe existir");
        let size = std::fs::metadata(qvoice).expect("metadata").len();
        assert!(
            size > 1_000_000,
            "el .qvoice debe pesar > 1 MB (era {})",
            size
        );
        let _ = avi_store::VoiceStore::new().remove(&name);
    }

    /// Clonado repetido → 6. La voz existente se fabrica con un `.qvoice` mínimo.
    #[test]
    fn voice_clone_repetido_sale_con_6() {
        let _guard = STATE_LOCK.lock().unwrap();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let name = etiqueta_unica("clon");
        let voices = avi_store::VoiceStore::new();
        let dir = voices.voice_dir(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reference.qvoice"), b"QVCE").unwrap();
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            &name,
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 6, "voz existente → ExitCode::StateConflict");
        assert_eq!(actual["reason"], Value::String("voice_exists".to_string()));
        let _ = voices.remove(&name);
    }

    #[test]
    fn voice_clone_nombre_invalido_sale_con_2() {
        let _guard = STATE_LOCK.lock().unwrap();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            "voz invalida",
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 2, "nombre inválido → ExitCode::InvalidInput");
        assert_eq!(
            actual["reason"],
            Value::String("invalid_voice_name".to_string())
        );
    }

    #[test]
    fn voice_clone_audio_inexistente_sale_con_3() {
        // Serializa con el resto de la suite (patrón de los demás `voice_clone_*`):
        // los E2E de daemon, al apagarse, matan `qwen_tts.exe` por nombre de imagen
        // (global), y sin este lock la síntesis de este test podría cruzarse con ese
        // kill en paralelo y salir con un código distinto de 3.
        let _guard = STATE_LOCK.lock().unwrap();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            "clon_ok",
            "--speech-reference",
            "no-existe.wav",
        ]);
        assert_eq!(code, 3, "audio inexistente → ExitCode::NotFound");
        assert_eq!(
            actual["reason"],
            Value::String("audio_not_found".to_string())
        );
    }

    // ─── daemon start/status/restart ────────────────────────────────

    #[test]
    fn daemon_start_exito() {
        let _guard = STATE_LOCK.lock().unwrap();
        // Asegurar estado limpio
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para daemon start");
            return;
        }
        let (code, actual) = run_json(&["--json", "daemon", "start"]);
        // Puede ser already_running si otro test dejó daemon; aceptar running
        assert!(
            code == 0,
            "daemon start debe salir 0, fue {} reason {:?}",
            code,
            actual
        );
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        // Verificar status running
        let (code2, actual2) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(code2, 0);
        assert_eq!(actual2["daemon"], Value::String("running".to_string()));
        // Cleanup garantizado: POST /shutdown
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (code3, actual3) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(
            actual3["daemon"],
            Value::String("stopped".to_string()),
            "tras stop debe quedar stopped"
        );
        let _ = code3;
    }

    #[test]
    fn daemon_restart_rearma() {
        let _guard = STATE_LOCK.lock().unwrap();
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para daemon restart");
            return;
        }
        // Asegurar que haya daemon corriendo
        let _ = run_json(&["--json", "daemon", "start"]);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (code, actual) = run_json(&["--json", "daemon", "restart"]);
        assert_eq!(code, 0, "daemon restart debe salir 0");
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert!(actual.get("pid").is_some() || actual.get("status").is_some());
        // Status debe seguir running
        let (code2, actual2) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(actual2["daemon"], Value::String("running".to_string()));
        let _ = code2;
        // Cleanup
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    #[test]
    fn daemon_status_running() {
        let _guard = STATE_LOCK.lock().unwrap();
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(300));
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado");
            return;
        }
        let _ = run_json(&["--json", "daemon", "start"]);
        std::thread::sleep(std::time::Duration::from_millis(500));
        let (code, actual) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(code, 0);
        // Cuando está running, el fixture running debe coincidir (schema_version 3)
        if actual["daemon"] == Value::String("running".to_string()) {
            assert_eq!(actual["schema_version"], Value::String("3".to_string()));
            let expected = fixture("cli_daemon_status_running.json");
            // Comparar daemon y engine
            assert_eq!(actual["daemon"], expected["daemon"]);
        }
        // Cleanup
        let _ = Command::new(BIN).args(["daemon", "stop"]).output();
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}
