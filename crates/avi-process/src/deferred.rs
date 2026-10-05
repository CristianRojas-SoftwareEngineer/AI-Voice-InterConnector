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
        GetLastError, SetHandleInformation, HANDLE_FLAG_INHERIT, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
        STARTUPINFOW,
    };

    use crate::restricted_spawn::{quote_arg, InheritFlagGuard, OwnedHandleWrap};

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
        // 0) no se espera: se pasa el nulo como centinela. El guard lo cierra
        // en todos los caminos, incluidos los de error.
        let wait_handle = if wait_pid == 0 {
            0
        } else {
            OpenProcess(SYNCHRONIZE, 0, wait_pid)
        };
        let _wait_guard = if wait_handle != 0 {
            Some(OwnedHandleWrap::new(wait_handle))
        } else {
            None
        };
        // El guard restaura la marca de heredabilidad al salir de este
        // ámbito, también si el lanzamiento del limpiador falla.
        let _inherit_guard = if wait_handle != 0 {
            SetHandleInformation(wait_handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            Some(InheritFlagGuard::new(wait_handle))
        } else {
            None
        };
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
        // Herencia restringida a la lista explícita: solo el HANDLE de espera
        // viaja a la copia (vacía si no hay proceso que aguardar); el stdio
        // del padre no llega al limpiador por construcción.
        let allowed: Vec<isize> = if wait_handle != 0 {
            vec![wait_handle]
        } else {
            Vec::new()
        };
        let mut child = launch_cleaner(
            cmdline.as_mut_ptr(),
            &startup,
            base_flags | CREATE_BREAKAWAY_FROM_JOB,
            &allowed,
        )
        .or_else(|| launch_cleaner(cmdline.as_mut_ptr(), &startup, base_flags, &allowed));
        let Some((process, thread)) = child.take() else {
            let code = GetLastError();
            let _ = std::fs::remove_file(&copy);
            anyhow::bail!("no se pudo lanzar el limpiador (código {code})");
        };
        // Los guards cierran el hilo y el proceso en todos los caminos.
        let _thread_guard = if thread != 0 {
            Some(OwnedHandleWrap::new(thread))
        } else {
            None
        };
        let _process_guard = OwnedHandleWrap::new(process);
        // Confirmación de arranque: si el limpiador murió al instante con
        // fallo, programar no sirvió. Si terminó bien (ruta ya ausente) o
        // sigue en marcha, la programación vale.
        let wait = WaitForSingleObject(process, CLEANER_START_GRACE_MS);
        if wait == WAIT_OBJECT_0 {
            let mut code = 1u32;
            GetExitCodeProcess(process, &mut code);
            if code != 0 {
                let _ = std::fs::remove_file(&copy);
                anyhow::bail!("el limpiador terminó con fallo al arrancar");
            }
        }
    }
    Ok(copy)
}

