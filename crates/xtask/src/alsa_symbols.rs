//! `xtask alsa-symbols [--check]`: genera o verifica la lista de funciones de
//! ALSA que `alsa_sys::availability()` resuelve antes de dar el audio por
//! disponible (`vendor/alsa-sys-0.4.0/src/required_symbols.rs`).
//!
//! La lista sale de las fuentes de la versión del crate `alsa` fijada en
//! `Cargo.lock`, el único consumidor de esos bindings. Con comentarios y
//! literales apagados, cuenta como llamada a `alsa-sys` cada `<alias>::snd_*`,
//! donde el alias es el de `extern crate alsa_sys as <alias>;`, y cada primer
//! argumento `snd_*` de una macro cuyo cuerpo reenvía por ese alias
//! (`<alias>::$f`). De esos nombres se quedan los que `src/generated.rs`
//! declara como función. Con `--check` falla si el fichero no coincide, de modo
//! que actualizar `alsa` sin regenerar la lista no pasa desapercibido.

use crate::language::strip_code;
use anyhow::{anyhow, bail, Result};
use regex::Regex;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Bindings de ALSA, relativos a la raíz del repositorio.
const BINDINGS: &str = "vendor/alsa-sys-0.4.0/src/generated.rs";

/// Fichero generado, relativo a la raíz del repositorio.
const OUTPUT: &str = "vendor/alsa-sys-0.4.0/src/required_symbols.rs";

pub(crate) fn run(check: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    let (version, source_dir) = locked_alsa_source()?;
    let mut files = Vec::new();
    collect_rust_files(&source_dir, &mut files)?;
    let sources = files
        .iter()
        .map(|f| {
            std::fs::read_to_string(f).map_err(|e| anyhow!("no se pudo leer {}: {e}", f.display()))
        })
        .collect::<Result<Vec<_>>>()?;
    let bindings = std::fs::read_to_string(root.join(BINDINGS))?;
    let symbols = required_symbols(&sources, &bindings)?;
    let rendered = render(&version, &symbols);
    let path = root.join(OUTPUT);
    if check {
        let current = std::fs::read_to_string(&path)?.replace("\r\n", "\n");
        if current != rendered {
            let listed = listed_symbols(&current);
            let added: Vec<_> = symbols.difference(&listed).collect();
            let removed: Vec<_> = listed.difference(&symbols).collect();
            bail!(
                "{OUTPUT} no coincide con alsa {version} (faltan {added:?}; sobran {removed:?}): ejecuta `cargo xtask alsa-symbols` y revisa el diff"
            );
        }
        println!(
            "{OUTPUT} en sincronía con alsa {version}: {} funciones",
            symbols.len()
        );
    } else {
        std::fs::write(&path, rendered)?;
        println!(
            "{OUTPUT} regenerado desde alsa {version}: {} funciones",
            symbols.len()
        );
    }
    Ok(())
}

/// Versión y directorio `src` del único paquete `alsa` de `cargo metadata`.
fn locked_alsa_source() -> Result<(String, PathBuf)> {
    let out = std::process::Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--locked"])
        .output()
        .map_err(|e| anyhow!("no se pudo invocar cargo metadata: {e}"))?;
    if !out.status.success() {
        bail!(
            "cargo metadata falló: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let alsa: Vec<&serde_json::Value> = json["packages"]
        .as_array()
        .ok_or_else(|| anyhow!("cargo metadata no devolvió `packages`"))?
        .iter()
        .filter(|p| p["name"] == "alsa")
        .collect();
    let [package] = alsa.as_slice() else {
        bail!(
            "se esperaba un único paquete `alsa` en Cargo.lock y hay {}",
            alsa.len()
        );
    };
    let version = package["version"]
        .as_str()
        .ok_or_else(|| anyhow!("el paquete `alsa` no tiene versión"))?;
    let manifest = package["manifest_path"]
        .as_str()
        .ok_or_else(|| anyhow!("el paquete `alsa` no tiene manifest_path"))?;
    let src = Path::new(manifest)
        .parent()
        .ok_or_else(|| anyhow!("manifest_path sin directorio: {manifest}"))?
        .join("src");
    Ok((version.to_string(), src))
}

/// Ficheros `.rs` bajo `dir`, en orden estable.
fn collect_rust_files(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let mut entries = std::fs::read_dir(dir)
        .map_err(|e| anyhow!("no se pudo listar {}: {e}", dir.display()))?
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|e| e.path());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_rust_files(&path, files)?;
        } else if path.extension().is_some_and(|e| e == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// Funciones de `bindings` que las fuentes de `alsa` llaman a través de
/// `alsa-sys`.
fn required_symbols(sources: &[String], bindings: &str) -> Result<BTreeSet<String>> {
    let code: Vec<String> = sources.iter().map(|s| strip_code(s)).collect();
    let alias_re = Regex::new(r"extern\s+crate\s+alsa_sys\s+as\s+(\w+)\s*;").unwrap();
    let alias = code
        .iter()
        .find_map(|c| alias_re.captures(c).map(|m| m[1].to_string()))
        .ok_or_else(|| anyhow!("las fuentes de `alsa` no importan `alsa_sys` con un alias"))?;
    let alias = regex::escape(&alias);
    let qualified = Regex::new(&format!(r"\b{alias}::(snd_\w+)")).unwrap();
    let mut called = BTreeSet::new();
    for c in &code {
        called.extend(qualified.captures_iter(c).map(|m| m[1].to_string()));
    }
    for name in forwarding_macros(&code, &alias) {
        let invocation = Regex::new(&format!(r"\b{name}!\s*\(\s*(snd_\w+)")).unwrap();
        for c in &code {
            called.extend(invocation.captures_iter(c).map(|m| m[1].to_string()));
        }
    }
    let declared_re = Regex::new(r"\bpub fn (snd_\w+)\s*\(").unwrap();
    let declared: BTreeSet<String> = declared_re
        .captures_iter(bindings)
        .map(|m| m[1].to_string())
        .collect();
    let symbols: BTreeSet<String> = called.intersection(&declared).cloned().collect();
    if symbols.is_empty() {
        bail!("no se encontró ninguna llamada de `alsa` a funciones de {BINDINGS}");
    }
    Ok(symbols)
}

/// Nombres de las macros `macro_rules!` cuyo cuerpo reenvía su argumento por
/// el alias de `alsa-sys` (`<alias>::$f`).
fn forwarding_macros(code: &[String], alias: &str) -> Vec<String> {
    let definition = Regex::new(r"macro_rules!\s*(\w+)\s*\{").unwrap();
    let forward = Regex::new(&format!(r"\b{alias}::\$\w+")).unwrap();
    let mut names = Vec::new();
    for c in code {
        for m in definition.captures_iter(c) {
            let open = m.get(0).unwrap().end() - 1;
            if forward.is_match(&c[open..block_end(c, open)]) {
                names.push(m[1].to_string());
            }
        }
    }
    names
}

/// Índice del `}` que cierra el `{` de `open` (o el final del texto).
fn block_end(text: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (i, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return open + i + 1;
                }
            }
            _ => {}
        }
    }
    text.len()
}

