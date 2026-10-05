/// Lanza el daemon en segundo plano desacoplado del proceso padre.
///
/// El daemon hijo **solo hereda los handles declarados**. En Windows el
/// lanzamiento usa `STARTUPINFOEX` con lista explícita (`avi-process`): la
/// entrada nula y el fichero de registro viajan al hijo; el pipe de captura
/// del lanzador, los sockets y cualquier otro handle heredable del padre no
/// llegan al daemon por construcción. En Unix el hijo se bifurca y ejecuta con
/// `FD_CLOEXEC` y `pre_exec(setsid)`: nace líder de su propia sesión y grupo
/// (`getpgid(child) == child`), así el kill de grupo alcanza solo a su árbol y
/// nunca al grupo del lanzador.
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
        // `cmd` y `taskkill` lanzados por él. Con DETACHED_PROCESS cada uno
        // quedaría sin consola y abriría una ventana visible.
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
/// Delega en la primitiva exacta de `avi-process` (Windows: `OpenProcess` +
/// `GetExitCodeProcess`; Unix: `libc::kill(pid, 0)`); ya no se lanzan
/// `tasklist` ni `kill` externos: la comparación por texto sobre `tasklist`
/// hacía que el PID 12 matcheara dentro del 112. `0` nunca está vivo.
/// Bloqueante y breve: apto para el handler de Ctrl+C y para los bucles de
/// verificación con deadline de las paradas.
pub fn pid_alive(pid: u32) -> bool {
    avi_process::pid_alive_exact(pid)
}

/// Mata el árbol preciso por PID: Windows `taskkill /F /T /PID` (mata el árbol);
/// Unix `libc::kill` con `SIGKILL` al grupo (`-<pid>`, el daemon es líder de
/// sesión por `setsid`) y luego al PID, vía la primitiva de `avi-process`.
/// No toca pidfile ni verifica: el llamante combina con `pid_alive` y deadline.
/// Nunca mata el PID 0 ni un PID que no cabe en `pid_t` (guarda en la
/// primitiva); la guarda contra auto-muerte (`pid != proceso propio` para la
/// imagen compartida CLI/daemon) vive en el llamante.
///
/// Reutilizada además para el reclamo por grupo ante líder muerto: el
/// llamante la invoca aunque el PID ya esté muerto para alcanzar al residente
/// reparentado del mismo grupo, con verificación por 8766.
pub fn kill_tree_by_pid(pid: u32) -> bool {
    avi_process::kill_tree_by_pid_exact(pid)
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

    /// La viveza es del PID exacto: el propio está vivo y el 0 no.
    ///
    /// Causa del caso 112/12: la implementación anterior de Windows ejecutaba
    /// `tasklist` y decidía con `text.contains(pid)`, de modo que preguntar por
    /// el PID 12 daba vivo cuando existía el 112 (la cadena "12" aparece dentro
    /// de "112"). La primitiva exacta de `avi-process` (`OpenProcess` +
    /// `GetExitCodeProcess`, vivo solo si `STILL_ACTIVE`) consulta el PID
    /// puntual y no compara texto.
    #[test]
    fn own_pid_is_alive_and_zero_is_not() {
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(0));
    }

    /// En Unix el hijo del lanzamiento nace líder de su propio grupo
    /// (`setsid`): `getpgid(child) == child`. Es la propiedad que hace exacto
    /// el kill de grupo (`kill(-pid)`): alcanza solo al árbol del daemon y
    /// nunca al grupo del lanzador. La prueba ejercita la primitiva que usa
    /// `spawn_background` con un hijo longevo; lanzar el binario de pruebas con
    /// los argumentos del daemon no es determinista (saldría de inmediato).
    #[cfg(unix)]
    #[test]
    fn unix_child_is_group_leader() {
        let log_path = std::env::temp_dir().join(format!(
            "avi-daemon-setsid-{}-{}.log",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let log_file = std::fs::File::create(&log_path).expect("debe crear el registro");
        let request = avi_process::RestrictedSpawnRequest {
            program: std::path::PathBuf::from("sleep"),
            args: vec![String::from("30")],
            stdin: avi_process::StdinSpec::Null,
            log_file,
            creation_flags: 0,
            extra_allowed: Vec::new(),
        };
        let mut child = avi_process::spawn_with_allowlist(request).expect("debe lanzar al hijo");
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
                "el hijo del lanzamiento no es líder de su grupo: pgid {pgid}, pid {pid}"
            );
            // El padre puede observar al hijo antes de que corra su
            // `pre_exec`: se espera la transición observable, con deadline, no
            // un tiempo fijo.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        child.kill().expect("debe poder matar al hijo");
        child.wait().expect("debe poder recolectar al hijo");
        let _ = std::fs::remove_file(&log_path);
    }
}
