// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Pruebas de conformidad del contrato de la CLI que **no** aporta
//! `cli_golden.rs`: el vocabulario cerrado de `status` por comando, la completitud de
//! las tablas de códigos de salida y la derivación de los flags globales.
//!
//! Las formas de los envelopes que exigen ejecutar el binario —la parada, los
//! simulacros de `cleanup`/`self uninstall` y el `usage_error` del rechazo de parseo—
//! viven en `cli_golden.rs`, que es el dueño del arnés (sandbox con `daemon.pid`
//! señuelo y `TEMP` reubicado) y de las fixtures doradas. Este fichero no ejecuta el
//! binario: sus pruebas leen documentos, fixtures y código.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Fuente única de los códigos de salida: el enum y su única traducción de `reason`.
const EXIT_CODES_SOURCE: &str = "crates/avi-core/src/exit_codes.rs";

/// Documentos que llevan una tabla de códigos de salida con su constante.
const EXIT_CODE_TABLE_DOC: (&str, &str) = ("docs/CLI/README.md", "ExitCode::");

/// Barrido manual donde cada código de dominio tiene su fila con su canal.
const MANUAL_VALIDATION_DOC: &str = "docs/MANUAL-VALIDATION.md";

/// Contrato normativo: flags globales, regla de exclusión y tabla de `speech dub`.
const CONTRACT_DOC: &str = "docs/CLI/CONTRACT.md";

/// Vocabulario cerrado de `status` por comando, derivado del código que lo emite, con
/// la fuente de la que se copió. Los simulacros se leen como planes (`planned`),
/// la parada toma el veredicto del motor y `say`/`dub` solo emiten su desenlace.
const STATUS_BY_COMMAND: [(&str, &[&str], &str); 19] = [
    (
        "setup",
        &["completed", "already_provisioned", "cancelled"],
        "crates/avi-lifecycle/src/setup.rs",
    ),
    (
        "self install",
        &["installed", "repaired", "cancelled"],
        "crates/avi-lifecycle/src/install.rs",
    ),
    (
        "self update",
        &["updated", "already_up_to_date", "cancelled"],
        "src/main.rs",
    ),
    ("doctor --repair", &["repaired"], "src/main.rs"),
    (
        "daemon start",
        &["started", "already_running"],
        "src/main.rs",
    ),
    ("daemon restart", &["restarted"], "src/main.rs"),
    ("translate", &["translated"], "src/main.rs"),
    ("voice clone", &["cloned", "cancelled"], "src/main.rs"),
    ("voice remove", &["removed", "cancelled"], "src/main.rs"),
    (
        "daemon stop",
        &["not_running", "shutdown_sent", "still_running"],
        "src/main.rs",
    ),
    (
        "cleanup",
        &["cleanup_complete", "planned", "cancelled"],
        "crates/avi-lifecycle/src/cleanup.rs",
    ),
    (
        "self uninstall",
        &[
            "not_installed",
            "cancelled",
            "planned",
            "uninstalled",
            "removal_scheduled",
        ],
        "crates/avi-lifecycle/src/uninstall.rs",
    ),
    (
        "speech synthesize",
        &["synthesized", "skipped", "discarded"],
        "src/main.rs",
    ),
    ("speech transcribe", &["transcribed"], "src/main.rs"),
    (
        "speech denoise",
        &["completed", "planned"],
        "crates/denoise/src/cli.rs",
    ),
    ("speech play", &["played"], "src/main.rs"),
    ("speech remove", &["removed", "cancelled"], "src/main.rs"),
    ("speech say", &["reproduced"], "src/main.rs"),
    ("speech dub", &["dubbed"], "src/main.rs"),
];

/// Envelopes reales que `cli_golden.rs` ya produce y contrasta contra su fixture. Se
/// recorren en lugar de invocar el binario porque el arnés que lo aísla vive en esa
/// puerta. `speech say` y `speech dub` no aparecen aquí porque necesitan síntesis real
/// y un dispositivo de salida: sin el motor no hay envelope que congelar.
const GOLDEN_ENVELOPES: [(&str, &str); 4] = [
    ("daemon stop", "cli_daemon_stop.json"),
    ("daemon stop", "cli_daemon_stop_failed.json"),
    ("cleanup", "cli_cleanup_dry_run.json"),
    ("self uninstall", "cli_uninstall_dry_run.json"),
];

