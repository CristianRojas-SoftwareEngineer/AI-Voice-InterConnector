pub mod engine;
pub mod exit_codes;
pub mod json_emitter;

#[cfg(test)]
mod tests {
    use crate::engine::{HierarchicalSegmenter, Segmenter};
    use crate::exit_codes::ExitCode;

    #[test]
    fn test_exit_codes() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Error.code(), 1);
        assert_eq!(ExitCode::InvalidInput.code(), 2);
        assert_eq!(ExitCode::NotFound.code(), 3);
        assert_eq!(ExitCode::ModelMissing.code(), 4);
        assert_eq!(ExitCode::DaemonUnreachable.code(), 5);
        assert_eq!(ExitCode::StateConflict.code(), 6);
        assert_eq!(ExitCode::NotApplicable.code(), 7);
        assert_eq!(ExitCode::PreconditionFailed.code(), 8);
        assert_eq!(ExitCode::TranslationFailed.code(), 9);
        assert_eq!(ExitCode::TranscriptionFailed.code(), 10);
        assert_eq!(ExitCode::Interrupted.code(), 130);
    }

    #[test]
    fn test_texto_corto_devuelve_segmento_unico() {
        // Derivado de `test_short_text_returns_single_segment`
        // (`tests/test_translation_segmenter.py`): un texto que cabe entero se
        // devuelve como único párrafo con un único segmento igual al texto.
        let segmenter = HierarchicalSegmenter::new(200);
        let result = segmenter.segment("Hola, ¿cómo estás?");
        assert_eq!(result, vec![vec!["Hola, ¿cómo estás?"]]);
    }

    #[test]
    fn test_texto_multi_oracion_se_particiona_en_es() {
        // Derivado de `test_multi_sentence_text_splits_by_sentence_via_pysbd_es`:
        // invariante estructural (el nivel de oración no es `pysbd`): un único
        // párrafo que excede `max_length` se parte en más de un segmento y
        // ninguno es igual al texto completo.
        let segmenter = HierarchicalSegmenter::new(30);
        let text = "Hola mundo. Esta es la segunda oración. Y una tercera aquí.";
        let result = segmenter.segment(text);

        assert_eq!(result.len(), 1, "un solo párrafo");
        let sentences = &result[0];
        assert!(sentences.len() > 1, "debe partirse en más de un segmento");
        assert!(
            sentences.iter().all(|s| s != text),
            "ningún segmento debe ser igual al texto completo"
        );
    }

    #[test]
    fn test_texto_multi_oracion_se_particiona_en_en() {
        // Derivado de `test_multi_sentence_text_splits_by_sentence_via_pysbd_en`:
        // misma invariante estructural que el caso en español; el segmentador es
        // agnóstico de idioma (limitación documentada en la Tarea 2 del plan).
        let segmenter = HierarchicalSegmenter::new(35);
        let text = "Hello world. This is the second sentence. And a third one here.";
        let result = segmenter.segment(text);

        assert_eq!(result.len(), 1, "un solo párrafo");
        let sentences = &result[0];
        assert!(sentences.len() > 1, "debe partirse en más de un segmento");
        assert!(
            sentences.iter().all(|s| s != text),
            "ningún segmento debe ser igual al texto completo"
        );
    }

    #[test]
    fn test_preserva_orden_de_parrafos() {
        // Derivado de `test_preserves_paragraph_order`: un texto con `"\n\n"`
        // produce tantos párrafos como separadores + 1, en el mismo orden, con
        // el primer y último segmento reconocibles por prefijo.
        let segmenter = HierarchicalSegmenter::new(10);
        let result = segmenter.segment("Uno. Dos.\n\nTres. Cuatro.");

        assert_eq!(result.len(), 2, "dos párrafos");
        assert!(result[0][0].starts_with("Uno"));
        assert!(result[1][result[1].len() - 1].starts_with("Cuatro"));
    }

    #[test]
    fn test_oracion_larga_cae_a_puntuacion_fuerte() {
        // Derivado de `test_exceptionally_long_sentence_falls_back_to_strong_punctuation`:
        // una "oración" sin punto final pero con comas que excede el límite cae
        // al fallback de puntuación fuerte (más de un segmento) sin perder texto.
        let segmenter = HierarchicalSegmenter::new(25);
        let text = "Primero esto, luego lo otro, y finalmente esto de aquí";
        let result = segmenter.segment(text);

        let sentences = &result[0];
        assert!(
            sentences.len() > 1,
            "debe caer al fallback de puntuación fuerte"
        );
        let joined = sentences.join(" ");
        assert!(
            joined.contains("Primero esto"),
            "no debe perderse el texto inicial"
        );
        assert!(
            joined.contains("finalmente esto de aquí"),
            "no debe perderse el texto final"
        );
    }

    #[test]
    fn test_caso_extremo_cae_a_tokens_sin_perder_texto() {
        // Derivado de `test_extreme_case_falls_back_to_tokenizer_without_losing_text`:
        // sin puntuación utilizable, cae al fallback por tokens; cada segmento
        // cumple el límite o es un único token que lo excede, y ninguna palabra
        // del texto original se pierde.
        let segmenter = HierarchicalSegmenter::new(10);
        let text = "una palabra muy larga sin puntuacion que fuerza el ultimo recurso";
        let result = segmenter.segment(text);

        let sentences = &result[0];
        assert!(sentences.len() > 1, "debe caer al fallback por tokens");
        for chunk in sentences {
            assert!(
                chunk.chars().count() <= 10 || !chunk.contains(' '),
                "cada segmento debe caber en el límite o ser un token indivisible"
            );
        }
        let joined = sentences.join(" ");
        for word in text.split(' ') {
            assert!(
                joined.contains(word),
                "no debe perderse la palabra '{}'",
                word
            );
        }
    }
}
