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
pub use deferred::{schedule_removal, schedule_removal_with};

pub use avi_shared::{OwnerState, OwnerToken};

pub mod registry;
pub mod restricted_spawn;
pub mod scratch;
pub use restricted_spawn::{
    spawn_with_allowlist, spawn_with_allowlist_env, RestrictedChild, RestrictedSpawnRequest,
    StdinSpec,
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

/// Desenlace de observar la identidad de un PID. Separa «no existe» de «existe
/// pero no se puede observar», que exigen decisiones opuestas: lo primero es un
/// proceso muerto y lo segundo, un dueño desconocido que no se debe dar por
/// muerto.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdentityObservation {
    /// El proceso existe y esta es su identidad.
    Present(ProcessIdentity),
    /// No existe un proceso vivo con ese PID.
    Gone,
    /// El sistema no permite observarlo, o la observación falló: no se sabe si
    /// existe.
    Denied,
}

/// Observa la identidad del proceso con ese PID. `None` si no existe o no se
/// puede observar; quien necesite distinguir ambos casos usa
/// [`observe_identity`].
pub fn process_identity(pid: u32) -> Option<ProcessIdentity> {
    match observe_identity(pid) {
        IdentityObservation::Present(identity) => Some(identity),
        IdentityObservation::Gone | IdentityObservation::Denied => None,
    }
}

/// Observa la identidad del proceso con ese PID. Windows: `OpenProcess` que
/// falla con `ERROR_INVALID_PARAMETER` es un PID sin proceso; con
/// `ERROR_ACCESS_DENIED` (o cualquier otro error) el proceso no es observable.
/// Un proceso que ya tiene hora de salida es `Gone` aunque el handle se abra.
#[cfg(windows)]
pub fn observe_identity(pid: u32) -> IdentityObservation {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, FILETIME,
    };
    use windows_sys::Win32::System::Threading::{
        GetProcessTimes, OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };

    if pid == 0 {
        return IdentityObservation::Gone;
    }
    // SAFETY: el handle se abre con permiso de solo consulta, se usa dentro de
    // esta función con búferes locales del tamaño declarado y se cierra siempre.
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle == 0 {
            return if GetLastError() == ERROR_INVALID_PARAMETER {
                IdentityObservation::Gone
            } else {
                IdentityObservation::Denied
            };
        }
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut created, mut exited, mut kernel, mut user) = (zero, zero, zero, zero);
        if GetProcessTimes(handle, &mut created, &mut exited, &mut kernel, &mut user) == 0 {
            CloseHandle(handle);
            return IdentityObservation::Denied;
        }
        // Un proceso terminado cuyo handle sigue abierto en otro sitio (un
        // antivirus, el agente de la CI) conserva la hora de salida: ya no es un
        // proceso vivo. Se decide antes de pedir el nombre de la imagen, que el
        // sistema ya no da para un proceso terminado y cuyo fallo lo haría pasar
        // por no observable.
        if exited.dwLowDateTime != 0 || exited.dwHighDateTime != 0 {
            CloseHandle(handle);
            return IdentityObservation::Gone;
        }
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let name_ok = QueryFullProcessImageNameW(handle, 0, buf.as_mut_ptr(), &mut len);
        CloseHandle(handle);
        if name_ok == 0 {
            return IdentityObservation::Denied;
        }
        let start = ((created.dwHighDateTime as u64) << 32) | created.dwLowDateTime as u64;
        let path = String::from_utf16_lossy(&buf[..len as usize]);
        let Some(name) = path.rsplit(['\\', '/']).next() else {
            return IdentityObservation::Denied;
        };
        IdentityObservation::Present(ProcessIdentity {
            start: start.to_string(),
            image: name.to_lowercase(),
        })
    }
}

