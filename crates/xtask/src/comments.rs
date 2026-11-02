// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask comments --check`: puerta que rechaza tres defectos de documentación: dos
//! formas de documentar que se desalinean con el tiempo en cuanto el código se mueve y
//! un texto corrompido por una conversión de codificación.
//!
//! La primera es la **cita de sección**: un comentario que remite a una sección
//! numerada de un documento en vez de explicar la regla. Un comentario debe ser
//! autocontenido, y una regla escrita que ninguna puerta vigila se degrada en silencio.
//!
//! La segunda es el **localizador de línea**: citar `fichero.ext:NNN` dentro de un
//! comentario. Apunta a una coordenada que se desplaza con la primera edición que toque
//! esas líneas, así que envejece peor que la cita de sección.
//!
//! **Lo que este control NO intenta**, por decisión y no por descuido: no rechaza que
//! un comentario nombre otro fichero. El nombre del fichero que una función escribe o
//! lee es parte legítima de la operación —el manifiesto del bundle, el fichero de
//! sumas, el directorio de una fixture—, y la frontera entre ese nombre y una remisión
//! del tipo "véase X" es un juicio que ninguna herramienta puede hacer. Un control que
//! las rechazara necesitaría una lista blanca que habría que podar cada vez que se toca
//! un comentario, y una lista blanca que se poda es un agujero.
//!
//! El tercer control es el **carácter de reemplazo** (U+FFFD): aparece cuando una
//! conversión de codificación sustituye un byte que no supo decodificar, así que su
//! presencia delata texto corrompido. Tiene su propia lista de tipos porque el texto
//! visible para el usuario no vive solo en los ficheros donde se vigilan las citas: la
//! documentación en Markdown es igual de visible y allí un U+FFFD también es un defecto,
//! mientras que las citas de sección o los localizadores sí pueden ser legítimos en un
//! documento que habla de ellos. Por la misma razón recorre también la documentación
//! que queda fuera de las raíces de código: el árbol `docs` y los `.md` de la raíz. El
//! carácter no se aplica a la lista de supervivencias.
//!
//! El control falla si algún fichero es ilegible o si el escaneo no encuentra ningún
//! fichero: un barrido vacío no demuestra nada.
//!
//! Sin dependencias nuevas: el lexer que hace falta es una expresión regular.

use crate::first_party;
use anyhow::{anyhow, Result};
use regex::Regex;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Tipos de archivo propio que el control lee: las extensiones que puede contener un
/// comentario o un diagnóstico de aserción en este repositorio, más los scripts sin
/// extensión reconocidos por su shebang. Añadir una aquí obliga a revisar si el texto
/// que se rechaza tiene sentido en ese lenguaje.
const KINDS: &[&str] = &[
    "rs",
    "yml",
    "toml",
    "json",
    "sh",
    "bash",
    "bats",
    "ps1",
    first_party::SHEBANG_SCRIPTS,
];

