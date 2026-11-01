// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask headers --check`: puerta que exige la cabecera de la MPL-2.0 en todo archivo
//! propio de código o de script.
//!
//! La cabecera canónica tiene cinco líneas, con el prefijo de comentario del lenguaje
//! (`//` en Rust, `#` en shell, PowerShell y bats) y, si el archivo empieza por un
//! shebang, justo después de él. Se rechaza un archivo sin cabecera, uno con una
//! cabecera de otro texto y uno donde la cabecera existe pero no está en su posición.
//! Los `.ps1` son ASCII puro, así que su cabecera escribe el titular sin tilde.
//!
//! Quedan fuera los crates cuyo `Cargo.toml` declara una licencia distinta de MPL-2.0
//! (`crates/denoise`, bajo MIT): su cabecera sería falsa.
//!
//! El control falla si algún archivo es ilegible o si el barrido no encuentra ninguno:
//! un barrido vacío no demuestra nada.

use crate::first_party;
use anyhow::{anyhow, Result};
use std::path::{Path, PathBuf};

/// Líneas de la cabecera canónica sin prefijo de comentario; una cadena vacía es una
/// línea en blanco del bloque (solo lleva el prefijo).
const HEADER: [&str; 5] = [
    "This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.",
    "If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.",
    "",
    "Copyright (c) 2026 Cristián Rojas Arredondo.",
    "SPDX-License-Identifier: MPL-2.0",
];

/// Licencia que declara un crate cuyos archivos llevan esta cabecera.
const LICENSE: &str = "MPL-2.0";

/// Tipos de archivo propio que deben llevar cabecera: código Rust y scripts.
const KINDS: &[&str] = &[
    "rs",
    "sh",
    "bash",
    "bats",
    "ps1",
    first_party::SHEBANG_SCRIPTS,
];

/// Por qué se rechaza un archivo.
#[derive(Debug, PartialEq, Eq)]
enum Problem {
    /// No hay cabecera.
    Missing,
    /// En la posición de la cabecera hay otra cabecera, con distinto texto.
    Different,
    /// La cabecera existe pero no está en la posición exigida.
    Misplaced,
}

impl Problem {
    fn describe(&self) -> &'static str {
        match self {
            Problem::Missing => "sin cabecera",
            Problem::Different => "cabecera con otro texto",
            Problem::Misplaced => "cabecera fuera de posición",
        }
    }
}

/// Prefijo de comentario del archivo según su extensión; los scripts sin extensión
/// (reconocidos por su shebang) comentan con `#`.
fn comment_prefix(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()) {
        Some("rs") => "//",
        _ => "#",
    }
}

/// Si el lenguaje del archivo exige ASCII puro. Los `.ps1` del bootstrap y de su suite lo
/// exigen (PowerShell 5.1 corrompe los bytes no ASCII que entrega `irm`), así que su
/// cabecera lleva el titular sin tilde.
fn requires_ascii(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()) == Some("ps1")
}

/// Cabecera canónica con el prefijo de comentario aplicado, y sin tildes si `ascii`.
fn expected_header(prefix: &str, ascii: bool) -> Vec<String> {
    HEADER
        .iter()
        .map(|line| {
            if line.is_empty() {
                prefix.to_string()
            } else if ascii {
                format!("{prefix} {}", line.replace('á', "a"))
            } else {
                format!("{prefix} {line}")
            }
        })
        .collect()
}

