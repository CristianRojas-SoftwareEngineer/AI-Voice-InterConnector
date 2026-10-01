//! Regla de clases de pruebas: ninguna prueba se omite en silencio.
//!
//! Una prueba que requiere un recurso externo (modelos, red, binarios) se marca
//! `#[ignore = "requiere …"]` y, cuando se ejecuta, falla con `expect` si el
//! recurso falta. Queda prohibido el patrón de imprimir un aviso de omisión por
//! stderr y retornar en verde, porque esconde pruebas que no corrieron.
//!
//! Esta prueba recorre las fuentes Rust y falla listando cada sitio donde un
//! `eprintln!` contiene el marcador de omisión. Los patrones se componen por
//! partes para que este archivo no se detecte a sí mismo.

use std::path::{Path, PathBuf};

/// Reúne recursivamente los `.rs` bajo `dir`.
fn collect_rs(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rs(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn no_skip_sites_remain() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut roots = vec![root.join("src"), root.join("tests")];
    if let Ok(crates) = std::fs::read_dir(root.join("crates")) {
        for krate in crates.flatten() {
            roots.push(krate.path().join("src"));
            roots.push(krate.path().join("tests"));
        }
    }
    let mut files = Vec::new();
    for dir in &roots {
        collect_rs(dir, &mut files);
    }
    files.sort();

    let print_macro = concat!("eprint", "ln!");
    let skip_marker = concat!("ski", "p:");
    let mut sites = Vec::new();
    for file in &files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        let lines: Vec<&str> = content.lines().collect();
        for (idx, line) in lines.iter().enumerate() {
            if line.trim_start().starts_with("//") {
                continue;
            }
            let Some(pos) = line.find(print_macro) else {
                continue;
            };
            // Texto desde la macro hasta su `;`, que puede abarcar varias líneas.
            let mut span = String::from(&line[pos..]);
            let mut next = idx + 1;
            while !span.contains(';') && next < lines.len() {
                span.push('\n');
                span.push_str(lines[next]);
                next += 1;
            }
            if span.contains(skip_marker) {
                let rel = file.strip_prefix(&root).unwrap_or(file);
                sites.push(format!("{}:{}", rel.display(), idx + 1));
            }
        }
    }
    assert!(
        sites.is_empty(),
        "{} sitios omiten pruebas en silencio:\n{}",
        sites.len(),
        sites.join("\n")
    );
}
