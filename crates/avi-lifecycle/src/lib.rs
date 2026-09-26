//! Motor del ciclo de vida del usuario: `self install` con reparación,
//! `self uninstall`, `cleanup` y la sección de ciclo de vida de `doctor` (§6.2).
//!
//! Aquí se traslada la lógica que hoy vive partida entre `src/main.rs` —los
//! cuerpos de `uninstall`, `cleanup`, `setup`, el protocolo de parada del daemon
//! y los ayudantes de pidfile— y los cinco scripts de la raíz, de modo que cada
//! regla de §7 tenga una única implementación por target.
//!
//! Es una biblioteca sin punto de entrada propio: el parseo de la CLI y el
//! cableado se quedan en el binario, que es quien conoce `clap` y el sobre
//! `--json`.
//!
//! Las rutas y los recursos del producto no se definen aquí: `avi-store` es la
//! fuente única (§7) y este crate las reexporta para que el motor tenga una
//! sola forma de nombrarlas.

pub mod channel;
pub mod cleanup;
pub mod confirm;
pub mod daemon_stop;
pub mod doctor;
pub mod faults;
pub mod install;
pub mod lock;
pub mod manifest;
pub mod path_unix;
#[cfg(windows)]
pub mod path_windows;
pub mod privileges;
pub mod quarantine;
pub mod receipt;
pub mod recovery;
pub mod setup;
pub mod target;
pub mod transaction;
pub mod uninstall;

pub use avi_store::{
    bin_dir, canonical_path_entry_matches, canonical_path_key, data_dir, install_dir,
    models_cache_dir, models_root_is_shared, shared_hf_root, APP_NAME, LIFECYCLE_LOCK_NAME,
    MODELS_XET_SUBDIR, PARKED_DIR_PREFIX, STAGING_DIR_PREFIX, TEMP_PREFIXES,
};

/// Superficie pública que el binario necesita para cablear el grupo `self` y para
/// delegar `cleanup` y `doctor` (§6.4). Los módulos internos siguen siendo públicos
/// porque las pruebas de integración los necesitan, pero el cableado entra por aquí y
/// no por dentro del motor.
pub mod prelude {
    pub use crate::cleanup::{self, Roots as CleanupRoots};
    pub use crate::daemon_stop::{self, ProcessControl};
    pub use crate::doctor::{self, Env as DoctorEnv};
    pub use crate::install::{self, Env as InstallEnv, Options as InstallOptions};
    pub use crate::receipt::{self, InstallReceipt};
    pub use crate::uninstall::{self, Env as UninstallEnv, Options as UninstallOptions};
    pub use crate::LifecycleError;
}

/// Lista canónica de archivos del bundle, embebida desde el archivo de datos del
/// repositorio. Vive fuera de `avi-store` a propósito: aquel crate depende de
/// `hf-hub`, y un manifiesto alojado ahí obligaría a `xtask` a compilar el árbol
/// TLS completo.
pub const BUNDLE_MANIFEST: &str = include_str!("../../../packaging/bundle-manifest.json");

/// Fallo de una operación de ciclo de vida que la especificación declara como
/// `reason` de contrato de máquina (§9.1).
///
/// El motor no depende de `avi-core` (§6.3: aquí no se parsea la CLI), así que el par
/// `reason` + código viaja como dato y lo traduce la variante de `ExitCode` que declara
/// T16 al cablear. Los enteros salen de la misma tabla cerrada:
/// `confirmation_required` y `usage_error` conservan el 2 de `ExitCode::InvalidInput`, y
/// `unsupported_platform` usa el 1 genérico porque su variante la declara el ciclo que
/// declare también `binary_incompatible`. Los siete nuevos van con el entero que fija la
/// consideración 2 del plan: `SetupFailed = 11`, `ExternallyManaged = 12`,
/// `RolledBack = 13`, `PathConflict = 14`, `BundleInvalid = 15`, `DaemonStopFailed = 16`
/// y `LifecycleLocked = 17`. Si ahí cambiara alguno, cambia aquí y en T16 a la vez.
/// Los `reason` de los ciclos 2 y 3 (`unsupported_platform` ya declarado aquí como
/// error genérico, y `binary_incompatible`, `network_error` y `checksum_mismatch`)
/// recibirán su propia variante en su propio ciclo, siguiendo el mismo patrón y sin
/// tocar las de aquí.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LifecycleError {
    pub reason: &'static str,
    pub exit_code: i32,
    pub message: String,
}

