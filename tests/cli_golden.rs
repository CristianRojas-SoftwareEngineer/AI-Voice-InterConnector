//! Harness de tests dorados del CLI (Tarea 8 del desbloqueo de Fase 0).
//!
//! Invoca el binario compilado con argumentos fijos y compara `stdout` (JSON) y el
//! código de salida contra fixtures en `tests/golden/`, replicando el contrato que
//! cubrían los scripts Python eliminados: `schema_version == "3"` (vía
//! `avi_core::json_emitter`) y los códigos de salida de `avi_core::exit_codes`.
//!
//! Se ubica como test de integración del paquete raíz (y no dentro de `src/main.rs`)
//! porque capturar `stdout` + exit code con fidelidad exige ejecutar el binario real,
//! y `CARGO_BIN_EXE_*` solo está disponible para tests de integración.

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

/// Serializa los tests que mutan estado compartido del almacén (cleanup borra
/// snapshots HF + data_dir; los tests TTS dependen de esa provisión). Sin este
/// lock, `cargo test` los corre en paralelo dentro del mismo binario y cleanup
/// puede borrar el estado que un test TTS está verificando (carrera intra-binario).
///
/// El tipo y el contrato no cambian (`Mutex<()>`, un solo guard por test): solo
/// se tolera el envenenado en el camino de fallo (D-03). Un `panic!` previo con
/// el lock tomado envenena el `Mutex`; el siguiente test lo recupera con
/// `bloquear_estado()` en vez de reventar en `unwrap()`.
static STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Toma `STATE_LOCK` tolerando el envenenado (D-03, Tarea 1): si un test previo
/// hizo `panic!` con el lock tomado, el `Mutex` queda envenenado y `lock()`
/// retorna `Err`; se recupera el guard con `into_inner()` para que el siguiente
/// test lo adquiera sin cambiar el tipo ni el contrato del lock.
fn bloquear_estado() -> std::sync::MutexGuard<'static, ()> {
    STATE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

// ─── Observabilidad F4b (solo instrumentación, sin cambios de comportamiento) ───
//
// Hitos por `eprintln!` (stderr, sin buffer) con formato único
// `[hito][mm:ss.mmm-desde-inicio-test] mensaje`. Sin `println!` para progreso.
// Guard de tiempo por test pesado: `hito_inicio_*` fija el techo y
// `comprobar_guard` falla con `panic!` (último hito + fase exacta) en los polls
// ya existentes (`esperar_estado_daemon`).
// Techos: 180 s (resto) y 360 s (dub). Salen de techos del producto en
// `src/main.rs` (cliente HTTP 120 s `:2516`, POST /dub 10 s `:3049`, arranque
// 10 s `:35`, parada 5 s en `wait_health_down` `:2494-2505` + 1.5 s shutdown
// `:1416`) más warmup TTS en segundo plano y presupuesto `:56-59`: 1 operación
// (120 s) + arranque/parada (~15-25 s) + margen → 180 s; dub encadena
// STT+traducción+TTS (hasta 2×120 s) + arranque/parada → 360 s. Sin baseline F5
// aún (F5 es posterior según F3); se re-medira en F5 y se ajustara si hace falta.

/// Techo del guard para tests pesados no-dub (3 min).
const GUARD_PESADO_SECS: u64 = 180;
/// Techo del guard para tests dub (6 min, encadenan STT+traducción+TTS).
/// Solo lo usan tests con `native-stt`; en compilación sin ese feature queda
/// sin usar (permitido para mantener `cargo check --tests` limpio en ambas).
#[allow(dead_code)]
const GUARD_DUB_SECS: u64 = 360;

