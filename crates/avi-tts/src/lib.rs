use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// Defaults de muestreo del motor Qwen3-TTS (fuente: `docs/server.md:140-141`
/// y `qwen_tts_server.c:295-298`); los defaults del host deben coincidir para
/// que la omisión de flags HTTP/CLI sea idéntica a pasarlos explícitos.
pub const DEFAULT_TEMPERATURE: f32 = 0.5;
pub const DEFAULT_TOP_K: u32 = 50;
pub const DEFAULT_TOP_P: f32 = 1.0;
pub const DEFAULT_REP_PENALTY: f32 = 1.05;

/// Puerto por defecto del servidor residente (el daemon del host ocupa el 8765).
pub const DEFAULT_PORT: u16 = 8766;

/// Nombre de imagen del proceso residente (`qwen_tts`). Fuente única para la
/// resolución del binario y para el barrido por imagen de último recurso
/// (`resident::sweep_resident_by_image`). El residente tiene imagen propia
/// —a diferencia del daemon, que comparte imagen con el CLI—, así que el
/// kill-por-imagen es seguro sólo para el residente.
#[cfg(windows)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts.exe";
#[cfg(unix)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts";

/// Resuelve el puerto del servidor residente con override por `QWEN3_TTS_PORT`.
pub fn default_port() -> u16 {
    std::env::var("QWEN3_TTS_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT)
}

/// Opciones de generación para la síntesis de voz (API agnóstica del motor)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationOptions {
    pub language: String,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    /// Penalización de repetición del motor (el motor la usa por defecto a
    /// 1.05).
    pub rep_penalty: f32,
    pub seed: Option<u64>,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            language: "es".to_string(),
            temperature: DEFAULT_TEMPERATURE,
            top_k: DEFAULT_TOP_K,
            top_p: DEFAULT_TOP_P,
            rep_penalty: DEFAULT_REP_PENALTY,
            seed: None,
        }
    }
}

impl GenerationOptions {
    /// Config de producción validada por oído: `temperature=0.35` y `seed=4`
    /// fijo, resto de campos igual a `Default`. El `temperature=0` previo corría
    /// el Talker en greedy argmax sobre un clon x-vector-only sin plantilla de
    /// prosodia, produciendo prosodia plana/extraña; 0.35 reactiva el muestreo
    /// estocástico (y vuelve efectivo el `seed`) preservando la naturalidad sin
    /// soltarse como el default del motor (0.9). No sustituye a `Default` (que
    /// debe seguir coincidiendo con los defaults del motor) sino que es la
    /// superficie que cablea la síntesis de producción (`Qwen3TtsEngine::synthesize`).
    /// `seed 4` fijado por sweep 2026-08-24: 10 frases ES, bench.qvoice --int4 -j4
    /// con temperature=0.35, usando WSL con seed 42 como oráculo, 3 oyentes
    /// comparando seed 4 (4/10) contra WSL (6/10), con WER máximo 0.000 y
    /// similitud de hablante mínima 0.822 en PASS (target/seed-sweep/wer.csv,
    /// speaker_sim.csv). El seed 42 previo también pasa, pero seed 4 iguala la
    /// prosodia nativa de Windows sin depender de WSL.
    pub fn production() -> Self {
        Self {
            temperature: 0.35,
            seed: Some(4),
            ..Self::default()
        }
    }

    /// Resuelve la temperatura opcional del CLI a opciones efectivas: sin flag
    /// se usa la config de producción; con flag se sobrescribe la temperatura.
    pub fn with_temperature(temperature: Option<f32>) -> Self {
        let mut opts = Self::production();
        if let Some(t) = temperature {
            opts.temperature = t;
        }
        opts
    }
}

/// Opciones de prosodia (ganancia y tempo), serializables al body HTTP.
/// `EmotionOptions` es no-op en el modelo 0.6B: se serializa si se usa, sin
/// prometer control emocional (restricción del plan de migración §2.4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProsodyOptions {
    pub volume: Option<f32>,
    pub rate: Option<f32>,
}

/// Opciones de emoción (no-op en 0.6B; solo se transporta el campo `emotion`).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EmotionOptions {
    pub emotion: Option<String>,
}

/// Perfil de voz que encapsula la referencia de audio / embeddings
#[derive(Debug, Clone)]
pub struct VoiceProfile {
    pub name: String,
    pub qvoice_path: Option<PathBuf>,
}

/// Trait público del motor de síntesis TTS
pub trait TtsEngine: Send + Sync {
    fn synthesize_with_options(
        &self,
        text: &str,
        profile: &VoiceProfile,
        options: &GenerationOptions,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf>;
}

/// Voz resuelta hacia la semántica del motor: preset del servidor o voz clonada
/// cargada al arranque con `--load-voice <qvoice> --icl-only`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEngine {
    Preset(String),
    Cloned(PathBuf),
}

/// Resolución voz → motor: una voz con `reference.qvoice` es clonada;
/// cualquier otra es preset. `default` con `reference.qvoice` graft resuelve
/// como `Cloned`; sin referencia resuelve como `Preset("default")` y el
/// motor cae a `ryan` por `spk_table` sólo si el binario lo exige.
pub fn resolve_voice_engine(voice: &str, qvoice: Option<&Path>) -> VoiceEngine {
    if let Some(q) = qvoice {
        if q.is_file() {
            return VoiceEngine::Cloned(q.to_path_buf());
        }
    }
    VoiceEngine::Preset(voice.to_string())
}

/// Resolución del binario del motor por capas:
/// 1. `QWEN3_TTS_BIN`; 2. `<exe_dir>/vendor/qwen3-tts/qwen_tts(.exe)`; 3. `<cwd>/vendor/qwen3-tts/qwen_tts(.exe)`;
/// 4. búsqueda en `PATH`.
fn resolve_binary() -> Option<PathBuf> {
    if let Some(b) = std::env::var_os("QWEN3_TTS_BIN") {
        let p = PathBuf::from(b);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join(if cfg!(windows) {
                "vendor/qwen3-tts/qwen_tts.exe"
            } else {
                "vendor/qwen3-tts/qwen_tts"
            });
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    let vendored = PathBuf::from(if cfg!(windows) {
        "vendor/qwen3-tts/qwen_tts.exe"
    } else {
        "vendor/qwen3-tts/qwen_tts"
    });
    if vendored.is_file() {
        return Some(vendored);
    }
    let name = RESIDENT_IMAGE_NAME;
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(name);
            if cand.is_file() {
                return Some(cand);
            }
        }
    }
    None
}

