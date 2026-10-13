// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

use thiserror::Error;

/// Códigos de salida del binario del producto.
///
/// Los enteros son contrato estable de la CLI: un valor no cambia ni se reutiliza.
/// Se reparten por familia:
///
/// - 0 al 6, 9 y 10: los comandos de uso (éxito, error genérico, entrada inválida,
///   recurso ausente, modelo ausente, daemon inalcanzable, conflicto de estado, y
///   fallo de traducción y de transcripción).
/// - 11 al 17: el ciclo de vida (`setup`, `self install`, `self update`,
///   `self uninstall`, `cleanup`), una variante por `reason`: `setup_failed`,
///   `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`,
///   `daemon_stop_failed` y `lifecycle_locked`.
/// - 18 al 21: la red y la integridad de `self update`: `unsupported_platform`,
///   `binary_incompatible`, `network_error` y `checksum_mismatch`.
/// - 22: `program_dir_kept`, de `self uninstall`.
/// - 130: interrupción por el usuario.
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
    /// código propio solo exige tocar `REASON_CODES`. Un `reason` desconocido se trata
    /// como ausente y sale con `ExitCode::Error`.
    pub fn from_reason(reason: &str) -> ExitCode {
        REASON_CODES
            .iter()
            .find(|(declared, _)| *declared == reason)
            .map_or(ExitCode::Error, |(_, code)| *code)
    }

    /// ¿Está `reason` declarado en `REASON_CODES`? Distingue un `reason` declarado con
    /// el código 1 de uno desconocido, que `from_reason` también resuelve a 1.
    pub fn is_declared_reason(reason: &str) -> bool {
        REASON_CODES.iter().any(|(declared, _)| *declared == reason)
    }
}

/// Tabla de todos los `reason` que emiten la CLI, el daemon y el ciclo de vida, con
/// su código de salida. Un `reason` emitido que no esté aquí o un `reason` de aquí que
/// nadie emita hacen fallar las pruebas de conformidad.
pub const REASON_CODES: &[(&str, ExitCode)] = &[
    ("confirmation_required", ExitCode::InvalidInput),
    ("usage_error", ExitCode::InvalidInput),
    ("empty_text", ExitCode::InvalidInput),
    ("text_too_long", ExitCode::InvalidInput),
    ("invalid_voice_name", ExitCode::InvalidInput),
    ("invalid_identifier", ExitCode::InvalidInput),
    ("cannot_remove_default", ExitCode::InvalidInput),
    ("invalid_audio", ExitCode::InvalidInput),
    ("audio_too_long", ExitCode::InvalidInput),
    ("voice_not_found", ExitCode::NotFound),
    ("warm_voice_missing", ExitCode::NotFound),
    ("audio_not_found", ExitCode::NotFound),
    ("speech_not_found", ExitCode::NotFound),
    ("not_installed", ExitCode::NotFound),
    ("model_missing", ExitCode::ModelMissing),
    ("daemon_unreachable", ExitCode::DaemonUnreachable),
    ("voice_exists", ExitCode::StateConflict),
    ("label_exists", ExitCode::StateConflict),
    ("port_in_use", ExitCode::StateConflict),
    ("translation_failed", ExitCode::TranslationFailed),
    ("transcription_failed", ExitCode::TranscriptionFailed),
    ("setup_failed", ExitCode::SetupFailed),
    ("externally_managed", ExitCode::ExternallyManaged),
    ("rolled_back", ExitCode::RolledBack),
    ("path_conflict", ExitCode::PathConflict),
    ("bundle_invalid", ExitCode::BundleInvalid),
    ("daemon_stop_failed", ExitCode::DaemonStopFailed),
    ("lifecycle_locked", ExitCode::LifecycleLocked),
    ("unsupported_platform", ExitCode::UnsupportedPlatform),
    ("binary_incompatible", ExitCode::BinaryIncompatible),
    ("network_error", ExitCode::NetworkError),
    ("checksum_mismatch", ExitCode::ChecksumMismatch),
    ("program_dir_kept", ExitCode::ProgramDirKept),
    ("io_error", ExitCode::Error),
    ("voice_clone_failed", ExitCode::Error),
    ("synthesis_failed", ExitCode::Error),
    ("synthesis_timeout", ExitCode::Error),
    ("stt_unsupported", ExitCode::Error),
    ("translation_unsupported", ExitCode::Error),
    ("playback_failed", ExitCode::Error),
    ("audio_enumeration_failed", ExitCode::Error),
    ("speech_list_failed", ExitCode::Error),
    ("voice_list_failed", ExitCode::Error),
    ("voice_store_init_failed", ExitCode::Error),
    ("daemon_error", ExitCode::Error),
    ("startup_failed", ExitCode::Error),
    ("sudo_not_supported", ExitCode::Error),
    ("lifecycle_failed", ExitCode::Error),
    ("doctor_failed", ExitCode::Error),
    ("self_install_failed", ExitCode::Error),
    ("self_uninstall_failed", ExitCode::Error),
    ("self_update_failed", ExitCode::Error),
];

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
        ("empty_text", 2),
        ("text_too_long", 2),
        ("invalid_voice_name", 2),
        ("invalid_identifier", 2),
        ("cannot_remove_default", 2),
        ("invalid_audio", 2),
        ("audio_too_long", 2),
        ("voice_not_found", 3),
        ("warm_voice_missing", 3),
        ("audio_not_found", 3),
        ("speech_not_found", 3),
        ("not_installed", 3),
        ("model_missing", 4),
        ("daemon_unreachable", 5),
        ("voice_exists", 6),
        ("label_exists", 6),
        ("port_in_use", 6),
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
        ("io_error", 1),
        ("voice_clone_failed", 1),
        ("synthesis_failed", 1),
        ("synthesis_timeout", 1),
        ("stt_unsupported", 1),
        ("translation_unsupported", 1),
        ("playback_failed", 1),
        ("audio_enumeration_failed", 1),
        ("speech_list_failed", 1),
        ("voice_list_failed", 1),
        ("voice_store_init_failed", 1),
        ("daemon_error", 1),
        ("startup_failed", 1),
        ("sudo_not_supported", 1),
        ("lifecycle_failed", 1),
        ("doctor_failed", 1),
        ("self_install_failed", 1),
        ("self_uninstall_failed", 1),
        ("self_update_failed", 1),
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

    /// El oráculo y la tabla declaran exactamente los mismos `reason`: ninguno sobra
    /// ni falta en un lado.
    #[test]
    fn oracle_and_table_declare_the_same_reasons() {
        assert_eq!(ORACLE.len(), super::REASON_CODES.len());
        for (reason, _) in super::REASON_CODES {
            assert!(
                ORACLE.iter().any(|(r, _)| r == reason),
                "el reason {reason} de la tabla no está en el oráculo"
            );
        }
    }

    #[test]
    fn from_reason_unknown_is_generic_error() {
        assert_eq!(ExitCode::from_reason("motivo_del_futuro"), ExitCode::Error);
    }
}