static PROCESO_T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Instante de arranque del proceso (respaldo del timestamp cuando el test no
/// fijó `hito_inicio`; los pesados siempre lo fijan).
fn proceso_t0() -> Instant {
    *PROCESO_T0.get_or_init(Instant::now)
}

thread_local! {
    static TEST_T0: RefCell<Option<Instant>> = RefCell::new(None);
    static TEST_NOMBRE: RefCell<String> = RefCell::new(String::new());
    static TEST_LIMITE: RefCell<Option<Duration>> = RefCell::new(None);
    static ULTIMO_HITO: RefCell<String> = RefCell::new(String::new());
}

/// Transcurrido desde el inicio del test (o del proceso si no hay inicio).
fn elapsed_test() -> Duration {
    let t0 = TEST_T0.with(|c| *c.borrow());
    match t0 {
        Some(t) => t.elapsed(),
        None => proceso_t0().elapsed(),
    }
}

/// Formato mm:ss.mmm del transcurrido.
fn formato_mm_ss(d: Duration) -> String {
    let ms = d.as_millis();
    format!("{:02}:{:02}.{:03}", ms / 60000, (ms / 1000) % 60, ms % 1000)
}

/// Hito único de progreso (stderr, sin buffer). Registra el último hito para
/// el diagnóstico del guard.
fn hito(mensaje: &str) {
    let ts = formato_mm_ss(elapsed_test());
    ULTIMO_HITO.with(|c| *c.borrow_mut() = mensaje.to_string());
    eprintln!("[hito][{}] {}", ts, mensaje);
}

/// Inicio de test pesado con techo explícito. Debe ser la primera línea del
/// test (antes de locks) para que el guard incluya la contención.
fn hito_inicio(nombre: &str, limite: Duration) {
    TEST_T0.with(|c| *c.borrow_mut() = Some(Instant::now()));
    TEST_NOMBRE.with(|c| *c.borrow_mut() = nombre.to_string());
    TEST_LIMITE.with(|c| *c.borrow_mut() = Some(limite));
    ULTIMO_HITO.with(|c| *c.borrow_mut() = format!("inicio {}", nombre));
    let ts = formato_mm_ss(Duration::from_millis(0));
    eprintln!(
        "[hito][{}] inicio {} (techo {:?})",
        ts, nombre, limite
    );
}

/// Inicio con techo estándar (3 min).
fn hito_inicio_pesado(nombre: &str) {
    hito_inicio(nombre, Duration::from_secs(GUARD_PESADO_SECS));
}

/// Inicio para dub (6 min). Solo lo usan tests con `native-stt`.
#[allow(dead_code)]
fn hito_inicio_dub(nombre: &str) {
    hito_inicio(nombre, Duration::from_secs(GUARD_DUB_SECS));
}

/// Fin de test pesado. Desactiva el guard para no filtrar al siguiente test
/// del mismo hilo del harness. Limpia además el inicio y el nombre (higiene
/// D-03: sin techos ni hitos heredados entre tests del mismo hilo, aun ante
/// `panic!` previo sin `hito_fin` — `hito_inicio` siempre sobrescribe).
fn hito_fin(nombre: &str) {
    let ts = formato_mm_ss(elapsed_test());
    eprintln!("[hito][{}] fin {}", ts, nombre);
    TEST_LIMITE.with(|c| *c.borrow_mut() = None);
    TEST_T0.with(|c| *c.borrow_mut() = None);
    TEST_NOMBRE.with(|c| *c.borrow_mut() = String::new());
    ULTIMO_HITO.with(|c| *c.borrow_mut() = String::new());
}

/// Último hito registrado (para el diagnóstico del guard).
fn ultimo_hito() -> String {
    ULTIMO_HITO.with(|c| c.borrow().clone())
}

/// Guard genérico: falla en vez de colgarse. Se llama en los polls ya
/// existentes (`esperar_estado_daemon`); al expirar mata el árbol best-effort
/// (`reaper_ante_fallo`, H-01) y hace `panic!` con test, fase, transcurrido,
/// techo y último hito. Inactivo sin `hito_inicio`.
///
/// Higiene D-03: `hito_inicio` siempre sobrescribe `TEST_T0`/`TEST_LIMITE`, así
/// que un `panic!` previo sin `hito_fin` no hereda techos al siguiente test
/// pesado; `hito_fin` y `GuardReaper` (en `Drop` ante `panic!`) limpian el
/// límite para que los tests ligeros sin `hito_inicio` tampoco lo hereden.
fn comprobar_guard(fase: &str) {
    let nombre = TEST_NOMBRE.with(|n| n.borrow().clone());
    let limite = TEST_LIMITE.with(|c| *c.borrow());
    let t0 = TEST_T0.with(|c| *c.borrow());
    if let (Some(lim), Some(t)) = (limite, t0) {
        let elapsed = t.elapsed();
        if elapsed > lim {
            TEST_LIMITE.with(|c| *c.borrow_mut() = None);
            reaper_ante_fallo(&format!("guard:{}", fase));
            panic!(
                "guardia de tiempo: test '{}' superó techo {:?} en fase '{}' (transcurrido {:.1} s; último hito: {})",
                nombre,
                lim,
                fase,
                elapsed.as_secs_f64(),
                ultimo_hito()
            );
        }
    }
}

// ─── Verificación a nivel de sistema y reaper ruidoso (H-01, T6) ──────
//
// El producto reclama el residual al arrancar (matar-y-rearrancar con payload
// `started`) y para con deadline global y verificación (`src/main.rs`:
// `clasificar_residual`, `reclamar_residual_degradado`,
// `stop_daemon_and_resident`; ayudantes SO `avi_daemon::{pid_vivo,
// matar_arbol_por_pid, esperar_muerte_pid}`). La fixture verifica esa conducta
// a nivel de sistema en vez de suponerla por HTTP. Fuente única de
// matar/verificar: `avi_daemon`, sin duplicar lógica SO en el harness.

/// Lee el PID de `daemon.pid` (espejo de solo-lectura de `read_daemon_pid` del
/// producto): `None` si no hay pista o es ilegible.
fn leer_pid_daemon() -> Option<u32> {
    let path = avi_store::data_dir().join("daemon.pid");
    let content = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v.get("pid")?.as_u64().map(|n| n as u32)
}

/// ¿Hay algo escuchando en `127.0.0.1:port`? Sondeo TCP breve, sin HTTP.
/// Puertos propios: 8765 (daemon) y 8766 (residente `qwen_tts`).
fn puerto_abierto(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// Reaper best-effort ante fallo (H-01, T6): mata el árbol preciso por PID con
/// verificación acotada (8 s, deadline global del producto) y lo registra como
/// hito. Nunca falla: un reaper que fallara enmascararía la causa original del
/// `panic!` que lo invocó.
fn reaper_ante_fallo(fase: &str) {
    match leer_pid_daemon() {
        Some(pid) if avi_daemon::pid_vivo(pid) => {
            hito(&format!(
                "reaper({}): árbol residual pid {} vivo, matando",
                fase, pid
            ));
            avi_daemon::matar_arbol_por_pid(pid);
            let muerto =
                avi_daemon::esperar_muerte_pid(pid, std::time::Duration::from_secs(8));
            hito(&format!("reaper({}): pid {} muerto={}", fase, pid, muerto));
        }
        Some(pid) => {
            hito(&format!(
                "reaper({}): pid {} ya muerto, sin árbol que matar",
                fase, pid
            ));
        }
        None => {
            hito(&format!(
                "reaper({}): sin pidfile, nada que matar por PID",
                fase
            ));
        }
    }
    // D-03 (cobertura total): el residente `qwen_tts` desacopla su servidor real
    // del árbol del daemon, de modo que el kill por árbol puede dejarlo vivo con
    // el 8766 abierto. Si el puerto sigue abierto tras el árbol, se barre al PID
    // que lo escucha (preciso por puerto, sin kill por imagen).
    if puerto_abierto(8766) {
        hito(&format!(
            "reaper({}): 8766 abierto tras el árbol, barriendo residente",
            fase
        ));
        barrer_residente_por_puerto(fase);
    }
}

/// Barrido preciso del residente por puerto 8766 (D-03): localiza el PID en
/// LISTENING sobre el 8766 y mata su árbol con verificación de cierre.
/// Preciso: solo quien ocupa el 8766 (el residente de la sesión o su resto);
/// nunca kill por imagen (un `qwen_tts` local no ocupa el 8766).
fn barrer_residente_por_puerto(fase: &str) {
    #[cfg(windows)]
    {
        let mut pids: Vec<u32> = Vec::new();
        if let Ok(o) = std::process::Command::new("netstat")
            .args(["-ano", "-p", "TCP"])
            .output()
        {
            for ln in String::from_utf8_lossy(&o.stdout).lines() {
                let c: Vec<&str> = ln.split_whitespace().collect();
                // `TCP 127.0.0.1:8766 0.0.0.0:0 LISTENING 1234`
                if c.len() >= 5 && c[0] == "TCP" && c[1].ends_with(":8766") && c[3] == "LISTENING" {
                    if let Ok(p) = c[4].parse::<u32>() {
                        if p != 0 && p != std::process::id() && !pids.contains(&p) {
                            pids.push(p);
                        }
                    }
                }
            }
        }
        for p in pids {
            hito(&format!(
                "reaper({}): 8766 en PID {}, matando árbol",
                fase, p
            ));
            avi_daemon::matar_arbol_por_pid(p);
        }
        let t0 = std::time::Instant::now();
        while puerto_abierto(8766) && t0.elapsed() < std::time::Duration::from_secs(8) {
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        hito(&format!(
            "reaper({}): 8766 cerrado={}",
            fase,
            !puerto_abierto(8766)
        ));
    }
    #[cfg(not(windows))]
    {
        hito(&format!(
            "reaper({}): 8766 abierto; barrido Unix pendiente (solo log)",
            fase
        ));
    }
}

/// Falla fuera de polls con reaper previo (D-03): ejecuta el reaper
/// best-effort antes del `panic!` para no abandonar daemon ni motor vivos.
/// Todo `panic!`/`assert!` fuera de `esperar_estado_daemon` pasa por aquí.
fn fallo_con_reaper(fase: &str, mensaje: String) -> ! {
    reaper_ante_fallo(fase);
    panic!("{}", mensaje);
}

/// Guard RAII que extiende el reaper a todo `panic!`/`assert!` fuera de polls
/// (D-03): el test lo arma tras tomar el lock (`let _reaper =
/// armar_reaper("...")`); en salida normal no hace nada, y si el hilo está en
/// `panic!` al dropearse ejecuta el reaper best-effort y restaura la higiene
/// de `TEST_LIMITE` para no heredar techos al siguiente test del mismo hilo.
struct GuardReaper {
    fase: &'static str,
}

fn armar_reaper(fase: &'static str) -> GuardReaper {
    GuardReaper { fase }
}

impl Drop for GuardReaper {
    fn drop(&mut self) {
        if std::thread::panicking() {
            reaper_ante_fallo(self.fase);
            TEST_LIMITE.with(|c| *c.borrow_mut() = None);
        }
    }
}

/// Verificación ruidosa de cero huérfanos tras el apagado (H-01, T6 + D-03): el árbol
/// debe estar muerto, los puertos 8765/8766 cerrados y el pidfile sin PID vivo
/// (el producto lo borra tras muerte verificada). Si queda resto, ejecuta el
/// reaper best-effort antes de fallar con `panic!` detallado: la suite nunca
/// pasa en verde con huérfanos vivos ni los abandona en la vía de fallo.
fn verificar_cero_huerfanos(contexto: &str) {
    let pid = leer_pid_daemon();
    let pid_vivo = pid.map(avi_daemon::pid_vivo).unwrap_or(false);
    let p_daemon = puerto_abierto(8765);
    let p_residente = puerto_abierto(8766);
    let pidfile = avi_store::data_dir().join("daemon.pid");
    let pidfile_existe = pidfile.exists();
    if pid_vivo || p_daemon || p_residente || pidfile_existe {
        fallo_con_reaper(
            &format!("verificar_cero_huerfanos({})", contexto),
            format!(
                "quedaron huérfanos tras {}: pid={:?} vivo={} puerto8765={} puerto8766={} pidfile={} (el apagado debe dejar cero restos a nivel SO)",
                contexto,
                pid,
                pid_vivo,
                p_daemon,
                p_residente,
                pidfile.display()
            ),
        );
    }
    hito(&format!(
        "{}: cero huérfanos verificados a nivel SO",
        contexto
    ));
}

// ─── Fixture por sesión del daemon (Tarea 1) ──────────────────────────
//
// Dueña única del ciclo de vida del daemon en la corrida pesada serial: un
// solo arranque y un solo apagado por corrida, determinados por la fixture y
// no por cada test. Suprime N-1 calentamientos (la síntesis de `warmup_tts`
// en `crates/avi-daemon/src/lib.rs:1209-1227`, lanzada en segundo plano sin
// bloquear el bind en `crates/avi-daemon/src/lib.rs:1244-1248`) y elimina la
// clase de huérfanos por ciclos interrumpidos a mitad.
//
// Semántica que verifica (cierre H-01: producto + harness):
// - Revalidación con reclamo: la vida del residual se comprueba por PID vivo
//   más probe (`clasificar_residual` en `src/main.rs`), no por probe solo ni
//   pidfile solo. Sano (probe + PID vivo) → la fixture reutiliza sin
//   rearrancar; degradado (probe sin PID vivo, PID vivo sin probe, probe sin
//   pidfile) → `daemon start` reclama el árbol y rearranca con salida 0 y
//   payload `started`. La fixture exige `started` cuando partió de
//   detenido/degradado, nunca `already_running` ciego.
// - Rancio reconciliado: pidfile con PID muerto y probe falso → vía libre,
//   `start` fresco con `started` y pidfile reescrito.
// - Readiness observada (intento 2): `daemon start` solo espera bind-ready
//   (deadline 10 s, poll 250 ms en `src/main.rs:35-37`, retorna en cuanto
//   `/health` responde), pero el warmup TTS corre en segundo plano y la
//   inferencia solo es fiable con `warm == "warm"` (`daemon status` propaga
//   `warm` desde `/health` vía `status_body` en `src/main.rs`); la
//   fixture exige `daemon == esperado` MÁS `warm == "warm"`, sin los sleeps
//   fijos (300/500 ms) que los ciclos por test suponían.
// - Apagado con reaper ruidoso: `daemon stop` unificada con deadline global
//   de 8 s (`stop_daemon_and_resident` en `src/main.rs`); la fixture observa
//   `stopped` por poll MÁS cero huérfanos a nivel de sistema (árbol muerto,
//   puertos 8765/8766 cerrados, pidfile sin PID vivo vía
//   `verificar_cero_huerfanos`) y falla si queda resto, en vez de suponer la
//   ausencia tras un sleep o un solo probe HTTP.
// - Guards con reaper (D-03): `comprobar_guard` y los `panic!` de
//   `esperar_estado_daemon` (timeout o warm fallido) matan el árbol
//   best-effort (`reaper_ante_fallo`) antes de fallar; además todo `panic!`
//   fuera de polls (`ensure`/`shutdown`/`verificar_cero_huerfanos`/asserts de
//   los tests del ciclo) lleva reaper previo — vía `fallo_con_reaper` en el
//   harness y `GuardReaper` (Drop ante `panic!`) en los tests — más
//   recuperación del lock envenenado (`bloquear_estado`) e higiene de
//   `TEST_LIMITE` restaurada, para no abandonar huérfanos en vías anormales.
//
// Presupuesto explícito por test (serie permanente, sin tocar el producto):
// techo = timeout del cliente HTTP del daemon, 120 s por petición
// (`src/main.rs:2440`; el POST /dub lo acota a 10 s). Ningún test de la serie
// espera más que eso por una operación contra el residente.
//
// Contrato de locks (la fixture NO toma locks: los aportan los llamantes):
// - llamar SIEMPRE bajo `STATE_LOCK` (excluye paradas/arranques concurrentes
//   del ciclo entre tests del mismo binario);
// - añadir `TTS_LOCK` (`tts::lock_tts`) si el test hace inferencia (serie por
//   diseño: el residente ocupa el puerto 8766 y ~2.7 GB de RAM).
// La contención del estado compartido la dan los namespaces por test ya
// existentes (`etiqueta_unica`), no la fixture.
//
// `run_json_env` queda intacto: la captura por tempfile (sin pipe para
// heredar el write-end al hijo) sigue valiendo con la fixture — el arranque
// único reduce además los holders transitorios del pipe.
//
// Reversión (Tareas 1-2): devolver a cada test su ciclo propio
// (`daemon stop` + sleep + `daemon start` + sleep … `daemon stop` + sleep).

/// Estado observado del daemon vía `daemon status` (sin locks: los llamantes
/// los aportan según el contrato de la fixture por sesión).
fn estado_daemon() -> Value {
    let (_, actual) = run_json(&["--json", "daemon", "status"]);
    actual
}

/// Espera por estado OBSERVADO (poll cada 200 ms, como `wait_health_down` en
/// `src/main.rs:2427`) hasta ver `daemon == esperado`. Cuando se espera
/// `running`, además exige `warm == "warm"` (intento 2: el bind-ready no basta,
/// el warmup TTS corre en segundo plano y la inferencia solo es fiable en
/// caliente). Retorna en cuanto se observa — no es un sleep fijo — y falla
/// explícito (panic) al agotar los reintentos o si el warmup falló
/// (`warm_failed` con su causa): nunca pasa en silencio.
fn esperar_estado_daemon(esperado: &str, reintentos: u32) -> Value {
    let mut ultimo = Value::Null;
    for intento in 0..reintentos {
        comprobar_guard(&format!("esperar_estado_daemon({})", esperado));
        ultimo = estado_daemon();
        if ultimo["daemon"] == Value::String(esperado.to_string()) {
            if esperado != "running" {
                hito(&format!(
                    "esperar_estado_daemon: '{}' observado en intento {}/{}",
                    esperado,
                    intento + 1,
                    reintentos
                ));
                return ultimo;
            }
            if ultimo["warm"] == Value::String("warm".to_string()) {
                hito(&format!(
                    "esperar_estado_daemon: 'running+warm' observado en intento {}/{}",
                    intento + 1,
                    reintentos
                ));
                return ultimo;
            }
            if ultimo["warm"] == Value::String("warm_failed".to_string()) {
                reaper_ante_fallo("esperar_estado_daemon(warm_failed)");
                panic!(
                    "el warmup del daemon falló (warm_error: {}) (último: {})",
                    ultimo.get("warm_error").unwrap_or(&Value::Null),
                    ultimo
                );
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    reaper_ante_fallo(&format!("esperar_estado_daemon({})-agotado", esperado));
    panic!(
        "el daemon no alcanzó el estado '{}' (con warm) tras {} reintentos (último: {})",
        esperado, reintentos, ultimo
    );
}

/// Asegura el daemon caliente de la sesión (idempotente). Revalidación
/// coherente con matar-y-rearrancar (H-01, T6): la adhesión exige vida real
/// (probe + PID vivo), no solo probe. Sano (running+warm con PID vivo) →
/// reutiliza sin rearrancar; sano en calentamiento (running con PID vivo pero
/// sin warm) → espera el warm sin rearrancar; degradado o detenido (sin PID
/// vivo) → `daemon start` reclama o parte de cero con salida 0 y payload
/// `started`. En todo caso espera (poll acotado, panic al agotar) a
/// `warm == "warm"` antes de retornar.
///
/// Fallos fuera de polls (D-03): todo `panic!` aquí lleva reaper best-effort
/// previo vía `fallo_con_reaper`, para no abandonar huérfanos en vías anormales.
fn ensure_session_daemon() {
    let actual = estado_daemon();
    let pid = leer_pid_daemon();
    let vivo = pid.map(avi_daemon::pid_vivo).unwrap_or(false);
    let running = actual["daemon"] == Value::String("running".to_string());
    let warm = actual["warm"] == Value::String("warm".to_string());
    if running && warm && vivo {
        hito("ensure_session_daemon: reutilización (sano: probe + PID vivo)");
        return;
    }
    if running && vivo {
        hito("ensure_session_daemon: sano en calentamiento (PID vivo), esperando warm (50 reintentos)");
    } else {
        hito(&format!(
            "ensure_session_daemon: residual degradado o detenido (daemon={}, warm={}, pid={:?} vivo={}), reclamo vía start",
            actual.get("daemon").unwrap_or(&Value::Null),
            actual.get("warm").unwrap_or(&Value::Null),
            pid,
            vivo
        ));
        let (code, nuevo) = run_json(&["--json", "daemon", "start"]);
        if code != 0 {
            fallo_con_reaper(
                "ensure_session_daemon(start)",
                format!(
                    "el reclamo/arranque de sesión debe salir 0 (fue {}): {}",
                    code, nuevo
                ),
            );
        }
        if nuevo["daemon"] != Value::String("running".to_string()) {
            fallo_con_reaper(
                "ensure_session_daemon(daemon)",
                format!("tras start el daemon debe estar running: {}", nuevo),
            );
        }
        // Sin PID vivo previo solo cabe fresco o reclamo: el producto responde
        // `started`, nunca `already_running` ciego.
        if nuevo["status"] != Value::String("started".to_string()) {
            fallo_con_reaper(
                "ensure_session_daemon(status)",
                format!(
                    "desde detenido/degradado el start debe reclamar o partir de cero (started): {}",
                    nuevo
                ),
            );
        }
        hito("ensure_session_daemon: start exit 0 (started), esperando warm (50 reintentos)");
    }
    esperar_estado_daemon("running", 50);
}

/// Apagado único de la sesión (idempotente) con reaper ruidoso (H-01, T6 + D-03).
/// Tolera exit 0 (`shutdown_sent`) y exit 5 (ya detenido); cualquier otro
/// código falla con reaper previo. La ausencia queda observada por poll (`stopped`)
/// MÁS cero huérfanos a nivel de sistema (árbol, puertos 8765/8766, pidfile):
/// `verificar_cero_huerfanos` falla con reaper si queda resto.
fn shutdown_session_daemon() {
    let (code, actual) = run_json(&["--json", "daemon", "stop"]);
    if !(code == 0 || code == 5) {
        fallo_con_reaper(
            "shutdown_session_daemon(stop)",
            format!(
                "el apagado de sesión debe salir 0 o 5 (fue {}): {}",
                code, actual
            ),
        );
    }
    if code == 0 {
        hito("shutdown_session_daemon: stop exit 0 (shutdown_sent), esperando stopped (75 reintentos)");
    } else {
        hito("shutdown_session_daemon: stop exit 5 tolerado (ya detenido), esperando stopped (75 reintentos)");
    }
    esperar_estado_daemon("stopped", 75);
    verificar_cero_huerfanos("shutdown_session_daemon");
    hito("shutdown_session_daemon: apagado ok (stopped + cero huérfanos SO)");
}

/// Carga una fixture dorada desde `tests/golden/`.
fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("fixture {} no es JSON válido: {}", name, e))
}

/// D-03: el lock envenenado se recupera (hermético, sin daemon). Un hilo
/// provoca `panic!` con el lock tomado (envenena `STATE_LOCK`); el siguiente
/// `bloquear_estado()` debe adquirirlo sin reventar en `unwrap()`.
#[test]
fn d03_lock_envenenado_se_recupera() {
    let r = std::thread::spawn(|| {
        let _g = STATE_LOCK.lock().unwrap();
        panic!("veneno intencional D-03");
    })
    .join();
    assert!(r.is_err(), "el hilo debe haber hecho panic");
    // El siguiente test (aquí mismo) lo adquiere vía recuperación.
    let _g = bloquear_estado();
    // Higiene: no heredar techo al siguiente test del mismo hilo.
    TEST_LIMITE.with(|c| *c.borrow_mut() = None);
}

/// D-03: el reaper ante fallo fuera de polls con pidfile sin PID vivo y
/// puertos cerrados no falla ni deja huérfanos (hermético, sin daemon real).
/// Si hay daemon vivo o puertos abiertos se salta sin efectos.
#[test]
fn d03_reaper_sin_pid_vivo_no_falla() {
    if puerto_abierto(8765) || puerto_abierto(8766) {
        eprintln!("[d03] skip: puertos 8765/8766 abiertos (daemon vivo)");
        return;
    }
    if leer_pid_daemon().is_some() {
        eprintln!("[d03] skip: hay pidfile real (no tocar el ciclo)");
        return;
    }
    // Pidfile rancio: PID garantizado muerto.
    let pid_muerto = 2_000_000_000u32;
    assert!(
        !avi_daemon::pid_vivo(pid_muerto),
        "el PID de prueba debe estar muerto"
    );
    let path = avi_store::data_dir().join("daemon.pid");
    let previo_existe = path.exists();
    if !previo_existe {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).expect("crear data_dir");
        }
        std::fs::write(&path, format!("{{\"pid\": {}}}", pid_muerto))
            .expect("escribir pidfile rancio");
    }
    // El reaper best-effort no debe fallar con PID muerto y puertos cerrados.
    reaper_ante_fallo("d03-prueba");
    assert!(!puerto_abierto(8765) && !puerto_abierto(8766));
    if !previo_existe {
        let _ = std::fs::remove_file(&path);
    }
}

/// Ejecuta el binario con `args` y envs extra, devolviendo (código de salida, stdout
/// parseado a JSON). Las envs se inyectan vía `.env()` en el `Command` hijo: los tests
/// que necesitan aislar el estado del sandbox (p. ej. `LOCALAPPDATA`/`HF_*` en
/// `uninstall_force_no_se_auto_mata`) las pasan por aquí; `run_json` delega sin envs.
///
/// Usa un *tempfile* (`Stdio::File`) en vez de `Command::output()` (que captura `stdout`
/// vía un **pipe** con `bInheritHandle=TRUE`). El comando `daemon start` lanza el daemon
/// hijo (y este, a su vez, `qwen_tts.exe` vendido/precompilado) que heredan el pipe del
/// test: `output()` no retorna hasta que **todos** los holders del write-end lo cierran —
/// es decir, hasta el graceful shutdown del daemon (~10 s) — colgando el E2E en timeout
/// (exit 124). El fix de `spawn_background` (`CREATE_NO_HANDLE_INHERIT` + `Stdio::null`)
/// es necesario pero insuficiente: el binario vendido no respeta `creation_flags` y Rust
/// std decide `bInheritHandles` de forma independiente al flag. Al redirigir `stdout` a
/// un tempfile **no hay pipe** para heredar: `spawn()`+`wait()` retorna en cuanto el CLI
/// termina (~1.3 s tras `daemon start` con el bind-first).
///
/// Patrón equivalente al del legacy Python: el daemon no comparte I/O (pipe) con el
/// proceso que lo lanza.
fn run_json_env(args: &[&str], envs: &[(&str, &str)]) -> (i32, Value) {
    let t_cmd = Instant::now();
    let (tmp, file) = open_atomic_tmp();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(file)
        .stderr(std::process::Stdio::null());
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("el binario debe ejecutarse");
    let status = child.wait().expect("el proceso debe terminar");
    let stdout = std::fs::read_to_string(&tmp)
        .unwrap_or_else(|e| panic!("no se pudo leer tempfile {}: {}", tmp.display(), e));
    let _ = std::fs::remove_file(&tmp);
    let code = status
        .code()
        .expect("el proceso debe terminar con un código");
    let json: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout no es JSON válido ({}): {:?}", e, stdout));
    hito(&format!(
        "run_json_env: `{}` → exit {} en {} ms",
        args.join(" "),
        code,
        t_cmd.elapsed().as_millis()
    ));
    (code, json)
}

/// Ejecuta el binario con `args` (sin envs extra). Delega en [`run_json_env`].
fn run_json(args: &[&str]) -> (i32, Value) {
    run_json_env(args, &[])
}

/// Crea un tempfile único con semántica atómica `O_CREAT|O_EXCL` (`create_new`).
///
/// Corrección estructural sin sobreingeniería: la causa no es el contenido del
/// daemon sino el primitivo de FS. `File::create` trunca y no es atómico: con
/// `SystemTime` de resolución gruesa en Windows + `cargo test` paralelo, dos
/// hilos generan el mismo `tmp` y el `remove_file` de uno borra el de otro
/// -> `read_to_string` falla con `NotFound (os 2)` solo en `win/server-2022`.
/// Se reemplaza por creación atómica `O_CREAT|O_EXCL` (`create_new(true)`):
/// el SO garantiza exclusión y el bucle reintenta solo en colisión, sin
/// depender de `ThreadId`/`sleep`/`retry` sintomático. Es el mismo coste que
/// `tempfile::NamedTempFile` pero sin añadir dependencia.
fn open_atomic_tmp() -> (PathBuf, std::fs::File) {
    let mut attempts = 0;
    loop {
        let candidate = std::env::temp_dir().join(format!(
            "cli_golden_{}_{}_{}.out",
            std::process::id(),
            TMP_COUNTER.fetch_add(1, Ordering::SeqCst),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(f) => break (candidate, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                attempts += 1;
                if attempts > 10 {
                    panic!("no se pudo crear tempfile único tras 10 intentos: {}", e);
                }
                continue;
            }
            Err(e) => panic!("no se pudo crear tempfile {}: {}", candidate.display(), e),
        }
    }
}

/// Modelo Parakeet TDT v3 presente. Los binarios bajo `models/` están
/// gitignoreados: en un checkout limpio (CI) los E2E que los requieren se
/// saltan con aviso; en desarrollo corren completos. Solo se compila con
/// `native-stt`: sin el feature el binario no transcribe, así que los E2E que
/// dependen de él se gatean por feature (no solo por presencia de modelo).
#[cfg(feature = "native-stt")]
fn parakeet_model_disponible() -> bool {
    avi_store::ModelStore::new().is_provisioned("parakeet-tdt-v3")
}

/// Modelo CT2 es→en presente (mismo criterio de skip que el Parakeet). Solo se
/// compila con `native-translation`: sin el feature el binario no traduce, así
/// que el E2E que lo usa se gatea por feature (no solo por presencia de modelo).
#[cfg(feature = "native-translation")]
fn ct2_model_disponible() -> bool {
    avi_store::is_ct2_provisioned("es-en") && avi_store::is_ct2_provisioned("en-es")
}

#[test]
fn version_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "version"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_version.json"));
}

// Requiere `native-stt`: sin el motor Parakeet el binario responde
// `stt_unsupported`, por lo que el contrato de transcripción solo aplica con el
// feature activo (en CI featureless no se compila).
#[cfg(feature = "native-stt")]
#[test]
fn speech_transcribe_con_audio_cumple_contrato() {
    if !parakeet_model_disponible() {
        eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (hf_cache_dir/ gitignoreado — ejecuta setup --with-stt)");
        return;
    }
    // Régimen con fixture (Tarea 5): testigo natural del despacho `Auto`
    // (`src/main.rs:794-810`): con sesión en ejecución delega al daemon, sin
    // ella cae a directo; ambas rutas emiten {text, source}+schema, así que
    // las invariantes no dependen de la ruta efectiva. El lock excluye
    // paradas del ciclo durante la petición (sin reenrute forzado: sin flags
    // `--daemon`/`--no-daemon`).
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--audio",
        "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(actual["source"], Value::String("es-latam".to_string()));
    let text = actual["text"].as_str().expect("`text` debe ser un string");
    assert!(!text.is_empty(), "`text` no debe estar vacío");
}

#[test]
fn speech_transcribe_sin_audio_ni_mic_sale_con_codigo_2() {
    let output = Command::new(BIN)
        .args([
            "--json",
            "speech",
            "transcribe",
            "--source-language",
            "es-latam",
        ])
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    assert_eq!(
        code, 2,
        "omitir --audio y --mic debe mapear a ExitCode::InvalidInput"
    );
}

#[test]
fn daemon_status_coincide_con_fixture() {
    // Desdoble por régimen (Tarea 5): con fixture de sesión en ejecución el
    // estado efectivo es `running`; sin daemon, `stopped` intacto. Se elimina
    // la comparación incondicional contra la fixture detenida (falso rojo
    // bajo sesión).
    // D-04: el `stopped` por probe incluye en el producto la búsqueda del
    // residente (8766) antes de declarar vía libre — el display sigue
    // `stopped` (contrato), pero `start` ante residente-solo reclama con
    // `started`, nunca declara fresco sin reclaim.
    let (code, actual) = run_json(&["--json", "daemon", "status"]);
    assert_eq!(code, 0);
    if actual["daemon"] == Value::String("running".to_string()) {
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        let expected = fixture("cli_daemon_status_running.json");
        assert_eq!(actual["daemon"], expected["daemon"]);
    } else {
        assert_eq!(actual, fixture("cli_daemon_status.json"));
    }
}

#[test]
fn cleanup_coincide_con_fixture() {
    // Redefinido: cleanup sin flags → exit 2 usage_error (paridad oráculo, CONTRACT §11)
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 2, "cleanup sin flags debe ser InvalidInput");
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
}

#[test]
fn cleanup_sin_flags_es_exit_2() {
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 2);
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
}