impl LifecycleError {
    /// Constructor común. Es `pub(crate)` y no público porque la mayoría de los
    /// `reason` de contrato viven como constructores con nombre justo debajo; lo usan
    /// directamente los módulos cuyo `reason` es específico de su plataforma o de su
    /// operación y por eso no merece una variante con nombre propio.
    pub(crate) fn new(reason: &'static str, exit_code: i32, message: String) -> Self {
        Self {
            reason,
            exit_code,
            message,
        }
    }

    /// Target no soportado (§3). Error genérico, sin variante propia en este
    /// ciclo.
    pub fn unsupported_platform(message: impl Into<String>) -> Self {
        Self::new("unsupported_platform", 1, message.into())
    }

    /// Falta un archivo obligatorio del bundle (§9.3, paso 2). Nada modificado.
    pub fn bundle_invalid(message: impl Into<String>) -> Self {
        Self::new("bundle_invalid", 15, message.into())
    }

    /// Hay otra operación de ciclo de vida en curso (§9.1).
    pub fn lifecycle_locked(message: impl Into<String>) -> Self {
        Self::new("lifecycle_locked", 17, message.into())
    }

    /// La copia la gestiona otra herramienta, con el comando correcto en el mensaje
    /// (§9.1, §9.5, paso 1).
    pub fn externally_managed(message: impl Into<String>) -> Self {
        Self::new("externally_managed", 12, message.into())
    }

    /// No se pudo detener el daemon y nada se ha modificado (§9.1, §9.5, paso 5).
    pub fn daemon_stop_failed(message: impl Into<String>) -> Self {
        Self::new("daemon_stop_failed", 16, message.into())
    }

    /// Operación destructiva sin terminal y sin `--yes` (§9.1). Error de uso.
    pub fn confirmation_required(message: impl Into<String>) -> Self {
        Self::new("confirmation_required", 2, message.into())
    }

    /// Invocación sin la categoría obligatoria (`cleanup` sin categoría, §9.1).
    /// Error de uso.
    pub fn usage_error(message: impl Into<String>) -> Self {
        Self::new("usage_error", 2, message.into())
    }

    /// Fallo durante el reemplazo con la versión anterior restaurada (§9.1).
    pub fn rolled_back(message: impl Into<String>) -> Self {
        Self::new("rolled_back", 13, message.into())
    }

    /// En la ruta del enlace hay un archivo ajeno (§9.1, §9.3.1). Error, salvo
    /// `--force`.
    pub fn path_conflict(message: impl Into<String>) -> Self {
        Self::new("path_conflict", 14, message.into())
    }

    /// El programa quedó instalado pero la provisión de modelos falló (§9.1). Es un
    /// **éxito parcial** con código propio, reintentable con `setup`.
    pub fn setup_failed(message: impl Into<String>) -> Self {
        Self::new("setup_failed", 11, message.into())
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.reason, self.message)
    }
}

impl std::error::Error for LifecycleError {}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// Serializa las pruebas que tocan el entorno del proceso: el entorno es
    /// global, así que dos pruebas que lo mutan a la vez se contaminarían. Las
    /// de `avi-store` usan su propio candado; este cubre las de este crate.
    pub static ENV_LOCK: Mutex<()> = Mutex::new(());

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Directorio de trabajo limpio y único para una prueba. El prefijo es
    /// deliberadamente neutro: los barridos de recuperación solo tocan
    /// temporales con los prefijos propios de §7 (`avi-`, `avi_`), y el sandbox
    /// de una prueba no debe ser confundido con un temporal del producto.
    pub fn scratch(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("lifecycle-test-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("se puede crear el sandbox de la prueba");
        path
    }

    /// Escribe un fichero con `contenido`, creando los directorios intermedios.
    pub fn write_file(path: &std::path::Path, contenido: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("se puede crear el directorio padre");
        }
        std::fs::write(path, contenido).expect("se puede escribir el fichero");
    }

    /// Lista recursivamente `(ruta relativa, tamaño)` de un directorio, para
    /// afirmar que una operación no modificó el disco.
    pub fn snapshot(root: &std::path::Path) -> Vec<(String, u64)> {
        let mut out = Vec::new();
        let mut stack = vec![root.to_path_buf()];
        while let Some(dir) = stack.pop() {
            let entries = match std::fs::read_dir(&dir) {
                Ok(entries) => entries,
                Err(_) => continue,
            };
            for entry in entries.flatten() {
                let path = entry.path();
                let meta = match entry.metadata() {
                    Ok(meta) => meta,
                    Err(_) => continue,
                };
                let rel = path
                    .strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/");
                if meta.is_dir() {
                    out.push((rel, 0));
                    stack.push(path);
                } else {
                    out.push((rel, meta.len()));
                }
            }
        }
        out.sort();
        out
    }
}
