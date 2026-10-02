#![cfg(windows)]

use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// El auxiliar debe ejecutar su script de verdad: tras morir el proceso
/// esperado, el directorio desaparece y el `.ps1` y su marca `.ready` se borran
/// solos.
#[test]
fn deferred_removal_deletes_path_and_self_deletes_script() {
    let dir = std::env::temp_dir().join(format!("avi-process-it-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("a.txt"), "a").unwrap();
    std::fs::write(dir.join("sub").join("b.txt"), "b").unwrap();

    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-Command", "Start-Sleep 2"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(avi_process::CREATE_NO_WINDOW)
        .spawn()
        .unwrap();

    let helper = avi_process::spawn_deferred_removal(&dir, child.id());
    let _ = child.wait();

    let deadline = Instant::now() + Duration::from_secs(30);
    let helper_path = helper.as_ref().ok().cloned();
    let ready_path = helper_path.as_ref().map(|p| p.with_extension("ready"));
    while Instant::now() < deadline {
        let helper_gone = helper_path.as_ref().is_none_or(|p| !p.exists());
        let ready_gone = ready_path.as_ref().is_none_or(|p| !p.exists());
        if !dir.exists() && helper_gone && ready_gone {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Limpieza defensiva previa a las aserciones.
    let dir_left = dir.exists();
    let helper_left = helper_path.as_ref().is_some_and(|p| p.exists());
    let ready_left = ready_path.as_ref().is_some_and(|p| p.exists());
    let _ = std::fs::remove_dir_all(&dir);
    if let Some(p) = &helper_path {
        let _ = std::fs::remove_file(p);
    }
    if let Some(p) = &ready_path {
        let _ = std::fs::remove_file(p);
    }

    helper.expect("el auxiliar debe arrancar");
    assert!(!dir_left, "el directorio debe haberse borrado");
    assert!(!helper_left, "el script debe autoborrarse");
    assert!(
        !ready_left,
        "la marca de arranque debe borrarla el auxiliar"
    );
}