#[test]
fn cleanup_voices_coincide_con_fixture() {
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup", "--voices", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["status"], Value::String("cleanup_complete".to_string()));
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
}

#[test]
fn cleanup_synthetic_speech_coincide_con_fixture() {
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup", "--synthetic-speech", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["status"], Value::String("cleanup_complete".to_string()));
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
}

#[test]
fn cleanup_model_coincide_con_fixture() {
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup", "--model", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["status"], Value::String("cleanup_complete".to_string()));
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
}

#[test]
fn cleanup_all_coincide_con_fixture() {
    let _guard = bloquear_estado();
    let (code, actual) = run_json(&["--json", "cleanup", "--all", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["status"], Value::String("cleanup_complete".to_string()));
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
}

#[test]
fn cleanup_dry_run_coincide_con_fixture() {
    let _guard = bloquear_estado();
    // --voices + --dry-run es el caso canónico de dry_run; valida fixture dedicada
    let (code, actual) = run_json(&["--json", "cleanup", "--voices", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    // fixture de referencia para dry_run debe coincidir en status
    let expected = fixture("cli_cleanup_dry_run.json");
    assert_eq!(actual["status"], expected["status"]);
    assert_eq!(actual["dry_run"], expected["dry_run"]);
}

/// Regresión del self-kill de `uninstall --force` en Windows (v0.18.10–v0.18.25):
/// el fallback `taskkill /F /IM ai-voice-interconnector.exe` mataba al propio CLI
/// (daemon y CLI comparten la imagen del binario) antes de borrar PATH/install_dir,
/// retornando `exit 1` sin tocar nada. La corrección sustituyó ese fallback por un
/// kill por **PID** leído de `daemon.pid` (con guarda `pid != process::id()`). Si
/// reapareciera el kill por imagen, el binario bajo test moriría por `taskkill` y
/// `status.code()` devolvería `None`, provocando el fallo del test (regresión en CI
/// `test-windows`).
///
/// Sandbox aislado: `LOCALAPPDATA` (honrada en `handle_uninstall` para el
/// `install_dir`) y `HF_HUB_CACHE`/`HF_HOME` (honradas por `hf_cache_dir` en
/// `avi-store`) apuntan a un directorio temporal propio, con un `install_dir` falso
/// como marcador. El nombre del sandbox no empieza por `avi_` ni
/// `ai-voice-interconnector-install-`, ajeno al barrido de temp huérfano de
/// `uninstall`. Si hubiera un daemon real activo en 127.0.0.1:8765, el test se salta
/// con aviso: `cargo test` no debe detener el daemon del usuario (esa ruta la cubre
/// el E2E manual).
#[cfg(windows)]
#[test]
fn uninstall_force_no_se_auto_mata() {
    let _guard = bloquear_estado();

    // (a) Skip si hay daemon real activo (no detenerlo desde `cargo test`).
    let (_, status) = run_json(&["--json", "daemon", "status"]);
    if status["daemon"] == Value::String("running".to_string()) {
        eprintln!("[uninstall] skip: daemon activo en 127.0.0.1:8765");
        return;
    }

    // (b) Sandbox con install_dir falso (marcador) y caches HF aisladas.
    let sandbox = std::env::temp_dir().join(format!(
        "uninstall_sandbox_{}_{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::SeqCst),
    ));
    let local = sandbox.join("LocalAppData");
    let programs = local.join("Programs/ai-voice-interconnector");
    std::fs::create_dir_all(&programs)
        .unwrap_or_else(|e| panic!("no se pudo crear install_dir falso: {}", e));
    std::fs::write(programs.join("ai-voice-interconnector.exe"), b"marker")
        .unwrap_or_else(|e| panic!("no se pudo escribir el marcador: {}", e));
    let hf = sandbox.join("hf");
    std::fs::create_dir_all(&hf).unwrap();

    // (c) uninstall --force contra el sandbox aislado.
    let (code, actual) = run_json_env(
        &["--json", "uninstall", "--force"],
        &[
            ("LOCALAPPDATA", local.to_str().unwrap()),
            ("HF_HUB_CACHE", hf.to_str().unwrap()),
            ("HF_HOME", hf.to_str().unwrap()),
        ],
    );

    // (d) Invariante crítico: no auto-muerte, contrato JSON intacto.
    // H2+H4 atómicos: `PATH` canónico sin residuo y helper desacoplado
    // determinista — no se afirma `!programs.exists()` síncrono porque `H4`
    // es `Wait-Process PID` + `Remove-Item -LiteralPath` tras la salida del
    // padre (determinista sin `PermissionDenied` aviso).
    assert_eq!(
        code, 0,
        "uninstall --force no debe auto-matarse: {}",
        actual
    );
    assert_eq!(actual["status"], Value::String("uninstalled".to_string()));
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    // H4: el helper se crea en `TEMP\avi-uninstall-*.ps1` y borra
    // `install_dir` tras la muerte del PID; el padre no intenta
    // `remove_dir_all` síncrono. En sandbox sin `exe` vivo el helper lo
    // borra en <1s; se espera de forma determinista sin best-effort.
    {
        let mut ok = !programs.exists();
        for _ in 0..10 {
            if ok {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            ok = !programs.exists();
        }
        assert!(ok, "el install_dir del sandbox debe borrarse (H4 determinista)");
    }

    // (e) Limpieza del sandbox.
    let _ = std::fs::remove_dir_all(&sandbox);
}

#[test]
fn voice_list_respeta_el_contrato_de_envelope() {
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato (envelope + presencia de `default`).
    let (code, actual) = run_json(&["--json", "voice", "list"]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    let voices = actual["voices"]
        .as_array()
        .expect("`voices` debe ser un array");
    assert!(
        voices
            .iter()
            .any(|v| v == &Value::String("default".to_string())),
        "debe listarse la voz de fábrica `default`"
    );
}

#[test]
fn translate_texto_vacio_sale_con_codigo_2() {
    // Entrada inválida → ExitCode::InvalidInput (2), con el envelope de error del CLI.
    let (code, actual) = run_json(&["--json", "translate", "--text", ""]);
    assert_eq!(code, 2, "texto vacío debe mapear a ExitCode::InvalidInput");
    assert_eq!(actual, fixture("cli_translate_empty.json"));
}

// Requiere `native-translation`: sin el motor CT2 el binario responde
// `translation_unsupported`, por lo que este contrato solo aplica con el feature
// activo (en CI featureless no se compila).
#[cfg(feature = "native-translation")]
#[test]
fn translate_es_a_en_produce_traduccion() {
    if !ct2_model_disponible() {
        eprintln!("[translate] skip: sin modelo CT2 es→en");
        return;
    }
    // Régimen con fixture (Tarea 5): testigo natural del despacho `Auto`:
    // con sesión en ejecución la ruta efectiva es el daemon, sin ella el
    // directo; validaciones (vacío/passthrough/par) y envelope son comunes
    // antes del despacho (`src/main.rs:442-471`), así que las invariantes no
    // dependen de la ruta. El lock excluye paradas del ciclo durante la
    // petición (sin reenrute forzado).
    let _guard = bloquear_estado();
    // El texto traducido depende del motor real; se verifican invariantes de
    // contrato (mismo patrón que `speech_transcribe_con_audio_cumple_contrato`).
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Hola, ¿cómo estás?",
        "--from",
        "es",
        "--to",
        "en",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(actual["source"], Value::String("es".to_string()));
    assert_eq!(actual["target"], Value::String("en".to_string()));
    let translated = actual["translated"]
        .as_str()
        .expect("`translated` debe ser un string");
    assert!(!translated.is_empty(), "`translated` no debe estar vacío");
}

#[test]
fn translate_passthrough_mismo_idioma_devuelve_texto_intacto() {
    // Passthrough: origen == destino tras normalizar → texto intacto.
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Hola",
        "--from",
        "es",
        "--to",
        "es",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["translated"], Value::String("Hola".to_string()));
}

#[test]
fn translate_par_no_soportado_sale_con_codigo_2() {
    // Par no soportado → ExitCode::InvalidInput (2), ruta de validación pura
    // sin depender de ningún modelo.
    let (code, actual) = run_json(&[
        "--json",
        "translate",
        "--text",
        "Bonjour",
        "--from",
        "fr",
        "--to",
        "de",
    ]);
    assert_eq!(
        code, 2,
        "par no soportado debe mapear a ExitCode::InvalidInput"
    );
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(
        actual["reason"],
        Value::String("unsupported_language_pair".to_string())
    );
}

// ─── Golden TTS (Fase 5, Tarea 11) ───────────────────────────────────

mod tts {
    use super::*;
    // El trait STT solo se necesita para el cálculo de WER real (native-stt).
    #[cfg(feature = "native-stt")]
    use avi_core::engine::SttEngine;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Ruta del binario del motor Qwen3-TTS (override o vendored).
    fn tts_binario() -> Option<PathBuf> {
        if let Ok(b) = std::env::var("QWEN3_TTS_BIN") {
            let p = PathBuf::from(b);
            if !p.as_os_str().is_empty() {
                return Some(p);
            }
        }
        let vendored = PathBuf::from("vendor/qwen3-tts/qwen_tts.exe");
        if vendored.is_file() {
            return Some(vendored);
        }
        None
    }

    /// Pesos del modelo Qwen3-TTS 0.6B presentes.
    fn tts_pesos() -> bool {
        Path::new("vendor/qwen3-tts/qwen3-tts-0.6b").is_dir()
    }

    /// Estado de provisión VERIFICADO AHORA (no cacheado): `doctor` consulta los
    /// snapshots HF vigentes. Si falta, corre `setup` una sola vez bajo lock
    /// (evita descargas paralelas) y re-verifica. No se cachea el resultado
    /// porque `cleanup_coincide_con_fixture` puede borrar la provisión en otro
    /// hilo entre tests: un caché obsoleto hacía que tests TTS posteriores a
    /// cleanup confiaran en estado ya eliminado (`model_missing`).
    fn tts_modelo_registrado() -> bool {
        static SETUP_LOCK: Mutex<()> = Mutex::new(());
        let doctor_ok = || {
            Command::new(BIN)
                .args(["doctor"])
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };
        if doctor_ok() {
            return true;
        }
        let _guard = SETUP_LOCK.lock().unwrap();
        if doctor_ok() {
            return true;
        }
        matches!(
            Command::new(BIN).args(["setup"]).output(),
            Ok(o) if o.status.success()
        )
    }

    /// Provisto = modelo registrado + binario + pesos.
    fn tts_provisioned() -> bool {
        tts_modelo_registrado() && tts_binario().is_some() && tts_pesos()
    }

    /// El clonado de voz exige el modelo Base del motor (graft ICL); el modelo
    /// CustomVoice vendorizado (`qwen3-tts-0.6b/`) no sirve para clonado. El
    /// modelo Base se provisiona vía `ModelStore` snapshot HF o directorio vendored
    /// (`qwen3-tts-0.6b-base/`, `config.json: "tts_model_type": "base"`).
    fn tts_clone_provisioned() -> bool {
        if !tts_provisioned() {
            return false;
        }
        if avi_store::ModelStore::new().is_provisioned("qwen3-tts-0.6b-base") {
            return true;
        }
        let config = Path::new("vendor/qwen3-tts/qwen3-tts-0.6b-base/config.json");
        match std::fs::read_to_string(config) {
            Ok(c) => c.contains("\"tts_model_type\": \"base\""),
            Err(_) => false,
        }
    }

    /// Mutex global para serializar los tests TTS pesados (el motor residente
    /// ocupa el puerto 8766 y cada corrida consume ~2.7 GB de RAM). Un fallo de
    /// un test no debe envenenar el lock de los demás.
    static TTS_LOCK: Mutex<()> = Mutex::new(());

    fn lock_tts() -> std::sync::MutexGuard<'static, ()> {
        TTS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Etiqueta/voz única por corrida (el oráculo normaliza a minúsculas).
    fn etiqueta_unica(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("reloj del sistema")
            .as_nanos();
        format!("{}{}_{}", prefix, nanos, std::process::id())
    }

    /// El WAV producido debe ser PCM s16le mono 24 kHz con muestras (spec del motor).
    /// Solo lo usan los E2E de síntesis que verifican WER real (native-stt).
    #[cfg(feature = "native-stt")]
    fn wav_valido_24k(path: &Path) {
        let reader = hound::WavReader::open(path)
            .unwrap_or_else(|e| panic!("WAV ilegible en {}: {}", path.display(), e));
        let spec = reader.spec();
        assert_eq!(spec.sample_rate, 24_000, "muestreo del motor: 24 kHz");
        assert_eq!(spec.channels, 1, "mono");
        assert_eq!(spec.bits_per_sample, 16, "16-bit PCM");
        assert!(reader.duration() > 0, "no puede estar vacío");
    }

    /// WER (por palabras normalizadas, Levenshtein) del WAV frente al texto
    /// fuente, vía Parakeet TDT v3 (ort/ONNX Runtime).
    #[cfg(feature = "native-stt")]
    fn wer_vs_texto(path: &Path, texto: &str) -> f64 {
        let pcm = avi_audio::load_wav_16k_mono_pcm(path.to_string_lossy().as_ref())
            .unwrap_or_else(|e| panic!("no se pudo cargar {} a 16k: {}", path.display(), e));
        let snapshot = avi_store::ModelStore::new()
            .model_snapshot_path("parakeet-tdt-v3")
            .expect("snapshot HF parakeet-tdt-v3 no provisionado — ejecuta setup --with-stt");
        let engine = avi_stt::ParakeetEngine::new(snapshot)
            .expect("el modelo Parakeet TDT v3 debe existir");
        let transcrito = engine
            .transcribe(&pcm, Some("es"))
            .expect("la transcripción no debe fallar");
        let a = normalizar(&transcrito);
        let b = normalizar(texto);
        if b.is_empty() {
            return 1.0;
        }
        let d = levenshtein(&a, &b);
        d as f64 / b.len() as f64
    }

    /// Palabras minúsculas sin diacríticos ni puntuación (señal de habla
    /// limpia). El plegado de diacríticos es manual para no depender de
    /// `unicode-normalization`.
    #[cfg(feature = "native-stt")]
    fn normalizar(s: &str) -> Vec<String> {
        s.to_lowercase()
            .chars()
            .map(|c| match c {
                'á' | 'ä' => 'a',
                'é' | 'ë' => 'e',
                'í' | 'ï' => 'i',
                'ó' | 'ö' => 'o',
                'ú' | 'ü' => 'u',
                'ñ' => 'n',
                c if c.is_ascii_alphanumeric() => c,
                _ => ' ',
            })
            .collect::<String>()
            .split_whitespace()
            .map(|w| w.to_string())
            .filter(|w| !w.is_empty())
            .collect()
    }

    /// Distancia de Levenshtein entre secuencias de palabras.
    #[cfg(feature = "native-stt")]
    fn levenshtein(a: &[String], b: &[String]) -> usize {
        let mut prev: Vec<usize> = (0..=b.len()).collect();
        for (i, x) in a.iter().enumerate() {
            let mut cur = vec![i + 1; b.len() + 1];
            for (j, y) in b.iter().enumerate() {
                cur[j + 1] = if x == y {
                    prev[j]
                } else {
                    1 + prev[j].min(cur[j]).min(prev[j + 1])
                };
            }
            prev = cur;
        }
        prev[b.len()]
    }

    /// Solo lo usan los E2E de reproducción/dub que dependen de STT (native-stt).
    #[cfg(feature = "native-stt")]
    fn hay_dispositivo_audio() -> bool {
        match avi_audio::get_devices_json() {
            Ok(devs) => !devs.is_empty(),
            Err(_) => false,
        }
    }

    // ─── synthesize ─────────────────────────────────────────────────────

    /// Éxito con `--label`: exit 0, WAV persistido en `speech/`, envelope y
    /// WER ≤ 0.25 frente al texto fuente. La verificación de WER exige el motor
    /// Parakeet (native-stt); sin el feature no se compila (en CI featureless los
    /// modelos tampoco están, así que no se pierde cobertura).
    #[cfg(feature = "native-stt")]
    #[test]
    fn synthesize_exito_con_label() {
        hito_inicio_pesado("tts::synthesize_exito_con_label");
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            hito_fin("tts::synthesize_exito_con_label (skip sin provisión)");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (hf_cache_dir/ gitignoreado — ejecuta setup --with-stt)");
            hito_fin("tts::synthesize_exito_con_label (skip sin STT)");
            return;
        }
        // Serie + ciclo: excluye cleanup (STATE) y paradas del ciclo durante la
        // vía caliente; el orden STATE→TTS coincide con el resto de la suite.
        let _state = bloquear_estado();
        let _guard = lock_tts();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = armar_reaper("synthesize_exito_con_label");
        let label = etiqueta_unica("golden");
        // Vía daemon caliente (Tarea 3): la sesión ya pagó la carga del motor
        // una sola vez; este test no recarga en frío. `--daemon` fuerza la ruta
        // (sin fallback silencioso a directo): si el daemon no responde, falla.
        // Paridad de envelope verificada en el producto: mismo
        // {status, audio_path, voice} persistido en el almacén, con chequeo de
        // colisión de etiqueta en el cliente en ambas rutas
        // (`src/main.rs:925-929` frente a `src/main.rs:984-988`;
        // `src/main.rs:2757` frente a `src/main.rs:948`).
        ensure_session_daemon();
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "speech",
            "synthesize",
            "--text",
            "Hola, este es un mensaje de prueba para la verificación.",
            "--voice",
            "default",
            "--label",
            &label,
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(
            audio_path,
            "Hola, este es un mensaje de prueba para la verificación.",
        );
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
        let _ = avi_store::SpeechStore::new().remove("default", &label);
        hito_fin("tts::synthesize_exito_con_label");
    }

    /// Gate WER texto corto — disparador exacto de H1 (Tarea 6).
    ///
    /// Cubre el caso que la E2E `test-windows-e2e` sintetizaba sin veredicto:
    /// texto de 2-4 palabras (`"Hola mundo"`) con voz `default` (preset ryan).
    /// Verifica `WAV 24kHz mono 16-bit` y `WER ≤ 0.25` vía Parakeet (`native-stt`),
    /// mismo patrón que `synthesize_exito_con_label` (11 palabras): requiere
    /// `tts_provisioned()` + `parakeet_model_disponible()`, usa `wav_valido_24k`
    /// y `wer_vs_texto`, falla la E2E/gate si `WER > 0.25`.
    #[cfg(feature = "native-stt")]
    #[test]
    fn synthesize_exito_texto_corto_wer_gate() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (hf_cache_dir/ gitignoreado — ejecuta setup --with-stt)");
            return;
        }
        let _guard = lock_tts();
        let texto_corto = "Hola mundo";
        let label = etiqueta_unica("golden_corto");
        // Testigo en directo de `synthesize` (Tarea 3): ruta local fijada con
        // `--no-daemon` para que la sesión en ejecución (Auto→daemon) no lo
        // reenrute en silencio. Es el más barato con gate WER.
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "synthesize",
            "--text",
            texto_corto,
            "--voice",
            "default",
            "--label",
            &label,
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(audio_path, texto_corto);
        assert!(
            wer <= 0.25,
            "WER texto corto '{}' = {} debe ser ≤ 0.25 (disparador H1)",
            texto_corto, wer
        );
        let _ = avi_store::SpeechStore::new().remove("default", &label);
    }

    #[test]
    fn synthesize_texto_vacio_sale_con_2() {
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "",
            "--label",
            "x",
        ]);
        assert_eq!(code, 2, "texto vacío → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("empty_text".to_string()));
    }

    #[test]
    fn synthesize_voz_inexistente_sale_con_3() {
        let _guard = bloquear_estado();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        // Ruta directa fijada (colateral del régimen con fixture): la
        // existencia de la voz solo se verifica en la rama local
        // (`src/main.rs:937-944`); la vía daemon la resuelve el residente.
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--voice",
            "voz_inexistente_xyz",
            "--label",
            "x",
        ]);
        assert_eq!(
            code, 3,
            "voz inexistente → ExitCode::NotFound (reason={:?})",
            actual["reason"]
        );
        assert_eq!(
            actual["reason"],
            Value::String("voice_not_found".to_string())
        );
    }

    /// Colisión de `--label` sin `--force` → 6. El almacén se fabrica con un
    /// sidecar + WAV mínimo (sin síntesis real).
    #[test]
    fn synthesize_colision_label_sale_con_6() {
        let _guard = bloquear_estado();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let label = etiqueta_unica("colision");
        let wav_min = {
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: 24_000,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut cursor = std::io::Cursor::new(Vec::new());
            {
                let mut w = hound::WavWriter::new(&mut cursor, spec).unwrap();
                w.write_sample(0i16).unwrap();
                w.finalize().unwrap();
            }
            cursor.into_inner()
        };
        let src = std::env::temp_dir().join(format!("{}_min.wav", label));
        std::fs::write(&src, &wav_min).unwrap();
        let store = avi_store::SpeechStore::new();
        store
            .save("default", &label, "fabricado", &src)
            .expect("el sidecar fabricado debe guardarse");
        let _ = std::fs::remove_file(&src);
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--label",
            &label,
        ]);
        assert_eq!(
            code, 6,
            "colisión de etiqueta → ExitCode::StateConflict (reason={:?})",
            actual["reason"]
        );
        assert_eq!(actual["reason"], Value::String("label_exists".to_string()));
        let _ = store.remove("default", &label);
    }

    // ─── say ───────────────────────────────────────────────────────────

    // Verifica WER real vía Parakeet (native-stt); sin el feature no se compila.
    #[cfg(feature = "native-stt")]
    #[test]
    fn say_exito_reproduce() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !hay_dispositivo_audio() {
            eprintln!("[tts] skip: sin dispositivo de salida de audio");
            return;
        }
        if !parakeet_model_disponible() {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (hf_cache_dir/ gitignoreado — ejecuta setup --with-stt)");
            return;
        }
        let _guard = lock_tts();
        // Testigo en directo de `say` (Tarea 3): ruta local fijada con
        // `--no-daemon` — la vía daemon borra su WAV efímero tras reproducir
        // (`src/main.rs:2823`) y rompería la verificación sobre archivo.
        // Humo único de audio (Tarea 4): esta es la ÚNICA reproducción real de
        // la suite, acotada a un texto corto ("Hola mundo", <2 s); la
        // verificación es sobre el archivo (WAV válido + WER), no sobre el
        // altavoz. Gate de dispositivo como hoy (sin mezclador no hay humo).
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "say",
            "--text",
            "Hola mundo",
            "--voice",
            "default",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("reproduced".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        wav_valido_24k(audio_path);
        let wer = wer_vs_texto(audio_path, "Hola mundo");
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn say_texto_vacio_sale_con_2() {
        let (code, actual) = run_json(&["--json", "speech", "say", "--text", ""]);
        assert_eq!(code, 2, "texto vacío → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("empty_text".to_string()));
    }

    // ─── dub ───────────────────────────────────────────────────────────

    /// Passthrough es→es con `--audio`: exit 0, WAV válido y WER ≤ 0.25 frente
    /// al texto transcrito (el pipeline devuelve `text`). El dub arranca por STT,
    /// así que exige `native-stt`; sin el feature no se compila.
    #[cfg(feature = "native-stt")]
    #[test]
    fn dub_audio_passthrough_es_es() {
        if !tts_provisioned() {
            eprintln!("[tts] skip: sin modelo/binario Qwen3-TTS provisionados");
            return;
        }
        if !avi_store::ModelStore::new().is_provisioned("parakeet-tdt-v3") {
            eprintln!("[stt] skip: sin modelo Parakeet TDT v3 (hf_cache_dir/ gitignoreado — ejecuta setup --with-stt)");
            return;
        }
        if !hay_dispositivo_audio() {
            eprintln!("[tts] skip: sin dispositivo de salida de audio");
            return;
        }
        let _guard = lock_tts();
        // Testigo en directo de `dub` (Tarea 3): ruta local fijada con
        // `--no-daemon` para que la sesión en ejecución no lo reenrute.
        // Verificación solo-archivo (Tarea 4): WAV válido + WER sobre el
        // archivo producido. El gate de audio se conserva porque `dub`
        // reproduce siempre en ambas rutas (`src/main.rs:1265` y
        // `src/main.rs:3059`): sin mezclador el comando falla con
        // `playback_failed` y no hay archivo que verificar.
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "dub",
            "--audio",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
            "--source-language",
            "es-latam",
            "--target-language",
            "es-latam",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        wav_valido_24k(audio_path);
        let texto = actual["text"].as_str().expect("text debe existir");
        let wer = wer_vs_texto(audio_path, texto);
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn dub_archivo_inexistente_sale_con_3() {
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "dub",
            "--audio",
            "no-existe.wav",
            "--source-language",
            "es-latam",
        ]);
        assert_eq!(code, 3, "archivo inexistente → ExitCode::NotFound");
        assert_eq!(
            actual["reason"],
            Value::String("audio_not_found".to_string())
        );
    }

    // ─── voice clone ───────────────────────────────────────────────────

    #[test]
    fn voice_clone_exito() {
        let _state = bloquear_estado();
        if !tts_clone_provisioned() {
            eprintln!(
                "[tts] skip: el clonado exige el modelo Base del motor Qwen3-TTS \
                 (usa setup --with-base)"
            );
            return;
        }
        let _guard = lock_tts();
        let name = etiqueta_unica("clon");
        // Testigo en directo de `clone` (Tarea 3): ruta local fijada con
        // `--no-daemon` para que la sesión en ejecución no lo reenrute.
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "voice",
            "clone",
            "--name",
            &name,
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["name"], Value::String(name.clone()));
        assert_eq!(actual["precomputed"], Value::Bool(false));
        let speech = actual["speech"].as_str().expect("speech debe existir");
        let qvoice = Path::new(speech);
        assert!(qvoice.is_file(), "reference.qvoice debe existir");
        let size = std::fs::metadata(qvoice).expect("metadata").len();
        assert!(
            size > 1_000_000,
            "el .qvoice debe pesar > 1 MB (era {})",
            size
        );
        let _ = avi_store::VoiceStore::new().remove(&name);
    }

    /// Clonado repetido → 6. La voz existente se fabrica con un `.qvoice` mínimo.
    #[test]
    fn voice_clone_repetido_sale_con_6() {
        let _guard = bloquear_estado();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let name = etiqueta_unica("clon");
        let voices = avi_store::VoiceStore::new();
        let dir = voices.voice_dir(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reference.qvoice"), b"QVCE").unwrap();
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            &name,
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 6, "voz existente → ExitCode::StateConflict");
        assert_eq!(actual["reason"], Value::String("voice_exists".to_string()));
        let _ = voices.remove(&name);
    }

    #[test]
    fn voice_clone_nombre_invalido_sale_con_2() {
        let _guard = bloquear_estado();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            "voz invalida",
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 2, "nombre inválido → ExitCode::InvalidInput");
        assert_eq!(
            actual["reason"],
            Value::String("invalid_voice_name".to_string())
        );
    }

    #[test]
    fn voice_clone_audio_inexistente_sale_con_3() {
        // Serializa con el resto de la suite (patrón de los demás `voice_clone_*`):
        // los E2E de daemon, al apagarse, matan `qwen_tts.exe` por nombre de imagen
        // (global), y sin este lock la síntesis de este test podría cruzarse con ese
        // kill en paralelo y salir con un código distinto de 3.
        let _guard = bloquear_estado();
        if !tts_modelo_registrado() {
            eprintln!("[tts] skip: sin ModelStore escribible");
            return;
        }
        let (code, actual) = run_json(&[
            "--json",
            "voice",
            "clone",
            "--name",
            "clon_ok",
            "--speech-reference",
            "no-existe.wav",
        ]);
        assert_eq!(code, 3, "audio inexistente → ExitCode::NotFound");
        assert_eq!(
            actual["reason"],
            Value::String("audio_not_found".to_string())
        );
    }

    // ─── daemon start/status/restart ────────────────────────────────

    #[test]
    fn daemon_start_exito() {
        hito_inicio_pesado("tts::daemon_start_exito");
        let _guard = bloquear_estado();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        // D-02: la ventana spawn→write ya no ciega al handler (PID en memoria).
        let _reaper = armar_reaper("daemon_start_exito");
        // Skip sin efectos: no tocar el ciclo si no hay provisión.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para daemon start");
            hito_fin("tts::daemon_start_exito (skip sin provisión)");
            return;
        }
        // Precondición observada (dueña: fixture de sesión): partir de detenido
        // con cero huérfanos verificados a nivel SO.
        // D-04: ante `Parado` con residente vivo el `start` reclama su árbol
        // (preciso-primero, imagen del residente solo último recurso) antes de
        // declarar fresco — desde detenido solo cabe `started`.
        shutdown_session_daemon();
        let (code, actual) = run_json(&["--json", "daemon", "start"]);
        // Desde detenido solo cabe fresco o reclamo: exit 0 con `started`
        // (matar-y-rearrancar, H-01), nunca `already_running` ciego.
        assert!(
            code == 0,
            "daemon start debe salir 0, fue {} reason {:?}",
            code,
            actual
        );
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert_eq!(
            actual["status"],
            Value::String("started".to_string()),
            "desde detenido el start debe partir de cero (started): {}",
            actual
        );
        // Presencia a nivel de sistema, no solo probe: pidfile con PID vivo.
        let pid = leer_pid_daemon();
        assert!(
            pid.map(avi_daemon::pid_vivo).unwrap_or(false),
            "el daemon recién arrancado debe estar vivo a nivel SO (pid {:?})",
            pid
        );
        // Verificar status running (observado por la fixture, sin sleeps fijos).
        esperar_estado_daemon("running", 50);
        // Cleanup garantizado por la fixture: apagado único con cero huérfanos
        // verificados a nivel SO (convención de la sesión).
        shutdown_session_daemon();
        let (code3, actual3) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(code3, 0);
        assert_eq!(
            actual3["daemon"],
            Value::String("stopped".to_string()),
            "tras stop debe quedar stopped"
        );
        hito_fin("tts::daemon_start_exito");
    }

    #[test]
    fn daemon_restart_rearma() {
        hito_inicio_pesado("tts::daemon_restart_rearma");
        let _guard = bloquear_estado();
        // D-03: reaper en todo `panic!` fuera de polls.
        let _reaper = armar_reaper("daemon_restart_rearma");
        // Skip sin efectos: no tocar el ciclo si no hay provisión.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para daemon restart");
            hito_fin("tts::daemon_restart_rearma (skip sin provisión)");
            return;
        }
        // Base observada: daemon en ejecución (revalidado con reclamo por la
        // fixture si el residual estuviera degradado).
        ensure_session_daemon();
        let previo = leer_pid_daemon();
        let (code, actual) = run_json(&["--json", "daemon", "restart"]);
        assert_eq!(code, 0, "daemon restart debe salir 0");
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert!(actual.get("pid").is_some() || actual.get("status").is_some());
        // Rearme a nivel de sistema: el PID nuevo está vivo y el árbol previo,
        // si cambió el PID, quedó muerto (sin huérfano del ciclo anterior).
        let nuevo = actual
            .get("pid")
            .and_then(|p| p.as_u64())
            .map(|n| n as u32)
            .or_else(leer_pid_daemon);
        assert!(
            nuevo.map(avi_daemon::pid_vivo).unwrap_or(false),
            "tras restart el daemon debe estar vivo a nivel SO (pid {:?})",
            nuevo
        );
        if let (Some(p), Some(q)) = (previo, nuevo) {
            if p != q {
                assert!(
                    !avi_daemon::pid_vivo(p),
                    "tras restart el árbol previo no debe quedar vivo (pid {})",
                    p
                );
            }
        }
        // Status debe seguir running (observado, sin sleeps fijos).
        esperar_estado_daemon("running", 50);
        // Restaurar detenido con cero huérfanos verificados a nivel SO
        // (convención de la sesión).
        shutdown_session_daemon();
        hito_fin("tts::daemon_restart_rearma");
    }

    #[test]
    fn daemon_status_running() {
        hito_inicio_pesado("tts::daemon_status_running");
        let _guard = bloquear_estado();
        // D-03: reaper en todo `panic!` fuera de polls.
        let _reaper = armar_reaper("daemon_status_running");
        // Skip sin efectos: no tocar el ciclo si no hay provisión.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado");
            hito_fin("tts::daemon_status_running (skip sin provisión)");
            return;
        }
        // Semántica verificada: `status` contra daemon en ejecución.
        ensure_session_daemon();
        let (code, actual) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(code, 0);
        // Endurecida: antes condicional (pasaba sin verificar si no estaba
        // running); ahora el `running` se exige porque la fixture lo garantiza.
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        // Presencia a nivel de sistema además del probe: el PID de la pista
        // está vivo (revalidación matar-y-rearrancar, H-01).
        let pid = leer_pid_daemon();
        assert!(
            pid.map(avi_daemon::pid_vivo).unwrap_or(false),
            "con status running el PID de la pista debe estar vivo (pid {:?})",
            pid
        );
        // Cuando está running, el fixture running debe coincidir (schema_version 3)
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        let expected = fixture("cli_daemon_status_running.json");
        // Comparar daemon y engine
        assert_eq!(actual["daemon"], expected["daemon"]);
        // Restaurar detenido (convención de la sesión: sin huérfanos al cerrar).
        shutdown_session_daemon();
        hito_fin("tts::daemon_status_running");
    }

    #[test]
    fn daemon_help_lista_auto_restart() {
        // Verifica que --help de start/serve lista los flags restaurados
        let out_start = Command::new(BIN)
            .args(["daemon", "start", "--help"])
            .output()
            .expect("daemon start --help");
        let stdout_start = String::from_utf8_lossy(&out_start.stdout);
        let stderr_start = String::from_utf8_lossy(&out_start.stderr);
        let combined_start = format!("{}{}", stdout_start, stderr_start);
        assert!(
            combined_start.contains("--auto-restart"),
            "daemon start --help debe listar --auto-restart, fue: {}",
            combined_start
        );
        assert!(
            combined_start.contains("--max-retries"),
            "daemon start --help debe listar --max-retries, fue: {}",
            combined_start
        );
        let out_serve = Command::new(BIN)
            .args(["daemon", "serve", "--help"])
            .output()
            .expect("daemon serve --help");
        let stdout_serve = String::from_utf8_lossy(&out_serve.stdout);
        let stderr_serve = String::from_utf8_lossy(&out_serve.stderr);
        let combined_serve = format!("{}{}", stdout_serve, stderr_serve);
        assert!(
            combined_serve.contains("--auto-restart"),
            "daemon serve --help debe listar --auto-restart, fue: {}",
            combined_serve
        );
        assert!(
            combined_serve.contains("--max-retries"),
            "daemon serve --help debe listar --max-retries, fue: {}",
            combined_serve
        );
    }

    #[test]
    fn daemon_start_con_auto_restart() {
        hito_inicio_pesado("tts::daemon_start_con_auto_restart");
        let _guard = bloquear_estado();
        // D-03: reaper en todo `panic!` fuera de polls.
        let _reaper = armar_reaper("daemon_start_con_auto_restart");
        // Skip sin efectos: no tocar el ciclo si no hay provisión.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para auto-restart");
            hito_fin("tts::daemon_start_con_auto_restart (skip sin provisión)");
            return;
        }
        // Precondición observada (dueña: fixture de sesión): partir de detenido
        // con cero huérfanos verificados a nivel SO.
        shutdown_session_daemon();
        // Start con supervisor habilitado y max 1 (no debe fallar en estado sano).
        // D-05: de semántica pasiva (`bind` de prueba) a reclamo activo del árbol
        // propio previo con deadline y verificación (solo árbol propio, nunca
        // otra instancia ni imagen global; `Ok` graceful sin reintento intacto).
        let (code, actual) = run_json(&[
            "--json",
            "daemon",
            "start",
            "--auto-restart",
            "--max-retries",
            "1",
        ]);
        assert_eq!(code, 0, "daemon start --auto-restart debe salir 0");
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        // Desde detenido: fresco con `started` y PID vivo a nivel SO.
        assert_eq!(
            actual["status"],
            Value::String("started".to_string()),
            "desde detenido el start debe partir de cero (started): {}",
            actual
        );
        let pid = leer_pid_daemon();
        assert!(
            pid.map(avi_daemon::pid_vivo).unwrap_or(false),
            "el daemon recién arrancado debe estar vivo a nivel SO (pid {:?})",
            pid
        );
        esperar_estado_daemon("running", 50);
        // Stop no debe reintentar (graceful): ausencia observada por la fixture
        // más cero huérfanos a nivel SO.
        shutdown_session_daemon();
        let (_, actual2) = run_json(&["--json", "daemon", "status"]);
        assert_eq!(
            actual2["daemon"],
            Value::String("stopped".to_string()),
            "tras stop no debe reintentar"
        );
        hito_fin("tts::daemon_start_con_auto_restart");
    }

    /// Prueba pesada de limpieza de H-01 (T6): cero huérfanos tras aborto
    /// simulado. Fase 1 (caída del padre: pidfile borrado con daemon vivo) →
    /// `start` reclama el árbol (payload `started`, PID previo muerto). Fase 2
    /// (timeout sin graceful: árbol matado sin POST /shutdown, pista rancia) →
    /// `start` parte de cero con `started`. Cierra con cero huérfanos
    /// verificados a nivel SO. H-07/clonado fuera de alcance: si la raíz roja
    /// del baseline interfiere, se documenta sin arreglarla.
    #[test]
    fn h01_aborto_simulado_reclama_y_no_deja_huerfanos() {
        hito_inicio_pesado("tts::h01_aborto_simulado_reclama_y_no_deja_huerfanos");
        let _guard = bloquear_estado();
        // D-03: reaper en todo `panic!` fuera de polls.
        // D-01 (tensado CI Unix): tras cada reclamo se exige además 8766
        // cerrado cuando el PID previo murió; en Windows local ese verde se
        // declara no probatorio de D-01 (runtime Unix diferido a CI).
        // D-02: la ventana spawn→write ya no ciega al handler (PID en memoria).
        let _reaper = armar_reaper("h01_aborto_simulado");
        // Skip sin efectos: no tocar el ciclo si no hay provisión.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS provisionado para aborto simulado");
            hito_fin("tts::h01_aborto_simulado_reclama_y_no_deja_huerfanos (skip sin provisión)");
            return;
        }
        // Precondición: detenido con cero huérfanos verificados.
        shutdown_session_daemon();
        // Fase 1 — caída del padre: daemon vivo sin pidfile (el dueño anterior
        // murió sin limpiar). El próximo `start` debe reclamar, no adherirse.
        ensure_session_daemon();
        let pid_a = leer_pid_daemon().expect("tras ensure debe haber pidfile");
        assert!(
            avi_daemon::pid_vivo(pid_a),
            "el daemon de sesión debe estar vivo (pid {})",
            pid_a
        );
        std::fs::remove_file(avi_store::data_dir().join("daemon.pid"))
            .expect("la caída simulada debe poder borrar el pidfile");
        hito(&format!(
            "aborto simulado (fase 1): pidfile borrado con daemon vivo (pid {})",
            pid_a
        ));
        let (code, actual) = run_json(&["--json", "daemon", "start"]);
        assert_eq!(
            code, 0,
            "el start tras caída del padre debe reclamar y salir 0: {}",
            actual
        );
        assert_eq!(
            actual["status"],
            Value::String("started".to_string()),
            "tras caída el start debe reclamar (started), no adherirse: {}",
            actual
        );
        esperar_estado_daemon("running", 50);
        assert!(
            !avi_daemon::pid_vivo(pid_a),
            "el reclamo debe haber matado el árbol residual (pid {} sigue vivo)",
            pid_a
        );
        let pid_b = leer_pid_daemon().expect("tras reclamo debe haber pidfile fresco");
        assert!(
            avi_daemon::pid_vivo(pid_b),
            "el daemon reclamado debe estar vivo (pid {})",
            pid_b
        );
        hito("aborto simulado (fase 1): reclamo ok, residual muerto y fresco vivo");
        // Fase 2 — timeout/aborto sin graceful: se mata el árbol sin POST
        // /shutdown (la pista queda rancia a propósito). El próximo `start`
        // parte de cero con `started`.
        avi_daemon::matar_arbol_por_pid(pid_b);
        avi_daemon::esperar_muerte_pid(pid_b, std::time::Duration::from_secs(8));
        // D-01 (tensado CI Unix, sin simular Unix en local): tras matar el
        // árbol sin graceful, el 8766 debe estar cerrado antes del rearranque.
        #[cfg(unix)]
        assert!(
            !puerto_abierto(8766),
            "tras matar el árbol sin graceful el puerto 8766 debe estar cerrado (D-01)"
        );
        hito(&format!(
            "aborto simulado (fase 2): árbol matado sin graceful (pid {})",
            pid_b
        ));
        let (code2, actual2) = run_json(&["--json", "daemon", "start"]);
        assert_eq!(
            code2, 0,
            "el start tras aborto sin graceful debe salir 0: {}",
            actual2
        );
        assert_eq!(
            actual2["status"],
            Value::String("started".to_string()),
            "tras aborto el start debe partir de cero (started): {}",
            actual2
        );
        esperar_estado_daemon("running", 50);
        // Cierre: cero huérfanos verificados a nivel SO.
        shutdown_session_daemon();
        hito_fin("tts::h01_aborto_simulado_reclama_y_no_deja_huerfanos");
    }

    #[test]
    #[allow(unreachable_code)]
    fn translate_con_daemon_delega() {
        hito_inicio_pesado("tts::translate_con_daemon_delega");
        let _guard = bloquear_estado();
        let _tts = lock_tts();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = armar_reaper("translate_con_daemon_delega");
        #[cfg(not(feature = "native-translation"))]
        {
            eprintln!("[translate] skip: sin feature native-translation");
            hito_fin("tts::translate_con_daemon_delega (skip sin feature)");
            return;
        }
        #[cfg(feature = "native-translation")]
        if !ct2_model_disponible() {
            eprintln!("[translate] skip: sin modelo CT2 es→en");
            hito_fin("tts::translate_con_daemon_delega (skip sin CT2)");
            return;
        }
        // Skip sin efectos antes de tocar el ciclo.
        if !tts_modelo_registrado() {
            eprintln!("[daemon] skip: sin modelo TTS para daemon warm");
            hito_fin("tts::translate_con_daemon_delega (skip sin provisión)");
            return;
        }
        // Daemon caliente de la sesión (revalidación con reclamo; sin ciclo propio ni sleeps).
        ensure_session_daemon();
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "translate",
            "--text",
            "Hola",
            "--from",
            "es",
            "--to",
            "en",
        ]);
        assert_eq!(code, 0, "translate --daemon debe delegar con exit 0");
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert!(actual.get("translated").is_some());
        let expected = fixture("cli_translate_daemon.json");
        assert_eq!(actual["source"], expected["source"] );
        // Sin apagado: la sesión es dueña del ciclo (un solo apagado por corrida).
        hito_fin("tts::translate_con_daemon_delega");
    }

    #[test]
    fn translate_force_daemon_sin_daemon_exit5() {
        hito_inicio_pesado("tts::translate_force_daemon_sin_daemon_exit5");
        let _guard = bloquear_estado();
        // Aislamiento total: la ausencia debe observarse sin carreras con
        // usuarios del daemon (serie de inferencia + ciclo de sesión).
        let _tts = lock_tts();
        // Partición previa: fixture detenida con ausencia observada a nivel SO
        // (sin sleeps). No se rearranca: el próximo `ensure_session_daemon` la
        // repara bajo demanda, así que el orden de ejecución no importa.
        shutdown_session_daemon();
        // La ausencia es real a nivel SO (no solo HTTP): con matar-y-rearrancar
        // el `start` solo ocurre explícito, nunca implícito en delegación, así
        // que el exit 5 sigue observable.
        assert!(
            !puerto_abierto(8765),
            "sin daemon el puerto 8765 debe estar cerrado a nivel SO"
        );
        assert!(
            leer_pid_daemon().map(avi_daemon::pid_vivo).unwrap_or(false) == false,
            "sin daemon no debe haber PID vivo en la pista"
        );
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "translate",
            "--text",
            "Hola",
            "--from",
            "es",
            "--to",
            "en",
        ]);
        assert_eq!(code, 5, "translate --daemon sin daemon debe salir 5");
        assert_eq!(actual["reason"], Value::String("daemon_unreachable".to_string()));
        hito_fin("tts::translate_force_daemon_sin_daemon_exit5");
    }

    #[test]
    fn clone_con_daemon_delega() {
        hito_inicio_pesado("tts::clone_con_daemon_delega");
        let _guard = bloquear_estado();
        let _tts = lock_tts();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = armar_reaper("clone_con_daemon_delega");
        if !tts_clone_provisioned() {
            eprintln!("[tts] skip: clonado exige Base");
            hito_fin("tts::clone_con_daemon_delega (skip sin Base)");
            return;
        }
        // Daemon caliente de la sesión (revalidación con reclamo; sin ciclo propio ni sleeps).
        ensure_session_daemon();
        let name = etiqueta_unica("clon_daemon");
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "voice",
            "clone",
            "--name",
            &name,
            "--speech-reference",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        ]);
        assert_eq!(code, 0, "voice clone --daemon debe delegar con exit 0");
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        assert_eq!(actual["name"], Value::String(name.clone()));
        let _ = avi_store::VoiceStore::new().remove(&name);
        // Sin apagado: la sesión es dueña del ciclo (un solo apagado por corrida).
        hito_fin("tts::clone_con_daemon_delega");
    }

    #[cfg(feature = "native-stt")]
    #[test]
    fn dub_daemon_passthrough() {
        hito_inicio_dub("tts::dub_daemon_passthrough");
        let _guard = bloquear_estado();
        let _tts = lock_tts();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = armar_reaper("dub_daemon_passthrough");
        if !tts_provisioned() || !parakeet_model_disponible() || !hay_dispositivo_audio() {
            eprintln!("[dub] skip: sin modelos/audio");
            hito_fin("tts::dub_daemon_passthrough (skip sin modelos/audio)");
            return;
        }
        // Daemon caliente de la sesión (revalidación con reclamo; sin ciclo propio ni sleeps).
        ensure_session_daemon();
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "speech",
            "dub",
            "--audio",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
            "--source-language",
            "es-latam",
            "--target-language",
            "es-latam",
        ]);
        assert_eq!(code, 0, "dub passthrough --daemon debe salir 0");
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        // Verificación sobre el archivo recibido (Tarea 4): WAV válido más
        // texto no vacío. Sin gate WER: este test no lo tenía y añadir un
        // umbral numérico sobre inferencia sin poder ejecutar la pesada sería
        // endurecer a ciegas; el gate WER vive en los testigos directos.
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV del daemon debe estar persistido"
        );
        wav_valido_24k(audio_path);
        let texto = actual["text"].as_str().expect("text debe existir");
        assert!(!texto.is_empty(), "`text` no debe estar vacío");
        // Sin apagado: la sesión es dueña del ciclo (un solo apagado por corrida).
        hito_fin("tts::dub_daemon_passthrough");
    }

    #[cfg(feature = "native-stt")]
    #[test]
    #[allow(unreachable_code)]
    fn dub_daemon_con_traduccion() {
        hito_inicio_dub("tts::dub_daemon_con_traduccion");
        let _guard = bloquear_estado();
        let _tts = lock_tts();
        // D-03: todo `panic!`/`assert!` fuera de polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = armar_reaper("dub_daemon_con_traduccion");
        if !tts_provisioned() || !parakeet_model_disponible() || !hay_dispositivo_audio() {
            eprintln!("[dub] skip: sin modelos/audio");
            hito_fin("tts::dub_daemon_con_traduccion (skip sin modelos/audio)");
            return;
        }
        #[cfg(not(feature = "native-translation"))]
        {
            eprintln!("[dub] skip: sin native-translation");
            hito_fin("tts::dub_daemon_con_traduccion (skip sin feature)");
            return;
        }
        #[cfg(feature = "native-translation")]
        if !ct2_model_disponible() {
            eprintln!("[translate] skip: sin CT2");
            hito_fin("tts::dub_daemon_con_traduccion (skip sin CT2)");
            return;
        }
        // Daemon caliente de la sesión (revalidación con reclamo; sin ciclo propio ni sleeps).
        ensure_session_daemon();
        let (code, actual) = run_json(&[
            "--json",
            "--daemon",
            "speech",
            "dub",
            "--audio",
            "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
            "--source-language",
            "es-latam",
            "--target-language",
            "en",
        ]);
        assert_eq!(code, 0, "dub con traducción --daemon debe salir 0");
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        assert_eq!(actual["schema_version"], Value::String("3".to_string()));
        // Verificación sobre el archivo recibido (Tarea 4): WAV válido más
        // texto traducido no vacío (el daemon devuelve en `text` el final
        // traducido; `src/main.rs:3051-3055`). Sin gate WER por el mismo
        // motivo que `dub_daemon_passthrough`: no endurecer a ciegas.
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV del daemon debe estar persistido"
        );
        wav_valido_24k(audio_path);
        let texto = actual["text"].as_str().expect("text debe existir");
        assert!(!texto.is_empty(), "`text` no debe estar vacío");
        // Sin apagado: la sesión es dueña del ciclo (un solo apagado por corrida).
        hito_fin("tts::dub_daemon_con_traduccion");
    }
}

