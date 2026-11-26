// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Estado global de la salida de la consola y líneas de contexto, aviso y
//! error.
//!
//! En Windows, el arranque fija la página de códigos de entrada y salida a
//! UTF-8 para que los mensajes con tildes se vean bien en la PowerShell tal
//! como viene configurada, y activa las secuencias ANSI. Best-effort y
//! silencioso: sin consola no hace nada y fuera de Windows es un no-op.
//!
//! La línea de fase es la última línea de stderr, que el indicador de progreso
//! repinta en una terminal sin salto final. Un texto escrito encima quedaría
//! pegado a ella, así que todo texto que pueda salir bajo una fase pasa por las
//! funciones de este módulo: borran la línea abierta, la marcan como cedida y
//! escriben el texto en su propia línea. El indicador publica aquí si tiene una
//! línea abierta y la vuelve a pintar en su siguiente repintado.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crate::fmt::{duration, sentence};
use crate::progress::{body_indent, stage_line};
use crate::report::{self, ContextValue, ReportGuard};
use crate::style::{paint, Stream, Tone, OK, RULE, WARN};

/// Hay una línea de fase pintada sin salto final en stderr. Solo el indicador
/// la abre, y solo en una terminal.
static PHASE_LINE_OPEN: AtomicBool = AtomicBool::new(false);

/// La salida es para una máquina (`--json`): solo cambia stdout, que lleva el
/// sobre JSON en vez de los resultados de texto. Stderr es igual con y sin
/// ella.
static MACHINE_MODE: AtomicBool = AtomicBool::new(false);

/// Longitud del separador entre las etapas y el cierre.
const RULE_LEN: usize = 50;

/// Ancho que se asume cuando no se puede consultar la terminal.
const DEFAULT_WIDTH: usize = 80;

/// Cierres especiales que se pintan detrás del separador.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpecialClosing {
    /// El usuario contestó que no a una pregunta: `Cancelado; no se ha borrado nada.`
    Cancelled,
    /// Ctrl+C con trabajo hecho o en curso: `Interrumpido en <t>`.
    Interrupted,
    /// Ctrl+C antes de cualquier etapa permanente o durante una pregunta:
    /// `Interrumpido; no se ha modificado nada.`
    NothingModified,
}

/// Texto que se escribe en stderr. En las pruebas se acumula en un búfer
/// global, porque stderr no se puede capturar de forma portable. Quien llama
/// tiene tomada la guarda del registro.
pub(crate) fn write_err(text: &str) {
    #[cfg(test)]
    test_capture::ERR
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push_str(text);
    #[cfg(not(test))]
    {
        use std::io::Write;
        let mut stderr = std::io::stderr().lock();
        #[expect(
            clippy::let_underscore_must_use,
            reason = "si stderr no admite la escritura, la salida sigue sin mostrar nada"
        )]
        let _ = stderr.write_all(text.as_bytes());
        #[expect(
            clippy::let_underscore_must_use,
            reason = "si stderr no admite el vaciado, la salida sigue mostrándose"
        )]
        let _ = stderr.flush();
    }
}

/// Línea que se escribe en stdout. En las pruebas se acumula en un búfer
/// global.
pub(crate) fn write_out_line(text: &str) {
    #[cfg(test)]
    {
        let mut out = test_capture::OUT.lock().unwrap_or_else(|e| e.into_inner());
        out.push_str(text);
        out.push('\n');
    }
    #[cfg(not(test))]
    println!("{text}");
}

/// Ancho de la terminal de stderr en columnas; 80 si no se puede consultar.
#[cfg(windows)]
pub(crate) fn terminal_width() -> usize {
    use windows_sys::Win32::System::Console::{
        GetConsoleScreenBufferInfo, GetStdHandle, CONSOLE_SCREEN_BUFFER_INFO, STD_ERROR_HANDLE,
    };
    // SAFETY: llamada a la API de consola con el descriptor estándar de error
    // y un puntero a una estructura local, que es de solo datos y admite el
    // valor cero; un descriptor que no es una consola hace fallar la llamada,
    // que es la respuesta por defecto.
    unsafe {
        let handle = GetStdHandle(STD_ERROR_HANDLE);
        let mut info: CONSOLE_SCREEN_BUFFER_INFO = std::mem::zeroed();
        if GetConsoleScreenBufferInfo(handle, &mut info) == 0 {
            return DEFAULT_WIDTH;
        }
        let width = i32::from(info.srWindow.Right) - i32::from(info.srWindow.Left) + 1;
        usize::try_from(width)
            .ok()
            .filter(|columns| *columns > 0)
            .unwrap_or(DEFAULT_WIDTH)
    }
}

