use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
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
    /// Config de producción validada por oído: `temperature=0.35` y `seed=42`
    /// fijo, resto de campos igual a `Default`. El `temperature=0` previo corría
    /// el Talker en greedy argmax sobre un clon x-vector-only sin plantilla de
    /// prosodia, produciendo prosodia plana/extraña; 0.35 reactiva el muestreo
    /// estocástico (y vuelve efectivo el `seed`) preservando la naturalidad sin
    /// soltarse como el default del motor (0.9). No sustituye a `Default` (que
    /// debe seguir coincidiendo con los defaults del motor) sino que es la
    /// superficie que cablea la síntesis de producción (`Qwen3TtsEngine::synthesize`).
    pub fn produccion() -> Self {
        Self {
            temperature: 0.35,
            seed: Some(42),
            ..Self::default()
        }
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
    fn synthesize(
        &self,
        text: &str,
        voice: &str,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf>;

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

/// Tabla de resolución voz → motor (decisión e3): la voz de fábrica `default`
/// se mapea al preset `ryan` (default del servidor); una voz con `reference.qvoice`
/// (o `reference.wav` legado, que el orquestador convierte antes de usar) es
/// clonada; cualquier otro nombre se pasa como preset del motor.
pub fn resolve_voice_motor(voice: &str, qvoice: Option<&Path>, reference: Option<&Path>) -> VozMotor {
    if voice == "default" {
        return VozMotor::Preset("ryan".to_string());
    }
    if let Some(q) = qvoice {
        if q.is_file() {
            return VozMotor::Clonada(q.to_path_buf());
        }
    }
    if let Some(r) = reference {
        if r.is_file() {
            return VozMotor::Clonada(r.to_path_buf());
        }
    }
    VozMotor::Preset(voice.to_string())
}

/// Resolución del binario del motor por capas (decisión e1):
/// 1. `QWEN3_TTS_BIN`; 2. `<cwd>/vendor/qwen3-tts/qwen_tts(.exe)`;
/// 3. búsqueda en `PATH`.
fn resolve_binary() -> Option<PathBuf> {
    if let Some(b) = std::env::var_os("QWEN3_TTS_BIN") {
        let p = PathBuf::from(b);
        if !p.as_os_str().is_empty() {
            return Some(p);
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
    let name = if cfg!(windows) { "qwen_tts.exe" } else { "qwen_tts" };
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
/// (`<dir del bin>/qwen3-tts-0.6b`); 3. `<cwd>/vendor/qwen3-tts/qwen3-tts-0.6b`.
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
/// 1. `QWEN3_TTS_BASE_MODEL_DIR`; 2. directorio hermano del binario
/// (`<dir del bin>/qwen3-tts-0.6b-base`); 3. `<cwd>/vendor/qwen3-tts/qwen3-tts-0.6b-base`.
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
    let vendored = PathBuf::from("vendor/qwen3-tts/qwen3-tts-0.6b-base");
    if vendored.is_dir() {
        return Some(vendored);
    }
    None
}

/// Motor Qwen3-TTS con servidor HTTP residente gestionado por el host y
/// fallback a subprocess con PCM por stdout.
pub struct Qwen3TtsEngine {
    pub server_url: Option<String>,
    pub binary_path: Option<PathBuf>,
    pub model_dir: Option<PathBuf>,
    pub base_model_dir: Option<PathBuf>,
    resident: Mutex<Option<ResidentState>>,
}

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
        }
    }

    /// Intentar la síntesis vía HTTP local (servidor manual o residente).
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

    /// Intentar la síntesis vía invocación de subprocess binario con `--stdout`.
    fn synthesize_via_subprocess(
        &self,
        text: &str,
        voz: &VozMotor,
        options: &GenerationOptions,
        out_path: &Path,
    ) -> Result<()> {
        let bin = self
            .binary_path
            .as_ref()
            .ok_or_else(|| anyhow!("El binario de síntesis Qwen3-TTS no está provisionado."))?;
        let model_dir = self
            .model_dir
            .as_ref()
            .ok_or_else(|| anyhow!("El modelo de síntesis Qwen3-TTS no está provisionado."))?;
        let mut cmd = build_synthesis_command(bin, model_dir, text, voz, options);
        let output = cmd.output().map_err(|e| {
            anyhow!(
                "No se pudo ejecutar el binario Qwen3-TTS ({}): {}",
                bin.display(),
                e
            )
        })?;
        if !output.status.success() {
            return Err(anyhow!(
                "Subproceso Qwen3-TTS finalizó con código de error: {:?}",
                output.status.code()
            ));
        }
        pcm_a_wav(&output.stdout, out_path)
    }

    /// Síntesis vía servidor residente: arranca (o reutiliza) el residente de
    /// la voz solicitada y hace `POST /v1/tts`.
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
        let mut guard = self.resident.lock().unwrap();
        if guard.as_ref().map(|s| s.voz_key.as_str()) != Some(voz_key.as_str()) {
            let load_voice = match voz {
                VozMotor::Clonada(p) => Some(p.as_path()),
                VozMotor::Preset(_) => None,
            };
            let port = default_port();
            let spawned = resident::Qwen3TtsResident::spawn(model_dir, port, load_voice)?;
            *guard = Some(ResidentState {
                resident: spawned,
                voz_key,
            });
        }
        let state = guard.as_ref().expect("residente recién arrancado");
        let url = format!("http://127.0.0.1:{}", state.resident.port);
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
        let qvoice = match (profile.qvoice_path.as_deref(), profile.reference_audio.as_deref()) {
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
        let voz = resolve_voice_motor(&profile.name, qvoice.as_deref(), None);

        // 1. HTTP manual configurado (solo presets; la voz clonada exige un
        //    servidor arrancado con su `--load-voice`, que solo gestiona el residente).
        if let Some(url) = &self.server_url {
            if matches!(voz, VozMotor::Preset(_)) {
                if self
                    .synthesize_via_http(url, text, &voz, options, None, None, &path)
                    .is_ok()
                {
                    return Ok(path);
                }
            }
        }

        // 2. Servidor residente gestionado por el host (decisión F0). Este orden
        //    (residente antes que subprocess) es OBLIGATORIO, no solo preferido:
        //    el subprocess recibe el texto por argv y el `.exe` MinGW mal-tokeniza
        //    UTF-8 acentuado en Windows, mientras que el residente lo transporta
        //    por body HTTP JSON (ruta segura). Invertir el orden reintroduciría el
        //    bug de calidad en español con tildes/eñes.
        match self.synthesize_via_residente(text, &voz, options, &path) {
            Ok(()) => return Ok(path),
            Err(e) => eprintln!(
                "[avi-tts] El servidor residente Qwen3-TTS falló; reintentando por subproceso: {}",
                e
            ),
        }

        // 3. Fallback subprocess con `--stdout`.
        if self.binary_path.is_some() {
            if self.synthesize_via_subprocess(text, &voz, options, &path).is_ok() {
                return Ok(path);
            }
        }

        // 4. Sin binario ni servidor disponibles, el modelo de inferencia no está provisionado.
        Err(anyhow!(
            "El modelo o binario de síntesis Qwen3-TTS no está provisionado."
        ))
    }
}

/// Construye el `Command` del subprocess de síntesis (Tarea 2): invocación real
/// `-d <model_dir> -t <text> -s <speaker> -l <language>` con flags condicionales
/// `-T/-k/-p/-r/--seed` cuando difieren de los defaults del motor, o
/// `--load-voice <qvoice> --icl-only` en lugar de `-s` para voz clonada;
/// siempre `--stdout` (PCM s16le 24 kHz por stdout).
pub(crate) fn build_synthesis_command(
    bin: &Path,
    model_dir: &Path,
    text: &str,
    voz: &VozMotor,
    options: &GenerationOptions,
) -> Command {
    let mut cmd = Command::new(bin);
    cmd.arg("-d").arg(model_dir).arg("-t").arg(text);
    match voz {
        VozMotor::Preset(speaker) => {
            cmd.arg("-s").arg(speaker).arg("-l").arg(&options.language);
        }
        VozMotor::Clonada(qvoice) => {
            cmd.arg("--load-voice").arg(qvoice).arg("--icl-only");
        }
    }
    cmd.arg("--int4");
    cmd.arg("-j").arg("4");
    if options.temperature != DEFAULT_TEMPERATURE {
        cmd.arg("-T").arg(options.temperature.to_string());
    }
    if options.top_k != DEFAULT_TOP_K {
        cmd.arg("-k").arg(options.top_k.to_string());
    }
    if options.top_p != DEFAULT_TOP_P {
        cmd.arg("-p").arg(options.top_p.to_string());
    }
    if options.rep_penalty != DEFAULT_REP_PENALTY {
        cmd.arg("-r").arg(options.rep_penalty.to_string());
    }
    if let Some(seed) = options.seed {
        cmd.arg("--seed").arg(seed.to_string());
    }
    cmd.arg("--stream");
    cmd.arg("--stdout");
    cmd
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
    obj.insert("text".to_string(), serde_json::Value::String(text.to_string()));
    match voz {
        VozMotor::Preset(speaker) => {
            obj.insert("speaker".to_string(), serde_json::Value::String(speaker.clone()));
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
fn http_exchange(url: &str, method: &str, body: Option<&str>, timeout: Duration) -> Result<(u16, Vec<u8>)> {
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
    let header_end = text
        .find("\r\n\r\n")
        .ok_or_else(|| anyhow!("Respuesta HTTP sin final de headers: {:?}", &text[..text.len().min(80)]))?;
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

/// Envuelve el PCM s16le 24 kHz mono crudo (canal `--stdout` del motor) en un
/// WAV válido con la misma spec que `/v1/tts` (24 kHz / 16-bit / mono).
pub(crate) fn pcm_a_wav(pcm: &[u8], out_path: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 24_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(out_path, spec)?;
    for chunk in pcm.chunks_exact(2) {
        writer.write_sample(i16::from_le_bytes([chunk[0], chunk[1]]))?;
    }
    writer.finalize()?;
    Ok(())
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
    use std::process::{Child, Stdio};
    use std::thread;
    use std::time::Duration;
    #[cfg(test)]
    use std::io::Read;
    #[cfg(test)]
    use std::io::Write;
    #[cfg(test)]
    use std::net::TcpListener;

    /// Gestor del proceso servidor del motor.
    pub struct Qwen3TtsResident {
        child: Option<Child>,
        pub port: u16,
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
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
            // Riesgo R2 documentado: el motor enlaza en INADDR_ANY, no en loopback.
            eprintln!(
                "[avi-tts] Aviso: el motor Qwen3-TTS enlaza en todas las interfaces \
                 (INADDR_ANY), puerto {}. El servidor es accesible desde la red local.",
                port
            );
            let child = cmd.spawn().map_err(|e| {
                anyhow!("No se pudo arrancar el servidor Qwen3-TTS ({}): {}", bin.display(), e)
            })?;
            Self::spawn_con_hijo(child, port, 60, 500)
        }

        /// Arranca el healthcheck sobre un hijo ya lanzado (retries/intervalo
        /// configurables para los tests de reintentos).
        pub(crate) fn spawn_con_hijo(
            child: Child,
            port: u16,
            retries: usize,
            interval_ms: u64,
        ) -> Result<Self> {
            wait_health(port, retries, interval_ms)?;
            Ok(Self {
                child: Some(child),
                port,
            })
        }
    }

    impl Drop for Qwen3TtsResident {
        fn drop(&mut self) {
            if let Some(mut child) = self.child.take() {
                // Windows: TerminateProcess; Unix: SIGKILL (shutdown determinista).
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }

    /// Healthcheck `GET /v1/health` con reintentos.
    pub(crate) fn wait_health(port: u16, retries: usize, interval_ms: u64) -> Result<()> {
        let url = format!("http://127.0.0.1:{}/v1/health", port);
        for i in 0..retries {
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
            "El servidor Qwen3-TTS no respondió a /v1/health en el puerto {} tras {} intentos.",
            port,
            retries
        ))
    }

    /// Simulador HTTP mínimo para tests: responde `200 OK` a `/v1/health` y
    /// captura el body de un único `POST /v1/tts`.
    #[cfg(test)]
    pub(crate) fn simular_servidor(body: std::sync::Arc<Mutex<String>>) -> (u16, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("debe bindear un puerto libre");
        let port = listener.local_addr().expect("puerto local").port();
        let handle = thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let mut buf = Vec::new();
                let mut tmp = [0u8; 8192];
                let mut headers = String::new();
                let mut content_length;
                loop {
                    match stream.read(&mut tmp) {
                        Ok(0) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&tmp[..n]);
                            let text = String::from_utf8_lossy(&buf);
                            if let Some(end) = text.find("\r\n\r\n") {
                                headers = text[..end].to_string();
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
                                    *body.lock().unwrap() =
                                        String::from_utf8_lossy(&buf[header_len..header_len + content_length])
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
        assert_eq!(p.seed, Some(42));
        assert_eq!(p.top_k, DEFAULT_TOP_K);
        assert_eq!(p.top_p, DEFAULT_TOP_P);
        assert_eq!(p.rep_penalty, DEFAULT_REP_PENALTY);
        assert_eq!(p.language, "es");
    }

    /// T2: args del subprocess para preset (con y sin overrides) y voz clonada.
    #[test]
    fn build_synthesis_command_args_preset() {
        let voz = VozMotor::Preset("ryan".to_string());
        let cmd = build_synthesis_command(
            Path::new("qwen_tts.exe"),
            Path::new("vendor/qwen3-tts/qwen3-tts-0.6b"),
            "Hola",
            &voz,
            &GenerationOptions::default(),
        );
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        // Con defaults no se emiten -T/-k/-p/-r (idempotente con los del motor).
        assert_eq!(
            args,
            vec![
                "-d",
                "vendor/qwen3-tts/qwen3-tts-0.6b",
                "-t",
                "Hola",
                "-s",
                "ryan",
                "-l",
                "es",
                "--int4",
                "-j",
                "4",
                "--stream",
                "--stdout",
            ]
        );

        let mut opts = GenerationOptions::default();
        opts.temperature = 0.9;
        opts.top_k = 20;
        opts.top_p = 0.8;
        opts.rep_penalty = 1.2;
        opts.seed = Some(42);
        let cmd = build_synthesis_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            "Hola",
            &voz,
            &opts,
        );
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec![
                "-d", "md", "-t", "Hola", "-s", "ryan", "-l", "es",
                "--int4", "-j", "4",
                "-T", "0.9", "-k", "20", "-p", "0.8", "-r", "1.2", "--seed", "42",
                "--stream", "--stdout",
            ]
        );
    }

    /// T2: voz clonada → `--load-voice <qvoice> --icl-only` en lugar de `-s/-l`.
    #[test]
    fn build_synthesis_command_args_voz_clonada() {
        let voz = VozMotor::Clonada(PathBuf::from("voz.qvoice"));
        let cmd = build_synthesis_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            "Hola",
            &voz,
            &GenerationOptions::default(),
        );
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec![
                "-d", "md", "-t", "Hola", "--load-voice", "voz.qvoice", "--icl-only",
                "--int4", "-j", "4", "--stream", "--stdout",
            ]
        );
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
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec![
                "-d", "vendor/qwen3-tts/qwen3-tts-0.6b", "--serve", "8766",
                "--int4", "-j", "4", "--stream",
            ]
        );

        let cmd = resident::build_resident_command(
            Path::new("qwen_tts.exe"),
            Path::new("md"),
            8766,
            Some(Path::new("voz.qvoice")),
        );
        let args: Vec<String> = cmd.get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert_eq!(
            args,
            vec![
                "-d", "md", "--serve", "8766", "--int4", "-j", "4", "--stream",
                "--load-voice", "voz.qvoice", "--icl-only",
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

    /// T6: tabla de resolución voz → motor.
    #[test]
    fn resolve_voice_motor_tabla() {
        assert_eq!(
            resolve_voice_motor("default", None, None),
            VozMotor::Preset("ryan".to_string())
        );
        let q = std::env::temp_dir().join("avi_tts_test_referencia.qvoice");
        std::fs::write(&q, b"QVCE").unwrap();
        assert_eq!(
            resolve_voice_motor("mi_voz", Some(&q), None),
            VozMotor::Clonada(q.clone())
        );
        let w = std::env::temp_dir().join("avi_tts_test_referencia.wav");
        std::fs::write(&w, b"RIFF").unwrap();
        assert_eq!(
            resolve_voice_motor("mi_voz", None, Some(&w)),
            VozMotor::Clonada(w.clone())
        );
        // Sin referencia → preset con el nombre dado.
        assert_eq!(
            resolve_voice_motor("vivian", None, None),
            VozMotor::Preset("vivian".to_string())
        );
        std::fs::remove_file(&q).ok();
        std::fs::remove_file(&w).ok();
    }

    /// T5: el healthcheck responde cuando el listener simula `/v1/health`, y
    /// el `Drop` del gestor termina al hijo.
    #[test]
    fn residente_healthcheck_ok_y_drop_mata_al_hijo() {
        let (port, handle) = resident::simular_servidor(Arc::new(Mutex::new(String::new())));
        let child = proceso_durmiente();
        let pid = child.id();
        let resident = resident::Qwen3TtsResident::spawn_con_hijo(child, port, 10, 50)
            .expect("el healthcheck debe pasar contra el listener simulado");
        drop(resident);
        // El servidor simulado no termina nunca: se desacopla el hilo.
        drop(handle);
        thread::sleep(Duration::from_millis(800));
        assert!(!proceso_vivo(pid), "el Drop del gestor debe terminar al hijo");
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
        let resultado = resident::Qwen3TtsResident::spawn_con_hijo(child, port, 5, 50);
        // El servidor simulado termina solo tras su N-ésima conexión: se
        // desacopla el hilo y se da margen para que drene las conexiones en vuelo.
        drop(handle);
        thread::sleep(Duration::from_millis(200));
        let n = *attempts.lock().unwrap();
        if let Err(e) = resultado {
            panic!("el healthcheck debe triunfar tras los reintentos ({} conexiones recibidas): {}", n, e);
        }
        if n < 3 {
            panic!("debe haberse reintentado al menos 3 veces (se recibieron {})", n);
        }
    }

    /// T5: el healthcheck falla si el servidor nunca responde.
    #[test]
    fn residente_healthcheck_falla_sin_servidor() {
        let child = proceso_durmiente();
        let result = resident::Qwen3TtsResident::spawn_con_hijo(child, 1, 3, 30);
        assert!(result.is_err(), "sin servidor el healthcheck debe fallar");
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
        assert_eq!(parsed["speaker"], "ryan");
        assert!(parsed.get("seed").is_none());

        let mut opts = GenerationOptions::default();
        opts.temperature = 0.9;
        opts.seed = Some(7);
        let body = Arc::new(Mutex::new(String::new()));
        let (port, handle) = resident::simular_servidor(body.clone());
        let engine = Qwen3TtsEngine::new(Some(format!("http://127.0.0.1:{}", port)));
        let _ = engine.synthesize_with_options("Hola", &profile, &opts, Some(&out));
        drop(handle);
        let parsed: serde_json::Value = serde_json::from_str(&body.lock().unwrap()).unwrap();
        assert!((parsed["temperature"].as_f64().unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(parsed["seed"], 7);
    }

    /// T1: `pcm_a_wav` envuelve PCM crudo en un WAV de la spec del motor.
    #[test]
    fn pcm_a_wav_escribe_wav_24k_mono_16bit() {
        let out = std::env::temp_dir().join("avi_tts_test_pcm.wav");
        let pcm: Vec<u8> = [0i16, 100i16, -100i16, 32767i16]
            .iter()
            .flat_map(|s| s.to_le_bytes())
            .collect();
        pcm_a_wav(&pcm, &out).unwrap();
        let reader = hound::WavReader::open(&out).unwrap();
        let spec = reader.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 24_000);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);
        assert_eq!(reader.duration(), 4);
        std::fs::remove_file(&out).ok();
    }

    /// Proceso que duerme para simular el hijo del residente en tests.
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

    /// ¿Sigue vivo el proceso con `pid`?
    fn proceso_vivo(pid: u32) -> bool {
        if cfg!(windows) {
            let out = Command::new("powershell")
                .args([
                    "-NoProfile",
                    "-Command",
                    &format!("Get-Process -Id {} -ErrorAction SilentlyContinue", pid),
                ])
                .output();
            match out {
                Ok(o) => !o.stdout.is_empty(),
                Err(_) => true,
            }
        } else {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .status()
                .map(|s| s.success())
                .unwrap_or(true)
        }
    }
}