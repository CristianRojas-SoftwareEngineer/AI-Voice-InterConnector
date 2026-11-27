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
/// - `denoise_busy` se traduce a `StateConflict` (6): conflicto transitorio de
///   recurso, no fallo del DSP.
/// - 11 al 17: el ciclo de vida (`setup`, `self install`, `self update`,
///   `self uninstall`, `cleanup`), una variante por `reason`: `setup_failed`,
///   `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`,
///   `daemon_stop_failed` y `lifecycle_locked`.
/// - 1 también para `model_unavailable` (`setup`, y anidado en `self install` y
///   `self update`) y `release_unavailable` (`self update` y los instaladores): el
///   servidor rechaza el modelo fijado o el release pedido, y reintentar no lo
///   resuelve.
/// - 18 al 21: la red y la integridad de `self update`: `unsupported_platform`,
///   `binary_incompatible`, `network_error` (solo fallo de transporte o del
///   servidor, que reintentar puede resolver) y `checksum_mismatch`.
/// - 22: `program_dir_kept`, de `self uninstall`.
/// - 23: `cleanup_incomplete`, de `cleanup`: borró lo que pudo y quedaron rutas sin
///   borrar.
/// - 24: `uninstall_incomplete`, de `self uninstall`: desinstaló lo que pudo y
///   quedaron rutas sin borrar.
/// - 1 también para `speech_metadata_corrupt`: los metadatos de un audio
///   sintetizado guardado están dañados y no se pueden leer.
/// - 2 también para `output_exists`: `speech denoise` no sobrescribe un archivo de
///   salida que ya existe sin `--overwrite` ni `--skip-existing`.
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
    #[error("Modelo no provisionado")]
    ModelMissing = 4,
    #[error("Daemon inalcanzable o no gestionable")]
    DaemonUnreachable = 5,
    #[error("Conflicto de estado")]
    StateConflict = 6,
    #[error("Fallo del pipeline de traducción")]
    TranslationFailed = 9,
    #[error("Fallo del pipeline de transcripción")]
    TranscriptionFailed = 10,
    #[error("Provisión de modelos fallida (el programa queda instalado)")]
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
    #[error("La limpieza terminó con rutas sin borrar")]
    CleanupIncomplete = 23,
    #[error("La desinstalación terminó con rutas sin borrar")]
    UninstallIncomplete = 24,
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
    ("cannot_clone_default", ExitCode::InvalidInput),
    ("invalid_audio", ExitCode::InvalidInput),
    ("empty_audio", ExitCode::InvalidInput),
    ("empty_transcript", ExitCode::InvalidInput),
    ("audio_too_long", ExitCode::InvalidInput),
    ("voice_not_found", ExitCode::NotFound),
    ("warm_voice_missing", ExitCode::NotFound),
    ("audio_not_found", ExitCode::NotFound),
    ("speech_not_found", ExitCode::NotFound),
    ("not_installed", ExitCode::NotFound),
    ("no_input_device", ExitCode::NotFound),
    ("no_output_device", ExitCode::NotFound),
    ("audio_backend_unavailable", ExitCode::NotFound),
    ("model_missing", ExitCode::ModelMissing),
    ("daemon_unreachable", ExitCode::DaemonUnreachable),
    ("daemon_version_mismatch", ExitCode::DaemonUnreachable),
    ("voice_exists", ExitCode::StateConflict),
    ("label_exists", ExitCode::StateConflict),
    ("voice_changed", ExitCode::StateConflict),
    ("port_in_use", ExitCode::StateConflict),
    ("denoise_busy", ExitCode::StateConflict),
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
    ("cleanup_incomplete", ExitCode::CleanupIncomplete),
    ("uninstall_incomplete", ExitCode::UninstallIncomplete),
    ("io_error", ExitCode::Error),
    ("model_unavailable", ExitCode::Error),
    ("release_unavailable", ExitCode::Error),
    ("voice_clone_failed", ExitCode::Error),
    ("synthesis_failed", ExitCode::Error),
    ("synthesis_timeout", ExitCode::Error),
    ("stt_unsupported", ExitCode::Error),
    ("translation_unsupported", ExitCode::Error),
    ("denoise_unsupported", ExitCode::Error),
    ("denoise_failed", ExitCode::Error),
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
    ("speech_metadata_corrupt", ExitCode::Error),
    ("output_exists", ExitCode::InvalidInput),
    ("interrupted", ExitCode::Interrupted),
];

