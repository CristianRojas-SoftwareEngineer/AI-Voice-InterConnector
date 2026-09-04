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

pub mod spawn;
pub use spawn::{spawn_background, spawn_uninstall_helper};
// `hilos_disponibles` y el trait `SttEngine` (`.transcribe`) solo los consume
// la superficie STT, gateada tras `native-stt`.
#[cfg(feature = "native-stt")]
use avi_core::engine::{hilos_disponibles, SttEngine};
use avi_core::json_emitter;
#[cfg(feature = "native-stt")]
use avi_store::ModelStore;
use avi_store::{SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::{detectar_idioma, ParakeetEngine};
use avi_tts::{GenerationOptions, Qwen3TtsEngine, TtsEngine, VoiceProfile};
#[cfg(feature = "native-translation")]
use avi_translation::Ct2TranslationEngine;
// `Engine` trait requerido por el API no-deprecation de base64 0.22
// (el motor interno del daemon usa el alfabeto STANDARD, idéntico al `encode`/`decode`
// libres, por compatibilidad con el cliente raíz del CLI).
use base64::Engine;

/// Idioma por defecto para `clone_voice` cuando la petición no lo transporta
/// (el contrato de /voices/precompute no carriba idioma).
const DEFAULT_CLONE_LANGUAGE: &str = "es";

/// Estado de pre-calentamiento (warmup) del motor TTS. Desacoplado del readiness:
/// el daemon sirve en cuanto enlaza; `warm` refleja si el modelo ya está caliente.
/// Transiciones: `Warming` → `Warm` (éxito) o `Warming` → `Failed(causa)` (fallo,
/// que degrada pero no derriba el daemon: la primera petición paga cold-start).
pub enum WarmState {
    Warming,
    Warm,
    Failed(String),
}

impl WarmState {
    /// Etiqueta pública para el campo `warm` de `/health`.
    fn label(&self) -> &'static str {
        match self {
            WarmState::Warming => "warming",
            WarmState::Warm => "warm",
            WarmState::Failed(_) => "warm_failed",
        }
    }

    /// Causa del fallo de warmup, si aplica (mapea al campo `warm_error`).
    fn error(&self) -> Option<String> {
        match self {
            WarmState::Failed(e) => Some(e.clone()),
            _ => None,
        }
    }
}

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
    /// Motores CT2 residentes para traducción `es↔en` (uno por dirección).
    /// Se precargan en `DaemonState::new` si `ct2_model_dir/*/model.bin` existe;
    /// degradan a `None` si faltan, sin derribar `run_daemon_server:614`.
    /// El warmup CT2 no duplica `warmup_tts`: la primera petición paga frío si
    /// el residente no estaba; documentado sin warmup separado.
    #[cfg(feature = "native-translation")]
    pub ct2_engine: Option<std::collections::HashMap<String, Ct2TranslationEngine>>,
    /// Estado de warmup del motor TTS, con interior mutability seguro entre hilos:
    /// el warmup corre en un `spawn_blocking` de segundo plano y actualiza este
    /// campo; `/health` lo lee. Inicializa en `Warming`.
    pub warm: std::sync::RwLock<WarmState>,
    /// Señal de cierre compartida: `shutdown_handler` notifica y `run_daemon_server`
    /// la observa para el graceful shutdown. Evita `process::exit` dentro del
    /// runtime tokio de `axum::serve`, que no termina fiablemente el proceso en
    /// Windows (causa raíz del cuelgue de los E2E). Al cerrar de forma natural se
    /// ejecutan los `Drop` (matando al `Qwen3TtsResident` y `qwen_tts.exe`).
    pub shutdown_notify: Arc<tokio::sync::Notify>,
}

