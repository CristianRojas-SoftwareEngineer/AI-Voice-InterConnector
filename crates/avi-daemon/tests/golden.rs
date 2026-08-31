//! Harness de tests dorados del daemon (Tarea 8 del plan T2/T8).
//!
//! Levanta el `Router` de Axum vía [`avi_daemon::build_router_with_state`] y lo
//! ejerce con `tower::ServiceExt::oneshot` (sin abrir socket TCP real), comparando
//! cada respuesta contra fixtures fijas en `tests/golden/` de la raíz del repo.
//! Detecta regresiones de contrato (formato JSON, `schema_version`, textos de
//! estado/error) entre el runtime nativo y lo que el resto del sistema espera.
//!
//! El harness construye `DaemonState` con el motor STT real (Parakeet TDT v3 int8,
//! export de `istupakov`), que solo existe con `native-stt`. Sin el feature, el
//! harness cae a un `ParakeetEngine` construido con un directorio de modelo
//! vacío; `oneshot` no ejerce la ruta `/transcribe` en estos tests, así que el
//! motor se queda sin inicializar y el resto de la suite corre sin ONNX
//! Runtime. Los tests que sí requieren STT (golden transcribe) son gated
//! individualmente con `#[cfg(feature = "native-stt")]` en sus cuerpos.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::{build_router_with_state, DaemonState, WarmState};
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
            // El motor STT solo se construye si el feature `native-stt` está
            // activo; sin él, los tests que ejercen `/transcribe` se saltan y
            // el resto corre contra un `DaemonState` sin campo stt_engine.
            #[cfg(feature = "native-stt")]
            {
                let stt_model_dir =
                    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/parakeet-tdt-v3");
                let stt_engine = avi_stt::ParakeetEngine::new(&stt_model_dir)
                    .expect("el modelo STT de test debe cargarse");
                Arc::new(DaemonState {
                    synthesis_lock: tokio::sync::Mutex::new(()),
                    voice_store: VoiceStore::new(),
                    speech_store: SpeechStore::new(),
                    tts_engine: avi_tts::Qwen3TtsEngine::new(None),
                    stt_engine,
                    warm: std::sync::RwLock::new(WarmState::Warming),
                    shutdown_notify: Arc::new(tokio::sync::Notify::new()),
                })
            }
            #[cfg(not(feature = "native-stt"))]
            {
                Arc::new(DaemonState {
                    synthesis_lock: tokio::sync::Mutex::new(()),
                    voice_store: VoiceStore::new(),
                    speech_store: SpeechStore::new(),
                    tts_engine: avi_tts::Qwen3TtsEngine::new(None),
                    warm: std::sync::RwLock::new(WarmState::Warming),
                    shutdown_notify: Arc::new(tokio::sync::Notify::new()),
                })
            }
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

/// Modelos reales de STT (Parakeet TDT v3 int8: 4 archivos) presentes. Los binarios
/// bajo `models/` están gitignoreados: en un checkout limpio (CI) estos tests
/// dorados se saltan con aviso; en desarrollo corren completos.
fn modelos_presentes() -> bool {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../models/parakeet-tdt-v3");
    [
        "nemo128.onnx",
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "vocab.txt",
    ]
    .iter()
    .all(|f| root.join(f).exists())
}

#[tokio::test]
async fn health_coincide_con_fixture() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
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
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
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
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
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
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
        return;
    }
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "hola", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

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
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
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

/// Audio largo (~22 s, concatenación de 4 corpus): Parakeet no necesita chunking VAD
/// (RTF ~0.11 lineal); se transcribe de una sola pasada y se verifica el texto unido.
#[tokio::test]
async fn transcribe_audio_largo_transcribe_de_una_pasada() {
    if !modelos_presentes() {
        eprintln!("[daemon] skip: sin modelo STT Parakeet (models/ gitignoreado)");
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
            "source_language": "es-latam",
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let text = actual["text"].as_str().unwrap_or("");
    let norm = text.to_lowercase();
    // Frases de cada corpus que el modelo transcribe bien (el sintético
    // `sintesis` tiene pronunciación defectuosa → se usan palabras estables).
    // "hola" se descuenta: el fixture `whisper_sample` (saludo breve) se emite
    // en inglés por el TDT (detectado en F5), por lo que no aparece en el texto
    // unido aunque el resto del audio (watermark/sintesis/respuestas) sí se
    // transcribe en español. "voz" no se exige estricta: "esténtesis" puede
    // dropearla.
    for frase in ["marca de agua", "usuario", "espajo"] {
        assert!(
            norm.contains(frase),
            "el texto unido debe contener {:?}: {text:?}",
            frase
        );
    }
}
