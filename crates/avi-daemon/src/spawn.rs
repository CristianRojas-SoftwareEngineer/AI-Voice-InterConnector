use std::process::Command;

/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// El daemon hijo **no debe heredar handles del padre**. En Windows el padre suele
/// ser el CLI raíz, pero en los E2E de `cli_golden` el CLI a su vez es hijo de
/// `cargo test` capturando su salida vía `Command::output()` (un *pipe*).
/// `DETACHED_PROCESS (0x8)` no deshabilita la herencia de handles: con
/// `bInheritHandles=TRUE` (default de `CreateProcessW` cuando Rust no lo fuerza a
/// FALSE) el daemon hijo —y `qwen_tts.exe`— heredan el handle de escritura del pipe.
/// Como `output()` solo retorna cuando todos los holders del pipe lo cierran, el
/// daemon (que vive ~10 s en graceful shutdown) colgaba el test. La técnica original
/// (`Stdio::null` + `creation_flags 0x8 | 0x200`) es necesaria pero **no suficiente**:
/// `CREATE_NO_HANDLE_INHERIT (0x02000000)` fuerza `bInheritHandles=FALSE`. En Unix
/// `fork/exec` con `Stdio::null` + `setsid` + `FD_CLOEXEC` ya logra lo análogo.
///
/// NOTA: el shutdown del daemon se resolvió aparte en `lib.rs::shutdown_handler` vía
/// `with_graceful_shutdown` + `tokio::sync::Notify` (el antiguo
/// `tokio::spawn(async { process::exit(0) })` no terminaba el proceso dentro del
/// runtime de `axum::serve`).
pub fn spawn_background() -> anyhow::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").arg("serve");

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        // `Stdio::null()` para stdin/stdout/stderr: en Windows Rust hereda por default
        // `GetStdHandle(STD_*_HANDLE)` del padre. En los E2E `cli_golden` el padre es el
        // CLI lanzado vía `Command::output()` (pipe): sin esto el daemon hijo hereda el
        // write-end del pipe y `output()` del test no retorna hasta que el daemon (10 s)
        // termine. Con null los STD son handles non-inheritable a NUL.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // CREATE_NO_HANDLE_INHERIT (0x02000000) fuerza bInheritHandles=FALSE en
        // CreateProcessW: doble garantía de que el hijo no herede handles del padre.
        // CREATE_NO_WINDOW (0x8) | CREATE_NEW_PROCESS_GROUP (0x200): no hereda
        // consola/ventana del padre.
        cmd.creation_flags(0x02000000 | 0x00000008 | 0x00000200);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    let child = cmd.spawn()?;
    Ok(child.id())
}
