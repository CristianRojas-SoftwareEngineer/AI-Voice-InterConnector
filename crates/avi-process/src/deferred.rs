use std::io::Write;
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
///    y lo hereda a la copia. Si el sistema no deja abrir un proceso que sigue
///    vivo, programar falla: esperar a un PID sin handle reintroduciría la
///    carrera de reutilización.
/// 3. Relanza la copia desacoplada y oculta con argumentos internos: espera la
///    muerte por el sistema, borra `path` con reintentos acotados con retroceso
///    y escribe su resultado en el registro `log_path`, que lee la
///    verificación del ciclo. Lo que siga bloqueado tras los reintentos queda
///    para el barrido de la siguiente operación y su red (`doctor --repair`).
///
/// El lanzamiento intenta separar la copia del Job Object del padre. Si el Job
/// no lo permite, se lanza sin separar y el aviso queda en stderr y en el
/// registro: sin separación, la copia puede morir con el padre.
///
/// Devuelve la ruta de la copia limpiadora. Nunca genera `.ps1`.
pub fn schedule_clean_removal(
    path: &Path,
    wait_pid: u32,
    log_path: &Path,
) -> anyhow::Result<PathBuf> {
    use windows_sys::Win32::Foundation::{GetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::SYNCHRONIZE;
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, WaitForSingleObject, CREATE_BREAKAWAY_FROM_JOB,
        STARTUPINFOW,
    };

    use crate::restricted_spawn::{mark_inheritable, quote_arg, InheritFlagGuard, OwnedHandleWrap};

    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let copy = std::env::temp_dir().join(format!("avi-cleaner-{}-{stamp}.exe", std::process::id()));
    // La copia a medio escribir también es un resto: si falla, se retira.
    // Toda salida con error retira la copia; su fallo se avisa sin tapar el
    // error de la operación.
    let discard_copy = || {
        avi_shared::best_effort(
            std::fs::remove_file(&copy),
            format_args!(
                "no se pudo borrar la copia del limpiador {}",
                copy.display()
            ),
        );
    };
    if let Err(e) = std::fs::copy(std::env::current_exe()?, &copy) {
        discard_copy();
        return Err(e.into());
    }

    // SAFETY: las llamadas al sistema usan búferes locales del tamaño
    // declarado; cada handle abierto se cierra en todos los caminos.
    unsafe {
        // El HANDLE se abre al programar: fija la identidad del proceso
        // esperado aunque su PID se reutilice después. El PID 0 y un PID que ya
        // no existe no se esperan: se pasa el nulo como centinela. Un PID vivo
        // que el sistema no deja abrir es un error. El guard cierra el handle
        // en todos los caminos, incluidos los de error.
        let wait_handle = if wait_pid == 0 {
            0
        } else {
            let handle = OpenProcess(SYNCHRONIZE, 0, wait_pid);
            if handle == 0 {
                let code = GetLastError();
                if crate::pid_alive_exact(wait_pid) {
                    discard_copy();
                    anyhow::bail!(
                        "no se pudo abrir el proceso {wait_pid} para esperar su muerte (código {code}) y sigue vivo"
                    );
                }
            }
            handle
        };
        let _wait_guard = if wait_handle != 0 {
            Some(OwnedHandleWrap::new(wait_handle))
        } else {
            None
        };
        // El guard restaura la marca de heredabilidad al salir de este
        // ámbito, también si el lanzamiento del limpiador falla.
        let _inherit_guard = if wait_handle != 0 {
            if let Err(e) = mark_inheritable(wait_handle) {
                discard_copy();
                anyhow::bail!("no se pudo marcar como heredable el handle de espera: {e}");
            }
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
        // Herencia restringida a la lista explícita: solo el HANDLE de espera
        // viaja a la copia (vacía si no hay proceso que aguardar); el stdio
        // del padre no llega al limpiador por construcción.
        let allowed: Vec<isize> = if wait_handle != 0 {
            vec![wait_handle]
        } else {
            Vec::new()
        };
        // Intento de separación del Job Object de la terminal: si el padre vive
        // en un Job que lo permite, el limpiador sobrevive a la muerte del
        // padre. Si el Job no lo permite, se avisa por stderr y en el registro
        // y se lanza sin separar; lo no borrado cae al barrido de la siguiente
        // operación.
        let launched = match launch_cleaner(
            cmdline.as_mut_ptr(),
            &startup,
            base_flags | CREATE_BREAKAWAY_FROM_JOB,
            &allowed,
        ) {
            Ok(pair) => Ok(pair),
            Err(code) => {
                let notice = format!(
                    "aviso: no se pudo lanzar el limpiador separado del Job Object del proceso (código {code}); se lanza sin separar y, si el proceso padre muere antes, lo no borrado queda para el barrido"
                );
                eprintln!("{notice}");
                if let Err(e) = append_to_log(log_path, &notice) {
                    eprintln!(
                        "aviso: no se pudo escribir el aviso en el registro {}: {e}",
                        log_path.display()
                    );
                }
                launch_cleaner(cmdline.as_mut_ptr(), &startup, base_flags, &allowed)
            }
        };
        let (process, thread) = match launched {
            Ok(pair) => pair,
            Err(code) => {
                discard_copy();
                anyhow::bail!("no se pudo lanzar el limpiador (código {code})");
            }
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
                discard_copy();
                anyhow::bail!("el limpiador terminó con fallo al arrancar");
            }
        }
    }
    Ok(copy)
}

