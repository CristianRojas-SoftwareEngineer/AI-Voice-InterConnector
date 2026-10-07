//! Bloqueo de ciclo de vida.
//!
//! Sin bloqueo, una actualización puede arrancar el daemon de la versión saliente
//! en mitad del reemplazo, y una desinstalación puede borrar modelos y binarios
//! mientras un comando los usa. El bloqueo es **de sistema operativo** (`flock` en
//! Unix, `LockFileEx` en Windows) sobre ficheros hermanos del directorio de
//! programa, así que el sistema lo libera aunque el proceso muera a mitad de una
//! operación, que es exactamente el caso que el motor debe poder recuperar al
//! empezar la siguiente.
//!
//! **Protocolo.** Hay dos ficheros: el bloqueo principal y la compuerta de traspaso.
//!
//! - Los comandos de **uso** (los que leen o escriben datos y modelos) toman la
//!   compuerta y el principal en modo **compartido**: coexisten entre sí y
//!   excluyen a toda operación.
//! - Las operaciones de **ciclo de vida** (`install`, `update`, `uninstall`,
//!   `cleanup`, `doctor --repair`) toman ambos en modo **exclusivo**: excluyen
//!   a los comandos de uso y entre sí.
//! - La compuerta existe por `self update`: antes de lanzar el `self install`
//!   del binario nuevo suelta solo el principal (que el hijo necesita) y retiene
//!   la compuerta hasta que el traspaso vuelve, de modo que ningún comando de
//!   uso se cuela en la ventana. El hijo, reconocido por la variable
//!   `AVI_HANDOVER_PARENT_PID`, toma solo el principal. Quien fije esa variable a
//!   mano solo se salta la compuerta: el principal exclusivo sigue excluyendo
//!   tanto el uso como a las demás operaciones.
//!
//! **Sin espera.** Todos los bloqueos son `try_*`: la contención o cualquier error
//! del sistema salen como `lifecycle_locked` y no se ha modificado nada.
//!
//! **Orden fijo.** Primero la compuerta y después el principal, en ambos modos, lo
//! que impide el interbloqueo entre dos procesos.
//!
//! **Verificación del fichero (Unix).** Tras bloquear, se comprueba que el inodo
//! bloqueado sigue siendo el de la ruta. Si otra operación borró el fichero
//! (`uninstall` lo hace al terminar) entre la apertura y el bloqueo, el descriptor
//! apunta a un inodo huérfano que ya nadie más ve, y bloquearlo no excluiría a
//! nadie: se trata como fallo de bloqueo. En Windows no hace falta: los ficheros se
//! abren sin `FILE_SHARE_DELETE`, de modo que mientras alguien los tiene abiertos
//! nadie puede borrarlos y el huérfano no llega a existir.
//!
//! **No hay crate de bloqueo**: `std::fs::File::try_lock` y `try_lock_shared`
//! están estables desde 1.89 y la toolchain del proyecto es 1.96.
//!
//! Los ficheros se abren con lectura **y** escritura, no solo para anexar: un
//! `O_APPEND` impide el bloqueo exclusivo en Windows.
//!
//! Uso: se toma al principio de cada operación y se suelta al final, también en
//! el camino de error, que es lo que hace `Drop`.

use crate::update::HANDOVER_PARENT_PID_ENV;
use crate::LifecycleError;
use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

/// Ruta del archivo de bloqueo a partir de un directorio de programa: hermano
/// suyo, que es donde la tabla de rutas lo coloca en los cuatro targets.
///
/// Es un parámetro y no la constante porque las operaciones trabajan sobre el
/// directorio de programa **registrado**, que puede no ser el de la convención, y
/// las pruebas aíslan las raíces a temporales. Tener la regla en un solo sitio es
/// lo que impide que una operación tome el bloqueo de un sitio y borre de otro.
pub fn lock_path_for(program_dir: &Path) -> PathBuf {
    sibling(program_dir, crate::LIFECYCLE_LOCK_NAME)
}

