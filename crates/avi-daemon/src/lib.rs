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
pub use spawn::spawn_background;
// `hilos_disponibles` y el trait `SttEngine` (`.transcribe`) solo los consume
// la superficie STT, gateada tras `native-stt`.
#[cfg(feature = "native-stt")]
use avi_core::engine::{hilos_disponibles, SttEngine};
use avi_core::json_emitter;
use avi_store::{SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_store::ModelStore;
#[cfg(feature = "native-stt")]
use avi_stt::{detectar_idioma, ParakeetEngine};
use avi_tts::{GenerationOptions, Qwen3TtsEngine, TtsEngine, VoiceProfile};
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
        let stt_engine = ParakeetEngine::new(&stt_dir)
            .map_err(|e| anyhow::anyhow!("fallo al cargar el modelo STT {}: {e}", stt_dir.display()))?;
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
        .route("/synthesize", post(synthesize_handler))
        .route("/shutdown", post(shutdown_handler));
    // `/transcribe` solo existe con el motor STT compilado (`native-stt`); sin el
    // feature el daemon expone el resto de rutas sin ONNX Runtime. El `#[cfg]` se
    // aplica a un método builder distinto (no como argumento anidado, que el macro
    // de axum 0.7 no parsea en Rust 2021/Windows → error E0061).
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
}
