// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Vista del progreso de la provisión de modelos, por stderr.
//!
//! Dibuja un encabezado con el número de modelos, una barra por modelo
//! etiquetada `[i/N] nombre` con su tamaño real, velocidad, tiempo transcurrido
//! y restante, y un cierre con el total. Cada barra se borra al terminar
//! (`finish_and_clear`) y se sustituye por una línea permanente con el tamaño,
//! la duración y la velocidad del modelo: así no queda en pantalla una línea
//! rellenada con espacios ni texto pegado a ella, y lo que se copia de la
//! terminal es legible. Sin terminal, `indicatif` no dibuja las barras, pero el
//! encabezado, las líneas por modelo y el cierre salen igual.
//!
//! Los textos salen de funciones puras para poder fijarlos en pruebas.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use avi_store::TransferObserver;
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

use crate::human_bytes;

/// Lo descargado de un modelo y cuánto tardó.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelStats {
    pub bytes: u64,
    pub elapsed: Duration,
}

/// Lo descargado en toda la provisión y cuánto tardó.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProvisionStats {
    pub bytes: u64,
    pub elapsed: Duration,
}

/// Vista de una provisión con al menos un modelo pendiente.
pub struct ProvisionView {
    count: usize,
    started: Instant,
    bytes: Arc<AtomicU64>,
}

impl ProvisionView {
    /// Abre el bloque de progreso: una línea en blanco que lo separa del bloque
    /// anterior y el encabezado con el número de modelos a descargar.
    pub fn start(count: usize) -> Self {
        eprintln!();
        eprintln!("{}", header_line(count));
        Self {
            count,
            started: Instant::now(),
            bytes: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Barra del modelo `index` (desde 1) de los `count` pendientes.
    pub fn model(&self, index: usize, name: &str) -> ModelBar {
        let label = label(index, self.count, name);
        let bar = ProgressBar::new(0)
            .with_style(bar_style())
            .with_prefix(label.clone());
        ModelBar {
            bar,
            label,
            started: Instant::now(),
            completed: AtomicU64::new(0),
            provision_bytes: Arc::clone(&self.bytes),
        }
    }

    /// Cierra el bloque con el total descargado y la duración de la provisión.
    pub fn finish(self) -> ProvisionStats {
        let stats = ProvisionStats {
            bytes: self.bytes.load(Ordering::Relaxed),
            elapsed: self.started.elapsed(),
        };
        eprintln!("{}", total_line(stats.bytes, stats.elapsed));
        stats
    }
}

/// Barra de un modelo; recibe el progreso de `avi-store` como observador.
pub struct ModelBar {
    bar: ProgressBar,
    label: String,
    started: Instant,
    completed: AtomicU64,
    provision_bytes: Arc<AtomicU64>,
}

impl TransferObserver for ModelBar {
    fn update(&self, completed: u64, total: u64) {
        self.bar.set_length(total);
        self.bar.set_position(completed);
        self.completed.store(completed, Ordering::Relaxed);
    }
}

impl ModelBar {
    /// Borra la barra y deja en su lugar la línea permanente del modelo.
    pub fn finish(&self) -> ModelStats {
        self.bar.finish_and_clear();
        let stats = ModelStats {
            bytes: self.completed.load(Ordering::Relaxed),
            elapsed: self.started.elapsed(),
        };
        self.provision_bytes
            .fetch_add(stats.bytes, Ordering::Relaxed);
        eprintln!(
            "{}",
            model_done_line(&self.label, stats.bytes, stats.elapsed)
        );
        stats
    }
}

/// Plantilla de la barra activa. Los tamaños, la velocidad y los tiempos usan
/// claves propias para que salgan con `human_bytes` (coma decimal) y con los
/// rótulos en español, en lugar de los formatos fijos de `indicatif`.
/// Escribir en el búfer de la plantilla no puede fallar de forma recuperable:
/// si fallara, la barra saldría sin ese campo, que es todo lo que cabe hacer.
fn bar_style() -> ProgressStyle {
    ProgressStyle::with_template(
        "  {prefix}  {bar:12}  {sizes}  {speed}  transcurrido {elapsed_clock}  quedan {eta_clock}",
    )
    .expect("la plantilla de la barra es válida")
    .progress_chars("█░")
    .with_key(
        "sizes",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            let total = state.len().unwrap_or(0);
            write!(w, "{}/{}", human_bytes(state.pos()), human_bytes(total)).ok();
        },
    )
    .with_key(
        "speed",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            write!(w, "{}/s", human_bytes(state.per_sec() as u64)).ok();
        },
    )
    .with_key(
        "elapsed_clock",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            write!(w, "{}", clock(state.elapsed())).ok();
        },
    )
    .with_key(
        "eta_clock",
        |state: &ProgressState, w: &mut dyn std::fmt::Write| {
            write!(w, "{}", clock(state.eta())).ok();
        },
    )
}

