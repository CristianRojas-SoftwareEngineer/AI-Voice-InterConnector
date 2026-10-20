// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Harness de tests dorados del daemon.
//!
//! CLASE DECLARADA: contrato puro paralelizable sin restricción.
//! Levanta el `Router` de Axum vía [`avi_daemon::build_router_with_state`] y lo
//! ejerce con `tower::ServiceExt::oneshot` (sin abrir socket TCP real), comparando
//! cada respuesta contra fixtures fijas en `tests/golden/` de la raíz del repo.
//! Detecta regresiones de contrato (formato JSON, `schema_version`, textos de
//! estado/error) entre el runtime nativo y lo que el resto del sistema espera.
//!
//! REGLA DE CLASES: el contrato corre en toda combinación de features; solo las
//! pruebas que ejercitan el motor llevan `#[ignore = "requiere …"]` y, cuando se
//! ejecutan, fallan con el mensaje de aprovisionamiento si el recurso falta.
//! Ninguna prueba se omite por activar un feature: no existe
//! `#[cfg_attr(feature = …, ignore = …)]` en este fichero, porque activando
//! `full` la clase de contrato no se invierte.
//!
//! El estado lo construye `DaemonState::with_stores`, que con `native-stt`
//! carga el motor STT real (Parakeet TDT v3 int8, export de `istupakov`) y sin
//! ese feature no tiene motor alguno (`oneshot` no ejerce `/transcribe` en las
//! pruebas de contrato, de modo que la suite sin features corre sin ONNX
//! Runtime). Un daemon sin STT no es un estado del producto, así que el estado
//! de prueba no se inventa: con `native-stt` activo,
//! `contract_state` exige Parakeet y la heredan sus derivados; su ausencia hace
//! fallar la prueba de contrato con el mensaje de aprovisionamiento, en vez de
//! ocultarla tras un `ignore`.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::{build_router_with_state, DaemonState, WarmState};
use avi_store::{SpeechStore, VoiceStore};

/// Raíz temporal única de este proceso de pruebas: `<temp>/golden-avi-daemon/<pid>`.
/// Con la configuración de cargo, `<temp>` es el temporal confinado dentro de
/// `target/`, que vacían `cargo clean` y `cargo xtask clean`.
/// Los estados estáticos no ejecutan `Drop`, así que sus carpetas no pueden
/// tener dueño; en su lugar viven bajo esta raíz y la siguiente ejecución
/// barre las raíces de procesos que ya no existen.
fn process_root() -> &'static std::path::Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        // Prueba: raíz propia bajo el temporal del sistema, ajena al scratch del producto.
        #[allow(clippy::disallowed_methods)]
        let parent = std::env::temp_dir().join("golden-avi-daemon");
        sweep_dead_process_dirs(&parent);
        let root = parent.join(std::process::id().to_string());
        std::fs::create_dir_all(&root).expect("crear la raíz temporal del proceso");
        root
    })
}

