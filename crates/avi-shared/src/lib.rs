// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Fuente única estructural del bundle, las rutas y los pines del producto.
//!
//! Micro-crate puro: tipos del manifiesto del bundle con su parseo, directorios
//! canónicos del producto con sus revisiones de modelos, lectura de
//! `packaging/pins.json`. Sin red ni TLS a propósito, de modo que `xtask`
//! puede depender de él sin arrastrar el árbol de `hf-hub` que impide usar
//! `avi-store` desde el tooling. Excepción a esa pureza, acotada: el módulo
//! opcional `onnx_runtime` carga ONNX Runtime en ejecución (sin red ni enlace
//! en build-time) solo cuando alguien activa su feature.

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

pub mod console;
pub mod diag_trace;
pub mod logs;
pub mod manifest;
#[cfg(feature = "native-ort")]
pub mod onnx_runtime;
pub mod owner;
pub mod paths;
pub mod pins;
pub mod staging_names;
pub mod temp_wav;

pub use owner::{OwnerState, OwnerToken};
pub use temp_wav::TempWav;

/// Limpieza de disco de mejor esfuerzo: su fallo no cambia el desenlace de la
/// operación que la pide, pero tampoco se calla. Un `NotFound` es el estado que
/// la limpieza busca (no quedaba nada) y no se avisa; cualquier otro error se
/// avisa por stderr con `Aviso:` y el contexto, que nombra lo que no se limpió.
pub fn best_effort(result: std::io::Result<()>, context: impl std::fmt::Display) {
    if let Some(warning) = best_effort_warning(result, context) {
        console::warn(warning);
    }
}

/// Aviso que corresponde al resultado de una limpieza, o `None` si no hay nada
/// que avisar.
fn best_effort_warning(
    result: std::io::Result<()>,
    context: impl std::fmt::Display,
) -> Option<String> {
    match result {
        Err(e) if e.kind() != std::io::ErrorKind::NotFound => Some(format!("{context}: {e}.")),
        _ => None,
    }
}

#[cfg(test)]
mod best_effort_tests {
    use super::best_effort_warning;
    use std::io::{Error, ErrorKind};

    /// El éxito y la ausencia no avisan; cualquier otro fallo avisa con su
    /// contexto y su causa.
    #[test]
    fn warns_only_on_failures_other_than_not_found() {
        assert_eq!(best_effort_warning(Ok(()), "x"), None);
        assert_eq!(
            best_effort_warning(Err(Error::from(ErrorKind::NotFound)), "x"),
            None
        );
        let warning = best_effort_warning(
            Err(Error::new(ErrorKind::PermissionDenied, "denegado")),
            "no se pudo borrar a.tmp",
        );
        assert_eq!(
            warning.as_deref(),
            Some("no se pudo borrar a.tmp: denegado.")
        );
    }
}
