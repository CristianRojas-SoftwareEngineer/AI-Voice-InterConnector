//! Enlace estático de los runtimes de GCC que pide la traducción en Linux.
//!
//! Con `native-translation`, `ct2rs` compila CTranslate2 con OpenMP de GNU y
//! pide `-lgomp`, y las piezas C++ (`cxx`, `sentencepiece-sys`, `esaxx-rs`)
//! piden `-lstdc++`. El enlazador busca cada nombre directorio a directorio y,
//! en cada uno, prefiere la `.so` a la `.a`; los directorios de
//! `rustc-link-search` se recorren antes que los del sistema. Este script copia
//! `libgomp.a` y `libstdc++.a` del compilador a un directorio propio sin
//! ninguna `.so` y lo añade a la búsqueda, de modo que ambos nombres resuelven a
//! la estática y el binario no declara `libgomp.so.1`, que una instalación
//! mínima de Linux no trae, ni `libstdc++.so.6`, cuya versión del sistema puede
//! ser más antigua que la del compilador. Las opciones `-static-libstdc++` y
//! `-static-libgcc` del driver no sirven aquí: solo afectan a las librerías que
//! el driver añade por su cuenta, no a las pedidas con `-l`.

use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

/// Librerías estáticas que se sirven en el directorio propio.
const STATIC_RUNTIMES: [&str; 2] = ["libgomp.a", "libstdc++.a"];

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CC");
    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "linux" || env::var_os("CARGO_FEATURE_NATIVE_TRANSLATION").is_none() {
        return;
    }
    let cc = env::var("CC").unwrap_or_else(|_| "cc".to_string());
    let out_dir = PathBuf::from(env::var("OUT_DIR").expect("cargo define OUT_DIR"));
    let dir = out_dir.join("static-runtime");
    fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("no se pudo crear {}: {e}", dir.display()));
    for name in STATIC_RUNTIMES {
        let source = locate(&cc, name);
        let dest = dir.join(name);
        fs::copy(&source, &dest).unwrap_or_else(|e| {
            panic!(
                "no se pudo copiar {} a {}: {e}",
                source.display(),
                dest.display()
            )
        });
    }
    println!("cargo:rustc-link-search=native={}", dir.display());
}

/// Ruta absoluta de `name` según el compilador `cc`. Sin la librería, el
/// compilador imprime el nombre tal cual, sin ruta, y la compilación falla
/// nombrando el paquete que la aporta.
fn locate(cc: &str, name: &str) -> PathBuf {
    let output = Command::new(cc)
        .arg(format!("-print-file-name={name}"))
        .output()
        .unwrap_or_else(|e| panic!("no se pudo ejecutar `{cc}` para localizar {name}: {e}"));
    let path = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim());
    if !output.status.success() || !path.is_absolute() || !path.is_file() {
        panic!(
            "`{cc}` no encuentra {name}, necesaria para enlazar los runtimes de GCC de forma \
             estática en Linux. La aportan los paquetes de desarrollo de GCC: \
             `libgcc-<versión>-dev` y `libstdc++-<versión>-dev` en Debian/Ubuntu, o el \
             gcc-toolset de la imagen manylinux_2_28 en EL8"
        );
    }
    path
}
