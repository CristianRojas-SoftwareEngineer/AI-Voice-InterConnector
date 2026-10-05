//! Pruebas de conformidad del contrato de la CLI que **no** aporta
//! `cli_golden.rs`: la sincronía de versión de las guías y el vocabulario cerrado de
//! `status` por comando.
//!
//! Lo demás que afirmaba este fichero —la forma del envelope de parada, la de los
//! simulacros de `cleanup`/`self uninstall` y el envelope `usage_error` del rechazo de
//! parseo— ya está en `cli_golden.rs`, que es el dueño del arnés (sandbox con
//! `daemon.pid` señuelo y `TEMP` reubicado) y de las fixtures doradas. Duplicar aquí
//! ese arnés solo multiplicaba su coste de mantenimiento, así que este fichero ya no
//! ejecuta el binario: las dos pruebas siguientes leen documentos, fixtures y código.

use std::path::{Path, PathBuf};

use serde_json::Value;

use avi_core::json_emitter::CLI_SCHEMA_VERSION;

/// Guías de la raíz que describen el contrato de la CLI.
const ROOT_GUIDES: [&str; 2] = ["README.md", "USAGE.md"];

/// Subárbol de documentación que también describe el contrato.
const DOCS_ROOT: &str = "docs";

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
/// Cierra el hueco de la comprobación anterior (`contract.contains(key)`): aquella solo
/// verificaba que el contrato mencionara cuatro claves, y no detectaba que una guía
/// hubiera quedado atrás tras una subida de versión. Ahora la afirmación es de
/// sincronía, no de presencia: recorre `docs/**` y las dos guías de la raíz, y falla
/// nombrando el fichero, la línea y el valor responsable.
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
