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
    /// Penalización de repetición del motor (añadida en Fase 5; el motor la
    /// usa por defecto a 1.05).
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
    /// T0.35, WSL seed42 como oráculo, 3 oyentes → seed4 4/10 vs wsl 6/10 (C3 verde),
    /// C1 WER max 0.000 y C2 SIM min 0.822 PASS (target/seed-sweep/wer.csv,
    /// speaker_sim.csv). Seed 42 previo sigue verde pero seed4 iguala prosodia
    /// nativa Windows sin WSL (docs/reviews/2026-08-14-tts-calidad-fase5.md §Cierre).
    pub fn produccion() -> Self {
        Self {
            temperature: 0.35,
            seed: Some(4),
            ..Self::default()
        }
    }

    /// Resuelve la temperatura opcional del CLI a opciones efectivas: sin flag
    /// se usa la config de producción; con flag se sobrescribe la temperatura.
    pub fn con_temperatura(temperature: Option<f32>) -> Self {
        let mut opts = Self::produccion();
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
    pub reference_audio: Option<PathBuf>,
    pub qvoice_path: Option<PathBuf>,
}

/// Trait público del motor de síntesis TTS
pub trait TtsEngine: Send + Sync {
    fn synthesize(&self, text: &str, voice: &str, output_path: Option<&PathBuf>)
        -> Result<PathBuf>;

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
pub enum VozMotor {
    Preset(String),
    Clonada(PathBuf),
}

/// Resolución voz → motor: una voz con `reference.qvoice` es clonada;
/// cualquier otra es preset. `default` con `reference.qvoice` graft resuelve
/// como `Clonada`; sin referencia resuelve como `Preset("default")` y el
/// motor cae a `ryan` por `spk_table` sólo si el binario lo exige.
pub fn resolve_voice_motor(voice: &str, qvoice: Option<&Path>) -> VozMotor {
    if let Some(q) = qvoice {
        if q.is_file() {
            return VozMotor::Clonada(q.to_path_buf());
        }
    }
    VozMotor::Preset(voice.to_string())
}

/// Resolución del binario del motor por capas (decisión e1):
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
    let name = if cfg!(windows) {
        "qwen_tts.exe"
    } else {
        "qwen_tts"
    };
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

/// Resolución del directorio de pesos por capas (decisión e1):
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
            let hermano = parent.join("qwen3-tts-0.6b");
            if hermano.is_dir() {
                return Some(hermano);
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
            let hermano = parent.join("qwen3-tts-0.6b-base");
            if hermano.is_dir() {
                return Some(hermano);
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
    // Capa HF: snapshot cacheado por setup --with-base
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

/// Ventana de la salud observada por petición (H-05): `retries=1`,
/// `interval_ms=2000` acotan el healthcheck a ~2 s por petición. Generoso
/// para un `GET /v1/health` sano en loopback (responde en ms) y suficiente
/// para fallar rápido ante un sumidero TCP que acepta y no responde, muy por
/// debajo del deadline de handler de 8 s (`avi-daemon`).
const HEALTH_OBS_RETRIES: usize = 1;
const HEALTH_OBS_INTERVAL_MS: u64 = 2000;

/// Estado del servidor residente: se indexa por voz (decisión e3) — al cambiar
/// de voz se termina el residente anterior y se arranca otro con `--load-voice`.
struct ResidentState {
    resident: resident::Qwen3TtsResident,
    voz_key: String,
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
    /// PID (señal que no requiere el lock, con verificación inmediata sin espera)
    /// y solo como último recurso documentado —si el PID sigue vivo o no hay
    /// PID— se mata por imagen; la recolección del estado se hace best-effort
    /// con `try_lock` (si el warmup lo tiene, no esperamos: el proceso ya está
    /// muerto y su `Drop` recolectará el estado al liberarse). La verificación
    /// con deadline la hace el llamante (graceful del daemon / parada del CLI).
    pub fn shutdown(&self) {
        let pid = self.resident_pid.load(Ordering::Relaxed);
        if pid != 0 {
            crate::resident::matar_arbol_residente_por_pid(pid);
            // Verificación inmediata sin espera (no bloquea al runtime): si el
            // árbol preciso no lo terminó, último recurso por imagen.
            if crate::resident::pid_vivo_residente(pid) {
                crate::resident::kill_resident_process();
            }
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
        let options = GenerationOptions::con_temperatura(temperature);
        let qvoice_path = avi_store::VoiceStore::new().find_reference(voice);
        let profile = VoiceProfile {
            name: voice.to_string(),
            reference_audio: None,
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
        voz: &VozMotor,
        options: &GenerationOptions,
        prosody: Option<&ProsodyOptions>,
        emotion: Option<&EmotionOptions>,
        out_path: &Path,
    ) -> Result<()> {
        let body = construir_body_tts(text, voz, options, prosody, emotion).to_string();
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

    /// Arranca un residente fresco para `voz` (Tarea 3): construye
    /// `load_voice`, hace `spawn` (que ya trae su propio `wait_health` de
    /// arranque, por lo que nace sano o falla con diagnóstico), actualiza
    /// `resident_pid` y ensambla el `ResidentState`. Compartido por el camino
    /// de cambio de voz y por el rearranque ante degradación (H-05).
    fn arrancar_residente(
        &self,
        model_dir: &Path,
        voz: &VozMotor,
        voz_key: String,
    ) -> Result<ResidentState> {
        let load_voice = match voz {
            VozMotor::Clonada(p) => Some(p.as_path()),
            VozMotor::Preset(_) => None,
        };
        let port = default_port();
        let spawned = resident::Qwen3TtsResident::spawn(model_dir, port, load_voice)?;
        self.resident_pid.store(spawned.pid(), Ordering::Relaxed);
        Ok(ResidentState {
            resident: spawned,
            voz_key,
        })
    }

    /// Síntesis vía servidor residente: arranca (o reutiliza) el residente de
    /// la voz solicitada y hace `POST /v1/tts`.
    ///
    /// H-05: la reutilización por `voz_key` no bastaba — un residente colgado
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
    fn synthesize_via_residente(
        &self,
        text: &str,
        voz: &VozMotor,
        options: &GenerationOptions,
        out_path: &Path,
    ) -> Result<()> {
        let model_dir = self
            .model_dir
            .as_ref()
            .ok_or_else(|| anyhow!("El modelo de síntesis Qwen3-TTS no está provisionado."))?;
        let voz_key = match voz {
            VozMotor::Preset(n) => format!("preset:{}", n),
            VozMotor::Clonada(p) => format!("clone:{}", p.display()),
        };
        let url = {
            let mut guard = self.resident.lock().unwrap();
            if guard.as_ref().map(|s| s.voz_key.as_str()) != Some(voz_key.as_str()) {
                *guard = Some(self.arrancar_residente(model_dir, voz, voz_key)?);
            } else if let Err(_e) = guard
                .as_mut()
                .expect("reutilización verificada arriba")
                .resident
                .health_check(HEALTH_OBS_RETRIES, HEALTH_OBS_INTERVAL_MS)
            {
                // Residente degradado (crash o hang): rearranque determinista.
                let pid = guard.as_ref().expect("residente reutilizado").resident.pid();
                resident::matar_arbol_residente_por_pid(pid);
                *guard = None;
                *guard = Some(self.arrancar_residente(model_dir, voz, voz_key)?);
            }
            let state = guard.as_ref().expect("residente arrancado o reutilizado sano");
            format!("http://127.0.0.1:{}", state.resident.port)
        };
        self.synthesize_via_http(&url, text, voz, options, None, None, out_path)
    }
}

impl TtsEngine for Qwen3TtsEngine {
    fn synthesize(
        &self,
        text: &str,
        voice: &str,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        let default_options = GenerationOptions::produccion();
        let qvoice_path = avi_store::VoiceStore::new().find_reference(voice);
        let profile = VoiceProfile {
            name: voice.to_string(),
            reference_audio: None,
            qvoice_path,
        };
        self.synthesize_with_options(text, &profile, &default_options, output_path)
    }

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

        // Conversión perezosa del `reference.wav` legado → `.qvoice` (decisión e3):
        // si la voz solo tiene un WAV de referencia, se clona una vez y se cachea
        // junto a él como `reference.qvoice`.
        let qvoice = match (
            profile.qvoice_path.as_deref(),
            profile.reference_audio.as_deref(),
        ) {
            (Some(q), _) if q.is_file() => Some(q.to_path_buf()),
            (None, Some(r)) if r.is_file() => {
                let dest = r.with_file_name("reference.qvoice");
                if dest.is_file() {
                    Some(dest)
                } else {
                    let model_dir = self.base_model_dir.as_ref().ok_or_else(|| {
                        anyhow!("El modelo Base de clonado Qwen3-TTS no está provisionado.")
                    })?;
                    clone_voice(model_dir, r, &dest, &profile.name, &options.language)?;
                    Some(dest)
                }
            }
            _ => None,
        };
        let voz = resolve_voice_motor(&profile.name, qvoice.as_deref());

        // 1. HTTP manual configurado (solo presets; la voz clonada exige un
        //    servidor arrancado con su `--load-voice`, que solo gestiona el residente).
        if let Some(url) = &self.server_url {
            if matches!(voz, VozMotor::Preset(_))
                && self
                    .synthesize_via_http(url, text, &voz, options, None, None, &path)
                    .is_ok()
            {
                return Ok(path);
            }
        }

        // 2. Servidor residente gestionado por el host (decisión F0): único
        //    camino restante, con healthcheck (30 s) y POST (30 s) acotados.
        //    El texto viaja por body HTTP JSON, ruta segura para UTF-8 acentuado
        //    (a diferencia del argv de un subprocess en Windows).
        self.synthesize_via_residente(text, &voz, options, &path)?;
        Ok(path)
    }
}

/// Construye el body HTTP de `POST /v1/tts` (Tarea 3): sin `format` (el
/// servidor lo ignora), claves solo-si-`Some`, y `speaker`/`language` omitidos
/// cuando la voz es clonada (el servidor conserva la voz y el idioma del
/// arranque, `docs/server.md:28-34`).
pub(crate) fn construir_body_tts(
    text: &str,
    voz: &VozMotor,
    options: &GenerationOptions,
    prosody: Option<&ProsodyOptions>,
    emotion: Option<&EmotionOptions>,
) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    obj.insert(
        "text".to_string(),
        serde_json::Value::String(text.to_string()),
    );
    match voz {
        VozMotor::Preset(speaker) => {
            obj.insert(
                "speaker".to_string(),
                serde_json::Value::String(speaker.clone()),
            );
            obj.insert(
                "language".to_string(),
                serde_json::Value::String(options.language.clone()),
            );
        }
        VozMotor::Clonada(_) => {}
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
fn referencia_24k_mono(ref_audio: &Path) -> Result<PathBuf> {
    let pcm = avi_audio::load_wav_24k_mono_pcm(ref_audio)?;
    let unico = format!(
        "avi_tts_ref24k_{}_{}.wav",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    let out_path = std::env::temp_dir().join(unico);
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
    let ref_wav = referencia_24k_mono(ref_audio)?;
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

/// Servidor residente del motor Qwen3-TTS (decisión e2): spawn perezoso con
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

    /// Construye el `Command` de arranque del residente (Tareas 2 y 3), sin
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
            // motor C (cuyos mensajes se perdían a null, cegando H-02/H-05/H-01).
            let log_path = resident_log_path();
            let log_file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .map_err(|e| anyhow!(
                    "No se pudo abrir el log de stderr del motor ({}): {}",
                    log_path.display(),
                    e
                ))?;
            use std::process::Stdio;
            // Windows: `qwen_tts.exe` NO debe heredar handles ni abrir terminal del
            // padre. `DETACHED_PROCESS (0x8)` evita la ventana de consola independiente.
            // La herencia del pipe (write-end) del proceso abuelo (test CLI) se corta
            // en la raíz: el daemon que spawnea este motor ya desheredó sus STD vía
            // `SetHandleInformation` (`main::desheredar_handles_estandar`); no existe
            // una creation flag que desactive la herencia. `Stdio::null` en stdin/stdout
            // cierra la herencia de stdin/tty; stderr va al log (Stdio::from marca el
            // handle no-heredable, H-04).
            // Árbol matable (H-01): a propósito SIN `CREATE_NEW_PROCESS_GROUP` ni
            // breakaway, para que el residente permanezca en el grupo/Job del daemon
            // y `taskkill /F /T /PID <daemon>` (o el Job con cierre) lo alcance.
            // En Unix tampoco se hace `setsid` aquí: hereda el grupo del daemon.
            // D-01: tras la muerte del líder el residente reparentado se verifica
            // por PID y 8766 desde el CLI (`reclamar_residual_degradado`); los
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
            Self::spawn_con_hijo(child, port, log_path, 60, 500)
        }

        /// Arranca el healthcheck sobre un hijo ya lanzado (retries/intervalo
        /// configurables para los tests de reintentos). `log_path` se guarda en el
        /// struct para incluirse en errores de `wait_health`.
        pub(crate) fn spawn_con_hijo(
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

        /// Salud observada por petición (H-05): reutiliza `wait_health` (combina
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
                // o vivo en un drop normal). D-05: insuficiente ante caída a medio
                // `spawn` (sin `Child` que recolectar): por eso `run_supervised`
                // reclama además el árbol propio previo con deadline y verificación
                // antes del siguiente `bind`.
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Viveza real de un PID a nivel de sistema (sin Mutex ni HTTP).
    pub(crate) fn pid_vivo_residente(pid: u32) -> bool {
        if pid == 0 {
            return false;
        }
        #[cfg(windows)]
        {
            let salida = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();
            match salida {
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

    /// Mata el árbol preciso del residente por PID (sin Mutex): Windows
    /// `taskkill /F /T /PID`; Unix `kill -9` al grupo y al PID. No verifica:
    /// el llamante combina con `pid_vivo_residente`.
    pub(crate) fn matar_arbol_residente_por_pid(pid: u32) -> bool {
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
            let _ = Command::new("kill")
                .args(["-9", &format!("-{}", pid)])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
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

    /// Mata el proceso residente del motor POR NOMBRE DE IMAGEN (`qwen_tts`).
    ///
    /// ÚLTIMO RECURSO DOCUMENTADO (H-01 + D-04): solo se llama cuando el kill preciso
    /// del árbol por PID falló o no hay PID (el `qwen_tts` vendido desacopla su
    /// proceso servidor real del `Child` que Rust captura, de modo que el kill
    /// por PID puede dejar vivo al servidor; D-04 lo reutiliza además ante
    /// `Parado` con residente vivo sin PID del daemon). Mata por nombre de imagen
    /// del residente y alcanza al servidor real (nunca imagen del daemon, que
    /// comparte imagen con el CLI). Con el residente como único camino de síntesis (sin
    /// fallback), el kill no puede desencadenar re-lanzamientos: la síntesis en
    /// curso simplemente falla. Sin tomar ningún `Mutex`.
    pub fn kill_resident_process() {
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // `DETACHED_PROCESS (0x8)`: sin ventana de consola. `Stdio::null` en los
            // tres STD evita heredar/exponer handles del padre (p. ej. pipes del test).
            let _ = Command::new("cmd")
                .args(["/C", "taskkill /F /T /IM qwen_tts.exe"])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .creation_flags(0x00000008)
                .status();
        }
        #[cfg(unix)]
        {
            let _ = Command::new("sh")
                .args(["-c", "pkill -9 -f 'qwen_tts.*--serve' || true"])
                .status();
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
    /// captura el body de un único `POST /v1/tts`. Siempre sano: nunca cuelga
    /// ni muere ni daemoniza (ceguera H-01, T7); nunca reproduce `panic!` con
    /// lock retenido ni aborto externo (D-03, solo-harness) ni crash con puerto
    /// ocupado (D-05, solo `run_supervised`); nunca usa imagen `qwen_tts` ni
    /// deja resto sin puerto (D-04). La ausencia real
    /// de huérfanos a nivel SO solo la verifica la serie pesada
    /// (`tests/cli_golden.rs`).
    #[cfg(test)]
    pub(crate) fn simular_servidor(
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
                let _ = stream.write_all(&wav_minimo());
                let _ = stream.flush();
            }
        });
        (port, handle)
    }

    /// WAV mínimo válido (24 kHz, 1 muestra silenciosa) para respuestas simuladas.
    #[cfg(test)]
    pub(crate) fn wav_minimo() -> Vec<u8> {
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

    /// T1: los defaults del host deben coincidir con los defaults del motor
    /// (`docs/server.md:140-141`). Afirma los defaults del `struct`/motor sin
    /// cambios, no los valores de producción de `GenerationOptions::produccion()`
    /// (Tarea 1) — este test queda intacto a propósito.
    #[test]
    fn default_generation_options_coinciden_con_motor() {
        let d = GenerationOptions::default();
        assert_eq!(d.temperature, 0.5);
        assert_eq!(d.top_k, 50);
        assert_eq!(d.top_p, 1.0);
        assert_eq!(d.rep_penalty, 1.05);
        assert_eq!(d.language, "es");
        assert_eq!(d.seed, None);
    }

    /// T1: `produccion()` fija temperatura y seed a la config validada por oído,
    /// sin alterar el resto de campos respecto a `Default`.
    #[test]
    fn generation_options_produccion_fija_temperatura_y_seed() {
        let p = GenerationOptions::produccion();
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        assert_eq!(p.top_k, DEFAULT_TOP_K);
        assert_eq!(p.top_p, DEFAULT_TOP_P);
        assert_eq!(p.rep_penalty, DEFAULT_REP_PENALTY);
        assert_eq!(p.language, "es");
    }

    /// `con_temperatura(None)` equivale a `produccion()`; con `Some` solo
    /// cambia la temperatura (bordes del rango válido incluidos).
    #[test]
    fn generation_options_con_temperatura_resuelve_override() {
        let p = GenerationOptions::con_temperatura(None);
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        let o = GenerationOptions::con_temperatura(Some(0.9));
        assert_eq!(o.temperature, 0.9);
        assert_eq!(o.seed, Some(4));
        assert_eq!(o.top_k, DEFAULT_TOP_K);
        let min = GenerationOptions::con_temperatura(Some(f32::MIN_POSITIVE));
        assert!(min.temperature > 0.0);
        let max = GenerationOptions::con_temperatura(Some(2.0));
        assert_eq!(max.temperature, 2.0);
    }

    /// T6: argv exacto de arranque del residente (preset y voz clonada), sin
    /// I/O real de proceso — cierra el hueco de cobertura señalado por F1.
    #[test]
    fn build_resident_command_incluye_int4_hilos_stream() {
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

    /// T3: body HTTP con defaults → claves exactas; voz clonada → sin speaker/language.
    /// Afirma los defaults del `struct`/motor sin cambios, no los valores de
    /// producción de `GenerationOptions::produccion()` (Tarea 1) — el body HTTP
    /// no transporta `int4`/`-j`/`--stream` (son flags de arranque de proceso).
    #[test]
    fn construir_body_tts_defaults_y_voz_clonada() {
        let voz = VozMotor::Preset("ryan".to_string());
        let body = construir_body_tts("Hola", &voz, &GenerationOptions::default(), None, None);
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

        let clonada = VozMotor::Clonada(PathBuf::from("voz.qvoice"));
        let body = construir_body_tts("Hola", &clonada, &GenerationOptions::default(), None, None);
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
        let body = construir_body_tts(
            "Hola",
            &voz,
            &GenerationOptions::default(),
            Some(&prosody),
            Some(&emotion),
        );
        let obj = body.as_object().expect("body debe ser objeto");
        assert!((obj.get("volume").and_then(|v| v.as_f64()).unwrap() - 1.1).abs() < 1e-6);
        assert!((obj.get("rate").and_then(|v| v.as_f64()).unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(obj.get("emotion").and_then(|v| v.as_str()), Some("joy"));
    }

    /// T6: tabla de resolución voz → motor (default resuelve como Preset(default)).
    #[test]
    fn resolve_voice_motor_tabla() {
        // default sin referencia resuelve como Preset("default"); con qvoice resuelve como Clonada.
        assert_eq!(
            resolve_voice_motor("default", None),
            VozMotor::Preset("default".to_string())
        );
        let q = std::env::temp_dir().join("avi_tts_test_referencia.qvoice");
        std::fs::write(&q, b"QVCE").unwrap();
        assert_eq!(
            resolve_voice_motor("mi_voz", Some(&q)),
            VozMotor::Clonada(q.clone())
        );
        // Sin referencia → preset con el nombre dado.
        assert_eq!(
            resolve_voice_motor("vivian", None),
            VozMotor::Preset("vivian".to_string())
        );
        std::fs::remove_file(&q).ok();
    }

    /// T5: el healthcheck responde cuando el listener simula `/v1/health`, y
    /// el `Drop` del gestor termina al hijo.
    #[test]
    fn residente_healthcheck_ok_y_drop_mata_al_hijo() {
        let (port, handle) = resident::simular_servidor(Arc::new(Mutex::new(String::new())));
        let child = proceso_durmiente();
        let pid = child.id();
        let resident = resident::Qwen3TtsResident::spawn_con_hijo(
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
            !proceso_vivo(pid),
            "el Drop del gestor debe terminar al hijo"
        );
    }

    /// T5: el healthcheck reintenta hasta que el servidor responde. El simulador
    /// cierra las dos primeras conexiones sin responder (fallo inmediato) y solo
    /// responde 200 a partir de la tercera (determinista, sin temporización).
    #[test]
    fn residente_healthcheck_reintenta_hasta_responder() {
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
        let child = proceso_durmiente();
        let resultado = resident::Qwen3TtsResident::spawn_con_hijo(
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
        if let Err(e) = resultado {
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

    /// T5: el healthcheck falla si el servidor nunca responde.
    #[test]
    fn residente_healthcheck_falla_sin_servidor() {
        let child = proceso_durmiente();
        let result = resident::Qwen3TtsResident::spawn_con_hijo(
            child,
            1,
            resident::resident_log_path(),
            3,
            30,
        );
        assert!(result.is_err(), "sin servidor el healthcheck debe fallar");
    }

    /// T5.1 (H-05): un sumidero TCP que acepta la conexión y nunca responde
    /// (a diferencia del crash de `wait_health_distingue_crash_de_hang`, aquí
    /// el proceso hijo sigue vivo) debe hacer que `wait_health(1, 2000)`
    /// devuelva `Err` en `≲` 3 s, sin colgarse — reproduce el cuelgue del
    /// motor C que motivó la salud observada por petición.
    #[test]
    fn wait_health_detecta_sumidero_tcp_sin_colgarse() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            // Acepta y retiene la conexión sin leer ni escribir: sumidero puro.
            if let Ok((stream, _)) = listener.accept() {
                thread::sleep(Duration::from_secs(3));
                drop(stream);
            }
        });
        let mut child = proceso_durmiente();
        let log_path = resident::resident_log_path();
        let inicio = std::time::Instant::now();
        let result = resident::wait_health(&mut child, port, 1, 2000, log_path.as_path());
        let transcurrido = inicio.elapsed();
        let _ = child.kill();
        let _ = child.wait();
        drop(handle);
        assert!(
            result.is_err(),
            "el sumidero TCP no debe pasar el healthcheck"
        );
        assert!(
            transcurrido < Duration::from_secs(3),
            "wait_health no debe colgarse ante un sumidero TCP: tardó {:?}",
            transcurrido
        );
    }

    /// T5: `resident_log_path()` crea el directorio `logs/` bajo `data_dir()` y
    /// devuelve un filename con el patrón `qwen3-tts_<pid>_<ms>.log`.
    #[test]
    fn log_path_crea_directorio_y_filename() {
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

    /// T5: `wait_health` distingue *crash* (el child muere inesperadamente) de
    /// *hang* (timeout). Un proceso que sale inmediatamente produce un error que
    /// menciona "terminó inesperadamente" + código de salida + ruta del log.
    #[test]
    fn wait_health_distingue_crash_de_hang() {
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

    /// T7: el body del POST contra un servidor simulado transporta los defaults
    /// del motor (e9) y sus overrides. Afirma los defaults del `struct`/motor sin
    /// cambios, no los valores de producción de `GenerationOptions::produccion()`
    /// (Tarea 1) — este test invoca `synthesize_with_options` directamente con
    /// `GenerationOptions::default()`, no `Qwen3TtsEngine::synthesize`.
    #[test]
    fn synthesize_http_envia_defaults_del_motor() {
        let body = Arc::new(Mutex::new(String::new()));
        let (port, handle) = resident::simular_servidor(body.clone());
        let engine = Qwen3TtsEngine::new(Some(format!("http://127.0.0.1:{}", port)));
        let profile = VoiceProfile {
            name: "default".to_string(),
            reference_audio: None,
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
        let (port, handle) = resident::simular_servidor(body.clone());
        let engine = Qwen3TtsEngine::new(Some(format!("http://127.0.0.1:{}", port)));
        let _ = engine.synthesize_with_options("Hola", &profile, &opts, Some(&out));
        drop(handle);
        let parsed: serde_json::Value = serde_json::from_str(&body.lock().unwrap()).unwrap();
        assert!((parsed["temperature"].as_f64().unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(parsed["seed"], 7);
    }

    /// Proceso que duerme para simular el hijo del residente en tests. Hijo
    /// directo bien portado (recolectable vía `Child`): no reproduce el
    /// desacoplo del `qwen_tts` real ni la daemonización (ceguera H-01, T7);
    /// nunca reproduce `panic!` con lock retenido ni aborto externo (D-03) ni
    /// la ventana spawn→write ni señales (D-02, solo-harness/producto); nunca
    /// usa imagen `qwen_tts` ni deja resto sin puerto (D-04);
    /// el cierre preciso por árbol se cubre en
    /// `residente_matar_arbol_por_pid_termina_al_hijo`.
    fn proceso_durmiente() -> std::process::Child {
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

    /// ¿Sigue vivo el proceso con `pid`? Doble de test (H-01, T7): delega en
    /// `resident::pid_vivo_residente`, la misma primitiva que el producto usa
    /// para verificar el cierre por árbol, sin reproducir daemonización real.
    fn proceso_vivo(pid: u32) -> bool {
        resident::pid_vivo_residente(pid)
    }

    /// H-01 (T7): el cierre preciso por árbol termina un hijo real a nivel SO
    /// con verificación, sin daemonización (hijo directo, no el `qwen_tts`
    /// desacoplado). Cubre `matar_arbol_residente_por_pid` +
    /// `pid_vivo_residente` con recolección del estado (como el `Drop`).
    #[test]
    fn residente_matar_arbol_por_pid_termina_al_hijo() {
        let mut child = proceso_durmiente();
        let pid = child.id();
        assert!(
            resident::pid_vivo_residente(pid),
            "el hijo debe estar vivo tras el spawn (pid {})",
            pid
        );
        resident::matar_arbol_residente_por_pid(pid);
        let inicio = std::time::Instant::now();
        while inicio.elapsed() < std::time::Duration::from_secs(10) {
            // Recolecta si ya murió (en Unix el zombi sin recolectar sigue
            // respondiendo al sondeo de viveza hasta el `wait`).
            let _ = child.try_wait();
            if !resident::pid_vivo_residente(pid) {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !resident::pid_vivo_residente(pid),
            "el árbol preciso debe terminar al hijo (pid {})",
            pid
        );
        // Recolección final del estado, como el `Drop` del residente.
        let _ = child.wait();
    }

    /// T4: `resolve_binary` halla el binario junto al `current_exe` aunque `cwd` no tenga vendor.
    #[test]
    fn resolve_binary_halla_exe_dir_vendor() {
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