/// Solución de cada `reason`, en el mismo orden y con el mismo conjunto que
/// `REASON_CODES`. `None` significa que el `reason` no tiene una orden concreta
/// que sugerir. El marcador `{cmd}` lo sustituye quien escribe el error por la
/// invocación real del usuario.
pub const REASON_SOLUTIONS: &[(&str, Option<&str>)] = &[
    (
        "confirmation_required",
        Some("Repite la orden con `--yes` para confirmarla: `{cmd} --yes`."),
    ),
    ("usage_error", None),
    ("empty_text", Some("Escribe un texto que no esté vacío.")),
    (
        "text_too_long",
        Some("Acorta el texto o divídelo en varias órdenes."),
    ),
    ("invalid_voice_name", Some("Elige otro nombre de voz.")),
    ("invalid_identifier", Some("Elige otro nombre o etiqueta.")),
    (
        "cannot_remove_default",
        Some("Solo se pueden borrar las voces clonadas; consúltalas con `ai-voice-interconnector voice list`."),
    ),
    (
        "cannot_clone_default",
        Some("Clona tu voz con otro nombre, por ejemplo `--name mi_voz`."),
    ),
    ("invalid_audio", Some("Usa un archivo de audio WAV válido.")),
    ("empty_audio", Some("Usa un audio que no esté vacío.")),
    (
        "empty_transcript",
        Some("Usa un audio en el que se oiga voz; si grabas con el micrófono, comprueba que no esté silenciado."),
    ),
    (
        "audio_too_long",
        Some("Recorta el audio o usa uno más corto."),
    ),
    (
        "voice_not_found",
        Some("Consulta las voces disponibles con `ai-voice-interconnector voice list`."),
    ),
    (
        "warm_voice_missing",
        Some("Consulta las voces disponibles con `ai-voice-interconnector voice list` y reinicia el daemon con `ai-voice-interconnector daemon restart`."),
    ),
    (
        "audio_not_found",
        Some("Comprueba la ruta del archivo de audio."),
    ),
    (
        "speech_not_found",
        Some("Consulta los audios sintetizados guardados con `ai-voice-interconnector speech list`."),
    ),
    (
        "not_installed",
        Some("Instala el programa con `ai-voice-interconnector self install`."),
    ),
    (
        "no_input_device",
        Some("Conecta un micrófono o actívalo en la configuración de sonido del sistema."),
    ),
    (
        "no_output_device",
        Some("Conecta unos altavoces o auriculares, o actívalos en la configuración de sonido del sistema."),
    ),
    (
        "audio_backend_unavailable",
        Some("Comprueba que el servicio de audio del sistema esté en marcha."),
    ),
    (
        "model_missing",
        Some("Descarga los modelos con `ai-voice-interconnector setup`."),
    ),
    (
        "daemon_unreachable",
        Some("Inicia el daemon con `ai-voice-interconnector daemon start` o repite la orden con `--no-daemon`."),
    ),
    (
        "daemon_version_mismatch",
        Some("Reinícialo con `ai-voice-interconnector daemon restart`."),
    ),
    (
        "voice_exists",
        Some("Elige otro nombre o añade `--force` para sobrescribir la voz."),
    ),
    (
        "label_exists",
        Some("Elige otra etiqueta con `--label` o añade `--force` para sobrescribirla."),
    ),
    ("voice_changed", Some("Repite la orden.")),
    (
        "port_in_use",
        Some("Cierra el programa que usa el puerto o elige otro con la variable `AVI_DAEMON_PORT`."),
    ),
    (
        "denoise_busy",
        Some("Espera a que termine la limpieza en curso y repite la orden."),
    ),
    (
        "translation_failed",
        Some("Repite la orden; si vuelve a fallar, revisa el entorno con `ai-voice-interconnector doctor`."),
    ),
    (
        "transcription_failed",
        Some("Repite la orden; si vuelve a fallar, revisa el entorno con `ai-voice-interconnector doctor`."),
    ),
    (
        "setup_failed",
        Some("Repite la descarga con `ai-voice-interconnector setup`."),
    ),
    (
        "externally_managed",
        Some("Actualiza o desinstala el programa con la herramienta que lo instaló."),
    ),
    (
        "rolled_back",
        Some("Repite la actualización con `ai-voice-interconnector self update`."),
    ),
    (
        "path_conflict",
        Some("Repite la orden con `--force` para reemplazar el enlace del PATH."),
    ),
    (
        "bundle_invalid",
        Some("Descarga de nuevo el paquete del programa y repite la instalación."),
    ),
    (
        "daemon_stop_failed",
        Some("Detén el daemon con `ai-voice-interconnector daemon stop` y repite la orden."),
    ),
    (
        "lifecycle_locked",
        Some("Espera a que termine la otra operación y repite la orden."),
    ),
    (
        "unsupported_platform",
        Some("Compila el programa desde el código fuente para esta plataforma."),
    ),
    (
        "binary_incompatible",
        Some("Repite la actualización; si vuelve a fallar, instala a mano el paquete de tu plataforma."),
    ),
    (
        "network_error",
        Some("Comprueba la conexión a Internet y repite la orden."),
    ),
    (
        "checksum_mismatch",
        Some("Repite la orden para descargar el archivo de nuevo."),
    ),
    (
        "program_dir_kept",
        Some("Cierra los programas que usen esa carpeta y bórrala a mano."),
    ),
    (
        "cleanup_incomplete",
        Some("Cierra los programas que usan esas rutas y repite `{cmd}`."),
    ),
    (
        "uninstall_incomplete",
        Some("Cierra los programas que usan esas rutas y repite `{cmd}`."),
    ),
    ("io_error", None),
    (
        "model_unavailable",
        Some("Actualiza el programa con `ai-voice-interconnector self update`."),
    ),
    (
        "release_unavailable",
        Some("Comprueba la versión pedida o repite la orden sin `--version` para usar la última."),
    ),
    (
        "voice_clone_failed",
        Some("Prueba con otra grabación de referencia."),
    ),
    (
        "synthesis_failed",
        Some("Repite la orden; si vuelve a fallar, revisa el entorno con `ai-voice-interconnector doctor`."),
    ),
    (
        "synthesis_timeout",
        Some("Repite la orden con un texto más corto."),
    ),
    (
        "stt_unsupported",
        Some("Instala la versión completa del programa, que incluye la transcripción."),
    ),
    (
        "translation_unsupported",
        Some("Instala la versión completa del programa, que incluye la traducción."),
    ),
    (
        "denoise_unsupported",
        Some("Instala la versión completa del programa, que incluye la limpieza de ruido."),
    ),
    ("denoise_failed", None),
    (
        "playback_failed",
        Some("Comprueba el dispositivo de salida con `ai-voice-interconnector devices`."),
    ),
    (
        "audio_enumeration_failed",
        Some("Comprueba que el servicio de audio del sistema esté en marcha."),
    ),
    ("speech_list_failed", None),
    ("voice_list_failed", None),
    ("voice_store_init_failed", None),
    (
        "daemon_error",
        Some("Reinicia el daemon con `ai-voice-interconnector daemon restart`."),
    ),
    (
        "startup_failed",
        Some("Revisa el entorno con `ai-voice-interconnector doctor`."),
    ),
    (
        "sudo_not_supported",
        Some("Ejecuta la orden con tu usuario, sin `sudo`."),
    ),
    ("lifecycle_failed", None),
    ("doctor_failed", None),
    ("self_install_failed", None),
    ("self_uninstall_failed", None),
    ("self_update_failed", None),
    (
        "speech_metadata_corrupt",
        Some("Borra ese audio con `ai-voice-interconnector speech remove --label <etiqueta> --voice <voz>`, que no necesita leer los metadatos."),
    ),
    (
        "output_exists",
        Some("Añade `--overwrite` para reemplazarlo, `--skip-existing` para saltarlo o elige otra carpeta con `--output-dir`."),
    ),
    ("interrupted", None),
];

