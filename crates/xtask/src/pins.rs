//! `xtask pins --check/--sync`: puerta única de los pines versionados.
//!
//! Los parámetros del pipeline no pueden derivarse de un fichero porque la CI
//! los interpola en claves de caché y etiquetas de imagen antes de ejecutar
//! ningún paso, así que la única invariante exigible es la comparación: cada
//! parámetro pineado espeja su clave de `packaging/pins.json`. Qué parámetros
//! son pines se decide por convención de sufijo (`_pin`), de modo que no hay
//! lista en ningún sitio y añadir un pin es añadir un parámetro.
//!
//! La clave `sccache` es la excepción escrita: no tiene parámetro porque ningún
//! paso lo interpola como tal; los instaladores leen el fichero directamente.
//! Exigirle parámetro contradiría esa decisión, así que el validador la perdona
//! aquí y en ningún otro sitio.
//!
//! Sin dependencias nuevas: el inventario congelado obliga a no añadir crates,
//! y lo que hace falta es lectura de ficheros y comparación de cadenas.

use anyhow::{anyhow, Result};
use avi_shared::pins::{self, Pins};
use std::path::{Path, PathBuf};

/// Sufijo que marca un parámetro del pipeline como pin versionado.
const PIN_SUFFIX: &str = "_pin";

/// Fichero del canal de Rust, relativo a la raíz del repositorio.
const TOOLCHAIN_REL: &str = "rust-toolchain.toml";

/// Ejecuta el subcomando: `--check` verifica y `--sync` regenera el canal.
pub(crate) fn run(check: bool, sync: bool, root: Option<&Path>) -> Result<()> {
    if !check && !sync {
        println!("Usa --check para verificar los pines o --sync para regenerar el canal de Rust");
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let pins = pins::load_from_root(&base).map_err(|e| anyhow!("{e}"))?;
    if sync {
        let out = base.join(TOOLCHAIN_REL);
        std::fs::write(&out, render_toolchain(&pins))
            .map_err(|e| anyhow!("no se pudo escribir {}: {e}", out.display()))?;
        println!(
            "canal de Rust regenerado desde packaging/pins.json: {}",
            pins.rust
        );
    }
    if check {
        let cfg_path = base.join(".circleci/config.yml");
        let cfg = std::fs::read_to_string(&cfg_path)
            .map_err(|e| anyhow!("no se pudo leer {}: {e}", cfg_path.display()))?;
        let toolchain_path = base.join(TOOLCHAIN_REL);
        let toolchain = std::fs::read_to_string(&toolchain_path)
            .map_err(|e| anyhow!("no se pudo leer {}: {e}", toolchain_path.display()))?;
        let errors = check_text(&pins, &cfg, &toolchain);
        if errors.is_empty() {
            println!("pines en sincronía: parámetros, canal de Rust e instaladores");
            return Ok(());
        }
        for error in &errors {
            eprintln!("ERROR: {error}");
        }
        eprintln!("PINES DIVERGENTES: {}", errors.len());
        std::process::exit(1);
    }
    Ok(())
}

/// Las nueve claves con su valor, en el orden del manifiesto.
fn all_pins(pins: &Pins) -> [(&'static str, &String); 10] {
    [
        ("rust", &pins.rust),
        ("ort", &pins.ort),
        ("sccache", &pins.sccache),
        ("msys2_base", &pins.msys2_base),
        ("msys2_gcc", &pins.msys2_gcc),
        ("msys2_openblas", &pins.msys2_openblas),
        ("msys2_make", &pins.msys2_make),
        ("ninja", &pins.ninja),
        ("bats", &pins.bats),
        ("pwsh", &pins.pwsh),
    ]
}

/// Parámetros pineados del texto de la CI: nombre y valor por defecto. Solo el
/// bloque de parámetros del pipeline usa sangría de dos espacios exactos; los
/// parámetros de comandos anidados van más adentro y no se tocan.
fn pin_params(cfg: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = cfg.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.starts_with("  ") || line.starts_with("   ") {
            continue;
        }
        if let Some(name) = line.trim().strip_suffix(':') {
            if !name.ends_with(PIN_SUFFIX) {
                continue;
            }
            for candidate in lines.iter().skip(i + 1).take(6) {
                if let Some(value) = default_of(candidate) {
                    out.push((name.to_string(), value));
                    break;
                }
            }
        }
    }
    out
}

/// Valor `default: "v"` de una línea de parámetro, si lo trae.
fn default_of(line: &str) -> Option<String> {
    line.trim()
        .strip_prefix("default: \"")
        .and_then(|v| v.strip_suffix('"'))
        .map(str::to_string)
}

/// Canal declarado en el texto del fichero del canal, si trae alguno. Solo se
/// mira esa línea: el resto del fichero es formato libre y no es invariante.
fn channel_of(toolchain: &str) -> Option<String> {
    for line in toolchain.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("channel") {
            let value = rest.trim().strip_prefix('=')?.trim();
            if value.len() >= 2 && value.starts_with('"') && value.ends_with('"') {
                return Some(value[1..value.len() - 1].to_string());
            }
        }
    }
    None
}

