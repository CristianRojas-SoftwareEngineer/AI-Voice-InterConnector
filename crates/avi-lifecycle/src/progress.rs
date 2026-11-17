// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Vista del progreso de la provisión de modelos, por stderr.
//!
//! Dibuja un encabezado con el número de modelos y una barra por modelo
//! etiquetada `[i/N] nombre` con su porcentaje, tamaño real, velocidad media y
//! tiempo restante. La barra se repinta cada 250 ms aunque no lleguen bytes, y
//! el tiempo restante sale de la velocidad media global de la provisión. Cada
//! barra se borra al terminar (`finish_and_clear`) y se sustituye por una línea
//! permanente con el tamaño, la duración y la velocidad del modelo: así no queda
//! en pantalla una línea rellenada con espacios ni texto pegado a ella, y lo que
//! se copia de la terminal es legible. El total de la provisión no se escribe
//! aquí: lo devuelve [`ProvisionView::finish`] y el cierre del comando lo
//! cuenta. Sin terminal, `indicatif` no dibuja las barras, pero el encabezado,
//! una línea de avance cada quince segundos por modelo y las líneas de los
//! modelos terminados salen igual: el avance sigue siendo visible en el log, que
//! es lo que distingue una descarga larga de un flujo mudo. Cuando `AVI_SETUP_WATCHDOG` está definido, envía además al padre un
//! latido interno de bytes cada cinco segundos, aunque se haya silenciado el
//! progreso visible.
//!
//! El indicador de etapas de la CLI y del motor (install, uninstall) vive en
//! `avi_term::progress::PhaseProgress`.
//!
//! Los textos salen de funciones puras para poder fijarlos en pruebas.

use std::io::IsTerminal;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use avi_store::TransferObserver;
use avi_term::fmt::{bytes as human_bytes, duration as format_duration};
use indicatif::{ProgressBar, ProgressState, ProgressStyle};

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
    /// Abre el bloque de progreso: el encabezado con el número de modelos a
    /// descargar.
    pub fn start(count: usize) -> Self {
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
        let started = Instant::now();
        let bar = ProgressBar::new(0)
            .with_style(bar_style(self.started, Arc::clone(&self.bytes), started))
            .with_prefix(label.clone());
        // Repinta aunque no lleguen bytes, para que el tiempo no se congele.
        bar.enable_steady_tick(REPAINT_EVERY);
        // Sin terminal, `indicatif` no dibuja la barra: el aviso periódico por
        // bytes es la única señal de que la descarga sigue escribiendo.
        let beat = (!std::io::stderr().is_terminal() && !avi_term::progress::progress_silenced())
            .then(|| AtomicU64::new(0));
        ModelBar {
            bar,
            label,
            started,
            completed: AtomicU64::new(0),
            provision_bytes: Arc::clone(&self.bytes),
            beat,
            watchdog_beat: std::env::var_os("AVI_SETUP_WATCHDOG").map(|_| AtomicU64::new(0)),
        }
    }

    /// Devuelve el total descargado y la duración de la provisión. No escribe
    /// nada: el cierre lo escribe el comando que la pidió (`setup` o
    /// `self install`).
    pub fn finish(self) -> ProvisionStats {
        ProvisionStats {
            bytes: self.bytes.load(Ordering::Relaxed),
            elapsed: self.started.elapsed(),
        }
    }
}

/// Barra de un modelo; recibe el progreso de `avi-store` como observador.
pub struct ModelBar {
    bar: ProgressBar,
    label: String,
    started: Instant,
    completed: AtomicU64,
    provision_bytes: Arc<AtomicU64>,
    /// Instante del último aviso de avance, en milisegundos desde `started`.
    /// `Some` solo cuando toca avisar: sin terminal no hay barra que mirar, y una
    /// línea periódica es lo único que distingue una descarga larga de un flujo
    /// que se quedó mudo.
    beat: Option<AtomicU64>,
    /// Último latido entregado al watchdog, para no emitir una línea por chunk.
    watchdog_beat: Option<AtomicU64>,
}

impl TransferObserver for ModelBar {
    fn heartbeat(&self, completed: u64, total: u64) {
        let Some(last) = &self.watchdog_beat else {
            return;
        };
        let now = self.started.elapsed().as_millis() as u64;
        let previous = last.load(Ordering::Relaxed);
        if now.saturating_sub(previous) >= WATCHDOG_HEARTBEAT.as_millis() as u64
            && last
                .compare_exchange(previous, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            eprintln!("AVI_SETUP_HEARTBEAT {completed} {total}");
        }
    }

