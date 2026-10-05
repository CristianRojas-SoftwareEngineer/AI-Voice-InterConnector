//! Regla de clases de pruebas: ninguna prueba se omite en silencio.
//!
//! Una prueba que requiere un recurso externo (modelos, red, binarios) se marca
//! `#[ignore = "requiere …"]` y, cuando se ejecuta, falla con `expect` si el
//! recurso falta. Queda prohibido el patrón de imprimir un aviso de omisión por
//! stderr y retornar en verde, porque esconde pruebas que no corrieron.
//!
//! Esta prueba recorre las fuentes Rust y falla listando cada sitio prohibido:
//!
//! - un `eprintln!` con el marcador de omisión, en cualquier posición;
//! - un atributo que apaga una prueba sin decir `requiere`, porque entonces no
//!   declara qué recurso la omite;
//! - un `return` dentro de una puerta de clase que no demuestra que el recurso
//!   esté disponible, porque la puerta se sale en verde sin comprobar nada.
//!
//! Los patrones se componen por partes para que este archivo no se detecte a sí
//! mismo.

use std::path::{Path, PathBuf};

/// Motivo de un aviso de omisión impreso por stderr.
const PRINTED_SKIP: &str = "eprintln con el marcador de omisión";
/// Motivo de una prueba apagada sin declarar el recurso del que depende.
const IGNORE_WITHOUT_REQUIRES: &str = "atributo `#[ignore]` cuyo motivo no dice `requiere`";
/// Motivo de una puerta de clase que se sale sin comprobar su requisito.
const UNJUSTIFIED_GATE_RETURN: &str = "return que se salta la comprobación de la puerta de clase";
/// Macro de aviso por stderr.
const PRINT_MACRO: &str = concat!("eprint", "ln!");
/// Marcador de aviso de omisión.
const SKIP_MARKER: &str = concat!("ski", "p:");
/// Apertura del atributo que apaga una prueba.
const IGNORE_ATTR: &str = concat!("#[ig", "nore");
/// Texto que declara el recurso del que depende una prueba apagada.
const REQUIRES_MARKER: &str = concat!("requie", "re");
/// Prefijo del nombre de una puerta de clase.
const GATE_PREFIX: &str = concat!("fn requi", "re_");
/// Sentencia de salida de una puerta de clase.
const RETURN_STMT: &str = concat!("retu", "rn");
/// Sondas que prueban que el recurso ya está disponible: solo ellas justifican
/// salir de la puerta antes de comprobar nada.
const AVAILABLE_PROBES: [&str; 3] = [
    concat!("is_som", "e("),
    concat!("is_o", "k("),
    concat!("is_fi", "le()"),
];
/// Sondas que prueban lo contrario, y que además anulan la disponibilidad: una
/// rama que sale con cualquiera de ellas, o con una negación, sale justo cuando el
/// recurso falta.
const UNAVAILABLE_PROBES: [&str; 2] = [concat!("is_non", "e("), concat!("is_er", "r(")];

/// Sitio prohibido: línea dentro del fichero y motivo.
struct Site {
    line: usize,
    reason: &'static str,
}

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

