// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Guardián del WAV temporal de `say`/`dub` y de los handlers del daemon.
//!
//! Quien crea el fichero lo borra al salir de ámbito, también en salidas de
//! error: el `Drop` ignora solo que el fichero ya no exista y avisa por stderr
//! de cualquier otro fallo de borrado.

use std::path::{Path, PathBuf};

/// WAV temporal con dueño: lo borra al salir de ámbito.
///
/// El constructor reserva una ruta con nombre aleatorio dentro del directorio
/// dado y crea el fichero vacío; el `Drop` lo retira sin fallar aunque ya se
/// haya borrado fuera.
pub struct TempWav {
    path: PathBuf,
}

impl TempWav {
    /// Reserva un WAV temporal dentro de `dir` con el prefijo de producto dado
    /// (p. ej. `"avi_say_"`) y un sufijo aleatorio.
    pub fn new(dir: &Path, prefix: &str) -> std::io::Result<Self> {
        let (_file, path) = tempfile::Builder::new()
            .prefix(prefix)
            .suffix(".wav")
            .tempfile_in(dir)?
            .keep()
            .map_err(|e| e.error)?;
        Ok(Self { path })
    }

    /// Ruta del temporal, para sintetizar, escribir o reproducir.
    ///
    /// Es `&PathBuf` y no `&Path` porque el motor de síntesis la recibe como
    /// `Option<&PathBuf>`; devolver `&Path` obligaría a convertir en cada punto
    /// de uso.
    pub fn path(&self) -> &PathBuf {
        &self.path
    }
}

impl Drop for TempWav {
    fn drop(&mut self) {
        match std::fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // `Drop` no puede propagar ni hacer pánico: el fallo queda visible
            // por stderr con la ruta que se queda en disco.
            Err(e) => eprintln!(
                "aviso: no se pudo borrar el temporal '{}': {e}",
                self.path.display()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::TempWav;

    /// Salida normal de ámbito: al soltarse el guardián el fichero desaparece.
    #[test]
    fn scope_exit_removes_the_file() {
        let dir = tempfile::tempdir().expect("directorio de destino");
        let path = {
            let guard = TempWav::new(dir.path(), "avi_shared_test_").expect("reservar el temporal");
            assert!(guard.path().is_file(), "el constructor crea el fichero");
            guard.path().clone()
        };
        assert!(
            !path.exists(),
            "al salir de ámbito no debe quedar el temporal"
        );
    }

    /// Descarte explícito con `drop`: también borra el fichero.
    #[test]
    fn explicit_drop_removes_the_file() {
        let dir = tempfile::tempdir().expect("directorio de destino");
        let guard = TempWav::new(dir.path(), "avi_shared_test_").expect("reservar el temporal");
        let path = guard.path().clone();
        assert!(path.is_file(), "el constructor crea el fichero");
        drop(guard);
        assert!(!path.exists(), "el descarte explícito debe borrarlo");
    }

    /// Un fallo de borrado distinto de «no existe» (aquí, una carpeta en la
    /// ruta) no entra en pánico al soltarse el guardián.
    #[test]
    fn drop_with_unremovable_path_does_not_panic() {
        let dir = tempfile::tempdir().expect("directorio de destino");
        let guard = TempWav::new(dir.path(), "avi_shared_test_").expect("reservar el temporal");
        let path = guard.path().clone();
        std::fs::remove_file(&path).expect("borrado externo");
        std::fs::create_dir(&path).expect("carpeta en la ruta del temporal");
        drop(guard);
        std::fs::remove_dir(&path).expect("limpieza de la carpeta");
    }

    /// Borrado externo previo: soltarse después no falla y no recrea nada.
    #[test]
    fn drop_after_external_delete_does_not_fail() {
        let dir = tempfile::tempdir().expect("directorio de destino");
        let guard = TempWav::new(dir.path(), "avi_shared_test_").expect("reservar el temporal");
        let path = guard.path().clone();
        std::fs::remove_file(&path).expect("borrado externo");
        drop(guard);
        assert!(
            !path.exists(),
            "tras el borrado externo no debe quedar nada"
        );
    }
}