/// Ruta de la compuerta de traspaso a partir de un directorio de programa: hermana
/// del archivo de bloqueo.
pub fn gate_path_for(program_dir: &Path) -> PathBuf {
    sibling(program_dir, crate::LIFECYCLE_GATE_NAME)
}

fn sibling(program_dir: &Path, name: &str) -> PathBuf {
    match program_dir.parent() {
        Some(parent) => parent.join(name),
        None => PathBuf::from(name),
    }
}

/// Bloqueo tomado. Se libera solo, al soltar el valor.
#[derive(Debug)]
pub struct LifecycleLock {
    /// Compuerta de traspaso; `None` en el hijo del traspaso, que no la toma. Se
    /// conserva abierta mientras viva el bloqueo: es el descriptor que el sistema
    /// tiene tomado, no un dato que se consulte.
    _gate: Option<File>,
    /// Bloqueo principal; `None` tras [`LifecycleLock::release_main`].
    main: Option<File>,
    path: PathBuf,
    gate_path: PathBuf,
}

impl LifecycleLock {
    /// Archivo de bloqueo principal, para consultas posteriores.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Archivo de la compuerta de traspaso.
    pub fn gate_path(&self) -> &Path {
        &self.gate_path
    }

    /// Suelta solo el bloqueo principal y conserva la compuerta. Es lo que hace
    /// `self update` antes de lanzar el `self install` del binario nuevo, que toma
    /// el principal, y retiene la compuerta hasta que el traspaso vuelve.
    pub fn release_main(&mut self) {
        self.main = None;
    }

    /// Borra el archivo de bloqueo y la compuerta, y suelta ambos. Es el último
    /// paso con efectos de `self uninstall`. Devuelve los ficheros que no se
    /// pudieron borrar, con su causa; uno que ya no existía no cuenta.
    ///
    /// En Unix se borra **con los bloqueos aún tomados**: la verificación de
    /// inodo de cada toma hace seguro el borrado, porque quien abrió el fichero
    /// antes de desvincularlo y lo bloquea después ve que su inodo ya no es el de
    /// la ruta y falla, en vez de creerse dueño de un bloqueo que nadie más
    /// respeta.
    ///
    /// En Windows los ficheros se abren sin `FILE_SHARE_DELETE`, así que mientras
    /// alguien los tiene abiertos nadie puede borrarlos, y por eso no existe el
    /// fichero huérfano. Para borrar hay que soltar primero los descriptores
    /// propios; si en ese instante otro proceso abrió el fichero, el borrado falla
    /// con una violación de compartición y el fichero se queda, sin consecuencias:
    /// su dueño sigue bloqueando el fichero que está en la ruta.
    pub fn remove_files(self) -> Vec<(PathBuf, std::io::Error)> {
        let held = (self._gate, self.main);
        #[cfg(windows)]
        drop(held);
        let mut failures = Vec::new();
        for path in [self.path, self.gate_path] {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => failures.push((path, e)),
            }
        }
        #[cfg(unix)]
        drop(held);
        failures
    }
}

/// Modo de toma de un bloqueo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Shared,
    Exclusive,
}

/// Toma el bloqueo de una operación de ciclo de vida: compuerta y principal en
/// modo exclusivo, sobre los ficheros hermanos de `program_dir`.
///
/// Falla con `lifecycle_locked` si otra operación o un comando de uso lo tiene
/// tomado, y entonces no se ha modificado nada. En el hijo del traspaso de
/// `self update` (variable `AVI_HANDOVER_PARENT_PID` presente) toma solo el
/// principal, porque su padre retiene la compuerta.
pub fn acquire_exclusive(program_dir: &Path) -> Result<LifecycleLock, LifecycleError> {
    acquire_exclusive_with(
        program_dir,
        std::env::var_os(HANDOVER_PARENT_PID_ENV).is_some(),
    )
}