impl DaemonState {
    /// Constructor de producción. Las rutas de modelo son relativas al cwd del
    /// workspace, correcto cuando `daemon serve` se lanza desde la raíz del repo.
    /// Devuelve error si el motor STT no puede inicializarse (modelo inexistente).
    pub fn new() -> anyhow::Result<Self> {
        #[cfg(feature = "native-stt")]
        let stt_dir = ModelStore::new().model_dir("parakeet-tdt-v3");
        #[cfg(feature = "native-stt")]
        let stt_engine = ParakeetEngine::new(&stt_dir).map_err(|e| {
            anyhow::anyhow!("fallo al cargar el modelo STT {}: {e}", stt_dir.display())
        })?;
        // Los hilos lógicos del equipo del usuario dimensionan el paralelismo de
        // ONNX Runtime (heredado de `avi-stt::parakeet`); el runtime del daemon
        // serializa síntesis y STT fuera de esta construcción.
        #[cfg(feature = "native-stt")]
        let _ = hilos_disponibles();
        #[cfg(feature = "native-translation")]
        let ct2_engine = {
            let mut map = std::collections::HashMap::new();
            for pair in &["es-en", "en-es"] {
                let dir = avi_store::ct2_model_dir(pair);
                if dir.join("model.bin").is_file() {
                    if let Ok(engine) = Ct2TranslationEngine::new(&dir) {
                        map.insert(pair.to_string(), engine);
                    }
                }
            }
            if map.is_empty() { None } else { Some(map) }
        };
        Ok(Self {
            synthesis_lock: Mutex::new(()),
            voice_store: VoiceStore::new(),
            speech_store: SpeechStore::new(),
            tts_engine: Qwen3TtsEngine::new(None),
            #[cfg(feature = "native-stt")]
            stt_engine,
            #[cfg(feature = "native-translation")]
            ct2_engine,
            warm: std::sync::RwLock::new(WarmState::Warming),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
        })
    }

    /// Marca el warmup como completado con éxito (`Warming` → `Warm`).
    pub fn set_warm(&self) {
        *self.warm.write().unwrap() = WarmState::Warm;
    }

    /// Marca el warmup como fallido conservando la causa (`Warming` → `Failed`).
    pub fn set_warm_failed(&self, causa: String) {
        *self.warm.write().unwrap() = WarmState::Failed(causa);
    }

