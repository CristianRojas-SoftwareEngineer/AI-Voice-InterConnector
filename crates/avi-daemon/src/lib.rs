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
// `hilos_disponibles` y el trait `SttEngine` (`.transcribe`) solo los consume
// la superficie STT, gateada tras `native-stt`.
#[cfg(feature = "native-stt")]
use avi_core::engine::{hilos_disponibles, SttEngine};
use avi_core::json_emitter;
use avi_store::{SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::{detectar_idioma, ParakeetEngine};
use avi_tts::{GenerationOptions, Qwen3TtsEngine, TtsEngine, VoiceProfile};
// `Engine` trait requerido por el API no-deprecation de base64 0.22
// (el motor interno del daemon usa el alfabeto STANDARD, idéntico al `encode`/`decode`
// libres, por compatibilidad con el cliente raíz del CLI).
use base64::Engine;

/// Ruta relativa (al cwd del workspace) del modelo Parakeet STT (export int8).
#[cfg(feature = "native-stt")]
const STT_MODEL_DIR: &str = "models/parakeet-tdt-v3";
/// Idioma por defecto para `clone_voice` cuando la petición no lo transporta
/// (el contrato de /voices/precompute no carriba idioma).
const DEFAULT_CLONE_LANGUAGE: &str = "es";

/// Estado compartido del daemon
pub struct DaemonState {
    /// Lock de serialización de síntesis (una a la vez)
    pub synthesis_lock: Mutex<()>,
    pub voice_store: VoiceStore,
    pub speech_store: SpeechStore,
    /// Motor TTS nativo (Qwen3-TTS), con ciclo de vida persistente entre peticiones.
    pub tts_engine: Qwen3TtsEngine,
    /// Motor STT nativo (Parakeet TDT v3 int8 vía ort). Parakeet no necesita
    /// chunking VAD: su RTF (~0.11 en audio largo) es lineal y no degrada con
    /// la duración, por lo que `transcribe_handler` transcribe de una sola vez.
    #[cfg(feature = "native-stt")]
    pub stt_engine: ParakeetEngine,
}

impl DaemonState {
    /// Constructor de producción. Las rutas de modelo son relativas al cwd del
    /// workspace, correcto cuando `daemon serve` se lanza desde la raíz del repo.
    /// Devuelve error si el motor STT no puede inicializarse (modelo inexistente).
    pub fn new() -> anyhow::Result<Self> {
        #[cfg(feature = "native-stt")]
        let stt_engine = ParakeetEngine::new(STT_MODEL_DIR)
            .map_err(|e| anyhow::anyhow!("fallo al cargar el modelo STT {}: {e}", STT_MODEL_DIR))?;
        // Los hilos lógicos del equipo del usuario dimensionan el paralelismo de
        // ONNX Runtime (heredado de `avi-stt::parakeet`); el runtime del daemon
        // serializa síntesis y STT fuera de esta construcción.
        #[cfg(feature = "native-stt")]
        let _ = hilos_disponibles();
        Ok(Self {
            synthesis_lock: Mutex::new(()),
            voice_store: VoiceStore::new(),
            speech_store: SpeechStore::new(),
            tts_engine: Qwen3TtsEngine::new(None),
            #[cfg(feature = "native-stt")]
            stt_engine,
        })
    }
}

type SharedState = Arc<DaemonState>;

// ─── Helpers internos ───────────────────────────────────────────────────

/// Inserta `schema_version` en un `Value`, reutilizado por handlers que devuelven
/// JSON directamente (coherencia con `emit_raw_json`).
fn with_sv(val: Value) -> Value {
    json_emitter::with_schema_version(val)
}

/// Serializa un evento NDJSON con envelope de schema_version al canal de salida.
async fn emit_ndjson(tx: &tokio::sync::mpsc::Sender<String>, event: Value) {
    let _ = tx
        .send(serde_json::to_string(&with_sv(event)).unwrap_or_default())
        .await;
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// GET /health — handshake con schema_version estricto
async fn health_handler() -> Json<Value> {
    Json(with_sv(json!({
        "status": "healthy",
        "engine": "rust_native"
    })))
}

/// GET /voices — listar voces registradas
async fn voices_handler(State(state): State<SharedState>) -> impl IntoResponse {
    match state.voice_store.list() {
        Ok(voices) => {
            let voice_list: Vec<Value> = voices
                .iter()
                .map(|v| {
                    json!({
                        "name": v.name,
                        "is_factory": v.is_factory,
                        "has_reference": v.reference_path.is_some(),
                    })
                })
                .collect();
            Json(with_sv(json!({ "voices": voice_list }))).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(with_sv(json!({ "error": e.to_string() }))),
        )
            .into_response(),
    }
}

/// POST /voices/precompute — precomputar conditionals de una voz clonada
///
/// Caso significativo: una voz clonada con `reference.wav` (legado, aún sin
/// `.qvoice`) se vuelve a clonar vía el motor real (`avi_tts::clone_voice`,
/// precedente en `src/main.rs`). Una voz ya con `reference.qvoice` ya está
/// precomputada; una voz de fábrica (preset/default, sin referencia) no tiene
/// conditionals que precomputar → "no aplica".
async fn voices_precompute_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let voice = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    match state.voice_store.find_reference(&voice) {
        // Sin referencia: voz de fábrica (preset/default). No hay conditionals.
        None => Json(with_sv(json!({
            "name": voice,
            "precomputed": false,
            "message": "La precomputación de conditionals no aplica a voces de fábrica.",
        })))
        .into_response(),
        // `reference.qvoice` ya existe → ya precomputado; no hay WAV fuente con
        // el que relanzar `clone_voice` (no se regenera de un .qvoice).
        Some(path) if path.extension().is_some_and(|e| e == "qvoice") => Json(with_sv(json!({
            "name": voice,
            "precomputed": true,
            "message": "Voz ya precomputada (reference.qvoice presente).",
        })))
        .into_response(),
        // `reference.wav` legado: relanzar clone_voice contra el motor real.
        Some(wav) => {
            let base_model_dir = match state.tts_engine.base_model_dir.as_ref() {
                Some(d) => d.clone(),
                None => {
                    return (
                        StatusCode::NOT_FOUND,
                        Json(with_sv(json!({
                            "name": voice,
                            "precomputed": false,
                            "reason": "model_missing",
                            "message": "El modelo base TTS no está provisionado.",
                        }))),
                    )
                        .into_response();
                }
            };
            let qvoice_out = wav.with_file_name("reference.qvoice");
            match avi_tts::clone_voice(
                &base_model_dir,
                &wav,
                &qvoice_out,
                &voice,
                DEFAULT_CLONE_LANGUAGE,
            ) {
                Ok(()) => Json(with_sv(json!({
                    "name": voice,
                    "precomputed": true,
                })))
                .into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(with_sv(json!({
                        "name": voice,
                        "precomputed": false,
                        "reason": "precompute_failed",
                        "message": e.to_string(),
                    }))),
                )
                    .into_response(),
            }
        }
    }
}

