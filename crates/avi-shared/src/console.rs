// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Consola del proceso: el arranque en UTF-8 y la propiedad de la línea de
//! fase de stderr.
//!
//! En Windows, el arranque fija la página de códigos de entrada y salida a
//! UTF-8 para que los mensajes con tildes se vean bien en la PowerShell tal
//! como viene configurada. Best-effort y silencioso: sin consola no hace nada
//! y fuera de Windows es un no-op.
//!
//! La línea de fase es la última línea de stderr, que el indicador de progreso
//! repinta en una terminal sin salto final. Un aviso escrito encima quedaría
//! pegado a ella, así que todo aviso que pueda salir bajo una fase pasa por
//! [`warn`]: borra la línea abierta, la marca como cedida y escribe el aviso
//! en su propia línea. El indicador publica aquí si tiene una línea abierta y
//! la vuelve a pintar en su siguiente repintado.

use std::sync::atomic::{AtomicBool, Ordering};

/// Hay una línea de fase pintada sin salto final en stderr. Solo el indicador
/// la abre, y solo en una terminal.
static PHASE_LINE_OPEN: AtomicBool = AtomicBool::new(false);

/// Publica si el indicador deja una línea de fase abierta en stderr.
pub fn set_phase_line_open(open: bool) {
    PHASE_LINE_OPEN.store(open, Ordering::Relaxed);
}

/// Indica si hay una línea de fase abierta en stderr.
pub fn phase_line_open() -> bool {
    PHASE_LINE_OPEN.load(Ordering::Relaxed)
}

/// Cede la línea de fase si está abierta: la borra y la marca como cerrada.
/// Devuelve si había una línea que ceder. Idempotente: la segunda llamada no
/// escribe nada.
pub fn take_phase_line() -> bool {
    let open = PHASE_LINE_OPEN.swap(false, Ordering::Relaxed);
    if open {
        eprint!("\r\x1b[2K");
    }
    open
}

/// Escribe un aviso en stderr en su propia línea. Si hay una línea de fase
/// abierta la borra antes y la deja cedida, para que el indicador la vuelva a
/// pintar debajo del aviso. La línea solo se abre en una terminal, así que sin
/// terminal el aviso se escribe tal cual.
pub fn warn(msg: impl std::fmt::Display) {
    take_phase_line();
    eprintln!("{msg}");
}

/// Fija la consola heredada a UTF-8. Llamar lo primero al arrancar, antes de
/// cualquier salida.
pub fn force_utf8_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::CP_UTF8;
        use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
            SetConsoleCP(CP_UTF8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;
    use std::sync::Mutex;

    /// Serializa las pruebas que tocan el estado global de la línea de fase.
    static PHASE_LINE_LOCK: Mutex<()> = Mutex::new(());

    /// Un aviso cede la línea abierta y la deja cerrada.
    #[test]
    fn warn_takes_an_open_phase_line() {
        let _guard = PHASE_LINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_phase_line_open(true);
        warn("Aviso: prueba del canal de consola");
        assert!(!phase_line_open());
    }

    /// Ceder la línea es idempotente: solo la primera vez había algo que ceder.
    #[test]
    fn taking_the_phase_line_is_idempotent() {
        let _guard = PHASE_LINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_phase_line_open(true);
        assert!(phase_line_open());
        assert!(take_phase_line());
        assert!(!take_phase_line());
        assert!(!phase_line_open());
    }

    /// Un aviso con la línea cerrada la deja cerrada.
    #[test]
    fn warn_with_a_closed_line_keeps_it_closed() {
        let _guard = PHASE_LINE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set_phase_line_open(false);
        warn("Aviso: prueba sin línea abierta");
        assert!(!phase_line_open());
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
