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
use std::io::{Read, Write};

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
#[derive(Debug)]
enum WatchEvent {
    Heartbeat(std::time::Instant),
    Eof,
    ReadError(String),
    ForwardError(String),
}

#[derive(Debug, PartialEq, Eq)]
enum WatchExit {
    Success,
    Failure(Option<i32>),
}

trait WatchRuntime {
    fn now(&self) -> std::time::Instant;
    fn event(&mut self) -> Result<Option<WatchEvent>>;
    fn wait_event(&mut self, duration: std::time::Duration) -> Result<Option<WatchEvent>>;
    fn child_status(&mut self) -> Result<Option<WatchExit>>;
    fn kill(&mut self) -> Result<()>;
    fn wait_child(&mut self) -> Result<()>;
    fn note_forward_error(&mut self, error: String);
}

fn watch_exit(status: WatchExit) -> Result<()> {
    match status {
        WatchExit::Success => Ok(()),
        WatchExit::Failure(code) => bail!("`setup` falló (exit {code:?})"),
    }
}

enum WatchAbort {
    Timeout(std::time::Duration),
    ReadError(String),
    StatusError(String),
}

fn abort_watched_child(runtime: &mut impl WatchRuntime, cause: WatchAbort) -> Result<()> {
    let timeout = matches!(cause, WatchAbort::Timeout(_));
    let message = match cause {
        WatchAbort::Timeout(limit) => {
            format!("`setup` no informó avance de bytes durante {limit:?}")
        }
        WatchAbort::ReadError(error) => format!("no se pudo leer stderr de `setup`: {error}"),
        WatchAbort::StatusError(error) => {
            format!("no se pudo comprobar el estado de `setup`: {error}")
        }
    };
    if let Err(error) = runtime.kill() {
        match runtime.child_status() {
            Ok(Some(status)) => {
                if timeout {
                    return watch_exit(status);
                }
                bail!("{message}; el hijo terminó antes del aborto: {status:?}");
            }
            Ok(None) => bail!("{message}; no se pudo matar el `setup`: {error}"),
            Err(check) => bail!("{message}; no se pudo matar el `setup`: {error}; tampoco se pudo comprobar su estado: {check}"),
        }
    }
    if let Err(error) = runtime.wait_child() {
        bail!("{message}; no se pudo recoger el `setup` abortado: {error}");
    }
    if timeout {
        bail!("{message}: se abortó para no esperar indefinidamente a un flujo mudo; `setup --force-update --yes` repara el estado parcial");
    }
    bail!("{message}");
}

fn checked_watch_status(runtime: &mut impl WatchRuntime) -> Result<Option<WatchExit>> {
    match runtime.child_status() {
        Ok(status) => Ok(status),
        Err(error) => {
            abort_watched_child(runtime, WatchAbort::StatusError(error.to_string()))?;
            Ok(None)
        }
    }
}

fn process_watch_event(
    runtime: &mut impl WatchRuntime,
    event: WatchEvent,
    last_progress: &mut std::time::Instant,
) -> Result<()> {
    match event {
        WatchEvent::Heartbeat(observed) => *last_progress = (*last_progress).max(observed),
        WatchEvent::Eof => {}
        WatchEvent::ReadError(error) => {
            return abort_watched_child(runtime, WatchAbort::ReadError(error));
        }
        WatchEvent::ForwardError(error) => {
            runtime.note_forward_error(error);
        }
    }
    Ok(())
}

fn supervise_child(
    runtime: &mut impl WatchRuntime,
    silence_limit: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<()> {
    let mut last_progress = runtime.now();
    loop {
        for _ in 0..4 {
            let Some(event) = runtime.event()? else { break };
            process_watch_event(runtime, event, &mut last_progress)?;
        }
        if let Some(status) = checked_watch_status(runtime)? {
            return watch_exit(status);
        }
        let elapsed = runtime.now().saturating_duration_since(last_progress);
        if elapsed >= silence_limit {
            if let Some(event) = runtime.event()? {
                process_watch_event(runtime, event, &mut last_progress)?;
                continue;
            }
            if let Some(status) = checked_watch_status(runtime)? {
                return watch_exit(status);
            }
            return abort_watched_child(runtime, WatchAbort::Timeout(silence_limit));
        }
        let remaining = silence_limit.saturating_sub(elapsed);
        if let Some(event) = runtime.wait_event(poll_interval.min(remaining))? {
            process_watch_event(runtime, event, &mut last_progress)?;
        }
    }
}

struct ChannelWatch<'a> {
    child: &'a mut std::process::Child,
    receiver: std::sync::mpsc::Receiver<()>,
    signals: std::sync::Arc<std::sync::Mutex<ReaderSignals>>,
    forward_error: Option<String>,
    forward_reported: bool,
    diagnostics: std::sync::mpsc::SyncSender<String>,
}

