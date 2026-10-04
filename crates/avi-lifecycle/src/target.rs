//! Target del binario en ejecución y nombre convencional del archivo de release.
//!
//! La tabla de targets soportados vive aquí, en código, como una sola definición: la
//! consumen
//! la validación del bundle (`manifest`), el nombre del archivo que descarga
//! `self update` y el nombre que produce `cargo xtask package`.
//! Que las tres cosas coincidan es lo que impide que un bundle se
//! valide con una lista y se publique con otra.
//!
//! **La detección del target del sistema operativo no se replica aquí.**
//! La tabla de `uname -m`, `AMD64` y `sysctl hw.optional.arm64` describe el
//! bootstrap de release, que vive en `packaging/bootstrap/`, y hoy solo
//! se compila para los targets de la tabla: el binario ya sabe con qué triple
//! se construyó, así que preguntar por la máquina sería una segunda fuente que
//! puede discrepar. Lo que este módulo rechaza es un binario compilado para un
//! triple que no está en la tabla.
//!
//! La variante de código propia de `unsupported_platform` se declara
//! junto con `binary_incompatible` y `checksum_mismatch`: el `reason` es el de la
//! tabla de targets y el código es el 18 de la tabla cerrada.

use crate::{LifecycleError, APP_NAME};

/// Triples soportados. Es la tabla en código, y `manifest` la usa para exigir
/// que el manifiesto tenga una sección por cada uno.
pub const SUPPORTED_TARGETS: [&str; 4] = [
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "aarch64-unknown-linux-gnu",
    "aarch64-apple-darwin",
];

/// Triple con el que se compiló este binario, estampado por `build.rs` desde la
/// variable `TARGET` de Cargo.
pub fn host_triple() -> &'static str {
    env!("AVI_TARGET_TRIPLE")
}

/// `true` si el triple está en la tabla de targets soportados.
pub fn is_supported(triple: &str) -> bool {
    SUPPORTED_TARGETS.contains(&triple)
}

/// Exige que el triple esté soportado, y si no emite `unsupported_platform` con
/// un mensaje que remite a compilar desde el código fuente, como manda la regla.
pub fn ensure_supported(triple: &str) -> Result<(), LifecycleError> {
    if is_supported(triple) {
        return Ok(());
    }
    Err(unsupported(triple))
}

/// Raíz del target: su arquitectura tal como la nombra la columna "Archivo de
/// release" de la tabla de targets, que no coincide con el prefijo del triple en arm64
/// (`aarch64-*` → `arm64`). La usan el nombre del archivo de release y quien
/// tenga que comparar contra un bundle ya publicado.
pub fn release_arch(triple: &str) -> Result<&'static str, LifecycleError> {
    match triple {
        "x86_64-pc-windows-msvc" | "x86_64-unknown-linux-gnu" => Ok("x86_64"),
        "aarch64-unknown-linux-gnu" | "aarch64-apple-darwin" => Ok("arm64"),
        other => Err(unsupported(other)),
    }
}

/// Etiqueta de sistema operativo y extensión del archivo de release de un
/// triple: la segunda mitad de la convención `ai-voice-interconnector-<ver>-
/// <arch>-<os>.<ext>` de la tabla de targets.
fn release_os(triple: &str) -> Result<(&'static str, &'static str), LifecycleError> {
    match triple {
        "x86_64-pc-windows-msvc" => Ok(("windows", "zip")),
        "x86_64-unknown-linux-gnu" | "aarch64-unknown-linux-gnu" => Ok(("linux", "tar.gz")),
        "aarch64-apple-darwin" => Ok(("macos", "tar.gz")),
        other => Err(unsupported(other)),
    }
}

/// Mensaje de `unsupported_platform`: nombra el triple y remite a compilar
/// desde el código fuente. La ruta que lleva el mensaje es la real
/// —`docs/BUILD.md`—, y no una relativa al sitio donde vive el enunciado.
fn unsupported(triple: &str) -> LifecycleError {
    LifecycleError::unsupported_platform(format!(
        "la plataforma de este binario ({triple}) no está soportada: compila \
         {APP_NAME} desde el código fuente (docs/BUILD.md)"
    ))
}

