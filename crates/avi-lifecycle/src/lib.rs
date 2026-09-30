//! Motor del ciclo de vida del usuario: `self install` con reparación,
//! `self uninstall`, `cleanup` y la sección de ciclo de vida de `doctor`.
//!
//! Aquí se trasladó la lógica que vivía partida entre el binario —los
//! cuerpos de `uninstall`, `cleanup`, `setup`, el protocolo de parada del daemon
//! y los ayudantes de pidfile— y los scripts de la raíz, de modo que cada
//! regla de rutas tenga una única implementación por target.
//!
//! Es una biblioteca sin punto de entrada propio: el parseo de la CLI y el
//! cableado se quedan en el binario, que es quien conoce `clap` y el sobre
//! `--json`.
//!
//! Las rutas y los recursos del producto no se definen aquí: `avi-store` es la
//! fuente única y este crate las reexporta para que el motor tenga una
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
pub mod update;
pub mod update_fetch;
pub mod update_resolve;

pub use avi_store::{
    bin_dir, canonical_path_entry_matches, canonical_path_key, data_dir, install_dir,
    models_cache_dir, models_root_is_shared, shared_hf_root, APP_NAME, LIFECYCLE_LOCK_NAME,
    MODELS_XET_SUBDIR, PARKED_DIR_PREFIX, STAGING_DIR_PREFIX, TEMP_PREFIXES,
};

/// Superficie pública que el binario necesita para cablear el grupo `self` y para
/// delegar `cleanup` y `doctor`. Los módulos internos siguen siendo públicos
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
/// `reason` de contrato de máquina.
///
/// El motor no depende de `avi-core` porque aquí no se parsea la CLI, así que el par
/// `reason` + código viaja como dato y lo traduce la variante de `ExitCode` que declara
/// T16 al cablear. Los enteros salen de la misma tabla cerrada:
/// `confirmation_required` y `usage_error` conservan el 2 de `ExitCode::InvalidInput`, y
/// los del Ciclo 2 los fija su propio plan (`unsupported_platform = 18`,
/// `binary_incompatible = 19`, `network_error = 20`, `checksum_mismatch = 21`) y
/// `program_dir_kept = 22` es el de `self uninstall`. Los
/// siete nuevos van con el entero que fija la
/// consideración 2 del plan: `SetupFailed = 11`, `ExternallyManaged = 12`,
/// `RolledBack = 13`, `PathConflict = 14`, `BundleInvalid = 15`, `DaemonStopFailed = 16`
/// y `LifecycleLocked = 17`. Si ahí cambiara alguno, cambia aquí y en T16 a la vez.
/// Los `reason` del ciclo 3 recibirán su propia variante en su propio ciclo, siguiendo
/// el mismo patrón y sin tocar las de aquí.
///
/// El Ciclo 2 declara aquí sus constructores con los enteros que fija su plan
/// (`unsupported_platform = 18`, `binary_incompatible = 19`, `network_error = 20`,
/// `checksum_mismatch = 21`); las variantes de `ExitCode` y su cableado en
/// `exit_code_for` llegan con U4, que es la bisagra visible del ciclo.
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

    /// Target no soportado. El entero lo fija el plan del Ciclo 2; la
    /// variante de `ExitCode` llega con U4.
    pub fn unsupported_platform(message: impl Into<String>) -> Self {
        Self::new("unsupported_platform", 18, message.into())
    }

    /// Falta un archivo obligatorio del bundle (paso 2). Nada modificado.
    pub fn bundle_invalid(message: impl Into<String>) -> Self {
        Self::new("bundle_invalid", 15, message.into())
    }

    /// Hay otra operación de ciclo de vida en curso.
    pub fn lifecycle_locked(message: impl Into<String>) -> Self {
        Self::new("lifecycle_locked", 17, message.into())
    }

    /// La copia la gestiona otra herramienta, con el comando correcto en el mensaje
    /// (paso 1 de la desinstalación).
    pub fn externally_managed(message: impl Into<String>) -> Self {
        Self::new("externally_managed", 12, message.into())
    }

    /// No se pudo detener el daemon y nada se ha modificado (paso 5).
    pub fn daemon_stop_failed(message: impl Into<String>) -> Self {
        Self::new("daemon_stop_failed", 16, message.into())
    }

    /// Operación destructiva sin terminal y sin `--yes`. Error de uso.
    pub fn confirmation_required(message: impl Into<String>) -> Self {
        Self::new("confirmation_required", 2, message.into())
    }

    /// Invocación sin la categoría obligatoria (`cleanup` sin categoría).
    /// Error de uso.
    pub fn usage_error(message: impl Into<String>) -> Self {
        Self::new("usage_error", 2, message.into())
    }

    /// Fallo durante el reemplazo con la versión anterior restaurada.
    pub fn rolled_back(message: impl Into<String>) -> Self {
        Self::new("rolled_back", 13, message.into())
    }

    /// En la ruta del enlace hay un archivo ajeno. Error, salvo
    /// `--force`.
    pub fn path_conflict(message: impl Into<String>) -> Self {
        Self::new("path_conflict", 14, message.into())
    }

    /// El programa quedó instalado pero la provisión de modelos falló. Es un
    /// **éxito parcial** con código propio, reintentable con `setup`.
    pub fn setup_failed(message: impl Into<String>) -> Self {
        Self::new("setup_failed", 11, message.into())
    }

    /// El resto de la desinstalación se completó y el directorio de programa sigue en
    /// disco: no se pudo borrar ni programar su borrado (paso 8).
    pub fn program_dir_kept(message: impl Into<String>) -> Self {
        Self::new("program_dir_kept", 22, message.into())
    }

    /// El binario descargado no arranca o no informa la versión objetivo (paso 8
    /// de la actualización). El entero lo fija el plan del Ciclo 2; la variante de `ExitCode`
    /// llega con U4.
    pub fn binary_incompatible(message: impl Into<String>) -> Self {
        Self::new("binary_incompatible", 19, message.into())
    }

    /// Fallo de red acotado por los reintentos que fija la tabla de reasons
    // (pasos 3 a 5 y 7 de la actualización). El
    /// entero lo fija el plan del Ciclo 2; la variante de `ExitCode` llega con U4.
    pub fn network_error(message: impl Into<String>) -> Self {
        Self::new("network_error", 20, message.into())
    }

    /// El archivo descargado no coincide con `SHA256SUMS.txt`: hash distinto
    /// o entrada ausente. Nada más queda modificado y el staging se borra. El
    /// entero lo fija el plan del Ciclo 2; la variante de `ExitCode` llega con U4.
    pub fn checksum_mismatch(message: impl Into<String>) -> Self {
        Self::new("checksum_mismatch", 21, message.into())
    }
}

