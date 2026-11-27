// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Registro de ejecución del proceso: la fuente única del texto, del JSON y del
//! manejador de Ctrl+C.
//!
//! Guarda, en un estado global protegido por un `Mutex`:
//! - el contexto del comando (texto de la cabecera y pares tipados);
//! - el recorrido declarado ([`Pipeline`]): fases y etapas numeradas;
//! - cada etapa ([`StageRecord`]) con su inicio, su fin y su desenlace;
//! - los resultados y los avisos, ligados a la última etapa abierta cuando se
//!   emitieron;
//! - las pausas por preguntas al usuario, que no cuentan como tiempo de trabajo;
//! - si ya salió el envelope JSON del proceso.
//!
//! Orden de cerrojos: toda escritura de este crate en stderr se hace con la
//! guarda del registro ([`lock`]) tomada. El indicador de etapas toma primero su
//! propio estado interno y después el registro, y nunca al revés. Mientras se
//! tiene la guarda no se llama a nada que vuelva a tomarla, porque el cerrojo no
//! es reentrante.

use std::ops::{Deref, DerefMut};
use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use crate::stage::Stage;

/// Ya salió el envelope JSON de este proceso. Es un `AtomicBool` aparte del
/// registro para que quien emite pueda comprobarlo y fijarlo sin tomar el
/// cerrojo del registro.
pub static ENVELOPE_EMITTED: AtomicBool = AtomicBool::new(false);

/// Valor tipado de un par del `context` del JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextValue {
    /// Texto.
    Text(String),
    /// Entero sin signo.
    Int(u64),
}

/// Cabecera del comando: el texto que se pinta y los pares del `context`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextRecord {
    /// Texto de la cabecera tal como se pinta.
    pub text: String,
    /// Pares clave/valor del `context` del JSON, en el orden en que se dieron.
    pub pairs: Vec<(String, ContextValue)>,
}

/// Una fase del recorrido declarado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhaseSpec {
    /// Identificador de máquina, por ejemplo `input`, `text`, `voice` o `save`.
    pub id: &'static str,
    /// Título de la fase, por ejemplo `Voz · streaming`.
    pub title: &'static str,
    /// Nombre corto para el mapa y el desglose, por ejemplo `Voz`.
    pub short: &'static str,
    /// Etapas numeradas de la fase, en orden.
    pub stages: Vec<&'static Stage>,
}

/// Recorrido declarado de un comando: la lista ordenada de sus fases. El total
/// de etapas (N de `[n/N]`) es la suma de las etapas de todas las fases.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Pipeline {
    /// Fases en orden de ejecución.
    pub phases: Vec<PhaseSpec>,
}

impl Pipeline {
    /// Recorrido con las fases dadas.
    pub fn new(phases: Vec<PhaseSpec>) -> Self {
        Self { phases }
    }

    /// Total de etapas declaradas (N).
    pub fn total(&self) -> usize {
        self.phases.iter().map(|phase| phase.stages.len()).sum()
    }

    /// Fase (identificador) y número de orden (desde 1) de la etapa con ese
    /// `label`, si el recorrido la incluye.
    pub fn locate(&self, label: &str) -> Option<(&'static str, usize)> {
        self.locate_nth(label, 0)
    }

    /// Como [`Pipeline::locate`], para la aparición número `skip + 1` de la
    /// etapa con ese `label`: sirve a los comandos que declaran la misma etapa
    /// varias veces, como una limpieza por archivo.
    pub fn locate_nth(&self, label: &str, skip: usize) -> Option<(&'static str, usize)> {
        let mut number = 0;
        let mut seen = 0;
        for phase in &self.phases {
            for stage in &phase.stages {
                number += 1;
                if stage.label == label {
                    if seen == skip {
                        return Some((phase.id, number));
                    }
                    seen += 1;
                }
            }
        }
        None
    }
}

/// Cómo terminó una etapa.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageOutcome {
    /// Terminó bien (también mientras sigue abierta).
    Ok,
    /// Falló.
    Failed,
    /// Se omitió durante la ejecución, con su motivo.
    Skipped(String),
    /// Etapa transitoria que el recorrido no declara: no deja línea ni número.
    Transient,
    /// La interrumpió el usuario con Ctrl+C.
    Interrupted,
}

