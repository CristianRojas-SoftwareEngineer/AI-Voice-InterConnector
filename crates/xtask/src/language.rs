//! `xtask language --check`: escáner de identificadores en español del código de
//! primera parte.
//!
//! Sin dependencias nuevas: las letras acentuadas se pliegan con una tabla
//! propia en lugar de un crate de normalización Unicode, porque solo hacen falta
//! las del español.
//!
//! El escáner falla si algún fichero es ilegible o si no encuentra ningún fichero
//! que escanear: un barrido vacío no demuestra nada.
//!
//! El recorrido es el inverso del problema: **antes** de tokenizar se apagan
//! comentarios y literales, porque un `//` en español o una cadena con
//! `resultado` no son identificadores y no pueden disparar el escáner.

use anyhow::{anyhow, Result};
use regex::Regex;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Raíces de código de primera parte que se recorren, en orden. Una raíz que no
/// exista se ignora: el fallo ante un escaneo sin ficheros lo da `run`.
const ROOTS: &[&str] = &["src", "crates", "tests"];

/// Componente de ruta que marca código de terceros vendorizado: se excluye.
const VENDOR: &str = "vendor";

/// Longitud mínima de un segmento ya normalizado para que se considere marca de
/// identificador en español. **4 y no 5**: `solo` y `caso` son tan castellanas
/// como `linea`, y con 5 se escapaban.
const MIN_SEGMENT: usize = 4;

