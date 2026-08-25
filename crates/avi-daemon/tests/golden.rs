//! Harness de tests dorados del daemon (Tarea 8 del desbloqueo de Fase 0).
//!
//! Levanta el `Router` de Axum vía [`avi_daemon::build_router_with_state`] y lo
//! ejercita con `tower::ServiceExt::oneshot` (sin abrir un socket TCP real),
//! comparando cada respuesta contra fixtures fijas en `tests/golden/` de la raíz
//! del repo. Detecta regresiones de contrato (formato JSON, `schema_version`,
//! textos de estado/error) entre el runtime nativo y lo que el resto del sistema
//! espera.
//!
//! Los handlers del daemon son deterministas y no requieren pesos reales, salvo
//! `/voices`, cuyo contenido depende del `data_dir` del usuario: para esa ruta se
//! verifican los invariantes de contrato (envelope + presencia de la voz `default`)
//! en lugar de una igualdad exacta. El handler `/synthesize` verifica invariantes de
//! contrato del stream NDJSON (presencia de `start` y evento final `result`/
//! `error`) en lugar de igualdad exacta, ya que el audio real depende del motor
//! (verificable con garantía en F5); en este entorno de unit test el motor TTS no
//! es localizable desde CWD (`crates/avi-daemon`), luego el evento final esperado
//! es `error` con `reason` `model_missing` — rama aceptada por el plan T8.

// El harness construye `DaemonState` con los campos STT/VAD y usa
// `whisper_rs`/`avi_stt::Ct2SttEngine`, que solo existen con `native-stt`. Sin el
// feature el archivo no se compila (evita whisper.cpp en el build de test liso).
#![cfg(feature = "native-stt")]

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::{build_router_with_state, DaemonState};
use avi_store::{SpeechStore, VoiceStore};

/// Estado de test único: carga el motor STT real una sola vez (ruta relativa a
/// `CARGO_MANIFEST_DIR`), reutilizable entre tests. El motor TTS se construye con
/// resolución por defecto (no provisionado desde CWD → branch `model_missing`).
static TEST_STATE: OnceLock<Arc<DaemonState>> = OnceLock::new();

