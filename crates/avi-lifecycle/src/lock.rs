//! Bloqueo de ciclo de vida (§9.1).
//!
//! Sin bloqueo, una actualización puede arrancar el daemon de la versión saliente
//! en mitad del reemplazo: nada impide que un comando que lanza el daemon lo
//! haga mientras otra operación de ciclo de vida está escribiendo el directorio
//! de programa. El bloqueo es **exclusivo de sistema operativo** (`flock` en
//! Unix, `LockFileEx` en Windows) sobre el archivo de §7, así que el sistema lo
//! libera aunque el proceso muera a mitad de una operación, que es exactamente
//! el caso que el motor debe poder recuperar al empezar la siguiente.
//!
//! **No hay crate de bloqueo**: `std::fs::File::lock` y `try_lock` están
//! estables desde 1.89 y la toolchain del proyecto es 1.96.
//!
//! El fichero se abre con lectura **y** escritura, no solo para anexar: un
//! `O_APPEND` impede el bloqueo exclusivo en Windows.
//!
//! Uso: se toma al principio de cada operación y se suelta al final, también en
//! el camino de error, que es lo que hace `Drop`. Quien solo necesita *saber* si
//! hay una operación en curso —los comandos que lanzan el daemon— consulta
//! [`is_locked`] y no toma el bloqueo, porque tomarlo para consultarlo lo
//! convertiría en el operador.

use crate::LifecycleError;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Ruta del archivo de bloqueo de §7: hermano del directorio de programa, que
/// es donde la tabla de rutas lo coloca en los cuatro targets.
pub fn lock_path() -> PathBuf {
    lock_path_for(&crate::install_dir())
}

/// Ruta del archivo de bloqueo de §7 a partir de un directorio de programa: hermano
/// suyo, que es donde la tabla de rutas lo coloca en los cuatro targets.
///
/// Es un parámetro y no la constante porque las tres operaciones destructivas —
/// `self install`, `cleanup` y `self uninstall`— trabajan sobre el directorio de
/// programa **registrado**, que puede no ser el de la convención, y §13 exige que las
/// pruebas aislen las raíces a temporales. Tener la regla en un solo sitio es lo que
/// impide que una de ellas tome el bloqueo de un sitio y borre de otro.
pub fn lock_path_for(program_dir: &Path) -> PathBuf {
    match program_dir.parent() {
        Some(parent) => parent.join(crate::LIFECYCLE_LOCK_NAME),
        None => PathBuf::from(crate::LIFECYCLE_LOCK_NAME),
    }
}

/// Bloqueo tomado. Se libera solo, al soltar el valor.
#[derive(Debug)]
pub struct LifecycleLock {
    /// Se conserva abierto mientras viva el bloqueo: es el descriptor que el
    /// sistema tiene tomado, no un dato que se consulten.
    _file: File,
    path: PathBuf,
}

impl LifecycleLock {
    /// Archivo de bloqueo tomado, para consultas posteriores.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

/// Toma el bloqueo de ciclo de vida sobre el archivo de §7.
///
/// Falla con `lifecycle_locked` si otra operación lo tiene tomado, y entonces no
/// se ha modificado nada.
pub fn acquire() -> Result<LifecycleLock, LifecycleError> {
    acquire_at(&lock_path())
}

/// Toma el bloqueo sobre `path`, que es un parámetro para que la prueba no
/// dependa del directorio de programa de quien la ejecuta.
pub fn acquire_at(path: &Path) -> Result<LifecycleLock, LifecycleError> {
    let file = open_lock_file(path)?;
    match file.try_lock() {
        Ok(()) => Ok(LifecycleLock {
            _file: file,
            path: path.to_path_buf(),
        }),
        // `TryLockError::WouldBlock` es la contención con otra operación; el
        // otro caso es un fallo del sistema, que también impide operar.
        Err(std::fs::TryLockError::WouldBlock) => Err(LifecycleError::lifecycle_locked(format!(
            "ya hay otra operación de ciclo de vida en curso ({})",
            path.display()
        ))),
        Err(std::fs::TryLockError::Error(e)) => Err(LifecycleError::lifecycle_locked(format!(
            "no se puede comprobar el bloqueo de ciclo de vida ({}): {e}",
            path.display()
        ))),
    }
}

/// `true` si hay una operación de ciclo de vida en curso.
///
/// No toma el bloqueo: solo lo consulta, que es lo que necesitan los comandos que
/// lanzan el daemon para abortar con `lifecycle_locked` sin entrar en la cola. Si
/// el archivo no existe, no hay operación en curso y no se crea nada.
pub fn is_locked() -> bool {
    is_locked_at(&lock_path())
}

/// `true` si el bloqueo de `path` está tomado. Igual que [`is_locked`], es una
/// consulta: no crea el archivo ni lo toma.
pub fn is_locked_at(path: &Path) -> bool {
    let file = match OpenOptions::new().read(true).write(true).open(path) {
        Ok(file) => file,
        Err(_) => return false,
    };
    match file.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(_)) => false,
    }
}

/// Abre el archivo de bloqueo con lectura y escritura, creándolo si hace falta.
/// El directorio es el del programa, que es de propiedad exclusiva (§7).
fn open_lock_file(path: &Path) -> Result<File, LifecycleError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            LifecycleError::lifecycle_locked(format!("no se puede crear {}: {e}", parent.display()))
        })?;
    }
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|e| {
            LifecycleError::lifecycle_locked(format!(
                "no se puede abrir el bloqueo de ciclo de vida ({}): {e}",
                path.display()
            ))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::scratch;

    /// Dos operaciones concurrentes: la segunda falla con `lifecycle_locked` sin
    /// haber modificado nada, y al soltar la primera el bloqueo queda libre.
    ///
    /// La contención se puede comprobar dentro de un mismo proceso porque el
    /// bloqueo es por descriptor abierto, no por proceso: dos `File` sobre el
    /// mismo archivo se estorban igual que en dos procesos distintos.
    #[test]
    fn second_operation_fails_with_lifecycle_locked() {
        let dir = scratch("lock");
        let path = dir.join(crate::LIFECYCLE_LOCK_NAME);
        assert!(!is_locked_at(&path), "sin operación no hay bloqueo");

        let primera = acquire_at(&path).expect("la primera operación toma el bloqueo");
        assert_eq!(primera.path(), path);
        assert!(
            is_locked_at(&path),
            "con la primera en curso, el bloqueo está tomado"
        );

        let err = acquire_at(&path).unwrap_err();
        assert_eq!(err.reason, "lifecycle_locked");
        assert_eq!(err.exit_code, 17, "LifecycleLocked = 17");
        assert!(err.message.contains("en curso"), "{}", err.message);

        drop(primera);
        assert!(
            !is_locked_at(&path),
            "al terminar la operación el bloqueo se libera solo"
        );
        let segunda = acquire_at(&path).expect("el bloqueo vuelve a estar libre");
        drop(segunda);
        std::fs::remove_dir_all(&dir).ok();
    }
}
