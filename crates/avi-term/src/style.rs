// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Paleta y reglas del color. El color solo se emite si el flujo concreto es
//! una terminal, `NO_COLOR` no está definida (o está vacía) y la consola acepta
//! secuencias ANSI.

use std::io::IsTerminal;
use std::sync::OnceLock;

/// Símbolo de una etapa terminada.
pub const OK: &str = "✓";
/// Símbolo de un fallo.
pub const FAIL: &str = "✗";
/// Símbolo de una etapa en curso.
pub const ACTIVE: &str = "…";
/// Símbolo de una grabación en curso.
pub const REC: &str = "●";

/// Flujo de salida al que va un texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Función del color de un texto.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Verde: `✓`.
    Ok,
    /// Rojo: `Error:` y `✗`.
    Error,
    /// Amarillo: `Aviso:`.
    Warning,
    /// Atenuado: duraciones y detalles.
    Dim,
    /// Negrita: resultados.
    Strong,
}

impl Tone {
    /// Código SGR del tono.
    fn sgr(self) -> &'static str {
        match self {
            Tone::Ok => "32",
            Tone::Error => "31",
            Tone::Warning => "33",
            Tone::Dim => "2",
            Tone::Strong => "1",
        }
    }
}

/// Decide si se puede emitir color: el flujo es una terminal, `NO_COLOR` no
/// está definida o está vacía y la consola acepta secuencias ANSI.
pub fn color_allowed(is_terminal: bool, no_color: Option<&str>, vt_enabled: bool) -> bool {
    is_terminal && no_color.is_none_or(str::is_empty) && vt_enabled
}

/// Envuelve el texto en SGR si `enabled`; si no, lo devuelve tal cual.
pub fn paint_if(enabled: bool, tone: Tone, text: &str) -> String {
    if enabled {
        format!("\x1b[{}m{text}\x1b[0m", tone.sgr())
    } else {
        text.to_string()
    }
}

/// Devuelve el texto envuelto en SGR solo si el color está permitido en el
/// flujo.
pub fn paint(stream: Stream, tone: Tone, text: &str) -> String {
    paint_if(color_enabled(stream), tone, text)
}

/// ¿Se emite color en este flujo? Se calcula una vez por flujo.
pub fn color_enabled(stream: Stream) -> bool {
    static STDOUT: OnceLock<bool> = OnceLock::new();
    static STDERR: OnceLock<bool> = OnceLock::new();
    let cell = match stream {
        Stream::Stdout => &STDOUT,
        Stream::Stderr => &STDERR,
    };
    *cell.get_or_init(|| {
        let is_terminal = match stream {
            Stream::Stdout => std::io::stdout().is_terminal(),
            Stream::Stderr => std::io::stderr().is_terminal(),
        };
        let no_color = std::env::var("NO_COLOR").ok();
        color_allowed(is_terminal, no_color.as_deref(), vt_enabled(stream))
    })
}

/// ¿Acepta la consola secuencias ANSI en este flujo? Se intenta activar una
/// sola vez por flujo. Fuera de Windows las terminales las aceptan siempre.
pub fn vt_enabled(stream: Stream) -> bool {
    static STDOUT: OnceLock<bool> = OnceLock::new();
    static STDERR: OnceLock<bool> = OnceLock::new();
    let cell = match stream {
        Stream::Stdout => &STDOUT,
        Stream::Stderr => &STDERR,
    };
    *cell.get_or_init(|| enable_vt(stream))
}

#[cfg(windows)]
fn enable_vt(stream: Stream) -> bool {
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetConsoleMode, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    let which = match stream {
        Stream::Stdout => STD_OUTPUT_HANDLE,
        Stream::Stderr => STD_ERROR_HANDLE,
    };
    // SAFETY: llamadas a la API de consola con el descriptor estándar del
    // proceso y un puntero a una variable local; un descriptor que no es una
    // consola hace fallar `GetConsoleMode`, que es la respuesta `false`.
    unsafe {
        let handle = GetStdHandle(which);
        let mut mode = 0u32;
        if GetConsoleMode(handle, &mut mode) == 0 {
            return false;
        }
        SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
    }
}

#[cfg(not(windows))]
fn enable_vt(_stream: Stream) -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_requires_terminal_no_no_color_and_vt() {
        assert!(color_allowed(true, None, true));
        assert!(color_allowed(true, Some(""), true));
        assert!(!color_allowed(false, None, true), "sin terminal");
        assert!(!color_allowed(true, Some("1"), true), "NO_COLOR definida");
        assert!(!color_allowed(true, None, false), "sin ANSI");
    }

    #[test]
    fn paint_wraps_in_sgr_only_when_enabled() {
        assert_eq!(paint_if(true, Tone::Ok, "x"), "\x1b[32mx\x1b[0m");
        assert_eq!(paint_if(true, Tone::Error, "x"), "\x1b[31mx\x1b[0m");
        assert_eq!(paint_if(true, Tone::Warning, "x"), "\x1b[33mx\x1b[0m");
        assert_eq!(paint_if(true, Tone::Dim, "x"), "\x1b[2mx\x1b[0m");
        assert_eq!(paint_if(true, Tone::Strong, "x"), "\x1b[1mx\x1b[0m");
        assert_eq!(paint_if(false, Tone::Ok, "x"), "x");
    }
}
