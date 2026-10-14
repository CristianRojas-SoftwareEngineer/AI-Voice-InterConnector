// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

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
//! aquí y en ningún otro sitio. `ort_sha256` no es una versión sino la tabla de
//! hashes que verifica `xtask package`, y ningún paso de la CI la interpola.
//!
//! La misma puerta compara el otro literal de la CI que espeja un valor del
//! código: la `schema_version` que el humo de `build-windows-x64` exige a
//! `version --json` debe ser `CLI_SCHEMA_VERSION`, de modo que subir la versión
//! del envelope sin tocar el humo falla aquí y no al publicar un tag.
//!
//! Sin dependencias nuevas: basta con leer ficheros y comparar cadenas.

use anyhow::{anyhow, Result};
use avi_shared::pins::{self, Pins};
use std::path::{Path, PathBuf};

/// Sufijo que marca un parámetro del pipeline como pin versionado.
const PIN_SUFFIX: &str = "_pin";

/// Fichero del canal de Rust, relativo a la raíz del repositorio.
const TOOLCHAIN_REL: &str = "rust-toolchain.toml";

/// Prefijos que preceden, en el humo de la CI, al literal de `schema_version`
/// que se exige a `version --json`: la comparación y su mensaje de error.
const SCHEMA_SMOKE_MARKERS: [&str; 2] = ["$j.schema_version -ne '", "campo 'schema_version' != '"];

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
            println!(
                "pines en sincronía: parámetros, canal de Rust, instaladores y schema_version del humo"
            );
            return Ok(());
        }
        for error in &errors {
            eprintln!("ERROR: {error}");
        }
        eprintln!("PINES DIVERGENTES: {}", errors.len());
        // Herramienta de desarrollo sin scratch: no hay nada que liberar antes de salir.
        #[allow(clippy::disallowed_methods)]
        std::process::exit(1);
    }
    Ok(())
}

