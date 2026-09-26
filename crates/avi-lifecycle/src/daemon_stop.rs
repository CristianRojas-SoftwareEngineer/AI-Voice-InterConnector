//! Protocolo de parada del daemon y del proceso residente (§9.3, paso 5; §9.5,
//! paso 5; §9.6).
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
//! en `%APPDATA%`— **no se encuentra**. Es un supuesto asumido (§14.2, §14.3: la
//! audiencia previa es nula) y está escrito en el módulo para que no se lea como un
//! olvido.
//!
//! **No hay cliente HTTP.** El plan declara que este crate se construye sin él, así
//! que los dos puntos del protocolo —`POST /shutdown` y `GET /health`— viajan por
//! `tokio::net::TcpStream` con HTTP/1.1 a pelo. Son cuatro líneas de petición y
//! una de lectura de la línea de estado, contra las dos dependencias que el plan
//! declara que este crate no tiene.

use crate::LifecycleError;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Plazo global de la parada (§9.3, paso 5). Viene de `STOP_DEADLINE_GLOBAL` en el
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

/// Nombre del pidfile dentro de la raíz de datos (§7: `daemon.pid`).
pub const PID_FILE: &str = "daemon.pid";

/// Ruta del pidfile bajo la raíz de datos de §7.
pub fn pid_path(data_dir: &Path) -> PathBuf {
    data_dir.join(PID_FILE)
}

/// Ruta del pidfile con las raíces de §7 ya resueltas.
pub fn pid_path_default() -> PathBuf {
    pid_path(&crate::data_dir())
}

/// Contenido del pidfile tal como está en disco. El esquema es plano a propósito:
/// un binario antiguo tiene que poder leerlo y un binario nuevo tiene que poder
/// ignorarlo sin romper (§8.1, el mismo criterio que el recibo).
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
}

/// Escribe el pidfile de forma atómica: temporal hermano y renombrado. El handler
/// Ctrl+C no depende solo de él —también lleva el PID en memoria—, pero la escritura
/// tardía y atómica es lo que evita que un lector vea medio JSON.
pub fn write_pid(data_dir: &Path, pid: u32, addr: &str, resident_pid: u32) -> anyhow::Result<()> {
    let path = pid_path(data_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let contenido = serde_json::to_string_pretty(&PidFile {
        pid,
        addr: Some(addr.to_string()),
        started_at: Some(crate::receipt::now_rfc3339()),
        resident_pid,
    })?;
    let temporal = path.with_extension("pid.tmp");
    std::fs::write(&temporal, contenido.as_bytes())?;
    match std::fs::rename(&temporal, &path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&temporal);
            Err(e.into())
        }
    }
}

