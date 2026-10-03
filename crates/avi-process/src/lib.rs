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
    let ps = |field: &str| -> Option<String> {
        let out = std::process::Command::new("ps")
            .args(["-o", field, "-p", &pid.to_string()])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!text.is_empty()).then_some(text)
    };
    let start = ps("lstart=")?;
    let comm = ps("comm=")?;
    let image = comm.rsplit('/').next()?.to_string();
    Some(ProcessIdentity { start, image })
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
