// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Carga única de ONNX Runtime para todo el workspace.
//!
//! El crate `ort` en modo `load-dynamic` no enlaza la librería en build-time:
//! la busca en ejecución y termina el proceso si no la encuentra, así que hay
//! que inicializarla explícitamente **antes** de crear la primera sesión. Este
//! módulo es la única autoridad de esa inicialización: lo usan el motor STT y
//! la sesión DPDFNet del crate `denoise`, de modo que ambas comparten un solo
//! `OnceLock` por proceso y una sola ruta resuelta.
//!
//! Sin auto-descarga y sin buscar en el PATH: o la librería está donde debe
//! estar, o el error nombra la ruta que se buscó.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// Nombre de la librería de ONNX Runtime que el bundle trae junto al
/// ejecutable en cada sistema.
const ORT_LIBRARY: &str = if cfg!(windows) {
    "onnxruntime.dll"
} else if cfg!(target_os = "macos") {
    "libonnxruntime.dylib"
} else {
    "libonnxruntime.so"
};

/// ONNX Runtime no se pudo cargar: la librería falta o no es válida. Lleva la
/// ruta buscada para que el mensaje diga dónde se esperaba.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OnnxRuntimeUnavailable {
    /// Ruta de la librería que se intentó cargar.
    pub path: PathBuf,
    /// Motivo del fallo.
    pub reason: String,
}

impl std::fmt::Display for OnnxRuntimeUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "no se pudo cargar ONNX Runtime desde {}: {}",
            self.path.display(),
            self.reason
        )
    }
}

impl std::error::Error for OnnxRuntimeUnavailable {}

/// Ruta de la librería de ONNX Runtime: la de `ORT_DYLIB_PATH` si está
/// definida y no vacía, que puede nombrar el fichero de la librería o el
/// directorio que la contiene, o la que el bundle trae en el directorio real
/// del programa, que sigue siendo el correcto cuando la app se invoca desde su
/// symlink.
pub fn onnx_runtime_path() -> Result<PathBuf, OnnxRuntimeUnavailable> {
    if let Some(value) = std::env::var_os("ORT_DYLIB_PATH").filter(|p| !p.is_empty()) {
        return Ok(resolve_ort_path(&value));
    }
    crate::paths::program_dir()
        .map(|dir| dir.join(ORT_LIBRARY))
        .map_err(|e| OnnxRuntimeUnavailable {
            path: PathBuf::from(ORT_LIBRARY),
            reason: format!("no se pudo resolver el directorio del programa: {e}"),
        })
}

/// Ruta de la librería según el valor de `ORT_DYLIB_PATH`: si nombra un
/// directorio existente, la librería de ONNX Runtime de este sistema dentro de
/// él; si no, la ruta tal cual.
fn resolve_ort_path(value: &OsStr) -> PathBuf {
    let path = PathBuf::from(value);
    if path.is_dir() {
        path.join(ORT_LIBRARY)
    } else {
        path
    }
}

/// Carga ONNX Runtime una sola vez por proceso y devuelve la ruta cargada;
/// las llamadas siguientes repiten el mismo resultado sin reintentar. Debe
/// preceder a cualquier otro uso de `ort`, que sin ella buscaría la librería
/// por su cuenta y abortaría si no la encuentra.
pub fn ensure_onnx_runtime() -> Result<PathBuf, OnnxRuntimeUnavailable> {
    static LOADED: std::sync::OnceLock<Result<PathBuf, OnnxRuntimeUnavailable>> =
        std::sync::OnceLock::new();
    LOADED
        .get_or_init(|| {
            let path = onnx_runtime_path()?;
            load_onnx_runtime(&path)?;
            Ok(path)
        })
        .clone()
}

/// Carga la librería de ONNX Runtime de `path` en `ort`. Un fichero
/// inexistente se rechaza antes de llamar a `ort`, de modo que el error no
/// depende de lo que el proceso haya cargado antes.
fn load_onnx_runtime(path: &Path) -> Result<(), OnnxRuntimeUnavailable> {
    let unavailable = |reason: String| OnnxRuntimeUnavailable {
        path: path.to_path_buf(),
        reason,
    };
    if !path.is_file() {
        return Err(unavailable("el fichero no existe".to_string()));
    }
    let builder = ort::init_from(path).map_err(|e| unavailable(e.to_string()))?;
    // `commit` devuelve `false` si el entorno ya estaba configurado; la
    // librería cargada es la misma, así que no es un fallo.
    builder.commit();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Una librería inexistente es un error con la ruta buscada, no un pánico.
    #[test]
    fn missing_onnx_runtime_is_typed_error() {
        // Prueba: ruta inexistente bajo el temporal del sistema, sin crear nada.
        #[allow(clippy::disallowed_methods)]
        let path = std::env::temp_dir()
            .join("avi-ort-inexistente")
            .join(ORT_LIBRARY);
        let err = load_onnx_runtime(&path).expect_err("la librería no existe");
        assert_eq!(err.path, path);
        assert!(
            err.to_string().contains(&path.display().to_string()),
            "{err}"
        );
    }

    /// Un directorio en `ORT_DYLIB_PATH` apunta a la librería de este sistema
    /// dentro de él.
    #[test]
    fn ort_path_directory_appends_library_name() {
        // Prueba: solo necesita un directorio existente, no escribe en él.
        #[allow(clippy::disallowed_methods)]
        let dir = std::env::temp_dir();
        assert_eq!(resolve_ort_path(dir.as_os_str()), dir.join(ORT_LIBRARY));
    }

    /// Una ruta que no es un directorio se usa tal cual.
    #[test]
    fn ort_path_file_is_used_as_is() {
        let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        assert_eq!(resolve_ort_path(file.as_os_str()), file);
    }
}
