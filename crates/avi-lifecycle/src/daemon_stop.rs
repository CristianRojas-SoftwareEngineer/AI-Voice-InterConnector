//! Protocolo de parada del daemon y del proceso residente. Es el paso 5 de la
//! instalación y de la desinstalación, y un paso propio de la limpieza.
//!
//! Este bloque **no se reescribe: se traslada**. El protocolo tiene historia real
//! detrás —`daemon.pid` con esquema plano, `POST /shutdown` acotado, árbol
//! preciso por PID con guarda anti-auto-muerte, verificación a nivel de sistema y
//! plazo global— y ese historial es justo lo que T11 quiere preservar. Los plazos
//! se mantienen tal cual: **8 s** de plazo global y **1500 ms** para el
//! `POST /shutdown`.
//!
//! **Lo que no se traslada aquí es el control de procesos.** Matar un árbol en
//! Windows es `taskkill /T` y en Unix es matar el grupo de sesión, y esa lógica
//! vive endurecida en `avi-daemon` (`kill_tree_by_pid`, `pid_alive`) y en
//! `avi-tts` (`resident::*`). Este crate no depende de ninguno de los dos: el
//! primero arrastra el árbol de `avi-stt` y el segundo el motor TTS, y el plan
//! declara que `avi-lifecycle` se construye sin cliente HTTP ni árbol de runtime
//! pesado. Por eso el control de procesos entra por [`ProcessControl`], que
//! el binario implementa en T16 con las funciones que ya existen. El **protocolo**,
//! que es lo que este módulo es, sí vive aquí y se afirma entero.
//!
//! **El `pidfile` cambia de sitio con D4.** La resolución usa la raíz de datos
//! vigente, de modo que un daemon vivo de una versión anterior —cuyo pidfile vivía
//! en `%APPDATA%`— **no se encuentra**. Es un supuesto asumido, sin audiencia previa
//! que lo apoye, y está escrito en el módulo para que no se lea como un
//! olvido.
//!
//! **No hay cliente HTTP.** El plan declara que este crate se construye sin él, así
//! que los dos puntos del protocolo —`POST /shutdown` y `GET /health`— viajan por
//! `tokio::net::TcpStream` con HTTP/1.1 a pelo. Son cuatro líneas de petición y
//! una de lectura de la línea de estado, contra las dos dependencias que el plan
//! declara que este crate no tiene.

use crate::LifecycleError;
pub use avi_process::ProcessIdentity;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Plazo global de la parada. Viene de `STOP_DEADLINE_GLOBAL` en el
/// binario y no cambia: es el presupuesto de todo el protocolo.
pub const STOP_DEADLINE_GLOBAL: Duration = Duration::from_secs(8);

/// Presupuesto del `POST /shutdown` acotado. El daemon puede tardar en cerrar; el
/// paso 2 lo reclama por PID si no llega.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(1500);

/// Presupuesto del sondeo de vida y del POST: ninguno de los dos puede heredar el
/// timeout largo del cliente de la CLI.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Dirección por defecto cuando no hay pidfile. Es la que publica el binario.
pub const DEFAULT_ADDR: &str = "127.0.0.1:8765";

/// Nombre del pidfile dentro de la raíz de datos.
pub const PID_FILE: &str = "daemon.pid";

/// Nombre del fichero ready dentro de la raíz de datos: el daemon publica en él
/// su dirección real y su PID cuando está listo.
pub const READY_FILE: &str = "daemon.ready";

/// Ruta del pidfile bajo la raíz de datos.
pub fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PID_FILE)
}

/// Ruta del pidfile con las raíces ya resueltas.
pub fn pid_path_default() -> PathBuf {
    pid_path(&crate::data_dir())
}

/// Contenido del pidfile tal como está en disco. El esquema es plano a propósito:
/// un binario antiguo tiene que poder leerlo y un binario nuevo tiene que poder
/// ignorarlo sin romper, con el mismo criterio que el recibo.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PidFile {
    pub pid: u32,
    /// Dirección **real** publicada por el hijo en el fichero ready: con puerto
    /// efímero difiere de la literal.
    #[serde(default)]
    pub addr: Option<String>,
    #[serde(default)]
    pub started_at: Option<String>,
    /// El daemon lo actualiza en disco al arrancar el residente
    /// (`start_resident`); al arrancar solo se conoce el PID del daemon.
    #[serde(default)]
    pub resident_pid: u32,
    /// Identidad del proceso del daemon observada al registrarlo. Sin ella, el PID
    /// se comprueba solo por su número.
    #[serde(default)]
    pub identity: Option<ProcessIdentity>,
    /// Identidad del residente observada al registrarlo, con el mismo criterio.
    #[serde(default)]
    pub resident_identity: Option<ProcessIdentity>,
}

/// Escribe el pidfile de forma atómica: temporal hermano y renombrado. El handler
/// Ctrl+C no depende solo de él —también lleva el PID en memoria—, pero la escritura
/// tardía y atómica es lo que evita que un lector vea medio JSON.
pub fn write_pid(
    data_dir: &Path,
    pid: u32,
    addr: &str,
    resident_pid: u32,
    identity: Option<ProcessIdentity>,
) -> anyhow::Result<()> {
    let path = pid_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let content = serde_json::to_string_pretty(&PidFile {
        pid,
        addr: Some(addr.to_string()),
        started_at: Some(crate::receipt::now_rfc3339()),
        resident_pid,
        identity,
        resident_identity: None,
    })?;
    let temp = path.with_extension("pid.tmp");
    std::fs::write(&temp, content.as_bytes())?;
    match std::fs::rename(&temp, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e.into())
        }
    }
}

/// Lee el pidfile completo. `Ok(None)` si no hay, si está ilegible o si es un
/// esquema que no se entiende: lectura tolerante, el llamante cae al valor por
/// defecto y nunca falla.
pub fn read_pid_file(data_dir: &Path) -> Option<PidFile> {
    let text = std::fs::read_to_string(pid_path(data_dir)).ok()?;
    serde_json::from_str(&text).ok()
}

