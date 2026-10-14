// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Scratch por proceso: el único sitio donde el producto crea temporales.
//!
//! Cada proceso tiene un directorio `<raíz de temporales>/<token propio>/`,
//! creado en el primer uso, donde caen todos sus temporales. El token es la
//! identidad exacta del proceso, así que cualquier otro proceso puede decidir si
//! el dueño de un directorio sigue vivo. La garantía de no dejar residuo es ese
//! barrido: al crear su scratch, cada proceso borra antes los directorios
//! hermanos cuyo dueño está muerto. Los de dueño vivo o no observable se
//! conservan, y los nombres que no son un token no se tocan.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{anyhow, Context, Result};
use avi_shared::{OwnerState, OwnerToken, TempWav};

/// Intentos de crear el scratch cuando otro proceso retira la raíz a la vez
/// (un barrido o un `cleanup` concurrente). Pasado el límite se devuelve error.
const CREATE_ATTEMPTS: usize = 5;

/// Scratch del proceso: `None` hasta el primer uso y de nuevo tras `release`.
static SCRATCH: Mutex<Option<PathBuf>> = Mutex::new(None);

fn lock() -> std::sync::MutexGuard<'static, Option<PathBuf>> {
    // Un pánico con el cerrojo tomado no deja el estado a medias: es un
    // `Option<PathBuf>` que solo se reemplaza entero.
    SCRATCH
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Directorio scratch del proceso. Lo crea en el primer uso (idempotente), tras
/// barrer los hermanos con dueño muerto.
pub fn dir() -> Result<PathBuf> {
    let mut slot = lock();
    if let Some(path) = slot.as_ref() {
        return Ok(path.clone());
    }
    let root =
        avi_shared::paths::scratch_root().context("no se pudo resolver la raíz de temporales")?;
    let own = crate::current_owner_token()?;
    let path = root.join(own.format());
    let mut last_error = None;
    for _ in 0..CREATE_ATTEMPTS {
        match prepare(&root, &path) {
            Ok(()) => {
                *slot = Some(path.clone());
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => last_error = Some(e),
            Err(e) => {
                return Err(
                    anyhow!(e).context(format!("no se pudo crear el scratch '{}'", path.display()))
                )
            }
        }
    }
    Err(anyhow!(last_error.expect("al menos un intento"))).context(format!(
        "no se pudo crear el scratch '{}' tras {CREATE_ATTEMPTS} intentos",
        path.display()
    ))
}

/// Barre los hermanos muertos y crea el directorio propio.
fn prepare(root: &Path, path: &Path) -> std::io::Result<()> {
    sweep_siblings(root, crate::classify_owner)?;
    std::fs::create_dir_all(path)
}

/// Borra en `root` los directorios cuyo nombre es un token con dueño `Dead`.
/// `Ours` y `Unknown` se conservan; lo que no parsea como token no se toca. Una
/// raíz inexistente no es error. Un directorio que no se puede borrar se avisa
/// por stderr con su ruta y el siguiente barrido lo reintenta.
fn sweep_siblings(
    root: &Path,
    classify: impl Fn(&OwnerToken) -> OwnerState,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        let Some(token) = entry.file_name().to_str().and_then(OwnerToken::parse) else {
            continue;
        };
        if !entry.file_type()?.is_dir() || classify(&token) != OwnerState::Dead {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => eprintln!(
                "aviso: no se pudo borrar el scratch huérfano '{}': {e}",
                entry.path().display()
            ),
        }
    }
    Ok(())
}

/// Builder de `tempfile` ya con el prefijo dado, para quien necesita elegir el
/// sufijo u otras opciones: debe terminar en `tempfile_in`/`tempdir_in` con
/// [`dir`].
pub fn tempfile(prefix: &str, suffix: &str) -> Result<tempfile::NamedTempFile> {
    tempfile::Builder::new()
        .prefix(prefix)
        .suffix(suffix)
        .tempfile_in(dir()?)
        .context("no se pudo crear el fichero temporal en el scratch")
}

