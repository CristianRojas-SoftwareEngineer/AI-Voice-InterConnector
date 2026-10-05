//! Lanzamiento con herencia restringida (allowlist).
//!
//! En Windows cada spawn declara exactamente qué handles hereda el hijo
//! mediante `STARTUPINFOEX` con `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`: solo lo
//! listado se hereda, aunque el resto de la tabla siga marcado como
//! heredable. En Unix se delega en `Command` (`close-on-exec` ya da la misma
//! propiedad).

use std::io;
use std::path::{Path, PathBuf};

/// Entrada estándar del hijo.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StdinSpec {
    /// Sin entrada (`NUL`).
    Null,
    /// Tubería propia cuyo extremo de escritura queda retenido en el hijo
    /// devuelto (vigía por tubería).
    Piped,
}

/// Devuelve exactamente los handles declarados, sin añadir ni quitar ninguno.
///
/// Existe para que la propiedad allowlist sea comprobable sin lanzar
/// procesos: lo que entra es lo que hereda el hijo.
pub fn allowed_handle_list(handles: &[isize]) -> Vec<isize> {
    handles.to_vec()
}

/// Petición de lanzamiento con herencia restringida.
#[derive(Debug)]
pub struct RestrictedSpawnRequest {
    /// Ejecutable del hijo.
    pub program: PathBuf,
    /// Argumentos del hijo (sin el ejecutable).
    pub args: Vec<String>,
    /// Entrada estándar del hijo.
    pub stdin: StdinSpec,
    /// Fichero de registro para stdout y stderr del hijo.
    pub log_file: std::fs::File,
    /// Flags de creación de Windows (`CREATE_NO_WINDOW`, etc.). En Unix se
    /// ignoran.
    pub creation_flags: u32,
    /// Handles extra que el hijo debe heredar además de su stdio
    /// (p. ej. el HANDLE de espera del limpiador). Normalmente vacío.
    pub extra_allowed: Vec<isize>,
}

/// Hijo lanzado con herencia restringida.
pub struct RestrictedChild {
    pid: u32,
    #[cfg(windows)]
    process: isize,
    /// Retiene el extremo de escritura de la tubería de entrada: mientras
    /// vive, el hijo con vigilancia por tubería no ve fin de fichero.
    #[cfg(windows)]
    #[allow(dead_code)]
    stdin_holder: Option<OwnedHandleWrap>,
    #[cfg(not(windows))]
    inner: std::process::Child,
}

#[cfg(windows)]
struct OwnedHandleWrap {
    handle: isize,
}

#[cfg(windows)]
impl Drop for OwnedHandleWrap {
    fn drop(&mut self) {
        // SAFETY: el handle es propio y válido hasta aquí.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

impl RestrictedChild {
    /// PID del hijo.
    pub fn id(&self) -> u32 {
        #[cfg(windows)]
        {
            self.pid
        }
        #[cfg(not(windows))]
        {
            self.inner.id()
        }
    }

    /// Consulta no bloqueante del estado del hijo.
    pub fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
            use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
            // SAFETY: el handle de proceso es propio y válido.
            unsafe {
                let wait = WaitForSingleObject(self.process, 0);
                if wait != WAIT_OBJECT_0 {
                    return Ok(None);
                }
                let mut code = 0u32;
                if GetExitCodeProcess(self.process, &mut code) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(exit_status_from_raw(code)))
            }
        }
        #[cfg(not(windows))]
        {
            self.inner.try_wait()
        }
    }

    /// Espera bloqueante hasta que el hijo termine.
    pub fn wait(&mut self) -> io::Result<std::process::ExitStatus> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::{
                GetExitCodeProcess, WaitForSingleObject, INFINITE,
            };
            // SAFETY: el handle de proceso es propio y válido.
            unsafe {
                WaitForSingleObject(self.process, INFINITE);
                let mut code = 0u32;
                if GetExitCodeProcess(self.process, &mut code) == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(exit_status_from_raw(code))
            }
        }
        #[cfg(not(windows))]
        {
            self.inner.wait()
        }
    }

    /// Termina el hijo directo (sin árbol; el árbol lo cierra el llamante por
    /// PID cuando lo necesita).
    pub fn kill(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Threading::TerminateProcess;
            // SAFETY: el handle de proceso es propio y válido.
            unsafe {
                if TerminateProcess(self.process, 1) == 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            Ok(())
        }
        #[cfg(not(windows))]
        {
            self.inner.kill()
        }
    }
}

