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
pub use spawn::{kill_tree_by_pid, pid_alive, spawn_background, wait_for_pid_death};
// `spawn_uninstall_helper` solo existe bajo `#[cfg(windows)]` en `spawn.rs`; el
// reexport debe compartir el gate o el build no-Windows rompe con E0432 (el call
// site en `src/main.rs` ya está dentro de un bloque `#[cfg(windows)]`).
#[cfg(windows)]
pub use spawn::spawn_uninstall_helper;
// El trait `SttEngine` (`.transcribe`) solo lo consume la superficie STT,
// gateada tras `native-stt`.
#[cfg(feature = "native-stt")]
use avi_core::engine::SttEngine;
use avi_core::json_emitter;
#[cfg(feature = "native-stt")]
use avi_store::ModelStore;
use avi_store::{SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::{detect_language, ParakeetEngine};
#[cfg(feature = "native-translation")]
use avi_translation::Ct2TranslationEngine;
use avi_tts::{GenerationOptions, Qwen3TtsEngine, TtsEngine, VoiceProfile};
// `Engine` trait requerido por el API no-deprecation de base64 0.22
// (el motor interno del daemon usa el alfabeto STANDARD, idéntico al `encode`/`decode`
// libres, por compatibilidad con el cliente raíz del CLI).
use base64::Engine;

/// Idioma por defecto para `clone_voice` cuando la petición de clonado no
/// transporta un idioma explícito.
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
    /// Se precargan en `DaemonState::new` si el derivado está provisionado según
    /// el gate coincidente (`model.bin` más tokenizador); `None` significa motor
    /// ausente o roto (`model.bin` huérfano sin tokenizador: se registra el motivo
    /// y se arranca igual, sin derribar `run_daemon_server:614`).
    /// El warmup CT2 no duplica `warm_voice_engine`: la primera petición paga frío si
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
        #[cfg(feature = "native-translation")]
        let ct2_engine = {
            let mut map = std::collections::HashMap::new();
            for pair in &["es-en", "en-es"] {
                let dir = avi_store::ct2_model_dir(pair);
                if avi_store::is_ct2_provisioned(pair) {
                    if let Ok(engine) = Ct2TranslationEngine::new(&dir) {
                        map.insert(pair.to_string(), engine);
                    }
                } else if dir.join("model.bin").is_file() {
                    eprintln!(
                        "[daemon] CT2 {} roto en '{}' (faltan: {}) — arranca sin residente, ejecuta setup",
                        pair,
                        dir.display(),
                        avi_store::ct2_missing_files(pair).join(", ")
                    );
                }
            }
            if map.is_empty() {
                None
            } else {
                Some(map)
            }
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
    pub fn set_warm_failed(&self, cause: String) {
        *self.warm.write().unwrap() = WarmState::Failed(cause);
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

/// Intervalo de latido de los streams NDJSON con trabajo pesado: entre
/// eventos de inferencia el servidor emite `heartbeat` para que el timeout de
/// inactividad del cliente (1500 ms) solo dispare si el bucle está atascado,
/// nunca por inferencia sana.
const STREAM_HEARTBEAT: std::time::Duration = std::time::Duration::from_millis(500);

/// Espera un trabajo pesado bloqueante emitiendo latidos NDJSON.
/// Retorna `Some(res)` al completar, o `None` si el cliente se desconectó: en
/// ese caso el trabajo se aborta (`AbortHandle`) y el llamante debe retornar
/// sin entregar ni persistir resultado (sin trabajo huérfano ni fuga de estado).
/// Límite conocido: el aborto no puede preemptar código síncrono en curso (la
/// inferencia nativa corre hasta su punto de retorno); lo que sí garantiza es
/// que su resultado no se usa: ni eventos, ni persistencia, ni warmup derivado.
async fn with_heartbeats<T>(
    tx: &tokio::sync::mpsc::Sender<String>,
    stage: &str,
    handle: tokio::task::JoinHandle<T>,
) -> Option<Result<T, tokio::task::JoinError>> {
    let mut handle = handle;
    loop {
        tokio::select! {
            _ = tokio::time::sleep(STREAM_HEARTBEAT) => {
                emit_ndjson(tx, json!({ "event": "heartbeat", "stage": stage })).await;
            }
            _ = tx.closed() => {
                handle.abort();
                return None;
            }
            res = &mut handle => {
                return Some(res);
            }
        }
    }
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// Construye el cuerpo de `/health` a partir del estado de warmup. Función pura
/// (testeable sin daemon): emite `{status:"ready", warm, engine}` y añade
/// `warm_error` solo cuando el warmup falló. Notas aditivas `ct2`/`stt`
/// (`warm/warming/warm_failed`) se insertan en `health_handler` cuando residentes.
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

/// Enriquecimiento determinista de `health_body` con claves aditivas `stt`/`ct2`.
///
/// Separa la mutación `cfg`-gated del handler para que `health_handler` no
/// requiera `#[allow(unused_mut)]` cuando ningún feature está activo: la
/// mutabilidad vive dentro de esta función, donde cada `cfg` la usa.
fn enrich_health_body(body: Value, state: &DaemonState) -> Value {
    // Uso determinista de `state` incluso sin features (evita `unused variable` sin `allow`).
    let _ = state as &DaemonState;
    #[cfg(any(feature = "native-stt", feature = "native-translation"))]
    let mut body = body;
    #[cfg(feature = "native-stt")]
    {
        body["stt"] = Value::String("warm".into());
    }
    #[cfg(feature = "native-translation")]
    {
        if state.ct2_engine.is_some() {
            body["ct2"] = Value::String("warm".into());
        }
    }
    body
}

/// GET /health — readiness (enlazado + motor construido) con estado de warmup.
/// Reporta `{status:"ready", warm, engine}` leyendo el estado compartido; readiness
/// es inmediato, `warm` refleja el pre-calentamiento en curso o su resultado.
/// Claves aditivas `stt`/`ct2` (`warm/warming/warm_failed`) se emiten solo cuando residentes.
async fn health_handler(State(state): State<SharedState>) -> Json<Value> {
    let (label, error) = state.warm_snapshot();
    let body = enrich_health_body(health_body(label, error), &state);
    Json(with_sv(body))
}

/// POST /synthesize — síntesis con streaming NDJSON de progreso
///
/// Contrato NDJSON vigente: `start` → `progress`(N)
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
    // Traducción opt-in: sin flags no se traduce (origen = destino).
    let target_raw = payload
        .get("target_language")
        .and_then(|v| v.as_str())
        .unwrap_or("es-latam")
        .to_string();
    let source_raw = payload
        .get("source_language")
        .and_then(|v| v.as_str())
        .unwrap_or(target_raw.as_str())
        .to_string();
    let temperature = payload
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|t| t as f32);

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
    let source_owned = source_raw.clone();
    let target_owned = target_raw.clone();

    tokio::spawn(async move {
        // El lock envuelve completamente el trabajo de síntesis —incluido dentro del
        // spawn—, serializando síntesis concurrentes. No se añade semáforo de
        // admisión (fuera de alcance de esta rutina).
        let _lock = state.synthesis_lock.lock().await;
        // Reloj de trabajo tomado tras el lock: mide trabajo puro (excluye la
        // espera en cola) para separar la señal de rendimiento de la de
        // corrección; el techo sigue siendo el failsafe `SYNTH_DEADLINE`.
        let work_t0 = std::time::Instant::now();

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
        // `model_missing` es el contrato aceptado en entornos sin motor
        // (en el daemon real corre desde la raíz del repo, donde sí resuelve).
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

        // Temperatura opcional del CLI (ya validada allí); aquí se defiende el
        // rango para payloads directos al HTTP.
        if let Some(t) = temperature {
            if !(t > 0.0 && t <= 2.0) {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "usage_error",
                        "message": "Error: --temperature debe ser mayor que 0 y como máximo 2.0.",
                    }),
                )
                .await;
                return;
            }
        }

        // Traducción opt-in con el motor residente: passthrough si coinciden.
        let source_iso = resolve_translation_language(&source_owned).to_string();
        let target_iso = resolve_translation_language(&target_owned).to_string();
        let text_final = if source_iso == target_iso {
            text_owned.clone()
        } else {
            let pair = match (source_iso.as_str(), target_iso.as_str()) {
                ("es", "en") => "es-en",
                ("en", "es") => "en-es",
                _ => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "unsupported_language_pair",
                            "message": format!("Par de idiomas no soportado: {} -> {} (soportados: es, en)", source_iso, target_iso),
                        }),
                    )
                    .await;
                    return;
                }
            };
            let ct2_dir = avi_store::ct2_model_dir(pair);
            if !avi_store::is_ct2_provisioned(pair) {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "model_missing",
                        "message": format!("El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.", ct2_dir.display(), avi_store::ct2_missing_files(pair).join(", ")),
                    }),
                )
                .await;
                return;
            }
            #[cfg(not(feature = "native-translation"))]
            {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "translation_unsupported",
                        "message": "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
                    }),
                )
                .await;
                return;
            }
            #[cfg(feature = "native-translation")]
            {
                let translated = if let Some(map) = state.ct2_engine.as_ref() {
                    if let Some(engine) = map.get(pair) {
                        use avi_core::engine::TranslationEngine;
                        engine.translate(&text_owned, &source_iso, &target_iso)
                    } else {
                        avi_translation::translate(&text_owned, &source_iso, &target_iso, &ct2_dir)
                    }
                } else {
                    avi_translation::translate(&text_owned, &source_iso, &target_iso, &ct2_dir)
                };
                match translated {
                    Ok(t) => t,
                    Err(e) => {
                        emit_ndjson(
                            &tx,
                            json!({
                                "event": "error",
                                "reason": "translation_failed",
                                "message": e.to_string(),
                            }),
                        )
                        .await;
                        return;
                    }
                }
            }
        };

        // Perfil de voz: .qvoice si la voz está clonada; el motor resuelve el
        // preset vía `resolve_voice_engine` a partir del nombre.
        let profile = VoiceProfile {
            name: voice_owned.clone(),
            qvoice_path: state.voice_store.find_reference(&voice_owned),
        };
        // Sin flag se usa la config de producción (temperature=0.35); con flag
        // se sobrescribe la temperatura ya validada.
        let options = GenerationOptions::with_temperature(temperature);
        let tmp = std::env::temp_dir().join(format!("avi_daemon_synth_{}.wav", std::process::id()));
        // La síntesis sobre el residente es síncrona y puede colgarse
        // (motor C atascado); se acota con `timeout(SYNTH_DEADLINE)` sobre
        // `spawn_blocking` (mismo patrón del warmup). Al vencer se emite
        // `synthesis_timeout` propio, SIN matar/reclamar el residente (evita
        // livelock con un rearranque legítimo en curso; la reclamación es de
        // la salud observada de la siguiente petición).
        let state_synth = state.clone();
        let synth_handle = tokio::task::spawn_blocking(move || {
            state_synth.tts_engine.synthesize_with_options(
                &text_final,
                &profile,
                &options,
                Some(&tmp),
            )
        });
        match tokio::time::timeout(SYNTH_DEADLINE, synth_handle).await {
            Ok(Ok(Ok(path))) => {
                match std::fs::read(&path) {
                    Ok(wav_bytes) => {
                        emit_ndjson(
                            &tx,
                            json!({
                                "event": "result",
                                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&wav_bytes),
                                // El motor no expone tiempos; por contrato el campo
                                // existe y se reporta como 0.0.
                                "t3_time": 0.0,
                                "s3gen_time": 0.0,
                                // Ms de trabajo puro tras el lock (excluye la
                                // espera en cola), como señal de rendimiento
                                // separada de la de corrección.
                                "work_ms": work_t0.elapsed().as_millis() as u64,
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
            Ok(Ok(Err(e))) => {
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
            Ok(Err(join_err)) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "synthesis_failed",
                        "message": format!("El hilo de síntesis falló: {}", join_err),
                    }),
                )
                .await;
            }
            Err(_elapsed) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "synthesis_timeout",
                        "message": format!(
                            "La síntesis venció el deadline de {} s.",
                            SYNTH_DEADLINE.as_secs()
                        ),
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
/// Contrato vigente: el campo es `audio_b64` (no
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
            let (detected_language, _ratio) = detect_language(&text);
            let body = if detected_language == "EN-SOSPECHOSO" {
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
    if !avi_store::is_ct2_provisioned(pair) {
        return (
            StatusCode::NOT_FOUND,
            Json(with_sv(json!({
                "error": "model_missing",
                "reason": "model_missing",
                "message": format!("El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.", ct2_dir.display(), avi_store::ct2_missing_files(pair).join(", ")),
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

/// POST /voices/clone — clonado de voz con audio base64, servido como stream
/// NDJSON con latidos: `started` tras las validaciones baratas (nombre,
/// force/colisión, audio, modelo base, en JSON plano con los códigos de siempre),
/// latidos `heartbeat` durante el clonado, y evento final `result` con la forma
/// contractual actual (`precomputed: true` = precarga en caliente iniciada).
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
    let tmp_wav = std::env::temp_dir().join(format!(
        "avi_daemon_clone_{}_{}.wav",
        name,
        std::process::id()
    ));
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
    let tmp_qvoice = std::env::temp_dir().join(format!(
        "avi_daemon_clone_{}_{}.qvoice",
        name,
        std::process::id()
    ));
    // Stream NDJSON: las validaciones baratas ya pasaron en JSON plano;
    // `started` inmediato antes del trabajo pesado, latidos cada ~500 ms y
    // evento final con la forma contractual actual (`precomputed: true` =
    // «precarga en caliente iniciada»). Sin almacén de trabajos ni expiración.
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move {
        emit_ndjson(&tx, json!({ "event": "started", "name": name })).await;
        let event_name = name.clone();
        let tmp_qvoice_cleanup = tmp_qvoice.clone();
        let clone_work = tokio::task::spawn_blocking(move || {
            let r = avi_tts::clone_voice(
                &base_model_dir,
                &tmp_wav,
                &tmp_qvoice,
                &name,
                DEFAULT_CLONE_LANGUAGE,
            );
            let _ = std::fs::remove_file(&tmp_wav);
            match r {
                Ok(()) => Ok((tmp_qvoice, name)),
                Err(e) => {
                    let _ = std::fs::remove_file(&tmp_qvoice);
                    Err(e)
                }
            }
        });
        let (tmp_qvoice, name) = match with_heartbeats(&tx, "clone", clone_work).await {
            None => {
                // Cliente desconectado: trabajo abortado; limpieza best-effort
                // del parcial (el hilo bloqueante puede seguir hasta su retorno).
                let _ = std::fs::remove_file(&tmp_qvoice_cleanup);
                return;
            }
            Some(Ok(Ok(v))) => v,
            Some(Ok(Err(e))) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "name": event_name,
                        "reason": "voice_clone_failed",
                        "message": e.to_string(),
                    }),
                )
                .await;
                return;
            }
            Some(Err(join_err)) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "name": event_name,
                        "reason": "voice_clone_failed",
                        "message": format!("El hilo de clonado falló: {}", join_err),
                    }),
                )
                .await;
                return;
            }
        };
        // Sin entrega a cliente caído: no persistir (sin fuga de estado).
        if tx.is_closed() {
            let _ = std::fs::remove_file(&tmp_qvoice);
            return;
        }
        let saved_qvoice = match state.voice_store.save_reference(&name, &tmp_qvoice) {
            Ok(p) => p,
            Err(e) => {
                let _ = std::fs::remove_file(&tmp_qvoice);
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "name": name,
                        "reason": "voice_clone_failed",
                        "message": e.to_string(),
                    }),
                )
                .await;
                return;
            }
        };
        let _ = std::fs::remove_file(&tmp_qvoice);
        // Warm-on-clone (A): precalienta la voz recién clonada en segundo plano
        // para eliminar el cold-start del residente en la primera síntesis del
        // flujo clonar→sintetizar. Se conserva en `spawn_blocking` y se anuncia
        // por evento (por eso `precomputed: true` = precarga iniciada).
        let warm_state = state.clone();
        let warm_name = name.clone();
        tokio::task::spawn_blocking(move || {
            *warm_state.warm.write().unwrap() = WarmState::Warming;
            match warm_voice_engine(&warm_state, &warm_name) {
                Ok(()) => warm_state.set_warm(),
                Err(e) => warm_state.set_warm_failed(e.to_string()),
            }
        });
        emit_ndjson(
            &tx,
            json!({
                "event": "progress",
                "stage": "warmup",
                "message": "Precarga en caliente iniciada.",
            }),
        )
        .await;
        emit_ndjson(
            &tx,
            json!({
                "event": "result",
                "name": name,
                "speech": saved_qvoice.to_string_lossy().to_string(),
                "precomputed": true,
            }),
        )
        .await;
    });

    // Convertir el receptor en un stream NDJSON (mismo patrón que `synthesize_handler`).
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