/// Ejecuta el binario con `args` capturando stdout como texto plano y
/// devolviendo (código de salida, stdout). Para aserciones sobre `--help`.
fn run_text(args: &[&str]) -> (i32, String) {
    let output = Command::new(BIN)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    (code, stdout)
}

/// Ayuda de `synthesize`/`say`/`dub`: expone los flags de idioma y
/// temperatura y ya no ofrece los parámetros sin efecto.
#[test]
fn speech_help_expone_idiomas_y_temperatura() {
    for sub in ["synthesize", "say", "dub"] {
        let (code, help) = run_text(&["speech", sub, "--help"]);
        assert_eq!(code, 0, "help de {} debe salir 0", sub);
        for flag in ["--source-language", "--target-language", "--temperature"] {
            assert!(
                help.contains(flag),
                "help de {} debe documentar {}",
                sub,
                flag
            );
        }
        for flag in ["--compute-backend", "--exaggeration", "--cfg-weight"] {
            assert!(
                !help.contains(flag),
                "help de {} no debe ofrecer {}",
                sub,
                flag
            );
        }
    }
    let (_, dub_help) = run_text(&["speech", "dub", "--help"]);
    assert!(
        !dub_help.contains("--from") && !dub_help.contains("--to "),
        "help de dub no debe ofrecer --from/--to"
    );
}

