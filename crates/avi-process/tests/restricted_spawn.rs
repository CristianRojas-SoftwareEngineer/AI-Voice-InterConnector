//! Pruebas deterministas del lanzamiento con herencia restringida: la lista
//! contiene solo lo declarado, el hijo informa su salida y el registro
//! declarado recibe la salida del hijo. Sin esperas fijas, sin modelos, sin red.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Contador global de temporales: PID + nanosegundos pueden colisionar entre
/// hilos (o entre llamadas rápidas), así que cada nombre lleva su secuencia.
static TEMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

fn next_temp_id() -> usize {
    TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
}

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
    let dir = std::env::temp_dir().join(format!(
        "avi-allowlist-output-{}-{}",
        std::process::id(),
        next_temp_id()
    ));
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

/// Un handle heredable FUERA de la allowlist no puede llegar al hijo. Si el
/// mecanismo de lista se rompiera, el hijo heredaría el extremo de lectura y
/// lo leería hasta EOF sin datos (el padre cerró el otro extremo), y escribiría
/// el testigo `eof` en `stdin.txt`; el estado esperado es `closed`.
#[cfg(windows)]
#[test]
fn handle_outside_the_list_does_not_reach_the_child() {
    use windows_sys::Win32::Foundation::{
        CloseHandle, SetHandleInformation, HANDLE_FLAG_INHERIT,
    };
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let dir = std::env::temp_dir().join(format!(
        "avi-allowlist-neg-{}-{}",
        std::process::id(),
        next_temp_id()
    ));
    std::fs::create_dir_all(&dir).expect("debe crear el directorio temporal");

    // Tubería creada por el padre: el extremo de lectura queda heredable y a
    // propósito NO entra en `extra_allowed`; el de escritura se desmarca para
    // que el hijo nunca pueda retenerlo abierto.
    let mut read = 0isize;
    let mut write = 0isize;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: std::ptr::null_mut(),
        bInheritHandle: 1,
    };
    let created = unsafe { CreatePipe(&mut read, &mut write, &attributes, 0) };
    assert_eq!(created, 1, "debe crear la tubería");
    unsafe { SetHandleInformation(write, HANDLE_FLAG_INHERIT, 0) };

    // El valor numérico del extremo viaja al hijo por un fichero conocido.
    std::fs::write(dir.join("handle.txt"), read.to_string()).expect("debe escribir el valor del handle");

    // Doble del hijo (modelado sobre el motor doble de avi-tts): sondea el
    // handle recibido y registra en `stdin.txt` si llegó a EOF sin datos o no.
    let script = r#"
$dir = '__DIR__'
$n = [int]((Get-Content -Raw (Join-Path $dir 'handle.txt')).Trim())
$witness = Join-Path $dir 'stdin.txt'
try {
    Add-Type -TypeDefinition @"
using System;
using System.Runtime.InteropServices;
public static class Probe {
    [DllImport("kernel32.dll")] public static extern uint GetFileType(IntPtr h);
}
"@
    $h = New-Object Microsoft.Win32.SafeHandles.SafeFileHandle -ArgumentList $n, $true
    if ($h.IsInvalid) { throw 'invalido' }
    $kind = [Probe]::GetFileType($h.DangerousGetHandle())
    if ($kind -ne 0x3) { throw 'no-pipe' }
    $fs = New-Object System.IO.FileStream -ArgumentList $h, ([System.IO.FileAccess]::Read)
    $buf = New-Object byte[] 1
    $r = $fs.Read($buf, 0, 1)
    if ($r -eq 0) { Set-Content -Path $witness -Value 'eof' } else { Set-Content -Path $witness -Value 'open' }
} catch {
    Set-Content -Path $witness -Value 'closed'
}
"#
    .replace("__DIR__", &dir.display().to_string());

    let log_path = dir.join("child.log");
    let log_file = std::fs::File::create(&log_path).expect("debe crear el registro");
    let request = avi_process::RestrictedSpawnRequest {
        program: PathBuf::from("powershell"),
        args: vec![
            String::from("-NoProfile"),
            String::from("-Command"),
            script,
        ],
        stdin: avi_process::StdinSpec::Null,
        log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    let mut child =
        avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo");

    // El padre retiene los extremos hasta aquí y los cierra: a partir de este
    // momento el extremo de lectura solo puede devolver EOF sin datos.
    unsafe {
        CloseHandle(read);
        CloseHandle(write);
    }
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "el doble debe salir 0: {status:?}");

    let witness = std::fs::read_to_string(dir.join("stdin.txt"))
        .expect("el doble debe escribir el testigo");
    let witness = witness.trim();
    assert_eq!(
        witness, "closed",
        "un handle fuera de la allowlist no debió llegar al hijo (eof/open indican fuga)"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// En Unix el hijo debe nacer líder de su propio grupo (`setsid`):
/// `getpgid(child) == child`. Sin esa propiedad, el kill de grupo
/// (`kill(-pid)`) mata al grupo del lanzador. La observación espera la
/// transición —el padre puede ver al hijo antes de que corra su `pre_exec`—
/// con deadline acotado, no con un tiempo fijo.
#[cfg(unix)]
#[test]
fn unix_child_is_group_leader() {
    let (program, args) = (PathBuf::from("sleep"), vec![String::from("30")]);
    let log_file = temp_log_file("group-leader");
    let mut child = spawn(&program, args, log_file);
    let pid = child.id() as i32;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        // SAFETY: `getpgid` solo consulta el grupo de un PID vivo recién
        // lanzado y representable como `pid_t`.
        let pgid = unsafe { libc::getpgid(pid) };
        if pgid == pid {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "el hijo no es líder de su grupo: pgid {pgid}, pid {pid}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    child.kill().expect("debe poder matar al hijo");
    child.wait().expect("debe poder recolectar al hijo");
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
        "avi-allowlist-{name}-{}-{}-{}.log",
        std::process::id(),
        next_temp_id(),
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
