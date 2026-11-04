// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Borrado de artefactos propios del ciclo de vida: un mecanismo y sus políticas.
//!
//! El **mecanismo** es de plataforma y lo implementa el binario —en Unix es
//! `remove_dir_all`, y en Windows el limpiador propio, desacoplado cuando el
//! anfitrión lo permite—, porque el motor no lo puede implementar sin arrastrar
//! `avi-daemon`. Entra por un único rasgo, [`PathRemover`].
//!
//! La **decisión** es del motor y tiene dos políticas con nombre, que deciden
//! distinto a propósito:
//!
//! - [`retire`], para la basura del ciclo (staging y aparcado): se borra ya, si no
//!   se puede se programa, y si tampoco se puede queda para el barrido de la
//!   siguiente operación. Nunca falla la operación.
//! - `uninstall::remove_program_dir`, para el programa en uso: si el ejecutable
//!   está dentro, solo se programa, y si no puede programarse el error sube.
//!
//! El limpiador siempre recibe la carpeta raíz de un artefacto propio, nunca los
//! archivos de dentro uno a uno.

use std::path::Path;

use avi_process::{OwnerToken, Scheduling};

/// Mecanismo de plataforma para borrar una ruta propia del ciclo.
pub trait PathRemover {
    /// Borra la ruta ya. Una ruta ausente es éxito: el plan dice lo que tiene
    /// que dejar de existir, y si ya no existe el objetivo está cumplido.
    fn remove_now(&self, path: &Path) -> anyhow::Result<()>;
    /// Programa el borrado de `path` para cuando termine el proceso `wait`,
    /// identificado por PID e inicio. `Ok` significa que el limpiador está en
    /// marcha y dice cómo quedó (separado del proceso anfitrión o dependiente de
    /// él); si no puede garantizarse, `Err`. Es el caso diferido de Windows.
    fn schedule(&self, path: &Path, wait: &OwnerToken) -> anyhow::Result<Scheduling>;
}

/// Cómo quedó un artefacto tras retirarlo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Retirement {
    /// Borrado ya, de forma síncrona.
    Removed,
    /// Programado para cuando termine el proceso que lo retiene (Windows).
    Scheduled(Scheduling),
    /// No se pudo borrar ni programar: lo barre la recuperación de la siguiente
    /// operación.
    Kept,
}

/// Política de la basura del ciclo: borra `target` ya; si no puede, programa su
/// borrado para cuando termine `wait`, el proceso que lo retiene; y si tampoco
/// puede, lo deja al barrido. No imprime nada: informar es del llamador.
pub fn retire(target: &Path, remover: &dyn PathRemover, wait: &OwnerToken) -> Retirement {
    if remover.remove_now(target).is_ok() {
        return Retirement::Removed;
    }
    match remover.schedule(target, wait) {
        Ok(scheduling) => Retirement::Scheduled(scheduling),
        Err(_) => Retirement::Kept,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::path::PathBuf;

    /// Doble con borrado y programación configurables que registra las llamadas.
    struct Scripted {
        removes: bool,
        schedules: Option<Scheduling>,
        calls: RefCell<Vec<(&'static str, PathBuf)>>,
    }

    impl PathRemover for Scripted {
        fn remove_now(&self, path: &Path) -> anyhow::Result<()> {
            self.calls
                .borrow_mut()
                .push(("remove_now", path.to_path_buf()));
            if self.removes {
                Ok(())
            } else {
                anyhow::bail!("en uso")
            }
        }

        fn schedule(&self, path: &Path, _wait: &OwnerToken) -> anyhow::Result<Scheduling> {
            self.calls
                .borrow_mut()
                .push(("schedule", path.to_path_buf()));
            self.schedules
                .ok_or_else(|| anyhow::anyhow!("sin limpiador"))
        }
    }

    /// `retire` da sus tres desenlaces: borra ya si puede, programa si no, y deja
    /// el artefacto al barrido si tampoco puede programarlo. Programa solo cuando
    /// el borrado inmediato falla.
    #[test]
    fn retire_has_three_outcomes() {
        let own = avi_process::current_owner_token().unwrap();
        let target = Path::new("/programa/.old-x");
        for (removes, schedules, expected, calls) in [
            (true, None, Retirement::Removed, 1),
            (
                false,
                Some(Scheduling::Detached),
                Retirement::Scheduled(Scheduling::Detached),
                2,
            ),
            (false, None, Retirement::Kept, 2),
        ] {
            let remover = Scripted {
                removes,
                schedules,
                calls: RefCell::new(Vec::new()),
            };
            assert_eq!(retire(target, &remover, &own), expected);
            let calls_made = remover.calls.borrow();
            assert_eq!(calls_made.len(), calls);
            assert!(calls_made.iter().all(|(_, path)| path == target));
        }
    }
}
