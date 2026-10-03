//! `xtask comments --check`: puerta que rechaza dos formas de documentar que se
//! desalinean con el tiempo en cuanto el código se mueve.
//!
//! La primera es la **cita de sección**: un comentario que remite a una sección
//! numerada de la especificación en vez de explicar la regla. El enunciado que la
//! prohíbe es la fila de la tabla por capa de `AGENTS.md` y el principio `P12` de la
//! especificación, y ninguno de los dos se vigila por sí solo: un principio escrito que
//! ninguna puerta mira se degrada en silencio, que es el punto ciego por el que la
//! norma de idioma ya pasó una vez.
//!
//! La segunda es el **localizador de línea**: citar `fichero.ext:NNN` dentro de un
//! comentario. Apunta a una coordenada que se desplaza con la primera edición que toque
//! esas líneas, así que envejece peor que la cita de sección: se han medido nueve que ya
//! apuntaban a ficheros borrados.
//!
//! **Lo que este control NO intenta**, por decisión y no por descuido: no rechaza que
//! un comentario nombre otro fichero. El nombre del fichero que una función escribe o
//! lee es parte legítima de la operación —el manifiesto del bundle, el fichero de
//! sumas, el directorio de una fixture—, y la frontera entre ese nombre y una remisión
//! del tipo "véase X" es un juicio que ninguna herramienta puede hacer. Medido en el
//! árbol: 76 menciones de nombre de fichero, unas 50 de ellas legítimas. Un control que
//! las rechazara necesitaría una lista blanca de unas 25 entradas que habría que podar
//! cada vez que se toca un comentario, y una lista blanca que se poda es un agujero.
//!
//! Sin dependencias nuevas: el inventario congelado de `THIRD-PARTY-LICENSES.md` obliga
//! a no añadir crates, y el lexer que hace falta es una expresión regular.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Raíces de ficheros de primera parte que se recorren, en orden. Las cuatro familias
/// del ciclo: código (`src`, `crates/*/src`), pruebas (`crates/*/tests`, `tests`),
/// configuración (`.circleci`, `packaging`, `.githooks`) y scripts (los `.sh` y `.ps1`
/// de `packaging/bootstrap`, `tests/bootstrap` y `.githooks`, que caen dentro de esas
/// raíces). Una raíz que no exista se ignora en silencio.
const ROOTS: &[&str] = &[
    "src",
    "crates",
    "tests",
    "packaging",
    ".circleci",
    ".githooks",
];

/// Ficheros de configuración que viven en la raíz del repositorio y no bajo ninguna de
/// las raíces anteriores.
const ROOT_FILES: &[&str] = &["Cargo.toml", "rust-toolchain.toml", "opencode.json"];

/// Extensiones que el control lee. Son las que puede contener un comentario o un
/// diagnóstico de aserción en este repositorio; añadir una aquí obliga a revisar si el
/// texto que se rechaza tiene sentido en ese lenguaje.
const EXTS: &[&str] = &["rs", "yml", "toml", "json", "sh", "ps1"];

/// Componentes de ruta que se excluyen: código de terceros vendorizado y directorios de
/// construcción o de producto que no son fuente.
const SKIP_DIRS: &[&str] = &["vendor", "target", "artifacts", ".git"];

/// Una supervivencia conocida, con la razón escrita al lado.
///
/// La lista es corta a propósito y hay dos propiedades que la sostienen. Ninguna entrada
/// se añade sin su razón, y una prueba falla si una entrada deja de ser necesaria, de
/// modo que la lista blanca no se puede dejar poda: si el código que la justificaba
/// cambia, la puerta avisa en vez de seguir perdonando en silencio.
struct AllowEntry {
    /// Ruta relativa a la raíz, con separador `/` en todas las plataformas.
    file: &'static str,
    /// Por qué sobreviven las líneas de ese fichero.
    reason: &'static str,
}