/// Evalúa el contenido de un archivo contra la cabecera de su lenguaje.
fn check_text(text: &str, path: &Path) -> Option<Problem> {
    let prefix = comment_prefix(path);
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let expected = expected_header(prefix, requires_ascii(path));
    let start = usize::from(lines.first().is_some_and(|line| line.starts_with("#!")));
    let at_position = lines.get(start..start + expected.len());
    if at_position.is_some_and(|found| found == expected.as_slice()) {
        // Un shebang detrás de la cabecera deja de ser shebang: la cabecera debe ir después.
        let shebang_below = lines
            .get(start + expected.len())
            .is_some_and(|line| line.starts_with("#!"));
        return shebang_below.then_some(Problem::Misplaced);
    }
    // La primera línea de la cabecera es la que identifica un intento de cabecera.
    let marker = expected[0].as_str();
    if lines
        .get(start)
        .is_some_and(|line| line.starts_with(prefix) && line.contains("Source Code Form"))
    {
        return Some(Problem::Different);
    }
    if lines.contains(&marker) {
        return Some(Problem::Misplaced);
    }
    if lines.iter().any(|line| line.contains("Source Code Form")) {
        return Some(Problem::Different);
    }
    Some(Problem::Missing)
}

/// Licencia que declara el `Cargo.toml` de un crate, si la declara con una cadena.
fn declared_license(manifest: &str) -> Option<String> {
    manifest.lines().find_map(|line| {
        let rest = line.trim().strip_prefix("license")?.trim_start();
        let value = rest.strip_prefix('=')?.trim();
        let value = value.strip_prefix('"')?;
        Some(value[..value.find('"')?].to_string())
    })
}

/// Si el archivo vive en un crate (`crates/<nombre>/...`) con otra licencia.
fn in_other_license_crate(base: &Path, relative: &str) -> Result<bool> {
    let mut parts = relative.split('/');
    if parts.next() != Some("crates") {
        return Ok(false);
    }
    let Some(name) = parts.next() else {
        return Ok(false);
    };
    let manifest = base.join("crates").join(name).join("Cargo.toml");
    if !manifest.is_file() {
        return Ok(false);
    }
    let text = std::fs::read_to_string(&manifest)
        .map_err(|e| anyhow!("no se pudo leer {}: {}", manifest.display(), e))?;
    Ok(declared_license(&text).is_some_and(|license| license != LICENSE))
}

/// Archivos propios que deben llevar cabecera, ordenados y sin los crates de otra
/// licencia.
fn collect_files(base: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for file in first_party::collect(base, KINDS)? {
        let relative = first_party::relative(base, &file);
        if !in_other_license_crate(base, &relative)? {
            files.push(file);
        }
    }
    Ok(files)
}

/// Una infracción: archivo y motivo.
#[derive(Debug)]
struct Hit {
    file: String,
    problem: Problem,
}

/// Evalúa los archivos y devuelve las infracciones en orden de archivo. Un archivo
/// ilegible es un error, no un archivo limpio.
fn scan(base: &Path, files: &[PathBuf]) -> Result<Vec<Hit>> {
    let mut hits = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", file.display(), e))?;
        if let Some(problem) = check_text(&text, file) {
            hits.push(Hit {
                file: first_party::relative(base, file),
                problem,
            });
        }
    }
    Ok(hits)
}