/// Temperatura fuera de rango en `say`/`synthesize`/`dub`: exit 2 con
/// `usage_error`, antes de cualquier trabajo.
#[test]
fn speech_temperatura_invalida_es_exit_2() {
    for args in [
        vec!["speech", "say", "--text", "Hola", "--temperature", "0"],
        vec!["speech", "say", "--text", "Hola", "--temperature", "2.5"],
        vec![
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--label",
            "x",
            "--temperature",
            "0",
        ],
        vec!["speech", "dub", "--temperature", "0"],
    ] {
        let output = Command::new(BIN)
            .args(&args)
            .arg("--json")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("el binario debe ejecutarse");
        let code = output.status.code().expect("el proceso debe terminar");
        assert_eq!(code, 2, "temperatura inválida debe salir 2: {:?}", args);
    }
}

/// `dub` sin `--source-language`: el parser lo exige → exit 2.
#[test]
fn speech_dub_sin_origen_es_exit_2() {
    let output = Command::new(BIN)
        .args(["speech", "dub", "--audio", "no-existe.wav"])
        .output()
        .expect("el binario debe ejecutarse");
    assert_eq!(
        output.status.code().expect("el proceso debe terminar"),
        2,
        "dub sin --source-language debe salir 2"
    );
}

/// Detector de drift contrato↔código: el contrato no promete parámetros sin
/// efecto y documenta los flags que el binario expone.
#[test]
fn contrato_speech_coincide_con_help() {
    let contrato = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/CLI/CONTRACT.md"),
    )
    .expect("el contrato debe leerse");
    for flag in ["--compute-backend", "--exaggeration", "--cfg-weight"] {
        assert!(
            !contrato.contains(flag),
            "el contrato no debe prometer {}",
            flag
        );
    }
    assert!(
        contrato.contains("--temperature"),
        "el contrato debe especificar --temperature"
    );
    let (_, help) = run_text(&["speech", "synthesize", "--help"]);
    for flag in ["--source-language", "--target-language", "--temperature"] {
        assert!(
            help.contains(flag),
            "el binario debe exponer lo contratado: {}",
            flag
        );
    }
}

/// Demostración del guard de tiempo F4b (prueba rápida, sin daemons ni minutos):
/// fija un techo diminuto a propósito y exige `panic!` con diagnóstico
/// (último hito + fase exacta). `#[should_panic]` mantiene la suite en verde
/// mientras demuestra que el mecanismo falla en vez de colgarse.
#[test]
#[should_panic(expected = "guardia de tiempo")]
fn hito_guard_expira_con_diagnostico() {
    hito_inicio(
        "hito_guard_expira_con_diagnostico",
        Duration::from_millis(50),
    );
    hito("hito_guard_expira_con_diagnostico: hito previo al guard");
    std::thread::sleep(std::time::Duration::from_millis(120));
    comprobar_guard("fase-demostracion-guard");
}
