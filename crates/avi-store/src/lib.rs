// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use avi_shared::paths::{
    data_dir, models_cache_dir, models_root_is_shared, xet_cache_dir, MODELS_XET_SUBDIR,
    MODEL_FILE_PATTERNS, MODEL_REVISIONS,
};

/// Voces de fábrica: `ryan`/`vivian` son presets del motor (modelo CustomVoice)
/// sin audio; `default` es voz clonada de fábrica (`.qvoice` con solo la huella
/// de timbre, x-vector, que se sintetiza con el modelo Base y `--graft`) para
/// garantizar `WER ≤0.25` en texto corto. La distinción preset/clonada vive
/// en `avi-tts::resolve_voice_engine` (presencia de `reference.qvoice`).
pub const FACTORY_VOICES: &[&str] = &["default", "ryan", "vivian"];

/// Asset embebido: `.qvoice` de fábrica para `default`, una voz clonada con solo
/// la huella de timbre (x-vector) que se sintetiza con el modelo Base y
/// `--graft`. Se materializa en `ensure_initialized` si falta. Se regenera
/// copiando aquí el `reference.qvoice` que deja en el almacén de voces la orden
/// `ai-voice-interconnector voice clone --no-daemon --name default --force
/// --speech-reference "assets/audios/Speech Sampler.wav"`.
const FACTORY_DEFAULT_QVOICE: &[u8] = include_bytes!("../assets/default/reference.qvoice");

pub fn is_factory_name(name: &str) -> bool {
    FACTORY_VOICES.contains(&name.to_lowercase().as_str())
}