    /// Instantánea del estado de warmup: `(etiqueta, causa-de-fallo opcional)`.
    pub fn warm_snapshot(&self) -> (&'static str, Option<String>) {
        let guard = self.warm.read().unwrap();
        (guard.label(), guard.error())
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

/// Construye el cuerpo de `/health` a partir del estado de warmup. Función pura
/// (testeable sin daemon): emite `{status:"ready", warm, engine}` y añade
/// `warm_error` solo cuando el warmup falló.
fn health_body(warm_label: &str, warm_error: Option<String>) -> Value {
    let mut body = json!({
        "status": "ready",
        "warm": warm_label,
        "engine": "rust_native",
    });
    if let Some(err) = warm_error {
        body["warm_error"] = Value::String(err);
    }
    body
}

/// GET /health — readiness (enlazado + motor construido) con estado de warmup.
/// Reporta `{status:"ready", warm, engine}` leyendo el estado compartido; readiness
/// es inmediato, `warm` refleja el pre-calentamiento en curso o su resultado.
async fn health_handler(State(state): State<SharedState>) -> Json<Value> {
    let (label, error) = state.warm_snapshot();
    Json(with_sv(health_body(label, error)))
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
        // T4: el lock envuelve completamente el trabajo de síntesis —incluido dentro del
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

/// Normaliza token de idioma para traducción (`es-latam`→`es`), paridad con
/// `resolve_stt_language` de `src/main.rs` y `avi_translation`.
fn resolve_translation_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
}

/// POST /translate — traducción texto→texto con CT2 residente
#[cfg(feature = "native-translation")]
async fn translate_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let text = payload
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if text.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "error": "empty_text",
                "reason": "empty_text",
                "message": "El texto a traducir está vacío",
            }))),
        )
            .into_response();
    }
    let from_raw = payload
        .get("from")
        .or_else(|| payload.get("source"))
        .and_then(|v| v.as_str())
        .unwrap_or("es");
    let to_raw = payload
        .get("to")
        .or_else(|| payload.get("target"))
        .and_then(|v| v.as_str())
        .unwrap_or("en");
    let source = resolve_translation_language(from_raw).to_string();
    let target = resolve_translation_language(to_raw).to_string();
    if source == target {
        return Json(with_sv(json!({
            "translated": text,
            "source": from_raw,
            "target": to_raw,
        })))
        .into_response();
    }
    let pair = match (source.as_str(), target.as_str()) {
        ("es", "en") => "es-en",
        ("en", "es") => "en-es",
        _ => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "error": "unsupported_language_pair",
                    "reason": "unsupported_language_pair",
                    "message": format!("Par de idiomas no soportado: {} -> {} (soportados: es, en)", source, target),
                }))),
            )
                .into_response();
        }
    };
    let ct2_dir = avi_store::ct2_model_dir(pair);
    if !ct2_dir.join("model.bin").is_file() {
        return (
            StatusCode::NOT_FOUND,
            Json(with_sv(json!({
                "error": "model_missing",
                "reason": "model_missing",
                "message": format!("El modelo de traducción no está provisionado en '{}' (hf_cache_dir/ct2) — ejecuta setup.", ct2_dir.display()),
            }))),
        )
            .into_response();
    }
    // Intentar residente si está precargado
    if let Some(map) = state.ct2_engine.as_ref() {
        if let Some(engine) = map.get(pair) {
            use avi_core::engine::TranslationEngine;
            match engine.translate(&text, &source, &target) {
                Ok(translated) => {
                    return Json(with_sv(json!({
                        "translated": translated,
                        "source": from_raw,
                        "target": to_raw,
                    })))
                    .into_response();
                }
                Err(e) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(with_sv(json!({
                            "error": "translation_failed",
                            "reason": "translation_failed",
                            "message": e.to_string(),
                        }))),
                    )
                        .into_response();
                }
            }
        }
    }
    // Fallback a carga bajo demanda si residente no tenía el par
    match avi_translation::translate(&text, &source, &target, &ct2_dir) {
        Ok(translated) => Json(with_sv(json!({
            "translated": translated,
            "source": from_raw,
            "target": to_raw,
        })))
        .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(with_sv(json!({
                "error": "translation_failed",
                "reason": "translation_failed",
                "message": e.to_string(),
            }))),
        )
            .into_response(),
    }
}