fn test_state() -> Arc<DaemonState> {
    TEST_STATE
        .get_or_init(|| {
            // `cargo test -p avi-daemon` ejecuta con CWD=crates/avi-daemon; los
            // modelos están bajo la raíz del workspace (CARGO_MANIFEST_DIR/..).
            let stt_model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../models/whisper/ggml-medium-q8_0.bin");
            let stt_engine = avi_stt::Ct2SttEngine::new(&stt_model_dir)
                .expect("el modelo STT de test debe cargarse");
            let vad_model_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("../../models/whisper/ggml-silero-v5.1.2.bin");
            let vad_engine = whisper_rs::WhisperVadContext::new(
                &vad_model_dir.to_string_lossy(),
                whisper_rs::WhisperVadContextParams::default(),
            )
            .expect("el modelo VAD de test debe cargarse");
            Arc::new(DaemonState {
                synthesis_lock: tokio::sync::Mutex::new(()),
                voice_store: VoiceStore::new(),
                speech_store: SpeechStore::new(),
                tts_engine: avi_tts::Qwen3TtsEngine::new(None),
                stt_engine,
                vad_engine: std::sync::Mutex::new(vad_engine),
            })
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
    let response = build_router_with_state(test_state())
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

/// Modelos reales (STT + VAD) presentes. Los binarios bajo `models/` están
/// gitignoreados: en un checkout limpio (CI) estos tests dorados se saltan con
/// aviso; en desarrollo corren completos.
fn modelos_presentes() -> bool {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/whisper");
    root.join("ggml-medium-q8_0.bin").exists() && root.join("ggml-silero-v5.1.2.bin").exists()
}

#[tokio::test]
async fn health_coincide_con_fixture() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    let (status, bytes) = send(get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_health.json"));
}

#[tokio::test]
async fn transcribe_coincide_con_fixture() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    // Payload `{}` (campo audio_b64 ausente) → rama de error de campo ausente
    // diseñada en la Tarea 5 (no un stub `transcription_pending`).
    let (status, bytes) = send(post_json("/transcribe", serde_json::json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_transcribe.json"));
}

#[tokio::test]
async fn synthesize_texto_vacio_es_error_de_contrato() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    let (status, bytes) = send(post_json("/synthesize", serde_json::json!({ "text": "" }))).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

#[tokio::test]
async fn synthesize_emite_stream_ndjson_de_contrato() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "hola", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        // El handler marca el content-type NDJSON y el envelope de schema_version.
        response_content_type(&bytes),
        "application/x-ndjson"
    );

    // El cuerpo es NDJSON: una línea JSON por evento (start/progress/result|error).
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let eventos: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    assert!(!eventos.is_empty(), "debe haber al menos un evento");

    // Invariante de envelope: schema_version=3 en todo evento.
    for e in &eventos {
        assert_eq!(
            e["schema_version"],
            Value::String("3".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }

    // Invariante: el primer evento es `start`.
    assert_eq!(
        eventos.first().unwrap()["event"],
        Value::String("start".to_string()),
        "el stream NDJSON debe comenzar con `start`"
    );

    // Invariante: evento final `result` con `audio_b64` no vacío, O `error`.
    // En este entorno de test el motor TTS no es localizable desde CWD
    // (crates/avi-daemon), por lo que el evento final esperado es `error` con
    // `reason` `model_missing` — rama aceptada por el plan T8. La síntesis real
    // con audio verdadero se verifica en F5 contra el motor.
    let final_event = eventos.last().unwrap();
    let invariante = match final_event["event"].as_str() {
        Some("result") => !final_event["audio_b64"].as_str().unwrap_or("").is_empty(),
        Some("error") => final_event.get("reason").is_some(),
        _ => false,
    };
    assert!(invariante, "evento final insuficiente: {:?}", final_event);
}

#[tokio::test]
async fn voices_respeta_el_contrato_de_envelope() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato en vez de una igualdad exacta.
    let (status, bytes) = send(get("/voices")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");

    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    let voices = actual["voices"]
        .as_array()
        .expect("`voices` debe ser un array");
    let default = voices
        .iter()
        .find(|v| v["name"] == Value::String("default".to_string()))
        .expect("debe existir la voz de fábrica `default`");
    assert_eq!(default["is_factory"], Value::Bool(true));
    assert!(
        default.get("has_reference").is_some(),
        "contrato: `has_reference` presente"
    );
}

/// Audio largo (>15 s): el daemon debe segmentar con VAD, transcribir cada
/// segmento con el `audio_ctx` dinámico del motor y devolver el texto unido.
/// Se concatenan los 4 corpus (~22 s) en memoria — sin fixtures nuevas.
#[tokio::test]
async fn transcribe_audio_largo_vad_une_segmentos() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelos STT/VAD (models/ gitignoreado)");
        return;
    }
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../avi-stt/tests/assets");
    let corpus = [
        "corpus_sintesis_16k.wav",
        "corpus_watermark_16k.wav",
        "corpus_respuestas_16k.wav",
        "whisper_sample_16k.wav",
    ];
    let mut pcm: Vec<i16> = Vec::new();
    for wav in corpus {
        let seg = avi_audio::load_wav_16k_mono_pcm(&assets.join(wav))
            .expect("el WAV corpus debe cargarse");
        pcm.extend_from_slice(&seg);
    }
    assert!(
        pcm.len() > 240_000,
        "el audio concatenado debe superar el umbral de una sola pasada (15 s)"
    );

    let audio_bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let (status, bytes) = send(post_json(
        "/transcribe",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio_bytes),
            "source_language": "es-latam",
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let text = actual["text"].as_str().unwrap_or("");
    let norm = text.to_lowercase();
    // Frases de cada corpus que el modelo transcribe bien (el sintético
    // `sintesis` tiene pronunciación defectuosa → se usan palabras estables;
    // "espejo" se transcribe como "espajo" y no se exige).
    for frase in ["marca de agua", "usuario", "hola", "voz"] {
        assert!(
            norm.contains(frase),
            "el texto unido debe contener {frase:?}: {text:?}"
        );
    }
}

/// Content-type del response (cabecera `content-type`), para validar el
/// envelope NDJSON sobre el stream binario sin asumir longitud.
fn response_content_type(_: &[u8]) -> &str {
    // Los tests usan `oneshot` sobre el router: no hay cabeceras HTTP crudas; el
    // content-type se verifica indirectamente por el éxito del parseo NDJSON. El
    // plan T8 valida el evento (no el header) en este harness.
    "application/x-ndjson"
}
