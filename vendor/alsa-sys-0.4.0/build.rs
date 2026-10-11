// Patch: el original sondeaba ALSA con `pkg-config` y la enlazaba, de modo que
// todo binario que usara el crate exigía `libasound.so.2` para arrancar, aunque
// nunca tocara el audio. Este `build.rs` reescribe `src/generated.rs` en
// `OUT_DIR/bindings.rs`: los tipos quedan iguales y cada función externa se
// convierte en una función Rust con la misma firma que resuelve su símbolo con
// `dlsym` sobre `libasound.so.2`, cargada con `dlopen` en el primer uso. Las
// funciones variádicas (`snd_input_scanf`, `snd_output_printf`,
// `snd_config_searchv`) no se pueden reenviar desde Rust estable y se omiten.
//
// La compilación falla si la salida conserva algún bloque `extern "C"`, que
// volvería a enlazar ALSA, o si `src/required_symbols.rs` nombra una función
// sin envoltura.

use std::collections::BTreeSet;
use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=src/generated.rs");
    println!("cargo:rerun-if-changed=src/required_symbols.rs");
    let source = fs::read_to_string("src/generated.rs").expect("se lee src/generated.rs");
    let (bindings, wrapped) = transform(&source);
    for marker in ["extern \"C\" {", "extern \"C\"{"] {
        if let Some(at) = bindings.find(marker) {
            let end = bindings[at..].find('\n').map_or(bindings.len(), |n| at + n);
            panic!(
                "queda un bloque externo sin transformar en bindings.rs: {}",
                &bindings[at..end]
            );
        }
    }
    let required =
        fs::read_to_string("src/required_symbols.rs").expect("se lee src/required_symbols.rs");
    let names = required_names(&required);
    assert!(!names.is_empty(), "src/required_symbols.rs no declara ningún símbolo");
    for name in names {
        assert!(
            wrapped.contains(name),
            "src/required_symbols.rs nombra {name}, que no tiene envoltura en src/generated.rs"
        );
    }
    let out = PathBuf::from(env::var("OUT_DIR").expect("cargo define OUT_DIR"));
    fs::write(out.join("bindings.rs"), bindings).expect("se escribe bindings.rs");
}

/// Nombres de los literales `c"…",` de `src/required_symbols.rs`, uno por
/// línea.
fn required_names(source: &str) -> Vec<&str> {
    source
        .lines()
        .filter_map(|line| line.trim().strip_prefix("c\""))
        .map(|rest| rest.strip_suffix("\",").expect("literal `c\"…\",` cerrado"))
        .collect()
}

/// Copia `source` sustituyendo cada bloque `unsafe extern "C" { ... }` por las
/// envolturas de sus funciones, y devuelve también los nombres envueltos.
fn transform(source: &str) -> (String, BTreeSet<String>) {
    let mut out = String::with_capacity(source.len() * 2);
    let mut wrapped = BTreeSet::new();
    let mut lines = source.lines();
    while let Some(line) = lines.next() {
        if line != "unsafe extern \"C\" {" {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        // bindgen cierra cada bloque con una línea que es solo `}`.
        let mut body = String::new();
        for inner in lines.by_ref() {
            if inner == "}" {
                break;
            }
            body.push_str(inner.trim());
            body.push(' ');
        }
        for decl in body.split(';').map(str::trim).filter(|d| !d.is_empty()) {
            if let Some((name, code)) = wrapper(decl) {
                out.push_str(&code);
                wrapped.insert(name);
            }
        }
    }
    (out, wrapped)
}

/// Nombre y envoltura de una declaración `pub fn nombre(parámetros) -> retorno`;
/// `None` si es variádica.
fn wrapper(decl: &str) -> Option<(String, String)> {
    let rest = decl
        .strip_prefix("pub fn ")
        .unwrap_or_else(|| panic!("declaración externa inesperada: {decl}"));
    let open = rest.find('(').expect("la función tiene parámetros");
    let name = &rest[..open];
    let close = matching_paren(rest, open);
    let params = rest[open + 1..close].trim().trim_end_matches(',');
    let ret = rest[close + 1..].trim();
    let args: Vec<&str> = split_top_level(params)
        .into_iter()
        .map(|p| p.split(':').next().expect("parámetro con nombre").trim())
        .collect();
    if args.contains(&"...") {
        return None;
    }
    let code = format!(
        "#[inline]\n\
         pub unsafe fn {name}({params}) {ret} {{\n\
         \x20   static SYMBOL: ::core::sync::atomic::AtomicPtr<::core::ffi::c_void> =\n\
         \x20       ::core::sync::atomic::AtomicPtr::new(::core::ptr::null_mut());\n\
         \x20   let f: unsafe extern \"C\" fn({params}) {ret} =\n\
         \x20       ::core::mem::transmute(crate::dynamic::resolve(&SYMBOL, c\"{name}\"));\n\
         \x20   f({args})\n\
         }}\n",
        args = args.join(", ")
    );
    Some((name.to_string(), code))
}

/// Índice del `)` que cierra el `(` de `open`.
fn matching_paren(text: &str, open: usize) -> usize {
    let mut depth = 0usize;
    for (i, c) in text.char_indices().skip_while(|&(i, _)| i < open) {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return i;
                }
            }
            _ => {}
        }
    }
    panic!("paréntesis sin cerrar en: {text}");
}

/// Separa la lista de parámetros por las comas de primer nivel: un tipo de
/// puntero a función lleva paréntesis y comas propios, y un genérico, `<>`
/// (la flecha `->` no abre ni cierra nada).
fn split_top_level(params: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    let bytes = params.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'<' => depth += 1,
            b')' => depth -= 1,
            b'>' if i == 0 || bytes[i - 1] != b'-' => depth -= 1,
            b',' if depth == 0 => {
                parts.push(params[start..i].trim());
                start = i + 1;
            }
            _ => {}
        }
    }
    let last = params[start..].trim();
    if !last.is_empty() {
        parts.push(last);
    }
    parts
}
