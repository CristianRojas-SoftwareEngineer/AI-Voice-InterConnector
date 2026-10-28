// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! `xtask bootstrap`: prepara o pone al día el entorno de desarrollo.
//!
//! Convergente: la misma orden prepara el entorno la primera vez y lo pone al
//! día tras un `git pull`; si nada cambió, no hace trabajo. Los
//! 6 pasos se ejecutan en orden y cada uno es no-op cuando su estado ya
//! converge. `--system` es la única vía que ejecuta gestores con `sudo`/UAC,
//! y solo por petición explícita (flag más confirmación). La versión de ONNX
//! sale de los pines y su aseguramiento reutiliza `ensure_ort_bundle` de
//! `package.rs`: `bootstrap` no implementa su propia descarga.
//! `--models` provisiona con `setup` del build local. Sin `rustup` (caso WSL),
//! el paso 2 indica la instalación antes de fallar. Sin red en los tests:
//! las decisiones son funciones puras sobre fixtures.

use anyhow::{bail, Result};
use std::io::{BufRead, Write};

/// Componentes de Rust que aseguran las puertas (`fmt --check`, clippy). La
/// versión ya la aplica `rust-toolchain.toml`.
const REQUIRED_COMPONENTS: &[&str] = &["clippy", "rustfmt"];

/// Una acción del sistema: ejecutable por proceso o solo manual (prosa que
/// ningún gestor puede correr, como instalar rustup a mano).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SystemAction {
    Run { program: String, args: Vec<String> },
    Manual(String),
}

/// Planifica los comandos del sistema a partir de las pistas de `doctor`:
/// las que empiezan por un ejecutable conocido se ejecutan, la prosa se
/// informa. Puro: sin `--system` nada se ejecuta (lo fija el llamador).
pub(crate) fn plan_system_commands(report: &super::doctor::Report) -> Vec<(String, SystemAction)> {
    let mut out = Vec::new();
    for row in &report.rows {
        if row.status == super::doctor::Status::Ok {
            continue;
        }
        let Some(hint) = &row.hint else { continue };
        let action = executable_hint(hint).map_or_else(
            || SystemAction::Manual(hint.clone()),
            |(program, args)| SystemAction::Run { program, args },
        );
        out.push((row.id.to_string(), action));
    }
    out
}

/// `Some((programa, args))` si la pista es un comando ejecutable (`sudo …`,
/// `winget …`, `brew …`, `xcode-select …`, `pacman …`, `cargo …`); `None` si
/// es prosa manual o una delegación a este mismo comando (`cargo xtask …`).
fn executable_hint(hint: &str) -> Option<(String, Vec<String>)> {
    let mut parts = hint.split_whitespace();
    let first = parts.next()?;
    let executable = matches!(
        first,
        "sudo" | "winget" | "brew" | "xcode-select" | "pacman" | "cargo"
    );
    if !executable || hint.starts_with("cargo xtask") {
        return None;
    }
    Some((first.to_string(), parts.map(str::to_string).collect()))
}

/// Puerta de `--system`: exige petición explícita (flag) y confirmación
/// (`--yes` o TTY). Pura sobre flags ya leídos, testeable sin procesos.
pub(crate) fn check_system_gate(system: bool, yes: bool, is_tty: bool) -> Result<bool> {
    if !system {
        return Ok(false);
    }
    if !yes && !is_tty {
        bail!("--system sin TTY: confirma con --yes (solo entonces se ejecuta con sudo/UAC)");
    }
    Ok(true)
}

/// Componentes ausentes en la salida de `rustup component list --installed`.
/// Pura: la lista instalada entra como texto (fixture en tests).
pub(crate) fn missing_components(installed: &str, wanted: &[&str]) -> Vec<String> {
    wanted
        .iter()
        .filter(|w| {
            !installed
                .lines()
                .any(|l| l.trim_start().starts_with(&format!("{w}-")))
        })
        .map(|w| (*w).to_string())
        .collect()
}

