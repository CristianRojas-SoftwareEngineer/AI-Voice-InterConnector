use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Defaults de muestreo del motor Qwen3-TTS, tomados de su código fuente; los
/// defaults del host deben coincidir para que la omisión de flags HTTP/CLI sea
/// idéntica a pasarlos explícitos.
pub const DEFAULT_TEMPERATURE: f32 = 0.5;
pub const DEFAULT_TOP_K: u32 = 50;
pub const DEFAULT_TOP_P: f32 = 1.0;
pub const DEFAULT_REP_PENALTY: f32 = 1.05;

/// Puerto por defecto del servidor residente (el daemon del host ocupa el 8765).
pub const DEFAULT_PORT: u16 = 8766;

/// Dirección de escucha del servidor residente. El residente no tiene
/// autenticación, así que solo escucha en loopback, igual que el daemon. Esta
/// constante fija tanto el `--host` que recibe el motor como las URLs con las
/// que el cliente le habla, de modo que servidor y cliente no puedan divergir.
pub const RESIDENT_HOST: std::net::Ipv4Addr = std::net::Ipv4Addr::LOCALHOST;

/// Nombre de imagen del proceso residente (`qwen_tts`). Fuente única para la
/// resolución del binario y para el barrido por imagen de último recurso
/// (`resident::sweep_resident_by_image`). El residente tiene imagen propia
/// —a diferencia del daemon, que comparte imagen con el CLI—, así que el
/// kill-por-imagen es seguro sólo para el residente.
#[cfg(windows)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts.exe";
#[cfg(unix)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts";

/// Familia de logs del motor, compartida por el residente y el clonado: ambos
/// escriben `qwen3-tts_<pid>_<ms>.log` en `logs_dir()`.
pub(crate) const ENGINE_LOG_FAMILY: &str = "qwen3-tts";

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
/// prometer control emocional: es una restricción conocida del modelo.
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
    /// Síntesis interrumpible: activar `cancel` cierra la conexión con el motor,
    /// que aborta la generación en curso y queda libre para la siguiente
    /// petición; la llamada devuelve entonces `SynthesisCancelled`.
    fn synthesize_cancellable(
        &self,
        text: &str,
        profile: &VoiceProfile,
        options: &GenerationOptions,
        output_path: Option<&PathBuf>,
        cancel: &AtomicBool,
    ) -> Result<PathBuf>;

    /// Síntesis sin cancelación externa (solo la acota su presupuesto).
    fn synthesize_with_options(
        &self,
        text: &str,
        profile: &VoiceProfile,
        options: &GenerationOptions,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        self.synthesize_cancellable(text, profile, options, output_path, &AtomicBool::new(false))
    }
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
/// 4. snapshot en la raíz de modelos `ModelStore::model_snapshot_path("qwen3-tts-0.6b")`;
/// 5. `<cwd>/vendor/qwen3-tts/qwen3-tts-0.6b`.
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
/// que exige el modelo Base, distinto del
/// CustomVoice usado por la síntesis general.
/// Orden: 1. `QWEN3_TTS_BASE_MODEL_DIR`; 2. directorio hermano del binario
/// (`<dir del bin>/qwen3-tts-0.6b-base`); 3. `<exe_dir>/vendor/qwen3-tts/qwen3-tts-0.6b-base`;
/// 4. snapshot en la raíz de modelos `ModelStore::model_snapshot_path("qwen3-tts-0.6b-base")`;
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
    // Capa modelos: snapshot provisionado en la raíz de modelos por setup --with-voice-cloning
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
/// único camino de síntesis (el healthcheck está acotado; el POST de síntesis
/// se acota con el presupuesto proporcional al texto).
pub struct Qwen3TtsEngine {
    pub server_url: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub model_dir: Option<PathBuf>,
    pub base_model_dir: Option<PathBuf>,
    resident: Mutex<Option<ResidentState>>,
    /// PID del proceso `qwen_tts.exe` arrancado (0 si no hay). Se usa en `shutdown`
    /// como señal binaria «hubo residente» (0 / no-0) para decidir si invocar el
    /// kill, SIN tomar el `Mutex<resident>` que el hilo `spawn_blocking(warmup)`
    /// retiene durante el spawn + `wait_health` + síntesis HTTP.
    resident_pid: AtomicU32,
}

/// Ventana de la salud observada por petición: `retries=1`,
/// `interval_ms=2000` acotan el healthcheck a ~2 s por petición. Generoso
/// para un `GET /v1/health` sano en loopback (responde en ms) y muy por
/// debajo del presupuesto de síntesis (mínimo 30 s), de modo que la
/// observación de salud nunca consume el plazo de una síntesis. El motor
/// atiende una petición cada vez, así que no responder en la ventana
/// significa colgado u ocupado: tras una síntesis cancelada el motor aborta
/// en un fotograma y responde dentro de la ventana, y uno que no aborta se
/// reemplaza aquí.
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

    /// Intentar la síntesis vía HTTP local (servidor manual o residente). El
    /// `budget` es el plazo total del `POST`; si vence, el error devuelto
    /// envuelve `SynthesisTimeout`, y si se activa `cancel`, `SynthesisCancelled`.
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
        budget: Duration,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let body = build_tts_body(text, voice, options, prosody, emotion).to_string();
        let (status, bytes) = http_exchange(
            &format!("{}/v1/tts", server_url),
            "POST",
            Some(&body),
            budget,
            Some(cancel),
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
    /// huérfano reteniendo el lock. El plazo total del POST es el presupuesto
    /// proporcional a la longitud del texto (`avi_core::synthesis_budget`).
    /// Al vencer o al activarse `cancel` se cierra la conexión y el motor
    /// aborta la generación, así que el residente sigue caliente y el
    /// `health_check` de la siguiente petición lo reutiliza.
    fn synthesize_via_resident(
        &self,
        text: &str,
        voice: &VoiceEngine,
        options: &GenerationOptions,
        out_path: &Path,
        cancel: &AtomicBool,
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
            format!("http://{}:{}", RESIDENT_HOST, state.resident.port)
        };
        // Una cancelación durante el arranque o la salud observada no debe
        // encargar al motor un trabajo que nadie espera.
        if cancel.load(Ordering::SeqCst) {
            return Err(anyhow::Error::new(SynthesisCancelled));
        }
        let budget = avi_core::synthesis_budget(text.chars().count());
        self.synthesize_via_http(
            &url, text, voice, options, None, None, out_path, budget, cancel,
        )
    }
}

