// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Indicador de etapas de trabajo, por stderr.
//!
//! Cada etapa del catálogo ([`crate::stage`]) tiene su propio reloj. Una etapa
//! permanente deja al terminar la línea `✓ <Nombre>  <duración>`; una
//! transitoria no deja nada, salvo que el recorrido declarado la numere. Con
//! recorrido declarado, la línea lleva `[n/N]` y va bajo el título de su fase;
//! los resultados `│` y los avisos `⚠` van sangrados bajo su etapa. Con
//! terminal, la etapa en curso se repinta en una sola línea debajo de las
//! terminadas (`● <texto> <duración>`), recortada al ancho de la terminal; sin
//! terminal, una etapa permanente se anuncia una vez, deja un latido cada 10 s y
//! se cierra con su línea terminada, sin `\r` ni secuencias ANSI.
//! `AVI_NO_PROGRESS` (cualquier valor no vacío) silencia las líneas de etapa,
//! pero no los resultados `│` ni la línea de cierre de [`PhaseProgress::finish`].
//!
//! El modo máquina (`--json`) no cambia nada de lo que sale por stderr: solo
//! decide si los resultados salen por stdout.
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
use crate::report::{self, Layout, ReportGuard, Snapshot, StageOutcome, StageRecord};
use crate::stage::Stage;
use crate::style::{color_enabled, paint_if, Stream, Tone, ACTIVE, DOT, FAIL, OK, RESULT, SKIPPED};

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
/// Sangría de un resultado o un aviso bajo una etapa sin numerar.
const UNNUMBERED_INDENT: usize = 2;

/// Una etapa viva.
struct Current {
    /// Id de su registro en [`report`].
    id: u64,
    label: &'static str,
    active: String,
    /// Prefijo de su línea (`  [n/N] `), vacío si no se numera.
    prefix: String,
    /// Fase del recorrido declarado a la que pertenece, si la hay.
    phase: Option<&'static str>,
    /// Deja su línea terminada al cerrarse: el catálogo la da por permanente,
    /// el recorrido declarado la numera o `done_as` la hizo permanente.
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
    /// stderr es una terminal con capacidad de línea viva (`TERM=dumb` no).
    tty: bool,
    /// stdout es una terminal.
    stdout_tty: bool,
    enabled: bool,
    /// Modo máquina: los resultados no salen por stdout.
    machine: bool,
    /// Modo de etapa única: cerrar una etapa permanente no escribe su línea
    /// terminada.
    single_stage: bool,
    color: bool,
    repaint_every: Duration,
    beat_every: Duration,
    /// Ancho fijado a mano; `None` consulta la terminal en cada repintado.
    width: Option<usize>,
    inner: Mutex<Inner>,
}

impl PhaseProgress {
    /// Crea el indicador: captura el reloj de inicio (que fija el origen del
    /// reloj del registro si es el primero del proceso) y decide terminal,
    /// silenciado, modo máquina y color una sola vez, para que cada llamada
    /// sea barata. La línea viva y el color dependen de que stderr sea una
    /// terminal; `TERM=dumb` cuenta como sin terminal para la línea viva.
    pub fn new() -> Self {
        let start = Instant::now();
        report::lock().set_origin_if_none(start);
        let dumb = std::env::var("TERM").is_ok_and(|term| term == "dumb");
        Self {
            start,
            tty: std::io::stderr().is_terminal() && !dumb,
            stdout_tty: std::io::stdout().is_terminal(),
            enabled: !progress_silenced(),
            machine: line::machine_mode(),
            single_stage: false,
            color: color_enabled(Stream::Stderr),
            repaint_every: MIN_REPAINT,
            beat_every: BEAT_INTERVAL,
            width: None,
            inner: Mutex::new(Inner {
                current: None,
                overlapped: None,
                last_paint: None,
                closed: false,
            }),
        }
    }

    /// Indicador de pruebas con el valor de `AVI_NO_PROGRESS` y la terminal de
    /// stderr fijados a mano (la de stdout, sin terminal), para no depender del
    /// entorno del proceso. Sin límite de repintado, sin color y con 80
    /// columnas.
    #[cfg(test)]
    pub(crate) fn for_test(no_progress_value: Option<&str>, tty: bool) -> Self {
        let mut progress = Self::new();
        progress.tty = tty;
        progress.stdout_tty = false;
        progress.enabled = !silenced_by(no_progress_value);
        progress.machine = false;
        progress.color = false;
        progress.repaint_every = Duration::ZERO;
        progress.width = Some(80);
        progress
    }

    /// Activa el modo de etapa única, para un comando cuya única etapa de
    /// trabajo repetiría lo que dice el cierre: al cerrarse, una etapa
    /// permanente no escribe su línea `✓`, pero sí queda en el registro con su
    /// duración. Sin terminal se conservan el anuncio `● <texto>` y los
    /// latidos. Actúa igual con y sin `--json`.
    pub fn single_stage(mut self) -> Self {
        self.single_stage = true;
        self
    }

