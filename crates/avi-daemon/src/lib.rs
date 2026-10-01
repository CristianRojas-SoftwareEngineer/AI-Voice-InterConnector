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
/// el daemon sirve en cuanto enlaza; `warm` refleja el warmup en curso o el
/// resultado de la última síntesis completada (testigo o petición). Transiciones:
/// un warmup pasa a `Warming` y termina en `Warm` (éxito) o `Failed(causa)` (fallo,
/// que degrada pero no derriba el daemon: la primera petición paga cold-start);
/// cualquier síntesis correcta posterior devuelve el estado a `Warm`.
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

    /// Constructor con los almacenes de voces y de habla ya anclados, para que
    /// pruebas y sandboxes no dependan del directorio de datos del usuario.
    pub fn with_stores(voice_store: VoiceStore, speech_store: SpeechStore) -> anyhow::Result<Self> {
        let _ = (voice_store, speech_store);
        Self::new()
    }

    /// Marca el motor como en calentamiento (`Warm`/`Failed` → `Warming`) cuando un
    /// warmup empieza a sintetizar su testigo.
    pub fn set_warming(&self) {
        *self.warm.write().unwrap() = WarmState::Warming;
    }

    /// Marca el motor como caliente tras un warmup o una síntesis correctos.
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
/// JSON directamente.
///
/// **El protocolo del daemon tiene su propia versión**, `DAEMON_SCHEMA_VERSION`, que
/// este ciclo no cambia: el emisor compartido de `avi-core` lo comparte con el sobre `--json`
/// de la CLI, pero los dos son contratos independientes y sus versiones se gobiernan por
/// separado. Por eso la versión se pasa explícitamente en vez de leerse de una
/// constante única: subir la de la CLI no puede arrastrar a esta.
fn with_sv(val: Value) -> Value {
    json_emitter::with_schema_version(val, json_emitter::DAEMON_SCHEMA_VERSION)
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

/// Margen que el deadline de la fase de síntesis añade al presupuesto del
/// texto: el timeout tipado del residente (igual al presupuesto) llega primero
/// y este deadline solo respalda un residente que no responde ni expira.
const SYNTHESIS_DEADLINE_MARGIN: std::time::Duration = std::time::Duration::from_secs(2);

/// Activa la bandera de cancelación de un trabajo de síntesis al soltarse.
/// Cubre cualquier salida de la fase (plazo vencido, cliente desconectado o
/// future descartado), así que ningún camino deja al motor generando para
/// nadie: la bandera cierra la conexión y el motor aborta.
struct CancelOnDrop(Arc<std::sync::atomic::AtomicBool>);

impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Fase de síntesis compartida por `/synthesize` y `/dub`.
///
/// 1. Espera `synthesis_lock` emitiendo latidos: `stage: "warming"` si el
///    daemon está calentando el motor, `queued` si otra síntesis retiene el lock.
/// 2. Ejecuta `job` en `spawn_blocking` con latidos `synthesis`, acotado a
///    `budget` más `SYNTHESIS_DEADLINE_MARGIN`. `job` recibe una bandera de
///    cancelación que se activa al vencer ese deadline, al desconectarse el
///    cliente o al descartarse la fase; el motor aborta entonces la
///    generación y queda caliente para la siguiente petición.
/// 3. Si vence el deadline o `job` devuelve un error que envuelve
///    `SynthesisTimeout`, emite `{event:"error", reason:"synthesis_timeout"}`.
///    Una síntesis correcta marca el motor como `warm`.
///
/// Devuelve `Some(resultado)` cuando el trabajo terminó (éxito o fallo propio,
/// incluido el fallo del hilo), y `None` cuando la fase ya no tiene nada que
/// entregar: el cliente se desconectó (el trabajo se cancela) o venció el
/// presupuesto (el evento de error ya se emitió). El llamante solo retorna.
async fn run_synthesis_phase<T>(
    tx: &tokio::sync::mpsc::Sender<String>,
    state: &DaemonState,
    budget: std::time::Duration,
    job: impl FnOnce(Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<T> + Send + 'static,
) -> Option<anyhow::Result<T>>
where
    T: Send + 'static,
{
    run_synthesis_phase_with(
        tx,
        state,
        budget,
        STREAM_HEARTBEAT,
        SYNTHESIS_DEADLINE_MARGIN,
        job,
    )
    .await
}

/// Variante interna de `run_synthesis_phase` con intervalo de latido y margen
/// del deadline explícitos, para probar la fase con tiempos de milisegundos.
async fn run_synthesis_phase_with<T>(
    tx: &tokio::sync::mpsc::Sender<String>,
    state: &DaemonState,
    budget: std::time::Duration,
    heartbeat: std::time::Duration,
    margin: std::time::Duration,
    job: impl FnOnce(Arc<std::sync::atomic::AtomicBool>) -> anyhow::Result<T> + Send + 'static,
) -> Option<anyhow::Result<T>>
where
    T: Send + 'static,
{
    // Espera del lock con latidos; el estado de warmup decide la etapa.
    let _lock = {
        let lock = state.synthesis_lock.lock();
        tokio::pin!(lock);
        loop {
            tokio::select! {
                guard = &mut lock => break guard,
                _ = tokio::time::sleep(heartbeat) => {
                    let stage = if state.warm_snapshot().0 == "warming" {
                        "warming"
                    } else {
                        "queued"
                    };
                    emit_ndjson(tx, json!({ "event": "heartbeat", "stage": stage })).await;
                }
                _ = tx.closed() => return None,
            }
        }
    };

    // Un cliente que ya se fue mientras esperaba no justifica arrancar trabajo.
    if tx.is_closed() {
        return None;
    }

    let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let job_cancel = cancel.clone();
    let mut handle = tokio::task::spawn_blocking(move || job(job_cancel));
    let deadline = tokio::time::sleep(budget + margin);
    tokio::pin!(deadline);
    let outcome = loop {
        tokio::select! {
            _ = tokio::time::sleep(heartbeat) => {
                emit_ndjson(tx, json!({ "event": "heartbeat", "stage": "synthesis" })).await;
            }
            _ = tx.closed() => return None,
            res = &mut handle => break Some(res),
            _ = &mut deadline => {
                cancel.store(true, std::sync::atomic::Ordering::SeqCst);
                break None;
            }
        }
    };

    let result = match outcome {
        Some(Ok(res)) => res,
        Some(Err(join_err)) => Err(anyhow::anyhow!("El hilo de síntesis falló: {}", join_err)),
        None => Err(anyhow::Error::new(avi_tts::SynthesisTimeout { budget })),
    };
    match &result {
        Ok(_) => state.set_warm(),
        // Solo ocurre cuando la fase ya se abandonó: no hay nada que entregar.
        Err(e) if e.downcast_ref::<avi_tts::SynthesisCancelled>().is_some() => return None,
        Err(e) if e.downcast_ref::<avi_tts::SynthesisTimeout>().is_some() => {
            emit_ndjson(
                tx,
                json!({
                    "event": "error",
                    "reason": "synthesis_timeout",
                    "message": format!(
                        "La síntesis superó su presupuesto de {} s y se canceló; el motor queda disponible para la siguiente petición.",
                        budget.as_secs()
                    ),
                }),
            )
            .await;
            return None;
        }
        Err(_) => {}
    }
    Some(result)
}

// ─── Handlers ────────────────────────────────────────────────────────────

/// Construye el cuerpo de `/health` a partir del estado de warmup (el warmup en
/// curso o el resultado de la última síntesis completada). Función pura
/// (testeable sin daemon): emite `{status:"ready", warm, engine}` y añade
/// `warm_error` solo cuando el último warmup falló. Notas aditivas `ct2`/`stt`
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

    // Validación de texto (vacío o demasiado largo): se evalúa antes del motor y
    // devuelve un 400 con cuerpo JSON plano (no stream).
    if let Err(e) = avi_core::validate_synthesis_text(&text) {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "error": e.reason,
                "reason": e.reason,
                "message": e.message,
            }))),
        )
            .into_response();
    }

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    let state = state.clone();
    let text_owned = text.clone();
    let voice_owned = voice.clone();
    let source_owned = source_raw.clone();
    let target_owned = target_raw.clone();

    tokio::spawn(async move {
        // `start` sale antes de esperar `synthesis_lock`: la espera en cola la
        // cubre `run_synthesis_phase` con latidos, así que el cliente nunca queda
        // en silencio mientras otra síntesis o el warmup retienen el lock.
        emit_ndjson(
            &tx,
            json!({
                "event": "start",
                "voice": voice_owned,
                "text_length": text_owned.chars().count(),
            }),
        )
        .await;

        // Temperatura opcional del CLI (ya validada allí); aquí se defiende el
        // rango para payloads directos al HTTP. Va antes de la comprobación del
        // modelo: una invocación mal formada es un error de uso aunque el motor
        // no esté provisionado.
        if let Some(t) = temperature {
            if !(t > 0.0 && t <= 2.0) {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "usage_error",
                        "message": "--temperature debe ser mayor que 0 y como máximo 2.0.",
                    }),
                )
                .await;
                return;
            }
        }

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
        // La síntesis sobre el residente es síncrona y puede colgarse; la fase
        // espera el lock con latidos y acota el trabajo al presupuesto del texto
        // que realmente se sintetiza (ya traducido). Al vencer o irse el
        // cliente, la fase cancela el trabajo: el motor aborta y queda caliente
        // para la siguiente petición.
        let budget = avi_core::synthesis_budget(text_final.chars().count());
        let state_synth = state.clone();
        let job = move |cancel: Arc<std::sync::atomic::AtomicBool>| {
            // Reloj de trabajo tomado dentro del trabajo, ya con el lock: mide
            // la síntesis pura y excluye la espera en cola.
            let work_t0 = std::time::Instant::now();
            state_synth
                .tts_engine
                .synthesize_cancellable(&text_final, &profile, &options, Some(&tmp), &cancel)
                .map(|path| (path, work_t0.elapsed()))
        };
        let Some(synth_res) = run_synthesis_phase(&tx, &state, budget, job).await else {
            return;
        };
        match synth_res {
            Ok((path, work_elapsed)) => {
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
                                "work_ms": work_elapsed.as_millis() as u64,
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
        .header("x-schema-version", json_emitter::DAEMON_SCHEMA_VERSION)
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
    let pcm = match validate_transcribe_input(&payload) {
        Ok(pcm) => pcm,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": e.reason,
                    "message": e.message,
                }))),
            )
                .into_response();
        }
    };

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
/// exige Parakeet, con la misma paridad que aplica el binario.
#[cfg(feature = "native-stt")]
fn resolve_stt_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
}