/// Lanza la copia limpiadora con esas flags de creación y herencia
/// restringida a `allowed` (normalmente solo el HANDLE de espera; vacío si no
/// hay proceso que aguardar). Devuelve los handles de proceso e hilo; el hilo
/// lo cierra el llamador.
///
/// SAFETY: la línea de comandos es un búfer local válido y terminado en nulo;
/// el arranque solo hereda los handles listados.
unsafe fn launch_cleaner(
    cmdline: *mut u16,
    startup: &windows_sys::Win32::System::Threading::STARTUPINFOW,
    flags: u32,
    allowed: &[isize],
) -> Option<(isize, isize)> {
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, InitializeProcThreadAttributeList, UpdateProcThreadAttribute,
        EXTENDED_STARTUPINFO_PRESENT, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
        STARTUPINFOEXW,
    };

    use crate::restricted_spawn::AttributeListGuard;

    let mut size = 0usize;
    InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
    let mut buffer = vec![0u8; size];
    let list = buffer.as_mut_ptr() as *mut std::ffi::c_void;
    if InitializeProcThreadAttributeList(list, 1, 0, &mut size) == 0 {
        return None;
    }
    // Mover el `Vec` no realoca: el puntero sigue válido y queda guardado.
    let list_guard = AttributeListGuard::new(buffer, list);
    // Lista vacía: el hijo no hereda ningún handle del padre. Una lista de
    // atributos no admite tamaño cero, así que se lista el handle nulo, que
    // no designa ningún objeto y resulta inocuo para el hijo.
    let null_handle = 0isize;
    let (list_ptr, list_len) = if allowed.is_empty() {
        (&null_handle as *const isize, 1usize)
    } else {
        (allowed.as_ptr(), allowed.len())
    };
    let updated = UpdateProcThreadAttribute(
        list_guard.as_ptr(),
        0,
        PROC_THREAD_ATTRIBUTE_HANDLE_LIST as usize,
        list_ptr as *mut std::ffi::c_void,
        list_len * std::mem::size_of::<isize>(),
        std::ptr::null_mut(),
        std::ptr::null_mut(),
    );
    if updated == 0 {
        return None;
    }

    let mut extended: STARTUPINFOEXW = std::mem::zeroed();
    extended.StartupInfo.cb = std::mem::size_of::<STARTUPINFOEXW>() as u32;
    extended.StartupInfo.dwFlags = startup.dwFlags;
    extended.StartupInfo.wShowWindow = startup.wShowWindow;
    extended.lpAttributeList = list_guard.as_ptr();

    let mut info: PROCESS_INFORMATION = std::mem::zeroed();
    let launched = CreateProcessW(
        std::ptr::null(),
        cmdline,
        std::ptr::null_mut(),
        std::ptr::null_mut(),
        1,
        flags | EXTENDED_STARTUPINFO_PRESENT,
        std::ptr::null(),
        std::ptr::null(),
        &extended.StartupInfo,
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
/// en el código de salida, porque nadie lee su terminal; cuando el propio
/// registro no se puede escribir, el motivo sale por stderr.
fn cleaner_worker_main(args: &[String]) -> i32 {
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};

    use crate::restricted_spawn::OwnedHandleWrap;

    let path = args.first().map(String::as_str).unwrap_or_default();
    let wait_pid: u32 = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let log = args.get(2).map(String::as_str).unwrap_or_default();
    let wait_handle: isize = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(0);

    // Si el directorio del registro no se puede crear, el informe no tendrá
    // dónde escribirse: la causa se guarda para incluirla en él y no depender
    // solo del propio fallo de escritura.
    let mut log_dir_note = String::new();
    if !log.is_empty() {
        if let Some(parent) = Path::new(log).parent() {
            if !parent.as_os_str().is_empty() {
                if let Err(e) = std::fs::create_dir_all(parent) {
                    log_dir_note = format!("; el directorio del registro no se pudo crear ({e})");
                }
            }
        }
    }
    // El proceso esperado se aguarda por el sistema, sin sondeo ni PID.
    let mut waited = false;
    if wait_handle != 0 {
        // El guard cierra el handle heredado al salir de este ámbito,
        // también si la espera falla.
        let _wait_guard = OwnedHandleWrap::new(wait_handle);
        // SAFETY: el handle lo abrió quien programó y lo heredó este proceso
        // con derecho de espera; el guard lo cierra tras aguardar.
        unsafe {
            WaitForSingleObject(wait_handle, INFINITE);
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
    let (mut report, code) = if target.exists() {
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
    // La causa del fallo de creación del directorio viaja en el informe: si el
    // registro no tiene dónde escribirse, al menos se intentó decirlo.
    report.push_str(&log_dir_note);
    if !log.is_empty() {
        // El registro es el único sitio donde este trabajo es observable (nadie
        // lee la terminal de un proceso desacoplado): si no se puede escribir,
        // el motivo sale por stderr antes de terminar.
        if let Err(e) = std::fs::write(log, &report) {
            eprintln!("avi-cleaner: no se pudo escribir el registro {log}: {e}; informe: {report}");
        }
    }
    // La copia en uso no se puede borrar a sí misma en Windows: el barrido de
    // temporales recoge las copias rancias por su prefijo. Si el borrado falla
    // por otra causa, se deja dicho para no perder la pista del resto.
    if let Err(e) = std::fs::remove_file(std::env::current_exe().unwrap_or_default()) {
        eprintln!(
            "avi-cleaner: no se pudo borrar la copia de sí mismo ({}): {e}; \
             la recogerá el barrido de temporales",
            std::env::current_exe().unwrap_or_default().display()
        );
    }
    code
}