/// Nombre convencional del archivo de release de `triple` para `version`, con
/// la convención de la tabla de targets:
/// `ai-voice-interconnector-<ver>-<arch>-<os>.<ext>`.
///
/// Es la convención de los cuatro jobs de build de `.circleci/config.yml`, que
/// es donde vive hoy. La consumen `self update` y
/// `cargo xtask package`; el bootstrap la busca con
/// el mismo criterio.
pub fn release_asset_name(triple: &str, version: &str) -> Result<String, LifecycleError> {
    let arch = release_arch(triple)?;
    let (os, ext) = release_os(triple)?;
    Ok(format!("{APP_NAME}-{version}-{arch}-{os}.{ext}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_core::exit_codes::ExitCode;

    /// La tabla de targets y el nombre del archivo de release se derivan de la misma
    /// definición, sin depender de la plataforma donde corra la prueba.
    #[test]
    fn release_asset_name_per_target() {
        let expected = [
            (
                "x86_64-pc-windows-msvc",
                "0.24.0",
                "ai-voice-interconnector-0.24.0-x86_64-windows.zip",
            ),
            (
                "x86_64-unknown-linux-gnu",
                "0.24.0",
                "ai-voice-interconnector-0.24.0-x86_64-linux.tar.gz",
            ),
            (
                "aarch64-unknown-linux-gnu",
                "0.24.0",
                "ai-voice-interconnector-0.24.0-arm64-linux.tar.gz",
            ),
            (
                "aarch64-apple-darwin",
                "0.24.0",
                "ai-voice-interconnector-0.24.0-arm64-macos.tar.gz",
            ),
        ];
        assert_eq!(
            expected.len(),
            SUPPORTED_TARGETS.len(),
            "la tabla de targets soportados tiene cuatro entradas"
        );
        for (triple, version, name) in expected {
            assert_eq!(
                release_asset_name(triple, version).unwrap(),
                name,
                "nombre de release de {triple}"
            );
            assert!(
                ensure_supported(triple).is_ok(),
                "{triple} está en la tabla de targets soportados"
            );
        }
        // La versión se interpola tal cual: el nombre no lleva la `v` del tag.
        assert_eq!(
            release_asset_name("aarch64-apple-darwin", "1.2.3").unwrap(),
            "ai-voice-interconnector-1.2.3-arm64-macos.tar.gz"
        );
    }

    /// Windows ARM64, macOS Intel, musl y userland de 32 bits: los cuatro
    /// casos que la tabla declara no soportados, incluidos los que solo se detectan
    /// al arrancar el binario y que por eso comparten `reason`.
    #[test]
    fn unsupported_target_is_rejected() {
        for triple in [
            "aarch64-pc-windows-msvc",   // Windows ARM64
            "x86_64-apple-darwin",       // macOS Intel
            "x86_64-unknown-linux-musl", // Linux con musl (Alpine)
            "i686-unknown-linux-gnu",    // userland de 32 bits
        ] {
            assert!(!is_supported(triple), "{triple} no puede estar soportado");
            let err = ensure_supported(triple).unwrap_err();
            assert_eq!(err.reason, "unsupported_platform", "reason de {triple}");
            assert_eq!(
                ExitCode::from_reason(err.reason).code(),
                18,
                "con variante propia, el código es el 18"
            );
            assert!(
                err.message.contains(triple) && err.message.contains("docs/BUILD.md"),
                "el mensaje nombra el triple y remite a compilar desde el código: {}",
                err.message
            );
            // El nombre de release tampoco se deriva de un target no soportado.
            assert_eq!(
                release_asset_name(triple, "0.24.0").unwrap_err().reason,
                "unsupported_platform",
                "nombre de release de {triple}"
            );
            assert_eq!(
                ExitCode::from_reason(release_asset_name(triple, "0.24.0").unwrap_err().reason)
                    .code(),
                18
            );
        }
    }

    /// El triple embebido es el real del binario en ejecución y está soportado:
    /// si el crate se compilara para un target fuera de la tabla, la propia
    /// construcción del motor lo diría.
    #[test]
    fn host_triple_is_supported() {
        let triple = host_triple();
        assert!(
            !triple.is_empty(),
            "build.rs debe estampar el triple de compilación"
        );
        assert!(
            is_supported(triple),
            "{triple} compila fuera de la tabla de targets soportados"
        );
        // La raíz del target y la etiqueta de sistema operativo son las que el
        // nombre de release usa, y ambas salen del mismo sitio.
        let name = release_asset_name(triple, "0.0.0").unwrap();
        assert!(name.starts_with(APP_NAME), "prefijo de {name}");
    }
}
