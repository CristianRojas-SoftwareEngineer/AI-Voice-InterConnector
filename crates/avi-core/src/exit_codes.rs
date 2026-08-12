use thiserror::Error;

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
    #[error("Interrupción por el usuario")]
    Interrupted = 130,
}

impl ExitCode {
    pub fn code(&self) -> i32 {
        *self as i32
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

    pub fn invalid_input(reason: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ExitCode::InvalidInput, reason, message)
    }

    pub fn not_found(reason: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(ExitCode::NotFound, reason, message)
    }
}