/// Ancho de la terminal de stderr en columnas. En Unix el crate no depende de
/// `libc`, así que no consulta `TIOCGWINSZ` y asume 80 columnas.
#[cfg(not(windows))]
pub(crate) fn terminal_width() -> usize {
    DEFAULT_WIDTH
}

/// Línea del separador entre las etapas y el cierre, sin salto final.
pub(crate) fn rule_text() -> String {
    RULE.repeat(RULE_LEN)
}

/// Escribe el separador en stderr, en su propia línea.
pub fn separator() {
    let _report = report::lock();
    take_phase_line();
    write_err(&format!("{}\n", rule_text()));
}

/// Pinta un cierre especial detrás del separador, tomando el registro.
pub fn special_closing(kind: SpecialClosing) {
    let mut report = report::lock();
    take_phase_line();
    special_closing_with(&mut report, kind);
}

/// Como [`special_closing`], con la guarda del registro ya tomada: es lo que
/// necesita el manejador de Ctrl+C, que no puede volver a tomarla. Con
/// `Interrupted`, antes marca como interrumpidas las etapas que siguen
/// abiertas y pinta su línea `✗ <Nombre> <duración> · interrumpido`. Quien
/// llama ya cedió la línea viva.
pub fn special_closing_with(report: &mut ReportGuard, kind: SpecialClosing) {
    let mut text = String::new();
    let color = crate::style::color_enabled(Stream::Stderr);
    if kind == SpecialClosing::Interrupted {
        report.interrupt_open_stages();
        let layout = report.layout();
        let unnumbered = report.unnumbered_width();
        let snapshot = report.snapshot();
        for record in snapshot
            .stages
            .iter()
            .filter(|record| record.outcome == report::StageOutcome::Interrupted)
        {
            if let Some(line) = stage_line(color, &layout, unnumbered, record) {
                text.push_str(&line);
                text.push('\n');
            }
        }
    }
    text.push_str(&rule_text());
    text.push('\n');
    let closing = match kind {
        SpecialClosing::Cancelled => "Cancelado; no se ha borrado nada.".to_string(),
        SpecialClosing::Interrupted => {
            let total: Duration = report.snapshot().total();
            format!("Interrumpido en {}", duration(total))
        }
        SpecialClosing::NothingModified => "Interrumpido; no se ha modificado nada.".to_string(),
    };
    text.push_str(&closing);
    text.push('\n');
    write_err(&text);
}

/// Publica si el indicador deja una línea de fase abierta en stderr.
pub fn set_phase_line_open(open: bool) {
    PHASE_LINE_OPEN.store(open, Ordering::Relaxed);
}

/// Indica si hay una línea de fase abierta en stderr.
pub fn phase_line_open() -> bool {
    PHASE_LINE_OPEN.load(Ordering::Relaxed)
}

/// Marca la línea de fase como cerrada sin escribir nada. Devuelve si estaba
/// abierta. Quien la usa se encarga de borrarla de la pantalla.
pub fn release_phase_line() -> bool {
    PHASE_LINE_OPEN.swap(false, Ordering::Relaxed)
}

/// Cede la línea de fase si está abierta: la borra y la marca como cerrada.
/// Devuelve si había una línea que ceder. Idempotente: la segunda llamada no
/// escribe nada.
pub fn take_phase_line() -> bool {
    let open = release_phase_line();
    if open {
        write_err("\r\x1b[2K");
    }
    open
}

/// Fija el modo máquina: con `--json` los resultados de texto no salen por
/// stdout, que lleva el sobre JSON. Stderr no cambia.
pub fn set_machine_mode(machine: bool) {
    MACHINE_MODE.store(machine, Ordering::Relaxed);
}

/// Indica si la salida es para una máquina.
pub fn machine_mode() -> bool {
    MACHINE_MODE.load(Ordering::Relaxed)
}