#[derive(Default)]
struct ReaderSignals {
    heartbeat: Option<std::time::Instant>,
    eof: bool,
    read_error: Option<String>,
    forward_error: Option<String>,
}

#[derive(Clone)]
struct ReaderNotifier {
    signals: std::sync::Arc<std::sync::Mutex<ReaderSignals>>,
    sender: std::sync::mpsc::SyncSender<()>,
}

impl ReaderNotifier {
    fn send(&self, event: WatchEvent) {
        let mut signals = self.signals.lock().unwrap_or_else(|e| e.into_inner());
        match event {
            WatchEvent::Heartbeat(at) => signals.heartbeat = Some(at),
            WatchEvent::Eof => signals.eof = true,
            WatchEvent::ReadError(error) => {
                signals.read_error.get_or_insert(error);
            }
            WatchEvent::ForwardError(error) => {
                signals.forward_error.get_or_insert(error);
            }
        }
        drop(signals);
        drop(self.sender.try_send(()).map_err(|error| error.to_string()));
    }
}

impl ChannelWatch<'_> {
    fn report_forward_error(&mut self) {
        if self.forward_reported {
            return;
        }
        if let Some(error) = &self.forward_error {
            let message =
                format!("aviso: no se pudieron mostrar todos los diagnósticos de `setup`: {error}");
            match self.diagnostics.try_send(message) {
                Ok(()) | Err(std::sync::mpsc::TrySendError::Disconnected(_)) => {
                    self.forward_reported = true
                }
                Err(std::sync::mpsc::TrySendError::Full(_)) => {}
            }
        }
    }

    fn take_event(&mut self) -> Option<WatchEvent> {
        let mut signals = self.signals.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(error) = signals.read_error.take() {
            return Some(WatchEvent::ReadError(error));
        }
        if let Some(at) = signals.heartbeat.take() {
            return Some(WatchEvent::Heartbeat(at));
        }
        if let Some(error) = signals.forward_error.take() {
            return Some(WatchEvent::ForwardError(error));
        }
        if std::mem::take(&mut signals.eof) {
            return Some(WatchEvent::Eof);
        }
        None
    }
}

impl WatchRuntime for ChannelWatch<'_> {
    fn now(&self) -> std::time::Instant {
        std::time::Instant::now()
    }

    fn event(&mut self) -> Result<Option<WatchEvent>> {
        self.report_forward_error();
        Ok(self.take_event())
    }

    fn wait_event(&mut self, duration: std::time::Duration) -> Result<Option<WatchEvent>> {
        self.report_forward_error();
        match self.receiver.recv_timeout(duration) {
            Ok(()) => Ok(self.take_event()),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                std::thread::sleep(duration);
                Ok(None)
            }
        }
    }

    fn child_status(&mut self) -> Result<Option<WatchExit>> {
        Ok(self.child.try_wait()?.map(|status| {
            if status.success() {
                WatchExit::Success
            } else {
                WatchExit::Failure(status.code())
            }
        }))
    }

    fn kill(&mut self) -> Result<()> {
        Ok(self.child.kill()?)
    }

    fn wait_child(&mut self) -> Result<()> {
        self.child.wait()?;
        Ok(())
    }

    fn note_forward_error(&mut self, error: String) {
        if self.forward_error.is_none() {
            self.forward_error = Some(error);
        }
        self.report_forward_error();
    }
}

struct PipeReader {
    stopped: std::sync::Arc<std::sync::atomic::AtomicBool>,
    done: std::sync::mpsc::Receiver<()>,
    thread: std::thread::JoinHandle<()>,
    #[cfg(unix)]
    cancel: std::os::unix::net::UnixStream,
    #[cfg(windows)]
    cancel: std::os::windows::io::OwnedHandle,
}

fn join_finished_thread(thread: std::thread::JoinHandle<()>, name: &str) -> Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !thread.is_finished() {
        if std::time::Instant::now() >= deadline {
            bail!("no se pudo confirmar la finalización de {name}");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    thread
        .join()
        .map_err(|_| anyhow::anyhow!("{name} terminó con pánico"))
}

#[cfg(unix)]
fn wait_for_pipe(
    stderr: &std::process::ChildStderr,
    cancel: &std::os::unix::net::UnixStream,
) -> std::io::Result<bool> {
    use std::os::unix::io::AsRawFd;
    let mut fds = [
        libc::pollfd {
            fd: stderr.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: cancel.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // Los descriptores permanecen abiertos y el arreglo cubre las dos entradas.
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if result < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        if fds[1].revents != 0 {
            return Ok(false);
        }
        if fds[0].revents & libc::POLLNVAL != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "descriptor de stderr inválido",
            ));
        }
        if fds[0].revents != 0 {
            return Ok(true);
        }
    }
}

