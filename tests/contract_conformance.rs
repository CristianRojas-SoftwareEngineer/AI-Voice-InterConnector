// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Pruebas de conformidad del contrato de la CLI que **no** aporta
//! `cli_golden.rs`: la sincronía de versión de las guías, el vocabulario cerrado de
//! `status` por comando, la completitud de las tablas de códigos de salida y la
//! derivación de los flags globales.
//!
//! Lo demás que afirmaba este fichero —la forma del envelope de parada, la de los
//! simulacros de `cleanup`/`self uninstall` y el envelope `usage_error` del rechazo de
//! parseo— ya está en `cli_golden.rs`, que es el dueño del arnés (sandbox con
//! `daemon.pid` señuelo y `TEMP` reubicado) y de las fixtures doradas. Duplicar aquí
//! ese arnés solo multiplicaba su coste de mantenimiento, así que este fichero ya no
//! ejecuta el binario: las pruebas siguientes leen documentos, fixtures y código.

use std::path::{Path, PathBuf};

use serde_json::Value;

use avi_core::json_emitter::CLI_SCHEMA_VERSION;

/// Guías de la raíz que describen el contrato de la CLI.
const ROOT_GUIDES: [&str; 2] = ["README.md", "USAGE.md"];

/// Subárbol de documentación que también describe el contrato.
const DOCS_ROOT: &str = "docs";

/// Fuente única de los códigos de salida: el enum y su única traducción de `reason`.
const EXIT_CODES_SOURCE: &str = "crates/avi-core/src/exit_codes.rs";

/// Documentos que llevan una tabla de códigos de salida con su constante.
const EXIT_CODE_TABLE_DOC: (&str, &str) = ("docs/CLI/README.md", "ExitCode::");

/// Barrido manual donde cada código de dominio tiene su fila con su canal.
const MANUAL_VALIDATION_DOC: &str = "docs/MANUAL-VALIDATION.md";

/// Contrato normativo: flags globales, regla de exclusión y tabla de `speech dub`.
const CONTRACT_DOC: &str = "docs/CLI/CONTRACT.md";

/// Caracteres posteriores a la mención en los que puede aparecer el valor que
/// declara. Cubre la redacción real, incluido el salto de línea de
/// «(actualmente\n`"5"`)» y la frase larga que separa la mención de su valor
/// («…inyecta `schema_version` vía `with_schema_version` …, que usa
/// `CLI_SCHEMA_VERSION = "5"`»), sin llegar tan lejos como a recoger el valor de otra
/// mención. Además, el valor tiene que estar en el **mismo bloque** que la mención: un
/// valor de otro párrafo no es una declaración de esta.
const VALUE_WINDOW: usize = 120;

/// Marcadores textuales del protocolo del daemon, que es un contrato aparte con su
/// propia versión. Una mención queda exenta cuando su contexto inmediato es este
/// protocolo, no cuando el documento entero lo menciona.
const DAEMON_MARKERS: [&str; 5] = [
    "DAEMON_SCHEMA_VERSION",
    "x-schema-version",
    "protocolo del daemon",
    "protocolo IPC",
    "daemon",
];

/// Marcadores del envelope de la CLI. Si la frase los lleva, la mención es del envelope
/// aunque la frase también hable del daemon: es el caso de las que contraponen las dos
/// versiones, donde solo la del daemon se exime.
const ENVELOPE_MARKERS: [&str; 3] = ["envelope", "--json", "CLI"];

