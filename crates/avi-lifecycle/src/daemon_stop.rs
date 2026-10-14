// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

//! Protocolo de parada del daemon y del proceso residente. Es el paso 5 de la
//! instalación y de la desinstalación, y un paso propio de la limpieza.
//!
//! El protocolo es: `daemon.pid` con esquema plano, `POST /shutdown` acotado,
//! árbol preciso por PID con guarda anti-auto-muerte, verificación a nivel de
//! sistema y plazo global. Los plazos son **8 s** de plazo global y **1500 ms**
//! para el `POST /shutdown`.
//!
//! **El control de procesos no vive aquí.** Matar un árbol en Windows es
//! `taskkill /T` y en Unix es matar el grupo de sesión, y esa lógica está en
//! `avi-daemon` (`kill_tree_by_pid`, `pid_alive`) y en `avi-tts` (`resident::*`).
//! Este crate no depende de ninguno de los dos: el primero arrastra el árbol de
//! `avi-stt` y el segundo el motor TTS, y `avi-lifecycle` se construye sin cliente
//! HTTP ni árbol de runtime pesado. Por eso el control de procesos entra por
//! [`ProcessControl`], que implementa el binario con esas funciones. El
//! **protocolo** sí vive aquí.
//!
//! **El `pidfile` está en la raíz de datos vigente.** Un daemon vivo cuyo pidfile
//! esté en otra raíz no se encuentra.
//!
//! **No hay cliente HTTP.** Este crate se construye sin él, así que los dos puntos
//! del protocolo —`POST /shutdown` y `GET /health`— viajan por
//! `tokio::net::TcpStream` con HTTP/1.1 a pelo: cuatro líneas de petición y una de
//! lectura de la línea de estado bastan, sin las dependencias de un cliente.

use crate::LifecycleError;
use avi_process::registry::{
    observe_pid_file, parse_ready_record, pid_path, PidFile, PidFileObservation, Registered,
    READY_FILE,
};
use avi_process::IdentityObservation;
pub use avi_process::ProcessIdentity;
use std::path::Path;
use std::time::Duration;

/// Plazo global de la parada. Es el presupuesto de todo el protocolo y **fuente
/// única**: el lanzador lo toma de aquí en lugar de declararlo por su cuenta, de
/// modo que los 8 s del protocolo se afirmen en un solo sitio.
pub const STOP_DEADLINE_GLOBAL: Duration = Duration::from_secs(8);

/// Presupuesto del `POST /shutdown` acotado. El daemon puede tardar en cerrar; el
/// paso 2 lo reclama por PID si no llega.
pub const SHUTDOWN_TIMEOUT: Duration = Duration::from_millis(1500);

/// Presupuesto del sondeo de vida y del POST: ninguno de los dos puede heredar el
/// timeout largo del cliente de la CLI.
pub const PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// Dirección por defecto cuando no hay pidfile. También **fuente única**: es la
/// que publica el binario, que la toma de aquí en lugar de escribir un literal
/// propio.
pub const DEFAULT_ADDR: &str = "127.0.0.1:8765";

/// Variable de entorno que desvía el puerto del daemon por instancia. Vive aquí
/// con la resolución que la lee, que es su única usuaria: el binario no la
/// nombra por su cuenta.
pub const DAEMON_PORT_ENV: &str = "AVI_DAEMON_PORT";

/// Emite el aviso de un pidfile descartado.
fn warn_discarded(data_dir: &Path, reason: &str) {
    eprintln!(
        "Aviso: {} descartado, {}: se trata como ausente.",
        pid_path(data_dir).display(),
        reason
    );
}

/// Lee el pidfile completo. `None` si no hay —ausencia normal—, y también si
/// está ilegible o es un esquema que no se entiende (p. ej. sin identidad, el
/// campo obligatorio): ese descarte se avisa, y el llamante cae al valor por
/// defecto sin que el protocolo falle.
pub fn read_pid_file(data_dir: &Path) -> Option<PidFile> {
    match observe_pid_file(data_dir) {
        PidFileObservation::Present(file) => Some(file),
        PidFileObservation::Absent => None,
        PidFileObservation::Discarded(reason) => {
            warn_discarded(data_dir, &reason);
            None
        }
    }
}

/// Lectura silenciosa: el fichero válido, o `None` en ausencia y en descarte.
fn read_pid_file_silent(data_dir: &Path) -> Option<PidFile> {
    match observe_pid_file(data_dir) {
        PidFileObservation::Present(file) => Some(file),
        PidFileObservation::Absent | PidFileObservation::Discarded(_) => None,
    }
}

/// PID del daemon registrado, o `None` si no hay pidfile, es ilegible o el esquema
/// no trae `pid`.
pub fn read_pid(data_dir: &Path) -> Option<u32> {
    read_pid_file_silent(data_dir)
        .map(|f| f.pid)
        .filter(|pid| *pid != 0)
}

/// PID del residente registrado: `0`, que significa desconocido, si el pidfile
/// falta o no trae el campo. Un pidfile descartado se avisa.
pub fn read_resident_pid(data_dir: &Path) -> u32 {
    read_pid_file(data_dir).map(|f| f.resident_pid).unwrap_or(0)
}

/// Igual que [`read_resident_pid`] sin avisar del descarte, para los pasos del
/// protocolo que ya lo avisaron al empezar.
fn read_resident_pid_silent(data_dir: &Path) -> u32 {
    read_pid_file_silent(data_dir)
        .map(|f| f.resident_pid)
        .unwrap_or(0)
}

/// Dirección publicada en el pidfile. Un pidfile ausente, descartado (con aviso) o
/// sin el campo da `None`, y el llamante cae al valor por defecto.
pub fn read_addr(data_dir: &Path) -> Option<String> {
    read_pid_file(data_dir)
        .and_then(|f| f.addr)
        .map(|addr| addr.trim().to_string())
        .filter(|addr| !addr.is_empty())
}