/// Normalización canónica de una entrada de `PATH` para comparación
/// determinista: prefijo verbatim de Windows fuera, `/` → `\`, `lowercase` y separadores
/// **finales** fuera. Hace que `C:\...\Programs/ai-voice-interconnector` y
/// `C:\...\Programs\ai-voice-interconnector\` sean idénticas, y que
/// `\\?\C:\...\ai-voice-interconnector` lo sea también.
///
/// **El separador inicial no se quita nunca**, en ninguna plataforma. Quitarlo
/// tendría dos consecuencias falsas:
///
/// - En Unix convertiría `/home/ana` en `home\ana`, de modo que una ruta absoluta
///   compararía igual a una relativa con el mismo nombre.
/// - En la UNC de Windows `\\servidor\recurso` perdería las dos barras iniciales, que son
///   justo lo que la distingue de una ruta local.
///
/// En Unix los `\` son **caracteres de nombre de archivo legítimos**, así que una ruta
/// que empieza por `\\` no es una verbatim: el prefijo no se quita y lo único que se
/// re-codifica es el separador `/` como `\`, que es lo que permite comparar la forma
/// absoluta con la que escribe el bloque del `PATH`.
///
/// **El prefijo verbatim se quita aquí y no en cada consumidor.** La capa de Windows
/// devuelve `\\?\.` en los modos de apertura extendidos, así que la ruta del ejecutable
/// en ejecución y la del directorio de programa —que viene de una variable de
/// reubicación— llegan con y sin él. Esta función es la fuente única de la semántica de
/// comparación de rutas.
///
/// El prefijo solo se quita en Windows: en Unix no existe, y una ruta que empiece por
/// `\\` es ahí un nombre de archivo legítimo.
pub fn canonical_path_key(p: &Path) -> String {
    without_prefix_verbatim(&p.to_string_lossy())
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
fn without_prefix_verbatim(raw: &str) -> std::borrow::Cow<'_, str> {
    if let Some(unc) = raw.strip_prefix(r"\\?\UNC\") {
        return std::borrow::Cow::Owned(format!(r"\\{unc}"));
    }
    std::borrow::Cow::Borrowed(raw.strip_prefix(r"\\?\").unwrap_or(raw))
}

/// En Unix no hay prefijo verbatim, así que la ruta se devuelve tal cual.
#[cfg(not(windows))]
fn without_prefix_verbatim(raw: &str) -> std::borrow::Cow<'_, str> {
    std::borrow::Cow::Borrowed(raw)
}

/// ¿Son la misma entrada de `PATH` dos rutas escritas de forma distinta?
///
/// La comparación canónica de entradas de `PATH` distingue mayúsculas, ignora
/// separadores
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
        // nombre se lee de la cola **sin** él; mezclarlos dejaría el `}` de cierre
        // dentro del valor expandido en la forma `${VAR}`.
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
/// Layout en disco: `<data_dir>/voices/<nombre>/`
#[derive(Clone)]
pub struct VoiceStore {
    base_dir: PathBuf,
}

impl VoiceStore {
    /// Ancla el almacén en `<data_dir>/voices`; falla si la raíz de datos no se
    /// puede resolver.
    pub fn new() -> Result<Self> {
        Ok(Self::at(data_dir()?.join("voices")))
    }

    /// Ancla el almacén de voces en el directorio indicado (el del propio
    /// almacén, no la raíz de datos).
    pub fn at(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Directorio donde el almacén guarda las voces.
    pub fn root(&self) -> &Path {
        &self.base_dir
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
        // Materializa `default` desde el asset embebido solo si no existe ya un
        // `reference.qvoice`, de modo que la inicialización es idempotente.
        let default_qvoice = self.base_dir.join("default").join("reference.qvoice");
        if !default_qvoice.is_file() {
            std::fs::write(&default_qvoice, FACTORY_DEFAULT_QVOICE)?;
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
                let ref_path = self.find_reference(&name)?;
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

    /// Validar un nombre de voz (regex del contrato `^[A-Za-z0-9._-]+$` +
    /// reglas de seguridad anti-escape)
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

    /// Verificar si una voz existe, con el nombre normalizado a minúsculas, según
    /// el contrato del registro. Inicializa antes el almacén para que las voces
    /// de fábrica existan en una raíz nueva; si la inicialización falla,
    /// devuelve el error.
    pub fn exists(&self, name: &str) -> Result<bool> {
        self.ensure_initialized()?;
        Ok(self.base_dir.join(name.to_lowercase()).is_dir())
    }

    /// Eliminar solo el registro de una voz (no permite eliminar voces de fábrica:
    /// default, ryan, vivian). No toca sus locuciones, así que no es pública: fuera
    /// del crate una voz solo se borra con [`remove_voice`], que se las lleva con ella.
    fn remove(&self, name: &str) -> Result<(), String> {
        let name = self.ensure_removable(name)?;
        std::fs::remove_dir_all(self.base_dir.join(&name)).map_err(|e| e.to_string())?;
        Ok(())
    }

    /// Nombre normalizado de una voz que se puede eliminar: descarta las de fábrica
    /// y las que no existen.
    fn ensure_removable(&self, name: &str) -> Result<String, String> {
        let name = name.to_lowercase();
        if is_factory_name(&name) {
            return Err(format!("La voz '{}' no se puede eliminar.", name));
        }
        if !self.base_dir.join(&name).is_dir() {
            return Err(format!("La voz '{}' no existe.", name));
        }
        Ok(name)
    }

    /// Buscar el archivo de referencia de una voz: `reference.qvoice`.
    /// Solo `reference.qvoice` determina la rama clonada; los presets no
    /// tienen referencia y resuelven como `Preset` en `resolve_voice_engine`.
    /// Inicializa antes el almacén para que `default` exista en una raíz nueva;
    /// si la inicialización falla, devuelve el error.
    pub fn find_reference(&self, name: &str) -> Result<Option<PathBuf>> {
        self.ensure_initialized()?;
        let dir = self.base_dir.join(name.to_lowercase());
        let path = dir.join("reference.qvoice");
        if path.is_file() {
            return Ok(Some(path));
        }
        Ok(None)
    }

    /// Directorio de una voz (nombre normalizado a minúsculas)
    pub fn voice_dir(&self, name: &str) -> PathBuf {
        self.base_dir.join(name.to_lowercase())
    }

    /// Guardar el `.qvoice` clonado como `reference.qvoice` de la voz
    /// (copia con temporal + rename; layout del contrato del registro).
    ///
    /// Con la voz ya existente se sustituye el fichero con un renombrado atómico.
    /// Con una voz nueva el contenido se prepara en un directorio hermano de
    /// `voices/` (misma partición) y se renombra de una vez a `voices/<nombre>/`:
    /// si la copia falla o el proceso muere, no aparece un directorio de voz
    /// vacío que `list` mostraría y que impediría clonar de nuevo con ese nombre.
    pub fn save_reference(&self, name: &str, src: &Path) -> Result<PathBuf> {
        let dir = self.voice_dir(name);
        let dest = dir.join("reference.qvoice");
        if dir.exists() {
            let tmp = dir.join("reference.qvoice.tmp");
            std::fs::copy(src, &tmp)?;
            std::fs::rename(&tmp, &dest)?;
            return Ok(dest);
        }

        let parent = self
            .base_dir
            .parent()
            .ok_or_else(|| anyhow::anyhow!("el directorio de voces no tiene directorio padre"))?;
        let staging = parent.join(format!(
            "avi-voice-staging-{}-{}",
            name.to_lowercase(),
            std::process::id()
        ));
        avi_shared::best_effort(
            std::fs::remove_dir_all(&staging),
            format_args!("no se pudo borrar el staging previo {}", staging.display()),
        );
        std::fs::create_dir_all(&staging)?;
        let published = std::fs::copy(src, staging.join("reference.qvoice"))
            .map_err(anyhow::Error::from)
            .and_then(|_| {
                std::fs::create_dir_all(&self.base_dir)?;
                std::fs::rename(&staging, &dir).map_err(anyhow::Error::from)
            });
        if let Err(e) = published {
            avi_shared::best_effort(
                std::fs::remove_dir_all(&staging),
                format_args!("no se pudo borrar el staging {}", staging.display()),
            );
            return Err(e);
        }
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

/// Entrada de una locución. El WAV es el registro: voz y etiqueta salen de su
/// ruta; los metadatos del sidecar son opcionales y faltan cuando no hay `.json`.
#[derive(Debug, Clone)]
pub struct SpeechEntry {
    pub voice: String,
    pub label: String,
    pub metadata: Option<SpeechMetadata>,
    pub audio_path: PathBuf,
    pub metadata_path: PathBuf,
}

/// El sidecar de una locución existe pero no se puede leer o interpretar.
/// Nombra el fichero para que el llamador pueda indicar cómo resolverlo.
#[derive(Debug)]
pub struct SidecarError {
    pub voice: String,
    pub label: String,
    pub path: PathBuf,
    pub reason: String,
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Los metadatos '{}' de la locución '{}' de la voz '{}' son ilegibles o están corruptos: {}",
            self.path.display(),
            self.label,
            self.voice,
            self.reason
        )
    }
}

impl std::error::Error for SidecarError {}

/// Almacén de habla sintética persistida.
/// Layout en disco: `<data_dir>/speech/<voz>/<etiqueta>.wav` + `<etiqueta>.json`
#[derive(Clone)]
pub struct SpeechStore {
    base_dir: PathBuf,
}

impl SpeechStore {
    /// Ancla el almacén en `<data_dir>/speech`; falla si la raíz de datos no se
    /// puede resolver.
    pub fn new() -> Result<Self> {
        Ok(Self::at(data_dir()?.join("speech")))
    }

    /// Ancla el almacén de habla en el directorio indicado (el del propio
    /// almacén, no la raíz de datos).
    pub fn at(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Directorio donde el almacén guarda las locuciones.
    pub fn root(&self) -> &Path {
        &self.base_dir
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

    /// Recorrido compartido del almacén con filtro opcional por voz: enumera los
    /// WAV, que son el registro; con filtro solo se lee el directorio de la voz
    /// pedida. Una locución sin sidecar aparece sin metadatos; un sidecar
    /// ilegible o corrupto hace fallar el recorrido con un `SidecarError`.
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
                if path.extension().and_then(|e| e.to_str()) != Some("wav") || !path.is_file() {
                    continue;
                }
                let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                    continue;
                };
                let voice = voice_dir.file_name().to_string_lossy().to_lowercase();
                entries.push(Self::entry_for(voice, stem.to_lowercase(), path)?);
            }
        }
        Ok(entries)
    }

    /// Entrada de un WAV existente: lee su sidecar, si lo hay.
    fn entry_for(voice: String, label: String, audio_path: PathBuf) -> Result<SpeechEntry> {
        let metadata_path = audio_path.with_extension("json");
        let metadata = match std::fs::read_to_string(&metadata_path) {
            Ok(content) => Some(
                serde_json::from_str::<SpeechMetadata>(&content).map_err(|e| SidecarError {
                    voice: voice.clone(),
                    label: label.clone(),
                    path: metadata_path.clone(),
                    reason: e.to_string(),
                })?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                return Err(SidecarError {
                    voice,
                    label,
                    path: metadata_path,
                    reason: e.to_string(),
                }
                .into())
            }
        };
        Ok(SpeechEntry {
            voice,
            label,
            metadata,
            audio_path,
            metadata_path,
        })
    }

    /// Directorio para una voz específica (nombre normalizado a minúsculas)
    pub fn voice_dir(&self, voice: &str) -> PathBuf {
        self.base_dir.join(voice.to_lowercase())
    }

    /// Etiquetas de las locuciones de una voz, ordenadas. El WAV es el registro y
    /// no se lee ningún sidecar, de modo que un sidecar corrupto no impide
    /// conocer qué locuciones tiene la voz. Voz sin locuciones → lista vacía.
    pub fn labels_of(&self, voice: &str) -> Result<Vec<String>> {
        let dir = self.voice_dir(voice);
        let mut labels = Vec::new();
        if !dir.is_dir() {
            return Ok(labels);
        }
        for file in std::fs::read_dir(&dir)? {
            let path = file?.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wav") || !path.is_file() {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                labels.push(stem.to_lowercase());
            }
        }
        labels.sort();
        Ok(labels)
    }

    /// Ruta del WAV para una locución (voice/label normalizados)
    pub fn audio_path(&self, voice: &str, label: &str) -> PathBuf {
        self.voice_dir(voice)
            .join(format!("{}.wav", label.to_lowercase()))
    }

    /// Buscar una locución por (voz, etiqueta). La decide el WAV: sin él es
    /// `None`; sin sidecar la entrada no lleva metadatos; con sidecar ilegible o
    /// corrupto devuelve un `SidecarError`.
    pub fn find(&self, voice: &str, label: &str) -> Result<Option<SpeechEntry>> {
        let label = label.to_lowercase();
        let wav_path = self.audio_path(voice, &label);
        if !wav_path.is_file() {
            return Ok(None);
        }
        Self::entry_for(voice.to_lowercase(), label, wav_path).map(Some)
    }

    /// Eliminar una locución: borra el WAV y el sidecar si existen, sin
    /// interpretar el contenido del sidecar.
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

/// Elimina una voz del usuario junto con su namespace de locuciones y devuelve las
/// etiquetas que se llevó. Las locuciones se van con su voz: así no queda ninguna
/// locución de una voz inexistente.
///
/// Las voces de fábrica y las inexistentes se rechazan antes de tocar nada. Las
/// locuciones se borran antes que el registro: si el proceso muere entre ambos
/// pasos queda una voz sin locuciones, que es un estado válido, y nunca locuciones
/// sin voz.
pub fn remove_voice(
    voices: &VoiceStore,
    speech: &SpeechStore,
    name: &str,
) -> Result<Vec<String>, String> {
    let name = voices.ensure_removable(name)?;
    let labels = speech.labels_of(&name).map_err(|e| e.to_string())?;
    let speech_dir = speech.voice_dir(&name);
    if speech_dir.is_dir() {
        std::fs::remove_dir_all(&speech_dir).map_err(|e| e.to_string())?;
    }
    voices.remove(&name)?;
    Ok(labels)
}

// ─── ModelStore ──────────────────────────────────────────────────────

/// Almacén de modelos descargados.
///
/// Fuente de verdad única: snapshots de HuggingFace en la raíz de modelos del
/// propio almacén, con layout `models--<org>--<repo>/snapshots/<hash>/`. Esa raíz
/// la toma `new()` de `models_cache_dir()` y la fija `at()` a mano, de modo que
/// toda lectura y todo borrado del almacén hablan del mismo árbol. Todos los
/// modelos están pinneados en `MODEL_REVISIONS`, así que la provisión se decide
/// solo por presencia del snapshot; no hay índice `manifest.json` intermedio.
pub struct ModelStore {
    base_dir: PathBuf,
}

impl ModelStore {
    /// Ancla el almacén en `models_cache_dir()`, nunca en `data_dir()/models`:
    /// los modelos son caché regenerable de propiedad exclusiva y no estado de
    /// usuario, y la tabla de rutas los coloca en raíces distintas.
    ///
    /// Fija además `HF_XET_CACHE` al subdirectorio `xet` de la raíz cuando esta
    /// es exclusiva. Es la única palanca disponible: `HFClientBuilder::cache_dir`
    /// solo fija la caché de `hub`, de modo que `xet` se sitúa por variable de
    /// entorno; y es la variable que resuelve `xet-runtime` (`HF_XET_CACHE` →
    /// `HF_HOME/xet` → `XDG_CACHE_HOME/huggingface/xet` →
    /// `~/.cache/huggingface/xet`). Con una raíz compartida la variable es del
    /// usuario y no se toca: su contenido no se relocaliza.
    ///
    /// La llamada a `set_var` es segura en la toolchain del proyecto
    /// (edition 2021); pasa a `unsafe` en edition 2024.
    ///
    /// Falla si la raíz de modelos no se puede resolver.
    pub fn new() -> Result<Self> {
        let store = Self::at(models_cache_dir()?);
        if !models_root_is_shared() {
            std::env::set_var("HF_XET_CACHE", store.base_dir.join(MODELS_XET_SUBDIR));
        }
        Ok(store)
    }

    /// Ancla el almacén en una raíz de modelos explícita. La diferencia con
    /// `new()` no es solo el ancla: `new()` además fija `HF_XET_CACHE`, porque
    /// descargar lo necesita, mientras que aquí no se toca el entorno del
    /// proceso. Un sandbox de pruebas o un reporte que debe describir una raíz
    /// ajena a la del usuario construyen el almacén con esta, de modo que leer
    /// el estado de esa raíz nunca dependa de la caché global.
    pub fn at(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    /// Resolución del repo HF y revisión pinneada de un modelo lógico.
    pub fn revision_of(model_name: &str) -> Option<(&'static str, &'static str)> {
        MODEL_REVISIONS
            .iter()
            .find(|pin| pin.name == model_name)
            .map(|pin| (pin.repo, pin.revision))
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
        let repo_dir = self
            .base_dir
            .join(format!("models--{}", repo.replace('/', "--")));
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
    /// exclusiva como en la compartida.
    pub fn remove_hf_snapshot(&self, model_name: &str) -> Result<bool> {
        if let Some((repo, _)) = ModelStore::revision_of(model_name) {
            let dir = self
                .base_dir
                .join(format!("models--{}", repo.replace('/', "--")));
            if dir.is_dir() {
                std::fs::remove_dir_all(&dir)?;
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// Borra la caché `xet` **solo** en la raíz de modelos exclusiva, donde
    /// cuelga de ella. En una raíz compartida devuelve `Ok(false)` sin tocar
    /// nada: `xet` es un subdirectorio global de la caché HF y no se borra,
    /// porque aloja los shards de todos los proyectos que la usan.
    pub fn remove_xet_cache() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let xet = xet_cache_dir()?;
        if xet.is_dir() {
            std::fs::remove_dir_all(&xet)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Borra los locks de descarga (`.locks`) **solo** en la raíz de modelos
    /// exclusiva. En una raíz compartida devuelve `Ok(false)`: el `.locks`
    /// completo no se borra, y los locks de los repos propios se van con
    /// `remove_hf_snapshot`, que sí los alcanza.
    pub fn remove_hf_locks() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let locks = models_cache_dir()?.join(".locks");
        if locks.is_dir() {
            std::fs::remove_dir_all(&locks)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Borra la raíz de modelos **entera** cuando es exclusiva, y `xet`
    /// incluido. Es el alcance que `cleanup --model` y `self uninstall` usan
    /// para dejar residuo cero dentro de las raíces de propiedad exclusiva; con
    /// una raíz compartida devuelve `Ok(false)` y el llamador se limita a los
    /// repos fijados y sus locks.
    pub fn remove_models_root() -> Result<bool> {
        if models_root_is_shared() {
            return Ok(false);
        }
        let root = models_cache_dir()?;
        if root.is_dir() {
            std::fs::remove_dir_all(&root)?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Descarga nativa de un modelo pinneado vía HuggingFace Hub.
    ///
    /// Usa `hf-hub` (`snapshot_download` con revisión de `MODEL_REVISIONS`): cache
    /// estándar en `models_cache_dir()`, ETag/commit-hash y reintentos del propio
    /// crate. No reanuda: ante una descarga interrumpida vuelve a bajar el archivo
    /// completo sobre `.incomplete`. Los punteros de `snapshots/` son symlinks en
    /// Unix y enlaces duros en Windows. La barra `indicatif` refleja archivos/bytes agregados vía
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
        // Cliente propio con `connect_timeout`: sin él, una red agujereada cuelga
        // cada intento hasta el timeout TCP del SO (~76 s en macOS; con los 6
        // intentos de `hf-hub` son ~7,6 min por archivo antes del `network_error`).
        // Solo acota el apretón de manos; las descargas lentas pero vivas no se
        // tocan. Los reintentos de `hf-hub` siguen aplicándose encima.
        let http = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()?;
        let client = hf_hub::HFClient::builder()
            .cache_dir(models_cache_dir()?)
            .client(http)
            .build()?;
        let (owner, name) = hf_hub::split_id(repo_id);
        let repo = client.model(owner, name);
        // allow_patterns acota la descarga a los ficheros que el motor usa
        // (crítico en repos multi-formato).
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
        // Validación: el snapshot debe contener los ficheros críticos con tamaño > 0.
        // Si la descarga dejó un snapshot vacío (hub/xet incoherente), se borra y se
        // falla para que el siguiente intento descargue de nuevo. Los errores de E/S
        // al inspeccionar o limpiar se propagan con su ruta.
        let is_non_empty_file = |p: &Path| -> Result<bool> {
            if !p.is_file() {
                return Ok(false);
            }
            let meta = std::fs::metadata(p).with_context(|| {
                format!("No se pudieron leer los metadatos de '{}'", p.display())
            })?;
            Ok(meta.len() > 0)
        };
        let valid = if let Some(pats) = patterns {
            let mut all = true;
            for pat in &pats {
                if !is_non_empty_file(&snapshot.join(pat))? {
                    all = false;
                    break;
                }
            }
            all
        } else {
            let mut any = false;
            for entry in std::fs::read_dir(&snapshot)
                .with_context(|| format!("No se pudo listar '{}'", snapshot.display()))?
            {
                let entry =
                    entry.with_context(|| format!("No se pudo listar '{}'", snapshot.display()))?;
                if is_non_empty_file(&entry.path())? {
                    any = true;
                    break;
                }
            }
            any
        };
        if !valid {
            std::fs::remove_dir_all(&snapshot).with_context(|| {
                format!(
                    "No se pudo borrar el snapshot incompleto '{}'",
                    snapshot.display()
                )
            })?;
            // Limpiar blobs incompletos del repo si existen
            let repo_dir =
                models_cache_dir()?.join(format!("models--{}", repo_id.replace('/', "--")));
            let blobs = repo_dir.join("blobs");
            if blobs.is_dir() {
                for entry in std::fs::read_dir(&blobs)
                    .with_context(|| format!("No se pudo listar '{}'", blobs.display()))?
                {
                    let path = entry
                        .with_context(|| format!("No se pudo listar '{}'", blobs.display()))?
                        .path();
                    if path.extension().and_then(|e| e.to_str()) == Some("incomplete") {
                        std::fs::remove_file(&path).with_context(|| {
                            format!("No se pudo borrar el blob incompleto '{}'", path.display())
                        })?;
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
                    // El acumulador por archivo solo lo escribe este handler, así
                    // que un veneno no lo invalida: un `unwrap()` mataría la
                    // descarga por un pánico de otro hilo. Se registra la traza
                    // y la barra sigue avanzando sobre el estado recuperado.
                    let mut map = self.per_file.lock().unwrap_or_else(|poisoned| {
                        tracing::warn!("mutex del progreso por archivo envenenado; se recupera");
                        poisoned.into_inner()
                    });
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
    use avi_shared::paths::{bin_dir, install_dir, shared_hf_root, APP_NAME};
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
    /// la aplicación**: nunca la caché compartida de HuggingFace, y con
    /// `xet` colgando de ella.
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
        let root = models_cache_dir().unwrap();
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
        assert_eq!(xet_cache_dir().unwrap(), root.join(MODELS_XET_SUBDIR));

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
        assert_eq!(
            models_cache_dir().unwrap(),
            PathBuf::from(r"C:\cache_custom\hub")
        );
        assert!(models_root_is_shared());

        std::env::remove_var("HF_HUB_CACHE");
        assert_eq!(
            shared_hf_root(),
            Some(PathBuf::from("/hf_home_custom").join("hub"))
        );
        assert_eq!(
            models_cache_dir().unwrap(),
            PathBuf::from("/hf_home_custom").join("hub")
        );

        std::env::set_var("AVI_CACHE_DIR", "/isolated_root");
        assert_eq!(models_cache_dir().unwrap(), PathBuf::from("/isolated_root"));
        assert!(
            models_root_is_shared(),
            "reubicar no cambia la propiedad: el contenido sigue siendo compartido"
        );

        env_restore(saved);
    }

    /// Directorio de programa y directorio del enlace según la tabla de rutas, en
    /// los dos layouts, y su reubicación por `AVI_INSTALL_DIR`/`AVI_BIN_DIR`.
    #[test]
    fn install_and_bin_dirs_per_platform() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_INSTALL_DIR", "AVI_BIN_DIR", "LOCALAPPDATA"]);
        std::env::remove_var("AVI_INSTALL_DIR");
        std::env::remove_var("AVI_BIN_DIR");
        // La aserción no puede depender de la máquina del que corre el test.
        std::env::set_var(
            "LOCALAPPDATA",
            std::env::temp_dir().join("AppData").join("Local"),
        );

        let (install, bin) = (install_dir().unwrap(), bin_dir().unwrap());
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

        std::env::set_var("AVI_INSTALL_DIR", "/opt/avi_relocated");
        std::env::set_var("AVI_BIN_DIR", "/usr/local/bin");
        assert_eq!(install_dir().unwrap(), PathBuf::from("/opt/avi_relocated"));
        assert_eq!(bin_dir().unwrap(), PathBuf::from("/usr/local/bin"));
        if cfg!(windows) {
            // Sin reubicar el enlace, la entrada del PATH cae en el programa.
            std::env::remove_var("AVI_BIN_DIR");
            assert_eq!(bin_dir().unwrap(), PathBuf::from("/opt/avi_relocated"));
        }

        env_restore(saved);
    }

    /// `xet` cuelga de la raíz de modelos exclusiva y `ModelStore::new()` la
    /// fija en `HF_XET_CACHE` porque es la única palanca disponible. Con raíz
    /// compartida la variable es del usuario y no se toca: `xet` sigue la
    /// cadena real de `xet-runtime` y queda fuera de cualquier borrado.
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

        let root = models_cache_dir().unwrap();
        assert_eq!(xet_cache_dir().unwrap(), root.join(MODELS_XET_SUBDIR));
        assert!(xet_cache_dir().unwrap().starts_with(&root));

        let _store = ModelStore::new().unwrap();
        assert_eq!(
            std::env::var("HF_XET_CACHE").ok(),
            Some(root.join(MODELS_XET_SUBDIR).to_string_lossy().to_string()),
            "`HFClientBuilder::cache_dir` solo fija `hub`: la variable es la única palanca"
        );

        std::env::set_var("HF_HOME", "/hf_home_custom");
        std::env::set_var("HF_XET_CACHE", "/user_xet");
        assert_eq!(
            models_cache_dir().unwrap(),
            PathBuf::from("/hf_home_custom").join("hub")
        );
        assert_eq!(xet_cache_dir().unwrap(), PathBuf::from("/user_xet"));
        let _store = ModelStore::new().unwrap();
        assert_eq!(
            std::env::var("HF_XET_CACHE").ok(),
            Some("/user_xet".to_string()),
            "con raíz compartida la variable es del usuario y no se toca"
        );

        std::env::remove_var("HF_XET_CACHE");
        assert_eq!(
            xet_cache_dir().unwrap(),
            PathBuf::from("/hf_home_custom").join("xet"),
            "sin HF_XET_CACHE, `xet-runtime` cae a HF_HOME/xet"
        );

        env_restore(saved);
    }

    /// La comparación de entradas de `PATH` no da un falso negativo
    /// con una entrada escrita con una variable sin expandir, que es como se
    /// lee el valor `Path` del registro de Windows.
    #[test]
    fn canonical_path_entry_comparison_handles_percent_vars() {
        let _guard = ENV_LOCK.lock().unwrap();
        let saved = env_guard(&["AVI_TEST_PATH_ROOT"]);
        let root_dir = tempfile::Builder::new()
            .prefix("avi_store_path_entry_")
            .tempdir()
            .unwrap();
        let root = root_dir.path().to_path_buf();
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
            &root.join("Programs").join("other")
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
        let without_prefix =
            Path::new(r"C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector");
        let with_prefix =
            Path::new(r"\\?\C:\Users\ana\AppData\Local\Programs\ai-voice-interconnector");
        assert_eq!(
            canonical_path_key(with_prefix),
            canonical_path_key(without_prefix)
        );
        assert!(
            canonical_path_entry_matches(with_prefix, without_prefix),
            "el comparador de entradas hereda la normalización"
        );
        assert!(canonical_path_entry_matches(without_prefix, with_prefix));

        // Con separadores mixtos y barra final, que es como suele venir de verdad.
        assert_eq!(
            canonical_path_key(Path::new(r"\\?\C:\Users\ana\AppData\Local\Programs\")),
            canonical_path_key(Path::new(r"C:\Users\ana\AppData\Local\Programs")),
            "el prefijo se quita antes que cualquier otra normalización"
        );

        // La forma de red: `\\?\UNC\server\resource` es `\\server\resource`, con la
        // barra inicial, que es la que la distingue de una ruta local.
        assert_eq!(
            canonical_path_key(Path::new(r"\\?\UNC\server\resource\bin")),
            canonical_path_key(Path::new(r"\\server\resource\bin")),
            "la UNC verbatim se normaliza a la UNC normal"
        );
        assert_ne!(
            canonical_path_key(Path::new(r"\\?\UNC\server\resource\bin")),
            canonical_path_key(Path::new(r"C:\server\resource\bin")),
            "y no se confunde con una ruta local del mismo nombre"
        );

        // Una ruta que solo se parece no se toca: el prefijo no amplía lo que compara
        // igual.
        assert_ne!(
            canonical_path_key(Path::new(r"\\?\C:\other-program")),
            canonical_path_key(Path::new(r"C:\ai-voice-interconnector"))
        );

        // Una ruta sin prefijo no cambia por quitar el prefijo verbatim.
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

    /// Directorio de trabajo de una prueba con dueño: se borra al soltarse el valor,
    /// también cuando la prueba falla.
    struct OwnedDir(tempfile::TempDir);

    impl std::ops::Deref for OwnedDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            self.0.path()
        }
    }

    fn temp_dir(tag: &str) -> OwnedDir {
        OwnedDir(
            tempfile::Builder::new()
                .prefix(&format!("avi_store_test_{tag}_"))
                .tempdir()
                .unwrap(),
        )
    }

    /// Normalización de mayúsculas en todas las operaciones del almacén, según
    /// el contrato del registro.
    #[test]
    fn normalization_lowercase() {
        let dir = temp_dir("norm");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();

        let saved = speech
            .save("VIVIAN", "TestGreeting", "Hola", &wav_src)
            .unwrap();
        let rel = saved
            .strip_prefix(dir.join("speech"))
            .unwrap()
            .to_string_lossy()
            .to_string();
        assert_eq!(rel.replace('\\', "/"), "vivian/testgreeting.wav");

        assert!(speech.find("vivian", "testgreeting").unwrap().is_some());
        assert!(speech.find("VIVIAN", "TESTGREETING").unwrap().is_some());
        let entries = speech.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].voice, "vivian");
        assert_eq!(entries[0].label, "testgreeting");

        speech.remove("VIVIAN", "TESTGREETING").unwrap();
        assert!(speech.find("vivian", "testgreeting").unwrap().is_none());

        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        voices.ensure_initialized().unwrap();
        let vdir = voices.voice_dir("MyVoice");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("reference.qvoice"), b"QVCE").unwrap();
        assert!(
            voices.exists("MYVOICE").unwrap(),
            "exists debe normalizar a minúsculas"
        );
        assert_eq!(
            voices
                .find_reference("MYVOICE")
                .unwrap()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "reference.qvoice"
        );
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
        speech.save("ryan", "greeting", "Hola", &wav_src).unwrap();
        speech
            .save("vivian", "farewell", "Adiós", &wav_src)
            .unwrap();

        let ryan = speech.list_by_voice("RYAN").unwrap();
        assert_eq!(ryan.len(), 1);
        assert_eq!(ryan[0].voice, "ryan");
        assert_eq!(ryan[0].label, "greeting");

        let vivian = speech.list_by_voice("vivian").unwrap();
        assert_eq!(vivian.len(), 1);
        assert_eq!(vivian[0].label, "farewell");

        assert!(speech.list_by_voice("nonexistent").unwrap().is_empty());
        assert_eq!(speech.list().unwrap().len(), 2);
    }

    /// `labels_of` lista los WAV de la voz ordenados y sin leer sidecars; una voz
    /// sin namespace da lista vacía.
    #[test]
    fn labels_of_lists_wavs_without_reading_sidecars() {
        let dir = temp_dir("labels_of");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "zeta", "Z", &wav_src).unwrap();
        speech.save("ryan", "Alfa", "A", &wav_src).unwrap();
        std::fs::write(speech.voice_dir("ryan").join("zeta.json"), b"{roto").unwrap();

        assert_eq!(speech.labels_of("RYAN").unwrap(), vec!["alfa", "zeta"]);
        assert!(speech.labels_of("vivian").unwrap().is_empty());
    }

    /// `remove_voice` borra el registro de la voz y su namespace de locuciones y
    /// devuelve las etiquetas; rechaza fábrica e inexistentes sin tocar nada.
    #[test]
    fn remove_voice_drags_speech_namespace() {
        let dir = temp_dir("remove_voice");
        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        voices.ensure_initialized().unwrap();
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        std::fs::create_dir_all(voices.voice_dir("locutor")).unwrap();
        std::fs::create_dir_all(voices.voice_dir("otra")).unwrap();
        speech.save("locutor", "b", "B", &wav_src).unwrap();
        speech.save("locutor", "a", "A", &wav_src).unwrap();
        speech.save("otra", "c", "C", &wav_src).unwrap();
        speech.save("default", "d", "D", &wav_src).unwrap();

        // Fábrica e inexistente: error y nada borrado.
        assert!(remove_voice(&voices, &speech, "default").is_err());
        assert!(remove_voice(&voices, &speech, "noexiste").is_err());
        assert!(speech.find("default", "d").unwrap().is_some());

        let labels = remove_voice(&voices, &speech, "LOCUTOR").unwrap();
        assert_eq!(labels, vec!["a", "b"]);
        assert!(!voices.voice_dir("locutor").exists());
        assert!(!speech.voice_dir("locutor").exists());
        // Las demás voces y sus locuciones quedan intactas.
        assert!(voices.voice_dir("otra").is_dir());
        assert!(speech.find("otra", "c").unwrap().is_some());
        assert!(speech.find("default", "d").unwrap().is_some());

        // Voz sin namespace de locuciones: se borra con lista vacía.
        assert!(remove_voice(&voices, &speech, "otra").is_ok());
        std::fs::create_dir_all(voices.voice_dir("sola")).unwrap();
        assert!(remove_voice(&voices, &speech, "sola").unwrap().is_empty());
    }

    /// round-trip `save` → `find` con `duration_secs` calculada del WAV.
    #[test]
    fn save_find_round_trip_with_duration() {
        let dir = temp_dir("roundtrip");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();

        let path = speech.save("ryan", "greeting", "Hola", &wav_src).unwrap();
        assert!(path.is_file());
        let entry = speech
            .find("ryan", "greeting")
            .unwrap()
            .expect("la locución debe existir");
        let metadata = entry.metadata.expect("la locución guardada lleva sidecar");
        assert_eq!(metadata.text, "Hola");
        // 1 muestra a 24 kHz → 1/24000 s
        assert!((metadata.duration_secs - 1.0 / 24_000.0).abs() < 1e-9);
    }

    /// Sin sidecar la locución sigue en el registro (la decide el WAV): `list` y
    /// `find` la devuelven sin metadatos.
    #[test]
    fn sidecar_missing_lists_speech_without_metadata() {
        let dir = temp_dir("sidecar_missing");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "greeting", "Hola", &wav_src).unwrap();
        std::fs::remove_file(speech.voice_dir("ryan").join("greeting.json")).unwrap();

        let entries = speech.list().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].voice, "ryan");
        assert_eq!(entries[0].label, "greeting");
        assert!(entries[0].metadata.is_none());

        let found = speech.find("ryan", "greeting").unwrap().unwrap();
        assert!(found.metadata.is_none());
    }

    /// Un sidecar huérfano (sin WAV) no es una locución: ni `list` ni `find` lo ven.
    #[test]
    fn orphan_sidecar_is_not_an_entry() {
        let dir = temp_dir("sidecar_orphan");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "greeting", "Hola", &wav_src).unwrap();
        std::fs::remove_file(speech.audio_path("ryan", "greeting")).unwrap();

        assert!(speech.list().unwrap().is_empty());
        assert!(speech.find("ryan", "greeting").unwrap().is_none());
    }

    /// Un sidecar corrupto hace fallar `list` y `find` con un `SidecarError` que
    /// nombra el fichero; `remove` borra ambos ficheros sin interpretarlo.
    #[test]
    fn sidecar_corrupt_fails_list_and_find_but_remove_succeeds() {
        let dir = temp_dir("sidecar_corrupt");
        let speech = SpeechStore::with_base_dir(dir.join("speech"));
        let wav_src = dir.join("src.wav");
        std::fs::write(&wav_src, min_wav()).unwrap();
        speech.save("ryan", "greeting", "Hola", &wav_src).unwrap();
        let sidecar = speech.voice_dir("ryan").join("greeting.json");
        std::fs::write(&sidecar, b"{roto").unwrap();

        for err in [
            speech.list().unwrap_err(),
            speech.list_by_voice("ryan").unwrap_err(),
            speech.find("ryan", "greeting").unwrap_err(),
        ] {
            let sidecar_err = err
                .downcast_ref::<SidecarError>()
                .expect("el error debe ser un SidecarError");
            assert_eq!(sidecar_err.path, sidecar);
            assert_eq!(sidecar_err.label, "greeting");
            assert_eq!(sidecar_err.voice, "ryan");
            assert!(err.to_string().contains("greeting.json"), "{err}");
        }

        speech.remove("ryan", "greeting").unwrap();
        assert!(!sidecar.exists());
        assert!(!speech.audio_path("ryan", "greeting").exists());
    }

    /// `validate_name` acepta el regex del contrato y rechaza lo demás.
    #[test]
    fn validate_name_accepts_contract_regex() {
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
        let src = dir.join("clone.qvoice");
        std::fs::write(&src, b"QVCE").unwrap();
        let saved = voices.save_reference("MyVoice", &src).unwrap();
        assert_eq!(
            saved.file_name().unwrap().to_string_lossy(),
            "reference.qvoice"
        );
        assert!(saved.is_file());
        assert_eq!(voices.find_reference("myvoice").unwrap().unwrap(), saved);

        // Sin fallback wav: solo qvoice es canónico.
        let vdir = voices.voice_dir("other");
        std::fs::create_dir_all(&vdir).unwrap();
        std::fs::write(vdir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert!(
            voices.find_reference("OTHER").unwrap().is_none(),
            "sin reference.qvoice no resuelve como clonada"
        );
    }

    /// Guardar la referencia de una voz nueva desde un origen inexistente
    /// devuelve error y no deja ni el directorio de la voz ni una entrada en
    /// el listado.
    #[test]
    fn save_reference_missing_source_leaves_no_voice_dir() {
        let dir = temp_dir("ref_missing_new");
        let voices_dir = dir.join("voices");
        std::fs::create_dir_all(&voices_dir).unwrap();
        let voices = VoiceStore::with_base_dir(voices_dir);
        let missing = dir.join("missing.qvoice");
        assert!(voices.save_reference("fresh", &missing).is_err());
        assert!(
            !voices.voice_dir("fresh").exists(),
            "un guardado fallido no debe dejar el directorio de la voz"
        );
        assert!(
            voices.list().unwrap().iter().all(|v| v.name != "fresh"),
            "un guardado fallido no debe dejar la voz en el listado"
        );
    }

    /// Con la voz ya existente, un guardado fallido por origen inexistente
    /// conserva su `reference.qvoice` previo y su directorio.
    #[test]
    fn save_reference_missing_source_keeps_existing_reference() {
        let dir = temp_dir("ref_missing_existing");
        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        let src = dir.join("clone.qvoice");
        std::fs::write(&src, b"QVCE").unwrap();
        let saved = voices.save_reference("existing", &src).unwrap();
        let missing = dir.join("missing.qvoice");
        assert!(voices.save_reference("existing", &missing).is_err());
        assert!(voices.voice_dir("existing").is_dir());
        assert_eq!(std::fs::read(&saved).unwrap(), b"QVCE");
    }

    /// La consulta de existencia garantiza las voces de fábrica en una raíz
    /// recién creada, sin necesidad de listar antes; una voz ajena no existe.
    #[test]
    fn exists_guarantees_factory_voices_on_fresh_root() {
        let dir = temp_dir("exists-fresh");
        let voices = VoiceStore::with_base_dir(dir.join("voices"));
        for name in ["default", "ryan", "vivian"] {
            assert!(voices.exists(name).unwrap(), "'{}' debe existir", name);
        }
        assert!(!voices.exists("nadie").unwrap());
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
                voices.exists(name).unwrap(),
                "exists('{}') debe ser true",
                name
            );
            // No debe haber WAV de referencia materializado
            assert!(
                !d.join("speech-reference.wav").is_file(),
                "no debe materializar speech-reference.wav para '{}'",
                name
            );
        }
        // `default` clonada de fábrica debe tener `reference.qvoice` materializado
        assert!(
            voices.find_reference("default").unwrap().is_some(),
            "fábrica 'default' debe tener reference.qvoice"
        );
        assert_eq!(
            voices
                .find_reference("default")
                .unwrap()
                .unwrap()
                .file_name()
                .unwrap()
                .to_string_lossy(),
            "reference.qvoice"
        );
        // `ryan`/`vivian` siguen presets puros sin referencia
        for name in ["ryan", "vivian"] {
            assert!(
                voices.find_reference(name).unwrap().is_none(),
                "fábrica '{}' sin qvoice no debe tener referencia (preset puro)",
                name
            );
        }
        // Idempotencia: segunda inicialización no borra directorios ni datos de usuario
        let custom_dir = voices.voice_dir("myvoice");
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
        // Voz sin qvoice no resuelve como clonada aunque tenga un wav inerte
        let clone_dir = voices.voice_dir("other");
        std::fs::create_dir_all(&clone_dir).unwrap();
        std::fs::write(clone_dir.join("speech-reference.wav"), b"RIFF").unwrap();
        assert!(
            voices.find_reference("OTHER").unwrap().is_none(),
            "sin qvoice no resuelve como clonada"
        );
    }

    /// Sobre una raíz nueva, `find_reference("default")` inicializa el almacén
    /// y devuelve el `.qvoice` de fábrica materializado.
    #[test]
    fn find_reference_default_initializes_fresh_root() {
        let dir = temp_dir("find_ref_fresh");
        let voices = VoiceStore::at(dir.join("voices"));
        let path = voices
            .find_reference("default")
            .unwrap()
            .expect("default debe resolver en una raíz nueva");
        assert!(path.is_file(), "el reference.qvoice debe existir en disco");
    }

    /// Si la base del almacén es un fichero, inicializar es imposible y tanto
    /// `ensure_initialized` como `find_reference` devuelven error.
    #[test]
    fn base_dir_being_a_file_fails_initialization() {
        let dir = temp_dir("base_is_file");
        let base = dir.join("voices");
        std::fs::write(&base, b"no soy un directorio").unwrap();
        let voices = VoiceStore::at(base);
        assert!(voices.ensure_initialized().is_err());
        assert!(voices.find_reference("default").is_err());
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
        std::env::set_var("AVI_CACHE_DIR", tmp.as_os_str());
        std::env::remove_var("HF_HUB_CACHE");
        std::env::remove_var("HF_HOME");
        let store = ModelStore::new().unwrap();
        assert_eq!(models_cache_dir().unwrap(), &*tmp);
        // Crear snapshot vacío con al menos un fichero para que is_provisioned sea true
        let (repo2, rev2) = ModelStore::revision_of("qwen3-tts-0.6b-base").unwrap();
        let repo_dir = models_cache_dir()
            .unwrap()
            .join(format!("models--{}", repo2.replace('/', "--")));
        let snap = repo_dir.join("snapshots").join(rev2);
        std::fs::create_dir_all(&snap).unwrap();
        std::fs::write(snap.join("config.json"), br#"{"tts_model_type":"base"}"#).unwrap();
        assert!(store.is_provisioned("qwen3-tts-0.6b-base"));
        let resolved = store.model_snapshot_path("qwen3-tts-0.6b-base").unwrap();
        assert!(resolved.is_dir());
        env_restore(saved);
    }
}