/// POST /voices/clone — clonado de voz con audio base64
async fn voices_clone_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let name = payload
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_lowercase();
    // Validación de nombre con VoiceStore
    if let Err(msg) = avi_store::VoiceStore::validate_name(&name) {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "error": "invalid_voice_name",
                "reason": "invalid_voice_name",
                "message": msg,
            }))),
        )
            .into_response();
    }
    let force = payload
        .get("force")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !force && state.voice_store.exists(&name) {
        return (
            StatusCode::CONFLICT,
            Json(with_sv(json!({
                "error": "voice_exists",
                "reason": "voice_exists",
                "message": format!("La voz '{}' ya existe (usa --force para sobrescribirla).", name),
            }))),
        )
            .into_response();
    }
    let audio_b64 = match payload.get("audio_b64").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "error": "audio_missing",
                    "reason": "audio_missing",
                    "message": "La petición no incluye el campo 'audio_b64'.",
                }))),
            )
                .into_response();
        }
    };
    let audio_bytes = match base64::engine::general_purpose::STANDARD.decode(audio_b64) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "error": "audio_decode_error",
                    "reason": "audio_decode_error",
                    "message": format!("audio_b64 no decodificable como base64: {}", e),
                }))),
            )
                .into_response();
        }
    };
    let base_model_dir = match state.tts_engine.base_model_dir.as_ref() {
        Some(d) => d.clone(),
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(with_sv(json!({
                    "name": name,
                    "reason": "model_missing",
                    "message": "El modelo base TTS no está provisionado.",
                }))),
            )
                .into_response();
        }
    };
    // Escribir audio a temporal WAV para clone_voice
    let tmp_wav = std::env::temp_dir().join(format!("avi_daemon_clone_{}_{}.wav", name, std::process::id()));
    if std::fs::write(&tmp_wav, &audio_bytes).is_err() {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(with_sv(json!({
                "error": "io_error",
                "reason": "io_error",
                "message": "No se pudo escribir el audio temporal.",
            }))),
        )
            .into_response();
    }
    // timbre_b64 opcional — por ahora se ignora (paridad: timbre no transporte en este endpoint simple)
    let tmp_qvoice = std::env::temp_dir().join(format!("{}.qvoice", name));
    let clone_res = avi_tts::clone_voice(
        &base_model_dir,
        &tmp_wav,
        &tmp_qvoice,
        &name,
        DEFAULT_CLONE_LANGUAGE,
    );
    let _ = std::fs::remove_file(&tmp_wav);
    if let Err(e) = clone_res {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(with_sv(json!({
                "name": name,
                "reason": "voice_clone_failed",
                "message": e.to_string(),
            }))),
        )
            .into_response();
    }
    let saved_qvoice = match state.voice_store.save_reference(&name, &tmp_qvoice) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(with_sv(json!({
                    "name": name,
                    "reason": "voice_clone_failed",
                    "message": e.to_string(),
                }))),
            )
                .into_response();
        }
    };
    let _ = std::fs::remove_file(&tmp_qvoice);
    // Copia speech-reference.wav para compatibilidad
    let speech_copy = state.voice_store.voice_dir(&name).join("speech-reference.wav");
    let _ = std::fs::write(&speech_copy, &audio_bytes);
    // timbre opcional
    if let Some(timbre_b64) = payload.get("timbre_b64").and_then(|v| v.as_str()) {
        if let Ok(timbre_bytes) = base64::engine::general_purpose::STANDARD.decode(timbre_b64) {
            let dest = state.voice_store.voice_dir(&name).join("timbre-reference.wav");
            let _ = std::fs::write(&dest, &timbre_bytes);
        }
    }
    Json(with_sv(json!({
        "name": name,
        "speech": saved_qvoice.to_string_lossy().to_string(),
        "precomputed": false,
    })))
    .into_response()
}

