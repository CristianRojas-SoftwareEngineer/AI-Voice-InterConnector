//! Fuente única estructural del bundle, las rutas y los pines del producto.
//!
//! Micro-crate puro: tipos del manifiesto del bundle con su parseo, directorios
//! canónicos del producto con sus revisiones de modelos, lectura de
//! `packaging/pins.json` y el formateador de tamaños. Sin red ni TLS a
//! propósito, de modo que `xtask` puede depender de él sin arrastrar el árbol
//! de `hf-hub` que impide usar `avi-store` desde el tooling. Única excepción:
//! el arranque de consola (`console`) usa la API del sistema solo en Windows.

pub mod console;
pub mod logs;
pub mod manifest;
pub mod paths;
pub mod pins;

pub use console::force_utf8_console;

/// Tamaño legible en escala decimal (B, KB, MB, GB, TB) con un decimal.
/// Es el único formateador de tamaños del workspace, compartido por el producto
/// y por `xtask`: cada unidad vale 1000 de la anterior y la etiqueta lo dice.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod human_bytes_tests {
    use super::human_bytes;

    #[test]
    fn formats_decimal_units_with_one_decimal() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1000), "1.0 KB");
        assert_eq!(human_bytes(1500), "1.5 KB");
        assert_eq!(human_bytes(5 * 1000 * 1000), "5.0 MB");
        assert_eq!(human_bytes(3 * 1000 * 1000 * 1000 / 2), "1.5 GB");
        assert_eq!(human_bytes(1000u64.pow(4)), "1.0 TB");
    }
}