/// Dirección del cliente: la del pidfile cuando existe, y cuando no la derivada de
/// [`DAEMON_PORT_ENV`] (`DEFAULT_ADDR` si la variable falta o no es un puerto).
///
/// La raíz de datos es la vigente: un daemon con su pidfile en otra raíz no se
/// encuentra y la conexión va a la dirección derivada del entorno.
///
/// Es el **único** resolver con esta semántica: el lanzador no declara otro.
pub fn resolve_client_addr(data_dir: &Path) -> String {
    resolve_client_addr_with(data_dir, std::env::var(DAEMON_PORT_ENV).ok().as_deref())
}

/// Variante de [`resolve_client_addr`] que recibe el valor crudo de
/// [`DAEMON_PORT_ENV`] en lugar de leer el entorno, para poder probarla sin
/// tocar variables del proceso.
pub fn resolve_client_addr_with(data_dir: &Path, port_env: Option<&str>) -> String {
    read_addr(data_dir).unwrap_or_else(|| addr_for_port_env(port_env))
}

/// Dirección a partir del valor crudo de [`DAEMON_PORT_ENV`]: el daemon escucha en
/// `127.0.0.1:<puerto>` con el puerto recortado (`0` pide uno efímero), y un valor
/// ausente o que no es un `u16` deja la dirección por defecto.
pub fn addr_for_port_env(raw: Option<&str>) -> String {
    match raw.and_then(|value| value.trim().parse::<u16>().ok()) {
        Some(port) => format!("127.0.0.1:{port}"),
        None => DEFAULT_ADDR.to_string(),
    }
}

/// Dirección del daemon en la que listen el binario y se para, según
/// [`DAEMON_PORT_ENV`] del entorno. Es el **único** resolver con esta semántica:
/// el lanzador lo usa también para escuchar, en vez de rederivar el puerto.
pub fn default_addr() -> String {
    addr_for_port_env(std::env::var(DAEMON_PORT_ENV).ok().as_deref())
}

