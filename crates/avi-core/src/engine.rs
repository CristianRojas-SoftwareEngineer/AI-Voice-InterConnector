use serde_json::Value;

pub trait SttEngine: Send + Sync {
    fn transcribe(&self, audio_pcm: &[i16], language: Option<&str>) -> anyhow::Result<String>;
}

pub trait TranslationEngine: Send + Sync {
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> anyhow::Result<String>;
}

pub trait Segmenter: Send + Sync {
    fn segment(&self, text: &str) -> Vec<String>;
}

pub struct DeterministicSegmenter;

impl Segmenter for DeterministicSegmenter {
    fn segment(&self, text: &str) -> Vec<String> {
        if text.trim().is_empty() {
            return vec![];
        }
        text.split(&['.', '!', '?'][..])
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }
}

pub struct DummySttEngine;

impl SttEngine for DummySttEngine {
    fn transcribe(&self, _audio_pcm: &[i16], _language: Option<&str>) -> anyhow::Result<String> {
        Ok("Transcripción de prueba".to_string())
    }
}

pub struct DummyTranslationEngine;

impl TranslationEngine for DummyTranslationEngine {
    fn translate(&self, text: &str, source_lang: &str, target_lang: &str) -> anyhow::Result<String> {
        if source_lang == target_lang {
            return Ok(text.to_string());
        }
        Ok(format!("[{}->{}] {}", source_lang, target_lang, text))
    }
}