/// PID del daemon registrado, o `None` si no hay pidfile, es ilegible o el esquema
/// no trae `pid`.
pub fn read_pid(data_dir: &Path) -> Option<u32> {
    read_pid_file(data_dir)
        .map(|f| f.pid)
        .filter(|pid| *pid != 0)
}

/// PID del residente registrado. Lectura tolerante: esquema viejo sin el campo,
/// fichero ausente o valor inválido es `0`, que significa desconocido.
pub fn read_resident_pid(data_dir: &Path) -> u32 {
    read_pid_file(data_dir).map(|f| f.resident_pid).unwrap_or(0)
}

/// Dirección publicada en el pidfile. Fichero ausente, ilegible o esquema viejo
/// sin el campo dan `None`, y el llamante cae al valor por defecto.
pub fn read_addr(data_dir: &Path) -> Option<String> {
    read_pid_file(data_dir)
        .and_then(|f| f.addr)
        .map(|addr| addr.trim().to_string())
        .filter(|addr| !addr.is_empty())
}

/// Dirección del cliente: la del pidfile cuando existe, y cuando no la derivada de
/// `AVI_DAEMON_PORT` (`127.0.0.1:8765` si la variable falta o no es un puerto).
///
/// Este es el **supuesto asumido de D4**: la raíz de datos es la vigente, así que un
/// daemon de la versión anterior, con su pidfile en la raíz antigua, no se
/// encuentra y la conexión va a la dirección de respaldo.
pub fn resolve_client_addr(data_dir: &Path) -> String {
    resolve_client_addr_with(data_dir, std::env::var("AVI_DAEMON_PORT").ok().as_deref())
}

/// Variante de [`resolve_client_addr`] que recibe el valor crudo de
/// `AVI_DAEMON_PORT` en lugar de leer el entorno, para poder probarla sin
/// tocar variables del proceso.
pub fn resolve_client_addr_with(data_dir: &Path, port_env: Option<&str>) -> String {
    read_addr(data_dir).unwrap_or_else(|| addr_for_port_env(port_env))
}

/// Dirección de parada a partir del valor crudo de `AVI_DAEMON_PORT`: el daemon
/// escucha en `127.0.0.1:<puerto>` con el puerto recortado (`0` pide uno
/// efímero), y un valor ausente o que no es un `u16` deja la dirección por
/// defecto.
pub fn addr_for_port_env(raw: Option<&str>) -> String {
    match raw.and_then(|value| value.trim().parse::<u16>().ok()) {
        Some(port) => format!("127.0.0.1:{port}"),
        None => DEFAULT_ADDR.to_string(),
    }
}

/// Dirección en la que detener el daemon, según `AVI_DAEMON_PORT` del entorno.
pub fn default_addr() -> String {
    addr_for_port_env(std::env::var("AVI_DAEMON_PORT").ok().as_deref())
}

/// Borra el pidfile. Solo lo hace quien ha comprobado antes que el daemon está
/// muerto: borrarlo con el daemon vivo dejaría sin pista al siguiente `start`.
pub fn remove_pid_file(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(pid_path(data_dir)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Control de procesos del sistema, que el motor no trae porque no lo puede
/// implementar sin arrastrar `avi-daemon` y `avi-tts`.
///
/// Lo implementa el binario, con las funciones que ya endurecieron el protocolo:
/// `avi_daemon::pid_alive` y `kill_tree_by_pid`, y
/// `avi_tts::resident::{resident_pid_alive, kill_tree_resident_by_pid,
/// sweep_resident_by_image}`.
pub trait ProcessControl {
    /// ¿Vive el proceso de ese PID?
    fn pid_alive(&self, pid: u32) -> bool;
    /// Reclama el árbol exacto de ese PID. Nunca por imagen: el daemon comparte
    /// imagen con el CLI, y matar por imagen mataría al propio invocador —el bug
    /// de v0.18.10 a v0.18.25.
    fn kill_tree_by_pid(&self, pid: u32) -> bool;
    /// ¿Vive el proceso residente con ese PID?
    fn resident_pid_alive(&self, pid: u32) -> bool;
    /// Reclama el árbol del residente por su PID registrado.
    fn kill_tree_resident_by_pid(&self, pid: u32) -> bool;
    /// Último recurso: barrido del residente **por imagen propia** (`qwen_tts`),
    /// que sí es seguro porque la imagen es del producto y no del CLI.
    fn sweep_resident_by_image(&self) -> bool;
    /// Identidad observada del proceso con ese PID, o `None` si no existe o no se
    /// puede observar. Por defecto no se observa nada.
    fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
        None
    }
}

/// Desenlace de la parada, que es lo que permite producir `daemon_stop_failed` y
/// lo que necesita el resumen de la operación para decir cómo reiniciar el daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopOutcome {
    /// El daemon estaba en ejecución antes de parar. Es el dato que el resumen
    /// necesita para indicar cómo reiniciarlo.
    pub was_running: bool,
    /// La parada se completó: no queda daemon ni residente registrado vivos. Con
    /// el pidfile perdido no hay PID de residente que verificar.
    pub stopped: bool,
    /// PID del daemon registrado, si lo había.
    pub pid: Option<u32>,
    /// El pidfile se borró, lo que solo ocurre tras muerte verificada.
    pub pidfile_removed: bool,
    /// Lo que quedó vivo cuando `stopped` es `false`. `None` si la parada fue
    /// completa.
    pub remaining: Option<String>,
}

impl StopOutcome {
    /// Sin daemon ni pidfile: la parada es un no-op verificable. Es el caso que
    /// afirma `stop_is_noop_without_pidfile`.
    pub fn is_noop(&self) -> bool {
        !self.was_running && self.stopped
    }
}