    /// Texto que se ha escrito hasta ahora por stderr (solo pruebas).
    #[cfg(test)]
    fn captured(&self) -> String {
        line::test_capture::err()
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

    /// Escribe `text` en stderr de una sola vez. Quien llama tiene tomada la
    /// guarda del registro.
    fn raw(&self, text: &str) {
        line::write_err(text);
    }

    /// Ancho de la terminal en columnas.
    fn columns(&self) -> usize {
        self.width.unwrap_or_else(line::terminal_width)
    }

    /// Borra la línea de fase de la pantalla si estaba abierta.
    fn clear_line(&self) {
        if line::release_phase_line() {
            self.raw(CLEAR_LINE);
        }
    }

    /// Pinta la línea activa de la etapa en curso sobre la misma línea y
    /// publica que queda abierta. Se recorta al ancho de la terminal y sale en
    /// una sola escritura.
    fn paint(&self, inner: &mut Inner) {
        let Some(current) = &inner.current else {
            return;
        };
        let nest = line::nested_prefix();
        let columns = self.columns().saturating_sub(nest.chars().count());
        let text = match &current.live {
            Some(live) => clip_plain(&format!("{}{live}", current.prefix), columns),
            None => fit_live(
                self.color,
                &format!("{}{ACTIVE} {}", current.prefix, current.active),
                &fmt::duration(current.started.elapsed()),
                columns,
            ),
        };
        self.raw(&format!("{CLEAR_LINE}{nest}{text}"));
        line::set_phase_line_open(true);
        inner.last_paint = Some(Instant::now());
    }

    /// ¿Ya pasó el intervalo mínimo desde el último repintado?
    fn repaint_due(&self, inner: &Inner) -> bool {
        inner
            .last_paint
            .is_none_or(|last| last.elapsed() >= self.repaint_every)
    }

    /// Línea terminada de la etapa `id` según el registro, sin salto final.
    fn done_line(&self, rep: &ReportGuard, id: u64) -> Option<String> {
        let record = rep.stage(id)?;
        stage_line(self.color, &rep.layout(), rep.unnumbered_width(), record)
    }

    /// Cierra una etapa en el instante `at`: una permanente deja su línea
    /// terminada (salvo en modo de etapa única), una transitoria se borra.
    fn close_one(&self, current: Current, rep: &mut ReportGuard, at: Instant) {
        rep.close_stage(current.id, at, None);
        if !self.enabled {
            return;
        }
        if self.tty {
            self.clear_line();
        }
        if self.single_stage || !current.permanent {
            return;
        }
        if let Some(line) = self.done_line(rep, current.id) {
            self.raw(&format!("{line}\n"));
        }
    }

    /// Cierra una etapa como fallida en `at` y pinta `✗ <Nombre> <duración>`.
    fn fail_one(&self, current: Current, rep: &mut ReportGuard, at: Instant) {
        rep.close_stage(current.id, at, Some(StageOutcome::Failed));
        if !self.enabled {
            return;
        }
        if self.tty {
            self.clear_line();
        }
        if let Some(line) = self.done_line(rep, current.id) {
            self.raw(&format!("{line}\n"));
        }
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
        let declared = rep.is_declared(stage.label);
        let (phase, number) = rep
            .stage(id)
            .map_or((None, None), |record| (record.phase, record.number));
        Current {
            id,
            label: stage.label,
            active: active.to_string(),
            prefix: stage_prefix(&rep.layout(), number),
            phase,
            permanent: !stage.transient || declared,
            declared,
            name_overridden: false,
            started,
            note: None,
            live: None,
            last_beat: started,
        }
    }

    /// Escribe el título de la fase de la etapa recién abierta si es la primera
    /// de su fase y el recorrido tiene al menos 2 fases.
    fn announce_phase_title(&self, rep: &mut ReportGuard, current: &Current) {
        if !self.enabled || !current.permanent {
            return;
        }
        if let Some(title) = current.phase.and_then(|phase| rep.take_phase_title(phase)) {
            self.raw(&format!("{title}\n"));
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
                    current.permanent = !stage.transient || current.declared;
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
        self.announce_phase_title(&mut rep, &current);
        let announce = (!self.tty && current.permanent).then(|| {
            format!(
                "{}{}{ACTIVE} {active}\n",
                line::nested_prefix(),
                current.prefix
            )
        });
        inner.current = Some(current);
        if !self.enabled {
            return;
        }
        if self.tty {
            self.paint(&mut inner);
        } else if let Some(text) = announce {
            self.raw(&text);
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
        self.announce_phase_title(&mut rep, &opened);
        if self.enabled && !self.tty && opened.permanent {
            self.raw(&format!(
                "{}{}{ACTIVE} {}\n",
                line::nested_prefix(),
                opened.prefix,
                stage.active
            ));
        }
        if inner.current.is_none() {
            inner.current = Some(opened);
        } else {
            inner.overlapped = Some(opened);
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
            if self.enabled && self.tty && inner.current.is_some() {
                self.paint(&mut inner);
            }
        } else if inner
            .overlapped
            .as_ref()
            .is_some_and(|c| c.label == stage.label)
        {
            if let Some(overlapped) = inner.overlapped.take() {
                self.close_one(overlapped, &mut rep, at);
            }
            if self.enabled && self.tty && inner.current.is_some() {
                self.paint(&mut inner);
            }
        }
    }

    /// Registra una etapa omitida durante la ejecución, con `start == end` en
    /// el instante en que se marca, y su motivo, y escribe su línea
    /// `○ <Nombre> · <motivo>` (con el título de su fase si le toca). No abre ni
    /// cierra ninguna etapa viva.
    pub fn skip(&self, stage: &'static Stage, reason: &str) {
        self.skip_named(stage, None, reason);
    }

    /// Como [`PhaseProgress::skip`], con `name` en la columna de la etapa en
    /// lugar del nombre del catálogo: sirve a las etapas que se nombran por su
    /// objeto, como un archivo.
    pub fn skip_as(&self, stage: &'static Stage, name: &str, reason: &str) {
        self.skip_named(stage, Some(name), reason);
    }

    fn skip_named(&self, stage: &'static Stage, name: Option<&str>, reason: &str) {
        let (mut inner, mut rep) = self.lock_both();
        let at = Instant::now();
        let id = rep.skip_stage(stage, reason, at);
        if let Some(name) = name {
            rep.rename_stage(id, name, true);
        }
        if !self.enabled {
            return;
        }
        let Some(record) = rep.last_shown_stage().cloned() else {
            return;
        };
        let mut text = String::new();
        if let Some(title) = record.phase.and_then(|phase| rep.take_phase_title(phase)) {
            text.push_str(title);
            text.push('\n');
        }
        if let Some(line) = stage_line(self.color, &rep.layout(), rep.unnumbered_width(), &record) {
            text.push_str(&line);
            text.push('\n');
        }
        if self.tty {
            self.clear_line();
        }
        self.raw(&text);
        if self.tty && inner.current.is_some() {
            self.paint(&mut inner);
        }
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

    /// Como [`PhaseProgress::note`], sobre la etapa viva `stage` sea la
    /// principal o la segunda abierta con [`PhaseProgress::overlap`]. Sin ninguna
    /// etapa viva con ese identificador no hace nada.
    pub fn note_stage(&self, stage: &'static Stage, text: &str) {
        let (mut inner, mut rep) = self.lock_both();
        let inner = &mut *inner;
        let target = [inner.current.as_mut(), inner.overlapped.as_mut()]
            .into_iter()
            .flatten()
            .find(|current| current.label == stage.label);
        if let Some(current) = target {
            current.note = Some(text.to_string());
            rep.set_note(current.id, text);
        }
    }

    /// Cambia el texto activo de la etapa viva `stage` (la principal o la
    /// segunda) sin abrir ni cerrar ninguna etapa, a diferencia de
    /// [`PhaseProgress::phase_with`]. Sin ninguna etapa viva con ese
    /// identificador no hace nada. Repinta si la etapa es la principal.
    pub fn live_text(&self, stage: &'static Stage, text: &str) {
        let (mut inner, _rep) = self.lock_both();
        if let Some(overlapped) = inner
            .overlapped
            .as_mut()
            .filter(|current| current.label == stage.label)
        {
            overlapped.active = text.to_string();
            return;
        }
        let Some(current) = inner
            .current
            .as_mut()
            .filter(|current| current.label == stage.label)
        else {
            return;
        };
        current.active = text.to_string();
        if self.enabled && self.tty {
            self.paint(&mut inner);
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
            let lead = if current.prefix.is_empty() {
                "  "
            } else {
                &current.prefix
            };
            let text = format!(
                "{}{lead}{ACTIVE} {} {}\n",
                line::nested_prefix(),
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
    /// leyó un Enter de un stdin que es terminal. Actúa igual con y sin
    /// `--json`. Sin terminal en stderr, silenciado o sin línea viva abierta no
    /// escribe nada.
    pub fn climb_echoed_enter(&self) {
        if !self.enabled || !self.tty || !line::phase_line_open() {
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
    /// escribe el resultado. El resultado queda ligado en el registro a la etapa
    /// que acaba de cerrarse y sale sangrado bajo ella (8 espacios bajo una
    /// etapa numerada, 2 bajo una sin numerar).
    ///
    /// - Si no es modo máquina, va a stdout: con terminal, `<sangría>│ <texto>`;
    ///   sin ella, `<Etiqueta>: <texto>`.
    /// - Si stdout no es una terminal o es modo máquina, además va a stderr:
    ///   con terminal, `<sangría>│ <texto>`; sin ella,
    ///   `<sangría>│ <Etiqueta>: <texto>`.
    ///
    /// `AVI_NO_PROGRESS` no silencia estas líneas.
    pub fn result(&self, label: Option<&str>, text: &str) {
        let (mut inner, mut rep) = self.lock_both();
        self.close_all(&mut inner, &mut rep);
        rep.add_result(label, text);
        let layout = rep.layout();
        let number = rep.last_shown_stage().and_then(|record| record.number);
        let indent = body_indent(&layout, number);
        if !self.machine {
            line::write_out_line(&result_line(
                self.stdout_tty,
                color_enabled(Stream::Stdout),
                indent,
                label,
                text,
            ));
        }
        if self.machine || !self.stdout_tty {
            let pad = " ".repeat(indent);
            let body = match (self.tty, label) {
                (true, _) | (false, None) => paint_if(self.color && self.tty, Tone::Strong, text),
                (false, Some(label)) => format!("{label}: {text}"),
            };
            self.raw(&format!("{pad}{RESULT} {body}\n"));
        }
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

    /// Cierra la etapa en curso y devuelve el tiempo total del comando según el
    /// registro (sin las pausas por preguntas), sin línea de cierre.
    pub fn close(self) -> Duration {
        let (mut inner, mut rep) = self.lock_both();
        self.close_all(&mut inner, &mut rep);
        inner.closed = true;
        rep.snapshot().total()
    }

    /// Como [`PhaseProgress::close`], y escribe por stderr, en una sola
    /// escritura:
    /// 1. el separador, si hay al menos 2 etapas numeradas o se pintó un `✗`;
    /// 2. la línea de cierre `✓ <acción> en <duración total>` seguida de
    ///    ` · <extra>` por cada extra;
    /// 3. el desglose por fase, si el recorrido tiene al menos 2 fases.
    ///
    /// El total es el del registro, el mismo que lleva el JSON.
    pub fn finish(self, action: &str, extras: &[String]) -> Duration {
        let (mut inner, mut rep) = self.lock_both();
        self.close_all(&mut inner, &mut rep);
        inner.closed = true;
        let snapshot = rep.snapshot();
        let elapsed = snapshot.total();
        let mut text = String::new();
        if separator_due(&snapshot) {
            text.push_str(&line::rule_text());
            text.push('\n');
        }
        text.push_str(&closing_line(self.color, action, elapsed, extras));
        text.push('\n');
        if let Some(breakdown) = phase_breakdown(&snapshot) {
            text.push_str(&format!("  {breakdown}\n"));
        }
        self.raw(&text);
        elapsed
    }

    /// Termina sin cierre: si queda una etapa abierta, permanente o declarada,
    /// la registra como fallida y pinta `✗ <Nombre> <duración>`; si no, solo
    /// borra la línea activa.
    pub fn fail(self) {
        // El `Drop` hace el trabajo: un indicador soltado sin `close`,
        // `finish` ni `fail` equivale a `fail`.
    }

    /// Termina por una interrupción: deja las etapas vivas marcadas como
    /// interrumpidas en el registro y borra la línea activa, sin pintar nada
    /// más. Quien interrumpe pinta después, con
    /// [`line::special_closing`] (`Interrupted`), el `✗ <Nombre> <duración> ·
    /// interrumpido`, el separador y el cierre.
    pub fn interrupt(self) {
        let (mut inner, mut rep) = self.lock_both();
        inner.current = None;
        inner.overlapped = None;
        inner.closed = true;
        rep.interrupt_open_stages();
        if self.enabled && self.tty {
            self.clear_line();
        }
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
            if current.permanent {
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
/// vacío)? Lo consultan quienes escriben por stderr fuera del indicador.
pub fn progress_silenced() -> bool {
    silenced_by(std::env::var("AVI_NO_PROGRESS").ok().as_deref())
}

/// Prefijo de la línea de una etapa: `  [n/N] ` si el recorrido numera al menos 2
/// etapas y esta tiene número; vacío si no.
pub(crate) fn stage_prefix(layout: &Layout, number: Option<usize>) -> String {
    match number {
        Some(n) if layout.numbered() => format!("  [{n}/{}] ", layout.total),
        _ => String::new(),
    }
}

/// Sangría de los resultados y avisos bajo una etapa: la del prefijo si es
/// numerada (8 espacios), 2 si no.
pub(crate) fn body_indent(layout: &Layout, number: Option<usize>) -> usize {
    let prefix = stage_prefix(layout, number);
    if prefix.is_empty() {
        UNNUMBERED_INDENT
    } else {
        prefix.chars().count()
    }
}

/// Línea de una etapa según su desenlace, sin salto final, o `None` si no deja
/// línea (una transitoria no declarada).
///
/// - Terminada: `✓ <Nombre> <duración> · <dato>`.
/// - Fallida: `✗ <Nombre> <duración>`.
/// - Omitida: `○ <Nombre> · <motivo>`, sin duración.
/// - Interrumpida: `✗ <Nombre> <duración> · interrumpido`.
///
/// Una etapa numerada lleva delante `  [n/N] ` y alinea el nombre al más largo
/// del recorrido; una sin numerar alinea al ancho `unnumbered_width`.
pub(crate) fn stage_line(
    color: bool,
    layout: &Layout,
    unnumbered_width: usize,
    record: &StageRecord,
) -> Option<String> {
    let own_prefix = stage_prefix(layout, record.number);
    let name_chars = record.name.chars().count();
    let width = if own_prefix.is_empty() {
        unnumbered_width.max(name_chars)
    } else {
        layout.name_width.max(name_chars)
    };
    let prefix = format!("{}{own_prefix}", line::nested_prefix());
    let name = &record.name;
    let elapsed = record
        .end
        .unwrap_or_else(Instant::now)
        .saturating_duration_since(record.start);
    let dim_duration = paint_if(color, Tone::Dim, &fmt::duration(elapsed));
    match &record.outcome {
        StageOutcome::Transient => None,
        StageOutcome::Ok => {
            let mut line = format!(
                "{prefix}{} {name:<width$}  {dim_duration}",
                paint_if(color, Tone::Ok, OK)
            );
            if let Some(note) = &record.note {
                line.push_str(&format!(" {DOT} {note}"));
            }
            Some(line)
        }
        StageOutcome::Failed => Some(format!(
            "{prefix}{} {name:<width$}  {dim_duration}",
            paint_if(color, Tone::Error, FAIL)
        )),
        StageOutcome::Skipped(reason) => Some(format!(
            "{prefix}{} {name} {DOT} {reason}",
            paint_if(color, Tone::Dim, SKIPPED)
        )),
        StageOutcome::Interrupted => Some(format!(
            "{prefix}{} {name:<width$}  {dim_duration} {DOT} interrumpido",
            paint_if(color, Tone::Error, FAIL)
        )),
    }
}

/// Recorta `text` a `columns - 1` caracteres (la última columna se deja libre
/// para que la terminal no salte de línea), con `…` al final si recortó. Un texto
/// con secuencias ANSI no se recorta: el corte rompería la secuencia.
fn clip_plain(text: &str, columns: usize) -> String {
    let limit = columns.saturating_sub(1).max(1);
    if text.contains('\x1b') || text.chars().count() <= limit {
        return text.to_string();
    }
    let mut clipped: String = text.chars().take(limit - 1).collect();
    clipped.push('…');
    clipped
}

/// Línea viva estándar `<cabeza> <duración>` recortada al ancho: lo que sobra se
/// quita de la cabeza, con `…`, y la duración se conserva.
fn fit_live(color: bool, head: &str, duration: &str, columns: usize) -> String {
    let limit = columns.saturating_sub(1).max(1);
    let tail_chars = duration.chars().count() + 1;
    let head_chars = head.chars().count();
    if head_chars + tail_chars <= limit {
        return format!("{head} {}", paint_if(color, Tone::Dim, duration));
    }
    if limit > tail_chars + 1 {
        let kept: String = head.chars().take(limit - tail_chars - 1).collect();
        return format!("{kept}… {}", paint_if(color, Tone::Dim, duration));
    }
    clip_plain(&format!("{head} {duration}"), columns)
}

/// ¿Hay que pintar el separador antes del cierre? Con al menos 2 etapas
/// numeradas o si alguna etapa falló o se interrumpió.
fn separator_due(snapshot: &Snapshot) -> bool {
    let numbered = snapshot
        .stages
        .iter()
        .filter(|record| record.number.is_some() && record.outcome != StageOutcome::Transient)
        .count();
    numbered >= 2
        || snapshot.stages.iter().any(|record| {
            matches!(
                record.outcome,
                StageOutcome::Failed | StageOutcome::Interrupted
            )
        })
}

/// Duración de cada fase del recorrido, en orden, que suma exactamente el total.
///
/// Una fase va desde el inicio de su primera etapa hasta el inicio de la
/// siguiente, o hasta el total en la última. La primera arranca en el origen del
/// reloj, así que el tiempo previo a ella (las etapas transitorias) cuenta en
/// ella. Una fase sin ninguna etapa dura 0 y la anterior llega hasta la
/// siguiente fase que sí empezó.
fn phase_durations(snapshot: &Snapshot) -> Vec<(&'static str, Duration)> {
    let Some(pipeline) = &snapshot.pipeline else {
        return Vec::new();
    };
    let total = snapshot.total();
    let mut bounds = vec![total; pipeline.phases.len() + 1];
    for index in (0..pipeline.phases.len()).rev() {
        let start = if index == 0 {
            Duration::ZERO
        } else {
            let id = pipeline.phases[index].id;
            snapshot
                .stages
                .iter()
                .filter(|record| record.phase == Some(id))
                .map(|record| snapshot.relative(record.start))
                .min()
                .unwrap_or(bounds[index + 1])
        };
        bounds[index] = start.min(bounds[index + 1]);
    }
    pipeline
        .phases
        .iter()
        .enumerate()
        .map(|(index, phase)| (phase.short, bounds[index + 1] - bounds[index]))
        .collect()
}

/// Desglose por fase (`Entrada 2,9 s · Texto 0,3 s · Voz 2,9 s`), solo si el
/// recorrido tiene al menos 2 fases.
fn phase_breakdown(snapshot: &Snapshot) -> Option<String> {
    let durations = phase_durations(snapshot);
    if durations.len() < 2 {
        return None;
    }
    Some(
        durations
            .iter()
            .map(|(short, elapsed)| format!("{short} {}", fmt::duration(*elapsed)))
            .collect::<Vec<_>>()
            .join(&format!(" {DOT} ")),
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
        line.push_str(&format!(" {DOT} {extra}"));
    }
    line
}

/// Línea de un resultado por stdout. Con terminal, `<sangría>│ <texto>` con el
/// texto en negrita y sin etiqueta; sin ella, `<Etiqueta>: <texto>` para que un
/// script lo lea.
pub fn result_line(
    stdout_tty: bool,
    color: bool,
    indent: usize,
    label: Option<&str>,
    text: &str,
) -> String {
    if stdout_tty {
        format!(
            "{}{RESULT} {}",
            " ".repeat(indent),
            paint_if(color, Tone::Strong, text)
        )
    } else {
        match label {
            Some(label) => format!("{label}: {text}"),
            None => text.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::line::test_capture::{err, normalize, out};
    use crate::report::{PhaseSpec, Pipeline};
    use crate::stage;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn lines_of(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn voice_pipeline() -> Pipeline {
        Pipeline::new(vec![
            PhaseSpec {
                id: "input",
                title: "Entrada",
                short: "Entrada",
                stages: vec![&stage::CAPTURING],
            },
            PhaseSpec {
                id: "text",
                title: "Texto",
                short: "Texto",
                stages: vec![&stage::TRANSCRIBING, &stage::TRANSLATING],
            },
            PhaseSpec {
                id: "voice",
                title: "Voz · streaming",
                short: "Voz",
                stages: vec![&stage::SYNTHESIZING, &stage::PLAYING],
            },
        ])
    }

    fn record(name: &str, number: Option<usize>, outcome: StageOutcome) -> StageRecord {
        let start = Instant::now();
        StageRecord {
            id: 0,
            label: "x",
            name: name.to_string(),
            phase: None,
            number,
            start,
            end: Some(start + Duration::from_millis(1400)),
            outcome,
            note: None,
        }
    }

    /// Ejecuta un recorrido completo de tres fases y devuelve lo que salió por
    /// stderr y por stdout, con los tiempos normalizados.
    fn run_dub(machine: bool, tty: bool, stdout_tty: bool) -> (String, String) {
        let _g = report::test_guard();
        line::set_machine_mode(machine);
        report::declare(voice_pipeline());
        line::context("Doblaje es → en · voz default");
        let mut progress = PhaseProgress::for_test(None, tty);
        progress.machine = machine;
        progress.stdout_tty = stdout_tty;
        progress.phase(&stage::VALIDATING);
        progress.phase(&stage::CAPTURING);
        progress.end_stage();
        progress.phase(&stage::TRANSCRIBING);
        progress.result(Some("Transcripción"), "Hola, mi nombre es Cristian.");
        progress.phase(&stage::TRANSLATING);
        line::warn("traducción con baja confianza");
        progress.result(Some("Traducción"), "Hello, my name is Cristian.");
        progress.phase(&stage::SYNTHESIZING);
        progress.note("primer audio 0,6 s");
        progress.overlap(&stage::PLAYING, Instant::now());
        progress.end_stage_at(&stage::SYNTHESIZING, Instant::now());
        progress.finish("Doblaje completado", &["primer audio 0,6 s".to_string()]);
        (normalize(&err()), normalize(&out()))
    }

    #[test]
    fn stage_line_formats_every_outcome() {
        // La guarda serializa con las pruebas que activan el modo anidado, que es
        // estado global y antepondría su sangría a las líneas de esta prueba.
        let _g = report::test_guard();
        let layout = Layout {
            total: 5,
            phases: 3,
            name_width: 13,
        };
        let numbered =
            |outcome| stage_line(false, &layout, 12, &record("Síntesis", Some(4), outcome));
        assert_eq!(
            numbered(StageOutcome::Ok).as_deref(),
            Some("  [4/5] ✓ Síntesis       1,4 s")
        );
        assert_eq!(
            numbered(StageOutcome::Failed).as_deref(),
            Some("  [4/5] ✗ Síntesis       1,4 s")
        );
        assert_eq!(
            numbered(StageOutcome::Interrupted).as_deref(),
            Some("  [4/5] ✗ Síntesis       1,4 s · interrumpido")
        );
        assert_eq!(
            numbered(StageOutcome::Skipped("ya existía".into())).as_deref(),
            Some("  [4/5] ○ Síntesis · ya existía")
        );
        assert_eq!(numbered(StageOutcome::Transient), None);
        let mut with_note = record("Lectura", None, StageOutcome::Ok);
        with_note.note = Some("52,0 MB".into());
        assert_eq!(
            stage_line(false, &layout, 12, &with_note).as_deref(),
            Some("✓ Lectura       1,4 s · 52,0 MB")
        );
        assert_eq!(
            stage_line(true, &layout, 12, &record("Ok", None, StageOutcome::Ok)).as_deref(),
            Some("\x1b[32m✓\x1b[0m Ok            \x1b[2m1,4 s\x1b[0m")
        );
        let single = Layout {
            total: 1,
            phases: 1,
            name_width: 8,
        };
        assert_eq!(
            stage_line(
                false,
                &single,
                12,
                &record("Síntesis", Some(1), StageOutcome::Ok)
            )
            .as_deref(),
            Some("✓ Síntesis      1,4 s"),
            "con una sola etapa numerada no hay [n/N]"
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
        assert_eq!(result_line(false, false, 8, None, "hola"), "hola");
        assert_eq!(
            result_line(false, false, 8, Some("Texto"), "hola"),
            "Texto: hola"
        );
        assert_eq!(result_line(true, false, 8, None, "hola"), "        │ hola");
        assert_eq!(
            result_line(true, false, 2, Some("Texto"), "hola"),
            "  │ hola",
            "con terminal no hay etiqueta"
        );
        assert_eq!(
            result_line(true, true, 2, None, "hola"),
            "  │ \x1b[1mhola\x1b[0m"
        );
    }

    /// Estructura completa de tres fases sin terminal: cabecera, mapa, títulos,
    /// `[n/N]`, `│` con etiqueta por stderr, `⚠` bajo su etapa, separador,
    /// cierre y desglose.
    #[test]
    fn three_phases_render_the_full_structure() {
        let (stderr, stdout) = run_dub(false, false, false);
        let expected = [
            "Doblaje es → en · voz default",
            "Entrada → Texto → Voz",
            "",
            "Entrada",
            "  [1/5] ● Grabando…",
            "  [1/5] ✓ Grabación      <t>",
            "Texto",
            "  [2/5] ● Transcribiendo…",
            "  [2/5] ✓ Transcripción  <t>",
            "        │ Transcripción: Hola, mi nombre es Cristian.",
            "  [3/5] ● Traduciendo…",
            "        ⚠ traducción con baja confianza.",
            "  [3/5] ✓ Traducción     <t>",
            "        │ Traducción: Hello, my name is Cristian.",
            "Voz · streaming",
            "  [4/5] ● Sintetizando…",
            "  [5/5] ● Reproduciendo…",
            "  [4/5] ✓ Síntesis       <t> · primer audio <t>",
            "  [5/5] ✓ Reproducción   <t>",
            "──────────────────────────────────────────────────",
            "✓ Doblaje completado en <t> · primer audio <t>",
            "  Entrada <t> · Texto <t> · Voz <t>",
        ];
        assert_eq!(lines_of(&stderr), expected, "{stderr}");
        assert_eq!(
            lines_of(&stdout),
            [
                "Transcripción: Hola, mi nombre es Cristian.",
                "Traducción: Hello, my name is Cristian."
            ]
        );
    }

    /// El stderr es idéntico con y sin modo máquina, en los dos modos de
    /// terminal.
    #[test]
    fn machine_mode_leaves_stderr_untouched() {
        for tty in [false, true] {
            for stdout_tty in [false, true] {
                let normal = run_dub(false, tty, stdout_tty).0;
                let (machine, machine_out) = run_dub(true, tty, stdout_tty);
                // En modo máquina stdout lleva el JSON, no los resultados: la
                // copia por stderr es la de «stdout sin terminal», así que solo
                // coincide con el modo normal cuando stdout tampoco es terminal.
                assert_eq!(machine_out, "", "tty={tty}");
                if !stdout_tty {
                    assert_eq!(normal, machine, "tty={tty}");
                } else {
                    assert!(machine.contains("        │ "), "{machine}");
                    // Salvo las líneas `│`, el stderr es el mismo.
                    let without_results = |text: &str| {
                        text.lines()
                            .filter(|line| !line.contains('│'))
                            .collect::<Vec<_>>()
                            .join("\n")
                    };
                    assert_eq!(without_results(&normal), without_results(&machine));
                }
            }
        }
    }

    /// `result` en sus cuatro combinaciones de terminal.
    #[test]
    fn result_goes_to_the_channels_each_combination_asks_for() {
        for (tty, stdout_tty) in [(false, false), (true, false), (false, true), (true, true)] {
            let (stderr, stdout) = run_dub(false, tty, stdout_tty);
            let in_stderr = stderr.contains("        │ ");
            let labelled = stderr.contains("        │ Transcripción: ");
            match (tty, stdout_tty) {
                (false, false) => {
                    assert!(labelled, "{stderr}");
                    assert!(stdout.contains("Transcripción: Hola"), "{stdout}");
                }
                (true, false) => {
                    assert!(in_stderr && !labelled, "{stderr}");
                    assert!(stdout.contains("Transcripción: Hola"), "{stdout}");
                }
                (false, true) | (true, true) => {
                    assert!(!in_stderr, "{stderr}");
                    assert!(stdout.contains("        │ Hola, mi nombre"), "{stdout}");
                }
            }
        }
    }

    /// Con dos fases y una sola etapa declarada por fase se numera `[1/2]` y
    /// `[2/2]`; con una sola fase no hay título ni mapa ni desglose.
    #[test]
    fn a_single_phase_has_numbers_but_no_titles_map_or_breakdown() {
        let _g = report::test_guard();
        report::declare(Pipeline::new(vec![PhaseSpec {
            id: "voice",
            title: "Voz · streaming",
            short: "Voz",
            stages: vec![&stage::SYNTHESIZING, &stage::PLAYING],
        }]));
        line::context("Lectura en voz alta es · voz default");
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::SYNTHESIZING);
        progress.phase(&stage::PLAYING);
        progress.finish("Lectura completada", &[]);
        assert_eq!(
            lines_of(&normalize(&err())),
            [
                "Lectura en voz alta es · voz default",
                "  [1/2] ● Sintetizando…",
                "  [1/2] ✓ Síntesis      <t>",
                "  [2/2] ● Reproduciendo…",
                "  [2/2] ✓ Reproducción  <t>",
                "──────────────────────────────────────────────────",
                "✓ Lectura completada en <t>",
            ]
        );
    }

    /// Sin recorrido declarado las etapas no se numeran y alinean a 12 como
    /// mínimo.
    #[test]
    fn without_a_pipeline_stages_are_unnumbered_and_aligned() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::READING);
        progress.phase(&stage::TRANSCRIBING);
        progress.finish("Listo", &[]);
        assert_eq!(
            lines_of(&normalize(&err())),
            [
                "● Leyendo el audio…",
                "✓ Lectura       <t>",
                "● Transcribiendo…",
                "✓ Transcripción  <t>",
                "✓ Listo en <t>"
            ]
        );
    }

    /// Sin terminal, una etapa permanente se anuncia una vez y deja su línea
    /// terminada; una transitoria no escribe nada.
    #[test]
    fn permanent_stage_without_tty_announces_once_and_leaves_a_done_line() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::VALIDATING);
        progress.phase(&stage::READING);
        progress.tick();
        progress.phase(&stage::READING);
        assert_eq!(lines_of(&progress.captured()), ["● Leyendo el audio…"]);
        progress.phase(&stage::TRANSCRIBING);
        let text = progress.captured();
        let all = lines_of(&text);
        assert_eq!(all.len(), 3, "{all:?}");
        assert!(all[1].starts_with("✓ Lectura"), "{all:?}");
        assert_eq!(all[2], "● Transcribiendo…");
        assert!(!text.contains('\r') && !text.contains('\x1b'));
    }

    /// `○` con su motivo, con el título de su fase si le toca, y `✗` al fallar.
    #[test]
    fn skipped_and_failed_stages_leave_their_symbols() {
        let _g = report::test_guard();
        report::declare(Pipeline::new(vec![
            PhaseSpec {
                id: "text",
                title: "Texto",
                short: "Texto",
                stages: vec![&stage::TRANSLATING],
            },
            PhaseSpec {
                id: "save",
                title: "Guardado",
                short: "Guardado",
                stages: vec![&stage::SAVING],
            },
        ]));
        let progress = PhaseProgress::for_test(None, false);
        progress.skip(&stage::TRANSLATING, "ya existía");
        progress.phase(&stage::SAVING);
        progress.fail_current();
        assert_eq!(
            lines_of(&normalize(&err())),
            [
                "Texto",
                "  [1/2] ○ Traducción · ya existía",
                "Guardado",
                "  [2/2] ● Guardando…",
                "  [2/2] ✗ Guardado    <t>",
            ]
        );
        let snap = report::snapshot();
        assert_eq!(
            snap.stages[0].outcome,
            StageOutcome::Skipped("ya existía".into())
        );
        assert_eq!(snap.stages[1].outcome, StageOutcome::Failed);
    }

    /// En modo anidado, las etapas, los avisos y el anuncio sin terminal llevan
    /// delante 8 espacios, y la línea viva con terminal también.
    #[test]
    fn nested_mode_indents_stages_warnings_and_the_live_line() {
        let _g = report::test_guard();
        line::set_nested(true);
        let indent = " ".repeat(8);

        // Sin ninguna etapa mostrada, el aviso sale como `Aviso:` con la sangría.
        line::warn("antes de las etapas");
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::CAPTURING);
        progress.end_stage();
        progress.phase(&stage::TRANSCRIBING);
        progress.fail_current();
        line::warn("bajo la etapa");
        let text = normalize(&err());
        let lines = lines_of(&text);
        assert_eq!(lines[0], format!("{indent}Aviso: antes de las etapas."));
        assert_eq!(lines[1], format!("{indent}● Grabando…"));
        assert!(
            lines[2].starts_with(&format!("{indent}✓ Grabación")),
            "{lines:?}"
        );
        assert_eq!(lines[3], format!("{indent}● Transcribiendo…"));
        assert!(
            lines[4].starts_with(&format!("{indent}✗ Transcripción")),
            "{lines:?}"
        );
        assert_eq!(lines[5], format!("{indent}  ⚠ bajo la etapa."));

        // Con terminal, la línea viva empieza por la sangría y descuenta su ancho.
        report::lock().reset();
        line::reset_for_tests();
        line::set_nested(true);
        let mut live = PhaseProgress::for_test(None, true);
        live.phase(&stage::CAPTURING);
        live.live("● Grabando 0:05 / 0:40".to_string());
        let painted = live.captured();
        assert!(
            painted.ends_with(&format!("\r\x1b[2K{indent}● Grabando 0:05 / 0:40")),
            "{painted:?}"
        );
        live.width = Some(30);
        live.live("● una línea viva muy larga que no cabe en el ancho".to_string());
        let clipped = live.captured();
        let last = clipped.rsplit("\r\x1b[2K").next().unwrap_or_default();
        assert!(last.starts_with(&indent), "{last:?}");
        assert!(last.chars().count() <= 29, "{last:?}");
        let _ = live.close();
    }

    /// El desglose por fase suma exactamente el total y las transitorias
    /// previas cuentan en la primera.
    #[test]
    fn the_breakdown_adds_up_to_the_total() {
        let _g = report::test_guard();
        report::declare(voice_pipeline());
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::VALIDATING);
        std::thread::sleep(Duration::from_millis(20));
        progress.phase(&stage::CAPTURING);
        std::thread::sleep(Duration::from_millis(20));
        progress.phase(&stage::TRANSCRIBING);
        std::thread::sleep(Duration::from_millis(20));
        progress.phase(&stage::SYNTHESIZING);
        std::thread::sleep(Duration::from_millis(20));
        progress.end_stage();
        let snap = report::snapshot();
        let parts = phase_durations(&snap);
        assert_eq!(parts.len(), 3);
        assert_eq!(
            parts.iter().map(|(_, d)| *d).sum::<Duration>(),
            snap.total()
        );
        assert!(parts[0].1 >= Duration::from_millis(40), "{parts:?}");
        let _ = progress.close();
    }

    #[test]
    fn discard_stage_leaves_no_done_line() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::DENOISING);
        progress.discard_stage();
        progress.phase(&stage::DENOISING);
        progress.end_stage();
        let all = lines_of(&progress.captured());
        assert_eq!(all.len(), 3, "{all:?}");
        assert_eq!(all[0], "● Limpiando el ruido…");
        assert_eq!(all[1], "● Limpiando el ruido…");
        assert!(all[2].starts_with("✓ Limpieza"), "{all:?}");
        assert_eq!(report::snapshot().stages.len(), 1);
    }

    #[test]
    fn end_stage_closes_the_current_stage_once() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::PLAYING);
        progress.end_stage();
        progress.end_stage();
        let before = progress.captured();
        assert_eq!(lines_of(&before).len(), 2, "{before}");
        let _ = progress.close();
        assert_eq!(err(), before, "cerrar después no repite la línea terminada");
    }

    /// En modo de etapa única, cerrar una etapa permanente no escribe su línea
    /// `✓`, con o sin terminal y con o sin modo máquina; sin terminal queda el
    /// anuncio. La etapa queda en el registro.
    #[test]
    fn single_stage_closes_without_its_line_in_every_mode() {
        let _g = report::test_guard();
        for tty in [false, true] {
            for machine in [false, true] {
                for use_end_stage in [false, true] {
                    let mut progress = PhaseProgress::for_test(None, tty).single_stage();
                    progress.machine = machine;
                    progress.phase(&stage::TRANSLATING);
                    if use_end_stage {
                        progress.end_stage();
                    }
                    let _ = progress.close();
                    let text = err();
                    assert!(!text.contains('✓'), "tty={tty} machine={machine}: {text}");
                    if !tty {
                        assert_eq!(text, "● Traduciendo…\n");
                    }
                    assert!(report::snapshot().stages[0].end.is_some());
                    report::lock().reset();
                    line::reset_for_tests();
                }
            }
        }
    }

    /// `done_as` convierte una transitoria en permanente y cambia el nombre de
    /// una permanente.
    #[test]
    fn done_as_makes_a_transient_stage_permanent_and_renames_a_permanent_one() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::STOPPING);
        progress.done_as("Daemon detenido");
        progress.phase(&stage::SYNTHESIZING);
        progress.done_as("Síntesis larga");
        progress.note("1 s");
        progress.phase(&stage::PLAYING);
        let text = progress.captured();
        assert!(text.contains("✓ Daemon detenido"), "{text}");
        assert!(text.contains("✓ Síntesis larga"), "{text}");
        assert!(text.contains(" · 1 s"), "{text}");
        let snap = report::snapshot();
        assert_eq!(snap.stages[0].name, "Daemon detenido");
        assert_eq!(snap.stages[0].outcome, StageOutcome::Ok);
        assert_eq!(snap.stages[1].note.as_deref(), Some("1 s"));
    }

    /// El mismo `label` solo actualiza los textos y conserva el reloj y el
    /// nombre fijado con `done_as`.
    #[test]
    fn same_label_keeps_the_clock_and_the_done_override() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        progress.phase(&stage::SYNTHESIZING);
        progress.done_as("Síntesis larga");
        let started = progress.lock().current.as_ref().unwrap().started;
        std::thread::sleep(Duration::from_millis(5));
        progress.phase_with(&stage::SYNTHESIZING, "Sintetizando otra toma…");
        let current_started = progress.lock().current.as_ref().unwrap().started;
        assert_eq!(current_started, started, "el reloj no se reinicia");
        assert_eq!(
            progress.lock().current.as_ref().unwrap().active,
            "Sintetizando otra toma…"
        );
        assert_eq!(report::snapshot().stages[0].name, "Síntesis larga");
    }

    #[test]
    fn permanent_stage_without_tty_beats_when_due() {
        let _g = report::test_guard();
        let mut progress = PhaseProgress::for_test(None, false);
        progress.beat_every = Duration::ZERO;
        progress.phase(&stage::DOWNLOADING);
        progress.tick();
        progress.tick();
        assert_eq!(progress.captured().matches("  ● Descargando… ").count(), 2);
        drop(progress);
        let mut quiet = PhaseProgress::for_test(None, false);
        quiet.beat_every = Duration::from_secs(3600);
        report::lock().reset();
        line::reset_for_tests();
        quiet.phase(&stage::DOWNLOADING);
        quiet.tick();
        assert_eq!(lines_of(&quiet.captured()).len(), 1);
    }

    /// Con terminal, la etapa se pinta sobre la misma línea y una transitoria
    /// se borra sin dejar nada.
    #[test]
    fn tty_paints_in_place_and_clears_a_transient_stage() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::VALIDATING);
        assert!(line::phase_line_open());
        progress.phase(&stage::READING);
        let text = progress.captured();
        assert!(text.starts_with("\r\x1b[2K● Validando… 0,"), "{text:?}");
        assert!(!text.contains('✓'), "{text:?}");
        progress.tick();
        let _ = progress.close();
        assert!(!line::phase_line_open());
    }

    #[test]
    fn tty_permanent_stage_is_replaced_by_its_done_line() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        progress.phase(&stage::TRANSCRIBING);
        assert!(progress.captured().contains("\r\x1b[2K✓ Lectura"));
        let _ = progress.close();
    }