/// Observa la identidad del proceso con ese PID. Se lee del núcleo, sin lanzar
/// herramientas externas, de modo que funciona en sistemas sin `procps`. Un
/// zombi ya no está vivo. Un error de «no existe» (`ESRCH`) es `Gone` y uno de
/// permisos (`EPERM`) o cualquier otro, `Denied`.
#[cfg(unix)]
pub fn observe_identity(pid: u32) -> IdentityObservation {
    // Un PID que no cabe en `pid_t` firmado se interpretaría como grupo o
    // difusión en las llamadas del sistema: no es un proceso.
    if pid == 0 || !fits_pid_t(pid) {
        return IdentityObservation::Gone;
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

/// Identificador del arranque actual del sistema, sin guiones y en minúsculas
/// (`[0-9a-f]+`). `None` si no se puede leer.
#[cfg(target_os = "linux")]
fn boot_id() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/sys/kernel/random/boot_id").ok()?;
    normalize_boot_id(&raw)
}

/// Quita los guiones y el salto de línea final del `boot_id` del núcleo y lo
/// pasa a minúsculas. `None` si queda vacío o con algo distinto de dígitos
/// hexadecimales. Se compila en todos los hosts para que sus pruebas corran
/// también fuera de Linux.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn normalize_boot_id(raw: &str) -> Option<String> {
    let id: String = raw
        .trim()
        .chars()
        .filter(|c| *c != '-')
        .map(|c| c.to_ascii_lowercase())
        .collect();
    (!id.is_empty() && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
compile_error!("la identidad de proceso solo está implementada para Linux y macOS");

/// Linux: una sola lectura de `/proc/<pid>/stat` da `comm` y `starttime`
/// juntos, así que ambos pertenecen a la misma vida del PID. `starttime` cuenta
/// ticks de reloj desde el arranque del sistema: no depende de la zona horaria
/// ni del locale y distingue procesos creados en el mismo segundo, pero se
/// repite tras un reinicio. Por eso el inicio lleva delante el `boot_id`, que
/// cambia en cada arranque. Un PID ausente en `/proc` es `Gone`; un error de
/// permisos o cualquier otro, `Denied`.
#[cfg(target_os = "linux")]
fn observe_linux(pid: u32) -> IdentityObservation {
    let stat = match std::fs::read(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return IdentityObservation::Gone,
        Err(_) => return IdentityObservation::Denied,
    };
    // Un zombi o un proceso muerto no tiene identidad: ya no está vivo.
    let Some(mut identity) = parse_proc_stat(&String::from_utf8_lossy(&stat)) else {
        return IdentityObservation::Gone;
    };
    let Some(boot) = boot_id() else {
        return IdentityObservation::Denied;
    };
    identity.start = format!("{boot}{}", identity.start);
    IdentityObservation::Present(identity)
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
/// distinto al de la estructura es un fallo, y un zombi ya no está vivo. Si la
/// llamada no escribe nada, `ESRCH` es un PID sin proceso (`Gone`) y cualquier
/// otro `errno`, como `EPERM`, un proceso no observable (`Denied`).
#[cfg(target_os = "macos")]
fn observe_macos(pid: u32) -> IdentityObservation {
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
        if written <= 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
                IdentityObservation::Gone
            } else {
                IdentityObservation::Denied
            };
        }
        if usize::try_from(written).ok() != Some(size) {
            return IdentityObservation::Denied;
        }
        info.assume_init()
    };
    if info.pbi_status == libc::SZOMB {
        return IdentityObservation::Gone;
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
        return IdentityObservation::Denied;
    }
    // Segundos y microsegundos con ancho fijo y sin separador: el inicio queda
    // en `[0-9]+` y se compara por igualdad como una sola cifra.
    IdentityObservation::Present(ProcessIdentity {
        start: format!("{:020}{:06}", info.pbi_start_tvsec, info.pbi_start_tvusec),
        image: String::from_utf8_lossy(&bytes).into_owned(),
    })
}

/// Token de propietario del proceso actual: su PID y el inicio observado. Falla
/// si el proceso no puede observarse a sí mismo, porque sin token no hay forma
/// de que otro lo reconozca como dueño.
pub fn current_owner_token() -> anyhow::Result<OwnerToken> {
    owner_token_of(std::process::id())
}

