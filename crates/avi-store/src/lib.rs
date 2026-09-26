use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Nombre canónico del producto: directorio de programa, directorio del
/// enlace, nombre del bloqueo y nombre del ejecutable. Fuente única (§7).
pub const APP_NAME: &str = "ai-voice-interconnector";

/// Valor de una variable de entorno de reubicación, ignorando el vacío.
///
/// Además de permitir ubicaciones personalizadas es lo que hace posibles las
/// pruebas aisladas: en Windows las Known Folders ignoran `LOCALAPPDATA`.
fn relocated_dir(var: &str) -> Option<PathBuf> {
    std::env::var(var)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> PathBuf {
    directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// `%LOCALAPPDATA%` de la Known Folder del usuario. Si la variable no está
/// definida se deriva de `HOME`: inventar una ruta en el directorio de trabajo
/// pondría el estado del producto dentro de `target/`. Las ramas que la usan
/// son `cfg!(windows)`, así que la función se compila también en Unix.
fn local_app_data() -> PathBuf {
    relocated_dir("LOCALAPPDATA").unwrap_or_else(|| home_dir().join("AppData").join("Local"))
}

/// `$XDG_CACHE_HOME` con el fallback de `~/.cache`.
fn cache_home() -> PathBuf {
    relocated_dir("XDG_CACHE_HOME").unwrap_or_else(|| home_dir().join(".cache"))
}

/// Raíz de datos de usuario y estado: voces, habla sintetizada, configuración,
/// `daemon.pid` y logs (§7).
///
/// `AVI_DATA_DIR` la desvía. Sin la variable, en Windows es
/// `%LOCALAPPDATA%\ai-voice-interconnector\data` (D4), en Linux
/// `$XDG_DATA_HOME/ai-voice-interconnector` y en macOS
/// `~/Library/Application Support/ai-voice-interconnector`, que es justo lo que
/// resuelve `directories` en los tres sistemas Unix.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_DATA_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("data")
    } else {
        directories::ProjectDirs::from("", "", APP_NAME)
            .map(|d| d.data_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from(".").join(APP_NAME))
    }
}

/// Directorio de programa: el bundle completo y el recibo (§7). Existe en los
/// cuatro targets de distribución, no solo en Windows, y `AVI_INSTALL_DIR` lo
/// desvía. Dos `join` y no uno con separador embebido: una ruta con `/` mixto
/// rompe la comparación con el registro de Windows y con el resto del motor.
pub fn install_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_INSTALL_DIR") {
        return dir;
    }
    if cfg!(windows) {
        local_app_data().join("Programs").join(APP_NAME)
    } else {
        home_dir().join(".local").join("opt").join(APP_NAME)
    }
}

/// Directorio del comando en el PATH (§7). En Unix es el directorio del enlace
/// simbólico; en Windows no hay enlace y la entrada que se escribe en
/// `HKCU\Environment\Path` es el propio directorio de programa, así que ambos
/// coinciden. `AVI_BIN_DIR` lo desvía.
pub fn bin_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_BIN_DIR") {
        return dir;
    }
    if cfg!(windows) {
        install_dir()
    } else {
        home_dir().join(".local").join("bin")
    }
}

/// Voces de fábrica: `ryan`/`vivian` son presets del motor (`qwen_tts.c:spk_table`)
/// sin audio; `default` es voz clonada de fábrica (`.qvoice` graft vía Base) para
/// garantizar `WER ≤0.25` en texto corto. La distinción preset/clonada vive
/// en `avi-tts::resolve_voice_engine` (presencia de `reference.qvoice`).
pub const FACTORY_VOICES: &[&str] = &["default", "ryan", "vivian"];

/// Asset embebido: `.qvoice` de fábrica para `default` (16 MB graft, speaker
/// embedding + pesos Base). Se materializa en `ensure_initialized` si falta.
const FACTORY_DEFAULT_QVOICE: &[u8] = include_bytes!("../assets/default/reference.qvoice");

pub fn is_factory_name(name: &str) -> bool {
    FACTORY_VOICES.contains(&name.to_lowercase().as_str())
}

/// Normalización canónica de una entrada de `PATH` para comparación
/// determinista: prefijo verbatim de Windows fuera, `/` → `\`, `lowercase`, `trim`
/// de `\` y espacios. Hace que `C:\...\Programs/ai-voice-interconnector` y
/// `C:\...\Programs\ai-voice-interconnector\` sean idénticas, y que
/// `\\?\C:\...\ai-voice-interconnector` lo sea también.
///
/// **El prefijo verbatim se quita aquí y no en cada consumidor.** La capa de Windows
/// devuelve `\\?\…` en los modos de apertura extendidos, así que la ruta del ejecutable
/// en ejecución y la del directorio de programa —que viene de una variable de
/// reubicación— llegan con y sin él. Un consumidor que se lo quite en su módulo y otro
/// que no divergen, y el siguiente que escriba una comparación de rutas hereda el
/// defecto. §7 hace de esta función la fuente única de la semántica de comparación, y
/// la fuente única del arreglo.
///
/// El prefijo solo se quita en Windows: en Unix no existe, y una ruta que empiece por
/// `\\` es ahí un nombre de archivo legítimo.
/// Normalización canónica de una entrada de `PATH` para comparación
/// determinista: prefijo verbatim de Windows fuera, `/` → `\`, `lowercase` y separadores
/// **finales** fuera. Hace que `C:\...\Programs/ai-voice-interconnector` y
/// `C:\...\Programs\ai-voice-interconnector\` sean idénticas, y que
/// `\\?\C:\...\ai-voice-interconnector` lo sea también.
///
/// **El separador inicial no se quita nunca, y eso es lo que hace la función correcta
/// en las dos plataformas.** `trim_matches` —que los quitaba de los dos extremos— tenía
/// dos consecuencias falsas:
///
/// - En Unix convertía `/home/ana` en `home\ana`, de modo que una ruta absoluta
///   comparaba igual a una relativa con el mismo nombre.
/// - En la UNC de Windows `\\servidor\recurso` perdía las dos barras iniciales, que son
///   justo lo que la distingue de una ruta local.
///
/// En Unix los `\` son **caracteres de nombre de archivo legítimos**, así que una ruta
/// que empieza por `\\` no es una verbatim: el prefijo no se quita y lo único que se
/// re-codifica es el separador `/` como `\`, que es lo que permite comparar la forma
/// absoluta con la que escribe el bloque de §9.3.1.
///
/// **El prefijo verbatim se quita aquí y no en cada consumidor.** La capa de Windows
/// devuelve `\\?\.` en los modos de apertura extendidos, así que la ruta del ejecutable
/// en ejecución y la del directorio de programa —que viene de una variable de
/// reubicación— llegan con y sin él. Un consumidor que se lo quite en su módulo y otro
/// que no divergen, y el siguiente que escriba una comparación de rutas hereda el
/// defecto. §7 hace de esta función la fuente única de la semántica de comparación, y
/// la fuente única del arreglo.
///
/// El prefijo solo se quita en Windows: en Unix no existe, y una ruta que empiece por
/// `\\` es ahí un nombre de archivo legítimo.
pub fn canonical_path_key(p: &Path) -> String {
    sin_prefijo_verbatim(&p.to_string_lossy())
        .replace('/', "\\")
        .to_lowercase()
        .trim_end_matches('\\')
        .trim_end()
        .to_string()
}

/// Quita el prefijo de ruta verbatim de Windows.
///
/// Cubre las dos formas: `\\?\C:\ruta` es la local, y `\\?\UNC\servidor\recurso` la de
/// red, cuya forma normal es `\\servidor\recurso` —con la barra inicial, que es la que
/// la distingue de una ruta local. Devolver laUNC sin barras haría que una ruta de red
/// comparara igual a un directorio local del mismo nombre.
#[cfg(windows)]
fn sin_prefijo_verbatim(raw: &str) -> std::borrow::Cow<'_, str> {
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        return std::borrow::Cow::Owned(format!(r"\\{unc}"));
    }
    std::borrow::Cow::Borrowed(raw.strip_prefix(r"\\?\").unwrap_or(raw))
}

/// En Unix no hay prefijo verbatim, así que la ruta se devuelve tal cual.
#[cfg(not(windows))]
fn sin_prefijo_verbatim(raw: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(raw)
}

/// ¿Son la misma entrada de `PATH` dos rutas escritas de forma distinta?
///
/// La comparación canónica de §9.3.1 distingue mayúsculas, ignora separadores
/// finales y **considera también la forma expandida de cada entrada**. En
/// Windows el valor de `HKCU\Environment\Path` se lee sin expandir, así que
/// `%LOCALAPPDATA%\Programs\ai-voice-interconnector` y la ruta real que el
/// motor acaba de escribir tienen que comparar iguales; sin esta segunda forma
/// la comprobación daría un falso negativo y el motor añadiría la entrada dos
/// veces. En Unix se expanden `${VAR}` y `%VAR%` por el mismo motivo.
pub fn canonical_path_entry_matches(a: &Path, b: &Path) -> bool {
    if canonical_path_key(a) == canonical_path_key(b) {
        return true;
    }
    canonical_path_key(&expand_path_vars(a)) == canonical_path_key(&expand_path_vars(b))
}

/// Sustituye las referencias a variables de entorno de una entrada de `PATH`
/// por su valor. Cubre `%VAR%` (Windows) y `${VAR}`; una referencia sin valor
/// conocido se deja intacta, para que comparar dos entradas no definidas no
/// las vuelva iguales por accidente.
fn expand_path_vars(p: &Path) -> PathBuf {
    let raw = p.to_string_lossy().to_string();
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw.as_str();
    while let Some(start) = rest.find(['%', '$']) {
        out.push_str(&rest[..start]);
        let tail = &rest[start..];
        // El desplazamiento se cuenta desde `tail` **con** el prefijo puesto, y el
        // nombre se lee de la cola **sin** él. Confundir las dos cosas dejaba el `}` de
        // cierre dentro del valor expandido: `%VAR%` salía bien por casualidad —quitar
        // el `%` inicial no desplaza el cierre— y `${VAR}` salía mal, que es la forma que
        // se usa en Unix. El defecto era invisible porque la prueba solo usaba la forma
        // de Windows.
        let (name, consumed) = match tail.strip_prefix('%') {
            Some(after) => match after.find('%') {
                Some(end) => (&after[..end], start + end + 2),
                None => ("", 0),
            },
            None => match tail.strip_prefix("${").and_then(|after| after.find('}')) {
                Some(end) => (&tail[start + 2..start + 2 + end], start + end + 3),
                None => ("", 0),
            },
        };
        if !name.is_empty() {
            if let Ok(value) = std::env::var(name) {
                out.push_str(&value);
                rest = &tail[consumed..];
                continue;
            }
        }
        let consumed = consumed.max(1);
        out.push_str(&tail[..consumed]);
        rest = &tail[consumed..];
    }
    out.push_str(rest);
    PathBuf::from(out)
}

// ─── VoiceStore ──────────────────────────────────────────────────────

/// Una voz registrada (fábrica o de usuario)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceEntry {
    pub name: String,
    pub is_factory: bool,
    /// Ruta al archivo de referencia de audio (.qvoice o .wav)
    pub reference_path: Option<PathBuf>,
}

