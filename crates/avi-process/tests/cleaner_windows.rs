// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg(windows)]

use std::os::windows::fs::OpenOptionsExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use avi_process::OwnerToken;

/// Lanza un proceso que duerme `seconds` segundos.
fn spawn_sleeper(seconds: u32) -> Child {
    Command::new("powershell")
        .args(["-NoProfile", "-Command", &format!("Start-Sleep {seconds}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(avi_process::CREATE_NO_WINDOW)
        .spawn()
        .unwrap()
}

/// Token exacto del proceso hijo, observado mientras vive.
fn token_of(child: &Child) -> OwnerToken {
    let identity = avi_process::process_identity(child.id()).expect("el hijo vive y es observable");
    OwnerToken::new(child.id(), identity.start).expect("token válido")
}

/// Espera hasta 30 s a que se cumpla la condición; devuelve si se cumplió.
fn wait_until(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    condition()
}

/// Nombres de las entradas del directorio, ordenados.
fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Crea un destino con contenido dentro de `root`.
fn make_target(root: &Path) -> PathBuf {
    let target = root.join("victim");
    std::fs::create_dir_all(target.join("sub")).unwrap();
    std::fs::write(target.join("a.txt"), "a").unwrap();
    target
}

/// Tras morir el proceso esperado el destino desaparece y no queda nada más
/// junto a él: ni copia del ejecutable ni registro.
#[test]
fn removal_succeeds_without_leaving_copy_or_log() {
    let root = tempfile::tempdir().unwrap();
    let target = make_target(root.path());

    let mut child = spawn_sleeper(5);
    avi_process::schedule_removal(&target, &token_of(&child)).expect("programar el borrado");
    // Con la identidad exacta el borrado espera: el proceso sigue vivo y el
    // destino también.
    assert!(
        target.exists(),
        "no debe borrarse mientras vive el proceso esperado"
    );
    let _ = child.wait();

    assert!(
        wait_until(|| !target.exists()),
        "el destino debe haberse borrado"
    );
    assert_eq!(
        entries(root.path()),
        Vec::<String>::new(),
        "un borrado correcto no deja registro"
    );
}

/// Si el destino sigue bloqueado tras los reintentos, solo queda el registro
/// `<destino>.removal-failed.log` junto al destino.
#[test]
fn removal_failure_leaves_only_removal_failed_log() {
    let root = tempfile::tempdir().unwrap();
    let target = make_target(root.path());
    // El handle abierto sin `FILE_SHARE_DELETE` retiene el archivo en Windows,
    // así que el borrado fracasa mientras viva este guardián.
    let _held = std::fs::OpenOptions::new()
        .read(true)
        .share_mode(
            windows_sys::Win32::Storage::FileSystem::FILE_SHARE_READ
                | windows_sys::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
        )
        .open(target.join("a.txt"))
        .unwrap();

    let mut child = spawn_sleeper(60);
    let token = token_of(&child);
    child.kill().unwrap();
    let _ = child.wait();
    avi_process::schedule_removal(&target, &token).expect("programar el borrado");

    let log = root.path().join("victim.removal-failed.log");
    assert!(wait_until(|| log.exists()), "debe escribirse el registro");
    assert!(target.exists(), "el destino bloqueado sigue en disco");
    assert_eq!(
        entries(root.path()),
        vec![
            "victim".to_string(),
            "victim.removal-failed.log".to_string()
        ]
    );
    assert!(std::fs::read_to_string(&log)
        .unwrap()
        .contains("no se pudo retirar"));
}

/// Un PID reutilizado por otro proceso (mismo PID, otro inicio) cuenta como
/// muerto: el borrado no espera a que termine el proceso vivo.
#[test]
fn reused_pid_with_other_start_does_not_block() {
    let root = tempfile::tempdir().unwrap();
    let target = make_target(root.path());

    let mut child = spawn_sleeper(60);
    let real = token_of(&child);
    let other_start: u64 = real.start().parse::<u64>().unwrap() + 1;
    let reused = OwnerToken::new(real.pid(), other_start.to_string()).unwrap();
    avi_process::schedule_removal(&target, &reused).expect("programar el borrado");

    let removed = wait_until(|| !target.exists());
    let still_alive = child.try_wait().unwrap().is_none();
    child.kill().unwrap();
    let _ = child.wait();

    assert!(removed, "el destino debe borrarse sin esperar al PID vivo");
    assert!(still_alive, "el proceso con el PID reutilizado sigue vivo");
}

/// Un `powershell.exe` inexistente falla al programar, sin tocar el destino ni
/// dejar efectos; un destino sin padre se rechaza.
#[test]
fn schedule_fails_when_powershell_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let target = make_target(root.path());
    let own = avi_process::current_owner_token().unwrap();

    let missing = root.path().join("no-existe").join("powershell.exe");
    let err = avi_process::schedule_removal_with(&missing, &target, &own)
        .expect_err("sin powershell no se puede programar");
    assert!(err.to_string().contains("no se pudo lanzar"), "{err}");
    assert!(target.join("a.txt").exists(), "el destino queda intacto");
    assert_eq!(entries(root.path()), vec!["victim".to_string()]);

    let err = avi_process::schedule_removal(Path::new("C:\\"), &own)
        .expect_err("un destino sin padre se rechaza");
    assert!(err.to_string().contains("directorio padre"), "{err}");
}