/// Como [`acquire_exclusive`], con el carácter de hijo del traspaso como dato
/// para que las pruebas no dependan del entorno global del proceso.
fn acquire_exclusive_with(
    program_dir: &Path,
    handover_child: bool,
) -> Result<LifecycleLock, LifecycleError> {
    acquire(program_dir, Mode::Exclusive, !handover_child)
}

/// Toma el bloqueo de un comando de uso: compuerta y principal en modo
/// compartido. Coexiste con otros comandos de uso y falla con `lifecycle_locked`
/// si hay una operación de ciclo de vida en curso, incluido el traspaso de un
/// `self update`.
pub fn acquire_shared(program_dir: &Path) -> Result<LifecycleLock, LifecycleError> {
    acquire(program_dir, Mode::Shared, true)
}

fn acquire(
    program_dir: &Path,
    mode: Mode,
    with_gate: bool,
) -> Result<LifecycleLock, LifecycleError> {
    let path = lock_path_for(program_dir);
    let gate_path = gate_path_for(program_dir);
    let gate = if with_gate {
        Some(take(&gate_path, mode)?)
    } else {
        None
    };
    let main = take(&path, mode)?;
    Ok(LifecycleLock {
        _gate: gate,
        main: Some(main),
        path,
        gate_path,
    })
}

/// Abre `path`, lo bloquea sin esperar en el modo pedido y verifica que lo
/// bloqueado sigue siendo el fichero de la ruta.
fn take(path: &Path, mode: Mode) -> Result<File, LifecycleError> {
    let file = open_lock_file(path)?;
    let locked = match mode {
        Mode::Exclusive => file.try_lock(),
        Mode::Shared => file.try_lock_shared(),
    };
    match locked {
        Ok(()) => {}
        // `TryLockError::WouldBlock` es la contención con otra operación; el
        // otro caso es un fallo del sistema, que también impide operar.
        Err(std::fs::TryLockError::WouldBlock) => {
            return Err(LifecycleError::lifecycle_locked(format!(
                "ya hay otra operación de ciclo de vida en curso ({})",
                path.display()
            )))
        }
        Err(std::fs::TryLockError::Error(e)) => {
            return Err(LifecycleError::lifecycle_locked(format!(
                "no se puede comprobar el bloqueo de ciclo de vida ({}): {e}",
                path.display()
            )))
        }
    }
    verify_same_file(&file, path)?;
    Ok(file)
}

/// Comprueba que el descriptor bloqueado sigue siendo el fichero de `path`: mismo
/// dispositivo e inodo. Si la ruta falta o apunta a otro inodo, el bloqueo no
/// excluiría a nadie y se rechaza.
#[cfg(unix)]
fn verify_same_file(file: &File, path: &Path) -> Result<(), LifecycleError> {
    use std::os::unix::fs::MetadataExt;
    let stale = |detail: String| {
        LifecycleError::lifecycle_locked(format!(
            "el fichero de bloqueo cambió mientras se tomaba ({}): {detail}",
            path.display()
        ))
    };
    let held = file.metadata().map_err(|e| stale(e.to_string()))?;
    let current = std::fs::metadata(path).map_err(|e| stale(e.to_string()))?;
    if held.dev() == current.dev() && held.ino() == current.ino() {
        Ok(())
    } else {
        Err(stale("la ruta apunta a otro fichero".to_string()))
    }
}

/// En Windows no hace falta verificar: los ficheros se abren sin
/// `FILE_SHARE_DELETE` (véase `open_lock_file`), así que nadie puede borrar ni
/// renombrar un fichero de bloqueo mientras este descriptor siga abierto, y el
/// fichero bloqueado es siempre el de la ruta.
#[cfg(not(unix))]
fn verify_same_file(_file: &File, _path: &Path) -> Result<(), LifecycleError> {
    Ok(())
}