/// Escribe un aviso en stderr en su propia línea y lo guarda en el registro.
/// Con una etapa mostrada en el registro (abierta o terminada) sale como
/// `⚠ <mensaje>.` sangrado bajo ella; sin ninguna, como `Aviso: <mensaje>.`. El
/// punto final está garantizado. Si hay una línea de fase abierta la borra
/// antes y la deja cedida, para que el indicador la vuelva a pintar debajo del
/// aviso. La línea solo se abre en una terminal, así que sin terminal el aviso
/// se escribe tal cual.
pub fn warn(msg: impl std::fmt::Display) {
    let text = sentence(&msg.to_string());
    let mut report = report::lock();
    report.add_warning(&text);
    take_phase_line();
    let shown = report.last_shown_stage().map(|stage| stage.number);
    let line = match shown {
        Some(number) => {
            let indent = " ".repeat(body_indent(&report.layout(), number));
            format!(
                "{indent}{} {text}\n",
                paint(Stream::Stderr, Tone::Warning, WARN)
            )
        }
        None => format!(
            "{} {text}\n",
            paint(Stream::Stderr, Tone::Warning, "Aviso:")
        ),
    };
    write_err(&line);
}

/// Escribe la línea de contexto de un comando por stderr y la guarda en el
/// registro de ejecución. Sale igual con y sin `--json`.
pub fn context(text: &str) {
    context_with(text, &[]);
}

/// Como [`context`], con pares clave/valor tipados para el `context` del
/// JSON. Si el recorrido declarado tiene al menos 2 fases, añade bajo la
/// cabecera el mapa de fases y una línea en blanco.
pub fn context_with(text: &str, pairs: &[(&str, ContextValue)]) {
    let mut report = report::lock();
    report.set_context(text, pairs);
    take_phase_line();
    let mut out = format!("{text}\n");
    if let Some(map) = report.take_phase_map() {
        out.push_str(&map);
    }
    write_err(&out);
}

/// Escribe `✓ <texto>` por stderr, para los cierres sin indicador.
pub fn ok(text: &str) {
    let _report = report::lock();
    take_phase_line();
    write_err(&format!("{} {text}\n", paint(Stream::Stderr, Tone::Ok, OK)));
}

/// Escribe un error por stderr: `Error: <mensaje>.` y, si hay solución,
/// `Solución: <solución>.`. El prefijo `Error:` va en rojo y la puntuación
/// final está garantizada.
pub fn error(message: &str, solution: Option<&str>) {
    let _report = report::lock();
    take_phase_line();
    let mut out = format!(
        "{} {}\n",
        paint(Stream::Stderr, Tone::Error, "Error:"),
        sentence(message)
    );
    if let Some(solution) = solution {
        out.push_str(&format!("Solución: {}\n", sentence(solution)));
    }
    write_err(&out);
}

/// Escribe una línea atenuada por stderr, para los detalles de un cierre.
pub fn detail(text: &str) {
    let _report = report::lock();
    take_phase_line();
    write_err(&format!("{}\n", paint(Stream::Stderr, Tone::Dim, text)));
}

/// Fija la consola heredada a UTF-8 y activa las secuencias ANSI de stdout y
/// stderr. Llamar lo primero al arrancar, antes de cualquier salida.
pub fn init_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::CP_UTF8;
        use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
        // SAFETY: fijaciones de la página de códigos del proceso, sin
        // punteros; sin consola fallan sin efecto.
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
            SetConsoleCP(CP_UTF8);
        }
    }
    crate::style::vt_enabled(Stream::Stdout);
    crate::style::vt_enabled(Stream::Stderr);
}

