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
//! Aloja además `PhaseProgress`, el indicador de fase que la CLI y el propio
//! motor (install, uninstall) usan en operaciones largas: una sola definición
//! del mecanismo para todos los consumidores.
//!
//! Los textos salen de funciones puras para poder fijarlos en pruebas.

use std::io::{IsTerminal, Write};
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

/// Indicador ligero de progreso por fase para operaciones locales de larga
/// duración. Todo va a stderr, nunca a stdout. En TTY imprime una única
/// línea actualizada con `\r`; en no-TTY imprime una línea por fase sin
/// `\r` ni secuencias ANSI. `AVI_NO_PROGRESS=1` (o cualquier valor no
/// vacío) silencia toda salida de progreso.
pub struct PhaseProgress {
    start: Instant,
    tty: bool,
    enabled: bool,
    current: Option<(&'static str, String)>,
    /// Líneas de fase o actividad pintadas. En modo no-TTY cada una es una
    /// línea de stderr; el contador permite comprobarlo sin capturar stderr,
    /// que no es portable en un test unitario. Solo existe en las pruebas:
    /// en producción no hay nada que contar.
    #[cfg(test)]
    renders: std::cell::Cell<usize>,
    /// Líneas de cierre pintadas. En modo no-TTY cada una es una línea de
    /// stderr, como las de fase.
    #[cfg(test)]
    closing_lines: std::cell::Cell<usize>,
}

impl PhaseProgress {
    /// Crea el indicador: captura el reloj de inicio y decide TTY/silenciado
    /// una sola vez, para que cada `phase`/`tick` sea barato.
    pub fn new() -> Self {
        let disabled = silenced_by(std::env::var("AVI_NO_PROGRESS").ok().as_deref());
        Self {
            start: Instant::now(),
            tty: std::io::stderr().is_terminal(),
            enabled: !disabled,
            current: None,
            #[cfg(test)]
            renders: std::cell::Cell::new(0),
            #[cfg(test)]
            closing_lines: std::cell::Cell::new(0),
        }
    }

    /// Indicador de pruebas con el valor de `AVI_NO_PROGRESS` y el modo de
    /// terminal fijados a mano, para no depender del entorno del proceso ni
    /// de que exista una terminal.
    #[cfg(test)]
    fn for_test(no_progress_value: Option<&str>, tty: bool) -> Self {
        Self {
            start: Instant::now(),
            tty,
            enabled: !silenced_by(no_progress_value),
            current: None,
            renders: std::cell::Cell::new(0),
            closing_lines: std::cell::Cell::new(0),
        }
    }

    /// Líneas de fase o actividad pintadas hasta ahora. En modo no-TTY, cada
    /// una es una línea de stderr.
    #[cfg(test)]
    pub fn renders(&self) -> usize {
        self.renders.get()
    }

    /// Líneas de cierre pintadas hasta ahora. En modo no-TTY, cada una es una
    /// línea de stderr.
    #[cfg(test)]
    pub fn closing_lines(&self) -> usize {
        self.closing_lines.get()
    }

    /// Cambia la fase actual y pinta una nueva línea de progreso. No-op si
    /// el indicador está deshabilitado.
    pub fn phase(&mut self, label: &'static str, msg: &str) {
        if !self.enabled {
            return;
        }
        self.current = Some((label, msg.to_string()));
        self.render();
    }

    /// Vuelve a pintar la línea de la fase actual con el tiempo transcurrido
    /// actualizado. Útil en bucles de espera.
    pub fn tick(&self) {
        if !self.enabled || self.current.is_none() {
            return;
        }
        self.render_ref();
    }

    /// Cierra el indicador con una línea final de estado (éxito o fallo).
    pub fn finish(&self, ok: bool, msg: &str) {
        if !self.enabled {
            return;
        }
        let line = finish_line(ok, msg, self.start.elapsed());
        #[cfg(test)]
        self.closing_lines.set(self.closing_lines.get() + 1);
        if self.tty {
            eprint!("\r\x1b[2K{line}\n");
            #[expect(
                clippy::let_underscore_must_use,
                reason = "si stderr no admite el vaciado, el indicador sigue mostrando la fase"
            )]
            let _ = std::io::stderr().flush();
        } else {
            eprintln!("{line}");
        }
    }

    fn render(&self) {
        self.render_ref();
    }

    fn render_ref(&self) {
        let Some((_, msg)) = &self.current else {
            return;
        };
        let line = phase_line(msg, self.start.elapsed());
        #[cfg(test)]
        self.renders.set(self.renders.get() + 1);
        if self.tty {
            eprint!("\r\x1b[2K{line}");
            #[expect(
                clippy::let_underscore_must_use,
                reason = "si stderr no admite el vaciado, el indicador sigue mostrando la fase"
            )]
            let _ = std::io::stderr().flush();
        } else {
            eprintln!("{line}");
        }
    }
}

impl Default for PhaseProgress {
    fn default() -> Self {
        Self::new()
    }
}