/// Normaliza token de idioma para traducción (`es-latam`→`es`), paridad con
/// `resolve_stt_language` de este crate y `avi_translation`.
fn resolve_translation_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
}

/// Error de validación de una petición: `reason` es el código del contrato y
/// `message` el texto para el usuario.
#[derive(Debug, PartialEq)]
pub struct InputError {
    pub reason: &'static str,
    pub message: String,
}

/// Resultado de validar una petición de traducción.
#[derive(Debug, PartialEq)]
pub enum TranslateInput {
    /// Idioma de origen y destino coinciden: se devuelve el texto sin traducir.
    Same,
    /// Par distinto y soportado, con el texto y los idiomas ya resueltos a ISO.
    Pair {
        text: String,
        source: String,
        target: String,
    },
}

/// Valida el cuerpo de `/transcribe` y devuelve el PCM i16 decodificado.
pub fn validate_transcribe_input(body: &Value) -> Result<Vec<i16>, InputError> {
    let audio_b64 = body
        .get("audio_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| InputError {
            reason: "usage_error",
            message:
                "La petición no incluye el campo 'audio_b64' (PCM int16 little-endian 16 kHz mono)."
                    .to_string(),
        })?;

    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .map_err(|e| InputError {
            reason: "invalid_audio",
            message: format!("audio_b64 no decodificable como base64: {}", e),
        })?;

    // PCM i16 little-endian → Vec<i16> mono 16 kHz (el motor normaliza a i16::MAX).
    Ok(audio_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect())
}

/// Extrae de la petición de traducción el texto y los idiomas sin resolver
/// (`from`/`source` por defecto `es`; `to`/`target` por defecto `en`).
fn translate_request_fields(body: &Value) -> (String, &str, &str) {
    let text = body
        .get("text")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let from_raw = body
        .get("from")
        .or_else(|| body.get("source"))
        .and_then(|v| v.as_str())
        .unwrap_or("es");
    let to_raw = body
        .get("to")
        .or_else(|| body.get("target"))
        .and_then(|v| v.as_str())
        .unwrap_or("en");
    (text, from_raw, to_raw)
}