/// Tipos que lee el control del carácter de reemplazo: los de [`KINDS`] más Markdown,
/// porque la documentación es tan visible para el usuario como un comentario y un U+FFFD
/// allí delata texto corrompido por una conversión de codificación. Se mantiene aparte
/// de [`KINDS`] porque ampliar éste sometería los `.md` al control de citas y
/// localizadores, que no se les quiere aplicar.
const ENCODING_KINDS: &[&str] = &[
    "rs",
    "yml",
    "toml",
    "json",
    "sh",
    "bash",
    "bats",
    "ps1",
    "md",
    first_party::SHEBANG_SCRIPTS,
];

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
/// Las dos son la **sección 3.2 de la MPL-2.0**, que es una sección del contrato de
/// licencia y no de la especificación. Este control no las reescribe y no puede hacerlo,
/// porque es el texto que la propia licencia exige publicar.
const ALLOWLIST: &[AllowEntry] = &[AllowEntry {
    file: "crates/xtask/src/main.rs",
    reason: "sección 3.2 de la MPL-2.0 en la plantilla y el anuncio del aviso de \
                 disponibilidad de la fuente; es contrato de licencia, no de la especificación",
}];

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
        println!(
            "Usa --check para escanear las citas de sección, los localizadores de línea y el carácter de reemplazo"
        );
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let files = collect_files(&base)?;
    let encoding_files = collect_encoding_files(&base)?;
    if files.is_empty() {
        return Err(anyhow!(
            "no se encontró ningún fichero de primera parte bajo {}",
            base.display()
        ));
    }
    let hits = scan(&base, &files, &encoding_files)?;

    if hits.is_empty() {
        println!(
            "0 citas de sección ni localizadores de línea en {} ficheros de primera parte, \
             0 caracteres de reemplazo en {}",
            files.len(),
            encoding_files.len()
        );
        return Ok(());
    }
    print_report(&hits);
    print_allowlist();
    let dirty: std::collections::HashSet<&str> = hits.iter().map(|h| h.file.as_str()).collect();
    println!(
        "CITAS, LOCALIZADORES O CARACTERES DE REEMPLAZO: {} en {} ficheros",
        hits.len(),
        dirty.len()
    );
    // Herramienta de desarrollo sin scratch: no hay nada que liberar antes de salir.
    #[allow(clippy::disallowed_methods)]
    std::process::exit(1);
}

/// Recorre los archivos propios y devuelve las líneas rechazadas: primero las citas y
/// localizadores de `files`, en orden de fichero y de línea, y luego los caracteres de
/// reemplazo de `encoding_files`. Un fichero ilegible es un error, no un fichero limpio.
fn scan(base: &Path, files: &[PathBuf], encoding_files: &[PathBuf]) -> Result<Vec<Hit>> {
    let mut hits = Vec::new();
    for file in files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", file.display(), e))?;
        let relative = first_party::relative(base, file);
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
    for file in encoding_files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", file.display(), e))?;
        let relative = first_party::relative(base, file);
        for (index, line) in text.lines().enumerate() {
            if line.contains('\u{FFFD}') {
                hits.push(Hit {
                    file: relative.clone(),
                    line: index + 1,
                    kind: "carácter de reemplazo",
                    text: line.trim().to_string(),
                });
            }
        }
    }
    Ok(hits)
}

/// Si el fichero tiene una supervivencia declarada. La lista es por fichero y no por
/// línea porque la razón es la misma para todas las de un fichero: aquí son las citas
/// de la sección 3.2 de la MPL-2.0, y separarlas daría entradas que alguien tendría
/// que podar en cuanto la línea se moviera.
fn is_allowed(relative: &str) -> bool {
    ALLOWLIST.iter().any(|e| e.file == relative)
}

/// Archivos propios que el control lee, ordenados.
fn collect_files(base: &Path) -> Result<Vec<PathBuf>> {
    first_party::collect(base, KINDS)
}