/// Borra el pidfile. Solo lo hace quien ha comprobado que el daemon está
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
/// `avi_tts::resident::{resident_pid_alive, kill_tree_resident_by_pid}`.
pub trait ProcessControl {
    /// ¿Vive el proceso de ese PID?
    fn pid_alive(&self, pid: u32) -> bool;
    /// Reclama el árbol exacto de ese PID. Nunca por imagen: el daemon comparte
    /// imagen con el CLI, y matar por imagen mataría al propio invocador.
    fn kill_tree_by_pid(&self, pid: u32) -> bool;
    /// ¿Vive el proceso residente con ese PID?
    fn resident_pid_alive(&self, pid: u32) -> bool;
    /// Reclama el árbol del residente por su PID registrado.
    fn kill_tree_resident_by_pid(&self, pid: u32) -> bool;
    /// Identidad observada del proceso con ese PID, o `None` si no existe o no se
    /// puede observar. Por defecto no se observa nada.
    fn identity(&self, _pid: u32) -> Option<ProcessIdentity> {
        None
    }
    /// Observación de la identidad que separa «no existe» de «no se puede
    /// observar». Por defecto se deriva de [`identity`](Self::identity), que no
    /// distingue y por tanto nunca da `Denied`; el control del producto la
    /// sobrescribe con la observación real del sistema.
    fn observe_identity(&self, pid: u32) -> IdentityObservation {
        match self.identity(pid) {
            Some(identity) => IdentityObservation::Present(identity),
            None => IdentityObservation::Gone,
        }
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
    /// Veredicto completo de la verificación a nivel de sistema, formado por los tres
    /// hechos que el motor observa por separado en el paso final: el daemon no
    /// responde, su PID registrado no está vivo y el residente tampoco. Vale `false`
    /// cuando la parada no concluye, y entonces es lo que distingue «el árbol sigue
    /// vivo» de «solo quedó vivo el residente». Es el dato que el envelope de la CLI
    /// expone como `daemon_fully_stopped`, sin sondeos fuera del motor.
    pub daemon_fully_stopped: bool,
    /// El residente registrado en el pidfile sigue vivo: el predicado
    /// `resident_alive_by_pid` que la verificación a nivel de sistema ya evaluó,
    /// expuesto como campo para que quien lee el desenlace no tenga que deducirlo de
    /// la frase de `remaining`.
    pub resident_alive: bool,
    /// PID del daemon registrado, si lo había.
    pub pid: Option<u32>,
    /// El pidfile se borró, lo que solo ocurre tras muerte verificada.
    pub pidfile_removed: bool,
    /// El fichero ready ya no existe tras una parada verificada. Es `false` si el
    /// borrado falló o si se conservó porque el PID que publica sigue vivo.
    pub ready_removed: bool,
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
    // Un pidfile descartado se avisa una sola vez por parada; el resto de las
    // lecturas del protocolo son silenciosas.
    if let PidFileObservation::Discarded(reason) = observe_pid_file(data_dir) {
        warn_discarded(data_dir, &reason);
    }
    // El daemon se busca en el pidfile y, si falta, en `daemon.ready`: con el
    // pidfile borrado a mano el daemon vivo sigue siendo encontrable.
    let addr = recorded_daemon(data_dir)
        .and_then(|daemon| daemon.addr)
        .unwrap_or_else(|| default_addr.to_string());
    let (previous_pid, previous_state) = daemon_registered(data_dir, control);
    // "Estaba en ejecución" se decide antes de tocar nada: es lo que el resumen de
    // la operación necesita para decir cómo reiniciarlo, y no puede depender de si
    // la parada tuvo éxito.
    let was_running = previous_state == Registered::Ours || probe(&addr).await;
    // Un punto de fallo armado aborta la parada antes de tocar nada: el desenlace
    // dice que no concluyó y la operación que llama falla con `daemon_stop_failed`.
    if let Err(fault) = crate::faults::trip(crate::faults::FaultPoint::OnDaemonStop) {
        return StopOutcome {
            was_running,
            pid: previous_pid,
            remaining: Some(format!("{fault:#}")),
            ..StopOutcome::default()
        };
    }

    // 1) Graceful acotado si responde (no hereda el timeout largo del cliente).
    if probe(&addr).await {
        #[expect(
            clippy::let_underscore_must_use,
            reason = "el apagado ordenado es un intento: su fallo o su vencimiento lo resuelve el árbol por PID y lo juzga la verificación"
        )]
        let _ = tokio::time::timeout(SHUTDOWN_TIMEOUT, post_shutdown(&addr)).await;
        let remaining = STOP_DEADLINE_GLOBAL
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::from_millis(500));
        let wait = remaining.min(Duration::from_secs(3));
        #[expect(
            clippy::let_underscore_must_use,
            reason = "la espera solo da tiempo al apagado; si vence, decide la verificación"
        )]
        let _ = tokio::time::timeout(wait, wait_health_down(&addr, wait)).await;
    }

    // 2) Árbol preciso por PID si sigue vivo, en las dos plataformas, con la
    // guarda `pid != process::id()`. Sin ella, un pidfile obsoleto con el PID del
    // propio invocador lo mataría.
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
    // preciso; sin PID registrado no hay residente que reclamar y nada se mata
    // por imagen. La verificación por `resident_pid` muerto vive en el paso 3.
    let resident = read_resident_pid_silent(data_dir);
    if resident != 0
        && resident != std::process::id()
        && resident_registered(data_dir, control) == Registered::Ours
    {
        control.kill_tree_resident_by_pid(resident);
    }

    // 3) Verificación con lo que queda del plazo global. El bucle se queda con la
    // observación con la que rompe; si el plazo se agota sin romper, se observa una
    // vez más para el veredicto.
    let mut settled = None;
    while start.elapsed() < STOP_DEADLINE_GLOBAL {
        let seen = observe_stop(data_dir, &addr, control).await;
        if seen.fully_stopped() {
            settled = Some(seen);
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    // Los tres hechos de la verificación a nivel de sistema vienen de una sola
    // observación: el veredicto completo, la frase de lo que quedó vivo y el
    // borrado del pidfile se apoyan en ella, en lugar de repetir sondeos que pueden
    // discrepar entre sí y que consumen plazo.
    let seen = match settled {
        Some(seen) => seen,
        None => observe_stop(data_dir, &addr, control).await,
    };

    // 4) Veredicto y borrado del pidfile solo tras muerte verificada.
    // El veredicto completo: el daemon no responde, su PID registrado no está vivo y
    // el residente tampoco. Lo afirma el motor también cuando la parada no concluye, y
    // entonces vale `false`: quien recibe el desenlace necesita saber si lo que quedó
    // vivo es el residente.
    let daemon_fully_stopped = seen.fully_stopped();
    let resident_alive = seen.resident_alive;
    // `stopped` es el mismo veredicto aplicado a las mutaciones del protocolo: sin él
    // el pidfile se conserva como pista y el fichero ready no se borra.
    let stopped = daemon_fully_stopped;
    let remaining = if stopped {
        None
    } else {
        let mut parts = Vec::new();
        if let (Some(pid), true) = (seen.pid, seen.daemon_alive) {
            parts.push(format!("daemon pid {pid}"));
        }
        if seen.responding {
            parts.push(format!("el daemon en {addr} responde"));
        }
        if resident_alive {
            parts.push(format!(
                "residente pid {}",
                read_resident_pid_silent(data_dir)
            ));
        }
        Some(parts.join("; "))
    };
    let pidfile_removed = stopped
        && match remove_pid_file(data_dir) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "Aviso: no se pudo borrar {}: {error}.",
                    pid_path(data_dir).display()
                );
                false
            }
        };
    // El fichero ready se borra con la parada verificada, salvo que el PID que
    // publica el propio fichero siga vivo: entonces es la única pista de ese
    // proceso. Es obsoleto un ready ilegible, sin PID, con el PID del propio proceso,
    // con un PID vivo pero sin las líneas `start=` e `image=` (no declara a su PID
    // como propio: cuenta como muerto) o con un PID que el sistema reasignó a otro
    // programa (la identidad observada no coincide con la registrada).
    let ready_removed = stopped
        && !ready_file_owner_alive(data_dir, control)
        && match remove_ready_file(data_dir) {
            Ok(()) => true,
            Err(error) => {
                eprintln!(
                    "Aviso: no se pudo borrar {}: {error}.",
                    data_dir.join(READY_FILE).display()
                );
                false
            }
        };

    StopOutcome {
        was_running,
        stopped,
        daemon_fully_stopped,
        resident_alive,
        pid: previous_pid,
        pidfile_removed,
        ready_removed,
        remaining,
    }
}

/// Los tres hechos de la verificación a nivel de sistema, observados una vez:
/// el estado del daemon registrado, el del residente y si el daemon responde.
struct StopObservation {
    /// PID del daemon registrado, si lo hay.
    pid: Option<u32>,
    /// El daemon registrado vive y es nuestro.
    daemon_alive: bool,
    /// El residente registrado vive y es nuestro.
    resident_alive: bool,
    /// El daemon responde en su dirección.
    responding: bool,
}

impl StopObservation {
    /// Nada queda vivo: el daemon no responde y ni su PID ni el residente viven.
    fn fully_stopped(&self) -> bool {
        !self.responding && !self.daemon_alive && !self.resident_alive
    }
}

/// Observa una vez los tres hechos de la verificación.
async fn observe_stop(
    data_dir: &Path,
    addr: &str,
    control: &dyn ProcessControl,
) -> StopObservation {
    let (pid, state) = daemon_registered(data_dir, control);
    StopObservation {
        pid,
        daemon_alive: state == Registered::Ours,
        resident_alive: resident_alive_by_pid(data_dir, control),
        responding: probe(addr).await,
    }
}