/// Supervivencias conocidas, una por fichero y con su razón.
///
/// Las cuatro son la **sección 6 de la GNU GPL**, que es una sección del contrato de
/// licencia y no de la especificación. Este control no las reescribe y no puede hacerlo,
/// porque es el texto que la propia licencia exige publicar.
const ALLOWLIST: &[AllowEntry] = &[
    AllowEntry {
        file: "crates/xtask/src/main.rs",
        reason: "sección 6 de la GNU GPL en la plantilla y el anuncio de la oferta de \
                 código fuente; es contrato de licencia, no de la especificación",
    },
    AllowEntry {
        file: ".circleci/config.yml",
        reason: "sección 6 de la GNU GPL en el título de la oferta de código fuente que \
                 publica el job de release; mismo contrato de licencia",
    },
];

/// Una línea rechazada, con su fichero, su número, su motivo y el texto tal cual.
#[derive(Debug)]
struct Hit {
    file: String,
    line: usize,
    kind: &'static str,
    text: String,
}

/// Cita de sección: el símbolo de sección seguido de un número. No exige punto, porque
/// las citas mal escritas también las escribe la gente.
fn section_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new("\u{a7}\\s*\\d").unwrap())
}

/// Localizador de línea: un nombre de fichero con extensión, dos puntos y dígitos. La
/// lista de extensiones es la de los ficheros a los que este repositorio apunta, más
/// `.c` y `.h` porque el motor vendorizado se citaba así y sus referencias quedaron
/// colgando cuando esos ficheros se borraron.
fn locator_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"[A-Za-z0-9_./\\-]+\.(rs|md|json|sh|ps1|toml|yml|c|h):\d+").unwrap()
    })
}

/// Entrada del subcomando `comments`. `root` sobrescribe la raíz a escanear (por
/// defecto, el directorio actual), igual que en el control de idioma: sin eso no se
/// puede comprobar el control contra un fixture.
pub(crate) fn run(check: bool, root: Option<&Path>) -> Result<()> {
    if !check {
        println!("Usa --check para escanear las citas de sección y los localizadores de línea");
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let files = collect_files(&base)?;
    let hits = scan(&base, &files);

    if hits.is_empty() {
        println!(
            "0 citas de sección ni localizadores de línea en {} ficheros de primera parte",
            files.len()
        );
        return Ok(());
    }
    print_report(&hits);
    print_allowlist();
    let dirty: std::collections::HashSet<&str> = hits.iter().map(|h| h.file.as_str()).collect();
    println!(
        "CITAS O LOCALIZADORES: {} en {} ficheros",
        hits.len(),
        dirty.len()
    );
    std::process::exit(1);
}

/// Recorre los ficheros de las cuatro familias y devuelve las líneas rechazadas, en
/// orden de fichero y de línea.
fn scan(base: &Path, files: &[PathBuf]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let relative = relative_to(base, file);
        if is_allowed(&relative) {
            continue;
        }
        for (index, line) in text.lines().enumerate() {
            let kind = if section_re().is_match(line) {
                "cita de sección"
            } else if locator_re().is_match(line) {
                "localizador de línea"
            } else {
                continue;
            };
            hits.push(Hit {
                file: relative.clone(),
                line: index + 1,
                kind,
                text: line.trim().to_string(),
            });
        }
    }
    hits
}

/// Si el fichero tiene una supervivencia declarada. La lista es por fichero y no por
/// línea porque la razón es la misma para todas las de un fichero: aquí son las cuatro
/// líneas de la GNU GPL, y separarlas daría cuatro entradas que alguien tendría que podar
/// en cuanto la línea se moviera.
fn is_allowed(relative: &str) -> bool {
    ALLOWLIST.iter().any(|e| e.file == relative)
}

/// Ficheros de las cuatro familias bajo la raíz, ordenados. Las raíces que no existen se
/// ignoran en silencio, y cualquier ruta con un componente de `SKIP_DIRS` queda fuera: es
/// código de terceros o un directorio de construcción.
fn collect_files(base: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in ROOTS {
        let dir = base.join(root);
        if dir.is_dir() {
            walk(&dir, &mut files)?;
        }
    }
    for name in ROOT_FILES {
        let file = base.join(name);
        if file.is_file() {
            files.push(file);
        }
    }
    files.sort();
    files.dedup();
    Ok(files)
}

