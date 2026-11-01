// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Definición única de «archivo propio» para las reglas de política (`comments`,
//! `language` y `headers`).
//!
//! Las tres reglas recorren el mismo conjunto: lo que escribe el proyecto, sin el
//! código de terceros vendorizado ni los directorios de construcción o de producto.
//! Tener un solo recorrido impide que una regla vea archivos que otra ignora.

use anyhow::{anyhow, Result};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Raíces de archivos propios que se recorren. Una raíz que no exista se ignora: el
/// fallo ante un barrido sin archivos lo da cada regla.
const ROOTS: &[&str] = &[
    "src",
    "crates",
    "tests",
    "packaging",
    ".circleci",
    ".githooks",
    ".claude",
];

/// Archivos de configuración que viven en la raíz del repositorio y no bajo ninguna de
/// las raíces anteriores.
const ROOT_FILES: &[&str] = &["Cargo.toml", "rust-toolchain.toml", "opencode.json"];

/// Componentes de ruta que se excluyen: código de terceros vendorizado y directorios de
/// construcción o de producto que no son fuente.
const SKIP_DIRS: &[&str] = &["vendor", "target", "artifacts", ".git"];

/// Marca que, dentro de `kinds`, pide además los scripts sin extensión cuya primera
/// línea empieza por un shebang.
pub(crate) const SHEBANG_SCRIPTS: &str = "#!";

/// Archivos propios bajo `base`, ordenados y sin repetidos.
///
/// `kinds` lista las extensiones que se devuelven (sin punto); si incluye
/// [`SHEBANG_SCRIPTS`], se añaden los archivos sin extensión que empiezan por `#!`.
/// Cualquier ruta con un componente de `SKIP_DIRS` queda fuera. Un directorio o un
/// archivo ilegible es un error, no un archivo ausente.
pub(crate) fn collect(base: &Path, kinds: &[&str]) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in ROOTS {
        let dir = base.join(root);
        if dir.is_dir() {
            walk(&dir, kinds, &mut files)?;
        }
    }
    for name in ROOT_FILES {
        let file = base.join(name);
        if file.is_file() && wanted(&file, kinds)? {
            files.push(file);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Ruta relativa a la raíz con separador `/` en todas las plataformas, para que los
/// informes y las listas blancas se escriban igual en todos los sistemas operativos.
pub(crate) fn relative(base: &Path, file: &Path) -> String {
    file.strip_prefix(base)
        .expect("todo archivo recorrido cuelga de la raíz")
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Recorrido en profundidad con las exclusiones aplicadas.
fn walk(dir: &Path, kinds: &[&str], files: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| anyhow!("no se pudo leer {}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| anyhow!("no se pudo leer una entrada de {}: {}", dir.display(), e))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.iter().any(|skipped| *skipped == name) {
            continue;
        }
        if path.is_dir() {
            walk(&path, kinds, files)?;
        } else if wanted(&path, kinds)? {
            files.push(path);
        }
    }
    Ok(())
}

/// Si el archivo es de alguno de los tipos pedidos: por extensión, o por shebang si no
/// tiene extensión y se pidieron los scripts.
fn wanted(path: &Path, kinds: &[&str]) -> Result<bool> {
    match path.extension().and_then(|e| e.to_str()) {
        Some(extension) => Ok(kinds.contains(&extension)),
        None => {
            if !kinds.contains(&SHEBANG_SCRIPTS) {
                return Ok(false);
            }
            starts_with_shebang(path)
        }
    }
}

/// Si los dos primeros bytes del archivo son `#!`.
fn starts_with_shebang(path: &Path) -> Result<bool> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| anyhow!("no se pudo abrir {}: {}", path.display(), e))?;
    let mut head = [0u8; 2];
    let mut read = 0;
    while read < head.len() {
        let count = file
            .read(&mut head[read..])
            .map_err(|e| anyhow!("no se pudo leer {}: {}", path.display(), e))?;
        if count == 0 {
            break;
        }
        read += count;
    }
    Ok(read == 2 && &head == b"#!")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Escribe un árbol de archivos en un temporal con dueño y lo devuelve.
    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let guard = tempfile::Builder::new()
            .prefix("xtask-first-party-")
            .tempdir()
            .unwrap();
        for (rel, body) in files {
            let path = guard.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        guard
    }

    fn names(base: &Path, kinds: &[&str]) -> Vec<String> {
        collect(base, kinds)
            .unwrap()
            .iter()
            .map(|file| relative(base, file))
            .collect()
    }

    #[test]
    fn third_party_and_build_directories_are_skipped() {
        let dir = tree(&[
            ("src/ok.rs", "fn a() {}\n"),
            ("vendor/third_party.rs", "fn b() {}\n"),
            ("target/build.rs", "fn c() {}\n"),
            ("artifacts/out.rs", "fn d() {}\n"),
            (".git/hook.rs", "fn e() {}\n"),
            ("crates/x/vendor/inner.rs", "fn f() {}\n"),
        ]);
        assert_eq!(names(dir.path(), &["rs"]), ["src/ok.rs"]);
    }

    #[test]
    fn a_script_without_extension_is_recognized_by_its_shebang() {
        let dir = tree(&[
            (".githooks/pre-commit", "#!/bin/sh\necho hola\n"),
            (".githooks/README", "no es un script\n"),
            (".githooks/vacio", ""),
        ]);
        assert_eq!(
            names(dir.path(), &["sh", SHEBANG_SCRIPTS]),
            [".githooks/pre-commit"]
        );
        assert!(names(dir.path(), &["sh"]).is_empty());
    }

    #[test]
    fn the_claude_directory_and_root_files_are_included() {
        let dir = tree(&[
            (".claude/skills/x/scripts/run.ps1", "Write-Host 1\n"),
            ("Cargo.toml", "[package]\n"),
            ("docs/otro.rs", "fn fuera() {}\n"),
        ]);
        assert_eq!(
            names(dir.path(), &["ps1", "toml"]),
            [".claude/skills/x/scripts/run.ps1", "Cargo.toml"]
        );
    }

    #[test]
    fn a_base_under_a_skipped_directory_is_still_scanned() {
        let guard = tempfile::Builder::new()
            .prefix("xtask-first-party-base-")
            .tempdir()
            .unwrap();
        let base = guard.path().join("vendor").join("repo");
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/lib.rs"), "fn english() {}\n").unwrap();
        assert_eq!(names(&base, &["rs"]), ["src/lib.rs"]);
    }
}
