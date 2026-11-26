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
//! aquí y en ningún otro sitio. `ort_nupkg_sha256` no es una versión sino el hash
//! del paquete NuGet que verifica `xtask package`, y ningún paso de la CI lo
//! interpola.
//!
//! La misma puerta compara los literales de la CI que espejan valores del
//! código: la limpieza anterior a guardar `target/` debe cubrir todos los
//! prefijos de prueba declarados en `avi-shared`, además de los temporales por
//! defecto y la raíz del daemon.
//! Además, cada executor de Windows debe tomar la versión de su imagen del
//! parámetro `windows_image_pin`.
//!
//! Sin dependencias nuevas: basta con leer ficheros y comparar cadenas.

use anyhow::{anyhow, Result};
use avi_shared::{
    paths,
    pins::{self, Pins},
};
use std::path::{Path, PathBuf};

/// Sufijo que marca un parámetro del pipeline como pin versionado.
const PIN_SUFFIX: &str = "_pin";

/// Fichero del canal de Rust, relativo a la raíz del repositorio.
const TOOLCHAIN_REL: &str = "rust-toolchain.toml";

/// Inicio del comando que retira temporales antes de guardar la caché de `target/`.
const CACHE_CLEANUP_MARKER: &str = "          command: >-\n            rm -rf -- ";

/// Targets de AVI a los que se les exige un SHA-256 del paquete del motor.
const ENGINE_TRIPLES: [&str; 4] = [
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
];

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
                "pines en sincronía: parámetros, canal de Rust, instaladores, limpieza de la caché e imagen de Windows"
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