/// Recorrido en profundidad con las exclusiones aplicadas.
fn walk(dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries =
        std::fs::read_dir(dir).map_err(|e| anyhow!("no se pudo leer {}: {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry
            .map_err(|e| anyhow!("no se pudo leer una entrada de {}: {}", dir.display(), e))?;
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if SKIP_DIRS.iter().any(|s| *s == name) {
            continue;
        }
        if path.is_dir() {
            walk(&path, files)?;
        } else if matches_extension(&path) {
            files.push(path);
        }
    }
    Ok(())
}

/// Si la extensión del fichero está entre las que el control lee.
fn matches_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.contains(&e))
}

/// Ruta del fichero relativa a la raíz, con separador `/` en todas las plataformas, para
/// que la lista blanca se escriba igual en todos los sistemas operativos.
fn relative_to(base: &Path, file: &Path) -> String {
    file.strip_prefix(base)
        .unwrap_or(file)
        .display()
        .to_string()
        .replace('\\', "/")
}

/// Informe de las líneas rechazadas, con el fichero, la línea y el motivo.
fn print_report(hits: &[Hit]) {
    let header = format!(
        "{:<44} {:>6}  {:<22} {}",
        "FICHERO", "LINEA", "MOTIVO", "TEXTO"
    );
    println!("{header}");
    for hit in hits {
        println!(
            "{:<44} {:>6}  {:<22} {}",
            hit.file,
            hit.line,
            hit.kind,
            truncate(&hit.text, 60)
        );
    }
}

/// Recorta el texto de la línea para que el informe no se vaya de ancho.
fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    format!("{cut}…")
}

