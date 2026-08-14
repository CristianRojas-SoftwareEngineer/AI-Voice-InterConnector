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
                let name = entry.file_name().to_string_lossy().to_string().to_lowercase();
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

    /// Validar un nombre de voz (regex del oráculo `^[A-Za-z0-9._-]+$` +
    /// reglas de seguridad anti-escape; paridad de contrato, divergencia 3 de F1)
    pub fn validate_name(name: &str) -> Result<(), String> {
        if name.is_empty() {
            return Err("El nombre de la voz no puede estar vacío.".into());
        }
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-') {
            return Err(format!("El nombre de voz '{}' contiene caracteres no permitidos.", name));
        }
        if name.contains('/') || name.contains('\\') || name.contains("..") || name.contains('\0') {
            return Err(format!("El nombre de voz '{}' contiene caracteres no permitidos.", name));
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

    /// Eliminar una voz (no permite eliminar "default")
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let name = name.to_lowercase();
        if name == "default" {
            return Err("La voz 'default' no se puede eliminar.".into());
        }
        let dir = self.base_dir.join(&name);
        if !dir.is_dir() {
            return Err(format!("La voz '{}' no existe.", name));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Buscar el archivo de referencia de una voz: `reference.qvoice`,
    /// `reference.wav` legado o `speech-reference.wav` (nombre normalizado)
    pub fn find_reference(&self, name: &str) -> Option<PathBuf> {
        let dir = self.base_dir.join(name.to_lowercase());
        for ext in &["qvoice", "wav"] {
            let path = dir.join(format!("reference.{}", ext));
            if path.is_file() {
                return Some(path);
            }
        }
        let legacy = dir.join("speech-reference.wav");
        if legacy.is_file() {
            return Some(legacy);
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
        self.voice_dir(voice).join(format!("{}.wav", label.to_lowercase()))
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let dir = std::env::temp_dir().join(format!(
            "avi_store_test_{}_{}",
            tag,
            std::process::id()
        ));
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

        let saved = speech.save("VIVIAN", "SaludoDePrueba", "Hola", &wav_src).unwrap();
        let rel = saved.strip_prefix(dir.join("speech")).unwrap().to_string_lossy().to_string();
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
        assert!(voices.exists("MIVOZ"), "exists debe normalizar a minúsculas");
        assert_eq!(
            voices.find_reference("MIVOZ").unwrap().file_name().unwrap().to_string_lossy(),
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
        let entry = speech.find("ryan", "saludo").expect("la locución debe existir");
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
        assert!(VoiceStore::validate_name("mi voz").is_err(), "espacios fuera del regex");
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

        // Fallback a speech-reference.wav (legado del oráculo).
        let vdir = voices.voice_dir("otra");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert_eq!(
            voices.find_reference("OTRA").unwrap().file_name().unwrap().to_string_lossy(),
            "speech-reference.wav"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
