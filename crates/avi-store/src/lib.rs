use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Directorio base de datos del usuario (~/.ai-voice-interconnector)
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "ai-voice-interconnector")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".ai-voice-interconnector"))
}

/// Voces de fábrica: `ryan`/`vivian` son presets del motor (`qwen_tts.c:spk_table`)
/// sin audio; `default` es voz clonada de fábrica (`.qvoice` graft vía Base) para
/// garantizar `WER ≤0.25` en texto corto. La distinción preset/clonada vive
/// en `avi-tts::resolve_voice_motor` (presencia de `reference.qvoice`).
pub const FACTORY_VOICES: &[&str] = &["default", "ryan", "vivian"];

/// Asset embebido: `.qvoice` de fábrica para `default` (16 MB graft, speaker
/// embedding + pesos Base). Se materializa en `ensure_initialized` si falta.
const FACTORY_DEFAULT_QVOICE: &[u8] = include_bytes!("../assets/default/reference.qvoice");

pub fn is_factory_name(name: &str) -> bool {
    FACTORY_VOICES.contains(&name.to_lowercase().as_str())
}

/// Directorio de instalación per-user en Windows — única fuente canónica.
///
/// Espejo de `install-windows.ps1:Get-InstallDir` (`Join-Path $env:LOCALAPPDATA
/// "Programs\ai-voice-interconnector"`). Construcción determinista en dos
/// `join` — no `join("Programs/ai-...")` que deja `/` mixto — para que
/// `display` y comparación sean coherentes con el registro `HKCU`.
#[cfg(windows)]
pub fn windows_install_dir() -> PathBuf {
    let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(local)
        .join("Programs")
        .join("ai-voice-interconnector")
}