/// Una etapa del registro.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageRecord {
    /// Identificador estable dentro del proceso, para ligar resultados y avisos.
    pub id: u64,
    /// Identificador de catálogo (`Stage.label`).
    pub label: &'static str,
    /// Nombre de columna.
    pub name: String,
    /// Fase del recorrido declarado, o `None`.
    pub phase: Option<&'static str>,
    /// Número de orden en el recorrido declarado, o `None`.
    pub number: Option<usize>,
    /// Instante de apertura.
    pub start: Instant,
    /// Instante de cierre; `None` mientras la etapa sigue abierta.
    pub end: Option<Instant>,
    /// Desenlace.
    pub outcome: StageOutcome,
    /// Dato complementario (`· …`).
    pub note: Option<String>,
}

/// Un resultado, ligado a la última etapa abierta cuando se emitió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResultRecord {
    /// Etapa a la que quedó ligado, o `None` si aún no había ninguna.
    pub stage_id: Option<u64>,
    /// Etiqueta del resultado.
    pub label: Option<String>,
    /// Texto del resultado.
    pub text: String,
}

/// Un aviso, ligado a la última etapa abierta cuando se emitió.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WarningRecord {
    /// Etapa a la que quedó ligado, o `None` si aún no había ninguna.
    pub stage_id: Option<u64>,
    /// Texto del aviso.
    pub text: String,
}

/// Copia inmutable del registro en un instante.
#[derive(Debug, Clone)]
pub struct Snapshot {
    /// Cabecera del comando, si se declaró.
    pub context: Option<ContextRecord>,
    /// Recorrido declarado, si se declaró.
    pub pipeline: Option<Pipeline>,
    /// Etapas en orden de apertura.
    pub stages: Vec<StageRecord>,
    /// Resultados en orden de emisión.
    pub results: Vec<ResultRecord>,
    /// Avisos en orden de emisión.
    pub warnings: Vec<WarningRecord>,
    /// Hay una pregunta al usuario en curso.
    pub question_active: bool,
    /// Instante en que se tomó la copia.
    pub taken_at: Instant,
    origin: Option<Instant>,
    pauses: Vec<(Instant, Instant)>,
}

impl Snapshot {
    /// Tiempo transcurrido desde el origen del reloj hasta `at`, sin las pausas
    /// por preguntas. Es cero si todavía no hay origen.
    pub fn relative(&self, at: Instant) -> Duration {
        relative_to(self.origin, &self.pauses, at)
    }

    /// Tiempo total del comando hasta el momento de la copia, sin pausas.
    pub fn total(&self) -> Duration {
        self.relative(self.taken_at)
    }

    /// ¿Hay alguna etapa que no sea transitoria?
    pub fn has_permanent_stage(&self) -> bool {
        self.stages
            .iter()
            .any(|stage| stage.outcome != StageOutcome::Transient)
    }
}

fn relative_to(origin: Option<Instant>, pauses: &[(Instant, Instant)], at: Instant) -> Duration {
    let Some(origin) = origin else {
        return Duration::ZERO;
    };
    let raw = at.saturating_duration_since(origin);
    let paused: Duration = pauses
        .iter()
        .map(|&(from, to)| {
            let from = from.max(origin);
            let to = to.min(at);
            to.saturating_duration_since(from)
        })
        .sum();
    raw.saturating_sub(paused)
}

/// Estado interno del registro. Solo se toca a través de [`ReportGuard`].
pub struct Report {
    context: Option<ContextRecord>,
    pipeline: Option<Pipeline>,
    stages: Vec<StageRecord>,
    results: Vec<ResultRecord>,
    warnings: Vec<WarningRecord>,
    next_id: u64,
    origin: Option<Instant>,
    pauses: Vec<(Instant, Instant)>,
    question_since: Option<Instant>,
    /// Fases cuyo título ya se pintó.
    titled_phases: Vec<&'static str>,
    /// El mapa de fases ya se pintó bajo la cabecera.
    map_printed: bool,
}