/// Contenido de `required_symbols.rs` para `symbols` de `alsa` `version`.
fn render(version: &str, symbols: &BTreeSet<String>) -> String {
    let mut out = format!(
        "// Patch: funciones de ALSA que llama el crate `alsa` {version}, el único\n\
         // consumidor de estos bindings (vía `cpal`). `availability()` las resuelve\n\
         // todas antes de dar el audio por disponible, de modo que a una ALSA a la\n\
         // que le falte alguna se le responde con un error y no con un pánico en\n\
         // mitad de una orden de audio.\n\
         //\n\
         // Fichero generado por `cargo xtask alsa-symbols` desde las fuentes de esa\n\
         // versión de `alsa`; `cargo xtask alsa-symbols --check` falla si no coincide.\n\
         \n\
         pub(crate) const REQUIRED_SYMBOLS: &[&core::ffi::CStr] = &[\n"
    );
    for symbol in symbols {
        out.push_str(&format!("    c\"{symbol}\",\n"));
    }
    out.push_str("];\n");
    out
}

/// Nombres de los literales `c"…",` de un `required_symbols.rs` existente.
fn listed_symbols(text: &str) -> BTreeSet<String> {
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("c\"")?.strip_suffix("\","))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const BINDINGS_FIXTURE: &str = "unsafe extern \"C\" {\n    pub fn snd_a(x: i32) -> i32;\n}\n\
                                    unsafe extern \"C\" {\n    pub fn snd_b() -> i32;\n}\n\
                                    unsafe extern \"C\" {\n    pub fn snd_c() -> i32;\n}\n\
                                    pub struct snd_t;\n";

    /// Cuentan las llamadas por el alias y las que pasan por una macro que
    /// reenvía por él; no cuentan los comentarios, los literales, los tipos ni
    /// las llamadas a otro crate.
    #[test]
    fn collects_qualified_and_forwarded_calls_only() {
        let lib = "extern crate alsa_sys as al;\n\
                   macro_rules! check { ($f: ident ( $($x: expr),* )) => { al::$f($($x),*) } }\n\
                   fn f() { unsafe { al::snd_a(1) }; check!(snd_b()); }\n\
                   // al::snd_c()\n\
                   fn g() -> &'static str { \"al::snd_c\" }\n\
                   fn h(_: *mut al::snd_t) { other::snd_c(); }\n"
            .to_string();
        let symbols = required_symbols(&[lib], BINDINGS_FIXTURE).unwrap();
        assert_eq!(symbols.into_iter().collect::<Vec<_>>(), ["snd_a", "snd_b"]);
    }

    /// Sin el alias de `alsa_sys` la extracción falla en vez de devolver una
    /// lista vacía.
    #[test]
    fn missing_alias_is_an_error() {
        let lib = "fn f() { alsa::snd_a(1); }\n".to_string();
        assert!(required_symbols(&[lib], BINDINGS_FIXTURE).is_err());
    }

    /// El fichero generado se vuelve a leer con los mismos nombres.
    #[test]
    fn render_round_trips_through_listed_symbols() {
        let symbols: BTreeSet<String> = ["snd_a", "snd_b"].map(str::to_string).into();
        assert_eq!(listed_symbols(&render("0.11.0", &symbols)), symbols);
    }
}