/// Raíz del repo (los tests de integración la reciben por `CARGO_MANIFEST_DIR`).
fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Estados declarados para `command` en [`STATUS_BY_COMMAND`].
fn states_for(command: &str) -> &'static [&'static str] {
    STATUS_BY_COMMAND
        .iter()
        .find(|(name, _, _)| *name == command)
        .map(|(_, values, _)| *values)
        .unwrap_or_else(|| panic!("`{command}` debe declararse en STATUS_BY_COMMAND"))
}

/// El `status` de cada envelope real que `cli_golden.rs` produce está en el vocabulario
/// cerrado de su comando, y ese conjunto no se ha alejado del código que lo emite.
///
/// El recorrido del parser real no es viable aquí: `speech say` y `speech dub` necesitan
/// un motor de síntesis real y un dispositivo de salida, y para `daemon stop`, `cleanup`
/// y `self uninstall` habría que repetir el arnés de sandbox que `cli_golden.rs` ya
/// posee. El conjunto es una constante y esta puerta lo contrasta contra dos fuentes
/// reales: los envelopes dorados que esa otra puerta produce, y el código que los emite.
#[test]
fn real_envelope_status_is_in_closed_vocabulary() {
    for (command, fixture) in GOLDEN_ENVELOPES {
        let allowed = states_for(command);
        let path = repo_root().join("tests/golden").join(fixture);
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {e}", path.display()));
        let envelope: Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("la fixture {} no es JSON válido: {e}", fixture));
        let status = envelope["status"]
            .as_str()
            .unwrap_or_else(|| panic!("el envelope de `{command}` debe llevar status: {envelope}"));
        assert!(
            allowed.contains(&status),
            "`{command}` declara el status `{status}` en su vocabulario cerrado {:?}, pero \
             {fixture} lo emite: o falta el valor en la constante, o el comando cambió de \
             vocabulario",
            allowed
        );
    }

    for (command, allowed, source) in STATUS_BY_COMMAND {
        let path = repo_root().join(source);
        let code = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("no se pudo leer {source}: {e}"));
        for value in allowed {
            assert!(
                code.contains(&format!("\"{value}\"")),
                "`{command}` declara el status `{value}`, pero {source} ya no lo emite: o el \
                 código lo renombró y hay que actualizar el contrato, o el valor está caducado \
                 en la constante"
            );
        }
    }
}

/// Líneas del bloque que abre `head`, hasta la primera que cierre solo con `}` (o hasta
/// el final del documento si no la hay). Acota la lectura a la declaración que se quiere
/// contrastar sin depender de su número de línea, que las correcciones desplazan.
fn block_after(text: &str, head: &str) -> Vec<String> {
    let start = text
        .find(head)
        .unwrap_or_else(|| panic!("{head:?} no aparece en el documento"))
        + head.len();
    text[start..]
        .lines()
        .take_while(|line| line.trim_end() != "}")
        .map(str::to_owned)
        .collect()
}

/// Variantes del enum `ExitCode` con su entero, tal y como las declara el código: los
/// enteros se leen del discriminante, no de una copia de la tabla, para que un
/// intercambio de valores en el enum también haga fallar la comprobación.
fn declared_exit_codes() -> Vec<(i32, String)> {
    let path = repo_root().join(EXIT_CODES_SOURCE);
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {EXIT_CODES_SOURCE}: {e}"));
    let mut declared = Vec::new();
    for line in block_after(&source, "pub enum ExitCode {") {
        let line = line.trim();
        let Some((name, code)) = line.split_once(" = ") else {
            continue;
        };
        let code = code
            .trim_end_matches(',')
            .parse::<i32>()
            .unwrap_or_else(|_| {
                panic!("{EXIT_CODES_SOURCE}: línea no parseable en el enum `ExitCode`: {line:?}")
            });
        let name = name.trim();
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            declared.push((code, name.to_string()));
        }
    }
    assert!(
        !declared.is_empty(),
        "{EXIT_CODES_SOURCE} no declara ninguna variante de `ExitCode`: el recorrido está \
         desalineado con el código"
    );
    declared
}