/// Solución de un `reason`, o `None` si no tiene una orden concreta que
/// sugerir o el `reason` es desconocido.
pub fn solution_for(reason: &str) -> Option<&'static str> {
    REASON_SOLUTIONS
        .iter()
        .find(|(declared, _)| *declared == reason)
        .and_then(|(_, solution)| *solution)
}

#[derive(Error, Debug)]
#[error("{message}")]
pub struct CliError {
    pub code: ExitCode,
    pub reason: String,
    pub message: String,
    /// Detalle técnico de la causa, por ejemplo el texto de una biblioteca en
    /// inglés. No sale en la salida de texto: va al campo `error` de `--json` y
    /// al registro de trazas.
    pub detail: Option<String>,
}

impl CliError {
    pub fn new(code: ExitCode, reason: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code,
            reason: reason.into(),
            message: message.into(),
            detail: None,
        }
    }

    /// Añade el detalle técnico de la causa.
    pub fn with_detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
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
        ("cannot_clone_default", 2),
        ("invalid_audio", 2),
        ("empty_audio", 2),
        ("empty_transcript", 2),
        ("audio_too_long", 2),
        ("voice_not_found", 3),
        ("warm_voice_missing", 3),
        ("audio_not_found", 3),
        ("speech_not_found", 3),
        ("not_installed", 3),
        ("no_input_device", 3),
        ("no_output_device", 3),
        ("audio_backend_unavailable", 3),
        ("model_missing", 4),
        ("daemon_unreachable", 5),
        ("daemon_version_mismatch", 5),
        ("voice_exists", 6),
        ("label_exists", 6),
        ("voice_changed", 6),
        ("port_in_use", 6),
        ("denoise_busy", 6),
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
        ("cleanup_incomplete", 23),
        ("uninstall_incomplete", 24),
        ("io_error", 1),
        ("model_unavailable", 1),
        ("release_unavailable", 1),
        ("voice_clone_failed", 1),
        ("synthesis_failed", 1),
        ("synthesis_timeout", 1),
        ("stt_unsupported", 1),
        ("translation_unsupported", 1),
        ("denoise_unsupported", 1),
        ("denoise_failed", 1),
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
        ("speech_metadata_corrupt", 1),
        ("output_exists", 2),
        ("interrupted", 130),
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

    /// `REASON_SOLUTIONS` declara exactamente los `reason` de `REASON_CODES`, en
    /// el mismo orden.
    #[test]
    fn solutions_match_the_reason_table_in_order() {
        let codes: Vec<&str> = super::REASON_CODES.iter().map(|(r, _)| *r).collect();
        let solutions: Vec<&str> = super::REASON_SOLUTIONS.iter().map(|(r, _)| *r).collect();
        assert_eq!(solutions, codes);
    }

    /// Toda solución termina en punto y solo el marcador `{cmd}` es un marcador.
    #[test]
    fn solutions_are_sentences_and_use_only_the_cmd_marker() {
        for (reason, solution) in super::REASON_SOLUTIONS {
            let Some(solution) = solution else { continue };
            assert!(solution.ends_with('.'), "{reason}: {solution}");
            let without_marker = solution.replace("{cmd}", "");
            assert!(
                !without_marker.contains('{') && !without_marker.contains('}'),
                "{reason}: marcador desconocido en {solution}"
            );
        }
    }

    #[test]
    fn solution_for_resolves_known_reasons_and_ignores_the_rest() {
        assert_eq!(
            super::solution_for("model_missing"),
            Some("Descarga los modelos con `ai-voice-interconnector setup`.")
        );
        assert_eq!(super::solution_for("io_error"), None);
        assert_eq!(super::solution_for("motivo_del_futuro"), None);
    }

    #[test]
    fn with_detail_keeps_the_message_apart_from_the_detail() {
        let error = super::CliError::new(ExitCode::Error, "io_error", "No se pudo leer.")
            .with_detail("os error 2");
        assert_eq!(error.message, "No se pudo leer.");
        assert_eq!(error.detail.as_deref(), Some("os error 2"));
        assert_eq!(error.to_string(), "No se pudo leer.");
    }

    #[test]
    fn from_reason_unknown_is_generic_error() {
        assert_eq!(ExitCode::from_reason("motivo_del_futuro"), ExitCode::Error);
    }
}