#[cfg(windows)]
impl Drop for RestrictedChild {
    fn drop(&mut self) {
        // SAFETY: el handle de proceso es propio y válido hasta aquí.
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.process);
        }
    }
}

#[cfg(windows)]
fn exit_status_from_raw(code: u32) -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(code)
}

/// Lanza el hijo heredando solo los handles declarados.
///
/// En Windows el stdio del hijo (nulo o tubería + fichero de registro) más
/// `extra_allowed` forman la allowlist; cualquier otro handle heredable del
/// padre no llega al hijo. Sin repliegue: si la lista no se puede aplicar, el
/// lanzamiento falla con error.
pub fn spawn_with_allowlist(request: RestrictedSpawnRequest) -> io::Result<RestrictedChild> {
    #[cfg(windows)]
    {
        spawn_with_allowlist_windows(request)
    }
    #[cfg(not(windows))]
    {
        spawn_with_allowlist_unix(request)
    }
}

#[cfg(not(windows))]
fn spawn_with_allowlist_unix(request: RestrictedSpawnRequest) -> io::Result<RestrictedChild> {
    use std::process::Stdio;
    let mut cmd = std::process::Command::new(&request.program);
    cmd.args(&request.args);
    match request.stdin {
        StdinSpec::Null => {
            cmd.stdin(Stdio::null());
        }
        StdinSpec::Piped => {
            cmd.stdin(Stdio::piped());
        }
    }
    let stdout = request.log_file.try_clone()?;
    cmd.stdout(Stdio::from(stdout));
    cmd.stderr(Stdio::from(request.log_file));
    let inner = cmd.spawn()?;
    Ok(RestrictedChild {
        pid: inner.id(),
        inner,
    })
}

#[cfg(windows)]
fn spawn_with_allowlist_windows(request: RestrictedSpawnRequest) -> io::Result<RestrictedChild> {
    use windows_sys::Win32::Foundation::{CloseHandle, SetHandleInformation, HANDLE_FLAG_INHERIT};

    // Prepara el stdio del hijo y la allowlist.
    let mut marked: Vec<isize> = Vec::new();
    let mut stdin_holder: Option<OwnedHandleWrap> = None;

    // SAFETY: todas las llamadas al sistema usan handles propios y búferes
    // locales; cada handle marcado se restaura tras el lanzamiento.
    unsafe {
        let (stdin_handle, stdin_is_null_holder): (isize, Option<()>) = match request.stdin {
            StdinSpec::Null => (open_null_handle()?, None),
            StdinSpec::Piped => {
                let (read, write) = create_stdin_pipe()?;
                stdin_holder = Some(OwnedHandleWrap { handle: write });
                (read, None)
            }
        };
        let _ = stdin_is_null_holder;
        // El handle de entrada del padre se cierra tras duplicarlo al hijo
        // (nulo de un solo uso o extremo de lectura de la tubería: el hijo
        // tiene su propia copia y el extremo de escritura vive en el holder).
        let close_after_spawn: Vec<isize> = vec![stdin_handle];
        mark_inheritable(stdin_handle);
        marked.push(stdin_handle);

        let stdout_handle = raw_handle_of(&request.log_file);
        mark_inheritable(stdout_handle);
        marked.push(stdout_handle);
        // stdout y stderr comparten el mismo fichero de registro.
        let stderr_handle = stdout_handle;

        for extra in &request.extra_allowed {
            mark_inheritable(*extra);
            marked.push(*extra);
        }

        let allowed = allowed_handle_list(&marked);
        let result = launch_process_with_list(
            &request.program,
            &request.args,
            stdin_handle,
            stdout_handle,
            stderr_handle,
            request.creation_flags,
            &allowed,
            request.extra_allowed.first().copied(),
        );

        // Restaura las marcas para no contaminar futuros lanzamientos con
        // `Command` clásico (p. ej. `tasklist` breves).
        for handle in &marked {
            SetHandleInformation(*handle, HANDLE_FLAG_INHERIT, 0);
        }
        for handle in close_after_spawn {
            CloseHandle(handle);
        }

        let (pid, process) = match result {
            Ok(pair) => pair,
            Err(code) => {
                return Err(io::Error::from_raw_os_error(code as i32));
            }
        };
        Ok(RestrictedChild {
            pid,
            process,
            stdin_holder,
        })
    }
}