/// Etiqueta de un modelo: `[i/N] nombre`.
fn label(index: usize, count: usize, name: &str) -> String {
    format!("[{index}/{count}] {name}")
}

/// Encabezado del bloque de progreso.
pub fn header_line(count: usize) -> String {
    format!("Descargando {count} modelo(s):")
}

/// Línea permanente de un modelo terminado. La velocidad se calcula sobre al
/// menos un milisegundo, para que una descarga instantánea no divida por cero.
pub fn model_done_line(label: &str, bytes: u64, elapsed: Duration) -> String {
    let seconds = elapsed.as_secs_f64().max(0.001);
    let speed = (bytes as f64 / seconds) as u64;
    format!(
        "  ✓ {label}  {} en {} ({}/s)",
        human_bytes(bytes),
        format_duration(elapsed),
        human_bytes(speed)
    )
}

/// Cierre del bloque de progreso.
pub fn total_line(bytes: u64, elapsed: Duration) -> String {
    format!(
        "Modelos descargados: {} en {}.",
        human_bytes(bytes),
        format_duration(elapsed)
    )
}

/// Duración legible: `7 s`, `1 min 59 s`, `1 h 02 min`.
pub fn format_duration(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else if secs < 3600 {
        format!("{} min {} s", secs / 60, secs % 60)
    } else {
        format!("{} h {:02} min", secs / 3600, (secs % 3600) / 60)
    }
}

/// Reloj `m:ss` de la barra activa.
pub fn clock(elapsed: Duration) -> String {
    let secs = elapsed.as_secs();
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_duration_uses_seconds_minutes_and_hours() {
        assert_eq!(format_duration(Duration::from_secs(7)), "7 s");
        assert_eq!(format_duration(Duration::from_secs(119)), "1 min 59 s");
        assert_eq!(format_duration(Duration::from_secs(3720)), "1 h 02 min");
    }

    #[test]
    fn clock_pads_the_seconds() {
        assert_eq!(clock(Duration::from_secs(4)), "0:04");
        assert_eq!(clock(Duration::from_secs(183)), "3:03");
    }

    #[test]
    fn header_line_counts_the_models() {
        assert_eq!(header_line(5), "Descargando 5 modelo(s):");
    }

    #[test]
    fn model_done_line_reports_size_time_and_speed() {
        assert_eq!(
            model_done_line(
                &label(1, 5, "qwen3-tts-0.6b"),
                2_500_000_000,
                Duration::from_secs(49)
            ),
            "  ✓ [1/5] qwen3-tts-0.6b  2,5 GB en 49 s (51,0 MB/s)"
        );
    }

    #[test]
    fn model_done_line_survives_a_zero_duration() {
        assert_eq!(
            model_done_line(&label(2, 2, "m"), 1000, Duration::ZERO),
            "  ✓ [2/2] m  1,0 KB en 0 s (1,0 MB/s)"
        );
    }

    #[test]
    fn total_line_reports_size_and_time() {
        assert_eq!(
            total_line(5_900_000_000, Duration::from_secs(119)),
            "Modelos descargados: 5,9 GB en 1 min 59 s."
        );
    }
}
