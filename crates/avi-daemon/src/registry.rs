//! Registro en disco del daemon y de su residente: el daemon es el único
//! escritor de `daemon.pid`.
//!
//! El registro vive en memoria y cada cambio reescribe el pidfile completo de
//! forma atómica, nunca leyendo, modificando y reescribiendo: así lo que hay en
//! disco es siempre el estado de este proceso. Si el usuario borra el fichero, el
//! siguiente cambio lo recrea.

use avi_process::registry::{
    classify_registered, now_rfc3339, observe_pid_file, write_pid_file, PidFile,
    PidFileObservation, Registered,
};
use avi_process::{pid_alive_exact, process_identity, ProcessIdentity};
use avi_tts::ResidentRegistry;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Mutex;

/// Registro del daemon y de su residente bajo la raíz de datos.
pub struct DaemonRegistry {
    data_dir: PathBuf,
    state: Mutex<PidFile>,
}

impl DaemonRegistry {
    /// Registro vacío de este proceso: aún no escribe nada hasta
    /// [`register_daemon`](Self::register_daemon).
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            state: Mutex::new(PidFile {
                pid: std::process::id(),
                addr: None,
                started_at: None,
                resident_pid: 0,
                identity: None,
                resident_identity: None,
                resident_port: None,
            }),
        }
    }

    /// Registra al daemon en `addr`, la dirección real enlazada. Falla si no se
    /// puede observar la identidad del propio proceso o si el pidfile registra a
    /// otro daemon vivo: sin identidad la parada no podría distinguirlo de un PID
    /// reasignado, y con otro daemon vivo el registro dejaría de ser veraz.
    ///
    /// Un reinicio en proceso conserva el PID: el propio registro se tolera y se
    /// reescribe sin residente, porque el de la vida anterior ya no existe.
    pub fn register_daemon(&self, addr: &SocketAddr) -> anyhow::Result<()> {
        let own_pid = std::process::id();
        let identity = process_identity(own_pid).ok_or_else(|| {
            anyhow::anyhow!(
                "no se pudo observar la identidad del proceso del daemon (pid {own_pid}): sin ella la parada no puede distinguirlo de otro proceso"
            )
        })?;
        self.ensure_no_live_peer(own_pid)?;
        let mut state = self.lock();
        state.pid = own_pid;
        state.addr = Some(addr.to_string());
        state.started_at = Some(now_rfc3339());
        state.identity = Some(identity);
        state.resident_pid = 0;
        state.resident_identity = None;
        state.resident_port = None;
        self.write(&state)
    }

    /// Rechaza el arranque si el pidfile registra a otro daemon vivo y nuestro.
    /// El propio PID no cuenta: un reinicio en proceso lo vería a sí mismo.
    fn ensure_no_live_peer(&self, own_pid: u32) -> anyhow::Result<()> {
        let PidFileObservation::Present(file) = observe_pid_file(&self.data_dir) else {
            return Ok(());
        };
        if file.pid == own_pid {
            return Ok(());
        }
        let state = classify_registered(
            file.pid,
            pid_alive_exact(file.pid),
            file.identity.as_ref(),
            || process_identity(file.pid),
        );
        if state == Registered::Ours {
            anyhow::bail!(
                "ya hay otro daemon en ejecución (pid {}): este arranque no se registra",
                file.pid
            );
        }
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, PidFile> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self, state: &PidFile) -> anyhow::Result<()> {
        write_pid_file(&self.data_dir, state)
            .map_err(|e| anyhow::anyhow!("no se pudo escribir daemon.pid: {e}"))
    }
}

/// El motor registra a su residente en el pidfile del daemon, que reescribe
/// completo en cada cambio.
impl ResidentRegistry for DaemonRegistry {
    /// Anota al residente recién lanzado con su PID, su identidad y su puerto.
    fn register(&self, pid: u32, identity: &ProcessIdentity, port: u16) -> anyhow::Result<()> {
        let mut state = self.lock();
        state.resident_pid = pid;
        state.resident_identity = Some(identity.clone());
        state.resident_port = Some(port);
        self.write(&state)
    }

