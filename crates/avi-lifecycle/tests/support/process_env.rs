//! Entorno del proceso: el candado que serializa a las pruebas que lo mutan y la
//! declaración de las variables de reubicación. Lo declaran con
//! `#[path = "support/process_env.rs"] mod process_env;`.

use crate::support::Sandbox;
use std::path::Path;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, MutexGuard};

/// Serializa las pruebas que tocan el entorno del proceso.
///
/// El entorno es global, así que dos pruebas que lo muten a la vez se contaminan. Es el
/// mismo argumento que el candado de `avi-store` y el `test_support` del crate, y
/// aplica también a las escrituras que hace el propio motor: `ModelStore::new()` fija
/// `HF_XET_CACHE` cuando la raíz de modelos es exclusiva, que es una escritura al
/// entorno aunque la prueba no la ordene.
pub static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Toma [`ENV_LOCK`], sin envenenarse si otra prueba lo dejó tomado al caer en pánico.
pub fn exclusively() -> MutexGuard<'static, ()> {
    ENV_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Sandbox {
    /// Declara —o borra— en el entorno del proceso las variables de reubicación que este sandbox
    /// representa. Sin esto, lo que el motor resuelve por entorno saldría de la máquina
    /// que ejecuta la puerta.
    ///
    /// Requiere [`ENV_LOCK`]. En modo compartido **borra** `AVI_CACHE_DIR`, porque en
    /// `avi-store` tiene precedencia sobre `HF_HUB_CACHE` y su presencia volvería
    /// exclusiva una raíz que el test declara compartida. Al soltar el sandbox se
    /// restauran las variables.
    pub fn seed_env(&self) {
        let value = |p: &Path| p.display().to_string();
        match &self.shared_hub {
            Some(hub) => {
                std::env::remove_var("AVI_CACHE_DIR");
                std::env::set_var("HF_HUB_CACHE", value(hub));
                std::env::remove_var("HF_HOME");
            }
            None => {
                std::env::set_var("AVI_CACHE_DIR", value(&self.models_dir));
                std::env::remove_var("HF_HUB_CACHE");
                std::env::remove_var("HF_HOME");
            }
        }
        std::env::set_var("AVI_INSTALL_DIR", value(&self.program_dir));
        std::env::set_var("AVI_BIN_DIR", value(&self.bin_dir));
        std::env::set_var("AVI_DATA_DIR", value(&self.data_dir));
        self.env_seeded.store(true, Ordering::Relaxed);
    }
}