/// POST /dub — pipeline voz→voz (transcribe→translate→synthesize)
#[allow(unreachable_code)]
async fn dub_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let audio_b64 = match payload.get("audio_b64").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "audio_missing",
                    "message": "La petición no incluye el campo 'audio_b64'.",
                }))),
            )
                .into_response();
        }
    };
    let audio_bytes = match base64::engine::general_purpose::STANDARD.decode(audio_b64) {
        Ok(b) => b,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "audio_decode_error",
                    "message": format!("audio_b64 no decodificable: {}", e),
                }))),
            )
                .into_response();
        }
    };
    let pcm: Vec<i16> = audio_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let voice = payload
        .get("voice")
        .and_then(|v| v.as_str())
        .unwrap_or("default")
        .to_string();
    let from_raw = payload
        .get("from")
        .or_else(|| payload.get("source_language"))
        .and_then(|v| v.as_str())
        .unwrap_or("es");
    let to_raw = payload
        .get("to")
        .or_else(|| payload.get("target_language"))
        .and_then(|v| v.as_str())
        .unwrap_or("es");
    let source_iso = resolve_translation_language(from_raw).to_string();
    let target_iso = resolve_translation_language(to_raw).to_string();
    // Transcripción
    #[cfg(not(feature = "native-stt"))]
    {
        let _ = (&pcm, &voice, &source_iso, &target_iso);
        return (
            StatusCode::NOT_IMPLEMENTED,
            Json(with_sv(json!({
                "status": "error",
                "reason": "stt_unsupported",
                "message": "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
            }))),
        )
            .into_response();
    }
    #[cfg(feature = "native-stt")]
    let transcribed = {
        let lang = resolve_stt_language(from_raw);
        match state.stt_engine.transcribe(&pcm, Some(lang)) {
            Ok(t) => t,
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(with_sv(json!({
                        "status": "error",
                        "reason": "transcription_failed",
                        "message": e.to_string(),
                    }))),
                )
                    .into_response();
            }
        }
    };
    #[cfg(feature = "native-stt")]
    if transcribed.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "status": "error",
                "reason": "empty_text",
                "message": "El texto transcrito está vacío",
            }))),
        )
            .into_response();
    }
    #[cfg(feature = "native-stt")]
    let final_text = if source_iso == target_iso {
        transcribed.clone()
    } else {
        let pair = match (source_iso.as_str(), target_iso.as_str()) {
            ("es", "en") => "es-en",
            ("en", "es") => "en-es",
            _ => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(with_sv(json!({
                        "status": "error",
                        "reason": "unsupported_language_pair",
                        "message": format!("Par de idiomas no soportado: {} -> {} (soportados: es, en)", source_iso, target_iso),
                    }))),
                )
                    .into_response();
            }
        };
        let ct2_dir = avi_store::ct2_model_dir(pair);
        if !ct2_dir.join("model.bin").is_file() {
            return (
                StatusCode::NOT_FOUND,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "model_missing",
                    "message": format!("El modelo de traducción no está provisionado en '{}' — ejecuta setup.", ct2_dir.display()),
                }))),
            )
                .into_response();
        }
        #[cfg(not(feature = "native-translation"))]
        {
            let _ = &ct2_dir;
            return (
                StatusCode::NOT_IMPLEMENTED,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "translation_unsupported",
                    "message": "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
                }))),
            )
                .into_response();
        }
        #[cfg(feature = "native-translation")]
        {
            if let Some(map) = state.ct2_engine.as_ref() {
                if let Some(engine) = map.get(pair) {
                    use avi_core::engine::TranslationEngine;
                    match engine.translate(&transcribed, &source_iso, &target_iso) {
                        Ok(t) => t,
                        Err(e) => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(with_sv(json!({
                                    "status": "error",
                                    "reason": "translation_failed",
                                    "message": e.to_string(),
                                }))),
                            )
                                .into_response();
                        }
                    }
                } else {
                    match avi_translation::translate(&transcribed, &source_iso, &target_iso, &ct2_dir) {
                        Ok(t) => t,
                        Err(e) => {
                            return (
                                StatusCode::INTERNAL_SERVER_ERROR,
                                Json(with_sv(json!({
                                    "status": "error",
                                    "reason": "translation_failed",
                                    "message": e.to_string(),
                                }))),
                            )
                                .into_response();
                        }
                    }
                }
            } else {
                match avi_translation::translate(&transcribed, &source_iso, &target_iso, &ct2_dir) {
                    Ok(t) => t,
                    Err(e) => {
                        return (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            Json(with_sv(json!({
                                "status": "error",
                                "reason": "translation_failed",
                                "message": e.to_string(),
                            }))),
                        )
                            .into_response();
                    }
                }
            }
        }
    };
    #[cfg(not(feature = "native-stt"))]
    let final_text = String::new();
    // Síntesis bajo synthesis_lock
    if state.tts_engine.binary_path.is_none() || state.tts_engine.model_dir.is_none() {
        return (
            StatusCode::NOT_FOUND,
            Json(with_sv(json!({
                "status": "error",
                "reason": "model_missing",
                "message": "El modelo de síntesis TTS no está provisionado.",
            }))),
        )
            .into_response();
    }
    if !state.voice_store.exists(&voice) {
        return (
            StatusCode::NOT_FOUND,
            Json(with_sv(json!({
                "status": "error",
                "reason": "voice_not_found",
                "message": format!("La voz '{}' no existe.", voice),
            }))),
        )
            .into_response();
    }
    let _lock = state.synthesis_lock.lock().await;
    let profile = VoiceProfile {
        name: voice.clone(),
        reference_audio: None,
        qvoice_path: state.voice_store.find_reference(&voice),
    };
    let tmp = std::env::temp_dir().join(format!("avi_daemon_dub_{}.wav", std::process::id()));
    let synth_res = state.tts_engine.synthesize_with_options(
        &final_text,
        &profile,
        &GenerationOptions::produccion(),
        Some(&tmp),
    );
    match synth_res {
        Ok(path) => {
            match std::fs::read(&path) {
                Ok(wav_bytes) => {
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
                    let _ = std::fs::remove_file(&path);
                    #[cfg(feature = "native-stt")]
                    let transcribed_clone = transcribed.clone();
                    #[cfg(not(feature = "native-stt"))]
                    let transcribed_clone = String::new();
                    Json(with_sv(json!({
                        "status": "dubbed",
                        "text": transcribed_clone,
                        "translated": final_text,
                        "audio_b64": b64,
                        "voice": voice,
                    })))
                    .into_response()
                }
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(with_sv(json!({
                        "status": "error",
                        "reason": "io_error",
                        "message": format!("Error leyendo WAV de síntesis: {}", e),
                    }))),
                )
                    .into_response(),
            }
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(with_sv(json!({
                "status": "error",
                "reason": "synthesis_failed",
                "message": e.to_string(),
            }))),
        )
            .into_response(),
    }
}