    #[test]
    fn yield_line_is_idempotent_and_reclaim_paints_again() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        progress.yield_line();
        progress.yield_line();
        let text = progress.captured();
        assert_eq!(text.matches("● Leyendo").count(), 1);
        assert_eq!(
            text.matches(CLEAR_LINE).count(),
            2,
            "pintar + ceder una vez"
        );
        assert!(!line::phase_line_open());
        progress.reclaim();
        assert!(line::phase_line_open());
        assert_eq!(progress.captured().matches("● Leyendo").count(), 2);
        let _ = progress.close();
    }

    #[test]
    fn a_console_warning_takes_the_line_and_tick_reopens_it() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        assert!(line::phase_line_open());
        line::warn("prueba de cesión por aviso");
        assert!(!line::phase_line_open());
        progress.yield_line();
        assert_eq!(progress.captured().matches(CLEAR_LINE).count(), 2);
        progress.tick();
        assert!(line::phase_line_open());
        let _ = progress.close();
    }

    /// `live` sustituye la línea activa con terminal y no escribe nada sin ella.
    #[test]
    fn live_replaces_the_active_line_only_with_a_terminal() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::CAPTURING);
        progress.live("● Grabando 0:05 / 0:40".to_string());
        progress.tick();
        assert!(progress
            .captured()
            .ends_with("\r\x1b[2K● Grabando 0:05 / 0:40"));
        let _ = progress.close();

        report::lock().reset();
        line::reset_for_tests();
        let plain = PhaseProgress::for_test(None, false);
        plain.phase(&stage::CAPTURING);
        plain.live("● Grabando 0:05 / 0:40".to_string());
        assert!(!plain.captured().contains("Grabando 0:05"));
    }

    /// Tras el eco de un Enter el indicador sube una línea antes de borrar,
    /// con y sin modo máquina; sin terminal no escribe nada.
    #[test]
    fn climb_echoed_enter_moves_up_before_clearing_in_every_mode() {
        let _g = report::test_guard();
        for machine in [false, true] {
            let mut progress = PhaseProgress::for_test(None, true);
            progress.machine = machine;
            progress.phase(&stage::CAPTURING);
            progress.live("● Grabando 0:05 / 0:40".to_string());
            progress.climb_echoed_enter();
            progress.phase(&stage::VALIDATING);
            assert!(
                progress.captured().contains("\x1b[1A\r\x1b[2K✓ Grabación"),
                "machine={machine}: {:?}",
                progress.captured()
            );
            let _ = progress.close();
            report::lock().reset();
            line::reset_for_tests();
        }
        let plain = PhaseProgress::for_test(None, false);
        plain.phase(&stage::CAPTURING);
        plain.climb_echoed_enter();
        assert!(!plain.captured().contains("\x1b[1A"));
    }

    /// La línea viva se recorta al ancho de la terminal conservando la duración.
    #[test]
    fn the_live_line_is_clipped_to_the_terminal_width() {
        let _g = report::test_guard();
        let mut progress = PhaseProgress::for_test(None, true);
        progress.width = Some(20);
        progress.phase_with(
            &stage::DOWNLOADING,
            "Descargando un modelo con un nombre larguísimo…",
        );
        let text = progress.captured();
        let painted = text.trim_start_matches(CLEAR_LINE);
        assert!(painted.chars().count() <= 19, "{painted:?}");
        assert!(painted.contains("… 0,"), "{painted:?}");
        assert!(painted.ends_with(" s"), "{painted:?}");
        let _ = progress.close();
    }

    /// `AVI_NO_PROGRESS` silencia las líneas de etapa y los latidos, también con
    /// terminal, pero no los `│` ni el cierre.
    #[test]
    fn no_progress_silences_the_stage_lines_only() {
        let _g = report::test_guard();
        for tty in [true, false] {
            report::lock().reset();
            line::reset_for_tests();
            let progress = PhaseProgress::for_test(Some("1"), tty);
            progress.phase(&stage::READING);
            progress.tick();
            progress.yield_line();
            progress.reclaim();
            progress.phase(&stage::TRANSCRIBING);
            progress.live("x".to_string());
            assert_eq!(err(), "", "tty = {tty}");
            progress.result(Some("Texto"), "hola");
            let expected = if tty {
                "  │ hola\n"
            } else {
                "  │ Texto: hola\n"
            };
            assert_eq!(err(), expected);
            progress.finish("Listo", &[]);
            assert!(err().ends_with("✓ Listo en 0,0 s\n"), "{}", err());
        }
    }

    #[test]
    fn empty_no_progress_value_does_not_silence() {
        assert!(!silenced_by(Some("")));
        assert!(!silenced_by(None));
        assert!(silenced_by(Some("0")));
        assert!(silenced_by(Some("1")));
    }

    #[test]
    fn close_and_drop_leave_no_open_line() {
        let _g = report::test_guard();
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

    /// `fail` pinta `✗` con el nombre de la etapa y la registra como fallida.
    #[test]
    fn fail_paints_the_failure_line_instead_of_the_done_line() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, true);
        progress.phase(&stage::READING);
        progress.fail();
        let text = err();
        assert!(!text.contains('✓'), "{text:?}");
        assert!(text.contains("✗ Lectura"), "{text:?}");
        assert!(!line::phase_line_open());
        assert_eq!(report::snapshot().stages[0].outcome, StageOutcome::Failed);
    }

    #[test]
    fn while_running_ticks_during_the_work_and_returns_its_value() {
        let _g = report::test_guard();
        let progress = PhaseProgress::for_test(None, false);
        let not_send = std::rc::Rc::new(7u32);
        assert_eq!(progress.while_running(|| *not_send + 1), 8);
    }

    #[test]
    fn while_running_ticks_during_long_work_and_stops_after() {
        let _g = report::test_guard();
        let mut progress = PhaseProgress::for_test(None, false);
        progress.beat_every = Duration::ZERO;
        progress.phase(&stage::DOWNLOADING);
        let value = progress.while_running(|| {
            std::thread::sleep(WHILE_RUNNING_TICK * 2 + Duration::from_millis(100));
            3u32
        });
        assert_eq!(value, 3);
        let beats = |p: &PhaseProgress| p.captured().matches("  ● Descargando…").count();
        let at_return = beats(&progress);
        assert!(
            at_return >= 1,
            "el trabajo largo debe repintar: {at_return}"
        );
        std::thread::sleep(WHILE_RUNNING_TICK + Duration::from_millis(100));
        assert_eq!(beats(&progress), at_return, "ningún repintado tras volver");
    }

    #[test]
    fn while_running_stops_the_helper_when_the_work_panics() {
        let _g = report::test_guard();
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
