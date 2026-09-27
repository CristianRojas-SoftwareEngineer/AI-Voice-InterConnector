//! Fuente única estructural del bundle, las rutas y los pines del producto.
//!
//! Micro-crate puro: tipos del manifiesto del bundle con su parseo, directorios
//! canónicos del producto con sus revisiones de modelos y lectura de
//! `packaging/pins.json`. Sin red ni TLS a propósito, de modo que `xtask`
//! puede depender de él sin arrastrar el árbol de `hf-hub` que impide usar
//! `avi-store` desde el tooling.

pub mod manifest;
pub mod paths;
pub mod pins;
