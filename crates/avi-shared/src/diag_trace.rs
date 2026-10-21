// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Instrumentación **temporal** de diagnóstico del arranque del daemon.
//!
//! No forma parte del producto: existe solo para localizar en qué fase se va
//! el tiempo de un arranque fallido en el ejecutor de CI, y se retira al
//! terminar el diagnóstico. Es inocua por construcción: sin la variable
//! `AVI_DIAG_TRACE_FILE` en el entorno no hace nada, no escribe en stdout, no
//! toca el protocolo y no cambia ningún plazo.
//!
//! Cada marca lleva dos relojes: uno monotónico relativo al inicio del
//! proceso (`mono_ms`), que mide duración de fase dentro de un mismo proceso,
//! y uno de pared (`epoch_ms`), que permite ordenar en una sola cronología las
//! marcas de procesos distintos (el lanzador y su hijo). El PID y la etiqueta
//! correlacionan los procesos; no se escribe ninguna ruta ni ningún dato de
//! estado.

use std::io::Write;
use std::sync::OnceLock;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

/// Variable de entorno con la ruta del fichero donde se acumulan las marcas.
pub const TRACE_FILE_ENV: &str = "AVI_DIAG_TRACE_FILE";

/// Variable de entorno con la etiqueta que correlaciona las marcas de una
/// misma prueba.
pub const TRACE_TAG_ENV: &str = "AVI_DIAG_TRACE_TAG";

/// Origen monotónico del proceso: todas sus marcas comparten referencia.
fn origin() -> &'static Instant {
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now)
}

/// Escribe una marca de fase en el fichero de traza, si el entorno la pide.
///
/// Es deliberadamente silenciosa ante cualquier fallo: una traza que no se
/// pudo escribir no puede alterar el desenlace de la operación observada.
pub fn mark(phase: &str) {
    let Ok(path) = std::env::var(TRACE_FILE_ENV) else {
        return;
    };
    if path.is_empty() {
        return;
    }
    let epoch_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let mono_ms = origin().elapsed().as_millis();
    let pid = std::process::id();
    let tag = std::env::var(TRACE_TAG_ENV).unwrap_or_default();
    let line = format!("epoch_ms={epoch_ms} mono_ms={mono_ms} pid={pid} tag={tag} phase={phase}\n");
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}