/// Entrada del subcomando `headers`. `root` sobrescribe la raíz a escanear (por
/// defecto, el directorio actual).
pub(crate) fn run(check: bool, root: Option<&Path>) -> Result<()> {
    if !check {
        println!("Usa --check para verificar la cabecera de licencia de los archivos propios");
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let files = collect_files(&base)?;
    if files.is_empty() {
        return Err(anyhow!(
            "no se encontró ningún archivo propio bajo {}",
            base.display()
        ));
    }
    let hits = scan(&base, &files)?;
    if hits.is_empty() {
        println!(
            "0 archivos sin cabecera de licencia en {} archivos propios",
            files.len()
        );
        return Ok(());
    }
    println!("{:<70} MOTIVO", "ARCHIVO");
    for hit in &hits {
        println!("{:<70} {}", hit.file, hit.problem.describe());
    }
    println!("CABECERAS INCORRECTAS: {}", hits.len());
    // Herramienta de desarrollo sin scratch: no hay nada que liberar antes de salir.
    #[allow(clippy::disallowed_methods)]
    std::process::exit(1);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(prefix: &str) -> String {
        expected_header(prefix, false).join("\n")
    }

    /// Cabecera de un `.ps1`: el titular va sin tilde porque el archivo es ASCII puro.
    fn ascii_header() -> String {
        expected_header("#", true).join("\n")
    }

    /// Escribe un árbol de archivos en un temporal con dueño y lo escanea.
    fn scan_tree(files: &[(&str, String)]) -> Vec<Hit> {
        let guard = tempfile::Builder::new()
            .prefix("xtask-headers-")
            .tempdir()
            .unwrap();
        for (rel, body) in files {
            let path = guard.path().join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        let collected = collect_files(guard.path()).unwrap();
        scan(guard.path(), &collected).unwrap()
    }

    #[test]
    fn a_correct_header_is_accepted_in_every_language() {
        let rust = format!("{}\n\nfn a() {{}}\n", header("//"));
        let shell = format!("#!/bin/sh\n{}\necho hola\n", header("#"));
        let powershell = format!("{}\nWrite-Host 1\n", ascii_header());
        let hits = scan_tree(&[
            ("src/a.rs", rust),
            ("tests/run.sh", shell.clone()),
            (".githooks/pre-commit", shell),
            ("tests/x.ps1", powershell),
        ]);
        assert!(hits.is_empty(), "{hits:?}");
    }

    #[test]
    fn a_powershell_header_must_be_ascii() {
        assert!(!ascii_header().contains('á'));
        let accented = format!("{}\nWrite-Host 1\n", header("#"));
        let hits = scan_tree(&[("tests/x.ps1", accented)]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].problem, Problem::Different);
    }

    #[test]
    fn a_file_without_header_is_rejected() {
        let hits = scan_tree(&[("src/a.rs", "fn a() {}\n".to_string())]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].problem, Problem::Missing);
    }

    #[test]
    fn a_header_with_other_text_is_rejected() {
        let wrong = header("//").replace("MPL/2.0/.", "MPL/2.0.");
        let hits = scan_tree(&[("src/a.rs", format!("{wrong}\n"))]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].problem, Problem::Different);
    }

    #[test]
    fn a_header_not_right_after_the_shebang_is_rejected() {
        let shell = format!("#!/bin/sh\nset -e\n{}\n", header("#"));
        let hits = scan_tree(&[("tests/run.sh", shell)]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].problem, Problem::Misplaced);
    }

    #[test]
    fn a_header_above_the_shebang_is_rejected() {
        let shell = format!("{}\n#!/bin/sh\n", header("#"));
        let hits = scan_tree(&[("tests/run.sh", shell)]);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn a_crate_with_another_license_is_excluded() {
        let hits = scan_tree(&[
            (
                "crates/otro/Cargo.toml",
                "[package]\nlicense = \"MIT\"\n".to_string(),
            ),
            ("crates/otro/src/lib.rs", "fn a() {}\n".to_string()),
            (
                "crates/propio/Cargo.toml",
                "[package]\nlicense = \"MPL-2.0\"\n".to_string(),
            ),
            ("crates/propio/src/lib.rs", "fn a() {}\n".to_string()),
        ]);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].file, "crates/propio/src/lib.rs");
    }

    #[test]
    fn an_empty_scan_is_an_error() {
        let guard = tempfile::Builder::new()
            .prefix("xtask-headers-empty-")
            .tempdir()
            .unwrap();
        assert!(run(true, Some(guard.path())).is_err());
    }

    #[test]
    fn the_repository_is_clean() {
        // La garantía que la regla vende, afirmada dentro de la suite para que no
        // dependa de que alguien se acuerde de lanzar la puerta.
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let files = collect_files(&base).unwrap();
        let hits = scan(&base, &files).unwrap();
        assert!(
            hits.is_empty(),
            "archivos propios sin la cabecera canónica: {hits:?}"
        );
    }
}
