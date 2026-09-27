//! Tipos del bundle y parseo del manifiesto, sin red.
//!
//! La lista canónica vive en `packaging/bundle-manifest.json`: empaquetado
//! (`xtask package`, que la lee del disco) e instalación (`avi-lifecycle`, que
//! la embebe) la parsean con este mismo código, de modo que no pueden divergir.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Sección de un target en el manifiesto: el ejecutable y las rutas
/// obligatorias del bundle, relativas a su raíz y con separador `/` en todos
/// los sistemas.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleTarget {
    pub triple: String,
    pub executable: String,
    pub required: Vec<String>,
}

impl BundleTarget {
    /// Ruta del ejecutable del bundle, con el separador de la plataforma.
    pub fn executable_path(&self) -> PathBuf {
        relative_path(&self.executable)
    }
}

/// Une un fragmento del manifiesto con la raíz del bundle respetando el
/// separador de la plataforma. El manifiesto usa siempre `/`, y en Windows
/// `Path::join` con una cadena con `/` funciona, pero normalizar evita depender
/// de eso.
pub fn relative_path(relative: &str) -> PathBuf {
    let mut path = PathBuf::new();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

#[derive(Debug, serde::Deserialize)]
struct RawManifest {
    targets: BTreeMap<String, RawTarget>,
}

#[derive(Debug, serde::Deserialize)]
struct RawTarget {
    executable: String,
    required: Vec<String>,
}

/// Parsea el texto del manifiesto a secciones por triple.
pub fn parse_manifest_text(text: &str) -> Result<BTreeMap<String, BundleTarget>, String> {
    let raw: RawManifest = serde_json::from_str(text)
        .map_err(|e| format!("el manifiesto de bundle no es JSON válido: {e}"))?;
    Ok(raw
        .targets
        .into_iter()
        .map(|(triple, section)| {
            let target = BundleTarget {
                triple: triple.clone(),
                executable: section.executable,
                required: section.required,
            };
            (triple, target)
        })
        .collect())
}