/// Medidas del recorrido declarado que necesita el render.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    /// Total de etapas numeradas declaradas (N).
    pub total: usize,
    /// Número de fases declaradas.
    pub phases: usize,
    /// Ancho del nombre más largo de las etapas declaradas.
    pub name_width: usize,
}

impl Layout {
    /// Ancho mínimo de la columna de nombres de las etapas sin numerar.
    pub(crate) const MIN_UNNUMBERED_WIDTH: usize = 12;

    /// ¿Se muestra la numeración `[n/N]`? Solo con al menos 2 etapas numeradas.
    pub(crate) fn numbered(&self) -> bool {
        self.total >= 2
    }

    /// ¿Se muestran los títulos de fase, el mapa y el desglose? Solo con al
    /// menos 2 fases.
    pub(crate) fn phased(&self) -> bool {
        self.phases >= 2
    }
}

impl Report {
    const fn new() -> Self {
        Self {
            context: None,
            pipeline: None,
            stages: Vec::new(),
            results: Vec::new(),
            warnings: Vec::new(),
            next_id: 0,
            origin: None,
            pauses: Vec::new(),
            question_since: None,
            titled_phases: Vec::new(),
            map_printed: false,
        }
    }

    /// Medidas del recorrido declarado (todo cero si no hay recorrido).
    pub(crate) fn layout(&self) -> Layout {
        let Some(pipeline) = &self.pipeline else {
            return Layout {
                total: 0,
                phases: 0,
                name_width: 0,
            };
        };
        let name_width = pipeline
            .phases
            .iter()
            .flat_map(|phase| phase.stages.iter())
            .map(|stage| stage.name.chars().count())
            .max()
            .unwrap_or(0);
        Layout {
            total: pipeline.total(),
            phases: pipeline.phases.len(),
            name_width,
        }
    }

    /// Ancho de la columna de nombres de las etapas sin numerar: el del más
    /// largo de las ya mostradas, con un mínimo de 12.
    pub(crate) fn unnumbered_width(&self) -> usize {
        let layout = self.layout();
        self.stages
            .iter()
            .filter(|stage| stage.outcome != StageOutcome::Transient)
            .filter(|stage| !(layout.numbered() && stage.number.is_some()))
            .map(|stage| stage.name.chars().count())
            .max()
            .unwrap_or(0)
            .max(Layout::MIN_UNNUMBERED_WIDTH)
    }

    /// Registro de la etapa `id`.
    pub(crate) fn stage(&self, id: u64) -> Option<&StageRecord> {
        self.stages.iter().find(|stage| stage.id == id)
    }

    /// Última etapa registrada que no es transitoria, si hay alguna.
    pub(crate) fn last_shown_stage(&self) -> Option<&StageRecord> {
        self.stages
            .iter()
            .rev()
            .find(|stage| stage.outcome != StageOutcome::Transient)
    }

