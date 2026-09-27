//! Pines versionados del repositorio (`packaging/pins.json`).
//!
//! La CI y Rust leen este archivo tal cual; nadie replica sus valores en el
//! código. `ort` es la versión de ONNX Runtime que empaqueta `xtask package` y
//! que el motor espera; `rust` fija la toolchain (`rust-toolchain.toml`).

use std::path::{Path, PathBuf};

/// Manifiesto de pines, relativo a la raíz del repositorio.
pub const PINS_REL: &str = "packaging/pins.json";

/// Pines versionados: un campo por parámetro de la CI.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Pins {
    pub rust: String,
    pub ort: String,
    pub sccache: String,
    pub msys2_base: String,
    pub msys2_gcc: String,
    pub msys2_openblas: String,
    pub msys2_make: String,
    pub ninja: String,
}

/// Parsea el texto de `pins.json`.
pub fn parse_text(text: &str) -> Result<Pins, String> {
    serde_json::from_str(text).map_err(|e| format!("pins.json no es JSON válido: {e}"))
}

/// Lee los pines desde un archivo.
pub fn load_from_file(path: &Path) -> Result<Pins, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("no se pudo leer {}: {e}", path.display()))?;
    parse_text(&text)
}

/// Lee los pines desde la raíz del repositorio.
pub fn load_from_root(root: &Path) -> Result<Pins, String> {
    load_from_file(&root.join(PathBuf::from(PINS_REL)))
}
