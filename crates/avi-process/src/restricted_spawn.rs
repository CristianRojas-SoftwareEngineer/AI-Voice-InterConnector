//! Lanzamiento con herencia restringida (allowlist).
//!
//! En Windows cada spawn declara exactamente qué handles hereda el hijo
//! mediante `STARTUPINFOEX` con `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`: solo lo
//! listado se hereda, aunque el resto de la tabla siga marcado como
//! heredable. En Unix se delega en `Command` (`close-on-exec` ya da la misma
//! propiedad) y el hijo nace líder de su propia sesión y grupo con `setsid`
//! en `pre_exec`.

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
    /// El extremo de entrada del propio padre: el hijo hereda la misma consola
    /// o tubería del invocador (traspaso de `self update`).
    Inherit,
}

/// Devuelve exactamente los handles declarados, sin añadir ni quitar ninguno:
/// lo que entra es lo que hereda el hijo.
#[cfg(windows)]
pub(crate) fn allowed_handle_list(handles: &[isize]) -> Vec<isize> {
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
    /// Fichero al que apunta el stdout del hijo.
    pub stdout_file: std::fs::File,
    /// Fichero al que apunta el stderr del hijo.
    pub stderr_file: std::fs::File,
    /// Flags de creación de Windows (`CREATE_NO_WINDOW`, etc.). En Unix se
    /// ignoran.
    pub creation_flags: u32,
    /// Handles extra que el hijo debe heredar además de su stdio
    /// (p. ej. el extremo de lectura de una tubería creada por el padre que el
    /// hijo debe leer). Normalmente vacío.
    pub extra_allowed: Vec<isize>,
}

/// Hijo lanzado con herencia restringida.
pub struct RestrictedChild {
    #[cfg(windows)]
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
pub(crate) struct OwnedHandleWrap {
    pub(crate) handle: isize,
}

#[cfg(windows)]
impl OwnedHandleWrap {
    pub(crate) fn new(handle: isize) -> Self {
        Self { handle }
    }
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

/// Restaura la marca de heredabilidad de un handle al salir de ámbito:
/// desmarca `HANDLE_FLAG_INHERIT`, que el lanzamiento con allowlist activó
/// temporalmente para prepararlo.
#[cfg(windows)]
pub(crate) struct InheritFlagGuard {
    handle: isize,
}

#[cfg(windows)]
impl InheritFlagGuard {
    pub(crate) fn new(handle: isize) -> Self {
        Self { handle }
    }
}

#[cfg(windows)]
impl Drop for InheritFlagGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        // SAFETY: el handle sigue siendo válido hasta este punto del flujo.
        unsafe {
            SetHandleInformation(self.handle, HANDLE_FLAG_INHERIT, 0);
        }
    }
}

/// Dueño de la attribute list: mantiene vivo el búfer y la libera con
/// `DeleteProcThreadAttributeList` en todos los caminos de salida.
#[cfg(windows)]
pub(crate) struct AttributeListGuard {
    // Lo retiene para que la attribute list no quede colgando.
    #[allow(dead_code)]
    buffer: Vec<u8>,
    list: *mut std::ffi::c_void,
}

#[cfg(windows)]
impl AttributeListGuard {
    pub(crate) fn new(buffer: Vec<u8>, list: *mut std::ffi::c_void) -> Self {
        Self { buffer, list }
    }

    pub(crate) fn as_ptr(&self) -> *mut std::ffi::c_void {
        self.list
    }
}

