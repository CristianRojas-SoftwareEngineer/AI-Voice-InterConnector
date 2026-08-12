use std::path::{Path, PathBuf};
use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Directorio base de datos del usuario (~/.ai-voice-interconnector)
pub fn data_dir() -> PathBuf {
    directories::ProjectDirs::from("", "", "ai-voice-interconnector")
        .map(|d| d.data_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".ai-voice-interconnector"))
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

impl VoiceStore {
    pub fn new() -> Self {
        let base_dir = data_dir().join("voices");
        Self { base_dir }
    }

    /// Asegura que el directorio base y la voz "default" existan
    pub fn ensure_initialized(&self) -> Result<()> {
        std::fs::create_dir_all(&self.base_dir)?;
        let default_dir = self.base_dir.join("default");
        if !default_dir.exists() {
            std::fs::create_dir_all(&default_dir)?;
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
                let name = entry.file_name().to_string_lossy().to_string();
                let is_factory = name == "default";
                let ref_path = self.find_reference(&name);
                voices.push(VoiceEntry {
                    name,
                    is_factory,
                    reference_path: ref_path,
                });
            }
        }
        // Asegurar que "default" esté primero
        voices.sort_by(|a, b| b.is_factory.cmp(&a.is_factory).then(a.name.cmp(&b.name)));
        Ok(voices)
    }

    /// Validar un nombre de voz (anti-escape de rutas)
    pub fn validate_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("El nombre de la voz no puede estar vacío.".into());
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
            return Err(format!("El nombre de voz '{}' contiene caracteres no permitidos.", name));
        }
        if name.len() > 64 {
            return Err("El nombre de la voz excede 64 caracteres.".into());
        }
        Ok(())
    }

    /// Verificar si una voz existe
    pub fn exists(&self, name: &str) -> bool {
        self.base_dir.join(name).is_dir()
    }

    /// Eliminar una voz (no permite eliminar "default")
    pub fn remove(&self, name: &str) -> Result<(), String> {
        if name == "default" {
            return Err("La voz 'default' no se puede eliminar.".into());
        }
        let dir = self.base_dir.join(name);
        if !dir.is_dir() {
            return Err(format!("La voz '{}' no existe.", name));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Buscar el archivo de referencia de una voz (.qvoice o .wav)
    fn find_reference(&self, name: &str) -> Option<PathBuf> {
        let dir = self.base_dir.join(name);
        for ext in &["qvoice", "wav"] {
            let path = dir.join(format!("reference.{}", ext));
            if path.is_file() {
                return Some(path);
            }
        }
        None
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
                        if let Ok(meta) = serde_json::from_str::<SpeechMetadata>(&content) {
                            let wav_path = path.with_extension("wav");
                            if wav_path.is_file() {
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

    /// Directorio para una voz específica
    pub fn voice_dir(&self, voice: &str) -> PathBuf {
        self.base_dir.join(voice)
    }

    /// Ruta del WAV para una locución
    pub fn audio_path(&self, voice: &str, label: &str) -> PathBuf {
        self.voice_dir(voice).join(format!("{}.wav", label))
    }

    /// Buscar una locución por (voz, etiqueta)
    pub fn find(&self, voice: &str, label: &str) -> Option<SpeechEntry> {
        let meta_path = self.voice_dir(voice).join(format!("{}.json", label));
        let wav_path = self.audio_path(voice, label);
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
        let wav = self.audio_path(voice, label);
        let meta = self.voice_dir(voice).join(format!("{}.json", label));
        if !wav.is_file() && !meta.is_file() {
            return Err(format!("La locución '{}' de la voz '{}' no existe.", label, voice));
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
    pub fn save_metadata(&self, voice: &str, label: &str, text: &str, duration_secs: f64) -> Result<PathBuf> {
        let dir = self.voice_dir(voice);
        std::fs::create_dir_all(&dir)?;
        let meta = SpeechMetadata {
            label: label.to_string(),
            voice: voice.to_string(),
            text: text.to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            duration_secs,
        };
        let meta_path = dir.join(format!("{}.json", label));
        let content = serde_json::to_string_pretty(&meta)?;
        std::fs::write(&meta_path, content)?;
        Ok(meta_path)
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

/// Almacén de modelos descargados.
/// Layout en disco: <data_dir>/models/<nombre>/
pub struct ModelStore {
    base_dir: PathBuf,
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

    /// Verificar si un modelo está provisionado
    pub fn is_provisioned(&self, model_name: &str) -> bool {
        let manifest = self.base_dir.join(model_name).join("manifest.json");
        if let Ok(content) = std::fs::read_to_string(&manifest) {
            if let Ok(entry) = serde_json::from_str::<ModelEntry>(&content) {
                return entry.status == ModelStatus::Ready;
            }
        }
        false
    }

    /// Listar todos los modelos
    pub fn list(&self) -> Result<Vec<ModelEntry>> {
        self.ensure_initialized()?;
        let mut entries = Vec::new();
        for dir in std::fs::read_dir(&self.base_dir)? {
            let dir = dir?;
            if !dir.file_type()?.is_dir() {
                continue;
            }
            let manifest = dir.path().join("manifest.json");
            if manifest.is_file() {
                if let Ok(content) = std::fs::read_to_string(&manifest) {
                    if let Ok(entry) = serde_json::from_str::<ModelEntry>(&content) {
                        entries.push(entry);
                    }
                }
            } else {
                // Directorio sin manifiesto → modelo incompleto
                entries.push(ModelEntry {
                    name: dir.file_name().to_string_lossy().to_string(),
                    revision: "unknown".to_string(),
                    status: ModelStatus::Missing,
                    path: dir.path(),
                    size_bytes: None,
                });
            }
        }
        Ok(entries)
    }

    /// Directorio de un modelo
    pub fn model_dir(&self, model_name: &str) -> PathBuf {
        self.base_dir.join(model_name)
    }

    /// Registrar un modelo como provisionado escribiendo su manifest.json
    pub fn register_provisioned(&self, model_name: &str, revision: &str) -> Result<ModelEntry> {
        self.ensure_initialized()?;
        let dir = self.model_dir(model_name);
        std::fs::create_dir_all(&dir)?;

        let entry = ModelEntry {
            name: model_name.to_string(),
            revision: revision.to_string(),
            status: ModelStatus::Ready,
            path: dir.clone(),
            size_bytes: Some(0),
        };

        let manifest_path = dir.join("manifest.json");
        let content = serde_json::to_string_pretty(&entry)?;
        std::fs::write(&manifest_path, content)?;

        Ok(entry)
    }
}