/// Lo que la puerta está perdonando, con la razón al lado.
///
/// Se imprime junto al informe de fallos y no en el de éxito: quien lee un rechazo tiene
/// que poder contrastarlo con lo que la puerta ya Tolera, y una lista blanca que no se
/// ve es una lista blanca que nadie audita.
fn print_allowlist() {
    if ALLOWLIST.is_empty() {
        return;
    }
    println!("\nPERDONADO ({})", ALLOWLIST.len());
    for entry in ALLOWLIST {
        println!("  {}: {}", entry.file, entry.reason);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El símbolo de sección y un localizador de línea, **construidos y no escritos**.
    ///
    /// Si el fichero fuente de este módulo contuviera el símbolo seguido de un número, o
    /// un nombre de fichero seguido de dos puntos y dígitos, el propio control lo
    /// rechazaría al escanearse, y la razón sería que la prueba necesita el patrón. Se
    /// arma en tiempo de ejecución para que la fuente quede limpia, y la prueba
    /// `the_repository_is_clean` lo verifica sobre el árbol entero.
    fn section_sign() -> char {
        char::from(0xA7)
    }

    fn section_citation() -> String {
        format!("{}9.1", section_sign())
    }

    fn localizador() -> String {
        ["crates/avi-store/src/lib", ".rs", ":", "660"].concat()
    }

    /// Raíz del repositorio, para comprobar el control contra el árbol real.
    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .to_path_buf()
    }

    /// Escribe un árbol de fixtures en el temporal y lo escanea, para probar el rechazo
    /// sin tocar el repositorio.
    ///
    /// El directorio lleva un contador y no el número de ficheros porque las pruebas
    /// corren en paralelo dentro del mismo proceso: dos fixtures con el mismo número de
    /// ficheros comparten nombre y una lee los archivos de la otra, que es como cinco de
    /// estas pruebas fallaron a la vez la primera vez.
    fn scan_fixture(files: &[(&str, String)]) -> Vec<Hit> {
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("xtask-comments-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (rel, body) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        let collected = collect_files(&dir).unwrap();
        let hits = scan(&dir, &collected);
        let _ = std::fs::remove_dir_all(&dir);
        hits
    }

    #[test]
    fn section_citation_in_a_comment_is_rejected() {
        let body = format!("//! El barrido es lo que dice {}.\n", section_citation());
        let hits = scan_fixture(&[("src/demo.rs", body)]);
        assert_eq!(
            hits.len(),
            1,
            "una cita en comentario debe rechazarse: {hits:?}"
        );
        assert_eq!(hits[0].kind, "cita de sección");
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn section_citation_in_an_assertion_diagnostic_is_rejected() {
        let body = format!(
            "assert_eq!(x, y, \"{} no se cumple\");\n",
            section_citation()
        );
        let hits = scan_fixture(&[("tests/demo.rs", body)]);
        assert_eq!(
            hits.len(),
            1,
            "una cita en un diagnóstico de aserción debe rechazarse: {hits:?}"
        );
        assert_eq!(hits[0].kind, "cita de sección");
    }

    #[test]
    fn line_locator_in_a_comment_is_rejected() {
        let body = format!("/// Ver `{}` para la constante.\n", localizador());
        let hits = scan_fixture(&[("src/demo.rs", body)]);
        assert_eq!(hits.len(), 1, "un localizador debe rechazarse: {hits:?}");
        assert_eq!(hits[0].kind, "localizador de línea");
    }

    #[test]
    fn a_path_without_a_line_number_is_accepted() {
        // El nombre del fichero que una función escribe o lee sí se nombra: solo el
        // localizador se rechaza, y esta prueba fija esa frontera.
        let hits = scan_fixture(&[(
            "src/demo.rs",
            "//! Lee `packaging/bundle-manifest.json` como lista canónica.\n".to_string(),
        )]);
        assert!(
            hits.is_empty(),
            "un nombre de fichero sin línea es legítimo: {hits:?}"
        );
    }

    #[test]
    fn a_section_symbol_without_a_number_is_accepted() {
        let body = format!("//! Explica el símbolo {} en la tabla.\n", section_sign());
        let hits = scan_fixture(&[("src/demo.rs", body)]);
        assert!(hits.is_empty(), "sin número no hay cita: {hits:?}");
    }

    #[test]
    fn third_party_and_build_directories_are_skipped() {
        let citation = section_citation();
        let hits = scan_fixture(&[
            ("src/ok.rs", "//! Limpio.\n".to_string()),
            ("vendor/tercero.rs", format!("//! Apunta a {citation}.\n")),
            ("target/build.rs", format!("//! Apunta a {citation}.\n")),
        ]);
        assert!(
            hits.is_empty(),
            "terceros y construcción van fuera: {hits:?}"
        );
    }

    #[test]
    fn an_allowlisted_file_is_not_reported() {
        let body = format!(
            "//! La oferta cita la {} de la licencia.\n",
            section_citation()
        );
        static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1000);
        let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("xtask-comments-allow-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Se replica la estructura real del fichero con supervivencia: la lista blanca
        // compara la ruta relativa, así que basta el mismo caminho relativo.
        let path = dir.join(ALLOWLIST[0].file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &body).unwrap();
        let collected = collect_files(&dir).unwrap();
        let hits = scan(&dir, &collected);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            hits.is_empty(),
            "un fichero de la lista blanca no se reporta: {hits:?}"
        );
    }

    #[test]
    fn every_allowlist_entry_is_still_needed() {
        // La propiedad que impide que la lista blanca se pode: si una entrada deja de ser
        // necesaria, esta prueba falla y obliga a borrarla con su razón o a arreglar el
        // código que la justificaba.
        let base = repo_root();
        for entry in ALLOWLIST {
            let path = base.join(entry.file);
            assert!(
                path.is_file(),
                "la entrada `{}` de la lista blanca no apunta a un fichero que exista",
                entry.file
            );
            let raw = std::fs::read_to_string(&path).expect("fichero legible");
            assert!(
                raw.lines()
                    .any(|l| section_re().is_match(l) || locator_re().is_match(l)),
                "la entrada `{}` de la lista blanca ya no rechaza nada: bórrala con su \
                 razón o arregla el código que la justificaba",
                entry.file
            );
        }
    }

    #[test]
    fn every_allowlist_entry_states_its_reason() {
        for entry in ALLOWLIST {
            assert!(
                entry.reason.len() > 20,
                "la entrada `{}` necesita una razón escrita, no un carácter",
                entry.file
            );
        }
    }

    #[test]
    fn the_repository_is_clean() {
        // La garantía que el control vende, afirmada dentro de la suite para que no
        // dependa de que alguien se acuerde de lanzar la puerta.
        let base = repo_root();
        let files = collect_files(&base).unwrap();
        let hits = scan(&base, &files);
        assert!(
            hits.is_empty(),
            "el árbol de primera parte tiene {} líneas rechazadas: {hits:?}",
            hits.len()
        );
    }
}