    /// Título de la fase `phase` si todavía no se pintó y el recorrido tiene al
    /// menos 2 fases; lo marca como pintado.
    pub(crate) fn take_phase_title(&mut self, phase: &'static str) -> Option<&'static str> {
        if !self.layout().phased() || self.titled_phases.contains(&phase) {
            return None;
        }
        let title = self
            .pipeline
            .as_ref()?
            .phases
            .iter()
            .find(|spec| spec.id == phase)?
            .title;
        self.titled_phases.push(phase);
        Some(title)
    }

    /// Mapa de fases (`Entrada → Texto → Voz`) seguido de una línea en blanco,
    /// si hay cabecera, el recorrido tiene al menos 2 fases y todavía no se
    /// pintó; lo marca como pintado.
    pub(crate) fn take_phase_map(&mut self) -> Option<String> {
        if self.map_printed || self.context.is_none() || !self.layout().phased() {
            return None;
        }
        let map = self
            .pipeline
            .as_ref()?
            .phases
            .iter()
            .map(|phase| phase.short)
            .collect::<Vec<_>>()
            .join(" → ");
        self.map_printed = true;
        Some(format!("{map}\n\n"))
    }

    /// Fija el origen del reloj si todavía no lo tiene.
    pub(crate) fn set_origin_if_none(&mut self, at: Instant) {
        self.origin.get_or_insert(at);
    }

    /// Guarda la cabecera del comando.
    pub(crate) fn set_context(&mut self, text: &str, pairs: &[(&str, ContextValue)]) {
        self.context = Some(ContextRecord {
            text: text.to_string(),
            pairs: pairs
                .iter()
                .map(|(key, value)| ((*key).to_string(), value.clone()))
                .collect(),
        });
    }

    /// Id de la última etapa abierta (la última registrada), si hay alguna.
    fn last_stage_id(&self) -> Option<u64> {
        self.stages.last().map(|stage| stage.id)
    }

    /// Registra un aviso ligado a la última etapa abierta.
    pub(crate) fn add_warning(&mut self, text: &str) {
        let stage_id = self.last_stage_id();
        self.warnings.push(WarningRecord {
            stage_id,
            text: text.to_string(),
        });
    }

    /// Registra un resultado ligado a la última etapa abierta.
    pub(crate) fn add_result(&mut self, label: Option<&str>, text: &str) {
        let stage_id = self.last_stage_id();
        self.results.push(ResultRecord {
            stage_id,
            label: label.map(str::to_string),
            text: text.to_string(),
        });
    }

    /// Abre una etapa en `start` y devuelve su id. Si el recorrido declarado la
    /// incluye, queda con fase y número; si no, una transitoria queda con el
    /// desenlace `Transient` y una permanente sin número. Una etapa que ya se
    /// abrió con número en este proceso y se vuelve a abrir (la acción extra de
    /// un menú, como volver a reproducir) es una repetición: queda sin fase ni
    /// número, para que el recorrido declarado no la cuente dos veces. Si el
    /// recorrido la declara varias veces (una limpieza por archivo), cada
    /// apertura toma la siguiente aparición declarada.
    pub(crate) fn open_stage(&mut self, stage: &'static Stage, start: Instant) -> u64 {
        let numbered_before = self
            .stages
            .iter()
            .filter(|opened| opened.label == stage.label && opened.number.is_some())
            .count();
        let located = self
            .pipeline
            .as_ref()
            .and_then(|pipeline| pipeline.locate_nth(stage.label, numbered_before));
        let outcome = if located.is_none() && stage.transient {
            StageOutcome::Transient
        } else {
            StageOutcome::Ok
        };
        let id = self.next_id;
        self.next_id += 1;
        self.stages.push(StageRecord {
            id,
            label: stage.label,
            name: stage.name.to_string(),
            phase: located.map(|(phase, _)| phase),
            number: located.map(|(_, number)| number),
            start,
            end: None,
            outcome,
            note: None,
        });
        id
    }

    /// ¿Incluye el recorrido declarado la etapa con ese `label`?
    pub(crate) fn is_declared(&self, label: &str) -> bool {
        self.pipeline
            .as_ref()
            .is_some_and(|pipeline| pipeline.locate(label).is_some())
    }

    fn stage_mut(&mut self, id: u64) -> Option<&mut StageRecord> {
        self.stages.iter_mut().find(|stage| stage.id == id)
    }

    /// Cierra la etapa `id` en `end`. Con `outcome`, además fija el desenlace.
    pub(crate) fn close_stage(&mut self, id: u64, end: Instant, outcome: Option<StageOutcome>) {
        if let Some(stage) = self.stage_mut(id) {
            stage.end = Some(end);
            if let Some(outcome) = outcome {
                stage.outcome = outcome;
            }
        }
    }

    /// Cambia el nombre de columna de la etapa `id` y, si `permanent`, la saca
    /// del desenlace `Transient`.
    pub(crate) fn rename_stage(&mut self, id: u64, name: &str, permanent: bool) {
        if let Some(stage) = self.stage_mut(id) {
            stage.name = name.to_string();
            if permanent && stage.outcome == StageOutcome::Transient {
                stage.outcome = StageOutcome::Ok;
            }
        }
    }

    /// Fija el dato complementario de la etapa `id`.
    pub(crate) fn set_note(&mut self, id: u64, note: &str) {
        if let Some(stage) = self.stage_mut(id) {
            stage.note = Some(note.to_string());
        }
    }

    /// Quita la etapa `id` del registro: se abandonó sin dejar rastro.
    pub(crate) fn remove_stage(&mut self, id: u64) {
        self.stages.retain(|stage| stage.id != id);
    }

    /// Registra una etapa omitida con `start == end == at`.
    pub(crate) fn skip_stage(&mut self, stage: &'static Stage, reason: &str, at: Instant) -> u64 {
        let id = self.open_stage(stage, at);
        self.close_stage(id, at, Some(StageOutcome::Skipped(reason.to_string())));
        id
    }

    /// Marca como interrumpidas, con cierre en `at`, las etapas que siguen
    /// abiertas. Devuelve cuántas eran.
    pub(crate) fn interrupt_open_stages(&mut self, at: Instant) -> usize {
        let mut count = 0;
        for stage in self.stages.iter_mut().filter(|stage| stage.end.is_none()) {
            stage.end = Some(at);
            stage.outcome = StageOutcome::Interrupted;
            count += 1;
        }
        count
    }

    fn only_transient_stages(&self) -> bool {
        self.stages
            .iter()
            .all(|stage| stage.outcome == StageOutcome::Transient)
    }

    /// Copia inmutable del estado.
    fn snapshot(&self) -> Snapshot {
        let mut pauses = self.pauses.clone();
        let taken_at = Instant::now();
        if let Some(since) = self.question_since {
            pauses.push((since, taken_at));
        }
        Snapshot {
            context: self.context.clone(),
            pipeline: self.pipeline.clone(),
            stages: self.stages.clone(),
            results: self.results.clone(),
            warnings: self.warnings.clone(),
            question_active: self.question_since.is_some(),
            taken_at,
            origin: self.origin,
            pauses,
        }
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }
}

static REPORT: Mutex<Report> = Mutex::new(Report::new());

/// Acceso exclusivo al registro y, por convención, a la escritura en stderr de
/// este crate. Mientras se tiene no se debe llamar a nada que vuelva a tomarla.
pub struct ReportGuard {
    inner: MutexGuard<'static, Report>,
}

impl Deref for ReportGuard {
    type Target = Report;

