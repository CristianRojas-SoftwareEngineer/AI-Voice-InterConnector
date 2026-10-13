// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Ejecución de una prueba sin terminal. Lo declaran las pruebas que necesitan la celda
//! «sin terminal» de la tabla de confirmaciones con
//! `#[path = "support/no_terminal.rs"] mod no_terminal;`.
//!
//! `uninstall::run`, `cleanup::run` y `update::run` leen `stdin_is_terminal` del **stdin
//! real** del proceso, con `std::io::IsTerminal`. No es un parámetro, así que no hay
//! forma de recorrer la celda «sin terminal» sin cambiar el entorno de la puerta: si el
//! proceso se lanza desde una consola interactiva, `stdin` es una terminal y la operación
//! pediría confirmación en vez de negarse. Afirmar lo contrario en un proceso con
//! terminal sería pasar la prueba por el motivo equivocado.
//!
//! La salida es un proceso hijo: el mismo binario de prueba reejecutado con la entrada
//! redirigida a la null. No es una prueba aparte con nombre propio —la marca se lee de
//! una variable, así que la ejecución directa es la del padre— y el padre exige que el
//! hijo termine en éxito, de modo que el trabajo nunca se omite.

use crate::support::Sandbox;
use std::process::{Command, Stdio};

/// Variable que marca este proceso como el hijo sin terminal.
const MARK_WITHOUT_TERMINAL: &str = "AVI_LIFECYCLE_CHILD_WITHOUT_TERMINAL";
/// Variable con la raíz del sandbox que el hijo debe reconstruir.
pub const VAR_ROOT: &str = "AVI_LIFECYCLE_ROOT";
/// Variable con la etiqueta del sandbox que el hijo debe reconstruir.
pub const VAR_TAG: &str = "AVI_LIFECYCLE_TAG";
/// Prefijo de las líneas con las que el hijo informa al padre.
const REPORT_PREFIX: &str = "AVI_LIFECYCLE_INFORME: ";

/// `true` si este proceso es el hijo sin terminal.
pub fn is_child_without_terminal() -> bool {
    std::env::var_os(MARK_WITHOUT_TERMINAL).is_some()
}

/// Reejecuta `test_name` en un proceso hijo con `stdin` redirigido a la null.
///
/// El hijo hereda el entorno, incluida la marca, y recibe la raíz del sandbox del padre y
/// la `tag` con la que se creó. `--nocapture` es lo que deja la salida del hijo en su
/// `stdout` real, que es por donde el padre lee el informe.
pub fn run_without_terminal(test_name: &str, tag: &str, sandbox: &Sandbox) -> std::process::Output {
    Command::new(std::env::current_exe().expect("la ruta del binario de prueba"))
        .args(["--exact", test_name, "--nocapture", "--test-threads=1"])
        .env(MARK_WITHOUT_TERMINAL, "1")
        .env(VAR_ROOT, &sandbox.root)
        .env(VAR_TAG, tag)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("se reejecuta la prueba sin terminal")
}

/// Línea de informe del hijo.
pub fn report_line(line: &str) {
    println!("{REPORT_PREFIX}{line}");
}

/// Líneas de informe del hijo que el padre lee de su salida.
///
/// `libtest` imprime `test <nombre> ... ` **sin salto de línea** antes de la salida de la
/// prueba, así que la primera línea de informe viene pegada a ese prefijo. Por eso se
/// busca el marcador en cualquier punto de la línea y se toma lo que viene detrás, en vez
/// de exigir que la línea empiece por él.
pub fn read_report_lines(stdout: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .filter_map(|line| line.split_once(REPORT_PREFIX))
        .map(|(_, rest)| rest.trim().to_string())
        .collect()
}