/// Devuelve los sitios prohibidos de `content`, en orden de línea.
///
/// Una puerta de clase es una función `require_*` que no devuelve `Result`: falla
/// con `assert`/`panic` cuando el recurso falta. Las precondiciones de producción
/// también empiezan por `require_`, pero devuelven `Result` y propagan su fallo
/// en vez de omitir una prueba, así que quedan fuera de esta regla.
///
/// Dentro de una puerta solo se tolera un `return` cuya rama demuestra que el
/// recurso ya está disponible (una sonda `is_some`/`is_ok`/`is_file`), como el
/// atajo de `require_clone_binary` cuando `QWEN3_TTS_BIN` ya apunta al motor.
/// Cualquier otro `return` abandona la puerta sin comprobar el requisito, y la
/// prueba de clase se vuelve contrato en silencio.
///
/// Límites conocidos de esta lectura por líneas, aceptados porque el código que
/// vigila está formateado con `rustfmt`: un `eprintln!` o un atributo escritos
/// dentro de un comentario de bloque se tienen por reales, y el cuerpo de una
/// puerta de clase se acota con la primera llave de cierre sangrada al nivel de
/// su `fn`.
fn find_rule_sites(content: &str) -> Vec<Site> {
    let lines: Vec<&str> = content.lines().collect();
    let mut sites = Vec::new();
    // Sangría de la puerta de clase abierta, si la hay.
    let mut gate_indent: Option<usize> = None;
    for (idx, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        // Las reglas leen código, no comentarios: un `//` que describa el patrón
        // no es un sitio prohibido.
        if trimmed.starts_with("//") {
            continue;
        }

        if let Some(gate) = gate_indent {
            // La llave de cierre sangrada al nivel de la puerta la cierra; lo que
            // venga después ya está fuera de ella.
            if trimmed.starts_with('}') && indent_of(line) == gate {
                gate_indent = None;
                continue;
            }
            if let Some(pos) = line.find(RETURN_STMT) {
                if !branch_proves_available(&lines, idx, pos) {
                    sites.push(Site {
                        line: idx + 1,
                        reason: UNJUSTIFIED_GATE_RETURN,
                    });
                }
            }
            continue;
        }

        if trimmed.starts_with(PRINT_MACRO) {
            // Texto desde la macro hasta su `;`, que puede abarcar varias líneas.
            let mut span = String::from(trimmed);
            let mut next = idx + 1;
            while !span.contains(';') && next < lines.len() {
                span.push('\n');
                span.push_str(lines[next]);
                next += 1;
            }
            if span.contains(SKIP_MARKER) {
                sites.push(Site {
                    line: idx + 1,
                    reason: PRINTED_SKIP,
                });
            }
        }

        if trimmed.starts_with(IGNORE_ATTR) {
            // El motivo puede ocupar varias líneas: se lee hasta cerrar el
            // atributo antes de decidir si declara el recurso.
            let mut span = String::from(trimmed);
            let mut next = idx + 1;
            while !span.contains(']') && next < lines.len() {
                span.push('\n');
                span.push_str(lines[next]);
                next += 1;
            }
            if !span.contains(REQUIRES_MARKER) {
                sites.push(Site {
                    line: idx + 1,
                    reason: IGNORE_WITHOUT_REQUIRES,
                });
            }
        }

        if trimmed.starts_with(GATE_PREFIX) && gate_returns_no_result(&lines, idx) {
            gate_indent = Some(indent_of(line));
        }
    }
    sites
}