/// Archivos propios que el control del carácter de reemplazo lee, ordenados: los de
/// las raíces de código más la documentación en Markdown (`docs` y los `.md` de la
/// raíz), que el usuario lee igual que los mensajes del producto.
fn collect_encoding_files(base: &Path) -> Result<Vec<PathBuf>> {
    let mut files = first_party::collect(base, ENCODING_KINDS)?;
    files.extend(first_party::collect_documentation(base)?);
    files.sort();
    files.dedup();
    Ok(files)
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

    fn line_locator() -> String {
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
    /// El directorio es único por llamada y con dueño: las pruebas corren en paralelo
    /// dentro del mismo proceso y dos fixtures no deben compartir nombre, y se borra al
    /// salir aunque la prueba falle.
    fn scan_fixture(files: &[(&str, String)]) -> Vec<Hit> {
        let guard = tempfile::Builder::new()
            .prefix("xtask-comments-")
            .tempdir()
            .unwrap();
        let dir = guard.path().to_path_buf();
        for (rel, body) in files {
            let path = dir.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, body).unwrap();
        }
        let collected = collect_files(&dir).unwrap();
        let encoding = collect_encoding_files(&dir).unwrap();
        scan(&dir, &collected, &encoding).unwrap()
    }

    #[test]
    fn an_unreadable_file_is_an_error() {
        // Un fichero que no es UTF-8 no se puede leer como texto: el escaneo debe
        // fallar y no darlo por limpio.
        let guard = tempfile::Builder::new()
            .prefix("xtask-comments-bin-")
            .tempdir()
            .unwrap();
        let dir = guard.path().to_path_buf();
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("src/bad.rs"), [0xff, 0xfe, 0xfd]).unwrap();
        let collected = collect_files(&dir).unwrap();
        assert!(scan(&dir, &collected, &[]).is_err());
    }

    #[test]
    fn an_empty_scan_is_an_error() {
        let guard = tempfile::Builder::new()
            .prefix("xtask-comments-empty-")
            .tempdir()
            .unwrap();
        assert!(run(true, Some(guard.path())).is_err());
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
        let body = format!("/// Ver `{}` para la constante.\n", line_locator());
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
    fn a_replacement_character_in_rust_source_is_rejected() {
        let body = format!("//! Texto corrupto{}.\n", '\u{FFFD}');
        let hits = scan_fixture(&[("src/demo.rs", body)]);
        assert_eq!(hits.len(), 1, "un U+FFFD debe rechazarse: {hits:?}");
        assert_eq!(hits[0].kind, "carácter de reemplazo");
        assert_eq!(hits[0].line, 1);
    }

    #[test]
    fn a_replacement_character_in_markdown_is_rejected() {
        let body = format!("# Título\n\nTexto corrupto{}.\n", '\u{FFFD}');
        let hits = scan_fixture(&[("crates/demo/README.md", body)]);
        assert_eq!(
            hits.len(),
            1,
            "un U+FFFD en Markdown debe rechazarse: {hits:?}"
        );
        assert_eq!(hits[0].kind, "carácter de reemplazo");
        assert_eq!(hits[0].line, 3);
    }

    #[test]
    fn a_clean_file_without_replacement_characters_passes() {
        let hits = scan_fixture(&[
            (
                "src/demo.rs",
                "//! Texto con acentos: canción.\n".to_string(),
            ),
            (
                "crates/demo/README.md",
                "# Guía\n\nTexto limpio.\n".to_string(),
            ),
        ]);
        assert!(hits.is_empty(), "un fichero limpio debe pasar: {hits:?}");
    }

    #[test]
    fn a_section_symbol_without_a_number_is_accepted() {
        let body = format!("//! Explica el símbolo {} en la tabla.\n", section_sign());
        let hits = scan_fixture(&[("src/demo.rs", body)]);
        assert!(hits.is_empty(), "sin número no hay cita: {hits:?}");
    }

    #[test]
    fn an_allowlisted_file_is_not_reported() {
        let body = format!(
            "//! La oferta cita la {} de la licencia.\n",
            section_citation()
        );
        let guard = tempfile::Builder::new()
            .prefix("xtask-comments-allow-")
            .tempdir()
            .unwrap();
        let dir = guard.path().to_path_buf();
        // Se replica la estructura real del fichero con supervivencia: la lista blanca
        // compara la ruta relativa, así que basta el mismo caminho relativo.
        let path = dir.join(ALLOWLIST[0].file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &body).unwrap();
        let collected = collect_files(&dir).unwrap();
        let hits = scan(&dir, &collected, &[]).unwrap();
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
        let encoding = collect_encoding_files(&base).unwrap();
        let hits = scan(&base, &files, &encoding).unwrap();
        assert!(
            hits.is_empty(),
            "el árbol de primera parte tiene {} líneas rechazadas: {hits:?}",
            hits.len()
        );
    }
}
