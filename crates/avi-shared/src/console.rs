//! Arranque de consola en Windows: fija la página de códigos de entrada y
//! salida a UTF-8 para que los mensajes con tildes se vean bien en la
//! PowerShell tal como viene configurada. Best-effort y silencioso: sin
//! consola no hace nada y fuera de Windows es un no-op.

/// Fija la consola heredada a UTF-8. Llamar lo primero al arrancar, antes de
/// cualquier salida.
pub fn force_utf8_console() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::Globalization::CP_UTF8;
        use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
        unsafe {
            SetConsoleOutputCP(CP_UTF8);
            SetConsoleCP(CP_UTF8);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    /// El manifiesto declara la dependencia del sistema solo para Windows:
    /// en Unix el crate sigue siendo puro.
    #[test]
    fn system_dep_is_windows_only() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let text = std::fs::read_to_string(&manifest).unwrap();
        let mut parts = text.split("[target.");
        let head = parts.next().unwrap();
        assert!(
            !head.contains("windows-sys"),
            "windows-sys no debe estar en dependencias generales"
        );
        assert!(
            parts.any(|p| p.contains("cfg(windows)") && p.contains("windows-sys")),
            "windows-sys debe declararse solo para Windows"
        );
    }
}