/// Long names de los flags que `Cli` declara globales en `src/main.rs`, deducidos del
/// nombre del campo en kebab-case igual que hace `clap`. Recorre solo el bloque de
/// `struct Cli`: un `global = true` en otro parser no es un flag global de la invocación y
/// no tiene por qué aparecer en el contrato de la raíz.
fn cli_global_flags() -> Vec<String> {
    let path = repo_root().join("src/main.rs");
    let source = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer src/main.rs: {e}"));
    let mut flags = Vec::new();
    let mut pending_global = false;
    for line in block_after(&source, "struct Cli {") {
        let trimmed = line.trim();
        if trimmed.starts_with("#[arg(") || trimmed.starts_with("#[command(") {
            pending_global = trimmed.contains("global = true");
            continue;
        }
        if !pending_global {
            continue;
        }
        let Some((field, _)) = trimmed.split_once(':') else {
            continue;
        };
        pending_global = false;
        // Kebab-case de `clap`: los `_` son separadores de palabra y cada mayúscula abre
        // una nueva, así que `no_daemon` se declara `--no-daemon`.
        let kebab = field
            .trim()
            .chars()
            .flat_map(|c| match c {
                '_' => vec!['-'],
                upper if upper.is_ascii_uppercase() => vec!['-', upper.to_ascii_lowercase()],
                other => vec![other],
            })
            .collect::<String>();
        flags.push(format!("--{kebab}"));
    }
    assert!(
        flags.contains(&"--json".to_string()),
        "src/main.rs declara `json` global: sin él esta comprobación no miraría nada"
    );
    flags
}

