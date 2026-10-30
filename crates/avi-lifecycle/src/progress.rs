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
//! encabezado, una línea de avance cada quince segundos por modelo, las líneas
//! de los modelos terminados y el cierre salen igual: el avance sigue siendo
//! visible en el log, que es lo que distingue una descarga larga de un flujo
//! mudo. Cuando `AVI_SETUP_WATCHDOG` está definido, envía además al padre un
//! latido interno de bytes cada cinco segundos, aunque se haya silenciado el
//! progreso visible.
//!
//! Aloja además `PhaseProgress`, el indicador de fase que la CLI y el propio
//! motor (install, uninstall) usan en operaciones largas: una sola definición
//! del mecanismo para todos los consumidores.
//!
//! Los textos salen de funciones puras para poder fijarlos en pruebas.

use std::io::{IsTerminal, Write};
#[cfg(test)]
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
        // Sin terminal, `indicatif` no dibuja la barra: el aviso periódico por
        // bytes es la única señal de que la descarga sigue escribiendo.
        let beat = (!std::io::stderr().is_terminal()
            && !silenced_by(std::env::var("AVI_NO_PROGRESS").ok().as_deref()))
        .then(|| AtomicU64::new(0));
        ModelBar {
            bar,
            label,
            started: Instant::now(),
            completed: AtomicU64::new(0),
            provision_bytes: Arc::clone(&self.bytes),
            beat,
            watchdog_beat: std::env::var_os("AVI_SETUP_WATCHDOG").map(|_| AtomicU64::new(0)),
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
/// duración. Todo va a stderr, nunca a stdout. En TTY mantiene una única
/// línea actualizada con `\r`; en no-TTY anuncia cada fase nueva con una
/// línea propia, sin `\r` ni secuencias ANSI, y **raciona** la actividad
/// repetida dentro de la misma fase: no abre una línea nueva. `AVI_NO_PROGRESS=1`
/// (o cualquier valor no vacío) silencia toda salida de progreso.
///
/// El indicador **no imprime estado**: ni un cierre `[ok]` ni `[error]`. El
/// mensaje final lo emite quien lo usa —el handler de la CLI o el flujo de la
/// librería—, por stdout, con la duración que devuelve [`PhaseProgress::close`].
pub struct PhaseProgress {
    start: Instant,
    tty: bool,
    enabled: bool,
    current: Option<(&'static str, String)>,
    /// Fase ya anunciada en modo no-TTY. Es el estado del racionado: una fase
    /// nueva se anuncia siempre, y una repetición de la misma fase no abre
    /// línea nueva.
    announced: Option<&'static str>,
    /// ¿Hay una línea pintada en la terminal que otro escritor pueda pisar?
    /// Un `AtomicBool` porque `tick` solo consulta el indicador y aun así
    /// tiene que dejar constancia de que la línea quedó abierta.
    line_open: AtomicBool,
    /// ¿Se abrió ya el bloque (la línea en blanco inicial)? Solo el primer
    /// `phase` la escribe.
    block_opened: bool,
    /// Líneas de fase o actividad pintadas. En modo no-TTY cada una es una
    /// línea de stderr; el contador permite comprobarlo sin capturar stderr,
    /// que no es portable en un test unitario. Solo existe en las pruebas:
    /// en producción no hay nada que contar. Es un `Arc` para que una prueba
    /// pueda inspectarlo después de que `close` consuma el indicador.
    #[cfg(test)]
    renders: Arc<AtomicUsize>,
    /// Líneas cedidas: veces que `close_line` devolvió la línea al producto.
    /// En modo no-TTY no hay línea que ceder, así que no cuenta ninguna.
    #[cfg(test)]
    closing_lines: Arc<AtomicUsize>,
    /// Repintados: veces que la fase se volvió a pintar sobre su línea sin que
    /// la fase cambiara. En no-TTY no abren línea nueva, así que se cuentan
    /// aparte de las líneas.
    #[cfg(test)]
    repaints: Arc<AtomicUsize>,
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
            announced: None,
            line_open: AtomicBool::new(false),
            block_opened: false,
            #[cfg(test)]
            renders: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            closing_lines: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            repaints: Arc::new(AtomicUsize::new(0)),
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
            announced: None,
            line_open: AtomicBool::new(false),
            block_opened: false,
            renders: Arc::new(AtomicUsize::new(0)),
            closing_lines: Arc::new(AtomicUsize::new(0)),
            repaints: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Líneas de fase o actividad pintadas hasta ahora. En modo no-TTY, cada
    /// una es una línea de stderr.
    #[cfg(test)]
    pub fn renders(&self) -> usize {
        self.renders.load(Ordering::Relaxed)
    }

    /// Líneas cedidas hasta ahora: veces que la línea se devolvió al producto.
    #[cfg(test)]
    pub fn closing_lines(&self) -> usize {
        self.closing_lines.load(Ordering::Relaxed)
    }

    /// Repintados hasta ahora: veces que la fase se volvió a pintar sobre su
    /// línea sin que la fase cambiara.
    ///
    /// Es distinto de `renders`, que cuenta líneas: en no-TTY la actividad
    /// repetida dentro de la misma fase no abre línea nueva, así que un
    /// repintado puede no producir ninguna línea. Por eso tiene su propio
    /// contador.
    #[cfg(test)]
    pub fn repaints(&self) -> usize {
        self.repaints.load(Ordering::Relaxed)
    }

    /// Cambia la fase actual.
    ///
    /// El primer `phase` de un indicador abre el bloque con una línea en
    /// blanco, como hace `ProvisionView::start`. En TTY la fase se repinta
    /// sobre la misma línea; en no-TTY se anuncia con una línea propia, y una
    /// repetición de la misma fase dentro del bloque no abre línea nueva.
    /// No-op si el indicador está deshabilitado.
    pub fn phase(&mut self, label: &'static str, msg: &str) {
        if !self.enabled {
            return;
        }
        if !self.block_opened {
            self.block_opened = true;
            eprintln!();
        }
        self.current = Some((label, msg.to_string()));
        if self.tty {
            self.paint();
            return;
        }
        // Racionado: solo la fase nueva se anuncia.
        if self.announced != Some(label) {
            self.announced = Some(label);
            self.paint();
        }
    }

    /// Actualiza la línea de la fase actual con el tiempo transcurrido
    /// actualizado. Útil en bucles de espera.
    ///
    /// En TTY repinta sobre la misma línea. En no-TTY no abre línea nueva: la
    /// actividad repetida dentro de la misma fase se raciona. El repintado se
    /// cuenta en cualquier modo, porque es el trabajo que el indicador hace
    /// aunque en no-TTY no produzca línea.
    pub fn tick(&self) {
        if !self.enabled || self.current.is_none() {
            return;
        }
        #[cfg(test)]
        self.repaints.fetch_add(1, Ordering::Relaxed);
        if !self.tty {
            return;
        }
        self.paint();
    }

    /// Cede la línea: la devuelve al producto para que nada quede pegado a
    /// lo que se escriba después.
    ///
    /// En TTY borra la línea de la fase y deja el cursor al principio; en
    /// no-TTY no hay línea que ceder y no escribe nada. Es idempotente: ceder
    /// una línea ya cedida no hace nada.
    pub fn close_line(&mut self) {
        if !self.enabled {
            return;
        }
        if !self.line_open.swap(false, Ordering::Relaxed) {
            return;
        }
        #[cfg(test)]
        self.closing_lines.fetch_add(1, Ordering::Relaxed);
        if self.tty {
            eprint!("\r\x1b[2K");
            #[expect(
                clippy::let_underscore_must_use,
                reason = "si stderr no admite el vaciado, el indicador sigue mostrando la fase"
            )]
            let _ = std::io::stderr().flush();
        }
    }

    /// Vuelve a pintar la fase actual sobre la línea, si la había.
    ///
    /// Es la operación complementaria de [`PhaseProgress::close_line`]: tras
    /// ceder la línea, `repaint` la devuelve a la fase vigente. En no-TTY no
    /// hay línea que repintar y no escribe nada.
    pub fn repaint(&mut self) {
        if !self.enabled || !self.tty || self.current.is_none() {
            return;
        }
        self.paint();
    }

    /// Cierra el indicador y devuelve la duración total.
    ///
    /// No imprime mensaje de estado: el estado lo dice quien cierra, con la
    /// duración que este cierre devuelve. Si la línea estaba abierta, la cede
    /// antes de terminar, para que nada quede pegado a la salida.
    pub fn close(mut self) -> Duration {
        let elapsed = self.start.elapsed();
        self.close_line();
        elapsed
    }

    /// Pinta la fase actual. En TTY la repinta sobre la misma línea y deja la
    /// línea abierta; en no-TTY escribe una línea propia.
    fn paint(&self) {
        let Some((_, msg)) = &self.current else {
            return;
        };
        let line = phase_line(msg, self.start.elapsed());
        #[cfg(test)]
        self.renders.fetch_add(1, Ordering::Relaxed);
        if self.tty {
            eprint!("\r\x1b[2K{line}");
            self.line_open.store(true, Ordering::Relaxed);
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

/// Cada cuánto deja una línea de avance un modelo cuando no hay terminal.
const NO_TTY_HEARTBEAT: Duration = Duration::from_secs(15);
/// Cadencia del latido interno para que el padre vigile descargas largas.
const WATCHDOG_HEARTBEAT: Duration = Duration::from_secs(5);

/// Línea periódica de avance de un modelo sin terminal:
/// `  … [i/N] nombre  1,2 GB de 2,5 GB (0:12)`.
///
/// Función pura, como el resto de las líneas del módulo, para que el texto
/// exacto sea comprobable sin capturar stderr.
pub fn transfer_line(label: &str, completed: u64, total: u64, elapsed: Duration) -> String {
    let elapsed_text = clock(elapsed);
    if total == 0 {
        return format!("  … {label}  {} ({elapsed_text})", human_bytes(completed));
    }
    format!(
        "  … {label}  {} de {} ({elapsed_text})",
        human_bytes(completed),
        human_bytes(total)
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
    fn transfer_line_shows_progress_and_elapsed() {
        assert_eq!(
            transfer_line(
                "[1/5] qwen3-tts-0.6b",
                1_200_000_000,
                2_500_000_000,
                Duration::from_secs(12)
            ),
            "  … [1/5] qwen3-tts-0.6b  1,2 GB de 2,5 GB (0:12)"
        );
        assert_eq!(
            transfer_line("[1/1] m", 1024, 0, Duration::from_secs(0)),
            "  … [1/1] m  1,0 KB (0:00)",
            "un total desconocido no inventa denominador"
        );
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

    // ─── Comportamiento del indicador de fase ───────────────────────────────
    //
    // Estas pruebas fijan el comportamiento vigente de `PhaseProgress` en modo
    // no-TTY, que es la forma en que un script lee la salida. La salida por
    // stderr no es capturable de forma portable en un test unitario, así que se
    // comprueba por dos vías: el texto exacto con la función pura `phase_line` y
    // el número de líneas emitidas con el contador que el indicador lleva para
    // las pruebas.

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

    /// La actividad repetida dentro de la misma fase **no** abre línea nueva
    /// en no-TTY: es el racionado del indicador.
    #[test]
    fn phase_then_tick_paints_one_line_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        assert_eq!(
            progress.renders(),
            1,
            "la actividad repetida no abre una segunda línea en no-TTY"
        );
    }

    /// Una fase nueva tras otra sí se anuncia: el racionado es por fase, no un
    /// tope global de líneas.
    #[test]
    fn a_new_phase_is_announced_again_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        progress.phase("stopping", "Deteniendo daemon…");
        progress.tick();
        assert_eq!(
            progress.renders(),
            2,
            "cada fase nueva se anuncia una vez: el racionado es por fase"
        );
    }

    /// Una fase con cuarenta actividades dentro **emite exactamente una
    /// línea**: la actividad repetida no abre línea nueva, y el racionado es
    /// por fase, no un tope global.
    #[test]
    fn one_phase_and_forty_ticks_paint_exactly_one_line_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        for _ in 0..40 {
            progress.tick();
        }
        assert_eq!(
            progress.renders(),
            1,
            "la actividad repetida dentro de la misma fase no abre línea nueva"
        );
    }

    /// `tick()` sin fase previa no pinta nada: no hay línea que repintar.
    #[test]
    fn tick_without_phase_paints_nothing() {
        let progress = PhaseProgress::for_test(None, false);
        progress.tick();
        assert_eq!(progress.renders(), 0);
    }

    /// El repintado se cuenta aunque no abra línea nueva: es el trabajo que
    /// hace el indicador para que el tiempo visible avance durante una
    /// espera, y en no-TTY no produce ninguna línea.
    ///
    /// Sin fase no hay nada que repintar, así que el contador no se mueve.
    #[test]
    fn repaints_are_counted_even_without_a_new_line() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        progress.tick();
        assert_eq!(
            progress.repaints(),
            2,
            "la actividad repetida repinta aunque no abra línea"
        );
        assert_eq!(
            progress.renders(),
            1,
            "y la línea sigue siendo una: el racionado no cede"
        );
    }

    /// `close` no imprime mensaje de estado: el indicador no dice cómo terminó
    /// la operación, solo cuánto duró.
    #[test]
    fn close_does_not_paint_a_closing_line() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        let renders = Arc::clone(&progress.renders);
        let closing_lines = Arc::clone(&progress.closing_lines);
        let elapsed = progress.close();
        assert_eq!(
            renders.load(Ordering::Relaxed),
            1,
            "el cierre no es una línea de fase"
        );
        assert_eq!(
            closing_lines.load(Ordering::Relaxed),
            0,
            "el cierre no imprime estado: lo dice quien cierra"
        );
        assert!(
            elapsed.as_secs() < 60,
            "la duración sale del reloj del indicador"
        );
    }

    /// `close_line` es idempotente: ceder una línea ya cedida no hace nada.
    ///
    /// La idempotencia se mide en TTY, que es la única rama que cede de
    /// verdad: en no-TTY no hay línea que ceder, así que el contador no se
    /// movería nunca y la aserción no distinguiría nada.
    #[test]
    fn close_line_is_idempotent_and_writes_nothing_in_no_tty() {
        let mut progress = PhaseProgress::for_test(None, false);
        progress.phase("cleaning", "Limpiando datos…");
        progress.close_line();
        progress.close_line();
        assert_eq!(
            progress.closing_lines(),
            0,
            "en no-TTY no hay línea que ceder"
        );
        assert_eq!(progress.renders(), 1, "ceder la línea no repinta la fase");
    }

    /// Ceder la línea dos veces cede una sola: la segunda no hace nada.
    ///
    /// Es la idempotencia medida, en la rama que cede de verdad.
    #[test]
    fn close_line_in_tty_is_idempotent() {
        let mut progress = PhaseProgress::for_test(None, true);
        progress.phase("cleaning", "Limpiando datos…");
        progress.close_line();
        progress.close_line();
        assert_eq!(
            progress.closing_lines(),
            1,
            "ceder la línea ya cedida no la cede otra vez"
        );
    }

    /// Tras ceder la línea, `repaint` vuelve a pintar la fase vigente.
    #[test]
    fn repaint_after_close_line_paints_the_phase_again() {
        let mut progress = PhaseProgress::for_test(None, true);
        progress.phase("cleaning", "Limpiando datos…");
        assert_eq!(progress.renders(), 1, "la fase se pinta al abrirla");
        progress.close_line();
        assert_eq!(
            progress.closing_lines(),
            1,
            "ceder la línea se cuenta una vez"
        );
        progress.repaint();
        assert_eq!(progress.renders(), 2, "tras ceder la línea, se repinta");
    }

    /// `AVI_NO_PROGRESS` con cualquier valor no vacío silencia toda salida:
    /// ni fases, ni actividad, ni cesión.
    ///
    /// La medida es en TTY, la única rama que escribe y que cuenta: en no-TTY
    /// el indicador nunca pintaría ni cedería, así que allí la aserción no
    /// distinguiría «silenciado» de «no-TTY».
    #[test]
    fn no_progress_silences_everything() {
        let mut progress = PhaseProgress::for_test(Some("1"), true);
        let renders = Arc::clone(&progress.renders);
        let closing_lines = Arc::clone(&progress.closing_lines);
        progress.phase("cleaning", "Limpiando datos…");
        progress.tick();
        progress.close_line();
        let _ = progress.close();
        assert_eq!(
            renders.load(Ordering::Relaxed),
            0,
            "el indicador silenciado no pinta"
        );
        assert_eq!(
            closing_lines.load(Ordering::Relaxed),
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