/// Lee el pidfile completo. `Ok(None)` si no hay, si está ilegible o si es un
/// esquema que no se entiende: lectura tolerante, el llamante cae al valor por
/// defecto y nunca falla.
pub fn read_pid_file(data_dir: &Path) -> Option<PidFile> {
    let texto = std::fs::read_to_string(pid_path(data_dir)).ok()?;
    serde_json::from_str(&texto).ok()
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

/// Dirección del cliente: la del pidfile cuando existe, y la literal cuando no.
///
/// Este es el **supuesto asumido de D4**: la raíz de datos es la vigente, así que un
/// daemon de la versión anterior, con su pidfile en la raíz antigua, no se
/// encuentra y la conexión va a la literal.
pub fn resolve_client_addr(data_dir: &Path) -> String {
    read_addr(data_dir).unwrap_or_else(|| DEFAULT_ADDR.to_string())
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
}

/// Desenlace de la parada, que es lo que permite producir `daemon_stop_failed` y
/// lo que necesita el resumen de §9.4 para decir cómo reiniciar el daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopOutcome {
    /// El daemon estaba en ejecución antes de parar. Es el dato que el resumen
    /// necesita para indicar cómo reiniciarlo.
    pub was_running: bool,
    /// La parada se completó: no queda daemon ni residente vivos.
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

/// Traduce un desenlace que no paró a `daemon_stop_failed` (§9.1), que es un error
/// con **nada modificado**: la operación que llama aborta antes de tocar el disco.
pub fn require_stopped(outcome: &StopOutcome) -> Result<(), LifecycleError> {
    if outcome.stopped {
        return Ok(());
    }
    Err(LifecycleError::new(
        "daemon_stop_failed",
        16,
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
    let pid_previo = read_pid(data_dir);
    // "Estaba en ejecución" se decide antes de tocar nada: es lo que el resumen de
    // §9.4 necesita para decir cómo reiniciarlo, y no puede depender de si la
    // parada tuvo éxito.
    let was_running = pid_previo.is_some_and(|pid| control.pid_alive(pid)) || probe(&addr).await;
    crate::faults::trip(crate::faults::FaultPoint::OnDaemonStop).ok();

    // 1) Graceful acotado si responde (no hereda el timeout largo del cliente).
    if probe(&addr).await {
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, post_shutdown(&addr)).await;
        let restante = STOP_DEADLINE_GLOBAL
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::from_millis(500));
        let espera = restante.min(Duration::from_secs(3));
        let _ = tokio::time::timeout(espera, wait_health_down(&addr, espera)).await;
    }

    // 2) Árbol preciso por PID si sigue vivo, en las dos plataformas, con la
    // guarda `pid != process::id()`. Sin ella, un pidfile rancio con el PID del
    // propio invocador lo mataría — de ahí el bug de v0.18.10 a v0.18.25.
    let pid = read_pid(data_dir);
    let alive = pid.is_some_and(|pid| control.pid_alive(pid));
    if probe(&addr).await || alive {
        if let Some(pid) = pid {
            if pid != 0 && pid != std::process::id() {
                control.kill_tree_by_pid(pid);
            }
        }
    }

    // 2b) Residente por identidad estable. Con PID registrado se mata su árbol
    // preciso; sin PID —pidfile perdido— el último recurso es el barrido por
    // imagen `qwen_tts`, seguro porque la imagen es del producto. La
    // verificación por `resident_pid` muerto vive en el paso 3.
    let resident = read_resident_pid(data_dir);
    if resident != 0 && resident != std::process::id() && control.resident_pid_alive(resident) {
        control.kill_tree_resident_by_pid(resident);
    } else if resident == 0 {
        control.sweep_resident_by_image();
    }

    // 3) Verificación con lo que queda del plazo global.
    while start.elapsed() < STOP_DEADLINE_GLOBAL {
        let vivo = read_pid(data_dir).is_some_and(|pid| control.pid_alive(pid));
        let residente_vivo = resident_alive_by_pid(data_dir, control);
        if !probe(&addr).await && !vivo && !residente_vivo {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // 4) Veredicto y borrado del pidfile solo tras muerte verificada.
    let pid_final = read_pid(data_dir);
    let final_alive = pid_final.is_some_and(|pid| control.pid_alive(pid));
    let stopped = !probe(&addr).await && !final_alive;
    let remaining = if stopped {
        None
    } else {
        let mut partes = Vec::new();
        if let Some(pid) = pid_final {
            partes.push(format!("daemon pid {pid}"));
        }
        if probe(&addr).await {
            partes.push(format!("el daemon en {addr} responde"));
        }
        if resident_alive_by_pid(data_dir, control) {
            partes.push(format!("residente pid {}", read_resident_pid(data_dir)));
        }
        Some(partes.join("; "))
    };
    let pidfile_removed = stopped && remove_pid_file(data_dir).is_ok();

    StopOutcome {
        was_running,
        stopped,
        pid: pid_previo,
        pidfile_removed,
        remaining,
    }
}

/// Verificación del residente por su PID registrado, que es el predicado que
/// §9.1 llama "verificación a nivel de sistema" en el paso 3.
fn resident_alive_by_pid(data_dir: &Path, control: &dyn ProcessControl) -> bool {
    let pid = read_resident_pid(data_dir);
    pid != 0 && control.resident_pid_alive(pid)
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
async fn request(addr: &str, metodo: &str, ruta: &str) -> std::io::Result<String> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    let mut stream = TcpStream::connect(addr).await?;
    let peticion = format!(
        "{metodo} {ruta} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\nContent-Length: 0\r\n\r\n"
    );
    stream.write_all(peticion.as_bytes()).await?;
    stream.flush().await?;
    let mut buf = vec![0u8; 1024];
    let leidos = stream.read(&mut buf).await?;
    let texto = String::from_utf8_lossy(&buf[..leidos]);
    Ok(texto.lines().next().unwrap_or_default().to_string())
}

/// `true` si la línea de estado es de la familia 2xx.
fn status_is_success(linea: &str) -> bool {
    linea
        .split_whitespace()
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
    use std::sync::atomic::{AtomicU32, Ordering};

    /// Puerto efímero que se enlaza y se suelta: garantiza que no hay nada
    /// escuchando, sin depender de que 8765 esté libre en la máquina que ejecuta la
    /// prueba.
    fn puerto_muerto() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("se enlaza un puerto");
        let addr = listener.local_addr().expect("dirección local");
        drop(listener);
        addr.to_string()
    }

    /// Control de procesos que no hace nada: `pid_alive` dice que no vive nadie, y
    /// se cuentan las llamadas para poder afirmar que el protocolo ni siquiera
    /// llegó a reclamar.
    struct Inerte {
        killed: AtomicU32,
        swept: AtomicU32,
    }

    impl Inerte {
        fn new() -> Self {
            Self {
                killed: AtomicU32::new(0),
                swept: AtomicU32::new(0),
            }
        }
    }

    impl ProcessControl for Inerte {
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
    struct Vivo {
        pid: u32,
    }

    impl ProcessControl for Vivo {
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
        let control = Inerte::new();
        let outcome = stop(&data, &puerto_muerto(), &control).await;

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
        write_pid(&data, 999_999, "127.0.0.1:1", 0).unwrap();
        let outcome = stop(&data, &puerto_muerto(), &control).await;
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

    /// El pidfile sobrevive a un fichero a medio escribir, y el protocolo no se
    /// cae: lectura tolerante, escritura atómica y ninguna mezcla de datos entre
    /// dos escrituras.
    #[test]
    fn pidfile_survives_partial_write() {
        let data = scratch("pidfile");
        let ruta = pid_path(&data);
        let control = Inerte::new();

        // 1. Un pidfile truncado —el caso que deja un corte a mitad de escritura—
        //    se lee como ausente, no como un error ni como un PID de basura.
        crate::test_support::write_file(&ruta, "{\"pid\": 4242, \"addr\": \"127.0.0.1:876");
        assert_eq!(read_pid(&data), None, "un JSON truncado no da PID");
        assert_eq!(read_resident_pid(&data), 0, "ni PID de residente");
        assert_eq!(read_addr(&data), None, "ni dirección");
        assert_eq!(
            resolve_client_addr(&data),
            DEFAULT_ADDR,
            "se cae al valor por defecto"
        );
        assert!(
            read_pid_file(&data).is_none(),
            "y el parseo entero falla limpio"
        );

        // 2. Un esquema viejo sin `resident_pid` ni `addr` se lee igual.
        crate::test_support::write_file(&ruta, r#"{"pid": 4242}"#);
        assert_eq!(read_pid(&data), Some(4242));
        assert_eq!(read_resident_pid(&data), 0, "sin el campo, desconocido");
        assert_eq!(read_addr(&data), None);
        assert_eq!(resolve_client_addr(&data), DEFAULT_ADDR);

        // 3. Un `pid` inválido no se interpreta.
        for basura in ["{}", r#"{"pid": "x"}"#, "[]", "no soy json", ""] {
            crate::test_support::write_file(&ruta, basura);
            assert_eq!(read_pid(&data), None, "basura: {basura:?}");
        }

        // 4. La escritura es atómica: temporal hermano y renombrado, sin residuo.
        write_pid(&data, 4242, "127.0.0.1:8765", 77).unwrap();
        let escrito = std::fs::read_to_string(&ruta).unwrap();
        assert!(
            serde_json::from_str::<PidFile>(&escrito).is_ok(),
            "lo que hay en disco es JSON completo, nunca medio: {escrito}"
        );
        assert_eq!(read_pid(&data), Some(4242));
        assert_eq!(read_resident_pid(&data), 77);
        assert_eq!(read_addr(&data).as_deref(), Some("127.0.0.1:8765"));
        let temporal = ruta.with_extension("pid.tmp");
        assert!(!temporal.exists(), "el temporal no sobrevive al renombrado");
        assert_eq!(
            std::fs::read_dir(&data).unwrap().count(),
            1,
            "solo queda el pidfile"
        );

        // 5. Intercalar muchas escrituras y lecturas: cada lectura devuelve un
        //    estado completo y coherente, nunca una mezcla de dos escrituras.
        for ronda in 0..64u32 {
            let pid = 1000 + ronda;
            write_pid(&data, pid, "127.0.0.1:9", ronda).unwrap();
            if let (Some(leido), Some(addr)) = (read_pid(&data), read_addr(&data)) {
                assert_eq!(leido, pid, "el PID leído es el último escrito");
                assert_eq!(
                    addr, "127.0.0.1:9",
                    "la dirección leída es la última escrita"
                );
                assert_eq!(read_resident_pid(&data), ronda, "el residente también");
            }
        }

        // 6. Con el pidfile truncado, la parada sigue siendo un no-op: es el
        //    contrato de "nada modificado" del que depende `daemon_stop_failed`.
        crate::test_support::write_file(&ruta, "{\"pid\": 4242, \"add");
        let outcome = runtime().block_on(stop(&data, &puerto_muerto(), &control));
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
        let control = Inerte::new();
        let propio = std::process::id();
        write_pid(&data, propio, &puerto_muerto(), propio).unwrap();

        let outcome = stop(&data, &puerto_muerto(), &control).await;
        assert_eq!(
            outcome.pid,
            Some(propio),
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
        let control = Vivo { pid };
        write_pid(&data, pid, &puerto_muerto(), 0).unwrap();

        let outcome = stop(&data, &puerto_muerto(), &control).await;
        assert!(outcome.was_running, "el daemon seguía vivo");
        assert!(!outcome.stopped, "y no se pudo parar");
        assert!(
            !outcome.pidfile_removed,
            "el pidfile se conserva como pista"
        );
        assert!(pid_path(&data).exists(), "el pidfile sigue en disco");
        let restante = outcome
            .remaining
            .as_deref()
            .expect("se nombra lo que quedó vivo");
        assert!(restante.contains(&pid.to_string()), "{restante}");

        let err = require_stopped(&outcome).expect_err("sin parada no hay `self install`");
        assert_eq!(err.reason, "daemon_stop_failed");
        assert_eq!(
            err.exit_code, 16,
            "`DaemonStopFailed = 16` de la tabla cerrada"
        );
        std::fs::remove_dir_all(&data).ok();
    }

    /// El barrido por imagen del residente es el último recurso y solo se dispara
    /// sin PID registrado: con PID, la vía es el árbol preciso.
    #[tokio::test]
    async fn resident_sweep_only_without_registered_pid() {
        let data = scratch("residente");

        let sin_pid = Inerte::new();
        write_pid(&data, 999_999, &puerto_muerto(), 0).unwrap();
        stop(&data, &puerto_muerto(), &sin_pid).await;
        assert_eq!(
            sin_pid.swept.load(Ordering::Relaxed),
            1,
            "sin PID de residente se barre por imagen propia"
        );

        let con_pid = Inerte::new();
        write_pid(&data, 999_999, &puerto_muerto(), 888_888).unwrap();
        stop(&data, &puerto_muerto(), &con_pid).await;
        assert_eq!(
            con_pid.swept.load(Ordering::Relaxed),
            0,
            "con PID de residente no se barre por imagen: se mata su árbol"
        );
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
