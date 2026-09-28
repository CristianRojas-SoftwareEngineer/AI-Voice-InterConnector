//! Primitivas de procesos del sistema operativo.
//!
//! Responsabilidad: flags de creación de procesos en Windows y borrado diferido
//! de rutas. Es la definición única para el binario y para `xtask`; ningún otro
//! crate escribe flags como literales ni reimplementa el borrado diferido.

// `CREATE_NO_WINDOW`: el proceso recibe una consola propia oculta que heredan
// sus hijos de consola, que así no abren ventana.
// `DETACHED_PROCESS`: el proceso queda sin consola; solo sirve para procesos
// que no lanzan hijos de consola, porque cada hijo abriría una ventana visible.
// `CREATE_NEW_PROCESS_GROUP`: grupo de procesos propio.
#[cfg(windows)]
pub use windows_sys::Win32::System::Threading::{
    CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS,
};

#[cfg(windows)]
mod deferred;
#[cfg(windows)]
pub use deferred::spawn_deferred_removal;
