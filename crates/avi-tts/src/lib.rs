#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Defaults de muestreo del motor Qwen3-TTS, tomados de su código fuente; los
/// defaults del host deben coincidir para que la omisión de flags HTTP/CLI sea
/// idéntica a pasarlos explícitos.
pub const DEFAULT_TEMPERATURE: f32 = 0.5;
pub const DEFAULT_TOP_K: u32 = 50;
pub const DEFAULT_TOP_P: f32 = 1.0;
pub const DEFAULT_REP_PENALTY: f32 = 1.05;

/// Dirección de escucha del servidor residente. El residente no tiene
/// autenticación, así que solo escucha en loopback, igual que el daemon. Esta
/// constante fija tanto el `--host` que recibe el motor como las URLs con las
/// que el cliente le habla, de modo que servidor y cliente no puedan divergir.
pub const RESIDENT_HOST: std::net::Ipv4Addr = std::net::Ipv4Addr::LOCALHOST;

/// Nombre de imagen del proceso residente (`qwen_tts`). Fuente única para la
/// resolución del binario del residente. Nunca se usa para matar por imagen: el
/// residente muere con su daemon y se reclama por PID registrado.
#[cfg(windows)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts.exe";
#[cfg(unix)]
pub const RESIDENT_IMAGE_NAME: &str = "qwen_tts";

/// Familia de logs del motor, compartida por el residente y el clonado: ambos
/// escriben `qwen3-tts_<pid>_<ms>.log` en `logs_dir()`.
pub(crate) const ENGINE_LOG_FAMILY: &str = "qwen3-tts";

/// Reserva un puerto efímero de loopback para un residente: enlaza
/// `127.0.0.1:0`, lee el puerto que asignó el sistema y lo suelta. Cada arranque
/// reserva el suyo, de modo que la comprobación de salud de un residente nunca
/// puede ser respondida por otro. Entre soltar el puerto y el enlace del motor
/// queda una ventana mínima; si otro proceso lo tomara, el motor muere en su
/// enlace y el arranque falla de forma visible.
pub fn reserve_loopback_port() -> Result<u16> {
    let listener = std::net::TcpListener::bind((RESIDENT_HOST, 0))
        .map_err(|e| anyhow!("No se pudo reservar un puerto efímero de loopback: {}", e))?;
    let port = listener
        .local_addr()
        .map_err(|e| anyhow!("No se pudo leer el puerto reservado: {}", e))?
        .port();
    Ok(port)
}

/// Registro externo del residente. Quien lo implementa (el daemon) anota el
/// proceso recién lanzado antes de que se compruebe su salud y lo anula cuando
/// ya no existe; el motor no escribe ficheros por su cuenta.
pub trait ResidentRegistry: Send + Sync {
    /// Anota al residente con su PID, su identidad observada y su puerto. Un
    /// error aborta el arranque del residente.
    fn register(&self, pid: u32, identity: &avi_process::ProcessIdentity, port: u16) -> Result<()>;

    /// Anula el registro del residente `pid`. No debe tocar el de otro PID.
    fn unregister(&self, pid: u32);
}

/// Residente vivo conocido por el motor: PID e identidad observados al nacer.
type LiveResident = Mutex<Option<(u32, avi_process::ProcessIdentity)>>;

/// Toma el estado vivo tolerando un mutex poisoned: el estado es un valor
/// simple que ningún hilo deja a medias.
fn lock_live(
    live: &LiveResident,
) -> std::sync::MutexGuard<'_, Option<(u32, avi_process::ProcessIdentity)>> {
    live.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Opciones de generación para la síntesis de voz (API agnóstica del motor)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationOptions {
    /// Idioma de destino en token del producto (`es` o `en`); se traduce al
    /// nombre que entiende el motor al construir la petición.
    pub language: String,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    /// Penalización de repetición del motor (el motor la usa por defecto a
    /// 1.05).
    pub rep_penalty: f32,
    pub seed: Option<u64>,
}

/// Traduce un token de idioma del producto al nombre completo que acepta el
/// motor Qwen3-TTS. Cualquier otro token es un error: nunca se cae a un idioma
/// por defecto.
pub fn engine_language_name(token: &str) -> Result<&'static str> {
    match token {
        "es" => Ok("Spanish"),
        "en" => Ok("English"),
        other => Err(anyhow!(
            "Idioma de síntesis no soportado: `{}` (se admite `es` o `en`).",
            other
        )),
    }
}

impl GenerationOptions {
    /// Opciones con los defaults de muestreo del motor para el idioma de
    /// destino indicado (token del producto).
    pub fn new(language: &str) -> Self {
        Self {
            language: language.to_string(),
            temperature: DEFAULT_TEMPERATURE,
            top_k: DEFAULT_TOP_K,
            top_p: DEFAULT_TOP_P,
            rep_penalty: DEFAULT_REP_PENALTY,
            seed: None,
        }
    }

    /// Config de producción: `temperature=0.35` y `seed=4` fijos, resto de
    /// campos igual a `new`. Con temperatura 0 el Talker decodifica en greedy
    /// argmax sobre un clon x-vector-only sin plantilla de prosodia y la
    /// prosodia sale plana; 0.35 mantiene el muestreo estocástico (con lo que
    /// el `seed` es efectivo) sin soltarse como el default del motor (0.9). El
    /// `seed 4` se eligió por escucha: iguala la prosodia de referencia con una
    /// similitud de hablante alta y sin errores de transcripción.
    pub fn production(language: &str) -> Self {
        Self {
            temperature: 0.35,
            seed: Some(4),
            ..Self::new(language)
        }
    }

    /// Resuelve la temperatura opcional del CLI a opciones efectivas: sin flag
    /// se usa la config de producción; con flag se sobrescribe la temperatura.
    pub fn with_temperature(temperature: Option<f32>, language: &str) -> Self {
        let mut opts = Self::production(language);
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

/// Perfil de voz: su nombre y, si es clonada, la ruta de su `reference.qvoice`.
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

/// Voz resuelta hacia la semántica del motor: preset del servidor (modelo
/// CustomVoice) o voz clonada cargada al arranque con
/// `--load-voice <qvoice> --graft` sobre el modelo Base.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VoiceEngine {
    Preset(String),
    Cloned(PathBuf),
}

/// Voces de fábrica que son presets del motor (modelo CustomVoice): solo
/// estas se sintetizan sin `reference.qvoice`.
const PRESET_VOICES: &[&str] = &["ryan", "vivian"];

/// La voz no es clonada (no tiene `reference.qvoice`) ni es uno de los presets
/// del motor. Los llamadores lo distinguen de otros fallos de síntesis para
/// reportarlo como `voice_not_found`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoiceNotFound {
    /// Nombre de la voz solicitada.
    pub voice: String,
}

impl std::fmt::Display for VoiceNotFound {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "La voz '{}' no tiene referencia .qvoice y no es un preset del motor ({}).",
            self.voice,
            PRESET_VOICES.join(", ")
        )
    }
}

impl std::error::Error for VoiceNotFound {}

/// Resolución voz → motor: una voz con `reference.qvoice` es clonada; sin
/// referencia solo `ryan` y `vivian` (sin distinguir mayúsculas) resuelven como
/// preset. Cualquier otra voz es `VoiceNotFound`.
pub fn resolve_voice_engine(
    voice: &str,
    qvoice: Option<&Path>,
) -> std::result::Result<VoiceEngine, VoiceNotFound> {
    if let Some(q) = qvoice {
        if q.is_file() {
            return Ok(VoiceEngine::Cloned(q.to_path_buf()));
        }
    }
    // El almacén de voces normaliza los nombres a minúsculas; el preset se
    // identifica igual para que `Ryan` y `ryan` sean la misma voz.
    let normalized = voice.to_lowercase();
    if PRESET_VOICES.contains(&normalized.as_str()) {
        return Ok(VoiceEngine::Preset(normalized));
    }
    Err(VoiceNotFound {
        voice: voice.to_string(),
    })
}

/// Valida el valor de una variable de entorno que designa una ruta. Vacía o
/// ausente significa «sin definir» (`None`); definida, debe apuntar a un
/// archivo (`expect_file`) o a un directorio existentes, y si no es un error
/// que nombra la variable.
fn validated_env_path(
    var: &str,
    value: Option<std::ffi::OsString>,
    expect_file: bool,
) -> Result<Option<PathBuf>> {
    let Some(value) = value.filter(|v| !v.is_empty()) else {
        return Ok(None);
    };
    let path = PathBuf::from(value);
    let valid = if expect_file {
        path.is_file()
    } else {
        path.is_dir()
    };
    if valid {
        Ok(Some(path))
    } else {
        Err(anyhow!(
            "{} apunta a '{}', que no es {} existente.",
            var,
            path.display(),
            if expect_file {
                "un archivo"
            } else {
                "un directorio"
            }
        ))
    }
}

/// Opt-in que habilita la capa `<cwd>/vendor/...` en la resolución del motor
/// y sus modelos. Sin `AVI_ALLOW_CWD_VENDOR=1`, el directorio de trabajo no
/// aporta binario ni modelos: un `vendor/` ajeno en el CWD no puede suplantar
/// al motor en silencio. Cualquier otro valor (incluido vacío o ausente)
/// mantiene la denegación por defecto.
const CWD_VENDOR_OPT_IN: &str = "AVI_ALLOW_CWD_VENDOR";

/// `true` solo con `AVI_ALLOW_CWD_VENDOR=1`.
fn cwd_vendor_allowed() -> bool {
    std::env::var(CWD_VENDOR_OPT_IN).as_deref() == Ok("1")
}

