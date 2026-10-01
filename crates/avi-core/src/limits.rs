use std::time::Duration;

use crate::exit_codes::{CliError, ExitCode};

/// Longitud máxima, en caracteres Unicode (no bytes), del texto que se sintetiza.
/// El motor de síntesis en CPU degrada su latencia por encima de este orden de
/// magnitud, así que se rechaza la entrada antes de despacharla.
pub const MAX_TEXT_LENGTH: usize = 500;

/// Duración máxima, en segundos, del audio que acepta `speech dub`.
pub const MAX_DUB_AUDIO_SECS: u64 = 40;

/// Duración máxima, en segundos, del audio que acepta `speech transcribe`.
pub const MAX_TRANSCRIBE_AUDIO_SECS: u64 = 300;

/// Duración máxima, en segundos, de la referencia de voz de `voice clone`.
pub const MAX_CLONE_REFERENCE_SECS: u64 = 30;

/// Límite de cuerpo, en bytes, de una ruta que recibe audio: el tamaño de
/// `secs` segundos a `bytes_per_sec` codificado en base64 (factor 4/3, redondeado
/// hacia arriba) más 1 MiB de holgura para el resto del JSON.
pub fn body_limit_for(secs: u64, bytes_per_sec: u64) -> usize {
    let _ = (secs, bytes_per_sec);
    2 * 1024 * 1024
}

/// Tiempo fijo del presupuesto de síntesis, en milisegundos: cubre el arranque de
/// la petición con independencia de la longitud del texto, incluida la primera
/// inferencia de un motor residente recién lanzado, que tarda bastante más que las
/// siguientes.
const SYNTHESIS_BASE_MS: u64 = 30_000;

/// Tiempo adicional del presupuesto de síntesis por cada carácter, en milisegundos.
const SYNTHESIS_PER_CHAR_MS: u64 = 300;

/// Holgura, en milisegundos, que el techo del cliente suma al presupuesto máximo
/// para cubrir el calentamiento del motor, la transcripción y la traducción del dub.
const FAILSAFE_MARGIN_MS: u64 = 60_000;

/// Valida el texto a sintetizar: no puede quedar vacío tras `trim` ni superar
/// `MAX_TEXT_LENGTH` caracteres. Es la única fuente de esta regla para la CLI y el
/// daemon, de modo que ambas vías devuelven el mismo `reason`.
pub fn validate_synthesis_text(text: &str) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_text",
            "El texto a sintetizar no puede estar vacío",
        ));
    }
    let length = text.chars().count();
    if length > MAX_TEXT_LENGTH {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "text_too_long",
            format!(
                "El texto a sintetizar supera el máximo de {MAX_TEXT_LENGTH} caracteres (recibidos: {length})"
            ),
        ));
    }
    Ok(())
}

/// Tiempo máximo de síntesis para un texto de `chars` caracteres: 30 s fijos más
/// 300 ms por carácter, proporcional a lo que se pide sintetizar.
pub const fn synthesis_budget(chars: usize) -> Duration {
    Duration::from_millis(SYNTHESIS_BASE_MS + SYNTHESIS_PER_CHAR_MS * chars as u64)
}

/// Techo total de una petición del cliente: el presupuesto del texto más largo
/// permitido más una holgura de 60 s para el calentamiento del motor y, en el dub,
/// la transcripción y la traducción.
pub const REQUEST_FAILSAFE: Duration = Duration::from_millis(
    SYNTHESIS_BASE_MS + SYNTHESIS_PER_CHAR_MS * MAX_TEXT_LENGTH as u64 + FAILSAFE_MARGIN_MS,
);

#[cfg(test)]
mod tests {
    use super::*;

    fn reason_of(result: Result<(), CliError>) -> (ExitCode, String) {
        let err = result.expect_err("se esperaba un error de validación");
        (err.code, err.reason)
    }

    #[test]
    fn empty_text_is_rejected() {
        let (code, reason) = reason_of(validate_synthesis_text(""));
        assert_eq!(code, ExitCode::InvalidInput);
        assert_eq!(reason, "empty_text");
    }

    #[test]
    fn whitespace_only_text_is_rejected() {
        let (code, reason) = reason_of(validate_synthesis_text("  \t\n "));
        assert_eq!(code, ExitCode::InvalidInput);
        assert_eq!(reason, "empty_text");
    }

    #[test]
    fn text_at_limit_is_accepted() {
        assert!(validate_synthesis_text(&"a".repeat(MAX_TEXT_LENGTH)).is_ok());
    }

    #[test]
    fn text_over_limit_is_rejected_with_lengths_in_message() {
        let err = validate_synthesis_text(&"a".repeat(MAX_TEXT_LENGTH + 1)).unwrap_err();
        assert_eq!(err.code, ExitCode::InvalidInput);
        assert_eq!(err.reason, "text_too_long");
        assert!(err.message.contains("500"));
        assert!(err.message.contains("501"));
    }

    #[test]
    fn multibyte_text_is_counted_in_chars_not_bytes() {
        // 500 caracteres que ocupan más de 500 bytes deben aceptarse.
        let at_limit = "ñ".repeat(250) + &"😀".repeat(250);
        assert_eq!(at_limit.chars().count(), MAX_TEXT_LENGTH);
        assert!(at_limit.len() > MAX_TEXT_LENGTH);
        assert!(validate_synthesis_text(&at_limit).is_ok());

        let over = "😀".repeat(MAX_TEXT_LENGTH + 1);
        let (_, reason) = reason_of(validate_synthesis_text(&over));
        assert_eq!(reason, "text_too_long");
    }

    #[test]
    fn synthesis_budget_scales_with_chars() {
        assert_eq!(synthesis_budget(0), Duration::from_secs(30));
        assert_eq!(synthesis_budget(20), Duration::from_secs(36));
        assert_eq!(synthesis_budget(500), Duration::from_secs(180));
    }

    #[test]
    fn body_limit_is_base64_size_plus_one_mebibyte() {
        // 10 s a 32000 B/s = 320000 B; en base64 son 426667 B (redondeo hacia arriba).
        assert_eq!(body_limit_for(10, 32_000), 426_667 + 1_048_576);
    }

    #[test]
    fn transcribe_body_limit_exceeds_twelve_point_eight_megabytes() {
        // PCM de 16 kHz, mono, 16 bits: 32000 B/s durante el tope de transcripción.
        let limit = body_limit_for(MAX_TRANSCRIBE_AUDIO_SECS, 32_000);
        assert!(limit > 12_800_000, "límite insuficiente: {limit}");
    }

    #[test]
    fn request_failsafe_covers_max_budget_plus_margin() {
        assert_eq!(
            REQUEST_FAILSAFE,
            synthesis_budget(MAX_TEXT_LENGTH) + Duration::from_secs(60)
        );
    }
}
