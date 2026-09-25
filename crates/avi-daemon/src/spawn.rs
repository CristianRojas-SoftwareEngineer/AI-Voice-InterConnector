use std::process::Command;

/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// El daemon hijo **no debe heredar handles del padre**. En Windows el padre suele
/// ser el CLI raíz, pero en los E2E de `cli_golden` el CLI a su vez es hijo de
/// `cargo test` capturando su salida vía `Command::output()` (un *pipe*).
/// `DETACHED_PROCESS (0x8)` no deshabilita la herencia de handles: con
/// `bInheritHandles=TRUE` (default de `CreateProcessW`, no forzable a FALSE en Rust
/// estable) el daemon hijo —y `qwen_tts.exe`— heredan el handle de escritura del
/// pipe. Como `output()` solo retorna cuando todos los holders del pipe lo cierran,
/// el daemon (que vive ~10 s en graceful shutdown) colgaba el test. `Stdio::null`
/// aquí no basta (fija los STD del hijo pero no impide heredar OTROS handles
/// heredables del padre): la protección real es cortar la herencia en la raíz con
/// `SetHandleInformation(HANDLE_FLAG_INHERIT, 0)` sobre los STD del proceso que
/// spawnea (`main::disinherit_standard_handles`, llamado en `handle_daemon`). No
/// existe una creation flag que desactive la herencia. En Unix `fork/exec` con
/// `Stdio::null` + `setsid` + `FD_CLOEXEC` ya logra lo análogo.
///
/// NOTA (cierre garantizado): el apagado ya no depende solo de
/// `lib.rs::shutdown_handler` vía `with_graceful_shutdown` + `tokio::sync::Notify`
/// (el antiguo `tokio::spawn(async { process::exit(0) })` no terminaba el proceso
/// dentro del runtime de `axum::serve`). El daemon escucha además señales del
/// sistema por la misma ruta que POST `/shutdown`, el CLI reclama el residual
/// degradado al arrancar (matar-y-rearrancar) y toda parada mata el árbol preciso
/// por PID con deadline y verificación (`kill_tree_by_pid` + `pid_alive`).
/// `ready_file` designa el fichero de señalización del evento
/// `avi-daemon-ready` y viaja al hijo como flag `--ready-file` (transporte
/// flag+fichero, nunca pipe heredable). `Stdio::null` en los tres flujos se
/// mantiene: el evento ya no depende de stdio heredado.
pub fn spawn_background(
    auto_restart: bool,
    max_retries: u32,
    warm_voice: &str,
    ready_file: Option<&std::path::Path>,
) -> anyhow::Result<u32> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").arg("serve");
    if auto_restart {
        cmd.arg("--auto-restart");
    }
    cmd.arg("--max-retries").arg(max_retries.to_string());
    cmd.arg("--warm-voice").arg(warm_voice);
    if let Some(ruta) = ready_file {
        cmd.arg("--ready-file").arg(ruta);
    }

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
        // DETACHED_PROCESS (0x8): sin consola del padre.
        // CREATE_NEW_PROCESS_GROUP (0x200): grupo propio.
        // La herencia de handles se corta en la raíz vía `SetHandleInformation`
        // (`main::disinherit_standard_handles`), no con una creation flag.
        cmd.creation_flags(0x00000008 | 0x00000200);
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
    let pid = child.id();
    // Grupo propio ya garantizado por flags (Windows: CREATE_NEW_PROCESS_GROUP;
    // Unix: setsid): el árbol es matable de forma precisa por PID con
    // `kill_tree_by_pid` (alternativa admitida: taskkill `/F /T` por PID con
    // verificación posterior). El Job Object con cierre del árbol NO se crea
    // aquí en el padre efímero (moriría con él y mataría al daemon recién
    // lanzado): lo instala el daemon longevo al arrancar vía
    // `install_job_with_tree_kill` (lado servidor).
    Ok(pid)
}