/// Traduce un desenlace que no paró a `daemon_stop_failed`, que es un error
/// con **nada modificado**: la operación que llama aborta antes de tocar el disco.
pub fn require_stopped(outcome: &StopOutcome) -> Result<(), LifecycleError> {
    if outcome.stopped {
        return Ok(());
    }
    Err(LifecycleError::new(
        "daemon_stop_failed",
        format!(
            "no se pudo detener el daemon de {}: {}",
            crate::APP_NAME,
            outcome.remaining.as_deref().unwrap_or("sigue en ejecución")
        ),
    ))
}

/// Ejecuta el protocolo completo de parada sobre `data_dir`.
///
/// `default_addr` es la dirección a la que se conecta cuando no hay pidfile: es un
/// parámetro, y no la constante, porque una prueba necesita apuntar a un puerto
/// donde no hay nada en lugar de al daemon de la máquina que la ejecuta.
pub async fn stop(
    data_dir: &Path,
    default_addr: &str,
    control: &dyn ProcessControl,
) -> StopOutcome {
    let start = std::time::Instant::now();
    let addr = resolve_addr(data_dir, default_addr);
    let (previous_pid, previous_state) = daemon_registered(data_dir, control);
    // "Estaba en ejecución" se decide antes de tocar nada: es lo que el resumen de
    // la operación necesita para decir cómo reiniciarlo, y no puede depender de si
    // la parada tuvo éxito.
    let was_running = previous_state == Registered::Ours || probe(&addr).await;
    crate::faults::trip(crate::faults::FaultPoint::OnDaemonStop).ok();

    // 1) Graceful acotado si responde (no hereda el timeout largo del cliente).
    if probe(&addr).await {
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, post_shutdown(&addr)).await;
        let remaining = STOP_DEADLINE_GLOBAL
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::from_millis(500));
        let wait = remaining.min(Duration::from_secs(3));
        let _ = tokio::time::timeout(wait, wait_health_down(&addr, wait)).await;
    }

    // 2) Árbol preciso por PID si sigue vivo, en las dos plataformas, con la
    // guarda `pid != process::id()`. Sin ella, un pidfile rancio con el PID del
    // propio invocador lo mataría — de ahí el bug de v0.18.10 a v0.18.25.
    // Solo se mata un proceso nuestro: un PID reasignado a otro programa cuenta
    // como muerto y se respeta.
    let (pid, state) = daemon_registered(data_dir, control);
    if state == Registered::Ours {
        if let Some(pid) = pid {
            if pid != std::process::id() {
                control.kill_tree_by_pid(pid);
            }
        }
    }

    // 2b) Residente por identidad estable. Con PID registrado se mata su árbol
    // preciso; sin PID —pidfile perdido— el último recurso es el barrido por
    // imagen `qwen_tts`, seguro porque la imagen es del producto. La
    // verificación por `resident_pid` muerto vive en el paso 3.
    let resident = read_resident_pid(data_dir);
    if resident != 0
        && resident != std::process::id()
        && resident_registered(data_dir, control) == Registered::Ours
    {
        control.kill_tree_resident_by_pid(resident);
    } else if resident == 0 {
        control.sweep_resident_by_image();
    }

    // 3) Verificación con lo que queda del plazo global.
    while start.elapsed() < STOP_DEADLINE_GLOBAL {
        let alive = daemon_registered(data_dir, control).1 == Registered::Ours;
        let resident_alive = resident_alive_by_pid(data_dir, control);
        if !probe(&addr).await && !alive && !resident_alive {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // 4) Veredicto y borrado del pidfile solo tras muerte verificada.
    let (pid_final, final_state) = daemon_registered(data_dir, control);
    let final_alive = final_state == Registered::Ours;
    let stopped = !probe(&addr).await && !final_alive && !resident_alive_by_pid(data_dir, control);
    let remaining = if stopped {
        None
    } else {
        let mut parts = Vec::new();
        if let (Some(pid), true) = (pid_final, final_alive) {
            parts.push(format!("daemon pid {pid}"));
        }
        if probe(&addr).await {
            parts.push(format!("el daemon en {addr} responde"));
        }
        if resident_alive_by_pid(data_dir, control) {
            parts.push(format!("residente pid {}", read_resident_pid(data_dir)));
        }
        Some(parts.join("; "))
    };
    let pidfile_removed = stopped && remove_pid_file(data_dir).is_ok();
    // El fichero ready se borra con la parada verificada, salvo que el PID que
    // publica el propio fichero siga vivo: entonces es la única pista de ese
    // proceso. Un ready ilegible, sin PID o con el PID del propio proceso es rancio.
    if stopped && !ready_file_owner_alive(data_dir, control) {
        let _ = remove_ready_file(data_dir);
    }

    StopOutcome {
        was_running,
        stopped,
        pid: previous_pid,
        pidfile_removed,
        remaining,
    }
}

/// Verificación del residente por su PID registrado, que es el predicado que
/// la tabla de comprobaciones llama "verificación a nivel de sistema".
fn resident_alive_by_pid(data_dir: &Path, control: &dyn ProcessControl) -> bool {
    resident_registered(data_dir, control) == Registered::Ours
}

/// Clasificación de un PID leído de un fichero de estado.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Registered {
    /// No hay proceso con ese PID (o el PID no está registrado).
    Absent,
    /// Hay un proceso con ese PID pero no es el registrado: el número se reasignó.
    /// Cuenta como muerto y nunca se mata.
    Foreign,
    /// El proceso es el registrado.
    Ours,
}

