//! Lista canónica de archivos del bundle y su validación (§9.3, paso 2).
//!
//! La lista vive en `packaging/bundle-manifest.json`, un archivo de datos del
//! repositorio, y se embebe aquí con `include_str!`: empaquetado e instalación
//! leen la misma fuente, que es lo que impide que diverjan. Lo consume también
//! `cargo xtask package` en el Ciclo 3, leyendo el archivo del disco, y por eso
//! no está en `avi-store` (ese crate depende de `hf-hub` y arrastraría el
//! árbol TLS completo al `xtask`).
//!
//! Este módulo **no descarga, no extrae y no verifica checksums**: son
//! responsabilidades de `self update` (Ciclo 2) y del bootstrap (Ciclo 3).
//! Solo mira qué archivos hay alrededor del ejecutable, y no toca el disco.

use crate::target;
use crate::LifecycleError;
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;

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
    pub fn executable_path(&self) -> std::path::PathBuf {
        relative_path(&self.executable)
    }
}

/// Une un fragmento del manifiesto con la raíz del bundle respetando el
/// separador de la plataforma. El manifiesto usa siempre `/`, y en Windows
/// `Path::join` con una cadena con `/` funciona, pero normalizar evita depender
/// de eso.
fn relative_path(relative: &str) -> std::path::PathBuf {
    let mut path = std::path::PathBuf::new();
    for part in relative.split('/') {
        path.push(part);
    }
    path
}

#[derive(Debug, Deserialize)]
struct RawManifest {
    targets: BTreeMap<String, RawTarget>,
}

#[derive(Debug, Deserialize)]
struct RawTarget {
    executable: String,
    required: Vec<String>,
}

fn raw() -> Result<RawManifest, LifecycleError> {
    serde_json::from_str(crate::BUNDLE_MANIFEST).map_err(|e| {
        LifecycleError::bundle_invalid(format!(
            "el manifiesto de bundle embebido no es JSON válido: {e}"
        ))
    })
}

