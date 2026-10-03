use std::process::Command;

/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// El daemon hijo **no debe heredar handles del padre**. En Windows el padre suele
/// ser el CLI raíz, pero en los E2E de `cli_golden` el CLI a su vez es hijo de
/// `cargo test` capturando su salida vía `Command::output()` (un *pipe*).
/// Ninguna creation flag de consola deshabilita la herencia de handles: con
/// `bInheritHandles=TRUE` (default de `CreateProcessW`, no forzable a FALSE en Rust
/// estable) el daemon hijo —y `qwen_tts.exe`— heredan el handle de escritura del
/// pipe. Como `output()` solo retorna cuando todos los holders del pipe lo cierran,
/// el daemon (que vive ~10 s en graceful shutdown) colgaba el test. Redirigir los
/// STD del hijo aquí no basta (no impide heredar OTROS handles
/// heredables del padre): la protección real es cortar la herencia en la raíz con
/// `SetHandleInformation(HANDLE_FLAG_INHERIT, 0)` sobre los STD del proceso que
/// spawnea (`main::disinherit_standard_handles`, llamado en `handle_daemon`). No
/// existe una creation flag que desactive la herencia. En Unix `fork/exec` con
/// `setsid` + `FD_CLOEXEC` ya logra lo análogo.
///
/// NOTA (cierre garantizado): el apagado ya no depende solo de
/// `shutdown_handler` del crate vía `with_graceful_shutdown` + `tokio::sync::Notify`
/// (el antiguo `tokio::spawn(async { process::exit(0) })` no terminaba el proceso
/// dentro del runtime de `axum::serve`). El daemon escucha además señales del
/// sistema por la misma ruta que POST `/shutdown`, el CLI reclama el residual
/// degradado al arrancar (matar-y-rearrancar) y toda parada mata el árbol preciso
/// por PID con deadline y verificación (`kill_tree_by_pid` + `pid_alive`).
/// `ready_file` viaja al hijo como flag `--ready-file` (transporte
/// flag+fichero, nunca pipe heredable): en él el hijo publica el registro de
/// éxito (dirección real) o el de fallo (causa tipada) de su arranque.
/// stdin va a `Stdio::null` para no heredar la tubería del abuelo; stdout y
/// stderr van a `log`, un fichero que no tiene extremo de tubería que heredar y
/// recoge trazas, pánicos, salida nativa y avisos del supervisor. La señal de
/// arranque no depende de stdio.
///
/// El llamante es dueño del `Child` devuelto: debe vigilarlo durante el
/// arranque (`try_wait` detecta una muerte que no publicó registro, con su
/// estado de salida real) y recolectarlo tras matar el árbol ante un fallo.
/// Mientras no se recolecta, su PID no puede reutilizarse.
pub fn spawn_background(
    auto_restart: bool,
    max_retries: u32,
    warm_voice: &str,
    ready_file: Option<&std::path::Path>,
    log: std::fs::File,
) -> anyhow::Result<std::process::Child> {
    let exe = std::env::current_exe()?;
    let mut cmd = Command::new(exe);
    cmd.arg("daemon").arg("serve");
    if auto_restart {
        cmd.arg("--auto-restart");
    }
    cmd.arg("--max-retries").arg(max_retries.to_string());
    cmd.arg("--warm-voice").arg(warm_voice);
    if let Some(path) = ready_file {
        cmd.arg("--ready-file").arg(path);
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Stdio;
        // En Windows Rust hereda por default `GetStdHandle(STD_*_HANDLE)` del padre. En
        // los E2E `cli_golden` el padre es el CLI lanzado vía `Command::output()`
        // (pipe): heredar el write-end haría que `output()` del test no retornara hasta
        // que el daemon (10 s) termine. stdin va a NUL; stdout y stderr van a un
        // fichero, que no tiene extremo de tubería. El daemon deshereda sus handles
        // estándar al arrancar, así que el motor que lanza no hereda este log.
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        // CREATE_NO_WINDOW: el daemon tiene una consola oculta propia, que heredan
        // `cmd`, `tasklist` y `taskkill` lanzados por él. Con DETACHED_PROCESS cada
        // uno quedaría sin consola y abriría una ventana visible.
        // CREATE_NEW_PROCESS_GROUP: grupo propio.
        // La herencia de handles se corta en la raíz vía `SetHandleInformation`
        // (`main::disinherit_standard_handles`), no con una creation flag.
        cmd.creation_flags(avi_process::CREATE_NO_WINDOW | avi_process::CREATE_NEW_PROCESS_GROUP);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        use std::process::Stdio;
        cmd.stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }

    // Grupo propio ya garantizado por flags (Windows: CREATE_NEW_PROCESS_GROUP;
    // Unix: setsid): el árbol es matable de forma precisa por PID con
    // `kill_tree_by_pid` (alternativa admitida: taskkill `/F /T` por PID con
    // verificación posterior). El motor muere con el daemon porque vigila su
    // entrada estándar, no por un agrupamiento del sistema operativo.
    let child = cmd.spawn()?;
    Ok(child)
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
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/FO", "CSV", "/NH"])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match output {
            Ok(o) if o.status.success() => {
                let text = String::from_utf8_lossy(&o.stdout);
                text.contains(&pid.to_string())
            }
            _ => false,
        }
    }
    #[cfg(unix)]
    {
        if !fits_pid_t(pid) {
            return false;
        }
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

/// Un PID de Unix es un `pid_t` con signo de 32 bits. `kill` trunca a ese tipo
/// el número que recibe, así que un valor mayor (de un pidfile corrupto) señala
/// a otro proceso: 4294967294 se convierte en el grupo -2 y su negativo, en el
/// PID 2. Un PID que no cabe no es un proceso y nunca se pasa a `kill`.
#[cfg(unix)]
fn fits_pid_t(pid: u32) -> bool {
    i32::try_from(pid).is_ok()
}

/// Mata el árbol preciso por PID: Windows `taskkill /F /T /PID` (mata el árbol); Unix `kill -9` al
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
        if !fits_pid_t(pid) {
            return false;
        }
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
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if !pid_alive(pid) {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    !pid_alive(pid)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Un PID que no cabe en `pid_t` no está vivo ni se puede matar: nunca llega
    /// a `kill`, que lo truncaría a otro proceso.
    #[test]
    fn out_of_range_pid_is_neither_alive_nor_killed() {
        let pid = u32::MAX - 1;
        assert!(!pid_alive(pid));
        assert!(!kill_tree_by_pid(pid));
    }
}
