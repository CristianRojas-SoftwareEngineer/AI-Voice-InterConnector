use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use avi_store::data_dir;

/// Configuración de preferencias del sistema
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub default_language: String,
    pub default_voice: String,
    pub audio_device: Option<String>,
    pub daemon_port: u16,
    pub sample_rate: u32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            default_language: "es".to_string(),
            default_voice: "default".to_string(),
            audio_device: None,
            daemon_port: 8765,
            sample_rate: 16000,
        }
    }
}

pub struct ConfigManager {
    config_path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Self {
        let config_path = data_dir().join("config.toml");
        Self { config_path }
    }

    /// Cargar la configuración desde disco o crear la default si no existe
    pub fn load_or_default(&self) -> AppConfig {
        if self.config_path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&self.config_path) {
                if let Ok(config) = toml::from_str::<AppConfig>(&content) {
                    return config;
                }
            }
        }
        let default_config = AppConfig::default();
        let _ = self.save(&default_config);
        default_config
    }

    /// Guardar la configuración en disco en formato TOML
    pub fn save(&self, config: &AppConfig) -> Result<()> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(config)?;
        std::fs::write(&self.config_path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_toml_config_serialization() {
        let config = crate::AppConfig::default();
        let serialized = toml::to_string_pretty(&config).unwrap();
        assert!(serialized.contains("default_language = \"es\""));
        assert!(serialized.contains("daemon_port = 8765"));

        let deserialized: crate::AppConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(deserialized.default_language, "es");
        assert_eq!(deserialized.daemon_port, 8765);
    }
}