/// Borra de `parent` solo las carpetas cuyo nombre es un PID sin vida. Un PID
/// reutilizado por un proceso vivo se respeta, y cualquier otro nombre se deja.
fn sweep_dead_process_dirs(parent: &std::path::Path) {
    let Ok(entries) = std::fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.is_empty() || !name.bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        let Ok(pid) = name.parse::<u32>() else {
            continue;
        };
        if !avi_process::pid_alive_exact(pid) {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

/// El barrido borra solo la carpeta de un PID muerto: la de otro formato y la
/// de un PID vivo (el propio) quedan intactas. Opera sobre un padre temporal
/// propio para no tocar la raíz compartida de otras ejecuciones.
#[test]
fn sweep_removes_only_dead_pid_dirs() {
    let parent = tempfile::tempdir().expect("directorio temporal");
    let dead = parent.path().join("2000000000");
    let other = parent.path().join("otro-formato");
    let alive = parent.path().join(std::process::id().to_string());
    for dir in [&dead, &other, &alive] {
        std::fs::create_dir_all(dir.join("sub")).expect("crear carpeta de prueba");
    }
    sweep_dead_process_dirs(parent.path());
    assert!(!dead.exists(), "la carpeta del PID muerto debe borrarse");
    assert!(other.is_dir(), "una carpeta de otro formato no se toca");
    assert!(alive.is_dir(), "la carpeta de un PID vivo no se toca");
}

/// Prepara una sola vez por proceso el entorno que comparten los dos estados:
/// `AVI_DATA_DIR` apunta a la raíz temporal propia del proceso para que los
/// logs del motor residente que lanza el clonado no caigan en el directorio de
/// datos del usuario y el registro del daemon (`daemon.pid`) no escriba en la
/// raíz real. Cada residente que lanza el motor reserva su propio puerto
/// efímero, así que no hay puerto que fijar.
fn prepare_shared_env() {
    static PREPARED: Once = Once::new();
    PREPARED.call_once(|| {
        std::env::set_var("AVI_DATA_DIR", process_root().join("data"));
    });
}

/// Estado de contrato único, reutilizable entre tests. Los almacenes de voces y
/// de habla se anclan en un directorio temporal propio del proceso. Su motor TTS
/// no tiene binario (`binary_path = None`) aunque exista vendor, `qwen_tts` en
/// el `PATH` o `QWEN3_TTS_BIN`, así que ninguna prueba de contrato puede lanzar
/// el motor real. Con `native-stt` el estado carga Parakeet, que se exige
/// antes. `warm` nace en `Warming` porque aquí no corre `run_daemon_server`
/// (sin bind ni warmup real); los tests de contrato leen el estado tal cual.
static TEST_STATE: OnceLock<Arc<DaemonState>> = OnceLock::new();

fn test_state() -> Arc<DaemonState> {
    TEST_STATE
        .get_or_init(|| {
            #[cfg(feature = "native-stt")]
            require_parakeet();
            prepare_shared_env();
            Arc::new(contract_state(&process_root().join("state")))
        })
        .clone()
}

/// Estado de contrato con los almacenes bajo la carpeta `tmp` y el motor TTS
/// sin binario.
fn contract_state(tmp: &std::path::Path) -> DaemonState {
    #[cfg(feature = "native-stt")]
    require_parakeet();
    let mut state = DaemonState::with_stores(
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
    )
    .expect("estado del daemon con almacenes temporales");
    state.tts_engine.binary_path = None;
    state
}

/// Estado de contrato sin modelo Base de clonado, para que una prueba que debe
/// cortar antes de buscarlo no pueda lanzar el clonado real aunque el modelo
/// esté provisionado en el equipo.
fn state_without_base_model() -> Arc<DaemonState> {
    prepare_shared_env();
    let mut state = contract_state(&process_root().join("reflimit"));
    state.tts_engine.base_model_dir = None;
    Arc::new(state)
}

/// Estado de clonado, solo para las pruebas que ejercen el motor real. Usa
/// almacenes en otro directorio temporal propio del proceso y su motor TTS
/// apunta a `QWEN3_TTS_BIN` si está definida o, si no, al binario de
/// `vendor/qwen3-tts`, porque el cwd del crate no ve el vendor del workspace.
/// El clonado y el arranque del residente buscan el binario en `QWEN3_TTS_BIN`
/// y no en el campo, así que se fija también la variable; el estado de contrato
/// no la ve porque su `binary_path = None` corta `/synthesize` antes del motor.
static CLONE_STATE: OnceLock<Arc<DaemonState>> = OnceLock::new();

fn clone_state() -> Arc<DaemonState> {
    CLONE_STATE
        .get_or_init(|| {
            #[cfg(feature = "native-stt")]
            require_parakeet();
            prepare_shared_env();
            let tmp = process_root().join("clone");
            let mut state = DaemonState::with_stores(
                VoiceStore::at(tmp.join("voices")),
                SpeechStore::at(tmp.join("speech")),
            )
            .expect("estado del daemon con almacenes temporales");
            let binary = std::env::var_os("QWEN3_TTS_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(vendor_binary);
            std::env::set_var("QWEN3_TTS_BIN", &binary);
            state.tts_engine.binary_path = Some(binary);
            Arc::new(state)
        })
        .clone()
}

/// Carga una fixture dorada desde `tests/golden/` en la raíz del workspace.
fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden")
        .join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("fixture {} no es JSON válido: {}", name, e))
}

/// Envía una petición al router y devuelve (status, cuerpo crudo en bytes).
async fn send(req: Request<Body>) -> (StatusCode, Vec<u8>) {
    send_to(test_state(), req).await
}

/// Envía una petición al router construido sobre el estado indicado.
async fn send_to(state: Arc<DaemonState>, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = build_router_with_state(state)
        .oneshot(req)
        .await
        .expect("el router debe responder");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("el cuerpo debe poder leerse")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// Petición GET simple.
fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Petición POST con cuerpo JSON.
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

// Límite deliberado del harness: `send`/`get`/`post_json` ejercen el contrato
// JSON vía `oneshot` sin abrir socket, sin lanzar procesos, sin señales, sin
// puertos reales ni pidfile; por tanto no prueban la ausencia de huérfanos
// (árbol de procesos, PID, puertos), ni la ventana entre el lanzamiento y la
// escritura del pidfile, ni el manejo de señales durante el lanzamiento, ni un
// crash con el puerto ocupado y un reclamo activo del árbol previo (este
// último solo aplica en `run_supervised`). No reproducen el ciclo de vida real
// del proceso. La ausencia real de huérfanos a nivel de sistema operativo solo
// la verifica la serie de tests pesada, con la cosecha por identidad de la
// instancia aislada.

/// Exige `nemo128.onnx` del snapshot de Parakeet: la raíz de modelos vive fuera
/// del repo y la provisiona `ai-voice-interconnector setup`. El mensaje lleva el
/// marcador `requiere` porque es la dependencia de recurso que el estado del
/// daemon carga con `native-stt`: sin el modelo, la prueba que lo exige falla
/// aquí en vez de omitirse.
#[cfg(feature = "native-stt")]
fn require_parakeet() {
    let model_dir = avi_store::ModelStore::new()
        .expect("la raíz de modelos debe resolverse")
        .model_snapshot_path("parakeet-tdt-v3")
        .expect("el modelo parakeet-tdt-v3 debe tener un pin de revisión");
    let preprocessor = model_dir.join("nemo128.onnx");
    std::fs::metadata(&preprocessor).unwrap_or_else(|e| {
        panic!(
            "requiere Parakeet: falta {} ({e}): provisiona Parakeet con `ai-voice-interconnector setup`",
            preprocessor.display()
        )
    });
}

/// Exige `model.safetensors` del snapshot del modelo Base de clonado: lo
/// provisiona `ai-voice-interconnector setup`.
fn require_base_model() {
    let model_dir = avi_store::ModelStore::new()
        .expect("la raíz de modelos debe resolverse")
        .model_snapshot_path("qwen3-tts-0.6b-base")
        .expect("el modelo qwen3-tts-0.6b-base debe tener un pin de revisión");
    let weights = model_dir.join("model.safetensors");
    std::fs::metadata(&weights).unwrap_or_else(|e| {
        panic!(
            "falta {} ({e}): provisiona el modelo Base con `ai-voice-interconnector setup`",
            weights.display()
        )
    });
}

/// Binario del motor en `vendor/qwen3-tts` del workspace.
fn vendor_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
        "../../vendor/qwen3-tts/qwen_tts.exe"
    } else {
        "../../vendor/qwen3-tts/qwen_tts"
    })
}

/// Exige el binario del motor, salvo que `QWEN3_TTS_BIN` ya apunte a otro. Lo
/// construye `cargo xtask build-engine`.
fn require_clone_binary() {
    if std::env::var_os("QWEN3_TTS_BIN").is_some() {
        return;
    }
    let binary = vendor_binary();
    assert!(
        binary.is_file(),
        "falta {}: constrúyelo con `cargo xtask build-engine`",
        binary.display()
    );
}