/// Resolución del directorio de pesos por capas:
/// 1. `QWEN3_TTS_MODEL_DIR`; 2. directorio hermano del binario
///    (`<dir del bin>/qwen3-tts-0.6b`); 3. `<exe_dir>/vendor/qwen3-tts/qwen3-tts-0.6b`;
/// 4. snapshot HF `ModelStore::model_snapshot_path("qwen3-tts-0.6b")`; 5. `<cwd>/vendor/qwen3-tts/qwen3-tts-0.6b`.
fn resolve_model_dir(bin: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QWEN3_TTS_MODEL_DIR") {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Some(b) = bin {
        if let Some(parent) = b.parent() {
            let sibling = parent.join("qwen3-tts-0.6b");
            if sibling.is_dir() {
                return Some(sibling);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("vendor/qwen3-tts/qwen3-tts-0.6b");
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    if let Some(p) = avi_store::ModelStore::new().model_snapshot_path("qwen3-tts-0.6b") {
        if p.is_dir()
            && p.read_dir()
                .map(|mut i| i.next().is_some())
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    let vendored = PathBuf::from("vendor/qwen3-tts/qwen3-tts-0.6b");
    if vendored.is_dir() {
        return Some(vendored);
    }
    None
}

/// Resolución del directorio del modelo Base por capas, deliberadamente
/// separada de `resolve_model_dir`: solo la usa el clonado (`--ref-audio`),
/// que exige el modelo Base (`vendor/qwen3-tts/main.c:1848`), distinto del
/// CustomVoice usado por la síntesis general.
/// Orden: 1. `QWEN3_TTS_BASE_MODEL_DIR`; 2. directorio hermano del binario
/// (`<dir del bin>/qwen3-tts-0.6b-base`); 3. `<exe_dir>/vendor/qwen3-tts/qwen3-tts-0.6b-base`;
/// 4. snapshot HF `ModelStore::model_snapshot_path("qwen3-tts-0.6b-base")`;
/// 5. `<cwd>/vendor/qwen3-tts/qwen3-tts-0.6b-base`.
pub fn resolve_base_model_dir(bin: Option<&Path>) -> Option<PathBuf> {
    if let Some(d) = std::env::var_os("QWEN3_TTS_BASE_MODEL_DIR") {
        let p = PathBuf::from(d);
        if !p.as_os_str().is_empty() {
            return Some(p);
        }
    }
    if let Some(b) = bin {
        if let Some(parent) = b.parent() {
            let sibling = parent.join("qwen3-tts-0.6b-base");
            if sibling.is_dir() {
                return Some(sibling);
            }
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let cand = dir.join("vendor/qwen3-tts/qwen3-tts-0.6b-base");
            if cand.is_dir() {
                return Some(cand);
            }
        }
    }
    // Capa HF: snapshot cacheado por setup --with-voice-cloning
    if let Some(p) = avi_store::ModelStore::new().model_snapshot_path("qwen3-tts-0.6b-base") {
        if p.is_dir()
            && p.read_dir()
                .map(|mut i| i.next().is_some())
                .unwrap_or(false)
        {
            return Some(p);
        }
    }
    let vendored = PathBuf::from("vendor/qwen3-tts/qwen3-tts-0.6b-base");
    if vendored.is_dir() {
        return Some(vendored);
    }
    None
}

/// Motor Qwen3-TTS con servidor HTTP residente gestionado por el host como
/// único camino de síntesis (healthcheck y POST ambos acotados a 30 s).
pub struct Qwen3TtsEngine {
    pub server_url: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub model_dir: Option<PathBuf>,
    pub base_model_dir: Option<PathBuf>,
    resident: Mutex<Option<ResidentState>>,
    /// PID del proceso `qwen_tts.exe` arrancado (0 si no hay). Se usa en `shutdown`
    /// como señal binaria «hubo residente» (0 / no-0) para decidir si invocar el
    /// kill, SIN tomar el `Mutex<resident>` que el hilo `spawn_blocking(warmup)`
    /// retiene durante el spawn + `wait_health` (30 s) + síntesis HTTP (30 s).
    resident_pid: AtomicU32,
}

/// Ventana de la salud observada por petición: `retries=1`,
/// `interval_ms=2000` acotan el healthcheck a ~2 s por petición. Generoso
/// para un `GET /v1/health` sano en loopback (responde en ms) y suficiente
/// para fallar rápido ante un sumidero TCP que acepta y no responde, muy por
/// debajo del deadline de handler de 8 s (`avi-daemon`).
const HEALTH_OBS_RETRIES: usize = 1;
const HEALTH_OBS_INTERVAL_MS: u64 = 2000;

/// Estado del servidor residente: se indexa por voz — al cambiar
/// de voz se termina el residente anterior y se arranca otro con `--load-voice`.
struct ResidentState {
    resident: resident::Qwen3TtsResident,
    voice_key: String,
}

impl Qwen3TtsEngine {
    pub fn new(server_url: Option<String>) -> Self {
        let binary_path = resolve_binary();
        let model_dir = resolve_model_dir(binary_path.as_deref());
        let base_model_dir = resolve_base_model_dir(binary_path.as_deref());
        Self {
            server_url,
            binary_path,
            model_dir,
            base_model_dir,
            resident: Mutex::new(None),
            resident_pid: AtomicU32::new(0),
        }
    }

    /// Detén el residente HTTP gestionado, SIN bloquear. Se llama desde
    /// `shutdown_handler` (avi-daemon) antes de notificar el graceful shutdown.
    ///
    /// Matar al residente hace fallar la síntesis en curso (el residente es el
    /// único camino, sin fallback), así que el hilo `spawn_blocking(warmup)`
    /// retorna y el runtime cierra el proceso limpio.
    ///
    /// No bloqueante ni dependiente del `Mutex<resident>`: el hilo del warmup lo
    /// retiene durante el spawn + `wait_health` + síntesis HTTP, así que
    /// `self.resident.lock()` se colgaría. Por eso se mata el árbol preciso por
    /// PID (señal que no requiere el lock, con verificación inmediata sin espera);
    /// el `shutdown` mata por PID exacto, no por imagen —si el árbol preciso no lo
    /// termina, la verificación con deadline la hace el llamante (graceful del
    /// daemon / parada del CLI) y el fallo es ruidoso. El barrido por imagen
    /// (`sweep_resident_by_image`) queda como último recurso del camino de
    /// reclamo cuando no hay `resident_pid` registrado. La recolección del estado
    /// se hace best-effort con `try_lock` (si el warmup lo tiene, no esperamos:
    /// el proceso ya está muerto y su `Drop` recolectará el estado al liberarse).
    pub fn shutdown(&self) {
        let pid = self.resident_pid.load(Ordering::Relaxed);
        if pid != 0 {
            crate::resident::kill_tree_resident_by_pid(pid);
        }
        if let Ok(mut guard) = self.resident.try_lock() {
            *guard = None;
        }
    }

    /// Síntesis con temperatura opcional del CLI: sin flag usa la config de
    /// producción; con flag sobrescribe la temperatura (ya validada en el CLI).
    pub fn synthesize_with_temperature(
        &self,
        text: &str,
        voice: &str,
        temperature: Option<f32>,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        let options = GenerationOptions::with_temperature(temperature);
        let qvoice_path = avi_store::VoiceStore::new().find_reference(voice);
        let profile = VoiceProfile {
            name: voice.to_string(),
            qvoice_path,
        };
        self.synthesize_with_options(text, &profile, &options, output_path)
    }

    /// Intentar la síntesis vía HTTP local (servidor manual o residente).
    #[allow(clippy::too_many_arguments)]
    fn synthesize_via_http(
        &self,
        server_url: &str,
        text: &str,
        voice: &VoiceEngine,
        options: &GenerationOptions,
        prosody: Option<&ProsodyOptions>,
        emotion: Option<&EmotionOptions>,
        out_path: &Path,
    ) -> Result<()> {
        let body = build_tts_body(text, voice, options, prosody, emotion).to_string();
        let (status, bytes) = http_exchange(
            &format!("{}/v1/tts", server_url),
            "POST",
            Some(&body),
            Duration::from_secs(30),
        )?;
        if (200..300).contains(&status) {
            std::fs::write(out_path, bytes)?;
            Ok(())
        } else {
            Err(anyhow!(
                "Servidor HTTP Qwen3-TTS devolvió código de error: {}",
                status
            ))
        }
    }

    /// Arranca un residente fresco para `voice`: construye
    /// `load_voice`, hace `spawn` (que ya trae su propio `wait_health` de
    /// arranque, por lo que nace sano o falla con diagnóstico), actualiza
    /// `resident_pid` y ensambla el `ResidentState`. Compartido por el camino
    /// de cambio de voz y por el rearranque ante degradación del residente.
    fn start_resident(
        &self,
        model_dir: &Path,
        voice: &VoiceEngine,
        voice_key: String,
    ) -> Result<ResidentState> {
        let load_voice = match voice {
            VoiceEngine::Cloned(p) => Some(p.as_path()),
            VoiceEngine::Preset(_) => None,
        };
        let port = default_port();
        let spawned = resident::Qwen3TtsResident::spawn(model_dir, port, load_voice)?;
        self.resident_pid.store(spawned.pid(), Ordering::Relaxed);
        // Contabilidad en disco acoplada al store en memoria — actualiza
        // `resident_pid` en `daemon.pid` sin fichero propio (best-effort: si el
        // daemon corre en foreground sin pidfile, no hay nada que actualizar).
        update_resident_pid_in_pidfile(spawned.pid());
        Ok(ResidentState {
            resident: spawned,
            voice_key,
        })
    }

    /// Síntesis vía servidor residente: arranca (o reutiliza) el residente de
    /// la voz solicitada y hace `POST /v1/tts`.
    ///
    /// La reutilización por `voice_key` no bastaba — un residente colgado
    /// tras el warmup se reutilizaba indefinidamente y todo `POST /v1/tts`
    /// se colgaba. Antes de reusar se verifica la salud real
    /// (`health_check`, `try_wait` + `GET /v1/health`); si está degradado se
    /// rearranca de forma determinista (mata el árbol por PID, suelta el
    /// estado viejo, `spawn` fresco). Sin fallback: si el `spawn` falla, el
    /// error se propaga y la petición falla con diagnóstico.
    ///
    /// Consideración 5 (narrowing del lock): el `guard` cubre solo la
    /// decisión reutilizar/arrancar, la salud observada, el eventual
    /// rearranque y la lectura del puerto (`u16` copiable); se suelta antes
    /// del `POST /v1/tts`, que corre sin lock. La serialización del uso del
    /// residente ya la da la capa superior (`synthesis_lock` en el daemon,
    /// secuencialidad en el CLI directo), así que la siguiente petición
    /// reclama el residente de inmediato en vez de esperar tras un hilo
    /// huérfano reteniendo el lock durante 30 s.
    fn synthesize_via_resident(
        &self,
        text: &str,
        voice: &VoiceEngine,
        options: &GenerationOptions,
        out_path: &Path,
    ) -> Result<()> {
        let model_dir = self
            .model_dir
            .as_ref()
            .ok_or_else(|| anyhow!("El modelo de síntesis Qwen3-TTS no está provisionado."))?;
        let voice_key = match voice {
            VoiceEngine::Preset(n) => format!("preset:{}", n),
            VoiceEngine::Cloned(p) => format!("clone:{}", p.display()),
        };
        let url = {
            let mut guard = self.resident.lock().unwrap();
            if guard.as_ref().map(|s| s.voice_key.as_str()) != Some(voice_key.as_str()) {
                *guard = Some(self.start_resident(model_dir, voice, voice_key)?);
            } else if let Err(_e) = guard
                .as_mut()
                .expect("reutilización verificada arriba")
                .resident
                .health_check(HEALTH_OBS_RETRIES, HEALTH_OBS_INTERVAL_MS)
            {
                // Residente degradado (crash o hang): rearranque determinista.
                let pid = guard
                    .as_ref()
                    .expect("residente reutilizado")
                    .resident
                    .pid();
                resident::kill_tree_resident_by_pid(pid);
                *guard = None;
                *guard = Some(self.start_resident(model_dir, voice, voice_key)?);
            }
            let state = guard
                .as_ref()
                .expect("residente arrancado o reutilizado sano");
            format!("http://127.0.0.1:{}", state.resident.port)
        };
        self.synthesize_via_http(&url, text, voice, options, None, None, out_path)
    }
}

/// Actualiza `resident_pid` en `daemon.pid` preservando el resto del esquema
/// como campo plano. Escritura atómica por tmp+rename; best-effort y
/// silenciosa: si no hay pidfile (p. ej. `serve` en foreground) o no parsea,
/// no hay nada que actualizar y se ignora. La lectura tolerante vive en el CLI
/// (`read_resident_pid`: ausente = 0/desconocido).
fn update_resident_pid_in_pidfile(pid: u32) {
    let path = avi_store::data_dir().join("daemon.pid");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    v["resident_pid"] = serde_json::Value::from(pid);
    let tmp = path.with_extension("pid.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

impl TtsEngine for Qwen3TtsEngine {
    fn synthesize_with_options(
        &self,
        text: &str,
        profile: &VoiceProfile,
        options: &GenerationOptions,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        let path = output_path
            .cloned()
            .unwrap_or_else(|| PathBuf::from("output.wav"));

        // La voz clonada se resuelve por `reference.qvoice` (`qvoice_path`); una
        // voz sin él resuelve como preset del motor.
        let qvoice = profile
            .qvoice_path
            .as_deref()
            .filter(|q| q.is_file())
            .map(|q| q.to_path_buf());
        let voice = resolve_voice_engine(&profile.name, qvoice.as_deref());

        // 1. HTTP manual configurado (solo presets; la voz clonada exige un
        //    servidor arrancado con su `--load-voice`, que solo gestiona el residente).
        if let Some(url) = &self.server_url {
            if matches!(voice, VoiceEngine::Preset(_))
                && self
                    .synthesize_via_http(url, text, &voice, options, None, None, &path)
                    .is_ok()
            {
                return Ok(path);
            }
        }

        // 2. Servidor residente gestionado por el host: único
        //    camino restante, con healthcheck (30 s) y POST (30 s) acotados.
        //    El texto viaja por body HTTP JSON, ruta segura para UTF-8 acentuado
        //    (a diferencia del argv de un subprocess en Windows).
        self.synthesize_via_resident(text, &voice, options, &path)?;
        Ok(path)
    }
}

/// Construye el body HTTP de `POST /v1/tts`: sin `format` (el
/// servidor lo ignora), claves solo-si-`Some`, y `speaker`/`language` omitidos
/// cuando la voz es clonada (el servidor conserva la voz y el idioma del
/// arranque, `docs/server.md:28-34`).
pub(crate) fn build_tts_body(
    text: &str,
    voice: &VoiceEngine,
    options: &GenerationOptions,
    prosody: Option<&ProsodyOptions>,
    emotion: Option<&EmotionOptions>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "text".to_string(),
        serde_json::Value::String(text.to_string()),
    );
    match voice {
        VoiceEngine::Preset(speaker) => {
            obj.insert(
                "speaker".to_string(),
                serde_json::Value::String(speaker.clone()),
            );
            obj.insert(
                "language".to_string(),
                serde_json::Value::String(options.language.clone()),
            );
        }
        VoiceEngine::Cloned(_) => {}
    }
    obj.insert(
        "temperature".to_string(),
        serde_json::Value::from(options.temperature),
    );
    obj.insert("top_k".to_string(), serde_json::Value::from(options.top_k));
    obj.insert("top_p".to_string(), serde_json::Value::from(options.top_p));
    obj.insert(
        "rep_penalty".to_string(),
        serde_json::Value::from(options.rep_penalty),
    );
    if let Some(seed) = options.seed {
        obj.insert("seed".to_string(), serde_json::Value::from(seed));
    }
    if let Some(p) = prosody {
        if let Some(v) = p.volume {
            obj.insert("volume".to_string(), serde_json::Value::from(v));
        }
        if let Some(r) = p.rate {
            obj.insert("rate".to_string(), serde_json::Value::from(r));
        }
    }
    if let Some(e) = emotion {
        if let Some(em) = &e.emotion {
            obj.insert("emotion".to_string(), serde_json::Value::String(em.clone()));
        }
    }
    serde_json::Value::Object(obj)
}

/// Cliente HTTP/1.1 mínimo sobre `TcpStream` (sin runtime async): suficiente
/// para `/v1/health` y `/v1/tts` del motor. Evita `reqwest::blocking`, que
/// paniquea al dropearse dentro del runtime tokio de la CLI ("Cannot drop a
/// runtime in a context where blocking is not allowed").
///
/// Envía `Connection: close` y lee la respuesta hasta EOF; devuelve
/// (código de estado, bytes del body).
fn http_exchange(
    url: &str,
    method: &str,
    body: Option<&str>,
    timeout: Duration,
) -> Result<(u16, Vec<u8>)> {
    let (host, port, path) = parse_http_url(url)?;
    let mut stream = std::net::TcpStream::connect(format!("{}:{}", host, port))?;
    stream.set_read_timeout(Some(timeout))?;
    stream.set_write_timeout(Some(timeout))?;
    let mut req = format!(
        "{} {} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n",
        method, path, host, port
    );
    if let Some(b) = body {
        req.push_str(&format!(
            "Content-Type: application/json\r\nContent-Length: {}\r\n",
            b.len()
        ));
    }
    req.push_str("\r\n");
    stream.write_all(req.as_bytes())?;
    if let Some(b) = body {
        stream.write_all(b.as_bytes())?;
    }
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf)?;
    let text = String::from_utf8_lossy(&buf);
    let header_end = text.find("\r\n\r\n").ok_or_else(|| {
        anyhow!(
            "Respuesta HTTP sin final de headers: {:?}",
            &text[..text.len().min(80)]
        )
    })?;
    let status_line = text[..header_end].lines().next().unwrap_or("");
    let status: u16 = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|c| c.parse().ok())
        .unwrap_or(0);
    Ok((status, buf[header_end + 4..].to_vec()))
}

/// Descompone `http://host:puerto/ruta` en sus tres partes.
fn parse_http_url(url: &str) -> Result<(String, u16, String)> {
    let rest = url
        .strip_prefix("http://")
        .ok_or_else(|| anyhow!("URL HTTP no soportada: {}", url))?;
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (
            h.to_string(),
            p.parse::<u16>()
                .map_err(|_| anyhow!("Puerto inválido en URL: {}", url))?,
        ),
        None => (authority.to_string(), 80),
    };
    Ok((host, port, path.to_string()))
}

/// Normaliza `ref_audio` (WAV de cualquier tasa/canales) al formato que exige el
/// clonado del motor —24 kHz / 16-bit / mono— escribiéndolo en un WAV temporal
/// único y devolviendo su ruta. El motor rechaza referencias que no sean 24 kHz;
/// el benchmark preprocesaba la referencia de la misma forma.
fn reference_24k_mono(ref_audio: &Path) -> Result<PathBuf> {
    let pcm = avi_audio::load_wav_24k_mono_pcm(ref_audio)?;
    let unique = format!(
        "avi_tts_ref24k_{}_{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let out_path = std::env::temp_dir().join(unique);
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&out_path, spec)?;
    for sample in &pcm {
        writer.write_sample(*sample)?;
    }
    writer.finalize()?;
    Ok(out_path)
}

/// Clona una voz desde `ref_audio` (WAV de cualquier tasa/canales) a `out_qvoice`
/// (`.qvoice` graft ICL) vía subprocess: `<bin> -d <model_dir> --ref-audio
/// <ref> --save-voice <out> --voice-name <name> -l <language>`. La referencia se
/// normaliza antes a 24 kHz mono (requisito del motor). Propaga el error con el
/// exit code del proceso.
pub fn clone_voice(
    model_dir: impl AsRef<Path>,
    ref_audio: &Path,
    out_qvoice: &Path,
    name: &str,
    language: &str,
) -> Result<()> {
    let bin = resolve_binary()
        .ok_or_else(|| anyhow!("El binario de clonado Qwen3-TTS no está provisionado."))?;
    let ref_wav = reference_24k_mono(ref_audio)?;
    let status = Command::new(&bin)
        .arg("-d")
        .arg(model_dir.as_ref())
        .arg("--ref-audio")
        .arg(&ref_wav)
        .arg("--save-voice")
        .arg(out_qvoice)
        .arg("--voice-name")
        .arg(name)
        .arg("-l")
        .arg(language)
        .status();
    let _ = std::fs::remove_file(&ref_wav);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "El subproceso de clonado Qwen3-TTS finalizó con código de error: {:?}",
            status.code()
        ))
    }
}

