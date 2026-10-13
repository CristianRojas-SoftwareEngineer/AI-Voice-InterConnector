// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Listado de directorios. Lo declaran las pruebas que comparan el contenido de un
//! directorio con `#[path = "support/listing.rs"] mod listing;`.

use std::path::Path;

/// Contenido de un directorio, con `/` al final en los subdirectorios. Un directorio
/// inexistente da una lista vacía, que es el residuo cero; cualquier otro fallo de
/// lectura detiene la prueba.
pub fn list(root: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => panic!("se lista {}: {e}", dir.display()),
        };
        for entry in entries {
            let entry = entry.expect("se lee una entrada del directorio");
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("toda entrada cuelga de la raíz listada")
                .to_string_lossy()
                .replace('\\', "/");
            let kind = entry
                .file_type()
                .unwrap_or_else(|e| panic!("se lee el tipo de {}: {e}", path.display()));
            if kind.is_dir() {
                out.push(format!("{relative}/"));
                stack.push(path);
            } else {
                out.push(relative);
            }
        }
    }
    out.sort();
    out
}