/// ¿Silencia el indicador el valor de `AVI_NO_PROGRESS`? Cualquier valor no
/// vacío lo silencia; el vacío y la ausencia lo dejan hablar.
fn silenced_by(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// Línea de una fase o de una actividad, sin los controles de línea que la
/// terminal añade: `… <msg> (<elapsed>)`.
///
/// Es una función pura para que el texto exacto sea comprobable en un test
/// unitario, donde stderr no se puede capturar de forma portable. El
/// indicador la usa tal cual, de modo que la línea que sale por stderr es
/// byte a byte la que esta función devuelve.
pub fn phase_line(msg: &str, elapsed: Duration) -> String {
    format!("… {msg} ({})", format_duration(elapsed))
}

/// Línea de cierre, sin los controles de línea: `[ok] <msg> (<elapsed>)` o
/// `[error] <msg> (<elapsed>)`.
///
/// Es la contrapartida pura de `phase_line`, con la misma razón: el texto
/// exacto del cierre se comprueba sin capturar stderr.
pub fn finish_line(ok: bool, msg: &str, elapsed: Duration) -> String {
    let mark = if ok { "[ok]" } else { "[error]" };
    format!("{mark} {msg} ({})", format_duration(elapsed))
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

    // ─── Línea base del indicador de fase ───────────────────────────────────
    //
    // Estas pruebas fijan el comportamiento ACTUAL de `PhaseProgress` en modo
    // no-TTY, para que un cambio posterior pueda distinguir «lo que cambió a
    // propósito» de «lo que cambió por descuido». La salida por stderr no es
    // capturable de forma portable en un test unitario, así que se comprueba
    // por dos vías: el texto exacto con las funciones puras (`phase_line` y
    // `finish_line`) y el número de líneas emitidas con el contador que el
    // indicador lleva para las pruebas.

    /// La línea de una fase es `… <msg> (<elapsed>)`, con el elapsed formateado
    /// por `format_duration`.
    #[test]
    fn phase_line_is_the_message_with_its_elapsed() {
        assert_eq!(
            phase_line("Limpiando datos…", Duration::from_secs(0)),
            "… Limpiando datos… (0 s)"
        );
        assert_eq!(
            phase_line("Traduciendo texto…", Duration::from_secs(7)),
            "… Traduciendo texto… (7 s)"
        );
        assert_eq!(
            phase_line("Traduciendo texto…", Duration::from_secs(119)),
            "… Traduciendo texto… (1 min 59 s)"
        );
    }

    /// El cierreSuccessful lleva `[ok]` y el fallido `[error]`, con el mismo
    /// formato de elapsed.
    #[test]
    fn finish_line_marks_success_and_failure() {
        assert_eq!(
            finish_line(true, "Limpieza completada.", Duration::from_secs(0)),
            "[ok] Limpieza completada. (0 s)"
        );
        assert_eq!(
            finish_line(false, "Limpieza fallida.", Duration::from_secs(3)),
            "[error] Limpieza fallida. (3 s)"
        );
    }

    /// Una fase nueva en no-TTY emite exactamente una línea.
    #[test]
    fn phase_paints_one_line_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        assert_eq!(progress.renders(), 1, "una fase, una línea");
        assert_eq!(
            progress.closing_lines(),
            0,
            "una fase no cierra el indicador"
        );
    }

    /// `phase()` seguido de `tick()` en no-TTY emite DOS líneas: la fase y la
    /// actividad repetida. Es el comportamiento actual —el defecto que
    /// corrige el Hito 3— y la línea base lo deja escrito a propósito.
    #[test]
    fn phase_then_tick_paints_two_lines_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        assert_eq!(
            progress.renders(),
            2,
            "la actividad repetida abre hoy una segunda línea en no-TTY"
        );
    }

    /// `tick()` sin fase previa no pinta nada: no hay línea que repintar.
    #[test]
    fn tick_without_phase_paints_nothing() {
        let progress = PhaseProgress::for_test(None, false);
        progress.tick();
        assert_eq!(progress.renders(), 0);
    }

    /// El cierre emite su línea y no cuenta como línea de fase.
    #[test]
    fn finish_paints_one_closing_line() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.finish(true, "Limpieza completada.");
        assert_eq!(progress.renders(), 1, "el cierre no es una línea de fase");
        assert_eq!(progress.closing_lines(), 1);
    }

    /// `AVI_NO_PROGRESS` con cualquier valor no vacío silencia toda salida:
    /// ni fases, ni actividad, ni cierre.
    #[test]
    fn no_progress_silences_everything() {
        let mut progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        progress.finish(true, "Limpieza completada.");
        assert_eq!(progress.renders(), 0, "el indicador silenciado no pinta");
        assert_eq!(
            progress.closing_lines(),
            0,
            "el indicador silenciado no cierra"
        );
    }

    /// El valor vacío de `AVI_NO_PROGRESS` no silencia: solo el valor no
    /// vacío lo hace.
    #[test]
    fn empty_no_progress_value_does_not_silence() {
        assert!(!silenced_by(Some("")));
        assert!(!silenced_by(None));
        assert!(silenced_by(Some("0")));
        assert!(silenced_by(Some("1")));
    }
}