/// Contenido generado del fichero del canal a partir de los pines.
fn render_toolchain(pins: &Pins) -> String {
    format!(
        "[toolchain]\n\
         # Fichero generado por `cargo xtask pins --sync`: no editar a mano.\n\
         # El canal copia la clave `rust` de `packaging/pins.json` y\n\
         # `cargo xtask pins --check` falla si diverge.\n\
         channel = \"{}\"\n",
        pins.rust
    )
}

/// Verifica los tres textos y devuelve un error por divergencia.
fn check_text(pins: &Pins, cfg: &str, toolchain: &str) -> Vec<String> {
    let mut errors = Vec::new();
    let params = pin_params(cfg);
    for (param, expected) in &params {
        let key = param.strip_suffix(PIN_SUFFIX).unwrap_or(param);
        let found = all_pins(pins)
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v.as_str());
        match found {
            Some(value) if value == expected => {}
            Some(value) => errors.push(format!(
                "pin divergente: {key} (pins.json={value:?}, pipeline={expected:?})"
            )),
            None => errors.push(format!(
                "parámetro {param} sin clave en packaging/pins.json"
            )),
        }
    }
    for (key, _) in all_pins(pins) {
        if key == "sccache" {
            continue;
        }
        if !params
            .iter()
            .any(|(p, _)| p == &format!("{key}{PIN_SUFFIX}"))
        {
            errors.push(format!("clave {key} sin parámetro espejo en la CI"));
        }
    }
    match channel_of(toolchain) {
        Some(channel) if channel == pins.rust => {}
        Some(channel) => errors.push(format!(
            "canal de Rust divergente (rust-toolchain.toml={channel:?}, pins.json={:?})",
            pins.rust
        )),
        None => errors.push("rust-toolchain.toml no declara ningún canal".to_string()),
    }
    if has_shell_literal(cfg, "SCCACHE_VERSION=\"")
        || has_shell_literal(cfg, "$SccacheVersion = \"")
    {
        errors.push(
            "la CI fija sccache en un literal de shell: debe leer packaging/pins.json".to_string(),
        );
    }
    errors
}

