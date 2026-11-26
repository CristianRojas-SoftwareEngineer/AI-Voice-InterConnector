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

use crate::fmt::sentence;
use crate::report::{self, ContextValue};
use crate::style::{paint, Stream, Tone, OK};

/// Hay una línea de fase pintada sin salto final en stderr. Solo el indicador
/// la abre, y solo en una terminal.
static PHASE_LINE_OPEN: AtomicBool = AtomicBool::new(false);

/// La salida es para una máquina (`--json`): el sobre sustituye a la línea de
/// contexto, a los resultados de texto y al cierre.
static MACHINE_MODE: AtomicBool = AtomicBool::new(false);

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
        eprint!("\r\x1b[2K");
    }
    open
}

/// Fija el modo máquina: con `--json` no se escriben la línea de contexto, los
/// resultados de texto ni el cierre.
pub fn set_machine_mode(machine: bool) {
    MACHINE_MODE.store(machine, Ordering::Relaxed);
}

/// Indica si la salida es para una máquina.
pub fn machine_mode() -> bool {
    MACHINE_MODE.load(Ordering::Relaxed)
}

/// Escribe un aviso en stderr en su propia línea: `Aviso: <mensaje>.`, con el
/// prefijo en amarillo y el punto final garantizado. Si hay una línea de fase
/// abierta la borra antes y la deja cedida, para que el indicador la vuelva a
/// pintar debajo del aviso. La línea solo se abre en una terminal, así que sin
/// terminal el aviso se escribe tal cual.
pub fn warn(msg: impl std::fmt::Display) {
    let text = sentence(&msg.to_string());
    let mut report = report::lock();
    report.add_warning(&text);
    take_phase_line();
    eprintln!("{} {text}", paint(Stream::Stderr, Tone::Warning, "Aviso:"));
}

/// Escribe la línea de contexto de un comando por stderr y la guarda en el
/// registro de ejecución. En modo máquina no la escribe, pero sí la guarda.
pub fn context(text: &str) {
    context_with(text, &[]);
}

/// Como [`context`], con pares clave/valor tipados para el `context` del
/// JSON.
pub fn context_with(text: &str, pairs: &[(&str, ContextValue)]) {
    let mut report = report::lock();
    report.set_context(text, pairs);
    if machine_mode() {
        return;
    }
    take_phase_line();
    eprintln!("{text}");
}

/// Escribe `✓ <texto>` por stderr, para los cierres sin indicador. En modo
/// máquina no escribe nada.
pub fn ok(text: &str) {
    if machine_mode() {
        return;
    }
    take_phase_line();
    eprintln!("{} {text}", paint(Stream::Stderr, Tone::Ok, OK));
}

/// Escribe un error por stderr: `Error: <mensaje>.` y, si hay solución,
/// `Solución: <solución>.`. El prefijo `Error:` va en rojo y la puntuación
/// final está garantizada.
pub fn error(message: &str, solution: Option<&str>) {
    take_phase_line();
    eprintln!(
        "{} {}",
        paint(Stream::Stderr, Tone::Error, "Error:"),
        sentence(message)
    );
    if let Some(solution) = solution {
        eprintln!("Solución: {}", sentence(solution));
    }
}

/// Escribe una línea atenuada por stderr, para los detalles de un cierre.
pub fn detail(text: &str) {
    take_phase_line();
    eprintln!("{}", paint(Stream::Stderr, Tone::Dim, text));
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Un aviso cede la línea abierta y la deja cerrada.
    #[test]
    fn warn_takes_an_open_phase_line() {
        let _guard = report::test_guard();
        set_phase_line_open(true);
        warn("prueba del canal de consola");
        assert!(!phase_line_open());
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

    /// El contexto, el cierre y el error ceden la línea abierta; el contexto y
    /// el cierre respetan el modo máquina.
    #[test]
    fn context_ok_and_error_take_the_line_and_obey_machine_mode() {
        let _guard = report::test_guard();
        set_phase_line_open(true);
        error("falló algo", Some("repítelo"));
        assert!(!phase_line_open());
        set_machine_mode(true);
        set_phase_line_open(true);
        context("no sale");
        ok("no sale");
        assert!(phase_line_open(), "en modo máquina no se toca la línea");
        set_machine_mode(false);
        context("sale");
        assert!(!phase_line_open());
        set_phase_line_open(false);
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