/// Servidor residente del motor Qwen3-TTS: spawn perezoso con
/// `--serve <puerto> --int4 -j 4 --stream [--load-voice <qvoice> --icl-only]`, healthcheck
/// `GET /v1/health` con reintentos y terminación del hijo en `Drop`.
pub mod resident {
    use super::*;
    #[cfg(test)]
    use std::io::Read;
    #[cfg(test)]
    use std::io::Write;
    #[cfg(test)]
    use std::net::TcpListener;
    use std::process::Child;
    use std::thread;
    use std::time::Duration;

    /// Gestor del proceso servidor del motor.
    pub struct Qwen3TtsResident {
        child: Option<Child>,
        pub port: u16,
        /// Ruta del fichero de log de stderr del motor (incluida en errores de healthcheck).
        #[allow(dead_code)]
        pub(crate) log_path: PathBuf,
    }

    /// Construye el `Command` de arranque del residente, sin
    /// I/O real: `-d <model_dir> --serve <port> --int4 -j 4 --stream
    /// [--load-voice <qvoice> --icl-only]`.
    pub(crate) fn build_resident_command(
        bin: &Path,
        model_dir: &Path,
        port: u16,
        load_voice: Option<&Path>,
    ) -> Command {
        let mut cmd = Command::new(bin);
        cmd.arg("-d")
            .arg(model_dir)
            .arg("--serve")
            .arg(port.to_string())
            .arg("--int4")
            .arg("-j")
            .arg("4")
            .arg("--stream");
        if let Some(lv) = load_voice {
            cmd.arg("--load-voice").arg(lv).arg("--icl-only");
        }
        cmd
    }