/// POST /synthesize — síntesis con streaming NDJSON de progreso
///
/// Contrato NDJSON (fuente de verdad: `protocol.py`): `start` → `progress`(N)
/// → `result`{`audio_b64`,`t3_time`,`s3gen_time`} OR `error`{`reason`,`message`}.
/// El motor `TtsEngine` no expone callback de progreso, por lo que se emite un
/// único marcador `progress` genérico antes de sintetizar.
async fn synthesize_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let voice = payload
        .get("voice")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();

    // Validación de texto vacío: se evalúa antes del motor y devuelve un cuerpo
    // JSON plano (no stream), para que el test de contrato de texto vacío siga
    // pasando sin modificación.
    if text.is_empty() {
        return Json(with_sv(json!({
            "error": "empty_text",
            "message": "El texto a sintetizar está vacío.",
        })))
        .into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    let state = state.clone();
    let text_owned = text.clone();
    let voice_owned = voice.clone();

    tokio::spawn(async move {
        // T4: el lock envuelve TODO el trabajo de síntesis —incluido dentro del
        // spawn—, serializando síntesis concurrentes. No se añade semáforo de
        // admisión (fuera de alcance de esta rutina).
        let _lock = state.synthesis_lock.lock().await;

        emit_ndjson(
            &tx,
            json!({
                "event": "start",
                "voice": voice_owned,
                "text_length": text_owned.len(),
            }),
        )
        .await;

        // Provisionamiento del motor: si binario/modelo no se resolvieron, la rama
        // `model_missing` es el contrato aceptado por el plan T8 en entornos sin
        // motor (en el daemon real corre desde la raíz del repo, donde sí resuelve).
        if state.tts_engine.binary_path.is_none() || state.tts_engine.model_dir.is_none() {
            emit_ndjson(
                &tx,
                json!({
                    "event": "error",
                    "reason": "model_missing",
                    "message": "El modelo de síntesis TTS no está provisionado.",
                }),
            )
            .await;
            return;
        }

        // Marcador genérico de fase (el motor no reporta progreso interno).
        emit_ndjson(
            &tx,
            json!({
                "event": "progress",
                "stage": "synthesis",
                "percent": 0,
                "message": "Síntesis en curso.",
            }),
        )
        .await;

        // Perfil de voz: .qvoice si la voz está clonada; el motor resuelve el
        // preset vía `resolve_voice_motor` a partir del nombre.
        let profile = VoiceProfile {
            name: voice_owned.clone(),
            reference_audio: None,
            qvoice_path: state.voice_store.find_reference(&voice_owned),
        };
        // `GenerationOptions::produccion()` fija temperature=0.35 / seed=42; no se
        // alteran temperatura ni seed (prohibido por el brief).
        let tmp = std::env::temp_dir().join(format!("avi_daemon_synth_{}.wav", std::process::id()));
        match state.tts_engine.synthesize_with_options(
            &text_owned,
            &profile,
            &GenerationOptions::produccion(),
            Some(&tmp),
        ) {
            Ok(path) => {
                match std::fs::read(&path) {
                    Ok(wav_bytes) => {
                        emit_ndjson(
                            &tx,
                            json!({
                                "event": "result",
                                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&wav_bytes),
                                // El motor no expone tiempos; por contrato el campo
                                // existe y se reporta como 0.0 (verdad en F5).
                                "t3_time": 0.0,
                                "s3gen_time": 0.0,
                            }),
                        )
                        .await;
                    }
                    Err(e) => {
                        emit_ndjson(
                            &tx,
                            json!({
                                "event": "error",
                                "reason": "io_error",
                                "message": format!("Error leyendo el WAV de síntesis: {}", e),
                            }),
                        )
                        .await;
                    }
                }
                let _ = std::fs::remove_file(&path);
            }
            Err(e) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "synthesis_failed",
                        "message": e.to_string(),
                    }),
                )
                .await;
            }
        }
    });

    // Convertir el receptor en un stream NDJSON.
    let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
    let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |line| {
        Ok::<_, std::convert::Infallible>(format!("{}\n", line))
    }));

    Response::builder()
        .header("content-type", "application/x-ndjson")
        .header("x-schema-version", json_emitter::SCHEMA_VERSION)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// POST /transcribe — transcripción de audio (PCM int16 base64)
