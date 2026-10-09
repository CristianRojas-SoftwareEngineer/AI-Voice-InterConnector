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
pub use deferred::schedule_clean_removal;

pub mod registry;
pub mod restricted_spawn;
pub use restricted_spawn::{
    spawn_with_allowlist, RestrictedChild, RestrictedSpawnRequest, StdinSpec,
};

/// Identidad observable de un proceso: la hora de creación y el nombre de la
/// imagen. Un PID reasignado a otro programa tiene otra identidad, de modo que
/// compararla con la registrada distingue a nuestro proceso de uno ajeno.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ProcessIdentity {
    /// Hora de creación del proceso, en una representación opaca que solo se
    /// compara por igualdad.
    pub start: String,
    /// Nombre de fichero de la imagen del proceso.
    pub image: String,
}

/// Observa la identidad del proceso con ese PID. `None` si no existe o no se
/// puede observar.
#[cfg(windows)]
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    use windows_sys::Win32::Foundation::{CloseHandle, FILETIME};
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return None;
    }
    // SAFETY: el handle se abre con permiso de solo consulta, se usa dentro de
    // esta función con búferes locales del tamaño declarado y se cierra siempre.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return None;
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        let times_ok = GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user);
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let name_ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if times_ok == 0 || name_ok == 0 {
            return None;
        }
        // Un proceso terminado cuyo handle sigue abierto en otro sitio conserva
        // la hora de salida: ya no es un proceso vivo.
        if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
            return None;
        }
        let start = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let image = path.rsplit(['\\', '/']).next()?.to_lowercase();
        Some(ProcessIdentity {
            start: start.to_string(),
            image,
        })
    }
}

/// Observa la identidad del proceso con ese PID. `None` si no existe o no se
/// puede observar.
#[cfg(unix)]
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    // Un PID que no cabe en `pid_t` firmado se interpretaría como grupo o
    // difusión en las llamadas del sistema: no es un proceso.
    if pid == 0 || i32::try_from(pid).is_err() {
        return None;
    }
    // Una sola invocación de `ps` con `LC_ALL=C`: dos llamadas separadas podrían
    // observar dos vidas distintas del PID, y sin locale fijo `lstart` cambia de
    // forma y la identidad no coincidiría con la registrada. `lstart=` y
    // `comm=` viajan juntos para leer ambas en el mismo resultado.
    let out = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-o", "lstart=,comm=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let (start, comm) = split_ps_identity(&out.stdout)?;
    let image = comm.rsplit('/').next()?.to_string();
    Some(ProcessIdentity { start, image })
}

/// Separa `lstart` y `comm` de la línea de una única invocación de
/// `ps -o lstart=,comm=`: `lstart` ocupa sus 24 caracteres fijos
/// (`%a %b %e %H:%M:%S %Y`, p. ej. `Wed Oct  5 12:34:56 2026`) y `comm` es lo
/// que sigue tras los espacios separadores.
#[cfg(unix)]
fn split_ps_identity(stdout: &[u8]) -> Option<(String, String)> {
    let text = String::from_utf8_lossy(stdout);
    let line = text.trim();
    let start = line.get(..24)?;
    let comm = line.get(24..)?.trim_start();
    if comm.is_empty() {
        return None;
    }
    Some((start.to_string(), comm.to_string()))
}

/// Viveza exacta de un PID a nivel de sistema.
///
/// Windows: `OpenProcess` + `GetExitCodeProcess` (vivo solo si el código de
/// salida es `STILL_ACTIVE`); la consulta es por PID exacto y nunca por
/// comparación de texto, que confundiría un PID con otro que lo contiene (el 12
/// dentro del 112). Un proceso que el sistema no deja abrir para consulta cuenta
/// como no vivo. Unix: `libc::kill(pid, 0)` (éxito = vivo), sin lanzar el
/// ejecutable `kill` externo.
/// `0` nunca está vivo. Bloqueante y breve.
#[cfg(windows)]
pub fn pid_alive_exact(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, STILL_ACTIVE};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return false;
    }
    // SAFETY: el handle se abre con permiso de solo consulta y se cierra
    // siempre antes de volver; el código de salida es una variable local.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return false;
        }
        let mut exit_code = 0u32;
        let ok = GetExitCodeProcess(handle, &mut exit_code);
        CloseHandle(handle);
        // `STILL_ACTIVE` es `i32` en windows-sys y el código de salida es u32.
        ok != 0 && exit_code == STILL_ACTIVE as u32
    }
}