#[tokio::test]
async fn health_matches_fixture() {
    // Los almacenes del estado deben anclarse bajo un directorio temporal propio
    // y no en el directorio de datos del usuario; se afirma antes de escribir.
    let tmp = process_root();
    let state = test_state();
    assert!(
        state.voice_store.root().starts_with(tmp),
        "raíz de voces fuera del temporal: {}",
        state.voice_store.root().display()
    );
    assert!(
        state.speech_store.root().starts_with(tmp),
        "raíz de habla fuera del temporal: {}",
        state.speech_store.root().display()
    );
    let (status, bytes) = send(get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let fixture_val = fixture("daemon_health.json");
    assert_eq!(actual["status"], fixture_val["status"]);
    assert_eq!(actual["warm"], fixture_val["warm"]);
    assert_eq!(actual["schema_version"], Value::String("5".to_string()));
    // Las claves aditivas `ct2` y `stt` no cambian `schema_version`.
    assert_eq!(actual["engine"], fixture_val["engine"]);
}

#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_matches_fixture() {
    require_parakeet();
    // Payload `{}` (campo audio_b64 ausente) → rama de error de campo ausente
    // diseñada a propósito (no un stub `transcription_pending`), con estado 400.
    let (status, bytes) = send(post_json("/transcribe", serde_json::json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_transcribe.json"));
}

#[tokio::test]
async fn synthesize_empty_text_is_contract_error() {
    let (status, bytes) = send(post_json("/synthesize", serde_json::json!({ "text": "" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

/// Texto de 501 caracteres: el daemon lo rechaza con 400 y `text_too_long`.
#[tokio::test]
async fn synthesize_text_too_long_is_contract_error() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "a".repeat(501) }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_too_long.json"));
}

/// El texto se valida antes que los demás campos: un payload mínimo con texto
/// vacío recibe el error de contrato de texto vacío.
#[tokio::test]
async fn synthesize_minimal_payload_rejects_empty_text() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

/// Temperatura fuera de rango (`0`): el stream NDJSON termina en error con
/// motivo `usage_error`, sin llegar a la síntesis.
#[tokio::test]
async fn synthesize_invalid_temperature_is_usage_error() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({
            "text": "hola", "voice": "default",
            "source_language": "es", "target_language": "es",
            "temperature": 0.0
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    let final_event = events.last().expect("debe haber al menos un evento");
    assert_eq!(final_event["event"], Value::String("error".to_string()));
    assert_eq!(
        final_event["reason"],
        Value::String("usage_error".to_string())
    );
}

#[tokio::test]
async fn synthesize_emits_contract_ndjson_stream() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({
            "text": "hola", "voice": "default",
            "source_language": "es", "target_language": "es"
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

    // El cuerpo es NDJSON: una línea JSON por evento (started/progress/result|error).
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    assert!(!events.is_empty(), "debe haber al menos un evento");

    // Invariante de envelope: schema_version=5 en todo evento.
    for e in &events {
        assert_eq!(
            e["schema_version"],
            Value::String("5".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }

    // Invariante: el primer evento es `started`.
    assert_eq!(
        events.first().unwrap()["event"],
        Value::String("started".to_string()),
        "el stream NDJSON debe comenzar con `started`"
    );

    // Invariante: `started` nombra la operación con la clave `operation`, que
    // es lo que permite al cliente anunciar «Iniciando síntesis…» en vez de
    // una operación genérica.
    assert_eq!(
        events.first().unwrap()["operation"],
        Value::String("síntesis".to_string()),
        "`started` nombra la operación"
    );

    // Invariante: evento final `error` por falta de motor.
    // El estado de contrato no tiene binario del motor, así que el evento final
    // es `error` con `reason` `model_missing`. La síntesis real con audio
    // verdadero se verifica por separado contra el motor.
    let final_event = events.last().unwrap();
    assert_eq!(
        final_event["event"],
        Value::String("error".to_string()),
        "evento final insuficiente: {:?}",
        final_event
    );
    assert_eq!(
        final_event["reason"],
        Value::String("model_missing".to_string()),
        "evento final insuficiente: {:?}",
        final_event
    );
}

/// Audio largo (~22 s, concatenación de 4 corpus): Parakeet no necesita chunking VAD
/// (RTF ~0.11 lineal); se transcribe de una sola pasada y se verifica el texto unido.
#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_long_audio_transcribes_in_one_pass() {
    require_parakeet();
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../avi-stt/tests/assets");
    let corpus = [
        "corpus_synthesis_16k.wav",
        "corpus_watermark_16k.wav",
        "corpus_responses_16k.wav",
        "parakeet_sample_16k.wav",
    ];
    let mut pcm: Vec<i16> = Vec::new();
    for wav in corpus {
        let seg = avi_audio::load_wav_16k_mono_pcm(assets.join(wav))
            .expect("el WAV corpus debe cargarse");
        pcm.extend_from_slice(&seg);
    }
    assert!(
        pcm.len() > 240_000,
        "el audio concatenado debe superar los 15 s de una sola pasada"
    );

    let audio_bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let (status, bytes) = send(post_json(
        "/transcribe",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio_bytes),
            "source_language": "es",
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let text = actual["text"].as_str().unwrap_or("");
    let norm = text.to_lowercase();
    // Frases de cada corpus que el modelo transcribe bien (el sintético
    // `sintesis` tiene pronunciación defectuosa → se usan palabras estables).
    // "hola" se descuenta: el fixture `parakeet_sample` (saludo breve) se emite
    // en inglés por el TDT, por lo que no aparece en el texto
    // unido aunque el resto del audio (watermark/sintesis/respuestas) sí se
    // transcribe en español. "voz" no se exige estricta: "esténtesis" puede
    // dropearla.
    for phrase in ["marca de agua", "usuario", "espajo"] {
        assert!(
            norm.contains(phrase),
            "el texto unido debe contener {:?}: {text:?}",
            phrase
        );
    }
}

/// Audio de ~70 s (PCM 16 kHz mono, ~3 MB de cuerpo en base64): supera el límite
/// de cuerpo por defecto de 2 MiB pero cabe en el tope de transcripción de 300 s,
/// así que `/transcribe` debe procesarlo y responder 200.
#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_audio_over_default_body_limit_is_accepted() {
    require_parakeet();
    let silence = vec![0u8; 70 * 16_000 * 2];
    let (status, bytes) = send(post_json(
        "/transcribe",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&silence),
            "source_language": "es",
        }),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
}

/// WAV PCM 16-bit mono 16 kHz de silencio con la duración indicada.
fn silent_wav(secs: u32) -> Vec<u8> {
    let data_len = secs * 16_000 * 2;
    let mut wav = Vec::<u8>::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(wav.len() + data_len as usize, 0);
    wav
}

/// Un cuerpo de más de 2 MiB con nombre de voz inválido se valida y responde 400
/// `invalid_voice_name`: el límite de cuerpo de la ruta no puede cortar antes con 413.
#[tokio::test]
async fn voices_clone_oversized_body_reports_invalid_name_not_413() {
    let oversized = vec![0u8; 3 * 1024 * 1024];
    let (status, bytes) = send(post_json(
        "/voices/clone",
        serde_json::json!({
            "name": "../invalid",
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&oversized),
        }),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], "invalid_voice_name");
}

/// Una referencia WAV de 31 s supera el tope de 30 s y se rechaza con 400
/// `audio_too_long` antes de buscar el modelo Base. El estado se construye sin
/// modelo Base para que la prueba no pueda lanzar el clonado real aunque el
/// modelo esté provisionado en el equipo.
#[tokio::test]
async fn voices_clone_reference_over_limit_is_audio_too_long() {
    #[cfg(feature = "native-stt")]
    require_parakeet();
    let (status, bytes) = send_to(
        state_without_base_model(),
        post_json(
            "/voices/clone",
            serde_json::json!({
                "name": "long_reference",
                "force": true,
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(silent_wav(31)),
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], "audio_too_long");
}

/// El `started` de `/voices/clone` nombra la operación con la clave
/// `operation`, que es lo que permite al cliente anunciar «Iniciando
/// clonación…».
///
/// La emisión del `started` ocurre **antes** del trabajo pesado, así que la
/// prueba no necesita el motor: monta un estado con un modelo Base presente
/// —su contenido no se usa, porque el clonado acabará fallando— y lee el
/// primer evento del stream. Es la cobertura que falta para que la clave
/// `operation` de esta ruta esté verificada fuera de la puerta que exige el
/// modelo provisioned.
#[tokio::test]
async fn voices_clone_names_the_operation_in_the_started_event() {
    let tmp = tempfile::Builder::new()
        .prefix("avi_daemon_clone_operation_")
        .tempdir()
        .expect("directorio temporal del estado");
    let mut state = DaemonState::with_stores(
        VoiceStore::at(tmp.path().join("voices")),
        SpeechStore::at(tmp.path().join("speech")),
    )
    .expect("daemon state");
    // El modelo Base solo tiene que estar *presente*: el handler lo comprueba
    // antes de abrir el stream y, con el clonado fallando después, el
    // `started` ya salió.
    state.tts_engine.base_model_dir = Some(tmp.path().join("base-model"));
    let state = Arc::new(state);

    // Un WAV mínimo y válido: la cabecera se lee antes del modelo.
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(minimal_wav());
    let (status, bytes) = send_to(
        state,
        post_json(
            "/voices/clone",
            serde_json::json!({
                "name": "viva_clonacion",
                "force": true,
                "audio_b64": audio_b64,
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "el handler abre el stream");

    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    assert!(!events.is_empty(), "debe haber al menos un evento");
    let started = &events[0];
    assert_eq!(
        started["event"],
        Value::String("started".to_string()),
        "el stream debe comenzar con `started`"
    );
    assert_eq!(
        started["operation"],
        Value::String("clonación".to_string()),
        "`started` nombra la operación"
    );
    assert_eq!(
        started["name"],
        Value::String("viva_clonacion".to_string()),
        "`started` nombra la voz"
    );
}

/// Un WAV de un segundo de silencio: la cabecera que el handler mide antes
/// de buscar el modelo.
fn minimal_wav() -> Vec<u8> {
    let muestras: Vec<i16> = vec![0; 16_000];
    let samples = muestras
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect::<Vec<u8>>();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
    wav.extend_from_slice(&1u16.to_le_bytes()); // mono
    wav.extend_from_slice(&16_000u32.to_le_bytes()); // frecuencia
    wav.extend_from_slice(&32_000u32.to_le_bytes()); // bytes por segundo
    wav.extend_from_slice(&2u16.to_le_bytes()); // alineación
    wav.extend_from_slice(&16u16.to_le_bytes()); // bits por muestra
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&(samples.len() as u32).to_le_bytes());
    wav.extend_from_slice(&samples);
    wav
}

/// Warm-on-clone: `POST /voices/clone` por daemon sirve un stream
/// NDJSON (`started` → latidos → `result`), y el evento final conserva
/// `precomputed: true` («precarga en caliente iniciada»; la completitud se
/// refleja en `/health`). El test solo pasa si la secuencia completa llega
/// hasta el final.
/// Requiere el modelo Base de clonado: sin él el handler retorna
/// `model_missing` en vez de clonar.
#[tokio::test]
#[ignore = "requiere el modelo Base de clonado y el binario del motor"]
async fn voices_clone_daemon_precomputed_true() {
    require_base_model();
    require_clone_binary();
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../avi-stt/tests/assets/parakeet_sample_16k.wav");
    let audio_bytes = std::fs::read(&wav).expect("el WAV de muestra debe leerse");
    let name = format!("clone_warm_{}", std::process::id());
    let (status, bytes) = send_to(
        clone_state(),
        post_json(
            "/voices/clone",
            serde_json::json!({
                "name": name,
                "force": true,
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio_bytes),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // El cuerpo es NDJSON: una línea JSON por evento.
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    // Un clonado con éxito deja en segundo plano una precarga que arranca el
    // motor residente; se espera a que termine y se detiene el residente antes
    // de las aserciones para que no sobreviva al proceso de pruebas.
    if events.last().is_some_and(|e| e["event"] == "result") {
        let state = clone_state();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        while matches!(*state.warm.read().unwrap(), WarmState::Warming) {
            assert!(
                std::time::Instant::now() < deadline,
                "la precarga del clon no terminó en 300 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        state.tts_engine.shutdown();
    }
    assert!(!events.is_empty(), "debe haber al menos un evento");
    // Invariante de envelope: schema_version=5 en todo evento.
    for e in &events {
        assert_eq!(
            e["schema_version"],
            Value::String("5".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }
    // Aceptación inmediata: el primer evento es `started` con el nombre.
    assert_eq!(
        events.first().unwrap()["event"],
        Value::String("started".to_string()),
        "el stream debe comenzar con `started`"
    );
    // `started` nombra la operación con la clave `operation`, que es lo que
    // permite al cliente anunciar «Iniciando clonación…».
    assert_eq!(
        events.first().unwrap()["operation"],
        Value::String("clonación".to_string()),
        "`started` nombra la operación"
    );
    assert_eq!(events.first().unwrap()["name"], Value::String(name.clone()));
    // Evento final `result` con la forma contractual actual (`precomputed: true`).
    let final_event = events.last().unwrap();
    assert_eq!(
        final_event["event"],
        Value::String("result".to_string()),
        "el stream debe terminar con `result`: {:?}",
        final_event
    );
    assert_eq!(final_event["name"], Value::String(name.clone()));
    assert_eq!(final_event["precomputed"], Value::Bool(true));
    // Intermedios: solo latidos o progreso (nunca un segundo `started`/`result`).
    if events.len() > 2 {
        for e in &events[1..events.len() - 1] {
            let ev = e["event"].as_str().unwrap_or("");
            assert!(
                ev == "heartbeat" || ev == "progress",
                "evento intermedio debe ser latido o progreso: {:?}",
                e
            );
        }
    }
    let state = clone_state();
    let _ = avi_store::remove_voice(&state.voice_store, &state.speech_store, &name);
}

/// Una referencia WAV truncada (la cabecera declara más datos de los que hay)
/// se rechaza en la carga, antes de invocar el motor de clonado, con un evento
/// `error` de razón `invalid_audio`.
/// Requiere el modelo Base de clonado: sin él el handler responde
/// `model_missing` antes de llegar a la carga del audio.
#[tokio::test]
#[ignore = "requiere el modelo Base de clonado"]
async fn voices_clone_truncated_wav_emits_invalid_audio() {
    require_base_model();
    // WAV PCM 16-bit mono 16 kHz: cabecera de 44 bytes que declara 3200 bytes de
    // datos y solo 1000 presentes.
    let mut wav = Vec::<u8>::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + 3200).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&3200u32.to_le_bytes());
    wav.extend_from_slice(&[0u8; 1000]);
    let name = format!("clone_truncated_{}", std::process::id());
    let (status, bytes) = send(post_json(
        "/voices/clone",
        serde_json::json!({
            "name": name,
            "force": true,
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&wav),
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let last: Value = serde_json::from_str(text.lines().rfind(|l| !l.trim().is_empty()).unwrap())
        .expect("cada línea debe ser JSON");
    assert_eq!(last["event"], Value::String("error".to_string()));
    assert_eq!(last["reason"], Value::String("invalid_audio".to_string()));
    let state = test_state();
    let _ = avi_store::remove_voice(&state.voice_store, &state.speech_store, &name);
}

/// Warm-voice configurable: `run_daemon_server` con una `--warm-voice`
/// inexistente aborta con `StartupError::WarmVoiceMissing`, porque la voz se
/// valida antes de enlazar y de cargar modelos (por eso esta prueba no
/// necesita modelos provisionados), y una voz existente pasa la validación de
/// arranque. La rama de aceptación se verifica sobre el mismo predicado que
/// usa la guarda (`exists(...)`): arrancar el servidor real
/// con voz válida bloquearía sirviendo, así que no se invoca.
#[tokio::test]
async fn warm_voice_fail_fast_and_acceptance() {
    // Fail-fast: voz inexistente → Err antes del bind.
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let nonexistent_voice = format!("warm_nonexistent_{}", std::process::id());
    let dir = tempfile::tempdir().expect("directorio temporal");
    let tmp = dir.path();
    let res = avi_daemon::run_daemon_server(
        addr,
        nonexistent_voice.clone(),
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
        None,
    )
    .await;
    let err = res.expect_err("una --warm-voice inexistente debe abortar el arranque");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::WarmVoiceMissing {
            voice: nonexistent_voice.clone()
        })
    );
    assert!(
        err.to_string().contains("--warm-voice"),
        "el error fail-fast debe mencionar --warm-voice: {err}"
    );

    // Aceptación: una voz existente satisface el predicado de la guarda.
    let ok_dir = tempfile::tempdir().expect("directorio temporal");
    let store = VoiceStore::at(ok_dir.path().join("voices"));
    let qvoice = ok_dir.path().join("warm_ok.qvoice");
    std::fs::write(&qvoice, b"qvoice-fixture").expect("escribir fixture qvoice");
    let name = format!("warm_ok_{}", std::process::id());
    store
        .save_reference(&name, &qvoice)
        .expect("registrar voz de prueba");
    assert!(
        store.exists(&name).expect("resolver la voz de prueba"),
        "la voz registrada debe pasar la validación de --warm-voice"
    );
    // Un preset del motor no tiene referencia `.qvoice`, pero existe igual.
    assert!(
        store.exists("ryan").expect("resolver el preset"),
        "un preset debe pasar la validación de --warm-voice"
    );
}

/// Puerto ocupado: `run_daemon_server` devuelve `StartupError::PortInUse` con
/// el puerto pedido en menos de 2 s, porque enlaza antes de cargar los
/// modelos. No necesita modelos provisionados.
#[tokio::test]
async fn port_in_use_fails_before_loading_models() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("ocupar un puerto");
    let busy = holder.local_addr().unwrap();
    let start = std::time::Instant::now();
    let dir = tempfile::tempdir().expect("directorio temporal");
    let err = avi_daemon::run_daemon_server(
        busy,
        "default".into(),
        VoiceStore::at(dir.path().join("voices")),
        SpeechStore::at(dir.path().join("speech")),
        None,
    )
    .await
    .expect_err("un puerto ocupado debe abortar el arranque");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::PortInUse { port: busy.port() })
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "el fallo por puerto ocupado debe ser inmediato: {:?}",
        start.elapsed()
    );
}

/// La supervisión no reintenta un fallo previo a estar listo: con el puerto
/// ocupado, `run_supervised` con `--auto-restart` devuelve `PortInUse` en
/// menos de 2 s. Un reintento costaría al menos el primer backoff (500 ms) más
/// la espera de puerto libre (hasta 5 s).
#[tokio::test]
async fn supervised_startup_failure_is_terminal() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("ocupar un puerto");
    let busy = holder.local_addr().unwrap();
    let start = std::time::Instant::now();
    let dir = tempfile::tempdir().expect("directorio temporal");
    let err = avi_daemon::run_supervised(
        busy,
        true,
        3,
        "default".into(),
        VoiceStore::at(dir.path().join("voices")),
        SpeechStore::at(dir.path().join("speech")),
        None,
    )
    .await
    .expect_err("un puerto ocupado debe abortar la supervisión");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::PortInUse { port: busy.port() })
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "la supervisión no debe reintentar un fallo de arranque: {:?}",
        start.elapsed()
    );
}

/// Proceso de larga vida distinto del de la prueba; muere al soltarse, también
/// si la prueba falla.
struct Sleeper(std::process::Child);

impl Sleeper {
    fn spawn() -> Self {
        #[cfg(windows)]
        let child = std::process::Command::new("ping")
            .args(["-n", "120", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn();
        #[cfg(not(windows))]
        let child = std::process::Command::new("sleep").arg("120").spawn();
        Self(child.expect("lanzar el proceso de larga vida"))
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Las pruebas del binario comparten la raíz de datos (`AVI_DATA_DIR`) y con
/// ella `daemon.pid`, que reescribe el registro de cualquier estado cuyo motor
/// lance un residente real al anotarlo y al anularlo. La prueba que afirma que
/// el pidfile no cambia y las que lanzan un residente real se serializan con
/// este cerrojo; si no, una escritura del residente entre sus dos lecturas la
/// haría fallar.
static REGISTRY_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Toma el cerrojo de `daemon.pid` del binario de pruebas.
async fn lock_registry() -> tokio::sync::MutexGuard<'static, ()> {
    REGISTRY_LOCK.lock().await
}

/// Un segundo daemon no arranca mientras el pidfile registre a uno vivo con la
/// identidad coincidente: el arranque falla con `StartupError::Failed` que nombra
/// al otro daemon, sin servir y sin tocar el pidfile ajeno.
#[tokio::test]
async fn second_daemon_is_rejected_while_a_live_one_is_registered() {
    let _registry = lock_registry().await;
    #[cfg(feature = "native-stt")]
    require_parakeet();
    prepare_shared_env();
    let data_dir = process_root().join("data");
    std::fs::create_dir_all(&data_dir).expect("crear la raíz de datos de la prueba");

    let peer = Sleeper::spawn();
    let peer_pid = peer.0.id();
    let peer_file = avi_process::registry::PidFile {
        pid: peer_pid,
        addr: Some("127.0.0.1:1".to_string()),
        started_at: None,
        resident_pid: 0,
        identity: Some(
            avi_process::process_identity(peer_pid).expect("observar la identidad del otro daemon"),
        ),
        resident_identity: None,
        resident_port: None,
    };
    avi_process::registry::write_pid_file(&data_dir, &peer_file).expect("registrar al otro daemon");
    let pidfile_before = std::fs::read(avi_process::registry::pid_path(&data_dir)).unwrap();

    let dir = tempfile::tempdir().expect("directorio temporal");
    let result = avi_daemon::run_daemon_server(
        "127.0.0.1:0".parse().unwrap(),
        "default".into(),
        VoiceStore::at(dir.path().join("voices")),
        SpeechStore::at(dir.path().join("speech")),
        None,
    )
    .await;
    let pidfile_after = std::fs::read(avi_process::registry::pid_path(&data_dir)).unwrap();
    let _ = std::fs::remove_file(avi_process::registry::pid_path(&data_dir));

    let err = result.expect_err("con otro daemon vivo registrado el arranque debe fallar");
    match err.downcast_ref::<avi_daemon::StartupError>() {
        Some(avi_daemon::StartupError::Failed { message }) => assert!(
            message.contains(&peer_pid.to_string()),
            "el fallo debe nombrar al otro daemon: {message}"
        ),
        other => panic!("se esperaba StartupError::Failed, fue {other:?}"),
    }
    assert_eq!(
        pidfile_before, pidfile_after,
        "el pidfile del daemon vivo no se modifica"
    );
}

/// Envía `body` a `/translate` y afirma 400 con `usage_error`: la validación de
/// los campos corre antes de tocar el modelo, sin CT2 ni TCP.
#[cfg(feature = "native-translation")]
async fn assert_translate_usage_error(body: Value) {
    let (status, bytes) = send(post_json("/translate", body.clone())).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "cuerpo: {body}");
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
}

/// Un idioma fuera de `es`/`en` vía IPC → 400 con `usage_error`.
#[cfg(feature = "native-translation")]
#[tokio::test]
async fn translate_unknown_language_returns_usage_error() {
    assert_translate_usage_error(serde_json::json!({
        "text": "Bonjour", "source_language": "fr", "target_language": "de"
    }))
    .await;
}

/// `es-latam` no pertenece al vocabulario del daemon → 400 con `usage_error`.
#[cfg(feature = "native-translation")]
#[tokio::test]
async fn translate_es_latam_returns_usage_error() {
    assert_translate_usage_error(serde_json::json!({
        "text": "Hola", "source_language": "es-latam", "target_language": "es"
    }))
    .await;
}

/// Un idioma ausente o enviado con una clave alias (`from`/`to`, `source`/`target`)
/// → 400 con `usage_error`.
#[cfg(feature = "native-translation")]
#[tokio::test]
async fn translate_missing_or_aliased_language_returns_usage_error() {
    for body in [
        serde_json::json!({ "text": "Hola", "target_language": "en" }),
        serde_json::json!({ "text": "Hola", "source_language": "es" }),
        serde_json::json!({ "text": "Hola", "from": "es", "to": "en" }),
        serde_json::json!({ "text": "Hola", "source": "es", "target": "en" }),
    ] {
        assert_translate_usage_error(body).await;
    }
}

/// Con el mismo idioma de origen y destino la vía daemon devuelve el texto
/// intacto y los idiomas bajo las claves `source_language`/`target_language`.
#[cfg(feature = "native-translation")]
#[tokio::test]
async fn translate_same_language_passthrough_returns_intact_text() {
    let (status, bytes) = send(post_json(
        "/translate",
        serde_json::json!({ "text": "Hola", "source_language": "es", "target_language": "es" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["translated"], Value::String("Hola".to_string()));
    assert_eq!(actual["source_language"], Value::String("es".to_string()));
    assert_eq!(actual["target_language"], Value::String("es".to_string()));
}

/// `/synthesize` rechaza con `usage_error` (400 plano) un campo ausente, un
/// idioma `es-latam`, una clave alias (`from`/`to`) y un tipo inválido en
/// `voice` o `temperature`.
#[tokio::test]
async fn synthesize_invalid_fields_return_usage_error() {
    let valid = serde_json::json!({
        "text": "hola", "voice": "default",
        "source_language": "es", "target_language": "es"
    });
    let mut cases: Vec<Value> = Vec::new();
    for missing in ["voice", "source_language", "target_language"] {
        let mut body = valid.clone();
        body.as_object_mut().unwrap().remove(missing);
        cases.push(body);
    }
    for (key, value) in [
        ("source_language", serde_json::json!("es-latam")),
        ("target_language", serde_json::json!("es-latam")),
        ("voice", serde_json::json!(7)),
        ("temperature", serde_json::json!("alta")),
    ] {
        let mut body = valid.clone();
        body[key] = value;
        cases.push(body);
    }
    cases.push(serde_json::json!({
        "text": "hola", "voice": "default", "from": "es", "to": "es"
    }));
    for body in cases {
        let (status, bytes) = send(post_json("/synthesize", body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cuerpo: {body}");
        let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
        assert_eq!(
            actual["reason"],
            Value::String("usage_error".to_string()),
            "cuerpo: {body}"
        );
    }
}

/// `/dub` rechaza con `usage_error` (400 plano) un campo ausente, un idioma
/// `es-latam`, una clave alias (`from`/`to`) y un tipo inválido en `voice` o
/// `temperature`. Los campos se validan antes de medir o procesar el audio.
#[tokio::test]
async fn dub_invalid_fields_return_usage_error() {
    let valid = serde_json::json!({
        "audio_b64": base64::engine::general_purpose::STANDARD.encode([0u8; 4]),
        "voice": "default",
        "source_language": "es", "target_language": "es"
    });
    let mut cases: Vec<Value> = Vec::new();
    for missing in ["voice", "source_language", "target_language"] {
        let mut body = valid.clone();
        body.as_object_mut().unwrap().remove(missing);
        cases.push(body);
    }
    for (key, value) in [
        ("source_language", serde_json::json!("es-latam")),
        ("target_language", serde_json::json!("es-latam")),
        ("voice", serde_json::json!(7)),
        ("temperature", serde_json::json!("alta")),
    ] {
        let mut body = valid.clone();
        body[key] = value;
        cases.push(body);
    }
    let mut aliased = valid.clone();
    let object = aliased.as_object_mut().unwrap();
    object.remove("source_language");
    object.remove("target_language");
    object.insert("from".into(), serde_json::json!("es"));
    object.insert("to".into(), serde_json::json!("es"));
    cases.push(aliased);
    for body in cases {
        let (status, bytes) = send(post_json("/dub", body.clone())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "cuerpo: {body}");
        let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
        assert_eq!(
            actual["reason"],
            Value::String("usage_error".to_string()),
            "cuerpo: {body}"
        );
    }
}

/// `/dub` emite un stream NDJSON cuyo primer evento nombra la operación con
/// la clave `operation`.
///
/// El caso es el mismo que el de `/synthesize` y `/voices/clone`: la clave
/// es lo que permite al cliente decir «Iniciando doblaje…» en vez de
/// anunciar una operación genérica. Sin motor, el stream termina con un
/// evento `error`, pero el `started` se emite antes de llegar al trabajo.
///
/// La ruta de doblaje necesita el soporte de transcripción, así que el stream
/// solo existe con el feature `native-stt`: sin él, la ruta no está montada
/// y la prueba lo dice en vez de fingir.
#[cfg(feature = "native-stt")]
#[tokio::test]
async fn dub_emits_contract_ndjson_stream_naming_the_operation() {
    let pcm: Vec<u8> = vec![0i16; 16_000]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let (status, bytes) = send(post_json(
        "/dub",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&pcm),
            "source_language": "es",
            "target_language": "es",
            "voice": "default",
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    assert!(!events.is_empty(), "debe haber al menos un evento");

    // Invariante de envelope: schema_version=5 en todo evento.
    for e in &events {
        assert_eq!(
            e["schema_version"],
            Value::String("5".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }

    // Invariante: el primer evento es `started` y nombra la operación.
    let first = events.first().expect("debe haber un primer evento");
    assert_eq!(
        first["event"],
        Value::String("started".to_string()),
        "el stream NDJSON debe comenzar con `started`"
    );
    assert_eq!(
        first["operation"],
        Value::String("doblaje".to_string()),
        "`started` nombra la operación"
    );
}

/// `/voices/clone` rechaza con `usage_error` un `force` que no es booleano.
#[tokio::test]
async fn voices_clone_non_boolean_force_returns_usage_error() {
    let (status, bytes) = send(post_json(
        "/voices/clone",
        serde_json::json!({ "name": "voz-prueba", "audio_b64": "AAAA", "force": "si" }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
}

/// `/dub` no deja su WAV temporal en ninguna salida, tampoco si la
/// lectura del fichero sintetizado falla.
///
/// La rama de error no se puede provocar sin motor (la síntesis escribe el
/// fichero que luego se lee), así que este test fija el invariante de punta a
/// punta con el motor cuando hay recursos: tras `/dub` no queda ningún
/// `avi_daemon_dub_*.wav`, salga el doblaje o un error. La rama `Err` queda
/// cubierta por construcción: el WAV tiene un tipo dueño que lo borra al
/// soltarse.
#[tokio::test]
#[ignore = "requiere Parakeet y el binario del motor Qwen3-TTS"]
async fn dub_leaves_no_temporary_on_any_outcome() {
    let _temporaries = lock_temporaries().await;
    let _registry = lock_registry().await;
    #[cfg(feature = "native-stt")]
    require_parakeet();
    require_clone_binary();
    let pcm: Vec<u8> = vec![0i16; 16_000]
        .iter()
        .flat_map(|s| s.to_le_bytes())
        .collect();
    let (status, bytes) = send_to(
        clone_state(),
        post_json(
            "/dub",
            serde_json::json!({
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&pcm),
                "source_language": "es",
                "target_language": "es",
                "voice": "default",
            }),
        ),
    )
    .await;
    if status == StatusCode::OK {
        let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
        let events: Vec<Value> = text
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
            .collect();
        let final_event = events.last().expect("debe haber al menos un evento");
        assert!(
            final_event.get("event").and_then(|v| v.as_str()) == Some("result")
                || final_event.get("event").and_then(|v| v.as_str()) == Some("error"),
            "el evento final es result o error: {final_event:?}"
        );
    } else {
        let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
        assert!(
            actual.get("reason").and_then(|v| v.as_str()).is_some(),
            "el error lleva reason: {actual:?}"
        );
    }
    let residue: Vec<String> = std::fs::read_dir(avi_process::scratch::dir().expect("scratch"))
        .expect("el scratch debe poder leerse")
        .map(|e| {
            e.expect("entrada del directorio temporal")
                .file_name()
                .to_string_lossy()
                .to_string()
        })
        .filter(|n| n.starts_with("avi_daemon_dub_") && n.ends_with(".wav"))
        .collect();
    assert!(
        residue.is_empty(),
        "tras /dub no debe quedar temporal: {residue:?}"
    );
}

// ─── barrido de temporales del daemon ───────────────────────────────────

/// Las pruebas que barren los temporales del daemon se serializan entre sí: el
/// binario de pruebas corre en paralelo y el barrido de residuo de una vería el
/// temporal en vuelo de otra. Cerrojo asíncrono para no retener un
/// `std::sync::MutexGuard` a través de un `await`.
static TEMPORARIES_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Toma el cerrojo de temporales del binario de pruebas.
async fn lock_temporaries() -> tokio::sync::MutexGuard<'static, ()> {
    TEMPORARIES_LOCK.lock().await
}

/// Exige `model.safetensors` del snapshot del modelo CustomVoice de los
/// presets: lo provisiona `ai-voice-interconnector setup`.
fn require_custom_voice_model() {
    let model_dir = avi_store::ModelStore::new()
        .expect("la raíz de modelos debe resolverse")
        .model_snapshot_path("qwen3-tts-0.6b")
        .expect("el modelo qwen3-tts-0.6b debe tener un pin de revisión");
    let weights = model_dir.join("model.safetensors");
    std::fs::metadata(&weights).unwrap_or_else(|e| {
        panic!(
            "falta {} ({e}): provisiona el modelo CustomVoice con `ai-voice-interconnector setup`",
            weights.display()
        )
    });
}

/// Temporales del daemon con el prefijo dado que hay en el scratch de este
/// proceso: el scratch es propio, así que el filtro no mezcla los de otros
/// procesos.
fn temp_wavs(prefix: &str) -> Vec<PathBuf> {
    std::fs::read_dir(avi_process::scratch::dir().expect("scratch"))
        .expect("el scratch debe poder leerse")
        .map(|e| e.expect("entrada del scratch").path())
        .filter(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(prefix))
        })
        .collect()
}

/// Sondea cada 20 ms hasta que `cond` sea cierta; falla con `what` si vence
/// `limit`.
async fn wait_until(what: &str, limit: std::time::Duration, mut cond: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + limit;
    while std::time::Instant::now() < deadline {
        if cond() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(cond(), "no se cumplió a tiempo: {what}");
}

/// Eventos NDJSON de un cuerpo de respuesta.
fn ndjson_events(bytes: &[u8]) -> Vec<Value> {
    String::from_utf8(bytes.to_vec())
        .expect("NDJSON debe ser UTF-8")
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect()
}

/// Audio decodificado del evento `result` de una síntesis; falla si el evento
/// final no es un resultado.
fn result_audio(events: &[Value]) -> Vec<u8> {
    let result = events.last().expect("debe haber un evento final");
    assert_eq!(result["event"], "result", "evento final: {result:?}");
    base64::engine::general_purpose::STANDARD
        .decode(result["audio_b64"].as_str().expect("audio_b64"))
        .expect("el audio_b64 debe decodificar")
}

/// Camino feliz de `/synthesize` con el motor real: un preset se sintetiza en el
/// residente, el handler entrega un WAV no vacío y no deja temporal.
#[tokio::test]
#[ignore = "requiere el binario del motor y el modelo CustomVoice"]
async fn synthesize_preset_delivers_audio_and_leaves_no_temporary() {
    require_clone_binary();
    require_custom_voice_model();
    let _temporaries = lock_temporaries().await;
    let (status, bytes) = send_to(
        clone_state(),
        post_json(
            "/synthesize",
            serde_json::json!({
                "text": "Hola, esto es una prueba.", "voice": "ryan",
                "source_language": "es", "target_language": "es"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let audio = result_audio(&ndjson_events(&bytes));
    assert!(audio.starts_with(b"RIFF"), "se esperaba un WAV");
    assert!(audio.len() > 44, "el WAV no debe estar vacío");
    assert!(
        temp_wavs("avi_daemon_synth_").is_empty(),
        "la ruta feliz no debe dejar temporal"
    );
}

/// El idioma de destino `es` y `en` de un preset producen audio no vacío.
#[tokio::test]
#[ignore = "requiere el binario del motor y el modelo CustomVoice"]
async fn synthesize_preset_in_each_language_delivers_audio() {
    require_clone_binary();
    require_custom_voice_model();
    let _temporaries = lock_temporaries().await;
    for (token, text) in [
        ("es", "Hola, esto es una prueba."),
        ("en", "Hello, this is a test."),
    ] {
        let (status, bytes) = send_to(
            clone_state(),
            post_json(
                "/synthesize",
                serde_json::json!({
                    "text": text, "voice": "ryan",
                    "source_language": token, "target_language": token
                }),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "idioma {token}");
        let audio = result_audio(&ndjson_events(&bytes));
        assert!(
            audio.len() > 44,
            "idioma {token}: el WAV no debe estar vacío"
        );
    }
}

/// Recorrido común de la salida por desconexión: espera a que la fase cree su
/// temporal, el cliente corta el stream sin leerlo hasta el final y se afirma
/// que, al terminar el trabajo bloqueante, no queda ningún temporal propio.
async fn disconnect_leaves_no_temporary(prefix: &str, response: axum::response::Response) {
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body();
    // Primer fotograma (`started`): la fase ya validó y está en marcha.
    body.frame()
        .await
        .expect("el stream debe emitir al menos un fotograma")
        .expect("el fotograma debe leerse");
    wait_until(
        "la fase crea su temporal",
        std::time::Duration::from_secs(120),
        || !temp_wavs(prefix).is_empty(),
    )
    .await;
    // El cliente corta a mitad del trabajo.
    drop(body);
    wait_until(
        "el trabajo bloqueante termina y borra su temporal",
        std::time::Duration::from_secs(180),
        || temp_wavs(prefix).is_empty(),
    )
    .await;
}

/// Al cortar el cliente el stream de `/synthesize`, el WAV temporal desaparece
/// cuando termina el trabajo bloqueante y no queda residuo.
#[tokio::test]
#[ignore = "requiere el binario del motor y el modelo CustomVoice"]
async fn synthesize_disconnect_removes_temporary_when_blocking_work_ends() {
    require_clone_binary();
    require_custom_voice_model();
    let _temporaries = lock_temporaries().await;
    let response = build_router_with_state(clone_state())
        .oneshot(post_json(
            "/synthesize",
            serde_json::json!({
                "text": "Esta es una frase algo más larga para que la síntesis tarde un poco y el cliente pueda cortar a mitad del trabajo.",
                "voice": "ryan",
                "source_language": "es", "target_language": "es"
            }),
        ))
        .await
        .expect("el router debe responder");
    disconnect_leaves_no_temporary("avi_daemon_synth_", response).await;
}

/// En `/dub`: misma salida por desconexión sobre la ruta de doblaje, con su
/// prefijo de temporal. `/dub` transcribe antes de sintetizar.
#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet, el binario del motor y el modelo CustomVoice"]
async fn dub_disconnect_removes_temporary_when_blocking_work_ends() {
    require_parakeet();
    require_clone_binary();
    require_custom_voice_model();
    let _temporaries = lock_temporaries().await;
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../avi-stt/tests/assets");
    let pcm = avi_audio::load_wav_16k_mono_pcm(assets.join("corpus_responses_16k.wav"))
        .expect("el WAV corpus debe cargarse");
    let audio: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let response = build_router_with_state(clone_state())
        .oneshot(post_json(
            "/dub",
            serde_json::json!({
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio),
                "source_language": "es",
                "target_language": "es",
                "voice": "ryan",
            }),
        ))
        .await
        .expect("el router debe responder");
    disconnect_leaves_no_temporary("avi_daemon_dub_", response).await;
}

/// Eventos NDJSON de la respuesta de `/synthesize` para `voice`, con un estado
/// cuyo motor tiene binario y modelo CustomVoice inexistentes y sin modelo Base:
/// ninguna síntesis puede llegar a lanzar un motor real.
async fn synthesize_events_without_engine(dir_prefix: &str, voice: &str) -> Vec<Value> {
    prepare_shared_env();
    let mut state = contract_state(&process_root().join(dir_prefix));
    // Prueba: ruta inexistente bajo el temporal del sistema, sin crear nada.
    #[allow(clippy::disallowed_methods)]
    let missing = std::env::temp_dir().join("avi_golden_nonexistent_engine");
    state.tts_engine.binary_path = Some(missing.clone());
    state.tts_engine.model_dir = Some(missing);
    state.tts_engine.base_model_dir = None;
    std::fs::create_dir_all(state.voice_store.root().join("sin_referencia"))
        .expect("registrar una voz sin referencia");
    let (status, bytes) = send_to(
        Arc::new(state),
        post_json(
            "/synthesize",
            serde_json::json!({
                "text": "hola", "voice": voice,
                "source_language": "es", "target_language": "es"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    text.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect()
}

/// Una voz clonada sin el modelo Base provisionado responde `model_missing`:
/// el motor falla antes de lanzar nada porque la selección del modelo por tipo
/// de voz exige el Base.
#[tokio::test]
async fn synthesize_cloned_voice_without_base_model_is_model_missing() {
    let events = synthesize_events_without_engine("avi_golden_no_base", "default").await;
    let last = events.last().expect("debe haber un evento final");
    assert_eq!(last["event"], "error", "evento final: {last:?}");
    assert_eq!(last["reason"], "model_missing", "evento final: {last:?}");
}

/// Una voz registrada que no tiene `reference.qvoice` ni es un preset del motor
/// responde `voice_not_found`: la síntesis no la degrada a un speaker inexistente.
#[tokio::test]
async fn synthesize_voice_without_reference_nor_preset_is_voice_not_found() {
    let events =
        synthesize_events_without_engine("avi_golden_no_reference", "sin_referencia").await;
    let last = events.last().expect("debe haber un evento final");
    assert_eq!(last["event"], "error", "evento final: {last:?}");
    assert_eq!(last["reason"], "voice_not_found", "evento final: {last:?}");
}

/// Sin binario ni modelo CustomVoice resueltos, `/synthesize` responde
/// `model_missing`: la falta de motor y la de voz se distinguen.
#[tokio::test]
async fn synthesize_without_engine_is_model_missing() {
    prepare_shared_env();
    let mut state = contract_state(&process_root().join("avi_golden_no_engine"));
    state.tts_engine.binary_path = None;
    state.tts_engine.model_dir = None;
    let (status, bytes) = send_to(
        Arc::new(state),
        post_json(
            "/synthesize",
            serde_json::json!({
                "text": "hola", "voice": "ryan",
                "source_language": "es", "target_language": "es"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let last: Value = serde_json::from_str(text.lines().last().expect("evento final"))
        .expect("cada línea debe ser JSON");
    assert_eq!(last["event"], "error", "evento final: {last:?}");
    assert_eq!(last["reason"], "model_missing", "evento final: {last:?}");
}