/// `true` si el texto fija un literal de versión tras el prefijo: el carácter
/// siguiente es un dígito. Así la lectura viva (`="\$\(...` o `= (Get-...`)
/// no se confunde con el literal que sustituye.
fn has_shell_literal(cfg: &str, prefix: &str) -> bool {
    let mut rest = cfg;
    while let Some(pos) = rest.find(prefix) {
        rest = &rest[pos + prefix.len()..];
        if rest.chars().next().is_some_and(|c| c.is_ascii_digit()) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Pines de juguete (los reales viven en `packaging/pins.json`).
    fn toy_pins() -> Pins {
        Pins {
            rust: "1.96.0".to_string(),
            ort: "1.28.0".to_string(),
            sccache: "0.8.2".to_string(),
            msys2_base: "2026-06-11".to_string(),
            msys2_gcc: "16.2.0".to_string(),
            msys2_openblas: "0.3.34-1".to_string(),
            msys2_make: "4.4.1-5".to_string(),
            ninja: "1.13.2".to_string(),
            bats: "1.14.0".to_string(),
            pwsh: "7.6.6".to_string(),
        }
    }

    /// CI de juguete con los nueve parámetros esperados y sin literales.
    fn toy_config() -> String {
        let pins = toy_pins();
        let mut cfg = String::from("parameters:\n");
        for (key, value) in all_pins(&pins) {
            if key == "sccache" {
                continue;
            }
            cfg.push_str(&format!(
                "  {key}_pin:\n    type: string\n    default: \"{value}\"\n"
            ));
        }
        cfg
    }

    /// Canal de juguete que copia el pin de Rust.
    fn toy_toolchain() -> String {
        render_toolchain(&toy_pins())
    }

    /// Directorio temporal propio de cada prueba: dos pruebas con el mismo
    /// nombre se pisarían si corrieran en el mismo proceso en paralelo.
    fn own_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask_pins_{name}_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn clean_tree_passes() {
        assert!(check_text(&toy_pins(), &toy_config(), &toy_toolchain()).is_empty());
    }

    #[test]
    fn param_without_key_is_rejected() {
        let mut cfg = toy_config();
        cfg.push_str("  unknown_pin:\n    type: string\n    default: \"9.9.9\"\n");
        let errors = check_text(&toy_pins(), &cfg, &toy_toolchain());
        assert!(errors.iter().any(|e| e.contains("unknown_pin")));
    }

    #[test]
    fn divergent_value_is_rejected() {
        let cfg = toy_config().replace("1.96.0", "1.95.0");
        let errors = check_text(&toy_pins(), &cfg, &toy_toolchain());
        assert!(errors.iter().any(|e| e.contains("pin divergente: rust")));
    }

    #[test]
    fn key_without_param_is_rejected() {
        let cfg = toy_config().replace(
            "  ninja_pin:\n    type: string\n    default: \"1.13.2\"\n",
            "",
        );
        let errors = check_text(&toy_pins(), &cfg, &toy_toolchain());
        assert!(errors
            .iter()
            .any(|e| e.contains("ninja") && e.contains("sin parámetro")));
    }

    #[test]
    fn sccache_without_param_is_accepted() {
        assert!(!toy_config().contains("sccache_pin"));
        assert!(check_text(&toy_pins(), &toy_config(), &toy_toolchain()).is_empty());
    }

    #[test]
    fn divergent_channel_is_rejected() {
        let toolchain = toy_toolchain().replace("1.96.0", "1.95.0");
        let errors = check_text(&toy_pins(), &toy_config(), &toolchain);
        assert!(errors
            .iter()
            .any(|e| e.contains("canal de Rust divergente")));
    }

    #[test]
    fn sccache_literal_is_rejected() {
        let mut cfg = toy_config();
        cfg.push_str("            SCCACHE_VERSION=\"0.8.2\"\n");
        let errors = check_text(&toy_pins(), &cfg, &toy_toolchain());
        assert!(errors.iter().any(|e| e.contains("literal de shell")));
    }

    #[test]
    fn live_sccache_read_is_accepted() {
        let mut cfg = toy_config();
        cfg.push_str("            SCCACHE_VERSION=\"$(python3 -c 'pass')\"\n");
        cfg.push_str("            $SccacheVersion = (Get-Content packaging/pins.json -Raw | ConvertFrom-Json).sccache\n");
        assert!(check_text(&toy_pins(), &cfg, &toy_toolchain()).is_empty());
    }

    #[test]
    fn sync_writes_channel_from_pins() {
        let dir = own_dir("sync");
        std::fs::create_dir_all(dir.join("packaging")).unwrap();
        std::fs::write(
            dir.join("packaging/pins.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "rust": "1.96.0", "ort": "1.28.0", "sccache": "0.8.2",
                "msys2_base": "2026-06-11", "msys2_gcc": "16.2.0",
                "msys2_openblas": "0.3.34-1", "msys2_make": "4.4.1-5",
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6",
            }))
            .unwrap(),
        )
        .unwrap();
        run(false, true, Some(&dir)).unwrap();
        let written = std::fs::read_to_string(dir.join("rust-toolchain.toml")).unwrap();
        assert_eq!(channel_of(&written).as_deref(), Some("1.96.0"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn check_reads_tree_from_disk() {
        let dir = own_dir("check");
        std::fs::create_dir_all(dir.join("packaging")).unwrap();
        std::fs::create_dir_all(dir.join(".circleci")).unwrap();
        std::fs::write(
            dir.join("packaging/pins.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "rust": "1.96.0", "ort": "1.28.0", "sccache": "0.8.2",
                "msys2_base": "2026-06-11", "msys2_gcc": "16.2.0",
                "msys2_openblas": "0.3.34-1", "msys2_make": "4.4.1-5",
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(".circleci/config.yml"), toy_config()).unwrap();
        std::fs::write(dir.join("rust-toolchain.toml"), toy_toolchain()).unwrap();
        run(true, false, Some(&dir)).unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }
}
