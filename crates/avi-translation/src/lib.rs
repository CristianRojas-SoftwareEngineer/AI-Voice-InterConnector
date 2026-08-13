//! Motor de traducción (Marian/opus-mt vía `ct2rs`).
//!
//! Expone `Ct2TranslationEngine`, implementación de
//! `avi_core::engine::TranslationEngine` que carga un modelo opus-mt convertido
//! a CT2 y traduce texto replicando la tokenización del oráculo Python
//! (`_MarianCT2Model.translate`): SentencePiece embebido + token `</s>` manual,
//! sin `sacremoses` ni `MarianTokenizer`.

use avi_core::engine::{HierarchicalSegmenter, Segmenter, TranslationEngine};
use ct2rs::{Config, Translator};

/// Motor de traducción real sobre un modelo Marian/opus-mt en formato CT2.
pub struct Ct2TranslationEngine {
    translator: Translator<ct2rs::tokenizers::auto::Tokenizer>,
}

impl Ct2TranslationEngine {
    /// Carga el modelo CT2 ubicado en `model_dir`.
    pub fn new(model_dir: impl AsRef<std::path::Path>) -> anyhow::Result<Self> {
        let translator = Translator::new(model_dir, &Config::default())?;
        Ok(Self { translator })
    }
}

impl TranslationEngine for Ct2TranslationEngine {
    fn translate(&self, text: &str, _source_lang: &str, _target_lang: &str) -> anyhow::Result<String> {
        // El motor se instancia para una dirección fija según el `model_dir`
        // con el que se construyó; `source_lang`/`target_lang` no se usan aquí
        // (mismo patrón que `DummyTranslationEngine` ignorando parámetros no
        // aplicables). Se anexa `</s>` manualmente al origen: el encoder
        // Marian/opus-mt lo exige y `ct2-transformers-converter` no lo añade
        // automáticamente (ver nota técnica en `crates/avi-stt/src/lib.rs`).
        let source = format!("{} </s>", text);
        let results = self
            .translator
            .translate_batch(&[source], &Default::default(), None)?;
        if results.is_empty() {
            anyhow::bail!("translate_batch no devolvió ningún resultado");
        }
        let (translated, _) = &results[0];
        // La hipótesis del decoder termina con el token `</s>` (EOS), que el
        // detokenizador de ct2rs reconstruye como texto literal; el oráculo lo
        // elimina al decodificar con el SentencePiece destino (los símbolos de
        // control decodifican a cadena vacía, `model_loader.py`). Se sanea aquí
        // para preservar la paridad de salida (hallazgo del reality-check de F5).
        let translated = translated.trim_end_matches("</s>").trim_end().to_string();
        Ok(translated)
    }
}

/// Traduce `text` de `source` a `target` segmentando jerárquicamente y
/// reensamblando el resultado igual que el oráculo (`SegmentAssembler`):
/// segmentos unidos con espacio dentro de cada párrafo y párrafos unidos con
/// `"\n\n"`. Precondición: `source != target` — el passthrough se resuelve en
/// la capa CLI antes de llamar a esta función.
pub fn translate(
    text: &str,
    source: &str,
    target: &str,
    model_dir: impl AsRef<std::path::Path>,
) -> anyhow::Result<String> {
    let engine = Ct2TranslationEngine::new(model_dir)?;
    let segmenter = HierarchicalSegmenter::default();
    let paragraphs = segmenter.segment(text);

    let translated: Vec<Vec<String>> = paragraphs
        .into_iter()
        .map(|segments| {
            segments
                .iter()
                .map(|segment| engine.translate(segment, source, target))
                .collect::<anyhow::Result<Vec<_>>>()
        })
        .collect::<anyhow::Result<Vec<_>>>()?;

    Ok(translated
        .into_iter()
        .map(|segments| segments.join(" "))
        .collect::<Vec<_>>()
        .join("\n\n"))
}

#[cfg(test)]
mod tests {
    use avi_core::engine::TranslationEngine;

    /// Carga el modelo opus-mt es→en real (ya convertido a CT2 y provisionado)
    /// vía `Ct2TranslationEngine` y traduce un texto corto, verificando que el
    /// resultado no esté vacío.
    #[test]
    fn ct2translationengine_traduce_texto_real() {
        use crate::Ct2TranslationEngine;

        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/ct2/opus-mt-es-en"
        );

        let engine = Ct2TranslationEngine::new(model_dir)
            .expect("el modelo opus-mt-es-en debe cargar pesos CT2 reales desde disco");
        let translated = engine
            .translate("Hola, ¿cómo estás?", "es", "en")
            .expect("la traducción debe completarse");