    impl Qwen3TtsResident {
        /// Arranca el motor con `--serve` en `port` y espera a que `/v1/health`
        /// responda (hasta 60 × 500 ms). Con `load_voice` (voz clonada) añade
        /// `--load-voice <qvoice> --icl-only` (el clonado solo aplica al arranque).
        pub fn spawn(
            model_dir: impl AsRef<Path>,
            port: u16,
            load_voice: Option<&Path>,
        ) -> Result<Self> {
            let bin = resolve_binary()
                .ok_or_else(|| anyhow!("El binario Qwen3-TTS no está provisionado."))?;
            let mut cmd = build_resident_command(&bin, model_dir.as_ref(), port, load_voice);
            // Redirige stderr del motor a un fichero de log (rotación por sesión).
            // stdin/stdout permanecen en null: el motor no necesita TTY ni stdin y su
            // stdout no se consume. stderr captura los ~20 `fprintf(stderr, *)` del
            // motor C (cuyos mensajes se perdían a null, dejando ciego el
            // diagnóstico del warmup, la síntesis por petición y la
            // terminación del residente).
            let log_path = resident_log_path();
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|e| {
                    anyhow!(
                        "No se pudo abrir el log de stderr del motor ({}): {}",
                        log_path.display(),
                        e
                    )
                })?;
            use std::process::Stdio;
            // Windows: `qwen_tts.exe` NO debe heredar handles ni abrir terminal del
            // padre. `DETACHED_PROCESS (0x8)` evita la ventana de consola independiente.
            // La herencia del pipe (write-end) del proceso abuelo (test CLI) se corta
            // en la raíz: el daemon que spawnea este motor ya desheredó sus STD vía
            // `SetHandleInformation` (`main::disinherit_standard_handles`); no existe
            // una creation flag que desactive la herencia. `Stdio::null` en stdin/stdout
            // cierra la herencia de stdin/tty; stderr va al log (Stdio::from marca el
            // handle no-heredable).
            // Árbol matable: a propósito SIN `CREATE_NEW_PROCESS_GROUP` ni
            // breakaway, para que el residente permanezca en el grupo/Job del daemon
            // y `taskkill /F /T /PID <daemon>` (o el Job con cierre) lo alcance.
            // En Unix tampoco se hace `setsid` aquí: hereda el grupo del daemon.
            // Tras la muerte del líder el residente reparentado se verifica
            // por PID y 8766 desde el CLI (`reclaim_degraded_residual`); los
            // dobles de test nunca reproducen ese reparentado.
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::from(log_file))
                    .creation_flags(0x00000008);
            }
            #[cfg(unix)]
            {
                cmd.stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::from(log_file));
            }
            // Riesgo R2 documentado: el motor enlaza en INADDR_ANY, no en loopback.
            eprintln!(
                "[avi-tts] Aviso: el motor Qwen3-TTS enlaza en todas las interfaces \
                 (INADDR_ANY), puerto {}. El servidor es accesible desde la red local.",
                port
            );
            let child = cmd.spawn().map_err(|e| {
                anyhow!(
                    "No se pudo arrancar el servidor Qwen3-TTS ({}): {}",
                    bin.display(),
                    e
                )
            })?;
            Self::spawn_with_child(child, port, log_path, 60, 500)
        }

        /// Arranca el healthcheck sobre un hijo ya lanzado (retries/intervalo
        /// configurables para los tests de reintentos). `log_path` se guarda en el
        /// struct para incluirse en errores de `wait_health`.
        pub(crate) fn spawn_with_child(
            child: Child,
            port: u16,
            log_path: PathBuf,
            retries: usize,
            interval_ms: u64,
        ) -> Result<Self> {
            let mut child = child;
            wait_health(&mut child, port, retries, interval_ms, log_path.as_path())?;
            Ok(Self {
                child: Some(child),
                port,
                log_path,
            })
        }

        /// PID del proceso `qwen_tts.exe` (0 si no hay). Permite matar el residente
        /// por PID durante `shutdown` sin tomar el `Mutex<resident>` del engine,
        /// evitando el deadlock con el warmup que lo retiene.
        pub fn pid(&self) -> u32 {
            self.child.as_ref().map(|c| c.id()).unwrap_or(0)
        }

        /// Salud observada por petición: reutiliza `wait_health` (combina
        /// `try_wait` para detectar *crash* y `GET /v1/health` para detectar
        /// *hang*) sobre el `child` del residente ya arrancado. Un healthcheck
        /// solo-HTTP perdería el diagnóstico de crash sin ganar nada.
        pub(crate) fn health_check(&mut self, retries: usize, interval_ms: u64) -> Result<()> {
            match self.child.as_mut() {
                Some(child) => wait_health(child, self.port, retries, interval_ms, &self.log_path),
                None => Err(anyhow!(
                    "El residente Qwen3-TTS no tiene proceso hijo asociado."
                )),
            }
        }
    }

    impl Drop for Qwen3TtsResident {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                // Cierre por árbol con recolección: el `shutdown()` previo ya mató el
                // árbol preciso por PID (imagen solo como último recurso); aquí
                // `kill+wait` recolecta el estado del `child` (ya muerto entonces,
                // o vivo en un drop normal). Insuficiente ante caída a medio
                // `spawn` (sin `Child` que recolectar): por eso `run_supervised`
                // reclama además el árbol propio previo con deadline y verificación
                // antes del siguiente `bind`.
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Viveza real de un PID a nivel de sistema (sin Mutex ni HTTP).
    /// `pub` para el camino de reclamo/parada del CLI: muerte por PID
    /// registrado + verificación de que ese PID quedó muerto; sin PID, el
    /// último recurso es `sweep_resident_by_image` + ausencia por imagen.
    pub fn resident_pid_alive(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        {
            let output = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();
            match output {
                Ok(o) if o.status.success() => {
                    String::from_utf8_lossy(&o.stdout).contains(&pid.to_string())
                }
                _ => false,
            }
        }
        #[cfg(unix)]
        {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    /// Termina el residente por su PID (sin Mutex). Windows: `taskkill /F /T
    /// /PID` cierra el árbol por parentesco real. Unix: `kill -9` al PID
    /// directo — el residente se spawnea a propósito SIN grupo/sesión propia
    /// (hereda el grupo del daemon para que su cierre lo arrastre), así que un
    /// `kill` al grupo `-<pid>` apuntaría a un pgid nunca establecido; el
    /// árbol/grupo lo cierra el daemon, aquí solo se termina el PID puntual del
    /// camino de reclamo/parada del CLI. No verifica: el llamante
    /// combina con `resident_pid_alive` o recolecta el estado vía `Child`.
    pub fn kill_tree_resident_by_pid(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        {
            Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(unix)]
        {
            Command::new("kill")
                .args(["-9", &pid.to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    /// Barrido por imagen del residente: termina todo proceso cuya imagen sea
    /// `qwen_tts(.exe)` (`RESIDENT_IMAGE_NAME`). Faro de último recurso e
    /// independiente del pidfile: se usa cuando no hay `resident_pid` registrado
    /// (pidfile perdido tras un aborto duro) para no dejar huérfanos. Seguro
    /// sólo porque el residente tiene imagen propia —el daemon comparte imagen
    /// con el CLI y por eso su kill-por-imagen está prohibido—. Best-effort y
    /// sin verificación interna: el llamante verifica la ausencia
    /// (`resident_pid_alive == false` y/o ausencia por imagen).
    pub fn sweep_resident_by_image() -> bool {
        #[cfg(windows)]
        {
            Command::new("taskkill")
                .args(["/F", "/IM", RESIDENT_IMAGE_NAME])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
        #[cfg(unix)]
        {
            Command::new("pkill")
                .args(["-9", "-x", RESIDENT_IMAGE_NAME])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()
                .map(|s| s.success())
                .unwrap_or(false)
        }
    }

    /// Ruta del fichero de log de stderr del motor residente. Crea el directorio
    /// `logs/` bajo `data_dir()` si no existe. El nombre incluye PID y timestamp
    /// para unicidad por sesión (rotación simple: un fichero por spawn).
    pub(crate) fn resident_log_path() -> PathBuf {
        let dir = avi_store::data_dir().join("logs");
        std::fs::create_dir_all(&dir).expect("no se pudo crear el directorio de logs");
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("SystemTime antes del epoch");
        let filename = format!("qwen3-tts_{}_{}.log", std::process::id(), now.as_millis());
        dir.join(filename)
    }

    /// Healthcheck `GET /v1/health` con reintentos. Distingue *crash* del motor
    /// (el `child` terminó inesperadamente) de *hang* (timeout agotado). En caso
    /// de crash incluye el código de salida y la ruta del log de stderr.
    pub(crate) fn wait_health(
        child: &mut Child,
        port: u16,
        retries: usize,
        interval_ms: u64,
        log_path: &Path,
    ) -> Result<()> {
        let url = format!("http://127.0.0.1:{}/v1/health", port);
        for i in 0..retries {
            // Detecta crash inmediato: si el proceso terminó, el motor no va a
            // responder nunca. `try_wait` no bloquea.
            if let Some(status) = child.try_wait()? {
                return Err(anyhow!(
                    "El servidor Qwen3-TTS terminó inesperadamente (exit {}) antes \
                     de que el healthcheck pasara. Log de stderr: {}",
                    status.code().unwrap_or(-1),
                    log_path.display()
                ));
            }
            let ok = http_exchange(&url, "GET", None, Duration::from_millis(interval_ms))
                .map(|(status, _)| (200..300).contains(&status))
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            if i + 1 < retries {
                thread::sleep(Duration::from_millis(interval_ms));
            }
        }
        Err(anyhow!(
            "El servidor Qwen3-TTS no respondió a /v1/health en el puerto {} tras {} \
             intentos. Log de stderr: {}",
            port,
            retries,
            log_path.display()
        ))
    }

    /// Simulador HTTP mínimo para tests: responde `200 OK` a `/v1/health` y
    /// captura el body de un único `POST /v1/tts`. Siempre sano: nunca cuelga,
    /// nunca muere ni daemoniza de verdad, por lo que no reproduce la
    /// terminación real de un árbol de procesos; nunca reproduce `panic!` con
    /// lock retenido ni aborto externo (solo el harness lo cubre) ni crash con
    /// puerto ocupado (solo `run_supervised` lo cubre); no corre bajo la imagen
    /// real `qwen_tts`, así que queda ciego al reclamo por PID/imagen del
    /// residente huérfano. La ausencia real de huérfanos a nivel SO —verificada
    /// por `resident_pid` muerto más ausencia por imagen— solo la comprueba la
    /// serie pesada (`tests/cli_golden.rs`).
    #[cfg(test)]
    pub(crate) fn simulate_server(
        body: std::sync::Arc<Mutex<String>>,
    ) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("debe bindear un puerto libre");
        let port = listener.local_addr().expect("puerto local").port();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let content_length;
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            let text = String::from_utf8_lossy(&buf);
                            if let Some(end) = text.find("\r\n\r\n") {
                                let headers = text[..end].to_string();
                                content_length = headers
                                    .lines()
                                    .find_map(|l| {
                                        l.to_lowercase()
                                            .strip_prefix("content-length:")
                                            .and_then(|v| v.trim().parse().ok())
                                    })
                                    .unwrap_or(0);
                                let header_len = end + 4;
                                while buf.len() < header_len + content_length {
                                    match stream.read(&mut tmp) {
                                        Ok(0) => break,
                                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                                        Err(_) => break,
                                    }
                                }
                                if buf.len() >= header_len + content_length {
                                    *body.lock().unwrap() = String::from_utf8_lossy(
                                        &buf[header_len..header_len + content_length],
                                    )
                                    .to_string();
                                }
                                break;
                            }
                        }
                        Err(_) => break,
                    }
                }
                let _ = stream.write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: audio/wav\r\nContent-Length: 46\r\n\r\n",
                );
                let _ = stream.write_all(&min_wav());
                let _ = stream.flush();
            }
        });
        (port, handle)
    }

    /// WAV mínimo válido (24 kHz, 1 muestra silenciosa) para respuestas simuladas.
    #[cfg(test)]
    pub(crate) fn min_wav() -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 24_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            writer.write_sample(0i16).unwrap();
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::io::Write;
    use std::net::TcpListener;
    use std::process::Stdio;
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Los defaults del host deben coincidir con los defaults del motor
    /// (`docs/server.md:140-141`). Afirma los defaults del `struct`/motor sin
    /// cambios, no los valores de producción de `GenerationOptions::production()`
    /// (config validada por oído) — este test queda intacto a propósito.
    #[test]
    fn default_generation_options_matches_engine() {
        let d = GenerationOptions::default();
        assert_eq!(d.temperature, 0.5);
        assert_eq!(d.top_k, 50);
        assert_eq!(d.top_p, 1.0);
        assert_eq!(d.rep_penalty, 1.05);
        assert_eq!(d.language, "es");
        assert_eq!(d.seed, None);
    }

    /// `production()` fija temperatura y seed a la config validada por oído,
    /// sin alterar el resto de campos respecto a `Default`.
    #[test]
    fn generation_options_production_sets_temperature_and_seed() {
        let p = GenerationOptions::production();
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        assert_eq!(p.top_k, DEFAULT_TOP_K);
        assert_eq!(p.top_p, DEFAULT_TOP_P);
        assert_eq!(p.rep_penalty, DEFAULT_REP_PENALTY);
        assert_eq!(p.language, "es");
    }

    /// `with_temperature(None)` equivale a `production()`; con `Some` solo
    /// cambia la temperatura (bordes del rango válido incluidos).
    #[test]
    fn generation_options_with_temperature_resolves_override() {
        let p = GenerationOptions::with_temperature(None);
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        let o = GenerationOptions::with_temperature(Some(0.9));
        assert_eq!(o.temperature, 0.9);
        assert_eq!(o.seed, Some(4));
        assert_eq!(o.top_k, DEFAULT_TOP_K);
        let min = GenerationOptions::with_temperature(Some(f32::MIN_POSITIVE));
        assert!(min.temperature > 0.0);
        let max = GenerationOptions::with_temperature(Some(2.0));
        assert_eq!(max.temperature, 2.0);
    }

    /// Argv exacto de arranque del residente (preset y voz clonada), sin
    /// I/O real de proceso — cierra un hueco de cobertura que ningún test de
    /// integración ejercitaba.
    #[test]
    fn build_resident_command_includes_int4_threads_stream() {
        let cmd = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("vendor/qwen3-tts/qwen3-tts-0.6b"),
            8766,
            None,
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-d",
                "vendor/qwen3-tts/qwen3-tts-0.6b",
                "--serve",
                "8766",
                "--int4",
                "-j",
                "4",
                "--stream",
            ]
        );

        let cmd = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            8766,
            Some(Path::new("voz.qvoice")),
        );
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            vec![
                "-d",
                "md",
                "--serve",
                "8766",
                "--int4",
                "-j",
                "4",
                "--stream",
                "--load-voice",
                "voz.qvoice",
                "--icl-only",
            ]
        );
    }

    /// Body HTTP con defaults → claves exactas; voz clonada → sin speaker/language.
    /// Afirma los defaults del `struct`/motor sin cambios, no los valores de
    /// producción de `GenerationOptions::production()` (config validada por oído) — el body HTTP
    /// no transporta `int4`/`-j`/`--stream` (son flags de arranque de proceso).
    #[test]
    fn build_tts_body_defaults_and_cloned_voice() {
        let voice = VoiceEngine::Preset("ryan".to_string());
        let body = build_tts_body("Hola", &voice, &GenerationOptions::default(), None, None);
        let obj = body.as_object().expect("body debe ser objeto");
        assert_eq!(obj.get("text").and_then(|v| v.as_str()), Some("Hola"));
        assert_eq!(obj.get("speaker").and_then(|v| v.as_str()), Some("ryan"));
        assert_eq!(obj.get("language").and_then(|v| v.as_str()), Some("es"));
        assert_eq!(obj.get("temperature").and_then(|v| v.as_f64()), Some(0.5));
        assert_eq!(obj.get("top_k").and_then(|v| v.as_u64()), Some(50));
        assert_eq!(obj.get("top_p").and_then(|v| v.as_f64()), Some(1.0));
        // f32 1.05 → f64 1.0499999523162842: comparación con tolerancia.
        assert!((obj.get("rep_penalty").and_then(|v| v.as_f64()).unwrap() - 1.05).abs() < 1e-6);
        assert!(!obj.contains_key("seed"), "seed None no debe emitirse");
        assert!(!obj.contains_key("format"), "format no debe emitirse");
        assert!(!obj.contains_key("volume"));
        assert!(!obj.contains_key("rate"));
        assert!(!obj.contains_key("emotion"));

        let cloned = VoiceEngine::Cloned(PathBuf::from("voz.qvoice"));
        let body = build_tts_body("Hola", &cloned, &GenerationOptions::default(), None, None);
        let obj = body.as_object().expect("body debe ser objeto");
        assert!(!obj.contains_key("speaker"), "voz clonada omite speaker");
        assert!(!obj.contains_key("language"), "voz clonada omite language");

        let prosody = ProsodyOptions {
            volume: Some(1.1),
            rate: Some(0.9),
        };
        let emotion = EmotionOptions {
            emotion: Some("joy".to_string()),
        };
        let body = build_tts_body(
            "Hola",
            &voice,
            &GenerationOptions::default(),
            Some(&prosody),
            Some(&emotion),
        );
        let obj = body.as_object().expect("body debe ser objeto");
        assert!((obj.get("volume").and_then(|v| v.as_f64()).unwrap() - 1.1).abs() < 1e-6);
        assert!((obj.get("rate").and_then(|v| v.as_f64()).unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(obj.get("emotion").and_then(|v| v.as_str()), Some("joy"));
    }

    /// Tabla de resolución voz → motor (default resuelve como Preset(default)).
    #[test]
    fn resolve_voice_engine_table() {
        // default sin referencia resuelve como Preset("default"); con qvoice resuelve como Clonada.
        assert_eq!(
            resolve_voice_engine("default", None),
            VoiceEngine::Preset("default".to_string())
        );
        let q = std::env::temp_dir().join("avi_tts_test_referencia.qvoice");
        std::fs::write(&q, b"QVCE").unwrap();
        assert_eq!(
            resolve_voice_engine("mi_voz", Some(&q)),
            VoiceEngine::Cloned(q.clone())
        );
        // Sin referencia → preset con el nombre dado.
        assert_eq!(
            resolve_voice_engine("vivian", None),
            VoiceEngine::Preset("vivian".to_string())
        );
        std::fs::remove_file(&q).ok();
    }

    /// El healthcheck responde cuando el listener simula `/v1/health`, y
    /// el `Drop` del gestor termina al hijo.
    #[test]
    fn resident_healthcheck_ok_and_drop_kills_child() {
        let (port, handle) = resident::simulate_server(Arc::new(Mutex::new(String::new())));
        let child = sleeping_process();
        let pid = child.id();
        let resident = resident::Qwen3TtsResident::spawn_with_child(
            child,
            port,
            resident::resident_log_path(),
            10,
            50,
        )
        .expect("el healthcheck debe pasar contra el listener simulado");
        drop(resident);
        // El servidor simulado no termina nunca: se desacopla el hilo.
        drop(handle);
        thread::sleep(Duration::from_millis(800));
        assert!(
            !process_alive(pid),
            "el Drop del gestor debe terminar al hijo"
        );
    }

    /// El healthcheck reintenta hasta que el servidor responde. El simulador
    /// cierra las dos primeras conexiones sin responder (fallo inmediato) y solo
    /// responde 200 a partir de la tercera (determinista, sin temporización).
    #[test]
    fn resident_healthcheck_retries_until_responding() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let attempts = Arc::new(Mutex::new(0usize));
        let att = attempts.clone();
        let handle = thread::spawn(move || {
            // El servidor simulado lee la petición (evita RST por datos sin
            // leer), cierra sin responder las dos primeras conexiones (fallo
            // inmediato) y responde 200 a la tercera; luego termina.
            for stream in listener.incoming().take(4) {
                let Ok(mut stream) = stream else { continue };
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                *att.lock().unwrap() += 1;
                if *att.lock().unwrap() >= 3 {
                    let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
                    let _ = stream.flush();
                }
            }
        });
        let child = sleeping_process();
        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            port,
            resident::resident_log_path(),
            5,
            50,
        );
        // El servidor simulado termina solo tras su N-ésima conexión: se
        // desacopla el hilo y se da margen para que drene las conexiones en vuelo.
        drop(handle);
        thread::sleep(Duration::from_millis(200));
        let n = *attempts.lock().unwrap();
        if let Err(e) = result {
            panic!(
                "el healthcheck debe triunfar tras los reintentos ({} conexiones recibidas): {}",
                n, e
            );
        }
        if n < 3 {
            panic!(
                "debe haberse reintentado al menos 3 veces (se recibieron {})",
                n
            );
        }
    }

    /// El healthcheck falla si el servidor nunca responde.
    #[test]
    fn resident_healthcheck_fails_without_server() {
        let child = sleeping_process();
        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            1,
            resident::resident_log_path(),
            3,
            30,
        );
        assert!(result.is_err(), "sin servidor el healthcheck debe fallar");
    }

    /// Un sumidero TCP que acepta la conexión y nunca responde
    /// (a diferencia del crash de `wait_health_distinguishes_crash_from_hang`, aquí
    /// el proceso hijo sigue vivo) debe hacer que `wait_health(1, 2000)`
    /// devuelva `Err` en `≲` 3 s, sin colgarse — reproduce el cuelgue del
    /// motor C que motivó la salud observada por petición.
    #[test]
    fn wait_health_detects_tcp_sink_without_hanging() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            // Acepta y retiene la conexión sin leer ni escribir: sumidero puro.
            if let Ok((stream, _)) = listener.accept() {
                thread::sleep(Duration::from_secs(3));
                drop(stream);
            }
        });
        let mut child = sleeping_process();
        let log_path = resident::resident_log_path();
        let start = std::time::Instant::now();
        let result = resident::wait_health(&mut child, port, 1, 2000, log_path.as_path());
        let elapsed = start.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        drop(handle);
        assert!(
            result.is_err(),
            "el sumidero TCP no debe pasar el healthcheck"
        );
        assert!(
            elapsed < Duration::from_secs(3),
            "wait_health no debe colgarse ante un sumidero TCP: tardó {:?}",
            elapsed
        );
    }

    /// `resident_log_path()` crea el directorio `logs/` bajo `data_dir()` y
    /// devuelve un filename con el patrón `qwen3-tts_<pid>_<ms>.log`.
    #[test]
    fn log_path_creates_directory_and_filename() {
        let path = resident::resident_log_path();
        let parent = path.parent().expect("el log debe tener directorio padre");
        assert!(
            parent.is_dir(),
            "el directorio de logs/ debe crearse: {}",
            parent.display()
        );
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .expect("el filename debe ser UTF-8 válido");
        assert!(
            name.starts_with("qwen3-tts_") && name.ends_with(".log"),
            "el filename debe seguir qwen3-tts_<pid>_<ms>.log: {}",
            name
        );
    }

    /// `wait_health` distingue *crash* (el child muere inesperadamente) de
    /// *hang* (timeout). Un proceso que sale inmediatamente produce un error que
    /// menciona "terminó inesperadamente" + código de salida + ruta del log.
    #[test]
    fn wait_health_distinguishes_crash_from_hang() {
        // Proceso que muere inmediatamente (exit 1) en lugar de servir.
        let mut child = if cfg!(windows) {
            Command::new("cmd")
                .args(["/C", "exit 1"])
                .spawn()
                .expect("cmd debe existir en Windows")
        } else {
            Command::new("sh")
                .arg("-c")
                .arg("exit 1")
                .spawn()
                .expect("sh debe existir en Unix")
        };
        let log_path = resident::resident_log_path();
        let err = resident::wait_health(&mut child, 1, 3, 50, log_path.as_path())
            .expect_err("el healthcheck debe fallar si el proceso muere");
        let msg = err.to_string();
        assert!(
            msg.contains("terminó inesperadamente") || msg.contains("crash"),
            "el error debe indicar crash/hang, no timeout: {}",
            msg
        );
        assert!(
            msg.contains(log_path.to_str().unwrap()),
            "el error debe incluir la ruta del log: {}",
            msg
        );
    }

    /// El body del POST contra un servidor simulado transporta los defaults
    /// del motor (e9) y sus overrides. Afirma los defaults del `struct`/motor sin
    /// cambios, no los valores de producción de `GenerationOptions::production()`
    /// (config validada por oído) — este test invoca `synthesize_with_options` directamente con
    /// `GenerationOptions::default()`, no `Qwen3TtsEngine::synthesize`.
    #[test]
    fn synthesize_http_sends_engine_defaults() {
        let body = Arc::new(Mutex::new(String::new()));
        let (port, handle) = resident::simulate_server(body.clone());
        let engine = Qwen3TtsEngine::new(Some(format!("http://127.0.0.1:{}", port)));
        let profile = VoiceProfile {
            name: "default".to_string(),
            qvoice_path: None,
        };
        let out = std::env::temp_dir().join("avi_tts_test_out.wav");
        let res = engine
            .synthesize_with_options("Hola", &profile, &GenerationOptions::default(), Some(&out))
            .expect("la síntesis HTTP simulada debe completarse");
        drop(handle);
        assert!(res.is_file());
        std::fs::remove_file(&out).ok();
        let parsed: serde_json::Value = serde_json::from_str(&body.lock().unwrap()).unwrap();
        assert_eq!(parsed["temperature"], 0.5);
        assert_eq!(parsed["top_p"], 1.0);
        assert_eq!(parsed["top_k"], 50);
        assert!((parsed["rep_penalty"].as_f64().unwrap() - 1.05).abs() < 1e-6);
        assert_eq!(parsed["language"], "es");
        assert_eq!(parsed["speaker"], "default");
        assert!(parsed.get("seed").is_none());

        let opts = GenerationOptions {
            temperature: 0.9,
            seed: Some(7),
            ..Default::default()
        };
        let body = Arc::new(Mutex::new(String::new()));
        let (port, handle) = resident::simulate_server(body.clone());
        let engine = Qwen3TtsEngine::new(Some(format!("http://127.0.0.1:{}", port)));
        let _ = engine.synthesize_with_options("Hola", &profile, &opts, Some(&out));
        drop(handle);
        let parsed: serde_json::Value = serde_json::from_str(&body.lock().unwrap()).unwrap();
        assert!((parsed["temperature"].as_f64().unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(parsed["seed"], 7);
    }

    /// Proceso que duerme para simular el hijo del residente en tests. Hijo
    /// directo bien portado (recolectable vía `Child`): no reproduce el
    /// desacoplo del `qwen_tts` real ni la daemonización, así que queda ciego
    /// a ese escenario; nunca reproduce `panic!` con lock retenido ni aborto
    /// externo (solo el harness lo cubre) ni la ventana spawn→write ni señales
    /// (solo harness/producto lo cubren); no corre bajo la imagen real
    /// `qwen_tts`, así que queda ciego al barrido por imagen del residente;
    /// el cierre preciso por árbol se cubre en
    /// `resident_kill_tree_by_pid_terminates_child`.
    fn sleeping_process() -> std::process::Child {
        if cfg!(windows) {
            Command::new("powershell")
                .args(["-NoProfile", "-Command", "Start-Sleep -Seconds 30"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("powershell debe existir en Windows")
        } else {
            Command::new("sleep")
                .arg("30")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("sleep debe existir en Unix")
        }
    }

    /// ¿Sigue vivo el proceso con `pid`? Doble de test: delega en
    /// `resident::resident_pid_alive`, la misma primitiva que el producto usa
    /// para verificar el cierre por árbol, sin reproducir daemonización real.
    fn process_alive(pid: u32) -> bool {
        resident::resident_pid_alive(pid)
    }

    /// La terminación por PID mata un hijo real a nivel SO, sin
    /// daemonización (hijo directo, no el `qwen_tts` desacoplado). Cubre
    /// `kill_tree_resident_by_pid` + `resident_pid_alive` con recolección
    /// determinista del estado (como el `Drop`). Verificación SIN sondeo sobre
    /// el singleton de PID del SO: un SIGKILL/`taskkill /F` es imparable, así
    /// que `wait()` bloquea hasta la muerte real y no puede colgar por un hijo
    /// que sobreviva (a diferencia de un bucle `kill -0`, cuya señal de grupo
    /// mal dirigida colgaba solo en Linux/Docker).
    #[test]
    fn resident_kill_tree_by_pid_terminates_child() {
        let mut child = sleeping_process();
        let pid = child.id();
        assert!(
            resident::resident_pid_alive(pid),
            "el hijo debe estar vivo tras el spawn (pid {})",
            pid
        );
        assert!(
            resident::kill_tree_resident_by_pid(pid),
            "la terminación por PID debe reportar éxito sobre un hijo vivo (pid {})",
            pid
        );
        // Verificación determinista sin sondeo: `wait` bloquea hasta la muerte
        // real y recolecta el estado (como el `Drop` del residente).
        let status = child.wait().expect("wait sobre el hijo debe tener éxito");
        assert!(
            !status.success(),
            "el hijo terminado por señal no debe reportar éxito (pid {}, status {:?})",
            pid,
            status
        );
    }

    /// `resolve_binary` halla el binario junto al `current_exe` aunque `cwd` no tenga vendor.
    #[test]
    fn resolve_binary_finds_exe_dir_vendor() {
        // Guardar env para no contaminar otros tests (serializados por --test-threads=1 en CI)
        let orig = std::env::var_os("QWEN3_TTS_BIN");
        std::env::remove_var("QWEN3_TTS_BIN");
        let exe_dir = std::env::current_exe()
            .expect("current_exe")
            .parent()
            .expect("parent")
            .to_path_buf();
        let cand = exe_dir.join(if cfg!(windows) {
            "vendor/qwen3-tts/qwen_tts.exe"
        } else {
            "vendor/qwen3-tts/qwen_tts"
        });
        let created_dir = cand.parent().unwrap().to_path_buf();
        let existed = cand.is_file();
        if !existed {
            std::fs::create_dir_all(&created_dir).unwrap();
            std::fs::write(&cand, b"fake").unwrap();
        }
        let found = resolve_binary();
        // Limpiar
        if !existed {
            let _ = std::fs::remove_file(&cand);
            // No borrar created_dir si quedó con otros ficheros
            let _ = std::fs::remove_dir(&created_dir);
            let _ = std::fs::remove_dir(exe_dir.join("vendor/qwen3-tts"));
            let _ = std::fs::remove_dir(exe_dir.join("vendor"));
        }
        if let Some(o) = orig {
            std::env::set_var("QWEN3_TTS_BIN", o);
        }
        assert!(
            found.is_some(),
            "resolve_binary debe hallar el binario en <exe_dir>/vendor"
        );
        assert_eq!(found.unwrap(), cand);
    }
}
