//! Sandbox con la raíz de modelos en modo compartido (la caché HF del usuario). Lo
//! declaran las pruebas que ejercitan ese modo con
//! `#[path = "support/shared.rs"] mod shared;`.

use crate::support::Sandbox;

impl Sandbox {
    /// Levanta un sandbox nuevo con la raíz de modelos compartida (`HF_HUB_CACHE`).
    pub fn new_shared(tag: &str) -> Self {
        let sandbox = Self::at(tag, Self::unique_root(tag), true);
        sandbox.create_directories();
        sandbox
    }
}