/// Token de propietario del proceso `pid`, con el inicio que se le observa
/// ahora. Falla si el proceso no existe o no puede observarse.
pub fn owner_token_of(pid: u32) -> anyhow::Result<OwnerToken> {
    match observe_identity(pid) {
        IdentityObservation::Present(identity) => OwnerToken::new(pid, identity.start.clone())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "el inicio observado del proceso {pid} ({:?}) no es un token válido",
                    identity.start
                )
            }),
        IdentityObservation::Gone | IdentityObservation::Denied => Err(anyhow::anyhow!(
            "no se pudo observar la identidad del proceso (pid {pid})"
        )),
    }
}

/// Clasifica el propietario que describe `token` observando el sistema.
pub fn classify_owner(token: &OwnerToken) -> OwnerState {
    classify_owner_with(token, observe_identity)
}

/// Núcleo de [`classify_owner`] con la observación inyectada, para probar los
/// desenlaces que el sistema real no produce a voluntad.
fn classify_owner_with(
    token: &OwnerToken,
    observe: impl FnOnce(u32) -> IdentityObservation,
) -> OwnerState {
    match observe(token.pid()) {
        IdentityObservation::Present(identity) if identity.start == token.start() => {
            OwnerState::Ours
        }
        IdentityObservation::Present(_) | IdentityObservation::Gone => OwnerState::Dead,
        IdentityObservation::Denied => OwnerState::Unknown,
    }
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

/// El recurso directo solo procede si `taskkill` falló y el PID aún vive.
#[cfg(windows)]
fn should_terminate_after_failed_taskkill(taskkill_ok: bool, alive: bool) -> bool {
    !taskkill_ok && alive
}

/// Mata el árbol preciso por PID. Windows: `taskkill /F /T /PID` (mata el
/// árbol; es exacto por PID), invocado por su ruta fija
/// `%SystemRoot%\System32\taskkill.exe` y con plazo de 2 s. Si falla y
/// el PID sigue vivo, termina ese proceso por API: en el daemon, el residente
/// supervisado recibe fin de fichero en su entrada y también termina. Unix:
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
    let taskkill_ok = run_bounded(
        std::process::Command::new(taskkill)
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW),
        EXTERNAL_TOOL_DEADLINE,
    )
    .is_some_and(|out| out.status.success());
    if taskkill_ok {
        return true;
    }
    // Bajo carga, `taskkill` puede no encontrar un PID que sigue vivo. Primero
    // se intentó reclamar el árbol; solo entonces se termina el PID exacto.
    if !should_terminate_after_failed_taskkill(taskkill_ok, pid_alive_exact(pid)) {
        return false;
    }
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{OpenProcess, TerminateProcess, PROCESS_TERMINATE};
    // SAFETY: se abre el PID vivo solo con permiso de terminación; el handle
    // se cierra tanto si la terminación funciona como si falla.
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, pid);
        if handle == 0 {
            return false;
        }
        let terminated = TerminateProcess(handle, 1) != 0;
        CloseHandle(handle);
        terminated
    }
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

    /// El recurso por API no se intenta si la herramienta ya mató el árbol
    /// ni si el proceso desapareció; solo cubre su fallo con el PID vivo.
    #[cfg(windows)]
    #[test]
    fn fallback_requires_failed_taskkill_and_live_pid() {
        assert!(!should_terminate_after_failed_taskkill(true, true));
        assert!(!should_terminate_after_failed_taskkill(true, false));
        assert!(!should_terminate_after_failed_taskkill(false, false));
        assert!(should_terminate_after_failed_taskkill(false, true));
    }

    /// Un hijo vivo queda terminado por su PID exacto; el handle que conserva
    /// la prueba permite verificar su muerte sin depender de la tabla de procesos.
    #[cfg(windows)]
    #[test]
    fn kill_tree_terminates_live_child() {
        let mut child = std::process::Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("se lanza un hijo vivo");
        let pid = child.id();
        assert!(pid_alive_exact(pid), "el hijo vive al comenzar");
        assert!(kill_tree_by_pid_exact(pid), "se reclama el proceso");
        assert!(!pid_alive_exact(pid), "el hijo ya no vive");
        child.wait().expect("se recolecta al hijo");
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

    /// Un proceso terminado cuyo handle sigue abierto en otro sitio no existe:
    /// es `Gone` y su dueño, `Dead`, no un `Denied` que lo conservaría como dueño
    /// desconocido. El `Child` sin soltar mantiene ese handle, como lo haría un
    /// antivirus o el agente de la CI.
    #[test]
    fn terminated_process_with_open_handle_is_gone() {
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
        let owner = owner_token_of(child.id()).expect("el hijo vivo se puede observar");
        child.kill().expect("se mata al hijo");
        child.wait().expect("se espera al hijo");
        assert_eq!(observe_identity(child.id()), IdentityObservation::Gone);
        assert_eq!(classify_owner(&owner), OwnerState::Dead);
        drop(child);
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

    /// En Linux, la identidad propia es el `boot_id` seguido del `starttime` que
    /// publica `/proc/self/stat`.
    #[cfg(target_os = "linux")]
    #[test]
    fn own_identity_matches_proc_self_stat() {
        let raw = std::fs::read_to_string("/proc/self/stat").expect("se lee /proc/self/stat");
        let stat = parse_proc_stat(&raw).expect("línea propia válida");
        let boot = boot_id().expect("se lee el boot_id");
        let own = process_identity(std::process::id()).expect("identidad propia");
        assert_eq!(own.image, stat.image);
        assert_eq!(own.start, format!("{boot}{}", stat.start));
    }

    /// El `boot_id` pierde guiones, salto de línea y mayúsculas; lo que no es
    /// hexadecimal se rechaza.
    #[test]
    fn boot_id_is_normalized() {
        assert_eq!(
            normalize_boot_id("1A2B3C4D-0000-4abc-8def-0123456789AB\n").as_deref(),
            Some("1a2b3c4d00004abc8def0123456789ab")
        );
        assert_eq!(normalize_boot_id(""), None);
        assert_eq!(normalize_boot_id("\n"), None);
        assert_eq!(normalize_boot_id("zz-top"), None);
    }

    fn token_of(pid: u32, start: &str) -> OwnerToken {
        OwnerToken::new(pid, start).expect("token válido")
    }

    fn identity_with_start(start: &str) -> ProcessIdentity {
        ProcessIdentity {
            start: start.to_string(),
            image: "x".to_string(),
        }
    }

    /// El proceso actual se reconoce como propio con su token.
    #[test]
    fn current_owner_is_ours() {
        let token = current_owner_token().expect("el proceso se observa a sí mismo");
        assert_eq!(token.pid(), std::process::id());
        assert_eq!(classify_owner(&token), OwnerState::Ours);
        assert_eq!(OwnerToken::parse(&token.format()), Some(token));
    }

    /// El mismo PID con otro inicio es otro proceso: está muerto para el token.
    #[test]
    fn same_pid_with_other_start_is_dead() {
        let token = token_of(42, "aaa111");
        let other = classify_owner_with(&token, |pid| {
            assert_eq!(pid, 42);
            IdentityObservation::Present(identity_with_start("bbb222"))
        });
        assert_eq!(other, OwnerState::Dead);
        let same = classify_owner_with(&token, |_| {
            IdentityObservation::Present(identity_with_start("aaa111"))
        });
        assert_eq!(same, OwnerState::Ours);
    }

    /// Un PID inexistente está muerto, en la inyección y en el sistema real.
    #[test]
    fn nonexistent_pid_is_dead() {
        let token = token_of(42, "aaa111");
        assert_eq!(
            classify_owner_with(&token, |_| IdentityObservation::Gone),
            OwnerState::Dead
        );
        assert_eq!(
            classify_owner(&token_of(4_000_000_000, "1")),
            OwnerState::Dead
        );
    }

    /// Una observación denegada no es ni vivo ni muerto: es desconocido.
    #[test]
    fn denied_observation_is_unknown() {
        let token = token_of(42, "aaa111");
        assert_eq!(
            classify_owner_with(&token, |_| IdentityObservation::Denied),
            OwnerState::Unknown
        );
    }
}
