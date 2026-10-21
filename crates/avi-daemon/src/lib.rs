// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use axum::{
    body::Body,
    extract::{DefaultBodyLimit, State},
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

pub mod registry;
pub mod spawn;
/// Familia de los logs por arranque del daemon (`daemon_<pid>_<ms>.log`).
pub const DAEMON_LOG_FAMILY: &str = "daemon";
pub use spawn::{kill_tree_by_pid, pid_alive, spawn_background, wait_for_pid_death};
// El trait `SttEngine` (`.transcribe`) solo lo consume la superficie STT,
// gateada tras `native-stt`.
#[cfg(feature = "native-stt")]
use avi_core::engine::SttEngine;
use avi_core::json_emitter;
use avi_store::{ModelStore, SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::ParakeetEngine;
#[cfg(feature = "native-translation")]
use avi_translation::Ct2TranslationEngine;
use avi_tts::{GenerationOptions, Qwen3TtsEngine, TtsEngine, VoiceProfile};
// `Engine` trait requerido por el API no-deprecation de base64 0.22
// (el motor interno del daemon usa el alfabeto STANDARD, idéntico al `encode`/`decode`
// libres, por compatibilidad con el cliente raíz del CLI).
use base64::Engine;

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

/// Estado compartido del daemon. El servidor solo lo construye mediante el
/// constructor que comprueba modelos y carga motores; el harness de contrato
/// puede ensamblarlo sin inferencia para comprobar respuestas previas a ella.
pub struct DaemonState {
    /// Lock de serialización de síntesis (una a la vez)
    pub synthesis_lock: Mutex<()>,
    pub voice_store: VoiceStore,
    pub speech_store: SpeechStore,
    /// Almacén de modelos, anclado una sola vez al arrancar: los handlers lo
    /// consultan sin volver a resolver la raíz de modelos.
    pub model_store: ModelStore,
    /// Motor TTS nativo (Qwen3-TTS), con ciclo de vida persistente entre peticiones.
    pub tts_engine: Qwen3TtsEngine,
    /// Motor STT nativo (Parakeet TDT v3 int8 vía ort). Parakeet no necesita
    /// chunking VAD: su RTF (~0.11 en audio largo) es lineal y no degrada con
    /// la duración, por lo que `transcribe_handler` transcribe de una sola vez.
    /// Los motores de inferencia están presentes juntos en producción y ausentes
    /// juntos en los estados usados para probar validaciones sin inferencia.
    #[cfg(any(feature = "native-stt", feature = "native-translation"))]
    inference_engines: Option<InferenceEngines>,
    /// Estado de warmup del motor TTS, con interior mutability seguro entre hilos:
    /// el warmup corre en un `spawn_blocking` de segundo plano y actualiza este
    /// campo; `/health` lo lee. Inicializa en `Warming`.
    pub warm: std::sync::RwLock<WarmState>,
    /// Señal de cierre compartida: `shutdown_handler` notifica y `run_daemon_server`
    /// la observa para el graceful shutdown. Evita `process::exit` dentro del
    /// runtime tokio de `axum::serve`, que no termina fiablemente el proceso en
    /// Windows. Al cerrar de forma natural se
    /// ejecutan los `Drop` (matando al `Qwen3TtsResident` y `qwen_tts.exe`).
    pub shutdown_notify: Arc<tokio::sync::Notify>,
    /// Registro en disco del daemon y de su residente: el daemon es el único
    /// escritor de `daemon.pid`. Se ancla en la raíz de datos vigente.
    pub registry: Arc<registry::DaemonRegistry>,
}

#[derive(Debug)]
struct MissingModel(&'static str);

impl std::fmt::Display for MissingModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "El modelo '{}' no está provisionado. Ejecuta 'setup' primero.",
            self.0
        )
    }
}

impl std::error::Error for MissingModel {}

#[cfg(any(feature = "native-stt", feature = "native-translation"))]
struct InferenceEngines {
    #[cfg(feature = "native-stt")]
    stt_engine: ParakeetEngine,
    #[cfg(feature = "native-translation")]
    ct2_engines: [Ct2TranslationEngine; 2],
}

struct DaemonEngines {
    tts_engine: Qwen3TtsEngine,
    registry: Arc<registry::DaemonRegistry>,
    #[cfg(any(feature = "native-stt", feature = "native-translation"))]
    inference_engines: Option<InferenceEngines>,
}

impl DaemonState {
    /// Constructor de producción: exige los modelos antes de publicar el daemon.
    pub fn new() -> anyhow::Result<Self> {
        Self::with_stores(VoiceStore::new()?, SpeechStore::new()?)
    }

    /// Constructor de producción con almacenes anclados: carga todos los motores
    /// antes de crear un estado que pueda llegar al servidor.
    pub fn with_stores(voice_store: VoiceStore, speech_store: SpeechStore) -> anyhow::Result<Self> {
        let model_store = ModelStore::new()?;
        let engines = Self::load_engines(&model_store)?;
        Self::assemble(voice_store, speech_store, model_store, engines)
    }

    /// Carga los motores de producción. Toda ausencia de un modelo fijado exige
    /// ejecutar `setup`, aunque el motor no cargue sus pesos hasta la primera petición.
    fn load_engines(model_store: &ModelStore) -> anyhow::Result<DaemonEngines> {
        for pin in avi_shared::paths::MODEL_REVISIONS {
            if !model_store.is_provisioned(pin.name) {
                return Err(MissingModel(pin.name).into());
            }
        }
        #[cfg(feature = "native-stt")]
        let stt_engine = {
            let stt_dir = model_store.model_dir("parakeet-tdt-v3");
            // El daemon vive hasta cerrar el servidor; carga Parakeet directamente.
            ParakeetEngine::new(&stt_dir).map_err(|e| {
                anyhow::anyhow!("fallo al cargar el modelo STT {}: {e}", stt_dir.display())
            })?
        };
        #[cfg(feature = "native-translation")]
        let load_ct2 = |pair| {
            let dir = model_store.model_dir(&format!("opus-mt-{pair}"));
            Ct2TranslationEngine::new(&dir).map_err(|e| {
                anyhow::anyhow!(
                    "fallo al cargar el modelo de traducción {}: {e}",
                    dir.display()
                )
            })
        };
        #[cfg(feature = "native-translation")]
        let ct2_engines = [load_ct2("es-en")?, load_ct2("en-es")?];
        let registry = Arc::new(registry::DaemonRegistry::new(avi_shared::paths::data_dir()?));
        let tts_engine = Qwen3TtsEngine::new(Some(registry.clone()))?;
        Ok(DaemonEngines {
            tts_engine,
            registry,
            #[cfg(any(feature = "native-stt", feature = "native-translation"))]
            inference_engines: Some(InferenceEngines {
                #[cfg(feature = "native-stt")]
                stt_engine,
                #[cfg(feature = "native-translation")]
                ct2_engines,
            }),
        })
    }

    /// Estado de contrato sin motores de inferencia: solo se usa para ejercitar
    /// validaciones, respuestas y eventos emitidos antes de la inferencia.
    /// No se debe pasar a `run_daemon_server`: ese camino construye su propio
    /// estado y exige todos los modelos antes de publicar el servicio.
    #[doc(hidden)]
    pub fn for_contract_tests(
        voice_store: VoiceStore,
        speech_store: SpeechStore,
    ) -> anyhow::Result<Self> {
        let model_store = ModelStore::new()?;
        let engines = DaemonEngines {
            tts_engine: Qwen3TtsEngine::new(None)?,
            registry: Arc::new(registry::DaemonRegistry::new(
                voice_store.root().join("contract-daemon"),
            )),
            #[cfg(any(feature = "native-stt", feature = "native-translation"))]
            inference_engines: None,
        };
        Self::assemble(voice_store, speech_store, model_store, engines)
    }