/// Verificación del residente por su PID registrado, que es el predicado que
/// la tabla de comprobaciones llama "verificación a nivel de sistema".
fn resident_alive_by_pid(data_dir: &Path, control: &dyn ProcessControl) -> bool {
    resident_registered(data_dir, control) == Registered::Ours
}

/// Decide si un PID registrado es nuestro con la identidad que observa
/// `control`. La regla vive en `avi_process::registry::classify_registered`;
/// aquí solo se le da la observación del control de procesos.
pub fn classify_registered(
    control: &dyn ProcessControl,
    pid: u32,
    alive: bool,
    recorded: Option<&ProcessIdentity>,
) -> Registered {
    avi_process::registry::classify_registered(pid, alive, recorded, || {
        control.observe_identity(pid)
    })
}

/// Daemon registrado: sus datos de identidad y su dirección, de donde se hayan
/// podido leer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordedDaemon {
    pub pid: u32,
    /// Identidad con la que se registró; sin ella no se clasifica como nuestro.
    pub identity: Option<ProcessIdentity>,
    /// Dirección real publicada, si la trae.
    pub addr: Option<String>,
}

/// Daemon registrado: el del pidfile y, si falta o no es legible, el que publica
/// `daemon.ready`. El daemon escribe ambos antes de servir, así que borrar el
/// pidfile a mano no lo deja sin pista.
pub fn recorded_daemon(data_dir: &Path) -> Option<RecordedDaemon> {
    if let Some(file) = read_pid_file_silent(data_dir).filter(|f| f.pid != 0) {
        return Some(RecordedDaemon {
            pid: file.pid,
            identity: file.identity,
            addr: file
                .addr
                .map(|addr| addr.trim().to_string())
                .filter(|addr| !addr.is_empty()),
        });
    }
    let text = std::fs::read_to_string(data_dir.join(READY_FILE)).ok()?;
    let record = parse_ready_record(&text)?;
    Some(RecordedDaemon {
        pid: record.pid?,
        identity: record.identity,
        addr: Some(record.addr),
    })
}

/// PID del daemon registrado y su clasificación.
fn daemon_registered(data_dir: &Path, control: &dyn ProcessControl) -> (Option<u32>, Registered) {
    let Some(daemon) = recorded_daemon(data_dir) else {
        return (None, Registered::Absent);
    };
    let state = classify_registered(
        control,
        daemon.pid,
        control.pid_alive(daemon.pid),
        daemon.identity.as_ref(),
    );
    (Some(daemon.pid), state)
}