/// Palabras castellanas que delatan un identificador en español. Cualquier palabra
/// que también sea inglés válido queda fuera a propósito.
///
/// `todo` se deja fuera **a propósito**, aunque sea castellana: en este código es
/// el marcador **inglés** del CHANGELOG (`## [No publicado]` se cura con
/// `<!-- TODO: curar -->`), y `with_todo` y
/// `test_promote_changelog_fails_with_leftover_todo` en `main.rs` lo demuestran.
/// Meterlo daría falsos positivos en `main.rs` (`with_todo` y los
/// `test_..._with_leftover_todo`). Si el marcador pasa a español, entra en la lista.
///
/// `total` tampoco entra, por la misma regla: es inglés válido y el árbol lo usa
/// como tal (`total_bytes`, `wer_total`, `total_samples`, `total_steps`,
/// `total_cmp`, `STREAM_TOTAL_DEADLINE`, `t_total` y un `total` acumulador).
/// Igual que `variable`, es inglés bien puesto y por eso no es deuda.
///
/// La lista no tiene duplicados, lo que una prueba fija.
const ES_WORDS: &[&str] = &[
    "ancestro",
    "anunciado",
    "archivo",
    "archivos",
    "atribuciones",
    "cambiado",
    "candidatos",
    "caso",
    "causa",
    "clave",
    "codigo",
    "cobertura",
    "contenido",
    "contenidos",
    "convertir",
    "crear",
    "curado",
    "dentro",
    "descargalos",
    "desarrollo",
    "desinstalacion",
    "despues",
    "destino",
    "directorio",
    "directorios",
    "durante",
    "envenenado",
    "escribir",
    "estado",
    "existe",
    "fallar",
    "fallido",
    "fallo",
    "fichero",
    "ficheros",
    "fuente",
    "habla",
    "hasta",
    "hecho",
    "incluido",
    "incluidos",
    "inexistente",
    "instalado",
    "leido",
    "leidos",
    "limpiado",
    "limpiar",
    "linea",
    "lineas",
    "marca",
    "marcado",
    "marcar",
    "mensaje",
    "metodo",
    "modelo",
    "modelos",
    "modo",
    "nunca",
    "numero",
    "numeros",
    "oferta",
    "orden",
    "padre",
    "pregunta",
    "preguntados",
    "previo",
    "primera",
    "prohibido",
    "programa",
    "propio",
    "propios",
    "publicada",
    "publicado",
    "razon",
    "raiz",
    "raices",
    "recibo",
    "resultado",
    "seccion",
    "seleccion",
    "sembrar",
    "separador",
    "simulacion",
    "sobre",
    "tamano",
    "temporal",
    "temporales",
    "tenemos",
    "texto",
    "tiene",
    "tienen",
    "tipo",
    "tipos",
    "unicos",
    "unico",
    "vacios",
    "vacio",
    "valor",
    "valores",
    "vecina",
    "verbo",
    "viajan",
    "vienen",
    "vistas",
    "vivo",
    "residente",
    "diario",
    "registro",
    "entradas",
    "canal",
    "enlazado",
    "falta",
    "faltan",
    "esperado",
    "esperada",
    "duplicado",
    "duplicada",
    "interrumpir",
    "integracion",
    "recuperacion",
    "instalacion",
    "sincronizar",
    "pendiente",
    "pendientes",
    "proceso",
    "procesos",
    "sesion",
    "transaccion",
    "verificar",
    "comprobar",
    "ejecutar",
    "leer",
    "nuevo",
    "nueva",
    "anterior",
    "siguiente",
    "copia",
    "copias",
    "mover",
    "abrir",
    "cerrar",
    "listo",
    "activo",
    "inactivo",
    "principal",
    "secundario",
    "exito",
    "borrar",
    "nombre",
    "nombres",
    "ruta",
    "rutas",
    "usuario",
    "usuarios",
    "operacion",
    "huerfano",
    "huerfanos",
    "resumen",
    "correcto",
    "resto",
    "madre",
    "anadir",
    "solo",
    // Seis palabras —`fin`, `uno`, `dos`, `eso`, `si` y `no`— están por debajo del
    // umbral de 4 caracteres, así que no pueden disparar nada: quedan por
    // completitud de vocabulario.
    "ventana",
    "arbol",
    "casilla",
    "hoja",
    "bosque",
    "nodo",
    "capa",
    "fila",
    "columna",
    "ancho",
    "alto",
    "llave",
    "paso",
    "tramo",
    "zona",
    "bloque",
    "grupo",
    "cola",
    "pila",
    "suma",
    "media",
    "minimo",
    "maximo",
    "inicio",
    "fin",
    "aviso",
    "alerta",
    "avanzar",
    "retroceder",
    "copiar",
    "guardar",
    "cargar",
    "destruir",
    "eliminar",
    "quitar",
    "poner",
    "obtener",
    "buscar",
    "encontrar",
    "filtrar",
    "ordenar",
    "contar",
    "comparar",
    "lleno",
    "llena",
    "viejo",
    "proximo",
    "futuro",
    "cada",
    "nada",
    "otro",
    "mismo",
    "cual",
    "cuando",
    "donde",
    "aqui",
    "uno",
    "dos",
    "tres",
    "casi",
    "esta",
    "este",
    "eso",
    "si",
    "no",
    "tambien",
    "antes",
    "entre",
    "bajo",
    "abierto",
    "aborto",
    "agotado",
    "aislada",
    "ajeno",
    "aparcado",
    "aparcar",
    "apunta",
    "atributo",
    "atributos",
    "ausente",
    "barrido",
    "basura",
    "borrado",
    "canales",
    "centinela",
    "clasifica",
    "clon",
    "clonado",
    "colision",
    "colocado",
    "colocar",
    "comando",
    "compartida",
    "confirmacion",
    "confirmado",
    "conflicto",
    "conocido",
    "conserva",
    "conservado",
    "constante",
    "corto",
    "cosa",
    "criterio",
    "datos",
    "delega",
    "demostracion",
    "deriva",
    "derivado",
    "descargado",
    "desconocidos",
    "desenlaces",
    "desinstalar",
    "despedida",
    "devuelve",
    "diferido",
    "dispositivo",
    "doblaje",
    "doble",
    "duro",
    "edad",
    "ejecutable",
    "encontrada",
    "entrada",
    "entregue",
    "espanol",
    "espera",
    "esperar",
    "estricto",
    "etapa",
    "etiqueta",
    "evento",
    "eventos",
    "exclusiva",
    "existente",
    "existio",
    "expandir",
    "falla",
    "fallida",
    "fallos",
    "falsa",
    "falso",
    "grabacion",
    "hermano",
    "herramienta",
    "herramientas",
    "hijo",
    "hola",
    "idempotencia",
    "identidad",
    "identidades",
    "igual",
    "ilegible",
    "imprime",
    "incompleto",
    "indice",
    "inerte",
    "inertes",
    "informe",
    "integrada",
    "interrumpida",
    "invalida",
    "invariante",
    "inyeccion",
    "inyectado",
    "jamas",
    "larga",
    "largo",
    "latidos",
    "lazo",
    "limite",
    "limpiador",
    "limpio",
    "llegaron",
    "localizador",
    "mata",
    "matriz",
    "medida",
    "menos",
    "migracion",
    "muerto",
    "necesita",
    "notas",
    "nuestro",
    "nulas",
    "objetivo",
    "objeto",
    "obligatorio",
    "observa",
    "obsoleta",
    "obsoleto",
    "otra",
    "otros",
    "paquete",
    "peor",
    "permisos",
    "personalizado",
    "pesos",
    "pines",
    "plazo",
    "poda",
    "programable",
    "programado",
    "programar",
    "programas",
    "propietario",
    "prueba",
    "purga",
    "quedan",
    "rearma",
    "rechazado",
    "reciba",
    "recorrido",
    "recupera",
    "recuperar",
    "recurso",
    "rellenar",
    "reloj",
    "reparacion",
    "reproduccion",
    "requisito",
    "requisitos",
    "residuo",
    "resolucion",
    "respuestas",
    "restante",
    "retenido",
    "reubicada",
    "reubicadas",
    "reubicado",
    "reversion",
    "revierte",
    "roto",
    "salida",
    "salir",
    "saludo",
    "segun",
    "segunda",
    "servidor",
    "simulacro",
    "simulado",
    "sintesis",
    "sintetizada",
    "sistema",
    "sonda",
    "sondea",
    "suelta",
    "tenga",
    "tercero",
    "tomar",
    "traduccion",
    "transcurrido",
    "truncado",
    "vacia",
    "validan",
    "veneno",
    "vigente",
    "vistos",
    "voces",
    "vuelta",
];