/// Decide si un PID registrado es nuestro. `alive` es la viveza que el llamante
/// observó con la primitiva que corresponde al proceso (daemon o residente).
/// Con identidad registrada, solo es nuestro si la identidad observada coincide;
/// si no se puede observar, es ajeno. Sin identidad registrada (fichero escrito
/// por un binario anterior) se comprueba solo el PID.
pub fn classify_registered(
    control: &dyn ProcessControl,
    pid: u32,
    alive: bool,
    recorded: Option<&ProcessIdentity>,
) -> Registered {
    if pid == 0 || !alive {
        return Registered::Absent;
    }
    match recorded {
        None => Registered::Ours,
        Some(recorded) if control.identity(pid).as_ref() == Some(recorded) => Registered::Ours,
        Some(_) => Registered::Foreign,
    }
}

/// PID del daemon del pidfile y su clasificación.
fn daemon_registered(data_dir: &Path, control: &dyn ProcessControl) -> (Option<u32>, Registered) {
    let Some(file) = read_pid_file(data_dir).filter(|f| f.pid != 0) else {
        return (None, Registered::Absent);
    };
    let state = classify_registered(
        control,
        file.pid,
        control.pid_alive(file.pid),
        file.identity.as_ref(),
    );
    (Some(file.pid), state)
}

/// Clasificación del residente del pidfile.
fn resident_registered(data_dir: &Path, control: &dyn ProcessControl) -> Registered {
    let Some(file) = read_pid_file(data_dir) else {
        return Registered::Absent;
    };
    classify_registered(
        control,
        file.resident_pid,
        file.resident_pid != 0 && control.resident_pid_alive(file.resident_pid),
        file.resident_identity.as_ref(),
    )
}

/// ¿Vive el proceso cuyo PID publica el fichero ready (línea `pid=<n>`)? Un
/// fichero ausente, ilegible, sin esa línea o con el PID del propio proceso no
/// tiene dueño vivo: es rancio.
fn ready_file_owner_alive(data_dir: &Path, control: &dyn ProcessControl) -> bool {
    let Ok(text) = std::fs::read_to_string(data_dir.join(READY_FILE)) else {
        return false;
    };
    let field = |key: &str| {
        text.lines()
            .find_map(|line| line.strip_prefix(key)?.strip_prefix('='))
            .map(|v| v.trim().to_string())
    };
    let Some(pid) = field("pid").and_then(|p| p.parse::<u32>().ok()) else {
        return false;
    };
    let recorded = match (field("start"), field("image")) {
        (Some(start), Some(image)) => Some(ProcessIdentity { start, image }),
        _ => None,
    };
    pid != std::process::id()
        && classify_registered(control, pid, control.pid_alive(pid), recorded.as_ref())
            == Registered::Ours
}

