use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::{CREATE_NEW_PROCESS_GROUP, CREATE_NO_WINDOW, DETACHED_PROCESS};

/// Limpiador propio de borrado diferido en Windows: borra una
/// ruta (staging, `.old-*`, directorio de programa) sin PowerShell ni scripts
/// generados, con un solo mecanismo para las tres rutas (staging, aparcado y
/// `self uninstall`):
///
/// 1. Copia el ejecutable propio a `%TEMP%` con nombre único: la copia
///    sobrevive al borrado del directorio de programa.
/// 2. Abre el HANDLE al proceso esperado **al programar**, sin carrera de PID,
///    y lo hereda a la copia.
/// 3. Relanza la copia desacoplada y oculta con argumentos internos: espera la
///    muerte por el sistema, borra `path` con reintentos acotados con retroceso
///    y escribe su resultado en el registro `log_path`, que lee la
///    verificación del ciclo. Lo que siga bloqueado tras los reintentos queda
///    para el barrido de la siguiente operación y su red (`doctor --repair`).
///
/// Devuelve la ruta de la copia limpiadora. Nunca genera `.ps1`.
pub fn schedule_clean_removal(
    path: &Path,
    wait_pid: u32,
    log_path: &Path,
) -> anyhow::Result<PathBuf> {
    use windows_sys::Win32::Foundation::{
        CloseHandle, GetLastError, SetHandleInformation, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
        STARTUPINFOW,
    };

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let copy = std::env::temp_dir().join(format!("avi-cleaner-{}-{stamp}.exe", std::process::id()));
    // La copia a medio escribir también es un resto: si falla, se retira.
    if let Err(e) = std::fs::copy(std::env::current_exe()?, &copy) {
        let _ = std::fs::remove_file(&copy);
        return Err(e.into());
    }

    // SAFETY: las llamadas al sistema usan búferes locales del tamaño
    // declarado; cada handle abierto se cierra en todos los caminos.
    unsafe {
        // El HANDLE se abre al programar: fija la identidad del proceso
        // esperado aunque su PID se reutilice después. Un PID ya muerto (o el
        // 0) no se espera: se pasa el nulo como centinela.
        let wait_handle = if wait_pid == 0 {
            0
        } else {
            OpenProcess(SYNCHRONIZE, 0, wait_pid)
        };
        if wait_handle != 0 {
            SetHandleInformation(wait_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
        }
        let cmdline = format!(
            "{} {CLEANER_WORKER_ARG} {} {} {} {}",
            quote_arg(&copy.to_string_lossy()),
            quote_arg(&path.to_string_lossy()),
            wait_pid,
            quote_arg(&log_path.to_string_lossy()),
            wait_handle,
        );
        let mut cmdline: Vec<u16> = cmdline.encode_utf16().chain([0]).collect();
        let mut startup: STARTUPINFOW = std::mem::zeroed();
        startup.cb = std::mem::size_of::<STARTUPINFOW>() as u32;
        // STARTF_USESHOWWINDOW con `wShowWindow` 0 (`SW_HIDE`): sin ventana.
        startup.dwFlags = 1;
        let base_flags = CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS;
        // Intento de separación del Job Object de la terminal: si el
        // padre vive en un Job que lo permite, el limpiador sobrevive a la
        // muerte del padre; si el Job no lo permite, se lanza sin separar y lo
        // no borrado cae al barrido de la siguiente operación.
        let mut child = launch_cleaner(
            cmdline.as_mut_ptr(),
            &startup,
            base_flags | CREATE_BREAKAWAY_FROM_JOB,
        )
        .or_else(|| launch_cleaner(cmdline.as_mut_ptr(), &startup, base_flags));
        if wait_handle != 0 {
            CloseHandle(wait_handle);
        }
        let Some((process, thread)) = child.take() else {
            let code = GetLastError();
            let _ = std::fs::remove_file(&copy);
            anyhow::bail!("no se pudo lanzar el limpiador (código {code})");
        };
        if thread != 0 {
            CloseHandle(thread);
        }
        // Confirmación de arranque: si el limpiador murió al instante con
        // fallo, programar no sirvió. Si terminó bien (ruta ya ausente) o
        // sigue en marcha, la programación vale.
        let wait = WaitForSingleObject(process, CLEANER_START_GRACE_MS);
        if wait == WAIT_OBJECT_0 {
            let mut code = 1u32;
            GetExitCodeProcess(process, &mut code);
            CloseHandle(process);
            if code != 0 {
                let _ = std::fs::remove_file(&copy);
                anyhow::bail!("el limpiador terminó con fallo al arrancar");
            }
        } else {
            CloseHandle(process);
        }
    }
    Ok(copy)
}

/// Lanza la copia limpiadora con esas flags de creación. Devuelve los handles
/// de proceso e hilo; el hilo lo cierra el llamador.
///
/// SAFETY: la línea de comandos es un búfer local válido y terminado en nulo;
/// el arranque no hereda más que los handles marcados (el de espera).
unsafe fn launch_cleaner(
    cmdline: *mut u16,
    startup: &windows_sys::Win32::System::Threading::STARTUPINFOW,
    flags: u32,
) -> Option<(isize, isize)> {
    use windows_sys::Win32::System::Threading::{CreateProcessW, PROCESS_INFORMATION};

    let mut info: PROCESS_INFORMATION = std::mem::zeroed();
    let launched = CreateProcessW(
        std::ptr::null(),
        cmdline,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        1,
        flags,
        std::ptr::null(),
        std::ptr::null(),
        startup,
        &mut info,
    );
    if launched == 0 {
        return None;
    }
    Some((info.hProcess, info.hThread))
}

/// Argumento interno del limpiador. Oculto: no figura en ninguna ayuda ni en
/// ningún contrato; solo lo entiende el despacho temprano de este módulo.
const CLEANER_WORKER_ARG: &str = "--avi-cleaner-worker";

/// Cortesía al limpiador recién lanzado antes de darlo por programado.
const CLEANER_START_GRACE_MS: u32 = 300;

/// Intentos de borrado del limpiador, con espera creciente entre ellos. Acotados:
/// lo que siga bloqueado tras el último intento queda para el barrido de la
/// siguiente operación.
const CLEANER_ATTEMPTS: u32 = 10;

/// Espera base entre intentos del limpiador; el intento `i` espera `500*i` ms
/// con tope, para no superar el plazo de la verificación del ciclo.
const CLEANER_RETRY_MS: u64 = 500;

/// Tope de la espera entre intentos del limpiador.
const CLEANER_RETRY_MAX_MS: u64 = 1500;

/// Despacho temprano del limpiador.
///
/// La copia limpiadora debe trabajar sin depender del binario que la lanzó
/// (el programa, `xtask` o un arnés de pruebas): este gancho corre antes de
/// `main` y, con los argumentos internos presentes, ejecuta al trabajador y
/// termina el proceso. Sin esos argumentos no hace nada.
#[ctor::ctor]
unsafe fn dispatch_cleaner_worker() {
    let mut args = std::env::args_os();
    if args.next().is_none() {
        return;
    }
    let is_worker = args
        .next()
        .and_then(|a| a.into_string().ok())
        .is_some_and(|a| a == CLEANER_WORKER_ARG);
    if !is_worker {
        return;
    }
    let rest: Vec<String> = args.map(|a| a.into_string().unwrap_or_default()).collect();
    let code = cleaner_worker_main(&rest);
    std::process::exit(code);
}

/// Trabajador del limpiador: espera, borra con reintentos e informa en el
/// registro. Nunca entra en pánico: cualquier fallo termina en el registro y
/// en el código de salida, porque nadie lee su terminal.
fn cleaner_worker_main(args: &[String]) -> i32 {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};

    let path = args.first().map(String::as_str).unwrap_or_default();
    let wait_pid: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let log = args.get(2).map(String::as_str).unwrap_or_default();
    let wait_handle: isize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);

    if !log.is_empty() {
        if let Some(parent) = Path::new(log).parent() {
            if !parent.as_os_str().is_empty() {
                let _ = std::fs::create_dir_all(parent);
            }
        }
    }
    // El proceso esperado se aguarda por el sistema, sin sondeo ni PID.
    let mut waited = false;
    if wait_handle != 0 {
        // SAFETY: el handle lo abrió quien programó y lo heredó este proceso
        // con derecho de espera; se cierra tras aguardar.
        unsafe {
            WaitForSingleObject(wait_handle, INFINITE);
            CloseHandle(wait_handle);
        }
        waited = true;
    }

    let target = Path::new(path);
    let mut attempts = 0u32;
    let mut last_error = String::from("la ruta ya no existe");
    while attempts < CLEANER_ATTEMPTS {
        attempts += 1;
        let outcome = if target.is_dir() {
            std::fs::remove_dir_all(target)
        } else {
            std::fs::remove_file(target)
        };
        match outcome {
            Ok(()) => {}
            Err(e) => {
                last_error = e.to_string();
            }
        }
        if !target.exists() {
            break;
        }
        if attempts < CLEANER_ATTEMPTS {
            let wait_ms = (CLEANER_RETRY_MS * attempts as u64).min(CLEANER_RETRY_MAX_MS);
            std::thread::sleep(Duration::from_millis(wait_ms));
        }
    }

    let waited_note = if waited {
        format!(" esperando al proceso {wait_pid}")
    } else {
        String::new()
    };
    let (report, code) = if target.exists() {
        (
            format!(
                "fallo: no se pudo retirar {path} tras {attempts} intentos{waited_note} ({last_error}); queda para el barrido"
            ),
            1,
        )
    } else {
        (
            format!("borrado {path} tras {attempts} intento(s){waited_note}"),
            0,
        )
    };
    if !log.is_empty() {
        let _ = std::fs::write(log, report);
    }
    // La copia en uso no se puede borrar a sí misma en Windows: el barrido de
    // temporales recoge las copias rancias por su prefijo.
    let _ = std::fs::remove_file(std::env::current_exe().unwrap_or_default());
    code
}

/// Cita un argumento para la línea de comandos de Windows según las reglas de
/// `CommandLineToArgvW`: entre comillas, con las barras invertidas duplicadas
/// ante cada comilla y al final.
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
