// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Token de propietario: la identidad exacta de un proceso, escrita como texto.
//!
//! Un PID solo no identifica a un proceso, porque el sistema lo reutiliza. El
//! token le añade el inicio del proceso, de modo que dos procesos con el mismo
//! PID en momentos distintos tienen tokens distintos. Su forma de texto es
//! `<pid>-<inicio>`: el PID en decimal canónico sin ceros a la izquierda y el
//! inicio como una cadena no vacía de `[0-9a-z]`. Ese alfabeto es válido en un
//! nombre de fichero en todos los sistemas, así que el token puede ir dentro de
//! uno. Este módulo solo da forma al texto; observar el proceso es cosa de quien
//! construye el token.

/// Identidad exacta de un proceso: su PID y el inicio con el que se observó.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct OwnerToken {
    pid: u32,
    start: String,
}

impl OwnerToken {
    /// Construye el token de un proceso. `None` si el PID es `0` o si el inicio
    /// está vacío o contiene algo distinto de `[0-9a-z]`.
    pub fn new(pid: u32, start: impl Into<String>) -> Option<Self> {
        let start = start.into();
        let valid_start = !start.is_empty()
            && start
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase());
        (pid != 0 && valid_start).then_some(Self { pid, start })
    }

    /// PID del proceso.
    pub fn pid(&self) -> u32 {
        self.pid
    }

    /// Inicio del proceso, en `[0-9a-z]+`.
    pub fn start(&self) -> &str {
        &self.start
    }

    /// Forma de texto `<pid>-<inicio>`.
    pub fn format(&self) -> String {
        format!("{}-{}", self.pid, self.start)
    }

    /// Interpreta la forma de texto. Estricto: solo acepta lo que produce
    /// [`format`](Self::format); cualquier otra cosa (signo, ceros a la
    /// izquierda, espacios, mayúsculas, separadores de más) es `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let (pid_text, start) = text.split_once('-')?;
        let canonical_pid = !pid_text.is_empty()
            && pid_text.bytes().all(|b| b.is_ascii_digit())
            && !pid_text.starts_with('0');
        if !canonical_pid {
            return None;
        }
        Self::new(pid_text.parse().ok()?, start)
    }
}

/// Estado de un propietario registrado respecto del sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OwnerState {
    /// Vive con la misma identidad del token.
    Ours,
    /// No existe, o existe con otra identidad (el PID se reutilizó).
    Dead,
    /// No se puede observar: no se sabe si vive, y no se debe dar por muerto.
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::OwnerToken;

    /// El formato y el parseo son inversos.
    #[test]
    fn format_and_parse_roundtrip() {
        for (pid, start) in [(1, "0"), (4242, "133456789012345678"), (u32::MAX, "ab12cd")] {
            let token = OwnerToken::new(pid, start).expect("token válido");
            let text = token.format();
            assert_eq!(text, format!("{pid}-{start}"));
            assert_eq!(OwnerToken::parse(&text), Some(token));
        }
    }

    /// El texto mal formado se rechaza.
    #[test]
    fn parse_rejects_malformed() {
        for bad in [
            "",
            "-",
            "42",
            "42-",
            "-abc",
            "0-abc",
            "042-abc",
            "+42-abc",
            "42-AB",
            "42-a-b",
            "42-a b",
            " 42-abc",
            "42-abc ",
            "42-abc\n",
            "4294967296-abc",
            "x-abc",
            "42-á",
        ] {
            assert_eq!(OwnerToken::parse(bad), None, "debía rechazarse: {bad:?}");
        }
    }

    /// El constructor aplica las mismas reglas que el parseo.
    #[test]
    fn new_rejects_invalid_parts() {
        assert_eq!(OwnerToken::new(0, "a"), None);
        assert_eq!(OwnerToken::new(1, ""), None);
        assert_eq!(OwnerToken::new(1, "A"), None);
        assert_eq!(OwnerToken::new(1, "a.b"), None);
    }
}