    fn deref(&self) -> &Report {
        &self.inner
    }
}

impl DerefMut for ReportGuard {
    fn deref_mut(&mut self) -> &mut Report {
        &mut self.inner
    }
}

impl ReportGuard {
    /// Copia inmutable del registro, sin volver a tomar el cerrojo.
    pub fn snapshot(&self) -> Snapshot {
        self.inner.snapshot()
    }

    /// Marca como interrumpidas las etapas que siguen abiertas, con cierre
    /// ahora. Devuelve cuántas eran.
    pub fn interrupt_open_stages(&mut self) -> usize {
        self.inner.interrupt_open_stages(Instant::now())
    }
}

/// Toma el registro y, con él, la escritura en stderr.
pub fn lock() -> ReportGuard {
    ReportGuard {
        inner: REPORT.lock().unwrap_or_else(|e| e.into_inner()),
    }
}

/// Declara el recorrido del comando. Las etapas que se abran después y estén en
/// él quedan con su fase y su número.
pub fn declare(pipeline: Pipeline) {
    let mut report = lock();
    report.pipeline = Some(pipeline);
    // Si la cabecera salió antes que el recorrido, el mapa sale ahora.
    if let Some(map) = report.take_phase_map() {
        crate::line::take_phase_line();
        crate::line::write_err(&map);
    }
}

/// Copia inmutable del registro.
pub fn snapshot() -> Snapshot {
    lock().snapshot()
}

/// Empieza una pregunta al usuario: su duración no cuenta como trabajo.
pub fn begin_question() {
    lock().question_since = Some(Instant::now());
}

/// Termina la pregunta. Si se confirmó y todavía no hay etapas no transitorias,
/// el reloj arranca ahora; si ya las hay, la duración de la pregunta se acumula
/// como pausa. Si no se confirmó, el reloj no se toca.
pub fn end_question(confirmed: bool) {
    let mut report = lock();
    let Some(since) = report.question_since.take() else {
        return;
    };
    if !confirmed {
        return;
    }
    let now = Instant::now();
    if report.only_transient_stages() {
        if report.origin.is_some() {
            report.origin = Some(now);
        }
        report.pauses.clear();
    } else {
        report.pauses.push((since, now));
    }
}

/// Cerrojo que serializa las pruebas que comparten el registro y el estado
/// global de `line`.
#[cfg(any(test, feature = "test-support"))]
static TEST_LOCK: Mutex<()> = Mutex::new(());

/// Toma el cerrojo de pruebas y reinicia el registro, la marca de envelope
/// emitido y el estado global de la línea de fase. Toda prueba que cree un
/// indicador, emita un envelope o lea el registro empieza con esta guarda.
#[cfg(any(test, feature = "test-support"))]
pub fn test_guard() -> MutexGuard<'static, ()> {
    use std::sync::atomic::Ordering;

