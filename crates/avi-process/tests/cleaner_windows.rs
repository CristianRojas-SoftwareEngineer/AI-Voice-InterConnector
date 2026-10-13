// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Lee el registro del limpiador; un registro todavía ausente cuenta como vacío
/// y cualquier otro fallo de lectura aborta la prueba.
fn read_log(log: &std::path::Path) -> String {
    match std::fs::read_to_string(log) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => panic!("no se pudo leer el registro {log:?}: {e}"),
    }
}

/// El limpiador propio borra la ruta cuando muere el proceso esperado, sin
/// PowerShell ni scripts: tras morir el hijo no queda el directorio y el
/// registro informa del borrado.
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
    let mut log_text = String::new();
    while Instant::now() < deadline {
        log_text = read_log(&log);
        if !dir.exists() && log_text.contains("borrado") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    let dir_left = dir.exists();
    remove_cleaner_copy(&copy);

    assert!(!dir_left, "el directorio debe haberse borrado");
    assert!(
        log_text.contains("borrado"),
        "el registro debe informar del borrado: {log_text:?}"
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

    let deadline = Instant::now() + Duration::from_secs(30);
    let mut log_text = String::new();
    while Instant::now() < deadline {
        log_text = read_log(&log);
        if log_text.contains("fallo: no se pudo retirar") {
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let dir_left = dir.exists();
    remove_cleaner_copy(&copy);

    assert!(
        dir_left,
        "la ruta bloqueada debe seguir en disco para el barrido"
    );
    assert!(
        log_text.contains("fallo: no se pudo retirar"),
        "el registro debe informar del fallo: {log_text:?}"
    );
}

/// Un trabajador con argumentos inválidos termina con código distinto de cero
/// y deja el motivo en el registro cuando este es utilizable.
#[test]
fn worker_with_invalid_arguments_fails_and_reports() {
    let log_root = tempfile::tempdir().unwrap();
    let log = log_root.path().join("cleaner.log");

    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--avi-cleaner-worker")
        .args(["C:\\ruta-cualquiera", "no-es-un-pid"])
        .arg(&log)
        .arg("0")
        .output()
        .expect("lanzar el trabajador");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let log_text = read_log(&log);
    assert!(
        log_text.contains("argumentos inválidos"),
        "el registro debe nombrar el fallo: {log_text:?}"
    );
}

/// Sin ruta de registro utilizable el motivo sale por stderr y el código es
/// distinto de cero.
#[test]
fn worker_without_log_argument_fails_on_stderr() {
    let output = Command::new(std::env::current_exe().unwrap())
        .arg("--avi-cleaner-worker")
        .arg("C:\\ruta-cualquiera")
        .output()
        .expect("lanzar el trabajador");

    assert_ne!(output.status.code(), Some(0), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("argumentos inválidos"), "{stderr}");
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
