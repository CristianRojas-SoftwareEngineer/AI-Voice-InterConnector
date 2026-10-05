//! Pruebas deterministas del lanzamiento con herencia restringida: la lista
//! contiene solo lo declarado, el hijo informa su salida y el registro
//! declarado recibe la salida del hijo. Sin tiempos, sin modelos, sin red.

use std::path::{Path, PathBuf};

/// La lista devuelve exactamente los handles declarados, sin añadir ni quitar
/// ninguno: lo que entra es lo que hereda el hijo.
#[test]
fn allowed_list_contains_only_declared_handles() {
    assert_eq!(avi_process::allowed_handle_list(&[]), Vec::<isize>::new());
    assert_eq!(avi_process::allowed_handle_list(&[7]), vec![7isize]);
    assert_eq!(
        avi_process::allowed_handle_list(&[3, 1, 2]),
        vec![3isize, 1, 2]
    );
}

/// Un hijo lanzado con la lista vacía informa su salida: el lanzamiento no
/// depende de ninguna herencia ambiental.
#[test]
fn spawn_with_allowlist_reports_child_exit() {
    let (program, args) = exit_command(0);
    let log_file = temp_log_file("exit-zero");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "exit 0 debe dar éxito: {status:?}");

    let (program, args) = exit_command(3);
    let log_file = temp_log_file("exit-three");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert_eq!(status.code(), Some(3), "exit 3 debe dar código 3");
}

/// El fichero de registro declarado recibe la salida del hijo: lo declarado
/// sí se hereda y funciona.
#[test]
fn listed_log_handle_receives_child_output() {
    let dir = std::env::temp_dir().join(format!("avi-allowlist-output-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("debe crear el directorio temporal");
    let log_path = dir.join("child.log");
    let log_file = std::fs::File::create(&log_path).expect("debe crear el registro");

    let (program, args) = echo_command("allowlist-marker");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "el eco debe salir 0: {status:?}");

    let text = std::fs::read_to_string(&log_path).expect("debe leer el registro");
    assert!(
        text.contains("allowlist-marker"),
        "el registro debe contener la salida del hijo: {text:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

fn spawn(
    program: &Path,
    args: Vec<String>,
    log_file: std::fs::File,
) -> avi_process::RestrictedChild {
    let request = avi_process::RestrictedSpawnRequest {
        program: program.to_path_buf(),
        args,
        stdin: avi_process::StdinSpec::Null,
        log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo")
}

fn temp_log_file(name: &str) -> std::fs::File {
    let path = std::env::temp_dir().join(format!(
        "avi-allowlist-{name}-{}-{}.log",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::File::create(&path).expect("debe crear el registro temporal")
}

fn exit_command(code: u32) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            PathBuf::from("powershell"),
            vec![
                String::from("-NoProfile"),
                String::from("-Command"),
                format!("exit {code}"),
            ],
        )
    } else {
        (
            PathBuf::from("sh"),
            vec![String::from("-c"), format!("exit {code}")],
        )
    }
}

fn echo_command(marker: &str) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            PathBuf::from("powershell"),
            vec![
                String::from("-NoProfile"),
                String::from("-Command"),
                format!("echo {marker}"),
            ],
        )
    } else {
        (
            PathBuf::from("sh"),
            vec![String::from("-c"), format!("echo {marker}")],
        )
    }
}