fn publish_line(
    line: &[u8],
    notifier: &ReaderNotifier,
    diagnostics: &std::sync::mpsc::SyncSender<String>,
) -> Result<()> {
    let line = std::str::from_utf8(line)?.trim_end_matches('\r');
    if line.starts_with("AVI_SETUP_HEARTBEAT ") {
        notifier.send(WatchEvent::Heartbeat(std::time::Instant::now()));
    } else if let Err(error) = diagnostics.try_send(line.to_owned()) {
        notifier.send(WatchEvent::ForwardError(format!(
            "no se pudo encolar un diagnóstico: {error}"
        )));
    }
    Ok(())
}

impl PipeReader {
    fn start(
        stderr: std::process::ChildStderr,
        notifier: ReaderNotifier,
        diagnostics: std::sync::mpsc::SyncSender<String>,
    ) -> Result<Self> {
        let stopped = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let thread_stopped = stopped.clone();
        let (done_tx, done) = std::sync::mpsc::channel();
        #[cfg(unix)]
        let (cancel_reader, cancel) = std::os::unix::net::UnixStream::pair()?;
        #[cfg(windows)]
        let (stderr, cancel) = {
            use std::os::windows::io::OwnedHandle;
            let owned: OwnedHandle = stderr.into();
            let cancel = owned.try_clone()?;
            (std::process::ChildStderr::from(owned), cancel)
        };
        let thread = std::thread::Builder::new()
            .name("setup-stderr".into())
            .spawn(move || {
                let mut stderr = stderr;
                let mut pending = Vec::new();
                let mut buffer = [0_u8; 4096];
                loop {
                    if thread_stopped.load(std::sync::atomic::Ordering::Acquire) {
                        break;
                    }
                    #[cfg(unix)]
                    match wait_for_pipe(&stderr, &cancel_reader) {
                        Ok(true) => {}
                        Ok(false) => break,
                        Err(error) => {
                            notifier.send(WatchEvent::ReadError(error.to_string()));
                            break;
                        }
                    }
                    match stderr.read(&mut buffer) {
                        Ok(0) => {
                            if !pending.is_empty() {
                                if let Err(error) = publish_line(&pending, &notifier, &diagnostics)
                                {
                                    notifier.send(WatchEvent::ReadError(error.to_string()));
                                    break;
                                }
                            }
                            notifier.send(WatchEvent::Eof);
                            break;
                        }
                        Ok(count) => {
                            pending.extend_from_slice(&buffer[..count]);
                            while let Some(end) = pending.iter().position(|byte| *byte == b'\n') {
                                let line: Vec<u8> = pending.drain(..=end).collect();
                                if let Err(error) =
                                    publish_line(&line[..line.len() - 1], &notifier, &diagnostics)
                                {
                                    notifier.send(WatchEvent::ReadError(error.to_string()));
                                    drop(done_tx.send(()).map_err(|error| error.to_string()));
                                    return;
                                }
                            }
                        }
                        Err(error) if thread_stopped.load(std::sync::atomic::Ordering::Acquire) => {
                            let _ = error;
                            break;
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            notifier.send(WatchEvent::ReadError(error.to_string()));
                            break;
                        }
                    }
                }
                drop(done_tx.send(()).map_err(|error| error.to_string()));
            })?;
        Ok(Self {
            stopped,
            done,
            thread,
            cancel,
        })
    }