/// Añade una línea al registro del limpiador, creando su directorio si falta.
fn append_to_log(log: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = log.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)?;
    writeln!(file, "{text}")
}

/// Lanza la copia limpiadora con esas flags de creación y herencia
/// restringida a `allowed` (normalmente solo el HANDLE de espera; vacío si no
/// hay proceso que aguardar). Devuelve los handles de proceso e hilo; el hilo
/// lo cierra el llamador. Si el sistema rechaza el lanzamiento, devuelve el
/// código de error de la llamada que falló.
///
/// SAFETY: la línea de comandos es un búfer local válido y terminado en nulo;
/// el arranque solo hereda los handles listados.
unsafe fn launch_cleaner(
    cmdline: *mut u16,
    startup: &windows_sys::Win32::System::Threading::STARTUPINFOW,
    flags: u32,
    allowed: &[isize],
) -> Result<(isize, isize), u32> {
    use windows_sys::Win32::Foundation::GetLastError;
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
        return Err(GetLastError());
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
        return Err(GetLastError());
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
        return Err(GetLastError());
    }
    Ok((info.hProcess, info.hThread))
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

/// Código de salida del trabajador ante argumentos inválidos.
const CLEANER_BAD_ARGS_EXIT: i32 = 2;

/// Código de salida del trabajador cuando la espera del proceso falla.
const CLEANER_WAIT_FAILED_EXIT: i32 = 3;

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
    let code = match args
        .map(|a| a.into_string())
        .collect::<Result<Vec<String>, _>>()
    {
        Ok(rest) => cleaner_worker_main(&rest),
        Err(raw) => {
            eprintln!("avi-cleaner: argumento que no es UTF-8 válido: {raw:?}");
            CLEANER_BAD_ARGS_EXIT
        }
    };
    std::process::exit(code);
}

/// Argumentos internos del trabajador, ya validados.
#[derive(Debug, PartialEq, Eq)]
struct WorkerArgs {
    path: String,
    wait_pid: u32,
    log: String,
    wait_handle: isize,
}

/// Valida los cuatro argumentos del trabajador (ruta a borrar, PID esperado,
/// ruta del registro y handle de espera heredado). Cualquier valor ausente,
/// vacío o no numérico es un error con su causa.
fn parse_worker_args(args: &[String]) -> Result<WorkerArgs, String> {
    let [path, wait_pid, log, wait_handle] = args else {
        return Err(format!(
            "se esperaban 4 argumentos y llegaron {}",
            args.len()
        ));
    };
    if path.is_empty() {
        return Err("la ruta a borrar está vacía".to_string());
    }
    if log.is_empty() {
        return Err("la ruta del registro está vacía".to_string());
    }
    let wait_pid = wait_pid
        .parse::<u32>()
        .map_err(|e| format!("PID de espera inválido '{wait_pid}': {e}"))?;
    let wait_handle = wait_handle
        .parse::<isize>()
        .map_err(|e| format!("handle de espera inválido '{wait_handle}': {e}"))?;
    Ok(WorkerArgs {
        path: path.clone(),
        wait_pid,
        log: log.clone(),
        wait_handle,
    })
}

