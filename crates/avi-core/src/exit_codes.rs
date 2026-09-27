use thiserror::Error;

/// Códigos de salida del binario del producto.
///
/// Los enteros del 0 al 10 y el 130 son los del contrato de la CLI y no cambian. Del 11
/// al 17 hay **una variante por cada `reason` nuevo que declara el ciclo de vida**,
/// con el orden y los enteros que fija la tabla cerrada del plan: `setup_failed`,
/// `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`,
/// `daemon_stop_failed` y `lifecycle_locked`. Del 18 al 21, los que el Ciclo 2 declara
/// para `self update` (red e integridad): `unsupported_platform`,
/// `binary_incompatible`, `network_error` y `checksum_mismatch`.
///
/// **`unsupported_platform` tiene variante propia desde el Ciclo 2** y sale con 18:
/// el ciclo que declara también `binary_incompatible` es el que elige su entero, y
/// ese ciclo es este.
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
}
