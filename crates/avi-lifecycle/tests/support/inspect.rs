// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Inspección del disco del sandbox. Lo declaran las pruebas que afirman sobre el
//! estado del disco con `#[path = "support/inspect.rs"] mod inspect;`.

use crate::support::Sandbox;
use std::path::Path;

impl Sandbox {
    /// Estado del sandbox como `(ruta relativa, tamaño)`, para afirmar que una
    /// operación no ha modificado el disco.
    pub fn snapshot(&self) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![self.root.clone()];
        while let Some(dir) = stack.pop() {
            for entry in std::fs::read_dir(&dir)
                .unwrap_or_else(|e| panic!("se lista {}: {e}", dir.display()))
            {
                let entry = entry.expect("se lee una entrada del sandbox");
                let path = entry.path();
                let meta = entry
                    .metadata()
                    .unwrap_or_else(|e| panic!("se leen los metadatos de {}: {e}", path.display()));
                let relative = path
                    .strip_prefix(&self.root)
                    .expect("toda entrada cuelga de la raíz del sandbox")
                    .to_string_lossy()
                    .replace('\\', "/");
                if meta.is_dir() {
                    out.push((relative, 0));
                    stack.push(path);
                } else {
                    out.push((relative, meta.len()));
                }
            }
        }
        out.sort();
        out
    }
}

/// `true` si la ruta existe, incluido como enlace roto.
pub fn exists(path: &Path) -> bool {
    path.exists() || path.symlink_metadata().is_ok()
}