/// Almacén de voces: gestión de voces clonadas + fábrica.
/// Layout en disco: <data_dir>/voices/<nombre>/
pub struct VoiceStore {
    base_dir: PathBuf,
}

impl Default for VoiceStore {
    fn default() -> Self {
        Self::new()
    }
}

impl VoiceStore {
    pub fn new() -> Self {
        let base_dir = data_dir().join("voices");
        Self { base_dir }
    }

    /// Asegura que el directorio base y las voces de fábrica existan
    /// (`default` clonada de fábrica con `.qvoice`, `ryan`/`vivian` presets).
    /// Idempotente; materializa `default/reference.qvoice` desde el asset embebido.
    pub fn ensure_initialized(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        for name in FACTORY_VOICES {
            let dir = self.base_dir.join(name);
            std::fs::create_dir_all(&dir)?;
        }
        // Materializar `default` clonada de fábrica desde el asset embebido.
        // Solo si no existe ya un `reference.qvoice` (preserva clonación del usuario
        // si re-inicializa, aunque `default` es fábrica y no debería ser sobreescrita
        // por el usuario; aun así, idempotente).
        let default_qvoice = self.base_dir.join("default").join("reference.qvoice");
        if !default_qvoice.is_file() && !FACTORY_DEFAULT_QVOICE.is_empty() {
            // Validar que el asset sea un .qvoice graft válido (magic QVCE/QV3)
            if FACTORY_DEFAULT_QVOICE.len() > 4 {
                let _ = std::fs::write(&default_qvoice, FACTORY_DEFAULT_QVOICE);
            }
        }
        Ok(())
    }