/// Reinicia el estado global de este módulo. Solo lo llama
/// [`report::test_guard`], que además serializa las pruebas.
#[cfg(any(test, feature = "test-support"))]
pub(crate) fn reset_for_tests() {
    PHASE_LINE_OPEN.store(false, Ordering::Relaxed);
    MACHINE_MODE.store(false, Ordering::Relaxed);
    #[cfg(test)]
    {
        test_capture::ERR
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        test_capture::OUT
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

/// Búferes que acumulan lo que se habría escrito en stderr y en stdout.
#[cfg(test)]
pub(crate) mod test_capture {
    use std::sync::Mutex;

    pub(crate) static ERR: Mutex<String> = Mutex::new(String::new());
    pub(crate) static OUT: Mutex<String> = Mutex::new(String::new());

    /// Lo escrito hasta ahora en stderr.
    pub(crate) fn err() -> String {
        ERR.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Lo escrito hasta ahora en stdout.
    pub(crate) fn out() -> String {
        OUT.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Sustituye cada duración (`1,2 s`, `2 min 35 s`, `1 h 02 min`) por
    /// `<t>`, para comparar salidas sin depender del reloj.
    pub(crate) fn normalize(text: &str) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut out = String::new();
        let mut index = 0;
        while index < chars.len() {
            if let Some(end) = duration_at(&chars, index) {
                out.push_str("<t>");
                index = end;
            } else {
                out.push(chars[index]);
                index += 1;
            }
        }
        out
    }

    fn digits(chars: &[char], mut at: usize) -> Option<usize> {
        let start = at;
        while chars.get(at).is_some_and(char::is_ascii_digit) {
            at += 1;
        }
        (at > start).then_some(at)
    }

    fn literal(chars: &[char], at: usize, text: &str) -> Option<usize> {
        let wanted: Vec<char> = text.chars().collect();
        (chars.get(at..at + wanted.len())? == wanted.as_slice()).then_some(at + wanted.len())
    }

    /// Final de la duración que empieza en `at`, si empieza una.
    fn duration_at(chars: &[char], at: usize) -> Option<usize> {
        if at > 0 && chars[at - 1].is_ascii_digit() {
            return None;
        }
        let end = digits(chars, at)?;
        // `N h NN min`
        if let Some(next) = literal(chars, end, " h ") {
            let minutes = digits(chars, next)?;
            return literal(chars, minutes, " min");
        }
        // `N min N s`
        if let Some(next) = literal(chars, end, " min ") {
            let seconds = digits(chars, next)?;
            return literal(chars, seconds, " s");
        }
        // `N s` o `N,D s`
        let mut after = end;
        if let Some(next) = literal(chars, after, ",") {
            after = digits(chars, next)?;
        }
        literal(chars, after, " s")
    }
}

#[cfg(test)]
mod tests {
    use super::test_capture::{err, normalize};
    use super::*;
    use crate::report::{PhaseSpec, Pipeline};
    use crate::stage;
    use std::path::Path;

    /// Un aviso cede la línea abierta y la deja cerrada.
    #[test]
    fn warn_takes_an_open_phase_line() {
        let _guard = report::test_guard();
        set_phase_line_open(true);
        warn("prueba del canal de consola");
        assert!(!phase_line_open());
    }

    /// El normalizador sustituye las tres formas de duración y deja el resto.
    #[test]
    fn normalize_replaces_the_three_duration_forms() {
        assert_eq!(
            normalize("a 0,7 s b 2 min 35 s c 1 h 02 min d 12 s e 3 pasos"),
            "a <t> b <t> c <t> d <t> e 3 pasos"
        );
    }

    /// Sin etapas, un aviso sale como `Aviso:`; con una etapa, como `⚠`
    /// sangrado bajo ella (8 bajo una numerada, 2 bajo una sin numerar).
    #[test]
    fn warn_is_a_warning_line_under_its_stage() {
        let _guard = report::test_guard();
        warn("sin etapa");
        assert_eq!(err(), "Aviso: sin etapa.\n");

        // Sin numerar: sangría de 2.
        let progress = crate::progress::PhaseProgress::for_test(None, false);
        progress.phase(&stage::TRANSCRIBING);
        warn("audio muy bajo");
        assert!(err().ends_with("  ⚠ audio muy bajo.\n"), "{}", err());
        drop(progress);

        // Numerada: sangría de 8.
        reset();
        report::declare(Pipeline::new(vec![PhaseSpec {
            id: "text",
            title: "Texto",
            short: "Texto",
            stages: vec![&stage::TRANSCRIBING, &stage::TRANSLATING],
        }]));
        let progress = crate::progress::PhaseProgress::for_test(None, false);
        progress.phase(&stage::TRANSCRIBING);
        warn("audio muy bajo");
        assert!(err().ends_with("        ⚠ audio muy bajo.\n"), "{}", err());
        drop(progress);
    }

    /// Reinicia el registro dentro de una prueba que ya tiene la guarda.
    fn reset() {
        report::lock().reset();
        reset_for_tests();
    }

    /// Ceder la línea es idempotente: solo la primera vez había algo que ceder.
    #[test]
    fn taking_the_phase_line_is_idempotent() {
        let _guard = report::test_guard();
        set_phase_line_open(true);
        assert!(phase_line_open());
        assert!(take_phase_line());
        assert!(!take_phase_line());
        assert!(!phase_line_open());
    }

    /// Un aviso con la línea cerrada la deja cerrada.
    #[test]
    fn warn_with_a_closed_line_keeps_it_closed() {
        let _guard = report::test_guard();
        set_phase_line_open(false);
        warn("prueba sin línea abierta");
        assert!(!phase_line_open());
    }

    /// El contexto, el cierre y el error ceden la línea abierta y escriben lo
    /// mismo con y sin modo máquina: `--json` no suprime nada en stderr.
    #[test]
    fn context_ok_and_error_take_the_line_and_ignore_machine_mode() {
        let _guard = report::test_guard();
        let mut outputs = Vec::new();
        for machine in [false, true] {
            reset();
            set_machine_mode(machine);
            set_phase_line_open(true);
            error("falló algo", Some("repítelo"));
            assert!(!phase_line_open());
            set_phase_line_open(true);
            context("Cabecera");
            ok("Hecho");
            assert!(!phase_line_open());
            outputs.push(err());
        }
        assert_eq!(outputs[0], outputs[1]);
        assert!(
            outputs[0].contains("Cabecera\n") && outputs[0].contains("✓ Hecho\n"),
            "{}",
            outputs[0]
        );
    }

    /// El mapa de fases sale bajo la cabecera con 2 fases o más, en cualquier
    /// orden de declaración, y una sola vez.
    #[test]
    fn the_phase_map_follows_the_header_in_either_order() {
        let _guard = report::test_guard();
        let pipeline = || {
            Pipeline::new(vec![
                PhaseSpec {
                    id: "input",
                    title: "Entrada",
                    short: "Entrada",
                    stages: vec![&stage::CAPTURING],
                },
                PhaseSpec {
                    id: "voice",
                    title: "Voz",
                    short: "Voz",
                    stages: vec![&stage::SYNTHESIZING],
                },
            ])
        };
        report::declare(pipeline());
        context("Doblaje");
        context("Doblaje otra vez");
        assert_eq!(err(), "Doblaje\nEntrada → Voz\n\nDoblaje otra vez\n");
        reset();
        context("Doblaje");
        report::declare(pipeline());
        assert_eq!(err(), "Doblaje\nEntrada → Voz\n\n");
        reset();
        report::declare(Pipeline::new(vec![PhaseSpec {
            id: "only",
            title: "Única",
            short: "Única",
            stages: vec![&stage::CAPTURING],
        }]));
        context("Una fase");
        assert_eq!(err(), "Una fase\n", "con una fase no hay mapa");
    }

    /// Los tres cierres especiales llevan el separador de 50 caracteres.
    #[test]
    fn special_closings_sit_behind_the_separator() {
        let _guard = report::test_guard();
        let rule = "─".repeat(50);
        special_closing(SpecialClosing::Cancelled);
        assert_eq!(
            err(),
            format!("{rule}\nCancelado; no se ha borrado nada.\n")
        );
        reset();
        special_closing(SpecialClosing::NothingModified);
        assert_eq!(
            err(),
            format!("{rule}\nInterrumpido; no se ha modificado nada.\n")
        );
        reset();
        let progress = crate::progress::PhaseProgress::for_test(None, false);
        progress.phase(&stage::TRANSCRIBING);
        special_closing(SpecialClosing::Interrupted);
        assert_eq!(
            normalize(&err()),
            format!(
                "● Transcribiendo…\n✗ Transcripción  <t> · interrumpido\n{rule}\nInterrumpido en <t>\n"
            )
        );
        drop(progress);
    }

    /// El manifiesto declara la dependencia del sistema solo para Windows:
    /// en Unix el crate sigue siendo puro.
    #[test]
    fn system_dep_is_windows_only() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let mut parts = text.split("[target.");
        let head = parts.next().unwrap();
        assert!(
            !head.contains("windows-sys"),
            "windows-sys no debe estar en dependencias generales"
        );
        assert!(
            parts.any(|p| p.contains("cfg(windows)") && p.contains("windows-sys")),
            "windows-sys debe declararse solo para Windows"
        );
    }
}