/// Valida el cuerpo de `/translate`: texto no vacío y par de idiomas soportado.
pub fn validate_translate_input(body: &Value) -> Result<TranslateInput, InputError> {
    let (text, from_raw, to_raw) = translate_request_fields(body);
    if text.trim().is_empty() {
        return Err(InputError {
            reason: "empty_text",
            message: "El texto a traducir está vacío".to_string(),
        });
    }
    let source = resolve_translation_language(from_raw).to_string();
    let target = resolve_translation_language(to_raw).to_string();
    if source == target {
        return Ok(TranslateInput::Same);
    }
    match (source.as_str(), target.as_str()) {
        ("es", "en") | ("en", "es") => Ok(TranslateInput::Pair {
            text,
            source,
            target,
        }),
        _ => Err(InputError {
            reason: "unsupported_language_pair",
            message: format!(
                "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                source, target
            ),
        }),
    }
}

/// POST /translate — traducción texto→texto con CT2 residente
#[cfg(feature = "native-translation")]
async fn translate_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    let (text, from_raw, to_raw) = translate_request_fields(&payload);
    let (source, target) = match validate_translate_input(&payload) {
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "error": e.reason,
                    "reason": e.reason,
                    "message": e.message,
                }))),
            )
                .into_response();
        }
        Ok(TranslateInput::Same) => {
            return Json(with_sv(json!({
                "translated": text,
                "source": from_raw,
                "target": to_raw,
            })))
            .into_response();
        }
        Ok(TranslateInput::Pair { source, target, .. }) => (source, target),
    };
    let pair = if source == "es" { "es-en" } else { "en-es" };
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
                    "error": "usage_error",
                    "reason": "usage_error",
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
                    "error": "invalid_audio",
                    "reason": "invalid_audio",
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
                // Una referencia ilegible se clasifica igual que en transcripción.
                let reason = match e.downcast_ref::<avi_audio::WavLoadError>() {
                    Some(avi_audio::WavLoadError::Invalid(_)) => "invalid_audio",
                    Some(avi_audio::WavLoadError::Io(_)) => "io_error",
                    _ => "voice_clone_failed",
                };
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "name": event_name,
                        "reason": reason,
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
            let _ = warm_voice_engine(&warm_state, &warm_name);
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
        .header("x-schema-version", json_emitter::DAEMON_SCHEMA_VERSION)
        .body(body)
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}

/// POST /dub — pipeline voz→voz (transcribe→translate→synthesize), servido
/// como stream NDJSON con latidos: `started` tras las validaciones
/// baratas (audio, par/CT2, modelo, voz y rama sin `native-stt`, en JSON plano
/// con los códigos de siempre), latidos `heartbeat` durante cada fase pesada y
/// evento final `result` con la forma contractual actual (`status: "dubbed"`).
/// La fase de síntesis la gobierna `run_synthesis_phase`: latidos en cola,
/// presupuesto proporcional al texto y cancelación del trabajo en el motor al
/// vencer o al desconectarse el cliente.
async fn dub_handler(State(state): State<SharedState>, Json(payload): Json<Value>) -> Response {
    let audio_b64 = match payload.get("audio_b64").and_then(|v| v.as_str()) {
        Some(s) => s,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "usage_error",
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
                    "reason": "invalid_audio",
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
    // Duración máxima del audio (PCM 16 kHz mono): se rechaza antes de arrancar
    // cualquier trabajo pesado.
    #[cfg(feature = "native-stt")]
    if pcm.len() as u64 > avi_core::MAX_DUB_AUDIO_SECS * 16_000 {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "status": "error",
                "reason": "audio_too_long",
                "message": format!(
                    "El audio dura más de {} s, el máximo admitido para doblar.",
                    avi_core::MAX_DUB_AUDIO_SECS
                ),
            }))),
        )
            .into_response();
    }
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
            // El texto final (transcrito o traducido) debe respetar el tope de
            // síntesis; se valida antes de tomar `synthesis_lock`.
            if let Err(e) = avi_core::validate_synthesis_text(&final_text) {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": e.reason,
                        "message": e.message,
                    }),
                )
                .await;
                return;
            }
            // Síntesis: la fase espera `synthesis_lock` con latidos (`warming` o
            // `queued`), acota el trabajo al presupuesto del texto, lo cancela
            // en el motor ante desconexión del cliente o vencimiento y, al
            // vencer, emite `synthesis_timeout` (nunca cierre silencioso).
            let profile = VoiceProfile {
                name: voice.clone(),
                qvoice_path: state.voice_store.find_reference(&voice),
            };
            let tmp =
                std::env::temp_dir().join(format!("avi_daemon_dub_{}.wav", std::process::id()));
            let budget = avi_core::synthesis_budget(final_text.chars().count());
            let text_synth = final_text.clone();
            let synth_state = state.clone();
            let synth_options = GenerationOptions::with_temperature(temperature);
            let job = move |cancel: Arc<std::sync::atomic::AtomicBool>| {
                // Reloj de trabajo tomado dentro del trabajo, ya con el lock: mide
                // solo la fase de síntesis, no transcribe/translate ni la cola.
                let work_t0 = std::time::Instant::now();
                synth_state
                    .tts_engine
                    .synthesize_cancellable(
                        &text_synth,
                        &profile,
                        &synth_options,
                        Some(&tmp),
                        &cancel,
                    )
                    .map(|path| (path, work_t0.elapsed()))
            };
            let Some(synth_res) = run_synthesis_phase(&tx, &state, budget, job).await else {
                return;
            };
            match synth_res {
                Ok((path, work_elapsed)) => {
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
                                    "work_ms": work_elapsed.as_millis() as u64,
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

        // Convertir el receptor en un stream NDJSON (mismo patrón que `synthesize_handler`).
        let stream = tokio_stream::wrappers::ReceiverStream::new(rx);
        let body = Body::from_stream(tokio_stream::StreamExt::map(stream, |line| {
            Ok::<_, std::convert::Infallible>(format!("{}\n", line))
        }));

        Response::builder()
            .header("content-type", "application/x-ndjson")
            .header("x-schema-version", json_emitter::DAEMON_SCHEMA_VERSION)
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
/// corre en `spawn_blocking`); en warm-on-clone el lock puede estar disputado
/// por tráfico vivo. Con el lock tomado marca `Warming` y al terminar deja
/// `Warm` o `Failed` con la causa. Su plazo es el arranque del residente más el
/// presupuesto del testigo: al vencer, el motor cancela el testigo y queda
/// caliente para la siguiente petición.
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
    state.set_warming();
    // Reloj de trabajo tomado tras el lock: mide warmup puro y excluye la espera
    // en cola contra tráfico vivo.
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
    match &result {
        Ok(_) => state.set_warm(),
        Err(e) => state.set_warm_failed(e.to_string()),
    }
    result.map(|_| ())
}

/// Nombre de la env interna que transporta la ruta del fichero ready dentro
/// del proceso `serve`: el flag `--ready-file` la fija el manejador del binario y
/// `run_daemon_server` la consume. No es contrato público:
/// el padre solo conoce el flag y el fichero resultante.
pub const READY_FILE_ENV: &str = "AVI_READY_FILE";

/// Fallo del arranque ocurrido antes de que el daemon esté listo (antes de
/// publicar el registro de éxito). Clasifica la causa para que el llamante la
/// traduzca a un código de salida propio, y la supervisión lo trata como
/// terminal: es un fallo de configuración o de recursos que un reintento no
/// arregla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupError {
    /// El puerto pedido está en uso o reservado por otro proceso.
    PortInUse { port: u16 },
    /// La voz de `--warm-voice` no existe en el almacén de voces.
    WarmVoiceMissing { voice: String },
    /// Cualquier otro fallo previo a estar listo, con su causa legible.
    Failed { message: String },
}