/// Vocabulario cerrado de `status` por comando, derivado del código que lo emite, con
/// la fuente de la que se copió. Los simulacros se leen como planes (`planned`),
/// la parada toma el veredicto del motor y `say`/`dub` solo emiten su desenlace.
const STATUS_BY_COMMAND: [(&str, &[&str], &str); 5] = [
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

/// Añede a `out` los `.md` de `dir` y sus subdirectorios, en orden estable, para que
/// el fallo de una prueba no dependa del recorrido del sistema de ficheros.
fn collect_markdown(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut children: Vec<PathBuf> = entries.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    children.sort();
    for child in children {
        if child.is_dir() {
            collect_markdown(&child, out);
        } else if child.extension().is_some_and(|e| e == "md") {
            out.push(child);
        }
    }
}

/// Devuelve (inicio, texto) del bloque markdown —el tramo entre líneas en blanco— que
/// contiene `offset`, que es un índice de byte válido devuelto por una búsqueda.
fn enclosing_block(text: &str, offset: usize) -> (usize, &str) {
    let block_start = text[..offset].rfind("\n\n").map(|i| i + 2).unwrap_or(0);
    let block_end = text[offset..]
        .find("\n\n")
        .map(|i| offset + i)
        .unwrap_or(text.len());
    (block_start, &text[block_start..block_end])
}

/// Primer valor de versión declarado en `tail`: una cadena de dígitos entre comillas,
/// que es como el envelope declara el suyo. Un entero sin comillas es el esquema de
/// otra cosa —el fichero de selección de `setup`, por ejemplo— y no se lee aquí.
fn first_value(tail: &str) -> Option<String> {
    let bytes = tail.as_bytes();
    for i in 0..bytes.len() {
        if bytes[i] != b'"' {
            continue;
        }
        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j > i + 1 && j < bytes.len() && bytes[j] == b'"' {
            return Some(tail[i..=j].to_string());
        }
    }
    None
}

/// Una mención de `schema_version` con el valor que declara y si queda exenta.
struct Mention {
    /// Índice de byte del inicio de la mención dentro del documento.
    start: usize,
    /// Valor declarado, con sus comillas (`"5"`).
    value: String,
    /// El contexto inmediato de la mención es el protocolo del daemon.
    exempt: bool,
}

/// Extremos de la frase que contiene `offset` (índice relativo dentro de `block`).
/// Delimitan la frase el punto seguido de espacio, el `;` y la raya `—`.
fn phrase_bounds(block: &str, offset: usize) -> (usize, usize) {
    let phrase_start = block[..offset]
        .char_indices()
        .filter(|(_, c)| matches!(c, ';' | '—'))
        .map(|(i, c)| i + c.len_utf8())
        .max()
        .into_iter()
        .chain(block[..offset].match_indices(". ").map(|(i, _)| i + 2))
        .max()
        .unwrap_or(0);
    let phrase_end = block[offset..]
        .char_indices()
        .find(|(_, c)| matches!(c, ';' | '—'))
        .map(|(i, _)| offset + i)
        .into_iter()
        .chain(block[offset..].match_indices(". ").map(|(i, _)| offset + i))
        .min()
        .unwrap_or(block.len());
    (phrase_start, phrase_end)
}

/// ¿La mención queda exenta por hablar del protocolo del daemon?
///
/// El contexto inmediato es la frase que la contiene. Si esa frase nombra al envelope de
/// la CLI —«el envelope de la CLI lleva `schema_version` "5"; el protocolo del daemon va
/// por "4"»— no hay exención: las dos versiones de la frase se comprueban, porque solo la
/// segunda es del daemon. La salvedad es la fila de tabla, que es donde el sujeto no cabe
/// en la frase: una fila de endpoints del daemon declara su protocolo sin nombrarlo, y ahí
/// el sujeto lo aporta el título del documento.
fn exempt_for_daemon_protocol(phrase: &str, in_table_row: bool, title: &str) -> bool {
    if ENVELOPE_MARKERS.iter().any(|m| phrase.contains(m)) {
        return false;
    }
    DAEMON_MARKERS.iter().any(|m| phrase.contains(m))
        || (in_table_row && DAEMON_MARKERS.iter().any(|m| title.contains(m)))
}

/// Mención de `schema_version` con el valor que declara y su exención. No cuenta
/// `with_schema_version`: es el nombre de una función, no una mención al campo.
fn mentions(text: &str, title: &str) -> Vec<Mention> {
    let mut findings = Vec::new();
    let mut cursor = 0;
    while let Some(pos) = text[cursor..].find("schema_version") {
        let start = cursor + pos;
        let end = start + "schema_version".len();
        cursor = end;
        let qualified = text[..start]
            .chars()
            .next_back()
            .is_some_and(|c| c.is_alphanumeric() || c == '_');
        if qualified {
            continue;
        }
        let (block_start, block) = enclosing_block(text, start);
        let relative = end - block_start;
        let tail = &block[relative..];
        let window_len = tail
            .char_indices()
            .nth(VALUE_WINDOW)
            .map_or(tail.len(), |(i, _)| i);
        let Some(value) = first_value(&tail[..window_len]) else {
            continue;
        };
        let (phrase_start, phrase_end) = phrase_bounds(block, relative);
        let line = text
            .lines()
            .nth(line_number(text, start) - 1)
            .unwrap_or_default();
        findings.push(Mention {
            start,
            value,
            exempt: exempt_for_daemon_protocol(
                &block[phrase_start..phrase_end],
                line.trim_start().starts_with('|'),
                title,
            ),
        });
    }
    findings
}

/// Título de primer nivel del documento: el sujeto del que escribe, y el único sujeto
/// que una fila de tabla puede dar por heredado.
fn document_title(text: &str) -> &str {
    text.lines()
        .find(|l| l.starts_with("# "))
        .unwrap_or_default()
}

/// Línea (base 1) del índice de byte dado, para nombrar el sitio del fallo.
fn line_number(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count() + 1
}

/// Estados declarados para `command` en [`STATUS_BY_COMMAND`].
fn states_for(command: &str) -> &'static [&'static str] {
    STATUS_BY_COMMAND
        .iter()
        .find(|(name, _, _)| *name == command)
        .map(|(_, values, _)| *values)
        .unwrap_or_else(|| panic!("`{command}` debe declararse en STATUS_BY_COMMAND"))
}