/// Clasificación del residente del pidfile.
fn resident_registered(data_dir: &Path, control: &dyn ProcessControl) -> Registered {
    let Some(file) = read_pid_file_silent(data_dir) else {
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
/// tiene dueño vivo: es obsoleto. También lo es el que lleva un PID vivo pero sin
/// las líneas `start=` e `image=` (no declara a su PID como propio y cuenta como
/// muerto) y el que lleva un PID que el sistema reasignó a otro programa (la
/// identidad observada no coincide con la registrada).
fn ready_file_owner_alive(data_dir: &Path, control: &dyn ProcessControl) -> bool {
    let Ok(text) = std::fs::read_to_string(data_dir.join(READY_FILE)) else {
        return false;
    };
    let Some(record) = parse_ready_record(&text) else {
        return false;
    };
    let Some(pid) = record.pid else {
        return false;
    };
    pid != std::process::id()
        && classify_registered(
            control,
            pid,
            control.pid_alive(pid),
            record.identity.as_ref(),
        ) == Registered::Ours
}

/// Borra el fichero ready, tolerando que ya no exista.
fn remove_ready_file(data_dir: &Path) -> std::io::Result<()> {
    match std::fs::remove_file(data_dir.join(READY_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
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
    use crate::test_support::scratch;
    use avi_core::exit_codes::ExitCode;
    use avi_process::registry::write_pid_file;
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
    }

    impl Inert {
        fn new() -> Self {
            Self {
                killed: AtomicU32::new(0),
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
    }

    /// Control que dice que un PID dado vive siempre: sirve para el caso del
    /// pidfile obsoleto, donde la parada no puede concluir.
    struct Alive {
        pid: u32,
        identity: Option<ProcessIdentity>,
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
        fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
            if pid == self.pid {
                return self.identity.clone();
            }
            None
        }
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
        assert!(
            outcome.daemon_fully_stopped,
            "sin nada vivo la parada está completa según el motor"
        );
        assert!(
            !outcome.resident_alive,
            "sin pidfile no hay residente registrado que pueda quedar vivo"
        );
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
        set_pid_file(
            &data,
            999_999,
            "127.0.0.1:1",
            0,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(!outcome.was_running, "el PID registrado no está vivo");
        assert_eq!(
            outcome.pid,
            Some(999_999),
            "pero se conserva para el resumen"
        );
        assert!(outcome.stopped);
        assert!(
            outcome.daemon_fully_stopped,
            "con el PID muerto la parada está completa según el motor"
        );
        assert!(
            outcome.pidfile_removed,
            "el pidfile se borra tras verificar"
        );
        assert!(!pid_path(&data).exists());
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

    /// Sin pidfile, la parada busca al daemon en la dirección derivada de
    /// `AVI_DAEMON_PORT` y no en la de por defecto: un daemon que escucha en un
    /// puerto desviado responde, recibe el `POST /shutdown` y la parada lo da por
    /// detenido cuando deja de responder.
    #[tokio::test]
    async fn stop_without_pidfile_targets_daemon_port() {
        use crate::test_support::{serve_responses, FakeResponse};
        let data = scratch("stop-port");
        // Dos sondeos de salud y el apagado; después el servidor cierra y el
        // daemon deja de responder.
        let (base, seen, server) = serve_responses(vec![
            FakeResponse::new(200, "ok"),
            FakeResponse::new(200, "ok"),
            FakeResponse::new(200, ""),
        ])
        .await;
        let port = base.rsplit(':').next().expect("la URL lleva puerto");
        let addr = addr_for_port_env(Some(port));
        assert_ne!(addr, DEFAULT_ADDR, "el puerto está desviado");

        let control = Inert::new();
        let outcome = stop(&data, &addr, &control).await;
        server.await.expect("el servidor termina");

        assert!(
            outcome.was_running,
            "el daemon del puerto desviado respondía"
        );
        assert!(outcome.stopped, "dejó de responder tras el apagado");
        let seen = seen.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert_eq!(seen, ["/health", "/health", "/shutdown"]);
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

        set_pid_file(
            &data,
            4242,
            "127.0.0.1:7001",
            77,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        assert_eq!(
            resolve_client_addr_with(&data, Some("9123")),
            "127.0.0.1:7001",
            "el pidfile prevalece sobre la variable"
        );
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

        // 2. Un esquema viejo sin identidad se descarta como ilegible: se
        //    propaga como ausencia, nunca como un PID sin contrastar.
        crate::test_support::write_file(&path, r#"{"pid": 4242}"#);
        assert_eq!(read_pid(&data), None, "sin identidad no hay PID legítimo");
        assert_eq!(read_resident_pid(&data), 0, "ni PID de residente");
        assert_eq!(read_addr(&data), None);
        assert_eq!(resolve_client_addr_with(&data, None), DEFAULT_ADDR);
        assert!(read_pid_file(&data).is_none());

        // 3. Un `pid` inválido no se interpreta.
        for garbage in ["{}", r#"{"pid": "x"}"#, "[]", "no soy json", ""] {
            crate::test_support::write_file(&path, garbage);
            assert_eq!(read_pid(&data), None, "basura: {garbage:?}");
        }

        // 4. La escritura es atómica: temporal hermano y renombrado, sin residuo.
        set_pid_file(
            &data,
            4242,
            "127.0.0.1:8765",
            77,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let written = std::fs::read_to_string(&path).unwrap();
        assert!(
            serde_json::from_str::<PidFile>(&written).is_ok(),
            "lo que hay en disco es JSON completo, nunca medio: {written}"
        );
        assert_eq!(read_pid(&data), Some(4242));
        assert_eq!(read_resident_pid(&data), 77);
        assert_eq!(read_addr(&data).as_deref(), Some("127.0.0.1:8765"));
        assert_eq!(
            std::fs::read_dir(&data).unwrap().count(),
            1,
            "solo queda el pidfile"
        );

        // 5. Intercalar muchas escrituras y lecturas: cada lectura devuelve un
        //    estado completo y coherente, nunca una mezcla de dos escrituras.
        for round in 0..64u32 {
            let pid = 1000 + round;
            set_pid_file(
                &data,
                pid,
                "127.0.0.1:9",
                round,
                identity_of("111", "daemon.exe"),
            )
            .unwrap();
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
    }

    /// La guarda anti-auto-muerte: un pidfile obsoleto que lleva el PID del propio
    /// proceso **no** lo mata, porque la comprobación no es "si el PID existe,
    /// mátalo".
    #[tokio::test]
    async fn stop_never_kills_its_own_process() {
        let data = scratch("self-guard");
        let control = Inert::new();
        let own = std::process::id();
        set_pid_file(
            &data,
            own,
            &dead_port(),
            own,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();

        let outcome = stop(&data, &dead_port(), &control).await;
        assert_eq!(
            outcome.pid,
            Some(own),
            "el PID obsoleto se conserva en el desenlace"
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
    }

    /// Un daemon que no se puede parar produce `daemon_stop_failed` con el código
    /// de la tabla cerrada, y no se toca nada.
    #[tokio::test]
    async fn unstoppable_daemon_yields_daemon_stop_failed() {
        let data = scratch("failure");
        let pid = 424_242;
        let control = Alive {
            pid,
            identity: Some(identity_of("111", "daemon.exe")),
        };
        set_pid_file(
            &data,
            pid,
            &dead_port(),
            0,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.was_running, "el daemon seguía vivo");
        assert!(!outcome.stopped, "y no se pudo parar");
        assert!(
            !outcome.daemon_fully_stopped,
            "con el daemon vivo la parada no está completa según el motor"
        );
        assert!(
            !outcome.resident_alive,
            "lo que sigue vivo es el árbol del daemon, no el residente"
        );
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
    }

    /// Sin `resident_pid` registrado no se mata ningún proceso por imagen: un
    /// residente vivo no registrado sobrevive a la parada y el motor no lo
    /// reclama, porque solo se reclama lo que el pidfile identifica.
    #[tokio::test]
    async fn stop_without_registered_resident_kills_nothing() {
        let data = scratch("resident");
        let unregistered = 888_888;
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            0,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let control = Observed::new(
            &[unregistered],
            &[(unregistered, identity_of("222", "qwen_tts.exe"))],
        );

        let outcome = stop(&data, &dead_port(), &control).await;
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "sin PID de residente registrado no se mata nada"
        );
        assert!(
            control.is_alive(unregistered),
            "el residente no registrado sigue vivo: no se barre por imagen"
        );
        assert!(
            !outcome.resident_alive,
            "el veredicto solo ve al residente que el pidfile registra"
        );
    }

    /// Control con el daemon muerto y un residente que sigue vivo pase lo que
    /// pase: reclamar su árbol no lo mata.
    struct ResidentSurvives {
        resident: u32,
        identity: ProcessIdentity,
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
        fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
            if pid == self.resident {
                return Some(self.identity.clone());
            }
            None
        }
    }

    /// Contenido del fichero ready de éxito tal como lo publica el daemon.
    fn ready_content(pid: u32) -> String {
        format!("addr={}\nwarm=ok\npid={pid}\n", dead_port())
    }

    /// Tras una parada verificada, el fichero ready desaparece junto al pidfile:
    /// un ready obsoleto haría creer al siguiente arranque que el daemon sigue listo.
    #[tokio::test]
    async fn stop_removes_ready_file_after_verified_stop() {
        let data = scratch("ready-deleted");
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            0,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        crate::test_support::write_file(&data.join(READY_FILE), &ready_content(999_999));

        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped, "el daemon está muerto");
        assert!(!pid_path(&data).exists(), "el pidfile se borra");
        assert!(
            !data.join(READY_FILE).exists(),
            "el fichero ready se borra tras la parada verificada"
        );
    }

    /// Si el PID que contiene el propio fichero ready sigue vivo, el fichero no se
    /// borra aunque el pidfile no lo mencione: es la única pista de ese proceso.
    #[tokio::test]
    async fn stop_keeps_ready_file_while_its_pid_is_alive() {
        let data = scratch("ready-alive");
        let ready_pid = 777_001;
        // El pidfile nombra a un daemon muerto: lo vivo que publica el ready no es
        // el daemon que se para, y su fichero es la única pista de ese proceso.
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            0,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        crate::test_support::write_file(
            &data.join(READY_FILE),
            &format!(
                "addr=127.0.0.1:8765\nwarm=ok\npid={ready_pid}\nstart=111\nimage=daemon.exe\n"
            ),
        );
        let control = Observed::new(
            &[ready_pid],
            &[(ready_pid, identity_of("111", "daemon.exe"))],
        );

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(
            outcome.stopped,
            "el daemon del pidfile está muerto y no responde: no hay nada que parar"
        );
        assert!(
            data.join(READY_FILE).exists(),
            "el PID del ready sigue vivo: el fichero se conserva"
        );
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "solo se reclama al daemon registrado en el pidfile"
        );
    }

    /// Con el pidfile borrado y el ready presente, `stop` encuentra al daemon por
    /// el ready: lo detiene, lo cuenta como en ejecución y borra el ready.
    #[tokio::test]
    async fn stop_finds_daemon_by_ready_file_when_pidfile_is_missing() {
        let data = scratch("ready-fallback");
        let daemon = 777_002;
        crate::test_support::write_file(
            &data.join(READY_FILE),
            &format!(
                "addr={}\nwarm=ok\npid={daemon}\nstart=111\nimage=daemon.exe\n",
                dead_port()
            ),
        );
        let control = Observed::new(&[daemon], &[(daemon, identity_of("111", "daemon.exe"))]);
        assert!(!pid_path(&data).exists(), "el pidfile no existe");

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(
            outcome.was_running,
            "el daemon vivo se encuentra aunque falte el pidfile"
        );
        assert_eq!(outcome.pid, Some(daemon));
        assert!(outcome.stopped, "el daemon queda detenido");
        assert!(!control.is_alive(daemon), "se reclamó su árbol");
        assert_eq!(control.killed.load(Ordering::Relaxed), 1);
        assert!(
            !data.join(READY_FILE).exists(),
            "con el daemon muerto el ready deja de ser pista"
        );
    }

    /// Un ready con un PID reasignado a otro programa no hace matar nada.
    #[tokio::test]
    async fn stop_spares_ready_file_pid_with_foreign_identity() {
        let data = scratch("ready-foreign");
        let pid = 777_003;
        crate::test_support::write_file(
            &data.join(READY_FILE),
            &format!(
                "addr={}\nwarm=ok\npid={pid}\nstart=111\nimage=daemon.exe\n",
                dead_port()
            ),
        );
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "otro.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.stopped, "un PID ajeno cuenta como muerto");
        assert_eq!(control.killed.load(Ordering::Relaxed), 0);
        assert!(control.is_alive(pid), "el proceso ajeno sigue vivo");
    }

    /// Control cuyo residente tarda en morir tras reclamar su árbol: sigue vivo
    /// durante algunos sondeos y luego desaparece.
    struct SlowDyingResident {
        resident: u32,
        identity: ProcessIdentity,
        killed: AtomicU32,
        polls_after_kill: AtomicU32,
    }

    impl ProcessControl for SlowDyingResident {
        fn pid_alive(&self, _pid: u32) -> bool {
            false
        }
        fn kill_tree_by_pid(&self, _pid: u32) -> bool {
            false
        }
        fn resident_pid_alive(&self, pid: u32) -> bool {
            if pid != self.resident {
                return false;
            }
            if self.killed.load(Ordering::Relaxed) == 0 {
                return true;
            }
            self.polls_after_kill.fetch_add(1, Ordering::Relaxed) < 3
        }
        fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
            self.killed.fetch_add(1, Ordering::Relaxed);
            true
        }
        fn identity(&self, pid: u32) -> Option<ProcessIdentity> {
            (pid == self.resident).then(|| self.identity.clone())
        }
    }

    /// El veredicto con un residente registrado vivo espera a su muerte: la
    /// parada no concluye hasta que el residente deja de vivir.
    #[tokio::test]
    async fn stop_waits_for_registered_resident_death() {
        let data = scratch("resident-wait");
        let resident = 888_889;
        let resident_id = identity_of("222", "qwen_tts.exe");
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            resident,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let mut file = read_pid_file(&data).unwrap();
        file.resident_identity = Some(resident_id.clone());
        file.resident_port = Some(40_123);
        write_pid_file(&data, &file).unwrap();
        let control = SlowDyingResident {
            resident,
            identity: resident_id,
            killed: AtomicU32::new(0),
            polls_after_kill: AtomicU32::new(0),
        };

        let outcome = stop(&data, &dead_port(), &control).await;
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            1,
            "se reclama el árbol del residente registrado"
        );
        assert!(
            control.polls_after_kill.load(Ordering::Relaxed) >= 3,
            "el veredicto sondeó hasta ver morir al residente"
        );
        assert!(
            outcome.stopped,
            "con el residente muerto la parada concluye"
        );
        assert!(!outcome.resident_alive);
        assert!(outcome.pidfile_removed);
    }

    /// Un fichero ready ilegible o con el PID del propio proceso es obsoleto y se
    /// borra, incluso si el control de procesos diera ese PID por vivo.
    #[tokio::test]
    async fn stop_removes_stale_ready_file() {
        let data = scratch("ready-stale");
        let ready = data.join(READY_FILE);
        let own = std::process::id();

        crate::test_support::write_file(&ready, "basura sin formato");
        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped);
        assert!(!ready.exists(), "un ready ilegible es obsoleto y se borra");

        crate::test_support::write_file(&ready, &ready_content(own));
        let outcome = stop(
            &data,
            &dead_port(),
            &Alive {
                pid: own,
                identity: None,
            },
        )
        .await;
        assert!(outcome.stopped);
        assert!(
            !ready.exists(),
            "un ready con el PID del propio proceso es obsoleto y se borra"
        );
    }

    /// Un residente registrado que sigue vivo tras los intentos de parada impide
    /// dar la parada por completa, aunque el daemon haya muerto: se conserva el
    /// pidfile como pista y se nombra al residente.
    #[tokio::test]
    async fn surviving_resident_prevents_complete_stop() {
        let data = scratch("resident-alive");
        let resident = 888_888;
        let resident_id = identity_of("222", "qwen_tts.exe");
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            resident,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let mut file = read_pid_file(&data).unwrap();
        file.resident_identity = Some(resident_id.clone());
        write_pid_file(&data, &file).unwrap();

        let outcome = stop(
            &data,
            &dead_port(),
            &ResidentSurvives {
                resident,
                identity: resident_id,
            },
        )
        .await;
        assert!(
            !outcome.stopped,
            "el residente sigue vivo: la parada no está completa"
        );
        assert!(
            !outcome.daemon_fully_stopped,
            "con el residente vivo la parada no está completa según el motor"
        );
        assert!(
            outcome.resident_alive,
            "el predicado del motor lo dice sin que haya que leer la frase de `remaining`"
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
    }

    fn identity_of(start: &str, image: &str) -> ProcessIdentity {
        ProcessIdentity {
            start: start.to_string(),
            image: image.to_string(),
        }
    }

    /// Escribe un pidfile completo del daemon, sin residente registrado salvo su
    /// PID.
    fn set_pid_file(
        data_dir: &Path,
        pid: u32,
        addr: &str,
        resident_pid: u32,
        identity: ProcessIdentity,
    ) -> anyhow::Result<()> {
        write_pid_file(
            data_dir,
            &PidFile {
                pid,
                addr: Some(addr.to_string()),
                started_at: None,
                resident_pid,
                identity: Some(identity),
                resident_identity: None,
                resident_port: None,
            },
        )
    }

    /// Control donde nadie vive y que cuenta cada consulta de viveza del daemon y
    /// del residente.
    struct CountingDead {
        daemon_checks: AtomicU32,
        resident_checks: AtomicU32,
    }

    impl ProcessControl for CountingDead {
        fn pid_alive(&self, _pid: u32) -> bool {
            self.daemon_checks.fetch_add(1, Ordering::Relaxed);
            false
        }
        fn kill_tree_by_pid(&self, _pid: u32) -> bool {
            true
        }
        fn resident_pid_alive(&self, _pid: u32) -> bool {
            self.resident_checks.fetch_add(1, Ordering::Relaxed);
            false
        }
        fn kill_tree_resident_by_pid(&self, _pid: u32) -> bool {
            true
        }
    }

    /// El veredicto reutiliza la observación con la que rompe la verificación: con
    /// todo muerto desde la primera vuelta, el daemon registrado se consulta tres
    /// veces (antes de parar, antes de reclamar su árbol y en la verificación) y
    /// el residente dos (antes de reclamarlo y en la verificación), sin una
    /// cuarta y una tercera consulta para el veredicto.
    #[tokio::test]
    async fn stop_verdict_reuses_breaking_observation() {
        let data = scratch("single-observation");
        set_pid_file(
            &data,
            999_998,
            &dead_port(),
            999_997,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let control = CountingDead {
            daemon_checks: AtomicU32::new(0),
            resident_checks: AtomicU32::new(0),
        };
        let outcome = stop(&data, &dead_port(), &control).await;

        assert!(outcome.stopped && outcome.daemon_fully_stopped);
        assert_eq!(control.daemon_checks.load(Ordering::Relaxed), 3);
        assert_eq!(control.resident_checks.load(Ordering::Relaxed), 2);
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

    /// Un pidfile con las claves de identidad presentes pero en `null` se
    /// lee, pero no habilita clasificación `Ours`: el ausente no es nuestro.
    #[test]
    fn pid_file_with_null_identities_does_not_classify_as_ours() {
        let data = scratch("null-identities");
        crate::test_support::write_file(
            &pid_path(&data),
            r#"{"pid": 4242, "addr": "127.0.0.1:7001", "resident_pid": 77, "identity": null, "resident_identity": null, "resident_port": null}"#,
        );
        let file = read_pid_file(&data).expect("las claves presentes se leen");
        assert_eq!(file.identity, None);
        let control = Observed::new(&[4242], &[]);
        assert_eq!(
            classify_registered(&control, file.pid, true, file.identity.as_ref()),
            Registered::Absent
        );
    }

    /// Si el fichero ready no se puede borrar tras una parada verificada, el
    /// desenlace lo refleja en `ready_removed` en lugar de darlo por borrado.
    #[tokio::test]
    async fn stop_reports_ready_file_removal_failure() {
        let data = scratch("ready-undeletable");
        // Un directorio con el nombre del fichero ready: existe y no se puede
        // borrar con `remove_file`.
        std::fs::create_dir_all(data.join(READY_FILE)).expect("se crea el obstáculo");

        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped, "la parada concluye sin daemon registrado");
        assert!(
            !outcome.ready_removed,
            "el borrado fallido del ready no se da por hecho"
        );
    }

    /// Con la parada verificada y sin fichero ready, el desenlace lo da por
    /// retirado.
    #[tokio::test]
    async fn stop_reports_ready_file_removed() {
        let data = scratch("ready-absent");

        let outcome = stop(&data, &dead_port(), &Inert::new()).await;
        assert!(outcome.stopped);
        assert!(outcome.ready_removed, "no queda fichero ready");
    }

    /// Un punto de fallo armado en la parada la aborta antes de tocar nada y el
    /// desenlace dice que no concluyó y por qué.
    #[cfg(feature = "faults")]
    #[tokio::test]
    async fn stop_injected_fault_aborts_without_stopping() {
        let data = scratch("fault-stop");
        let _guard = crate::faults::armed(crate::faults::FaultPoint::OnDaemonStop);

        let control = Inert::new();
        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(!outcome.stopped, "el fallo inyectado impide concluir");
        assert!(
            outcome
                .remaining
                .as_deref()
                .is_some_and(|text| text.contains("on_daemon_stop")),
            "la causa queda en el desenlace: {:?}",
            outcome.remaining
        );
        assert_eq!(control.killed.load(Ordering::Relaxed), 0);
    }

    /// Un PID de daemon reasignado a otro programa no bloquea la parada y no se mata.
    #[tokio::test]
    async fn stop_spares_daemon_pid_with_foreign_identity() {
        let data = scratch("foreign-daemon");
        let pid = 424_243;
        set_pid_file(
            &data,
            pid,
            &dead_port(),
            0,
            identity_of("111", "ai-voice-interconnector.exe"),
        )
        .unwrap();
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "other.exe"))]);

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
    }

    /// Un PID de residente reasignado a otro programa no bloquea la parada y no se mata.
    #[tokio::test]
    async fn stop_spares_resident_pid_with_foreign_identity() {
        let data = scratch("foreign-resident");
        let resident = 424_244;
        set_pid_file(
            &data,
            999_999,
            &dead_port(),
            resident,
            identity_of("111", "daemon.exe"),
        )
        .unwrap();
        let mut file = read_pid_file(&data).unwrap();
        file.resident_identity = Some(identity_of("222", "qwen_tts.exe"));
        write_pid_file(&data, &file).unwrap();
        let control = Observed::new(&[resident], &[(resident, identity_of("999", "other.exe"))]);

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
    }

    /// Con la identidad registrada coincidente, el daemon es nuestro y se mata.
    #[tokio::test]
    async fn stop_kills_daemon_pid_with_matching_identity() {
        let data = scratch("own-daemon");
        let pid = 424_245;
        let id = identity_of("111", "ai-voice-interconnector.exe");
        set_pid_file(&data, pid, &dead_port(), 0, id.clone()).unwrap();
        let control = Observed::new(&[pid], &[(pid, id)]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.was_running);
        assert!(outcome.stopped);
        assert_eq!(control.killed.load(Ordering::Relaxed), 1);
        assert!(!control.is_alive(pid));
    }

    /// Un pidfile sin identidad, aunque su PID siga vivo, no es nuestro: la
    /// lectura lo descarta como ausente (con aviso) y la parada no lo mata.
    #[tokio::test]
    async fn pidfile_without_identity_is_not_classified_as_ours_nor_killed() {
        let data = scratch("no-identity");
        let pid = 424_248;
        crate::test_support::write_file(
            &pid_path(&data),
            &format!(r#"{{"pid": {pid}, "addr": "127.0.0.1:7001", "resident_pid": 0}}"#),
        );
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "other.exe"))]);

        assert!(
            read_pid_file(&data).is_none(),
            "un fichero sin identidad es ilegible, se propaga como ausencia"
        );
        assert_eq!(
            classify_registered(&control, pid, true, None),
            Registered::Absent,
            "sin identidad registrada no hay clasificación Ours"
        );
        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(
            !outcome.was_running,
            "sin identidad no se declara el daemon en ejecución"
        );
        assert_eq!(
            control.killed.load(Ordering::Relaxed),
            0,
            "un PID vivo sin identidad registrada no se mata"
        );
        assert!(control.is_alive(pid));
    }

    /// Un ready cuyo PID pertenece ya a otro programa es obsoleto y se borra.
    #[tokio::test]
    async fn stop_removes_ready_file_whose_pid_is_foreign() {
        let data = scratch("foreign-ready");
        let pid = 424_247;
        crate::test_support::write_file(
            &data.join(READY_FILE),
            &format!(
                "{}start=111\nimage=ai-voice-interconnector.exe\n",
                ready_content(pid)
            ),
        );
        let control = Observed::new(&[pid], &[(pid, identity_of("999", "other.exe"))]);

        let outcome = stop(&data, &dead_port(), &control).await;
        assert!(outcome.stopped);
        assert!(
            !data.join(READY_FILE).exists(),
            "el PID del ready es de un ajeno: el fichero es obsoleto"
        );
        assert_eq!(control.killed.load(Ordering::Relaxed), 0);
    }

    /// Los plazos del protocolo y la dirección por defecto tienen los valores
    /// fijados, y el presupuesto está anidado.
    #[test]
    fn protocol_deadlines_are_preserved() {
        assert_eq!(STOP_DEADLINE_GLOBAL, Duration::from_secs(8));
        assert_eq!(SHUTDOWN_TIMEOUT, Duration::from_millis(1500));
        assert_eq!(PROBE_TIMEOUT, Duration::from_millis(500));
        assert!(
            PROBE_TIMEOUT < SHUTDOWN_TIMEOUT && SHUTDOWN_TIMEOUT < STOP_DEADLINE_GLOBAL,
            "el presupuesto está anidado: un sondeo no puede agotar el del POST, ni el POST el global"
        );
        assert_eq!(DEFAULT_ADDR, "127.0.0.1:8765");
    }

    /// Los dos resolvers de dirección convergen sin pidfile: el del cliente
    /// (pidfile primero) y el del entorno (parada y escucha) dan la dirección
    /// por defecto del protocolo.
    #[test]
    fn address_resolvers_converge_without_pidfile() {
        let data = scratch("resolvers");
        assert_eq!(
            resolve_client_addr_with(&data, None),
            addr_for_port_env(None)
        );
        assert_eq!(addr_for_port_env(None), DEFAULT_ADDR);
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