/// Punto de entrada de `xtask bootstrap`.
pub fn run(system: bool, models: bool, yes: bool) -> Result<()> {
    use std::io::{IsTerminal, Write};
    let root = std::env::current_dir()?;
    let pins = super::doctor::load_pins(&root)?;
    let mut worked = false;

    // Paso 1. `doctor`: imprime los comandos que falten; con `--system` y
    // confirmación explícita, los ejecuta (única vía con sudo/UAC).
    let report = super::doctor::check(&root, &pins);
    let plan = plan_system_commands(&report);
    if plan.is_empty() {
        println!("Paso 1/6 (doctor): requisitos del sistema al día.");
    } else {
        println!("Paso 1/6 (doctor): faltan requisitos del sistema:");
        for (id, action) in &plan {
            match action {
                SystemAction::Run { program, args } => {
                    println!("  [{id}] {program} {}", args.join(" "));
                }
                SystemAction::Manual(text) => println!("  [{id}] {text}"),
            }
        }
        if check_system_gate(system, yes, std::io::stdin().is_terminal())? {
            if !yes {
                print!("¿Ejecutar los comandos ejecutables con sudo/UAC? [s/N]: ");
                std::io::stdout().flush()?;
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)?;
                let t = input.trim().to_lowercase();
                if !matches!(t.as_str(), "s" | "si" | "sí" | "y" | "yes") {
                    println!("Cancelado: el sistema queda sin tocar.");
                    return Ok(());
                }
            }
            for (id, action) in &plan {
                match action {
                    SystemAction::Run { program, args } => {
                        println!("  Ejecutando [{id}]: {program} {}", args.join(" "));
                        let status = std::process::Command::new(program)
                            .args(args)
                            .status()
                            .map_err(|e| anyhow::anyhow!("no se pudo ejecutar {program}: {e}"))?;
                        if !status.success() {
                            bail!(
                                "el comando del sistema [{id}] falló (exit {:?}): corrígelo a mano y repite `cargo xtask bootstrap`",
                                status.code()
                            );
                        }
                        worked = true;
                    }
                    SystemAction::Manual(text) => {
                        println!("  Manual [{id}]: {text}");
                    }
                }
            }
        } else if !system {
            println!("  (con --system se ejecutan los comandos ejecutables; el resto es manual)");
        }
    }

    // Paso 2. Componentes de Rust. Sin `rustup`, indicar la instalación
    // antes de fallar (caso WSL sin rustup).
    match rustup_installed_list()? {
        None => {
            println!("Paso 2/6 (Rust): sin `rustup` en el PATH.");
            bail!(
                "instala rustup primero (https://rustup.rs) y repite `cargo xtask bootstrap`: rust-toolchain.toml aplicará la versión {} sola",
                pins.rust
            );
        }
        Some(installed) => {
            let missing = missing_components(&installed, REQUIRED_COMPONENTS);
            if missing.is_empty() {
                println!("Paso 2/6 (Rust): componentes al día (clippy, rustfmt).");
            } else {
                println!("Paso 2/6 (Rust): añadiendo {} …", missing.join(", "));
                let status = std::process::Command::new("rustup")
                    .arg("component")
                    .arg("add")
                    .args(&missing)
                    .status()?;
                if !status.success() {
                    bail!("`rustup component add` falló (exit {:?})", status.code());
                }
                worked = true;
            }
        }
    }

    // Paso 3. ONNX Runtime en la versión fijada (reutiliza si está fresco).
    let triple = super::package::host_triple()?;
    super::package::ensure_ort_bundle(&root, triple, &pins)?;
    println!(
        "Paso 3/6 (ONNX Runtime {}): asegurado en ort-bundle/.",
        pins.ort
    );

    // Paso 4. Motor TTS si falta o está desactualizado.
    if super::doctor::engine_is_fresh(&root)? {
        println!("Paso 4/6 (motor TTS): al día, se omite la compilación.");
    } else {
        println!("Paso 4/6 (motor TTS): compilando (build-engine --self-test) …");
        let xtask = std::env::current_exe()?;
        let status = std::process::Command::new(&xtask)
            .args(["build-engine", "--self-test"])
            .status()?;
        if !status.success() {
            bail!("la compilación del motor falló (exit {:?})", status.code());
        }
        worked = true;
    }

    // Paso 5. Modelos solo con `--models`, vía `setup` del build local.
    if !models {
        println!("Paso 5/6 (modelos): omitido (pídelo con --models).");
    } else if super::doctor::models_are_provisioned() {
        println!("Paso 5/6 (modelos): ya provisionados, se omite `setup`.");
    } else {
        println!("Paso 5/6 (modelos): ejecutando `setup` del build local …");
        run_setup_with_watchdog(&root)?;
        worked = true;
    }

    // Paso 6. Resumen del estado alcanzado.
    if worked {
        println!("Paso 6/6: entorno puesto al día.");
    } else {
        println!("Paso 6/6: sin trabajo, el entorno ya estaba convergente.");
    }
    Ok(())
}

