// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Indicador de etapas de trabajo, por stderr.
//!
//! Cada etapa del catálogo ([`crate::stage`]) tiene su propio reloj. Una etapa
//! permanente deja al terminar la línea `✓ <Nombre>  <duración>`; una
//! transitoria no deja nada. Con terminal, la etapa en curso se repinta en una
//! sola línea debajo de las terminadas (`… <texto> <duración>`); sin terminal,
//! una etapa permanente se anuncia una vez, deja un latido cada 10 s y se
//! cierra con su línea terminada, sin `\r` ni secuencias ANSI. `AVI_NO_PROGRESS`
//! (cualquier valor no vacío) silencia todo salvo los resultados y la línea de
//! cierre de [`PhaseProgress::finish`].
//!
//! Cada transición se escribe además en el registro de ejecución
//! ([`crate::report`]), que es la fuente única del JSON y del manejador de
//! Ctrl+C. Todos los métodos toman `&self`: el estado vive tras un `Mutex`, así
//! que el indicador se comparte por referencia con el hilo que lo repinta y con
//! los receptores de eventos. Orden de cerrojos: primero el estado interno del
//! indicador y después el registro, y nunca al revés.

use std::io::IsTerminal;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::fmt;
use crate::line;
use crate::report::{self, ReportGuard, StageOutcome};
use crate::stage::Stage;
use crate::style::{color_enabled, paint_if, Stream, Tone, ACTIVE, FAIL, OK};

/// Ancho al que se rellena el texto de una línea terminada, para que las
/// duraciones queden alineadas.
const DONE_TEXT_WIDTH: usize = 16;
/// Intervalo mínimo entre dos repintados de la línea activa.
const MIN_REPAINT: Duration = Duration::from_millis(100);
/// Cada cuánto deja un latido una etapa permanente cuando no hay terminal.
const BEAT_INTERVAL: Duration = Duration::from_secs(10);
/// Cadencia del hilo que repinta mientras se espera un trabajo bloqueante.
const WHILE_RUNNING_TICK: Duration = Duration::from_millis(250);
/// Borra la línea actual de la terminal y deja el cursor al principio.
const CLEAR_LINE: &str = "\r\x1b[2K";
/// Sube el cursor una línea.
const CURSOR_UP: &str = "\x1b[1A";

/// Una etapa viva.
struct Current {
    /// Id de su registro en [`report`].
    id: u64,
    label: &'static str,
    active: String,
    /// Nombre de la etapa en su columna al terminar.
    name: String,
    /// Deja su línea terminada al cerrarse: el catálogo la da por permanente o
    /// `done_as` la hizo permanente.
    permanent: bool,
    /// El recorrido declarado del comando la incluye.
    declared: bool,
    /// `done_as` fijó el nombre: un `phase` del mismo `label` no lo pisa con el
    /// del catálogo.
    name_overridden: bool,
    started: Instant,
    note: Option<String>,
    /// Línea activa completa que sustituye a la estándar.
    live: Option<String>,
    /// Último latido sin terminal.
    last_beat: Instant,
}

/// Estado mutable del indicador.
struct Inner {
    /// La etapa principal: la que se pinta en la línea viva.
    current: Option<Current>,
    /// Segunda etapa viva, abierta con [`PhaseProgress::overlap`] sin cerrar la
    /// principal.
    overlapped: Option<Current>,
    last_paint: Option<Instant>,
    /// El indicador ya se cerró con `close`, `finish` o `fail`: su `Drop` no
    /// hace nada.
    closed: bool,
}

/// Indicador de etapas de trabajo. Todo va a stderr, nunca a stdout, salvo
/// los resultados de [`PhaseProgress::result`].
pub struct PhaseProgress {
    start: Instant,
    tty: bool,
    enabled: bool,
    machine: bool,
    /// Modo de etapa única: cerrar una etapa permanente no escribe su línea
    /// terminada.
    single_stage: bool,
    color: bool,
    repaint_every: Duration,
    beat_every: Duration,
    inner: Mutex<Inner>,
    /// Texto que habría salido por stderr. Solo existe en las pruebas, que no
    /// pueden capturar stderr de forma portable.
    #[cfg(test)]
    captured: std::sync::Arc<Mutex<String>>,
}

impl PhaseProgress {
    /// Crea el indicador: captura el reloj de inicio (que fija el origen del
    /// reloj del registro si es el primero del proceso) y decide terminal,
    /// silenciado, modo máquina y color una sola vez, para que cada llamada
    /// sea barata.
    pub fn new() -> Self {
        let start = Instant::now();
        report::lock().set_origin_if_none(start);
        Self {
            start,
            tty: std::io::stderr().is_terminal(),
            enabled: !progress_silenced(),
            machine: line::machine_mode(),
            single_stage: false,
            color: color_enabled(Stream::Stderr),
            repaint_every: MIN_REPAINT,
            beat_every: BEAT_INTERVAL,
            inner: Mutex::new(Inner {
                current: None,
                overlapped: None,
                last_paint: None,
                closed: false,
            }),
            #[cfg(test)]
            captured: std::sync::Arc::new(Mutex::new(String::new())),
        }
    }

    /// Indicador de pruebas con el valor de `AVI_NO_PROGRESS` y el modo de
    /// terminal fijados a mano, para no depender del entorno del proceso ni de
    /// que exista una terminal. Sin límite de repintado y sin color.
    #[cfg(test)]
    pub(crate) fn for_test(no_progress_value: Option<&str>, tty: bool) -> Self {
        let mut progress = Self::new();
        progress.tty = tty;
        progress.enabled = !silenced_by(no_progress_value);
        progress.machine = false;
        progress.color = false;
        progress.repaint_every = Duration::ZERO;
        progress
    }