    let guard = TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    lock().reset();
    ENVELOPE_EMITTED.store(false, Ordering::SeqCst);
    crate::line::reset_for_tests();
    guard
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::progress::PhaseProgress;
    use crate::stage;
    use std::sync::atomic::Ordering;

    fn two_phase_pipeline() -> Pipeline {
        Pipeline::new(vec![
            PhaseSpec {
                id: "input",
                title: "Entrada",
                short: "Entrada",
                stages: vec![&stage::CAPTURING],
            },
            PhaseSpec {
                id: "voice",
                title: "Voz · streaming",
                short: "Voz",
                stages: vec![&stage::SYNTHESIZING, &stage::PLAYING],
            },
        ])
    }

    #[test]
    fn declare_counts_the_numbered_stages() {
        let _g = test_guard();
        let pipeline = two_phase_pipeline();
        assert_eq!(pipeline.total(), 3);
        assert_eq!(pipeline.locate("capturing"), Some(("input", 1)));
        assert_eq!(pipeline.locate("playing"), Some(("voice", 3)));
        assert_eq!(pipeline.locate("saving"), None);
        declare(pipeline);
        assert_eq!(snapshot().pipeline.map(|p| p.total()), Some(3));
    }

    #[test]
    fn a_closed_stage_keeps_phase_number_and_times() {
        let _g = test_guard();
        declare(two_phase_pipeline());
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::CAPTURING);
        progress.note("2,9 s");
        progress.end_stage();
        let snap = snapshot();
        let [record] = snap.stages.as_slice() else {
            panic!("se esperaba una etapa: {:?}", snap.stages);
        };
        assert_eq!(record.label, "capturing");
        assert_eq!(record.name, "Grabación");
        assert_eq!(record.phase, Some("input"));
        assert_eq!(record.number, Some(1));
        assert!(record.end.is_some());
        assert_eq!(record.outcome, StageOutcome::Ok);
        assert_eq!(record.note.as_deref(), Some("2,9 s"));
    }

    #[test]
    fn undeclared_stages_are_transient_or_unnumbered() {
        let _g = test_guard();
        declare(two_phase_pipeline());
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::VALIDATING);
        progress.phase(&stage::READING);
        progress.end_stage();
        let snap = snapshot();
        assert_eq!(snap.stages[0].outcome, StageOutcome::Transient);
        assert_eq!(snap.stages[0].number, None);
        assert_eq!(snap.stages[1].outcome, StageOutcome::Ok);
        assert_eq!(snap.stages[1].number, None);
    }

    #[test]
    fn a_declared_transient_stage_is_numbered() {
        let _g = test_guard();
        declare(Pipeline::new(vec![PhaseSpec {
            id: "save",
            title: "Guardado",
            short: "Guardado",
            stages: vec![&stage::SAVING],
        }]));
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::SAVING);
        progress.end_stage();
        let record = snapshot().stages.remove(0);
        assert_eq!(record.number, Some(1));
        assert_eq!(record.outcome, StageOutcome::Ok);
    }

    #[test]
    fn a_repeated_declared_stage_is_unnumbered() {
        let _g = test_guard();
        declare(two_phase_pipeline());
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::SYNTHESIZING);
        progress.end_stage();
        progress.phase(&stage::SYNTHESIZING);
        progress.end_stage();
        let snap = snapshot();
        assert_eq!(snap.stages[0].number, Some(2));
        assert_eq!(snap.stages[0].phase, Some("voice"));
        assert_eq!(snap.stages[1].number, None);
        assert_eq!(snap.stages[1].phase, None);
        assert_eq!(snap.stages[1].outcome, StageOutcome::Ok);
    }

    /// Un recorrido que declara la misma etapa varias veces (una limpieza por
    /// archivo) numera cada apertura con la siguiente aparición, también las
    /// omitidas, y la apertura que excede lo declarado queda sin número.
    #[test]
    fn a_stage_declared_several_times_numbers_each_opening() {
        let _g = test_guard();
        declare(Pipeline::new(vec![PhaseSpec {
            id: "files",
            title: "Limpieza",
            short: "Limpieza",
            stages: vec![&stage::DENOISING, &stage::DENOISING, &stage::DENOISING],
        }]));
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::DENOISING);
        progress.done_as("uno.wav");
        progress.end_stage();
        progress.skip_as(&stage::DENOISING, "dos.wav", "ya existía");
        progress.phase(&stage::DENOISING);
        progress.done_as("tres.wav");
        progress.end_stage();
        progress.phase(&stage::DENOISING);
        progress.end_stage();
        let numbers: Vec<_> = snapshot().stages.iter().map(|s| s.number).collect();
        assert_eq!(numbers, vec![Some(1), Some(2), Some(3), None]);
        let names: Vec<_> = snapshot().stages.iter().map(|s| s.name.clone()).collect();
        assert_eq!(&names[..3], ["uno.wav", "dos.wav", "tres.wav"]);
    }

    /// `interrupt` deja la etapa viva como interrumpida y no vuelve a pintarla
    /// al soltar el indicador.
    #[test]
    fn interrupt_marks_the_open_stage_without_painting_it_again() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::DENOISING);
        progress.interrupt();
        let record = snapshot().stages.remove(0);
        assert_eq!(record.outcome, StageOutcome::Interrupted);
        assert!(record.end.is_some());
    }

    #[test]
    fn skip_records_an_empty_interval() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.skip(&stage::SAVING, "ya existía");
        let record = snapshot().stages.remove(0);
        assert_eq!(record.end, Some(record.start));
        assert_eq!(record.outcome, StageOutcome::Skipped("ya existía".into()));
    }

    #[test]
    fn fail_current_marks_the_open_stage_as_failed() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::TRANSCRIBING);
        progress.fail_current();
        let record = snapshot().stages.remove(0);
        assert_eq!(record.outcome, StageOutcome::Failed);
        assert!(record.end.is_some());
    }

    #[test]
    fn dropping_with_an_open_permanent_stage_records_a_failure() {
        let _g = test_guard();
        {
            let progress = PhaseProgress::for_test(None, false);
            progress.phase(&stage::SYNTHESIZING);
        }
        let record = snapshot().stages.remove(0);
        assert_eq!(record.outcome, StageOutcome::Failed);
    }

    #[test]
    fn dropping_with_an_open_transient_stage_keeps_it_transient() {
        let _g = test_guard();
        {
            let progress = PhaseProgress::for_test(Some("1"), false);
            progress.phase(&stage::VALIDATING);
        }
        let record = snapshot().stages.remove(0);
        assert_eq!(record.outcome, StageOutcome::Transient);
        assert!(record.end.is_some());
    }

    #[test]
    fn overlap_keeps_two_stages_alive() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::SYNTHESIZING);
        let at = Instant::now();
        progress.overlap(&stage::PLAYING, at);
        let snap = snapshot();
        assert_eq!(snap.stages.len(), 2);
        assert!(snap.stages.iter().all(|s| s.end.is_none()));
        assert_eq!(snap.stages[1].start, at);
        let end = Instant::now();
        progress.end_stage_at(&stage::SYNTHESIZING, end);
        let snap = snapshot();
        assert_eq!(snap.stages[0].end, Some(end));
        assert!(snap.stages[1].end.is_none());
        progress.end_stage();
        assert!(snapshot().stages.iter().all(|s| s.end.is_some()));
    }

    #[test]
    fn confirming_without_stages_restarts_the_clock() {
        let _g = test_guard();
        let _progress = PhaseProgress::for_test(Some("1"), false);
        begin_question();
        std::thread::sleep(Duration::from_millis(60));
        assert!(snapshot().question_active);
        end_question(true);
        let snap = snapshot();
        assert!(!snap.question_active);
        assert!(
            snap.total() < Duration::from_millis(40),
            "{:?}",
            snap.total()
        );
    }

    #[test]
    fn confirming_with_stages_accumulates_a_pause() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::READING);
        progress.end_stage();
        begin_question();
        std::thread::sleep(Duration::from_millis(80));
        end_question(true);
        let snap = snapshot();
        assert!(
            snap.total() < Duration::from_millis(60),
            "{:?}",
            snap.total()
        );
        let (from, to) = snap.pauses[0];
        assert!(to.duration_since(from) >= Duration::from_millis(80));
    }

    #[test]
    fn declining_a_question_leaves_the_clock_alone() {
        let _g = test_guard();
        let _progress = PhaseProgress::for_test(Some("1"), false);
        begin_question();
        std::thread::sleep(Duration::from_millis(40));
        end_question(false);
        let snap = snapshot();
        assert!(snap.pauses.is_empty());
        assert!(snap.total() >= Duration::from_millis(40));
    }

    #[test]
    fn warnings_and_results_bind_to_the_last_opened_stage() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        crate::line::warn("antes de todo");
        progress.phase(&stage::TRANSCRIBING);
        crate::line::warn("durante la transcripción");
        progress.result(Some("Transcripción"), "hola");
        let snap = snapshot();
        let id = snap.stages[0].id;
        assert_eq!(snap.warnings[0].stage_id, None);
        assert_eq!(snap.warnings[1].stage_id, Some(id));
        assert_eq!(snap.results[0].stage_id, Some(id));
        assert_eq!(snap.results[0].label.as_deref(), Some("Transcripción"));
    }

    #[test]
    fn context_keeps_text_and_typed_pairs() {
        let _g = test_guard();
        crate::line::context_with(
            "Doblaje es → en",
            &[
                ("voice", ContextValue::Text("default".into())),
                ("max_secs", ContextValue::Int(40)),
            ],
        );
        let context = snapshot().context.expect("hay contexto");
        assert_eq!(context.text, "Doblaje es → en");
        assert_eq!(context.pairs.len(), 2);
        assert_eq!(context.pairs[1], ("max_secs".into(), ContextValue::Int(40)));
    }

    #[test]
    fn interrupt_marks_only_open_stages() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        progress.phase(&stage::READING);
        progress.end_stage();
        progress.phase(&stage::TRANSCRIBING);
        let count = lock().interrupt_open_stages();
        assert_eq!(count, 1);
        let snap = snapshot();
        assert_eq!(snap.stages[0].outcome, StageOutcome::Ok);
        assert_eq!(snap.stages[1].outcome, StageOutcome::Interrupted);
    }

    /// Dos hilos escriben (etapas y avisos) mientras un tercero toma copias: no
    /// hay bloqueo mutuo y las copias son coherentes.
    #[test]
    fn concurrent_writers_and_a_reader_do_not_deadlock() {
        let _g = test_guard();
        let progress = PhaseProgress::for_test(Some("1"), false);
        let stop = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..50 {
                    progress.phase(&stage::READING);
                    progress.tick();
                    progress.end_stage();
                }
            });
            scope.spawn(|| {
                for index in 0..50 {
                    crate::line::warn(format!("aviso {index}"));
                }
            });
            let reader = scope.spawn(|| {
                let mut reads = 0;
                while !stop.load(Ordering::SeqCst) {
                    let snap = snapshot();
                    assert!(snap.stages.len() <= 50);
                    reads += 1;
                }
                reads
            });
            std::thread::sleep(Duration::from_millis(50));
            while snapshot().warnings.len() < 50 {
                std::thread::yield_now();
            }
            stop.store(true, Ordering::SeqCst);
            assert!(reader.join().unwrap() > 0);
        });
        assert_eq!(snapshot().warnings.len(), 50);
    }
}