/// Ninguna guía declara una versión del envelope de la CLI distinta de la vigente.
///
/// Recorre `docs/**` y las dos guías de la raíz, y falla nombrando el fichero,
/// la línea y el valor responsable.
#[test]
fn guides_do_not_declare_retired_envelope_version() {
    let root = repo_root();
    let mut documents: Vec<PathBuf> = ROOT_GUIDES.iter().map(|n| root.join(n)).collect();
    let docs_dir = root.join(DOCS_ROOT);
    assert!(
        docs_dir.is_dir(),
        "la documentación debe estar en {}: sin ella el recorrido no comprobaría nada",
        docs_dir.display()
    );
    let base_count = documents.len();
    collect_markdown(&docs_dir, &mut documents);
    assert!(
        documents.len() > base_count,
        "el recorrido de {} debe encontrar los `.md` de la documentación",
        docs_dir.display()
    );

    let mut stale = Vec::new();
    for document in &documents {
        let text = std::fs::read_to_string(document)
            .unwrap_or_else(|e| panic!("no se pudo leer {}: {e}", document.display()));
        let title = document_title(&text);
        for mention in mentions(&text, title) {
            if mention.exempt || mention.value.trim_matches('"') == CLI_SCHEMA_VERSION {
                continue;
            }
            let line = line_number(&text, mention.start);
            let excerpt = text.lines().nth(line - 1).unwrap_or_default().trim();
            stale.push(format!(
                "{}:{} declara {} — {excerpt}",
                document.strip_prefix(&root).unwrap_or(document).display(),
                line,
                mention.value
            ));
        }
    }

    assert!(
        stale.is_empty(),
        "ninguna guía puede declarar un `schema_version` del envelope de la CLI distinto de \
         \"{CLI_SCHEMA_VERSION}\" (crates/avi-core/src/json_emitter.rs); corrige estas {} \
         mención(es):\n  {}",
        stale.len(),
        stale.join("\n  ")
    );
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
/// La tabla se quedó en 17 durante el ciclo de vida y no recogió los cinco códigos que
/// `self update` y `self uninstall` añadieron después. Recorrer el enum es lo que
/// convierte la afirmación en sincronía: un código nuevo nace sin fila y esta puerta
/// falla hasta que la guía lo declare.
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