///
/// Contrato (fuente de verdad: `protocol.py`): el campo es `audio_b64` (no
/// `audio_pcm_base64`); el audio es PCM i16 little-endian 16 kHz mono; la
/// respuesta exitosa es `TranscribeResponse{text}`.
#[cfg(feature = "native-stt")]
async fn transcribe_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let audio_b64 = payload.get("audio_b64").and_then(|v| v.as_str());

    let audio_b64 = match audio_b64 {
        Some(s) => s,
        None => {
            return Json(with_sv(json!({
                "status": "error",
                "reason": "audio_missing",
                "message": "La petición no incluye el campo 'audio_b64' (PCM int16 little-endian 16 kHz mono).",
            })))
            .into_response();
        }
    };

    let audio_bytes = match base64::engine::general_purpose::STANDARD.decode(audio_b64) {
        Ok(b) => b,
        Err(e) => {
            return Json(with_sv(json!({
                "status": "error",
                "reason": "audio_decode_error",
                "message": format!("audio_b64 no decodificable como base64: {}", e),
            })))
            .into_response();
        }
    };

    // PCM i16 little-endian → Vec<i16> mono 16 kHz (el motor normaliza a i16::MAX).
    let pcm: Vec<i16> = audio_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();

    let source_language = payload
        .get("source_language")
        .and_then(|v| v.as_str())
        .unwrap_or("es-latam");
    let language = resolve_stt_language(source_language);

    // Parakeet no necesita chunking VAD (no degrada con la duración de audio);
    // se transcribe de una sola pasada.
    let result = state.stt_engine.transcribe(&pcm, Some(language));
    match result {
        Ok(text) => {
            // Guardia de idioma: si el detector heurístico marca inglés
            // sospechoso en una sesión en español, se anexa el campo aditivo
            // `language_warning` al JSON de respuesta. El campo es opcional y
            // aditivo: clientes que lo ignoren no se ven afectados.
            let (idioma, _ratio) = detectar_idioma(&text);
            let body = if idioma == "EN-SOSPECHOSO" {
                with_sv(json!({ "text": text, "language_warning": true }))
            } else {
                with_sv(json!({ "text": text }))
            };
            Json(body).into_response()
        }
        Err(e) => {
            let body = with_sv(json!({
                "status": "error",
                "reason": "transcription_failed",
                "message": e.to_string(),
            }));
            (StatusCode::INTERNAL_SERVER_ERROR, Json(body)).into_response()
        }
    }
}