/// Actualiza `resident_pid` en `daemon.pid` preservando el resto del esquema
/// como campo plano. Escritura atómica por tmp+rename; best-effort y
/// silenciosa: si no hay pidfile (p. ej. `serve` en foreground) o no parsea,
/// no hay nada que actualizar y se ignora. La lectura tolerante vive en el CLI
/// (`read_resident_pid`: ausente = 0/desconocido).
fn update_resident_pid_in_pidfile(pid: u32) {
    update_resident_pid_at(&avi_store::data_dir().join("daemon.pid"), pid);
}

/// Cuerpo de [`update_resident_pid_in_pidfile`] sobre una ruta concreta.
fn update_resident_pid_at(path: &Path, pid: u32) {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => return,
    };
    let mut v: serde_json::Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return,
    };
    v["resident_pid"] = serde_json::Value::from(pid);
    // La identidad se observa ahora, con el residente recién creado: es la que
    // la parada compara después para no confundir un PID reasignado con él.
    v["resident_identity"] =
        serde_json::to_value(avi_process::process_identity(pid)).unwrap_or(serde_json::Value::Null);
    let tmp = path.with_extension("pid.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&v).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(&tmp, path);
    }
}

impl TtsEngine for Qwen3TtsEngine {
    fn synthesize_cancellable(
        &self,
        text: &str,
        profile: &VoiceProfile,
        options: &GenerationOptions,
        output_path: Option<&PathBuf>,
        cancel: &AtomicBool,
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
                    .synthesize_via_http(
                        url,
                        text,
                        &voice,
                        options,
                        None,
                        None,
                        &path,
                        avi_core::synthesis_budget(text.chars().count()),
                        cancel,
                    )
                    .is_ok()
            {
                return Ok(path);
            }
        }

        // 2. Servidor residente gestionado por el host: único
        //    camino restante, con healthcheck y POST acotados (este último por
        //    el presupuesto de síntesis del texto).
        //    El texto viaja por body HTTP JSON, ruta segura para UTF-8 acentuado
        //    (a diferencia del argv de un subprocess en Windows).
        self.synthesize_via_resident(text, &voice, options, &path, cancel)?;
        Ok(path)
    }
}

/// Construye el body HTTP de `POST /v1/tts`: sin `format` (el
/// servidor lo ignora), claves solo-si-`Some`, y `speaker`/`language` omitidos
/// cuando la voz es clonada (el servidor conserva la voz y el idioma del
/// arranque, según el servidor).
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

/// La síntesis no respondió dentro de su presupuesto de tiempo (proporcional a
/// la longitud del texto). Se distingue de otros fallos de E/S para que los
/// llamadores lo reporten como `synthesis_timeout`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthesisTimeout {
    /// Tiempo máximo que se concedió a la síntesis.
    pub budget: Duration,
}

impl std::fmt::Display for SynthesisTimeout {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "La síntesis superó su presupuesto de {} s sin completarse.",
            self.budget.as_secs()
        )
    }
}

impl std::error::Error for SynthesisTimeout {}

/// El llamante canceló la síntesis antes de que terminara (plazo vencido en
/// una capa superior, cliente desconectado o fase descartada).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SynthesisCancelled;

impl std::fmt::Display for SynthesisCancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "La síntesis se canceló antes de completarse.")
    }
}

impl std::error::Error for SynthesisCancelled {}

/// Intervalo máximo entre dos consultas de la bandera de cancelación y del
/// plazo mientras se espera la respuesta: acota a ~100 ms la latencia con la
/// que una cancelación cierra la conexión.
const CANCEL_POLL: Duration = Duration::from_millis(100);