impl std::fmt::Display for StartupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StartupError::PortInUse { port } => write!(
                f,
                "El puerto {} está en uso o reservado por otro proceso: libéralo o arranca el daemon en otro puerto con AVI_DAEMON_PORT=<puerto> (0 = efímero).",
                port
            ),
            StartupError::WarmVoiceMissing { voice } => write!(
                f,
                "La voz de warmup '{}' no existe: clónala o elige otra con --warm-voice.",
                voice
            ),
            StartupError::Failed { message } => f.write_str(message),
        }
    }
}

impl std::error::Error for StartupError {}

/// Señal leída del fichero ready: el daemon está listo en `addr` (con el PID
/// que publicó, si es legible) o falló antes de estarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadySignal {
    Ready { addr: String, pid: Option<u32> },
    Failed(StartupError),
}

// El fichero ready admite dos registros, ambos `clave=valor` por línea:
// - éxito: `addr=<dirección real>`, `warm=<estado>` y `pid=<pid del daemon>`;
// - fallo: `error=port_in_use` + `port=`, `error=warm_voice_missing` + `voice=`
//   o `error=startup_failed` + `message=`.
// El registro de fallo nunca lleva `addr=`: los lectores toman `addr=` como
// señal de listo, y uno que no conozca `error=` debe seguir viendo aún-no-listo.