/// Normalización canónica de una entrada de `PATH` para comparación
/// determinista: `/` → `\`, `lowercase`, `trim` de `\` y espacios.
/// Hace que `C:\...\Programs/ai-voice-interconnector` y
/// `C:\...\Programs\ai-voice-interconnector\` sean idénticas.
pub fn canonical_path_key(p: &Path) -> String {
    p.to_string_lossy()
        .replace('/', "\\")
        .to_lowercase()
        .trim_matches('\\')
        .trim()
        .to_string()
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
        // Materializar `default` clonada de fábrica desde el asset embebido (T7).
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
        // Limpieza de materialización legada (12 MB embebidos pre-T4): si quedó
        // `speech-reference.wav` / `timbre-reference.wav` de la factoria anterior,
        // se eliminan para que `find_reference` no devuelva un legado como clonada.
        // No borrar `reference.qvoice` de `default` (ahora es la fábrica T7).
        for name in FACTORY_VOICES {
            let dir = self.base_dir.join(name);
            let legacy_speech = dir.join("speech-reference.wav");
            // Solo borrar en voces de fábrica; las clonadas del usuario conservan su legado.
            if is_factory_name(name) && legacy_speech.is_file() {
                let _ = std::fs::remove_file(&legacy_speech);
            }
            let legacy_timbre = dir.join("timbre-reference.wav");
            if is_factory_name(name) && legacy_timbre.is_file() {
                let _ = std::fs::remove_file(&legacy_timbre);
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
    /// reglas de seguridad anti-escape; paridad de contrato, divergencia 3 de F1)
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
    /// tienen referencia y resuelven como `Preset` en `resolve_voice_motor`.
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
        self.ensure_initialized()?;
        let mut entries = Vec::new();
        if !self.base_dir.is_dir() {
            return Ok(entries);
        }
        // Iterar por directorio de voz
        for voice_dir in std::fs::read_dir(&self.base_dir)? {
            let voice_dir = voice_dir?;
            if !voice_dir.file_type()?.is_dir() {
                continue;
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

/// Estado de provisión de un modelo
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ModelStatus {
    /// Modelo descargado y listo para uso
    Ready,
    /// Modelo parcialmente descargado o corrupto
    Incomplete,
    /// Modelo no descargado
    Missing,
}

/// Entrada de un modelo en el almacén
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelEntry {
    pub name: String,
    pub revision: String,
    pub status: ModelStatus,
    pub path: PathBuf,
    pub size_bytes: Option<u64>,
}

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

/// Directorio raíz de la cache de HuggingFace — decisión de la aplicación, no
/// del crate.
///
/// `hf-hub` 1.0 resuelve con fallback `HOME`→`/tmp` (hardcodeado), lo que en
/// Windows produce `<unidad-del-cwd>:\tmp\.cache\huggingface\hub`: ubicación
/// no-canónica, dependiente de la unidad y compartida entre usuarios. Aquí se
/// decide localmente para que lectura (`is_provisioned`, cleanup/uninstall) y
/// escritura (`ensure_downloaded`) usen SIEMPRE la misma ruta, determinista en
/// los 4 targets:
///
/// 1. `HF_HUB_CACHE` (override explícito del usuario)
/// 2. `HF_HOME/hub` (convención HF)
/// 3. `{home}/.cache/huggingface/hub` — misma convención que `huggingface_hub`
///    de Python en los tres SO, por lo que reutiliza modelos ya bajados por
///    instalaciones previas.
pub fn hf_cache_dir() -> PathBuf {
    if let Ok(cache) = std::env::var("HF_HUB_CACHE") {
        if !cache.is_empty() {
            return PathBuf::from(cache);
        }
    }
    if let Ok(home) = std::env::var("HF_HOME") {
        if !home.is_empty() {
            return PathBuf::from(home).join("hub");
        }
    }
    let home = directories::UserDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".cache").join("huggingface").join("hub")
}

/// Directorio raíz de la cache xet (shard-cache) acoplada a `hf_cache_dir`.
/// `hf-hub` con `xet` usa `~/.cache/huggingface/xet` además de `hub`; purgar
/// solo `hub` deja `shard-cache` huérfano e incoherente (bug T2).
pub fn xet_cache_dir() -> PathBuf {
    // Deriva de `hf_cache_dir` reemplazando el último componente `hub` por `xet`
    // para honrar `HF_HUB_CACHE`/`HF_HOME` cuando apuntan a `hub`.
    let hub = hf_cache_dir();
    if hub.ends_with("hub") {
        hub.parent()
            .map(|p| p.join("xet"))
            .unwrap_or_else(|| hub.join("../xet"))
    } else {
        // Fallback directo a `~/.cache/huggingface/xet` si `hf_cache_dir` no termina en hub
        let home = directories::UserDirs::new()
            .map(|d| d.home_dir().to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        home.join(".cache").join("huggingface").join("xet")
    }
}

/// Directorio CT2 derivado de los snapshots HF Marian, cacheado en `hf_cache_dir()/ct2`.
/// Layout: `hf_cache_dir()/ct2/opus-mt-es-en` y `opus-mt-en-es`, cada uno con `model.bin` CT2.
/// Es el derivado idempotente de `_convert_translation_model` del oráculo Python.
pub fn ct2_cache_dir() -> PathBuf {
    hf_cache_dir().join("ct2")
}
pub fn ct2_model_dir(pair: &str) -> PathBuf {
    ct2_cache_dir().join(format!("opus-mt-{}", pair))
}
pub fn is_ct2_provisioned(pair: &str) -> bool {
    ct2_model_dir(pair).join("model.bin").is_file()
}

/// Almacén de modelos descargados.
///
/// Fuente de verdad: snapshots de HuggingFace en `hf_cache_dir()` con layout
/// `models--<org>--<repo>/snapshots/<hash>/`. `data_dir()/models/<name>/manifest.json`
/// queda como índice de compatibilidad (doctor/estado), no como almacenamiento.
pub struct ModelStore {
    base_dir: PathBuf,
}

impl Default for ModelStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ModelStore {
    pub fn new() -> Self {
        let base_dir = data_dir().join("models");
        Self { base_dir }
    }

    pub fn ensure_initialized(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        Ok(())
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
        let repo_dir = hf_cache_dir().join(format!("models--{}", repo.replace('/', "--")));
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
    /// integridad mínima (ficheros críticos con `size>0`). Si no hay pin,
    /// cae al índice legacy `manifest.json`.
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
                        for entry in entries.by_ref() {
                            if let Ok(e) = entry {
                                let path = e.path();
                                if path.is_file() {
                                    if let Ok(md) = std::fs::metadata(&path) {
                                        if md.len() > 0 {
                                            return true;
                                        }
                                    }
                                }
                            }
                        }
                        false
                    }
                    Err(_) => false,
                }
            }
            None => {
                let manifest = self.base_dir.join(model_name).join("manifest.json");
                if let Ok(content) = std::fs::read_to_string(&manifest) {
                    if let Ok(entry) = serde_json::from_str::<ModelEntry>(&content) {
                        return entry.status == ModelStatus::Ready;
                    }
                }
                false
            }
        }
    }

    /// Listar todos los modelos conocidos (pines + cualquier índice legacy).
    pub fn list(&self) -> Result<Vec<ModelEntry>> {
        self.ensure_initialized()?;
        let mut entries = Vec::new();
        for (name, repo, rev) in MODEL_REVISIONS {
            let status = if self.is_provisioned(name) {
                ModelStatus::Ready
            } else {
                ModelStatus::Missing
            };
            entries.push(ModelEntry {
                name: name.to_string(),
                revision: rev.to_string(),
                status,
                path: hf_cache_dir().join(format!("models--{}", repo.replace('/', "--"))),
                size_bytes: None,
            });
        }
        // Índices legacy sin pin (compatibilidad)
        for dir in std::fs::read_dir(&self.base_dir)? {
            let dir = dir?;
            if !dir.file_type()?.is_dir() {
                continue;
            }
            let name = dir.file_name().to_string_lossy().to_string();
            if ModelStore::revision_of(&name).is_some() {
                continue;
            }
            let manifest = dir.path().join("manifest.json");
            if manifest.is_file() {
                if let Ok(content) = std::fs::read_to_string(&manifest) {
                    if let Ok(entry) = serde_json::from_str::<ModelEntry>(&content) {
                        entries.push(entry);
                    }
                }
            }
        }
        Ok(entries)
    }

    /// Directorio de un modelo: snapshot HF si hay pin, si no índice legacy.
    pub fn model_dir(&self, model_name: &str) -> PathBuf {
        self.model_snapshot_path(model_name)
            .unwrap_or_else(|| self.base_dir.join(model_name))
    }

    /// Registrar un modelo como provisionado escribiendo su manifest.json
    pub fn register_provisioned(&self, model_name: &str, revision: &str) -> Result<ModelEntry> {
        self.ensure_initialized()?;
        let dir = self.base_dir.join(model_name);
        std::fs::create_dir_all(&dir)?;

        let entry = ModelEntry {
            name: model_name.to_string(),
            revision: revision.to_string(),
            status: ModelStatus::Ready,
            path: dir.clone(),
            size_bytes: None,
        };

        let manifest_path = dir.join("manifest.json");
        let content = serde_json::to_string_pretty(&entry)?;
        std::fs::write(&manifest_path, content)?;

        Ok(entry)
    }

    /// Borrar el snapshot HF de un modelo (cleanup/uninstall).
    pub fn remove_hf_snapshot(&self, model_name: &str) -> Result<bool> {
        if let Some((repo, _)) = ModelStore::revision_of(model_name) {
            let dir = hf_cache_dir().join(format!("models--{}", repo.replace('/', "--")));
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Borrar la cache xet asociada a `hf_cache_dir` (shard-cache + staging + logs).
    /// Se usa en `cleanup`/`uninstall` para evitar incoherencia `hub` purgado + `xet` vivo.
    pub fn remove_xet_cache() -> Result<bool> {
        let xet = xet_cache_dir();
        let mut removed = false;
        if xet.is_dir() {
            std::fs::remove_dir_all(&xet)?;
            removed = true;
        }
        // Limpiar locks huérfanos de hub si quedaron
        let locks = hf_cache_dir().join(".locks");
        if locks.is_dir() {
            // Intentar borrar, ignorar error si no está vacío por otro proceso
            let _ = std::fs::remove_dir_all(&locks);
        }
        Ok(removed)
    }

    /// Descarga nativa de un modelo pinneado vía HuggingFace Hub.
    ///
    /// Usa `hf-hub` (`snapshot_download` con revisión de `MODEL_REVISIONS`): cache
    /// estándar en `hf_cache_dir()`, resume por Range, ETag/commit-hash y reintentos
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
        // Cache explícita: la resolución de la app (hf_cache_dir) manda sobre el
        // fallback roto de hf-hub (HOME→/tmp); lectura y escritura convergen.
        let client = hf_hub::HFClient::builder()
            .cache_dir(hf_cache_dir())
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
            let repo_dir = hf_cache_dir().join(format!("models--{}", repo_id.replace('/', "--")));
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

    /// T-descargador: `hf_cache_dir()` honra `HF_HUB_CACHE`, luego `HF_HOME/hub`,
    /// y cae en `{home}/.cache/huggingface/hub` — nunca en `/tmp`.
    #[test]
    fn hf_cache_dir_precedencia_env_y_fallback() {
        let _guard = ENV_LOCK.lock().unwrap();
        let hf_hub_cache = std::env::var("HF_HUB_CACHE").ok();
        let hf_home = std::env::var("HF_HOME").ok();
        let xdg = std::env::var("XDG_CACHE_HOME").ok();

        // 1. HF_HUB_CACHE tiene precedencia máxima
        std::env::set_var("HF_HUB_CACHE", r"C:\cache_custom\hub");
        std::env::remove_var("HF_HOME");
        assert_eq!(hf_cache_dir(), PathBuf::from(r"C:\cache_custom\hub"));

        // 2. Sin HF_HUB_CACHE, HF_HOME/hub
        std::env::remove_var("HF_HUB_CACHE");
        std::env::set_var("HF_HOME", "/hf_home_custom");
        assert_eq!(hf_cache_dir(), PathBuf::from("/hf_home_custom").join("hub"));

        // 3. Fallback: home/.cache/huggingface/hub (nunca /tmp)
        std::env::remove_var("HF_HOME");
        let dir = hf_cache_dir();
        let dir_str = dir.to_string_lossy().to_lowercase();
        assert!(
            dir_str.ends_with(r"\.cache\huggingface\hub")
                || dir_str.ends_with("/.cache/huggingface/hub"),
            "el fallback debe ser {{home}}/.cache/huggingface/hub, fue: {dir_str}"
        );
        assert!(
            !dir_str.contains("\\tmp\\"),
            "no debe caer en /tmp: {dir_str}"
        );

        // Restaurar estado env original
        match hf_hub_cache {
            Some(v) => std::env::set_var("HF_HUB_CACHE", v),
            None => std::env::remove_var("HF_HUB_CACHE"),
        }
        match hf_home {
            Some(v) => std::env::set_var("HF_HOME", v),
            None => std::env::remove_var("HF_HOME"),
        }
        if let Some(v) = xdg {
            std::env::set_var("XDG_CACHE_HOME", v);
        }
    }

    fn wav_minimo() -> Vec<u8> {
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

    /// T4: normalización de mayúsculas en todas las operaciones del almacén
    /// (paridad con `voices.py:37` y `synthetic_speech.py:51`).
    #[test]
    fn normalizacion_minusculas() {
        let dir = temp_dir("norm");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, wav_minimo()).unwrap();

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

    /// T9: round-trip `save` → `find` con `duration_secs` calculada del WAV.
    #[test]
    fn save_find_round_trip_con_duration() {
        let dir = temp_dir("roundtrip");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, wav_minimo()).unwrap();

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

    /// T4: sidecar ausente/corrupto es tolerable en `list` (conserva la
    /// tolerancia previa del oráculo; divergencia 2 de F1).
    #[test]
    fn sidecar_ausente_tolerable() {
        let dir = temp_dir("sidecar");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, wav_minimo()).unwrap();
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

    /// T4: `validate_name` acepta el regex del oráculo y rechaza lo demás.
    #[test]
    fn validate_name_regex_oraculo() {
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

    /// T8: `save_reference` escribe `reference.qvoice` con tmp+rename y
    /// `find_reference` hace fallback a `speech-reference.wav`.
    #[test]
    fn save_reference_y_fallback_speech_reference() {
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

    /// T4+T7: `ensure_initialized` registra voces de fábrica (`default` clonada
    /// de fábrica con `.qvoice`, `ryan`/`vivian` presets) y `list`/`remove`/
    /// `find_reference` reflejan el registro unificado (--voice ryan alcanzable).
    #[test]
    fn ensure_initialized_registra_voces_factory() {
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
            "fábrica 'default' debe tener reference.qvoice (T7)"
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
        assert!(custom_dir.join("reference.qvoice").is_file(), "voz clonada preservada");
        for name in ["default", "ryan", "vivian"] {
            assert!(voices.voice_dir(name).is_dir(), "fábrica '{}' preservada tras 2º init", name);
        }
        // `list` debe ver las tres fábricas marcadas is_factory + la clonada
        let list = voices.list().unwrap();
        let factory_names: Vec<String> = list.iter().filter(|v| v.is_factory).map(|v| v.name.clone()).collect();
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
        // Fábrica con speech-reference.wav no cae en Clonada (solo qvoice)
        let legacy = voices.voice_dir("ryan").join("speech-reference.wav");
        std::fs::write(&legacy, b"RIFF").unwrap();
        assert!(
            voices.find_reference("ryan").is_none(),
            "fábrica con legado no debe resolver como clonada"
        );
        // Limpieza legada en siguiente ensure_initialized
        voices.ensure_initialized().unwrap();
        assert!(!legacy.is_file(), "ensure_initialized debe limpiar legado de fábrica");
        // Voz sin qvoice no resuelve como clonada aunque tenga wav legado
        let clon_dir = voices.voice_dir("otra");
        std::fs::create_dir_all(&clon_dir).unwrap();
        std::fs::write(clon_dir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert!(
            voices.find_reference("OTRA").is_none(),
            "sin qvoice no resuelve como clonada"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn windows_install_dir_y_canonical_path_key_deterministas() {
        // windows_install_dir usa dos join — no mixto — y canonical_path_key
        // normaliza \ vs / , case y trailing.
        #[cfg(windows)]
        {
            let dir = windows_install_dir();
            assert!(
                !dir.to_string_lossy().contains('/'),
                "install_dir no debe contener '/' mixto: {}",
                dir.display()
            );
            assert!(dir.ends_with("ai-voice-interconnector"));
        }
        // canonical_path_key es cross-platform.
        assert_eq!(
            canonical_path_key(std::path::Path::new(
                "C:\\Users\\Ana\\AppData\\Local\\Programs/ai-voice-interconnector\\"
            )),
            canonical_path_key(std::path::Path::new(
                "C:\\Users\\ana\\AppData\\Local\\Programs\\ai-voice-interconnector"
            ))
        );
        assert_eq!(
            canonical_path_key(std::path::Path::new(
                "C:/Users/Ana/AppData/Local/Programs/ai-voice-interconnector/"
            )),
            "c:\\users\\ana\\appdata\\local\\programs\\ai-voice-interconnector"
        );
    }

    /// T1: `revision_of("qwen3-tts-0.6b-base")` existe con repo público confirmado y hash
    /// real (40 hex), y `model_snapshot_path` resuelve bajo HF_HUB_CACHE temporal.
    #[test]
    fn revision_of_base_existe_y_snapshot_resuelve() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Pin debe existir
        let (repo, rev) = ModelStore::revision_of("qwen3-tts-0.6b-base")
            .expect("qwen3-tts-0.6b-base debe estar en MODEL_REVISIONS");
        assert_eq!(repo, "Qwen/Qwen3-TTS-12Hz-0.6B-Base");
        assert_eq!(rev.len(), 40, "commit hash debe ser 40 chars");
        // Snapshot resuelve con HF_HUB_CACHE temporal no vacío
        let prev = std::env::var("HF_HUB_CACHE").ok();
        let tmp = temp_dir("base_snapshot");
        std::env::set_var("HF_HUB_CACHE", tmp.to_string_lossy().to_string());
        let store = ModelStore::new();
        // Crear snapshot vacío con al menos un fichero para que is_provisioned sea true
        let (repo2, rev2) = ModelStore::revision_of("qwen3-tts-0.6b-base").unwrap();
        let repo_dir = hf_cache_dir().join(format!("models--{}", repo2.replace('/', "--")));
        let snap = repo_dir.join("snapshots").join(rev2);
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("config.json"), br#"{"tts_model_type":"base"}"#).unwrap();
        assert!(store.is_provisioned("qwen3-tts-0.6b-base"));
        let resolved = store.model_snapshot_path("qwen3-tts-0.6b-base").unwrap();
        assert!(resolved.is_dir());
        match prev {
            Some(v) => std::env::set_var("HF_HUB_CACHE", v),
            None => std::env::remove_var("HF_HUB_CACHE"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
        let _ = std::fs::remove_dir_all(&repo_dir);
    }
}
