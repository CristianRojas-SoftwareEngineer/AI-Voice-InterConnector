//! Módulo `errors.rs` — enum `E_*` + `exit_code()`, testeable puro sin I/O.
//! Cada variante nombra la clase de fallo y fija su código de salida: 2 para
//! entrada, destino o ausencia de audio; 1 para ffmpeg, modelo, runtime ONNX o
//! E/S; 3 para cancelación; 0 solo cuando no hubo fallo.
//!
//! También es el único sitio que traduce cada variante a su `reason` del
//! contrato de la CLI (`reason()`) y a un `CliError` (`to_cli_error()`), que
//! comparten la raíz, el daemon y `speech denoise`.

use avi_core::exit_codes::{CliError, ExitCode};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum E {
    #[error("{0}")]
    EInvalidInput(String),
    /// Lleva la ruta del archivo de salida que ya existe.
    #[error("Ya existe el archivo de salida {0}.")]
    EOutputExists(String),
    #[error("El archivo no contiene audio.")]
    ENoAudio,
    #[error("{0}")]
    EFfmpegNotFound(String),
    #[error("{0}")]
    EModelMissing(String),
    #[error("{0}")]
    EOnnxRuntimeUnavailable(String),
    #[error("{0}")]
    EFfmpegFailed(String),
    /// El texto del error del sistema no sale en el mensaje: va al detalle.
    #[error("No se pudo completar una operación con archivos.")]
    EIo(#[from] std::io::Error),
    #[error("Cancelado por el usuario.")]
    ECancelled,
}

impl E {
    /// Código de salida asociado al error.
    /// ECancelled → 3
    /// EInvalidInput / EOutputExists / ENoAudio → 2
    /// EFfmpegNotFound / EModelMissing / EOnnxRuntimeUnavailable / EFfmpegFailed / EIo → 1
    pub fn exit_code(&self) -> i32 {
        match self {
            E::ECancelled => 3,
            E::EInvalidInput(_) | E::EOutputExists(_) | E::ENoAudio => 2,
            E::EFfmpegNotFound(_)
            | E::EModelMissing(_)
            | E::EOnnxRuntimeUnavailable(_)
            | E::EFfmpegFailed(_)
            | E::EIo(_) => 1,
        }
    }

    /// `reason` del contrato de la CLI para este error. La cancelación no pasa
    /// por aquí en la práctica (la gestiona la ruta de cancelación del lote);
    /// si llegara, cae en `denoise_failed`.
    pub fn reason(&self) -> &'static str {
        match self {
            E::EModelMissing(_) => "model_missing",
            E::EInvalidInput(_) | E::ENoAudio => "invalid_audio",
            E::EOutputExists(_) => "output_exists",
            E::EIo(_) => "io_error",
            E::EFfmpegNotFound(_)
            | E::EOnnxRuntimeUnavailable(_)
            | E::EFfmpegFailed(_)
            | E::ECancelled => "denoise_failed",
        }
    }

    /// Detalle técnico que no se muestra en el texto: el error del sistema de
    /// una operación de E/S.
    pub fn detail(&self) -> Option<String> {
        match self {
            E::EIo(source) => Some(source.to_string()),
            _ => None,
        }
    }

    /// Error de la CLI equivalente: su `reason` es el de `reason()` y su código
    /// el que el contrato asigna a ese `reason` (2 para entrada y destino, 4
    /// para el modelo ausente y 1 para el resto). El código de salida de un
    /// lote de `speech denoise` no sale de aquí sino de `exit_code()`.
    pub fn to_cli_error(&self) -> CliError {
        let reason = self.reason();
        let error = CliError::new(ExitCode::from_reason(reason), reason, self.to_string());
        match self.detail() {
            Some(detail) => error.with_detail(detail),
            None => error,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::E;

    /// Tabla de mapeo de cada variante a su `reason` y su código de salida.
    #[test]
    fn every_variant_maps_to_its_reason_and_exit_code() {
        let io = || E::EIo(std::io::Error::other("fallo"));
        let table: Vec<(E, &str, i32)> = vec![
            (E::EModelMissing("x".into()), "model_missing", 1),
            (E::EInvalidInput("x".into()), "invalid_audio", 2),
            (E::ENoAudio, "invalid_audio", 2),
            (E::EOutputExists("x".into()), "output_exists", 2),
            (io(), "io_error", 1),
            (E::EFfmpegNotFound("x".into()), "denoise_failed", 1),
            (E::EOnnxRuntimeUnavailable("x".into()), "denoise_failed", 1),
            (E::EFfmpegFailed("x".into()), "denoise_failed", 1),
            (E::ECancelled, "denoise_failed", 3),
        ];
        for (error, reason, code) in table {
            assert_eq!(error.reason(), reason, "{error:?}");
            assert_eq!(error.exit_code(), code, "{error:?}");
        }
    }

    /// El `CliError` conserva el código del lote y manda el error del sistema
    /// al detalle, no al mensaje.
    #[test]
    fn cli_error_keeps_the_batch_code_and_moves_library_text_to_detail() {
        let invalid = E::EInvalidInput("Archivo no encontrado: a.wav".into()).to_cli_error();
        assert_eq!(invalid.code.code(), 2);
        assert_eq!(invalid.reason, "invalid_audio");
        assert_eq!(invalid.message, "Archivo no encontrado: a.wav");
        assert!(invalid.detail.is_none());

        let io = E::EIo(std::io::Error::other("os error 5")).to_cli_error();
        assert_eq!(io.code.code(), 1);
        assert_eq!(io.reason, "io_error");
        assert!(!io.message.contains("os error 5"));
        assert_eq!(io.detail.as_deref(), Some("os error 5"));

        let missing = E::EModelMissing("Falta el modelo.".into()).to_cli_error();
        assert_eq!(missing.code.code(), 4);
        assert_eq!(missing.reason, "model_missing");
    }

    #[test]
    fn messages_carry_no_internal_prefixes() {
        assert_eq!(
            E::EOutputExists("a.wav".into()).to_string(),
            "Ya existe el archivo de salida a.wav."
        );
        assert_eq!(E::ENoAudio.to_string(), "El archivo no contiene audio.");
    }
}