/// Viveza exacta de un PID a nivel de sistema.
///
/// Unix: `libc::kill(pid, 0)` (éxito = vivo), sin lanzar el ejecutable `kill`
/// externo. `0` nunca está vivo. Bloqueante y breve.
#[cfg(unix)]
pub fn pid_alive_exact(pid: u32) -> bool {
    if pid == 0 || !fits_pid_t(pid) {
        return false;
    }
    // SAFETY: la señal 0 no se entrega; solo sondea la existencia del PID, ya
    // validado como `pid_t` representable.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

/// Un PID de Unix es un `pid_t` con signo de 32 bits. `kill` trunca a ese tipo
/// el número que recibe, así que un valor mayor (de un pidfile corrupto) señala
/// a otro proceso: 4294967294 se convierte en el grupo -2 y su negativo, en el
/// PID 2. Un PID que no cabe no es un proceso y nunca se pasa a `kill`. La
/// guarda vive aquí, en la primitiva, y cubre a todo llamante (daemon y
/// residente).
#[cfg(unix)]
fn fits_pid_t(pid: u32) -> bool {
    i32::try_from(pid).is_ok()
}

/// Mata el árbol preciso por PID. Windows: `taskkill /F /T /PID` (mata el
/// árbol; es exacto por PID). Unix: `libc::kill(-pid, SIGKILL)` al grupo (el
/// daemon es líder de sesión por `setsid`) y luego `libc::kill(pid, SIGKILL)`,
/// sin lanzar el ejecutable `kill` externo. No verifica: el llamante combina
/// con `pid_alive_exact` y deadline. Nunca mata el PID 0 ni un PID que no
/// cabe en `pid_t`; la guarda contra auto-muerte (`pid != proceso propio`)
/// vive en el llamante.
#[cfg(windows)]
pub fn kill_tree_by_pid_exact(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use std::os::windows::process::CommandExt;
    // Sin consola propia, un invocador sin consola vería abrirse y cerrarse una
    // ventana por cada `taskkill`.
    std::process::Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Mata el árbol preciso por PID: `libc::kill(-pid, SIGKILL)` al grupo (el
/// daemon es líder de sesión por `setsid`) y luego `libc::kill(pid, SIGKILL)`,
/// sin lanzar el ejecutable `kill` externo. No verifica: el llamante combina
/// con `pid_alive_exact` y deadline. Nunca mata el PID 0 ni un PID que no cabe
/// en `pid_t`; la guarda contra auto-muerte (`pid != proceso propio`) vive en
/// el llamante. Solo alcanza al grupo y al PID indicados: el residente nace con
/// su propia sesión y no pertenece al grupo del daemon.
#[cfg(unix)]
pub fn kill_tree_by_pid_exact(pid: u32) -> bool {
    if pid == 0 || !fits_pid_t(pid) {
        return false;
    }
    let pid = pid as i32;
    // SAFETY: señales `SIGKILL` a un PID ya validado como `pid_t`
    // representable; el negativo señala al grupo de ese PID.
    unsafe {
        let _ = libc::kill(-pid, libc::SIGKILL);
        libc::kill(pid, libc::SIGKILL) == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// La identidad propia se puede observar y no cambia entre lecturas.
    #[test]
    fn own_process_identity_is_stable() {
        let first =
            process_identity(std::process::id()).expect("el proceso propio tiene identidad");
        let second = process_identity(std::process::id()).expect("segunda lectura");
        assert_eq!(first, second);
        assert!(!first.start.is_empty() && !first.image.is_empty());
    }

    /// El PID 0 y un PID inexistente no tienen identidad.
    #[test]
    fn absent_pid_has_no_identity() {
        assert_eq!(process_identity(0), None);
        assert_eq!(process_identity(4_000_000_000), None);
    }

    /// El PID propio está vivo: la primitiva exacta debe reconocerlo.
    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive_exact(std::process::id()));
    }

    /// El PID 0 nunca está vivo ni se mata.
    #[test]
    fn zero_pid_is_neither_alive_nor_killed() {
        assert!(!pid_alive_exact(0));
        assert!(!kill_tree_by_pid_exact(0));
    }

    /// Un PID que no cabe en `pid_t` firmado no está vivo ni se mata: en Unix
    /// `kill` trunca el valor a 32 bits con signo y señalaría a otro proceso
    /// (4294967294 se convierte en el grupo -2 y su negativo, en el PID 2); en
    /// Windows `OpenProcess` no abre un PID inexistente.
    #[test]
    fn out_of_range_pid_is_neither_alive_nor_killed() {
        let pid = u32::MAX - 1;
        assert!(!pid_alive_exact(pid));
        assert!(!kill_tree_by_pid_exact(pid));
    }

    /// Un hijo muerto y recolectado deja de estar vivo: la viveza es del PID
    /// exacto, no de una coincidencia de texto sobre la tabla de procesos (el
    /// PID 12 no se da por vivo porque exista el 112).
    #[test]
    fn dead_child_is_not_alive() {
        #[cfg(windows)]
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("se lanza el hijo");
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("se lanza el hijo");
        let pid = child.id();
        assert!(pid_alive_exact(pid), "el hijo vivo está vivo");
        child.kill().expect("se mata al hijo");
        child.wait().expect("se espera al hijo");
        assert!(!pid_alive_exact(pid), "el hijo muerto no está vivo");
    }

    /// Un hijo vivo tiene identidad distinta de la propia y deja de tenerla al morir.
    #[test]
    fn child_identity_differs_from_own() {
        #[cfg(windows)]
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("se lanza el hijo");
        #[cfg(not(windows))]
        let mut child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("se lanza el hijo");
        let pid = child.id();
        let own = process_identity(std::process::id()).expect("identidad propia");
        let theirs = process_identity(pid).expect("el hijo vivo tiene identidad");
        assert_ne!(own, theirs);
        child.kill().expect("se mata al hijo");
        child.wait().expect("se espera al hijo");
        assert_eq!(
            process_identity(pid),
            None,
            "el hijo muerto no tiene identidad"
        );
    }
}