/// Resolución del binario del motor por capas:
/// 1. `QWEN3_TTS_BIN` (si está definida debe ser un archivo existente);
///    2. `<directorio real del programa>/vendor/qwen3-tts/qwen_tts(.exe)`; 3. `<cwd>/vendor/qwen3-tts/qwen_tts(.exe)`
///    (solo con `AVI_ALLOW_CWD_VENDOR=1`; si habría resuelto, avisa qué se
///    ignoró y cómo habilitarlo);
///    4. búsqueda en `PATH`. `Ok(None)` indica que ninguna capa lo halló.
fn resolve_binary() -> Result<Option<PathBuf>> {
    if let Some(bin) = validated_env_path("QWEN3_TTS_BIN", std::env::var_os("QWEN3_TTS_BIN"), true)?
    {
        return Ok(Some(bin));
    }
    if let Ok(dir) = avi_shared::paths::program_dir() {
        let cand = dir.join(if cfg!(windows) {
            "vendor/qwen3-tts/qwen_tts.exe"
        } else {
            "vendor/qwen3-tts/qwen_tts"
        });
        if cand.is_file() {
            return Ok(Some(cand));
        }
    }
    let vendored = PathBuf::from(if cfg!(windows) {
        "vendor/qwen3-tts/qwen_tts.exe"
    } else {
        "vendor/qwen3-tts/qwen_tts"
    });
    if vendored.is_file() {
        if cwd_vendor_allowed() {
            return Ok(Some(vendored));
        }
        eprintln!(
            "Aviso: se ignoró '{}' del directorio de trabajo: define {}=1 para habilitarlo.",
            vendored.display(),
            CWD_VENDOR_OPT_IN
        );
    }
    let name = RESIDENT_IMAGE_NAME;
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let cand = dir.join(name);
            if cand.is_file() {
                return Ok(Some(cand));
            }
        }
    }
    Ok(None)
}

/// Resolución por capas del directorio de un modelo del motor llamado
/// `dir_name`:
/// 1. la variable `env_var` (si está definida debe ser un directorio existente);
/// 2. directorio hermano del binario (`<dir del bin>/<dir_name>`);
/// 3. `<exe_dir>/vendor/qwen3-tts/<dir_name>`;
/// 4. snapshot no vacío en la raíz de modelos (`ModelStore::model_snapshot_path`);
/// 5. `<cwd>/vendor/qwen3-tts/<dir_name>` (solo con `AVI_ALLOW_CWD_VENDOR=1`;
///    si habría resuelto, avisa qué se ignoró y cómo habilitarlo).
///
/// `Ok(None)` indica que ninguna capa lo halló; un error de E/S al inspeccionar
/// el snapshot se propaga.
fn resolve_named_model_dir(
    env_var: &str,
    dir_name: &str,
    bin: Option<&Path>,
) -> Result<Option<PathBuf>> {
    if let Some(dir) = validated_env_path(env_var, std::env::var_os(env_var), false)? {
        return Ok(Some(dir));
    }
    if let Some(b) = bin {
        if let Some(parent) = b.parent() {
            let sibling = parent.join(dir_name);
            if sibling.is_dir() {
                return Ok(Some(sibling));
            }
        }
    }
    if let Ok(dir) = avi_shared::paths::program_dir() {
        let cand = dir.join("vendor/qwen3-tts").join(dir_name);
        if cand.is_dir() {
            return Ok(Some(cand));
        }
    }
    if let Some(p) = avi_store::ModelStore::new()?.model_snapshot_path(dir_name) {
        if p.is_dir() {
            let has_entries = p
                .read_dir()
                .and_then(|mut entries| entries.next().transpose())
                .map_err(|e| {
                    anyhow!(
                        "No se pudo inspeccionar el directorio del modelo '{}': {}",
                        p.display(),
                        e
                    )
                })?
                .is_some();
            if has_entries {
                return Ok(Some(p));
            }
        }
    }
    let vendored = PathBuf::from("vendor/qwen3-tts").join(dir_name);
    if vendored.is_dir() {
        if cwd_vendor_allowed() {
            return Ok(Some(vendored));
        }
        eprintln!(
            "Aviso: se ignoró '{}' del directorio de trabajo: define {}=1 para habilitarlo.",
            vendored.display(),
            CWD_VENDOR_OPT_IN
        );
    }
    Ok(None)
}

/// Directorio de pesos CustomVoice (presets), `qwen3-tts-0.6b`; la variable de
/// entorno es `QWEN3_TTS_MODEL_DIR`. Capas en [`resolve_named_model_dir`].
fn resolve_model_dir(bin: Option<&Path>) -> Result<Option<PathBuf>> {
    resolve_named_model_dir("QWEN3_TTS_MODEL_DIR", "qwen3-tts-0.6b", bin)
}

/// Directorio del modelo Base (`qwen3-tts-0.6b-base`), deliberadamente
/// separado de `resolve_model_dir`: lo usan el clonado (`--ref-audio`) y la
/// síntesis de voces clonadas, que exigen el modelo Base, distinto del
/// CustomVoice usado por los presets. La variable de entorno es
/// `QWEN3_TTS_BASE_MODEL_DIR`. Capas en [`resolve_named_model_dir`].
pub fn resolve_base_model_dir(bin: Option<&Path>) -> Result<Option<PathBuf>> {
    resolve_named_model_dir("QWEN3_TTS_BASE_MODEL_DIR", "qwen3-tts-0.6b-base", bin)
}

/// Falta el modelo de pesos que exige el tipo de voz a sintetizar (CustomVoice
/// para un preset, Base para una voz clonada). Los llamadores lo distinguen de
/// otros fallos de síntesis para reportarlo como `model_missing`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelMissing {
    /// Mensaje en español que indica qué modelo falta.
    pub message: &'static str,
}

impl std::fmt::Display for ModelMissing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}

impl std::error::Error for ModelMissing {}

/// Directorio de pesos con el que se arranca el residente de `voice`: el
/// modelo Base para una voz clonada y CustomVoice para un preset. Si falta el
/// modelo que exige el tipo de voz es un error explícito, sin caer al otro.
/// Es la única fuente de la regla «clonada exige Base, preset exige CustomVoice».
fn select_model_dir<'a>(
    voice: &VoiceEngine,
    model_dir: Option<&'a Path>,
    base_model_dir: Option<&'a Path>,
) -> std::result::Result<&'a Path, ModelMissing> {
    match voice {
        VoiceEngine::Preset(_) => model_dir.ok_or(ModelMissing {
            message: "El modelo de síntesis Qwen3-TTS no está provisionado.",
        }),
        VoiceEngine::Cloned(_) => base_model_dir.ok_or(ModelMissing {
            message: "El modelo Base de Qwen3-TTS, necesario para sintetizar voces clonadas, no está provisionado. Ejecuta 'setup' primero.",
        }),
    }
}

/// Motor Qwen3-TTS con servidor HTTP residente gestionado por el host como
/// único camino de síntesis (el healthcheck está acotado; el POST de síntesis
/// se acota con el presupuesto proporcional al texto).
pub struct Qwen3TtsEngine {
    pub binary_path: Option<PathBuf>,
    pub model_dir: Option<PathBuf>,
    pub base_model_dir: Option<PathBuf>,
    resident: Mutex<Option<ResidentState>>,
    /// PID e identidad del proceso `qwen_tts` arrancado (`None` si no hay),
    /// observados al nacer. `shutdown` los usa para matar solo al proceso que
    /// sigue siendo el registrado, SIN tomar el `Mutex<resident>` que el hilo
    /// `spawn_blocking(warmup)` retiene durante el spawn + `wait_health`.
    live: LiveResident,
    /// Registro externo del residente: el daemon pasa el suyo y la ruta directa
    /// de la CLI ninguno, así que el motor nunca escribe `daemon.pid`.
    registry: Option<Arc<dyn ResidentRegistry>>,
    /// Activo tras [`close`](Self::close): ningún residente nuevo llega a quedar
    /// registrado, aunque un hilo de warmup lo estuviera lanzando.
    closed: AtomicBool,
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

/// Plazo con el que `shutdown` espera la muerte del residente tras matarlo.
const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Plazo con el que `clone_voice` espera al subproceso de clonado: la carga
/// del modelo y la inferencia en CPU pueden tardar minutos, así que 10 min es
/// cota holgada, no ajuste fino. Al vencer, el hijo se mata y el clonado falla
/// sin colgar al llamador.
const CLONE_WAIT: Duration = Duration::from_secs(600);

/// Estado del servidor residente: se indexa por voz — al cambiar
/// de voz se termina el residente anterior y se arranca otro con `--load-voice`.
struct ResidentState {
    resident: resident::Qwen3TtsResident,
    voice_key: String,
}

impl Qwen3TtsEngine {
    /// Motor con `registry` opcional: el daemon pasa el suyo para que el
    /// residente quede registrado desde que nace; la ruta directa pasa `None` y
    /// nunca toca el registro. Falla si `QWEN3_TTS_BIN`, `QWEN3_TTS_MODEL_DIR`
    /// o `QWEN3_TTS_BASE_MODEL_DIR` están definidas y no apuntan a una ruta
    /// existente, o si no se puede inspeccionar un directorio de modelo.
    pub fn new(registry: Option<Arc<dyn ResidentRegistry>>) -> Result<Self> {
        let binary_path = resolve_binary()?;
        let model_dir = resolve_model_dir(binary_path.as_deref())?;
        let base_model_dir = resolve_base_model_dir(binary_path.as_deref())?;
        Ok(Self {
            binary_path,
            model_dir,
            base_model_dir,
            resident: Mutex::new(None),
            live: Mutex::new(None),
            registry,
            closed: AtomicBool::new(false),
        })
    }

    /// Vista del registro y del estado vivo que recibe el arranque del residente.
    fn registration(&self) -> resident::Registration<'_> {
        resident::Registration {
            registry: self.registry.as_deref(),
            live: &self.live,
            closed: &self.closed,
        }
    }

