pub mod engine;
pub mod exit_codes;
pub mod json_emitter;

#[cfg(test)]
mod tests {
    use crate::engine::{DeterministicSegmenter, Segmenter};
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
    fn test_deterministic_segmenter() {
        let segmenter = DeterministicSegmenter;
        let sentences = segmenter.segment("Hola mundo. Esta es una prueba! Funciona bien?");
        assert_eq!(sentences, vec!["Hola mundo", "Esta es una prueba", "Funciona bien"]);
    }
}