    fn finish(self) -> Result<()> {
        match self.done.recv_timeout(std::time::Duration::from_secs(2)) {
            Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                join_finished_thread(self.thread, "el lector de stderr")?;
                return Ok(());
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
        self.stopped
            .store(true, std::sync::atomic::Ordering::Release);
        #[cfg(unix)]
        let cancel_error = (&self.cancel).write_all(b"x").err();
        #[cfg(windows)]
        let mut cancel_error: Option<std::io::Error> = None;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        loop {
            #[cfg(windows)]
            {
                use std::os::windows::io::AsRawHandle;
                // El handle duplicado sigue abierto mientras se solicita la cancelación.
                let result = unsafe {
                    windows_sys::Win32::System::IO::CancelIoEx(
                        self.cancel.as_raw_handle() as isize,
                        std::ptr::null(),
                    )
                };
                if result == 0 {
                    let error = std::io::Error::last_os_error();
                    if error.raw_os_error()
                        != Some(windows_sys::Win32::Foundation::ERROR_NOT_FOUND as i32)
                    {
                        cancel_error.get_or_insert(error);
                    }
                }
            }
            match self.done.recv_timeout(std::time::Duration::from_millis(20)) {
                Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    join_finished_thread(self.thread, "el lector de stderr")?;
                    return Ok(());
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    if std::time::Instant::now() < deadline => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(error) = cancel_error {
                        bail!("no se pudo cancelar el lector de stderr: {error}");
                    }
                    bail!("no se pudo confirmar la finalización del lector de stderr");
                }
            }
        }
    }
}

fn fail_to_start_reader(child: &mut std::process::Child, cause: &str) -> anyhow::Error {
    match child.kill() {
        Ok(()) => match child.wait() {
            Ok(_) => anyhow::anyhow!("{cause}; el hijo se terminó y recogió"),
            Err(error) => anyhow::anyhow!("{cause}; no se pudo recoger el hijo: {error}"),
        },
        Err(error) => match child.try_wait() {
            Ok(Some(status)) => anyhow::anyhow!("{cause}; el hijo ya terminó: {status}"),
            Ok(None) => anyhow::anyhow!("{cause}; no se pudo terminar el hijo: {error}"),
            Err(check) => anyhow::anyhow!("{cause}; no se pudo terminar el hijo: {error}; tampoco se pudo comprobar su estado: {check}"),
        },
    }
}

fn forward_diagnostics(
    mut writer: impl Write,
    output: std::sync::mpsc::Receiver<String>,
    notifier: ReaderNotifier,
) {
    let mut can_forward = true;
    for line in output {
        if can_forward {
            if let Err(error) = writeln!(writer, "{line}") {
                can_forward = false;
                notifier.send(WatchEvent::ForwardError(error.to_string()));
            }
        }
    }
}

