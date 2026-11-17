// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Formateadores puros del texto para personas: duraciones, relojes, tamaños,
//! plurales, medidor de nivel y puntuación final.

use std::time::Duration;

/// Duración legible, en una sola forma para todo el producto.
///
/// Por debajo de 60 s lleva un decimal con coma (`0,7 s`, `12,4 s`). Desde 60 s
/// usa minutos (`2 min 35 s`) y desde 1 h, horas (`1 h 02 min`). Si el
/// redondeo de un valor menor de 60 s da `60,0`, se usa la forma en minutos.
pub fn duration(elapsed: Duration) -> String {
    let tenths = (elapsed.as_secs_f64() * 10.0).round() as u64;
    if tenths < 600 {
        return format!("{},{} s", tenths / 10, tenths % 10);
    }
    let secs = elapsed.as_secs_f64().round() as u64;
    if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
    }
}

/// Reloj `m:ss`, solo para el contador de la grabación y los de transferencia.
pub fn clock(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// Tamaño legible en escala decimal (B, KB, MB, GB, TB) con un decimal separado
/// por coma, porque el texto es para personas en español. Es el único
/// formateador de tamaños del workspace: cada unidad vale 1000 de la anterior y
/// la etiqueta lo dice.
pub fn bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!(
            "{} {}",
            format!("{value:.1}").replace('.', ","),
            UNITS[unit]
        )
    }
}

/// Cantidad con su sustantivo concordado: `1 ruta`, `0 rutas`, `2 rutas`.
pub fn count(n: u64, singular: &str, plural: &str) -> String {
    if n == 1 {
        format!("{n} {singular}")
    } else {
        format!("{n} {plural}")
    }
}

/// Escalones del medidor de nivel, de menor a mayor.
const METER_STEPS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Medidor de nivel: un carácter por nivel, que reparte el rango de −60 a
/// −20 dBFS en 8 escalones. Por debajo de −60 da `▁` y por encima de −20 da
/// `█`.
pub fn level_meter(levels_dbfs: &[f32]) -> String {
    levels_dbfs
        .iter()
        .map(|&db| {
            // Un nivel no numérico no tiene escalón: se muestra el más bajo.
            let step = if db.is_nan() {
                0
            } else {
                (((db + 60.0) / 5.0).floor().max(0.0) as usize).min(METER_STEPS.len() - 1)
            };
            METER_STEPS[step]
        })
        .collect()
}

/// Añade `.` al texto recortado si no termina en `.`, `?`, `!` ni `…`.
pub fn sentence(text: &str) -> String {
    let trimmed = text.trim();
    if trimmed.is_empty() || trimmed.ends_with(['.', '?', '!', '…']) {
        trimmed.to_string()
    } else {
        format!("{trimmed}.")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_follows_the_normative_table() {
        let cases = [
            (0.0, "0,0 s"),
            (0.7, "0,7 s"),
            (12.44, "12,4 s"),
            (59.96, "1 min 0 s"),
            (155.0, "2 min 35 s"),
            (3720.0, "1 h 02 min"),
        ];
        for (secs, expected) in cases {
            assert_eq!(duration(Duration::from_secs_f64(secs)), expected, "{secs}");
        }
    }

    #[test]
    fn duration_keeps_one_decimal_just_below_a_minute() {
        assert_eq!(duration(Duration::from_secs_f64(59.94)), "59,9 s");
    }

    #[test]
    fn clock_pads_the_seconds() {
        assert_eq!(clock(Duration::from_secs(4)), "0:04");
        assert_eq!(clock(Duration::from_secs(183)), "3:03");
    }

    #[test]
    fn bytes_formats_decimal_units_with_one_decimal() {
        assert_eq!(bytes(0), "0 B");
        assert_eq!(bytes(999), "999 B");
        assert_eq!(bytes(1000), "1,0 KB");
        assert_eq!(bytes(1500), "1,5 KB");
        assert_eq!(bytes(5 * 1000 * 1000), "5,0 MB");
        assert_eq!(bytes(3 * 1000 * 1000 * 1000 / 2), "1,5 GB");
        assert_eq!(bytes(1000u64.pow(4)), "1,0 TB");
    }

    #[test]
    fn count_agrees_with_the_number() {
        assert_eq!(count(1, "ruta", "rutas"), "1 ruta");
        assert_eq!(count(0, "ruta", "rutas"), "0 rutas");
        assert_eq!(count(2, "ruta", "rutas"), "2 rutas");
    }

    #[test]
    fn level_meter_spreads_the_range_in_eight_steps() {
        assert_eq!(level_meter(&[-90.0, -60.0]), "▁▁");
        assert_eq!(level_meter(&[-20.0, 0.0]), "██");
        assert_eq!(
            level_meter(&[-58.0, -53.0, -48.0, -43.0, -38.0, -33.0, -28.0, -23.0]),
            "▁▂▃▄▅▆▇█"
        );
        assert_eq!(level_meter(&[f32::NAN]), "▁");
        assert_eq!(level_meter(&[]), "");
    }

    #[test]
    fn sentence_adds_a_final_stop_only_when_missing() {
        assert_eq!(sentence("Falta el modelo"), "Falta el modelo.");
        assert_eq!(sentence("Falta el modelo."), "Falta el modelo.");
        assert_eq!(sentence("¿Seguro?"), "¿Seguro?");
        assert_eq!(sentence("¡Listo!"), "¡Listo!");
        assert_eq!(sentence("Espera…"), "Espera…");
        assert_eq!(sentence("  con espacios  "), "con espacios.");
    }
}