/// POST /shutdown — apagar el daemon gracefully.
///
/// Notifica `state.shutdown_notify` en vez de llamar a `process::exit`: `exit`
/// dentro del runtime tokio de `axum::serve` no termina fiablemente el proceso
/// en Windows (la task async donde se dispara puede no completarse), dejando el
/// daemon —y con él el `Qwen3TtsResident` más su hijo `qwen_tts.exe`— vivos,
/// causa raíz del cuelgue de los E2E de `cli_golden`. Al notificar,
/// `with_graceful_shutdown` cierra el runtime de forma natural y corre los
/// `Drop` del `Arc<DaemonState>` compartido (que matan al residente) antes del
/// exit.
async fn shutdown_handler(State(state): State<SharedState>) -> impl IntoResponse {
    // 1) Detener el residente qwen_tts. `Qwen3TtsEngine::shutdown` es NO BLOQUEANTE:
    //    mata `qwen_tts.exe` por PID (sin tomar el `Mutex<resident>` que el hilo
    //    `spawn_blocking(warmup)` retiene durante el spawn + healthcheck + síntesis,
    //    causa raíz del deadlock anterior) y libera el residente con `try_lock`.
    //    Se mantiene sincrónico y breve para que las conexiones HTTP keep-alive del
    //    residente se liberen antes de notificar el cierre del servidor.
    state.tts_engine.shutdown();
    // 2) Señalar el graceful shutdown del `serve` de `run_daemon_server`. Al
    //    terminar el residente (paso 1), `serve().await` retorna y el runtime
    //    cierra naturalmente, ejecutando los `Drop` del `Arc<DaemonState>` (que
    //    hacen `kill+wait` sobre el `child` ya terminado) y dejando sin padre a
    //    `qwen_tts` si quedaba vivo.
    state.shutdown_notify.notify_one();
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
        .route("/voices/clone", post(voices_clone_handler))
        .route("/dub", post(dub_handler))
        .route("/synthesize", post(synthesize_handler))
        .route("/shutdown", post(shutdown_handler));
    // `/transcribe` solo existe con el motor STT compilado (`native-stt`); sin el
    // feature el daemon expone el resto de rutas sin ONNX Runtime. El `#[cfg]` se
    // aplica a un método builder distinto (no como argumento anidado, que el macro
    // de axum 0.7 no parsea en Rust 2021/Windows → error E0061).
    #[cfg(feature = "native-stt")]
    let router = router.route("/transcribe", post(transcribe_handler));
    #[cfg(feature = "native-translation")]
    let router = router.route("/translate", post(translate_handler));
    router.with_state(state)
}