/// Las nueve claves con su valor, en el orden del manifiesto.
fn all_pins(pins: &Pins) -> [(&'static str, &String); 9] {
    [
        ("rust", &pins.rust),
        ("ort", &pins.ort),
        ("sccache", &pins.sccache),
        ("ninja", &pins.ninja),
        ("bats", &pins.bats),
        ("pwsh", &pins.pwsh),
        ("pester", &pins.pester),
        ("nextest", &pins.nextest),
        ("windows_image", &pins.windows_image),
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
    errors.extend(cache_cleanup_errors(cfg));
    errors.extend(windows_executor_errors(cfg));
    errors.extend(engine_pin_errors(pins));
    errors
}

/// Errores del pin del motor TTS: una versión vacía, un target de AVI sin
/// hash, un hash que no son 64 dígitos hexadecimales en minúsculas, o una
/// tabla con un target que AVI no usa. El aprovisionamiento verifica el hash
/// del paquete, así que aquí se comprueba que la tabla esté completa y bien
/// formada antes de intentar ninguna descarga.
fn engine_pin_errors(pins: &Pins) -> Vec<String> {
    let mut errors = Vec::new();
    if pins.qwen_tts_engine.trim().is_empty() {
        errors.push("qwen_tts_engine no fija ninguna versión".to_string());
    }
    for triple in ENGINE_TRIPLES {
        match pins.qwen_tts_engine_sha256.get(triple) {
            Some(hash) if super::package::is_sha256(hash) => {}
            Some(hash) => errors.push(format!(
                "qwen_tts_engine_sha256[{triple}] no son 64 dígitos hexadecimales en minúsculas: {hash:?}"
            )),
            None => errors.push(format!(
                "qwen_tts_engine_sha256 no fija el SHA-256 del motor para {triple}"
            )),
        }
    }
    for triple in pins.qwen_tts_engine_sha256.keys() {
        if !ENGINE_TRIPLES.contains(&triple.as_str()) {
            errors.push(format!(
                "qwen_tts_engine_sha256 fija un target que AVI no soporta: {triple}"
            ));
        }
    }
    let source = &pins.qwen_tts_engine_source_sha256;
    if !super::package::is_sha256(source) {
        errors.push(format!(
            "qwen_tts_engine_source_sha256 no son 64 dígitos hexadecimales en minúsculas: {source:?}"
        ));
    }
    errors
}

/// Executor del orb de Windows cuya imagen fija `windows_image_pin`.
const WINDOWS_EXECUTOR: &str = "win/server-2022";

/// Línea que fija la imagen del executor de Windows con el parámetro del pin.
const WINDOWS_IMAGE_VERSION: &str = "version: << pipeline.parameters.windows_image_pin >>";

/// Exige que cada executor `win/server-2022` tome la versión de la imagen de
/// `windows_image_pin`: la forma corta `executor: win/server-2022` usa la imagen
/// `current` del orb, que cambia el compilador MSVC sin que el repositorio lo
/// registre.
fn windows_executor_errors(cfg: &str) -> Vec<String> {
    let lines: Vec<&str> = cfg.lines().map(str::trim).collect();
    let mut errors = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if *line == format!("executor: {WINDOWS_EXECUTOR}") {
            errors.push(format!(
                "executor {WINDOWS_EXECUTOR} sin versión de imagen (línea {}): declara `name: {WINDOWS_EXECUTOR}` y `{WINDOWS_IMAGE_VERSION}`",
                i + 1
            ));
        } else if *line == format!("name: {WINDOWS_EXECUTOR}")
            && lines.get(i + 1) != Some(&WINDOWS_IMAGE_VERSION)
        {
            errors.push(format!(
                "executor {WINDOWS_EXECUTOR} sin la versión del pin (línea {}): añade `{WINDOWS_IMAGE_VERSION}` tras `name`",
                i + 1
            ));
        }
    }
    errors
}

/// Exige que la orden anterior a `save_cache` retire las familias de prueba,
/// los temporales por defecto y la raíz del daemon, sin borrar todo `target/`.
fn cache_cleanup_errors(cfg: &str) -> Vec<String> {
    let Some((_, command)) = cfg.split_once(CACHE_CLEANUP_MARKER) else {
        return vec!["falta la limpieza de target/ en cargo_save_target: restaura `rm -rf --` antes de save_cache y enumera los prefijos de TEST_SANDBOX_PREFIXES".to_string()];
    };
    let Some((command, _)) = command.split_once("\n      - save_cache:") else {
        return vec!["la limpieza de target/ no precede a save_cache en cargo_save_target: mueve el comando antes de guardar la caché".to_string()];
    };
    let paths: Vec<&str> = command.split_whitespace().collect();
    let mut errors = Vec::new();
    for pattern in std::iter::once("target/.tmp*".to_string())
        .chain(std::iter::once(format!(
            "target/{}",
            paths::GOLDEN_DAEMON_DIR_NAME
        )))
        .chain(
            paths::TEST_SANDBOX_PREFIXES
                .iter()
                .map(|prefix| format!("target/{prefix}*")),
        )
    {
        if !paths.contains(&pattern.as_str()) {
            errors.push(format!(
                "la limpieza de la caché no cubre {pattern}: añade ese patrón a `rm -rf --` en cargo_save_target antes de save_cache"
            ));
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
            ort_nupkg_sha256: String::new(),
            ffmpeg: "9.0.2".to_string(),
            ffmpeg_sha256: std::collections::BTreeMap::new(),
            ffmpeg_source_sha256: std::collections::BTreeMap::new(),
            qwen_tts_engine: "1.0.0".to_string(),
            qwen_tts_engine_sha256: ENGINE_TRIPLES
                .iter()
                .map(|triple| (triple.to_string(), "a".repeat(64)))
                .collect(),
            qwen_tts_engine_source_sha256: "b".repeat(64),
            sccache: "0.8.2".to_string(),
            ninja: "1.13.2".to_string(),
            bats: "1.14.0".to_string(),
            pwsh: "7.6.6".to_string(),
            pester: "5.8.0".to_string(),
            nextest: "0.9.148".to_string(),
            windows_image: "2026.10.1".to_string(),
        }
    }

    /// CI de fixture con los ocho parámetros esperados, la limpieza de caché y
    /// un executor de Windows vigentes.
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
        cfg.push_str("  cargo_save_target:\n    steps:\n      - run:\n");
        cfg.push_str(CACHE_CLEANUP_MARKER);
        cfg.push_str("target/.tmp* ");
        cfg.push_str(&format!("target/{} ", paths::GOLDEN_DAEMON_DIR_NAME));
        for prefix in paths::TEST_SANDBOX_PREFIXES {
            cfg.push_str(&format!("target/{prefix}* "));
        }
        cfg.push_str("\n      - save_cache:\n");
        cfg.push_str(FIXTURE_WINDOWS_EXECUTOR);
        cfg
    }

    /// Executor de Windows de fixture con la versión del pin.
    const FIXTURE_WINDOWS_EXECUTOR: &str = "  test-windows:\n    executor:\n      name: win/server-2022\n      version: << pipeline.parameters.windows_image_pin >>\n";

    /// Canal de fixture que copia el pin de Rust.
    fn fixture_toolchain() -> String {
        render_toolchain(&fixture_pins())
    }

    /// Tabla de hashes del motor de fixture: los cuatro targets de AVI, cada
    /// uno con un hash bien formado.
    fn engine_fixture_hashes() -> serde_json::Value {
        let hash = "a".repeat(64);
        serde_json::json!({
            "x86_64-pc-windows-msvc": hash,
            "x86_64-unknown-linux-gnu": hash,
            "aarch64-unknown-linux-gnu": hash,
            "aarch64-apple-darwin": hash,
        })
    }

    /// Hash de fixture del archivo de fuentes del motor.
    fn engine_fixture_source_hash() -> String {
        "b".repeat(64)
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

    /// Un target de AVI sin hash del motor se rechaza: el aprovisionamiento no
    /// puede verificar una descarga que no tiene hash fijado.
    #[test]
    fn engine_pin_without_a_target_hash_is_rejected() {
        let mut pins = fixture_pins();
        pins.qwen_tts_engine_sha256.remove("aarch64-apple-darwin");
        let errors = check_text(&pins, &fixture_config(), &fixture_toolchain());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("aarch64-apple-darwin") && e.contains("no fija")),
            "{errors:?}"
        );
    }

    /// Un hash del motor que no son 64 dígitos hexadecimales en minúsculas se
    /// rechaza en cualquier target.
    #[test]
    fn engine_pin_with_malformed_hash_is_rejected() {
        let mut pins = fixture_pins();
        pins.qwen_tts_engine_sha256
            .insert("x86_64-unknown-linux-gnu".to_string(), "ABC".to_string());
        let errors = check_text(&pins, &fixture_config(), &fixture_toolchain());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("x86_64-unknown-linux-gnu") && e.contains("64 dígitos")),
            "{errors:?}"
        );
    }

    /// La tabla solo admite los cuatro targets de AVI: una entrada de otro
    /// target no se verifica en ninguna descarga y se rechaza.
    #[test]
    fn engine_pin_with_unsupported_target_is_rejected() {
        let mut pins = fixture_pins();
        pins.qwen_tts_engine_sha256
            .insert("riscv64gc-unknown-linux-gnu".to_string(), "a".repeat(64));
        let errors = check_text(&pins, &fixture_config(), &fixture_toolchain());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("riscv64gc-unknown-linux-gnu") && e.contains("no soporta")),
            "{errors:?}"
        );
    }

    /// El hash de las fuentes mal formado se rechaza: es el que comprueba la
    /// oferta de código fuente.
    #[test]
    fn engine_source_pin_with_malformed_hash_is_rejected() {
        let mut pins = fixture_pins();
        pins.qwen_tts_engine_source_sha256 = "xyz".to_string();
        let errors = check_text(&pins, &fixture_config(), &fixture_toolchain());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("qwen_tts_engine_source_sha256") && e.contains("64 dígitos")),
            "{errors:?}"
        );
    }

    /// Una versión del motor vacía se rechaza: no puede formar ni la URL ni el
    /// nombre del paquete.
    #[test]
    fn engine_pin_without_version_is_rejected() {
        let mut pins = fixture_pins();
        pins.qwen_tts_engine = " ".to_string();
        let errors = check_text(&pins, &fixture_config(), &fixture_toolchain());
        assert!(
            errors
                .iter()
                .any(|e| e.contains("qwen_tts_engine no fija ninguna versión")),
            "{errors:?}"
        );
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

    /// La cobertura debe fallar si falta un prefijo, el temporal por defecto,
    /// la raíz del daemon o la propia orden de limpieza.
    #[test]
    fn stale_cache_cleanup_is_rejected() {
        let cfg = fixture_config();
        for pattern in [
            "target/ai-voice-interconnector-menu-*",
            "target/.tmp*",
            "target/golden-avi-daemon",
        ] {
            let stale = cfg.replace(pattern, "");
            let errors = cache_cleanup_errors(&stale);
            assert!(
                errors
                    .iter()
                    .any(|e| e.contains(pattern) && e.contains("añade")),
                "{errors:?}"
            );
        }
        let missing = cfg.replace(CACHE_CLEANUP_MARKER, "");
        assert!(cache_cleanup_errors(&missing)[0].contains("restaura"));
    }

    /// El comando real debe cubrir los prefijos compartidos antes de guardar `target/`.
    #[test]
    fn real_pipeline_cache_cleanup_covers_test_sandboxes() {
        let cfg = include_str!("../../../.circleci/config.yml");
        let errors = cache_cleanup_errors(cfg);
        assert!(errors.is_empty(), "{errors:?}");
    }

    /// Un executor de Windows en forma corta (imagen `current`) o con `name` y
    /// sin la versión del pin se rechaza.
    #[test]
    fn windows_executor_without_pinned_image_is_rejected() {
        let short = fixture_config().replace(
            FIXTURE_WINDOWS_EXECUTOR,
            "  test-windows:\n    executor: win/server-2022\n",
        );
        let errors = check_text(&fixture_pins(), &short, &fixture_toolchain());
        assert!(
            errors.iter().any(|e| e.contains("sin versión de imagen")),
            "{errors:?}"
        );

        let unpinned = fixture_config().replace(
            "      version: << pipeline.parameters.windows_image_pin >>\n",
            "      version: current\n",
        );
        let errors = check_text(&fixture_pins(), &unpinned, &fixture_toolchain());
        assert!(
            errors.iter().any(|e| e.contains("sin la versión del pin")),
            "{errors:?}"
        );
    }

    /// Los executors de Windows del pipeline real toman la versión del pin.
    #[test]
    fn real_pipeline_windows_executors_use_pinned_image() {
        let cfg = include_str!("../../../.circleci/config.yml");
        assert!(cfg.contains("name: win/server-2022"));
        let errors = windows_executor_errors(cfg);
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
                "rust": "1.96.0", "ort": "1.28.0", "ort_nupkg_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ffmpeg": "9.0.2", "ffmpeg_sha256": {}, "ffmpeg_source_sha256": {}, "sccache": "0.8.2",
                "qwen_tts_engine": "1.0.0", "qwen_tts_engine_sha256": engine_fixture_hashes(), "qwen_tts_engine_source_sha256": engine_fixture_source_hash(),
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6", "pester": "5.8.0",
                "nextest": "0.9.148", "windows_image": "2026.10.1",
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
                "rust": "1.96.0", "ort": "1.28.0", "ort_nupkg_sha256": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "ffmpeg": "9.0.2", "ffmpeg_sha256": {}, "ffmpeg_source_sha256": {}, "sccache": "0.8.2",
                "qwen_tts_engine": "1.0.0", "qwen_tts_engine_sha256": engine_fixture_hashes(), "qwen_tts_engine_source_sha256": engine_fixture_source_hash(),
                "ninja": "1.13.2", "bats": "1.14.0", "pwsh": "7.6.6", "pester": "5.8.0",
                "nextest": "0.9.148", "windows_image": "2026.10.1",
            }))
            .unwrap(),
        )
        .unwrap();
        std::fs::write(dir.join(".circleci/config.yml"), fixture_config()).unwrap();
        std::fs::write(dir.join("rust-toolchain.toml"), fixture_toolchain()).unwrap();
        run(true, false, Some(dir)).unwrap();
    }
}