/// Todas las secciones del manifiesto, indexadas por triple.
pub fn targets() -> Result<BTreeMap<String, BundleTarget>, LifecycleError> {
    let parsed = raw()?;
    Ok(parsed
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

/// Sección de `triple`, exigiendo antes que el triple esté soportado (§3). Un
/// triple soportado sin sección es un defecto de empaquetado, no del bundle del
/// usuario, y se reporta como `bundle_invalid` nombrando el triple.
pub fn target_section(triple: &str) -> Result<BundleTarget, LifecycleError> {
    target::ensure_supported(triple)?;
    let parsed = raw()?;
    let section = parsed.targets.get(triple).ok_or_else(|| {
        LifecycleError::bundle_invalid(format!(
            "el manifiesto de bundle no tiene sección para el target {triple}"
        ))
    })?;
    Ok(BundleTarget {
        triple: triple.to_string(),
        executable: section.executable.clone(),
        required: section.required.clone(),
    })
}

/// Rutas obligatorias que faltan en `dir`, en el orden del manifiesto.
///
/// No falla por archivos ausentes: es el inspector que usa `validate_bundle`
/// para componer su mensaje, y el que necesita el plan de un `cleanup`
/// (§9.6) sin producir un error.
pub fn missing_files(triple: &str, dir: &Path) -> Result<Vec<String>, LifecycleError> {
    Ok(missing_in(&target_section(triple)?, dir))
}

/// Entradas de la sección que no están en `dir`.
fn missing_in(section: &BundleTarget, dir: &Path) -> Vec<String> {
    section
        .required
        .iter()
        .filter(|relative| !dir.join(relative_path(relative)).exists())
        .cloned()
        .collect()
}

/// Valida el bundle que hay alrededor del ejecutable contra la lista del
/// target. Devuelve la lista de rutas obligatorias, todas presentes, o falla
/// con `bundle_invalid` nombrando cada ruta ausente.
///
/// **No modifica nada**: el llamador valida antes de tocar el disco (§9.3,
/// paso 2), de modo que un bundle incompleto deja la instalación como estaba.
pub fn validate_bundle(triple: &str, dir: &Path) -> Result<Vec<String>, LifecycleError> {
    let section = target_section(triple)?;
    let missing = missing_in(&section, dir);
    if !missing.is_empty() {
        return Err(LifecycleError::bundle_invalid(format!(
            "el bundle en {} está incompleto: faltan {} archivo(s) obligatorio(s): {}",
            dir.display(),
            missing.len(),
            missing.join(", ")
        )));
    }
    Ok(section.required)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{scratch, write_file};

    /// Monta un bundle completo del target pedido y devuelve su raíz.
    fn complete_bundle(tag: &str, triple: &str) -> std::path::PathBuf {
        let dir = scratch(tag);
        for relative in &target_section(triple).unwrap().required {
            write_file(&dir.join(relative_path(relative)), "contenido");
        }
        dir
    }

    /// El manifiesto está embebido, tiene una sección por cada target de §3 y
    /// ninguna de más, y cada sección es coherente: nada vacío, el ejecutable
    /// entre los obligatorios, sin rutas absolutas ni escapes hacia arriba, y
    /// con los archivos que de verdad exige el producto.
    #[test]
    fn manifest_covers_every_target() {
        let targets = targets().unwrap();
        assert_eq!(
            targets.len(),
            target::SUPPORTED_TARGETS.len(),
            "una sección por cada target de §3: {:?}",
            targets.keys().collect::<Vec<_>>()
        );
        for triple in target::SUPPORTED_TARGETS {
            let section = target_section(triple).unwrap();
            assert_eq!(section.triple, triple);
            assert!(!section.required.is_empty(), "{triple}: sección vacía");
            assert!(
                section.required.contains(&section.executable),
                "{triple}: el ejecutable {} debe estar entre los obligatorios",
                section.executable
            );
            let mut unique: Vec<&String> = section.required.iter().collect();
            unique.sort();
            unique.dedup();
            assert_eq!(
                unique.len(),
                section.required.len(),
                "{triple}: sin duplicados"
            );
            for relative in &section.required {
                assert!(
                    !relative.starts_with('/') && !relative.contains('\\'),
                    "{triple}: {relative} debe ser relativa y usar `/`"
                );
                assert!(
                    !relative.split('/').any(|part| part == ".." || part == "."),
                    "{triple}: {relative} no puede escapar de la raíz del bundle"
                );
                if relative.contains('/') {
                    assert!(
                        relative.starts_with("vendor/"),
                        "{triple}: lo único anidado es el motor ({relative})"
                    );
                }
            }
            // Los cuatro documentos, el motor y la librería de ONNX Runtime con
            // el nombre que el crate `ort` busca en `load-dynamic`.
            for document in [
                "LICENSE",
                "THIRD-PARTY-LICENSES.md",
                "SOURCE-OFFER.md",
                "README.md",
            ] {
                assert!(
                    section.required.iter().any(|r| r == document),
                    "{triple}: falta {document} ({:?})",
                    section.required
                );
            }
            // El motor lleva la extensión del target: `qwen_tts.exe` en Windows.
            let engine = format!(
                "vendor/qwen3-tts/qwen_tts{}",
                if section.executable.ends_with(".exe") {
                    ".exe"
                } else {
                    ""
                }
            );
            assert!(
                section.required.contains(&engine),
                "{triple}: falta el motor {engine} ({:?})",
                section.required
            );
            // La librería de ONNX Runtime con el nombre que el crate `ort` busca
            // en `load-dynamic`, derivado del triple y no de la máquina.
            let ort = if triple.contains("-windows-") {
                "onnxruntime.dll"
            } else if triple.contains("-apple-") {
                "libonnxruntime.dylib"
            } else {
                "libonnxruntime.so"
            };
            assert!(
                section.required.iter().any(|r| r == ort),
                "{triple}: falta la librería de ONNX Runtime {ort}"
            );
            if triple.contains("-windows-") {
                for dll in ["vcruntime140.dll", "vcruntime140_1.dll", "msvcp140.dll"] {
                    assert!(
                        section.required.iter().any(|r| r == dll),
                        "{triple}: falta la DLL del runtime de VC++ {dll}"
                    );
                }
            } else {
                assert!(
                    !section
                        .required
                        .iter()
                        .any(|r| r.ends_with("vcruntime140.dll") || r.ends_with("msvcp140.dll")),
                    "{triple}: las DLL de VC++ son solo de Windows"
                );
            }
        }
    }

    /// La validación acepta un bundle completo y rechaza uno al que le falta
    /// cualquier entrada, nombrando la ruta ausente con el separador del
    /// manifiesto. También rechaza un triple sin sección, que es una divergencia
    /// entre empaquetado e instalación.
    #[test]
    fn manifest_validation_rejects_incomplete_bundle() {
        let triple = target::host_triple();
        let dir = complete_bundle("manifest-incompleto", triple);
        let section = target_section(triple).unwrap();
        assert_eq!(
            validate_bundle(triple, &dir).unwrap(),
            section.required,
            "un bundle completo se valida"
        );
        assert!(missing_files(triple, &dir).unwrap().is_empty());

        // Cada entrada obligatoria es imprescindible: se quita una de cada
        // forma —plana y anidada— y la validación falla nombrándola.
        let engine = format!(
            "vendor/qwen3-tts/qwen_tts{}",
            if section.executable.ends_with(".exe") {
                ".exe"
            } else {
                ""
            }
        );
        for missing in [section.executable.clone(), engine] {
            std::fs::remove_file(dir.join(relative_path(&missing))).unwrap();
            let err = validate_bundle(triple, &dir).unwrap_err();
            assert_eq!(err.reason, "bundle_invalid");
            assert_eq!(err.exit_code, 15, "código de bundle_invalid");
            assert!(
                err.message.contains(&missing),
                "el mensaje nombra la ruta ausente con `/`: {}",
                err.message
            );
            assert_eq!(missing_files(triple, &dir).unwrap(), vec![missing.clone()]);
            // Restaurar para probar la siguiente entrada.
            write_file(&dir.join(relative_path(&missing)), "contenido");
            assert!(validate_bundle(triple, &dir).is_ok());
        }

        // Un directorio que no existe es el caso de `target/release` sin bundle
        // alrededor: faltan todas las entradas, y el ejecutable entre ellas.
        let empty = scratch("manifest-vacio");
        let err = validate_bundle(triple, &empty).unwrap_err();
        assert_eq!(err.reason, "bundle_invalid");
        for relative in &section.required {
            assert!(
                err.message.contains(relative.as_str()),
                "faltan todas: {} no está en el mensaje",
                relative
            );
        }
        assert_eq!(
            missing_files(triple, &empty).unwrap().len(),
            section.required.len()
        );

        // Un triple no soportado se rechaza antes de mirar el manifiesto.
        assert_eq!(
            validate_bundle("i686-unknown-linux-gnu", &dir)
                .unwrap_err()
                .reason,
            "unsupported_platform"
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&empty).ok();
    }
}