/// Trabajador del limpiador: espera, borra con reintentos e informa en el
/// registro. Nunca entra en pánico: cualquier fallo termina en el registro y
/// en el código de salida, porque nadie lee su terminal; cuando el propio
/// registro no se puede escribir, el motivo sale por stderr.
fn cleaner_worker_main(args: &[String]) -> i32 {
    use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
    use windows_sys::Win32::System::Threading::{WaitForSingleObject, INFINITE};

    use crate::restricted_spawn::OwnedHandleWrap;

    let WorkerArgs {
        path,
        wait_pid,
        log,
        wait_handle,
    } = match parse_worker_args(args) {
        Ok(parsed) => parsed,
        Err(reason) => {
            let report = format!("fallo: argumentos inválidos del limpiador ({reason})");
            eprintln!("avi-cleaner: {report}");
            // Si el tercer argumento es una ruta utilizable, el registro es el
            // único sitio donde el fallo es observable por quien programó.
            if let Some(log) = args.get(2).filter(|l| !l.is_empty()) {
                if let Err(e) = append_to_log(Path::new(log), &report) {
                    eprintln!("avi-cleaner: no se pudo escribir el registro {log}: {e}");
                }
            }
            return CLEANER_BAD_ARGS_EXIT;
        }
    };

    // El proceso esperado se aguarda por el sistema, sin sondeo ni PID.
    let mut waited = false;
    if wait_handle != 0 {
        // El guard cierra el handle heredado al salir de este ámbito,
        // también si la espera falla.
        let _wait_guard = OwnedHandleWrap::new(wait_handle);
        // SAFETY: el handle lo abrió quien programó y lo heredó este proceso
        // con derecho de espera; el guard lo cierra tras aguardar.
        let wait = unsafe { WaitForSingleObject(wait_handle, INFINITE) };
        // Solo el handle señalado prueba que el proceso murió: ante cualquier
        // otro resultado no se borra nada.
        if wait != WAIT_OBJECT_0 {
            let report = format!(
                "fallo: la espera del proceso {wait_pid} terminó con un resultado inesperado ({wait:#x}); no se borró nada"
            );
            eprintln!("avi-cleaner: {report}");
            if let Err(e) = append_to_log(Path::new(&log), &report) {
                eprintln!("avi-cleaner: no se pudo escribir el registro {log}: {e}");
            }
            return CLEANER_WAIT_FAILED_EXIT;
        }
        waited = true;
    }

    let target = Path::new(&path);
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
    // El registro es el único sitio donde este trabajo es observable (nadie
    // lee la terminal de un proceso desacoplado): si no se puede escribir, el
    // motivo sale por stderr antes de terminar.
    if let Err(e) = append_to_log(Path::new(&log), &report) {
        eprintln!("avi-cleaner: no se pudo escribir el registro {log}: {e}; informe: {report}");
    }
    // La copia en ejecución no se borra a sí misma: Windows no deja borrar un
    // ejecutable en uso, y el barrido de temporales recoge las copias
    // obsoletas por su prefijo `avi-cleaner-`.
    code
}

#[cfg(test)]
mod tests {
    use super::{parse_worker_args, WorkerArgs};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).to_string()).collect()
    }

    /// Cuatro argumentos bien formados se interpretan tal cual.
    #[test]
    fn worker_args_parse_when_well_formed() {
        let parsed = parse_worker_args(&args(&["C:\\x", "42", "C:\\log.txt", "7"])).unwrap();
        assert_eq!(
            parsed,
            WorkerArgs {
                path: "C:\\x".to_string(),
                wait_pid: 42,
                log: "C:\\log.txt".to_string(),
                wait_handle: 7,
            }
        );
    }

    /// Faltan, sobran, vienen vacíos o no son numéricos: error con la causa,
    /// nunca un valor por defecto.
    #[test]
    fn worker_args_reject_invalid_values() {
        for bad in [
            args(&[]),
            args(&["C:\\x", "1", "C:\\log.txt"]),
            args(&["C:\\x", "1", "C:\\log.txt", "0", "extra"]),
            args(&["", "1", "C:\\log.txt", "0"]),
            args(&["C:\\x", "1", "", "0"]),
            args(&["C:\\x", "no", "C:\\log.txt", "0"]),
            args(&["C:\\x", "1", "C:\\log.txt", "no"]),
        ] {
            assert!(parse_worker_args(&bad).is_err(), "{bad:?}");
        }
    }
}
