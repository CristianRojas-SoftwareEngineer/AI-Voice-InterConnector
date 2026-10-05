/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// El daemon hijo **solo hereda los handles declarados**. En Windows el
/// lanzamiento usa `STARTUPINFOEX` con lista explícita (`avi-process`): la
/// entrada nula y el fichero de registro viajan al hijo; el pipe de captura
/// del lanzador, los sockets y cualquier otro handle heredable del padre no
/// llegan al daemon por construcción. En Unix `fork/exec` con `setsid` +
/// `FD_CLOEXEC` ya logra lo análogo.
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
) -> anyhow::Result<avi_process::RestrictedChild> {
    let exe = std::env::current_exe()?;
    let mut args = vec![String::from("daemon"), String::from("serve")];
    if auto_restart {
        args.push(String::from("--auto-restart"));
    }
    args.push(String::from("--max-retries"));
    args.push(max_retries.to_string());
    args.push(String::from("--warm-voice"));
    args.push(warm_voice.to_string());
    if let Some(path) = ready_file {
        args.push(String::from("--ready-file"));
        args.push(path.to_string_lossy().into_owned());
    }

    #[cfg(windows)]
    {
        // CREATE_NO_WINDOW: el daemon tiene una consola oculta propia, que heredan
        // `cmd`, `tasklist` y `taskkill` lanzados por él. Con DETACHED_PROCESS cada
        // uno quedaría sin consola y abriría una ventana visible.
        // CREATE_NEW_PROCESS_GROUP: grupo propio.
        // La herencia queda restringida a la lista explícita (registro del
        // daemon); el pipe del lanzador no viaja al hijo por construcción.
        let request = avi_process::RestrictedSpawnRequest {
            program: exe,
            args,
            stdin: avi_process::StdinSpec::Null,
            log_file: log,
            creation_flags: avi_process::CREATE_NO_WINDOW | avi_process::CREATE_NEW_PROCESS_GROUP,
            extra_allowed: Vec::new(),
        };
        Ok(avi_process::spawn_with_allowlist(request)?)
    }

    #[cfg(unix)]
    {
        let request = avi_process::RestrictedSpawnRequest {
            program: exe,
            args,
            stdin: avi_process::StdinSpec::Null,
            log_file: log,
            creation_flags: 0,
            extra_allowed: Vec::new(),
        };
        Ok(avi_process::spawn_with_allowlist(request)?)
    }
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