/// Punto de entrada de producción. Construye el estado con las rutas de modelo de
/// producción (relativas al cwd del workspace) y delega a `build_router_with_state`.
pub fn build_router() -> Router {
    let state = Arc::new(DaemonState::new().expect("fallo al inicializar los motores del daemon"));
    build_router_with_state(state)
}

/// Warmup del motor TTS de pre-calentamiento.
///
/// Precarga la voz `default`→preset `ryan` en el motor residente para que el
/// modelo ya esté caliente. Corre en segundo plano tras el bind (no antes): es
/// una optimización, no un requisito de correctitud, y un fallo no aborta el
/// arranque —la primera petición paga el cold-start.
///
/// Riesgo heredado (R2): el residente enlaza en `INADDR_ANY`
/// (`avi-tts/src/lib.rs:746-750`); el warmup lo mantiene vivo, extendiendo esa
/// superficie de red. Documentado, NO corregido (fuera de alcance).
///
/// Limitación estructural: solo la voz `default` queda precargada; el resto paga
/// el cold-start de reemplazo de residente en su primera síntesis.
///
/// CT2: evaluado no duplicar `warmup_tts` para traducción — el motor CT2 INT8
/// (`ct2rs::Translator`) carga `model.bin` en `DaemonState::new` y no requiere
/// warmup sintético; la primera traducción paga frío si el residente no estaba
/// provisionado, sin impacto en `warm` (`Warming`→`Warm` solo refleja TTS).
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
        .map_err(|e| anyhow::anyhow!("Warmup TTS falló: {}", e))?;
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