/// Abre el archivo de bloqueo con lectura y escritura, creándolo si hace falta.
/// El directorio es el del programa, que es de propiedad exclusiva del producto.
fn open_lock_file(path: &Path) -> Result<File, LifecycleError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            LifecycleError::lifecycle_locked(format!("no se puede crear {}: {e}", parent.display()))
        })?;
    }
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    // Sin `FILE_SHARE_DELETE` (`std` lo añade por defecto): mientras haya un
    // descriptor abierto nadie puede borrar el fichero, así que no hay fichero
    // huérfano que bloquear. `FILE_SHARE_READ` es 0x1 y `FILE_SHARE_WRITE` es 0x2.
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0x1 | 0x2);
    }
    options.open(path).map_err(|e| {
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
    use avi_core::exit_codes::ExitCode;

    /// Directorio de programa dentro de un temporal con dueño: los ficheros de
    /// bloqueo quedan como hermanos suyos, dentro del temporal.
    fn program_dir(dir: &Path) -> PathBuf {
        dir.join("program")
    }

    fn assert_locked(result: Result<LifecycleLock, LifecycleError>) {
        let err = result.expect_err("debe fallar por contención");
        assert_eq!(err.reason, "lifecycle_locked");
        assert_eq!(
            ExitCode::from_reason(err.reason).code(),
            17,
            "LifecycleLocked = 17"
        );
    }

    /// Dos operaciones concurrentes: la segunda falla con `lifecycle_locked` sin
    /// haber modificado nada, y al soltar la primera el bloqueo queda libre.
    ///
    /// La contención se puede comprobar dentro de un mismo proceso porque el
    /// bloqueo es por descriptor abierto, no por proceso: dos `File` sobre el
    /// mismo archivo se estorban igual que en dos procesos distintos.
    #[test]
    fn second_exclusive_fails_with_lifecycle_locked() {
        let dir = scratch("lock");
        let program = program_dir(&dir);

        let first = acquire_exclusive_with(&program, false).expect("la primera toma el bloqueo");
        assert_eq!(first.path(), lock_path_for(&program));
        assert_eq!(first.gate_path(), gate_path_for(&program));

        let err = acquire_exclusive_with(&program, false).unwrap_err();
        assert_eq!(err.reason, "lifecycle_locked");
        assert_eq!(ExitCode::from_reason(err.reason).code(), 17);
        assert!(err.message.contains("en curso"), "{}", err.message);

        drop(first);
        let second =
            acquire_exclusive_with(&program, false).expect("el bloqueo vuelve a estar libre");
        drop(second);
    }

    /// Los comandos de uso coexisten entre sí.
    #[test]
    fn shared_with_shared_coexists() {
        let dir = scratch("lock-shared-shared");
        let program = program_dir(&dir);
        let first = acquire_shared(&program).expect("el primer uso entra");
        let second = acquire_shared(&program).expect("el segundo uso entra a la vez");
        drop((first, second));
    }

    /// Un uso en curso impide la operación, y una operación en curso impide el uso.
    #[test]
    fn shared_and_exclusive_exclude_each_other() {
        let dir = scratch("lock-shared-exclusive");
        let program = program_dir(&dir);

        let usage = acquire_shared(&program).expect("el uso entra");
        assert_locked(acquire_exclusive_with(&program, false));
        drop(usage);

        let operation = acquire_exclusive_with(&program, false).expect("la operación entra");
        assert_locked(acquire_shared(&program));
        drop(operation);
        drop(acquire_shared(&program).expect("el uso vuelve a entrar"));
    }

    /// La compuerta exclusiva impide el uso aunque el principal esté libre: es la
    /// ventana del traspaso de `self update`.
    #[test]
    fn exclusive_gate_blocks_shared_even_with_free_main() {
        let dir = scratch("lock-gate");
        let program = program_dir(&dir);

        let mut parent = acquire_exclusive_with(&program, false).expect("el padre toma ambos");
        parent.release_main();
        assert_locked(acquire_shared(&program));
        drop(parent);
        drop(acquire_shared(&program).expect("sin compuerta, el uso entra"));
    }

    /// El hijo del traspaso toma el principal con la compuerta retenida por su
    /// padre; un hijo no excluye a los demás, y el principal sigue siendo exclusivo.
    #[test]
    fn handover_child_takes_main_while_gate_is_held() {
        let dir = scratch("lock-handover");
        let program = program_dir(&dir);

        let mut parent = acquire_exclusive_with(&program, false).expect("el padre toma ambos");
        // Con el principal aún en manos del padre, el hijo no entra.
        assert_locked(acquire_exclusive_with(&program, true));
        parent.release_main();

        // Quien no es hijo no atraviesa la compuerta.
        assert_locked(acquire_exclusive_with(&program, false));
        let child = acquire_exclusive_with(&program, true).expect("el hijo toma solo el principal");
        // El principal exclusivo del hijo excluye al uso y a otro hijo.
        assert_locked(acquire_shared(&program));
        assert_locked(acquire_exclusive_with(&program, true));
        drop(child);
        drop(parent);
    }

    /// `remove_files` borra el bloqueo y la compuerta del dueño de ambos.
    #[test]
    fn remove_files_deletes_both_files() {
        let dir = scratch("lock-remove");
        let program = program_dir(&dir);
        let lock = acquire_exclusive_with(&program, false).expect("toma ambos");
        assert!(lock_path_for(&program).exists());
        assert!(gate_path_for(&program).exists());

        let failures = lock.remove_files();
        assert!(failures.is_empty(), "se borran sin fallos: {failures:?}");

        assert!(!lock_path_for(&program).exists(), "el bloqueo no queda");
        assert!(!gate_path_for(&program).exists(), "la compuerta no queda");
        drop(acquire_exclusive_with(&program, false).expect("se pueden volver a crear"));
    }

    /// En Windows nadie puede borrar un fichero de bloqueo mientras haya un
    /// descriptor abierto, así que no existe el fichero huérfano; y si otro lo
    /// mantiene abierto, `remove_files` informa del fallo y el fichero se queda.
    #[cfg(windows)]
    #[test]
    fn open_lock_file_cannot_be_deleted_while_held() {
        let dir = scratch("lock-no-delete");
        let program = program_dir(&dir);
        let usage = acquire_shared(&program).expect("el uso toma el bloqueo");

        assert!(
            std::fs::remove_file(lock_path_for(&program)).is_err(),
            "el bloqueo abierto no se borra por ruta"
        );
        assert!(
            std::fs::remove_file(gate_path_for(&program)).is_err(),
            "la compuerta abierta no se borra por ruta"
        );

        let owner = acquire_shared(&program).expect("otro uso coexiste");
        let failures = owner.remove_files();
        assert_eq!(failures.len(), 2, "con otro uso vivo no se borra ninguno");
        assert!(lock_path_for(&program).exists());
        drop(usage);
    }

    /// Un fichero desvinculado tras abrirlo no excluye a nadie: la verificación de
    /// inodo lo rechaza.
    #[cfg(unix)]
    #[test]
    fn unlinked_file_fails_verification() {
        let dir = scratch("lock-unlinked");
        let path = dir.join(crate::LIFECYCLE_LOCK_NAME);
        let file = open_lock_file(&path).expect("se abre");
        std::fs::remove_file(&path).expect("se desvincula");
        file.try_lock().expect("el inodo huérfano se deja bloquear");

        let err = verify_same_file(&file, &path).unwrap_err();
        assert_eq!(err.reason, "lifecycle_locked");

        // La ruta recreada es otro inodo: tampoco coincide.
        let _other = open_lock_file(&path).expect("se recrea");
        let err = verify_same_file(&file, &path).unwrap_err();
        assert_eq!(err.reason, "lifecycle_locked");
    }

    /// La ruta registrada y la de la compuerta son hermanas del directorio de
    /// programa y distintas entre sí.
    #[test]
    fn lock_and_gate_are_distinct_siblings() {
        let program = Path::new("base").join("program");
        assert_eq!(
            lock_path_for(&program),
            Path::new("base").join(crate::LIFECYCLE_LOCK_NAME)
        );
        assert_eq!(
            gate_path_for(&program),
            Path::new("base").join(crate::LIFECYCLE_GATE_NAME)
        );
    }
}