fn watch_child(
    child: &mut std::process::Child,
    silence_limit: std::time::Duration,
    poll_interval: std::time::Duration,
) -> Result<()> {
    let stderr = match child.stderr.take() {
        Some(stderr) => stderr,
        None => {
            return Err(fail_to_start_reader(
                child,
                "`setup` se lanzó sin canal stderr",
            ))
        }
    };
    let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let notifier = ReaderNotifier {
        signals: signals.clone(),
        sender,
    };
    let (diagnostics, output) = std::sync::mpsc::sync_channel::<String>(32);
    let output_notifier = notifier.clone();
    let (output_done_tx, output_done) = std::sync::mpsc::channel();
    let reader = match PipeReader::start(stderr, notifier, diagnostics.clone()) {
        Ok(reader) => reader,
        Err(error) => {
            return Err(fail_to_start_reader(
                child,
                &format!("no se pudo iniciar el lector de stderr: {error}"),
            ))
        }
    };
    let output_thread = std::thread::Builder::new()
        .name("setup-diagnostics".into())
        .spawn(move || {
            forward_diagnostics(std::io::stderr(), output, output_notifier);
            drop(output_done_tx.send(()).map_err(|error| error.to_string()));
        });
    let output_thread = match output_thread {
        Ok(thread) => thread,
        Err(error) => {
            let cause = fail_to_start_reader(
                child,
                &format!("no se pudo iniciar el reenviador de stderr: {error}"),
            );
            return match reader.finish() {
                Ok(()) => Err(cause),
                Err(cleanup) => Err(anyhow::anyhow!("{cause}; {cleanup}")),
            };
        }
    };
    let mut runtime = ChannelWatch {
        child,
        receiver,
        signals,
        forward_error: None,
        forward_reported: false,
        diagnostics,
    };
    let result = supervise_child(&mut runtime, silence_limit, poll_interval);
    let reader_result = reader.finish();
    let (pending_error, pending_forward) = {
        let mut signals = runtime.signals.lock().unwrap_or_else(|e| e.into_inner());
        (signals.read_error.take(), signals.forward_error.take())
    };
    if let Some(error) = pending_forward {
        runtime.note_forward_error(error);
    }
    let warning_deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while runtime.forward_error.is_some()
        && !runtime.forward_reported
        && std::time::Instant::now() < warning_deadline
    {
        runtime.report_forward_error();
        if !runtime.forward_reported {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    drop(runtime);
    let output_result = match output_done.recv_timeout(std::time::Duration::from_secs(2)) {
        Ok(()) | Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            join_finished_thread(output_thread, "el reenviador de stderr")
        }
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(anyhow::anyhow!(
            "no se pudo confirmar la finalización del reenviador de stderr"
        )),
    };
    if let Err(error) = reader_result {
        bail!("resultado de setup: {result:?}; {error}");
    }
    if let Err(error) = output_result {
        bail!("resultado de setup: {result:?}; {error}");
    }
    if let Some(error) = pending_error {
        bail!("resultado de setup: {result:?}; no se pudo leer stderr de `setup`: {error}");
    }
    result
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

    struct ScriptedWatch {
        now: std::time::Instant,
        events: std::collections::VecDeque<Option<WatchEvent>>,
        waits: std::collections::VecDeque<Option<WatchEvent>>,
        statuses: std::collections::VecDeque<Option<WatchExit>>,
        kills: usize,
        reaps: usize,
        kill_error: bool,
        wait_error: bool,
        status_error: bool,
        forward_errors: Vec<String>,
    }

    impl ScriptedWatch {
        fn new() -> Self {
            Self {
                now: std::time::Instant::now(),
                events: Default::default(),
                waits: Default::default(),
                statuses: Default::default(),
                kills: 0,
                reaps: 0,
                kill_error: false,
                wait_error: false,
                status_error: false,
                forward_errors: Vec::new(),
            }
        }
    }

    impl WatchRuntime for ScriptedWatch {
        fn now(&self) -> std::time::Instant {
            self.now
        }
        fn event(&mut self) -> Result<Option<WatchEvent>> {
            Ok(self.events.pop_front().flatten())
        }
        fn wait_event(&mut self, duration: std::time::Duration) -> Result<Option<WatchEvent>> {
            self.now += duration;
            Ok(self.waits.pop_front().flatten())
        }
        fn child_status(&mut self) -> Result<Option<WatchExit>> {
            if self.status_error {
                bail!("fallo de consulta")
            }
            Ok(self.statuses.pop_front().flatten())
        }
        fn kill(&mut self) -> Result<()> {
            self.kills += 1;
            if self.kill_error {
                bail!("fallo de terminación")
            }
            Ok(())
        }
        fn wait_child(&mut self) -> Result<()> {
            self.reaps += 1;
            if self.wait_error {
                bail!("fallo de recogida")
            }
            Ok(())
        }

        fn note_forward_error(&mut self, error: String) {
            self.forward_errors.push(error);
        }
    }

    #[test]
    fn watchdog_scripted_exit_after_wait_does_not_kill() {
        for status in [WatchExit::Success, WatchExit::Failure(Some(3))] {
            let success = status == WatchExit::Success;
            let mut runtime = ScriptedWatch::new();
            runtime.statuses.extend([None, Some(status)]);
            let result = supervise_child(
                &mut runtime,
                std::time::Duration::from_secs(1),
                std::time::Duration::from_secs(1),
            );
            if success {
                assert!(result.is_ok(), "{result:?}");
            } else {
                assert!(result.unwrap_err().to_string().contains("exit Some(3)"));
            }
            assert_eq!(runtime.kills, 0);
        }
    }

    #[test]
    fn watchdog_scripted_timeout_kills_and_reaps() {
        let mut runtime = ScriptedWatch::new();
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("no informó avance de bytes"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 1));
    }

    #[test]
    fn watchdog_scripted_read_failure_kills_and_reaps() {
        let mut runtime = ScriptedWatch::new();
        runtime
            .events
            .push_back(Some(WatchEvent::ReadError("fallo de lectura".into())));
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fallo de lectura"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 1));
    }

    #[test]
    fn watchdog_scripted_failure_to_reap_is_not_reported_as_timeout() {
        let mut runtime = ScriptedWatch::new();
        runtime.wait_error = true;
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fallo de recogida"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 1));
    }

    #[test]
    fn watchdog_scripted_eof_does_not_end_the_child() {
        let mut runtime = ScriptedWatch::new();
        runtime.events.extend([Some(WatchEvent::Eof), None]);
        runtime.statuses.extend([None, Some(WatchExit::Success)]);
        assert!(supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        assert_eq!(runtime.kills, 0);
    }

    #[test]
    fn watchdog_scripted_heartbeat_extends_the_deadline() {
        let mut runtime = ScriptedWatch::new();
        let start = runtime.now;
        runtime.waits.push_back(Some(WatchEvent::Heartbeat(
            start + std::time::Duration::from_secs(1),
        )));
        runtime
            .statuses
            .extend([None, None, Some(WatchExit::Success)]);
        assert!(supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(2),
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        assert_eq!(runtime.kills, 0);
    }

    #[test]
    fn watchdog_scripted_stale_heartbeat_does_not_extend_deadline() {
        let mut runtime = ScriptedWatch::new();
        runtime
            .waits
            .push_back(Some(WatchEvent::Heartbeat(runtime.now)));
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("no informó avance de bytes"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 1));
    }

    #[test]
    fn watchdog_scripted_kill_error_observes_child_exit() {
        let mut runtime = ScriptedWatch::new();
        runtime.kill_error = true;
        runtime
            .statuses
            .extend([None, None, None, Some(WatchExit::Success)]);
        assert!(supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        assert_eq!((runtime.kills, runtime.reaps), (1, 0));
    }

    #[test]
    fn watchdog_scripted_forward_failure_does_not_abort() {
        let mut runtime = ScriptedWatch::new();
        runtime.events.extend([
            Some(WatchEvent::ForwardError("salida cerrada".into())),
            None,
        ]);
        runtime.statuses.push_back(Some(WatchExit::Success));
        assert!(supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        assert_eq!(runtime.kills, 0);
        assert_eq!(runtime.forward_errors, ["salida cerrada"]);
    }

    #[test]
    fn watchdog_scripted_status_error_terminates_and_reaps() {
        let mut runtime = ScriptedWatch::new();
        runtime.status_error = true;
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fallo de consulta"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 1));
    }

    #[test]
    fn watchdog_scripted_final_heartbeat_prevents_timeout() {
        let mut runtime = ScriptedWatch::new();
        let observed = runtime.now + std::time::Duration::from_secs(1);
        runtime
            .events
            .extend([None, None, Some(WatchEvent::Heartbeat(observed)), None]);
        runtime
            .statuses
            .extend([None, None, Some(WatchExit::Success)]);
        assert!(supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1)
        )
        .is_ok());
        assert_eq!(runtime.kills, 0);
    }

    #[test]
    fn watchdog_scripted_kill_failure_with_live_child_is_explicit() {
        let mut runtime = ScriptedWatch::new();
        runtime.kill_error = true;
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("fallo de terminación"));
        assert_eq!((runtime.kills, runtime.reaps), (1, 0));
    }

    #[test]
    fn watchdog_scripted_read_and_kill_errors_are_preserved() {
        let mut runtime = ScriptedWatch::new();
        runtime.kill_error = true;
        runtime
            .events
            .push_back(Some(WatchEvent::ReadError("lectura rota".into())));
        let error = supervise_child(
            &mut runtime,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("lectura rota"), "{message}");
        assert!(message.contains("fallo de terminación"), "{message}");
    }

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

    #[test]
    fn watchdog_missing_stderr_terminates_and_reaps_child() {
        let mut child = slow_silent_child();
        drop(child.stderr.take());
        let error = watch_child(
            &mut child,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(error.to_string().contains("sin canal stderr"), "{error}");
        assert!(
            child.try_wait().unwrap().is_some(),
            "el hijo queda recogido"
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
        let error = watch_child(&mut failing, limits.0, limits.1).unwrap_err();
        assert!(
            error.to_string().contains("falló (exit Some(3))"),
            "{error}"
        );
    }

    /// El hijo recibe órdenes y confirma cada hito sin depender de retardos.
    #[test]
    fn watchdog_controlled_child_process() {
        use std::io::BufRead;
        if std::env::var_os("AVI_WATCHDOG_TEST_CHILD").is_none() {
            return;
        }
        println!("AVI_TEST READY");
        std::io::stdout().flush().unwrap();
        for line in std::io::stdin().lock().lines() {
            match line.unwrap().as_str() {
                "heartbeat" => {
                    eprintln!("AVI_SETUP_HEARTBEAT 1 10");
                    println!("AVI_TEST HEARTBEAT");
                }
                "diagnostic" => {
                    eprintln!("diagnóstico de prueba");
                    println!("AVI_TEST DIAGNOSTIC");
                }
                "invalid" => {
                    std::io::stderr().write_all(&[0xff, b'\n']).unwrap();
                    println!("AVI_TEST INVALID");
                }
                "close" => {
                    #[cfg(unix)]
                    assert_eq!(unsafe { libc::close(2) }, 0);
                    #[cfg(windows)]
                    {
                        let handle = unsafe {
                            windows_sys::Win32::System::Console::GetStdHandle(
                                windows_sys::Win32::System::Console::STD_ERROR_HANDLE,
                            )
                        };
                        assert_ne!(
                            unsafe { windows_sys::Win32::Foundation::CloseHandle(handle) },
                            0
                        );
                    }
                    println!("AVI_TEST CLOSED");
                }
                "exit" => return,
                _ => panic!("orden desconocida"),
            }
            std::io::stdout().flush().unwrap();
        }
    }

    struct ControlledChild {
        child: std::process::Child,
        input: Option<std::process::ChildStdin>,
        output: std::sync::mpsc::Receiver<String>,
    }

    impl ControlledChild {
        fn start() -> Self {
            use std::io::BufRead;
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "bootstrap::tests::watchdog_controlled_child_process",
                    "--nocapture",
                ])
                .env("AVI_WATCHDOG_TEST_CHILD", "1")
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .expect("se lanza el hijo controlado");
            let input = child.stdin.take().unwrap();
            let stdout = child.stdout.take().unwrap();
            let (sender, output) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                for line in std::io::BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.starts_with("AVI_TEST ") && sender.send(line).is_err() {
                        break;
                    }
                }
            });
            let fixture = Self {
                child,
                input: Some(input),
                output,
            };
            fixture.expect("AVI_TEST READY");
            fixture
        }

        fn expect(&self, expected: &str) {
            assert_eq!(
                self.output
                    .recv_timeout(std::time::Duration::from_secs(30))
                    .unwrap(),
                expected
            );
        }

        fn command(&mut self, command: &str, expected: &str) {
            let input = self.input.as_mut().unwrap();
            writeln!(input, "{command}").unwrap();
            input.flush().unwrap();
            self.expect(expected);
        }

        fn wait(&mut self) -> std::process::ExitStatus {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
            loop {
                if let Some(status) = self.child.try_wait().expect("se consulta el hijo") {
                    return status;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "el hijo de prueba no terminó"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    impl Drop for ControlledChild {
        fn drop(&mut self) {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            if let Err(error) = self.child.kill() {
                drop(
                    writeln!(
                        std::io::stderr(),
                        "no se pudo terminar el hijo de prueba: {error}"
                    )
                    .map_err(|error| error.to_string()),
                );
                return;
            }
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) => return,
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(std::time::Duration::from_millis(10));
                    }
                    Ok(None) => {
                        drop(
                            writeln!(
                                std::io::stderr(),
                                "no se pudo confirmar la recogida del hijo de prueba"
                            )
                            .map_err(|error| error.to_string()),
                        );
                        return;
                    }
                    Err(error) => {
                        drop(
                            writeln!(
                                std::io::stderr(),
                                "no se pudo consultar el hijo de prueba: {error}"
                            )
                            .map_err(|error| error.to_string()),
                        );
                        return;
                    }
                }
            }
        }
    }

    #[test]
    fn watchdog_reader_receives_heartbeat_and_eof_while_child_is_alive() {
        let mut fixture = ControlledChild::start();
        let stderr = fixture.child.stderr.take().unwrap();
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let notifier = ReaderNotifier {
            signals: signals.clone(),
            sender,
        };
        let (diagnostics, _output) = std::sync::mpsc::sync_channel(32);
        let reader = PipeReader::start(stderr, notifier, diagnostics).unwrap();
        fixture.command("heartbeat", "AVI_TEST HEARTBEAT");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        assert!(signals.lock().unwrap().heartbeat.take().is_some());
        fixture.command("close", "AVI_TEST CLOSED");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        assert!(signals.lock().unwrap().eof);
        assert!(fixture.child.try_wait().unwrap().is_none());
        reader.finish().expect("el lector termina después del EOF");
        writeln!(fixture.input.as_mut().unwrap(), "exit").unwrap();
        fixture.input.as_mut().unwrap().flush().unwrap();
        assert!(fixture.wait().success());
    }

    #[test]
    fn watchdog_reader_cancels_while_child_keeps_pipe_open() {
        let mut fixture = ControlledChild::start();
        let stderr = fixture.child.stderr.take().unwrap();
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        let notifier = ReaderNotifier { signals, sender };
        let (diagnostics, _output) = std::sync::mpsc::sync_channel(32);
        let reader = PipeReader::start(stderr, notifier, diagnostics).unwrap();
        let result = reader.finish();
        assert!(fixture.child.try_wait().unwrap().is_none());
        writeln!(fixture.input.as_mut().unwrap(), "exit").unwrap();
        fixture.input.as_mut().unwrap().flush().unwrap();
        assert!(fixture.wait().success());
        result.expect("el lector termina aunque el hijo conserve el pipe");
    }

    #[test]
    fn watchdog_reader_continues_after_forward_failure() {
        let mut fixture = ControlledChild::start();
        let stderr = fixture.child.stderr.take().unwrap();
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let notifier = ReaderNotifier {
            signals: signals.clone(),
            sender,
        };
        let (diagnostics, output) = std::sync::mpsc::sync_channel(1);
        drop(output);
        let reader = PipeReader::start(stderr, notifier, diagnostics).unwrap();
        fixture.command("diagnostic", "AVI_TEST DIAGNOSTIC");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        assert!(signals.lock().unwrap().forward_error.take().is_some());
        fixture.command("heartbeat", "AVI_TEST HEARTBEAT");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        assert!(signals.lock().unwrap().heartbeat.take().is_some());
        fixture.command("close", "AVI_TEST CLOSED");
        reader.finish().unwrap();
        writeln!(fixture.input.as_mut().unwrap(), "exit").unwrap();
        fixture.input.as_mut().unwrap().flush().unwrap();
        assert!(fixture.wait().success());
    }

    #[test]
    fn watchdog_reader_distinguishes_read_error_from_eof() {
        let mut fixture = ControlledChild::start();
        let stderr = fixture.child.stderr.take().unwrap();
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (sender, receiver) = std::sync::mpsc::sync_channel(1);
        let notifier = ReaderNotifier {
            signals: signals.clone(),
            sender,
        };
        let (diagnostics, _output) = std::sync::mpsc::sync_channel(1);
        let reader = PipeReader::start(stderr, notifier, diagnostics).unwrap();
        fixture.command("invalid", "AVI_TEST INVALID");
        receiver
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap();
        let signals = signals.lock().unwrap();
        assert!(signals.read_error.is_some());
        assert!(!signals.eof);
        drop(signals);
        reader.finish().unwrap();
        writeln!(fixture.input.as_mut().unwrap(), "exit").unwrap();
        fixture.input.as_mut().unwrap().flush().unwrap();
        assert!(fixture.wait().success());
    }

    #[test]
    fn watchdog_read_error_aborts_and_reaps_real_child() {
        let mut fixture = ControlledChild::start();
        fixture.command("invalid", "AVI_TEST INVALID");
        let error = watch_child(
            &mut fixture.child,
            std::time::Duration::from_secs(30),
            std::time::Duration::from_secs(1),
        )
        .unwrap_err();
        assert!(
            error.to_string().contains("no se pudo leer stderr"),
            "{error}"
        );
        assert!(
            fixture.child.try_wait().unwrap().is_some(),
            "el hijo queda recogido"
        );
    }

    #[test]
    fn watchdog_diagnostic_writer_reports_error_and_drains() {
        struct BrokenWriter;
        impl Write for BrokenWriter {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "salida cerrada",
                ))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (sender, _receiver) = std::sync::mpsc::sync_channel(1);
        let notifier = ReaderNotifier {
            signals: signals.clone(),
            sender,
        };
        let (diagnostics, output) = std::sync::mpsc::sync_channel(2);
        diagnostics.send("primero".into()).unwrap();
        diagnostics.send("segundo".into()).unwrap();
        drop(diagnostics);
        forward_diagnostics(BrokenWriter, output, notifier);
        assert!(signals
            .lock()
            .unwrap()
            .forward_error
            .as_deref()
            .unwrap()
            .contains("salida cerrada"));
    }

    #[test]
    fn watchdog_retries_forward_warning_after_queue_is_full() {
        let mut fixture = ControlledChild::start();
        let signals = std::sync::Arc::new(std::sync::Mutex::new(ReaderSignals::default()));
        let (_sender, receiver) = std::sync::mpsc::sync_channel(1);
        let (diagnostics, output) = std::sync::mpsc::sync_channel(1);
        diagnostics.send("diagnóstico anterior".into()).unwrap();
        let mut runtime = ChannelWatch {
            child: &mut fixture.child,
            receiver,
            signals,
            forward_error: None,
            forward_reported: false,
            diagnostics,
        };
        runtime.note_forward_error("cola llena".into());
        assert!(!runtime.forward_reported);
        assert_eq!(output.try_recv().unwrap(), "diagnóstico anterior");
        assert!(runtime.event().unwrap().is_none());
        assert!(output.try_recv().unwrap().contains("cola llena"));
        assert!(runtime.forward_reported);
        drop(runtime);
        writeln!(fixture.input.as_mut().unwrap(), "exit").unwrap();
        fixture.input.as_mut().unwrap().flush().unwrap();
        assert!(fixture.wait().success());
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