/// Directorio temporal con dueño dentro del scratch: se borra al soltarse.
pub fn tempdir(prefix: &str) -> Result<tempfile::TempDir> {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir_in(dir()?)
        .context("no se pudo crear el directorio temporal en el scratch")
}

/// WAV temporal con dueño dentro del scratch.
pub fn temp_wav(prefix: &str) -> Result<TempWav> {
    TempWav::new(&dir()?, prefix).context("no se pudo reservar el WAV temporal en el scratch")
}

/// Borra el scratch propio. Si falla informa por stderr con la ruta, y el
/// barrido de otro proceso lo recoge. Sin scratch creado no hace nada.
pub fn release() {
    let Some(path) = lock().take() else {
        return;
    };
    match std::fs::remove_dir_all(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => eprintln!(
            "aviso: no se pudo borrar el scratch '{}': {e}",
            path.display()
        ),
    }
}

/// Guardián de proceso: al soltarse libera el scratch. Se crea al inicio de
/// `main` para cubrir la vuelta normal y el pánico; las salidas explícitas
/// pasan por [`terminate`], que no ejecuta `Drop`.
pub struct ReleaseGuard;

impl Drop for ReleaseGuard {
    fn drop(&mut self) {
        release();
    }
}

/// Única salida del proceso con código explícito: libera el scratch, vacía
/// stdout y sale. `std::process::exit` no ejecuta `Drop`, de modo que sin esto
/// el scratch quedaría a merced del barrido de otro proceso.
// Es la única llamada permitida a `process::exit` del producto: el resto de
// salidas pasan por aquí, que primero libera el scratch.
#[allow(clippy::disallowed_methods)]
pub fn terminate(code: i32) -> ! {
    release();
    // Sin canal donde avisar de un fallo al vaciar stdout (puede estar
    // cerrado): se sale igualmente con el código pedido.
    drop(std::io::Write::flush(&mut std::io::stdout()));
    std::process::exit(code)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn make(root: &Path, name: &str) -> PathBuf {
        let path = root.join(name);
        std::fs::create_dir_all(&path).unwrap();
        std::fs::write(path.join("a.tmp"), b"x").unwrap();
        path
    }

    /// Solo se borran los hermanos `Dead`; `Ours` y `Unknown` se conservan y lo
    /// que no es un token no se toca.
    #[test]
    fn sweep_removes_only_dead_siblings() {
        let root = tempfile::tempdir().unwrap();
        let dead = make(root.path(), "11-aaa");
        let ours = make(root.path(), "22-bbb");
        let unknown = make(root.path(), "33-ccc");
        let foreign = make(root.path(), "no-es-un-token");
        let file = root.path().join("44-ddd");
        std::fs::write(&file, b"x").unwrap();
        let seen = RefCell::new(Vec::new());
        sweep_siblings(root.path(), |token| {
            seen.borrow_mut().push(token.pid());
            match token.pid() {
                22 => OwnerState::Ours,
                33 => OwnerState::Unknown,
                _ => OwnerState::Dead,
            }
        })
        .unwrap();
        assert!(!dead.exists());
        assert!(ours.exists() && unknown.exists());
        assert!(foreign.exists(), "un nombre no parseable no se toca");
        assert!(file.exists(), "un fichero con forma de token no se toca");
        assert!(
            !seen.borrow().contains(&0),
            "el nombre no parseable no llega al clasificador"
        );
    }

    /// Una raíz inexistente no es error.
    #[test]
    fn sweep_of_missing_root_is_ok() {
        let root = tempfile::tempdir().unwrap();
        sweep_siblings(&root.path().join("no-existe"), |_| OwnerState::Dead).unwrap();
    }

    /// `prepare` barre con la clasificación real y crea el directorio propio:
    /// un hermano con PID imposible (reutilizado por otro inicio) se retira.
    #[test]
    fn prepare_creates_own_dir_and_sweeps_dead() {
        let root = tempfile::tempdir().unwrap();
        let dead = make(root.path(), "4294967295-1");
        let own = root.path().join("own");
        prepare(root.path(), &own).unwrap();
        assert!(own.is_dir());
        assert!(!dead.exists());
    }
}