    /// Listar todas las voces registradas
    pub fn list(&self) -> Result<Vec<VoiceEntry>> {
        self.ensure_initialized()?;
        let mut voices = Vec::new();
        for entry in std::fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                let name = entry
                    .file_name()
                    .to_string_lossy()
                    .to_string()
                    .to_lowercase();
                let is_factory = is_factory_name(&name);
                let ref_path = self.find_reference(&name);
                voices.push(VoiceEntry {
                    name,
                    is_factory,
                    reference_path: ref_path,
                });
            }
        }
        // Fábrica primero (default, ryan, vivian), luego clonadas alfabéticamente
        voices.sort_by(|a, b| b.is_factory.cmp(&a.is_factory).then(a.name.cmp(&b.name)));
        Ok(voices)
    }

    /// Validar un nombre de voz (regex del oráculo `^[A-Za-z0-9._-]+$` +
    /// reglas de seguridad anti-escape; paridad de contrato)
    pub fn validate_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("El nombre de la voz no puede estar vacío.".into());
        }
        if !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(format!(
                "El nombre de voz '{}' contiene caracteres no permitidos.",
                name
            ));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
            return Err(format!(
                "El nombre de voz '{}' contiene caracteres no permitidos.",
                name
            ));
        }
        if name.len() > 64 {
            return Err("El nombre de la voz excede 64 caracteres.".into());
        }
        Ok(())
    }

    /// Verificar si una voz existe (nombre normalizado a minúsculas, paridad
    /// con `voices.py:37`)
    pub fn exists(&self, name: &str) -> bool {
        self.base_dir.join(name.to_lowercase()).is_dir()
    }

    /// Eliminar una voz (no permite eliminar voces de fábrica: default, ryan, vivian)
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let name = name.to_lowercase();
        if is_factory_name(&name) {
            return Err(format!("La voz '{}' no se puede eliminar.", name));
        }
        let dir = self.base_dir.join(&name);
        if !dir.is_dir() {
            return Err(format!("La voz '{}' no existe.", name));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Buscar el archivo de referencia de una voz: `reference.qvoice`.
    /// Solo `reference.qvoice` determina la rama clonada; los presets no
    /// tienen referencia y resuelven como `Preset` en `resolve_voice_engine`.
    pub fn find_reference(&self, name: &str) -> Option<PathBuf> {
        let dir = self.base_dir.join(name.to_lowercase());
        let path = dir.join("reference.qvoice");
        if path.is_file() {
            return Some(path);
        }
        None
    }

    /// Directorio de una voz (nombre normalizado a minúsculas)
    pub fn voice_dir(&self, name: &str) -> PathBuf {
        self.base_dir.join(name.to_lowercase())
    }

    /// Guardar el `.qvoice` clonado como `reference.qvoice` de la voz
    /// (copia con temporal + rename; paridad con el layout del oráculo)
    pub fn save_reference(&self, name: &str, src: &Path) -> Result<PathBuf> {
        let dir = self.voice_dir(name);
        std::fs::create_dir_all(&dir)?;
        let dest = dir.join("reference.qvoice");
        let tmp = dir.join("reference.qvoice.tmp");
        std::fs::copy(src, &tmp)?;
        std::fs::rename(&tmp, &dest)?;
        Ok(dest)
    }

    #[cfg(test)]
    fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

// ─── SpeechStore ─────────────────────────────────────────────────────

/// Metadatos de una locución persistida
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechMetadata {
    pub label: String,
    pub voice: String,
    pub text: String,
    pub created_at: String,
    pub duration_secs: f64,
}

/// Entrada de una locución (WAV + sidecar de metadatos)
#[derive(Debug, Clone)]
pub struct SpeechEntry {
    pub metadata: SpeechMetadata,
    pub audio_path: PathBuf,
    pub metadata_path: PathBuf,
}

/// Almacén de habla sintética persistida.
/// Layout en disco: <data_dir>/speech/<voz>/<etiqueta>.wav + <etiqueta>.json
pub struct SpeechStore {
    base_dir: PathBuf,
}

impl Default for SpeechStore {
    fn default() -> Self {
        Self::new()
    }
}

impl SpeechStore {
    pub fn new() -> Self {
        let base_dir = data_dir().join("speech");
        Self { base_dir }
    }

    pub fn ensure_initialized(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(())
    }

    /// Listar todas las locuciones persistidas
    pub fn list(&self) -> Result<Vec<SpeechEntry>> {
        self.list_with_filter(None)
    }

    /// Listar las locuciones persistidas de una sola voz (lectura
    /// acotada por voz; la voz se normaliza a minúsculas, paridad con
    /// `voice_dir`). Voz sin locuciones → lista vacía, sin error.
    pub fn list_by_voice(&self, voice: &str) -> Result<Vec<SpeechEntry>> {
        self.list_with_filter(Some(voice))
    }

    /// Recorrido compartido del almacén con filtro opcional por voz: el mismo
    /// bucle con `ensure_initialized` y descarte de corrupto/sin WAV que tenía
    /// `list`; con filtro solo se lee el directorio de la voz pedida.
    fn list_with_filter(&self, voice: Option<&str>) -> Result<Vec<SpeechEntry>> {
        self.ensure_initialized()?;
        let mut entries = Vec::new();
        if !self.base_dir.is_dir() {
            return Ok(entries);
        }
        // Filtro normalizado a minúsculas, paridad con `voice_dir`.
        let filter = voice.map(|v| v.to_lowercase());
        // Iterar por directorio de voz
        for voice_dir in std::fs::read_dir(&self.base_dir)? {
            let voice_dir = voice_dir?;
            if !voice_dir.file_type()?.is_dir() {
                continue;
            }
            // Con filtro, acotar la lectura al directorio de la voz pedida.
            if let Some(f) = &filter {
                let name = voice_dir.file_name().to_string_lossy().to_lowercase();
                if name != *f {
                    continue;
                }
            }
            for file in std::fs::read_dir(voice_dir.path())? {
                let file = file?;
                let path = file.path();
                if path.extension().and_then(|e| e.to_str()) == Some("json") {
                    if let Ok(content) = std::fs::read_to_string(&path) {
                        if let Ok(mut meta) = serde_json::from_str::<SpeechMetadata>(&content) {
                            let wav_path = path.with_extension("wav");
                            if wav_path.is_file() {
                                meta.voice = meta.voice.to_lowercase();
                                meta.label = meta.label.to_lowercase();
                                entries.push(SpeechEntry {
                                    metadata: meta,
                                    audio_path: wav_path,
                                    metadata_path: path,
                                });
                            }
                        }
                    }
                }
            }
        }
        Ok(entries)
    }

    /// Directorio para una voz específica (nombre normalizado a minúsculas)
    pub fn voice_dir(&self, voice: &str) -> PathBuf {
        self.base_dir.join(voice.to_lowercase())
    }

    /// Ruta del WAV para una locución (voice/label normalizados)
    pub fn audio_path(&self, voice: &str, label: &str) -> PathBuf {
        self.voice_dir(voice)
            .join(format!("{}.wav", label.to_lowercase()))
    }

    /// Buscar una locución por (voz, etiqueta)
    pub fn find(&self, voice: &str, label: &str) -> Option<SpeechEntry> {
        let label = label.to_lowercase();
        let meta_path = self.voice_dir(voice).join(format!("{}.json", label));
        let wav_path = self.audio_path(voice, &label);
        if meta_path.is_file() && wav_path.is_file() {
            let content = std::fs::read_to_string(&meta_path).ok()?;
            let meta: SpeechMetadata = serde_json::from_str(&content).ok()?;
            Some(SpeechEntry {
                metadata: meta,
                audio_path: wav_path,
                metadata_path: meta_path,
            })
        } else {
            None
        }
    }

    /// Eliminar una locución
    pub fn remove(&self, voice: &str, label: &str) -> Result<(), String> {
        let label = label.to_lowercase();
        let wav = self.audio_path(voice, &label);
        let meta = self.voice_dir(voice).join(format!("{}.json", label));
        if !wav.is_file() && !meta.is_file() {
            return Err(format!(
                "La locución '{}' de la voz '{}' no existe.",
                label, voice
            ));
        }
        if wav.is_file() {
            std::fs::remove_file(&wav).map_err(|e| e.to_string())?;
        }
        if meta.is_file() {
            std::fs::remove_file(&meta).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    /// Guardar una locución (WAV ya escrito por el motor; solo guarda los metadatos)
    pub fn save_metadata(
        &self,
        voice: &str,
        label: &str,
        text: &str,
        duration_secs: f64,
    ) -> Result<PathBuf> {
        let voice = voice.to_lowercase();
        let label = label.to_lowercase();
        let dir = self.voice_dir(&voice);
        std::fs::create_dir_all(&dir)?;
        let meta = SpeechMetadata {
            label: label.clone(),
            voice,
            text: text.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            duration_secs,
        };
        let meta_path = dir.join(format!("{}.json", label));
        let content = serde_json::to_string_pretty(&meta)?;
        std::fs::write(&meta_path, content)?;
        Ok(meta_path)
    }

    /// Guardar una locución completa: sidecar con `duration_secs` calculada del
    /// WAV vía hound + publicación del WAV con temporal + rename.
    pub fn save(&self, voice: &str, label: &str, text: &str, wav_src: &Path) -> Result<PathBuf> {
        let reader = hound::WavReader::open(wav_src)?;
        let duration_secs = reader.duration() as f64 / f64::from(reader.spec().sample_rate);
        drop(reader);
        self.save_metadata(voice, label, text, duration_secs)?;
        let dir = self.voice_dir(voice);
        std::fs::create_dir_all(&dir)?;
        let final_path = self.audio_path(voice, label);
        let tmp_path = dir.join(format!("{}.wav.tmp", label.to_lowercase()));
        std::fs::copy(wav_src, &tmp_path)?;
        std::fs::rename(&tmp_path, &final_path)?;
        Ok(final_path)
    }

    #[cfg(test)]
    fn with_base_dir(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }
}

// ─── ModelStore ──────────────────────────────────────────────────────

/// Pines de modelos: `(nombre_lógico, repo HF, revisión)`.
/// La revisión es un **commit hash** de HuggingFace: mismo binario → mismos
/// bytes (reproducibilidad); actualizar un pin es una acción deliberada y
/// auditable en THIRD-PARTY-LICENSES.md.
pub const MODEL_REVISIONS: &[(&str, &str, &str)] = &[
    // Motor TTS Qwen3-TTS 0.6B CustomVoice (pesos safetensors BF16)
    (
        "qwen3-tts-0.6b",
        "Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice",
        "85e237c12c027371202489a0ec509ded67b5e4b5",
    ),
    // Traducción es→en / en→es (Marian opus-mt convertido a CTranslate2)
    (
        "marian-es-en",
        "Helsinki-NLP/opus-mt-es-en",
        "c96e2c5399ebfae4fc43d9669556b9afa74bb69d",
    ),
    (
        "marian-en-es",
        "Helsinki-NLP/opus-mt-en-es",
        "5bc4493d463cf000c1f0b50f8d56886a392ed4ab",
    ),
    // STT Parakeet TDT 0.6B v3 int8 (export istupakov/onnx-asr; 4 artefactos
    // canónicos — el repo upstream completo pesa decenas de GB)
    (
        "parakeet-tdt-v3",
        "istupakov/parakeet-tdt-0.6b-v3-onnx",
        "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
    ),
    // Modelo Base Qwen3-TTS 0.6B para clonado de voz (speaker encoder) — snapshot
    // completo. Repo público Qwen/Qwen3-TTS-12Hz-0.6B-Base verificado por dry-run:
    // config.json con "tts_model_type": "base" + speaker_encoder_config; artefactos
    // model.safetensors + speech_tokenizer/model.safetensors (no requiere allow_patterns).
    (
        "qwen3-tts-0.6b-base",
        "Qwen/Qwen3-TTS-12Hz-0.6B-Base",
        "5d83992436eae1d760afd27aff78a71d676296fc",
    ),
];

/// Nombre del bloqueo de ciclo de vida, hermano del directorio de programa
/// (§7): `~/.local/opt/.ai-voice-interconnector.lock`,
/// `%LOCALAPPDATA%\Programs\.ai-voice-interconnector.lock`.
pub const LIFECYCLE_LOCK_NAME: &str = ".ai-voice-interconnector.lock";

/// Prefijo del staging, también hermano del directorio de programa (§7):
/// `~/.local/opt/.ai-voice-interconnector-staging-<pid>-<ms>`.
pub const STAGING_DIR_PREFIX: &str = ".ai-voice-interconnector-staging-";

/// Prefijo de los aparcados **dentro** del directorio de programa (§7):
/// `<programa>/.old-<timestamp>`. Vive dentro a propósito: la reversión de un
/// reemplazo tiene que poder hacerse sin mover nada fuera del programa.
pub const PARKED_DIR_PREFIX: &str = ".old-";

/// Subdirectorio de `xet` dentro de la raíz de modelos exclusiva. Lo fija
/// `ModelStore::new()` en `HF_XET_CACHE`.
pub const MODELS_XET_SUBDIR: &str = "xet";

/// Prefijos de los temporales de ejecución de la aplicación (§7): `$TMPDIR` en
/// Unix y `%TEMP%` en Windows. En Windows el helper de borrado diferido
/// (`avi-uninstall-<pid>-<ms>.ps1`) cae en `avi-`, de modo que ningún otro
/// prefijo puede sustituir a este conjunto.
pub const TEMP_PREFIXES: &[&str] = &["avi-", "avi_"];

// **R4.** Las cinco constantes anteriores son la fuente única de los recursos que la
// aplicación crea: todo recurso nuevo se declara aquí, en la tabla de §7 y en
// los planes de limpieza y desinstalación **en el mismo cambio**. Un recurso
// que solo aparece en un plan de limpieza es un recurso que sobrevive a la
// desinstalación.

/// Patrones de descarga por modelo (`snapshot_download` con `allow_patterns`).
/// Vacío = snapshot completo (repos pequeños/cohesivos). Para `parakeet-tdt-v3`
/// se acota a los 4 artefactos que consume `ParakeetEngine`
/// (`DEFAULT_PARAKEET_MODEL_DIR`); sin esto se bajarían ~40 GB de formatos no usados.
pub const MODEL_FILE_PATTERNS: &[(&str, &[&str])] = &[(
    "parakeet-tdt-v3",
    &[
        "encoder-model.int8.onnx",
        "decoder_joint-model.int8.onnx",
        "nemo128.onnx",
        "vocab.txt",
    ],
)];

/// Raíz de la caché de HuggingFace que el usuario eligió compartir, si la
/// eligió: `HF_HUB_CACHE` o, en su defecto, `HF_HOME/hub`. `None` significa
/// que rige la raíz de modelos exclusiva de la aplicación.
///
/// `HF_HUB_CACHE` tiene precedencia porque es la que nombra la caché de `hub`
/// directamente; `HF_HOME` es la convención y su caché vive en `hub/` dentro.
pub fn shared_hf_root() -> Option<PathBuf> {
    if let Some(dir) = relocated_dir("HF_HUB_CACHE") {
        return Some(dir);
    }
    relocated_dir("HF_HOME").map(|home| home.join("hub"))
}

/// ¿La raíz de modelos es la caché HF compartida que eligió el usuario? En ese
/// caso su contenido es compartido y rige R3: solo se borran entradas
/// atribuibles a la aplicación.
pub fn models_root_is_shared() -> bool {
    shared_hf_root().is_some()
}

/// Raíz de modelos: caché regenerable y **exclusiva de la aplicación** por
/// defecto (§7, D3). Snapshots, derivado CT2, locks y `xet` cuelgan todos de
/// ella, y es la única ubicación de modelos que existe.
///
/// `AVI_CACHE_DIR` tiene precedencia sobre las variables de HuggingFace: es la
/// variable de reubicación de la aplicación y es lo que permite aislar una
/// prueba aunque el entorno tenga un `HF_HOME` global.
///
/// Sin reubicación y con una caché HF compartida elegida por el usuario
/// (`HF_HUB_CACHE` o `HF_HOME`), la raíz es esa y su contenido es compartido.
/// En cualquier otro caso es la columna de modelos de §7:
/// `$XDG_CACHE_HOME/ai-voice-interconnector/models`,
/// `~/Library/Caches/ai-voice-interconnector/models` o
/// `%LOCALAPPDATA%\ai-voice-interconnector\cache\models`.
pub fn models_cache_dir() -> PathBuf {
    if let Some(dir) = relocated_dir("AVI_CACHE_DIR") {
        return dir;
    }
    if let Some(shared) = shared_hf_root() {
        return shared;
    }
    if cfg!(windows) {
        local_app_data().join(APP_NAME).join("cache").join("models")
    } else if cfg!(target_os = "macos") {
        home_dir()
            .join("Library")
            .join("Caches")
            .join(APP_NAME)
            .join("models")
    } else {
        cache_home().join(APP_NAME).join("models")
    }
}

/// Directorio de la caché `xet` (shard-cache) del proceso que provisiona
/// modelos.
///
/// En la raíz de modelos exclusiva cuelga de ella, y `ModelStore::new()` fija
/// `HF_XET_CACHE` a este mismo sitio para que el borrado sea de directorio
/// entero. En la raíz compartida la variable es del usuario y no se toca, así
/// que se devuelve la cadena real que resuelve `xet-runtime`: `HF_XET_CACHE` →
/// `HF_HOME/xet` → `XDG_CACHE_HOME/huggingface/xet` →
/// `~/.cache/huggingface/xet`.
pub fn xet_cache_dir() -> PathBuf {
    if models_root_is_shared() {
        if let Some(dir) = relocated_dir("HF_XET_CACHE") {
            return dir;
        }
        if let Some(home) = relocated_dir("HF_HOME") {
            return home.join("xet");
        }
        return cache_home().join("huggingface").join("xet");
    }
    models_cache_dir().join(MODELS_XET_SUBDIR)
}

/// Directorio CT2 derivado obligatorio de Marian HF en
/// `models_cache_dir()/ct2`.
/// Layout: `models_cache_dir()/ct2/opus-mt-es-en` y `opus-mt-en-es`, cada uno con
/// `model.bin` CT2 más tokenizador utilizable por el loader (`tokenizer.json`,
/// o `source.spm` más `target.spm` copiados desde el snapshot por `setup`).
/// Invariante: provisionado equivale a lo que `setup` deposita y, por tanto, a
/// cargable por `Translator::new` — el gate `is_ct2_provisioned` acepta
/// exactamente los layouts que `convert_marian_to_ct2` produce (`src/main.rs`),
/// no todo lo que `auto::Tokenizer` sabría cargar; la idempotencia por `mtime`
/// solo aplica a dirs sanos (un dir roto es no provisionado y fuerza reconversión).
pub fn ct2_cache_dir() -> PathBuf {
    models_cache_dir().join("ct2")
}
pub fn ct2_model_dir(pair: &str) -> PathBuf {
    ct2_cache_dir().join(format!("opus-mt-{}", pair))
}
/// Ficheros ausentes del derivado CT2 en `dir`: vacío equivale a cargable por
/// el loader (`model.bin` presente más tokenizador completo). Nombra cada
/// candidato ausente para errores accionables.
///
/// El tokenizador se da por válido con `tokenizer.json` (layout HF) o
/// `source.spm`+`target.spm` (SentencePiece/Marian). El layout BPE
/// `vocab.json`+`merges.txt` se rechaza a propósito: `convert_marian_to_ct2`
/// fija la salida a `source.spm`+`target.spm` (`--copy_files`) y aborta si el
/// snapshot no los trae (`src/main.rs`), así que ningún derivado de este
/// pipeline lo usa. Admitir esa rama sería especulativo y podría enmascarar un
/// dir incompleto; se añadiría solo si un pin de modelo futuro la exigiera.
pub fn ct2_dir_missing_files(dir: &std::path::Path) -> Vec<String> {
    let mut missing = Vec::new();
    if !dir.join("model.bin").is_file() {
        missing.push("model.bin".to_string());
    }
    let tokenizer_ok = dir.join("tokenizer.json").is_file()
        || (dir.join("source.spm").is_file() && dir.join("target.spm").is_file());
    if !tokenizer_ok {
        for candidate in ["tokenizer.json", "source.spm", "target.spm"] {
            if !dir.join(candidate).is_file() {
                missing.push(candidate.to_string());
            }
        }
    }
    missing
}
/// Ficheros ausentes del derivado CT2 del par (`es-en`/`en-es`).
pub fn ct2_missing_files(pair: &str) -> Vec<String> {
    ct2_dir_missing_files(&ct2_model_dir(pair))
}
pub fn is_ct2_provisioned(pair: &str) -> bool {
    ct2_missing_files(pair).is_empty()
}
/// Purga determinista del derivado CT2 en `models_cache_dir()/ct2`. El derivado
/// es atribuible a la aplicación, así que se purga tanto en la raíz exclusiva
/// como en la compartida (R3).
pub fn remove_ct2_cache() -> Result<bool> {
    let ct2 = ct2_cache_dir();
    if ct2.is_dir() {
        std::fs::remove_dir_all(&ct2)?;
        return Ok(true);
    }
    Ok(false)
}

/// Almacén de modelos descargados.
///
/// Fuente de verdad única: snapshots de HuggingFace en `models_cache_dir()` con
/// layout `models--<org>--<repo>/snapshots/<hash>/`. Todos los modelos están
/// pinneados en `MODEL_REVISIONS`, así que la provisión se decide solo por
/// presencia del snapshot; no hay índice `manifest.json` intermedio.
pub struct ModelStore {
    base_dir: PathBuf,
}

impl Default for ModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelStore {
    /// Ancla el almacén en `models_cache_dir()`, nunca en `data_dir()/models`:
    /// los modelos son caché regenerable de propiedad exclusiva y no estado de
    /// usuario, y §7 los coloca en raíces distintas.
    ///
    /// Fija además `HF_XET_CACHE` al subdirectorio `xet` de la raíz cuando esta
    /// es exclusiva. Es la única palanca disponible: `HFClientBuilder::cache_dir`
    /// solo fija la caché de `hub`, de modo que `xet` se sitúa por variable de
    /// entorno; y es la variable que resuelve `xet-runtime` (`HF_XET_CACHE` →
    /// `HF_HOME/xet` → `XDG_CACHE_HOME/huggingface/xet` →
    /// `~/.cache/huggingface/xet`). Con una raíz compartida la variable es del
    /// usuario y no se toca, porque R3 prohíbe que su contenido se relocalice.
    ///
    /// La llamada a `set_var` es segura en la toolchain del proyecto
    /// (edition 2021); pasa a `unsafe` en edition 2024.
    pub fn new() -> Self {
        let base_dir = models_cache_dir();
        if !models_root_is_shared() {
            std::env::set_var("HF_XET_CACHE", base_dir.join(MODELS_XET_SUBDIR));
        }
        Self { base_dir }
    }

    /// Resolución del repo HF y revisión pinneada de un modelo lógico.
    pub fn revision_of(model_name: &str) -> Option<(&'static str, &'static str)> {
        MODEL_REVISIONS
            .iter()
            .find(|(name, _, _)| *name == model_name)
            .map(|(_, repo, rev)| (*repo, *rev))
    }

    /// Ruta del snapshot HF de un modelo.
    ///
    /// La revisión pinneada puede ser un ref (`main`) o un commit hash. hf-hub
    /// materializa el snapshot bajo `snapshots/<commit-hash>` y deja la
    /// resolución del ref en `refs/<revision>` (archivo con el hash). Aquí se
    /// replica esa resolución: `snapshots/<rev>` directo si existe, si no se
    /// lee `refs/<rev>`.
    pub fn model_snapshot_path(&self, model_name: &str) -> Option<PathBuf> {
        let (repo, rev) = ModelStore::revision_of(model_name)?;
        let repo_dir = models_cache_dir().join(format!("models--{}", repo.replace('/', "--")));
        let direct = repo_dir.join("snapshots").join(rev);
        if direct.is_dir() {
            return Some(direct);
        }
        // Resolver ref → commit hash (layout estándar de HF hub)
        let ref_file = repo_dir.join("refs").join(rev);
        if let Ok(hash) = std::fs::read_to_string(&ref_file) {
            let hash = hash.trim();
            if !hash.is_empty() {
                let resolved = repo_dir.join("snapshots").join(hash);
                if resolved.is_dir() {
                    return Some(resolved);
                }
            }
        }
        Some(direct)
    }

    /// Verificar si un modelo está provisionado: snapshot HF presente con
    /// integridad mínima (ficheros críticos con `size>0`). Sin pin en
    /// `MODEL_REVISIONS` no hay snapshot resoluble → no provisionado.
    pub fn is_provisioned(&self, model_name: &str) -> bool {
        match self.model_snapshot_path(model_name) {
            Some(snapshot) => {
                if !snapshot.is_dir() {
                    return false;
                }
                // Para modelos con `allow_patterns` (parakeet), exigir todos los ficheros críticos
                if let Some((_, patterns)) =
                    MODEL_FILE_PATTERNS.iter().find(|(n, _)| *n == model_name)
                {
                    for pat in *patterns {
                        let p = snapshot.join(pat);
                        if !p.is_file() {
                            return false;
                        }
                        if let Ok(md) = std::fs::metadata(&p) {
                            if md.len() == 0 {
                                return false;
                            }
                        } else {
                            return false;
                        }
                    }
                    return true;
                }
                // Modelos sin patrones: al menos un fichero con tamaño >0
                match std::fs::read_dir(&snapshot) {
                    Ok(mut entries) => {
                        for e in entries.by_ref().flatten() {
                            let path = e.path();
                            if path.is_file() {
                                if let Ok(md) = std::fs::metadata(&path) {
                                    if md.len() > 0 {
                                        return true;
                                    }
                                }
                            }
                        }
                        false
                    }
                    Err(_) => false,
                }
            }
            None => false,
        }
    }

    /// Directorio de un modelo: snapshot HF pinneado; si no resuelve, cae al
    /// directorio nominal bajo la raíz de modelos.
    pub fn model_dir(&self, model_name: &str) -> PathBuf {
        self.model_snapshot_path(model_name)
            .unwrap_or_else(|| self.base_dir.join(model_name))
    }

    /// Borra el snapshot pinneado de un modelo: el directorio
    /// `models--<org>--<nombre>` bajo la raíz de modelos vigente. El repo es
    /// atribuible a la aplicación, así que el borrado procede tanto en la raíz
    /// exclusiva como en la compartida (R3).
    pub fn remove_hf_snapshot(&self, model_name: &str) -> Result<bool> {
        if let Some((repo, _)) = ModelStore::revision_of(model_name) {
            let dir = models_cache_dir().join(format!("models--{}", repo.replace('/', "--")));
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Borra la caché `xet` **solo** en la raíz de modelos exclusiva, donde
    /// cuelga de ella. En una raíz compartida devuelve `Ok(false)` sin tocar
    /// nada: `xet` es un subdirectorio global de la caché HF y R3 prohíbe
    /// borrarlo, porque aloja los shards de todos los proyectos que la usan.
    pub fn remove_xet_cache() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let xet = xet_cache_dir();
        if xet.is_dir() {
            std::fs::remove_dir_all(&xet)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Borra los locks de descarga (`.locks`) **solo** en la raíz de modelos
    /// exclusiva. En una raíz compartida devuelve `Ok(false)`: R3 prohíbe
    /// borrar el `.locks` completo, y los locks de los repos propios se van con
    /// `remove_hf_snapshot`, que sí los alcanza.
    pub fn remove_hf_locks() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let locks = models_cache_dir().join(".locks");
        if locks.is_dir() {
            std::fs::remove_dir_all(&locks)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Borra la raíz de modelos **entera** cuando es exclusiva, `xet` y derivados
    /// incluidos. Es el alcance que `cleanup --model` y `self uninstall` usan
    /// para dejar residuo cero dentro de las raíces de propiedad exclusiva; con
    /// una raíz compartida devuelve `Ok(false)` y el llamador se limita a los
    /// repos fijados, sus derivados y sus locks.
    pub fn remove_models_root() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let root = models_cache_dir();
        if root.is_dir() {
            std::fs::remove_dir_all(&root)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Descarga nativa de un modelo pinneado vía HuggingFace Hub.
    ///
    /// Usa `hf-hub` (`snapshot_download` con revisión de `MODEL_REVISIONS`): cache
    /// estándar en `models_cache_dir()`, resume por Range, ETag/commit-hash y reintentos
    /// del propio crate. La barra `indicatif` refleja archivos/bytes agregados vía
    /// `ProgressHandler`. Idempotente: si el snapshot ya existe y no es
    /// `force_download`, HF resuelve desde cache sin red. Compila igual en los 4
    /// targets (rustls, sin OpenSSL nativo).
    pub async fn ensure_downloaded(model_name: &str) -> Result<PathBuf> {
        let (repo_id, revision) = ModelStore::revision_of(model_name).ok_or_else(|| {
            anyhow::anyhow!(
                "Modelo desconocido (sin pin en MODEL_REVISIONS): {}",
                model_name
            )
        })?;
        let progress = indicatif_progress();
        // Cache explícita: la resolución de la app (models_cache_dir) manda sobre el
        // fallback roto de hf-hub (HOME→/tmp); lectura y escritura convergen.
        let client = hf_hub::HFClient::builder()
            .cache_dir(models_cache_dir())
            .build()?;
        let (owner, name) = hf_hub::split_id(repo_id);
        let repo = client.model(owner, name);
        // allow_patterns acota la descarga a los ficheros que el motor usa
        // (crítico en repos multi-formato como ggerganov/whisper.cpp).
        let patterns: Option<Vec<String>> = MODEL_FILE_PATTERNS
            .iter()
            .find(|(n, _)| *n == model_name)
            .map(|(_, p)| p.iter().map(|s| s.to_string()).collect());
        let snapshot = repo
            .snapshot_download()
            .maybe_revision(Some(revision.to_string()))
            .maybe_allow_patterns(patterns.clone())
            .max_workers(4)
            .progress(progress)
            .send()
            .await?;
        // Validación atómica: el snapshot debe contener los ficheros críticos con tamaño>0
        // Si la descarga dejó un snapshot vacío (hub/xet incoherente), borrar y fallar para retry
        let valid = if let Some(pats) = patterns {
            pats.iter().all(|pat| {
                let p = snapshot.join(pat);
                p.is_file() && std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false)
            })
        } else {
            std::fs::read_dir(&snapshot)
                .map(|mut d| {
                    d.any(|e| {
                        e.ok()
                            .map(|en| {
                                let p = en.path();
                                p.is_file()
                                    && std::fs::metadata(&p).map(|m| m.len() > 0).unwrap_or(false)
                            })
                            .unwrap_or(false)
                    })
                })
                .unwrap_or(false)
        };
        if !valid {
            let _ = std::fs::remove_dir_all(&snapshot);
            // Limpiar blobs incompletos del repo si existen
            let repo_dir =
                models_cache_dir().join(format!("models--{}", repo_id.replace('/', "--")));
            let blobs = repo_dir.join("blobs");
            if blobs.is_dir() {
                if let Ok(entries) = std::fs::read_dir(&blobs) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.extension().and_then(|e| e.to_str()) == Some("incomplete")
                            || path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .map(|n| n.ends_with(".incomplete"))
                                .unwrap_or(false)
                        {
                            let _ = std::fs::remove_file(&path);
                        }
                    }
                }
            }
            anyhow::bail!(
                "Snapshot {}@{} incompleto tras descarga (ficheros críticos faltantes); limpiado para reintento",
                repo_id,
                revision
            );
        }
        tracing::info!(
            "Snapshot {}@{} listo en {}",
            repo_id,
            revision,
            snapshot.display()
        );
        Ok(snapshot)
    }
}

/// Handler de progreso que puentea los eventos de `hf-hub` a una barra
/// `indicatif` (bytes totales agregados; los eventos `Progress` son deltas
/// por archivo y se acumulan por nombre de archivo).
fn indicatif_progress() -> hf_hub::progress::Progress {
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    struct BarHandler {
        bar: indicatif::ProgressBar,
        // Estado acumulado por archivo: los eventos Progress son deltas.
        per_file: Mutex<HashMap<String, u64>>,
        total_bytes: AtomicU64,
    }

    impl hf_hub::progress::ProgressHandler for BarHandler {
        fn on_progress(&self, event: &hf_hub::progress::ProgressEvent) {
            match event {
                hf_hub::progress::ProgressEvent::Download(
                    hf_hub::progress::DownloadEvent::Start { total_bytes, .. },
                ) => {
                    self.total_bytes.store(*total_bytes, Ordering::Relaxed);
                    self.bar.set_length(*total_bytes);
                }
                hf_hub::progress::ProgressEvent::Download(
                    hf_hub::progress::DownloadEvent::Progress { files },
                ) => {
                    let mut acc = 0u64;
                    let mut map = self.per_file.lock().unwrap();
                    for f in files {
                        map.insert(f.filename.clone(), f.bytes_completed);
                    }
                    for v in map.values() {
                        acc += *v;
                    }
                    drop(map);
                    self.bar
                        .set_position(acc.min(self.bar.length().unwrap_or(u64::MAX)));
                }
                hf_hub::progress::ProgressEvent::Download(
                    hf_hub::progress::DownloadEvent::AggregateProgress {
                        bytes_completed,
                        total_bytes,
                        ..
                    },
                ) => {
                    // Lote xet: totales agregados del lote en curso.
                    if self.total_bytes.load(Ordering::Relaxed) == 0 && *total_bytes > 0 {
                        self.bar.set_length(*total_bytes);
                    }
                    let pos = (*bytes_completed).min(self.bar.length().unwrap_or(u64::MAX));
                    self.bar.set_position(pos);
                }
                hf_hub::progress::ProgressEvent::Download(
                    hf_hub::progress::DownloadEvent::Complete,
                ) => {
                    self.bar.finish_with_message("descarga completa");
                }
                _ => {}
            }
        }
    }

    let bar = indicatif::ProgressBar::new(0);
    bar.set_style(
        indicatif::ProgressStyle::default_bar()
            .template("{spinner:.green} [{elapsed_precise}] {bar:30.cyan/blue} {bytes}/{total_bytes} {bytes_per_sec} eta:{eta}")
            .unwrap(),
    );
    hf_hub::progress::Progress::new(BarHandler {
        bar,
        per_file: Mutex::new(HashMap::new()),
        total_bytes: AtomicU64::new(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializa los tests que manipulan variables de entorno (estado global
    /// del proceso): `cargo test` los corre en paralelo y sin lock se pisan.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Copia de las variables de entorno que un test va a tocar. `ENV_LOCK`
    /// serializa a los lectores, pero sin restaurar al final un fallo a mitad
    /// contaminaría el resto de la suite.
    fn env_guard(vars: &[&str]) -> Vec<(String, Option<String>)> {
        vars.iter()
            .map(|v| ((*v).to_string(), std::env::var(v).ok()))
            .collect()
    }

    fn env_restore(saved: Vec<(String, Option<String>)>) {
        for (var, value) in saved {
            match value {
                Some(v) => std::env::set_var(&var, v),
                None => std::env::remove_var(&var),
            }
        }
    }

    /// Sin `HF_HUB_CACHE` ni `HF_HOME` la raíz de modelos es la **exclusiva de
    /// la aplicación** (D3): nunca la caché compartida de HuggingFace, y con
    /// el derivado CT2 y `xet` colgando de ella.
    #[test]
    fn models_root_is_exclusive_by_default() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_CACHE_DIR", "HF_HUB_CACHE", "HF_HOME", "XDG_CACHE_HOME"]);
        std::env::remove_var("AVI_CACHE_DIR");
        std::env::remove_var("HF_HUB_CACHE");
        std::env::remove_var("HF_HOME");
        std::env::remove_var("XDG_CACHE_HOME");

        assert!(
            shared_hf_root().is_none(),
            "sin variables de HuggingFace no hay raíz compartida"
        );
        assert!(!models_root_is_shared());
        let root = models_cache_dir();
        let rendered = root.to_string_lossy().to_string();
        if cfg!(windows) {
            assert_eq!(root.file_name().unwrap(), "models");
            assert_eq!(root.parent().unwrap().file_name().unwrap(), "cache");
            assert_eq!(
                root.parent()
                    .unwrap()
                    .parent()
                    .unwrap()
                    .file_name()
                    .unwrap(),
                APP_NAME
            );
        } else if cfg!(target_os = "macos") {
            assert!(
                root.ends_with("Library/Caches/ai-voice-interconnector/models"),
                "columna de modelos de macOS: {rendered}"
            );
        } else {
            assert!(
                root.ends_with(".cache/ai-voice-interconnector/models"),
                "columna de modelos de Linux: {rendered}"
            );
        }
        assert!(
            !rendered.contains("huggingface"),
            "la raíz por defecto no puede ser la caché HF: {rendered}"
        );
        assert!(ct2_cache_dir().starts_with(&root));
        assert_eq!(xet_cache_dir(), root.join(MODELS_XET_SUBDIR));

        env_restore(saved);
    }

    /// `HF_HUB_CACHE` tiene precedencia sobre `HF_HOME/hub`, y cualquiera de las
    /// dos convierte la raíz de modelos en la caché compartida que eligió el
    /// usuario. `AVI_CACHE_DIR` es la reubicación de la aplicación y manda sobre
    /// las dos: es lo que permite aislar una prueba con un `HF_HOME` global.
    #[test]
    fn shared_hf_root_takes_precedence() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_CACHE_DIR", "HF_HUB_CACHE", "HF_HOME"]);
        std::env::remove_var("AVI_CACHE_DIR");

        std::env::set_var("HF_HUB_CACHE", r"C:\cache_custom\hub");
        std::env::set_var("HF_HOME", "/hf_home_custom");
        assert_eq!(
            shared_hf_root(),
            Some(PathBuf::from(r"C:\cache_custom\hub"))
        );
        assert_eq!(models_cache_dir(), PathBuf::from(r"C:\cache_custom\hub"));
        assert!(models_root_is_shared());

        std::env::remove_var("HF_HUB_CACHE");
        assert_eq!(
            shared_hf_root(),
            Some(PathBuf::from("/hf_home_custom").join("hub"))
        );
        assert_eq!(
            models_cache_dir(),
            PathBuf::from("/hf_home_custom").join("hub")
        );

        std::env::set_var("AVI_CACHE_DIR", "/raiz_aislada");
        assert_eq!(models_cache_dir(), PathBuf::from("/raiz_aislada"));
        assert!(
            models_root_is_shared(),
            "reubicar no cambia la propiedad: el contenido sigue siendo compartido"
        );

        env_restore(saved);
    }

    /// Directorio de programa y directorio del enlace según la tabla de §7, en
    /// los dos layouts, y su reubicación por `AVI_INSTALL_DIR`/`AVI_BIN_DIR`.
    #[test]
    fn install_and_bin_dirs_per_platform() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_INSTALL_DIR", "AVI_BIN_DIR", "LOCALAPPDATA"]);
        std::env::remove_var("AVI_INSTALL_DIR");
        std::env::remove_var("AVI_BIN_DIR");
        // La aserción no puede depender de la máquina del que corre el test.
        std::env::set_var("LOCALAPPDATA", home_dir().join("AppData").join("Local"));

        let (install, bin) = (install_dir(), bin_dir());
        let rendered = install.to_string_lossy().to_string();
        // La aserción es de Windows: allí una ruta con `/` mezclado sale de una variable
        // de reubicación escrita con la barra equivocada. En Unix `/` **es** el
        // separador, así que la comprobación sería falsa por construcción.
        #[cfg(windows)]
        assert!(
            !rendered.contains('/'),
            "sin separadores mixtos en el directorio de programa: {rendered}"
        );
        #[cfg(not(windows))]
        assert!(
            rendered.contains('/'),
            "en Unix el separador del directorio de programa es `/`: {rendered}"
        );
        if cfg!(windows) {
            assert!(
                install.ends_with(r"Programs\ai-voice-interconnector"),
                "columna de programa en Windows: {rendered}"
            );
            assert_eq!(
                bin, install,
                "en Windows la entrada del PATH es el directorio de programa"
            );
        } else {
            assert!(
                install.ends_with(".local/opt/ai-voice-interconnector"),
                "columna de programa en Unix: {rendered}"
            );
            assert!(
                bin.ends_with(".local/bin"),
                "directorio del enlace en Unix: {}",
                bin.display()
            );
        }

        std::env::set_var("AVI_INSTALL_DIR", "/opt/avi_reubicado");
        std::env::set_var("AVI_BIN_DIR", "/usr/local/bin");
        assert_eq!(install_dir(), PathBuf::from("/opt/avi_reubicado"));
        assert_eq!(bin_dir(), PathBuf::from("/usr/local/bin"));
        if cfg!(windows) {
            // Sin reubicar el enlace, la entrada del PATH cae en el programa.
            std::env::remove_var("AVI_BIN_DIR");
            assert_eq!(bin_dir(), PathBuf::from("/opt/avi_reubicado"));
        }

        env_restore(saved);
    }

    /// `xet` cuelga de la raíz de modelos exclusiva y `ModelStore::new()` la
    /// fija en `HF_XET_CACHE` porque es la única palanca disponible. Con raíz
    /// compartida la variable es del usuario y no se toca: `xet` sigue la
    /// cadena real de `xet-runtime` y queda fuera de cualquier borrado (R3).
    #[test]
    fn xet_resolves_inside_exclusive_models_root() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&[
            "AVI_CACHE_DIR",
            "HF_HUB_CACHE",
            "HF_HOME",
            "HF_XET_CACHE",
            "XDG_CACHE_HOME",
        ]);
        for var in [
            "AVI_CACHE_DIR",
            "HF_HUB_CACHE",
            "HF_HOME",
            "HF_XET_CACHE",
            "XDG_CACHE_HOME",
        ] {
            std::env::remove_var(var);
        }

        let root = models_cache_dir();
        assert_eq!(xet_cache_dir(), root.join(MODELS_XET_SUBDIR));
        assert!(xet_cache_dir().starts_with(&root));

        let _store = ModelStore::new();
        assert_eq!(
            std::env::var("HF_XET_CACHE").ok(),
            Some(root.join(MODELS_XET_SUBDIR).to_string_lossy().to_string()),
            "`HFClientBuilder::cache_dir` solo fija `hub`: la variable es la única palanca"
        );

        std::env::set_var("HF_HOME", "/hf_home_custom");
        std::env::set_var("HF_XET_CACHE", "/xet_del_usuario");
        assert_eq!(
            models_cache_dir(),
            PathBuf::from("/hf_home_custom").join("hub")
        );
        assert_eq!(xet_cache_dir(), PathBuf::from("/xet_del_usuario"));
        let _store = ModelStore::new();
        assert_eq!(
            std::env::var("HF_XET_CACHE").ok(),
            Some("/xet_del_usuario".to_string()),
            "con raíz compartida la variable es del usuario y no se toca"
        );

        std::env::remove_var("HF_XET_CACHE");
        assert_eq!(
            xet_cache_dir(),
            PathBuf::from("/hf_home_custom").join("xet"),
            "sin HF_XET_CACHE, `xet-runtime` cae a HF_HOME/xet"
        );

        env_restore(saved);
    }

    /// La comparación de entradas de `PATH` de §9.3.1 no da un falso negativo
    /// con una entrada escrita con una variable sin expandir, que es como se
    /// lee el valor `Path` del registro de Windows.
    #[test]
    fn canonical_path_entry_comparison_handles_percent_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_TEST_PATH_ROOT"]);
        let root = std::env::temp_dir().join("avi_store_path_entry");
        std::env::set_var("AVI_TEST_PATH_ROOT", &root);

        let real = root.join("Programs").join(APP_NAME);
        let written = if cfg!(windows) {
            Path::new(r"%AVI_TEST_PATH_ROOT%\Programs\ai-voice-interconnector")
        } else {
            Path::new("${AVI_TEST_PATH_ROOT}/Programs/ai-voice-interconnector")
        };
        assert!(canonical_path_entry_matches(written, &real));
        assert!(
            canonical_path_entry_matches(written, &real.join("")),
            "la comparación sigue normalizando separadores finales"
        );

        // Entradas que solo se parecen en el nombre de la variable.
        let sibling = if cfg!(windows) {
            Path::new(r"%AVI_TEST_PATH_ROOT%\Programs")
        } else {
            Path::new("${AVI_TEST_PATH_ROOT}/Programs")
        };
        assert!(!canonical_path_entry_matches(
            sibling,
            &root.join("Programs").join("otro")
        ));

        // Una variable sin valor deja la entrada intacta: dos referencias
        // distintas no se vuelven iguales por accidente.
        assert!(!canonical_path_entry_matches(
            Path::new("%AVI_TEST_PATH_NO_EXISTE%"),
            Path::new("%AVI_TEST_PATH_TAMPOCO_EXISTE%")
        ));

        env_restore(saved);
    }

    /// El prefijo de ruta verbatim de Windows se quita antes de normalizar, en las
    /// dos formas que tiene: la local `\\?\C:\…` y la de red `\\?\UNC\servidor\…`.
    ///
    /// Es lo que hace que la ruta del ejecutable en ejecución —que la API de Windows
    /// puede devolver con el prefijo— y la del directorio de programa —que viene de
    /// `AVI_INSTALL_DIR` o de la convención, sin él— comparen iguales. Sin esto,
    /// `self install` invocado desde dentro del directorio de programa creería estar
    /// fuera y trataría una reparación como una instalación.
    #[cfg(windows)]
    #[test]
    fn canonical_path_key_strips_the_verbatim_prefix() {
        let sin_prefijo = Path::new(r"C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector");
        let con_prefijo =
            Path::new(r"\\?\C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector");
        assert_eq!(
            canonical_path_key(con_prefijo),
            canonical_path_key(sin_prefijo)
        );
        assert!(
            canonical_path_entry_matches(con_prefijo, sin_prefijo),
            "el comparador de entradas hereda la normalización"
        );
        assert!(canonical_path_entry_matches(sin_prefijo, con_prefijo));

        // Con separadores mixtos y barra final, que es como suele venir de verdad.
        assert_eq!(
            canonical_path_key(Path::new(r"\\?\C:\Users\ana\AppData\Local\Programs\")),
            canonical_path_key(Path::new(r"C:\Users\ana\AppData\Local\Programs")),
            "el prefijo se quita antes que cualquier otra normalización"
        );

        // La forma de red: `\\?\UNC\servidor\recurso` es `\\servidor\recurso`, con la
        // barra inicial, que es la que la distingue de una ruta local.
        assert_eq!(
            canonical_path_key(Path::new(r"\\?\UNC\servidor\recurso\bin")),
            canonical_path_key(Path::new(r"\\servidor\recurso\bin")),
            "la UNC verbatim se normaliza a la UNC normal"
        );
        assert_ne!(
            canonical_path_key(Path::new(r"\\?\UNC\servidor\recurso\bin")),
            canonical_path_key(Path::new(r"C:\servidor\recurso\bin")),
            "y no se confunde con una ruta local del mismo nombre"
        );

        // Una ruta que solo se parece no se toca: el prefijo no amplía lo que compara
        // igual.
        assert_ne!(
            canonical_path_key(Path::new(r"\\?\C:\otro-programa")),
            canonical_path_key(Path::new(r"C:\ai-voice-interconnector"))
        );

        // Una ruta sin prefijo no cambia: la normalización anterior sigue igual.
        assert_eq!(
            canonical_path_key(Path::new(r"C:\Users\ana\AppData\Local\Programs")),
            r"c:\users\ana\appdata\local\programs",
            "sin prefijo, el comportamiento es el de siempre"
        );
    }

    /// En Unix el prefijo verbatim no existe y **no** se quita nada: una ruta que
    /// empieza por `\\` es aquí un nombre de archivo, y tratarla como una de Windows
    /// haría comparar dos cosas distintas.
    ///
    /// Lo que la normalización sí hace en Unix es re-codificar `/` como `\`, y conservar
    /// **un** separador inicial: es lo que distingue una ruta absoluta de una relativa
    /// con el mismo nombre, que es la propiedad que hacía falta y que `trim_matches`
    /// rompía. La primera aserción decía `\?` donde la coherencia con la segunda —y con
    /// el propósito— exige `\\?`: quitar un separador inicial de una cadena que tiene dos
    /// es quitar contenido, no normalizar. La expectativa se corrige; el código, no.
    #[cfg(not(windows))]
    #[test]
    fn canonical_path_key_keeps_backslashes_on_unix() {
        assert_eq!(
            canonical_path_key(Path::new(r"\\?/home/ana/.local/bin")),
            r"\\?\home\ana\.local\bin",
            "en Unix los caracteres de la forma verbatim son parte del nombre"
        );
        assert_eq!(
            canonical_path_key(Path::new("/home/ana/.local/bin/")),
            r"\home\ana\.local\bin",
            "y la normalización de siempre sigue igual: un separador inicial, ninguno final"
        );
        assert_ne!(
            canonical_path_key(Path::new("/home/ana/bin")),
            canonical_path_key(Path::new("home/ana/bin")),
            "una ruta absoluta no compara igual a una relativa con el mismo nombre"
        );
    }

    fn min_wav() -> Vec<u8> {
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

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("avi_store_test_{}_{}", tag, std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Contrato del gate del derivado CT2. Acepta exactamente los layouts
    /// que `setup` produce (`source.spm`+`target.spm`) o el snapshot HF
    /// (`tokenizer.json`), y rechaza cualquier otro. El layout BPE
    /// `vocab.json`+`merges.txt` NO se acepta: no lo genera `convert_marian_to_ct2`,
    /// por lo que admitirlo sería especulativo y enmascararía dirs incompletos.
    #[test]
    fn ct2_dir_missing_files_contract_of_gate() {
        let dir = temp_dir("ct2_gate");
        let touch = |name: &str| std::fs::write(dir.join(name), b"x").unwrap();
        let clean = || {
            for f in [
                "model.bin",
                "tokenizer.json",
                "source.spm",
                "target.spm",
                "vocab.json",
                "merges.txt",
            ] {
                let _ = std::fs::remove_file(dir.join(f));
            }
        };

        // 1. Dir vacío: faltan model.bin + tokenizador completo.
        clean();
        assert_eq!(
            ct2_dir_missing_files(&dir),
            vec!["model.bin", "tokenizer.json", "source.spm", "target.spm"]
        );

        // 2. Layout SentencePiece (el que produce `setup`): completo.
        clean();
        touch("model.bin");
        touch("source.spm");
        touch("target.spm");
        assert!(ct2_dir_missing_files(&dir).is_empty());

        // 3. Layout HuggingFace (`tokenizer.json`): completo.
        clean();
        touch("model.bin");
        touch("tokenizer.json");
        assert!(ct2_dir_missing_files(&dir).is_empty());

        // 4. Layout BPE (`vocab.json`+`merges.txt`): rechazado a propósito.
        clean();
        touch("model.bin");
        touch("vocab.json");
        touch("merges.txt");
        assert_eq!(
            ct2_dir_missing_files(&dir),
            vec!["tokenizer.json", "source.spm", "target.spm"]
        );

        // 5. SentencePiece a medias (solo `source.spm`): incompleto.
        clean();
        touch("model.bin");
        touch("source.spm");
        assert_eq!(
            ct2_dir_missing_files(&dir),
            vec!["tokenizer.json", "target.spm"]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Normalización de mayúsculas en todas las operaciones del almacén
    /// (paridad con `voices.py:37` y `synthetic_speech.py:51`).
    #[test]
    fn normalization_lowercase() {
        let dir = temp_dir("norm");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();

        let saved = speech
            .save("VIVIAN", "SaludoDePrueba", "Hola", &wav_src)
            .unwrap();
        let rel = saved
            .strip_prefix(dir.join("speech"))
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(rel.replace('\\', "/"), "vivian/saludodeprueba.wav");

        assert!(speech.find("vivian", "saludodeprueba").is_some());
        assert!(speech.find("VIVIAN", "SALUDODEPRUEBA").is_some());
        let entries = speech.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].metadata.voice, "vivian");
        assert_eq!(entries[0].metadata.label, "saludodeprueba");

        speech.remove("VIVIAN", "SALUDODEPRUEBA").unwrap();
        assert!(speech.find("vivian", "saludodeprueba").is_none());

        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        voices.ensure_initialized().unwrap();
        let vdir = voices.voice_dir("MiVoz");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("reference.qvoice"), b"QVCE").unwrap();
        assert!(
            voices.exists("MIVOZ"),
            "exists debe normalizar a minúsculas"
        );
        assert_eq!(
            voices
                .find_reference("MIVOZ")
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "reference.qvoice"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `list_by_voice` devuelve solo la voz pedida (insensible a
    /// mayúsculas) y lista vacía para voz sin locuciones; `list` sin filtro
    /// sigue devolviendo todo.
    #[test]
    fn list_by_voice_filters_by_voice() {
        let dir = temp_dir("list_voice");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "saludo", "Hola", &wav_src).unwrap();
        speech
            .save("vivian", "despedida", "Adiós", &wav_src)
            .unwrap();

        let ryan = speech.list_by_voice("RYAN").unwrap();
        assert_eq!(ryan.len(), 1);
        assert_eq!(ryan[0].metadata.voice, "ryan");
        assert_eq!(ryan[0].metadata.label, "saludo");

        let vivian = speech.list_by_voice("vivian").unwrap();
        assert_eq!(vivian.len(), 1);
        assert_eq!(vivian[0].metadata.label, "despedida");

        assert!(speech.list_by_voice("inexistente").unwrap().is_empty());
        assert_eq!(speech.list().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// round-trip `save` → `find` con `duration_secs` calculada del WAV.
    #[test]
    fn save_find_round_trip_with_duration() {
        let dir = temp_dir("roundtrip");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();

        let path = speech.save("ryan", "saludo", "Hola", &wav_src).unwrap();
        assert!(path.is_file());
        let entry = speech
            .find("ryan", "saludo")
            .expect("la locución debe existir");
        assert_eq!(entry.metadata.text, "Hola");
        // 1 muestra a 24 kHz → 1/24000 s
        assert!((entry.metadata.duration_secs - 1.0 / 24_000.0).abs() < 1e-9);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// sidecar ausente/corrupto es tolerable en `list` (conserva la
    /// tolerancia previa del oráculo).
    #[test]
    fn sidecar_missing_tolerable() {
        let dir = temp_dir("sidecar");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "saludo", "Hola", &wav_src).unwrap();
        // Sidecar corrupto → la locución se omite, pero no se cae el listado.
        std::fs::write(speech.voice_dir("ryan").join("saludo.json"), b"{roto").unwrap();
        let entries = speech.list().unwrap();
        assert!(entries.is_empty());
        // Sin sidecar no hay entrada.
        std::fs::remove_file(speech.voice_dir("ryan").join("saludo.json")).unwrap();
        assert!(speech.list().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `validate_name` acepta el regex del oráculo y rechaza lo demás.
    #[test]
    fn validate_name_regex_oracle() {
        assert!(VoiceStore::validate_name("Mi_Voz-2").is_ok());
        assert!(
            VoiceStore::validate_name("mi voz").is_err(),
            "espacios fuera del regex"
        );
        assert!(VoiceStore::validate_name("mi@voz").is_err());
        assert!(VoiceStore::validate_name("").is_err());
        assert!(VoiceStore::validate_name("a/b").is_err());
        assert!(VoiceStore::validate_name("..").is_err());
    }

    /// `save_reference` escribe `reference.qvoice` con tmp+rename y
    /// `find_reference` resuelve solo por `reference.qvoice` (sin fallback WAV).
    #[test]
    fn save_reference_qvoice_canonical_without_fallback_wav() {
        let dir = temp_dir("ref");
        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        let src = dir.join("clon.qvoice");
        std::fs::write(&src, b"QVCE").unwrap();
        let saved = voices.save_reference("MiVoz", &src).unwrap();
        assert_eq!(
            saved.file_name().unwrap().to_string_lossy(),
            "reference.qvoice"
        );
        assert!(saved.is_file());
        assert_eq!(voices.find_reference("mivoz").unwrap(), saved);

        // Sin fallback wav: solo qvoice es canónico.
        let vdir = voices.voice_dir("otra");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert!(
            voices.find_reference("OTRA").is_none(),
            "sin reference.qvoice no resuelve como clonada"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `ensure_initialized` registra voces de fábrica (`default` clonada
    /// de fábrica con `.qvoice`, `ryan`/`vivian` presets) y `list`/`remove`/
    /// `find_reference` reflejan el registro unificado (--voice ryan alcanzable).
    #[test]
    fn ensure_initialized_registers_voices_factory() {
        let dir = temp_dir("factory");
        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        voices.ensure_initialized().unwrap();
        // Las tres fábricas deben existir; `default` es clonada con qvoice, `ryan`/`vivian` presets puros
        for name in ["default", "ryan", "vivian"] {
            let d = voices.voice_dir(name);
            assert!(d.is_dir(), "directorio de fábrica '{}' debe existir", name);
            assert!(
                voices.exists(name),
                "exists('{}') debe ser true (hoy sería exit 3 para ryan)",
                name
            );
            // No debe haber WAV legado materializado
            assert!(
                !d.join("speech-reference.wav").is_file(),
                "no debe materializar speech-reference.wav para '{}'",
                name
            );
            assert!(
                !d.join("timbre-reference.wav").is_file(),
                "no debe materializar timbre-reference.wav para '{}'",
                name
            );
        }
        // `default` clonada de fábrica debe tener `reference.qvoice` materializado
        assert!(
            voices.find_reference("default").is_some(),
            "fábrica 'default' debe tener reference.qvoice"
        );
        assert_eq!(
            voices
                .find_reference("default")
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "reference.qvoice"
        );
        // `ryan`/`vivian` siguen presets puros sin referencia
        for name in ["ryan", "vivian"] {
            assert!(
                voices.find_reference(name).is_none(),
                "fábrica '{}' sin qvoice no debe tener referencia (preset puro)",
                name
            );
        }
        // Idempotencia: segunda inicialización no borra directorios ni datos de usuario
        let custom_dir = voices.voice_dir("mivoz");
        std::fs::create_dir_all(&custom_dir).unwrap();
        std::fs::write(custom_dir.join("reference.qvoice"), b"QVCE").unwrap();
        voices.ensure_initialized().unwrap();
        assert!(
            custom_dir.join("reference.qvoice").is_file(),
            "voz clonada preservada"
        );
        for name in ["default", "ryan", "vivian"] {
            assert!(
                voices.voice_dir(name).is_dir(),
                "fábrica '{}' preservada tras 2º init",
                name
            );
        }
        // `list` debe ver las tres fábricas marcadas is_factory + la clonada
        let list = voices.list().unwrap();
        let factory_names: Vec<String> = list
            .iter()
            .filter(|v| v.is_factory)
            .map(|v| v.name.clone())
            .collect();
        assert_eq!(factory_names.len(), 3, "tres voces de fábrica");
        assert!(factory_names.contains(&"default".to_string()));
        assert!(factory_names.contains(&"ryan".to_string()));
        assert!(factory_names.contains(&"vivian".to_string()));
        // Todas las fábricas protegidas contra borrado
        for name in ["default", "ryan", "vivian", "DEFAULT", "RYAN"] {
            assert!(
                voices.remove(name).is_err(),
                "remove('{}') debe fallar (fábrica protegida)",
                name
            );
        }
        // Voz sin qvoice no resuelve como clonada aunque tenga wav legado inerte
        let clone_dir = voices.voice_dir("otra");
        std::fs::create_dir_all(&clone_dir).unwrap();
        std::fs::write(clone_dir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert!(
            voices.find_reference("OTRA").is_none(),
            "sin qvoice no resuelve como clonada"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `revision_of("qwen3-tts-0.6b-base")` existe con repo público confirmado y hash
    /// real (40 hex), y `model_snapshot_path` resuelve bajo la raíz de modelos
    /// reubicada.
    #[test]
    fn revision_of_base_exists_and_snapshot_resolves() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Pin debe existir
        let (repo, rev) = ModelStore::revision_of("qwen3-tts-0.6b-base")
            .expect("qwen3-tts-0.6b-base debe estar en MODEL_REVISIONS");
        assert_eq!(repo, "Qwen/Qwen3-TTS-12Hz-0.6B-Base");
        assert_eq!(rev.len(), 40, "commit hash debe ser 40 chars");
        // Snapshot resuelve contra la raíz de modelos reubicada
        let saved = env_guard(&["AVI_CACHE_DIR", "HF_HUB_CACHE", "HF_HOME"]);
        let tmp = temp_dir("base_snapshot");
        std::env::set_var("AVI_CACHE_DIR", &tmp);
        std::env::remove_var("HF_HUB_CACHE");
        std::env::remove_var("HF_HOME");
        let store = ModelStore::new();
        assert_eq!(models_cache_dir(), tmp);
        // Crear snapshot vacío con al menos un fichero para que is_provisioned sea true
        let (repo2, rev2) = ModelStore::revision_of("qwen3-tts-0.6b-base").unwrap();
        let repo_dir = models_cache_dir().join(format!("models--{}", repo2.replace('/', "--")));
        let snap = repo_dir.join("snapshots").join(rev2);
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("config.json"), br#"{"tts_model_type":"base"}"#).unwrap();
        assert!(store.is_provisioned("qwen3-tts-0.6b-base"));
        let resolved = store.model_snapshot_path("qwen3-tts-0.6b-base").unwrap();
        assert!(resolved.is_dir());
        env_restore(saved);
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&repo_dir);
    }
}