/// Borra el fichero ready, tolerando que ya no exista.
fn remove_ready_file(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(data_dir.join(READY_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Dirección efectiva: la del pidfile si lo hay, y la dada si no.
fn resolve_addr(data_dir: &Path, default_addr: &str) -> String {
    read_addr(data_dir).unwrap_or_else(|| default_addr.to_string())
}

/// Sondea `GET /health` con deadline corto. `false` en cualquier fallo, incluido
/// el de conexión rechazada, que es el caso de "no hay daemon".
pub async fn probe(addr: &str) -> bool {
    match tokio::time::timeout(PROBE_TIMEOUT, request(addr, "GET", "/health")).await {
        Ok(Ok(line)) => status_is_success(&line),
        _ => false,
    }
}

/// `POST /shutdown` acotado. El cuerpo de la respuesta se ignora: lo que importa
/// es que el daemon lo haya recibido, y la verificación es el paso 3.
pub async fn post_shutdown(addr: &str) -> std::io::Result<()> {
    request(addr, "POST", "/shutdown").await.map(|_| ())
}

/// Una petición HTTP/1.1 y la línea de estado de la respuesta.
///
/// No hay cliente HTTP en este crate, y el daemon expone solo cuatro rutas HTTP en
/// `127.0.0.1` sin TLS: una cabecera, un cuerpo de longitud cero y leer hasta el
/// primer salto bastan para ambos usos. Con `Connection: close` el servidor cierra
/// después de responder, así que una sola lectura alcanza para tener la línea de
/// estado sin esperar a un cuerpo que puede no llegar.
async fn request(addr: &str, method: &str, path: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await?;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;
    let mut buf = vec![0u8; 1024];
    let bytes_read = stream.read(&mut buf).await?;
    let text = String::from_utf8_lossy(&buf[..bytes_read]);
    Ok(text.lines().next().unwrap_or_default().to_string())
}

/// `true` si la línea de estado es de la familia 2xx.
fn status_is_success(line: &str) -> bool {
    line.split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .is_some_and(|code| (200..300).contains(&code))
}

/// Espera a que el daemon deje de responder, con el plazo dado.
async fn wait_health_down(addr: &str, timeout: Duration) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !probe(addr).await {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    anyhow::bail!("el daemon no se apagó tras {timeout:?}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use avi_core::exit_codes::ExitCode;
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Puerto efímero que se enlaza y se suelta: garantiza que no hay nada
    /// escuchando, sin depender de que 8765 esté libre en la máquina que ejecuta la
    /// prueba.
    fn dead_port() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
        let addr = listener.local_addr().expect("dirección local");
        drop(listener);
        addr.to_string()
    }

    /// Control de procesos que no hace nada: `pid_alive` dice que no vive nadie, y
    /// se cuentan las llamadas para poder afirmar que el protocolo ni siquiera
    /// llegó a reclamar.
    struct Inert {
        killed: AtomicU32,
        swept: AtomicU32,
    }

    impl Inert {
        fn new() -> Self {
            Self {
                killed: AtomicU32::new(0),
                swept: AtomicU32::new(0),
            }
        }
    }

    impl ProcessControl for Inert {
        fn pid_alive(&self, _pid: u32) -> bool {
            false
        }
        fn kill_tree_by_pid(&self, _pid: u32) -> bool {
            self.killed.fetch_add(1, Ordering::Relaxed);
            true
        }
        fn resident_pid_alive(&self, _pid: u32) -> bool {
            false
        }
        fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
            self.killed.fetch_add(1, Ordering::Relaxed);
            true
        }
        fn sweep_resident_by_image(&self) -> bool {
            self.swept.fetch_add(1, Ordering::Relaxed);
            true
        }
    }

    /// Control que dice que un PID dado vive siempre: sirve para el caso del
    /// pidfile rancio, donde la parada no puede concluir.
    struct Alive {
        pid: u32,
    }

    impl ProcessControl for Alive {
        fn pid_alive(&self, pid: u32) -> bool {
            pid == self.pid
        }
        fn kill_tree_by_pid(&self, _pid: u32) -> bool {
            false
        }
        fn resident_pid_alive(&self, _pid: u32) -> bool {
            false
        }
        fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
            false
        }
        fn sweep_resident_by_image(&self) -> bool {
            false
        }
    }

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("daemon-stop-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Tokio necesita un runtime: el protocolo es asíncrono y la prueba también.
    fn runtime() -> tokio::runtime::Runtime {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("se construye el runtime de la prueba")
    }

    /// Sin pidfile ni daemon, la parada es un no-op verificable: no es un error,
    /// no toca el disco y no reclama ningún árbol.
    #[tokio::test]
    async fn stop_is_noop_without_pidfile() {
        let data = scratch("noop");
        let control = Inert::new();
        let outcome = stop(&data, &dead_port(), &control).await;

        assert!(!outcome.was_running, "no había daemon en ejecución");
        assert!(outcome.stopped, "parar nada es parar bien");
        assert_eq!(outcome.pid, None, "no hay PID que registrar");
        assert!(
            outcome.remaining.is_none(),
            "no queda nada vivo que nombrar"
        );
        assert!(outcome.is_noop(), "es un no-op, y `is_noop` lo reconoce");
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "sin PID no se reclama ningún árbol"
        );
        assert!(
            !pid_path(&data).exists(),
            "no se crea un pidfile para poder pararlo"
        );
        // Y `require_stopped` lo acepta: un no-op no produce `daemon_stop_failed`.
        assert!(require_stopped(&outcome).is_ok());

        // Con un pidfile que apunta a un proceso muerto, el desenlace es el mismo
        // salvo que ahora sí hubo un PID: es lo que permite al resumen decir que
        // no hace falta reiniciar nada.
        write_pid(&data, 999_999, "127.0.0.1:1", 0, None).unwrap();
        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(!outcome.was_running, "el PID registrado no está vivo");
        assert_eq!(
            outcome.pid,
            Some(999_999),
            "pero se conserva para el resumen"
        );
        assert!(outcome.stopped);
        assert!(
            outcome.pidfile_removed,
            "el pidfile se borra tras verificar"
        );
        assert!(!pid_path(&data).exists());
        std::fs::remove_dir_all(&data).ok();
    }

    /// La dirección de parada sigue el puerto de `AVI_DAEMON_PORT` como el
    /// arranque del daemon: recorta espacios, acepta `0` y cae al valor por
    /// defecto si falta o no es un puerto válido.
    #[test]
    fn addr_for_port_env_follows_daemon_port() {
        assert_eq!(addr_for_port_env(Some("9123")), "127.0.0.1:9123");
        assert_eq!(addr_for_port_env(Some("0")), "127.0.0.1:0");
        assert_eq!(addr_for_port_env(Some(" 9123 ")), "127.0.0.1:9123");
        assert_eq!(addr_for_port_env(None), DEFAULT_ADDR);
        assert_eq!(addr_for_port_env(Some("abc")), DEFAULT_ADDR);
    }

    /// Sin pidfile, el cliente busca el daemon en el puerto de `AVI_DAEMON_PORT`;
    /// con pidfile manda la dirección publicada en él.
    #[test]
    fn client_addr_without_pidfile_follows_daemon_port() {
        let data = scratch("client-addr-port");
        assert_eq!(
            resolve_client_addr_with(&data, Some("9123")),
            "127.0.0.1:9123"
        );
        assert_eq!(resolve_client_addr_with(&data, None), DEFAULT_ADDR);

        write_pid(&data, 4242, "127.0.0.1:7001", 77, None).unwrap();
        assert_eq!(
            resolve_client_addr_with(&data, Some("9123")),
            "127.0.0.1:7001",
            "el pidfile prevalece sobre la variable"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// El pidfile sobrevive a un fichero a medio escribir, y el protocolo no se
    /// cae: lectura tolerante, escritura atómica y ninguna mezcla de datos entre
    /// dos escrituras.
    #[test]
    fn pidfile_survives_partial_write() {
        let data = scratch("pidfile");
        let path = pid_path(&data);
        let control = Inert::new();

        // 1. Un pidfile truncado —el caso que deja un corte a mitad de escritura—
        //    se lee como ausente, no como un error ni como un PID de basura.
        crate::test_support::write_file(&path, "{\"pid\": 4242, \"addr\": \"127.0.0.1:876");
        assert_eq!(read_pid(&data), None, "un JSON truncado no da PID");
        assert_eq!(read_resident_pid(&data), 0, "ni PID de residente");
        assert_eq!(read_addr(&data), None, "ni dirección");
        assert_eq!(
            resolve_client_addr_with(&data, None),
            DEFAULT_ADDR,
            "se cae al valor por defecto"
        );
        assert!(
            read_pid_file(&data).is_none(),
            "y el parseo entero falla limpio"
        );

        // 2. Un esquema viejo sin `resident_pid` ni `addr` se lee igual.
        crate::test_support::write_file(&path, r#"{"pid": 4242}"#);
        assert_eq!(read_pid(&data), Some(4242));
        assert_eq!(read_resident_pid(&data), 0, "sin el campo, desconocido");
        assert_eq!(read_addr(&data), None);
        assert_eq!(resolve_client_addr_with(&data, None), DEFAULT_ADDR);

        // 3. Un `pid` inválido no se interpreta.
        for garbage in ["{}", r#"{"pid": "x"}"#, "[]", "no soy json", ""] {
            crate::test_support::write_file(&path, garbage);
            assert_eq!(read_pid(&data), None, "basura: {garbage:?}");
        }

        // 4. La escritura es atómica: temporal hermano y renombrado, sin residuo.
        write_pid(&data, 4242, "127.0.0.1:8765", 77, None).unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            serde_json::from_str::<PidFile>(&written).is_ok(),
            "lo que hay en disco es JSON completo, nunca medio: {written}"
        );
        assert_eq!(read_pid(&data), Some(4242));
        assert_eq!(read_resident_pid(&data), 77);
        assert_eq!(read_addr(&data).as_deref(), Some("127.0.0.1:8765"));
        let temp = path.with_extension("pid.tmp");
        assert!(!temp.exists(), "el temporal no sobrevive al renombrado");
        assert_eq!(
            std::fs::read_dir(&data).unwrap().count(),
            1,
            "solo queda el pidfile"
        );

        // 5. Intercalar muchas escrituras y lecturas: cada lectura devuelve un
        //    estado completo y coherente, nunca una mezcla de dos escrituras.
        for round in 0..64u32 {
            let pid = 1000 + round;
            write_pid(&data, pid, "127.0.0.1:9", round, None).unwrap();
            if let (Some(loaded), Some(addr)) = (read_pid(&data), read_addr(&data)) {
                assert_eq!(loaded, pid, "el PID leído es el último escrito");
                assert_eq!(
                    addr, "127.0.0.1:9",
                    "la dirección leída es la última escrita"
                );
                assert_eq!(read_resident_pid(&data), round, "el residente también");
            }
        }

        // 6. Con el pidfile truncado, la parada sigue siendo un no-op: es el
        //    contrato de "nada modificado" del que depende `daemon_stop_failed`.
        crate::test_support::write_file(&path, "{\"pid\": 4242, \"add");
        let outcome = runtime().block_on(stop(&data, &dead_port(), &control));
        assert!(!outcome.was_running);
        assert!(
            outcome.stopped,
            "un pidfile ilegible no impide parar: no hay daemon"
        );
        assert_eq!(outcome.pid, None);
        std::fs::remove_dir_all(&data).ok();
    }

    /// La guarda anti-auto-muerte sobrevive al traslado: un pidfile rancio que
    /// lleva el PID del propio proceso **no** lo mata. Es el bug de v0.18.10 a
    /// v0.18.25, y por eso la comprobación no es "si el PID existe, mátalo".
    #[tokio::test]
    async fn stop_never_kills_its_own_process() {
        let data = scratch("autoguardia");
        let control = Inert::new();
        let own = std::process::id();
        write_pid(&data, own, &dead_port(), own, None).unwrap();

        let outcome = stop(&data, &dead_port(), &control).await;
        assert_eq!(
            outcome.pid,
            Some(own),
            "el PID rancio se conserva en el desenlace"
        );
        assert!(
            outcome.stopped,
            "no hay nada que parar más allá del propio proceso"
        );
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "la guarda `pid != process::id()` impide reclamar el árbol propio"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un daemon que no se puede parar produce `daemon_stop_failed` con el código
    /// de la tabla cerrada, y no se toca nada.
    #[tokio::test]
    async fn unstoppable_daemon_yields_daemon_stop_failed() {
        let data = scratch("falla");
        let pid = 424_242;
        let control = Alive { pid };
        write_pid(&data, pid, &dead_port(), 0, None).unwrap();

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.was_running, "el daemon seguía vivo");
        assert!(!outcome.stopped, "y no se pudo parar");
        assert!(
            !outcome.pidfile_removed,
            "el pidfile se conserva como pista"
        );
        assert!(pid_path(&data).exists(), "el pidfile sigue en disco");
        let remaining = outcome
            .remaining
            .as_deref()
            .expect("se nombra lo que quedó vivo");
        assert!(remaining.contains(&pid.to_string()), "{remaining}");

        let err = require_stopped(&outcome).expect_err("sin parada no hay `self install`");
        assert_eq!(err.reason, "daemon_stop_failed");
        assert_eq!(
            ExitCode::from_reason(err.reason).code(),
            16,
            "`DaemonStopFailed = 16` de la tabla única"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// El barrido por imagen del residente es el último recurso y solo se dispara
    /// sin PID registrado: con PID, la vía es el árbol preciso.
    #[tokio::test]
    async fn resident_sweep_only_without_registered_pid() {
        let data = scratch("residente");

        let without_pid = Inert::new();
        write_pid(&data, 999_999, &dead_port(), 0, None).unwrap();
        stop(&data, &dead_port(), &without_pid).await;
        assert_eq!(
            without_pid.swept.load(Ordering::Relaxed),
            1,
            "sin PID de residente se barre por imagen propia"
        );

        let with_pid = Inert::new();
        write_pid(&data, 999_999, &dead_port(), 888_888, None).unwrap();
        stop(&data, &dead_port(), &with_pid).await;
        assert_eq!(
            with_pid.swept.load(Ordering::Relaxed),
            0,
            "con PID de residente no se barre por imagen: se mata su árbol"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// Control con el daemon muerto y un residente que sigue vivo pase lo que
    /// pase: reclamar su árbol no lo mata.
    struct ResidentSurvives {
        resident: u32,
    }

    impl ProcessControl for ResidentSurvives {
        fn pid_alive(&self, _pid: u32) -> bool {
            false
        }
        fn kill_tree_by_pid(&self, _pid: u32) -> bool {
            false
        }
        fn resident_pid_alive(&self, pid: u32) -> bool {
            pid == self.resident
        }
        fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
            false
        }
        fn sweep_resident_by_image(&self) -> bool {
            false
        }
    }

    /// Contenido del fichero ready de éxito tal como lo publica el daemon.
    fn ready_content(pid: u32) -> String {
        format!("addr=127.0.0.1:8765\nwarm=ok\npid={pid}\n")
    }

    /// Tras una parada verificada, el fichero ready desaparece junto al pidfile:
    /// un ready rancio haría creer al siguiente arranque que el daemon sigue listo.
    #[tokio::test]
    async fn stop_removes_ready_file_after_verified_stop() {
        let data = scratch("ready-borrado");
        write_pid(&data, 999_999, &dead_port(), 0, None).unwrap();
        crate::test_support::write_file(&data.join(READY_FILE), &ready_content(999_999));

        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped, "el daemon está muerto");
        assert!(!pid_path(&data).exists(), "el pidfile se borra");
        assert!(
            !data.join(READY_FILE).exists(),
            "el fichero ready se borra tras la parada verificada"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// Si el PID que contiene el propio fichero ready sigue vivo, el fichero no se
    /// borra aunque el pidfile no lo mencione: es la única pista de ese proceso.
    #[tokio::test]
    async fn stop_keeps_ready_file_while_its_pid_is_alive() {
        let data = scratch("ready-vivo");
        let ready_pid = 777_001;
        crate::test_support::write_file(&data.join(READY_FILE), &ready_content(ready_pid));

        let outcome = stop(&data, &dead_port(), &Alive { pid: ready_pid }).await;
        assert!(
            outcome.stopped,
            "sin pidfile ni respuesta no hay nada que parar"
        );
        assert!(
            data.join(READY_FILE).exists(),
            "el PID del ready sigue vivo: el fichero se conserva"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un fichero ready ilegible o con el PID del propio proceso es rancio y se
    /// borra, incluso si el control de procesos diera ese PID por vivo.
    #[tokio::test]
    async fn stop_removes_stale_ready_file() {
        let data = scratch("ready-rancio");
        let ready = data.join(READY_FILE);
        let own = std::process::id();

        crate::test_support::write_file(&ready, "basura sin formato");
        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped);
        assert!(!ready.exists(), "un ready ilegible es rancio y se borra");

        crate::test_support::write_file(&ready, &ready_content(own));
        let outcome = stop(&data, &dead_port(), &Alive { pid: own }).await;
        assert!(outcome.stopped);
        assert!(
            !ready.exists(),
            "un ready con el PID del propio proceso es rancio y se borra"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un residente registrado que sigue vivo tras los intentos de parada impide
    /// dar la parada por completa, aunque el daemon haya muerto: se conserva el
    /// pidfile como pista y se nombra al residente.
    #[tokio::test]
    async fn surviving_resident_prevents_complete_stop() {
        let data = scratch("residente-vivo");
        let resident = 888_888;
        write_pid(&data, 999_999, &dead_port(), resident, None).unwrap();

        let outcome = stop(&data, &dead_port(), &ResidentSurvives { resident }).await;
        assert!(
            !outcome.stopped,
            "el residente sigue vivo: la parada no está completa"
        );
        assert!(
            !outcome.pidfile_removed,
            "el pidfile se conserva mientras viva el residente"
        );
        assert!(pid_path(&data).exists(), "el pidfile sigue en disco");
        let remaining = outcome
            .remaining
            .as_deref()
            .expect("se nombra lo que quedó vivo");
        assert!(remaining.contains(&resident.to_string()), "{remaining}");
        std::fs::remove_dir_all(&data).ok();
    }

    fn identity_of(start: &str, image: &str) -> ProcessIdentity {
        ProcessIdentity {
            start: start.to_string(),
            image: image.to_string(),
        }
    }

    /// Control con procesos vivos e identidades configurables. Matar un PID lo
    /// retira de los vivos y se cuentan los kills, para afirmar a quién se mató.
    struct Observed {
        alive: std::sync::Mutex<Vec<u32>>,
        identities: Vec<(u32, ProcessIdentity)>,
        killed: AtomicU32,
    }

    impl Observed {
        fn new(alive: &[u32], identities: &[(u32, ProcessIdentity)]) -> Self {
            Self {
                alive: std::sync::Mutex::new(alive.to_vec()),
                identities: identities.to_vec(),
                killed: AtomicU32::new(0),
            }
        }

        fn is_alive(&self, pid: u32) -> bool {
            self.alive.lock().unwrap().contains(&pid)
        }

        fn kill(&self, pid: u32) -> bool {
            self.killed.fetch_add(1, Ordering::Relaxed);
            self.alive.lock().unwrap().retain(|p| *p != pid);
            true
        }
    }

    impl ProcessControl for Observed {
        fn pid_alive(&self, pid: u32) -> bool {
            self.is_alive(pid)
        }
        fn kill_tree_by_pid(&self, pid: u32) -> bool {
            self.kill(pid)
        }
        fn resident_pid_alive(&self, pid: u32) -> bool {
            self.is_alive(pid)
        }
        fn kill_tree_resident_by_pid(&self, pid: u32) -> bool {
            self.kill(pid)
        }
        fn sweep_resident_by_image(&self) -> bool {
            false
        }
        fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
            if !self.is_alive(pid) {
                return None;
            }
            self.identities
                .iter()
                .find(|(p, _)| *p == pid)
                .map(|(_, id)| id.clone())
        }
    }

    /// El pidfile guarda y devuelve las dos identidades.
    #[test]
    fn pid_file_roundtrips_identities() {
        let data = scratch("identidades-ida-vuelta");
        write_pid(
            &data,
            4242,
            "127.0.0.1:7001",
            77,
            Some(identity_of("111", "daemon.exe")),
        )
        .unwrap();
        let mut file = read_pid_file(&data).unwrap();
        assert_eq!(file.identity, Some(identity_of("111", "daemon.exe")));
        assert_eq!(file.resident_identity, None);

        file.resident_identity = Some(identity_of("222", "qwen_tts.exe"));
        crate::test_support::write_file(&pid_path(&data), &serde_json::to_string(&file).unwrap());
        let back = read_pid_file(&data).unwrap();
        assert_eq!(back, file);
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un pidfile escrito sin identidades se sigue leyendo, con ambas ausentes.
    #[test]
    fn pid_file_without_identities_still_reads() {
        let data = scratch("sin-identidades");
        crate::test_support::write_file(
            &pid_path(&data),
            r#"{"pid": 4242, "addr": "127.0.0.1:7001", "resident_pid": 77}"#,
        );
        let file = read_pid_file(&data).expect("se lee");
        assert_eq!(file.pid, 4242);
        assert_eq!(file.resident_pid, 77);
        assert_eq!(file.identity, None);
        assert_eq!(file.resident_identity, None);
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un PID de daemon reasignado a otro programa no bloquea la parada y no se mata.
    #[tokio::test]
    async fn stop_spares_daemon_pid_with_foreign_identity() {
        let data = scratch("daemon-ajeno");
        let pid = 424_243;
        write_pid(
            &data,
            pid,
            &dead_port(),
            0,
            Some(identity_of("111", "ai-voice-interconnector.exe")),
        )
        .unwrap();
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "otro.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(!outcome.was_running, "el PID registrado es de un ajeno");
        assert!(outcome.stopped, "un proceso ajeno no bloquea la parada");
        assert!(outcome.pidfile_removed);
        assert!(outcome.remaining.is_none());
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "no se mata al ajeno"
        );
        assert!(control.is_alive(pid));
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un PID de residente reasignado a otro programa no bloquea la parada y no se mata.
    #[tokio::test]
    async fn stop_spares_resident_pid_with_foreign_identity() {
        let data = scratch("residente-ajeno");
        let resident = 424_244;
        write_pid(&data, 999_999, &dead_port(), resident, None).unwrap();
        let mut file = read_pid_file(&data).unwrap();
        file.resident_identity = Some(identity_of("222", "qwen_tts.exe"));
        crate::test_support::write_file(&pid_path(&data), &serde_json::to_string(&file).unwrap());
        let control = Observed::new(&[resident], &[(resident, identity_of("999", "otro.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.stopped, "un proceso ajeno no bloquea la parada");
        assert!(outcome.pidfile_removed);
        assert!(outcome.remaining.is_none());
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "no se mata al ajeno"
        );
        assert!(control.is_alive(resident));
        std::fs::remove_dir_all(&data).ok();
    }

    /// Con la identidad registrada coincidente, el daemon es nuestro y se mata.
    #[tokio::test]
    async fn stop_kills_daemon_pid_with_matching_identity() {
        let data = scratch("daemon-propio");
        let pid = 424_245;
        let id = identity_of("111", "ai-voice-interconnector.exe");
        write_pid(&data, pid, &dead_port(), 0, Some(id.clone())).unwrap();
        let control = Observed::new(&[pid], &[(pid, id)]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.was_running);
        assert!(outcome.stopped);
        assert_eq!(control.killed.load(Ordering::Relaxed), 1);
        assert!(!control.is_alive(pid));
        std::fs::remove_dir_all(&data).ok();
    }

    /// Sin identidad registrada, la comprobación es solo por PID: el vivo se mata.
    #[tokio::test]
    async fn stop_without_recorded_identity_checks_pid_only() {
        let data = scratch("daemon-sin-identidad");
        let pid = 424_246;
        write_pid(&data, pid, &dead_port(), 0, None).unwrap();
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "otro.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.was_running);
        assert!(outcome.stopped);
        assert_eq!(control.killed.load(Ordering::Relaxed), 1);
        std::fs::remove_dir_all(&data).ok();
    }

    /// Un ready cuyo PID pertenece ya a otro programa es rancio y se borra.
    #[tokio::test]
    async fn stop_removes_ready_file_whose_pid_is_foreign() {
        let data = scratch("ready-ajeno");
        let pid = 424_247;
        crate::test_support::write_file(
            &data.join(READY_FILE),
            &format!(
                "{}start=111\nimage=ai-voice-interconnector.exe\n",
                ready_content(pid)
            ),
        );
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "otro.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.stopped);
        assert!(
            !data.join(READY_FILE).exists(),
            "el PID del ready es de un ajeno: el fichero es rancio"
        );
        assert_eq!(control.killed.load(Ordering::Relaxed), 0);
        std::fs::remove_dir_all(&data).ok();
    }

    /// Los plazos del protocolo son los de hoy y no cambian con el traslado.
    #[test]
    fn protocol_deadlines_are_preserved() {
        assert_eq!(STOP_DEADLINE_GLOBAL, Duration::from_secs(8));
        assert_eq!(SHUTDOWN_TIMEOUT, Duration::from_millis(1500));
        assert_eq!(PROBE_TIMEOUT, Duration::from_millis(500));
        assert!(
            PROBE_TIMEOUT < SHUTDOWN_TIMEOUT && SHUTDOWN_TIMEOUT < STOP_DEADLINE_GLOBAL,
            "el presupuesto está anidado: un sondeo no puede agotar el del POST, ni el POST el global"
        );
    }

    /// La línea de estado se interpreta bien, que es de lo que depende que un
    /// `200` cuente como vivo y un `503` como muerto.
    #[test]
    fn status_line_parsing() {
        assert!(status_is_success("HTTP/1.1 200 OK"));
        assert!(status_is_success("HTTP/1.1 204 No Content"));
        assert!(!status_is_success("HTTP/1.1 503 Service Unavailable"));
        assert!(!status_is_success("HTTP/1.1 500 Internal Server Error"));
        assert!(!status_is_success(""));
        assert!(!status_is_success("basura"));
    }
}