#[cfg(windows)]
fn raw_handle_of(file: &std::fs::File) -> isize {
    use std::os::windows::io::AsRawHandle;
    file.as_raw_handle() as isize
}

#[cfg(windows)]
fn mark_inheritable(handle: isize) {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    // SAFETY: el handle es propio y válido.
    unsafe {
        SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
    }
}

#[cfg(windows)]
fn open_null_handle() -> io::Result<isize> {
    use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE};
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    let wide: Vec<u16> = "NUL\0".encode_utf16().collect();
    // SAFETY: la cadena está terminada en nulo y los parámetros son los del
    // dispositivo nulo.
    unsafe {
        let handle = CreateFileW(
            wide.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            std::ptr::null(),
            OPEN_EXISTING,
            0,
            0,
        );
        if handle == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(handle)
    }
}

#[cfg(windows)]
fn create_stdin_pipe() -> io::Result<(isize, isize)> {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;
    // SAFETY: los atributos piden handles heredables; tras crearla, el
    // extremo de escritura se desmarca para que solo el de lectura viaje al
    // hijo mediante la allowlist.
    unsafe {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 1,
        };
        let mut read = 0isize;
        let mut write = 0isize;
        if CreatePipe(&mut read, &mut write, &attributes, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        SetHandleInformation(write, HANDLE_FLAG_INHERIT, 0);
        Ok((read, write))
    }
}

#[cfg(windows)]
#[allow(clippy::too_many_arguments)]
fn launch_process_with_list(
    program: &Path,
    args: &[String],
    stdin_handle: isize,
    stdout_handle: isize,
    stderr_handle: isize,
    creation_flags: u32,
    allowed: &[isize],
    _extra_first: Option<isize>,
) -> Result<(u32, isize), u32> {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError};
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
        UpdateProcThreadAttribute, EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST, STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES,
        STARTUPINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    // SAFETY: la línea de comandos y la lista de atributos son búferes
    // locales válidos durante la llamada; los handles listados son propios.
    unsafe {
        let cmdline = build_command_line(program, args);
        let mut cmdline: Vec<u16> = cmdline.encode_utf16().chain([0]).collect();

        let mut size = 0usize;
        InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        let mut buffer = vec![0u8; size];
        let list = buffer.as_mut_ptr() as *mut std::ffi::c_void;
        if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
            return Err(GetLastError());
        }
        let updated = UpdateProcThreadAttribute(
            list,
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            allowed.as_ptr() as *mut std::ffi::c_void,
            std::mem::size_of_val(allowed),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if updated == 0 {
            let code = GetLastError();
            DeleteProcThreadAttributeList(list);
            return Err(code);
        }

        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
        startup.StartupInfo.wShowWindow = SW_HIDE as u16;
        startup.StartupInfo.hStdInput = stdin_handle;
        startup.StartupInfo.hStdOutput = stdout_handle;
        startup.StartupInfo.hStdError = stderr_handle;
        startup.lpAttributeList = list;

        let mut info: PROCESS_INFORMATION = std::mem::zeroed();
        let launched = CreateProcessW(
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            creation_flags | EXTENDED_STARTUPINFO_PRESENT,
            std::ptr::null(),
            std::ptr::null(),
            &startup.StartupInfo,
            &mut info,
        );
        let code = GetLastError();
        DeleteProcThreadAttributeList(list);
        if launched == 0 {
            return Err(code);
        }
        if info.hThread != 0 {
            CloseHandle(info.hThread);
        }
        Ok((info.dwProcessId, info.hProcess))
    }
}

#[cfg(windows)]
fn build_command_line(program: &Path, args: &[String]) -> String {
    let mut parts = vec![quote_arg(&program.to_string_lossy())];
    for arg in args {
        parts.push(quote_arg(arg));
    }
    parts.join(" ")
}

#[cfg(windows)]
fn quote_arg(arg: &str) -> String {
    let mut out = String::from("\"");
    let mut backslashes = 0usize;
    for c in arg.chars() {
        if c == '\\' {
            backslashes += 1;
            continue;
        }
        if c == '"' {
            out.push_str(&"\\".repeat(backslashes * 2 + 1));
        } else {
            out.push_str(&"\\".repeat(backslashes));
        }
        out.push(c);
        backslashes = 0;
    }
    out.push_str(&"\\".repeat(backslashes * 2));
    out.push('"');
    out
}
