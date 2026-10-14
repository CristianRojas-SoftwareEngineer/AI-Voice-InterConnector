// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Pruebas deterministas del lanzamiento con herencia restringida: el hijo
//! informa su salida y el registro declarado recibe la salida del hijo. Sin
//! esperas fijas, sin modelos, sin red.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// Un hijo lanzado con la lista vacía informa su salida: el lanzamiento no
/// depende de ninguna herencia ambiental.
#[test]
fn spawn_with_allowlist_reports_child_exit() {
    let (program, args) = exit_command(0);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "exit 0 debe dar éxito: {status:?}");

    let (program, args) = exit_command(3);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert_eq!(status.code(), Some(3), "exit 3 debe dar código 3");
}

/// Las tres variables del temporal se sustituyen en el hijo y apuntan al
/// directorio propio del padre, tanto con el bloque de entorno de Windows
/// como con el entorno explícito de Unix.
#[test]
fn child_receives_scratch_environment() {
    let scratch = tempfile::tempdir().expect("debe crear el scratch de prueba");
    let mut reader = tempfile::tempfile().expect("debe crear el registro anónimo");
    let log_file = reader.try_clone().expect("debe clonar el registro");
    let (program, args) = if cfg!(windows) {
        (
            powershell(),
            vec![
                String::from("-NoProfile"),
                String::from("-Command"),
                String::from("Write-Output \"$env:TMP|$env:TEMP|$env:TMPDIR\""),
            ],
        )
    } else {
        (
            PathBuf::from("sh"),
            vec![
                String::from("-c"),
                String::from("printf '%s|%s|%s' \"$TMP\" \"$TEMP\" \"$TMPDIR\""),
            ],
        )
    };
    let request = avi_process::RestrictedSpawnRequest {
        program,
        args,
        stdin: avi_process::StdinSpec::Null,
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    let env = [
        ("TMP", scratch.path().as_os_str()),
        ("TEMP", scratch.path().as_os_str()),
        ("TMPDIR", scratch.path().as_os_str()),
    ];
    let mut child = avi_process::spawn_with_allowlist_env(request, &env)
        .expect("debe lanzar al hijo con el scratch");
    assert!(child.wait().expect("debe esperar al hijo").success());
    reader.seek(SeekFrom::Start(0)).expect("debe rebobinar");
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .expect("debe leer el registro");
    let expected = scratch.path().display().to_string();
    assert_eq!(text.trim(), format!("{expected}|{expected}|{expected}"));
}

/// El fichero de registro declarado recibe la salida del hijo: lo declarado
/// sí se hereda y funciona.
#[test]
fn listed_log_handle_receives_child_output() {
    let mut reader = tempfile::tempfile().expect("debe crear el registro anónimo");
    let log_file = reader.try_clone().expect("debe clonar el registro");

    let (program, args) = echo_command("allowlist-marker");
    let mut child = spawn(&program, args, log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "el eco debe salir 0: {status:?}");

    reader
        .seek(SeekFrom::Start(0))
        .expect("debe rebobinar el registro");
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .expect("debe leer el registro");
    assert!(
        text.contains("allowlist-marker"),
        "el registro debe contener la salida del hijo: {text:?}"
    );
}

/// Un handle heredable FUERA de la allowlist no puede llegar al hijo. Si el
/// mecanismo de lista se rompiera, el hijo heredaría el extremo de lectura y
/// lo leería hasta EOF sin datos (el padre cerró el otro extremo), y escribiría
/// el testigo `eof` en `stdin.txt`; el estado esperado es `closed`.
#[cfg(windows)]
#[test]
fn handle_outside_the_list_does_not_reach_the_child() {
    let witness = run_pipe_probe(false);
    assert_eq!(
        witness, "closed",
        "un handle fuera de la allowlist no debió llegar al hijo (eof/open indican fuga)"
    );
}

/// Un handle listado en `extra_allowed` sí llega al hijo: el padre cierra el
/// otro extremo de la tubería, así que el hijo lee EOF sin datos y escribe el
/// testigo `eof`. Si la sonda del doble falla (por ejemplo `Add-Type`
/// bloqueado), el testigo es `closed` y esta prueba enrojece.
#[cfg(windows)]
#[test]
fn listed_handle_reaches_the_child() {
    let witness = run_pipe_probe(true);
    assert_eq!(
        witness, "eof",
        "un handle listado en extra_allowed debió llegar al hijo"
    );
}

/// Un hijo lanzado con `StdinSpec::Piped` lee su entrada hasta el fin de
/// fichero y termina cuando el padre suelta el `RestrictedChild`: el único
/// extremo de escritura vivo es el que retiene el padre. El hijo deja un
/// testigo al ver el fin de fichero y la espera es un sondeo acotado.
#[cfg(windows)]
#[test]
fn piped_child_sees_eof_when_the_child_handle_is_dropped() {
    let temp_dir = tempfile::tempdir().expect("debe crear el directorio temporal");
    let witness = temp_dir.path().join("eof.txt");
    let script = format!(
        "[Console]::In.ReadToEnd() | Out-Null; Set-Content -Path '{}' -Value 'eof'",
        witness.display()
    );
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let request = avi_process::RestrictedSpawnRequest {
        program: powershell(),
        args: vec![String::from("-NoProfile"), String::from("-Command"), script],
        stdin: avi_process::StdinSpec::Piped,
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    let mut child = avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo");
    assert!(
        child.try_wait().expect("debe consultar al hijo").is_none(),
        "con el extremo de escritura retenido el hijo no debe terminar"
    );

    drop(child);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !witness.exists() {
        assert!(
            std::time::Instant::now() < deadline,
            "el hijo no vio fin de fichero tras soltarse el RestrictedChild"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

/// Lanza un hijo que sondea el extremo de lectura de una tubería creada por el
/// padre y devuelve el testigo que escribe (`eof`, `open` o `closed`).
/// `listed` indica si ese extremo entra en `extra_allowed`.
#[cfg(windows)]
fn run_pipe_probe(listed: bool) -> String {
    use windows_sys::Win32::Foundation::{CloseHandle, SetHandleInformation, HANDLE_FLAG_INHERIT};
    use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
    use windows_sys::Win32::System::Pipes::CreatePipe;

    let temp_dir = tempfile::tempdir().expect("debe crear el directorio temporal");
    let dir = temp_dir.path().to_path_buf();

    // Tubería creada por el padre: el extremo de lectura queda heredable y a
    // propósito entra en `extra_allowed` solo si `listed`; el de escritura se
    // desmarca para que el hijo nunca pueda retenerlo abierto.
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
    std::fs::write(dir.join("handle.txt"), read.to_string())
        .expect("debe escribir el valor del handle");

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

    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let request = avi_process::RestrictedSpawnRequest {
        program: powershell(),
        args: vec![String::from("-NoProfile"), String::from("-Command"), script],
        stdin: avi_process::StdinSpec::Null,
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: if listed { vec![read] } else { Vec::new() },
    };
    let mut child = avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo");

    // El padre retiene los extremos hasta aquí y los cierra: a partir de este
    // momento el extremo de lectura solo puede devolver EOF sin datos.
    unsafe {
        CloseHandle(read);
        CloseHandle(write);
    }
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "el doble debe salir 0: {status:?}");

    let witness =
        std::fs::read_to_string(dir.join("stdin.txt")).expect("el doble debe escribir el testigo");
    witness.trim().to_string()
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
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
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

/// En Unix el hijo no hereda los descriptores que el padre abrió con
/// `FD_CLOEXEC`, que es como `std` abre ficheros, tuberías y sockets: esa marca
/// es la que impide que la tubería de captura del lanzador o los sockets del
/// CLI lleguen al daemon. La sonda pregunta desde el hijo si el número de
/// descriptor sigue abierto (`/dev/fd/N`). El control positivo quita la marca
/// al mismo descriptor y exige que el hijo sí lo vea: sin él, una sonda que no
/// detectara nunca nada daría la prueba por buena. Solo se desmarca el extremo
/// de lectura, cuyo escritor retiene el padre, así que un hijo de otra prueba
/// que lo heredara en esa ventana no altera nada.
#[cfg(unix)]
#[test]
fn unix_child_does_not_inherit_cloexec_descriptors() {
    use std::os::fd::AsRawFd;

    let (reader, writer) = std::io::pipe().expect("debe crear la tubería");
    let fd = reader.as_raw_fd();
    assert_eq!(
        probe_descriptor(fd),
        "closed",
        "un descriptor con FD_CLOEXEC no debió llegar al hijo"
    );

    // SAFETY: `fcntl` solo cambia las marcas de un descriptor propio y abierto.
    let cleared = unsafe { libc::fcntl(fd, libc::F_SETFD, 0) };
    assert_eq!(cleared, 0, "debe poder quitar FD_CLOEXEC al descriptor");
    assert_eq!(
        probe_descriptor(fd),
        "open",
        "sin FD_CLOEXEC el hijo debió heredar el descriptor: la sonda no detecta la herencia"
    );
    drop(writer);
    drop(reader);
}

/// En Unix el lanzamiento no puede declarar descriptores extra: una lista
/// `extra_allowed` no vacía falla sin lanzar nada, en vez de lanzar a un hijo
/// que no recibiría lo pedido.
#[cfg(unix)]
#[test]
fn unix_rejects_extra_allowed() {
    let (program, args) = exit_command(0);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let request = avi_process::RestrictedSpawnRequest {
        program,
        args,
        stdin: avi_process::StdinSpec::Null,
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: vec![3],
    };
    match avi_process::spawn_with_allowlist(request) {
        Ok(_) => panic!("una lista extra_allowed no vacía debió fallar en Unix"),
        Err(error) => assert_eq!(
            error.kind(),
            std::io::ErrorKind::Unsupported,
            "el fallo debe ser de operación no admitida: {error}"
        ),
    }
}

/// Lanza `sh` con el lanzamiento restringido para que informe si el
/// descriptor `fd` está abierto en el hijo, y devuelve su testigo (`open` o
/// `closed`) leído del registro.
#[cfg(unix)]
fn probe_descriptor(fd: i32) -> String {
    let mut reader = tempfile::tempfile().expect("debe crear el registro anónimo");
    let log_file = reader.try_clone().expect("debe clonar el registro");
    let script = format!("if [ -e /dev/fd/{fd} ]; then echo open; else echo closed; fi");
    let mut child = spawn(Path::new("sh"), vec![String::from("-c"), script], log_file);
    let status = child.wait().expect("debe poder esperar al hijo");
    assert!(status.success(), "la sonda debe salir 0: {status:?}");
    reader
        .seek(SeekFrom::Start(0))
        .expect("debe rebobinar el registro");
    let mut text = String::new();
    reader
        .read_to_string(&mut text)
        .expect("debe leer el registro");
    text.trim().to_string()
}

/// `wait_timeout` devuelve el estado real cuando el hijo termina a tiempo.
#[test]
fn wait_timeout_returns_exit_when_child_finishes_in_time() {
    let (program, args) = exit_command(3);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let mut child = spawn(&program, args, log_file);
    let status = child
        .wait_timeout(std::time::Duration::from_secs(30))
        .expect("la espera acotada no debe fallar")
        .expect("el hijo terminó a tiempo");
    assert_eq!(status.code(), Some(3), "exit 3 debe dar código 3");
}

/// `wait_timeout` mata al hijo colgado al vencer y lo recolecta: la muerte es
/// real (el PID ya no vive), no solo el retorno.
#[test]
fn wait_timeout_kills_hanging_child() {
    let (program, args) = sleep_command(30);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let mut child = spawn(&program, args, log_file);
    let pid = child.id();
    let outcome = child
        .wait_timeout(std::time::Duration::from_secs(2))
        .expect("la espera acotada no debe fallar");
    assert!(outcome.is_none(), "el hijo colgado debió vencer el plazo");
    assert!(
        child.try_wait().expect("debe consultar al hijo").is_some(),
        "el hijo vencido debe estar muerto y recolectado, sin huérfano"
    );
    assert!(
        !avi_process::pid_alive_exact(pid),
        "el PID del hijo vencido no debe seguir vivo"
    );
}

/// El supresor de UI restaura el modo de error del hilo tras el éxito y tras
/// el fallo: el lanzamiento no deja el hilo enmudecido.
#[cfg(windows)]
#[test]
fn error_mode_is_restored_after_success_and_failure() {
    use windows_sys::Win32::System::Diagnostics::Debug::GetThreadErrorMode;
    // SAFETY: solo consulta el modo de error del propio hilo.
    let before = unsafe { GetThreadErrorMode() };

    let (program, args) = exit_command(0);
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let mut child = spawn(&program, args, log_file);
    let status = child
        .wait_timeout(std::time::Duration::from_secs(30))
        .expect("la espera acotada no debe fallar")
        .expect("el hijo termina a tiempo");
    assert!(status.success(), "exit 0 debe dar éxito: {status:?}");
    // SAFETY: como arriba.
    assert_eq!(
        unsafe { GetThreadErrorMode() },
        before,
        "el éxito debe restaurar el modo"
    );

    let missing = tempfile::tempfile().expect("debe crear el registro anónimo");
    let request = avi_process::RestrictedSpawnRequest {
        program: PathBuf::from("binario-que-no-existe-xyz"),
        args: Vec::new(),
        stdin: avi_process::StdinSpec::Null,
        stderr_file: missing.try_clone().expect("debe clonar el registro"),
        stdout_file: missing,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    assert!(
        avi_process::spawn_with_allowlist(request).is_err(),
        "el binario ausente debe fallar"
    );
    // SAFETY: como arriba.
    assert_eq!(
        unsafe { GetThreadErrorMode() },
        before,
        "el fallo debe restaurar el modo"
    );
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
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo")
}

/// El lanzamiento ejecuta exactamente la ruta recibida y no recorre el orden de
/// búsqueda del sistema: un nombre suelto que el `PATH` sí resolvería no se
/// lanza, de modo que un ejecutable homónimo en el `PATH` nunca suplanta al
/// programa resuelto.
#[cfg(windows)]
#[test]
fn bare_program_name_is_not_searched_in_path() {
    let log_file = tempfile::tempfile().expect("debe crear el registro anónimo");
    let request = avi_process::RestrictedSpawnRequest {
        program: PathBuf::from("powershell"),
        args: vec![
            String::from("-NoProfile"),
            String::from("-Command"),
            String::from("exit 0"),
        ],
        stdin: avi_process::StdinSpec::Null,
        stderr_file: log_file.try_clone().expect("debe clonar el registro"),
        stdout_file: log_file,
        creation_flags: 0,
        extra_allowed: Vec::new(),
    };
    assert!(
        avi_process::spawn_with_allowlist(request).is_err(),
        "un nombre suelto no se busca en el PATH"
    );
}

/// PowerShell del sistema por su ruta completa: el lanzamiento restringido
/// ejecuta exactamente la ruta que recibe y no busca un nombre suelto.
fn powershell() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").expect("Windows define SystemRoot"))
        .join(r"System32\WindowsPowerShell\v1.0\powershell.exe")
}

fn exit_command(code: u32) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            powershell(),
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

fn sleep_command(secs: u64) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            powershell(),
            vec![
                String::from("-NoProfile"),
                String::from("-Command"),
                format!("Start-Sleep -Seconds {secs}"),
            ],
        )
    } else {
        (PathBuf::from("sleep"), vec![secs.to_string()])
    }
}

fn echo_command(marker: &str) -> (PathBuf, Vec<String>) {
    if cfg!(windows) {
        (
            powershell(),
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
