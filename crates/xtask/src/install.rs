//! `xtask install`: instala el build local por el camino del release (§10.5).
//!
//! Ejecuta `package --no-compress` en un staging temporal y después
//! `<staging>/ai-voice-interconnector self install --channel dev` (`--channel`
//! es la opción oculta del producto): el build local queda instalado por el
//! mismo camino que un release, de modo que cada instalación de desarrollo
//! ejercita el instalador real y queda marcada `dev` (criterio 26). Con
//! `--no-setup` la delegación tampoco provisiona modelos. El staging se borra
//! siempre, con éxito o con error, y el código de salida se propaga.
//! Sin red ni instalación real en los tests: los argumentos delegados, la
//! resolución del binario y el borrado del staging son funciones puras sobre
//! fixtures.

use anyhow::{bail, Result};
use std::path::{Path, PathBuf};

/// Nombre del binario del producto según la plataforma.
fn product_bin_name() -> &'static str {
    if cfg!(windows) {
        "ai-voice-interconnector.exe"
    } else {
        "ai-voice-interconnector"
    }
}

/// Argumentos de la delegación: `self install --channel dev`, más
/// `--no-setup` cuando se pide. Puros, fijados por test.
pub(crate) fn delegate_args(no_setup: bool) -> Vec<String> {
    let mut args = vec![
        "self".to_string(),
        "install".to_string(),
        "--channel".to_string(),
        "dev".to_string(),
    ];
    if no_setup {
        args.push("--no-setup".to_string());
    }
    args
}

/// Localiza el binario del producto dentro del árbol sin comprimir que dejó
/// `package --no-compress` en `dir` (el ejecutable del manifiesto, en la
/// raíz del árbol). `None` si no está: el staging no sirve.
pub(crate) fn find_staged_binary(dir: &Path) -> Option<PathBuf> {
    let candidate = dir.join(product_bin_name());
    if candidate.is_file() {
        return Some(candidate);
    }
    // Búsqueda acotada a un nivel (el árbol del bundle es plano con
    // `vendor/` como único subdirectorio con binarios ajenos al producto).
    std::fs::read_dir(dir).ok()?.flatten().find_map(|entry| {
        let path = entry.path();
        if path.is_dir() {
            let nested = path.join(product_bin_name());
            nested.is_file().then_some(nested)
        } else {
            None
        }
    })
}

/// Borra el staging sin fallar (lo mejor posible): se invoca en todos los
/// caminos de salida.
fn remove_staging(dir: &Path) {
    std::fs::remove_dir_all(dir).ok();
}

/// Ejecuta `package --no-compress` por proceso contra `out`: el empaquetado
/// real, sin duplicar su lógica aquí.
fn run_package_no_compress(xtask: &Path, out: &Path) -> Result<()> {
    let status = std::process::Command::new(xtask)
        .args(["package", "--no-compress", "--out"])
        .arg(out)
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo lanzar `package --no-compress`: {e}"))?;
    if !status.success() {
        bail!("`package --no-compress` falló (exit {:?})", status.code());
    }
    Ok(())
}

/// Punto de entrada de `xtask install`.
pub fn run(no_setup: bool) -> Result<()> {
    let root = std::env::current_dir()?;
    if !root.join("Cargo.toml").is_file() || !root.join("crates").join("xtask").is_dir() {
        bail!("ejecuta `cargo xtask install` desde la raíz del repositorio");
    }
    let xtask = std::env::current_exe()?;
    let staging = std::env::temp_dir().join(format!("avi-xtask-install-{}", std::process::id()));
    if staging.is_dir() {
        remove_staging(&staging);
    }
    std::fs::create_dir_all(&staging)?;

    // El staging cae en todos los caminos: éxito, fallo de `package` o
    // fallo de la delegación. Guarda que lo borra al salir del ámbito.
    struct Guard<'a> {
        dir: &'a Path,
    }
    impl Drop for Guard<'_> {
        fn drop(&mut self) {
            remove_staging(self.dir);
        }
    }
    let _guard = Guard { dir: &staging };

    run_package_no_compress(&xtask, &staging)?;
    let binary = find_staged_binary(&staging).ok_or_else(|| {
        anyhow::anyhow!(
            "el staging no trae el binario del producto: {}",
            staging.display()
        )
    })?;
    println!("Instalando {} …", binary.display());
    let args = delegate_args(no_setup);
    let status = std::process::Command::new(&binary)
        .args(&args)
        .status()
        .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {}: {e}", binary.display()))?;
    if !status.success() {
        bail!(
            "`self install --channel dev` falló (exit {:?}): el staging ya se borró",
            status.code()
        );
    }
    println!("Instalación dev completada (canal dev).");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La delegación fija el canal `dev` (opción oculta del producto) y
    /// propaga `--no-setup` solo cuando se pide.
    #[test]
    fn delegation_pins_dev_channel() {
        assert_eq!(
            delegate_args(false),
            ["self", "install", "--channel", "dev"]
        );
        assert_eq!(
            delegate_args(true),
            ["self", "install", "--channel", "dev", "--no-setup"]
        );
    }

    /// El binario se resuelve en la raíz del árbol y, como repliegue, un
    /// nivel por debajo; si no está, no hay instalación.
    #[test]
    fn staged_binary_resolution() {
        let base = std::env::temp_dir().join(format!("xtask_install_{}", std::process::id()));
        let flat = base.join("flat");
        let nested = base.join("nested").join("inner");
        let empty = base.join("empty");
        std::fs::create_dir_all(&flat).unwrap();
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::create_dir_all(&empty).unwrap();
        std::fs::write(flat.join(product_bin_name()), b"bin").unwrap();
        std::fs::write(nested.join(product_bin_name()), b"bin").unwrap();

        assert_eq!(
            find_staged_binary(&flat).unwrap(),
            flat.join(product_bin_name())
        );
        assert_eq!(
            find_staged_binary(&base.join("nested")).unwrap(),
            nested.join(product_bin_name())
        );
        assert!(find_staged_binary(&empty).is_none());

        std::fs::remove_dir_all(&base).ok();
    }

    /// El staging se borra ante el fallo (aquí simulado: borrar dos veces
    /// no falla y la segunda no encuentra nada).
    #[test]
    fn staging_removal_is_idempotent() {
        let dir = std::env::temp_dir().join(format!("xtask_install_rm_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(dir.join("sub").join("f"), b"x").unwrap();
        remove_staging(&dir);
        assert!(!dir.exists());
        remove_staging(&dir);
    }
}
