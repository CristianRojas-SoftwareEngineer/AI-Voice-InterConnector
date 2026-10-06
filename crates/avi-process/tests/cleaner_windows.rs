#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// El limpiador propio borra la ruta cuando muere el proceso esperado, sin
/// PowerShell: tras morir el hijo no queda el directorio, el
/// registro informa del borrado y ningún `.ps1` del auxiliar antiguo aparece.
#[test]
fn cleaner_removes_path_after_watched_process_death() {
    let target_root = tempfile::tempdir().unwrap();
    let log_root = tempfile::tempdir().unwrap();
    let dir = target_root.path().to_path_buf();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();

    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-Command", "Start-Sleep 2"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(avi_process::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();
    let watched = child.id();

    let log = log_root.path().join("cleaner.log");
    let copy =
        avi_process::schedule_clean_removal(&dir, watched, &log).expect("programar el limpiador");
    let _ = child.wait();

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut ps1_left = true;
    while Instant::now() < deadline {
        ps1_left = std::fs::read_dir(std::env::temp_dir())
            .map(|entries| {
                entries.filter_map(|e| e.ok()).any(|e| {
                    e.file_name()
                        .to_string_lossy()
                        .starts_with(&format!("avi-deferred-{watched}-"))
                        && e.path().extension().is_some_and(|x| x == "ps1")
                })
            })
            .unwrap_or(false);
        if !dir.exists() && log.exists() && !ps1_left {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let dir_left = dir.exists();
    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    remove_cleaner_copy(&copy);

    assert!(!dir_left, "el directorio debe haberse borrado");
    assert!(
        log_text.contains("borrado"),
        "el registro debe informar del borrado: {log_text:?}"
    );
    assert!(
        !ps1_left,
        "el limpiador no usa PowerShell: no debe aparecer ningún avi-deferred-*.ps1"
    );
}

/// El limpiador informa en su registro cuando la ruta sigue bloqueada al
/// agotar los reintentos, en vez de dejarla en silencio.
#[test]
fn cleaner_reports_failure_when_path_stays_blocked() {
    let target_root = tempfile::tempdir().unwrap();
    let log_root = tempfile::tempdir().unwrap();
    let dir = target_root.path().to_path_buf();
    std::fs::write(dir.join("held.txt"), "x").unwrap();
    // El handle abierto retiene el archivo en Windows (sin compartir el
    // borrado), así que el borrado fracasa mientras viva este guardián.
    // `File::open` comparte el borrado por defecto y no retendría nada: se
    // abre con modo de compartición explícito sin `FILE_SHARE_DELETE`. Se
    // declara después de los directorios para que se suelte antes que ellos.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
        )
        .open(dir.join("held.txt"))
        .unwrap();

    let dead_pid = 2_000_000_000u32;
    let log = log_root.path().join("cleaner.log");
    let copy =
        avi_process::schedule_clean_removal(&dir, dead_pid, &log).expect("programar el limpiador");

    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline && !log.exists() {
        std::thread::sleep(Duration::from_millis(200));
    }

    let log_text = std::fs::read_to_string(&log).unwrap_or_default();
    let dir_left = dir.exists();
    remove_cleaner_copy(&copy);

    assert!(
        dir_left,
        "la ruta bloqueada debe seguir en disco para el barrido"
    );
    assert!(
        log_text.contains("no se pudo borrar") || log_text.contains("fallo"),
        "el registro debe informar del fallo: {log_text:?}"
    );
}

/// Borra la copia `avi-cleaner-*.exe` que devuelve `schedule_clean_removal`: el
/// trabajador no puede borrarse a sí mismo en Windows y, mientras siga vivo,
/// el sistema la mantiene bloqueada. Se sondea con plazo acotado de 10 s.
fn remove_cleaner_copy(copy: &std::path::Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        match std::fs::remove_file(copy) {
            Ok(()) => return,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return,
            Err(e) => {
                assert!(
                    Instant::now() < deadline,
                    "no se pudo borrar la copia del limpiador {copy:?}: {e}"
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
}
