#![allow(non_camel_case_types)]
#![allow(non_snake_case)]
#![no_std]

use libc::{pid_t, pollfd, timespec, timeval, FILE};

pub const SND_PCM_NONBLOCK: core::ffi::c_int = 0x1;
pub const SND_PCM_ASYNC: core::ffi::c_int = 0x2;

pub const SND_SEQ_OPEN_OUTPUT: i32 = 1;
pub const SND_SEQ_OPEN_INPUT: i32 = 2;
pub const SND_SEQ_OPEN_DUPLEX: i32 = SND_SEQ_OPEN_OUTPUT | SND_SEQ_OPEN_INPUT;
pub const SND_SEQ_NONBLOCK: i32 = 0x0001;
pub const SND_SEQ_ADDRESS_BROADCAST: u8 = 255;
pub const SND_SEQ_ADDRESS_SUBSCRIBERS: u8 = 254;
pub const SND_SEQ_ADDRESS_UNKNOWN: u8 = 253;
pub const SND_SEQ_QUEUE_DIRECT: u8 = 253;
pub const SND_SEQ_TIME_MODE_MASK: u8 = 1 << 1;
pub const SND_SEQ_TIME_STAMP_MASK: u8 = 1 << 0;
pub const SND_SEQ_TIME_MODE_REL: u8 = 1 << 1;
pub const SND_SEQ_TIME_STAMP_REAL: u8 = 1 << 0;
pub const SND_SEQ_TIME_STAMP_TICK: u8 = 0 << 0;
pub const SND_SEQ_TIME_MODE_ABS: u8 = 0 << 1;
pub const SND_SEQ_CLIENT_SYSTEM: u8 = 0;
pub const SND_SEQ_PORT_SYSTEM_TIMER: u8 = 0;
pub const SND_SEQ_PORT_SYSTEM_ANNOUNCE: u8 = 1;
pub const SND_SEQ_PRIORITY_HIGH: u8 = 1 << 4;
pub const SND_SEQ_EVENT_LENGTH_FIXED: u8 = 0 << 2;
pub const SND_SEQ_EVENT_LENGTH_MASK: u8 = 3 << 2;
pub const SND_SEQ_EVENT_LENGTH_VARIABLE: u8 = 1 << 2;
pub const SND_SEQ_EVENT_LENGTH_VARUSR: u8 = 2 << 2;

// Patch: las funciones no se enlazan contra la librería de ALSA; `build.rs`
// genera en `bindings.rs` envolturas con la misma firma que resuelven cada
// símbolo en tiempo de ejecución (ver `dynamic`).
include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

mod required_symbols;

/// Motivo por el que ALSA no se puede usar en este sistema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unavailable {
    /// `libasound.so.2` no se puede cargar.
    Library,
    /// La librería carga pero no define esta función, que usa el crate `alsa`.
    Symbol(&'static core::ffi::CStr),
}

/// Comprueba que `libasound.so.2` se puede cargar y define todas las funciones
/// que usa el crate `alsa`. Llamarla antes de usar cualquier función del crate
/// permite fallar con un error propio en lugar de abortar en la primera
/// llamada a una función ausente.
pub fn availability() -> Result<(), Unavailable> {
    let lib = dynamic::handle();
    if lib.is_null() {
        return Err(Unavailable::Library);
    }
    match required_symbols::REQUIRED_SYMBOLS
        .iter()
        .copied()
        // SAFETY: handle válido devuelto por `dlopen` y nombres terminados en NUL.
        .find(|name| unsafe { libc::dlsym(lib, name.as_ptr()) }.is_null())
    {
        Some(name) => Err(Unavailable::Symbol(name)),
        None => Ok(()),
    }
}

/// Carga diferida de `libasound.so.2` y resolución de sus símbolos.
mod dynamic {
    use core::ffi::{c_void, CStr};
    use core::ptr::null_mut;
    use core::sync::atomic::{AtomicPtr, Ordering};

    static HANDLE: AtomicPtr<c_void> = AtomicPtr::new(null_mut());

    /// Handle de `libasound.so.2`, abierto una sola vez; nulo si el sistema no
    /// la tiene. Dos hilos que la abran a la vez obtienen el mismo handle, porque
    /// el cargador cuenta referencias de una misma librería.
    pub(crate) fn handle() -> *mut c_void {
        let current = HANDLE.load(Ordering::Acquire);
        if !current.is_null() {
            return current;
        }
        // SAFETY: nombre terminado en NUL y flags válidos de `dlopen`.
        let opened = unsafe { libc::dlopen(c"libasound.so.2".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL) };
        if !opened.is_null() {
            HANDLE.store(opened, Ordering::Release);
        }
        opened
    }

    /// Dirección del símbolo `name`, resuelta una vez y guardada en `cache`.
    /// Llamar a una función de ALSA sin la librería, o con una versión que no
    /// define el símbolo, es un error de programación, porque
    /// `availability()` ya lo habría detectado: aborta nombrándolo.
    pub(crate) fn resolve(cache: &AtomicPtr<c_void>, name: &CStr) -> *mut c_void {
        let cached = cache.load(Ordering::Acquire);
        if !cached.is_null() {
            return cached;
        }
        let lib = handle();
        if lib.is_null() {
            panic!("libasound.so.2 no está disponible y se llamó a {name:?}");
        }
        // SAFETY: handle válido devuelto por `dlopen` y nombre terminado en NUL.
        let symbol = unsafe { libc::dlsym(lib, name.as_ptr()) };
        if symbol.is_null() {
            panic!("libasound.so.2 no define {name:?}");
        }
        cache.store(symbol, Ordering::Release);
        symbol
    }
}