/// Mapea el token de idioma del cliente (`es-latam`/`en`) al código ISO que
/// exige Parakeet (paridad con `resolve_stt_language` en `src/main.rs`).
#[cfg(feature = "native-stt")]
fn resolve_stt_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
}

/// POST /shutdown — apagar el daemon gracefully
async fn shutdown_handler() -> impl IntoResponse {
    // Programar el shutdown en un task separado (da tiempo a enviar la respuesta)
    tokio::spawn(async {
        tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
        std::process::exit(0);
    });

    Json(with_sv(json!({ "status": "shutting_down" })))
}

// ─── Servidor ────────────────────────────────────────────────────────────

/// Construye el `Router` de Axum a partir de un `Arc<DaemonState>` ya construido
/// externamente. Extraído de `build_router()` para testeabilidad (T8): permite
/// ejercer las rutas en tests de integración inyectando un estado con rutas de
/// modelo apuntando a `CARGO_MANIFEST_DIR`.
pub fn build_router_with_state(state: Arc<DaemonState>) -> Router {
    let router = Router::new()
        .route("/health", get(health_handler))
        .route("/voices", get(voices_handler))
        .route("/voices/precompute", post(voices_precompute_handler))
        .route("/synthesize", post(synthesize_handler))
        .route("/shutdown", post(shutdown_handler));
    // `/transcribe` solo existe con el motor STT compilado (`native-stt`); sin el
    // feature el daemon expone el resto de rutas sin ONNX Runtime.
    #[cfg(feature = "native-stt")]
    let router = router.route("/transcribe", post(transcribe_handler));
    router.with_state(state)
}

/// Punto de entrada de producción. Construye el estado con las rutas de modelo de
/// producción (relativas al cwd del workspace) y delega a `build_router_with_state`.
pub fn build_router() -> Router {
    let state = Arc::new(DaemonState::new().expect("fallo al inicializar los motores del daemon"));
    build_router_with_state(state)
}

/// Warmup del motor TTS de arranque (Tarea 3).
///
/// Precarga la voz `default`→preset `ryan` en el motor residente, para que el
/// daemon ya tenga el modelo caliente antes de aceptar peticiones.
///
/// Riesgo heredado (R2): el residente enlaza en `INADDR_ANY`
/// (`avi-tts/src/lib.rs:746-750`); el warmup lo mantiene vivo, extendiendo esa
/// superficie de red. Documentado, NO corregido (fuera de alcance).
///
/// Limitación estructural: solo la voz `default` queda precargada; el resto paga
/// el cold-start de reemplazo de residente en su primera síntesis.
pub fn warmup_tts(state: &DaemonState) -> anyhow::Result<()> {
    let profile = VoiceProfile {
        name: "default".to_string(),
        reference_audio: None,
        qvoice_path: state.voice_store.find_reference("default"),
    };
    let tmp = std::env::temp_dir().join(format!("avi_daemon_warmup_{}.wav", std::process::id()));
    state
        .tts_engine
        .synthesize_with_options(
            "Calentamiento del daemon.",
            &profile,
            &GenerationOptions::produccion(),
            Some(&tmp),
        )
        .map_err(|e| anyhow::anyhow!("Warmup TTS falló; abortando arranque del daemon: {}", e))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

/// Inicia el daemon nativo escuchando en `addr`. Construye el estado (propagando
/// errores de inicialización de motores), ejecuta el warmup TTS, y luego enlaza
/// el listener y sirve. El warmup ocurre ANTES del bind para que el modelo ya
/// esté caliente cuando se empiecen a aceptar peticiones.
pub async fn run_daemon_server(addr: SocketAddr) -> anyhow::Result<()> {
    let state = Arc::new(DaemonState::new()?);

    // T3: warmup TTS antes del bind.
    warmup_tts(&state)?;

    let app = build_router_with_state(state);
    let listener = TcpListener::bind(addr).await?;
    println!("Daemon nativo escuchando en http://{}", addr);
    axum::serve(listener, app).await?;
    Ok(())
}