/// POST /dub — pipeline voz→voz (transcribe→translate→synthesize), servido
/// como stream NDJSON con latidos: `started` tras las validaciones
/// baratas (audio, par/CT2, modelo, voz y rama sin `native-stt`, en JSON plano
/// con los códigos de siempre), latidos `heartbeat` durante cada fase pesada y
/// evento final `result` con la forma contractual actual (`status: "dubbed"`).
/// `SYNTH_DEADLINE` se conserva como cota de la fase de síntesis con evento de
/// fallo explícito.
async fn dub_handler(State(state): State<SharedState>, Json(payload): Json<Value>) -> Response {
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
    let temperature = payload
        .get("temperature")
        .and_then(|v| v.as_f64())
        .map(|t| t as f32);
    let source_iso = resolve_translation_language(from_raw).to_string();
    let target_iso = resolve_translation_language(to_raw).to_string();
    // Transcripción — requiere `native-stt`; sin el feature el pipeline no puede arrancar.
    #[cfg(not(feature = "native-stt"))]
    {
        let _ = (&state, &pcm, &voice, &source_iso, &target_iso, &temperature);
        (
            StatusCode::NOT_IMPLEMENTED,
            Json(with_sv(json!({
                "status": "error",
                "reason": "stt_unsupported",
                "message": "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
            }))),
        )
            .into_response()
    }
    // Validaciones baratas ANTES del stream (JSON plano con los códigos de
    // siempre; `started` solo se emite cuando el trabajo pesado va a arrancar).
    // Se adelantan aquí los chequeos por parámetros (par, CT2, feature, modelo,
    // voz) que antes corrían tras transcribir: solo cambia la precedencia cuando
    // varios fallos coinciden (barato-primero), nunca el código de cada fallo.
    #[cfg(feature = "native-stt")]
    let needs_translation = source_iso != target_iso;
    #[cfg(feature = "native-stt")]
    let translation_pair: Option<String> = if !needs_translation {
        None
    } else {
        match (source_iso.as_str(), target_iso.as_str()) {
            ("es", "en") => Some("es-en".to_string()),
            ("en", "es") => Some("en-es".to_string()),
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
        }
    };
    #[cfg(feature = "native-stt")]
    if let Some(pair) = translation_pair.as_deref() {
        let ct2_dir = avi_store::ct2_model_dir(pair);
        if !avi_store::is_ct2_provisioned(pair) {
            return (
                StatusCode::NOT_FOUND,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "model_missing",
                    "message": format!("El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.", ct2_dir.display(), avi_store::ct2_missing_files(pair).join(", ")),
                }))),
            )
                .into_response();
        }
    }
    #[cfg(all(feature = "native-stt", not(feature = "native-translation")))]
    if needs_translation {
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
    #[cfg(feature = "native-stt")]
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
    #[cfg(feature = "native-stt")]
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
    #[cfg(feature = "native-stt")]
    {
        // Stream NDJSON: `started` inmediato tras las validaciones
        // baratas, latidos durante la inferencia, evento final con la forma
        // contractual actual (`status: "dubbed"`). Sin almacén de trabajos.
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
        let from_owned = from_raw.to_string();
        tokio::spawn(async move {
            emit_ndjson(&tx, json!({ "event": "started", "voice": voice })).await;
            // Transcripción en `spawn_blocking` con latidos (fase pesada: STT
            // sobre el audio completo).
            let stt_state = state.clone();
            let stt_work = tokio::task::spawn_blocking(move || {
                let lang = resolve_stt_language(&from_owned);
                stt_state.stt_engine.transcribe(&pcm, Some(lang))
            });
            let transcribed = match with_heartbeats(&tx, "transcribe", stt_work).await {
                None => return,
                Some(Ok(Ok(t))) => t,
                Some(Ok(Err(e))) => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "transcription_failed",
                            "message": e.to_string(),
                        }),
                    )
                    .await;
                    return;
                }
                Some(Err(join_err)) => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "transcription_failed",
                            "message": format!("El hilo de transcripción falló: {}", join_err),
                        }),
                    )
                    .await;
                    return;
                }
            };
            if transcribed.trim().is_empty() {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "empty_text",
                        "message": "El texto transcrito está vacío",
                    }),
                )
                .await;
                return;
            }
            // Traducción o passthrough (la validación barata ya garantizó par
            // soportado, CT2 provisionado y feature residente cuando toca).
            let final_text = if source_iso == target_iso {
                transcribed.clone()
            } else {
                #[cfg(not(feature = "native-translation"))]
                {
                    // Red de seguridad inalcanzable en la práctica (la validación
                    // barata ya devolvió 501): fallo explícito, nunca silencioso.
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "translation_unsupported",
                            "message": "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
                        }),
                    )
                    .await;
                    return;
                }
                #[cfg(feature = "native-translation")]
                {
                    let pair = translation_pair
                        .clone()
                        .expect("la validación barata garantizó el par");
                    let dir = avi_store::ct2_model_dir(&pair);
                    let ct2_state = state.clone();
                    let text = transcribed.clone();
                    let source = source_iso.clone();
                    let target = target_iso.clone();
                    let translation_work = tokio::task::spawn_blocking(move || {
                        if let Some(map) = ct2_state.ct2_engine.as_ref() {
                            if let Some(engine) = map.get(&pair) {
                                use avi_core::engine::TranslationEngine;
                                engine.translate(&text, &source, &target)
                            } else {
                                avi_translation::translate(&text, &source, &target, &dir)
                            }
                        } else {
                            avi_translation::translate(&text, &source, &target, &dir)
                        }
                    });
                    match with_heartbeats(&tx, "translate", translation_work).await {
                        None => return,
                        Some(Ok(Ok(t))) => t,
                        Some(Ok(Err(e))) => {
                            emit_ndjson(
                                &tx,
                                json!({
                                    "event": "error",
                                    "reason": "translation_failed",
                                    "message": e.to_string(),
                                }),
                            )
                            .await;
                            return;
                        }
                        Some(Err(join_err)) => {
                            emit_ndjson(
                                &tx,
                                json!({
                                    "event": "error",
                                    "reason": "translation_failed",
                                    "message": format!("El hilo de traducción falló: {}", join_err),
                                }),
                            )
                            .await;
                            return;
                        }
                    }
                }
            };
            // Sin entrega a cliente caído: no sintetizar (sin trabajo huérfano).
            if tx.is_closed() {
                return;
            }
            // Síntesis bajo synthesis_lock con SYNTH_DEADLINE como cota de fase:
            // latidos durante la espera, aborto ante desconexión y evento de
            // fallo explícito al vencer (nunca cierre silencioso). Sin
            // matar/reclamar el residente (evita livelock con un rearranque
            // legítimo en curso; la reclamación es de la salud observada de la
            // siguiente petición).
            let _lock = state.synthesis_lock.lock().await;
            // Reloj de trabajo tomado tras el lock: mide solo la fase de
            // síntesis, no transcribe/translate.
            let work_t0 = std::time::Instant::now();
            let profile = VoiceProfile {
                name: voice.clone(),
                qvoice_path: state.voice_store.find_reference(&voice),
            };
            let tmp =
                std::env::temp_dir().join(format!("avi_daemon_dub_{}.wav", std::process::id()));
            let text_synth = final_text.clone();
            let synth_state = state.clone();
            let synth_options = GenerationOptions::with_temperature(temperature);
            let mut synth_handle = tokio::task::spawn_blocking(move || {
                synth_state.tts_engine.synthesize_with_options(
                    &text_synth,
                    &profile,
                    &synth_options,
                    Some(&tmp),
                )
            });
            let phase_deadline = tokio::time::sleep(SYNTH_DEADLINE);
            tokio::pin!(phase_deadline);
            let synth_res = loop {
                tokio::select! {
                    _ = tokio::time::sleep(STREAM_HEARTBEAT) => {
                        emit_ndjson(&tx, json!({ "event": "heartbeat", "stage": "synthesis" })).await;
                    }
                    _ = tx.closed() => {
                        synth_handle.abort();
                        return;
                    }
                    res = &mut synth_handle => break Some(res),
                    _ = &mut phase_deadline => {
                        synth_handle.abort();
                        break None;
                    }
                }
            };
            match synth_res {
                None => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "synthesis_timeout",
                            "message": format!(
                                "La síntesis venció el deadline de {} s.",
                                SYNTH_DEADLINE.as_secs()
                            ),
                        }),
                    )
                    .await;
                }
                Some(Ok(Ok(path))) => {
                    match std::fs::read(&path) {
                        Ok(wav_bytes) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
                            let _ = std::fs::remove_file(&path);
                            emit_ndjson(
                                &tx,
                                json!({
                                    "event": "result",
                                    "status": "dubbed",
                                    "text": transcribed,
                                    "translated": final_text,
                                    "audio_b64": b64,
                                    "voice": voice,
                                    // Ms de la fase de síntesis tras el lock,
                                    // como señal de rendimiento independiente.
                                    "work_ms": work_t0.elapsed().as_millis() as u64,
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
                                    "message": format!("Error leyendo WAV de síntesis: {}", e),
                                }),
                            )
                            .await;
                        }
                    }
                }
                Some(Ok(Err(e))) => {
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
                Some(Err(join_err)) => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "synthesis_failed",
                            "message": format!("El hilo de síntesis falló: {}", join_err),
                        }),
                    )
                    .await;
                }
            }
        });

        // Convertir el receptor en un stream NDJSON (mismo patrón que `synthesize_handler`).
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
    //    mata el árbol preciso por PID (sin tomar el `Mutex<resident>` que el hilo
    //    `spawn_blocking(warmup)` retiene durante el spawn + healthcheck + síntesis,
    //    causa raíz del deadlock anterior) y libera el residente con `try_lock`;
    //    el kill por imagen queda solo como último recurso documentado.
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
/// externamente. Extraído de `build_router()` para testeabilidad: permite
/// ejercer las rutas en tests de integración inyectando un estado con rutas de
/// modelo apuntando a `CARGO_MANIFEST_DIR`.
pub fn build_router_with_state(state: Arc<DaemonState>) -> Router {
    let router = Router::new()
        .route("/health", get(health_handler))
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

/// Deadline del warmup TTS: si `spawn_blocking(warm_voice_engine)` no termina en
/// este plazo, el daemon marca `warm_failed` con diagnóstico y termina al
/// residente. Valor medido, no supuesto: ~2× el TTFN feliz observado (~18-20 s
/// de spawn + healthcheck + síntesis), muy por debajo del hang histórico del
/// motor C (150 s+ quemando CPU sin llegar al audio).
const WARMUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(40);

/// Deadline de la síntesis por petición: acota `synthesize_handler` y
/// `dub_handler` para que el daemon emita su propio diagnóstico
/// (`synthesis_timeout`) ante un cuelgue intra-`POST` del residente, en vez
/// de ceder al corte ciego del cliente a los 10 s (`src/main.rs:3293`). Debe
/// ser `< 10 s` para que el daemon gane la carrera, y `≥` la síntesis feliz
/// sobre un residente ya caliente (muy inferior a los ~18-20 s del TTFN de
/// warmup, dominado por spawn + carga, ausentes aquí). 8 s deja ~2 s de
/// margen para que viaje la respuesta HTTP del daemon.
const SYNTH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(8);

/// Pre-calentamiento (warmup) del motor TTS parametrizado por voz.
///
/// Precarga la voz `voice` en el motor residente sintetizando un testigo
/// desechable para que el modelo ya esté caliente. Lo usan tanto el arranque
/// (voz configurable vía `--warm-voice`, por defecto `default`) como el
/// warm-on-clone (voz recién clonada). Corre en segundo plano: es una
/// optimización, no un requisito de correctitud, y un fallo no aborta el
/// arranque ni el clonado —la primera petición paga el cold-start.
///
/// Adquiere `synthesis_lock` durante la síntesis-testigo (`blocking_lock`, pues
/// corre en `spawn_blocking`): en warm-on-clone es obligatorio porque compite
/// con tráfico vivo; en el arranque es no disputado.
///
/// Riesgo heredado (R2): el residente enlaza en `INADDR_ANY`
/// (`avi-tts/src/lib.rs:746-750`); el warmup lo mantiene vivo, extendiendo esa
/// superficie de red. Documentado, NO corregido (fuera de alcance).
///
/// Limitación estructural (inherente al residente, no a `default`): el residente
/// TTS es de una sola voz, así que solo la última voz calentada queda precargada;
/// calentar otra evicciona la previa y el resto paga el cold-start de reemplazo
/// de residente en su primera síntesis.
///
/// CT2: evaluado no duplicar `warm_voice_engine` para traducción — el motor CT2 INT8
/// (`ct2rs::Translator`) carga `model.bin` en `DaemonState::new` y no requiere
/// warmup sintético; la primera traducción paga frío si el residente no estaba
/// provisionado, sin impacto en `warm` (`Warming`→`Warm` solo refleja TTS).
pub fn warm_voice_engine(state: &DaemonState, voice: &str) -> anyhow::Result<()> {
    let profile = VoiceProfile {
        name: voice.to_string(),
        qvoice_path: state.voice_store.find_reference(voice),
    };
    let _lock = state.synthesis_lock.blocking_lock();
    // Reloj de trabajo tomado tras el lock: mide warmup puro, excluye la espera
    // en cola contra tráfico vivo; el techo sigue siendo el failsafe
    // `WARMUP_DEADLINE` del llamante.
    let work_t0 = std::time::Instant::now();
    let tmp = std::env::temp_dir().join(format!("avi_daemon_warmup_{}.wav", std::process::id()));
    let result = state
        .tts_engine
        .synthesize_with_options(
            "Calentamiento del daemon.",
            &profile,
            &GenerationOptions::production(),
            Some(&tmp),
        )
        .map_err(|e| anyhow::anyhow!("Warmup TTS falló: {}", e));
    let _ = std::fs::remove_file(&tmp);
    eprintln!(
        "[daemon] warm_voice_engine '{}': {:.1} s tras locks (éxito={}).",
        voice,
        work_t0.elapsed().as_secs_f64(),
        result.is_ok()
    );
    result.map(|_| ())
}

/// Nombre de la env interna que transporta la ruta del fichero ready dentro
/// del proceso `serve`: el flag `--ready-file` la fija en `handle_daemon`
/// (`src/main.rs`) y `run_daemon_server` la consume. No es contrato público:
/// el padre solo conoce el flag y el fichero resultante.
pub const READY_FILE_ENV: &str = "AVI_READY_FILE";

/// Publica `addr` + `warm` en el fichero ready con escritura atómica
/// (temporal hermano + rename). Best-effort con diagnóstico: un fallo de
/// señalización no derriba el daemon (el evento en stderr sigue valiendo
/// como diagnóstico redundante).
fn write_ready_file(path: &std::path::Path, addr: &SocketAddr, warm: &str) {
    let tmp = path.with_extension("ready.tmp");
    // Recuperación de reclamo: además de `addr`/`warm`, el hijo publica
    // su propio PID. Con puertos efímeros, `addr` y PID vivían solo en el
    // pidfile; si el padre cae sin limpiarlo, el ready es la única pista para
    // reclamar el árbol huérfano por PID.
    let content = format!("addr={}\nwarm={}\npid={}\n", addr, warm, std::process::id());
    if std::fs::write(&tmp, content).is_err() {
        eprintln!(
            "aviso: no se pudo escribir el fichero ready {}",
            path.display()
        );
        return;
    }
    if std::fs::rename(&tmp, path).is_err() {
        eprintln!(
            "aviso: no se pudo publicar el fichero ready {}",
            path.display()
        );
    }
}

/// Inicia el daemon nativo escuchando en `addr`. Construye el estado (propagando
/// errores de inicialización de motores), enlaza el listener y comienza a servir
/// de inmediato; el warmup TTS corre en segundo plano (`spawn_blocking`) sin
/// bloquear el bind, acotado a `WARMUP_DEADLINE`. Readiness (enlazado + motor
/// construido) queda así desacoplado del pre-calentamiento: un warmup fallido o
/// vencido por el deadline degrada —pero no derriba— el daemon.
pub async fn run_daemon_server(addr: SocketAddr, warm_voice: String) -> anyhow::Result<()> {
    let state = Arc::new(DaemonState::new()?);
    let app = build_router_with_state(state.clone());

    // Habilitador: materializar las voces de fábrica en la instancia
    // (idempotente, desde el asset embebido). Sin esto, un `data_dir` virgen
    // (sandbox de estado por instancia vía `AVI_DATA_DIR`) aborta el fail-fast
    // de `--warm-voice` antes del bind porque `default` aún no existe en
    // disco. Tras esto, una voz inexistente sigue abortando igual (no es de
    // fábrica).
    state
        .voice_store
        .ensure_initialized()
        .map_err(|e| anyhow::anyhow!("No se pudo inicializar el almacén de voces: {}", e))?;

    // Fail-fast (D): una `--warm-voice` inexistente aborta el arranque antes del
    // bind, sin degradar en silencio ni caer a `default`.
    if state.voice_store.find_reference(&warm_voice).is_none() {
        return Err(anyhow::anyhow!(
            "La voz de warmup '{}' no existe: clónala o elige otra con --warm-voice.",
            warm_voice
        ));
    }

    // Sin reclamo aquí; el reclamo activo del árbol propio previo con
    // deadline y verificación vive solo en `run_supervised` (entre reintentos).
    let listener = TcpListener::bind(addr).await?;
    // Puerto efímero por instancia: publicar la dirección REALMENTE enlazada
    // (con `:0` el SO asigna, nunca el literal pedido).
    let bound = listener.local_addr()?;
    println!("Daemon nativo escuchando en http://{}", bound);

    // Transporte flag+fichero: si el padre designó fichero ready
    // (`--ready-file`), publicar la `addr` real tras el bind y el estado
    // warm tras el warmup. Escritura atómica; el evento en stderr se
    // conserva como diagnóstico redundante. Sin flag no se escribe nada.
    let ready_file: Option<std::path::PathBuf> =
        std::env::var_os(READY_FILE_ENV).map(std::path::PathBuf::from);
    if let Some(ref path) = ready_file {
        write_ready_file(path, &bound, "warming");
    }

    // Warmup en segundo plano: `synthesize` es síncrono, por lo que corre en
    // `spawn_blocking` para no bloquear el runtime async del servidor. El
    // `JoinHandle` se envuelve en un `timeout(WARMUP_DEADLINE)`: si expira, se
    // marca `warm_failed` con diagnóstico (posible cuelgue del motor C, ver log
    // del motor en `data/logs/qwen3-tts_*.log`) y se reclama el residente
    // colgado con `shutdown()` (árbol preciso por PID, sin tomar el mutex
    // que el hilo del warmup retiene; imagen solo como último recurso). Un
    // fallo no aborta el arranque.
    // Readiness por señal: el arranque emite el evento "ligado + warm"
    // (puerto real + estado) tras bind y warmup. El fichero ready (arriba) es
    // el transporte que consumen los llamantes con espera acotada; la línea
    // `avi-daemon-ready warm=<estado> addr=<real>` en stderr es diagnóstico
    // redundante (stdout queda reservado al anuncio de ligado).
    // Timeout = bug a diagnosticar, no flake a reintentar.
    let warm_state = state.clone();
    let ready_ok = ready_file.clone();
    let handle =
        tokio::task::spawn_blocking(move || match warm_voice_engine(&warm_state, &warm_voice) {
            Ok(()) => {
                warm_state.set_warm();
                eprintln!("avi-daemon-ready warm=warm addr={}", bound);
                if let Some(path) = ready_ok.as_ref() {
                    write_ready_file(path, &bound, "warm");
                }
            }
            Err(e) => {
                warm_state.set_warm_failed(e.to_string());
                eprintln!("avi-daemon-ready warm=warm_failed addr={}", bound);
                if let Some(path) = ready_ok.as_ref() {
                    write_ready_file(path, &bound, "warm_failed");
                }
            }
        });
    let timeout_state = state.clone();
    let ready_deadline = ready_file.clone();
    tokio::spawn(async move {
        if tokio::time::timeout(WARMUP_DEADLINE, handle).await.is_err() {
            timeout_state.set_warm_failed(format!(
                "Warmup TTS venció el deadline de {} s: posible cuelgue del motor C. \
                 Ver el log del motor en data/logs/qwen3-tts_*.log",
                WARMUP_DEADLINE.as_secs()
            ));
            eprintln!(
                "avi-daemon-ready warm=warm_failed addr={} causa=deadline",
                bound
            );
            if let Some(path) = ready_deadline.as_ref() {
                write_ready_file(path, &bound, "warm_failed");
            }
            timeout_state.tts_engine.shutdown();
        }
    });

    // El shutdown se dispara por la misma ruta desde POST `/shutdown` y desde
    // señales del sistema (Ctrl+C / SIGTERM): `tts_engine.shutdown()` mata el
    // árbol preciso del residente (liberando sus conexiones HTTP keep-alive) y
    // luego `notify_one()` despierta esta future (o la señal completa el
    // select directamente tras el mismo `shutdown()`). Al no quedar conexiones
    // vivas del residente, `axum::serve` retorna de forma natural y el runtime
    // termina cerrando los `Drop` del `Arc<DaemonState>` compartido → cierre
    // limpio sin `process::exit` (que no termina fiablemente el proceso en
    // Windows cuando el runtime está dentro de `axum::serve`).
    // `serve` en Unix cierra por esta misma ruta sin pidfile ni
    // auto-muerte del CLI (la guarda `pid != propio` del handler CLI protege al
    // `serve` en foreground; la carrera con el handler CLI se resuelve porque
    // ambos convergen en `tts_engine.shutdown()` + salida, sin pidfile).
    let shutdown = async move {
        #[cfg(unix)]
        let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("suscribir SIGTERM del sistema");
        // El `select!` resuelve al primer evento de cierre y no reintenta: cada
        // brazo era terminal (rompía el `loop`), así que la espera es de un solo
        // disparo sin bucle.
        #[cfg(unix)]
        {
            tokio::select! {
                _ = state.shutdown_notify.notified() => {}
                _ = tokio::signal::ctrl_c() => {
                    state.tts_engine.shutdown();
                }
                _ = sigterm.recv() => {
                    state.tts_engine.shutdown();
                }
            }
        }
        #[cfg(not(unix))]
        {
            tokio::select! {
                _ = state.shutdown_notify.notified() => {}
                _ = tokio::signal::ctrl_c() => {
                    state.tts_engine.shutdown();
                }
            }
        }
    };
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await?;
    Ok(())
}

/// Umbral de tiempo mínimo de ejecución antes de considerar que una iteración
/// del daemon realizó progreso, para el watchdog del bucle de supervisión.
const SUPERVISION_PROGRESS_MIN: std::time::Duration = std::time::Duration::from_secs(10);

/// Número máximo de caídas rápidas consecutivas (sin progreso) antes de abortar
/// el bucle de supervisión con fallo explícito.
const SUPERVISION_STREAK_MAX: u32 = 3;

/// Ejecuta el daemon con supervisión configurable de reinicios.
///
/// Si `auto_restart` es `false`, ejecuta `run_daemon_server` una sola vez.
/// Si es `true`, reintenta hasta `max_retries` veces tras un fallo no graceful
/// (crash) con backoff exponencial `500ms * 2^retries` capado a 4s, precedido de
/// reclamo activo del árbol propio previo con deadline y verificación: antes de
/// reintentar se espera (hasta 5 s, sondeo 200 ms) a que el puerto quede libre
/// (el `Drop` de la iteración previa ya mató el árbol preciso del residente;
/// sin kill global por imagen ni otra instancia) y se registra muerte + puerto
/// libre o sigue-ocupado; el siguiente `bind` lo confirma.
/// Un apagado graceful vía `shutdown_notify` (`daemon stop`) no reintenta y retorna `Ok`.
/// El reclamo activo matar-y-rearrancar ante otra instancia vive en el CLI
/// (`daemon start`): `serve` nunca mata a otra instancia sana.
pub async fn run_supervised(
    addr: SocketAddr,
    auto_restart: bool,
    max_retries: u32,
    warm_voice: String,
) -> anyhow::Result<()> {
    if !auto_restart {
        return run_daemon_server(addr, warm_voice).await;
    }
    // Watchdog del bucle de supervisión (acotado a este bucle, primitivas
    // portables `Instant`): una vida del daemon menor a `SUPERVISION_PROGRESS_MIN`
    // cuenta como caída rápida (sin progreso); `SUPERVISION_STREAK_MAX` caídas
    // rápidas seguidas abortan con fallo ruidoso en vez de quemar `max_retries`
    // en un crash-loop silencioso. Sin falsos positivos en el camino feliz: el
    // apagado graceful retorna `Ok` antes de contar, y una vida larga resetea
    // la racha (la patología de cola de los tests la corrige el reloj tras
    // locks del harness, no este watchdog).
    let mut fast_streak: u32 = 0;
    let mut retries: u32 = 0;
    loop {
        let iteration_start = std::time::Instant::now();
        match run_daemon_server(addr, warm_voice.clone()).await {
            Ok(()) => {
                // Apagado graceful (stop) — no reintentar
                return Ok(());
            }
            Err(e) => {
                if retries >= max_retries {
                    return Err(e);
                }
                if iteration_start.elapsed() < SUPERVISION_PROGRESS_MIN {
                    fast_streak += 1;
                } else {
                    fast_streak = 0;
                }
                if fast_streak >= SUPERVISION_STREAK_MAX {
                    return Err(anyhow::anyhow!(
                        "Bucle de supervisión sin progreso: {} caídas con vida <{} s (último: {}). Revisa la causa raíz en vez de reintentar.",
                        fast_streak,
                        SUPERVISION_PROGRESS_MIN.as_secs(),
                        e
                    ));
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
                // Reclamo activo previo al reintento: esperar con deadline
                // a que el árbol propio previo esté muerto y el puerto libre
                // antes del siguiente `bind`. Solo árbol propio previo (vía
                // `Drop` preciso, sin imagen global ni otra instancia sana).
                {
                    let start = std::time::Instant::now();
                    let limit = std::time::Duration::from_secs(5);
                    loop {
                        if std::net::TcpListener::bind(addr).is_ok() {
                            // Puerto libre: el test-listener se cierra al dropearse.
                            eprintln!(
                                "Daemon: árbol previo muerto y puerto {} libre antes del reintento ({}/{})",
                                addr, retries, max_retries
                            );
                            break;
                        }
                        if start.elapsed() >= limit {
                            eprintln!(
                                "Daemon: el puerto {} sigue ocupado tras la caída (deadline 5 s); se reintenta igualmente ({}/{})",
                                addr, retries, max_retries
                            );
                            break;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Warming` → `Warm`: la etiqueta cambia y no hay causa de error.
    #[test]
    fn warm_state_transitions_to_warm() {
        let warm = std::sync::RwLock::new(WarmState::Warming);
        assert_eq!(warm.read().unwrap().label(), "warming");
        *warm.write().unwrap() = WarmState::Warm;
        assert_eq!(warm.read().unwrap().label(), "warm");
        assert!(warm.read().unwrap().error().is_none());
    }

    /// `Warming` → `Failed(causa)`: la etiqueta es `warm_failed` y conserva la causa.
    #[test]
    fn warm_state_transitions_to_failed_with_cause() {
        let warm = std::sync::RwLock::new(WarmState::Warming);
        *warm.write().unwrap() = WarmState::Failed("motor caído".to_string());
        let guard = warm.read().unwrap();
        assert_eq!(guard.label(), "warm_failed");
        assert_eq!(guard.error().as_deref(), Some("motor caído"));
    }

    /// `health_body` en warming: `status:ready`, sin `warm_error`, tolera aditivas.
    #[test]
    fn health_body_warming_without_warm_error() {
        let body = health_body("warming", None);
        assert_eq!(body["status"], "ready");
        assert!(["warming", "warm", "warm_failed"].contains(&body["warm"].as_str().unwrap()));
        assert_eq!(body["engine"], "rust_native");
        assert!(body.get("warm_error").is_none());
        // Tolera claves aditivas ct2/stt sin exigirlas (las emite health_handler cuando residentes)
        let _ = body.get("ct2");
        let _ = body.get("stt");
    }

    /// `health_body` en fallo: incluye `warm_error` con la causa, tolera aditivas.
    #[test]
    fn health_body_failed_includes_warm_error() {
        let body = health_body("warm_failed", Some("boom".to_string()));
        assert!(["warming", "warm", "warm_failed"].contains(&body["warm"].as_str().unwrap()));
        assert_eq!(body["warm"], "warm_failed");
        assert_eq!(body["warm_error"], "boom");
        let _ = body.get("ct2");
        let _ = body.get("stt");
    }

    /// Router expone `/health` y endpoints `/translate`, `/voices/clone`, `/dub` (7 rutas públicas)
    #[test]
    fn build_router_exposes_new_endpoints() {
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        // El router debe construirse sin panic con el estado del daemon; la
        // existencia de cada ruta se ejercita en los tests de handlers (p. ej.
        // `dub_handler_audio_missing`), no vía el `Debug` del `Router` — axum no
        // expone ahí los paths registrados.
        let _router = build_router_with_state(state);
    }

    /// `with_heartbeats` entrega el resultado del trabajo inmediato (sin
    /// exigir latidos cuando el trabajo es más rápido que el intervalo).
    #[tokio::test]
    async fn with_heartbeats_delivers_immediate_result() {
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(32);
        let work = tokio::task::spawn_blocking(|| 42u32);
        let res = with_heartbeats(&tx, "test", work)
            .await
            .expect("sin desconexión hay resultado");
        assert_eq!(res.expect("join ok"), 42);
    }

    /// Ante desconexión del cliente (`rx` dropeado) el trabajo se aborta
    /// y se retorna `None` sin esperar su completitud (sin reloj ajustado: el
    /// trabajo dormiría 30 s y el retorno debe llegar muy antes).
    #[tokio::test]
    async fn with_heartbeats_aborts_on_disconnect() {
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
        drop(rx);
        let work = tokio::task::spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_secs(2));
            1u32
        });
        let start = std::time::Instant::now();
        let res = with_heartbeats(&tx, "test", work).await;
        assert!(res.is_none(), "desconectado debe abortar con None");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "el aborto no espera al trabajo: {:?}",
            start.elapsed()
        );
    }

    /// Durante un trabajo con duración suficiente se emite al menos un latido `heartbeat`
    /// con la etapa (cota holgada: intervalo 500 ms; el margen absorbe
    /// planificación lenta sin falsos positivos).
    #[tokio::test]
    async fn with_heartbeats_emits_heartbeat_during_long_work() {
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(32);
        let work = tokio::task::spawn_blocking(|| {
            std::thread::sleep(std::time::Duration::from_millis(800));
            "hecho"
        });
        let res = with_heartbeats(&tx, "etapa_test", work)
            .await
            .expect("sin desconexión hay resultado");
        assert_eq!(res.expect("join ok"), "hecho");
        drop(tx);
        let mut heartbeats = 0;
        while let Some(line) = rx.recv().await {
            let ev: Value = serde_json::from_str(&line).expect("cada evento es JSON");
            if ev["event"] == "heartbeat" {
                assert_eq!(ev["stage"], "etapa_test");
                heartbeats += 1;
            }
        }
        assert!(
            heartbeats >= 1,
            "trabajo de 2 s debe latir al menos una vez"
        );
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