    fn assemble(
        voice_store: VoiceStore,
        speech_store: SpeechStore,
        model_store: ModelStore,
        engines: DaemonEngines,
    ) -> anyhow::Result<Self> {
        Ok(Self {
            synthesis_lock: Mutex::new(()),
            voice_store,
            speech_store,
            model_store,
            tts_engine: engines.tts_engine,
            #[cfg(any(feature = "native-stt", feature = "native-translation"))]
            inference_engines: engines.inference_engines,
            warm: std::sync::RwLock::new(WarmState::Warming),
            shutdown_notify: Arc::new(tokio::sync::Notify::new()),
            registry: engines.registry,
        })
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

/// Inserta `schema_version` en un envelope objeto, reutilizado por handlers que
/// devuelven JSON directamente.
///
/// **El protocolo del daemon tiene su propia versión**, `DAEMON_SCHEMA_VERSION = 5`: el
/// emisor compartido de `avi-core` lo comparte con el envelope `--json` de la CLI,
/// pero los dos son contratos independientes y sus versiones se gobiernan por
/// separado. Por eso la versión se pasa explícitamente en vez de leerse de una
/// constante única: subir la de la CLI no puede arrastrar a esta. Un valor que no
/// es objeto JSON es un error de programación y aborta.
fn with_sv(val: Value) -> Value {
    json_emitter::with_schema_version(
        json_emitter::envelope_object(val),
        json_emitter::DAEMON_SCHEMA_VERSION,
    )
}

/// Serializa un evento NDJSON con envelope de schema_version al canal de salida.
/// Best-effort con traza: si el receptor ya no existe, el evento se pierde sin
/// que nada lo delate, así que el motivo del descarte queda escrito. El
/// `SendError` devuelve la línea, así que el evento perdido se imprime entero
/// sin copiarlo antes.
async fn emit_ndjson(tx: &tokio::sync::mpsc::Sender<String>, event: Value) {
    let line = serde_json::to_string(&with_sv(event))
        .expect("un `Value` siempre se serializa: sus claves son cadenas");
    if let Err(e) = tx.send(line).await {
        eprintln!(
            "aviso: evento NDJSON perdido, el canal está cerrado: {}",
            e.0
        );
    }
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
///
/// Abandonar la fase no suelta nada del trabajo en vuelo: el guardián del WAV
/// temporal viaja dentro de `job`, de modo que lo borra el hilo cuando termina
/// (y no el handler al retornar). Es lo que impide que un borrado temprano
/// compita con la escritura del motor y quede el fichero recreado detrás.
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

/// `reason` de un fallo de la fase de síntesis: la falta del modelo que exige la
/// voz (Base para una clonada, CustomVoice para un preset) es `model_missing`,
/// igual que cuando el motor no está provisionado; una voz sin referencia que no
/// es preset es `voice_not_found`; cualquier otro fallo es `synthesis_failed`.
fn synthesis_failure_reason(e: &anyhow::Error) -> &'static str {
    if e.downcast_ref::<avi_tts::ModelMissing>().is_some() {
        "model_missing"
    } else if e.downcast_ref::<avi_tts::VoiceNotFound>().is_some() {
        "voice_not_found"
    } else {
        "synthesis_failed"
    }
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
fn enrich_health_body(body: Value, _state: &DaemonState) -> Value {
    #[cfg(any(feature = "native-stt", feature = "native-translation"))]
    let mut body = body;
    #[cfg(feature = "native-stt")]
    {
        body["stt"] = Value::String("warm".into());
    }
    #[cfg(feature = "native-translation")]
    {
        if _state.inference_engines.is_some() {
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
/// Contrato NDJSON vigente: `started` → `progress`(N)
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

    // Voz e idiomas son obligatorios (la traducción ocurre solo si difieren) y
    // la temperatura, si viene, debe ser numérica: el incumplimiento es un
    // error de uso con 400 plano, antes de abrir el stream.
    let fields = (|| {
        Ok::<_, InputError>((
            required_str(&payload, "voice")?.to_string(),
            required_language(&payload, "source_language")?.to_string(),
            required_language(&payload, "target_language")?.to_string(),
            optional_temperature(&payload)?,
        ))
    })();
    let (voice, source_raw, target_raw, temperature) = match fields {
        Ok(fields) => fields,
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
    };

    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    let state = state.clone();
    let text_owned = text.clone();
    let voice_owned = voice.clone();
    let source_owned = source_raw.clone();
    let target_owned = target_raw.clone();

    tokio::spawn(async move {
        // `started` sale antes de esperar `synthesis_lock`: la espera en cola la
        // cubre `run_synthesis_phase` con latidos, así que el cliente nunca queda
        // en silencio mientras otra síntesis o el warmup retienen el lock.
        emit_ndjson(
            &tx,
            json!({
                "event": "started",
                "operation": "síntesis",
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
        let source_iso = source_owned.clone();
        let target_iso = target_owned.clone();
        let text_final = if source_iso == target_iso {
            text_owned.clone()
        } else {
            // Ambos idiomas son `es` o `en` y distintos: el par queda determinado.
            let pair = translation_pair_for(&source_iso);
            let model_name = format!("opus-mt-{pair}");
            let ct2_dir = state.model_store.model_dir(&model_name);
            // El modelo puede borrarse del disco después del arranque; detectar
            // esa pérdida evita informar una traducción exitosa con recursos ausentes.
            if !state.model_store.is_provisioned(&model_name) {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "model_missing",
                        "message": format!("El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.", model_name, ct2_dir.display()),
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
                let Some(engines) = state.inference_engines.as_ref() else {
                    emit_ndjson(&tx, json!({"event": "error", "reason": "daemon_error", "message": "El estado de contrato no puede traducir."})).await;
                    return;
                };
                use avi_core::engine::TranslationEngine;
                let translated =
                    engines.ct2_engines[usize::from(pair == "en-es")].translate(&text_owned);
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
        let qvoice_path = match state.voice_store.find_reference(&voice_owned) {
            Ok(path) => path,
            Err(e) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "io_error",
                        "message": format!("Error resolviendo la voz '{}': {}", voice_owned, e),
                    }),
                )
                .await;
                return;
            }
        };
        let profile = VoiceProfile {
            name: voice_owned.clone(),
            qvoice_path,
        };
        // Sin flag se usa la config de producción (temperature=0.35); con flag
        // se sobrescribe la temperatura ya validada.
        let options = GenerationOptions::with_temperature(temperature, &target_owned);
        // Guardián del temporal: su dueño es el trabajo que escribe en él, no el
        // ámbito del handler. La propiedad pasa al `job` bloqueante y solo vuelve
        // al handler en el éxito, para que lea el WAV ya sintetizado y lo borre al
        // salir de su brazo. Si la fase se abandona (plazo o desconexión) nadie
        // suelta el guardián aquí: lo suelta el hilo cuando termina, de modo que
        // el borrado nunca antecede a la escritura del motor y este no puede
        // recrear el fichero detrás; así no queda residuo en ninguna salida, ni
        // en el camino feliz ni en las salidas por plazo y desconexión.
        let tmp_guard = match avi_process::scratch::temp_wav("avi_daemon_synth_") {
            Ok(guard) => guard,
            Err(e) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": "io_error",
                        "message": format!("Error creando el WAV temporal de síntesis: {:#}", e),
                    }),
                )
                .await;
                return;
            }
        };
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
            match state_synth.tts_engine.synthesize_cancellable(
                &text_final,
                &profile,
                &options,
                Some(tmp_guard.path()),
                &cancel,
            ) {
                // Solo el éxito devuelve el guardián al handler, que lo lee y lo
                // borra. El fallo lo suelta aquí mismo, al acabar el trabajo.
                Ok(path) => Ok(((path, work_t0.elapsed()), tmp_guard)),
                Err(e) => Err(e),
            }
        };
        let Some(synth_res) = run_synthesis_phase(&tx, &state, budget, job).await else {
            return;
        };
        match synth_res {
            // `_tmp_guard` es el dueño del WAV leído: lo borra al salir de este
            // brazo, ya con el audio entregado.
            Ok(((path, work_elapsed), _tmp_guard)) => {
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
                // Sin borrado manual: lo hace `_tmp_guard` al salir de este brazo.
            }
            Err(e) => {
                emit_ndjson(
                    &tx,
                    json!({
                        "event": "error",
                        "reason": synthesis_failure_reason(&e),
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
/// El campo es `audio_b64`; el audio es PCM i16 little-endian 16 kHz mono; la
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

    // El idioma de origen es obligatorio y válido aunque el motor no lo use:
    // Parakeet autodetecta el idioma del audio.
    if let Err(e) = required_language(&payload, "source_language") {
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

    // Parakeet no necesita chunking VAD (no degrada con la duración de audio);
    // se transcribe de una sola pasada.
    let Some(engines) = state.inference_engines.as_ref() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(with_sv(json!({
            "status": "error", "reason": "daemon_error", "message": "El estado de contrato no puede transcribir."
        })))).into_response();
    };
    let result = engines.stt_engine.transcribe(&pcm);
    match result {
        Ok(text) => {
            let body = with_sv(json!({ "text": text }));
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

/// Error de validación de una petición: `reason` es el código del contrato y
/// `message` el texto para el usuario.
#[derive(Debug, PartialEq)]
pub struct InputError {
    pub reason: &'static str,
    pub message: String,
}

impl InputError {
    /// Error de uso: campo ausente, de tipo inválido o con valor fuera de rango.
    fn usage(message: String) -> Self {
        Self {
            reason: "usage_error",
            message,
        }
    }
}

/// Campo de texto obligatorio de la petición; ausente o de otro tipo es error de uso.
fn required_str<'a>(body: &'a Value, key: &str) -> Result<&'a str, InputError> {
    body.get(key).and_then(|v| v.as_str()).ok_or_else(|| {
        InputError::usage(format!("La petición requiere el campo de texto '{key}'."))
    })
}

/// Campo de idioma obligatorio: solo `es` o `en`.
fn required_language<'a>(body: &'a Value, key: &str) -> Result<&'a str, InputError> {
    let value = required_str(body, key)?;
    if matches!(value, "es" | "en") {
        Ok(value)
    } else {
        Err(InputError::usage(format!(
            "El campo '{key}' debe ser 'es' o 'en' (recibido '{value}')."
        )))
    }
}

/// Temperatura opcional: si está presente debe ser numérica.
fn optional_temperature(body: &Value) -> Result<Option<f32>, InputError> {
    match body.get("temperature") {
        None => Ok(None),
        Some(v) => v
            .as_f64()
            .map(|t| Some(t as f32))
            .ok_or_else(|| InputError::usage("El campo 'temperature' debe ser numérico.".into())),
    }
}

/// Resultado de validar una petición de traducción.
#[derive(Debug, PartialEq)]
pub enum TranslateInput {
    /// Idioma de origen y destino coinciden: se devuelve el texto sin traducir.
    Same,
    /// Par distinto y soportado (`es`→`en` o `en`→`es`).
    Pair { source: String, target: String },
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

    if audio_bytes.len() / 2 > avi_core::MAX_TRANSCRIBE_AUDIO_SECS as usize * 16_000 {
        return Err(InputError {
            reason: "audio_too_long",
            message: format!(
                "El audio de la transcripción no puede superar {} segundos.",
                avi_core::MAX_TRANSCRIBE_AUDIO_SECS
            ),
        });
    }

    // PCM i16 little-endian → Vec<i16> mono 16 kHz (el motor normaliza a i16::MAX).
    let pcm = audio_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect::<Vec<i16>>();
    if pcm.is_empty() {
        return Err(InputError {
            reason: "empty_audio",
            message: "El audio de la transcripción no contiene muestras PCM.".to_string(),
        });
    }
    Ok(pcm)
}

/// Par de traducción CT2 (`es-en` o `en-es`) que corresponde a un idioma de
/// origen ya validado (`es` o `en`) cuando origen y destino difieren. Es el
/// único punto del daemon que elige el modelo `opus-mt-<par>`.
fn translation_pair_for(source: &str) -> &'static str {
    if source == "es" {
        "es-en"
    } else {
        "en-es"
    }
}

/// Valida el cuerpo de `/translate`: texto no vacío e idiomas de origen y
/// destino (`source_language`/`target_language`, obligatorios, `es` o `en`).
pub fn validate_translate_input(body: &Value) -> Result<TranslateInput, InputError> {
    let text = body.get("text").and_then(|v| v.as_str()).unwrap_or("");
    if text.trim().is_empty() {
        return Err(InputError {
            reason: "empty_text",
            message: "El texto a traducir está vacío".to_string(),
        });
    }
    let source = required_language(body, "source_language")?;
    let target = required_language(body, "target_language")?;
    if source == target {
        Ok(TranslateInput::Same)
    } else {
        Ok(TranslateInput::Pair {
            source: source.to_string(),
            target: target.to_string(),
        })
    }
}

/// POST /translate — traducción texto→texto con CT2 residente
#[cfg(feature = "native-translation")]
async fn translate_handler(
    State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
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
                "translated": payload["text"],
                "source_language": payload["source_language"],
                "target_language": payload["target_language"],
            })))
            .into_response();
        }
        Ok(TranslateInput::Pair { source, target }) => (source, target),
    };
    let text = payload["text"].as_str().unwrap_or_default().to_string();
    let pair = translation_pair_for(&source);
    let model_name = format!("opus-mt-{pair}");
    let ct2_dir = state.model_store.model_dir(&model_name);
    // El snapshot puede borrarse tras cargar el residente; la petición informa
    // la pérdida del recurso sin reconstruir el motor bajo demanda.
    if !state.model_store.is_provisioned(&model_name) {
        return (
            StatusCode::NOT_FOUND,
            Json(with_sv(json!({
                "error": "model_missing",
                "reason": "model_missing",
                "message": format!("El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.", model_name, ct2_dir.display()),
            }))),
        )
            .into_response();
    }
    let Some(engines) = state.inference_engines.as_ref() else {
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(with_sv(json!({
            "error": "daemon_error", "reason": "daemon_error", "message": "El estado de contrato no puede traducir."
        })))).into_response();
    };
    use avi_core::engine::TranslationEngine;
    match engines.ct2_engines[usize::from(pair == "en-es")].translate(&text) {
        Ok(translated) => Json(with_sv(json!({
            "translated": translated,
            "source_language": source,
            "target_language": target,
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
    // `force` es opcional (ausente = no sobrescribir); si viene, debe ser booleano.
    let force = match payload.get("force") {
        None => false,
        Some(v) => match v.as_bool() {
            Some(force) => force,
            None => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(with_sv(json!({
                        "error": "usage_error",
                        "reason": "usage_error",
                        "message": "El campo 'force' debe ser booleano.",
                    }))),
                )
                    .into_response();
            }
        },
    };
    let voice_exists = match state.voice_store.exists(&name) {
        Ok(found) => found,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(with_sv(json!({
                    "error": "io_error",
                    "reason": "io_error",
                    "message": format!("Error resolviendo la voz '{}': {}", name, e),
                }))),
            )
                .into_response();
        }
    };
    if !force && voice_exists {
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
    // La duración se mide por la cabecera, antes de buscar el modelo; un audio que
    // no es WAV no tiene cabecera que medir y lo rechaza después el clonado.
    if avi_audio::wav_bytes_duration_secs(&audio_bytes)
        .is_some_and(|secs| secs > avi_core::MAX_CLONE_REFERENCE_SECS as f64)
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(with_sv(json!({
                "error": "audio_too_long",
                "reason": "audio_too_long",
                "message": format!(
                    "La referencia de clonado no puede superar {} segundos.",
                    avi_core::MAX_CLONE_REFERENCE_SECS
                ),
            }))),
        )
            .into_response();
    }
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
    // Directorio de trabajo con dueño: contiene el WAV de referencia y el
    // `.qvoice` parcial, y se borra al soltarse por cualquier camino de salida.
    // Cuelga del scratch del daemon, cuyo barrido recoge el residuo si el proceso muere.
    let work_dir = match avi_process::scratch::tempdir("avi_daemon_clone_") {
        Ok(dir) => dir,
        Err(_) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(with_sv(json!({
                    "error": "io_error",
                    "reason": "io_error",
                    "message": "No se pudo crear el directorio temporal.",
                }))),
            )
                .into_response();
        }
    };
    let tmp_wav = work_dir.path().join("reference.wav");
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
    let tmp_qvoice = work_dir.path().join("voice.qvoice");
    // Stream NDJSON: las validaciones baratas ya pasaron en JSON plano;
    // `started` inmediato antes del trabajo pesado, latidos cada ~500 ms y
    // evento final con la forma contractual actual (`precomputed: true` =
    // «precarga en caliente iniciada»). Sin almacén de trabajos ni expiración.
    let (tx, rx) = tokio::sync::mpsc::channel::<String>(32);
    tokio::spawn(async move {
        emit_ndjson(
            &tx,
            json!({ "event": "started", "operation": "clonación", "name": name }),
        )
        .await;
        let event_name = name.clone();
        let clone_qvoice = tmp_qvoice.clone();
        let clone_work = tokio::task::spawn_blocking(move || {
            avi_tts::clone_voice(&base_model_dir, &tmp_wav, &clone_qvoice, &name)
                .map(|()| (work_dir, name))
        });
        // `_work_dir` mantiene vivo el directorio hasta el final de la tarea.
        let (_work_dir, name) = match with_heartbeats(&tx, "clone", clone_work).await {
            None => {
                // Cliente desconectado: el hilo bloqueante conserva el directorio
                // de trabajo y lo borra al retornar.
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
            return;
        }
        let saved_qvoice = match state.voice_store.save_reference(&name, &tmp_qvoice) {
            Ok(p) => p,
            Err(e) => {
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
        // Warm-on-clone (A): precalienta la voz recién clonada en segundo plano
        // para eliminar el cold-start del residente en la primera síntesis del
        // flujo clonar→sintetizar. Se conserva en `spawn_blocking` y se anuncia
        // por evento (por eso `precomputed: true` = precarga iniciada).
        let warm_state = state.clone();
        let warm_name = name.clone();
        tokio::task::spawn_blocking(move || {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "la precarga registra su fallo en el estado de calentamiento del daemon"
            )]
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

/// Respuesta JSON plana de error de entrada con su `reason`.
fn input_error_response(reason: &str, message: String) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(with_sv(json!({
            "status": "error",
            "reason": reason,
            "message": message,
        }))),
    )
        .into_response()
}

/// Audio de `/dub` decodificado a PCM de 16 kHz mono 16 bits.
fn dub_pcm(payload: &Value) -> Result<Vec<i16>, InputError> {
    let audio_b64 = payload
        .get("audio_b64")
        .and_then(|v| v.as_str())
        .ok_or_else(|| {
            InputError::usage("La petición no incluye el campo 'audio_b64'.".to_string())
        })?;
    let audio_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64)
        .map_err(|e| InputError {
            reason: "invalid_audio",
            message: format!("audio_b64 no decodificable: {}", e),
        })?;
    let pcm = audio_bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect::<Vec<i16>>();
    if pcm.is_empty() {
        return Err(InputError {
            reason: "empty_audio",
            message: "El audio del doblaje no contiene muestras PCM.".to_string(),
        });
    }
    Ok(pcm)
}

/// Voz, idiomas de origen y destino y temperatura opcional de `/dub`.
fn dub_fields(payload: &Value) -> Result<(String, String, String, Option<f32>), InputError> {
    Ok((
        required_str(payload, "voice")?.to_string(),
        required_language(payload, "source_language")?.to_string(),
        required_language(payload, "target_language")?.to_string(),
        optional_temperature(payload)?,
    ))
}

/// POST /dub — pipeline voz→voz (transcribe→translate→synthesize), servido
/// como stream NDJSON con latidos: `started` tras las validaciones
/// baratas (audio, par/CT2, modelo, voz y rama sin `native-stt`, en JSON plano
/// con los códigos de siempre), latidos `heartbeat` durante cada fase pesada y
/// evento final `result` con la forma contractual actual (`status: "dubbed"`).
/// La fase de síntesis la gobierna `run_synthesis_phase`: latidos en cola,
/// presupuesto proporcional al texto y cancelación del trabajo en el motor al
/// vencer o al desconectarse el cliente.
async fn dub_handler(
    #[cfg(feature = "native-stt")] State(state): State<SharedState>,
    Json(payload): Json<Value>,
) -> Response {
    // Transcripción — requiere `native-stt`; sin el feature el pipeline no puede arrancar.
    #[cfg(not(feature = "native-stt"))]
    {
        if let Err(e) = dub_pcm(&payload).and_then(|_| dub_fields(&payload)) {
            return input_error_response(e.reason, e.message);
        }
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
    #[cfg(feature = "native-stt")]
    let pcm = match dub_pcm(&payload) {
        Ok(pcm) => pcm,
        Err(e) => return input_error_response(e.reason, e.message),
    };
    #[cfg(feature = "native-stt")]
    let (voice, source_iso, target_iso, temperature) = match dub_fields(&payload) {
        Ok(fields) => fields,
        Err(e) => return input_error_response(e.reason, e.message),
    };
    // Validaciones baratas ANTES del stream (JSON plano con los códigos de
    // siempre; `started` solo se emite cuando el trabajo pesado va a arrancar).
    // Los chequeos por parámetros (par, CT2, feature, modelo, voz) corren antes
    // de transcribir: con varios fallos a la vez gana el más barato de detectar,
    // y cada fallo conserva su código.
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
    // Ambos idiomas son `es` o `en`: si difieren, el par de traducción queda
    // determinado por el origen.
    #[cfg(feature = "native-stt")]
    let needs_translation = source_iso != target_iso;
    #[cfg(feature = "native-stt")]
    let translation_pair: Option<String> =
        needs_translation.then(|| translation_pair_for(&source_iso).to_string());
    #[cfg(feature = "native-stt")]
    if let Some(pair) = translation_pair.as_deref() {
        let model_name = format!("opus-mt-{pair}");
        let ct2_dir = state.model_store.model_dir(&model_name);
        // El snapshot puede borrarse después del arranque; la comprobación
        // informa `model_missing` sin intentar cargar motores bajo demanda.
        if !state.model_store.is_provisioned(&model_name) {
            return (
                StatusCode::NOT_FOUND,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "model_missing",
                    "message": format!("El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.", model_name, ct2_dir.display()),
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
    let voice_found = match state.voice_store.exists(&voice) {
        Ok(found) => found,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(with_sv(json!({
                    "status": "error",
                    "reason": "io_error",
                    "message": format!("Error resolviendo la voz '{}': {}", voice, e),
                }))),
            )
                .into_response();
        }
    };
    #[cfg(feature = "native-stt")]
    if !voice_found {
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
        tokio::spawn(async move {
            emit_ndjson(
                &tx,
                json!({ "event": "started", "operation": "doblaje", "voice": voice }),
            )
            .await;
            // Transcripción en `spawn_blocking` con latidos (fase pesada: STT
            // sobre el audio completo).
            let stt_state = state.clone();
            let stt_work = tokio::task::spawn_blocking(move || {
                stt_state
                    .inference_engines
                    .as_ref()
                    .map(|engines| engines.stt_engine.transcribe(&pcm))
            });
            let transcribed = match with_heartbeats(&tx, "transcribe", stt_work).await {
                None => return,
                Some(Ok(Some(Ok(t)))) => t,
                Some(Ok(None)) => {
                    emit_ndjson(&tx, json!({"event": "error", "reason": "daemon_error", "message": "El estado de contrato no puede transcribir."})).await;
                    return;
                }
                Some(Ok(Some(Err(e)))) => {
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
                    let ct2_state = state.clone();
                    let text = transcribed.clone();
                    let translation_work = tokio::task::spawn_blocking(move || {
                        use avi_core::engine::TranslationEngine;
                        ct2_state.inference_engines.as_ref().map(|engines| {
                            engines.ct2_engines[usize::from(pair == "en-es")].translate(&text)
                        })
                    });
                    match with_heartbeats(&tx, "translate", translation_work).await {
                        None => return,
                        Some(Ok(Some(Ok(t)))) => t,
                        Some(Ok(None)) => {
                            emit_ndjson(&tx, json!({"event": "error", "reason": "daemon_error", "message": "El estado de contrato no puede traducir."})).await;
                            return;
                        }
                        Some(Ok(Some(Err(e)))) => {
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
            let qvoice_path = match state.voice_store.find_reference(&voice) {
                Ok(path) => path,
                Err(e) => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "io_error",
                            "message": format!("Error resolviendo la voz '{}': {}", voice, e),
                        }),
                    )
                    .await;
                    return;
                }
            };
            let profile = VoiceProfile {
                name: voice.clone(),
                qvoice_path,
            };
            // Guardián del temporal: su dueño es el trabajo que escribe en él,
            // no el ámbito del handler. La propiedad pasa al `job` bloqueante y
            // solo vuelve al handler en el éxito, para que lea el WAV ya
            // sintetizado y lo borre al salir de su brazo; si la fase se abandona
            // por plazo o desconexión, lo suelta el hilo cuando termina, así el
            // borrado nunca antecede a la escritura del motor ni este recrea el
            // fichero detrás y no queda residuo en ninguna salida.
            let tmp_guard = match avi_process::scratch::temp_wav("avi_daemon_dub_") {
                Ok(guard) => guard,
                Err(e) => {
                    emit_ndjson(
                        &tx,
                        json!({
                            "event": "error",
                            "reason": "io_error",
                            "message": format!("Error creando el WAV temporal del doblaje: {:#}", e),
                        }),
                    )
                    .await;
                    return;
                }
            };
            let budget = avi_core::synthesis_budget(final_text.chars().count());
            let text_synth = final_text.clone();
            let synth_state = state.clone();
            let synth_options = GenerationOptions::with_temperature(temperature, &target_iso);
            let job = move |cancel: Arc<std::sync::atomic::AtomicBool>| {
                // Reloj de trabajo tomado dentro del trabajo, ya con el lock: mide
                // solo la fase de síntesis, no transcribe/translate ni la cola.
                let work_t0 = std::time::Instant::now();
                match synth_state.tts_engine.synthesize_cancellable(
                    &text_synth,
                    &profile,
                    &synth_options,
                    Some(tmp_guard.path()),
                    &cancel,
                ) {
                    // Solo el éxito devuelve el guardián al handler, que lo lee y
                    // lo borra. El fallo lo suelta aquí mismo, al acabar el trabajo.
                    Ok(path) => Ok(((path, work_t0.elapsed()), tmp_guard)),
                    Err(e) => Err(e),
                }
            };
            let Some(synth_res) = run_synthesis_phase(&tx, &state, budget, job).await else {
                return;
            };
            match synth_res {
                // `_tmp_guard` es el dueño del WAV leído: lo borra al salir de
                // este brazo, ya con el audio entregado.
                Ok(((path, work_elapsed), _tmp_guard)) => {
                    match std::fs::read(&path) {
                        Ok(wav_bytes) => {
                            let b64 = base64::engine::general_purpose::STANDARD.encode(&wav_bytes);
                            // Sin borrado manual: lo hace `_tmp_guard` al salir de
                            // este brazo.
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
                            "reason": synthesis_failure_reason(&e),
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
/// en Windows (la task async donde se dispara puede no completarse), y dejaría
/// vivos al daemon y, con él, al `Qwen3TtsResident` y su hijo `qwen_tts.exe`.
/// Al notificar, `with_graceful_shutdown` cierra el runtime de forma natural y
/// corre los `Drop` del `Arc<DaemonState>` compartido (que matan al residente)
/// antes del exit.
async fn shutdown_handler(State(state): State<SharedState>) -> impl IntoResponse {
    // 1) Detener el residente qwen_tts. `Qwen3TtsEngine::shutdown` es NO BLOQUEANTE:
    //    mata el árbol del residente solo por su PID registrado con identidad (daemon
    //    y CLI comparten ejecutable, por lo que nunca se mata por nombre de imagen)
    //    y sin tomar el `Mutex<resident>` que el hilo `spawn_blocking(warmup)` retiene
    //    durante el spawn + healthcheck + síntesis, lo que bloquearía el apagado;
    //    libera el residente con `try_lock`.
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
    // Peor caso de la referencia de clonado: el daemon no restringe su formato
    // (cualquier WAV que lea `hound`), así que se dimensiona para 96 kHz, estéreo,
    // 32 bits; un WAV más pesado se rechaza con 413 aunque esté dentro del tope.
    const CLONE_REFERENCE_BYTES_PER_SEC: u64 = 96_000 * 2 * 4;
    let router = Router::new()
        .route("/health", get(health_handler))
        .route(
            "/voices/clone",
            post(voices_clone_handler).layer(DefaultBodyLimit::max(avi_core::body_limit_for(
                avi_core::MAX_CLONE_REFERENCE_SECS,
                CLONE_REFERENCE_BYTES_PER_SEC,
            ))),
        )
        .route("/dub", post(dub_handler))
        .route("/synthesize", post(synthesize_handler))
        .route("/shutdown", post(shutdown_handler));
    // `/transcribe` solo existe con el motor STT compilado (`native-stt`); sin el
    // feature el daemon expone el resto de rutas sin ONNX Runtime. El `#[cfg]` se
    // aplica a un método builder distinto (no como argumento anidado, que el macro
    // de axum 0.7 no parsea en Rust 2021/Windows → error E0061).
    #[cfg(feature = "native-stt")]
    let router = router.route(
        "/transcribe",
        // PCM de 16 kHz, mono, 16 bits: 32000 bytes por segundo.
        post(transcribe_handler).layer(DefaultBodyLimit::max(avi_core::body_limit_for(
            avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
            32_000,
        ))),
    );
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
/// warmup sintético; los dos pares se cargan antes de servir peticiones y
/// `warm` (`Warming`→`Warm`) solo refleja TTS.
pub fn warm_voice_engine(state: &DaemonState, voice: &str) -> anyhow::Result<()> {
    let qvoice_path = match state.voice_store.find_reference(voice) {
        Ok(path) => path,
        Err(e) => {
            state.set_warm_failed(e.to_string());
            return Err(e);
        }
    };
    let profile = VoiceProfile {
        name: voice.to_string(),
        qvoice_path,
    };
    let _lock = state.synthesis_lock.blocking_lock();
    state.set_warming();
    // Reloj de trabajo tomado tras el lock: mide warmup puro y excluye la espera
    // en cola contra tráfico vivo.
    let work_t0 = std::time::Instant::now();
    // El WAV de calentamiento tiene dueño: se borra al salir, también si falla.
    // El texto de calentamiento es fijo y en español: el idioma es siempre `es`.
    let result = avi_process::scratch::temp_wav("avi_daemon_warmup_")
        .map_err(|e| anyhow::anyhow!("No se pudo reservar el WAV de calentamiento: {:#}", e))
        .and_then(|tmp| {
            state
                .tts_engine
                .synthesize_with_options(
                    "Calentamiento del daemon.",
                    &profile,
                    &GenerationOptions::production("es"),
                    Some(tmp.path()),
                )
                .map_err(|e| anyhow::anyhow!("Warmup TTS falló: {}", e))
        });
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
    /// Falta un modelo exigido antes de servir.
    ModelMissing { message: String },
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
            StartupError::ModelMissing { message } | StartupError::Failed { message } => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for StartupError {}

/// Señal leída del fichero ready: el daemon está listo en `addr` (con el PID
/// que publicó, si es legible) o falló antes de estarlo.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadySignal {
    Ready {
        addr: String,
        pid: Option<u32>,
        /// Identidad del proceso del daemon publicada en el fichero, si la trae.
        identity: Option<avi_process::ProcessIdentity>,
    },
    Failed(StartupError),
}

// El fichero ready admite dos registros, ambos `clave=valor` por línea:
// - éxito: `addr=<dirección real>`, `warm=<estado>` y `pid=<pid del daemon>`;
// - fallo: `error=port_in_use` + `port=`, `error=warm_voice_missing` + `voice=`,
//   `error=model_missing` + `message=` o `error=startup_failed` + `message=`.
// El registro de fallo nunca lleva `addr=`: los lectores toman `addr=` como
// señal de listo, y uno que no conozca `error=` debe seguir viendo aún-no-listo.

/// Publica `content` en el fichero ready con escritura atómica (temporal
/// hermano + rename), de modo que un lector nunca ve un registro a medias.
/// Un fallo de escritura o de rename se devuelve con la ruta: sin fichero ready
/// el padre no puede saber que el daemon está listo.
fn publish_ready_record(path: &std::path::Path, content: &str) -> anyhow::Result<()> {
    let owner = avi_process::current_owner_token()?;
    let tmp = avi_shared::staging_names::temp_file::sibling(path, &owner)
        .ok_or_else(|| anyhow::anyhow!("{} no tiene un nombre de fichero UTF-8", path.display()))?;
    std::fs::write(&tmp, content).map_err(|e| {
        anyhow::anyhow!(
            "no se pudo escribir el fichero ready {}: {e}",
            tmp.display()
        )
    })?;
    std::fs::rename(&tmp, path).map_err(|e| {
        anyhow::anyhow!(
            "no se pudo publicar el fichero ready {}: {e}",
            path.display()
        )
    })
}

/// Contenido del registro de éxito: `addr` + `warm` + el PID propio + la
/// identidad del proceso (`start` e `image`).
///
/// El PID propio permite reclamar el árbol si el padre cae sin escribir o sin
/// limpiar el pidfile: con puertos efímeros el ready es entonces la única pista
/// de qué proceso matar. La identidad distingue, tras un apagado abrupto, a este
/// proceso de otro programa al que el sistema reasigne el PID, por lo que su
/// ausencia es un error y no se publica un registro sin ella.
fn ready_success_content(
    addr: &SocketAddr,
    warm: &str,
    identity: Option<avi_process::ProcessIdentity>,
) -> anyhow::Result<String> {
    let identity = identity.ok_or_else(|| {
        anyhow::anyhow!(
            "no se pudo observar la identidad del proceso del daemon (pid {}): sin ella el fichero ready no identifica a este proceso",
            std::process::id()
        )
    })?;
    Ok(format!(
        "addr={}\nwarm={}\npid={}\nstart={}\nimage={}\n",
        addr,
        warm,
        std::process::id(),
        identity.start,
        identity.image
    ))
}

/// Publica el registro de éxito de este proceso.
fn write_ready_file(path: &std::path::Path, addr: &SocketAddr, warm: &str) -> anyhow::Result<()> {
    let identity = avi_process::process_identity(std::process::id());
    publish_ready_record(path, &ready_success_content(addr, warm, identity)?)
}

/// Publica el registro de fallo de un arranque que no llegó a estar listo.
fn write_ready_failure(path: &std::path::Path, err: &StartupError) -> anyhow::Result<()> {
    let content = match err {
        StartupError::PortInUse { port } => format!("error=port_in_use\nport={}\n", port),
        StartupError::WarmVoiceMissing { voice } => {
            format!("error=warm_voice_missing\nvoice={}\n", voice)
        }
        StartupError::ModelMissing { message } => format!(
            "error=model_missing\nmessage={}\n",
            message.replace(['\r', '\n'], " ")
        ),
        // El formato es de una línea por clave: los saltos del mensaje se
        // aplanan para que no corten el registro.
        StartupError::Failed { message } => format!(
            "error=startup_failed\nmessage={}\n",
            message.replace(['\r', '\n'], " ")
        ),
    };
    publish_ready_record(path, &content)
}

/// Lee el fichero ready. Devuelve `None` si no existe o aún no contiene un
/// registro de éxito completo (el llamante sigue esperando). `error=` tiene
/// prioridad sobre `addr=`: una clase de error desconocida, o una conocida con
/// su campo ausente o malformado, se entrega como `Failed` con la causa en el
/// mensaje. Un `pid=` ausente, ilegible o `0` da `pid: None`.
pub fn read_ready_signal(path: &std::path::Path) -> Option<ReadySignal> {
    let content = std::fs::read_to_string(path).ok()?;
    let field = |key: &str| avi_process::registry::ready_field(&content, key);
    if let Some(class) = field("error") {
        let malformed = |what: &str| StartupError::Failed {
            message: format!("el registro de fallo del daemon es inválido: error={class} {what}"),
        };
        let err = match class.as_str() {
            "port_in_use" => match field("port").and_then(|p| p.parse().ok()) {
                Some(port) => StartupError::PortInUse { port },
                None => malformed("sin un 'port=' numérico"),
            },
            "warm_voice_missing" => match field("voice") {
                Some(voice) => StartupError::WarmVoiceMissing { voice },
                None => malformed("sin 'voice='"),
            },
            "model_missing" => match field("message") {
                Some(message) => StartupError::ModelMissing { message },
                None => malformed("sin 'message='"),
            },
            "startup_failed" => match field("message") {
                Some(message) => StartupError::Failed { message },
                None => malformed("sin 'message='"),
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
    let record = avi_process::registry::parse_ready_record(&content)?;
    Some(ReadySignal::Ready {
        addr: record.addr,
        pid: record.pid,
        identity: record.identity,
    })
}

/// Fase previa a servir, ordenada de lo barato a lo caro para fallar antes de
/// pagar la carga de modelos: valida la voz de warmup, enlaza el puerto y solo
/// entonces construye el estado. Todo fallo sale clasificado como
/// `StartupError`.
async fn prepare_server(
    addr: SocketAddr,
    warm_voice: &str,
    voices: VoiceStore,
    speech: SpeechStore,
) -> Result<(TcpListener, SocketAddr, Arc<DaemonState>), StartupError> {
    // Habilitador: materializar las voces de fábrica en la instancia
    // (idempotente, desde el asset embebido). Sin esto, un `data_dir` virgen
    // (sandbox de estado por instancia vía `AVI_DATA_DIR`) rechazaría
    // `--warm-voice default` porque `default` aún no existe en disco. Tras
    // esto, una voz inexistente se rechaza igual (no es de fábrica).
    avi_shared::diag_trace::mark("child.prepare_server.enter");
    voices
        .ensure_initialized()
        .map_err(|e| StartupError::Failed {
            message: format!("No se pudo inicializar el almacén de voces: {}", e),
        })?;
    avi_shared::diag_trace::mark("child.voices_initialized");

    // Guarda de arranque: una `--warm-voice` inexistente aborta el arranque antes de
    // enlazar y de cargar modelos, sin degradar en silencio ni caer a `default`.
    // Existe tanto una voz clonada como un preset del motor (`ryan`, `vivian`),
    // que no tiene referencia `.qvoice`.
    let warm_exists = voices
        .exists(warm_voice)
        .map_err(|e| StartupError::Failed {
            message: format!("No se pudo inicializar el almacén de voces: {}", e),
        })?;
    if !warm_exists {
        avi_shared::diag_trace::mark("child.warm_voice_missing");
        return Err(StartupError::WarmVoiceMissing {
            voice: warm_voice.to_string(),
        });
    }
    avi_shared::diag_trace::mark("child.warm_voice_validated");

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
            avi_shared::diag_trace::mark("child.bind_failed_port_in_use");
            StartupError::PortInUse { port: addr.port() }
        } else {
            avi_shared::diag_trace::mark("child.bind_failed_other");
            StartupError::Failed {
                message: format!("No se pudo enlazar {}: {}", addr, e),
            }
        }
    })?;
    avi_shared::diag_trace::mark("child.port_bound");
    // Puerto efímero por instancia: publicar la dirección REALMENTE enlazada
    // (con `:0` el SO asigna, nunca el literal pedido).
    let bound = listener.local_addr().map_err(|e| StartupError::Failed {
        message: format!("No se pudo leer la dirección enlazada: {}", e),
    })?;

    let state = DaemonState::with_stores(voices, speech).map_err(|e| {
        if e.downcast_ref::<MissingModel>().is_some() {
            avi_shared::diag_trace::mark("child.state_model_missing");
            StartupError::ModelMissing {
                message: e.to_string(),
            }
        } else {
            avi_shared::diag_trace::mark("child.state_failed");
            StartupError::Failed {
                message: format!("No se pudo inicializar el estado del daemon: {}", e),
            }
        }
    })?;
    avi_shared::diag_trace::mark("child.state_built");
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
///
/// `guard` es la guardia opaca del bloqueo de ciclo de vida que el lanzador
/// tomó para este proceso: se suelta justo después de registrar el daemon,
/// porque desde ese momento lo alcanza la parada de las operaciones. Si el
/// arranque falla antes, se suelta al devolver el error.
pub async fn run_daemon_server(
    addr: SocketAddr,
    warm_voice: String,
    voices: VoiceStore,
    speech: SpeechStore,
    guard: Option<Box<dyn Send>>,
) -> anyhow::Result<()> {
    let (listener, bound, state) = prepare_server(addr, &warm_voice, voices, speech)
        .await
        .map_err(anyhow::Error::new)?;
    avi_shared::diag_trace::mark("child.prepare_server.done");
    // El daemon se registra a sí mismo con la dirección real, antes de publicar
    // `daemon.ready` y de cualquier warmup: desde que alguien puede verlo listo,
    // `daemon.pid` ya lo nombra. Sin identidad observable o con otro daemon vivo
    // registrado el arranque falla y no sirve.
    state
        .registry
        .register_daemon(&bound)
        .map_err(|e| StartupError::Failed {
            message: e.to_string(),
        })
        .map_err(anyhow::Error::new)?;
    drop(guard);
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
        write_ready_file(path, &bound, "warming")
            .map_err(|e| StartupError::Failed {
                message: e.to_string(),
            })
            .map_err(anyhow::Error::new)?;
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
            // El daemon ya está listo y sirviendo: un fallo al actualizar el
            // estado `warm` no tumba el arranque, pero queda escrito.
            if let Err(e) = write_ready_file(path, &bound, warm) {
                eprintln!("aviso: {e}");
            }
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
    let closing_state = state.clone();
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
    let served = axum::serve(listener, app)
        .with_graceful_shutdown(shutdown)
        .await;
    // Cierre ordenado del motor en toda salida de `serve`, también ante un
    // error: mata al residente y espera su muerte antes de terminar y antes de
    // cualquier reinicio en proceso de `run_supervised`, que reescribe el
    // registro sin residente. El warmup en segundo plano puede conservar el
    // estado compartido, así que el cierre no depende de soltarlo.
    closing_state.tts_engine.close();
    // Con el motor muerto nadie usa ya el scratch: se libera aquí y no al
    // terminar el proceso, porque el supervisor puede reiniciar `serve` en proceso.
    avi_process::scratch::release();
    served?;
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
        // El error original es el que se devuelve; si el registro de fallo no
        // se puede publicar, el motivo queda escrito junto a él.
        if let Err(e) = write_ready_failure(std::path::Path::new(&path), startup) {
            eprintln!("aviso: {e}");
        }
    }
    avi_shared::diag_trace::mark("child.failure_published");
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
/// (crash) con backoff exponencial `500ms * 2^(retries-1)` capado a 4s, precedido de
/// reclamo activo del árbol propio previo con deadline y verificación: antes de
/// reintentar se espera (hasta 5 s, sondeo 200 ms) a que el puerto quede libre
/// (el `Drop` de la iteración previa ya mató el árbol preciso del residente;
/// sin kill global por imagen ni otra instancia) y se registra muerte + puerto
/// libre o sigue-ocupado; el siguiente `bind` lo confirma.
/// Un apagado graceful vía `shutdown_notify` (`daemon stop`) no reintenta y retorna `Ok`.
/// El reclamo activo matar-y-rearrancar ante otra instancia vive en el CLI
/// (`daemon start`): `serve` nunca mata a otra instancia sana.
///
/// `guard` solo acompaña a la primera iteración, que la suelta al registrarse:
/// en los reinicios en proceso el daemon ya está registrado y no queda guardia.
pub async fn run_supervised(
    addr: SocketAddr,
    auto_restart: bool,
    max_retries: u32,
    warm_voice: String,
    voices: VoiceStore,
    speech: SpeechStore,
    guard: Option<Box<dyn Send>>,
) -> anyhow::Result<()> {
    let mut guard = guard;
    if !auto_restart {
        let result = run_daemon_server(addr, warm_voice, voices, speech, guard).await;
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
        match run_daemon_server(
            addr,
            warm_voice.clone(),
            voices.clone(),
            speech.clone(),
            guard.take(),
        )
        .await
        {
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

/// Pruebas unitarias del daemon.
///
/// Las pruebas de contrato usan un estado sin motores y validan entradas antes
/// de inferencia; las pruebas de recursos usan un constructor que comprueba
/// Parakeet antes de cargar el estado de producción, que exige los cinco modelos.
/// Las pruebas con recursos externos declaran `#[ignore = "requiere …"]`.
/// Activar un feature no convierte una prueba de contrato en prueba de recursos.
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

    /// `health_body` en warming: `status:ready`, `warm:warming`, sin `warm_error`
    /// ni claves `ct2`/`stt` (las añade `enrich_health_body` cuando hay motores
    /// residentes).
    #[test]
    fn health_body_warming_without_warm_error() {
        let body = health_body("warming", None);
        assert_eq!(body["status"], "ready");
        assert_eq!(body["warm"], "warming");
        assert_eq!(body["engine"], "rust_native");
        assert!(body.get("warm_error").is_none());
        assert!(body.get("ct2").is_none());
        assert!(body.get("stt").is_none());
    }

    /// `health_body` en fallo: incluye `warm_error` con la causa y ninguna
    /// clave `ct2`/`stt`.
    #[test]
    fn health_body_failed_includes_warm_error() {
        let body = health_body("warm_failed", Some("boom".to_string()));
        assert_eq!(body["warm"], "warm_failed");
        assert_eq!(body["warm_error"], "boom");
        assert!(body.get("ct2").is_none());
        assert!(body.get("stt").is_none());
    }

    fn bound_addr() -> SocketAddr {
        "127.0.0.1:7001".parse().unwrap()
    }

    /// El registro de éxito lleva dirección, estado, PID e identidad completa, y
    /// `read_ready_signal` lo devuelve tal cual.
    #[test]
    fn ready_success_content_round_trips_identity() {
        let identity = avi_process::ProcessIdentity {
            start: "123".to_string(),
            image: "ai-voice-interconnector.exe".to_string(),
        };
        let content =
            ready_success_content(&bound_addr(), "warming", Some(identity.clone())).unwrap();
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("daemon.ready");
        publish_ready_record(&path, &content).unwrap();

        assert_eq!(
            read_ready_signal(&path),
            Some(ReadySignal::Ready {
                addr: "127.0.0.1:7001".to_string(),
                pid: Some(std::process::id()),
                identity: Some(identity),
            })
        );
    }

    /// Sin identidad observable no se publica un registro de éxito.
    #[test]
    fn ready_success_content_requires_identity() {
        let err = ready_success_content(&bound_addr(), "warming", None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("identidad"), "{err}");
    }

    /// Un fallo de escritura del fichero ready se devuelve con la ruta y no deja
    /// registro.
    #[test]
    fn publish_ready_record_propagates_write_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("sin-directorio").join("daemon.ready");
        let err = publish_ready_record(&path, "addr=127.0.0.1:1\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no se pudo escribir el fichero ready"),
            "{err}"
        );
        assert!(!path.exists());
    }

    /// Un fallo del rename final se devuelve con la ruta de destino.
    #[test]
    fn publish_ready_record_propagates_rename_failure() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("daemon.ready");
        // Un directorio en el destino impide el rename del temporal sobre él.
        std::fs::create_dir(&path).unwrap();
        let err = publish_ready_record(&path, "addr=127.0.0.1:1\n")
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("no se pudo publicar el fichero ready"),
            "{err}"
        );
    }

    /// Un registro de fallo conocido con su campo ausente o malformado se lee
    /// como `Failed` con mensaje explícito, no como espera.
    #[test]
    fn read_ready_signal_reports_malformed_failure_records() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("daemon.ready");
        let cases = [
            (
                "error=port_in_use\nport=abc\n",
                "port_in_use sin un 'port=' numérico",
            ),
            ("error=port_in_use\n", "port_in_use sin un 'port=' numérico"),
            (
                "error=warm_voice_missing\n",
                "warm_voice_missing sin 'voice='",
            ),
            ("error=model_missing\n", "model_missing sin 'message='"),
            ("error=startup_failed\n", "startup_failed sin 'message='"),
        ];
        for (content, expected) in cases {
            std::fs::write(&path, content).unwrap();
            let Some(ReadySignal::Failed(StartupError::Failed { message })) =
                read_ready_signal(&path)
            else {
                panic!("{content:?} debe leerse como fallo de arranque");
            };
            assert!(message.contains(expected), "{content:?}: {message}");
        }
    }

    /// Estado del daemon con los almacenes anclados en un directorio temporal
    /// único por llamada, sin tocar el directorio de datos real. Devuelve también
    /// el directorio con dueño: el llamante lo retiene mientras use el estado y
    /// se borra al soltarse.
    fn test_state() -> (DaemonState, tempfile::TempDir) {
        let tmp = tempfile::Builder::new()
            .prefix("avi_daemon_test_state_")
            .tempdir()
            .expect("directorio temporal del estado");
        let state = DaemonState::for_contract_tests(
            VoiceStore::at(tmp.path().join("voices")),
            SpeechStore::at(tmp.path().join("speech")),
        )
        .expect("daemon state");
        (state, tmp)
    }

    /// Router expone `/health` y endpoints `/translate`, `/voices/clone`, `/dub` (7 rutas públicas)
    #[test]
    fn build_router_exposes_new_endpoints() {
        let (state, _dir) = test_state();
        let state = Arc::new(state);
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
        let res = with_heartbeats(&tx, "test_stage", work)
            .await
            .expect("sin desconexión hay resultado");
        assert_eq!(res.expect("join ok"), "hecho");
        drop(tx);
        let mut heartbeats = 0;
        while let Some(line) = rx.recv().await {
            let ev: Value = serde_json::from_str(&line).expect("cada evento es JSON");
            if ev["event"] == "heartbeat" {
                assert_eq!(ev["stage"], "test_stage");
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

    /// Espera a que el canal entregue al menos `target` latidos de la etapa
    /// `stage`, con `deadline` como límite duro, y devuelve todos los eventos
    /// leídos hasta ese momento (los que sobren, para las aserciones
    /// posteriores).
    ///
    /// El hecho que espera es «llegaron N latidos de esta etapa», no «ha pasado
    /// un tiempo»: por eso sondea el canal en vez de dormir, y por eso al
    /// agotarse el plazo falla diciendo cuántos llegaron y en cuánto.
    async fn wait_for_heartbeats(
        rx: &mut tokio::sync::mpsc::Receiver<String>,
        stage: &str,
        target: usize,
        deadline: std::time::Duration,
    ) -> Vec<Value> {
        let limit = std::time::Instant::now() + deadline;
        let mut events = Vec::new();
        let mut seen = 0usize;
        while seen < target {
            let remaining = limit.saturating_duration_since(std::time::Instant::now());
            let line = tokio::time::timeout(remaining, rx.recv())
                .await
                .unwrap_or_else(|_| {
                    panic!(
                        "no llegaron {target} latidos de la etapa `{stage}` en {deadline:?} \
                         (llegaron {seen})"
                    )
                })
                .expect("la fase sigue viva: su emisor no debe cerrarse antes de los latidos");
            let event: Value = serde_json::from_str(&line).expect("cada evento es JSON");
            if event["event"] == "heartbeat" && event["stage"] == stage {
                seen += 1;
            }
            events.push(event);
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
        let (state, _dir) = test_state();
        let state = Arc::new(state);
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
        // No se duerme a ciegas: se espera a que la fase haya emitido los dos
        // latidos `queued` que este test afirma, y solo entonces se suelta el
        // lock. Un plazo fijo dependería de que la máquina llegue a esos 150 ms.
        let mut events =
            wait_for_heartbeats(&mut rx, "queued", 2, std::time::Duration::from_secs(5)).await;
        drop(guard);
        let res = phase.await.expect("join").expect("hay resultado");
        assert_eq!(res.expect("el trabajo termina bien"), 7);
        events.extend(drain_events(&mut rx));
        assert!(heartbeats_with_stage(&events, "queued") >= 2);
        assert_eq!(heartbeats_with_stage(&events, "warming"), 0);
    }

    /// Si el daemon está calentando, la espera del lock late con `warming`.
    #[tokio::test]
    async fn synthesis_phase_emits_warming_stage_during_warmup() {
        let (state, _dir) = test_state();
        let state = Arc::new(state);
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
        // Igual que arriba: se espera el hecho (los latidos `warming`), no un
        // tiempo, antes de soltar el lock.
        let mut events =
            wait_for_heartbeats(&mut rx, "warming", 2, std::time::Duration::from_secs(5)).await;
        drop(guard);
        assert!(phase.await.expect("join").is_some());
        events.extend(drain_events(&mut rx));
        assert!(heartbeats_with_stage(&events, "warming") >= 2);
        assert_eq!(heartbeats_with_stage(&events, "queued"), 0);
    }

    /// Un trabajo más largo que varios latidos, pero dentro del presupuesto,
    /// mantiene los latidos `synthesis` y termina con `Ok`.
    ///
    /// El trabajo no «dura» 150 ms: espera a que la prueba lo suelte, y la
    /// prueba lo suelta cuando ha visto tres latidos. Así los tres latidos son
    /// un hecho observado y no una consecuencia de acertar con una duración.
    #[tokio::test]
    async fn synthesis_phase_slow_job_keeps_heartbeats_and_succeeds() {
        let (state, _dir) = test_state();
        let state = Arc::new(state);
        let (tx, mut rx) = tokio::sync::mpsc::channel::<String>(64);
        let (release, hold) = std::sync::mpsc::channel::<()>();
        let phase_state = state.clone();
        let phase = tokio::spawn(async move {
            run_synthesis_phase_with(
                &tx,
                &phase_state,
                std::time::Duration::from_secs(5),
                TEST_HEARTBEAT,
                std::time::Duration::from_secs(1),
                move |_cancel| {
                    // Bloquea el hilo del trabajo hasta que la prueba lo libera.
                    hold.recv().map_err(|_| {
                        anyhow::anyhow!("la prueba soltó el trabajo antes de tiempo")
                    })?;
                    Ok("listo")
                },
            )
            .await
        });
        let mut events =
            wait_for_heartbeats(&mut rx, "synthesis", 3, std::time::Duration::from_secs(5)).await;
        release
            .send(())
            .expect("el trabajo sigue esperando a que lo liberen");
        let res = phase.await.expect("join").expect("hay resultado");
        assert_eq!(res.expect("el trabajo termina bien"), "listo");
        events.extend(drain_events(&mut rx));
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
        let (state, _dir) = test_state();

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
        let (state, _dir) = test_state();
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
        let (state, _dir) = test_state();
        let state = Arc::new(state);

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

    /// Dub handler rechaza con 400 `audio_too_long` un audio que supera el tope.
    /// Es contrato, pero el `cfg` no es de clase: la validación vive en el bloque
    /// `#[cfg(feature = "native-stt")]` de `dub_handler` (sin el feature la ruta
    /// responde `stt_unsupported`), así que la aserción solo existe con el feature.
    #[cfg(feature = "native-stt")]
    #[tokio::test]
    async fn dub_handler_audio_too_long() {
        use axum::body::Body;
        use base64::Engine;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let (state, _dir) = test_state();
        let state = Arc::new(state);
        let app = build_router_with_state(state);
        // 41 s de silencio PCM 16 kHz mono (i16 little-endian).
        let silence = vec![0u8; 41 * 16_000 * 2];
        let body = json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(silence),
            "voice": "default",
            "source_language": "es",
            "target_language": "es",
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

    /// Codifica `samples` muestras de silencio PCM i16 como el `audio_b64` de `/transcribe`.
    fn silence_transcribe_body(samples: usize) -> Value {
        use base64::Engine;
        json!({ "audio_b64": base64::engine::general_purpose::STANDARD.encode(vec![0u8; samples * 2]) })
    }

    /// Un audio de exactamente 300 s (el tope de transcripción) se acepta.
    #[test]
    fn validate_transcribe_accepts_audio_at_limit() {
        let body = silence_transcribe_body(300 * 16_000);
        let result = validate_transcribe_input(&body);
        assert_eq!(result.map(|pcm| pcm.len()).ok(), Some(300 * 16_000));
    }

    /// Una sola muestra por encima de los 300 s se rechaza como `audio_too_long`.
    #[test]
    fn validate_transcribe_rejects_audio_over_limit() {
        let body = silence_transcribe_body(300 * 16_000 + 1);
        let result = validate_transcribe_input(&body);
        assert_eq!(result.err().map(|e| e.reason), Some("audio_too_long"));
    }

    /// Un `audio_b64` que decodifica a 0 muestras PCM se rechaza como `empty_audio`.
    #[test]
    fn validate_transcribe_empty_pcm_is_empty_audio() {
        let body = json!({ "audio_b64": "" });
        let result = validate_transcribe_input(&body);
        assert_eq!(result.err().map(|e| e.reason), Some("empty_audio"));
    }

    /// Un texto vacío o solo de espacios no se traduce.
    #[test]
    fn validate_translate_empty_text_is_empty_text() {
        let result = validate_translate_input(
            &json!({ "text": "   ", "source_language": "es", "target_language": "en" }),
        );
        assert_eq!(result.err().map(|e| e.reason), Some("empty_text"));
    }

    /// Con el mismo idioma de origen y destino no hay nada que traducir.
    #[test]
    fn validate_translate_same_language_is_same() {
        let result = validate_translate_input(
            &json!({ "text": "hola", "source_language": "es", "target_language": "es" }),
        );
        assert!(
            matches!(result, Ok(TranslateInput::Same)),
            "resultado: {result:?}"
        );
    }

    /// Idiomas distintos (`es`→`en` o `en`→`es`) se devuelven como par.
    #[test]
    fn validate_translate_distinct_languages_is_pair() {
        let result = validate_translate_input(
            &json!({ "text": "hola", "source_language": "es", "target_language": "en" }),
        );
        assert_eq!(
            result,
            Ok(TranslateInput::Pair {
                source: "es".to_string(),
                target: "en".to_string()
            })
        );
    }

    /// Un idioma fuera de `es`/`en` (incluido `es-latam`) es error de uso.
    #[test]
    fn validate_translate_unknown_language_is_usage_error() {
        for bad in ["fr", "es-latam"] {
            let result = validate_translate_input(
                &json!({ "text": "hola", "source_language": "es", "target_language": bad }),
            );
            assert_eq!(result.err().map(|e| e.reason), Some("usage_error"), "{bad}");
        }
    }

    /// Un idioma ausente, o enviado con una clave alias, es error de uso.
    #[test]
    fn validate_translate_missing_or_aliased_language_is_usage_error() {
        let bodies = [
            json!({ "text": "hola", "target_language": "en" }),
            json!({ "text": "hola", "source_language": "es" }),
            json!({ "text": "hola", "from": "es", "to": "en" }),
            json!({ "text": "hola", "source": "es", "target": "en" }),
        ];
        for body in bodies {
            let result = validate_translate_input(&body);
            assert_eq!(
                result.err().map(|e| e.reason),
                Some("usage_error"),
                "{body}"
            );
        }
    }

    /// `temperature` presente pero no numérica es error de uso; ausente es `None`.
    #[test]
    fn optional_temperature_rejects_non_numeric() {
        assert_eq!(optional_temperature(&json!({})), Ok(None));
        assert_eq!(
            optional_temperature(&json!({ "temperature": 0.5 })),
            Ok(Some(0.5))
        );
        let bad = optional_temperature(&json!({ "temperature": "alta" }));
        assert_eq!(bad.err().map(|e| e.reason), Some("usage_error"));
    }

    /// El motor y el estado comparten el registro: anotar un residente no borra
    /// la identidad ni la dirección del daemon del pidfile.
    #[test]
    fn engine_and_state_share_daemon_registry() {
        use avi_process::registry::{observe_pid_file, PidFileObservation};
        use avi_tts::ResidentRegistry;

        let dir = tempfile::tempdir().unwrap();
        let registry = Arc::new(registry::DaemonRegistry::new(dir.path().to_path_buf()));
        let engines = DaemonEngines {
            tts_engine: Qwen3TtsEngine::new(Some(registry.clone())).unwrap(),
            registry: registry.clone(),
            #[cfg(any(feature = "native-stt", feature = "native-translation"))]
            inference_engines: None,
        };
        let state = DaemonState::assemble(
            VoiceStore::at(dir.path().join("voices")),
            SpeechStore::at(dir.path().join("speech")),
            ModelStore::new().unwrap(),
            engines,
        )
        .unwrap();
        assert!(Arc::ptr_eq(&registry, &state.registry));
        state
            .registry
            .register_daemon(&"127.0.0.1:7001".parse().unwrap())
            .unwrap();
        let resident = avi_process::ProcessIdentity {
            start: "1".to_string(),
            image: "qwen_tts.exe".to_string(),
        };
        registry.register(4321, &resident, 40_000).unwrap();
        let PidFileObservation::Present(file) = observe_pid_file(dir.path()) else {
            panic!("el pidfile debe existir y ser legible");
        };
        assert_eq!(file.addr.as_deref(), Some("127.0.0.1:7001"));
        assert!(file.identity.is_some());
        assert!(file.started_at.is_some());
        assert_eq!(file.resident_identity, Some(resident));
    }

    /// El estado de contrato ancla los almacenes bajo el directorio recibido:
    /// la raíz se afirma antes de inicializar para no escribir en la instalación real.
    #[test]
    fn contract_state_anchors_stores_under_given_dir() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let tmp = dir.path().to_path_buf();
        let state = DaemonState::for_contract_tests(
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
        state
            .voice_store
            .ensure_initialized()
            .expect("inicializar voces");
        assert!(tmp.join("voices").join("default").is_dir());
        state
            .registry
            .register_daemon(&"127.0.0.1:7001".parse().unwrap())
            .unwrap();
        assert!(tmp.join("voices/contract-daemon/daemon.pid").exists());
        assert!(!tmp.join("daemon.pid").exists());
    }

    /// Envía un POST JSON al router y devuelve el estado y el cuerpo completo.
    async fn post_json(uri: &str, body: Value) -> (StatusCode, Vec<u8>) {
        use axum::body::Body;
        use http_body_util::BodyExt;
        use tower::ServiceExt;
        let (state, _dir) = test_state();
        let state = Arc::new(state);
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

    /// El evento `started` de `/synthesize` mide `text_length` en caracteres: «canción»
    /// tiene 7 caracteres y 8 bytes. La temperatura fuera de rango cierra el stream
    /// sin llegar al motor.
    ///
    /// El evento nombra además la operación con la clave `operation`, que es
    /// lo que permite al cliente decir «Iniciando síntesis…» en vez de
    /// anunciar una operación genérica.
    #[tokio::test]
    async fn synthesize_start_event_counts_chars() {
        let (_, bytes) = post_json(
            "/synthesize",
            json!({
                "text": "canción", "voice": "default", "temperature": 5.0,
                "source_language": "es", "target_language": "es"
            }),
        )
        .await;
        let body = String::from_utf8_lossy(&bytes);
        let start: Value = body
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .find(|ev| ev["event"] == "started")
            .expect("el stream emite el evento started");
        assert_eq!(start["text_length"], 7);
        assert_eq!(start["operation"], "síntesis");
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

    /// Cada `reason` literal que emite el código de producción del daemon está
    /// declarado en la tabla única de códigos de salida. Se leen cuatro formas: la
    /// clave `"reason"` y su espejo `"error"` de los envelopes, el campo `reason:` de
    /// los errores tipados y la línea `error=<reason>` del fichero ready. Un `reason`
    /// con código 1 también debe estar declarado: la tabla no se infiere por defecto.
    #[test]
    fn every_daemon_reason_is_in_exit_code_table() {
        let source = include_str!("lib.rs");
        let production = source
            .split("#[cfg(test)]")
            .next()
            .expect("el código de producción precede al módulo de pruebas");
        let mut found = 0;
        for key in ["\"reason\": \"", "\"error\": \"", "reason: \"", "error="] {
            for (i, _) in production.match_indices(key) {
                let rest = &production[i + key.len()..];
                let reason: String = rest
                    .chars()
                    .take_while(|c| c.is_ascii_lowercase() || *c == '_')
                    .collect();
                if reason.is_empty() {
                    continue;
                }
                // Los literales entre comillas deben cerrar justo tras el nombre; en
                // `error=` el nombre lo cierra cualquier carácter que no sea del reason.
                if key.ends_with('"') && !rest[reason.len()..].starts_with('"') {
                    continue;
                }
                found += 1;
                assert!(
                    avi_core::exit_codes::ExitCode::is_declared_reason(&reason),
                    "el reason {reason} que emite el daemon no está en la tabla"
                );
            }
        }
        assert!(
            found > 0,
            "la búsqueda de literales no encontró ningún reason"
        );
    }

    /// Cada clase de fallo sobrevive la ida y vuelta por el fichero ready.
    #[test]
    fn ready_failure_round_trips_each_class() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        for err in [
            StartupError::PortInUse { port: 8765 },
            StartupError::WarmVoiceMissing {
                voice: "nadie".to_string(),
            },
            StartupError::ModelMissing {
                message: "falta Parakeet: ejecuta setup".to_string(),
            },
            StartupError::Failed {
                message: "motor caído".to_string(),
            },
        ] {
            write_ready_failure(&path, &err).unwrap();
            assert_eq!(read_ready_signal(&path), Some(ReadySignal::Failed(err)));
        }
    }

    /// El registro de éxito se lee como `Ready` con el PID propio y sin `error`.
    #[test]
    fn ready_success_reads_addr_and_pid() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        let addr: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        write_ready_file(&path, &addr, "warming").unwrap();
        let Some(ReadySignal::Ready { addr, pid, .. }) = read_ready_signal(&path) else {
            panic!("el registro de éxito se lee como Ready");
        };
        assert_eq!(addr, "127.0.0.1:8765");
        assert_eq!(pid, Some(std::process::id()));
    }

    /// El ready publica la identidad propia del daemon y el lector la devuelve.
    #[test]
    fn ready_file_publishes_own_identity() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        let addr: SocketAddr = "127.0.0.1:8765".parse().unwrap();
        write_ready_file(&path, &addr, "ok").unwrap();
        let own = avi_process::process_identity(std::process::id()).expect("identidad propia");
        let content = std::fs::read_to_string(&path).unwrap();
        assert!(
            content.contains(&format!("start={}", own.start)),
            "{content}"
        );
        assert!(
            content.contains(&format!("image={}", own.image)),
            "{content}"
        );
        let Some(ReadySignal::Ready { identity, .. }) = read_ready_signal(&path) else {
            panic!("el registro de éxito se lee como Ready");
        };
        assert_eq!(identity, Some(own));
    }

    /// Fichero ausente o vacío: aún no hay señal.
    #[test]
    fn ready_missing_or_empty_is_none() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        assert_eq!(read_ready_signal(&path), None);
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_ready_signal(&path), None);
    }

    /// Una clase desconocida se entrega como `Failed` con la clase en el mensaje.
    #[test]
    fn ready_unknown_failure_class_is_failed() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        std::fs::write(&path, "error=cosa_nueva\n").unwrap();
        match read_ready_signal(&path) {
            Some(ReadySignal::Failed(StartupError::Failed { message })) => {
                assert!(message.contains("cosa_nueva"), "{}", message)
            }
            other => panic!("se esperaba Failed, llegó {:?}", other),
        }
    }

    /// Un mensaje multilínea queda en una sola línea del registro.
    #[test]
    fn ready_failure_message_is_flattened() {
        let dir = tempfile::tempdir().expect("directorio temporal");
        let path = dir.path().join("daemon.ready");
        let err = StartupError::Failed {
            message: "línea uno\r\nlínea dos\nlínea tres".to_string(),
        };
        write_ready_failure(&path, &err).unwrap();
        assert_eq!(
            read_ready_signal(&path),
            Some(ReadySignal::Failed(StartupError::Failed {
                message: "línea uno  línea dos línea tres".to_string(),
            }))
        );
    }
}