/// Viveza real de un PID a nivel de sistema (sin probe HTTP ni pidfile).
///
/// Windows: `tasklist` con filtro exacto; Unix: `kill -0` (éxito = vivo).
/// `0` nunca está vivo. Bloqueante y breve: apto para el handler de Ctrl+C y
/// para los bucles de verificación con deadline de las paradas.
pub fn pid_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        let salida = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match salida {
            Ok(o) if o.status.success() => {
                let texto = String::from_utf8_lossy(&o.stdout);
                texto.contains(&pid.to_string())
            }
            _ => false,
        }
    }
    #[cfg(unix)]
    {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Mata el árbol preciso por PID con la alternativa admitida (sin Job en el
/// padre): Windows `taskkill /F /T /PID` (mata el árbol); Unix `kill -9` al
/// grupo (`-<pid>`, el daemon es líder de sesión por `setsid`) y luego al PID.
/// No toca pidfile ni verifica: el llamante combina con `pid_alive` y deadline.
/// Nunca mata el PID 0; la guarda contra auto-muerte (`pid != proceso propio`
/// para la imagen compartida CLI/daemon) vive en el llamante.
///
/// Reutilizada además para el reclamo por grupo ante líder muerto: el
/// llamante la invoca aunque el PID ya esté muerto para alcanzar al residente
/// reparentado del mismo grupo, con verificación por 8766.
pub fn kill_tree_by_pid(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    #[cfg(windows)]
    {
        std::process::Command::new("taskkill")
            .args(["/F", "/T", "/PID", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
    #[cfg(unix)]
    {
        let _ = std::process::Command::new("kill")
            .args(["-9", &format!("-{}", pid)])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        std::process::Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Espera bloqueante a la muerte del PID hasta el deadline (sondeo 100 ms).
/// Para el handler de Ctrl+C y verificaciones síncronas; los caminos async
/// usan su propio bucle con `tokio::time::sleep` + `pid_alive`.
pub fn wait_for_pid_death(pid: u32, deadline: std::time::Duration) -> bool {
    let inicio = std::time::Instant::now();
    while inicio.elapsed() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    !pid_alive(pid)
}

// Instala en el proceso actual (lado daemon longevo) un Job Object con
// `KILL_ON_JOB_CLOSE`. La implementación vive en el binario (`src/main.rs`,
// rama `Serve`, que sí dispone de `windows-sys` vía el workspace): este crate
// no añade la dependencia para no exceder el alcance (alternativa admitida:
// `kill_tree_by_pid` con verificación). Ver `install_job_with_tree_kill`
// en el CLI.

/// Helper determinista de desinstalación en Windows.
///
/// No borra `install_dir` desde el proceso vivo (determinista: `PermissionDenied`
/// si lo intentara). Escribe un `.ps1` en `%TEMP%` que espera la muerte del
/// `PID` padre y luego borra `LiteralPath` con `Remove-Item -Recurse -Force`,
/// sin best-effort: si crear el archivo o spawnear falla, retorna `Err` y
/// `handle_uninstall` falla — no hay aviso `Bórralo manualmente`.
#[cfg(windows)]
pub fn spawn_uninstall_helper(
    install_dir: &std::path::Path,
    pid: u32,
) -> anyhow::Result<std::path::PathBuf> {
    use std::os::windows::process::CommandExt;
    use std::process::Stdio;

    let dir_literal = install_dir.to_string_lossy().replace('\'', "''");
    let script = format!(
        "Wait-Process -Id {pid} -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 500; \
         if (Test-Path -LiteralPath '{dir}') {{ \
           Remove-Item -LiteralPath '{dir}' -Recurse -Force -ErrorAction SilentlyContinue \
         }}; \
         Remove-Item -LiteralPath $PSCommandPath -Force -ErrorAction SilentlyContinue\n",
        pid = pid,
        dir = dir_literal
    );

    let helper = std::env::temp_dir().join(format!(
        "avi-uninstall-{}-{}.ps1",
        pid,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
    ));
    std::fs::write(&helper, script)?;

    let mut cmd = std::process::Command::new("powershell.exe");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
        helper.to_string_lossy().as_ref(),
    ]);
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // DETACHED_PROCESS (0x8) | CREATE_NEW_PROCESS_GROUP (0x200): sin consola del
    // padre, grupo propio. La herencia de handles se corta en la raíz vía
    // `SetHandleInformation` (`main::disinherit_standard_handles`), no con flag.
    cmd.creation_flags(0x00000008 | 0x00000200);
    cmd.spawn()?;
    Ok(helper)
}