/// Sangría de una línea, en columnas.
fn indent_of(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Indica si la rama que contiene el `return` de la línea `idx` demuestra que el
/// recurso ya está disponible, con lo que salir de la puerta no omite nada.
///
/// La demostración cabe en la propia línea (`if hay { return; }`) o en la línea
/// previa no vacía, que es donde vive la condición de la rama. La condición
/// cuenta solo si afirma disponibilidad: negarla o comprobar su ausencia
/// (`!preprocesador.is_file()`, `is_none()`) describe justo el caso en que el
/// recurso falta, y salir ahí es la omisión silenciosa que esta regla prohibe.
fn branch_proves_available(lines: &[&str], idx: usize, pos: usize) -> bool {
    let same_line = &lines[idx][..pos];
    if !same_line.trim_start().starts_with("if ") {
        let mut prev = idx;
        while prev > 0 {
            prev -= 1;
            let candidate = lines[prev].trim();
            if candidate.is_empty() || candidate.starts_with("//") {
                continue;
            }
            return condition_proves_availability(candidate);
        }
        return false;
    }
    condition_proves_availability(same_line)
}

/// Indica si la condición `if` afirma que el recurso está disponible.
fn condition_proves_availability(condition: &str) -> bool {
    // Se corta en la llave de apertura para no leer el cuerpo de la rama.
    let condition = condition.split('{').next().unwrap_or(condition);
    !condition.contains('!')
        && !UNAVAILABLE_PROBES
            .iter()
            .any(|probe| condition.contains(probe))
        && AVAILABLE_PROBES
            .iter()
            .any(|probe| condition.contains(probe))
}

/// Indica si la puerta de clase abierta en `idx` es una prueba y no una
/// precondición de producción: se distingue por devolver `Result`.
fn gate_returns_no_result(lines: &[&str], idx: usize) -> bool {
    let mut signature = String::new();
    for line in &lines[idx..] {
        signature.push_str(line);
        if line.contains('{') || line.contains(';') {
            break;
        }
    }
    !signature.contains("Result")
}

/// Fichero sembrado con los dos patrones prohibidos: una prueba apagada que no
/// declara el recurso del que depende, y una puerta de clase que se sale sin
/// comprobarlo.
///
/// Los patrones se componen por partes para que sembrar la regresión no convierta
/// a este archivo en un sitio prohibido.
fn seeded_silent_skip() -> String {
    [
        concat!("#[ig", "nore = \"falta Parakeet\"]"),
        "#[test]",
        "fn parakeet_transcribe_apagada() {}",
        "",
        concat!("fn requi", "re_parakeet() {"),
        concat!("    if !preprocesador.is_file() { retu", "rn; }"),
        concat!(
            "    assert!(preprocesador.is_file(), \"",
            "requie",
            "re Parakeet\");"
        ),
        "}",
    ]
    .iter()
    .map(|line| (*line).to_string())
    .collect::<Vec<_>>()
    .join("\n")
}

/// Fichero con las puertas bien escritas: la que falla en voz alta cuando falta
/// el recurso, y la que solo sale antes de tiempo con el motor ya apuntado por
/// `QWEN3_TTS_BIN`.
fn well_formed_class_gates() -> String {
    [
        concat!("#[ig", "nore = \"", "requie", "re Parakeet\"]"),
        "#[test]",
        "fn parakeet_transcribe() {}",
        "",
        concat!("fn requi", "re_parakeet() {"),
        "    let preprocesador = Path::new(\"nemo128.onnx\");",
        concat!(
            "    assert!(preprocesador.is_file(), \"",
            "requie",
            "re Parakeet\");"
        ),
        "}",
        "",
        concat!("fn requi", "re_clone_binary() {"),
        "    if std::env::var_os(\"QWEN3_TTS_BIN\").is_some() {",
        concat!("        retu", "rn;"),
        "    }",
        "    assert!(vendor.is_file(), \"falta el motor\");",
        "}",
    ]
    .iter()
    .map(|line| (*line).to_string())
    .collect::<Vec<_>>()
    .join("\n")
}

/// Regresión: sembrar los dos patrones prohibidos pone el detector en rojo.
#[test]
fn detector_flags_seeded_silent_skip() {
    let sites = find_rule_sites(&seeded_silent_skip());
    let found: Vec<(usize, &str)> = sites.iter().map(|site| (site.line, site.reason)).collect();
    assert_eq!(
        found,
        vec![(1, IGNORE_WITHOUT_REQUIRES), (6, UNJUSTIFIED_GATE_RETURN)],
        "el detector debe señalar la prueba apagada sin `requiere` y la puerta que se salta la comprobación"
    );
}

/// Las puertas escritas como deben siguen en verde: el atajo con `QWEN3_TTS_BIN`
/// sí demuestra que el recurso está disponible.
#[test]
fn detector_accepts_well_formed_gates() {
    assert!(
        find_rule_sites(&well_formed_class_gates()).is_empty(),
        "una puerta que falla en voz alta y un atajo que demuestra disponibilidad no son sitios prohibidos"
    );
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

    let mut sites = Vec::new();
    for file in &files {
        let Ok(content) = std::fs::read_to_string(file) else {
            continue;
        };
        for site in find_rule_sites(&content) {
            let rel = file.strip_prefix(&root).unwrap_or(file);
            sites.push(format!("{}:{} ({})", rel.display(), site.line, site.reason));
        }
    }
    assert!(
        sites.is_empty(),
        "{} sitios omiten pruebas en silencio:\n{}",
        sites.len(),
        sites.join("\n")
    );
}