impl std::fmt::Display for LifecycleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.reason, self.message)
    }
}

impl std::error::Error for LifecycleError {}

/// Tamaño legible en escala binaria (B, KiB, MiB, GiB, TiB) con un decimal.
/// Es el único formateador de tamaños del producto: cada unidad vale 1024 de la
/// anterior y la etiqueta lo dice.
pub(crate) fn human_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
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
    fn formats_binary_units_with_one_decimal() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1023), "1023 B");
        assert_eq!(human_bytes(1024), "1.0 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024 / 2), "1.5 GiB");
        assert_eq!(human_bytes(1024u64.pow(4)), "1.0 TiB");
    }
}

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
    /// temporales con los prefijos propios de la tabla de rutas (`avi-`, `avi_`), y el sandbox
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
    pub fn write_file(path: &std::path::Path, content: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("se puede crear el directorio padre");
        }
        std::fs::write(path, content).expect("se puede escribir el fichero");
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

    /// Respuesta enlatada de un servidor HTTP local de prueba: estado, cuerpo y
    /// cabeceras adicionales (p. ej. `Location` para una redirección).
    pub struct FakeResponse {
        pub status: u16,
        pub body: Vec<u8>,
        pub extra_headers: Vec<(String, String)>,
    }

    impl FakeResponse {
        /// Respuesta con solo estado y cuerpo, sin cabeceras adicionales.
        pub fn new(status: u16, body: impl Into<Vec<u8>>) -> Self {
            Self {
                status,
                body: body.into(),
                extra_headers: Vec::new(),
            }
        }

        /// Redirección 302 hacia `location`, con cuerpo vacío.
        pub fn redirect(location: &str) -> Self {
            Self {
                status: 302,
                body: Vec::new(),
                extra_headers: vec![("Location".to_string(), location.to_string())],
            }
        }
    }

    /// Sirve `responses` en orden a conexiones locales, una respuesta por
    /// conexión, y devuelve la URL base más las rutas pedidas en orden.
    ///
    /// Es el mock de red a nivel de cliente que el plan de `self update` exige:
    /// ninguna prueba de resolución o descarga sale a internet. Solo atiende
    /// `GET` sin cuerpo: lee hasta el fin de las cabeceras, anota la ruta de la
    /// primera línea y responde con `Content-Length` y cierre de conexión, que es
    /// lo que un cliente HTTP necesita para no quedarse esperando.
    pub async fn serve_responses(
        responses: Vec<FakeResponse>,
    ) -> (
        String,
        std::sync::Arc<std::sync::Mutex<Vec<String>>>,
        tokio::task::JoinHandle<()>,
    ) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("se puede enlazar el servidor local de la prueba");
        let addr = listener
            .local_addr()
            .expect("el servidor local tiene dirección");
        let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
        let handle = {
            let seen = std::sync::Arc::clone(&seen);
            tokio::spawn(async move {
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                for canned in responses {
                    let Ok((mut conn, _)) = listener.accept().await else {
                        return;
                    };
                    let mut request = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while let Ok(read) = conn.read(&mut chunk).await {
                        if read == 0 {
                            break;
                        }
                        request.extend_from_slice(&chunk[..read]);
                        if request.len() > 8192 || request.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let path = String::from_utf8_lossy(&request)
                        .lines()
                        .next()
                        .unwrap_or_default()
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or_default()
                        .to_string();
                    seen.lock()
                        .unwrap_or_else(|poisoned| poisoned.into_inner())
                        .push(path);
                    let reason = match canned.status {
                        200 => "OK",
                        302 => "Found",
                        404 => "Not Found",
                        500 => "Internal Server Error",
                        _ => "OK",
                    };
                    let mut head = format!(
                        "HTTP/1.1 {} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n",
                        canned.status,
                        canned.body.len()
                    );
                    for (name, value) in &canned.extra_headers {
                        head.push_str(&format!("{name}: {value}\r\n"));
                    }
                    head.push_str("\r\n");
                    {
                        let _ = conn.write_all(head.as_bytes()).await;
                        let _ = conn.write_all(&canned.body).await;
                    }
                }
            })
        };
        (format!("http://{addr}"), seen, handle)
    }
}