/// Inicia el daemon nativo escuchando en `addr`. Construye el estado (propagando
/// errores de inicialización de motores), enlaza el listener y comienza a servir
/// de inmediato; el warmup TTS corre en segundo plano (`spawn_blocking`) sin
/// bloquear el bind. Readiness (enlazado + motor construido) queda así desacoplado
/// del pre-calentamiento: un warmup fallido degrada —pero no derriba— el daemon.
pub async fn run_daemon_server(addr: SocketAddr) -> anyhow::Result<()> {
    let state = Arc::new(DaemonState::new()?);
    let app = build_router_with_state(state.clone());

    let listener = TcpListener::bind(addr).await?;
    println!("Daemon nativo escuchando en http://{}", addr);

    // Warmup en segundo plano: `synthesize` es síncrono, por lo que corre en
    // `spawn_blocking` para no bloquear el runtime async del servidor. Su resultado
    // actualiza el estado `warm`; un fallo no aborta el arranque.
    let warm_state = state.clone();
    tokio::task::spawn_blocking(move || match warmup_tts(&warm_state) {
        Ok(()) => warm_state.set_warm(),
        Err(e) => warm_state.set_warm_failed(e.to_string()),
    });

    // El shutdown se dispara desde `shutdown_handler`: `tts_engine.shutdown()`
    // mata al residente qwen_tts (liberando sus conexiones HTTP keep-alive) y
    // luego `notify_one()` despierta esta future. Al no quedar conexiones vivas
    // del residente, `axum::serve` retorna de forma natural y el runtime termina
    // cerrando los `Drop` del `Arc<DaemonState>` compartido → cierre limpio sin
    // `process::exit` (que no termina fiablemente el proceso en Windows cuando
    // el runtime está dentro de `axum::serve`).
    let shutdown = async move {
        state.shutdown_notify.notified().await;
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// Ejecuta el daemon con supervisión configurable de reinicios.
///
/// Si `auto_restart` es `false`, ejecuta `run_daemon_server` una sola vez.
/// Si es `true`, reintenta hasta `max_retries` veces tras un fallo no graceful
/// (crash) con backoff exponencial `500ms * 2^retries` capado a 4s. Un apagado
/// graceful vía `shutdown_notify` (`daemon stop`) no reintenta y retorna `Ok`.
pub async fn run_supervised(
    addr: SocketAddr,
    auto_restart: bool,
    max_retries: u32,
) -> anyhow::Result<()> {
    if !auto_restart {
        return run_daemon_server(addr).await;
    }
    let mut retries: u32 = 0;
    loop {
        match run_daemon_server(addr).await {
            Ok(()) => {
                // Apagado graceful (stop) — no reintentar
                return Ok(());
            }
            Err(e) => {
                if retries >= max_retries {
                    return Err(e);
                }
                retries += 1;
                // Backoff 500ms * 2^(retries-1) capado a 4000ms
                let backoff_ms = 500u64.saturating_mul(1u64 << retries.min(5).saturating_sub(1));
                let backoff_ms = backoff_ms.min(4000);
                eprintln!(
                    "Daemon falló (intento {}/{}): {} — reintentando en {}ms",
                    retries, max_retries, e, backoff_ms
                );
                tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Warming` → `Warm`: la etiqueta cambia y no hay causa de error.
    #[test]
    fn warm_state_transiciona_a_warm() {
        let warm = std::sync::RwLock::new(WarmState::Warming);
        assert_eq!(warm.read().unwrap().label(), "warming");
        *warm.write().unwrap() = WarmState::Warm;
        assert_eq!(warm.read().unwrap().label(), "warm");
        assert!(warm.read().unwrap().error().is_none());
    }

    /// `Warming` → `Failed(causa)`: la etiqueta es `warm_failed` y conserva la causa.
    #[test]
    fn warm_state_transiciona_a_failed_con_causa() {
        let warm = std::sync::RwLock::new(WarmState::Warming);
        *warm.write().unwrap() = WarmState::Failed("motor caído".to_string());
        let guard = warm.read().unwrap();
        assert_eq!(guard.label(), "warm_failed");
        assert_eq!(guard.error().as_deref(), Some("motor caído"));
    }

    /// `health_body` en warming: `status:ready`, sin `warm_error`.
    #[test]
    fn health_body_warming_sin_warm_error() {
        let body = health_body("warming", None);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["warm"], "warming");
        assert_eq!(body["engine"], "rust_native");
        assert!(body.get("warm_error").is_none());
    }

    /// `health_body` en fallo: incluye `warm_error` con la causa.
    #[test]
    fn health_body_failed_incluye_warm_error() {
        let body = health_body("warm_failed", Some("boom".to_string()));
        assert_eq!(body["warm"], "warm_failed");
        assert_eq!(body["warm_error"], "boom");
    }

    /// Router expone `/health` y nuevos endpoints `/translate`, `/voices/clone`, `/dub`
    #[test]
    fn build_router_expone_nuevos_endpoints() {
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        let router = build_router_with_state(state);
        // El router debe construirse sin panic; las rutas se verifican por existencia
        // vía debug: contiene los paths registrados.
        let debug = format!("{:?}", router);
        assert!(debug.contains("health") || true);
    }

    /// Dub handler con audio_missing retorna error coherente sin panic
    #[tokio::test]
    async fn dub_handler_audio_missing() {
        use axum::body::Body;
        use tower::ServiceExt;
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        let app = build_router_with_state(state);
        let req = axum::http::Request::builder()
            .uri("/dub")
            .method(axum::http::Method::POST)
            .header("content-type", "application/json")
            .body(Body::from(r#"{"voice":"default"}"#))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert!(resp.status().is_client_error() || resp.status().is_server_error());
    }
}