/// Cliente HTTP/1.1 mínimo sobre `TcpStream` (sin runtime async): suficiente
/// para `/v1/health` y `/v1/tts` del motor. Evita `reqwest::blocking`, que
/// paniquea al dropearse dentro del runtime tokio de la CLI ("Cannot drop a
/// runtime in a context where blocking is not allowed").
///
/// Envía `Connection: close` y lee la respuesta hasta EOF; devuelve
/// (código de estado, bytes del body). `timeout` es el plazo total del
/// intercambio (conexión, envío y lectura): al vencer devuelve
/// `SynthesisTimeout`, aunque el servidor siga enviando bytes a goteo. Si
/// `cancel` se activa, devuelve `SynthesisCancelled`. En ambos casos el
/// socket se cierra al soltarse, y ese cierre es la señal con la que el motor
/// aborta la generación. Por eso nunca se cierra la mitad de escritura tras
/// enviar la petición: el motor lo interpretaría como un abandono.
fn http_exchange(
    url: &str,
    method: &str,
    body: Option<&str>,
    timeout: Duration,
    cancel: Option<&AtomicBool>,
) -> Result<(u16, Vec<u8>)> {
    let deadline = Instant::now() + timeout;
    let timed_out = || anyhow::Error::new(SynthesisTimeout { budget: timeout });
    let remaining = || deadline.saturating_duration_since(Instant::now());
    let (host, port, path) = parse_http_url(url)?;
    let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))?
        .next()
        .ok_or_else(|| anyhow!("No se pudo resolver {}:{}", host, port))?;
    let mut stream = std::net::TcpStream::connect_timeout(&addr, timeout)?;
    let is_timeout = |e: &std::io::Error| {
        matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
        )
    };
    let left = remaining();
    if left.is_zero() {
        return Err(timed_out());
    }
    stream.set_write_timeout(Some(left))?;
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
    if let Some(b) = body {
        req.push_str(b);
    }
    stream.write_all(req.as_bytes()).map_err(|e| {
        if is_timeout(&e) {
            timed_out()
        } else {
            anyhow::Error::new(e)
        }
    })?;
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
            return Err(anyhow::Error::new(SynthesisCancelled));
        }
        let left = remaining();
        if left.is_zero() {
            return Err(timed_out());
        }
        stream.set_read_timeout(Some(left.min(CANCEL_POLL)))?;
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
            Err(e) if is_timeout(&e) || e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(anyhow::Error::new(e)),
        }
    }
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
/// el benchmark preprocesaba la referencia de la misma forma. El fallo de carga se
/// propaga como `avi_audio::WavLoadError` dentro del `anyhow::Error`, de modo que
/// quien llama lo recupera con `downcast_ref`.
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
/// <ref> --save-voice <out> --voice-name <name> -l <language> --watch-stdin`. La referencia se
/// normaliza antes a 24 kHz mono (requisito del motor). Propaga el error con el
/// exit code del proceso. Si la referencia no se puede cargar (WAV inválido,
/// truncado o fallo de lectura), el error conserva el `avi_audio::WavLoadError`
/// recuperable con `downcast_ref`.
///
/// stdout y stderr del motor van al log de su familia (`qwen3-tts_*.log`), no a
/// la terminal: en la vía directa tapaban el resumen del comando y en la vía
/// daemon se perdían. El error de un clonado fallido cita la ruta de ese log.
///
/// La entrada estándar del motor es una tubería que este proceso mantiene
/// abierta hasta que el motor termina; con `--watch-stdin` el motor se detiene
/// si esa tubería se cierra, es decir, si este proceso muere.
pub fn clone_voice(
    model_dir: impl AsRef<Path>,
    ref_audio: &Path,
    out_qvoice: &Path,
    name: &str,
    language: &str,
) -> Result<()> {
    // La referencia se carga antes de buscar el binario: un audio inválido se
    // rechaza sin depender de que el motor esté provisionado.
    let ref_wav = reference_24k_mono(ref_audio)?;
    let Some(bin) = resolve_binary() else {
        let _ = std::fs::remove_file(&ref_wav);
        return Err(anyhow!(
            "El binario de clonado Qwen3-TTS no está provisionado."
        ));
    };
    let (log_path, log_file) =
        match avi_store::create_log(&avi_store::logs_dir(), ENGINE_LOG_FAMILY) {
            Ok(log) => log,
            Err(e) => {
                let _ = std::fs::remove_file(&ref_wav);
                return Err(anyhow!("No se pudo crear el log del motor: {}", e));
            }
        };
    // La entrada del motor es una tubería con `--watch-stdin`: el motor termina
    // si su entrada se cierra, así que el proceso que clona muere con él aunque
    // sea abatido. El extremo de escritura queda retenido en el hijo
    // restringido hasta que `wait` devuelve; si no, el motor vería fin de
    // fichero al instante. La herencia queda restringida a la lista explícita
    // (tubería de entrada y fichero de registro).
    let status = (|| -> Result<std::process::ExitStatus> {
        let args = vec![
            String::from("-d"),
            model_dir.as_ref().to_string_lossy().into_owned(),
            String::from("--ref-audio"),
            ref_wav.to_string_lossy().into_owned(),
            String::from("--save-voice"),
            out_qvoice.to_string_lossy().into_owned(),
            String::from("--voice-name"),
            name.to_string(),
            String::from("-l"),
            language.to_string(),
            String::from("--watch-stdin"),
        ];
        #[cfg(windows)]
        let creation_flags = avi_process::DETACHED_PROCESS;
        #[cfg(not(windows))]
        let creation_flags = 0u32;
        let request = avi_process::RestrictedSpawnRequest {
            program: bin.clone(),
            args,
            stdin: avi_process::StdinSpec::Piped,
            log_file,
            creation_flags,
            extra_allowed: Vec::new(),
        };
        let mut child = avi_process::spawn_with_allowlist(request)?;
        Ok(child.wait()?)
    })();
    let _ = std::fs::remove_file(&ref_wav);
    let status = status?;
    if status.success() {
        Ok(())
    } else {
        Err(anyhow!(
            "El subproceso de clonado Qwen3-TTS finalizó con código de error: {:?}. Log del motor: {}",
            status.code(),
            log_path.display()
        ))
    }
}

/// Servidor residente del motor Qwen3-TTS: spawn perezoso con
/// `--serve <puerto> --host 127.0.0.1 --int4 -j 4 --stream --watch-stdin [--load-voice <qvoice> --icl-only]`,
/// healthcheck `GET /v1/health` con reintentos y terminación del hijo en `Drop`.
/// El residente guarda abierto el extremo de escritura de la tubería de entrada
/// del motor; si el daemon muere de forma abrupta, la tubería se cierra y el
/// motor termina.
pub mod resident {
    use super::*;
    #[cfg(test)]
    use std::io::Read;
    #[cfg(test)]
    use std::io::Write;
    #[cfg(test)]
    use std::net::TcpListener;
    use std::thread;
    use std::time::Duration;