    fn update(&self, completed: u64, total: u64) {
        self.bar.set_length(total);
        self.bar.set_position(completed);
        self.completed.store(completed, Ordering::Relaxed);
        let Some(beat) = &self.beat else {
            return;
        };
        let now = self.started.elapsed().as_millis() as u64;
        let last = beat.load(Ordering::Relaxed);
        if now.saturating_sub(last) >= NO_TTY_HEARTBEAT.as_millis() as u64
            && beat
                .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        {
            eprintln!(
                "{}",
                transfer_line(&self.label, completed, total, self.started.elapsed())
            );
        }
    }
}

impl ModelBar {
    /// Borra la barra y deja en su lugar la línea permanente del modelo.
    pub fn finish(&self) -> ModelStats {
        self.bar.disable_steady_tick();
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

/// Plantilla de la barra activa:
/// `  [i/N] nombre  ████░░░░░░░░   42 %  1,0 GB/2,5 GB  51,0 MB/s  quedan 30,0 s`.
///
/// Los tamaños, el porcentaje, la velocidad y el tiempo restante usan claves
/// propias para que salgan con `avi_term::fmt` (coma decimal, duraciones del
/// producto) y con los rótulos en español, en lugar de los formatos fijos de
/// `indicatif`. La velocidad es la media del modelo (sus bytes entre su
/// tiempo); el tiempo restante usa la velocidad media global de la provisión,
/// que es más estable que la del modelo en curso. Escribir en el búfer de la
/// plantilla no puede fallar de forma recuperable: si fallara, la barra saldría
/// sin ese campo, que es todo lo que cabe hacer.
fn bar_style(
    provision_started: Instant,
    provision_bytes: Arc<AtomicU64>,
    model_started: Instant,
) -> ProgressStyle {
    ProgressStyle::with_template("  {prefix}  {bar:12}  {pct}  {sizes}  {speed}  quedan {eta}")
        .expect("la plantilla de la barra es válida")
        .progress_chars("█░")
        .with_key(
            "pct",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                write!(w, "{:>3} %", (state.fraction() * 100.0).floor() as u64).ok();
            },
        )
        .with_key(
            "sizes",
            |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                let total = state.len().unwrap_or(0);
                write!(w, "{}/{}", human_bytes(state.pos()), human_bytes(total)).ok();
            },
        )
        .with_key(
            "speed",
            move |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                let speed = bytes_per_second(state.pos(), model_started.elapsed());
                write!(w, "{}/s", human_bytes(speed)).ok();
            },
        )
        .with_key(
            "eta",
            move |state: &ProgressState, w: &mut dyn std::fmt::Write| {
                let remaining = state.len().unwrap_or(0).saturating_sub(state.pos());
                let global_bytes = provision_bytes.load(Ordering::Relaxed) + state.pos();
                write!(
                    w,
                    "{}",
                    eta_text(
                        remaining,
                        global_bytes,
                        provision_started.elapsed(),
                        model_started.elapsed()
                    )
                )
                .ok();
            },
        )
}

/// Etiqueta de un modelo: `[i/N] nombre`.
fn label(index: usize, count: usize, name: &str) -> String {
    format!("[{index}/{count}] {name}")
}

/// Encabezado del bloque de progreso.
pub fn header_line(count: usize) -> String {
    format!(
        "Descargando {}:",
        avi_term::fmt::count(count as u64, "modelo", "modelos")
    )
}

/// Velocidad media de `bytes` en `elapsed`, en bytes por segundo. Se calcula
/// sobre al menos un milisegundo, para que una descarga instantánea no divida
/// por cero.
fn bytes_per_second(bytes: u64, elapsed: Duration) -> u64 {
    (bytes as f64 / elapsed.as_secs_f64().max(0.001)) as u64
}

/// Línea permanente de un modelo terminado:
/// `  ✓ [i/N] nombre  2,5 GB en 49,0 s · 51,0 MB/s`.
pub fn model_done_line(label: &str, bytes: u64, elapsed: Duration) -> String {
    format!(
        "  ✓ {label}  {} en {} · {}/s",
        human_bytes(bytes),
        format_duration(elapsed),
        human_bytes(bytes_per_second(bytes, elapsed))
    )
}

/// Cada cuánto deja una línea de avance un modelo cuando no hay terminal.
const NO_TTY_HEARTBEAT: Duration = Duration::from_secs(15);
/// Cadencia del latido interno para que el padre vigile descargas largas.
const WATCHDOG_HEARTBEAT: Duration = Duration::from_secs(5);
/// Cada cuánto se repinta una barra aunque no lleguen bytes.
const REPAINT_EVERY: Duration = Duration::from_millis(250);
/// Tiempo que debe llevar un modelo descargando para que se estime lo que
/// queda: antes la velocidad media aún no es representativa.
const ETA_WARMUP: Duration = Duration::from_secs(2);

