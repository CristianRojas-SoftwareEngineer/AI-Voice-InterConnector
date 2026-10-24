//! Módulo `errors.rs` — enum `E_*` + `exit_code()`, testeable puro sin I/O.
//! Cada variante nombra la clase de fallo y fija su código de salida: 2 para
//! entrada, destino o ausencia de audio; 1 para ffmpeg, modelo, runtime ONNX o
//! E/S; 3 para cancelación; 0 solo cuando no hubo fallo.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum E {
    #[error("Entrada inválida: {0}")]
    EInvalidInput(String),
    #[error("Destino ya existe: {0}")]
    EOutputExists(String),
    #[error("Sin pista de audio")]
    ENoAudio,
    #[error("ffmpeg no encontrado: {0}")]
    EFfmpegNotFound(String),
    #[error("Modelo faltante: {0}")]
    EModelMissing(String),
    #[error("{0}")]
    EOnnxRuntimeUnavailable(String),
    #[error("ffmpeg falló: {0}")]
    EFfmpegFailed(String),
    #[error("Error de E/S: {0}")]
    EIo(#[from] std::io::Error),
    #[error("Cancelado por el usuario")]
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
}