/// Publica `content` en el fichero ready con escritura atómica (temporal
/// hermano + rename), de modo que un lector nunca ve un registro a medias.
/// Best-effort con diagnóstico: un fallo de señalización no derriba el daemon
/// (el evento en stderr sigue valiendo como diagnóstico redundante).
fn publish_ready_record(path: &std::path::Path, content: &str) {
    let tmp = path.with_extension("ready.tmp");
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

/// Publica el registro de éxito: `addr` + `warm` + el PID propio.
fn write_ready_file(path: &std::path::Path, addr: &SocketAddr, warm: &str) {
    // El PID propio permite reclamar el árbol si el padre cae sin escribir o
    // sin limpiar el pidfile: con puertos efímeros el ready es entonces la
    // única pista de qué proceso matar.
    let content = format!("addr={}\nwarm={}\npid={}\n", addr, warm, std::process::id());
    publish_ready_record(path, &content);
}

/// Publica el registro de fallo de un arranque que no llegó a estar listo.
fn write_ready_failure(path: &std::path::Path, err: &StartupError) {
    let content = match err {
        StartupError::PortInUse { port } => format!("error=port_in_use\nport={}\n", port),
        StartupError::WarmVoiceMissing { voice } => {
            format!("error=warm_voice_missing\nvoice={}\n", voice)
        }
        // El formato es de una línea por clave: los saltos del mensaje se
        // aplanan para que no corten el registro.
        StartupError::Failed { message } => format!(
            "error=startup_failed\nmessage={}\n",
            message.replace(['\r', '\n'], " ")
        ),
    };
    publish_ready_record(path, &content);
}

/// Lee el fichero ready. Devuelve `None` si no existe o aún no contiene un
/// registro completo (el llamante sigue esperando). `error=` tiene prioridad
/// sobre `addr=`; una clase de error desconocida se entrega como `Failed` con
/// la clase en el mensaje, y un `pid=` ausente, ilegible o `0` da `pid: None`.
pub fn read_ready_signal(path: &std::path::Path) -> Option<ReadySignal> {
    let content = std::fs::read_to_string(path).ok()?;
    let field = |key: &str| {
        content.lines().find_map(|line| {
            line.strip_prefix(key)
                .and_then(|rest| rest.strip_prefix('='))
                .map(|v| v.trim().to_string())
        })
    };
    if let Some(class) = field("error") {
        let err = match class.as_str() {
            "port_in_use" => match field("port").and_then(|p| p.parse().ok()) {
                Some(port) => StartupError::PortInUse { port },
                None => return None,
            },
            "warm_voice_missing" => StartupError::WarmVoiceMissing {
                voice: field("voice")?,
            },
            "startup_failed" => StartupError::Failed {
                message: field("message")?,
            },
            other => StartupError::Failed {
                message: format!(
                    "el daemon publicó un fallo de arranque desconocido: {}",
                    other
                ),
            },
        };
        return Some(ReadySignal::Failed(err));
    }
    let addr = field("addr").filter(|a| !a.is_empty())?;
    let pid = field("pid")
        .and_then(|p| p.parse::<u32>().ok())
        .filter(|&p| p != 0);
    Some(ReadySignal::Ready { addr, pid })
}

/// Fase previa a servir, ordenada de lo barato a lo caro para fallar antes de
/// pagar la carga de modelos: valida la voz de warmup, enlaza el puerto y solo
/// entonces construye el estado. Todo fallo sale clasificado como
/// `StartupError`.
async fn prepare_server(
    addr: SocketAddr,
    warm_voice: &str,
) -> Result<(TcpListener, SocketAddr, Arc<DaemonState>), StartupError> {
    // Habilitador: materializar las voces de fábrica en la instancia
    // (idempotente, desde el asset embebido). Sin esto, un `data_dir` virgen
    // (sandbox de estado por instancia vía `AVI_DATA_DIR`) rechazaría
    // `--warm-voice default` porque `default` aún no existe en disco. Tras
    // esto, una voz inexistente se rechaza igual (no es de fábrica).
    let voices = VoiceStore::new();
    voices
        .ensure_initialized()
        .map_err(|e| StartupError::Failed {
            message: format!("No se pudo inicializar el almacén de voces: {}", e),
        })?;

    // Fail-fast: una `--warm-voice` inexistente aborta el arranque antes de
    // enlazar y de cargar modelos, sin degradar en silencio ni caer a `default`.
    if voices.find_reference(warm_voice).is_none() {
        return Err(StartupError::WarmVoiceMissing {
            voice: warm_voice.to_string(),
        });
    }

    // El puerto se enlaza antes de cargar los modelos: un puerto ocupado falla
    // en milisegundos y el puerto queda reservado durante la carga. Mientras
    // tanto no se sirve: una conexión espera en el backlog hasta `axum::serve`
    // y el registro de éxito solo se publica tras cargar el estado. En Windows,
    // WSAEACCES (10013) es el bind contra un puerto retenido en uso exclusivo o
    // en un rango reservado: el remedio es el mismo que el de un puerto en uso.
    // Sin reclamo aquí; el reclamo del árbol propio previo entre reintentos
    // vive solo en `run_supervised`.
    let listener = TcpListener::bind(addr).await.map_err(|e| {
        let reserved = cfg!(windows) && e.raw_os_error() == Some(10013);
        if e.kind() == std::io::ErrorKind::AddrInUse || reserved {
            StartupError::PortInUse { port: addr.port() }
        } else {
            StartupError::Failed {
                message: format!("No se pudo enlazar {}: {}", addr, e),
            }
        }
    })?;
    // Puerto efímero por instancia: publicar la dirección REALMENTE enlazada
    // (con `:0` el SO asigna, nunca el literal pedido).
    let bound = listener.local_addr().map_err(|e| StartupError::Failed {
        message: format!("No se pudo leer la dirección enlazada: {}", e),
    })?;

    let state = DaemonState::new().map_err(|e| StartupError::Failed {
        message: format!("No se pudo inicializar el estado del daemon: {}", e),
    })?;
    Ok((listener, bound, Arc::new(state)))
}

/// Inicia el daemon nativo escuchando en `addr`. Valida la voz de warmup,
/// enlaza el listener y construye el estado (en ese orden); todo fallo de esa
/// fase se devuelve como `StartupError` sin escribir el fichero ready (lo
/// publica `run_supervised`, que decide si es terminal). Después anuncia la
/// dirección, publica el registro de éxito y comienza a servir; el warmup TTS
/// corre en segundo plano (`spawn_blocking`), acotado por el arranque del
/// residente más el presupuesto del testigo. Readiness (enlazado + motor
/// construido) queda así desacoplado del pre-calentamiento: un warmup fallido
/// o vencido degrada —pero no derriba— el daemon.
pub async fn run_daemon_server(addr: SocketAddr, warm_voice: String) -> anyhow::Result<()> {
    let (listener, bound, state) = prepare_server(addr, &warm_voice)
        .await
        .map_err(anyhow::Error::new)?;
    let app = build_router_with_state(state.clone());
    println!("Daemon nativo escuchando en http://{}", bound);

    // Transporte flag+fichero: si el padre designó fichero ready
    // (`--ready-file`), publicar la `addr` real tras enlazar y cargar el
    // estado, y el estado warm tras el warmup. Escritura atómica; el evento en
    // stderr se conserva como diagnóstico redundante. Sin flag no se escribe
    // nada.
    let ready_file: Option<std::path::PathBuf> =
        std::env::var_os(READY_FILE_ENV).map(std::path::PathBuf::from);
    if let Some(ref path) = ready_file {
        write_ready_file(path, &bound, "warming");
    }

    // Warmup en segundo plano: `synthesize` es síncrono, por lo que corre en
    // `spawn_blocking` para no bloquear el runtime async del servidor.
    // `warm_voice_engine` gestiona el estado `warm` y queda acotado por el
    // arranque del residente más el presupuesto del testigo; un fallo o
    // vencimiento deja `warm_failed` con la causa y no aborta el arranque.
    // Readiness por señal: el arranque emite el evento "ligado + warm"
    // (puerto real + estado) tras bind y warmup. El fichero ready (arriba) es
    // el transporte que consumen los llamantes con espera acotada; la línea
    // `avi-daemon-ready warm=<estado> addr=<real>` en stderr es diagnóstico
    // redundante (stdout queda reservado al anuncio de ligado).
    let warm_state = state.clone();
    let ready_ok = ready_file.clone();
    tokio::task::spawn_blocking(move || {
        let warm = match warm_voice_engine(&warm_state, &warm_voice) {
            Ok(()) => "warm",
            Err(_) => "warm_failed",
        };
        eprintln!("avi-daemon-ready warm={} addr={}", warm, bound);
        if let Some(path) = ready_ok.as_ref() {
            write_ready_file(path, &bound, warm);
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

/// Si `err` es un fallo previo a estar listo y el padre designó fichero ready,
/// publica en él el registro de fallo para que el padre salga con su causa.
fn publish_startup_failure(err: &anyhow::Error) {
    if let (Some(startup), Some(path)) = (
        err.downcast_ref::<StartupError>(),
        std::env::var_os(READY_FILE_ENV),
    ) {
        write_ready_failure(std::path::Path::new(&path), startup);
    }
}

/// Ejecuta el daemon con supervisión configurable de reinicios.
///
/// Un fallo previo a estar listo (`StartupError`: puerto ocupado, voz de
/// warmup inexistente, fallo al cargar el estado) antes de que el daemon haya
/// servido alguna vez es de configuración: se publica en el fichero ready y se
/// devuelve sin reintentar, en ambos modos. La supervisión solo recupera
/// caídas de un daemon que llegó a servir.
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
        let result = run_daemon_server(addr, warm_voice).await;
        if let Err(ref e) = result {
            publish_startup_failure(e);
        }
        return result;
    }
    // Watchdog del bucle de supervisión (acotado a este bucle, primitivas
    // portables `Instant`): una vida del daemon menor a `SUPERVISION_PROGRESS_MIN`
    // cuenta como caída rápida (sin progreso); `SUPERVISION_STREAK_MAX` caídas
    // rápidas seguidas abortan con fallo ruidoso en vez de quemar `max_retries`
    // en un crash-loop silencioso. Sin falsos positivos en el camino feliz: el
    // apagado graceful retorna `Ok` antes de contar, y una vida larga resetea
    // la racha (la patología de cola de los tests la corrige el reloj tras
    // locks del harness, no este watchdog). Los fallos previos a estar listo
    // no llegan a este conteo: se devuelven antes (ver abajo).
    let mut fast_streak: u32 = 0;
    let mut retries: u32 = 0;
    // Verdadero en cuanto una iteración llegó a estar lista: un error que no
    // es `StartupError` solo puede salir de un servidor que ya servía.
    let mut ready_once = false;
    loop {
        let iteration_start = std::time::Instant::now();
        match run_daemon_server(addr, warm_voice.clone()).await {
            Ok(()) => {
                // Apagado graceful (stop) — no reintentar
                return Ok(());
            }
            Err(e) => {
                if e.downcast_ref::<StartupError>().is_none() {
                    ready_once = true;
                } else if !ready_once {
                    // Fallo de configuración antes de servir nunca: terminal.
                    // Si ya sirvió, se reintenta como una caída y no se
                    // publica nada: el registro de éxito conserva el `pid=` de
                    // este proceso, todavía vivo, y el reclamo de huérfanos
                    // lo necesita.
                    publish_startup_failure(&e);
                    return Err(e);
                }
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

    /// Latido corto y márgenes de milisegundos para probar la fase de síntesis.
    const TEST_HEARTBEAT: std::time::Duration = std::time::Duration::from_millis(20);

    /// Vacía los eventos ya emitidos y los devuelve parseados.
    fn drain_events(rx: &mut tokio::sync::mpsc::Receiver<String>) -> Vec<Value> {
        let mut events = Vec::new();
        while let Ok(line) = rx.try_recv() {
            events.push(serde_json::from_str(&line).expect("cada evento es JSON"));
        }
        events
    }

    fn heartbeats_with_stage(events: &[Value], stage: &str) -> usize {
        events
            .iter()
            .filter(|e| e["event"] == "heartbeat" && e["stage"] == stage)
            .count()
    }

    /// Mientras otra síntesis retiene el lock, la fase emite latidos `queued` y,
    /// al soltarse, ejecuta el trabajo y entrega su resultado.
    #[tokio::test]
    async fn synthesis_phase_emits_queued_heartbeats_while_lock_held() {
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        state.set_warm();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let guard = state.synthesis_lock.lock().await;
        let phase_state = state.clone();
        let phase = tokio::spawn(async move {
            run_synthesis_phase_with(
                &tx,
                &phase_state,
                std::time::Duration::from_secs(5),
                TEST_HEARTBEAT,
                std::time::Duration::from_secs(1),
                |_cancel| Ok(7u32),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drop(guard);
        let res = phase.await.expect("join").expect("hay resultado");
        assert_eq!(res.expect("el trabajo termina bien"), 7);
        let events = drain_events(&mut rx);
        assert!(heartbeats_with_stage(&events, "queued") >= 2);
        assert_eq!(heartbeats_with_stage(&events, "warming"), 0);
    }

    /// Si el daemon está calentando, la espera del lock late con `warming`.
    #[tokio::test]
    async fn synthesis_phase_emits_warming_stage_during_warmup() {
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        state.set_warming();
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let guard = state.synthesis_lock.lock().await;
        let phase_state = state.clone();
        let phase = tokio::spawn(async move {
            run_synthesis_phase_with(
                &tx,
                &phase_state,
                std::time::Duration::from_secs(5),
                TEST_HEARTBEAT,
                std::time::Duration::from_secs(1),
                |_cancel| Ok(1u32),
            )
            .await
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        drop(guard);
        assert!(phase.await.expect("join").is_some());
        let events = drain_events(&mut rx);
        assert!(heartbeats_with_stage(&events, "warming") >= 2);
        assert_eq!(heartbeats_with_stage(&events, "queued"), 0);
    }

    /// Un trabajo más largo que varios latidos, pero dentro del presupuesto,
    /// mantiene los latidos `synthesis` y termina con `Ok`.
    #[tokio::test]
    async fn synthesis_phase_slow_job_keeps_heartbeats_and_succeeds() {
        let state = DaemonState::new().expect("daemon state");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_secs(5),
            TEST_HEARTBEAT,
            std::time::Duration::from_secs(1),
            |_cancel| {
                std::thread::sleep(std::time::Duration::from_millis(150));
                Ok("listo")
            },
        )
        .await
        .expect("hay resultado");
        assert_eq!(res.expect("el trabajo termina bien"), "listo");
        let events = drain_events(&mut rx);
        assert!(heartbeats_with_stage(&events, "synthesis") >= 3);
    }

    /// Espera en el hilo del trabajo hasta ver la bandera de cancelación (con un
    /// tope de 2 s) y deja constancia en `seen` de si la vio.
    fn wait_for_cancel(
        cancel: &std::sync::atomic::AtomicBool,
        seen: &std::sync::atomic::AtomicBool,
    ) {
        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_secs(2) {
            if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                seen.store(true, std::sync::atomic::Ordering::SeqCst);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    /// Espera (con tope) a que el trabajo registre que vio la cancelación.
    async fn assert_job_saw_cancel(seen: &std::sync::atomic::AtomicBool) {
        let start = std::time::Instant::now();
        while !seen.load(std::sync::atomic::Ordering::SeqCst) {
            assert!(
                start.elapsed() < std::time::Duration::from_secs(1),
                "el trabajo debe ver la cancelación"
            );
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    }

    /// Al vencer el presupuesto la fase emite `synthesis_timeout` y activa la
    /// cancelación del trabajo, tanto por el deadline como por el timeout
    /// tipado que devuelve el propio trabajo.
    #[tokio::test]
    async fn synthesis_phase_timeout_cancels_job_and_emits_reason() {
        let state = DaemonState::new().expect("daemon state");

        // Deadline de la fase: el trabajo tarda más que presupuesto + margen.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_seen = seen.clone();
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_millis(50),
            TEST_HEARTBEAT,
            std::time::Duration::from_millis(50),
            move |cancel| {
                wait_for_cancel(&cancel, &job_seen);
                Ok(())
            },
        )
        .await;
        assert!(res.is_none());
        assert_job_saw_cancel(&seen).await;
        let events = drain_events(&mut rx);
        let error = events
            .iter()
            .find(|e| e["event"] == "error")
            .expect("hay evento de error");
        assert_eq!(error["reason"], "synthesis_timeout");
        assert!(error["message"].as_str().unwrap().contains("presupuesto"));

        // Timeout tipado del residente: llega antes que el deadline de la fase.
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let cancel_seen = Arc::new(std::sync::Mutex::new(None));
        let job_cancel_seen = cancel_seen.clone();
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_secs(5),
            TEST_HEARTBEAT,
            std::time::Duration::from_secs(1),
            move |cancel| {
                *job_cancel_seen.lock().unwrap() = Some(cancel);
                Err::<(), _>(anyhow::Error::new(avi_tts::SynthesisTimeout {
                    budget: std::time::Duration::from_secs(5),
                }))
            },
        )
        .await;
        assert!(res.is_none());
        let flag = cancel_seen
            .lock()
            .unwrap()
            .take()
            .expect("el trabajo corrió");
        assert!(flag.load(std::sync::atomic::Ordering::SeqCst));
        let events = drain_events(&mut rx);
        assert!(events
            .iter()
            .any(|e| e["event"] == "error" && e["reason"] == "synthesis_timeout"));
    }

    /// Una síntesis correcta devuelve el estado a `warm` aunque el último warmup
    /// hubiera fallado.
    #[tokio::test]
    async fn synthesis_phase_success_marks_warm() {
        let state = DaemonState::new().expect("daemon state");
        state.set_warm_failed("fallo previo".to_string());
        let (tx, _rx) = tokio::sync::mpsc::channel::<String>(64);
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_secs(5),
            TEST_HEARTBEAT,
            std::time::Duration::from_secs(1),
            |_cancel| Ok(()),
        )
        .await;
        assert!(res.expect("hay resultado").is_ok());
        let (label, error) = state.warm_snapshot();
        assert_eq!(label, "warm");
        assert!(error.is_none());
    }

    /// Si el cliente se desconecta, la fase retorna `None` sin esperar al
    /// trabajo, tanto en la cola como durante la síntesis, y en este último
    /// caso activa la cancelación del trabajo.
    #[tokio::test]
    async fn synthesis_phase_disconnect_cancels_job() {
        let state = Arc::new(DaemonState::new().expect("daemon state"));

        // Desconexión durante la espera del lock.
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        let guard = state.synthesis_lock.lock().await;
        drop(rx);
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_secs(5),
            TEST_HEARTBEAT,
            std::time::Duration::from_secs(1),
            |_cancel| -> anyhow::Result<()> { panic!("no debe arrancar el trabajo") },
        )
        .await;
        assert!(res.is_none());
        drop(guard);

        // Desconexión durante la síntesis: no espera al trabajo y lo cancela.
        let (tx, rx) = tokio::sync::mpsc::channel::<String>(64);
        let dropper = tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            drop(rx);
        });
        let seen = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let job_seen = seen.clone();
        let start = std::time::Instant::now();
        let res = run_synthesis_phase_with(
            &tx,
            &state,
            std::time::Duration::from_secs(30),
            TEST_HEARTBEAT,
            std::time::Duration::from_secs(1),
            move |cancel| {
                wait_for_cancel(&cancel, &job_seen);
                Ok(())
            },
        )
        .await;
        dropper.await.expect("join");
        assert!(res.is_none());
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
        assert_job_saw_cancel(&seen).await;
    }

    /// Dub sin `audio_b64` responde 400 con el `reason` de contrato `usage_error`
    #[tokio::test]
    async fn dub_handler_audio_missing() {
        let (status, body) = post_json("/dub", json!({"voice": "default"})).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["reason"], "usage_error");
    }

    /// Dub handler rechaza con 400 `audio_too_long` un audio que supera el tope
    #[cfg(feature = "native-stt")]
    #[tokio::test]
    async fn dub_handler_audio_too_long() {
        use axum::body::Body;
        use base64::Engine;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        let app = build_router_with_state(state);
        // 41 s de silencio PCM 16 kHz mono (i16 little-endian).
        let silence = vec![0u8; 41 * 16_000 * 2];
        let body = json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(silence),
        });
        let req = axum::http::Request::builder()
            .uri("/dub")
            .method(axum::http::Method::POST)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["reason"], "audio_too_long");
    }

    /// Una petición de transcripción sin `audio_b64` es un error de invocación.
    #[test]
    fn validate_transcribe_missing_audio_is_usage_error() {
        let result = validate_transcribe_input(&json!({ "source_language": "es" }));
        assert_eq!(result.err().map(|e| e.reason), Some("usage_error"));
    }

    /// Un `audio_b64` que no es base64 válido es audio ilegible.
    #[test]
    fn validate_transcribe_bad_base64_is_invalid_audio() {
        let result = validate_transcribe_input(&json!({ "audio_b64": "%%% no es base64 %%%" }));
        assert_eq!(result.err().map(|e| e.reason), Some("invalid_audio"));
    }

    /// Un texto vacío o solo de espacios no se traduce.
    #[test]
    fn validate_translate_empty_text_is_empty_text() {
        let result = validate_translate_input(&json!({ "text": "   ", "from": "es", "to": "en" }));
        assert_eq!(result.err().map(|e| e.reason), Some("empty_text"));
    }

    /// Con el mismo idioma de origen y destino no hay nada que traducir.
    #[test]
    fn validate_translate_same_language_is_same() {
        let result = validate_translate_input(&json!({ "text": "hola", "from": "es-latam", "to": "es" }));
        assert!(matches!(result, Ok(TranslateInput::Same)), "resultado: {result:?}");
    }

    /// Un par distinto de es↔en no está soportado.
    #[test]
    fn validate_translate_unsupported_pair_is_rejected() {
        let result = validate_translate_input(&json!({ "text": "hola", "from": "es", "to": "fr" }));
        assert_eq!(result.err().map(|e| e.reason), Some("unsupported_language_pair"));
    }

    /// `with_stores` ancla los almacenes bajo el directorio recibido: la raíz se
    /// afirma antes de inicializar para no escribir nunca en la instalación real.
    #[test]
    fn with_stores_anchors_stores_under_given_dir() {
        let tmp = std::env::temp_dir().join(format!("avi_daemon_stores_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let state = DaemonState::with_stores(
            VoiceStore::at(tmp.join("voices")),
            SpeechStore::at(tmp.join("speech")),
        )
        .expect("estado del daemon");
        assert!(
            state.voice_store.root().starts_with(&tmp),
            "raíz de voces fuera del temporal: {}",
            state.voice_store.root().display()
        );
        assert!(
            state.speech_store.root().starts_with(&tmp),
            "raíz de habla fuera del temporal: {}",
            state.speech_store.root().display()
        );
        state.voice_store.ensure_initialized().expect("inicializar voces");
        assert!(tmp.join("voices").join("default").is_dir());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// Envía un POST JSON al router y devuelve el estado y el cuerpo completo.
    async fn post_json(uri: &str, body: Value) -> (StatusCode, Vec<u8>) {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let state = Arc::new(DaemonState::new().expect("daemon state"));
        let app = build_router_with_state(state);
        let req = axum::http::Request::builder()
            .uri(uri)
            .method(axum::http::Method::POST)
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let resp = app.oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        (status, bytes.to_vec())
    }

    /// El evento `start` de `/synthesize` mide `text_length` en caracteres: «canción»
    /// tiene 7 caracteres y 8 bytes. La temperatura fuera de rango cierra el stream
    /// sin llegar al motor.
    #[tokio::test]
    async fn synthesize_start_event_counts_chars() {
        let (_, bytes) = post_json(
            "/synthesize",
            json!({ "text": "canción", "voice": "default", "temperature": 5.0 }),
        )
        .await;
        let body = String::from_utf8_lossy(&bytes);
        let start: Value = body
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|ev| ev["event"] == "start")
            .expect("el stream emite el evento start");
        assert_eq!(start["text_length"], 7);
    }

    /// Ningún mensaje del daemon lleva el prefijo que el cliente ya añade al
    /// imprimirlo: se revisa cada literal `"message"` del código fuente.
    #[test]
    fn no_message_literal_starts_with_error_prefix() {
        let prefix = format!("{}:", "Error");
        for line in include_str!("lib.rs").lines() {
            let Some((_, rest)) = line.split_once("\"message\":") else {
                continue;
            };
            let rest = rest.trim_start();
            let rest = rest.strip_prefix("format!(").unwrap_or(rest);
            assert!(
                !rest.starts_with(&format!("\"{prefix}")),
                "mensaje con prefijo duplicado: {line}"
            );
        }
    }

    /// Cada `reason` literal que emite el daemon (en `reason` o en su espejo `error`)
    /// está en la tabla única de códigos de salida: o la tabla le asigna un código
    /// propio, o es uno de los que el contrato deja en el 1 genérico.
    #[test]
    fn every_daemon_reason_is_in_exit_code_table() {
        const GENERIC: &[&str] = &[
            "io_error",
            "synthesis_failed",
            "synthesis_timeout",
            "stt_unsupported",
            "translation_unsupported",
            "voice_clone_failed",
            "daemon_error",
        ];
        let source = include_str!("lib.rs");
        let mut found = 0;
        for key in ["\"reason\": \"", "\"error\": \""] {
            for (i, _) in source.match_indices(key) {
                let rest = &source[i + key.len()..];
                let reason: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                    .collect();
                if reason.is_empty() || !rest[reason.len()..].starts_with('"') {
                    continue;
                }
                found += 1;
                assert!(
                    avi_core::exit_codes::ExitCode::from_reason(&reason)
                        != avi_core::exit_codes::ExitCode::Error
                        || GENERIC.contains(&reason.as_str()),
                    "el reason {reason} que emite el daemon no está en la tabla"
                );
            }
        }
        assert!(
            found > 0,
            "la búsqueda de literales no encontró ningún reason"
        );
    }

    /// Ruta de fichero ready en un directorio temporal propio de cada prueba.
    fn ready_path(test: &str) -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("avi_daemon_ready_{}_{}", test, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("crear directorio temporal");
        dir.join("daemon.ready")
    }

    /// Cada clase de fallo sobrevive la ida y vuelta por el fichero ready.
    #[test]
    fn ready_failure_round_trips_each_class() {
        let path = ready_path("failure_round_trip");
        for err in [
            StartupError::PortInUse { port: 8765 },
            StartupError::WarmVoiceMissing {
                voice: "nadie".to_string(),
            },
            StartupError::Failed {
                message: "motor caído".to_string(),
            },
        ] {
            write_ready_failure(&path, &err);
            assert_eq!(read_ready_signal(&path), Some(ReadySignal::Failed(err)));
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// El registro de éxito se lee como `Ready` con el PID propio y sin `error`.
    #[test]
    fn ready_success_reads_addr_and_pid() {
        let path = ready_path("success");
        let addr: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        write_ready_file(&path, &addr, "warming");
        assert_eq!(
            read_ready_signal(&path),
            Some(ReadySignal::Ready {
                addr: "127.0.0.1:8765".to_string(),
                pid: Some(std::process::id()),
            })
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Fichero ausente o vacío: aún no hay señal.
    #[test]
    fn ready_missing_or_empty_is_none() {
        let path = ready_path("missing");
        assert_eq!(read_ready_signal(&path), None);
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_ready_signal(&path), None);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Una clase desconocida se entrega como `Failed` con la clase en el mensaje.
    #[test]
    fn ready_unknown_failure_class_is_failed() {
        let path = ready_path("unknown_class");
        std::fs::write(&path, "error=cosa_nueva\n").unwrap();
        match read_ready_signal(&path) {
            Some(ReadySignal::Failed(StartupError::Failed { message })) => {
                assert!(message.contains("cosa_nueva"), "{}", message)
            }
            other => panic!("se esperaba Failed, llegó {:?}", other),
        }
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    /// Un mensaje multilínea queda en una sola línea del registro.
    #[test]
    fn ready_failure_message_is_flattened() {
        let path = ready_path("flatten");
        let err = StartupError::Failed {
            message: "línea uno\r\nlínea dos\nlínea tres".to_string(),
        };
        write_ready_failure(&path, &err);
        assert_eq!(
            read_ready_signal(&path),
            Some(ReadySignal::Failed(StartupError::Failed {
                message: "línea uno  línea dos línea tres".to_string(),
            }))
        );
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