/// Línea periódica de avance de un modelo sin terminal:
/// `  … [i/N] nombre  1,2 GB de 2,5 GB · 12,0 s`.
///
/// Función pura, como el resto de las líneas del módulo, para que el texto
/// exacto sea comprobable sin capturar stderr.
pub fn transfer_line(label: &str, completed: u64, total: u64, elapsed: Duration) -> String {
    let elapsed_text = format_duration(elapsed);
    if total == 0 {
        return format!("  … {label}  {} · {elapsed_text}", human_bytes(completed));
    }
    format!(
        "  … {label}  {} de {} · {elapsed_text}",
        human_bytes(completed),
        human_bytes(total)
    )
}

/// Tiempo restante de un modelo: lo que falta por descargar entre la velocidad
/// media global de la provisión (bytes descargados en total entre el tiempo
/// desde que empezó). Da `—` durante los primeros 2 s del modelo y mientras la
/// velocidad global sea cero, porque entonces no hay estimación fiable.
pub fn eta_text(
    remaining_bytes: u64,
    global_bytes: u64,
    global_elapsed: Duration,
    model_elapsed: Duration,
) -> String {
    let speed = global_bytes as f64 / global_elapsed.as_secs_f64();
    if model_elapsed < ETA_WARMUP || !speed.is_finite() || speed <= 0.0 {
        return "—".to_string();
    }
    format_duration(Duration::from_secs_f64(remaining_bytes as f64 / speed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_heartbeat_survives_disabled_visible_progress() {
        const CHILD: &str = "AVI_TEST_WATCHDOG_PROGRESS_CHILD";
        if std::env::var_os(CHILD).is_some() {
            let view = ProvisionView::start(1);
            let mut model = view.model(1, "test");
            model.started = Instant::now() - Duration::from_secs(6);
            model.heartbeat(1, 10);
            model.heartbeat(2, 10);
            return;
        }
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "progress::tests::watchdog_heartbeat_survives_disabled_visible_progress",
                "--nocapture",
            ])
            .env(CHILD, "1")
            .env("AVI_SETUP_WATCHDOG", "1")
            .env("AVI_NO_PROGRESS", "1")
            .output()
            .expect("se ejecuta el hijo de la prueba");
        assert!(output.status.success(), "{:?}", output.status);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("AVI_SETUP_HEARTBEAT 1 10"), "{stderr}");
        assert_eq!(
            stderr.matches("AVI_SETUP_HEARTBEAT ").count(),
            1,
            "{stderr}"
        );
    }

    #[test]
    fn transfer_line_shows_progress_and_elapsed() {
        assert_eq!(
            transfer_line(
                "[1/5] qwen3-tts-0.6b",
                1_200_000_000,
                2_500_000_000,
                Duration::from_secs(12)
            ),
            "  … [1/5] qwen3-tts-0.6b  1,2 GB de 2,5 GB · 12,0 s"
        );
        assert_eq!(
            transfer_line("[1/1] m", 1024, 0, Duration::from_secs(0)),
            "  … [1/1] m  1,0 KB · 0,0 s",
            "un total desconocido no inventa denominador"
        );
    }

    #[test]
    fn header_line_counts_the_models() {
        assert_eq!(header_line(5), "Descargando 5 modelos:");
    }

    #[test]
    fn model_done_line_reports_size_time_and_speed() {
        assert_eq!(
            model_done_line(
                &label(1, 5, "qwen3-tts-0.6b"),
                2_500_000_000,
                Duration::from_secs(49)
            ),
            "  ✓ [1/5] qwen3-tts-0.6b  2,5 GB en 49,0 s · 51,0 MB/s"
        );
    }

    #[test]
    fn model_done_line_survives_a_zero_duration() {
        assert_eq!(
            model_done_line(&label(2, 2, "m"), 1000, Duration::ZERO),
            "  ✓ [2/2] m  1,0 KB en 0,0 s · 1,0 MB/s"
        );
    }

    /// Antes de 2 s de modelo, o con velocidad global cero, no hay estimación.
    #[test]
    fn eta_is_unknown_before_two_seconds_and_without_speed() {
        let ten = Duration::from_secs(10);
        assert_eq!(
            eta_text(1_000_000_000, 100_000_000, ten, Duration::from_millis(1999)),
            "—"
        );
        assert_eq!(eta_text(1_000_000_000, 0, ten, Duration::from_secs(5)), "—");
    }

    /// Con 100 MB en 10 s la velocidad global es 10 MB/s: 1 GB tarda 100 s.
    #[test]
    fn eta_uses_the_global_average_speed() {
        assert_eq!(
            eta_text(
                1_000_000_000,
                100_000_000,
                Duration::from_secs(10),
                Duration::from_secs(2)
            ),
            "1 min 40 s"
        );
        assert_eq!(
            eta_text(
                0,
                100_000_000,
                Duration::from_secs(10),
                Duration::from_secs(9)
            ),
            "0,0 s"
        );
    }
}
