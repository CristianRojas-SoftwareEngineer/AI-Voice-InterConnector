//! `xtask alsa-symbols [--check] --binary <ruta>`: genera o verifica la lista de
//! funciones de ALSA que `alsa_sys::availability()` resuelve antes de dar el
//! audio por disponible (`vendor/alsa-sys-0.4.0/src/required_symbols.rs`).
//!
//! La lista sale de la tabla de símbolos de un binario Linux ya enlazado. Cada
//! envoltura de `alsa-sys` guarda su dirección en un estático con el nombre de
//! símbolo estable `avi_alsa_fn_<función>`; rustc emite cada estático en su
//! propia sección y el enlazador descarta las de las envolturas que nadie
//! alcanza, así que los símbolos presentes son exactamente las funciones que el
//! binario puede llamar. Con `--check` se exige igualdad exacta con el fichero.

use anyhow::{anyhow, bail, Result};
use object::{Object, ObjectSymbol};
use std::collections::BTreeSet;
use std::path::Path;

/// Prefijo del nombre de símbolo de cada envoltura; debe coincidir con el
/// `export_name` que genera `vendor/alsa-sys-0.4.0/build.rs`.
pub(crate) const SYMBOL_PREFIX: &str = "avi_alsa_fn_";

/// Fichero generado, relativo a la raíz del repositorio.
const OUTPUT: &str = "vendor/alsa-sys-0.4.0/src/required_symbols.rs";

pub(crate) fn run(check: bool, binary: &Path) -> Result<()> {
    let root = std::env::current_dir()?;
    let symbols = binary_symbols(binary)?;
    let rendered = render(&symbols);
    let path = root.join(OUTPUT);
    if check {
        compare(&path, binary, &symbols, &rendered)?;
        println!(
            "{OUTPUT} coincide con {}: {} funciones",
            binary.display(),
            symbols.len()
        );
    } else {
        std::fs::write(&path, rendered)?;
        println!(
            "{OUTPUT} regenerado desde {}: {} funciones",
            binary.display(),
            symbols.len()
        );
    }
    Ok(())
}

/// Exige que la lista del repositorio coincida con la del binario `binary`.
pub(crate) fn check_binary(root: &Path, binary: &Path) -> Result<()> {
    let symbols = binary_symbols(binary)?;
    let rendered = render(&symbols);
    compare(&root.join(OUTPUT), binary, &symbols, &rendered)?;
    println!(
        "{OUTPUT} coincide con {}: {} funciones",
        binary.display(),
        symbols.len()
    );
    Ok(())
}

/// Falla, listando lo que falta y lo que sobra, si `path` no es `rendered`.
fn compare(path: &Path, binary: &Path, symbols: &BTreeSet<String>, rendered: &str) -> Result<()> {
    let current = std::fs::read_to_string(path)
        .map_err(|e| anyhow!("no se pudo leer {}: {e}", path.display()))?
        .replace("\r\n", "\n");
    if current == rendered {
        return Ok(());
    }
    let listed = listed_symbols(&current);
    let missing: Vec<_> = symbols.difference(&listed).collect();
    let extra: Vec<_> = listed.difference(symbols).collect();
    bail!(
        "{OUTPUT} no coincide con {} (faltan {missing:?}; sobran {extra:?}): ejecuta `cargo xtask alsa-symbols --binary <binario Linux>` y revisa el diff",
        binary.display()
    )
}

/// Funciones de ALSA que alcanza el ELF `binary`; falla si no es un ELF o no
/// tiene ninguna.
fn binary_symbols(binary: &Path) -> Result<BTreeSet<String>> {
    let data =
        std::fs::read(binary).map_err(|e| anyhow!("no se pudo leer {}: {e}", binary.display()))?;
    let file = object::File::parse(&*data)
        .map_err(|e| anyhow!("{} no es un binario válido: {e}", binary.display()))?;
    if file.format() != object::BinaryFormat::Elf {
        bail!("{} no es un ELF", binary.display());
    }
    let names = file
        .symbols()
        .filter_map(|s| s.name().ok().map(str::to_string))
        .collect::<Vec<_>>();
    let functions = functions_from_symbols(names.iter().map(String::as_str));
    if functions.is_empty() {
        bail!(
            "{} no tiene ningún símbolo {SYMBOL_PREFIX}*: ¿está sin símbolos o no enlaza alsa-sys?",
            binary.display()
        );
    }
    Ok(functions)
}

/// Funciones (sin el prefijo) de los nombres de símbolo que lo llevan.
fn functions_from_symbols<'a>(names: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
    names
        .filter_map(|n| n.strip_prefix(SYMBOL_PREFIX))
        .filter(|f| !f.is_empty())
        .map(str::to_string)
        .collect()
}

/// Si `alsa-sys` está en el grafo de dependencias del target `triple`, según
/// cargo y no según el nombre del sistema.
pub(crate) fn linked_in(root: &Path, triple: &str) -> Result<bool> {
    let out = std::process::Command::new("cargo")
        .args([
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--all-features",
            "--filter-platform",
            triple,
        ])
        .current_dir(root)
        .output()
        .map_err(|e| anyhow!("no se pudo invocar cargo metadata: {e}"))?;
    if !out.status.success() {
        bail!(
            "cargo metadata falló: {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    let id = json["packages"]
        .as_array()
        .ok_or_else(|| anyhow!("cargo metadata no devolvió `packages`"))?
        .iter()
        .find(|p| p["name"] == "alsa-sys")
        .and_then(|p| p["id"].as_str());
    let Some(id) = id else {
        return Ok(false);
    };
    Ok(json["resolve"]["nodes"]
        .as_array()
        .ok_or_else(|| anyhow!("cargo metadata no devolvió `resolve.nodes`"))?
        .iter()
        .any(|n| n["id"] == id))
}

/// Contenido de `required_symbols.rs` para `symbols`.
fn render(symbols: &BTreeSet<String>) -> String {
    let mut out = String::from(
        "// Patch: funciones de ALSA que alcanza el binario del producto. `availability()`\n\
         // las resuelve todas antes de dar el audio por disponible, de modo que a una\n\
         // ALSA a la que le falte alguna se le responde con un error y no con un pánico\n\
         // en mitad de una orden de audio.\n\
         //\n\
         // Fichero generado por `cargo xtask alsa-symbols --binary <binario Linux>` desde\n\
         // la tabla de símbolos del binario enlazado; `--check` exige igualdad exacta.\n\
         \n\
         pub(crate) const REQUIRED_SYMBOLS: &[&core::ffi::CStr] = &[\n",
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

    /// Solo cuentan los símbolos con el prefijo, y se devuelven sin él.
    #[test]
    fn keeps_only_prefixed_symbols() {
        let names = [
            "main",
            "avi_alsa_fn_snd_pcm_open",
            "avi_alsa_fn_snd_card_next",
            "avi_alsa_fn_",
            "snd_pcm_open",
            "x_avi_alsa_fn_snd_a",
        ];
        let functions = functions_from_symbols(names.into_iter());
        assert_eq!(
            functions.into_iter().collect::<Vec<_>>(),
            ["snd_card_next", "snd_pcm_open"]
        );
    }

    /// El fichero generado se vuelve a leer con los mismos nombres.
    #[test]
    fn render_round_trips_through_listed_symbols() {
        let symbols: BTreeSet<String> = ["snd_a", "snd_b"].map(str::to_string).into();
        assert_eq!(listed_symbols(&render(&symbols)), symbols);
    }
}
