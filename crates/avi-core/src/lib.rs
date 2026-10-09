pub mod engine;
pub mod exit_codes;
pub mod json_emitter;
pub mod limits;

pub use limits::{
    body_limit_for, synthesis_budget, validate_synthesis_text, MAX_CLONE_REFERENCE_SECS,
    MAX_DUB_AUDIO_SECS, MAX_TEXT_LENGTH, MAX_TRANSCRIBE_AUDIO_SECS, REQUEST_FAILSAFE,
};

#[cfg(test)]
mod tests {
    use crate::engine::{HierarchicalSegmenter, Segmenter};
    use crate::exit_codes::ExitCode;
    use crate::json_emitter::{
        envelope_object, render_envelope, with_schema_version, CLI_SCHEMA_VERSION,
        DAEMON_SCHEMA_VERSION,
    };
    use serde_json::{json, Value};

    #[test]
    fn test_exit_codes() {
        assert_eq!(ExitCode::Ok.code(), 0);
        assert_eq!(ExitCode::Error.code(), 1);
        assert_eq!(ExitCode::InvalidInput.code(), 2);
        assert_eq!(ExitCode::NotFound.code(), 3);
        assert_eq!(ExitCode::ModelMissing.code(), 4);
        assert_eq!(ExitCode::DaemonUnreachable.code(), 5);
        assert_eq!(ExitCode::StateConflict.code(), 6);
        assert_eq!(ExitCode::TranslationFailed.code(), 9);
        assert_eq!(ExitCode::TranscriptionFailed.code(), 10);
        // Variantes del ciclo de vida, con el entero que fija la tabla cerrada.
        assert_eq!(ExitCode::SetupFailed.code(), 11);
        assert_eq!(ExitCode::ExternallyManaged.code(), 12);
        assert_eq!(ExitCode::RolledBack.code(), 13);
        assert_eq!(ExitCode::PathConflict.code(), 14);
        assert_eq!(ExitCode::BundleInvalid.code(), 15);
        assert_eq!(ExitCode::DaemonStopFailed.code(), 16);
        assert_eq!(ExitCode::LifecycleLocked.code(), 17);
        assert_eq!(ExitCode::Interrupted.code(), 130);
    }

    #[test]
    fn test_short_text_returns_single_segment() {
        // Un texto que cabe entero se devuelve como único párrafo con un
        // único segmento igual al texto.
        let segmenter = HierarchicalSegmenter::new(200);
        let result = segmenter.segment("Hola, ¿cómo estás?");
        assert_eq!(result, vec![vec!["Hola, ¿cómo estás?"]]);
    }

    #[test]
    fn test_multi_sentence_text_splits_in_es() {
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
    fn test_multi_sentence_text_splits_in_en() {
        // Derivado de `test_multi_sentence_text_splits_by_sentence_via_pysbd_en`:
        // misma invariante estructural que el caso en español; el segmentador es
        // agnóstico de idioma (no detecta el idioma del texto de entrada, por lo
        // que las mismas reglas de segmentación aplican sin distinción).
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
    fn test_preserves_paragraph_order() {
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
    fn test_long_sentence_falls_back_to_strong_punctuation() {
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
    fn test_extreme_case_falls_back_to_tokenizer_without_losing_text() {
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

    #[test]
    fn test_emit_raw_json_includes_schema_version() {
        // Las dos versiones se afirman por separado y con su valor: la de la CLI y
        // la del protocolo del daemon son contratos independientes.
        assert_eq!(CLI_SCHEMA_VERSION, "5");
        assert_eq!(DAEMON_SCHEMA_VERSION, "4");
        let rendered = render_envelope(json!({ "status": "ok" }), CLI_SCHEMA_VERSION);
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            parsed,
            json!({ "status": "ok", "schema_version": CLI_SCHEMA_VERSION })
        );
    }

    #[test]
    fn test_emit_raw_json_preserves_data() {
        let input = json!({ "status": "ok", "count": 42, "label": "test" });
        let rendered = render_envelope(input, CLI_SCHEMA_VERSION);
        // La salida serializada conserva los campos originales y añade la versión.
        let parsed: Value = serde_json::from_str(&rendered).unwrap();
        assert_eq!(
            parsed,
            json!({
                "status": "ok",
                "count": 42,
                "label": "test",
                "schema_version": CLI_SCHEMA_VERSION,
            })
        );
    }

    #[test]
    fn test_emit_raw_json_flatten_schema_version() {
        // `schema_version` es campo raíz con la versión del daemon, no anidado en `data`.
        let map = envelope_object(json!({ "data": { "nested": true } }));
        let val = with_schema_version(map, DAEMON_SCHEMA_VERSION);
        assert_eq!(
            val.get("schema_version").and_then(|v| v.as_str()),
            Some(DAEMON_SCHEMA_VERSION),
            "schema_version debe ser campo raíz con la versión del daemon"
        );
        let nested = val.get("data").and_then(|d| d.get("nested"));
        assert_eq!(
            nested,
            Some(&Value::Bool(true)),
            "los campos anidados en `data` deben preservarse"
        );
    }

    #[test]
    #[should_panic(expected = "un envelope JSON debe ser un objeto")]
    fn test_envelope_rejects_non_object() {
        envelope_object(json!(["no", "objeto"]));
    }
}