    /// Anula el registro del residente, solo si el registrado es `pid`. El fallo
    /// de escritura no se propaga: se anula cuando el residente ya murió o se
    /// descarta, y un registro obsoleto lo corrige el siguiente cambio.
    fn unregister(&self, pid: u32) {
        let mut state = self.lock();
        if state.resident_pid != pid {
            return;
        }
        state.resident_pid = 0;
        state.resident_identity = None;
        state.resident_port = None;
        if let Err(e) = self.write(&state) {
            eprintln!("aviso: no se pudo actualizar daemon.pid al anular el residente: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_process::registry::pid_path;

    fn addr() -> SocketAddr {
        "127.0.0.1:7001".parse().unwrap()
    }

    fn read(dir: &std::path::Path) -> PidFile {
        let PidFileObservation::Present(file) = observe_pid_file(dir) else {
            panic!("el pidfile debe existir y ser legible");
        };
        file
    }

    /// El registro del daemon escribe su PID, su identidad observada y la
    /// dirección real, sin residente.
    #[test]
    fn register_daemon_writes_own_identity() {
        let dir = tempfile::TempDir::new().unwrap();
        let registry = DaemonRegistry::new(dir.path().to_path_buf());
        registry.register_daemon(&addr()).unwrap();

        let file = read(dir.path());
        assert_eq!(file.pid, std::process::id());
        assert_eq!(file.addr.as_deref(), Some("127.0.0.1:7001"));
        assert_eq!(file.identity, process_identity(std::process::id()));
        assert!(file.started_at.is_some());
        assert_eq!(file.resident_pid, 0);
        assert_eq!(file.resident_identity, None);
        assert_eq!(file.resident_port, None);
    }

    /// Un reinicio en proceso conserva el PID: el registro se tolera a sí mismo y
    /// descarta el residente de la vida anterior.
    #[test]
    fn register_daemon_tolerates_its_own_previous_record() {
        let dir = tempfile::TempDir::new().unwrap();
        let registry = DaemonRegistry::new(dir.path().to_path_buf());
        registry.register_daemon(&addr()).unwrap();
        let resident = ProcessIdentity {
            start: "1".to_string(),
            image: "qwen_tts.exe".to_string(),
        };
        registry.register(4321, &resident, 40_000).unwrap();
        assert_eq!(read(dir.path()).resident_pid, 4321);

        registry.register_daemon(&addr()).unwrap();
        let file = read(dir.path());
        assert_eq!(file.pid, std::process::id());
        assert_eq!(file.resident_pid, 0, "el residente anterior ya no existe");
    }

    /// Un pidfile con otro daemon vivo y con la identidad registrada impide el
    /// registro y no lo modifica; con una identidad ajena el PID cuenta como
    /// muerto y se sustituye.
    #[test]
    fn register_daemon_rejects_live_peer_only() {
        let dir = tempfile::TempDir::new().unwrap();
        // Un proceso vivo distinto del propio: el padre de la prueba no es
        // portable, así que se lanza un hijo longevo y se recolecta al final.
        let mut child = spawn_sleeper();
        let peer_pid = child.id();
        let peer_identity = wait_identity(peer_pid);

        let peer = PidFile {
            pid: peer_pid,
            addr: Some("127.0.0.1:7002".to_string()),
            started_at: None,
            resident_pid: 0,
            identity: Some(peer_identity.clone()),
            resident_identity: None,
            resident_port: None,
        };
        write_pid_file(dir.path(), &peer).unwrap();
        let registry = DaemonRegistry::new(dir.path().to_path_buf());
        let err = registry.register_daemon(&addr()).unwrap_err().to_string();
        assert!(err.contains(&peer_pid.to_string()), "{err}");
        assert_eq!(read(dir.path()), peer, "el pidfile del otro no se toca");

        let foreign = PidFile {
            identity: Some(ProcessIdentity {
                start: "no-coincide".to_string(),
                image: peer_identity.image.clone(),
            }),
            ..peer
        };
        write_pid_file(dir.path(), &foreign).unwrap();
        registry.register_daemon(&addr()).unwrap();
        assert_eq!(read(dir.path()).pid, std::process::id());

        child.kill().unwrap();
        child.wait().unwrap();
    }

    /// Anular el residente solo limpia el registro si el PID coincide.
    #[test]
    fn unregister_requires_matching_pid() {
        let dir = tempfile::TempDir::new().unwrap();
        let registry = DaemonRegistry::new(dir.path().to_path_buf());
        registry.register_daemon(&addr()).unwrap();
        let resident = ProcessIdentity {
            start: "1".to_string(),
            image: "qwen_tts.exe".to_string(),
        };
        registry.register(4321, &resident, 40_000).unwrap();

        registry.unregister(9999);
        assert_eq!(read(dir.path()).resident_pid, 4321);
        assert_eq!(read(dir.path()).resident_port, Some(40_000));

        registry.unregister(4321);
        let file = read(dir.path());
        assert_eq!(file.resident_pid, 0);
        assert_eq!(file.resident_identity, None);
        assert_eq!(file.resident_port, None);
    }

    /// Si el pidfile se borra, el siguiente cambio lo recrea completo.
    #[test]
    fn deleted_pidfile_is_recreated_on_next_change() {
        let dir = tempfile::TempDir::new().unwrap();
        let registry = DaemonRegistry::new(dir.path().to_path_buf());
        registry.register_daemon(&addr()).unwrap();
        std::fs::remove_file(pid_path(dir.path())).unwrap();

        let resident = ProcessIdentity {
            start: "1".to_string(),
            image: "qwen_tts.exe".to_string(),
        };
        registry.register(4321, &resident, 40_000).unwrap();
        let file = read(dir.path());
        assert_eq!(file.pid, std::process::id());
        assert_eq!(file.identity, process_identity(std::process::id()));
        assert_eq!(file.resident_pid, 4321);
    }

    fn spawn_sleeper() -> std::process::Child {
        #[cfg(windows)]
        {
            std::process::Command::new("ping")
                .args(["-n", "60", "127.0.0.1"])
                .stdout(std::process::Stdio::null())
                .spawn()
                .expect("lanza un proceso de espera")
        }
        #[cfg(not(windows))]
        {
            std::process::Command::new("sleep")
                .arg("60")
                .spawn()
                .expect("lanza un proceso de espera")
        }
    }

    /// Identidad del hijo, con sondeo acotado hasta que el sistema la expone.
    fn wait_identity(pid: u32) -> ProcessIdentity {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            if let Some(identity) = process_identity(pid) {
                return identity;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "no se observó la identidad del hijo"
            );
            std::thread::yield_now();
        }
    }
}