    /// Cierre definitivo del motor: impide que un residente lanzado desde ahora
    /// quede registrado y detiene el vigente como [`shutdown`](Self::shutdown).
    /// Lo llama el daemon al terminar de servir, para que ni un warmup en curso
    /// deje un residente vivo tras el cierre o el reinicio en proceso.
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        self.shutdown();
    }

    /// Detén el residente HTTP gestionado y espera su muerte, con plazo. Se
    /// llama desde `shutdown_handler` (avi-daemon) antes de notificar el
    /// graceful shutdown y al terminar el servidor.
    ///
    /// Matar al residente hace fallar la síntesis en curso (el residente es el
    /// único camino, sin fallback), así que el hilo `spawn_blocking(warmup)`
    /// retorna y el runtime cierra el proceso limpio.
    ///
    /// No depende del `Mutex<resident>`: el hilo del warmup lo retiene durante el
    /// spawn + `wait_health`, así que `self.resident.lock()` se colgaría. Se mata
    /// el árbol por PID solo si la identidad observada ahora coincide con la
    /// registrada al nacer: un PID reasignado a otro programa no se toca. Tras
    /// matar se espera, con sondeo acotado, a que el proceso muera; en cada
    /// vuelta se intenta liberar el estado del residente con `try_lock`, lo que
    /// recolecta al hijo (en Unix un hijo sin recolectar sigue pareciendo vivo).
    /// El registro externo se anula solo cuando el proceso ya no existe; si
    /// sigue vivo tras el plazo se conserva anotado y el aviso es ruidoso.
    pub fn shutdown(&self) {
        let registered = lock_live(&self.live).take();
        if let Some((pid, identity)) = registered {
            let state = avi_process::registry::classify_registered(
                pid,
                avi_process::pid_alive_exact(pid),
                Some(&identity),
                || avi_process::process_identity(pid),
            );
            if state == avi_process::registry::Registered::Ours {
                crate::resident::kill_tree_resident_by_pid(pid);
                if !self.wait_resident_gone(pid) {
                    eprintln!(
                        "Aviso: el residente (pid {pid}) sigue vivo tras el plazo de cierre; se conserva anotado"
                    );
                    *lock_live(&self.live) = Some((pid, identity));
                    return;
                }
            }
            if let Some(registry) = &self.registry {
                registry.unregister(pid);
            }
        }
        if let Ok(mut guard) = self.resident.try_lock() {
            *guard = None;
        }
    }

    /// Espera con sondeo acotado a que `pid` muera, recolectando el estado del
    /// residente en cada vuelta. Devuelve si el proceso ya no existe.
    fn wait_resident_gone(&self, pid: u32) -> bool {
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        loop {
            if let Ok(mut guard) = self.resident.try_lock() {
                *guard = None;
            }
            if !avi_process::pid_alive_exact(pid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Arranca un residente fresco para `voice`: construye `load_voice`, reserva
    /// un puerto efímero propio y hace `spawn`, que lanza el proceso, observa su
    /// identidad, lo registra y solo entonces espera su salud (nace sano o falla
    /// con diagnóstico, sin dejar el hijo vivo ni el registro anotado). Ensambla
    /// el `ResidentState`. Compartido por el camino de cambio de voz y por el
    /// rearranque ante degradación del residente.
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
        let port = reserve_loopback_port()?;
        let spawned =
            resident::Qwen3TtsResident::spawn(model_dir, port, load_voice, &self.registration())?;
        Ok(ResidentState {
            resident: spawned,
            voice_key,
        })
    }

    /// Síntesis vía servidor residente: arranca (o reutiliza) el residente de
    /// la voz solicitada y hace `POST /v1/tts`.
    ///
    /// Antes de reutilizar un residente se verifica su salud real
    /// (`health_check`, `try_wait` + `GET /v1/health`): un residente colgado
    /// seguiría aceptando la conexión y dejaría el `POST` esperando. Si está
    /// degradado se registra el diagnóstico en stderr y se rearranca de forma
    /// determinista (mata el árbol por PID, suelta el estado viejo, `spawn`
    /// fresco). Sin fallback: si el `spawn` falla, el error se propaga y la
    /// petición falla con diagnóstico.
    ///
    /// El `guard` cubre solo la decisión reutilizar/arrancar, la salud
    /// observada, el eventual rearranque y la lectura del puerto (`u16`
    /// copiable); se suelta antes del `POST /v1/tts`, que corre sin lock. La
    /// serialización del uso del residente la da la capa superior
    /// (`synthesis_lock` en el daemon, secuencialidad en el CLI directo), así
    /// que la siguiente petición reclama el residente de inmediato en vez de
    /// esperar tras un hilo huérfano reteniendo el lock. El plazo total del
    /// POST es el presupuesto proporcional a la longitud del texto
    /// (`avi_core::synthesis_budget`); si vence, el error envuelve
    /// `SynthesisTimeout`. Al vencer o al activarse `cancel` (que produce
    /// `SynthesisCancelled`) se cierra la conexión y el motor aborta la
    /// generación, así que el residente sigue caliente y el `health_check` de
    /// la siguiente petición lo reutiliza.
    fn synthesize_via_resident(
        &self,
        text: &str,
        voice: &VoiceEngine,
        options: &GenerationOptions,
        out_path: &Path,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let model_dir = select_model_dir(
            voice,
            self.model_dir.as_deref(),
            self.base_model_dir.as_deref(),
        )?;
        let voice_key = match voice {
            VoiceEngine::Preset(n) => format!("preset:{}", n),
            VoiceEngine::Cloned(p) => format!("clone:{}", p.display()),
        };
        let url = {
            // Un hilo que murió con el mutex del residente tomado lo deja
            // poisoned; un `unwrap()` convertiría eso en un pánico que tumba
            // el proceso. Lo que envenenó el mutex fue ese hilo, no el estado
            // del residente, así que se registra la traza y la decisión
            // reutilizar/arrancar se sigue tomando sobre el guard recuperado.
            let mut guard = self.resident.lock().unwrap_or_else(|poisoned| {
                eprintln!(
                    "Aviso: mutex del residente poisoned; se sigue sobre el estado recuperado"
                );
                poisoned.into_inner()
            });
            if guard.as_ref().map(|s| s.voice_key.as_str()) != Some(voice_key.as_str()) {
                // El residente anterior se termina antes de lanzar el nuevo: el
                // registro solo anota uno y ninguno puede vivir sin anotar.
                *guard = None;
                *guard = Some(self.start_resident(model_dir, voice, voice_key)?);
            } else if let Err(health_error) = guard
                .as_mut()
                .expect("reutilización verificada arriba")
                .resident
                .health_check(HEALTH_OBS_RETRIES, HEALTH_OBS_INTERVAL_MS)
            {
                // Residente degradado (crash o hang): rearranque determinista.
                eprintln!(
                    "Aviso: el residente de síntesis está degradado ({health_error}); se rearranca"
                );
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
        let body = build_tts_body(text, voice, options, None, None)?.to_string();
        let (status, bytes) = http_exchange(
            &format!("{}/v1/tts", url),
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
                "El residente Qwen3-TTS devolvió código de error: {}",
                status
            ))
        }
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

        // La voz clonada se resuelve por `reference.qvoice` (`qvoice_path`); sin
        // él solo `ryan` y `vivian` son presets, y cualquier otra voz es
        // `VoiceNotFound`.
        let qvoice = profile
            .qvoice_path
            .as_deref()
            .filter(|q| q.is_file())
            .map(|q| q.to_path_buf());
        let voice = resolve_voice_engine(&profile.name, qvoice.as_deref())?;

        // Servidor residente gestionado por el host, con healthcheck y POST
        // acotados (este último por el presupuesto de síntesis del texto). El
        // texto viaja por body HTTP JSON, ruta segura para UTF-8 acentuado (a
        // diferencia del argv de un subprocess en Windows).
        self.synthesize_via_resident(text, &voice, options, &path, cancel)?;
        Ok(path)
    }
}

/// Construye el body HTTP de `POST /v1/tts`: sin `format` (el
/// servidor lo ignora) y claves solo-si-`Some`. `language` lleva siempre el
/// nombre del idioma que entiende el motor, tanto con preset como con voz
/// clonada; `speaker` solo se emite con preset. Un idioma desconocido es error.
pub(crate) fn build_tts_body(
    text: &str,
    voice: &VoiceEngine,
    options: &GenerationOptions,
    prosody: Option<&ProsodyOptions>,
    emotion: Option<&EmotionOptions>,
) -> Result<serde_json::Value> {
    let language = engine_language_name(&options.language)?;
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
        }
        VoiceEngine::Cloned(_) => {}
    }
    obj.insert(
        "language".to_string(),
        serde_json::Value::String(language.to_string()),
    );
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
    Ok(serde_json::Value::Object(obj))
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
/// único con dueño y devolviendo su ruta: el fichero se borra al soltar el valor
/// devuelto, por cualquier camino de salida. El motor rechaza referencias que no sean 24 kHz;
/// el benchmark preprocesaba la referencia de la misma forma. El fallo de carga se
/// propaga como `avi_audio::WavLoadError` dentro del `anyhow::Error`, de modo que
/// quien llama lo recupera con `downcast_ref`.
fn reference_24k_mono(ref_audio: &Path) -> Result<tempfile::TempPath> {
    let pcm = avi_audio::load_wav_24k_mono_pcm(ref_audio)?;
    // `into_temp_path` cierra el manejador abierto: así el escritor y el motor
    // hijo abren el fichero por ruta sin choque de uso compartido en Windows.
    // El PID en el nombre permite al barrido del producto reconocer al dueño vivo.
    let out_path = tempfile::Builder::new()
        .prefix(&format!("avi_tts_ref24k_{}_", std::process::id()))
        .suffix(".wav")
        .tempfile()?
        .into_temp_path();
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

/// Construye el argv del clonado, sin I/O real: programa más argumentos
/// `-d <model_dir> --ref-audio <ref> --xvector-only --save-voice <out>
/// --voice-name <name> --watch-stdin`. `--xvector-only` guarda solo la huella
/// de timbre (x-vector): sin transcripción ni códigos ICL. No lleva idioma: la
/// huella sale del audio y cada petición de síntesis fija el suyo.
fn build_clone_command(
    bin: &Path,
    model_dir: &Path,
    ref_wav: &Path,
    out_qvoice: &Path,
    name: &str,
) -> (PathBuf, Vec<String>) {
    let args = vec![
        String::from("-d"),
        model_dir.to_string_lossy().into_owned(),
        String::from("--ref-audio"),
        ref_wav.to_string_lossy().into_owned(),
        String::from("--xvector-only"),
        String::from("--save-voice"),
        out_qvoice.to_string_lossy().into_owned(),
        String::from("--voice-name"),
        name.to_string(),
        String::from("--watch-stdin"),
    ];
    (bin.to_path_buf(), args)
}

/// Clona una voz desde `ref_audio` (WAV de cualquier tasa/canales) a `out_qvoice`
/// (`.qvoice` con solo la huella de timbre, x-vector, que se sintetiza con el
/// modelo Base y `--graft`) vía subprocess: `<bin> -d <model_dir> --ref-audio
/// <ref> --xvector-only --save-voice <out> --voice-name <name> --watch-stdin`.
/// La referencia se normaliza antes a 24 kHz mono (requisito del motor). Propaga
/// el error con el exit code del proceso. Si la referencia no se puede cargar
/// (WAV inválido, truncado o fallo de lectura), el error conserva el
/// `avi_audio::WavLoadError` recuperable con `downcast_ref`.
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
) -> Result<()> {
    // La referencia se carga antes de buscar el binario: un audio inválido se
    // rechaza sin depender de que el motor esté provisionado.
    let ref_wav = reference_24k_mono(ref_audio)?;
    let Some(bin) = resolve_binary()? else {
        return Err(anyhow!(
            "El binario de clonado Qwen3-TTS no está provisionado."
        ));
    };
    let (log_path, log_file) = avi_shared::logs::create_log(
        &avi_shared::paths::logs_dir()
            .map_err(|e| anyhow!("No se pudo resolver el directorio de logs: {}", e))?,
        ENGINE_LOG_FAMILY,
        &avi_process::pid_alive_exact,
    )
    .map_err(|e| anyhow!("No se pudo crear el log del motor: {}", e))?;
    // La entrada del motor es una tubería con `--watch-stdin`: el motor termina
    // si su entrada se cierra, así que el proceso que clona muere con él aunque
    // sea abatido. El extremo de escritura queda retenido en el hijo
    // restringido hasta que la espera devuelve; si no, el motor vería fin de
    // fichero al instante. La herencia queda restringida a la lista explícita
    // (tubería de entrada y fichero de registro).
    let status = (|| -> Result<std::process::ExitStatus> {
        let (program, args) =
            build_clone_command(&bin, model_dir.as_ref(), &ref_wav, out_qvoice, name);
        #[cfg(windows)]
        let creation_flags = avi_process::DETACHED_PROCESS;
        #[cfg(not(windows))]
        let creation_flags = 0u32;
        let request = avi_process::RestrictedSpawnRequest {
            program,
            args,
            stdin: avi_process::StdinSpec::Piped,
            stderr_file: log_file.try_clone()?,
            stdout_file: log_file,
            creation_flags,
            extra_allowed: Vec::new(),
        };
        let mut child = avi_process::spawn_with_allowlist(request)?;
        match child.wait_timeout(CLONE_WAIT)? {
            Some(status) => Ok(status),
            None => Err(anyhow!(
                "El subproceso de clonado Qwen3-TTS no terminó en {} s y se terminó.",
                CLONE_WAIT.as_secs()
            )),
        }
    })();
    // La referencia normalizada se borra al soltar `ref_wav`, tras esperar al hijo.
    drop(ref_wav);
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
/// `--serve <puerto> --host 127.0.0.1 -j 4 --stream --watch-stdin [--load-voice <qvoice> --graft]`,
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

    /// Registro del residente que se está lanzando: el registro externo, si lo
    /// hay, y el estado vivo del motor. Se anotan juntos al nacer el proceso y
    /// se anulan juntos si el arranque falla.
    pub(crate) struct Registration<'a> {
        pub(crate) registry: Option<&'a dyn crate::ResidentRegistry>,
        pub(crate) live: &'a crate::LiveResident,
        pub(crate) closed: &'a AtomicBool,
    }

    impl Registration<'_> {
        /// Anota al residente en el estado vivo y, si hay registro externo, en él.
        /// Con el motor cerrado falla: se anota primero y se comprueba después,
        /// de modo que un cierre concurrente o ve al residente anotado y lo mata,
        /// o lo ve cerrado aquí y el arranque lo termina.
        fn record(
            &self,
            pid: u32,
            identity: &avi_process::ProcessIdentity,
            port: u16,
        ) -> Result<()> {
            *crate::lock_live(self.live) = Some((pid, identity.clone()));
            if self.closed.load(Ordering::SeqCst) {
                return Err(anyhow!("el motor está cerrado"));
            }
            if let Some(registry) = self.registry {
                registry.register(pid, identity, port)?;
            }
            Ok(())
        }

        /// Anula el estado vivo (solo si es de `pid`) y el registro externo.
        fn clear(&self, pid: u32) {
            let mut live = crate::lock_live(self.live);
            if live.as_ref().is_some_and(|(live_pid, _)| *live_pid == pid) {
                *live = None;
            }
            drop(live);
            if let Some(registry) = self.registry {
                registry.unregister(pid);
            }
        }
    }

    /// Gestor del proceso servidor del motor.
    pub struct Qwen3TtsResident {
        child: Option<avi_process::RestrictedChild>,
        pub port: u16,
        /// Ruta del fichero de log de stderr del motor (incluida en errores de healthcheck).
        pub(crate) log_path: PathBuf,
    }

    /// Construye el argv de arranque del residente, sin I/O real:
    /// programa más argumentos `-d <model_dir> --serve <port> --host 127.0.0.1
    /// -j 4 --stream --watch-stdin [--load-voice <qvoice> --graft]`. El llamador
    /// pasa en `model_dir` el modelo Base si hay `load_voice` y CustomVoice si no.
    /// `--graft` clona por el camino solo x-vector (el motor ignora la
    /// transcripción y los códigos de referencia del `.qvoice`) y no aplica
    /// los pesos diferenciales de la voz.
    /// `--watch-stdin` hace que el motor termine al cerrarse su entrada estándar.
    /// No lleva `--int4`: en el modelo 0.6B cuantiza el code predictor y el
    /// residente deja de ser determinista, con salidas truncadas.
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
            String::from("-j"),
            String::from("4"),
            String::from("--stream"),
            String::from("--watch-stdin"),
        ];
        if let Some(lv) = load_voice {
            args.push(String::from("--load-voice"));
            args.push(lv.to_string_lossy().into_owned());
            args.push(String::from("--graft"));
        }
        (bin.to_path_buf(), args)
    }

    impl Qwen3TtsResident {
        /// Arranca el motor con `--serve` en `port`, lo registra desde que nace y
        /// espera a que `/v1/health` responda (hasta 60 × 500 ms). Con
        /// `load_voice` (voz clonada) añade `--load-voice <qvoice> --graft` (el
        /// clonado solo aplica al arranque); `model_dir` debe ser entonces el
        /// modelo Base.
        pub(crate) fn spawn(
            model_dir: impl AsRef<Path>,
            port: u16,
            load_voice: Option<&Path>,
            registration: &Registration<'_>,
        ) -> Result<Self> {
            let bin = resolve_binary()?
                .ok_or_else(|| anyhow!("El binario Qwen3-TTS no está provisionado."))?;
            // El stderr del motor va a un log de la familia `qwen3-tts`, creado y
            // podado por el auxiliar común (los 10 más recientes). Captura los ~20
            // `fprintf(stderr, *)` del motor C (cuyos mensajes se perdían a null,
            // dejando ciego el diagnóstico del warmup, la síntesis por petición y
            // la terminación del residente).
            let (log_path, log_file) = avi_shared::logs::create_log(
                &avi_shared::paths::logs_dir()
                    .map_err(|e| anyhow!("No se pudo resolver el directorio de logs: {}", e))?,
                crate::ENGINE_LOG_FAMILY,
                &avi_process::pid_alive_exact,
            )
            .map_err(|e| anyhow!("No se pudo crear el log del motor: {}", e))?;
            let child =
                launch_resident_process(&bin, model_dir.as_ref(), port, load_voice, log_file)?;
            Self::spawn_with_child(child, port, log_path, 60, 500, registration)
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
            // stdout va al mismo fichero de registro que el stderr.
            // Windows: `qwen_tts.exe` NO debe heredar handles del padre salvo
            // los declarados. `DETACHED_PROCESS` lo deja sin consola:
            // `qwen_tts` no lanza procesos de consola, así que no necesita
            // una propia, y va sin grupo propio para que `taskkill /T` lo
            // alcance. La herencia queda restringida a la lista explícita
            // (tubería de entrada y fichero de registro): el pipe del abuelo
            // no viaja al motor por construcción.
            // Sin `CREATE_NEW_PROCESS_GROUP` ni breakaway, para que
            // `taskkill /F /T /PID <daemon>` alcance al residente como descendiente.
            // En Unix `spawn_with_allowlist` lo hace líder de su propia sesión
            // (`setsid`): su grupo solo contiene su propio árbol y el daemon no
            // comparte grupo con él.
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
                stderr_file: log_file.try_clone()?,
                stdout_file: log_file,
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
        /// Registra a un hijo ya lanzado y arranca su healthcheck (retries/intervalo
        /// configurables para los tests de reintentos). El orden es: observar la
        /// identidad, anotarlo en `registration` y solo entonces esperar la salud,
        /// de modo que está registrado durante toda la espera. Una identidad no
        /// observable, un registro fallido o una salud fallida terminan el hijo,
        /// lo recogen y anulan el registro. `log_path` se guarda en el struct para
        /// incluirse en errores de `wait_health`.
        pub(crate) fn spawn_with_child(
            child: avi_process::RestrictedChild,
            port: u16,
            log_path: PathBuf,
            retries: usize,
            interval_ms: u64,
            registration: &Registration<'_>,
        ) -> Result<Self> {
            let mut child = child;
            let pid = child.id();
            let started = register_launched(pid, port, registration).and_then(|()| {
                wait_health(&mut child, port, retries, interval_ms, log_path.as_path())
            });
            if let Err(e) = started {
                // Un motor que no llegó a estar sano se termina y se recoge aquí:
                // soltar el `Child` sin `wait` dejaría un zombi en Unix, que
                // seguiría pareciendo vivo para una comprobación por PID.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "el motor puede haber muerto ya; la recolección siguiente cubre ambos casos"
                )]
                let _ = child.kill();
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "el desenlace ya es el error del arranque; la espera solo recolecta"
                )]
                let _ = child.wait();
                registration.clear(pid);
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
                // árbol preciso por PID; aquí la espera es acotada y en mejor
                // esfuerzo (nunca `INFINITE`): si el hijo no muere en el plazo,
                // `wait_timeout` lo mata y lo recolecta. Insuficiente ante caída
                // a medio `spawn` (sin `Child` que recolectar): por eso
                // `run_supervised` reclama además el árbol propio previo con
                // deadline y verificación antes del siguiente `bind`.
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "en `drop` no hay a quién devolver el error; la espera acotada mata y recolecta"
                )]
                let _ = child.wait_timeout(super::SHUTDOWN_WAIT);
            }
        }
    }

    /// Observa la identidad del hijo recién lanzado y lo anota. Sin identidad no
    /// hay registro posible y el arranque falla: la parada no podría distinguir
    /// al residente de un PID reasignado.
    fn register_launched(pid: u32, port: u16, registration: &Registration<'_>) -> Result<()> {
        let identity = avi_process::process_identity(pid).ok_or_else(|| {
            anyhow!(
                "No se pudo observar la identidad del residente Qwen3-TTS (pid {}): sin ella no se registra",
                pid
            )
        })?;
        registration.record(pid, &identity, port).map_err(|e| {
            anyhow!(
                "No se pudo registrar el residente Qwen3-TTS (pid {}): {}",
                pid,
                e
            )
        })
    }

    /// Viveza real de un PID a nivel de sistema (sin Mutex ni HTTP).
    /// `pub` para el camino de reclamo/parada del CLI: muerte por PID
    /// registrado + verificación de que ese PID quedó muerto; sin PID no hay
    /// residente que reclamar.
    ///
    /// Delega en `avi_process::pid_alive_exact`: `OpenProcess` +
    /// `GetExitCodeProcess` en Windows y `kill(pid, 0)` en Unix. Así el
    /// residente se verifica con la misma primitiva exacta que el daemon
    /// (con la guarda de `pid_t`) y sin lanzar `tasklist`, cuya comparación por
    /// texto hacía que el PID 12 matcheara dentro del 112.
    pub fn resident_pid_alive(pid: u32) -> bool {
        avi_process::pid_alive_exact(pid)
    }

    /// Termina el residente por su PID (sin Mutex). Delega en
    /// `avi_process::kill_tree_by_pid_exact`: `taskkill /F /T /PID` en Windows
    /// (árbol por parentesco real) y `SIGKILL` al grupo y al PID en Unix, sin
    /// lanzar el ejecutable `kill` ni quedar fuera de la guarda de `pid_t`.
    ///
    /// En Unix el kill al grupo no alcanza nunca al daemon: el residente nace
    /// líder de su propia sesión por el `setsid` de `spawn_with_allowlist`, así
    /// que su grupo solo contiene su propio árbol. No verifica: el llamante
    /// combina con `resident_pid_alive` o recolecta el estado vía `Child`.
    pub fn kill_tree_resident_by_pid(pid: u32) -> bool {
        avi_process::kill_tree_by_pid_exact(pid)
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
    /// real `qwen_tts`, así que queda ciego al reclamo por PID del
    /// residente huérfano. La ausencia real de huérfanos a nivel SO —verificada
    /// por `resident_pid` muerto— solo la comprueba la
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
    use std::process::{Command, Stdio};
    use std::sync::Arc;
    use std::thread;
    use std::time::Duration;

    /// Registro doble: anota las llamadas y puede fallar al registrar.
    #[derive(Default)]
    struct RecordingRegistry {
        fail_register: bool,
        registered: Mutex<Vec<(u32, avi_process::ProcessIdentity, u16)>>,
        unregistered: Mutex<Vec<u32>>,
    }

    impl RecordingRegistry {
        fn registered_pids(&self) -> Vec<u32> {
            self.registered
                .lock()
                .unwrap()
                .iter()
                .map(|(pid, _, _)| *pid)
                .collect()
        }
    }

    impl ResidentRegistry for RecordingRegistry {
        fn register(
            &self,
            pid: u32,
            identity: &avi_process::ProcessIdentity,
            port: u16,
        ) -> Result<()> {
            if self.fail_register {
                return Err(anyhow!("registro doble que falla"));
            }
            self.registered
                .lock()
                .unwrap()
                .push((pid, identity.clone(), port));
            Ok(())
        }

        fn unregister(&self, pid: u32) {
            self.unregistered.lock().unwrap().push(pid);
        }
    }

    /// Espera con sondeo acotado (hasta 20 s) a que `condition` se cumpla.
    fn wait_until(what: &str, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while Instant::now() < deadline {
            if condition() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("no se cumplió a tiempo: {what}");
    }

    /// Registro nulo para los arranques que no prueban el registro.
    fn bare(live: &LiveResident) -> resident::Registration<'_> {
        static OPEN: AtomicBool = AtomicBool::new(false);
        resident::Registration {
            registry: None,
            live,
            closed: &OPEN,
        }
    }

    /// Tras `close`, un residente que se lanza después no llega a quedar
    /// registrado: el arranque lo mata y devuelve error.
    #[test]
    fn closed_engine_rejects_a_late_resident() {
        let registry = Arc::new(RecordingRegistry::default());
        let engine = Qwen3TtsEngine::new(Some(registry.clone())).expect("motor de prueba");
        engine.close();
        let child = sleeping_process();
        let pid = child.id();

        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            reserve_loopback_port().expect("puerto efímero"),
            test_log_path(),
            600,
            50,
            &engine.registration(),
        );

        assert!(result.is_err(), "un motor cerrado no acepta residentes");
        assert!(!avi_process::pid_alive_exact(pid));
        assert!(registry.registered_pids().is_empty());
        assert!(lock_live(&engine.live).is_none());
    }

    /// El residente queda registrado antes de que venza la salud: con un motor
    /// que nunca responde, el registro ya contiene su PID mientras `wait_health`
    /// sigue esperando, y `shutdown` lo mata, espera su muerte y anula el registro.
    #[test]
    fn resident_is_registered_before_health_and_shutdown_kills_it() {
        let registry = Arc::new(RecordingRegistry::default());
        let engine = Qwen3TtsEngine::new(Some(registry.clone())).expect("motor de prueba");
        let child = sleeping_process();
        let pid = child.id();
        let port = reserve_loopback_port().expect("puerto efímero");

        let result = thread::scope(|scope| {
            let spawn = scope.spawn(|| {
                resident::Qwen3TtsResident::spawn_with_child(
                    child,
                    port,
                    test_log_path(),
                    600,
                    50,
                    &engine.registration(),
                )
            });
            wait_until("el registro contiene el PID del residente", || {
                registry.registered_pids().contains(&pid)
            });
            assert!(
                !spawn.is_finished(),
                "el registro debe preceder al vencimiento de la salud"
            );
            let (_, identity, registered_port) = registry.registered.lock().unwrap()[0].clone();
            assert_eq!(Some(identity), avi_process::process_identity(pid));
            assert_eq!(registered_port, port);
            engine.shutdown();
            spawn
                .join()
                .expect("el hilo de arranque no debe entrar en pánico")
        });

        assert!(result.is_err(), "un motor muerto no llega a estar sano");
        assert!(
            !avi_process::pid_alive_exact(pid),
            "shutdown debe matar al residente"
        );
        assert!(registry.unregistered.lock().unwrap().contains(&pid));
        assert!(lock_live(&engine.live).is_none());
    }

    /// Si el registro falla, el arranque devuelve error y deja muerto al
    /// residente lanzado, sin estado vivo ni registro anotado.
    #[test]
    fn failed_registration_kills_the_launched_resident() {
        let registry = Arc::new(RecordingRegistry {
            fail_register: true,
            ..Default::default()
        });
        let engine = Qwen3TtsEngine::new(Some(registry.clone())).expect("motor de prueba");
        let child = sleeping_process();
        let pid = child.id();

        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            reserve_loopback_port().expect("puerto efímero"),
            test_log_path(),
            600,
            50,
            &engine.registration(),
        );

        let err = result
            .err()
            .expect("un registro fallido debe abortar el arranque");
        assert!(err.to_string().contains("registrar"), "{err}");
        assert!(
            !avi_process::pid_alive_exact(pid),
            "el residente debe haber muerto"
        );
        assert!(lock_live(&engine.live).is_none());
        assert!(registry.registered_pids().is_empty());
        assert!(registry.unregistered.lock().unwrap().contains(&pid));
    }

    /// Un motor sin registro (ruta directa) nunca toca `daemon.pid`: con uno
    /// previo en la raíz de datos, sus bytes y el contenido del directorio
    /// quedan intactos tras un arranque que falla y otro que triunfa.
    #[test]
    fn engine_without_registry_never_touches_daemon_pid() {
        let data = tempfile::TempDir::new().unwrap();
        let pidfile = data.path().join("daemon.pid");
        let previous = br#"{"pid": 4242, "contenido": "previo"}"#;
        std::fs::write(&pidfile, previous).unwrap();
        let original = std::env::var_os("AVI_DATA_DIR");
        std::env::set_var("AVI_DATA_DIR", data.path());

        let engine = Qwen3TtsEngine::new(None).expect("motor de prueba");
        let live = Mutex::new(None);
        let failed = resident::Qwen3TtsResident::spawn_with_child(
            sleeping_process(),
            1,
            test_log_path(),
            2,
            30,
            &engine.registration(),
        );
        let (port, server) = resident::simulate_server(Arc::new(Mutex::new(String::new())));
        let healthy = resident::Qwen3TtsResident::spawn_with_child(
            sleeping_process(),
            port,
            test_log_path(),
            10,
            50,
            &bare(&live),
        );
        drop(server);

        match original {
            Some(value) => std::env::set_var("AVI_DATA_DIR", value),
            None => std::env::remove_var("AVI_DATA_DIR"),
        }
        assert!(failed.is_err());
        assert!(healthy.is_ok());
        assert_eq!(std::fs::read(&pidfile).unwrap(), previous);
        assert_eq!(std::fs::read_dir(data.path()).unwrap().count(), 1);
    }

    /// `shutdown` no mata a un proceso cuya identidad observada no coincide con
    /// la registrada: es un PID reasignado a otro programa.
    #[test]
    fn shutdown_spares_a_process_with_a_different_identity() {
        let engine = Qwen3TtsEngine::new(None).expect("motor de prueba");
        let mut child = sleeping_process();
        let pid = child.id();
        let mut other = avi_process::process_identity(pid).expect("identidad del señuelo");
        other.start.push_str("-otra");
        *lock_live(&engine.live) = Some((pid, other));

        engine.shutdown();

        let alive = child.try_wait().expect("consulta al señuelo").is_none();
        let _ = child.kill();
        let _ = child.wait();
        assert!(alive, "un proceso con otra identidad no debe morir");
        assert!(lock_live(&engine.live).is_none());
    }

    /// Dos motores anotan cada uno su propio puerto y la reserva devuelve un
    /// puerto de loopback utilizable. Los puertos de los residentes simulados son
    /// distintos porque ambos servidores siguen enlazados a la vez.
    #[test]
    fn each_engine_registers_its_own_port() {
        let first_registry = Arc::new(RecordingRegistry::default());
        let second_registry = Arc::new(RecordingRegistry::default());
        let first =
            Qwen3TtsEngine::new(Some(first_registry.clone())).expect("primer motor de prueba");
        let second =
            Qwen3TtsEngine::new(Some(second_registry.clone())).expect("segundo motor de prueba");
        let (first_port, first_server) =
            resident::simulate_server(Arc::new(Mutex::new(String::new())));
        let (second_port, second_server) =
            resident::simulate_server(Arc::new(Mutex::new(String::new())));
        assert_ne!(first_port, second_port);

        let first_resident = resident::Qwen3TtsResident::spawn_with_child(
            sleeping_process(),
            first_port,
            test_log_path(),
            10,
            50,
            &first.registration(),
        )
        .expect("el primer residente debe estar sano");
        let second_resident = resident::Qwen3TtsResident::spawn_with_child(
            sleeping_process(),
            second_port,
            test_log_path(),
            10,
            50,
            &second.registration(),
        )
        .expect("el segundo residente debe estar sano");

        let first_noted = first_registry.registered.lock().unwrap()[0].clone();
        let second_noted = second_registry.registered.lock().unwrap()[0].clone();
        assert_eq!(
            (first_noted.0, first_noted.2),
            (first_resident.pid(), first_port)
        );
        assert_eq!(
            (second_noted.0, second_noted.2),
            (second_resident.pid(), second_port)
        );
        drop((first_server, second_server));

        let reserved = reserve_loopback_port().expect("puerto efímero");
        assert_ne!(reserved, 0);
        TcpListener::bind((RESIDENT_HOST, reserved)).expect("el puerto reservado debe estar libre");
    }

    /// `GenerationOptions::new` debe coincidir con los defaults de muestreo del
    /// motor; no afirma los valores de `GenerationOptions::production`.
    #[test]
    fn new_generation_options_matches_engine_defaults() {
        let d = GenerationOptions::new("en");
        assert_eq!(d.temperature, 0.5);
        assert_eq!(d.top_k, 50);
        assert_eq!(d.top_p, 1.0);
        assert_eq!(d.rep_penalty, 1.05);
        assert_eq!(d.language, "en");
        assert_eq!(d.seed, None);
    }

    /// `production()` fija temperatura y seed a la config validada por oído,
    /// sin alterar el resto de campos respecto a `new`, y conserva el idioma.
    #[test]
    fn generation_options_production_sets_temperature_and_seed() {
        let p = GenerationOptions::production("es");
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        assert_eq!(p.top_k, DEFAULT_TOP_K);
        assert_eq!(p.top_p, DEFAULT_TOP_P);
        assert_eq!(p.rep_penalty, DEFAULT_REP_PENALTY);
        assert_eq!(p.language, "es");
    }

    /// `with_temperature(None, ..)` equivale a `production(..)`; con `Some` solo
    /// cambia la temperatura (bordes del rango válido incluidos).
    #[test]
    fn generation_options_with_temperature_resolves_override() {
        let p = GenerationOptions::with_temperature(None, "en");
        assert_eq!(p.temperature, 0.35);
        assert_eq!(p.seed, Some(4));
        assert_eq!(p.language, "en");
        let o = GenerationOptions::with_temperature(Some(0.9), "en");
        assert_eq!(o.temperature, 0.9);
        assert_eq!(o.seed, Some(4));
        assert_eq!(o.top_k, DEFAULT_TOP_K);
        let min = GenerationOptions::with_temperature(Some(f32::MIN_POSITIVE), "en");
        assert!(min.temperature > 0.0);
        let max = GenerationOptions::with_temperature(Some(2.0), "en");
        assert_eq!(max.temperature, 2.0);
    }

    /// El mapeo token del producto → nombre del motor solo admite `es` y `en`;
    /// cualquier otro token es un error.
    #[test]
    fn engine_language_name_maps_product_tokens_and_rejects_others() {
        assert_eq!(engine_language_name("es").unwrap(), "Spanish");
        assert_eq!(engine_language_name("en").unwrap(), "English");
        for bad in ["es-latam", "fr", "", "Spanish"] {
            assert!(
                engine_language_name(bad).is_err(),
                "el token `{}` debe rechazarse",
                bad
            );
        }
    }

    /// El argv de clonado pide solo la huella de timbre (`--xvector-only`) y no
    /// lleva `--ref-text`: el motor no genera códigos ICL ni guarda transcripción.
    #[test]
    fn build_clone_command_requests_xvector_only() {
        let (program, args) = build_clone_command(
            Path::new("qwen_tts.exe"),
            Path::new("base"),
            Path::new("ref24k.wav"),
            Path::new("out.qvoice"),
            "ana",
        );
        assert_eq!(program, PathBuf::from("qwen_tts.exe"));
        assert_eq!(
            args,
            vec![
                "-d",
                "base",
                "--ref-audio",
                "ref24k.wav",
                "--xvector-only",
                "--save-voice",
                "out.qvoice",
                "--voice-name",
                "ana",
                "--watch-stdin",
            ]
        );
        assert!(!args.iter().any(|a| a == "--ref-text" || a == "-l"));
    }

    /// Un idioma desconocido hace fallar la construcción del body: nunca se
    /// cae a un idioma por defecto.
    #[test]
    fn build_tts_body_rejects_unknown_language() {
        let opts = GenerationOptions::new("fr");
        let preset = VoiceEngine::Preset("ryan".to_string());
        assert!(build_tts_body("Hola", &preset, &opts, None, None).is_err());
        let cloned = VoiceEngine::Cloned(PathBuf::from("voice.qvoice"));
        assert!(build_tts_body("Hola", &cloned, &opts, None, None).is_err());
    }

    /// Argv exacto de arranque del residente (preset y voz clonada con
    /// `--graft`), sin I/O real de proceso. Sin `--int4`: en el modelo 0.6B
    /// cuantiza el code predictor y la misma petición deja de ser determinista
    /// y sale truncada.
    #[test]
    fn build_resident_command_has_threads_and_stream_without_int4() {
        let (program, args) = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("vendor/qwen3-tts/qwen3-tts-0.6b"),
            45123,
            None,
        );
        assert_eq!(program, PathBuf::from("qwen_tts.exe"));
        assert_eq!(
            args,
            vec![
                "-d",
                "vendor/qwen3-tts/qwen3-tts-0.6b",
                "--serve",
                "45123",
                "--host",
                "127.0.0.1",
                "-j",
                "4",
                "--stream",
                "--watch-stdin",
            ]
        );

        let (program, args) = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            45123,
            Some(Path::new("voice.qvoice")),
        );
        assert_eq!(program, PathBuf::from("qwen_tts.exe"));
        assert_eq!(
            args,
            vec![
                "-d",
                "md",
                "--serve",
                "45123",
                "--host",
                "127.0.0.1",
                "-j",
                "4",
                "--stream",
                "--watch-stdin",
                "--load-voice",
                "voice.qvoice",
                "--graft",
            ]
        );
    }

    /// La voz clonada se sintetiza con el modelo Base y el preset con
    /// CustomVoice: cada tipo de voz elige su directorio de pesos.
    #[test]
    fn select_model_dir_picks_base_for_cloned_and_custom_voice_for_preset() {
        let custom = PathBuf::from("modelos/qwen3-tts-0.6b");
        let base = PathBuf::from("modelos/qwen3-tts-0.6b-base");
        let preset = VoiceEngine::Preset("ryan".to_string());
        let cloned = VoiceEngine::Cloned(PathBuf::from("voice.qvoice"));
        assert_eq!(
            select_model_dir(&preset, Some(&custom), Some(&base)).unwrap(),
            custom.as_path()
        );
        assert_eq!(
            select_model_dir(&cloned, Some(&custom), Some(&base)).unwrap(),
            base.as_path()
        );
    }

    /// Una voz clonada sin el modelo Base provisionado es un error explícito,
    /// aunque CustomVoice exista; un preset sin CustomVoice también falla.
    #[test]
    fn select_model_dir_fails_when_the_required_model_is_missing() {
        let custom = PathBuf::from("modelos/qwen3-tts-0.6b");
        let base = PathBuf::from("modelos/qwen3-tts-0.6b-base");
        let preset = VoiceEngine::Preset("ryan".to_string());
        let cloned = VoiceEngine::Cloned(PathBuf::from("voice.qvoice"));
        let err = select_model_dir(&cloned, Some(&custom), None).unwrap_err();
        assert!(err.to_string().contains("Base"), "mensaje: {err}");
        assert!(select_model_dir(&preset, None, Some(&base)).is_err());
    }

    /// La síntesis de una voz clonada sin modelo Base falla con el error
    /// tipado `ModelMissing` antes de lanzar ningún motor, aunque CustomVoice
    /// esté provisionado; así las capas superiores lo reportan como
    /// `model_missing` y no como un fallo genérico de síntesis.
    #[test]
    fn synthesize_cloned_voice_without_base_model_fails_with_model_missing() {
        let dir = tempfile::tempdir().unwrap();
        let qvoice = dir.path().join("reference.qvoice");
        std::fs::write(&qvoice, b"qvoice").unwrap();
        let mut engine = Qwen3TtsEngine::new(None).expect("motor de prueba");
        engine.model_dir = Some(dir.path().to_path_buf());
        engine.base_model_dir = None;
        let profile = VoiceProfile {
            name: "clonada".to_string(),
            qvoice_path: Some(qvoice),
        };
        let err = engine
            .synthesize_with_options(
                "Hola",
                &profile,
                &GenerationOptions::new("es"),
                Some(&dir.path().join("salida.wav")),
            )
            .unwrap_err();
        assert!(
            err.downcast_ref::<ModelMissing>().is_some(),
            "se esperaba ModelMissing, fue: {err}"
        );
    }

    /// Mata y recoge al motor al soltarse, también si la prueba falla, para no
    /// dejar procesos `qwen_tts` vivos.
    struct EngineGuard(std::process::Child);

    impl Drop for EngineGuard {
        fn drop(&mut self) {
            // Mejor esfuerzo acotado, nunca espera sin cota: tras matar, el
            // hijo está muerto y solo falta cosechar su estado con sondeo.
            let _ = self.0.kill();
            let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            while self
                .0
                .try_wait()
                .map(|slot| slot.is_none())
                .unwrap_or(false)
                && std::time::Instant::now() < reap_deadline
            {
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
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
        let model_dir = resolve_model_dir(Some(&bin))
            .expect("no se pudo resolver el directorio de pesos qwen3-tts-0.6b")
            .expect(
                "faltan los pesos qwen3-tts-0.6b: aprovisiónalos con `ai-voice-interconnector setup`",
            );
        let port = reserve_loopback_port().expect("debe poder reservar un puerto efímero");
        let mut cmd = Command::new(&bin);
        cmd.arg("-d")
            .arg(&model_dir)
            .arg("--serve")
            .arg(port.to_string())
            .arg("--host")
            .arg("127.0.0.1")
            .arg("-j")
            .arg("4");
        if watch_stdin {
            cmd.arg("--watch-stdin");
        }
        // Sin consola propia, un invocador sin consola vería abrirse una ventana.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(avi_process::CREATE_NO_WINDOW);
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
        dir: tempfile::TempDir,
    }

    impl Drop for DoubleCleanup {
        fn drop(&mut self) {
            // El directorio se borra al soltar el campo `dir`, ya sin el proceso.
            let _ = resident::kill_tree_resident_by_pid(self.pid);
        }
    }

    /// Escribe un motor doble ejecutable en un directorio temporal propio y
    /// devuelve su ruta y el directorio. Al arrancar registra sus argumentos en
    /// `args.txt`, sondea su entrada estándar durante 500 ms y registra en
    /// `stdin.txt` si vio `eof` (entrada nula o cerrada) u `open` (tubería
    /// abierta). Con `hold`: si la tubería estaba abierta espera a que se cierre
    /// y sale; si vio `eof` no sale por sí mismo (duerme 60 s). Sin `hold`
    /// sale tras el sondeo.
    fn write_engine_double(name: &str, hold: bool) -> (PathBuf, tempfile::TempDir) {
        let temp = tempfile::Builder::new()
            .prefix(&format!("avi-tts-double-{name}-"))
            .tempdir()
            .expect("debe poder crear el directorio del doble");
        let dir = temp.path().to_path_buf();
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
        let mut rustc = Command::new("rustc");
        rustc
            .arg("--edition")
            .arg("2021")
            .arg(&src)
            .arg("-o")
            .arg(&bin);
        // Sin consola propia, un invocador sin consola vería abrirse una ventana.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            rustc.creation_flags(avi_process::CREATE_NO_WINDOW);
        }
        let compiled = rustc
            .output()
            .expect("rustc debe estar disponible para compilar el doble");
        assert!(
            compiled.status.success(),
            "no se pudo compilar el doble: {}",
            String::from_utf8_lossy(&compiled.stderr)
        );
        (bin, temp)
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
    fn launch_double(
        bin: &Path,
        dir: tempfile::TempDir,
    ) -> (avi_process::RestrictedChild, DoubleCleanup) {
        let log_file = std::fs::File::create(dir.path().join("engine.log")).expect("log del doble");
        let child =
            resident::launch_resident_process(bin, Path::new("modelos"), 45123, None, log_file)
                .expect("debe poder lanzar el motor doble");
        let cleanup = DoubleCleanup {
            pid: child.id(),
            dir,
        };
        (child, cleanup)
    }

    /// El arranque del residente pasa `--watch-stdin` al motor, para que
    /// termine cuando su entrada se cierre.
    #[test]
    fn launch_resident_process_passes_watch_stdin() {
        let (bin, dir) = write_engine_double("args", false);
        let (_child, cleanup) = launch_double(&bin, dir);
        let args = wait_for_marker(&cleanup.dir.path().join("args.txt"));
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
        let (_child, cleanup) = launch_double(&bin, dir);
        let state = wait_for_marker(&cleanup.dir.path().join("stdin.txt"));
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
        let (child, cleanup) = launch_double(&bin, dir);
        // El sondeo de entrada del doble debe haber concluido antes de provocar
        // el fallo, para que el cierre de la tubería llegue después de verla abierta.
        let _ = wait_for_marker(&cleanup.dir.path().join("stdin.txt"));
        let live = Mutex::new(None);
        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            1,
            test_log_path(),
            2,
            30,
            &bare(&live),
        );
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

    /// Body HTTP con defaults → claves exactas; voz clonada → sin speaker pero con
    /// `language` en nombre del motor.
    /// Afirma los defaults del `struct`/motor sin cambios, no los valores de
    /// producción de `GenerationOptions::production(..)` (config validada por oído) — el body HTTP
    /// no transporta `-j`/`--stream` (son flags de arranque de proceso).
    #[test]
    fn build_tts_body_defaults_and_cloned_voice() {
        let voice = VoiceEngine::Preset("ryan".to_string());
        let body = build_tts_body("Hola", &voice, &GenerationOptions::new("es"), None, None)
            .expect("el idioma es válido");
        let obj = body.as_object().expect("body debe ser objeto");
        assert_eq!(obj.get("text").and_then(|v| v.as_str()), Some("Hola"));
        assert_eq!(obj.get("speaker").and_then(|v| v.as_str()), Some("ryan"));
        assert_eq!(
            obj.get("language").and_then(|v| v.as_str()),
            Some("Spanish")
        );
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

        let cloned = VoiceEngine::Cloned(PathBuf::from("voice.qvoice"));
        let body = build_tts_body("Hola", &cloned, &GenerationOptions::new("en"), None, None)
            .expect("el idioma es válido");
        let obj = body.as_object().expect("body debe ser objeto");
        assert!(!obj.contains_key("speaker"), "voz clonada omite speaker");
        assert_eq!(
            obj.get("language").and_then(|v| v.as_str()),
            Some("English")
        );
        let body = build_tts_body("Hola", &voice, &GenerationOptions::new("en"), None, None)
            .expect("el idioma es válido");
        assert_eq!(
            body.get("language").and_then(|v| v.as_str()),
            Some("English")
        );

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
            &GenerationOptions::new("es"),
            Some(&prosody),
            Some(&emotion),
        )
        .expect("el idioma es válido");
        let obj = body.as_object().expect("body debe ser objeto");
        assert!((obj.get("volume").and_then(|v| v.as_f64()).unwrap() - 1.1).abs() < 1e-6);
        assert!((obj.get("rate").and_then(|v| v.as_f64()).unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(obj.get("emotion").and_then(|v| v.as_str()), Some("joy"));
    }

    /// Tabla de resolución voz → motor: con `reference.qvoice` la voz es
    /// clonada (también `default`); sin él solo `ryan` y `vivian` son presets y
    /// cualquier otra voz es `VoiceNotFound`.
    #[test]
    fn resolve_voice_engine_table() {
        let tmp = tempfile::tempdir().unwrap();
        let q = tmp.path().join("reference.qvoice");
        std::fs::write(&q, b"QVCE").unwrap();
        assert_eq!(
            resolve_voice_engine("my_voice", Some(&q)),
            Ok(VoiceEngine::Cloned(q.clone()))
        );
        assert_eq!(
            resolve_voice_engine("default", Some(&q)),
            Ok(VoiceEngine::Cloned(q.clone()))
        );
        for preset in ["ryan", "vivian"] {
            assert_eq!(
                resolve_voice_engine(preset, None),
                Ok(VoiceEngine::Preset(preset.to_string()))
            );
        }
        // El nombre del preset no distingue mayúsculas, como el almacén.
        assert_eq!(
            resolve_voice_engine("Ryan", None),
            Ok(VoiceEngine::Preset("ryan".to_string()))
        );
        // Sin referencia y sin ser preset (incluida `default`): voz no hallada.
        for name in ["default", "my_voice"] {
            let err = resolve_voice_engine(name, None).unwrap_err();
            assert_eq!(err.voice, name);
        }
        // Una referencia inexistente en disco equivale a no tener referencia.
        let missing = tmp.path().join("missing.qvoice");
        assert!(resolve_voice_engine("my_voice", Some(&missing)).is_err());
    }

    /// La síntesis de una voz que no es preset ni tiene referencia falla con el
    /// error tipado `VoiceNotFound`, sin lanzar ningún motor, para que las
    /// capas superiores lo reporten como `voice_not_found`.
    #[test]
    fn synthesize_unknown_voice_fails_with_voice_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let engine = Qwen3TtsEngine::new(None).expect("motor de prueba");
        let profile = VoiceProfile {
            name: "inexistente".to_string(),
            qvoice_path: None,
        };
        let err = engine
            .synthesize_with_options(
                "Hola",
                &profile,
                &GenerationOptions::new("es"),
                Some(&dir.path().join("salida.wav")),
            )
            .unwrap_err();
        let not_found = err
            .downcast_ref::<VoiceNotFound>()
            .unwrap_or_else(|| panic!("se esperaba VoiceNotFound, fue: {err}"));
        assert_eq!(not_found.voice, "inexistente");
    }

    /// Una variable de entorno de ruta definida debe apuntar a un archivo o
    /// directorio existente (según corresponda) y el error nombra la variable;
    /// vacía o ausente significa «sin definir».
    #[test]
    fn validated_env_path_requires_existing_path_naming_the_variable() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("qwen_tts");
        std::fs::write(&file, b"bin").unwrap();

        assert_eq!(validated_env_path("VAR", None, true).unwrap(), None);
        assert_eq!(
            validated_env_path("VAR", Some("".into()), false).unwrap(),
            None
        );
        assert_eq!(
            validated_env_path("VAR", Some(file.clone().into_os_string()), true).unwrap(),
            Some(file.clone())
        );
        assert_eq!(
            validated_env_path("VAR", Some(tmp.path().as_os_str().to_owned()), false).unwrap(),
            Some(tmp.path().to_path_buf())
        );

        let missing = tmp.path().join("no_existe");
        for (value, expect_file) in [
            (missing.clone(), true),
            (missing.clone(), false),
            (tmp.path().to_path_buf(), true),
            (file, false),
        ] {
            let err = validated_env_path("QWEN3_TTS_X", Some(value.into_os_string()), expect_file)
                .unwrap_err()
                .to_string();
            assert!(
                err.contains("QWEN3_TTS_X"),
                "debe nombrar la variable: {err}"
            );
        }
    }

    /// El healthcheck responde cuando el listener simula `/v1/health`, y
    /// el `Drop` del gestor termina al hijo.
    #[test]
    fn resident_healthcheck_ok_and_drop_kills_child() {
        let (port, handle) = resident::simulate_server(Arc::new(Mutex::new(String::new())));
        let child = sleeping_process();
        let pid = child.id();
        let live = Mutex::new(None);
        let resident = resident::Qwen3TtsResident::spawn_with_child(
            child,
            port,
            test_log_path(),
            10,
            50,
            &bare(&live),
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
        let live = Mutex::new(None);
        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            port,
            test_log_path(),
            5,
            50,
            &bare(&live),
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
        let live = Mutex::new(None);
        let result = resident::Qwen3TtsResident::spawn_with_child(
            child,
            1,
            test_log_path(),
            3,
            30,
            &bare(&live),
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

    /// Un servidor que acepta la conexión pero nunca responde agota el
    /// presupuesto inyectado y produce `SynthesisTimeout` con ese presupuesto.
    /// `simulate_server` responde siempre, así que aquí se usa un listener mudo.
    #[test]
    fn http_exchange_silent_server_yields_synthesis_timeout() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("puerto libre");
        let port = listener.local_addr().unwrap().port();
        let handle = thread::spawn(move || {
            let (_stream, _) = listener.accept().expect("conexión del cliente");
            thread::sleep(Duration::from_millis(1500));
        });
        let budget = Duration::from_millis(200);
        let err = http_exchange(
            &format!("http://127.0.0.1:{}/v1/tts", port),
            "POST",
            Some("{}"),
            budget,
            Some(&AtomicBool::new(false)),
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

    /// Activar la bandera de cancelación cierra la conexión en pocos periodos
    /// de sondeo y el servidor observa EOF: es la señal con la que el motor
    /// aborta la generación. El tiempo se mide desde la activación, no desde
    /// el inicio del intercambio, para no contar la espera del hilo que la
    /// activa ni la planificación de hilos de una máquina cargada. El tope,
    /// diez periodos de sondeo, sigue muy por debajo de los 10 s de plazo del
    /// intercambio, así que solo se cumple si el corte lo causó la bandera.
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
                let activated = Instant::now();
                cancel.store(true, Ordering::SeqCst);
                activated
            })
        };
        let err = http_exchange(
            &format!("http://127.0.0.1:{}/v1/tts", port),
            "POST",
            Some("{}"),
            Duration::from_secs(10),
            Some(&cancel),
        )
        .expect_err("la bandera debe cancelar el intercambio");
        let ended = Instant::now();
        assert!(
            err.downcast_ref::<SynthesisCancelled>().is_some(),
            "el error debe envolver SynthesisCancelled: {}",
            err
        );
        let activated = setter.join().unwrap();
        let elapsed = ended.duration_since(activated);
        assert!(
            elapsed < CANCEL_POLL * 10,
            "la cancelación debe cerrar pronto tras activar la bandera: {:?}",
            elapsed
        );
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
    /// `qwen_tts`; el cierre preciso por árbol se cubre en
    /// `resident_kill_tree_by_pid_terminates_child`.
    fn sleeping_process() -> avi_process::RestrictedChild {
        let log_file = tempfile::tempfile().expect("log del proceso durmiente");
        // Rutas completas: el lanzamiento restringido no busca nombres sueltos.
        let (program, args) = if cfg!(windows) {
            (
                PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                    .join(r"System32\WindowsPowerShell\v1.0\powershell.exe"),
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
            stderr_file: log_file.try_clone().expect("clon del log durmiente"),
            stdout_file: log_file,
            creation_flags: 0,
            extra_allowed: Vec::new(),
        };
        avi_process::spawn_with_allowlist(request).expect("debe lanzar el proceso durmiente")
    }

    /// Hijo que termina de inmediato con exit 1, lanzado con herencia
    /// restringida como el producto.
    fn exiting_process() -> avi_process::RestrictedChild {
        let log_file = tempfile::tempfile().expect("log del proceso saliente");
        let (program, args) = if cfg!(windows) {
            (
                PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                    .join(r"System32\cmd.exe"),
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
            stderr_file: log_file.try_clone().expect("clon del log saliente"),
            stdout_file: log_file,
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
        let _guard = RESOLUTION_ENV_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
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
        let found = resolve_binary().expect("la resolución no debe fallar sin QWEN3_TTS_BIN");
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

    /// Candado que serializa las pruebas que tocan el CWD o las variables de
    /// resolución (el entorno y el CWD son globales al proceso).
    static RESOLUTION_ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Restaura el CWD y las variables tocadas al soltarse, también si la
    /// prueba falla.
    struct ResolutionRestore {
        prev_cwd: std::path::PathBuf,
        vars: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl ResolutionRestore {
        fn take(vars: &[&'static str]) -> Self {
            let prev_cwd = std::env::current_dir().expect("debe leer el CWD");
            let saved = vars
                .iter()
                .map(|key| (*key, std::env::var_os(key)))
                .collect();
            Self {
                prev_cwd,
                vars: saved,
            }
        }
    }

    impl Drop for ResolutionRestore {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.prev_cwd);
            for (key, value) in self.vars.drain(..) {
                match value {
                    Some(previous) => std::env::set_var(key, previous),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    /// Nombre del binario del motor según la plataforma, como lo espera la
    /// capa `<cwd>/vendor`.
    fn cwd_vendor_bin() -> std::path::PathBuf {
        std::path::PathBuf::from(if cfg!(windows) {
            "vendor/qwen3-tts/qwen_tts.exe"
        } else {
            "vendor/qwen3-tts/qwen_tts"
        })
    }

    /// Sin opt-in, la capa `<cwd>/vendor` no aporta el binario aunque exista;
    /// con `=1`, resuelve como antes. Capas superiores neutralizadas.
    #[test]
    fn resolve_binary_cwd_needs_opt_in() {
        let _guard = RESOLUTION_ENV_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _restore = ResolutionRestore::take(&["QWEN3_TTS_BIN", "AVI_ALLOW_CWD_VENDOR", "PATH"]);
        let work = tempfile::tempdir().expect("debe crear el CWD de prueba");
        let rel = cwd_vendor_bin();
        let abs = work.path().join(&rel);
        std::fs::create_dir_all(abs.parent().unwrap()).expect("debe crear vendor");
        std::fs::write(&abs, b"falso").expect("debe escribir el señuelo");
        std::env::set_current_dir(work.path()).expect("debe cambiar el CWD");
        std::env::remove_var("QWEN3_TTS_BIN");
        std::env::set_var("PATH", work.path().join("vacio"));
        std::env::remove_var("AVI_ALLOW_CWD_VENDOR");
        let denied = resolve_binary().expect("la resolución no debe fallar");
        assert_ne!(
            denied,
            Some(rel.clone()),
            "sin opt-in el CWD no debe aportar el binario"
        );
        std::env::set_var("AVI_ALLOW_CWD_VENDOR", "1");
        let allowed = resolve_binary().expect("la resolución no debe fallar");
        assert_eq!(
            allowed,
            Some(rel),
            "con opt-in el CWD debe resolver como antes"
        );
    }

    /// Sin opt-in, la capa `<cwd>/vendor` no aporta el directorio del modelo;
    /// con `=1`, sí. Fixture con nombre imposible en snapshots reales.
    #[test]
    fn resolve_named_model_dir_cwd_needs_opt_in() {
        let _guard = RESOLUTION_ENV_GUARD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _restore =
            ResolutionRestore::take(&["AVI_ALLOW_CWD_VENDOR", "AVI_TTS_TEST_SIN_DEFINIR"]);
        let work = tempfile::tempdir().expect("debe crear el CWD de prueba");
        let rel = std::path::PathBuf::from("vendor/qwen3-tts").join("fixture-inexistente-xyz");
        std::fs::create_dir_all(work.path().join(&rel)).expect("debe crear el modelo falso");
        std::env::set_current_dir(work.path()).expect("debe cambiar el CWD");
        std::env::remove_var("AVI_ALLOW_CWD_VENDOR");
        let denied =
            resolve_named_model_dir("AVI_TTS_TEST_SIN_DEFINIR", "fixture-inexistente-xyz", None)
                .expect("la resolución no debe fallar");
        assert_eq!(denied, None, "sin opt-in el CWD no debe aportar modelos");
        std::env::set_var("AVI_ALLOW_CWD_VENDOR", "1");
        let allowed =
            resolve_named_model_dir("AVI_TTS_TEST_SIN_DEFINIR", "fixture-inexistente-xyz", None)
                .expect("la resolución no debe fallar");
        assert_eq!(
            allowed,
            Some(rel),
            "con opt-in el CWD debe resolver como antes"
        );
    }
}