/// Un hallazgo: el identificador tal cual aparece en el código, y los segmentos
/// normalizados que lo marcan, unidos por coma (columna `word` del informe).
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Hit {
    id: String,
    words: String,
}

/// Conjunto de palabras castellanas, en minúsculas (la lista ya lo está).
fn word_set() -> &'static HashSet<&'static str> {
    static WORDS: OnceLock<HashSet<&'static str>> = OnceLock::new();
    WORDS.get_or_init(|| ES_WORDS.iter().copied().collect())
}

/// Tokenizador de identificadores. **Unicode y no ASCII**: Rust admite
/// identificadores no ASCII, y una letra fuera de la clase ASCII rompía la
/// tokenización (`añadir` se leía como `adir`, que además caía por el gate de
/// longitud y era invisible).
fn identifier_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"[_\p{L}][\p{L}\p{N}_]*").unwrap())
}

/// Entrada del subcomando `language`. `root` sobrescribe la raíz a escanear
/// (por defecto, el directorio actual): sin eso no se pueden escanear fixtures.
pub(crate) fn run(check: bool, root: Option<&Path>) -> Result<()> {
    if !check {
        println!("Usa --check para escanear los identificadores del código de primera parte");
        return Ok(());
    }
    let base = root.map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let files = collect_rust_files(&base)?;
    if files.is_empty() {
        return Err(anyhow!(
            "no se encontró ningún fichero .rs de primera parte bajo {}",
            base.display()
        ));
    }
    let mut rows: Vec<(String, Hit)> = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow!("no se pudo leer {}: {}", file.display(), e))?;
        let relative = file
            .strip_prefix(&base)
            .expect("todo fichero recorrido cuelga de la raíz")
            .display()
            .to_string();
        for hit in spanish_identifiers(&strip_code(&text)) {
            rows.push((relative.clone(), hit));
        }
    }

    if rows.is_empty() {
        println!(
            "0 identificadores en español en {} ficheros de primera parte",
            files.len()
        );
        return Ok(());
    }
    rows.sort();
    print_table(&rows);
    let dirty: HashSet<&str> = rows.iter().map(|(file, _)| file.as_str()).collect();
    println!(
        "IDENTIFICADORES EN ESPAÑOL: {} en {} ficheros",
        rows.len(),
        dirty.len()
    );
    std::process::exit(1);
}