/// Silencio máximo tolerado del `setup` hijo antes de abortarlo.
///
/// Es un plazo **sin progreso**, no un plazo total: una transferencia lenta puede
/// durar horas y sigue viva mientras hf-hub emita latidos con bytes completados.
const SETUP_SILENCE_LIMIT: std::time::Duration = std::time::Duration::from_secs(600);

/// Cada cuánto se mira el rastro en disco del `setup` hijo.
const SETUP_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

/// Lanza el `setup` del build local y lo acota.
///
/// El binario se ejecuta directamente, no por `cargo run`: así el proceso que se
/// acota y se mata es el propio `setup`, y no un `cargo` intermedio cuyo hijo
/// seguiría descargando después del `kill`.
///
/// La compilación previa se acota al paquete del producto porque `default-members`
/// incluye todo el workspace; sin `--package`, Cargo intentaría reenlazar
/// `xtask.exe` mientras está en uso. En Windows eso falla con «Acceso denegado».
fn run_setup_with_watchdog(root: &std::path::Path) -> Result<()> {
    let build = std::process::Command::new("cargo")
        .args(["build", "--quiet", "--package", "ai-voice-interconnector"])
        .current_dir(root)
        .stderr(std::process::Stdio::null())
        .status()?;
    if !build.success() {
        bail!(
            "la compilación del binario local falló (exit {:?})",
            build.code()
        );
    }
    let exe = root.join("target").join("debug").join(if cfg!(windows) {
        "ai-voice-interconnector.exe"
    } else {
        "ai-voice-interconnector"
    });
    let mut child = std::process::Command::new(&exe)
        .arg("setup")
        .current_dir(root)
        .env("AVI_SETUP_WATCHDOG", "1")
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| anyhow::anyhow!("no se pudo lanzar '{}': {e}", exe.display()))?;
    watch_child(&mut child, SETUP_SILENCE_LIMIT, SETUP_POLL_INTERVAL)
}

/// Espera a `child` mientras el `setup` emita latidos por el canal reservado.
///
/// La longitud de los blobs no es una señal fiable: hf-hub puede preextenderlos
/// antes de transferir y seguir escribiendo con el tamaño de árbol estable. El
/// hijo comunica progreso agregado de bytes por stderr; cualquier otra salida se
/// conserva como diagnóstico normal. Los plazos entran por parámetro para que las
/// pruebas no esperen los diez minutos reales.
fn watch_child(
    child: &mut std::process::Child,
    silence_limit: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<()> {
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow::anyhow!("`setup` se lanzó sin canal stderr"))?;
    let (sender, receiver) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr).lines() {
            let Ok(line) = line else { break };
            if let Some(payload) = line.strip_prefix("AVI_SETUP_HEARTBEAT ") {
                let _ = sender.send(payload.to_owned());
            } else {
                let _ = std::io::stderr().write_all(line.as_bytes());
                let _ = std::io::stderr().write_all(b"\n");
            }
        }
    });

    let mut last_progress = std::time::Instant::now();
    loop {
        loop {
            match receiver.try_recv() {
                Ok(_) => last_progress = std::time::Instant::now(),
                Err(std::sync::mpsc::TryRecvError::Empty)
                | Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        if let Some(status) = child.try_wait()? {
            if !status.success() {
                bail!("`setup` falló (exit {:?})", status.code());
            }
            return Ok(());
        }
        match receiver.recv_timeout(poll_interval) {
            Ok(_) => last_progress = std::time::Instant::now(),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(poll_interval);
            }
        }
        if last_progress.elapsed() >= silence_limit {
            if let Err(error) = child.kill() {
                eprintln!("aviso: no se pudo matar el `setup` colgado: {error}");
            }
            if let Err(error) = child.wait() {
                eprintln!("aviso: no se pudo recoger el `setup` abortado: {error}");
            }
            bail!(
                "`setup` no informó avance de bytes durante {:?}: se abortó para no esperar indefinidamente a un flujo mudo; `setup --force-update --yes` repara el estado parcial",
                silence_limit
            );
        }
    }
}

