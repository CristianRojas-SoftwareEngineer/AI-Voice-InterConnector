use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Opciones de generación para la síntesis de voz (API agnóstica del motor)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GenerationOptions {
    pub language: String,
    pub temperature: f32,
    pub top_k: u32,
    pub top_p: f32,
    pub seed: Option<u64>,
}

impl Default for GenerationOptions {
    fn default() -> Self {
        Self {
            language: "es".to_string(),
            temperature: 0.7,
            top_k: 50,
            top_p: 0.9,
            seed: None,
        }
    }
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

/// Motor Qwen3-TTS con soporte de fallback por subprocess o servidor HTTP local
pub struct Qwen3TtsEngine {
    pub server_url: Option<String>,
    pub binary_path: Option<PathBuf>,
}

impl Qwen3TtsEngine {
    pub fn new(server_url: Option<String>) -> Self {
        let binary_path = std::env::var_os("QWEN3_TTS_BIN").map(PathBuf::from);
        Self {
            server_url,
            binary_path,
        }
    }

    /// Intentar la síntesis vía HTTP local si hay servidor activo
    fn synthesize_via_http(
        &self,
        server_url: &str,
        text: &str,
        voice: &str,
        out_path: &Path,
    ) -> Result<()> {
        let client = reqwest::blocking::Client::new();
        let resp = client
            .post(format!("{}/v1/tts", server_url))
            .json(&serde_json::json!({
                "text": text,
                "voice": voice,
                "format": "wav"
            }))
            .send()?;

        if resp.status().is_success() {
            let bytes = resp.bytes()?;
            std::fs::write(out_path, bytes)?;
            Ok(())
        } else {
            Err(anyhow!(
                "Servidor HTTP Qwen3-TTS devolvió código de error: {}",
                resp.status()
            ))
        }
    }

    /// Intentar la síntesis vía invocación de subprocess binario
    fn synthesize_via_subprocess(
        &self,
        bin: &Path,
        text: &str,
        voice: &str,
        out_path: &Path,
    ) -> Result<()> {
        let status = Command::new(bin)
            .arg("--text")
            .arg(text)
            .arg("--voice")
            .arg(voice)
            .arg("--output")
            .arg(out_path)
            .status()?;

        if status.success() {
            Ok(())
        } else {
            Err(anyhow!(
                "Subproceso Qwen3-TTS finalizó con código de error: {:?}",
                status.code()
            ))
        }
    }
}

impl TtsEngine for Qwen3TtsEngine {
    fn synthesize(
        &self,
        text: &str,
        voice: &str,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        let default_options = GenerationOptions::default();
        let profile = VoiceProfile {
            name: voice.to_string(),
            reference_audio: None,
            qvoice_path: None,
        };
        self.synthesize_with_options(text, &profile, &default_options, output_path)
    }

    fn synthesize_with_options(
        &self,
        text: &str,
        profile: &VoiceProfile,
        _options: &GenerationOptions,
        output_path: Option<&PathBuf>,
    ) -> Result<PathBuf> {
        let path = output_path
            .cloned()
            .unwrap_or_else(|| PathBuf::from("output.wav"));

        // 1. Intentar vía HTTP si hay server_url configurado
        if let Some(url) = &self.server_url {
            if self
                .synthesize_via_http(url, text, &profile.name, &path)
                .is_ok()
            {
                return Ok(path);
            }
        }

        // 2. Intentar vía binario de subproceso si está en env
        if let Some(bin) = &self.binary_path {
            if bin.is_file() {
                self.synthesize_via_subprocess(bin, text, &profile.name, &path)?;
                return Ok(path);
            }
        }

        // 3. Si no hay binario ni servidor disponible, el modelo de inferencia no está provisionado
        Err(anyhow!(
            "El modelo o binario de síntesis Qwen3-TTS no está provisionado."
        ))
    }
}
