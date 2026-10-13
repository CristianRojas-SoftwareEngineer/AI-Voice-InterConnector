// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Primitivas de procesos del sistema operativo.
//!
//! Responsabilidad: flags de creación de procesos en Windows y borrado diferido
//! de rutas. Es la definición única para el binario y para `xtask`; ningún otro
//! crate escribe flags como literales ni reimplementa el borrado diferido.

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

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

/// Observa la identidad del proceso con ese PID. `None` si no existe, es un
/// zombi o no se puede observar. Se lee del núcleo, sin lanzar herramientas
/// externas, de modo que funciona en sistemas sin `procps`.
#[cfg(unix)]
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    // Un PID que no cabe en `pid_t` firmado se interpretaría como grupo o
    // difusión en las llamadas del sistema: no es un proceso.
    if pid == 0 || !fits_pid_t(pid) {
        return None;
    }
    #[cfg(target_os = "linux")]
    {
        observe_linux(pid)
    }
    #[cfg(target_os = "macos")]
    {
        observe_macos(pid)
    }
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
compile_error!("la identidad de proceso solo está implementada para Linux y macOS");

/// Linux: una sola lectura de `/proc/<pid>/stat` da `comm` y `starttime`
/// juntos, así que ambos pertenecen a la misma vida del PID. `starttime` cuenta
/// ticks de reloj desde el arranque del sistema: no depende de la zona horaria
/// ni del locale y distingue procesos creados en el mismo segundo.
#[cfg(target_os = "linux")]
fn observe_linux(pid: u32) -> Option<ProcessIdentity> {
    let stat = std::fs::read(format!("/proc/{pid}/stat")).ok()?;
    parse_proc_stat(&String::from_utf8_lossy(&stat))
}

/// Extrae la identidad de una línea de `/proc/<pid>/stat`. `comm` va entre el
/// primer `(` y el último `)`, porque el propio nombre puede contener espacios
/// y paréntesis; tras el último `)` siguen campos separados por espacios, el
/// primero es el estado y el vigésimo, `starttime`. Un zombi (`Z`) o un proceso
/// muerto (`X`) ya no está vivo y no tiene identidad. Se compila en todos los
/// hosts para que sus pruebas corran también fuera de Linux.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn parse_proc_stat(stat: &str) -> Option<ProcessIdentity> {
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let comm = stat.get(open + 1..close)?;
    let mut fields = stat.get(close + 1..)?.split_ascii_whitespace();
    let state = fields.next()?;
    if state == "Z" || state == "X" {
        return None;
    }
    let start = fields.nth(18)?;
    if comm.is_empty() || start.parse::<u64>().is_err() {
        return None;
    }
    Some(ProcessIdentity {
        start: start.to_string(),
        image: comm.to_string(),
    })
}