/// Salida de `rustup component list --installed`; `None` si no hay `rustup`.
fn rustup_installed_list() -> Result<Option<String>> {
    let out = std::process::Command::new("rustup")
        .args(["component", "list", "--installed"])
        .output();
    let Ok(out) = out else {
        return Ok(None);
    };
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doctor::{Report, Row, Status};

    /// Hijo que tarda mucho y no informa latidos: el vigilante tiene que abortarlo.
    /// Se lanza el binario directo, sin `cmd /C`, para que el `kill` alcance al
    /// proceso que se mide y no quede un huérfano.
    fn slow_silent_child() -> std::process::Child {
        let mut command = if cfg!(windows) {
            let mut c = std::process::Command::new("ping");
            c.args(["-n", "30", "127.0.0.1"]);
            c
        } else {
            let mut c = std::process::Command::new("sleep");
            c.arg("30");
            c
        };
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("se lanza el hijo lento de la prueba")
    }

    /// Un hijo sin latidos de progreso se aborta y queda terminado: el vigilante
    /// no espera indefinidamente a un flujo mudo.
    #[test]
    fn watchdog_aborts_a_child_without_progress_heartbeats() {
        let mut child = slow_silent_child();
        let error = watch_child(
            &mut child,
            std::time::Duration::from_millis(200),
            std::time::Duration::from_millis(20),
        )
        .expect_err("un hijo que no informa progreso se aborta");
        assert!(
            error.to_string().contains("no informó avance de bytes"),
            "el error nombra el silencio, no otra causa: {error}"
        );
        assert!(
            child
                .try_wait()
                .expect("se puede consultar el hijo")
                .is_some(),
            "el hijo abortado queda terminado, no huérfano"
        );
    }

    /// Hijo que termina de inmediato con `code`.
    fn spawn_exit_child(code: i32) -> std::process::Child {
        let script = format!("exit {code}");
        let mut command = if cfg!(windows) {
            std::process::Command::new("cmd")
        } else {
            std::process::Command::new("sh")
        };
        if cfg!(windows) {
            command.args(["/C", &script]);
        } else {
            command.args(["-c", &script]);
        }
        command
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("se lanza el hijo de la prueba")
    }

    /// Un hijo que termina conserva su veredicto: éxito no se toca y el fallo se
    /// propaga con su código.
    #[test]
    fn watchdog_reports_child_exit_status() {
        let limits = (
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(20),
        );
        let mut ok = spawn_exit_child(0);
        assert!(watch_child(&mut ok, limits.0, limits.1).is_ok());

        let mut failing = spawn_exit_child(3);
        assert!(watch_child(&mut failing, limits.0, limits.1).is_err());
    }

    /// Hijo que emite latidos aunque mantenga igual el tamaño de un archivo.
    fn heartbeat_child(duration_ms: u64) -> std::process::Child {
        let exe = std::env::current_exe().expect("se resuelve el binario de prueba");
        std::process::Command::new(exe)
            .args([
                "--exact",
                "bootstrap::tests::heartbeat_child_process",
                "--nocapture",
            ])
            .env("AVI_SETUP_HEARTBEAT_CHILD", duration_ms.to_string())
            .stderr(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("se lanza el hijo de latidos de prueba")
    }

    #[test]
    fn heartbeat_child_process() {
        let Ok(duration) = std::env::var("AVI_SETUP_HEARTBEAT_CHILD") else {
            return;
        };
        let duration = duration.parse::<u64>().expect("duración entera");
        let dir = tempfile::tempdir().expect("directorio temporal del archivo preextendido");
        let path = dir.path().join("blob.incomplete");
        let file = std::fs::File::create(path).expect("se crea el blob");
        file.set_len(1024 * 1024).expect("se preextiende el blob");
        let end = std::time::Instant::now() + std::time::Duration::from_millis(duration);
        let mut completed = 0;
        while std::time::Instant::now() < end {
            completed += 1;
            eprintln!("AVI_SETUP_HEARTBEAT {completed} 1000000");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    /// Una transferencia válida no se aborta aunque no se mueva el tamaño del árbol.
    #[test]
    fn watchdog_keeps_child_alive_on_byte_heartbeats() {
        let mut child = heartbeat_child(350);
        watch_child(
            &mut child,
            std::time::Duration::from_millis(100),
            std::time::Duration::from_millis(20),
        )
        .expect("los latidos mantienen vivo el setup aunque el tamaño no cambie");
    }

    /// Informe de fixture: una fila ejecutable, una manual y una correcta.
    fn fixture_report() -> Report {
        Report {
            rows: vec![
                Row {
                    id: "cmake",
                    label: "CMake",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("≥ 3.20".to_string()),
                    hint: Some("sudo apt install cmake".to_string()),
                },
                Row {
                    id: "rust",
                    label: "Rust",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("1.96.0".to_string()),
                    hint: Some(
                        "instala rustup (https://rustup.rs): rust-toolchain.toml aplica la versión sola"
                            .to_string(),
                    ),
                },
                Row {
                    id: "onnx",
                    label: "ONNX",
                    mandatory: true,
                    status: Status::Missing,
                    found: None,
                    want: Some("1.28.0".to_string()),
                    hint: Some("cargo xtask bootstrap".to_string()),
                },
                Row {
                    id: "sccache",
                    label: "sccache",
                    mandatory: false,
                    status: Status::Ok,
                    found: Some("0.8.2".to_string()),
                    want: Some("cualquiera".to_string()),
                    hint: None,
                },
            ],
            engine_drift: None,
            probe_errors: Vec::new(),
            onnx_drift: None,
            models_drift: Vec::new(),
        }
    }

    /// El plan distingue ejecutables, manuales y delegaciones propias, y
    /// omite las filas correctas: sin `--system` nada se ejecuta.
    #[test]
    fn plan_classifies_without_executing() {
        let plan = plan_system_commands(&fixture_report());
        assert_eq!(plan.len(), 3);
        assert_eq!(
            plan[0],
            (
                "cmake".to_string(),
                SystemAction::Run {
                    program: "sudo".to_string(),
                    args: ["apt", "install", "cmake"]
                        .iter()
                        .map(|s| (*s).to_string())
                        .collect(),
                }
            )
        );
        assert!(matches!(plan[1].1, SystemAction::Manual(_)));
        // `cargo xtask …` es delegación propia: manual, nunca ejecutable.
        assert!(matches!(plan[2].1, SystemAction::Manual(_)));
        // La fila correcta (sccache) no entra en el plan.
        assert!(!plan.iter().any(|(id, _)| id == "sccache"));
    }

    /// `--system` exige petición explícita y confirmación (o TTY).
    #[test]
    fn system_gate_requires_explicit_request() {
        // Sin --system: no se ejecuta nada, sin error.
        assert!(!check_system_gate(false, false, false).unwrap());
        assert!(!check_system_gate(false, true, true).unwrap());
        // Con --system y --yes: adelante.
        assert!(check_system_gate(true, true, false).unwrap());
        // Con --system y TTY: adelante (confirmará en terminal).
        assert!(check_system_gate(true, false, true).unwrap());
        // Con --system, sin --yes y sin TTY: falla antes de tocar nada.
        assert!(check_system_gate(true, false, false).is_err());
    }

    /// Convergencia de componentes: la segunda ejecución no añade nada.
    #[test]
    fn missing_components_is_convergent() {
        let installed = "cargo-x86_64-pc-windows-msvc\n\
             clippy-x86_64-pc-windows-msvc\n\
             rustfmt-x86_64-pc-windows-msvc\n";
        assert!(missing_components(installed, REQUIRED_COMPONENTS).is_empty());
        let installed = "cargo-x86_64-pc-windows-msvc\nclippy-x86_64-pc-windows-msvc\n";
        assert_eq!(
            missing_components(installed, REQUIRED_COMPONENTS),
            ["rustfmt".to_string()]
        );
    }

    /// Pistas no ejecutables (prosa, delegaciones) quedan como manuales.
    #[test]
    fn executable_hint_parsing() {
        assert!(executable_hint("instala rustup a mano").is_none());
        assert!(executable_hint("cargo xtask bootstrap").is_none());
        let (program, args) = executable_hint("winget install Kitware.CMake").unwrap();
        assert_eq!(program, "winget");
        assert_eq!(args, ["install", "Kitware.CMake"]);
    }
}