/// Líneas de la sección markdown que abre `heading`, hasta el siguiente encabezado de
/// cualquier nivel. Acota la afirmación a su propia sección en vez de al documento
/// entero, que es donde dos frases se contradicen sin que ninguna se entere.
fn section_after(text: &str, heading: &str) -> Vec<String> {
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("la sección {heading:?} no aparece en el documento"))
        + heading.len();
    text[start..]
        .lines()
        .take_while(|line| !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Toda variante de `ExitCode` tiene su fila en la tabla de `docs/CLI/README.md`.
///
/// Recorrer el enum es lo que convierte la afirmación en sincronía: un código nuevo
/// nace sin fila y esta puerta falla hasta que la guía lo declare.
#[test]
fn cli_guide_table_declares_every_exit_code() {
    let (document, marker) = EXIT_CODE_TABLE_DOC;
    let path = repo_root().join(document);
    let guide = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {document}: {e}"));

    let mut missing = Vec::new();
    for (code, variant) in declared_exit_codes() {
        let row = format!("| {code} | `{marker}{variant}` |");
        if !guide.contains(&row) {
            missing.push(format!("{code} (`ExitCode::{variant}`)"));
        }
    }

    assert!(
        missing.is_empty(),
        "la tabla de códigos de {document} no declara {} variante(s) de `ExitCode` \
         ({EXIT_CODES_SOURCE}):\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// El barrido manual nombra todos los códigos de salida de error, con su canal.
///
/// El 0 queda fuera: la sección es de casos de error y el éxito no tiene sonda que
/// disparar. De los demás no se exige una fila —`speech transcribe` con un `--audio`
/// ilegible no sabe producir el 10 a mano y el documento lo dice— pero sí que el código
/// aparezca, sea en su fila o en la frase que explica por qué no la tiene. Lo que vigila
/// es que el barrido no se quede atrás al añadirse una salida.
#[test]
fn manual_validation_names_every_exit_code() {
    let path = repo_root().join(MANUAL_VALIDATION_DOC);
    let manual = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {MANUAL_VALIDATION_DOC}: {e}"));

    let mut missing = Vec::new();
    for (code, variant) in declared_exit_codes() {
        if code == 0 {
            continue;
        }
        if !manual.contains(&format!("**{code}**")) {
            missing.push(format!("{code} (`ExitCode::{variant}`)"));
        }
    }

    assert!(
        missing.is_empty(),
        "{MANUAL_VALIDATION_DOC} no menciona {} código(s) de salida, así que el barrido \
         queda incompleto para ellos:\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// Todo flag que `Cli` declara global aparece en el contrato como global, y `speech dub`
/// declara el `--json` que hereda.
///
/// Cierra los dos fallos que el barrido manual no podía ver: un flag global que el
/// contrato leía como propio de tres subcomandos, y un comando que afirmaba no declarar
/// `--json` cuando lo recibe de la raíz. La regla es de sincronía en las dos
/// direcciones: la lista sale del parser y el contrato tiene que cubrirla entera.
#[test]
fn contract_declares_every_global_flag() {
    let path = repo_root().join(CONTRACT_DOC);
    let contract = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {CONTRACT_DOC}: {e}"));

    let global = cli_global_flags();
    let undocumented: Vec<&str> = global
        .iter()
        .map(String::as_str)
        .filter(|flag| {
            let backticked = format!("`{flag}`");
            !contract
                .lines()
                .any(|line| line.contains(&backticked) && line.contains("global"))
        })
        .collect();
    assert!(
        undocumented.is_empty(),
        "el contrato no declara estos flags globales de `Cli` como globales: {undocumented:?} \
         ({CONTRACT_DOC}, src/main.rs); o el flag dejó de ser global, o el contrato no lo \
         nombra en una frase que lo llame global"
    );

    assert!(
        section_after(&contract, "### `speech dub`")
            .iter()
            .any(|line| line.starts_with('|') && line.contains("--json")),
        "la tabla de parámetros de `speech dub` debe listar `--json`: el flag es global \
         (`Cli::json`) y el comando lo honra en las dos vías"
    );
}

/// Añade a `out` los `.rs` de `dir` y sus subdirectorios, en orden estable.
fn collect_rust(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            collect_rust(&child, out);
        } else if child.extension().is_some_and(|e| e == "rs") {
            out.push(child);
        }
    }
}

/// Código de producción del binario y de cada crate (sin módulos `tests` ni la tabla
/// misma), con la ruta de cada fichero.
fn production_sources() -> Vec<(PathBuf, String)> {
    let root = repo_root();
    let mut dirs = vec![root.join("src")];
    let mut crates: Vec<PathBuf> = std::fs::read_dir(root.join("crates"))
        .expect("el directorio crates existe")
        .map(|e| e.expect("entrada legible de crates").path())
        .collect();
    crates.sort();
    dirs.extend(crates.into_iter().map(|c| c.join("src")));

    let mut files = Vec::new();
    for dir in &dirs {
        collect_rust(dir, &mut files);
    }
    files
        .into_iter()
        .filter(|f| !f.ends_with("avi-core/src/exit_codes.rs"))
        .map(|f| {
            let text = std::fs::read_to_string(&f)
                .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", f.display()));
            let production = text
                .split("\nmod tests {")
                .next()
                .expect("el código de producción precede al módulo de pruebas")
                .to_string();
            (f, production)
        })
        .collect()
}

/// Nombre `reason` (minúsculas y `_`) al inicio de `text`, si lo cierra una comilla
/// cuando `quoted` es verdadero.
fn reason_prefix(text: &str, quoted: bool) -> Option<String> {
    let name: String = text
        .chars()
        .take_while(|c| c.is_ascii_lowercase() || *c == '_')
        .collect();
    if name.is_empty() || (quoted && !text[name.len()..].starts_with('"')) {
        return None;
    }
    Some(name)
}

/// Literal de `reason` que sigue a `text` si este empieza (tras espacios) por una
/// comilla.
fn leading_literal(text: &str) -> Option<String> {
    reason_prefix(text.trim_start().strip_prefix('"')?, true)
}

/// Todo `reason` que el código de producción declara de forma literal: el segundo
/// argumento de `CliError::new`, el primero de `LifecycleError::new` (y de su
/// constructor `Self::new` en el crate de ciclo de vida), las claves `"reason"` y
/// `"error"` de los envelopes, el campo `reason:` de los errores tipados y la línea
/// `error=<reason>` del fichero ready.
fn emitted_reasons() -> Vec<(String, String)> {
    let mut emitted = Vec::new();
    for (path, text) in production_sources() {
        let file = path.display().to_string();
        for key in ["\"reason\": \"", "\"error\": \"", "reason: \""] {
            for (i, _) in text.match_indices(key) {
                if let Some(r) = reason_prefix(&text[i + key.len()..], true) {
                    emitted.push((r, file.clone()));
                }
            }
        }
        for (i, _) in text.match_indices("error=") {
            if let Some(r) = reason_prefix(&text[i + "error=".len()..], false) {
                emitted.push((r, file.clone()));
            }
        }
        for (i, _) in text.match_indices("CliError::new(") {
            let rest = &text[i + "CliError::new(".len()..];
            if let Some(comma) = rest.find(',') {
                if let Some(r) = leading_literal(&rest[comma + 1..]) {
                    emitted.push((r, file.clone()));
                }
            }
        }
        let mut constructors = vec!["LifecycleError::new("];
        if file.contains("avi-lifecycle") {
            constructors.push("Self::new(");
        }
        for constructor in constructors {
            for (i, _) in text.match_indices(constructor) {
                if let Some(r) = leading_literal(&text[i + constructor.len()..]) {
                    emitted.push((r, file.clone()));
                }
            }
        }
    }
    emitted
}

/// Todo `reason` que emite el código de producción de la CLI, el daemon y el ciclo de
/// vida está declarado en la tabla única de códigos de salida.
#[test]
fn every_emitted_reason_is_in_exit_code_table() {
    let emitted = emitted_reasons();
    assert!(
        !emitted.is_empty(),
        "la búsqueda de literales no encontró ningún reason"
    );
    let undeclared: Vec<String> = emitted
        .iter()
        .filter(|(reason, _)| !avi_core::exit_codes::ExitCode::is_declared_reason(reason))
        .map(|(reason, file)| format!("{reason} ({file})"))
        .collect();
    assert!(
        undeclared.is_empty(),
        "reason emitidos que `REASON_CODES` ({EXIT_CODES_SOURCE}) no declara:\n  {}",
        undeclared.join("\n  ")
    );
}

/// Todo `reason` de la tabla única de códigos de salida aparece como literal en algún
/// punto del código de producción: un `reason` que nadie emite se retira de la tabla.
#[test]
fn every_table_reason_is_emitted_somewhere() {
    let sources = production_sources();
    let missing: Vec<&str> = avi_core::exit_codes::REASON_CODES
        .iter()
        .map(|(reason, _)| *reason)
        .filter(|reason| {
            let quoted = format!("\"{reason}\"");
            let ready_line = format!("error={reason}");
            !sources
                .iter()
                .any(|(_, text)| text.contains(&quoted) || text.contains(&ready_line))
        })
        .collect();
    assert!(
        missing.is_empty(),
        "reason de `REASON_CODES` ({EXIT_CODES_SOURCE}) que ningún código de producción emite: \
         {missing:?}"
    );
}

/// El contrato nombra, en la sección de códigos de salida, cada `reason` de la tabla.
#[test]
fn contract_names_every_table_reason() {
    let path = repo_root().join(CONTRACT_DOC);
    let contract = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer {CONTRACT_DOC}: {e}"));
    let missing: Vec<&str> = avi_core::exit_codes::REASON_CODES
        .iter()
        .map(|(reason, _)| *reason)
        .filter(|reason| !contract.contains(&format!("`{reason}`")))
        .collect();
    assert!(
        missing.is_empty(),
        "{CONTRACT_DOC} no nombra estos reason de la tabla: {missing:?}"
    );
}