    /// Activa el modo de etapa única, para un comando cuya única etapa de
    /// trabajo repetiría lo que dice el cierre: al cerrarse, una etapa
    /// permanente no escribe su línea `✓`, pero sí queda en el registro con su
    /// duración. Sin terminal se conservan el anuncio `… <texto>` y los
    /// latidos. En modo máquina no tiene efecto: no hay cierre, así que la
    /// línea `✓` de la etapa es la única que da su duración y se escribe como
    /// siempre.
    pub fn single_stage(mut self) -> Self {
        self.single_stage = true;
        self
    }

    /// Texto que el indicador ha escrito hasta ahora por stderr.
    #[cfg(test)]
    fn captured(&self) -> String {
        self.captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Toma el estado del indicador y, después, el registro: el único orden
    /// permitido.
    fn lock_both(&self) -> (MutexGuard<'_, Inner>, ReportGuard) {
        let inner = self.lock();
        (inner, report::lock())
    }

    /// Escribe `text` en stderr y lo vacía. En las pruebas lo acumula. Quien
    /// llama tiene tomada la guarda del registro.
    fn raw(&self, text: &str) {
        #[cfg(test)]
        self.captured
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push_str(text);
        #[cfg(not(test))]
        write_stderr(text);
    }

    /// Borra la línea de fase de la pantalla si estaba abierta.
    fn clear_line(&self) {
        if line::release_phase_line() {
            self.raw(CLEAR_LINE);
        }
    }

    /// Pinta la línea activa de la etapa en curso sobre la misma línea y
    /// publica que queda abierta.
    fn paint(&self, inner: &mut Inner) {
        let Some(current) = &inner.current else {
            return;
        };
        let text = current.live.clone().unwrap_or_else(|| {
            format!(
                "{ACTIVE} {} {}",
                current.active,
                paint_if(
                    self.color,
                    Tone::Dim,
                    &fmt::duration(current.started.elapsed())
                )
            )
        });
        self.raw(&format!("{CLEAR_LINE}{text}"));
        line::set_phase_line_open(true);
        inner.last_paint = Some(Instant::now());
    }

    /// ¿Ya pasó el intervalo mínimo desde el último repintado?
    fn repaint_due(&self, inner: &Inner) -> bool {
        inner
            .last_paint
            .is_none_or(|last| last.elapsed() >= self.repaint_every)
    }

    /// Cierra una etapa en el instante `at`: una permanente deja su línea
    /// terminada (salvo en modo de etapa única), una transitoria se borra.
    fn close_one(&self, current: Current, rep: &mut ReportGuard, at: Instant) {
        let elapsed = at.saturating_duration_since(current.started);
        rep.close_stage(current.id, at, None);
        if !self.enabled {
            return;
        }
        if self.tty {
            self.clear_line();
        }
        if self.single_stage && !self.machine {
            return;
        }
        if current.permanent {
            let line = finished_line(self.color, &current.name, elapsed, current.note.as_deref());
            self.raw(&format!("{line}\n"));
        }
    }

    /// Cierra una etapa como fallida en `at` y pinta `✗ <Nombre> <duración>`.
    fn fail_one(&self, current: Current, rep: &mut ReportGuard, at: Instant) {
        let elapsed = at.saturating_duration_since(current.started);
        rep.close_stage(current.id, at, Some(StageOutcome::Failed));
        if !self.enabled {
            return;
        }
        if self.tty {
            self.clear_line();
        }
        let line = failed_line(self.color, &current.name, elapsed);
        self.raw(&format!("{line}\n"));
    }

    /// Cierra todas las etapas vivas.
    fn close_all(&self, inner: &mut Inner, rep: &mut ReportGuard) {
        let at = Instant::now();
        if let Some(current) = inner.current.take() {
            self.close_one(current, rep, at);
        }
        if let Some(overlapped) = inner.overlapped.take() {
            self.close_one(overlapped, rep, at);
        }
    }

    /// Abre una etapa viva con inicio `started` y la registra.
    fn open(
        &self,
        rep: &mut ReportGuard,
        stage: &'static Stage,
        active: &str,
        started: Instant,
    ) -> Current {
        let id = rep.open_stage(stage, started);
        Current {
            id,
            label: stage.label,
            active: active.to_string(),
            name: stage.name.to_string(),
            permanent: !stage.transient,
            declared: rep.is_declared(stage.label),
            name_overridden: false,
            started,
            note: None,
            live: None,
            last_beat: started,
        }
    }

    /// Abre la etapa del catálogo con su texto activo. Si ya es la etapa en
    /// curso, solo actualiza sus textos y no reinicia su reloj; si es otra,
    /// cierra la actual y abre la nueva con un reloj propio.
    pub fn phase(&self, stage: &'static Stage) {
        self.phase_with(stage, stage.active);
    }

    /// Igual que [`PhaseProgress::phase`], con un texto activo dinámico, por
    /// ejemplo `Descargando la versión 0.46.0…`.
    pub fn phase_with(&self, stage: &'static Stage, active: &str) {
        let (mut inner, mut rep) = self.lock_both();
        if let Some(current) = inner.current.as_mut() {
            if current.label == stage.label {
                current.active = active.to_string();
                if !current.name_overridden {
                    current.name = stage.name.to_string();
                    current.permanent = !stage.transient;
                    rep.rename_stage(current.id, stage.name, current.permanent);
                }
                if self.enabled && self.tty {
                    self.paint(&mut inner);
                }
                return;
            }
        }
        self.close_all(&mut inner, &mut rep);
        let current = self.open(&mut rep, stage, active, Instant::now());
        inner.current = Some(current);
        if !self.enabled {
            return;
        }
        if self.tty {
            self.paint(&mut inner);
        } else if !stage.transient {
            self.raw(&format!("{ACTIVE} {active}\n"));
        }
    }

    /// Abre una segunda etapa viva con inicio explícito `at`, sin cerrar la
    /// actual: sirve para dos etapas que corren a la vez, como la síntesis y la
    /// reproducción que empieza con la primera muestra. Si no había ninguna
    /// etapa en curso, queda como la principal.
    pub fn overlap(&self, stage: &'static Stage, at: Instant) {
        let (mut inner, mut rep) = self.lock_both();
        let already_open = [inner.current.as_ref(), inner.overlapped.as_ref()]
            .into_iter()
            .flatten()
            .any(|current| current.label == stage.label);
        if already_open {
            return;
        }
        let opened = self.open(&mut rep, stage, stage.active, at);
        if inner.current.is_none() {
            inner.current = Some(opened);
        } else {
            inner.overlapped = Some(opened);
        }
        if self.enabled && !self.tty && !stage.transient {
            self.raw(&format!("{ACTIVE} {}\n", stage.active));
        }
    }

    /// Cierra una etapa concreta con un instante explícito, sin tocar la otra
    /// si hay dos vivas. Si era la principal, la segunda pasa a serlo. Si
    /// ninguna etapa viva es `stage`, no hace nada.
    pub fn end_stage_at(&self, stage: &'static Stage, at: Instant) {
        let (mut inner, mut rep) = self.lock_both();
        if inner
            .current
            .as_ref()
            .is_some_and(|c| c.label == stage.label)
        {
            if let Some(current) = inner.current.take() {
                self.close_one(current, &mut rep, at);
            }
            inner.current = inner.overlapped.take();
        } else if inner
            .overlapped
            .as_ref()
            .is_some_and(|c| c.label == stage.label)
        {
            if let Some(overlapped) = inner.overlapped.take() {
                self.close_one(overlapped, &mut rep, at);
            }
        }
    }

    /// Registra una etapa omitida durante la ejecución, con `start == end` en
    /// el instante en que se marca, y su motivo. No abre ni cierra ninguna
    /// etapa viva.
    pub fn skip(&self, stage: &'static Stage, reason: &str) {
        let (_inner, mut rep) = self.lock_both();
        rep.skip_stage(stage, reason, Instant::now());
    }

    /// Marca la etapa en curso como fallida y pinta `✗ <Nombre> <duración>`.
    /// Si había una segunda etapa viva, esta pasa a ser la principal. Sin
    /// etapa en curso no hace nada.
    pub fn fail_current(&self) {
        let (mut inner, mut rep) = self.lock_both();
        if let Some(current) = inner.current.take() {
            self.fail_one(current, &mut rep, Instant::now());
            inner.current = inner.overlapped.take();
        }
    }

    /// Fija el nombre de la etapa en curso en su columna y la hace permanente:
    /// sustituye al del catálogo, incluso en una transitoria.
    pub fn done_as(&self, text: &str) {
        let (mut inner, mut rep) = self.lock_both();
        if let Some(current) = inner.current.as_mut() {
            current.name = text.to_string();
            current.name_overridden = true;
            current.permanent = true;
            rep.rename_stage(current.id, text, true);
        }
    }

    /// Añade ` · <texto>` a la línea terminada de la etapa en curso.
    pub fn note(&self, text: &str) {
        let (mut inner, mut rep) = self.lock_both();
        if let Some(current) = inner.current.as_mut() {
            current.note = Some(text.to_string());
            rep.set_note(current.id, text);
        }
    }

    /// Sustituye la línea activa completa de la etapa en curso. Sin terminal
    /// no escribe nada.
    pub fn live(&self, line: String) {
        let (mut inner, _rep) = self.lock_both();
        let Some(current) = inner.current.as_mut() else {
            return;
        };
        current.live = Some(line);
        if self.enabled && self.tty && self.repaint_due(&inner) {
            self.paint(&mut inner);
        }
    }

    /// Repinta la línea activa con el tiempo actualizado, como mucho cada
    /// 100 ms. Sin terminal, deja el latido de una etapa permanente si toca.
    pub fn tick(&self) {
        if !self.enabled {
            return;
        }
        let (mut inner, _rep) = self.lock_both();
        if inner.current.is_none() {
            return;
        }
        if self.tty {
            if self.repaint_due(&inner) {
                self.paint(&mut inner);
            }
            return;
        }
        let beat_every = self.beat_every;
        let Some(current) = inner.current.as_mut() else {
            return;
        };
        if current.permanent && current.last_beat.elapsed() >= beat_every {
            current.last_beat = Instant::now();
            let text = format!(
                "  {ACTIVE} {} {}\n",
                current.active,
                fmt::duration(current.started.elapsed())
            );
            self.raw(&text);
        }
    }

    /// Cede la línea: la borra para que nada quede pegado a lo que se escriba
    /// después. Sin terminal no hay línea que ceder. Es idempotente: ceder una
    /// línea ya cedida, también la que cedió un aviso, no hace nada.
    pub fn yield_line(&self) {
        if !self.enabled || !self.tty {
            return;
        }
        let (_inner, _rep) = self.lock_both();
        self.clear_line();
    }

    /// Sube el cursor una línea cuando la terminal ha hecho eco de un Enter
    /// que el usuario pulsó mientras la línea viva seguía abierta: el eco baja
    /// el cursor, y sin subir, el siguiente borrado limpiaría la línea nueva y
    /// dejaría la viva congelada encima. El llamador la invoca solo cuando
    /// leyó un Enter de un stdin que es terminal. Sin terminal en stderr, en
    /// modo máquina, silenciado o sin línea viva abierta no escribe nada.
    pub fn climb_echoed_enter(&self) {
        if !self.enabled || !self.tty || self.machine || !line::phase_line_open() {
            return;
        }
        let (_inner, _rep) = self.lock_both();
        self.raw(CURSOR_UP);
    }

    /// Recupera la línea cedida: vuelve a pintar la etapa en curso, si la hay.
    pub fn reclaim(&self) {
        if !self.enabled || !self.tty {
            return;
        }
        let (mut inner, _rep) = self.lock_both();
        if inner.current.is_some() {
            self.paint(&mut inner);
        }
    }

    /// Cierra la etapa en curso sin abrir otra: una permanente deja su línea
    /// terminada, una transitoria se borra. Sirve para fijar la duración de una
    /// etapa antes de una pausa que no es trabajo, como una pregunta al usuario.
    pub fn end_stage(&self) {
        let (mut inner, mut rep) = self.lock_both();
        self.close_all(&mut inner, &mut rep);
    }

    /// Abandona la etapa en curso sin dejar línea terminada, sea permanente o
    /// transitoria: borra su línea activa y la quita del registro. Sirve cuando
    /// el trabajo de la etapa no se completó (se omitió o falló) y el llamador
    /// escribe su propia línea en su lugar.
    pub fn discard_stage(&self) {
        let (mut inner, mut rep) = self.lock_both();
        if let Some(current) = inner.current.take() {
            rep.remove_stage(current.id);
            inner.current = inner.overlapped.take();
            if self.enabled && self.tty {
                self.clear_line();
            }
        }
    }

    /// Cierra la etapa en curso, con su línea terminada si es permanente, y
    /// escribe el resultado por stdout. Con terminal en stdout:
    /// `  <etiqueta>: <texto en negrita>`, o `  <texto>` sin etiqueta; sin
    /// terminal: `<etiqueta>: <texto>`, o `<texto>`. En modo máquina no
    /// escribe nada. El resultado queda ligado en el registro a la etapa
    /// que acaba de cerrarse.
    pub fn result(&self, label: Option<&str>, text: &str) {
        {
            let (mut inner, mut rep) = self.lock_both();
            self.close_all(&mut inner, &mut rep);
            rep.add_result(label, text);
        }
        if self.machine {
            return;
        }
        let stdout_tty = std::io::stdout().is_terminal();
        println!(
            "{}",
            result_line(stdout_tty, color_enabled(Stream::Stdout), label, text)
        );
    }

    /// Ejecuta `f` en el hilo actual mientras un hilo auxiliar repinta la
    /// etapa cada 250 ms, hasta que `f` termina, también si entra en pánico.
    ///
    /// `f` y su resultado no necesitan ser `Send`: sus cierres pueden capturar
    /// objetos que solo se usan desde el hilo que los creó, como los
    /// reproductores de audio. Solo se comparte el indicador, que es `Sync`.
    pub fn while_running<T>(&self, f: impl FnOnce() -> T) -> T {
        let (stop, stopped) = std::sync::mpsc::channel::<()>();
        std::thread::scope(|scope| {
            scope.spawn(move || {
                // Solo un `Timeout` implica que el trabajo sigue en marcha; al
                // cerrarse el canal (el emisor se suelta) se sale sin repintar.
                while let Err(std::sync::mpsc::RecvTimeoutError::Timeout) =
                    stopped.recv_timeout(WHILE_RUNNING_TICK)
                {
                    self.tick();
                }
            });
            // El emisor se suelta al salir del cierre, incluso por pánico.
            let _stop = stop;
            f()
        })
    }

    /// Tiempo total desde que se creó el indicador.
    pub fn elapsed(&self) -> Duration {
        self.start.elapsed()
    }

    /// Cierra la etapa en curso y devuelve el tiempo total, sin línea de
    /// cierre.
    pub fn close(self) -> Duration {
        {
            let (mut inner, mut rep) = self.lock_both();
            self.close_all(&mut inner, &mut rep);
            inner.closed = true;
        }
        self.start.elapsed()
    }

    /// Como [`PhaseProgress::close`], y escribe por stderr la línea de cierre
    /// `✓ <acción> en <duración total>` seguida de ` · <extra>` por cada
    /// extra. En modo máquina no escribe la línea de cierre.
    pub fn finish(self, action: &str, extras: &[String]) -> Duration {
        let machine = self.machine;
        let color = self.color;
        let elapsed = self.close();
        if !machine {
            let _rep = report::lock();
            eprintln!("{}", closing_line(color, action, elapsed, extras));
        }
        elapsed
    }

    /// Termina sin cierre: si queda una etapa abierta, permanente o declarada,
    /// la registra como fallida y pinta `✗ <Nombre> <duración>`; si no, solo
    /// borra la línea activa.
    pub fn fail(self) {
        // El `Drop` hace el trabajo: un indicador soltado sin `close`,
        // `finish` ni `fail` equivale a `fail`.
    }
}

impl Default for PhaseProgress {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for PhaseProgress {
    fn drop(&mut self) {
        let inner = self.inner.get_mut().unwrap_or_else(|e| e.into_inner());
        if inner.closed {
            return;
        }
        let open = [inner.current.take(), inner.overlapped.take()];
        let mut rep = report::lock();
        let at = Instant::now();
        for current in open.into_iter().flatten() {
            if current.permanent || current.declared {
                self.fail_one(current, &mut rep, at);
            } else {
                rep.close_stage(current.id, at, None);
            }
        }
        if self.enabled && self.tty {
            self.clear_line();
        }
    }
}

/// ¿Silencia el indicador el valor de `AVI_NO_PROGRESS`? Cualquier valor no
/// vacío lo silencia; el vacío y la ausencia lo dejan hablar.
fn silenced_by(value: Option<&str>) -> bool {
    value.is_some_and(|v| !v.is_empty())
}

/// ¿Pide el entorno silenciar las etapas (`AVI_NO_PROGRESS` con un valor no
/// vacío)? Lo usan también las vistas de progreso que no son un
/// [`PhaseProgress`].
pub fn progress_silenced() -> bool {
    silenced_by(std::env::var("AVI_NO_PROGRESS").ok().as_deref())
}

/// Línea terminada de una etapa permanente: `✓ ` + el nombre relleno a 16
/// caracteres + la duración atenuada + ` · <nota>` si la hay.
pub fn finished_line(color: bool, text: &str, elapsed: Duration, note: Option<&str>) -> String {
    let mut line = format!(
        "{} {text:<width$}  {}",
        paint_if(color, Tone::Ok, OK),
        paint_if(color, Tone::Dim, &fmt::duration(elapsed)),
        width = DONE_TEXT_WIDTH
    );
    if let Some(note) = note {
        line.push_str(&format!(" · {note}"));
    }
    line
}

/// Línea de una etapa fallida: `✗ ` + el nombre relleno a 16 caracteres + la
/// duración atenuada.
fn failed_line(color: bool, text: &str, elapsed: Duration) -> String {
    format!(
        "{} {text:<width$}  {}",
        paint_if(color, Tone::Error, FAIL),
        paint_if(color, Tone::Dim, &fmt::duration(elapsed)),
        width = DONE_TEXT_WIDTH
    )
}

/// Línea de cierre de un comando: `✓ <acción> en <duración>` seguida de
/// ` · <extra>` por cada extra.
pub fn closing_line(color: bool, action: &str, elapsed: Duration, extras: &[String]) -> String {
    let mut line = format!(
        "{} {action} en {}",
        paint_if(color, Tone::Ok, OK),
        fmt::duration(elapsed)
    );
    for extra in extras {
        line.push_str(&format!(" · {extra}"));
    }
    line
}

/// Línea de un resultado por stdout. Con terminal, sangrada y con el texto en
/// negrita; sin ella, el texto limpio para que un script lo lea.
pub fn result_line(stdout_tty: bool, color: bool, label: Option<&str>, text: &str) -> String {
    if stdout_tty {
        let text = paint_if(color, Tone::Strong, text);
        match label {
            Some(label) => format!("  {label}: {text}"),
            None => format!("  {text}"),
        }
    } else {
        match label {
            Some(label) => format!("{label}: {text}"),
            None => text.to_string(),
        }
    }
}

/// Escribe en stderr y lo vacía. Si stderr no admite la escritura, el
/// indicador sigue funcionando sin mostrar nada.
#[cfg(not(test))]
fn write_stderr(text: &str) {
    use std::io::Write;
    let mut stderr = std::io::stderr().lock();
    #[expect(
        clippy::let_underscore_must_use,
        reason = "si stderr no admite la escritura, el indicador sigue sin mostrar nada"
    )]
    let _ = stderr.write_all(text.as_bytes());
    #[expect(
        clippy::let_underscore_must_use,
        reason = "si stderr no admite el vaciado, el indicador sigue mostrando la etapa"
    )]
    let _ = stderr.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Serializa las pruebas que crean un indicador: la línea de fase abierta
    /// y el registro de ejecución son estado global del proceso.
    fn tty_lock() -> MutexGuard<'static, ()> {
        report::test_guard()
    }

    fn lines(text: &str) -> Vec<&str> {
        text.lines().collect()
    }

    #[test]
    fn finished_line_pads_the_text_and_appends_duration_and_note() {
        assert_eq!(
            finished_line(false, "Lectura", Duration::from_millis(700), None),
            "✓ Lectura           0,7 s"
        );
        assert_eq!(
            finished_line(
                false,
                "Descarga verificada",
                Duration::from_secs(155),
                Some("52,0 MB")
            ),
            "✓ Descarga verificada  2 min 35 s · 52,0 MB"
        );
        assert_eq!(
            finished_line(true, "Ok", Duration::ZERO, None),
            "\x1b[32m✓\x1b[0m Ok                \x1b[2m0,0 s\x1b[0m"
        );
    }

    #[test]
    fn closing_line_joins_the_extras() {
        assert_eq!(
            closing_line(false, "Doblaje listo", Duration::from_secs(12), &[]),
            "✓ Doblaje listo en 12,0 s"
        );
        assert_eq!(
            closing_line(
                false,
                "Doblaje listo",
                Duration::from_secs(12),
                &["primer audio 1,2 s".to_string(), "RTF 0,5".to_string()]
            ),
            "✓ Doblaje listo en 12,0 s · primer audio 1,2 s · RTF 0,5"
        );
    }

    #[test]
    fn result_line_depends_on_stdout_being_a_terminal() {
        assert_eq!(result_line(false, false, None, "hola"), "hola");
        assert_eq!(
            result_line(false, false, Some("Texto"), "hola"),
            "Texto: hola"
        );
        assert_eq!(result_line(true, false, None, "hola"), "  hola");
        assert_eq!(
            result_line(true, false, Some("Texto"), "hola"),
            "  Texto: hola"
        );
        assert_eq!(
            result_line(true, true, Some("Texto"), "hola"),
            "  Texto: \x1b[1mhola\x1b[0m"
        );
    }

    /// Sin terminal, una etapa permanente se anuncia una vez, deja su línea
    /// terminada al cerrarse y no escribe línea en blanco inicial.
    #[test]
    fn permanent_stage_without_tty_announces_once_and_leaves_a_done_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::READING);
        progress.tick();
        progress.phase(&stage::READING);
        let out = progress.captured();
        assert_eq!(lines(&out), ["… Leyendo el audio…"]);
        progress.phase(&stage::TRANSCRIBING);
        let out = progress.captured();
        let all = lines(&out);
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(all[1].starts_with("✓ Lectura"), "{all:?}");
        assert_eq!(all[2], "… Transcribiendo…");
        assert!(!out.contains('\r'), "sin terminal no hay retornos de carro");
        assert!(!out.contains('\x1b'), "sin terminal no hay secuencias ANSI");
    }

    /// Una etapa transitoria no escribe nada sin terminal, ni al abrirse ni al
    /// cerrarse.
    #[test]
    fn transient_stage_without_tty_writes_nothing() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::VALIDATING);
        progress.tick();
        progress.phase(&stage::SAVING);
        let _ = progress.close();
    }

    /// `discard_stage` abandona la etapa permanente sin dejar su línea
    /// terminada y deja el indicador listo para abrir otra con reloj propio.
    #[test]
    fn discard_stage_leaves_no_done_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::DENOISING);
        progress.discard_stage();
        progress.phase(&stage::DENOISING);
        progress.end_stage();
        let out = progress.captured();
        let all = lines(&out);
        assert_eq!(all.len(), 3, "{all:?}");
        assert_eq!(all[0], "… Limpiando el ruido…");
        assert_eq!(all[1], "… Limpiando el ruido…");
        assert!(all[2].starts_with("✓ Limpieza"), "{all:?}");
        assert_eq!(
            report::snapshot().stages.len(),
            1,
            "la etapa descartada no queda en el registro"
        );
    }

    /// `end_stage` deja la línea terminada de la etapa permanente sin abrir
    /// otra, así que el cierre posterior no vuelve a escribirla.
    #[test]
    fn end_stage_closes_the_current_stage_once() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::PLAYING);
        progress.end_stage();
        progress.end_stage();
        let out = progress.captured();
        let all = lines(&out);
        assert_eq!(all.len(), 2, "{all:?}");
        assert!(all[1].starts_with("✓ Reproducción"), "{all:?}");
        let captured = std::sync::Arc::clone(&progress.captured);
        let _ = progress.close();
        let after = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
        assert_eq!(after, out, "cerrar después no repite la línea terminada");
    }

    /// En modo de etapa única, cerrar una etapa permanente (con `close` o con
    /// `end_stage`) no escribe su línea `✓`, con o sin terminal; sin terminal
    /// queda el anuncio `… <texto>`, y la etapa queda en el registro.
    #[test]
    fn single_stage_closes_a_permanent_stage_without_its_line() {
        let _guard = tty_lock();
        for tty in [false, true] {
            for use_end_stage in [false, true] {
                let progress = PhaseProgress::for_test(None, tty).single_stage();
                progress.phase(&stage::TRANSLATING);
                if use_end_stage {
                    progress.end_stage();
                }
                let captured = std::sync::Arc::clone(&progress.captured);
                let _ = progress.close();
                let out = captured.lock().unwrap_or_else(|e| e.into_inner()).clone();
                assert!(
                    !out.contains('✓'),
                    "tty={tty} end_stage={use_end_stage}: {out}"
                );
                if !tty {
                    assert_eq!(out, "… Traduciendo…\n", "{out}");
                }
            }
        }
    }

    /// La etapa cerrada en modo de etapa única queda en el registro con su
    /// instante de cierre.
    #[test]
    fn single_stage_keeps_the_closed_stage_in_the_report() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false).single_stage();
        progress.phase(&stage::TRANSLATING);
        progress.phase(&stage::SAVING);
        let snap = report::snapshot();
        assert_eq!(snap.stages[0].label, "translating");
        assert!(snap.stages[0].end.is_some());
    }

    /// En modo máquina el modo de etapa única no actúa: la línea `✓` se
    /// escribe como siempre.
    #[test]
    fn single_stage_has_no_effect_in_machine_mode() {
        let _guard = tty_lock();
        let mut progress = PhaseProgress::for_test(None, false).single_stage();
        progress.machine = true;
        progress.phase(&stage::TRANSLATING);
        progress.end_stage();
        let out = progress.captured();
        assert!(out.contains("✓ Traducción"), "{out}");
    }

    /// `done_as` convierte una transitoria en permanente y cambia el nombre
    /// de una permanente.
    #[test]
    fn done_as_makes_a_transient_stage_permanent_and_renames_a_permanent_one() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::STOPPING);
        progress.done_as("Daemon detenido");
        progress.phase(&stage::SYNTHESIZING);
        progress.done_as("Primer audio");
        progress.note("1 s");
        progress.phase(&stage::PLAYING);
        let out = progress.captured();
        assert!(out.contains("✓ Daemon detenido"), "{out}");
        assert!(out.contains("✓ Primer audio"), "{out}");
        assert!(out.contains(" · 1 s"), "{out}");
        assert!(!out.contains("✓ Síntesis"), "{out}");
        let snap = report::snapshot();
        assert_eq!(snap.stages[0].name, "Daemon detenido");
        assert_eq!(snap.stages[0].outcome, StageOutcome::Ok);
        assert_eq!(snap.stages[1].name, "Primer audio");
        assert_eq!(snap.stages[1].note.as_deref(), Some("1 s"));
    }

    /// El mismo `label` solo actualiza los textos y conserva el reloj y el
    /// nombre fijado con `done_as`.
    #[test]
    fn same_label_keeps_the_clock_and_the_done_override() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::SYNTHESIZING);
        progress.done_as("Primer audio");
        let started = progress.lock().current.as_ref().unwrap().started;
        std::thread::sleep(Duration::from_millis(5));
        progress.phase_with(&stage::SYNTHESIZING, "Sintetizando otra toma…");
        let inner = progress.lock();
        let current = inner.current.as_ref().unwrap();
        assert_eq!(current.started, started, "el reloj no se reinicia");
        assert_eq!(current.active, "Sintetizando otra toma…");
        assert_eq!(current.name, "Primer audio");
    }

    /// Sin terminal, una etapa permanente deja un latido cada intervalo.
    #[test]
    fn permanent_stage_without_tty_beats_when_due() {
        let _guard = tty_lock();
        let mut progress = PhaseProgress::for_test(None, false);
        progress.beat_every = Duration::ZERO;
        progress.phase(&stage::DOWNLOADING);
        progress.tick();
        progress.tick();
        let out = progress.captured();
        assert_eq!(out.matches("  … Descargando… ").count(), 2, "{out}");
        let mut quiet = PhaseProgress::for_test(None, false);
        quiet.beat_every = Duration::from_secs(3600);
        quiet.phase(&stage::DOWNLOADING);
        quiet.tick();
        assert_eq!(lines(&quiet.captured()).len(), 1);
    }

    /// Con terminal, la etapa se pinta sobre la misma línea y la cierra una
    /// línea terminada; una transitoria se borra sin dejar nada.
    #[test]
    fn tty_paints_in_place_and_clears_a_transient_stage() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::VALIDATING);
        assert!(line::phase_line_open());
        progress.phase(&stage::READING);
        let out = progress.captured();
        assert!(out.starts_with("\r\x1b[2K… Validando… 0,"), "{out:?}");
        assert!(!out.contains('✓'), "la transitoria no deja línea: {out:?}");
        progress.tick();
        let _ = progress.close();
        assert!(!line::phase_line_open());
    }

    /// Con terminal, cerrar una etapa permanente borra la activa y escribe la
    /// terminada.
    #[test]
    fn tty_permanent_stage_is_replaced_by_its_done_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        progress.phase(&stage::TRANSCRIBING);
        let out = progress.captured();
        assert!(out.contains("\r\x1b[2K✓ Lectura"), "{out:?}");
        let _ = progress.close();
    }

    /// Ceder la línea es idempotente y recuperarla vuelve a pintar la etapa.
    #[test]
    fn yield_line_is_idempotent_and_reclaim_paints_again() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        progress.yield_line();
        progress.yield_line();
        let out = progress.captured();
        assert_eq!(out.matches("… Leyendo").count(), 1);
        assert_eq!(out.matches(CLEAR_LINE).count(), 2, "pintar + ceder una vez");
        assert!(!line::phase_line_open());
        progress.reclaim();
        assert!(line::phase_line_open());
        assert_eq!(progress.captured().matches("… Leyendo").count(), 2);
        let _ = progress.close();
    }

    /// Un aviso de `line` cede la línea del indicador; el siguiente repintado
    /// la recupera debajo del aviso.
    #[test]
    fn a_console_warning_takes_the_line_and_tick_reopens_it() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        assert!(line::phase_line_open());
        line::warn("prueba de cesión por aviso");
        assert!(!line::phase_line_open());
        progress.yield_line();
        assert_eq!(
            progress.captured().matches(CLEAR_LINE).count(),
            1,
            "la línea ya la cedió el aviso"
        );
        progress.tick();
        assert!(line::phase_line_open());
        let _ = progress.close();
    }

    /// `live` sustituye la línea activa completa con terminal y no escribe
    /// nada sin ella.
    #[test]
    fn live_replaces_the_active_line_only_with_a_terminal() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::CAPTURING);
        progress.live("● Grabando 0:05 / 0:40".to_string());
        progress.tick();
        let out = progress.captured();
        assert!(out.ends_with("\r\x1b[2K● Grabando 0:05 / 0:40"), "{out:?}");
        let _ = progress.close();

        let plain = PhaseProgress::for_test(None, false);
        plain.phase(&stage::CAPTURING);
        plain.live("● Grabando 0:05 / 0:40".to_string());
        assert!(!plain.captured().contains("Grabando 0:05"));
    }

    /// Tras el eco de un Enter, el indicador sube una línea antes de borrar:
    /// la línea viva desaparece y solo queda la terminada. Sin terminal no
    /// escribe nada.
    #[test]
    fn climb_echoed_enter_moves_up_before_clearing_the_live_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::CAPTURING);
        progress.live("● Grabando 0:05 / 0:40".to_string());
        progress.climb_echoed_enter();
        progress.phase(&stage::VALIDATING);
        let out = progress.captured();
        assert!(out.contains("\x1b[1A\r\x1b[2K✓ Grabación"), "{out:?}");
        let _ = progress.close();

        let plain = PhaseProgress::for_test(None, false);
        plain.phase(&stage::CAPTURING);
        plain.climb_echoed_enter();
        assert!(!plain.captured().contains("\x1b[1A"));
    }

    /// `AVI_NO_PROGRESS` silencia etapas, latidos y cesiones, también con
    /// terminal.
    #[test]
    fn no_progress_silences_everything() {
        let _guard = tty_lock();
        for tty in [true, false] {
            let progress = PhaseProgress::for_test(Some("1"), tty);
            progress.phase(&stage::READING);
            progress.tick();
            progress.yield_line();
            progress.reclaim();
            progress.phase(&stage::TRANSCRIBING);
            progress.live("x".to_string());
            let captured = progress.captured.clone();
            let _ = progress.close();
            assert_eq!(*captured.lock().unwrap(), "", "tty = {tty}");
        }
    }

    /// El valor vacío de `AVI_NO_PROGRESS` no silencia: solo el no vacío.
    #[test]
    fn empty_no_progress_value_does_not_silence() {
        assert!(!silenced_by(Some("")));
        assert!(!silenced_by(None));
        assert!(silenced_by(Some("0")));
        assert!(silenced_by(Some("1")));
    }

    /// `close` no escribe línea de cierre y suelta la línea abierta; el
    /// `Drop` de un indicador sin cerrar la borra.
    #[test]
    fn close_and_drop_leave_no_open_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        let elapsed = progress.close();
        assert!(elapsed < Duration::from_secs(60));
        assert!(!line::phase_line_open());

        let abandoned = PhaseProgress::for_test(None, true);
        abandoned.phase(&stage::READING);
        assert!(line::phase_line_open());
        abandoned.fail();
        assert!(!line::phase_line_open());
    }

    /// `fail` no escribe la línea terminada de la etapa en curso: pinta `✗`
    /// con su nombre y la registra como fallida.
    #[test]
    fn fail_paints_the_failure_line_instead_of_the_done_line() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        let captured = progress.captured.clone();
        progress.fail();
        let out = captured.lock().unwrap().clone();
        assert!(out.contains("Leyendo"), "{out:?}");
        assert!(!out.contains('✓'), "{out:?}");
        assert!(out.contains("✗ Lectura"), "{out:?}");
        assert!(!line::phase_line_open());
        assert_eq!(report::snapshot().stages[0].outcome, StageOutcome::Failed);
    }

    /// En modo máquina `result` no escribe y las etapas siguen en stderr.
    #[test]
    fn machine_mode_keeps_the_stages_and_drops_the_result() {
        let _guard = tty_lock();
        let mut progress = PhaseProgress::for_test(None, false);
        progress.machine = true;
        progress.phase(&stage::READING);
        progress.result(Some("Texto"), "no debe salir");
        let out = progress.captured();
        assert!(out.contains("… Leyendo el audio…"), "{out}");
        assert!(out.contains("✓ Lectura"), "`result` cierra la etapa: {out}");
    }

    /// `while_running` repinta desde un hilo auxiliar mientras dura el trabajo
    /// y devuelve el valor, sin exigir `Send` al trabajo.
    #[test]
    fn while_running_ticks_during_the_work_and_returns_its_value() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        let not_send = std::rc::Rc::new(7u32);
        let value = progress.while_running(|| *not_send + 1);
        assert_eq!(value, 8);
    }

    /// El hilo auxiliar repinta mientras dura un trabajo de varios intervalos y
    /// se detiene en cuanto termina: no hay repintados posteriores al retorno.
    #[test]
    fn while_running_ticks_during_long_work_and_stops_after() {
        let _guard = tty_lock();
        let mut progress = PhaseProgress::for_test(None, false);
        progress.beat_every = Duration::ZERO;
        progress.phase(&stage::DOWNLOADING);
        let value = progress.while_running(|| {
            std::thread::sleep(WHILE_RUNNING_TICK * 2 + Duration::from_millis(100));
            3u32
        });
        assert_eq!(value, 3);
        let beats = |p: &PhaseProgress| p.captured().matches("  … Descargando…").count();
        let at_return = beats(&progress);
        assert!(
            at_return >= 1,
            "el trabajo largo debe repintar: {at_return}"
        );
        std::thread::sleep(WHILE_RUNNING_TICK + Duration::from_millis(100));
        assert_eq!(beats(&progress), at_return, "ningún repintado tras volver");
    }

    /// Si el trabajo entra en pánico, el hilo auxiliar se detiene y el pánico
    /// se propaga.
    #[test]
    fn while_running_stops_the_helper_when_the_work_panics() {
        let _guard = tty_lock();
        let progress = PhaseProgress::for_test(None, false);
        let ticks = AtomicUsize::new(0);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            progress.while_running(|| {
                ticks.fetch_add(1, Ordering::Relaxed);
                panic!("trabajo fallido");
            })
        }));
        assert!(outcome.is_err());
        assert_eq!(ticks.load(Ordering::Relaxed), 1);
    }
}