    /// Gestor del proceso servidor del motor.
    pub struct Qwen3TtsResident {
        child: Option<avi_process::RestrictedChild>,
        pub port: u16,
        /// Ruta del fichero de log de stderr del motor (incluida en errores de healthcheck).
        #[allow(dead_code)]
        pub(crate) log_path: PathBuf,
    }

    /// Construye el argv de arranque del residente, sin I/O real:
    /// programa más argumentos `-d <model_dir> --serve <port> --host 127.0.0.1
    /// --int4 -j 4 --stream --watch-stdin [--load-voice <qvoice> --icl-only]`.
    /// `--watch-stdin` hace que el motor termine al cerrarse su entrada estándar.
    pub(crate) fn build_resident_command(
        bin: &Path,
        model_dir: &Path,
        port: u16,
        load_voice: Option<&Path>,
    ) -> (PathBuf, Vec<String>) {
        let mut args = vec![
            String::from("-d"),
            model_dir.to_string_lossy().into_owned(),
            String::from("--serve"),
            port.to_string(),
            String::from("--host"),
            crate::RESIDENT_HOST.to_string(),
            String::from("--int4"),
            String::from("-j"),
            String::from("4"),
            String::from("--stream"),
            String::from("--watch-stdin"),
        ];
        if let Some(lv) = load_voice {
            args.push(String::from("--load-voice"));
            args.push(lv.to_string_lossy().into_owned());
            args.push(String::from("--icl-only"));
        }
        (bin.to_path_buf(), args)
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
            // El stderr del motor va a un log de la familia `qwen3-tts`, creado y
            // podado por el auxiliar común (los 10 más recientes). Captura los ~20
            // `fprintf(stderr, *)` del motor C (cuyos mensajes se perdían a null,
            // dejando ciego el diagnóstico del warmup, la síntesis por petición y
            // la terminación del residente).
            let (log_path, log_file) =
                avi_store::create_log(&avi_store::logs_dir(), crate::ENGINE_LOG_FAMILY)
                    .map_err(|e| anyhow!("No se pudo crear el log del motor: {}", e))?;
            let child =
                launch_resident_process(&bin, model_dir.as_ref(), port, load_voice, log_file)?;
            Self::spawn_with_child(child, port, log_path, 60, 500)
        }
    }

    /// Lanza el proceso del motor residente: construye el argv de arranque,
    /// configura la entrada por tubería propia y el error al fichero de
    /// registro, y devuelve el hijo con herencia restringida sin esperar a
    /// que el motor esté sano.
    pub(crate) fn launch_resident_process(
        bin: &Path,
        model_dir: &Path,
        port: u16,
        load_voice: Option<&Path>,
        log_file: std::fs::File,
    ) -> Result<avi_process::RestrictedChild> {
        let (program, args) = build_resident_command(bin, model_dir, port, load_voice);
        {
            // La entrada estándar es una tubería cuyo extremo de escritura queda
            // retenido dentro del hijo devuelto: mientras el residente viva,
            // quien lo guarda mantiene la tubería abierta. El motor, lanzado
            // con `--watch-stdin`, lee su entrada hasta fin de fichero y
            // termina; así, si el daemon muere de forma abrupta, el sistema
            // cierra el extremo de escritura y el residente muere con él. El
            // stdout va a null porque no se consume.
            // Windows: `qwen_tts.exe` NO debe heredar handles del padre salvo
            // los declarados. `DETACHED_PROCESS` lo deja sin consola:
            // `qwen_tts` no lanza procesos de consola, así que no necesita
            // una propia, y va sin grupo propio para que `taskkill /T` lo
            // alcance. La herencia queda restringida a la lista explícita
            // (tubería de entrada y fichero de registro): el pipe del abuelo
            // no viaja al motor por construcción.
            // Sin `CREATE_NEW_PROCESS_GROUP` ni breakaway, para que
            // `taskkill /F /T /PID <daemon>` alcance al residente como descendiente.
            // En Unix tampoco se hace `setsid` aquí: hereda el grupo del daemon.
            // Si el daemon muere de forma abrupta, la tubería de entrada se cierra y
            // el residente termina por sí mismo; los dobles de test nunca
            // reproducen la muerte abrupta del padre.
            #[cfg(windows)]
            let creation_flags = avi_process::DETACHED_PROCESS;
            #[cfg(not(windows))]
            let creation_flags = 0u32;
            let request = avi_process::RestrictedSpawnRequest {
                program,
                args,
                stdin: avi_process::StdinSpec::Piped,
                log_file,
                creation_flags,
                extra_allowed: Vec::new(),
            };
            avi_process::spawn_with_allowlist(request).map_err(|e| {
                anyhow!(
                    "No se pudo arrancar el servidor Qwen3-TTS ({}): {}",
                    bin.display(),
                    e
                )
            })
        }
    }

    impl Qwen3TtsResident {
        /// Arranca el healthcheck sobre un hijo ya lanzado (retries/intervalo
        /// configurables para los tests de reintentos). `log_path` se guarda en el
        /// struct para incluirse en errores de `wait_health`.
        pub(crate) fn spawn_with_child(
            child: avi_process::RestrictedChild,
            port: u16,
            log_path: PathBuf,
            retries: usize,
            interval_ms: u64,
        ) -> Result<Self> {
            let mut child = child;
            if let Err(e) = wait_health(&mut child, port, retries, interval_ms, log_path.as_path())
            {
                // Un motor que no llegó a estar sano se termina y se recoge aquí:
                // soltar el `Child` sin `wait` dejaría un zombi en Unix, que
                // seguiría pareciendo vivo para una comprobación por PID.
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
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

    /// Healthcheck `GET /v1/health` con reintentos. Distingue *crash* del motor
    /// (el `child` terminó inesperadamente) de *hang* (timeout agotado). En caso
    /// de crash incluye el código de salida y la ruta del log de stderr.
    pub(crate) fn wait_health(
        child: &mut avi_process::RestrictedChild,
        port: u16,
        retries: usize,
        interval_ms: u64,
        log_path: &Path,
    ) -> Result<()> {
        let url = format!("http://{}:{}/v1/health", crate::RESIDENT_HOST, port);
        let crashed = |status: std::process::ExitStatus| {
            anyhow!(
                "El servidor Qwen3-TTS terminó inesperadamente (exit {}) antes \
                 de que el healthcheck pasara. Log de stderr: {}",
                status.code().unwrap_or(-1),
                log_path.display()
            )
        };
        for i in 0..retries {
            // Detecta crash inmediato: si el proceso terminó, el motor no va a
            // responder nunca. `try_wait` no bloquea.
            if let Some(status) = child.try_wait()? {
                return Err(crashed(status));
            }
            let ok = http_exchange(&url, "GET", None, Duration::from_millis(interval_ms), None)
                .map(|(status, _)| (200..300).contains(&status))
                .unwrap_or(false);
            if ok {
                return Ok(());
            }
            if i + 1 < retries {
                thread::sleep(Duration::from_millis(interval_ms));
            }
        }
        // Un motor que muere durante el último intervalo también es un crash: sin
        // esta comprobación final se diagnosticaría como cuelgue.
        if let Some(status) = child.try_wait()? {
            return Err(crashed(status));
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
    /// serie pesada de extremo a extremo.
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

    /// Al registrar el residente, el pidfile trae su identidad y conserva el resto.
    #[test]
    fn resident_update_records_resident_identity() {
        let dir = std::env::temp_dir().join(format!("avi_tts_pidfile_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("daemon.pid");
        std::fs::write(
            &path,
            r#"{"pid": 4242, "addr": "127.0.0.1:7001", "resident_pid": 0}"#,
        )
        .unwrap();

        let own_pid = std::process::id();
        update_resident_pid_at(&path, own_pid);

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        let own = avi_process::process_identity(own_pid).expect("identidad propia");
        assert_eq!(v["resident_pid"], own_pid);
        assert_eq!(v["resident_identity"]["start"], own.start.as_str());
        assert_eq!(v["resident_identity"]["image"], own.image.as_str());
        assert_eq!(v["pid"], 4242);
        assert_eq!(v["addr"], "127.0.0.1:7001");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Los defaults del host deben coincidir con los defaults del motor
    /// (los del motor). Afirma los defaults del `struct`/motor sin
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
        let (program, args) = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("vendor/qwen3-tts/qwen3-tts-0.6b"),
            8766,
            None,
        );
        assert_eq!(program, PathBuf::from("qwen_tts.exe"));
        assert_eq!(
            args,
            vec![
                "-d",
                "vendor/qwen3-tts/qwen3-tts-0.6b",
                "--serve",
                "8766",
                "--host",
                "127.0.0.1",
                "--int4",
                "-j",
                "4",
                "--stream",
                "--watch-stdin",
            ]
        );

        let (program, args) = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            8766,
            Some(Path::new("voz.qvoice")),
        );
        assert_eq!(program, PathBuf::from("qwen_tts.exe"));
        assert_eq!(
            args,
            vec![
                "-d",
                "md",
                "--serve",
                "8766",
                "--host",
                "127.0.0.1",
                "--int4",
                "-j",
                "4",
                "--stream",
                "--watch-stdin",
                "--load-voice",
                "voz.qvoice",
                "--icl-only",
            ]
        );
    }

    /// Mata y recoge al motor al soltarse, también si la prueba falla, para no
    /// dejar procesos `qwen_tts` vivos.
    struct EngineGuard(std::process::Child);

    impl Drop for EngineGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    /// Lanza el motor real en modo `--serve` sobre un puerto efímero de
    /// loopback, con stdout y stderr descartados. Exige el binario y los pesos:
    /// si faltan, falla de forma explícita.
    fn spawn_real_engine(watch_stdin: bool, stdin: Stdio) -> EngineGuard {
        // El directorio de trabajo de `cargo test` es el del crate, así que la
        // ruta relativa `vendor/…` se ancla a la raíz del repositorio.
        let vendored = Path::new(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
            "../../vendor/qwen3-tts/qwen_tts.exe"
        } else {
            "../../vendor/qwen3-tts/qwen_tts"
        });
        let bin = std::env::var_os("QWEN3_TTS_BIN")
            .map(PathBuf::from)
            .or_else(|| vendored.is_file().then_some(vendored))
            .expect(
                "falta el binario del motor (QWEN3_TTS_BIN o vendor/qwen3-tts/qwen_tts.exe): \
             constrúyelo con `cargo xtask build-engine`",
            );
        assert!(bin.is_file(), "falta {}", bin.display());
        let model_dir = resolve_model_dir(Some(&bin)).expect(
            "faltan los pesos qwen3-tts-0.6b: aprovisiónalos con `ai-voice-interconnector setup`",
        );
        let port = TcpListener::bind("127.0.0.1:0")
            .expect("debe poder reservar un puerto efímero")
            .local_addr()
            .expect("el listener debe tener dirección")
            .port();
        let mut cmd = Command::new(&bin);
        cmd.arg("-d")
            .arg(&model_dir)
            .arg("--serve")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("--int4")
            .arg("-j")
            .arg("4");
        if watch_stdin {
            cmd.arg("--watch-stdin");
        }
        let child = cmd
            .stdin(stdin)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("debe poder lanzar el motor");
        EngineGuard(child)
    }

    /// Sin `--watch-stdin`, un motor lanzado con la entrada nula (que está en
    /// fin de fichero desde el primer byte) no termina al arrancar: el
    /// comportamiento sin el flag se conserva.
    #[test]
    #[ignore = "requiere el binario del motor y los pesos qwen3-tts-0.6b"]
    fn engine_without_watch_stdin_survives_null_stdin() {
        let mut engine = spawn_real_engine(false, Stdio::null());
        thread::sleep(Duration::from_secs(3));
        let status = engine.0.try_wait().expect("debe poder consultar al motor");
        assert!(
            status.is_none(),
            "sin --watch-stdin el motor no debe terminar con la entrada nula: {status:?}"
        );
    }

    /// Con `--watch-stdin` y la entrada en tubería, el motor sigue vivo mientras
    /// la tubería está abierta y termina en menos de 10 s al cerrarla.
    #[test]
    #[ignore = "requiere el binario del motor y los pesos qwen3-tts-0.6b"]
    fn engine_with_watch_stdin_exits_when_pipe_closes() {
        let mut engine = spawn_real_engine(true, Stdio::piped());
        let stdin = engine.0.stdin.take().expect("la entrada debe ser tubería");
        thread::sleep(Duration::from_secs(3));
        let status = engine.0.try_wait().expect("debe poder consultar al motor");
        assert!(
            status.is_none(),
            "con la tubería abierta el motor debe seguir vivo: {status:?}"
        );
        drop(stdin);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            if engine
                .0
                .try_wait()
                .expect("debe poder consultar al motor")
                .is_some()
            {
                exited = true;
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            exited,
            "al cerrar la tubería el motor debe terminar en menos de 10 s"
        );
    }

    /// Limpieza del motor doble: mata su árbol por PID y borra su directorio
    /// temporal al soltarse, también si la prueba falla, para no dejar procesos.
    struct DoubleCleanup {
        pid: u32,
        dir: PathBuf,
    }

    impl Drop for DoubleCleanup {
        fn drop(&mut self) {
            let _ = resident::kill_tree_resident_by_pid(self.pid);
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// Escribe un motor doble ejecutable en un directorio temporal propio y
    /// devuelve su ruta y el directorio. Al arrancar registra sus argumentos en
    /// `args.txt`, sondea su entrada estándar durante 500 ms y registra en
    /// `stdin.txt` si vio `eof` (entrada nula o cerrada) u `open` (tubería
    /// abierta). Con `hold`: si la tubería estaba abierta espera a que se cierre
    /// y sale; si vio `eof` no sale por sí mismo (duerme 60 s). Sin `hold`
    /// sale tras el sondeo.
    fn write_engine_double(name: &str, hold: bool) -> (PathBuf, PathBuf) {
        let dir =
            std::env::temp_dir().join(format!("avi-tts-double-{}-{}", name, std::process::id()));
        std::fs::create_dir_all(&dir).expect("debe poder crear el directorio del doble");
        // El doble se compila con `rustc` como ejecutable nativo en lugar de un
        // script: un `.cmd` intermedio bajo `DETACHED_PROCESS` abre una consola
        // nueva y el script hijo hereda su entrada de consola (siempre abierta)
        // en vez de la entrada que pasó el lanzador, falseando el sondeo.
        let source = r#"use std::io::Read;
use std::sync::mpsc;
use std::time::Duration;

const DIR: &str = r"__DIR__";
const HOLD: bool = __HOLD__;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::fs::write(format!("{DIR}/args.txt"), args.join(" ")).unwrap();
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let mut byte = [0u8; 1];
        let read = std::io::stdin().read(&mut byte).unwrap_or(0);
        let _ = tx.send(read);
    });
    let seen_eof = matches!(rx.recv_timeout(Duration::from_millis(500)), Ok(0));
    let state = if seen_eof { "eof" } else { "open" };
    std::fs::write(format!("{DIR}/stdin.txt"), state).unwrap();
    if HOLD {
        if seen_eof {
            std::thread::sleep(Duration::from_secs(60));
        } else {
            let _ = rx.recv();
        }
    }
}
"#
        .replace("__DIR__", &dir.display().to_string())
        .replace("__HOLD__", if hold { "true" } else { "false" });
        let src = dir.join("engine_double.rs");
        std::fs::write(&src, source).expect("debe escribir el código del doble");
        let bin = dir.join(if cfg!(windows) {
            "engine_double.exe"
        } else {
            "engine_double"
        });
        let compiled = Command::new("rustc")
            .arg("--edition")
            .arg("2021")
            .arg(&src)
            .arg("-o")
            .arg(&bin)
            .output()
            .expect("rustc debe estar disponible para compilar el doble");
        assert!(
            compiled.status.success(),
            "no se pudo compilar el doble: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        (bin, dir)
    }

    /// Espera (hasta 20 s) a que `path` exista con contenido y lo devuelve
    /// sin espacios en los extremos.
    fn wait_for_marker(path: &Path) -> String {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.trim().is_empty() {
                    return text.trim().to_string();
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        panic!("el motor doble no escribió {}", path.display());
    }

    /// Lanza el motor doble con la función de arranque del residente y un log
    /// temporal propio. Devuelve el hijo y la limpieza.
    fn launch_double(bin: &Path, dir: &Path) -> (avi_process::RestrictedChild, DoubleCleanup) {
        let log_file = std::fs::File::create(dir.join("engine.log")).expect("log del doble");
        let child =
            resident::launch_resident_process(bin, Path::new("modelos"), 8766, None, log_file)
                .expect("debe poder lanzar el motor doble");
        let cleanup = DoubleCleanup {
            pid: child.id(),
            dir: dir.to_path_buf(),
        };
        (child, cleanup)
    }

    /// El arranque del residente pasa `--watch-stdin` al motor, para que
    /// termine cuando su entrada se cierre.
    #[test]
    fn launch_resident_process_passes_watch_stdin() {
        let (bin, dir) = write_engine_double("args", false);
        let (_child, _cleanup) = launch_double(&bin, &dir);
        let args = wait_for_marker(&dir.join("args.txt"));
        assert!(
            args.split_whitespace().any(|a| a == "--watch-stdin"),
            "el motor debe recibir --watch-stdin; argumentos recibidos: {args}"
        );
    }

    /// Mientras el motor corre, su entrada estándar es una tubería abierta
    /// (no nula): si estuviera nula, el motor vería fin de fichero de inmediato.
    #[test]
    fn launch_resident_process_keeps_stdin_pipe_open() {
        let (bin, dir) = write_engine_double("stdin", false);
        let (_child, _cleanup) = launch_double(&bin, &dir);
        let state = wait_for_marker(&dir.join("stdin.txt"));
        assert_eq!(
            state, "open",
            "la entrada del motor debe ser una tubería abierta mientras corre"
        );
    }

    /// Si el healthcheck falla, el motor muere en menos de 5 s: al perder la
    /// tubería de entrada (o por terminación explícita) no queda vivo un
    /// motor sin gestor. El doble solo sale tras ver su tubería abierta y luego
    /// cerrada; con entrada nula duerme 60 s, así que la prueba lo distingue.
    #[test]
    fn spawn_with_child_failure_terminates_engine() {
        let (bin, dir) = write_engine_double("health", true);
        let (child, cleanup) = launch_double(&bin, &dir);
        // El sondeo de entrada del doble debe haber concluido antes de provocar
        // el fallo, para que el cierre de la tubería llegue después de verla abierta.
        let _ = wait_for_marker(&dir.join("stdin.txt"));
        let result = resident::Qwen3TtsResident::spawn_with_child(child, 1, test_log_path(), 2, 30);
        assert!(result.is_err(), "sin servidor el healthcheck debe fallar");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline && process_alive(cleanup.pid) {
            thread::sleep(Duration::from_millis(100));
        }
        assert!(
            !process_alive(cleanup.pid),
            "el motor debe haber muerto en menos de 5 s tras el fallo del healthcheck (pid {})",
            cleanup.pid
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
        let resident =
            resident::Qwen3TtsResident::spawn_with_child(child, port, test_log_path(), 10, 50)
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
        let result =
            resident::Qwen3TtsResident::spawn_with_child(child, port, test_log_path(), 5, 50);
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
        let result = resident::Qwen3TtsResident::spawn_with_child(child, 1, test_log_path(), 3, 30);
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
        let log_path = test_log_path();
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

    /// `wait_health` distingue *crash* (el child muere inesperadamente) de
    /// *hang* (timeout). Un proceso que sale inmediatamente produce un error que
    /// menciona "terminó inesperadamente" + código de salida + ruta del log.
    #[test]
    fn wait_health_distinguishes_crash_from_hang() {
        // Proceso que muere inmediatamente (exit 1) en lugar de servir.
        let mut child = exiting_process();
        // Se espera la muerte del hijo antes del healthcheck: lo que se prueba es
        // el diagnóstico de un hijo muerto, no cuánto tarda el SO en terminarlo.
        child.wait().expect("el proceso debe terminar");
        let log_path = test_log_path();
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

    /// Un hijo que muere cuando ya no quedan intentos sigue siendo un *crash*:
    /// `wait_health` lo comprueba también al agotar los reintentos, en lugar de
    /// informar de un cuelgue. Sin intentos, solo esa comprobación final lo ve.
    #[test]
    fn wait_health_detects_crash_after_retries_exhausted() {
        let mut child = exiting_process();
        child.wait().expect("el proceso debe terminar");
        let log_path = test_log_path();
        let err = resident::wait_health(&mut child, 1, 0, 50, log_path.as_path())
            .expect_err("el healthcheck debe fallar si el proceso muere");
        let msg = err.to_string();
        assert!(
            msg.contains("terminó inesperadamente"),
            "el error debe indicar crash, no timeout: {}",
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

    /// Un servidor que acepta la conexión pero nunca responde agota el
    /// presupuesto inyectado y produce `SynthesisTimeout` con ese presupuesto.
    /// `simulate_server` responde siempre, así que aquí se usa un listener mudo.
    #[test]
    fn synthesize_http_silent_server_yields_synthesis_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("puerto libre");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("conexión del cliente");
            thread::sleep(Duration::from_millis(1500));
        });
        let engine = Qwen3TtsEngine::new(None);
        let out = std::env::temp_dir().join("avi_tts_test_timeout.wav");
        let budget = Duration::from_millis(200);
        let err = engine
            .synthesize_via_http(
                &format!("http://127.0.0.1:{}", port),
                "Hola",
                &VoiceEngine::Preset("default".to_string()),
                &GenerationOptions::default(),
                None,
                None,
                &out,
                budget,
                &AtomicBool::new(false),
            )
            .expect_err("un servidor mudo debe agotar el presupuesto");
        let timeout = err
            .downcast_ref::<SynthesisTimeout>()
            .expect("el error debe envolver SynthesisTimeout");
        assert_eq!(timeout.budget, budget);
        handle.join().unwrap();
    }

    /// Un servidor que envía un byte cada 50 ms nunca deja vencer una lectura
    /// individual; el plazo total igualmente corta el intercambio.
    #[test]
    fn http_exchange_trickling_server_hits_total_deadline() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("puerto libre");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("conexión del cliente");
            for _ in 0..40 {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(50));
            }
        });
        let budget = Duration::from_millis(300);
        let started = Instant::now();
        let err = http_exchange(
            &format!("http://127.0.0.1:{}/v1/tts", port),
            "POST",
            Some("{}"),
            budget,
            None,
        )
        .expect_err("el goteo no debe extender el plazo total");
        let elapsed = started.elapsed();
        assert!(
            err.downcast_ref::<SynthesisTimeout>().is_some(),
            "el error debe envolver SynthesisTimeout: {}",
            err
        );
        assert!(
            elapsed < budget + Duration::from_secs(1),
            "el plazo total debe cortar el goteo: {:?}",
            elapsed
        );
        handle.join().unwrap();
    }

    /// Activar la bandera de cancelación cierra la conexión en menos de
    /// 500 ms y el servidor observa EOF: es la señal con la que el motor
    /// aborta la generación.
    #[test]
    fn http_exchange_cancel_flag_closes_connection() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("puerto libre");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("conexión del cliente");
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            // Consume la petición completa y espera el cierre del cliente.
            let mut buf = [0u8; 1024];
            loop {
                match stream.read(&mut buf) {
                    Ok(0) => return 0,
                    Ok(_) => continue,
                    Err(_) => return usize::MAX,
                }
            }
        });
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = {
            let cancel = cancel.clone();
            thread::spawn(move || {
                thread::sleep(Duration::from_millis(100));
                cancel.store(true, Ordering::SeqCst);
            })
        };
        let started = Instant::now();
        let err = http_exchange(
            &format!("http://127.0.0.1:{}/v1/tts", port),
            "POST",
            Some("{}"),
            Duration::from_secs(10),
            Some(&cancel),
        )
        .expect_err("la bandera debe cancelar el intercambio");
        let elapsed = started.elapsed();
        assert!(
            err.downcast_ref::<SynthesisCancelled>().is_some(),
            "el error debe envolver SynthesisCancelled: {}",
            err
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "la cancelación debe cerrar pronto: {:?}",
            elapsed
        );
        setter.join().unwrap();
        assert_eq!(handle.join().unwrap(), 0, "el servidor debe leer EOF");
    }

    /// Ruta de log de prueba bajo el directorio temporal: ninguna prueba toca los
    /// logs reales de `data_dir()`. No se crea el fichero, solo se pasa la ruta.
    fn test_log_path() -> PathBuf {
        std::env::temp_dir().join("avi-tts-test-engine.log")
    }

    /// Proceso que duerme para simular el hijo del residente en tests, lanzado
    /// con herencia restringida como el producto. Hijo directo bien portado
    /// (recolectable vía `RestrictedChild`): no reproduce el
    /// desacoplo del `qwen_tts` real ni la daemonización, así que queda ciego
    /// a ese escenario; nunca reproduce `panic!` con lock retenido ni aborto
    /// externo (solo el harness lo cubre) ni la ventana spawn→write ni señales
    /// (solo harness/producto lo cubren); no corre bajo la imagen real
    /// `qwen_tts`, así que queda ciego al barrido por imagen del residente;
    /// el cierre preciso por árbol se cubre en
    /// `resident_kill_tree_by_pid_terminates_child`.
    fn sleeping_process() -> avi_process::RestrictedChild {
        let log_file = std::fs::File::create(
            std::env::temp_dir().join(format!("avi-tts-sleep-{}.log", std::process::id())),
        )
        .expect("log del proceso durmiente");
        let (program, args) = if cfg!(windows) {
            (
                PathBuf::from("powershell"),
                vec![
                    String::from("-NoProfile"),
                    String::from("-Command"),
                    String::from("Start-Sleep -Seconds 30"),
                ],
            )
        } else {
            (PathBuf::from("sleep"), vec![String::from("30")])
        };
        let request = avi_process::RestrictedSpawnRequest {
            program,
            args,
            stdin: avi_process::StdinSpec::Null,
            log_file,
            creation_flags: 0,
            extra_allowed: Vec::new(),
        };
        avi_process::spawn_with_allowlist(request).expect("debe lanzar el proceso durmiente")
    }

    /// Hijo que termina de inmediato con exit 1, lanzado con herencia
    /// restringida como el producto.
    fn exiting_process() -> avi_process::RestrictedChild {
        let log_file = std::fs::File::create(
            std::env::temp_dir().join(format!("avi-tts-exit-{}.log", std::process::id())),
        )
        .expect("log del proceso saliente");
        let (program, args) = if cfg!(windows) {
            (
                PathBuf::from("cmd"),
                vec![String::from("/C"), String::from("exit 1")],
            )
        } else {
            (
                PathBuf::from("sh"),
                vec![String::from("-c"), String::from("exit 1")],
            )
        };
        let request = avi_process::RestrictedSpawnRequest {
            program,
            args,
            stdin: avi_process::StdinSpec::Null,
            log_file,
            creation_flags: 0,
            extra_allowed: Vec::new(),
        };
        avi_process::spawn_with_allowlist(request).expect("debe lanzar el proceso saliente")
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