        assert!(
            !translated.trim().is_empty(),
            "la traducción no debe estar vacía"
        );
        assert!(
            !translated.contains("</s>"),
            "el token EOS de origen no debe filtrarse a la salida"
        );
    }

    /// `Ct2TranslationEngine::new` sobre una ruta de modelo inexistente debe
    /// devolver `Err`, mismo patrón que
    /// `ct2sttengine_new_con_ruta_inexistente_devuelve_err` de `avi-stt`.
    #[test]
    fn ct2translationengine_new_con_ruta_inexistente_devuelve_err() {
        use crate::Ct2TranslationEngine;

        let result = Ct2TranslationEngine::new("ruta/que/no/existe/opus-mt-es-en");
        assert!(result.is_err(), "una ruta de modelo inexistente debe fallar");
    }

    /// Test de paridad contra el oráculo Python (Decisión cerrada #2 de F0).
    ///
    /// IGNORADO: la precondición no se cumple en este entorno — el modelo del
    /// oráculo se resuelve en `<data_root>/translation-models/opus-mt-es-en`
    /// (directorio de datos de usuario), no en `models/ct2/opus-mt-es-en`
    /// (formato CT2, raíz del repo), y no está provisionado. Detalle completo
    /// del hallazgo en `F1-exploracion.md` (fase 4 de traducción). La paridad
    /// textual se difiere al reality-check de F5.
    #[test]
    #[ignore = "fixture de paridad no generado: modelo del oráculo Python no provisionado en este entorno, ver F1-exploracion.md"]
    fn ct2translationengine_coincide_con_oraculo_python() {
        use crate::Ct2TranslationEngine;

        let model_dir = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../models/ct2/opus-mt-es-en"
        );
        let fixture_path = concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/assets/translate_es_en.oraculo.txt"
        );

        let engine = Ct2TranslationEngine::new(model_dir)
            .expect("el modelo opus-mt-es-en debe cargar");
        let actual = engine
            .translate("Hola, ¿cómo estás?", "es", "en")
            .expect("la traducción debe completarse")
            .trim()
            .to_string();

        let esperado = std::fs::read_to_string(fixture_path)
            .expect("el fixture de referencia del oráculo debe existir")
            .trim()
            .to_string();

        if actual == esperado {
            return;
        }

        // Igualdad textual no se cumple: umbral de paridad por WER a nivel de
        // palabra (distancia de Levenshtein entre secuencias de palabras).
        let ref_palabras: Vec<&str> = esperado.split_whitespace().collect();
        let hip_palabras: Vec<&str> = actual.split_whitespace().collect();
        let distancia = levenshtein_palabras(&ref_palabras, &hip_palabras);
        let wer = distancia as f64 / ref_palabras.len().max(1) as f64;

        assert!(
            wer <= 0.05,
            "WER {:.4} supera el umbral de paridad 0.05 (esperado: {:?}, obtenido: {:?})",
            wer,
            esperado,
            actual
        );
    }

    /// Distancia de Levenshtein a nivel de palabra entre `referencia` e
    /// `hipotesis`, usada para calcular el WER de la prueba de paridad.
    fn levenshtein_palabras(referencia: &[&str], hipotesis: &[&str]) -> usize {
        let n = referencia.len();
        let m = hipotesis.len();
        let mut dp = vec![vec![0usize; m + 1]; n + 1];

        for i in 0..=n {
            dp[i][0] = i;
        }
        for j in 0..=m {
            dp[0][j] = j;
        }
        for i in 1..=n {
            for j in 1..=m {
                if referencia[i - 1] == hipotesis[j - 1] {
                    dp[i][j] = dp[i - 1][j - 1];
                } else {
                    dp[i][j] = 1 + dp[i - 1][j - 1].min(dp[i - 1][j]).min(dp[i][j - 1]);
                }
            }
        }
        dp[n][m]
    }

    /// End-to-end con el modelo real: un texto multi-párrafo (`"\n\n"` presente
    /// en la entrada) se traduce sin panic, con resultado no vacío y
    /// preservando la separación de párrafos en la salida.
    #[test]
    fn translate_multi_parrafo_preserva_separadores() {
        let result = crate::translate(
            "Hola, ¿cómo estás?\n\nBuenos días, señor.",
            "es",
            "en",
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../models/ct2/opus-mt-es-en"
            ),
        );

        let translated = result.expect("la traducción multi-párrafo debe completarse");
        assert!(!translated.trim().is_empty(), "el resultado no debe estar vacío");
        assert!(
            translated.contains("\n\n"),
            "la separación de párrafos debe preservarse en la salida"
        );
    }

    /// Cobertura acordada del exit 9 (`TranslationFailed`, ver Tarea 9 del
    /// plan): un `model_dir` inexistente hace fallar el pipeline con `Err`
    /// (defensa en profundidad; no sustituye el chequeo `Path::exists` de la
    /// capa CLI).
    #[test]
    fn translate_con_model_dir_inexistente_devuelve_err() {
        let result = crate::translate(
            "Hola, ¿cómo estás?",
            "es",
            "en",
            "ruta/que/no/existe/opus-mt-es-en",
        );
        assert!(result.is_err(), "un model_dir inexistente debe fallar");
    }
}