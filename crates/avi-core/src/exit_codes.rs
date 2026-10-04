use thiserror::Error;

/// Códigos de salida del binario del producto.
///
/// Los enteros del 0 al 10 y el 130 son los del contrato de la CLI y no cambian. Del 11
/// al 17 hay **una variante por cada `reason` nuevo que declara el ciclo de vida**,
/// con el orden y los enteros que fija la tabla cerrada del plan: `setup_failed`,
/// `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`,
/// `daemon_stop_failed` y `lifecycle_locked`. Del 18 al 21, los de `self update`
/// (red e integridad): `unsupported_platform`,
/// `binary_incompatible`, `network_error` y `checksum_mismatch`. El 22 es `program_dir_kept`
/// de `self uninstall`.
///
/// **`unsupported_platform` tiene variante propia** y sale con 18:
/// su entero lo fija la misma tabla cerrada que declara `binary_incompatible`.
#[derive(Error, Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitCode {
    #[error("Éxito")]
    Ok = 0,
    #[error("Error genérico")]
    Error = 1,
    #[error("Entrada inválida")]
    InvalidInput = 2,
    #[error("Recurso no encontrado")]
    NotFound = 3,
    #[error("Modelo no provisionado (ejecutar 'setup')")]
    ModelMissing = 4,
    #[error("Daemon inalcanzable o no gestionable")]
    DaemonUnreachable = 5,
    #[error("Conflicto de estado")]
    StateConflict = 6,
    #[error("Operación no aplicable al contexto actual")]
    NotApplicable = 7,
    #[error("Precondición de entorno incumplida")]
    PreconditionFailed = 8,
    #[error("Fallo del pipeline de traducción")]
    TranslationFailed = 9,
    #[error("Fallo del pipeline de transcripción")]
    TranscriptionFailed = 10,
    #[error("Provisión de modelos fallida (reintentable con 'setup')")]
    SetupFailed = 11,
    #[error("La copia la gestiona otra herramienta")]
    ExternallyManaged = 12,
    #[error("Fallo en el reemplazo; versión anterior restaurada")]
    RolledBack = 13,
    #[error("Conflicto en la ruta del enlace del PATH")]
    PathConflict = 14,
    #[error("Falta un archivo obligatorio del bundle")]
    BundleInvalid = 15,
    #[error("No se pudo detener el daemon; nada modificado")]
    DaemonStopFailed = 16,
    #[error("Hay otra operación de ciclo de vida en curso")]
    LifecycleLocked = 17,
    #[error("Plataforma no soportada (compilar desde el código fuente)")]
    UnsupportedPlatform = 18,
    #[error("Binario descargado incompatible")]
    BinaryIncompatible = 19,
    #[error("Fallo de red")]
    NetworkError = 20,
    #[error("El archivo descargado no coincide con su hash")]
    ChecksumMismatch = 21,
    #[error("El directorio de programa no se pudo borrar")]
    ProgramDirKept = 22,
    #[error("Interrupción por el usuario")]
    Interrupted = 130,
}

impl ExitCode {
    pub fn code(&self) -> i32 {
        *self as i32
    }

    /// Traduce un `reason` del contrato a su código de salida.
    ///
    /// Es la única traducción de `reason` a código de la CLI: la usan los clientes de
    /// la vía daemon, el ciclo de vida y las pruebas, así que añadir un `reason` con
    /// código propio solo exige tocar este `match`. Un `reason` desconocido se trata
    /// como ausente y sale con `ExitCode::Error`.
    pub fn from_reason(reason: &str) -> ExitCode {
        match reason {
            "confirmation_required"
            | "usage_error"
            | "daemon_not_supported"
            | "empty_text"
            | "text_too_long"
            | "unsupported_language_pair"
            | "invalid_voice_name"
            | "invalid_audio"
            | "audio_too_long" => ExitCode::InvalidInput,
            "voice_not_found" => ExitCode::NotFound,
            "model_missing" => ExitCode::ModelMissing,
            "voice_exists" => ExitCode::StateConflict,
            "translation_failed" => ExitCode::TranslationFailed,
            "transcription_failed" => ExitCode::TranscriptionFailed,
            "setup_failed" => ExitCode::SetupFailed,
            "externally_managed" => ExitCode::ExternallyManaged,
            "rolled_back" => ExitCode::RolledBack,
            "path_conflict" => ExitCode::PathConflict,
            "bundle_invalid" => ExitCode::BundleInvalid,
            "daemon_stop_failed" => ExitCode::DaemonStopFailed,
            "lifecycle_locked" => ExitCode::LifecycleLocked,
            "unsupported_platform" => ExitCode::UnsupportedPlatform,
            "binary_incompatible" => ExitCode::BinaryIncompatible,
            "network_error" => ExitCode::NetworkError,
            "checksum_mismatch" => ExitCode::ChecksumMismatch,
            "program_dir_kept" => ExitCode::ProgramDirKept,
            _ => ExitCode::Error,
        }
    }
}

#[derive(Error, Debug)]
#[error("{message}")]
pub struct CliError {
    pub code: ExitCode,
    pub reason: String,
    pub message: String,
}

impl CliError {
    pub fn new(code: ExitCode, reason: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            message: message.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ExitCode;

    /// Oráculo literal de la traducción de `reason` a código de salida. Los enteros se
    /// escriben a mano, no a través de las variantes, para que un intercambio de
    /// valores en el enum también haga fallar la prueba.
    const ORACLE: &[(&str, i32)] = &[
        ("confirmation_required", 2),
        ("usage_error", 2),
        ("daemon_not_supported", 2),
        ("empty_text", 2),
        ("text_too_long", 2),
        ("unsupported_language_pair", 2),
        ("invalid_voice_name", 2),
        ("invalid_audio", 2),
        ("audio_too_long", 2),
        ("voice_not_found", 3),
        ("model_missing", 4),
        ("voice_exists", 6),
        ("translation_failed", 9),
        ("transcription_failed", 10),
        ("setup_failed", 11),
        ("externally_managed", 12),
        ("rolled_back", 13),
        ("path_conflict", 14),
        ("bundle_invalid", 15),
        ("daemon_stop_failed", 16),
        ("lifecycle_locked", 17),
        ("unsupported_platform", 18),
        ("binary_incompatible", 19),
        ("network_error", 20),
        ("checksum_mismatch", 21),
        ("program_dir_kept", 22),
        ("synthesis_failed", 1),
        ("synthesis_timeout", 1),
        ("stt_unsupported", 1),
        ("translation_unsupported", 1),
        ("daemon_error", 1),
        ("sudo_not_supported", 1),
    ];

    #[test]
    fn from_reason_matches_oracle() {
        for (reason, code) in ORACLE {
            assert_eq!(
                ExitCode::from_reason(reason).code(),
                *code,
                "el reason {reason} debe salir con {code}"
            );
        }
    }

    #[test]
    fn from_reason_unknown_is_generic_error() {
        assert_eq!(ExitCode::from_reason("motivo_del_futuro"), ExitCode::Error);
    }
}
