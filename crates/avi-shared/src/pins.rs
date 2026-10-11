//! Pines versionados del repositorio (`packaging/pins.json`).
//!
//! La CI y Rust leen este archivo tal cual; nadie replica sus valores en el
//! código. `ort` es la versión de ONNX Runtime que empaqueta `xtask package` y
//! que el motor espera, y `ort_sha256` el SHA-256 de su asset oficial por
//! target; `rust` fija la toolchain (`rust-toolchain.toml`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Manifiesto de pines, relativo a la raíz del repositorio.
pub const PINS_REL: &str = "packaging/pins.json";

/// Pines versionados: un campo por clave del fichero. Cada clave de versión
/// tiene su parámetro espejo en la CI salvo `sccache`, que no tiene parámetro y
/// solo lo leen los pasos de instalación, y `ort_sha256`, que no es una versión
/// sino la tabla de hashes que verifica la descarga de ONNX Runtime.
#[derive(Debug, Clone, PartialEq, Eq, serde::Deserialize)]
pub struct Pins {
    pub rust: String,
    pub ort: String,
    /// SHA-256 (hexadecimal en minúsculas) del archivo oficial de ONNX Runtime
    /// de la versión `ort`, indexado por triple de Rust. Una descarga cuyo hash
    /// no coincide, o un triple sin entrada, es un error.
    pub ort_sha256: BTreeMap<String, String>,
    pub sccache: String,
    pub msys2_base: String,
    pub msys2_gcc: String,
    pub msys2_openblas: String,
    pub msys2_make: String,
    pub ninja: String,
    pub bats: String,
    pub pwsh: String,
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