#[cfg(windows)]
impl Drop for AttributeListGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Threading::DeleteProcThreadAttributeList;
        // SAFETY: la lista fue inicializada y el búfer sigue vivo.
        unsafe {
            DeleteProcThreadAttributeList(self.list);
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

    /// Espera acotada hasta que el hijo termine: sondea con `try_wait` y, al
    /// vencer el plazo, mata al hijo y lo recolecta con sondeo acotado antes
    /// de informar. Devuelve `Some(estado)` si terminó a tiempo y `None` si
    /// venció (el hijo queda muerto y recolectado, sin huérfanos ni handles
    /// retenidos). Nunca espera sin cota: ni siquiera la recolecta tras la
    /// muerte usa `INFINITE`.
    pub fn wait_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> io::Result<Option<std::process::ExitStatus>> {
        let start = std::time::Instant::now();
        loop {
            if let Some(status) = self.try_wait()? {
                return Ok(Some(status));
            }
            if start.elapsed() >= timeout {
                let _ = self.kill();
                // Recolecta tras la muerte con su propio plazo acotado: el
                // hijo ya está muerto, solo falta cosechar su estado para no
                // dejar zombis en Unix ni handles abiertos en Windows.
                let reap_deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    match self.try_wait()? {
                        Some(_) => break,
                        None if std::time::Instant::now() >= reap_deadline => break,
                        None => std::thread::sleep(std::time::Duration::from_millis(5)),
                    }
                }
                return Ok(None);
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
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
/// Lanza al hijo con `setsid` (vía `libc` en `pre_exec`): nace líder de su
/// propia sesión y grupo, de modo que el kill de grupo (`kill(-pid)`) alcanza
/// solo a su árbol y nunca al grupo del lanzador. El resto del cableado
/// (stdio y registro) no cambia.
fn spawn_with_allowlist_unix(request: RestrictedSpawnRequest) -> io::Result<RestrictedChild> {
    use std::os::unix::process::CommandExt;
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
        StdinSpec::Inherit => {
            cmd.stdin(Stdio::inherit());
        }
    }
    cmd.stdout(Stdio::from(request.stdout_file));
    cmd.stderr(Stdio::from(request.stderr_file));
    // SAFETY: `pre_exec` corre en el hijo recién bifurcado, antes del `exec`;
    // `setsid` es seguro en ese punto (llamada de sistema sin memoria
    // compartida ni asignaciones). Un fallo de `setsid` aborta el lanzamiento
    // en vez de degradar en silencio.
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let inner = cmd.spawn()?;
    Ok(RestrictedChild { inner })
}

#[cfg(windows)]
fn spawn_with_allowlist_windows(request: RestrictedSpawnRequest) -> io::Result<RestrictedChild> {
    // Prepara el stdio del hijo y la allowlist. Los guards de restauración
    // se declaran DESPUÉS que los de cierre para que, al soltarse en orden
    // inverso, los flags se restauren ANTES de cerrar los handles.
    let mut close_guards: Vec<OwnedHandleWrap> = Vec::new();
    let mut inherit_guards: Vec<InheritFlagGuard> = Vec::new();
    let mut stdin_holder: Option<OwnedHandleWrap> = None;
    let mut marked: Vec<isize> = Vec::new();

    let stdin_handle: isize = match request.stdin {
        StdinSpec::Null => open_null_handle()?,
        StdinSpec::Piped => {
            let (read, write) = create_stdin_pipe()?;
            stdin_holder = Some(OwnedHandleWrap::new(write));
            read
        }
        StdinSpec::Inherit => duplicate_parent_stdin()?,
    };
    // El handle de entrada del padre se cierra tras duplicarlo al hijo
    // (nulo de un solo uso o extremo de lectura de la tubería: el hijo
    // tiene su propia copia y el extremo de escritura vive en el holder).
    close_guards.push(OwnedHandleWrap::new(stdin_handle));
    mark_inheritable(stdin_handle)?;
    marked.push(stdin_handle);
    inherit_guards.push(InheritFlagGuard::new(stdin_handle));

    let stdout_handle = raw_handle_of(&request.stdout_file);
    mark_inheritable(stdout_handle)?;
    marked.push(stdout_handle);
    inherit_guards.push(InheritFlagGuard::new(stdout_handle));
    let stderr_handle = raw_handle_of(&request.stderr_file);
    mark_inheritable(stderr_handle)?;
    marked.push(stderr_handle);
    inherit_guards.push(InheritFlagGuard::new(stderr_handle));

    for extra in &request.extra_allowed {
        mark_inheritable(*extra)?;
        marked.push(*extra);
        inherit_guards.push(InheritFlagGuard::new(*extra));
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
    );

    // Los guards restauran los flags y cierran los handles al salir de
    // este ámbito, en todos los caminos (incluido el error de spawn, que
    // deja los handles del padre en su estado original).
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

#[cfg(windows)]
fn raw_handle_of(file: &std::fs::File) -> isize {
    use std::os::windows::io::AsRawHandle;
    file.as_raw_handle() as isize
}

/// Duplica el extremo de entrada del proceso padre como handle propio y
/// heredable, para que el hijo lo reciba por la allowlist como su entrada.
#[cfg(windows)]
fn duplicate_parent_stdin() -> io::Result<isize> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::{DuplicateHandle, DUPLICATE_SAME_ACCESS};
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let source = std::io::stdin().as_raw_handle() as isize;
    let mut target = 0isize;
    // SAFETY: duplica el handle del padre; el objeto es propio y se marca
    // heredable para listarlo en la allowlist.
    unsafe {
        if DuplicateHandle(
            GetCurrentProcess(),
            source,
            GetCurrentProcess(),
            &mut target,
            0,
            1,
            DUPLICATE_SAME_ACCESS,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(target)
}

/// Marca el handle como heredable; si el sistema lo rechaza, el lanzamiento
/// falla en vez de entregar al hijo un handle que no puede usar.
#[cfg(windows)]
pub(crate) fn mark_inheritable(handle: isize) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
    // SAFETY: el handle es propio y válido.
    unsafe {
        if SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT) == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
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
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;
    // SAFETY: los atributos piden handles no heredables, de modo que ningún
    // extremo es heredable ni un instante tras crearse (otro hilo que lance un
    // proceso con herencia no puede capturar el de escritura). La herencia del
    // extremo de lectura la concede solo la lista de handles del lanzamiento.
    unsafe {
        let attributes = SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: std::ptr::null_mut(),
            bInheritHandle: 0,
        };
        let mut read = 0isize;
        let mut write = 0isize;
        if CreatePipe(&mut read, &mut write, &attributes, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok((read, write))
    }
}

/// Restaura el modo de error del hilo al salir de ámbito: el lanzamiento
/// suprime los diálogos del sistema ante imágenes inválidas y debe devolver
/// el modo intacto en éxito y en fallo.
#[cfg(windows)]
struct ErrorModeGuard {
    previous: u32,
}

#[cfg(windows)]
impl ErrorModeGuard {
    /// Suprime los diálogos de error del sistema en este hilo (fallos
    /// críticos, GPF y cajas de archivo) y retiene el modo previo para
    /// restaurarlo al soltarse.
    /// El modo es el del hilo y no el del proceso: con el del proceso, dos
    /// lanzamientos concurrentes se cruzan al guardar y restaurar, y el modo
    /// suprimido queda fijado para todo el proceso.
    fn suppress_system_ui() -> Self {
        use windows_sys::Win32::System::Diagnostics::Debug::{
            GetThreadErrorMode, SetThreadErrorMode, SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX,
            SEM_NOOPENFILEERRORBOX,
        };
        // SAFETY: ambas consultan y fijan el modo de error del propio hilo;
        // sin puntero de salida, porque el modo previo ya se leyó.
        let previous = unsafe { GetThreadErrorMode() };
        unsafe {
            SetThreadErrorMode(
                previous | SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX | SEM_NOOPENFILEERRORBOX,
                std::ptr::null_mut(),
            )
        };
        Self { previous }
    }
}

#[cfg(windows)]
impl Drop for ErrorModeGuard {
    fn drop(&mut self) {
        use windows_sys::Win32::System::Diagnostics::Debug::SetThreadErrorMode;
        // SAFETY: restaura el modo retenido del propio hilo.
        unsafe {
            SetThreadErrorMode(self.previous, std::ptr::null_mut());
        }
    }
}

#[cfg(windows)]
fn launch_process_with_list(
    program: &Path,
    args: &[String],
    stdin_handle: isize,
    stdout_handle: isize,
    stderr_handle: isize,
    creation_flags: u32,
    allowed: &[isize],
) -> Result<(u32, isize), u32> {
    use windows_sys::Win32::Foundation::GetLastError;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        STARTF_USESHOWWINDOW, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_HIDE;

    // Supresor de UI del sistema: ante imágenes inválidas, el fallo vuelve
    // como código en vez de ventana. El guard restaura el modo del hilo en
    // todos los caminos de salida.
    let _error_mode = ErrorModeGuard::suppress_system_ui();
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
        // Mover el `Vec` no realoca: el puntero sigue válido y queda dentro
        // del guard, que además la libera en todos los caminos.
        let list_guard = AttributeListGuard::new(buffer, list);
        let updated = UpdateProcThreadAttribute(
            list_guard.as_ptr(),
            0,
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
            allowed.as_ptr() as *mut std::ffi::c_void,
            std::mem::size_of_val(allowed),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
        if updated == 0 {
            return Err(GetLastError());
        }

        let mut startup: STARTUPINFOEXW = std::mem::zeroed();
        startup.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
        startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES | STARTF_USESHOWWINDOW;
        startup.StartupInfo.wShowWindow = SW_HIDE as u16;
        startup.StartupInfo.hStdInput = stdin_handle;
        startup.StartupInfo.hStdOutput = stdout_handle;
        startup.StartupInfo.hStdError = stderr_handle;
        startup.lpAttributeList = list_guard.as_ptr();

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
        if launched == 0 {
            return Err(code);
        }
        // El guard cierra el hilo principal del hijo en todos los caminos.
        let _thread_guard = if info.hThread != 0 {
            Some(OwnedHandleWrap::new(info.hThread))
        } else {
            None
        };
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

/// Cita un argumento para la línea de comandos de Windows según las reglas de
/// `CommandLineToArgvW`: entre comillas, con las barras invertidas duplicadas
/// ante cada comilla y al final.
///
/// Definición única del crate: la comparten `build_command_line` (lanzamiento
/// con allowlist) y el limpiador de borrado diferido.
#[cfg(windows)]
pub(crate) fn quote_arg(arg: &str) -> String {
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

#[cfg(all(test, windows))]
mod tests {
    use super::{create_stdin_pipe, quote_arg};
    use windows_sys::Win32::Foundation::{CloseHandle, GetHandleInformation, HANDLE_FLAG_INHERIT};

    /// Ningún extremo de la tubería de vida nace heredable: así el de
    /// escritura no puede filtrarse a un proceso lanzado por otro hilo, y el
    /// de lectura solo viaja al hijo por la lista de handles del lanzamiento.
    #[test]
    fn stdin_pipe_ends_are_not_inheritable() {
        let (read, write) = create_stdin_pipe().expect("debe crear la tubería");
        let mut read_flags = 0u32;
        let mut write_flags = 0u32;
        // SAFETY: ambos handles son propios y válidos hasta el cierre.
        unsafe {
            assert_ne!(GetHandleInformation(read, &mut read_flags), 0);
            assert_ne!(GetHandleInformation(write, &mut write_flags), 0);
            CloseHandle(read);
            CloseHandle(write);
        }
        assert_eq!(
            write_flags & HANDLE_FLAG_INHERIT,
            0,
            "el extremo de escritura no debe ser heredable"
        );
        assert_eq!(
            read_flags & HANDLE_FLAG_INHERIT,
            0,
            "el extremo de lectura no debe ser heredable fuera del lanzamiento"
        );
    }

    /// Todo argumento se cita siempre entre comillas, incluso sin espacios:
    /// una ruta sin cita se reinterpretaría al partir la línea de comandos.
    #[test]
    fn quote_arg_always_quotes() {
        assert_eq!(quote_arg("avi.exe"), "\"avi.exe\"");
        assert_eq!(quote_arg("con espacio"), "\"con espacio\"");
        assert_eq!(quote_arg(""), "\"\"");
    }

    /// Las comillas internas se escapan con una barra invertida y las barras
    /// invertidas finales se duplican, como exige `CommandLineToArgvW`: sin ese
    /// duplicado, una ruta terminada en `\` cerraría la cita antes de tiempo.
    #[test]
    fn quote_arg_escapes_quotes_and_trailing_backslashes() {
        assert_eq!(quote_arg("con\"comilla"), "\"con\\\"comilla\"");
        assert_eq!(quote_arg("C:\\ruta\\"), "\"C:\\ruta\\\\\"");
        assert_eq!(quote_arg("C:\\ruta\\\""), "\"C:\\ruta\\\\\\\"\"");
    }
}