/// Las doce claves con su valor, en el orden del manifiesto.
fn all_pins(pins: &Pins) -> [(&'static str, &String); 12] {
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
        ("pester", &pins.pester),
        ("nextest", &pins.nextest),
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
        let key = param
            .strip_suffix(PIN_SUFFIX)
            .expect("`pin_params` solo devuelve parámetros con el sufijo de pin");
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
    errors.extend(schema_smoke_errors(
        cfg,
        avi_core::json_emitter::CLI_SCHEMA_VERSION,
    ));
    errors
}

/// Errores del literal de `schema_version` del humo: debe aparecer en cada
/// prefijo de `SCHEMA_SMOKE_MARKERS` y coincidir siempre con `expected`.
fn schema_smoke_errors(cfg: &str, expected: &str) -> Vec<String> {
    let mut errors = Vec::new();
    for marker in SCHEMA_SMOKE_MARKERS {
        let literals: Vec<&str> = cfg
            .match_indices(marker)
            .filter_map(|(pos, _)| cfg[pos + marker.len()..].split('\'').next())
            .collect();
        if literals.is_empty() {
            errors.push(format!(
                "el humo de la CI no contiene la comprobación de schema_version (`{marker}`)"
            ));
        }
        for literal in literals {
            if literal != expected {
                errors.push(format!(
                    "el humo de la CI exige schema_version {literal:?} y el envelope de la CLI es {expected:?} (CLI_SCHEMA_VERSION)"
                ));
            }
        }
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

    /// Pines de fixture (los reales viven en `packaging/pins.json`).
    fn fixture_pins() -> Pins {
        Pins {
            rust: "1.96.0".to_string(),
            ort: "1.28.0".to_string(),
            ort_sha256: std::collections::BTreeMap::new(),
            sccache: "0.8.2".to_string(),
            msys2_base: "2026-06-11".to_string(),
            msys2_gcc: "16.2.0".to_string(),
            msys2_openblas: "0.3.34-1".to_string(),
            msys2_make: "4.4.1-5".to_string(),
            ninja: "1.13.2".to_string(),
            bats: "1.14.0".to_string(),
            pwsh: "7.6.6".to_string(),
            pester: "5.8.0".to_string(),
            nextest: "0.9.148".to_string(),
        }
    }

    /// CI de fixture con los once parámetros esperados, sin literales de
    /// versión y con el humo de `schema_version` en la versión vigente.
    fn fixture_config() -> String {
        let pins = fixture_pins();
        let mut cfg = String::from("parameters:\n");
        for (key, value) in all_pins(&pins) {
            if key == "sccache" {
                continue;
            }
            cfg.push_str(&format!(
                "  {key}_pin:\n    type: string\n    default: \"{value}\"\n"
            ));
        }
        cfg.push_str(&fixture_schema_smoke(
            avi_core::json_emitter::CLI_SCHEMA_VERSION,
        ));
        cfg
    }

    /// Línea del humo de Windows que exige `version` a `version --json`.
    fn fixture_schema_smoke(version: &str) -> String {
        format!(
            "            if (-not $j.PSObject.Properties['schema_version'] -or $j.schema_version -ne '{version}') {{ throw \"version --json: campo 'schema_version' != '{version}': $jsonRaw\" }}\n"
        )
    }

    /// Canal de fixture que copia el pin de Rust.
    fn fixture_toolchain() -> String {
        render_toolchain(&fixture_pins())
    }

    /// Directorio temporal propio de cada prueba, con dueño: se borra al soltarse,
    /// también si la prueba falla.
    fn own_dir(name: &str) -> tempfile::TempDir {
        tempfile::Builder::new()
            .prefix(&format!("xtask_pins_{name}_"))
            .tempdir()
            .unwrap()
    }

    #[test]
    fn clean_tree_passes() {
        assert!(check_text(&fixture_pins(), &fixture_config(), &fixture_toolchain()).is_empty());
    }

    #[test]
    fn param_without_key_is_rejected() {
        let mut cfg = fixture_config();
        cfg.push_str("  unknown_pin:\n    type: string\n    default: \"9.9.9\"\n");
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert!(errors.iter().any(|e| e.contains("unknown_pin")));
    }

    #[test]
    fn divergent_value_is_rejected() {
        let cfg = fixture_config().replace("1.96.0", "1.95.0");
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert!(errors.iter().any(|e| e.contains("pin divergente: rust")));
    }

    #[test]
    fn key_without_param_is_rejected() {
        let cfg = fixture_config().replace(
            "  ninja_pin:\n    type: string\n    default: \"1.13.2\"\n",
            "",
        );
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert!(errors
            .iter()
            .any(|e| e.contains("ninja") && e.contains("sin parámetro")));
    }

    #[test]
    fn sccache_without_param_is_accepted() {
        assert!(!fixture_config().contains("sccache_pin"));
        assert!(check_text(&fixture_pins(), &fixture_config(), &fixture_toolchain()).is_empty());
    }

    #[test]
    fn divergent_channel_is_rejected() {
        let toolchain = fixture_toolchain().replace("1.96.0", "1.95.0");
        let errors = check_text(&fixture_pins(), &fixture_config(), &toolchain);
        assert!(errors
            .iter()
            .any(|e| e.contains("canal de Rust divergente")));
    }

    #[test]
    fn sccache_literal_is_rejected() {
        let mut cfg = fixture_config();
        cfg.push_str("            SCCACHE_VERSION=\"0.8.2\"\n");
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert!(errors.iter().any(|e| e.contains("literal de shell")));
    }

    #[test]
    fn live_sccache_read_is_accepted() {
        let mut cfg = fixture_config();
        cfg.push_str(
            "            SCCACHE_VERSION=\"$(sed -n 's/.*\"sccache\": *\"\\([^\"]*\\)\".*/\\1/p' packaging/pins.json)\"\n",
        );
        cfg.push_str("            $SccacheVersion = (Get-Content packaging/pins.json -Raw | ConvertFrom-Json).sccache\n");
        assert!(check_text(&fixture_pins(), &cfg, &fixture_toolchain()).is_empty());
    }

    /// Un humo que exige otra `schema_version` que la del envelope vigente se
    /// rechaza, también si solo diverge el mensaje de error.
    #[test]
    fn stale_schema_smoke_is_rejected() {
        let current = avi_core::json_emitter::CLI_SCHEMA_VERSION;
        let cfg =
            fixture_config().replace(&fixture_schema_smoke(current), &fixture_schema_smoke("0"));
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert_eq!(errors.len(), 2, "comparación y mensaje: {errors:?}");
        assert!(errors
            .iter()
            .all(|e| e.contains("\"0\"") && e.contains(current)));

        let half = fixture_config().replacen(&format!("-ne '{current}'"), "-ne '0'", 1);
        let errors = check_text(&fixture_pins(), &half, &fixture_toolchain());
        assert_eq!(errors.len(), 1, "solo diverge la comparación: {errors:?}");
    }

    /// Un pipeline sin la comprobación de `schema_version` se rechaza: retirarla
    /// no puede pasar la puerta en silencio.
    #[test]
    fn missing_schema_smoke_is_rejected() {
        let cfg = fixture_config().replace(
            &fixture_schema_smoke(avi_core::json_emitter::CLI_SCHEMA_VERSION),
            "",
        );
        let errors = check_text(&fixture_pins(), &cfg, &fixture_toolchain());
        assert_eq!(errors.len(), 2, "{errors:?}");
        assert!(errors.iter().all(|e| e.contains("no contiene")));
    }

    /// El pipeline real exige al humo la `schema_version` vigente.
    #[test]
    fn real_pipeline_smoke_matches_cli_schema_version() {
        let cfg = include_str!("../../../.circleci/config.yml");
        let errors = schema_smoke_errors(cfg, avi_core::json_emitter::CLI_SCHEMA_VERSION);
        assert!(errors.is_empty(), "{errors:?}");
    }

    #[test]
    fn sync_writes_channel_from_pins() {
        let guard = own_dir("sync");
        let dir = guard.path();
        std::fs::create_dir_all(dir.join("packaging")).unwrap();
        std::fs::write(
            dir.join("packaging/pins.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "rust": "1.96.0", "ort": "1.28.0", "ort_sha256": {}, "sccache": "0.8.2",
                "msys2_base": "2026-06-11", "msys2_gcc": "16.2.0",
                "msys2_openblas": "0.3.34-1", "msys2_make": "4.4.1-5",
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6", "pester": "5.8.0",
                "nextest": "0.9.148",
            }))
            .unwrap(),
        )
        .unwrap();
        run(false, true, Some(dir)).unwrap();
        let written = std::fs::read_to_string(dir.join("rust-toolchain.toml")).unwrap();
        assert_eq!(channel_of(&written).as_deref(), Some("1.96.0"));
    }

    #[test]
    fn check_reads_tree_from_disk() {
        let guard = own_dir("check");
        let dir = guard.path();
        std::fs::create_dir_all(dir.join("packaging")).unwrap();
        std::fs::create_dir_all(dir.join(".circleci")).unwrap();
        std::fs::write(
            dir.join("packaging/pins.json"),
            serde_json::to_string_pretty(&serde_json::json!({
                "rust": "1.96.0", "ort": "1.28.0", "ort_sha256": {}, "sccache": "0.8.2",
                "msys2_base": "2026-06-11", "msys2_gcc": "16.2.0",
                "msys2_openblas": "0.3.34-1", "msys2_make": "4.4.1-5",
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6", "pester": "5.8.0",
                "nextest": "0.9.148",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(".circleci/config.yml"), fixture_config()).unwrap();
        std::fs::write(dir.join("rust-toolchain.toml"), fixture_toolchain()).unwrap();
        run(true, false, Some(dir)).unwrap();
    }
}