/// Ficheros `.rs` bajo las raíces de primera parte, ordenados. Las raíces que no
/// existen se ignoran y cualquier ruta con un componente `vendor` relativo a la
/// base queda fuera: es código de terceros.
fn collect_rust_files(base: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for root in ROOTS {
        let dir = base.join(root);
        if !dir.is_dir() {
            continue;
        }
        walk(base, &dir, &mut files)?;
    }
    files.sort();
    Ok(files)
}

/// Recorrido en profundidad de un directorio, en orden de entrada.
fn walk(base: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let relative = dir
        .strip_prefix(base)
        .expect("todo directorio recorrido cuelga de la raíz");
    if is_vendor_path(relative) {
        return Ok(());
    }
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            walk(base, &path, files)?;
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            files.push(path);
        }
    }
    Ok(())
}

/// ¿La ruta, relativa a la base escaneada, tiene un componente `vendor`? Solo se
/// mira la parte relativa: escanear un repo que cuelgue de un directorio `vendor`
/// no debe dejar el escaneo vacío.
fn is_vendor_path(relative: &Path) -> bool {
    relative
        .components()
        .any(|component| component.as_os_str() == VENDOR)
}

/// Devuelve el código con comentarios y literales apagados: cada `//` (que
/// cubre `///` y `//!`), cada bloque `/* */` —con profundidad, porque Rust los
/// anida—, cada *raw string* (`r"…"`, `r#"…"#`, `r##"…"##`), cada cadena normal
/// con escapes y cada literal de carácter se sustituyen por un espacio. Los
/// saltos de línea se conservan dentro de comentarios y cadenas para no alterar
/// la numeración de líneas del texto resultante.
///
/// Un lifetime (`'a`) no es un literal y **no** se apaga: solo se consume una
/// comilla si cierra en tres caracteres (`'a'`, `'\n'`).
fn strip_code(text: &str) -> String {
    let src: Vec<char> = text.chars().collect();
    let n = src.len();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < n {
        let c = src[i];
        if c == '/' && i + 1 < n && src[i + 1] == '/' {
            while i < n && src[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && src[i + 1] == '*' {
            let mut depth = 1;
            i += 2;
            while i < n && depth > 0 {
                if src[i] == '/' && i + 1 < n && src[i + 1] == '*' {
                    depth += 1;
                    i += 2;
                    continue;
                }
                if src[i] == '*' && i + 1 < n && src[i + 1] == '/' {
                    depth -= 1;
                    i += 2;
                    continue;
                }
                if src[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            continue;
        }
        if c == 'r' && i + 1 < n && (src[i + 1] == '"' || src[i + 1] == '#') {
            let mut j = i + 1;
            let mut hashes = 0;
            while j < n && src[j] == '#' {
                hashes += 1;
                j += 1;
            }
            if j < n && src[j] == '"' {
                j += 1;
                i = raw_string_end(&src, j, hashes);
                out.push(' ');
                continue;
            }
        }
        if c == '"' {
            i += 1;
            while i < n {
                if src[i] == '\\' {
                    i += 2;
                    continue;
                }
                if src[i] == '"' {
                    i += 1;
                    break;
                }
                if src[i] == '\n' {
                    out.push('\n');
                }
                i += 1;
            }
            out.push(' ');
            continue;
        }
        if c == '\'' {
            // Literal de carácter o lifetime: solo se consume si cierra en tres.
            if i + 2 < n && src[i + 2] == '\'' {
                i += 3;
                out.push(' ');
                continue;
            }
            out.push(c);
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Índice del primer terminador de un *raw string* a partir de `from`: una
/// comilla seguida de `hashes` almohadillas. Devuelve el índice **posterior** al
/// terminador, o el final del texto si no lo hay (raw string sin cerrar).
fn raw_string_end(src: &[char], from: usize, hashes: usize) -> usize {
    let term_len = 1 + hashes;
    let mut k = from;
    while k + term_len <= src.len() {
        if src[k] == '"' && src[k + 1..k + term_len].iter().all(|&c| c == '#') {
            return k + term_len;
        }
        k += 1;
    }
    src.len()
}

/// Identificadores del código (ya sin comentarios ni literales) que tienen algún
/// segmento castellano.
fn spanish_identifiers(code: &str) -> Vec<Hit> {
    let words = word_set();
    identifier_re()
        .find_iter(code)
        .filter_map(|m| {
            let id = m.as_str();
            let marked: Vec<String> = segments(id)
                .into_iter()
                .filter(|segment| {
                    segment.chars().count() >= MIN_SEGMENT
                        && words.contains(segment.to_lowercase().as_str())
                })
                .collect();
            if marked.is_empty() {
                return None;
            }
            Some(Hit {
                id: id.to_string(),
                words: marked.join(","),
            })
        })
        .collect()
}

/// Parte un identificador en sus segmentos, tanto por `_` como por frontera
/// camelCase: sin esto, `resultadoFinal` y `ruta_destino` serían invisibles.
/// Cada segmento sale sin diacritics, con la capitalización original (lo que se
/// compara con la lista es su versión en minúsculas).
fn segments(id: &str) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in id.split('_') {
        if chunk.is_empty() {
            continue;
        }
        let chars: Vec<char> = chunk.chars().collect();
        let mut piece = String::new();
        for (i, &c) in chars.iter().enumerate() {
            if i > 0 && is_camel_boundary(&chars, i) {
                out.push(strip_diacritics(&piece));
                piece.clear();
            }
            piece.push(c);
        }
        out.push(strip_diacritics(&piece));
    }
    out
}

/// Frontera camelCase en la posición `i`: minúscula o dígito seguido de mayúscula
/// (`resultadoFinal`), o mayúscula entre mayúscula y minúscula (`XMLArchivo`).
fn is_camel_boundary(chars: &[char], i: usize) -> bool {
    let previous = chars[i - 1];
    let current = chars[i];
    if previous.is_ascii_lowercase() || previous.is_ascii_digit() {
        return current.is_ascii_uppercase();
    }
    previous.is_ascii_uppercase()
        && current.is_ascii_uppercase()
        && chars
            .get(i + 1)
            .is_some_and(|next| next.is_ascii_lowercase())
}

/// Equivalencia de una letra acentuada española con su forma ASCII base.
///
/// La tabla es **deliberadamente parcial**: cubre `á é í ó ú ü ñ` y sus
/// mayúsculas, y **no** hace una descomposición Unicode completa. No cubre, por
/// tanto, el resto de letras latinas precompuestas (`à â ä ç ë î ï ô õ ù û œ å`),
/// ni las marcas combinantes que ya vinieran sueltas en el fuente, ni los
/// alfabetos no latinos (cirílico, griego…). Se acepta porque la lista de
/// palabras es castellana y el código de primera parte es ASCII: lo que la tabla
/// no cubre es lo que no puede aparecer en la lista. Una descomposición completa
/// exigiría un crate de normalización Unicode para resolver solo las letras del
/// español, que esta tabla ya cubre.
fn base_letter(c: char) -> char {
    match c {
        'á' | 'Á' => 'a',
        'é' | 'É' => 'e',
        'í' | 'Í' => 'i',
        'ó' | 'Ó' => 'o',
        'ú' | 'Ú' | 'ü' | 'Ü' => 'u',
        'ñ' | 'Ñ' => 'n',
        other => other,
    }
}

/// Quita los diacritics de las letras españolas. La comparación con la lista
/// (escrita sin acentos) **exige** normalizar antes: sin esto, `código` pasaría
/// el filtro.
fn strip_diacritics(s: &str) -> String {
    s.chars().map(base_letter).collect()
}

/// Informe tabular de los hallazgos, con las columnas ajustadas al contenido.
fn print_table(rows: &[(String, Hit)]) {
    let headers = ["file", "id", "word"];
    let cells: Vec<[&str; 3]> = rows
        .iter()
        .map(|(file, hit)| [file.as_str(), hit.id.as_str(), hit.words.as_str()])
        .collect();
    let mut widths = [0usize; 3];
    for (i, header) in headers.iter().enumerate() {
        widths[i] = header.chars().count();
    }
    for row in &cells {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let render = |row: [&str; 3]| {
        row.iter()
            .enumerate()
            .map(|(i, cell)| format!("{cell:<width$}", width = widths[i]))
            .collect::<Vec<String>>()
            .join("  ")
            .trim_end()
            .to_string()
    };
    println!("{}", render(headers));
    println!(
        "{}",
        widths
            .iter()
            .map(|width| "-".repeat(*width))
            .collect::<Vec<String>>()
            .join("  ")
    );
    for row in &cells {
        println!("{}", render([row[0], row[1], row[2]]));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Identificadores que el escáner marca en un fragmento de código.
    fn detected(source: &str) -> Vec<String> {
        spanish_identifiers(&strip_code(source))
            .into_iter()
            .map(|hit| hit.id)
            .collect()
    }

    /// Un `//` en español no dispara: los comentarios se apagan antes de
    /// tokenizar, y `///` y `//!` son comentarios de línea igual que `//`.
    #[test]
    fn test_spanish_line_comments_do_not_fire() {
        let source = "// resultado_valido, ruta_destino, archivoTemporal\n\
                      /// codigo_fuente\n\
                      //! entradas_pendientes\n\
                      let english_name = 1;\n";
        assert!(detected(source).is_empty());
    }

    /// Un bloque `/* */` anidado se apaga entero, hasta su cierre externo; lo que
    /// viene después, no.
    #[test]
    fn test_nested_block_comment_is_silenced_whole() {
        let source = "/* ruta /* destino /* archivo */ linea */ bloque */\nlet english = 1;\n";
        assert!(detected(source).is_empty());
        assert_eq!(
            detected("/* /* ruta */ */ let ruta = 1;\n"),
            vec!["ruta".to_string()]
        );
    }

    /// Un *raw string* se apaga completo, con sus almohadillas y sus comillas
    /// interiores, y en las tres formas (`r"…"`, `r#"…"#`, `r##"…"##`).
    #[test]
    fn test_raw_strings_are_silenced_completely() {
        assert!(detected("let r = r#\"resultado ruta_destino\"#;\n").is_empty());
        assert!(detected("let r = r\"resultado\";\n").is_empty());
        assert!(detected("let r = r##\"resultado \"#\" ruta\"##;\n").is_empty());
    }

    /// Cadena normal con escapes y literal de carácter también se apagan.
    #[test]
    fn test_strings_and_chars_are_silenced() {
        assert!(detected("let s = \"resultado \\\" ruta\";\n").is_empty());
        assert!(detected("let c = 'r';\nlet d = 'a';\n").is_empty());
    }

    /// Un lifetime `'a` no es un literal: no puede apagar lo que viene detrás.
    #[test]
    fn test_lifetime_is_not_silenced() {
        let source = "fn borrow<'a>(value: &'a str) -> &'a str { value }\n";
        assert!(detected(source).is_empty());
        let source = "fn borrow<'a>(resultado: &'a str) -> &'a str { resultado }\n";
        assert_eq!(
            detected(source),
            vec!["resultado".to_string(), "resultado".to_string()]
        );
    }

    /// El troceado: por `_` y por frontera camelCase, en ambos sentidos.
    #[test]
    fn test_camel_case_splits_into_segments() {
        assert_eq!(segments("resultadoFinal"), ["resultado", "Final"]);
        assert_eq!(segments("rutaDestino"), ["ruta", "Destino"]);
        assert_eq!(segments("VERSION_ANTERIOR"), ["VERSION", "ANTERIOR"]);
        // Sin minúscula detrás no hay frontera: `XML` no se parte.
        assert_eq!(segments("XMLArchivo"), ["XML", "Archivo"]);
        assert_eq!(segments("archivoHTML"), ["archivo", "HTML"]);
        assert_eq!(segments("__"), Vec::<String>::new());
    }

    /// Un identificador con `ñ` se tokeniza entero: con una clase ASCII,
    /// `añadir` se leía como `adir` y pasaba el filtro.
    #[test]
    fn test_unicode_identifier_is_tokenized_whole() {
        assert_eq!(detected("fn añadir() {}\n"), ["añadir".to_string()]);
    }

    /// Los diacritics se quitan **antes** de comparar: la lista está escrita sin
    /// acentos, así que sin normalizar `código` pasaría el filtro.
    #[test]
    fn test_diacritics_are_normalized_before_matching() {
        assert_eq!(strip_diacritics("código"), "codigo");
        // La mayúscula acentuada cae a su forma ASCII en minúscula, que es lo
        // que se compara contra la lista.
        assert_eq!(strip_diacritics("ÁÉÍÓÚÜÑ"), "aeiouun");
        assert_eq!(detected("fn código() {}\n"), ["código".to_string()]);
        assert_eq!(detected("fn número() {}\n"), ["número".to_string()]);
    }

    /// Las palabras que se corrigieron a inglés en los renombrados del código
    /// quedan en la lista, de modo que una vuelta al español se detecta.
    #[test]
    fn test_corrected_names_are_detected() {
        for source in [
            "fn esperar_latidos() {}\n",
            "let plazo = 1;\n",
            "fn tomar_lock() {}\n",
            "const AVI_RAIZ: u32 = 0;\nfn resolucion_mtime() {}\n",
            "let limite = 3;\n",
        ] {
            assert!(!detected(source).is_empty(), "no se detectó: {source}");
        }
    }

    /// El gate es de 4 caracteres, no de 5: `solo` y `caso` son tan castellanas
    /// como `linea`. Y por debajo del gate no se marca nada, aunque la palabra
    /// esté en la lista.
    #[test]
    fn test_four_character_gate() {
        assert_eq!(detected("fn solo() {}\n"), ["solo".to_string()]);
        assert_eq!(detected("fn caso() {}\n"), ["caso".to_string()]);
        assert!(detected("fn no() {}\nfn si() {}\nfn fin() {}\nfn dos() {}\n").is_empty());
    }

    /// `todo` no entra en la lista: en este repo es el marcador inglés del
    /// CHANGELOG, no la palabra castellana.
    #[test]
    fn test_todo_is_not_flagged() {
        assert!(!word_set().contains("todo"));
        assert!(detected("fn with_todo() {}\n").is_empty());
    }

    /// Control de falsos positivos: identificadores ingleses intactos.
    #[test]
    fn test_english_identifiers_are_not_flagged() {
        let source = "fn route(file: &str) -> String { file.to_string() }\n\
                      struct File { route: String, cache: Vec<String> }\n";
        assert!(detected(source).is_empty());
    }

    /// La lista no tiene duplicados.
    #[test]
    fn test_word_list_has_no_duplicates() {
        let mut unique: HashSet<&str> = HashSet::new();
        for word in ES_WORDS {
            assert!(unique.insert(word), "`{word}` está duplicado en la lista");
        }
    }

    /// El conjunto de raíces y la exclusión de `vendor` son la puerta de
    /// alcance: lo que no es código de primera parte no se escanea.
    #[test]
    fn test_scan_roots_and_vendor_exclusion() {
        assert_eq!(ROOTS, ["src", "crates", "tests"]);
        assert!(is_vendor_path(Path::new(
            "repo/vendor/qwen3-tts/src/lib.rs"
        )));
        assert!(is_vendor_path(Path::new("vendor/repo/src/main.rs")));
        assert!(!is_vendor_path(Path::new(
            "repo/crates/avi-core/src/lib.rs"
        )));
    }

    /// Una base que cuelga de un directorio `vendor` se escanea igual: la
    /// exclusión mira solo la parte relativa a la base.
    #[test]
    fn test_base_under_vendor_directory_is_scanned() {
        let guard = tempfile::Builder::new()
            .prefix("xtask-language-")
            .tempdir()
            .unwrap();
        let base = guard.path().join("vendor").join("repo");
        std::fs::create_dir_all(base.join("src")).unwrap();
        std::fs::write(base.join("src/lib.rs"), "fn english() {}\n").unwrap();
        let files = collect_rust_files(&base).unwrap();
        assert_eq!(files.len(), 1);
    }

    /// Un escaneo sin ficheros es un error, no un pase.
    #[test]
    fn test_empty_scan_is_an_error() {
        let guard = tempfile::Builder::new()
            .prefix("xtask-language-empty-")
            .tempdir()
            .unwrap();
        assert!(run(true, Some(guard.path())).is_err());
    }
}