/// macOS: `proc_pidinfo(PROC_PIDTBSDINFO)` da en una sola llamada la hora de
/// creación, con microsegundos, y el nombre del proceso. Una respuesta de tamaño
/// distinto al de la estructura es un fallo, y un zombi ya no está vivo.
#[cfg(target_os = "macos")]
fn observe_macos(pid: u32) -> Option<ProcessIdentity> {
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // SAFETY: el búfer es una `proc_bsdinfo` local del tamaño declarado, el PID
    // ya está validado como `pid_t` representable y la estructura solo se lee
    // si el núcleo informa haberla escrito completa.
    let info = unsafe {
        let written = libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as libc::c_int,
        );
        if usize::try_from(written).ok()? != size {
            return None;
        }
        info.assume_init()
    };
    if info.pbi_status == libc::SZOMB {
        return None;
    }
    // `pbi_name` admite nombres de hasta 32 bytes; `pbi_comm`, el respaldo del
    // núcleo cuando no hay nombre largo, se corta en 16.
    let name: &[libc::c_char] = if info.pbi_name[0] != 0 {
        &info.pbi_name
    } else {
        &info.pbi_comm
    };
    let bytes: Vec<u8> = name
        .iter()
        .take_while(|&&c| c != 0)
        .map(|&c| c as u8)
        .collect();
    if bytes.is_empty() {
        return None;
    }
    Some(ProcessIdentity {
        start: format!("{}.{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        image: String::from_utf8_lossy(&bytes).into_owned(),
    })
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

/// Plazo de `taskkill`. Responde en milisegundos; 2 s dejan margen ante un
/// sistema cargado y caben varias veces dentro del plazo global de 8 s de la
/// parada del daemon, de modo que una herramienta colgada no lo anula.
#[cfg(windows)]
const EXTERNAL_TOOL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// Lanza `cmd` sin stdin, con stdout capturado y stderr descartado, y espera
/// su fin como mucho `deadline`. `None` si no se pudo lanzar o si venció el
/// plazo, en cuyo caso el hijo ya está muerto y recolectado.
#[cfg(windows)]
fn run_bounded(
    cmd: &mut std::process::Command,
    deadline: std::time::Duration,
) -> Option<std::process::Output> {
    let child = cmd
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    wait_bounded(child, deadline)
}

/// Espera a `child` como mucho `deadline`, sondeando cada 20 ms. Al vencer lo
/// mata y lo recolecta: un hijo abandonado seguiría vivo.
/// La salida capturada es pequeña (una línea), así que la tubería no se llena
/// mientras se sondea.
#[cfg(windows)]
fn wait_bounded(
    mut child: std::process::Child,
    deadline: std::time::Duration,
) -> Option<std::process::Output> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if start.elapsed() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            _ => {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "el hijo puede haber terminado entre el sondeo y la señal; la recolección siguiente cubre ambos casos"
                )]
                let _ = child.kill();
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "el desenlace ya es el vencimiento; la espera solo recolecta al hijo muerto"
                )]
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Mata el árbol preciso por PID. Windows: `taskkill /F /T /PID` (mata el
/// árbol; es exacto por PID), invocado por su ruta fija
/// `%SystemRoot%\System32\taskkill.exe` y con plazo de 2 s. Unix:
/// `libc::kill(-pid, SIGKILL)` al grupo (el daemon es líder de sesión por
/// `setsid`) y luego `libc::kill(pid, SIGKILL)`, sin lanzar el ejecutable
/// `kill` externo. No verifica: el llamante combina con `pid_alive_exact` y
/// deadline. Nunca mata el PID 0 ni un PID que no cabe en `pid_t`; la guarda
/// contra auto-muerte (`pid != proceso propio`) vive en el llamante.
#[cfg(windows)]
pub fn kill_tree_by_pid_exact(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    use std::os::windows::process::CommandExt;
    // Ruta fija: un `taskkill` puesto antes en el `PATH` no suplanta al del
    // sistema. Sin `SystemRoot` la herramienta no se localiza y la primitiva
    // falla.
    let Some(root) = std::env::var_os("SystemRoot") else {
        return false;
    };
    let taskkill = std::path::Path::new(&root)
        .join("System32")
        .join("taskkill.exe");
    // Sin consola propia, un invocador sin consola vería abrirse y cerrarse una
    // ventana por cada `taskkill`.
    run_bounded(
        std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_DEADLINE,
    )
    .is_some_and(|out| out.status.success())
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
        assert!(
            !first.start.contains(['\n', '\r']) && !first.image.contains(['\n', '\r']),
            "la identidad no lleva saltos de línea: {first:?}"
        );
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

    /// Un hijo que no termina dentro del plazo se abandona como fallo: la espera
    /// vuelve al vencer, no cuando el hijo acabaría, y el hijo queda muerto.
    #[cfg(windows)]
    #[test]
    fn bounded_wait_kills_child_on_deadline() {
        let child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::piped())
            .spawn()
            .expect("se lanza el hijo");
        let pid = child.id();
        let start = std::time::Instant::now();
        let out = wait_bounded(child, std::time::Duration::from_millis(300));
        assert!(out.is_none(), "el vencimiento es un fallo");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(5),
            "la espera respeta el plazo: {:?}",
            start.elapsed()
        );
        assert!(!pid_alive_exact(pid), "el hijo vencido está muerto");
    }

    /// Un comando que termina dentro del plazo devuelve su salida capturada.
    #[cfg(windows)]
    #[test]
    fn bounded_run_returns_output_of_fast_command() {
        let mut cmd = std::process::Command::new("cmd");
        cmd.args(["/C", "echo avi"]);
        let out = run_bounded(&mut cmd, std::time::Duration::from_secs(10))
            .expect("el comando termina dentro del plazo");
        assert!(out.status.success());
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "avi");
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

    /// Línea de `/proc/<pid>/stat` con `comm` y `starttime` dados: tras el
    /// último `)` van el estado y 18 campos de relleno antes de `starttime`.
    fn stat_line(comm: &str, state: &str, starttime: &str) -> String {
        let filler: Vec<String> = (1..=18).map(|n| n.to_string()).collect();
        format!(
            "4242 ({comm}) {state} {} {starttime} 7 8 9",
            filler.join(" ")
        )
    }

    /// Una línea normal da `comm` como imagen y `starttime` como inicio.
    #[test]
    fn proc_stat_parses_comm_and_starttime() {
        let id = parse_proc_stat(&stat_line("bash", "S", "987654")).expect("línea válida");
        assert_eq!(id.image, "bash");
        assert_eq!(id.start, "987654");
    }

    /// `comm` puede contener espacios: se toma completo hasta el último `)`.
    #[test]
    fn proc_stat_comm_with_spaces() {
        let id = parse_proc_stat(&stat_line("tmux: server", "S", "11")).expect("línea válida");
        assert_eq!(id.image, "tmux: server");
        assert_eq!(id.start, "11");
    }

    /// `comm` puede contener paréntesis: el corte es el último `)` de la línea.
    #[test]
    fn proc_stat_comm_with_parentheses() {
        let id = parse_proc_stat(&stat_line("a) b (c", "R", "12")).expect("línea válida");
        assert_eq!(id.image, "a) b (c");
        assert_eq!(id.start, "12");
    }

    /// Una línea sin todos los campos no tiene identidad.
    #[test]
    fn proc_stat_truncated_line_has_no_identity() {
        assert_eq!(parse_proc_stat("4242 (bash) S 1 2 3"), None);
        assert_eq!(parse_proc_stat("4242 (bash"), None);
        assert_eq!(parse_proc_stat(""), None);
    }

    /// Un `starttime` no numérico no es una identidad.
    #[test]
    fn proc_stat_non_numeric_starttime_has_no_identity() {
        assert_eq!(parse_proc_stat(&stat_line("bash", "S", "x12")), None);
    }

    /// Un zombi o un proceso muerto no tiene identidad.
    #[test]
    fn proc_stat_zombie_has_no_identity() {
        assert_eq!(parse_proc_stat(&stat_line("bash", "Z", "12")), None);
        assert_eq!(parse_proc_stat(&stat_line("bash", "X", "12")), None);
    }

    /// En Linux, la identidad propia coincide con lo que publica
    /// `/proc/self/stat`.
    #[cfg(target_os = "linux")]
    #[test]
    fn own_identity_matches_proc_self_stat() {
        let raw = std::fs::read_to_string("/proc/self/stat").expect("se lee /proc/self/stat");
        let expected = parse_proc_stat(&raw).expect("línea propia válida");
        let own = process_identity(std::process::id()).expect("identidad propia");
        assert_eq!(own, expected);
    }
}
