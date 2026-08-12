use axum::{
    body::Body,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use crate::json_emitter;
use crate::store::{VoiceStore, SpeechStore};

/// Estado compartido del daemon
pub struct DaemonState {
    /// Lock de serialización de síntesis (una a la vez)
    pub synthesis_lock: Mutex<()>,
    pub voice_store: VoiceStore,
    pub speech_store: SpeechStore,
}

impl DaemonState {
    pub fn new() -> Self {
        Self {
            synthesis_lock: Mutex::new(()),
            voice_store: VoiceStore::new(),
            speech_store: SpeechStore::new(),
        }
    }
}

type SharedState = Arc<DaemonState>;

// ─── Handlers ────────────────────────────────────────────────────────

/// GET /health — handshake con schema_version estricto
async fn health_handler() -> Json<Value> {
    Json(json_emitter::with_schema_version(json!({
        "status": "healthy",
        "engine": "rust_native"
    })))
}

/// GET /voices — listar voces registradas
async fn voices_handler(State(state): State<SharedState>) -> impl IntoResponse {
    match state.voice_store.list() {
        Ok(voices) => {
            let voice_list: Vec<Value> = voices.iter().map(|v| {
                json!({
                    "name": v.name,
                    "is_factory": v.is_factory,
                    "has_reference": v.reference_path.is_some(),
                })
            }).collect();
            Json(json_emitter::with_schema_version(json!({
                "voices": voice_list,
            }))).into_response()
        }
        Err(e) => {
            (StatusCode::INTERNAL_SERVER_ERROR, Json(json_emitter::with_schema_version(json!({
                "error": e.to_string(),
            })))).into_response()
        }
    }
}

/// POST /voices/precompute — precomputar conditionals de una voz
async fn voices_precompute_handler(
    State(_state): State<SharedState>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let voice = payload.get("voice").and_then(|v| v.as_str()).unwrap_or("default");
    // Cuando el motor TTS esté integrado, aquí se precomputan los conditionals
    Json(json_emitter::with_schema_version(json!({
        "status": "precompute_pending",
        "voice": voice,
        "message": "La precomputación de conditionals requiere el motor TTS nativo.",
    })))
}

/// POST /synthesize — síntesis con streaming NDJSON de progreso
async fn synthesize_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    // Adquirir lock de serialización (una síntesis a la vez)
    let _lock = state.synthesis_lock.lock().await;

    let text = payload.get("text").and_then(|v| v.as_str()).unwrap_or("");
    let voice = payload.get("voice").and_then(|v| v.as_str()).unwrap_or("default");

    if text.is_empty() {
        return Json(json_emitter::with_schema_version(json!({
            "error": "empty_text",
            "message": "El texto a sintetizar está vacío.",
        }))).into_response();
    }

    // Streaming NDJSON: progreso línea por línea
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);

    // Emitir eventos de progreso en un task separado
    let text_owned = text.to_string();
    let voice_owned = voice.to_string();
    tokio::spawn(async move {
        // Evento: inicio
        let _ = tx.send(serde_json::to_string(&json_emitter::with_schema_version(json!({
            "event": "start",
            "voice": voice_owned,
            "text_length": text_owned.len(),
        }))).unwrap_or_default()).await;

        // Evento: progreso (el motor TTS emitirá eventos reales cuando esté integrado)
        let _ = tx.send(serde_json::to_string(&json_emitter::with_schema_version(json!({
            "event": "progress",
            "stage": "synthesis",
            "percent": 0,
            "message": "Motor TTS no provisionado.",
        }))).unwrap_or_default()).await;

        // Evento: completado (o error)
        let _ = tx.send(serde_json::to_string(&json_emitter::with_schema_version(json!({
            "event": "error",
            "reason": "model_missing",
            "message": "El modelo de síntesis TTS no está provisionado.",
        }))).unwrap_or_default()).await;
    });

    // Convertir el receptor en un stream NDJSON
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = Body::from_stream(
        tokio_stream::StreamExt::map(stream, |line| {
            Ok::<_, std::convert::Infallible>(format!("{}\n", line))
        })
    );

    Response::builder()
        .header("content-type", "application/x-ndjson")
        .header("x-schema-version", json_emitter::SCHEMA_VERSION)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// POST /transcribe — transcripción de audio (PCM int16 base64)
async fn transcribe_handler(
    State(_state): State<SharedState>,
    Json(payload): Json<Value>,
) -> impl IntoResponse {
    let _audio_b64 = payload.get("audio_pcm_base64").and_then(|v| v.as_str());

    // Cuando el motor STT esté integrado, aquí se decodifica el PCM y se transcribe
    Json(json_emitter::with_schema_version(json!({
        "status": "transcription_pending",
        "message": "El motor STT nativo no está provisionado.",
    })))
}

/// POST /shutdown — apagar el daemon gracefully
async fn shutdown_handler() -> impl IntoResponse {
    // Programar el shutdown en un task separado (da tiempo a enviar la respuesta)
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    Json(json_emitter::with_schema_version(json!({
        "status": "shutting_down",
    })))
}

// ─── Servidor ────────────────────────────────────────────────────────

pub async fn run_daemon_server(addr: SocketAddr) -> anyhow::Result<()> {
    let state = Arc::new(DaemonState::new());

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/voices", get(voices_handler))
        .route("/voices/precompute", post(voices_precompute_handler))
        .route("/synthesize", post(synthesize_handler))
        .route("/transcribe", post(transcribe_handler))
        .route("/shutdown", post(shutdown_handler))
        .with_state(state);

    let listener = TcpListener::bind(addr).await?;
    println!("Daemon nativo escuchando en http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
