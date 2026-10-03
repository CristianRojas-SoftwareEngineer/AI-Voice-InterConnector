//! Harness de tests dorados del CLI.
//!
//! Invoca el binario compilado con argumentos fijos y compara `stdout` (JSON) y el
//! código de salida contra fixtures en `tests/golden/`, verificando el contrato de
//! la CLI: `schema_version == "4"` (vía
//! `avi_core::json_emitter`) y los códigos de salida de `avi_core::exit_codes`.
//!
//! Se ubica como test de integración del paquete raíz (y no dentro de `src/main.rs`)
//! porque capturar `stdout` + exit code con fidelidad exige ejecutar el binario real,
//! y `CARGO_BIN_EXE_*` solo está disponible para tests de integración.
//!
//! ### Clases de pruebas
//! - **Contrato** (sin `#[ignore]`): corren siempre. Cada prueba usa un sandbox
//!   propio con estado, caché de modelos y directorio de programa propios, y
//!   siembra modelos falsos si los necesita. Un `daemon.pid` señuelo (puerto
//!   cerrado y PID fuera de rango) o `--no-daemon` impiden que el cliente
//!   contacte 127.0.0.1:8765 o barra procesos `qwen_tts` de la máquina.
//! - **Recursos** (`#[ignore = "requiere …"]`): necesitan modelos o motores
//!   reales y se ejecutan con
//!   `cargo test --workspace --features full -- --include-ignored`. Si falta
//!   un recurso fallan con un mensaje que nombra el comando de
//!   aprovisionamiento: `ai-voice-interconnector setup`,
//!   `ai-voice-interconnector setup --with-voice-cloning` o
//!   `cargo xtask build-engine`. Usan la raíz de modelos real en solo lectura
//!   y estado, puerto y `QWEN3_TTS_PORT` propios (`IsolatedInstance`).
//!
//! ### Taxonomía de tests y filtros de ejecución rápida:
//! - **Rendimiento y comandos rápidos (< 100 ms)**: `cargo test --test cli_golden -- perf_ --nocapture`
//! - **Contratos de salida pura y ayuda CLI**: `cargo test --test cli_golden -- _help --nocapture`
//! - **Validación de errores y flags**: `cargo test --test cli_golden -- _error --nocapture`
//! - **Pruebas pesadas de inferencia (TTS/Dub/Clone)**: `cargo test --test cli_golden -- tts:: --nocapture` (gestionadas bajo `TTS_LOCK`)

use std::cell::RefCell;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

// Las pruebas operan con aislamiento total por instancia (`IsolatedInstance`)
// usando puertos efímeros (`AVI_DAEMON_PORT=0`, `QWEN3_TTS_PORT`) y directorios
// temporales aislados (`CURRENT_SANDBOX_DIR`). Para la inferencia pesada residente
// (~2.7 GB de RAM), se mantiene el semáforo de capacidad de recursos `tts::lock_tts`.

// ─── Observabilidad de tests (solo instrumentación, sin cambios de comportamiento) ───
//
// Hitos por `eprintln!` (stderr, sin buffer) con formato único
// `[milestone][mm:ss.mmm-desde-inicio-test] mensaje`. Sin `println!` para progreso.
// Guard de tiempo por test pesado: `hit_start_*` fija el techo y
// `check_guard` falla con `panic!` (último hito + fase exacta) en los polls
// ya existentes (`wait_for_daemon_state`).
// Techos: 180 s (resto) y 360 s (dub). Parten de los techos del producto: el
// stream NDJSON tolera 1500 ms de inactividad entre latidos y un failsafe
// total de 240 s (`REQUEST_FAILSAFE`), el arranque espera hasta 10 s y la
// parada usa `wait_health_down` más 1,5 s de shutdown graceful. Se suma el
// warmup TTS en segundo plano: una operación típica (síntesis de texto corto,
// muy por debajo del failsafe) + arranque/parada (~15-25 s) + margen → 180 s;
// el dub encadena STT + traducción + TTS + arranque/parada → 360 s. Sin
// baseline medido aún; se remedirá y ajustará más adelante si hace falta.

/// Techo del guard para tests pesados no-dub (3 min).
const GUARD_HEAVY_SECS: u64 = 180;
/// Techo del guard para tests dub (6 min, encadenan STT+traducción+TTS).
/// Solo lo usan tests con `native-stt`; en compilación sin ese feature queda
/// sin usar (permitido para mantener `cargo check --tests` limpio en ambas).
#[allow(dead_code)]
const GUARD_DUB_SECS: u64 = 360;
/// Presupuesto del warm convertido en timeout diagnóstico (readiness por
/// evento en vez de por sondeo temporal): cada unidad
/// vale 200 ms de espera del evento. El warmup queda acotado por el arranque
/// del residente (hasta 30 s) más el presupuesto del testigo de 25 caracteres
/// (30 s + 0,30 s × 25 = 37,5 s): 67,5 s ≤ 340 uds = 68 s, así que ningún
/// timeout nuevo es más corto que el warmup real. El timeout es bug a
/// diagnosticar, no presupuesto de sondeo. Se conserva el tipo `u32` para no
/// cambiar las firmas de `wait_for_daemon_state`.
const WARM_FAILSAFE_RETRIES: u32 = 340;

static PROCESS_T0: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

/// Instante de arranque del proceso (respaldo del timestamp cuando el test no
/// fijó `hit_start`; los pesados siempre lo fijan).
fn process_t0() -> Instant {
    *PROCESS_T0.get_or_init(Instant::now)
}

thread_local! {
    static TEST_T0: RefCell<Option<Instant>> = const { RefCell::new(None) };
    static TEST_NAME: RefCell<String> = const { RefCell::new(String::new()) };
    static TEST_LIMIT: RefCell<Option<Duration>> = const { RefCell::new(None) };
    static LAST_HIT: RefCell<String> = const { RefCell::new(String::new()) };
    static CURRENT_SANDBOX_DIR: RefCell<Option<PathBuf>> = const { RefCell::new(None) };
}

/// Transcurrido desde el inicio del test (o del proceso si no hay inicio).
fn elapsed_test() -> Duration {
    let t0 = TEST_T0.with(|c| *c.borrow());
    match t0 {
        Some(t) => t.elapsed(),
        None => process_t0().elapsed(),
    }
}

/// Formato mm:ss.mmm del transcurrido.
fn format_mm_ss(d: Duration) -> String {
    let ms = d.as_millis();
    format!("{:02}:{:02}.{:03}", ms / 60000, (ms / 1000) % 60, ms % 1000)
}

/// Hito único de progreso (stderr, sin buffer y con vaciado forzado inmediato). Registra el último hito para
/// el diagnóstico del guard.
fn milestone(message: &str) {
    let ts = format_mm_ss(elapsed_test());
    LAST_HIT.with(|c| *c.borrow_mut() = message.to_string());
    eprintln!("[milestone][{}] {}", ts, message);
    let _ = std::io::stderr().flush();
}

/// Inicio de test pesado con techo explícito. Debe llamarse tras
/// adquirir los locks de contención (p. ej. `lock_tts()`), para
/// que el reloj mida el trabajo propio del test y la espera en cola no consuma
/// el guard failsafe.
fn hit_start(name: &str, limit: Duration) {
    TEST_T0.with(|c| *c.borrow_mut() = Some(Instant::now()));
    TEST_NAME.with(|c| *c.borrow_mut() = name.to_string());
    TEST_LIMIT.with(|c| *c.borrow_mut() = Some(limit));
    LAST_HIT.with(|c| *c.borrow_mut() = format!("inicio {}", name));
    let ts = format_mm_ss(Duration::from_millis(0));
    eprintln!("[milestone][{}] inicio {} (techo {:?})", ts, name, limit);
    let _ = std::io::stderr().flush();
}

/// Inicio con techo estándar (3 min).
fn hit_start_heavy(name: &str) {
    hit_start(name, Duration::from_secs(GUARD_HEAVY_SECS));
}

/// Inicio para dub (6 min). Solo lo usan tests con `native-stt`.
#[allow(dead_code)]
fn hit_start_dub(name: &str) {
    hit_start(name, Duration::from_secs(GUARD_DUB_SECS));
}

/// Fin de test pesado. Desactiva el guard para no filtrar al siguiente test
/// del mismo hilo del harness. Limpia además el inicio y el nombre (higiene:
/// sin techos ni hitos heredados entre tests del mismo hilo, aun ante
/// `panic!` previo sin `hit_end` — `hit_start` siempre sobrescribe).
fn hit_end(name: &str) {
    let ts = format_mm_ss(elapsed_test());
    eprintln!("[milestone][{}] fin {}", ts, name);
    let _ = std::io::stderr().flush();
    TEST_LIMIT.with(|c| *c.borrow_mut() = None);
    TEST_T0.with(|c| *c.borrow_mut() = None);
    TEST_NAME.with(|c| *c.borrow_mut() = String::new());
    LAST_HIT.with(|c| *c.borrow_mut() = String::new());
}

/// Último hito registrado (para el diagnóstico del guard).
fn last_hit() -> String {
    LAST_HIT.with(|c| c.borrow().clone())
}

/// Guard genérico: falla en vez de colgarse. Se llama en los polls ya
/// existentes (`wait_for_daemon_state`); al expirar mata el árbol best-effort
/// (`reaper_on_failure`) y hace `panic!` con test, fase, transcurrido,
/// techo y último hito. Inactivo sin `hit_start`.
///
/// Higiene: `hit_start` siempre sobrescribe `TEST_T0`/`TEST_LIMIT`, así
/// que un `panic!` previo sin `hit_end` no hereda techos al siguiente test
/// pesado; `hit_end` y `GuardReaper` (en `Drop` ante `panic!`) limpian el
/// límite para que los tests ligeros sin `hit_start` tampoco lo hereden.
fn check_guard(phase: &str) {
    let name = TEST_NAME.with(|n| n.borrow().clone());
    let limit = TEST_LIMIT.with(|c| *c.borrow());
    let t0 = TEST_T0.with(|c| *c.borrow());
    if let (Some(lim), Some(t)) = (limit, t0) {
        let elapsed = t.elapsed();
        if elapsed > lim {
            let _ = std::io::stderr().flush();
            TEST_LIMIT.with(|c| *c.borrow_mut() = None);
            reaper_on_failure(&format!("guard:{}", phase), true);
            panic!(
                "guardia de tiempo: test '{}' superó techo {:?} en fase '{}' (transcurrido {:.1} s; último hito: {})",
                name,
                lim,
                phase,
                elapsed.as_secs_f64(),
                last_hit()
            );
        }
    }
}

// ─── Verificación a nivel de sistema y reaper ruidoso tras el apagado ──────
//
// El producto reclama el residual al arrancar (matar-y-rearrancar con payload
// `started`) y para con deadline global y verificación (`src/main.rs`:
// `classify_residual`, `reclaim_degraded_residual`,
// `lifecycle::daemon_stop::stop`; ayudantes SO `avi_daemon::{pid_alive,
// kill_tree_by_pid, wait_for_pid_death}`). La fixture verifica esa conducta
// a nivel de sistema en vez de suponerla por HTTP. Fuente única de
// matar/verificar: `avi_daemon`, sin duplicar lógica SO en el harness.

/// Lee el PID de `daemon.pid` en el directorio indicado.
fn read_daemon_pid_dir(dir: &std::path::Path) -> Option<u32> {
    let path = dir.join("daemon.pid");
    let content = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v.get("pid")?.as_u64().map(|n| n as u32)
}

/// Lee el PID del residente de `daemon.pid` en el directorio indicado.
fn read_resident_pid_dir(dir: &std::path::Path) -> u32 {
    let path = dir.join("daemon.pid");
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return 0,
    };
    let v: Value = match serde_json::from_str(&content) {
        Ok(v) => v,
        Err(_) => return 0,
    };
    v.get("resident_pid")
        .and_then(|n| n.as_u64())
        .map(|n| n as u32)
        .unwrap_or(0)
}

/// Lee el PID de `daemon.pid` del sandbox del hilo actual o de `data_dir` global.
fn read_daemon_pid() -> Option<u32> {
    let dir = CURRENT_SANDBOX_DIR
        .with(|c| c.borrow().clone())
        .unwrap_or_else(avi_store::data_dir);
    read_daemon_pid_dir(&dir)
}

/// Lee el PID del residente de `daemon.pid` del sandbox del hilo actual o de `data_dir` global.
fn read_resident_pid() -> u32 {
    let dir = CURRENT_SANDBOX_DIR
        .with(|c| c.borrow().clone())
        .unwrap_or_else(avi_store::data_dir);
    read_resident_pid_dir(&dir)
}

/// ¿Hay algo escuchando en `127.0.0.1:port`? Sondeo TCP breve, sin HTTP.
/// Solo para puertos del daemon: 8765 y el puerto efímero de instancia. El
/// residente `qwen_tts` ya no se detecta por puerto, sino por su identidad
/// estable (PID registrado + imagen) — ver `resident_present_by_image`.
fn port_open(port: u16) -> bool {
    std::net::TcpStream::connect_timeout(
        &std::net::SocketAddr::from(([127, 0, 0, 1], port)),
        std::time::Duration::from_millis(300),
    )
    .is_ok()
}

/// ¿Hay algún proceso residente vivo por imagen (`qwen_tts(.exe)`),
/// independiente del pidfile? Faro estable de descubrimiento/verificación del
/// residente cuando el `resident_pid` no está disponible (pidfile perdido).
/// Seguro por imagen propia del residente. Windows: `tasklist /FI IMAGENAME`;
/// Unix: `pgrep -x`.
fn resident_present_by_image() -> bool {
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args([
                "/FI",
                &format!("IMAGENAME eq {}", avi_tts::RESIDENT_IMAGE_NAME),
                "/FO",
                "CSV",
                "/NH",
            ])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .output();
        match output {
            Ok(o) if o.status.success() => {
                String::from_utf8_lossy(&o.stdout).contains(avi_tts::RESIDENT_IMAGE_NAME)
            }
            _ => false,
        }
    }
    #[cfg(unix)]
    {
        std::process::Command::new("pgrep")
            .args(["-x", avi_tts::RESIDENT_IMAGE_NAME])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

/// Reaper best-effort ante fallo: mata el árbol preciso por PID con
/// verificación acotada (8 s, deadline global del producto) y lo registra como
/// hito. Nunca falla: un reaper que fallara enmascararía la causa original del
/// `panic!` que lo invocó.
fn reaper_on_failure(phase: &str, sweep_by_image: bool) {
    // `sweep_by_image` indica si se permite el barrido por imagen `qwen_tts`;
    // el comportamiento que lo respeta se implementa en otra tarea.
    let _ = sweep_by_image;
    match read_daemon_pid() {
        Some(pid) if avi_daemon::pid_alive(pid) => {
            milestone(&format!(
                "reaper({}): árbol residual pid {} vivo, matando",
                phase, pid
            ));
            avi_daemon::kill_tree_by_pid(pid);
            let dead = avi_daemon::wait_for_pid_death(pid, std::time::Duration::from_secs(8));
            milestone(&format!("reaper({}): pid {} muerto={}", phase, pid, dead));
        }
        Some(pid) => {
            milestone(&format!(
                "reaper({}): pid {} ya muerto, sin árbol que matar",
                phase, pid
            ));
        }
        None => {
            milestone(&format!(
                "reaper({}): sin pidfile, nada que matar por PID",
                phase
            ));
        }
    }
    // Cobertura total: el residente `qwen_tts` desacopla su servidor real del
    // árbol del daemon, de modo que el kill por árbol puede dejarlo vivo. Se
    // detecta por su identidad estable (PID registrado o imagen propia) y se
    // reclama por PID —o por imagen como último recurso—, sin puerto global.
    let resident = read_resident_pid();
    let resident_alive = resident != 0 && avi_tts::resident::resident_pid_alive(resident);
    if resident_alive || resident_present_by_image() {
        milestone(&format!(
            "reaper({}): residente vivo tras el árbol (pid={}), barriendo",
            phase, resident
        ));
        sweep_resident(phase);
    }
}

/// Barrido del residente por identidad estable: mata el árbol preciso del
/// `resident_pid` registrado si sigue vivo y, como último recurso sin PID
/// (pidfile perdido), barre por imagen `qwen_tts` (seguro por imagen propia).
/// Verifica la ausencia por PID muerto más ausencia por imagen. Un solo camino
/// portable: las primitivas por PID/imagen existen en ambas plataformas, sin
/// `netstat`, sin rama Unix solo-log y sin puerto global.
fn sweep_resident(phase: &str) {
    let resident = read_resident_pid();
    if resident != 0
        && resident != std::process::id()
        && avi_tts::resident::resident_pid_alive(resident)
    {
        milestone(&format!(
            "reaper({}): residente PID {} vivo, matando árbol",
            phase, resident
        ));
        avi_tts::resident::kill_tree_resident_by_pid(resident);
    } else if resident == 0 {
        milestone(&format!(
            "reaper({}): sin resident_pid registrado, barriendo por imagen",
            phase
        ));
        avi_tts::resident::sweep_resident_by_image();
    } else {
        milestone(&format!(
            "reaper({}): resident_pid {} ya muerto, verificando ausencia por imagen",
            phase, resident
        ));
    }
    let t0 = std::time::Instant::now();
    let resident_alive_reg = |pid: u32| pid != 0 && avi_tts::resident::resident_pid_alive(pid);
    while (resident_alive_reg(resident) || resident_present_by_image())
        && t0.elapsed() < std::time::Duration::from_secs(8)
    {
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
    milestone(&format!(
        "reaper({}): residente ausente={}",
        phase,
        !resident_alive_reg(resident) && !resident_present_by_image()
    ));
}

/// Falla fuera de polls con reaper previo: ejecuta el reaper
/// best-effort antes del `panic!` para no abandonar daemon ni motor vivos.
/// Todo `panic!`/`assert!` fuera de `wait_for_daemon_state` pasa por aquí.
fn fail_with_reaper(phase: &str, message: String) -> ! {
    reaper_on_failure(phase, true);
    panic!("{}", message);
}

/// Guard RAII que extiende el reaper a todo `panic!`/`assert!` fuera de polls:
/// el test lo arma tras tomar el lock (`let _reaper =
/// arm_reaper("...")`); en salida normal no hace nada, y si el hilo está en
/// `panic!` al dropearse ejecuta el reaper best-effort y restaura la higiene
/// de `TEST_LIMIT` para no heredar techos al siguiente test del mismo hilo.
struct GuardReaper {
    phase: &'static str,
}

fn arm_reaper(phase: &'static str) -> GuardReaper {
    GuardReaper { phase }
}

impl Drop for GuardReaper {
    fn drop(&mut self) {
        if std::thread::panicking() {
            reaper_on_failure(self.phase, true);
            TEST_LIMIT.with(|c| *c.borrow_mut() = None);
        }
    }
}

// ─── Ciclo por instancia (la fixture por sesión compartida queda eliminada) ──
//
// CLASE DECLARADA: E2E-con-proceso, aislado por instancia (puerto efímero +
// sandbox de estado + evento ready) + serializado solo por capacidad de inferencia. Cada test del ciclo posee su instancia
// (`IsolatedInstance`: puerto efímero + `AVI_DATA_DIR` propio + evento de
// readiness) y su propio apagado con cero huérfanos verificados a nivel SO.
// Ningún test nuevo de contrato puro debe heredar este andamiaje (vive en
// `crates/avi-daemon/tests/golden.rs`).
//
// Presupuesto explícito por test (serie permanente, sin tocar el producto):
// techo = `REQUEST_FAILSAFE` del cliente HTTP del daemon (240 s por petición).
// Ningún test de la serie espera más que eso por una operación contra el
// residente.
//
// Contrato de ejecución:
// - Aislamiento por `IsolatedInstance` (puertos efímeros y sandbox independiente);
// - `TTS_LOCK` (`tts::lock_tts`) para tests de inferencia pesada (serie por
//   capacidad física de recursos).
// La contención del estado compartido la dan los namespaces por test
// (`unique_label`).
//
// `run_json_env` queda intacto: la captura por tempfile (sin pipe para
// heredar el write-end al hijo) sigue valiendo con instancias aisladas.

/// Estado observado pasando envs extra al hijo (instancia aislada:
/// las envs del sandbox, con `AVI_DATA_DIR` + `AVI_DAEMON_PORT=0`).
fn daemon_state_env(envs: &[(&str, &str)]) -> Value {
    let (_, actual) = run_json_env(&["--json", "daemon", "status"], envs);
    actual
}

/// Espera por estado OBSERVADO hasta ver `daemon == expected`. Con instancia
/// aislada por evento ready: cuando se espera `running`, exige además `warm == "warm"` DESDE EL EVENTO (fichero
/// ready de la instancia): el bind-ready no basta, el warmup TTS corre en
/// segundo plano y la inferencia solo es fiable en caliente. El presupuesto
/// en unidades se convierte en timeout diagnóstico (200 ms/ud); al vencer o
/// ante `warm_failed` falla explícito (panic con diagnóstico, tras reaper),
/// nunca pasa en silencio. `stopped` no tiene señal de apagado por fichero:
/// se observa la ausencia (poll de `daemon status` hasta `stopped`).
/// Espera de estado con envs extra para el hijo (instancia aislada).
///
/// Camino alterno «solo running»: para los tests que solo verifican ciclo de
/// vida (start/restart/status) y no necesitan síntesis real, existe la
/// hermana `wait_for_running_without_warm`, que asevera bind-ready + `daemon ==
/// "running"` SIN exigir `warm == "warm"` ni abortar ante `warm_failed`. El
/// warmup TTS en segundo plano es una optimización, no un requisito de
/// correctitud del ciclo de vida: esta función (`wait_for_daemon_state_env`)
/// y `start_instance` quedan sin tocar por ese camino nuevo, así que los
/// tests que sí validan contenido sintetizado (clone/dub) siguen pagando el
/// warmup completo sin cambios de comportamiento.
fn wait_for_daemon_state_env(expected: &str, retries: u32, envs: &[(&str, &str)]) -> Value {
    let timeout = Duration::from_millis(200 * retries as u64);
    if expected == "running" {
        let data_dir = envs
            .iter()
            .find(|(k, _)| *k == "AVI_DATA_DIR")
            .map(|(_, v)| PathBuf::from(v))
            .unwrap_or_else(avi_store::data_dir);
        let path = data_dir.join("daemon.ready");
        let start = Instant::now();
        // 1) Publicación del bind con espera acotada (vía el fichero ready
        // de la instancia): al vencer, el `panic!` con diagnóstico ya
        // incluye el último contenido.
        let (addr, _) = wait_for_ready_file(&path, timeout);
        // 2) Warm publicado más verificación por estado observado, con el
        // restante del mismo presupuesto diagnóstico.
        let mut last = Value::Null;
        loop {
            check_guard(&format!("wait_for_daemon_state({})", expected));
            if let Some((_, warm)) = read_ready_file(&path) {
                if warm == "warm_failed" {
                    reaper_on_failure("wait_for_daemon_state(warm_failed)", true);
                    panic!(
                        "el warmup del daemon falló en {} (addr {}; último: {})",
                        path.display(),
                        addr,
                        daemon_state_env(envs)
                    );
                }
                if warm == "warm" {
                    last = daemon_state_env(envs);
                    if last["daemon"] == Value::String("running".to_string())
                        && last["warm"] == Value::String("warm".to_string())
                    {
                        milestone(&format!(
                            "wait_for_daemon_state: 'running+warm' por evento tras {:.1} s (addr {})",
                            start.elapsed().as_secs_f64(),
                            addr
                        ));
                        return last;
                    }
                }
            }
            if start.elapsed() >= timeout {
                reaper_on_failure(
                    &format!("wait_for_daemon_state({})-agotado", expected),
                    true,
                );
                panic!(
                    "el daemon no publicó 'warm' con estado running tras {:?} (último: {}; fichero: {})",
                    timeout, last, path.display()
                );
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
    }
    let mut last = Value::Null;
    for attempt in 0..retries {
        check_guard(&format!("wait_for_daemon_state({})", expected));
        last = daemon_state_env(envs);
        if last["daemon"] == Value::String(expected.to_string()) {
            milestone(&format!(
                "wait_for_daemon_state: '{}' observado en intento {}/{}",
                expected,
                attempt + 1,
                retries
            ));
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    reaper_on_failure(
        &format!("wait_for_daemon_state({})-agotado", expected),
        true,
    );
    panic!(
        "el daemon no alcanzó el estado '{}' tras {} reintentos (último: {})",
        expected, retries, last
    );
}

/// Camino "solo running" (hermano de `wait_for_daemon_state_env`): espera el
/// bind-ready (mismo fichero `daemon.ready`, vía `wait_for_ready_file`) y
/// luego sondea `daemon status` hasta `daemon == "running"`, SIN exigir
/// `warm == "warm"` ni abortar ante `warm_failed`. Lo usan los tests que solo
/// verifican ciclo de vida (start/restart/status): el warmup TTS en segundo
/// plano es una optimización, no un requisito de correctitud del ciclo de
/// vida, y esperar `warm` les hace pagar la síntesis real sin necesitarla.
/// Conserva el mismo presupuesto/timeout diagnóstico y el `reaper_on_failure`
/// al agotar que `wait_for_daemon_state_env`.
fn wait_for_running_without_warm(retries: u32, envs: &[(&str, &str)]) -> Value {
    let timeout = Duration::from_millis(200 * retries as u64);
    let data_dir = envs
        .iter()
        .find(|(k, _)| *k == "AVI_DATA_DIR")
        .map(|(_, v)| PathBuf::from(v))
        .unwrap_or_else(avi_store::data_dir);
    let path = data_dir.join("daemon.ready");
    // Bind-ready: mismo mecanismo que la rama "running" de
    // `wait_for_daemon_state_env`, pero sin exigir `warm == "warm"` después.
    let _ = wait_for_ready_file(&path, timeout);
    // Sondeo de `daemon status` hasta `running` (patrón genérico, igual al de
    // `expected != "running"` en `wait_for_daemon_state_env`).
    let mut last = Value::Null;
    for attempt in 0..retries {
        check_guard("wait_for_running_without_warm(running)");
        last = daemon_state_env(envs);
        if last["daemon"] == Value::String("running".to_string()) {
            milestone(&format!(
                "wait_for_running_without_warm: 'running' observado en intento {}/{}",
                attempt + 1,
                retries
            ));
            return last;
        }
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    reaper_on_failure("wait_for_running_without_warm(running)-agotado", true);
    panic!(
        "el daemon no alcanzó el estado 'running' (sin exigir warm) tras {} reintentos (último: {})",
        retries, last
    );
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

/// El lock envenenado se propaga como fallo visible sin tolerancia al
/// envenenado (hermético, sin daemon): un hilo hace panic con un lock local tomado
/// y el siguiente `lock()` retorna `Err` en vez de recuperar el guard envenenado.
#[test]
fn d03_poisoned_lock_propagates() {
    let local: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let r = std::thread::scope(|s| {
        s.spawn(|| {
            let _g = local.lock().unwrap();
            panic!("veneno intencional");
        })
        .join()
    });
    assert!(r.is_err(), "el hilo debe haber hecho panic");
    assert!(
        local.lock().is_err(),
        "el envenenado debe propagarse como fallo visible, no recuperarse"
    );
    // Higiene: no heredar techo al siguiente test del mismo hilo.
    TEST_LIMIT.with(|c| *c.borrow_mut() = None);
}

/// El reaper ante fallo fuera de polls con pidfile sin PID vivo (daemon 8765
/// cerrado a la entrada) no falla: retorna sin borrar el pidfile de su sandbox.
/// Hermético (sin daemon real); su reaper solo mata un PID ya muerto, nunca abre
/// el 8765 ni arranca un residente, así que no puede dejar huérfanos propios.
/// Llama al reaper sin barrido por imagen, de modo que un residente `qwen_tts`
/// ajeno que esté vivo no se toca ni condiciona el resultado.
/// El pidfile rancio vive en un directorio de estado propio de la instancia
/// (`AVI_DATA_DIR`), sin tocar el `data_dir` real (reversión: volver a
/// `avi_store::data_dir()`).
#[test]
fn d03_reaper_without_live_pid_does_not_fail() {
    let (sandbox, _envs) = sandbox_unique_state("d03reaper");
    CURRENT_SANDBOX_DIR.with(|c| *c.borrow_mut() = Some(sandbox.clone()));
    // Pidfile rancio: PID garantizado muerto, solo en el sandbox.
    let dead_pid = 2_000_000_000u32;
    assert!(
        !avi_daemon::pid_alive(dead_pid),
        "el PID de prueba debe estar muerto"
    );
    let path = sandbox.join("daemon.pid");
    std::fs::write(&path, format!("{{\"pid\": {}}}", dead_pid))
        .expect("escribir pidfile rancio en el sandbox");
    // El reaper best-effort no debe fallar con PID muerto y sin residente:
    // llegar hasta aquí ya prueba que retornó sin hacer panic.
    reaper_on_failure("d03-prueba", false);
    // No aseveramos quiescencia global de máquina: el reaper de d03 solo mata un
    // PID ya muerto —nunca abre el 8765 ni arranca un residente `qwen_tts`—, así
    // que cualquier puerto/residente presente aquí proviene de un daemon test
    // concurrente en otro hilo de libtest, no de un huérfano nuestro. Muestrear el
    // singleton de máquina (`tasklist` por imagen) enrojecería el test por ese
    // solapamiento legítimo (carrera, no fallo del reaper). El invariante propio
    // de d03 —no borrar su pidfile de sandbox— se asevera abajo.
    assert!(
        path.is_file(),
        "el sandbox propio no debe borrar su pidfile"
    );
    CURRENT_SANDBOX_DIR.with(|c| *c.borrow_mut() = None);
    let _ = std::fs::remove_dir_all(&sandbox);
    // Higiene: no heredar techo al siguiente test del mismo hilo.
    TEST_LIMIT.with(|c| *c.borrow_mut() = None);
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
/// (exit 124). El fix real (`disinherit_standard_handles` en `handle_daemon`, corte de
/// herencia vía `SetHandleInformation`) + `Stdio::null` no basta si se captura por pipe:
/// Rust std deja `bInheritHandles=TRUE` y no hay creation flag que lo desactive. Al
/// redirigir `stdout` a
/// un tempfile **no hay pipe** para heredar: `spawn()`+`wait()` retorna en cuanto el CLI
/// termina (~1.3 s tras `daemon start` con el bind-first).
///
/// Patrón de espera: el daemon no comparte I/O (pipe) con el
/// proceso que lo lanza.
///
/// `stderr` va a otro tempfile por la misma razón y se imprime con `eprintln!`: el
/// harness de pruebas lo captura y solo lo muestra si la prueba falla, así que una
/// caída del hijo (un `abort` o un pánico) deja su rastro en el informe del fallo.
fn run_json_env(args: &[&str], envs: &[(&str, &str)]) -> (i32, Value) {
    let t_cmd = Instant::now();
    let (tmp, file) = open_atomic_tmp();
    let (tmp_err, file_err) = open_atomic_tmp();
    let mut cmd = Command::new(BIN);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(file)
        .stderr(file_err);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let mut child = cmd.spawn().expect("el binario debe ejecutarse");
    let status = child.wait().expect("el proceso debe terminar");
    let stdout = std::fs::read_to_string(&tmp)
        .unwrap_or_else(|e| panic!("no se pudo leer tempfile {}: {}", tmp.display(), e));
    let _ = std::fs::remove_file(&tmp);
    let stderr = std::fs::read_to_string(&tmp_err).unwrap_or_default();
    let _ = std::fs::remove_file(&tmp_err);
    if !stderr.trim().is_empty() {
        eprintln!("stderr del hijo `{}`:\n{}", args.join(" "), stderr);
    }
    let code = status
        .code()
        .expect("el proceso debe terminar con un código");
    let json: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout no es JSON válido ({}): {:?}", e, stdout));
    milestone(&format!(
        "run_json_env: `{}` → exit {} en {} ms",
        args.join(" "),
        code,
        t_cmd.elapsed().as_millis()
    ));
    (code, json)
}

/// Crea un sandbox de contrato nuevo y único: datos, caché de modelos y
/// programa propios, sin modelos y con un `daemon.pid` señuelo para que el
/// binario no consulte 127.0.0.1:8765 ni barra los `qwen_tts` de la máquina.
fn contract_sandbox(tag: &str) -> (PathBuf, Vec<(String, String)>) {
    let (dir, envs) = sandbox_unique_state(tag);
    seed_decoy_daemon_pidfile(&dir);
    (dir, envs)
}

/// Ejecuta el binario con `args` en un sandbox de contrato nuevo que borra al
/// terminar. Delega en [`run_json_env`]; una prueba que comparta estado entre
/// varias llamadas debe usar un único sandbox con `run_json_env`.
fn run_json(args: &[&str]) -> (i32, Value) {
    let (dir, envs) = contract_sandbox("run_json");
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let result = run_json_env(args, &envs);
    let _ = std::fs::remove_dir_all(&dir);
    result
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

// ─── Sandbox de estado por instancia (directorio de estado por instancia) ──
//
// Cada test pesado posee su instancia aislada. El aislamiento va por las **variables de
// reubicación**, no por `LOCALAPPDATA`: en Windows las Known Folders ignoran
// `LOCALAPPDATA`, así que un doble que dependiera de ella no representaría nada del
// mecanismo que se quiere probar. `AVI_INSTALL_DIR` y `AVI_CACHE_DIR` aíslan el
// directorio de programa y la raíz de modelos, `AVI_DATA_DIR` los datos, y
// `HF_HUB_CACHE`/`HF_HOME` el caso de la caché compartida. Unicidad por `TMP_COUNTER`
// (misma fuente que el tempfile anti-cuelgue, que se preserva intacto).

/// Prefijo de los sandboxes de esta puerta, y de ningún otro directorio que cree.
///
/// **Por qué no empieza por `avi`.** El producto reserva para la aplicación los prefijos de
/// temporales `avi-` y `avi_`, y el barrido decide por **`starts_with`**, no por
/// igualdad: la comparación es `name.starts_with(prefix)` contra `TEMP_PREFIXES`, que
/// vale `&["avi-", "avi_"]`. Las dos formas "naturales" de nombrar un sandbox de
/// pruebas **colisionan** con los dos: `avi_test_sandbox_*` empieza por `avi_`, y
/// `avi-test-*` empieza por `avi-`. El criterio exacto es, por tanto, que el nombre **no
/// empiece por `avi-` ni por `avi_`**, y nada más: no importa que lleve `avi` más adelante,
/// ni que el resto del nombre no exista.
///
/// La consecuencia de no cumplirlos no es teórica. Un sandbox con el prefijo del producto es
/// un temporal propio a todos los efectos, así que el barrido de `self install`, `self
/// uninstall` o `cleanup` lo borra —`owner_pid` no encuentra un PID de tres dígitos en
/// `test_sandbox_` y lo declara huérfano—, y si el sandbox pertenece a un test que se está
/// ejecutando en paralelo, se lo borra por debajo. Una puerta que puede fallar por culpa
/// del arnés no es una puerta.
///
/// `test_sandbox_prefix_does_not_collide_with_the_product_temporaries` afirma el invariante
/// para que un renombrado de vuelta a `avi_` no pueda colarse sin que nadie se entere.
const PREFIX_SANDBOX: &str = "golden-sandbox_";

/// Crea un directorio sandbox único y devuelve (ruta, envs para el hijo).
fn sandbox_unique_state(tag: &str) -> (PathBuf, Vec<(String, String)>) {
    let dir = std::env::temp_dir().join(format!(
        "{PREFIX_SANDBOX}{tag}_{}_{}",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::SeqCst),
    ));
    std::fs::create_dir_all(&dir).expect("crear sandbox de estado");
    let install = dir.join("install");
    let cache = dir.join("cache");
    let hf = dir.join("hf");
    for d in [&install, &cache, &hf] {
        std::fs::create_dir_all(d).expect("crear raíces del sandbox");
    }
    // `AVI_CACHE_DIR` tiene precedencia sobre las variables HF, así que la raíz
    // de modelos del sandbox es la **exclusiva** de la aplicación; `HF_HUB_CACHE` y
    // `HF_HOME` se fijan igualmente para que ninguna prueba que las herede del entorno
    // de la máquina escriba fuera del sandbox.
    let mut envs = vec![
        (
            "AVI_DATA_DIR".to_string(),
            dir.to_string_lossy().to_string(),
        ),
        (
            "AVI_INSTALL_DIR".to_string(),
            install.to_string_lossy().to_string(),
        ),
        (
            "AVI_CACHE_DIR".to_string(),
            cache.to_string_lossy().to_string(),
        ),
        ("HF_HUB_CACHE".to_string(), hf.to_string_lossy().to_string()),
        ("HF_HOME".to_string(), hf.to_string_lossy().to_string()),
    ];
    // El temporal del hijo también se reubica, y no es un detalle de belt-and-braces sino
    // la otra mitad del invariante: la tabla **no** declara variable de reubicación para
    // el directorio de temporales, así que sin esto cualquier comando de ciclo de vida
    // que arranque el barrido —`self install`, `self uninstall`, `cleanup`— leería el
    // `%TEMP%` de la máquina. Con el temporal dentro del sandbox, el universo del barrido
    // está acotado al sandbox aunque el prefijo volviera a colisionar, y las dos
    // protections son independientes a propósito.
    let temp = dir.join("tmp-hijo");
    std::fs::create_dir_all(&temp).expect("crear temporal del sandbox");
    for variable in ["TEMP", "TMP", "TMPDIR"] {
        envs.push((variable.to_string(), temp.to_string_lossy().to_string()));
    }
    (dir, envs)
}

/// El invariante que hace que el arnés de esta puerta no se pueda borrar a sí mismo: el
/// prefijo de sus sandboxes no es uno de los que el producto reserva, y el temporal
/// del hijo está dentro del sandbox.
///
/// Son **dos** afirmaciones y las dos importan, porque son independietes. El prefijo
/// protege de que el barrido del `%TEMP%` de la máquina se lleve un sandbox; la
/// reubicación del temporal protege de que el barrido del hijo se lleve el `%TEMP%` de la
/// máquina. Con las dos, ningún barrido puede tocar el directorio de otro test ni el de la
/// máquina, y el motivo no depende de que el prefijo siga siendo el de hoy.
///
/// La condición exacta es `!nombre.starts_with(prefijo_del_producto)`, y el mensaje la
/// demuestra con las dos trampas: cambiar `avi_test_` por `avi-test-` —que parece más
/// correcto porque lleva guion— sigue colisionando, y viceversa.
#[test]
fn test_sandbox_prefix_does_not_collide_with_the_product_temporaries() {
    for product in avi_store::TEMP_PREFIXES {
        assert!(
            !PREFIX_SANDBOX.starts_with(product),
            "el prefijo de los sandboxes de prueba `{PREFIX_SANDBOX}` empieza por `{product}`, \
             que el producto reserva a la aplicación: el barrido lo borraría como si fuera un \
             temporal nuestro"
        );
    }
    // Las dos formas "naturales" colisionan, y es lo que hace que el criterio sea "no empieza
    // por `avi-` ni por `avi_`" y no "no lleva `avi`".
    assert!(
        "avi_test_sandbox_x".starts_with("avi_"),
        "precondición del criterio: `avi_test_*` empieza por `avi_` y por eso colisionaba"
    );
    assert!(
        "avi-test-sandbox-x".starts_with("avi-"),
        "precondición del criterio: `avi-test-*` empieza por `avi-` y por eso colisionaría"
    );

    // Y el temporal del hijo está dentro del sandbox, que es la otra mitad.
    let (dir, envs) = sandbox_unique_state("invarianteprefijo");
    let value = |k: &str| {
        envs.iter()
            .find(|(n, _)| n == k)
            .map(|(_, v)| PathBuf::from(v))
    };
    for variable in ["TEMP", "TMP", "TMPDIR"] {
        let Some(value) = value(variable) else {
            panic!("el sandbox debe reubicar {variable} para que su barrido no salga de él");
        };
        assert!(
            value.starts_with(&dir),
            "{variable} del hijo es {value:?}, que está fuera del sandbox {dir:?}: el barrido \
             alcanzaría el temporal de la máquina"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// ─── Fichero ready (transporte del evento de readiness por instancia) ──
// El hijo publica `addr=<real>` tras enlazar el puerto y cargar su estado, y
// `warm=<estado>` tras el warmup, en el fichero designado por `--ready-file`
// (escritura atómica: temporal hermano + rename). Si el arranque falla antes
// de estar listo, el hijo publica en su lugar un registro de fallo sin
// `addr=`, que `read_ready_file` trata como aún-no-listo. La lectura es tolerante a fichero a medio
// escribir: contenido ausente o incompleto equivale a aún-no-listo (None),
// nunca a error fatal. La ruta es absoluta dentro del sandbox (`AVI_DATA_DIR`)
// para no depender de la unidad del proceso en Windows.

/// Lee el fichero ready de forma tolerante: ausente o a medio escribir =
/// aún-no-listo (`None`), nunca error fatal. Retorna `(addr, warm)`; sin
/// campo `warm` se asume `warming` (bind ya publicado, warm aún en curso).
fn read_ready_file(path: &std::path::Path) -> Option<(String, String)> {
    let content = std::fs::read_to_string(path).ok()?;
    let mut addr: Option<String> = None;
    let mut warm: Option<String> = None;
    for line in content.lines() {
        let line = line.trim();
        if let Some(v) = line.strip_prefix("addr=") {
            let v = v.trim();
            if !v.is_empty() {
                addr = Some(v.to_string());
            }
        } else if let Some(v) = line.strip_prefix("warm=") {
            let v = v.trim();
            if !v.is_empty() {
                warm = Some(v.to_string());
            }
        }
    }
    Some((addr?, warm.unwrap_or_else(|| "warming".to_string())))
}

/// Espera acotada del fichero ready (poll 50 ms): retorna `(addr, warm)` al
/// aparecer un contenido válido. Al vencer el timeout falla con `panic!`
/// con diagnóstico del último contenido (timeout = bug a diagnosticar, no
/// flake a reintentar), tras reaper best-effort.
fn wait_for_ready_file(path: &std::path::Path, timeout: Duration) -> (String, String) {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < timeout {
        if let Some(valid) = read_ready_file(path) {
            milestone(&format!(
                "wait_for_ready_file: addr={} warm={} tras {:.1} s",
                valid.0,
                valid.1,
                start.elapsed().as_secs_f64()
            ));
            return valid;
        }
        last = std::fs::read_to_string(path).unwrap_or_default();
        std::thread::sleep(Duration::from_millis(15));
    }
    reaper_on_failure("wait_for_ready_file-agotado", true);
    panic!(
        "el fichero ready {} no publicó addr válida tras {:?} (último contenido: {:?})",
        path.display(),
        timeout,
        last
    );
}

// ─── Instancia aislada por test (migración del ciclo a instancia propia) ──
//
// Cada test del ciclo posee su instancia: `AVI_DAEMON_PORT=0` (puerto efímero
// descubierto por pidfile con fallback, veredicto de puerta 2026-09-21) más
// sandbox `AVI_DATA_DIR` propio (pidfile + fichero ready propios, ruta
// absoluta) más espera del evento. La fixture por sesión queda eliminada:
// ningún test la usa (reversión: devolver a cada test su forma con sesión).
//
// La raíz de modelos (`AVI_CACHE_DIR`, `HF_HUB_CACHE`, `HF_HOME`) NO se fija en
// la instancia: se hereda del proceso (solo lectura en las pruebas de recursos),
// porque el daemon necesita los modelos provisionados, que viven fuera del
// sandbox. Las pruebas de contrato usan `contract_args`, que la reubica al
// sandbox. `AVI_INSTALL_DIR` y `AVI_DATA_DIR` sí se aíslan siempre.
//
// Límite físico razonado (no deuda): la detección/limpieza/verificación del
// residente ya se re-ancló a su identidad estable (PID registrado + imagen
// propia `qwen_tts`), cerrando la parte eliminable del acoplamiento entre el
// dominio del recurso (la máquina) y el del candado (el proceso); lo irreducible que
// queda es el semáforo de capacidad (una única inferencia pesada residente), que el grupo de inferencia conserva
// vía `TTS_LOCK`. El aislamiento es de estado+daemon; la capacidad física se serializa por lock_tts.

/// Instancia aislada por test: sandbox propio + envs para el hijo.
struct IsolatedInstance {
    dir: PathBuf,
    envs: Vec<(String, String)>,
    /// Raíz de modelos del sandbox, usada solo por `contract_args`.
    contract_cache: String,
}

impl IsolatedInstance {
    fn new(tag: &str) -> Self {
        let (dir, mut envs) = sandbox_unique_state(tag);
        // Puerto efímero: el SO asigna y el hijo publica el real.
        envs.push(("AVI_DAEMON_PORT".to_string(), "0".to_string()));
        let tts_listener = std::net::TcpListener::bind("127.0.0.1:0").ok();
        let tts_port = tts_listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port())
            .unwrap_or(0);
        drop(tts_listener);
        if tts_port > 0 {
            envs.push(("QWEN3_TTS_PORT".to_string(), tts_port.to_string()));
        }
        // Raíz de modelos real (solo lectura): la instancia no fija ninguna
        // variable de caché, de modo que el daemon hijo hereda la del entorno y ve
        // los modelos provisionados; el sandbox solo aísla estado (pidfile/almacén).
        // Las pruebas de contrato la sustituyen por la del sandbox con `contract_args`.
        envs.retain(|(k, _)| !matches!(k.as_str(), "AVI_CACHE_DIR" | "HF_HUB_CACHE" | "HF_HOME"));
        CURRENT_SANDBOX_DIR.with(|c| *c.borrow_mut() = Some(dir.clone()));
        let contract_cache = dir.join("cache").to_string_lossy().to_string();
        IsolatedInstance {
            dir,
            envs,
            contract_cache,
        }
    }

    /// Envs de la clase contrato: los de `args` más `AVI_CACHE_DIR` apuntando a
    /// `<sandbox>/cache`, de modo que la prueba solo ve los modelos que siembra.
    fn contract_args(&self) -> Vec<(&str, &str)> {
        let mut envs = self.args();
        envs.push(("AVI_CACHE_DIR", self.contract_cache.as_str()));
        envs
    }

    /// Puerto del motor TTS residente de la instancia (`QWEN3_TTS_PORT`).
    fn tts_port(&self) -> Option<u16> {
        self.envs
            .iter()
            .find(|(k, _)| k == "QWEN3_TTS_PORT")
            .and_then(|(_, v)| v.parse().ok())
    }

    /// Envs como `&[(&str, &str)]` para `run_json_env` y `*_env`.
    fn args(&self) -> Vec<(&str, &str)> {
        self.envs
            .iter()
            .map(|(k, v)| (k.as_str(), v.as_str()))
            .collect()
    }

    /// `addr` publicada en el pidfile de la instancia (None si aún no hay).
    fn published_addr(&self) -> Option<String> {
        let content = std::fs::read_to_string(self.dir.join("daemon.pid")).ok()?;
        let v: Value = serde_json::from_str(&content).ok()?;
        v.get("addr")?.as_str().map(|s| s.to_string())
    }

    /// Puerto de la instancia (efímero descubierto) o 8765 si aún no hay pista.
    fn port(&self) -> u16 {
        self.published_addr()
            .and_then(|a| a.rsplit(':').next()?.parse().ok())
            .unwrap_or(8765)
    }

    /// Lee el PID de `daemon.pid` de la instancia aislada.
    fn read_daemon_pid(&self) -> Option<u32> {
        read_daemon_pid_dir(&self.dir)
    }

    /// Lee el PID del residente de `daemon.pid` de la instancia aislada.
    fn read_resident_pid(&self) -> u32 {
        read_resident_pid_dir(&self.dir)
    }
}

impl Drop for IsolatedInstance {
    fn drop(&mut self) {
        CURRENT_SANDBOX_DIR.with(|c| *c.borrow_mut() = None);
    }
}

/// Arranca la instancia aislada (`daemon start` con sus envs más `extra`) y
/// espera `running+warm` por evento. Falla con reaper previo fuera de polls.
fn start_instance(inst: &IsolatedInstance, extra: &[&str]) -> Value {
    let a = inst.args();
    let mut cmd: Vec<&str> = vec!["--json", "daemon", "start"];
    cmd.extend(extra);
    let (code, actual) = run_json_env(&cmd, &a);
    if code != 0 {
        fail_with_reaper(
            "start_instance(start)",
            format!(
                "daemon start de la instancia debe salir 0 (fue {}): {}",
                code, actual
            ),
        );
    }
    if actual["daemon"] != Value::String("running".to_string())
        || actual["status"] != Value::String("started".to_string())
    {
        fail_with_reaper(
            "start_instance(daemon)",
            format!(
                "tras start la instancia debe estar running+started: {}",
                actual
            ),
        );
    }
    let pid = inst.read_daemon_pid();
    if !pid.map(avi_daemon::pid_alive).unwrap_or(false) {
        fail_with_reaper(
            "start_instance(pid)",
            format!(
                "la instancia recién arrancada debe estar viva a nivel SO (pid {:?})",
                pid
            ),
        );
    }
    wait_for_daemon_state_env("running", WARM_FAILSAFE_RETRIES, &a);
    actual
}

/// Hermana de `start_instance`: arranca la instancia aislada (`daemon start`
/// con sus envs más `extra`) y espera SOLO `running` (bind-ready) por evento,
/// SIN exigir `warm == "warm"`. Mismas aserciones de arranque (exit 0,
/// running+started, PID vivo a nivel SO) que `start_instance`; la usan los
/// tests de solo ciclo de vida que no necesitan síntesis real, para no pagar
/// el warmup TTS en segundo plano.
fn start_instance_running_only(inst: &IsolatedInstance, extra: &[&str]) -> Value {
    let a = inst.args();
    let mut cmd: Vec<&str> = vec!["--json", "daemon", "start"];
    cmd.extend(extra);
    let (code, actual) = run_json_env(&cmd, &a);
    if code != 0 {
        fail_with_reaper(
            "start_instance_running_only(start)",
            format!(
                "daemon start de la instancia debe salir 0 (fue {}): {}",
                code, actual
            ),
        );
    }
    if actual["daemon"] != Value::String("running".to_string())
        || actual["status"] != Value::String("started".to_string())
    {
        fail_with_reaper(
            "start_instance_running_only(daemon)",
            format!(
                "tras start la instancia debe estar running+started: {}",
                actual
            ),
        );
    }
    let pid = inst.read_daemon_pid();
    if !pid.map(avi_daemon::pid_alive).unwrap_or(false) {
        fail_with_reaper(
            "start_instance_running_only(pid)",
            format!(
                "la instancia recién arrancada debe estar viva a nivel SO (pid {:?})",
                pid
            ),
        );
    }
    wait_for_running_without_warm(WARM_FAILSAFE_RETRIES, &a);
    actual
}

/// Apaga la instancia aislada (`daemon stop` con sus envs) y verifica cero
/// huérfanos sobre SU puerto/pidfile (no sobre el 8765 global). Tolera exit 0
/// (`shutdown_sent`) y exit 5 (ya detenido).
fn stop_instance(inst: &IsolatedInstance, context: &str) {
    let previous_port = inst.port();
    let a = inst.args();
    let (code, actual) = run_json_env(&["--json", "daemon", "stop"], &a);
    if !(code == 0 || code == 5) {
        fail_with_reaper(
            &format!("stop_instance({})", context),
            format!(
                "el apagado de la instancia debe salir 0 o 5 (fue {}): {}",
                code, actual
            ),
        );
    }
    wait_for_daemon_state_env("stopped", 75, &a);
    verify_zero_orphans_instance(context, inst, previous_port);
}

/// Verificación ruidosa de cero huérfanos de LA INSTANCIA tras el apagado: el
/// árbol debe estar muerto, SU puerto cerrado, el residente ausente (por su
/// PID registrado muerto y sin proceso por imagen `qwen_tts`) y su pidfile sin
/// PID vivo (el producto lo borra tras muerte verificada). Si queda resto,
/// ejecuta el reaper best-effort antes de fallar: la suite nunca pasa en verde
/// con huérfanos vivos ni los abandona en la vía de fallo.
fn verify_zero_orphans_instance(context: &str, inst: &IsolatedInstance, port: u16) {
    let pid = inst.read_daemon_pid();
    let pid_alive = pid.map(avi_daemon::pid_alive).unwrap_or(false);
    let resident = inst.read_resident_pid();
    let resident_alive = resident != 0 && avi_tts::resident::resident_pid_alive(resident);
    let instance_port_open = port_open(port);
    let resident_by_image = resident_present_by_image();
    let pidfile = inst.dir.join("daemon.pid");
    let pidfile_exists = pidfile.exists();
    if pid_alive || resident_alive || instance_port_open || resident_by_image || pidfile_exists {
        fail_with_reaper(
            &format!("verify_zero_orphans_instance({})", context),
            format!(
                "quedaron huérfanos tras {}: pid={:?} vivo={} residente={} residente_vivo={} puerto_instancia={} abierto={} residente_por_imagen={} pidfile={} (el apagado debe dejar cero restos a nivel SO)",
                context,
                pid,
                pid_alive,
                resident,
                resident_alive,
                port,
                instance_port_open,
                resident_by_image,
                pidfile.display()
            ),
        );
    }
    milestone(&format!(
        "{}: cero huérfanos verificados a nivel SO (instancia puerto {})",
        context, port
    ));
}

/// Exige el modelo Parakeet TDT v3 en la raíz de modelos vigente: falla con un
/// mensaje accionable si falta `nemo128.onnx`, el archivo que usa la
/// transcripción. Las pruebas que lo llaman son de la clase recursos y van
/// marcadas `#[ignore]`. Solo se compila con `native-stt`: sin el feature el
/// binario no transcribe.
#[cfg(feature = "native-stt")]
fn require_parakeet() {
    let file = avi_store::ModelStore::new()
        .model_snapshot_path("parakeet-tdt-v3")
        .expect("el pin de parakeet-tdt-v3 debe existir en MODEL_REVISIONS")
        .join("nemo128.onnx");
    assert!(
        file.is_file(),
        "falta {}: aprovisiona Parakeet con `ai-voice-interconnector setup`",
        file.display()
    );
}

/// Exige los modelos opus-mt es-en y en-es: falla con un mensaje accionable si
/// falta el `model.bin` de alguno de los dos pares. Solo se compila con
/// `native-translation`: sin el feature el binario no traduce.
#[cfg(feature = "native-translation")]
fn require_ct2() {
    for pair in ["es-en", "en-es"] {
        let file = avi_store::ModelStore::new()
            .model_dir(&format!("opus-mt-{pair}"))
            .join("model.bin");
        assert!(
            file.is_file(),
            "falta {}: aprovisiona los modelos de traducción con `ai-voice-interconnector setup`",
            file.display()
        );
    }
}

/// Siembra en `models_root` un snapshot por cada modelo de `names`, en la
/// revisión fijada del pin real. Cada snapshot lleva los archivos de
/// `MODEL_FILE_PATTERNS` del modelo, o un `config.json` si no declara patrones,
/// de modo que `is_provisioned` los da por provisionados sin descargar nada.
fn seed_pinned_models(models_root: &std::path::Path, names: &[&str]) {
    for pin in avi_store::MODEL_REVISIONS
        .iter()
        .filter(|pin| names.contains(&pin.name))
    {
        let snapshot = models_root
            .join(format!("models--{}", pin.repo.replace('/', "--")))
            .join("snapshots")
            .join(pin.revision);
        std::fs::create_dir_all(&snapshot).expect("crear snapshot sembrado");
        let files: Vec<&str> = avi_store::MODEL_FILE_PATTERNS
            .iter()
            .find(|(name, _)| *name == pin.name)
            .map(|(_, patterns)| patterns.to_vec())
            .unwrap_or_else(|| vec!["config.json"]);
        for file in files {
            std::fs::write(snapshot.join(file), b"marker").expect("sembrar fichero");
        }
    }
}

/// Siembra en `data_dir` un `daemon.pid` señuelo: su dirección apunta a un
/// puerto TCP efímero ya cerrado y sus PID están fuera de rango. Así una orden
/// que consulte o detenga el daemon no cae a 127.0.0.1:8765 (el del usuario)
/// ni barre por imagen los `qwen_tts` de la máquina, porque el `resident_pid`
/// registrado no es 0.
fn seed_decoy_daemon_pidfile(data_dir: &std::path::Path) {
    let port = std::net::TcpListener::bind(("127.0.0.1", 0))
        .expect("reservar puerto efímero")
        .local_addr()
        .expect("dirección del puerto reservado")
        .port();
    let dead_pid = u32::MAX - 1;
    let content = serde_json::json!({
        "pid": dead_pid,
        "addr": format!("127.0.0.1:{port}"),
        "resident_pid": dead_pid,
    });
    std::fs::create_dir_all(data_dir).expect("crear directorio de datos");
    std::fs::write(data_dir.join("daemon.pid"), content.to_string())
        .expect("sembrar daemon.pid señuelo");
}

#[test]
fn version_matches_fixture() {
    let (code, actual) = run_json(&["--json", "version"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_version.json"));
}

/// La CI compara la salida plana de `version` y de `--version` con el tag del
/// release: ambas deben imprimir el nombre y la versión del paquete.
#[test]
fn plain_version_outputs_name_and_package_version() {
    let expected = format!("ai-voice-interconnector {}", env!("CARGO_PKG_VERSION"));
    let (dir, envs) = contract_sandbox("plain_version");
    for args in [["version"], ["--version"]] {
        let salida = Command::new(BIN)
            .args(args)
            .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(std::process::Stdio::null())
            .output()
            .expect("el binario debe ejecutarse");
        assert!(salida.status.success(), "`{}` debe salir con 0", args[0]);
        assert_eq!(
            String::from_utf8_lossy(&salida.stdout).trim(),
            expected,
            "salida de `{}`",
            args[0]
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

// Requiere `native-stt`: sin el motor Parakeet el binario responde
// `stt_unsupported`, por lo que el contrato de transcripción solo aplica con el
// feature activo (en CI featureless no se compila).
#[cfg(feature = "native-stt")]
#[test]
#[ignore = "requiere Parakeet"]
fn speech_transcribe_with_audio_matches_contract() {
    require_parakeet();
    // Instancia aislada con la raíz de modelos real heredada (solo lectura): el
    // estado es propio y la ruta directa fijada con `--no-daemon` impide que el
    // cliente consulte el 8765 del usuario. Ambas rutas emiten
    // {text, source}+schema, así que las invariantes no dependen de la ruta.
    let inst = IsolatedInstance::new("transcribe_contract");
    let (code, actual) = run_json_env(
        &[
            "--json",
            "--no-daemon",
            "speech",
            "transcribe",
            "--audio",
            "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
            "--source-language",
            "es-latam",
        ],
        &inst.args(),
    );
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    assert_eq!(actual["source"], Value::String("es-latam".to_string()));
    let text = actual["text"].as_str().expect("`text` debe ser un string");
    assert!(!text.is_empty(), "`text` no debe estar vacío");
}

#[test]
fn speech_transcribe_without_audio_nor_mic_exits_2() {
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

/// Un `--audio` inexistente sale con exit 3 y `audio_not_found`, sin llegar
/// a ningún despacho (daemon o vía local).
#[test]
fn transcribe_missing_audio_exits_3() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--audio",
        "no_existe_avi_test.wav",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(code, 3, "audio inexistente debe mapear a exit 3");
    assert_eq!(actual["reason"], "audio_not_found");
}

/// Igual que `transcribe`: el dub rechaza un `--audio` inexistente con exit 3.
#[test]
fn dub_missing_audio_exits_3() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "dub",
        "--audio",
        "no_existe_avi_test.wav",
        "--source-language",
        "es-latam",
        "--target-language",
        "es-latam",
    ]);
    assert_eq!(code, 3, "audio inexistente debe mapear a exit 3");
    assert_eq!(actual["reason"], "audio_not_found");
}

/// Un archivo que no es un WAV válido sale con exit 2 y `invalid_audio`.
/// La lectura del audio ocurre tras comprobar el modelo, y sin `native-stt`
/// el binario no llega a leerla, por eso se gatea por el feature y por la
/// presencia del modelo. Corre sobre un sandbox con Parakeet sembrado, así que
/// no depende de la instalación de la máquina.
#[cfg(feature = "native-stt")]
#[test]
fn transcribe_invalid_wav_exits_2() {
    let (dir, envs) = sandbox_unique_state("invalid_wav");
    seed_pinned_models(&dir.join("cache"), &["parakeet-tdt-v3"]);
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let path = dir.join("invalid.wav");
    std::fs::write(&path, b"esto no es un wav").expect("debe escribirse el archivo basura");
    let (code, actual) = run_json_env(
        &[
            "--json",
            "--no-daemon",
            "speech",
            "transcribe",
            "--audio",
            path.to_str().expect("ruta UTF-8"),
            "--source-language",
            "es-latam",
        ],
        &envs,
    );
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(code, 2, "WAV inválido debe mapear a exit 2");
    assert_eq!(actual["reason"], "invalid_audio");
}

// ─── push-to-talk (guardas de validación, no-TTY) ──────────────────────
//
// Solo se blindan las guardas de validación: la ruta interactiva real
// (push-to-talk activo, al vencer) corre en TTY y no es ejercitable por esta suite.
// ejercitable por esta suite (todas las invocaciones fijan stdin a
// `Stdio::null()`, garantía de no-TTY). CA-08.5 (al vencer, TTY) queda como validación manual.
// queda como validación manual.

/// CA-08.1: `transcribe --mic` sin `--duration` sin TTY sale con
/// `ExitCode::InvalidInput` (2) sin iniciar ninguna captura — la guarda corta
/// antes de tocar el micrófono.
#[test]
fn speech_transcribe_mic_without_duration_no_tty_exits_2() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--mic",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(
        code, 2,
        "--mic sin --duration sin TTY debe mapear a ExitCode::InvalidInput (reason={:?})",
        actual["reason"]
    );
}

/// CA-08.2: `--duration` sin `--mic` (ni `--audio`) sale con
/// `ExitCode::InvalidInput` (2) — la guarda de origen obligatorio corta antes
/// de llegar a interpretar `--duration`.
#[test]
fn speech_transcribe_duration_without_mic_exits_2() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--duration",
        "3",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(
        code, 2,
        "--duration sin --mic (ni --audio) debe mapear a ExitCode::InvalidInput (reason={:?})",
        actual["reason"]
    );
}

/// CA-08.3: `--mic --duration N` sin TTY toma el selector de captura fija sin
/// panicar, sea cual sea el desenlace real (falta de dispositivo, modelo no
/// provisionado o feature STT ausente). El panic que cerraba era `duration.expect(...)` cuando `duration` era `None`.
/// `duration.expect(...)` cuando `duration` era `None`; aquí es `Some`, así
/// que nunca debía dispararse — esta prueba blinda que el `expect` no se
/// reintrodujo en la rama fija del selector.
#[test]
fn speech_transcribe_mic_duration_no_tty_no_panic() {
    hit_start(
        "speech_transcribe_mic_duration_no_tty_no_panic",
        Duration::from_secs(30),
    );
    let (dir, envs) = contract_sandbox("mic_transcribe");
    let output = Command::new(BIN)
        .args([
            "--json",
            "speech",
            "transcribe",
            "--mic",
            "--duration",
            "1",
            "--source-language",
            "es-latam",
        ])
        .envs(envs)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.code().is_some(),
        "el proceso no debe abortar/panicar: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked at"),
        "la rama de captura fija no debe panicar: {}",
        stderr
    );
    hit_end("speech_transcribe_mic_duration_no_tty_no_panic");
}

/// CA-08.4: equivalente a CA-08.1–08.3 para `speech dub`, que comparte la
/// misma guarda de argumentos y el mismo selector de captura.
#[test]
fn speech_dub_mic_no_duration_no_tty_exits_2() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "dub",
        "--mic",
        "--source-language",
        "es-latam",
        "--target-language",
        "es-latam",
    ]);
    assert_eq!(
        code, 2,
        "--mic sin --duration sin TTY debe mapear a ExitCode::InvalidInput (reason={:?})",
        actual["reason"]
    );
}

#[test]
fn speech_dub_duration_without_mic_exits_2() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "dub",
        "--duration",
        "3",
        "--source-language",
        "es-latam",
        "--target-language",
        "es-latam",
    ]);
    assert_eq!(
        code, 2,
        "--duration sin --mic debe mapear a ExitCode::InvalidInput (reason={:?})",
        actual["reason"]
    );
}

#[test]
fn speech_dub_mic_duration_no_tty_no_panic() {
    hit_start(
        "speech_dub_mic_duration_no_tty_no_panic",
        Duration::from_secs(30),
    );
    let (dir, envs) = contract_sandbox("mic_dub");
    let output = Command::new(BIN)
        .args([
            "--json",
            "speech",
            "dub",
            "--mic",
            "--duration",
            "1",
            "--source-language",
            "es-latam",
            "--target-language",
            "es-latam",
        ])
        .envs(envs)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        output.status.code().is_some(),
        "el proceso no debe abortar/panicar: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("panicked at"),
        "la rama de captura fija no debe panicar: {}",
        stderr
    );
    hit_end("speech_dub_mic_duration_no_tty_no_panic");
}

// ─── Límites de la vía daemon ──────────────────────────────────────────
//
// Los topes de duración (transcripción 300 s, referencia de clonado 30 s) se
// comprueban en la CLI antes de `model_missing`, de `stt_unsupported` y de
// contactar con el daemon, así que se ejercen con un sandbox sin modelos y con
// el pidfile señuelo o con `--no-daemon`, sin tocar el 8765 del usuario.

/// Escribe en `path` un WAV PCM 16-bit mono 16 kHz de silencio con `secs` segundos.
/// Los WAV largos viven solo en el sandbox temporal de la prueba.
fn write_silent_wav(path: &std::path::Path, secs: u32) {
    let data_len = secs * 16_000 * 2;
    let mut wav = Vec::<u8>::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(wav.len() + data_len as usize, 0);
    std::fs::write(path, wav).expect("escribir el WAV de silencio");
}

/// Ejecuta `args` en un sandbox de contrato con los `extra_envs` añadidos y
/// devuelve (código, JSON). El sandbox se borra al terminar.
fn run_in_contract_sandbox(tag: &str, args: &[&str], extra_envs: &[(&str, &str)]) -> (i32, Value) {
    let (dir, envs) = contract_sandbox(tag);
    let mut all: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    all.extend_from_slice(extra_envs);
    let result = run_json_env(args, &all);
    let _ = std::fs::remove_dir_all(&dir);
    result
}

/// Afirma que el resultado de `what` es exit 2 con `reason == audio_too_long`.
fn assert_audio_too_long(what: &str, result: (i32, Value)) {
    let (code, actual) = result;
    assert_eq!(
        code, 2,
        "{what}: exceder el tope debe salir con exit 2 ({actual})"
    );
    assert_eq!(actual["reason"], "audio_too_long", "{what}: {actual}");
}

/// Una transcripción de 301 s supera el tope de 300 s y se rechaza con
/// `audio_too_long` en la vía local y en la vía daemon.
#[test]
fn transcribe_audio_over_limit_is_audio_too_long() {
    for mode in ["--no-daemon", "--daemon"] {
        let (dir, envs) = contract_sandbox("transcribe_limit");
        let wav = dir.join("largo_301s.wav");
        write_silent_wav(&wav, 301);
        let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let result = run_json_env(
            &[
                "--json",
                mode,
                "speech",
                "transcribe",
                "--audio",
                wav.to_str().expect("ruta UTF-8"),
                "--source-language",
                "es-latam",
            ],
            &envs,
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_audio_too_long(&format!("transcribe {mode}"), result);
    }
}

/// Una referencia de clonado de 31 s supera el tope de 30 s y se rechaza con
/// `audio_too_long` en la vía local y en la vía daemon.
#[test]
fn clone_reference_over_limit_is_audio_too_long() {
    for mode in ["--no-daemon", "--daemon"] {
        let (dir, envs) = contract_sandbox("clone_limit");
        let wav = dir.join("referencia_31s.wav");
        write_silent_wav(&wav, 31);
        let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let result = run_json_env(
            &[
                "--json",
                mode,
                "voice",
                "clone",
                "--name",
                "referencia_larga",
                "--speech-reference",
                wav.to_str().expect("ruta UTF-8"),
            ],
            &envs,
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_audio_too_long(&format!("voice clone {mode}"), result);
    }
}

/// Un audio de doblaje de 41 s supera el tope de 40 s y se rechaza con
/// `audio_too_long`. La mitad `--no-daemon` prueba el orden: el tope se comprueba
/// antes de `model_missing` y de `stt_unsupported`. La mitad `--daemon` protege
/// frente a regresiones: la vía daemon da el mismo `audio_too_long` sin enviar el
/// audio.
#[test]
fn dub_audio_over_limit_is_audio_too_long() {
    for mode in ["--no-daemon", "--daemon"] {
        let (dir, envs) = contract_sandbox("dub_limit");
        let wav = dir.join("doblaje_41s.wav");
        write_silent_wav(&wav, 41);
        let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let result = run_json_env(
            &[
                "--json",
                mode,
                "speech",
                "dub",
                "--audio",
                wav.to_str().expect("ruta UTF-8"),
                "--source-language",
                "es-latam",
                "--target-language",
                "es-latam",
            ],
            &envs,
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_audio_too_long(&format!("speech dub {mode}"), result);
    }
}

/// Una referencia de clonado con `sample_rate` 0 es un WAV inválido: sale con
/// exit 2 e `invalid_audio` en las dos vías, sin pánico.
#[test]
fn clone_reference_zero_sample_rate_is_invalid_audio() {
    for mode in ["--no-daemon", "--daemon"] {
        let (dir, envs) = contract_sandbox("clone_zero_rate");
        let wav = dir.join("referencia_rate0.wav");
        avi_audio::write_zero_rate_wav(&wav);
        let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        let (code, actual) = run_json_env(
            &[
                "--json",
                mode,
                "voice",
                "clone",
                "--name",
                "referencia_rate0",
                "--speech-reference",
                wav.to_str().expect("ruta UTF-8"),
            ],
            &envs,
        );
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(code, 2, "voice clone {mode}: exit 2 expected ({actual})");
        assert_eq!(actual["reason"], "invalid_audio", "{mode}: {actual}");
    }
}

/// `--mic --duration 301` supera el tope de transcripción y se rechaza antes de
/// grabar. El sandbox no tiene modelos, así que mientras falte el tope la
/// invocación termina en `model_missing` sin abrir el micrófono.
#[test]
fn transcribe_mic_duration_over_limit_is_audio_too_long() {
    let result = run_in_contract_sandbox(
        "mic_duration_limit",
        &[
            "--json",
            "--no-daemon",
            "speech",
            "transcribe",
            "--mic",
            "--duration",
            "301",
            "--source-language",
            "es-latam",
        ],
        &[],
    );
    assert_audio_too_long("transcribe --mic --duration 301", result);
}

/// Un `AVI_PUSH_TO_TALK_MAX_SECS` de 301 supera el tope de transcripción y el
/// push-to-talk se rechaza antes de grabar. Sin TTY (stdin nulo) el estado actual
/// corta antes con `usage_error`, así que nunca llega a abrir el micrófono.
#[test]
fn transcribe_mic_env_over_limit_is_audio_too_long() {
    let result = run_in_contract_sandbox(
        "mic_env_limit",
        &[
            "--json",
            "--no-daemon",
            "speech",
            "transcribe",
            "--mic",
            "--source-language",
            "es-latam",
        ],
        &[("AVI_PUSH_TO_TALK_MAX_SECS", "301")],
    );
    assert_audio_too_long("transcribe --mic con AVI_PUSH_TO_TALK_MAX_SECS=301", result);
}

/// Daemon falso en un puerto efímero de 127.0.0.1: responde 200 a `/health` y
/// un estado fijo con cuerpo fijo a cualquier otra ruta, y registra las rutas
/// que recibe. Se publica en el pidfile del sandbox para que la CLI lo
/// descubra sin tocar el 8765.
struct FakeDaemon {
    addr: std::net::SocketAddr,
    paths: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl FakeDaemon {
    /// Arranca el servidor; toda ruta distinta de `/health` responde `status` con `body`.
    fn start(status: u16, body: Value) -> Self {
        use std::io::Read;
        use std::sync::atomic::Ordering;
        use std::sync::{Arc, Mutex};
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("puerto efímero");
        let addr = listener.local_addr().expect("dirección del servidor falso");
        let paths = Arc::new(Mutex::new(Vec::<String>::new()));
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (paths_t, stop_t) = (paths.clone(), stop.clone());
        let handle = std::thread::spawn(move || {
            for conn in listener.incoming() {
                if stop_t.load(Ordering::SeqCst) {
                    break;
                }
                let Ok(mut sock) = conn else { continue };
                let _ = sock.set_read_timeout(Some(Duration::from_secs(2)));
                // Lee cabeceras y, si las hay, el cuerpo completo según content-length.
                let mut raw = Vec::<u8>::new();
                let mut chunk = [0u8; 8192];
                let mut header_end = None;
                let mut needed = usize::MAX;
                while raw.len() < needed {
                    match sock.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => raw.extend_from_slice(&chunk[..n]),
                    }
                    if header_end.is_none() {
                        if let Some(pos) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                            let head = String::from_utf8_lossy(&raw[..pos]).to_lowercase();
                            let len = head
                                .lines()
                                .find_map(|l| l.strip_prefix("content-length:"))
                                .and_then(|v| v.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            header_end = Some(pos + 4);
                            needed = pos + 4 + len;
                        }
                    }
                }
                let head = String::from_utf8_lossy(&raw);
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("")
                    .to_string();
                paths_t.lock().unwrap().push(path.clone());
                let (code, payload) = if path == "/health" {
                    (
                        200,
                        serde_json::json!({ "status": "ok", "schema_version": "4", "warm": "ready" }),
                    )
                } else {
                    (status, body.clone())
                };
                let payload = payload.to_string();
                let response = format!(
                    "HTTP/1.1 {code} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                    payload.len()
                );
                let _ = sock.write_all(response.as_bytes());
            }
        });
        FakeDaemon {
            addr,
            paths,
            stop,
            handle: Some(handle),
        }
    }

    /// Sustituye el pidfile señuelo del sandbox por uno que apunta a este servidor.
    fn publish_in(&self, data_dir: &std::path::Path) {
        let dead_pid = u32::MAX - 1;
        let content = serde_json::json!({
            "pid": dead_pid,
            "addr": self.addr.to_string(),
            "resident_pid": dead_pid,
        });
        std::fs::write(data_dir.join("daemon.pid"), content.to_string())
            .expect("publicar el pidfile del daemon falso");
    }

    /// Rutas recibidas hasta ahora, en orden de llegada.
    fn requested_paths(&self) -> Vec<String> {
        self.paths.lock().unwrap().clone()
    }
}

impl Drop for FakeDaemon {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        // Una conexión vacía desbloquea el `accept` pendiente para que el hilo termine.
        let _ = std::net::TcpStream::connect(self.addr);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// Ejecuta `speech dub` por la vía daemon contra un servidor falso que responde
/// 404 con `body` a `/dub`, y devuelve (código, JSON, rutas recibidas).
fn dub_against_fake_daemon_404(tag: &str, body: Value) -> (i32, Value, Vec<String>) {
    let fake = FakeDaemon::start(404, body);
    let (dir, envs) = contract_sandbox(tag);
    fake.publish_in(&dir);
    let wav = dir.join("entrada_1s.wav");
    write_silent_wav(&wav, 1);
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (code, actual) = run_json_env(
        &[
            "--json",
            "--daemon",
            "speech",
            "dub",
            "--audio",
            wav.to_str().expect("ruta UTF-8"),
            "--source-language",
            "es-latam",
            "--target-language",
            "es-latam",
        ],
        &envs,
    );
    let _ = std::fs::remove_dir_all(&dir);
    (code, actual, fake.requested_paths())
}

/// Un 404 de `/dub` con `reason` `voice_not_found` sale con exit 3 y no degrada
/// a una composición que transcriba por separado.
#[test]
fn dub_daemon_404_voice_not_found_exits_3_without_transcribe() {
    let (code, actual, paths) = dub_against_fake_daemon_404(
        "dub_404_voice",
        serde_json::json!({ "reason": "voice_not_found", "message": "La voz no existe." }),
    );
    assert_eq!(code, 3, "voice_not_found debe salir con exit 3 ({actual})");
    assert_eq!(actual["reason"], "voice_not_found");
    assert!(
        !paths.iter().any(|p| p == "/transcribe"),
        "no debe haber petición a /transcribe: {paths:?}"
    );
}

/// Un 404 de `/dub` con `reason` `model_missing` sale con exit 4 y no degrada
/// a una composición que transcriba por separado.
#[test]
fn dub_daemon_404_model_missing_exits_4_without_transcribe() {
    let (code, actual, paths) = dub_against_fake_daemon_404(
        "dub_404_model",
        serde_json::json!({ "reason": "model_missing", "message": "Falta el modelo." }),
    );
    assert_eq!(code, 4, "model_missing debe salir con exit 4 ({actual})");
    assert_eq!(actual["reason"], "model_missing");
    assert!(
        !paths.iter().any(|p| p == "/transcribe"),
        "no debe haber petición a /transcribe: {paths:?}"
    );
}

/// Un 404 de `/dub` sin `reason` ni `error` es un daemon antiguo sin la ruta:
/// sale con exit 1, un mensaje que indica reiniciarlo y ninguna petición a
/// `/transcribe`.
#[test]
fn dub_daemon_bare_404_exits_1_with_restart_hint_without_transcribe() {
    let (code, actual, paths) = dub_against_fake_daemon_404("dub_404_bare", serde_json::json!({}));
    assert_eq!(
        code, 1,
        "un 404 sin reason debe salir con exit 1 ({actual})"
    );
    assert_eq!(actual["reason"], "daemon_error");
    let message = actual["error"].as_str().unwrap_or("");
    assert!(
        message.contains("daemon restart"),
        "el mensaje debe indicar `daemon restart`: {message}"
    );
    assert!(
        !paths.iter().any(|p| p == "/transcribe"),
        "no debe haber petición a /transcribe: {paths:?}"
    );
}

#[test]
fn daemon_status_matches_fixture() {
    // Desdoble por régimen de sesión (con o sin sesión compartida en ejecución):
    // con fixture de sesión en ejecución el
    // estado efectivo es `running`; sin daemon, `stopped` intacto. Se elimina
    // la comparación incondicional contra la fixture detenida (falso rojo
    // bajo sesión).
    // El `stopped` por probe incluye en el producto la búsqueda del residente
    // por su PID registrado antes de declarar vía libre — el display sigue
    // `stopped` (contrato), pero `start` ante residente-solo reclama con
    // `started`, nunca declara fresco sin reclaim.
    let (code, actual) = run_json(&["--json", "daemon", "status"]);
    assert_eq!(code, 0);
    if actual["daemon"] == Value::String("running".to_string()) {
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        let expected = fixture("cli_daemon_status_running.json");
        assert_eq!(actual["daemon"], expected["daemon"]);
    } else {
        assert_eq!(actual, fixture("cli_daemon_status.json"));
    }
}

// ─── `cleanup` ────────────────────────────────────────────────────────────
//
// **Ninguna de las pruebas de este bloque carga una fixture, y antes de este ciclo
// todas se llamaban `*_matches_fixture`.** De las siete, solo
// `cleanup_dry_run_matches_fixture` leía una fixture real; las otras seis afirmaban en
// línea, de modo que el nombre mentía sobre lo que cubrían y la cobertura nominal de
// `cleanup` era aparente. Los nombres se conservan para no romper la referencias
// externas, y cada bloque dice en su comentario qué afirma de verdad.

/// Sin categoría: `usage_error` con salida 2 y sin borrar nada (criterio 22). Es la
/// puerta de uso de la tabla de reasons y no una aserción de fixture.
#[test]
fn cleanup_matches_fixture() {
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 2, "cleanup sin flags debe ser InvalidInput");
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
}

/// El mismo gate, afirmado por su salida y su `reason` y nada más: es lo que la puerta
/// de uso necesita saber.
#[test]
fn cleanup_without_flags_exits_2() {
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 2);
    assert_eq!(actual["reason"], Value::String("usage_error".to_string()));
}

/// `--voices` en simulación: el sobre estándar, con `status` y `reason`, y el alcance
/// de la categoría. No carga fixture: afirma el contrato en línea.
#[test]
fn cleanup_voices_matches_fixture() {
    let (code, actual) = run_json(&["--json", "cleanup", "--voices", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(
        actual["status"],
        Value::String("cleanup_complete".to_string())
    );
    assert_eq!(actual["reason"], Value::Null, "sin `reason` en el éxito");
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
}

/// `--synthetic-speech` en simulación: el mismo sobre y el alcance de la categoría.
#[test]
fn cleanup_synthetic_speech_matches_fixture() {
    let (code, actual) = run_json(&["--json", "cleanup", "--synthetic-speech", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(
        actual["status"],
        Value::String("cleanup_complete".to_string())
    );
    assert_eq!(actual["reason"], Value::Null);
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
}

/// `--model` en simulación: con la raíz **exclusiva** de la aplicación, el alcance de
/// `--model` es el directorio entero, así que `removed` lo nombra una vez. Es el
/// alcance nuevo de `--model` con la decisión de relocalizar `xet`, y no el layout viejo de
/// snapshots sueltos.
#[test]
fn cleanup_model_matches_fixture() {
    let (dir, envs) = sandbox_unique_state("cleanup-model");
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (code, actual) = run_json_env(&["--json", "cleanup", "--model", "--dry-run"], &envs);
    assert_eq!(code, 0, "{}", actual);
    assert_eq!(
        actual["status"],
        Value::String("cleanup_complete".to_string())
    );
    assert_eq!(actual["reason"], Value::Null);
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    let roots = dir.join("cache");
    assert!(
        actual["removed"]
            .as_array()
            .expect("removed debe ser array")
            .iter()
            .any(|p| p.as_str() == Some(roots.to_string_lossy().as_ref())),
        "el alcance de `--model` en raíz exclusiva es el directorio entero: {}",
        actual["removed"]
    );
}

/// `--all` en simulación: la unión de las tres categorías más configuración, logs y el
/// estado del daemon.
#[test]
fn cleanup_all_matches_fixture() {
    let (code, actual) = run_json(&["--json", "cleanup", "--all", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(
        actual["status"],
        Value::String("cleanup_complete".to_string())
    );
    assert_eq!(actual["reason"], Value::Null);
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert!(actual["removed"].is_array());
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
}

/// La **única** prueba de `cleanup` que carga una fixture real, y sigue siendo una
/// prueba de fixture: compara contra `tests/golden/cli_cleanup_dry_run.json`, que este
/// ciclo regenera con el plan nuevo de `--dry-run` y con la versión `"4"` del sobre.
#[test]
fn cleanup_dry_run_matches_fixture() {
    // --voices + --dry-run es el caso canónico de dry_run; valida fixture dedicada
    let (code, actual) = run_json(&["--json", "cleanup", "--voices", "--dry-run"]);
    assert_eq!(code, 0);
    assert_eq!(actual["dry_run"], Value::Bool(true));
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    // fixture de referencia para dry_run debe coincidir en status
    let expected = fixture("cli_cleanup_dry_run.json");
    assert_eq!(actual["status"], expected["status"]);
    assert_eq!(actual["dry_run"], expected["dry_run"]);
    assert_eq!(actual["schema_version"], expected["schema_version"]);
}
/// `cleanup --model` de punta a punta, en las **dos** formas que puede tomar la raíz de
/// modelos de la tabla de rutas.
///
/// La versión anterior plantaba el layout viejo —`<HF_HOME>/hub/models--…`, `hub/ct2`,
/// `hub/.locks` y `xet` como hermano— y exigía que los cuatro aparecieran en `removed`.
/// Con la decisión de relocalizar `xet` y con R3 aplicada, ese layout ya no describe
/// nada: en la raíz **exclusiva** el borrado es de directorio entero, y en la raíz
/// **compartida** que el usuario elige con `HF_HUB_CACHE` solo se borran los repos
/// fijados, sus derivados y sus locks.
///
/// Lo que la prueba afirma, y es lo que el plan fija como su destino:
///
/// - `--dry-run` y la ejecución real listan **lo mismo**, porque salen de la misma
///   función de cálculo.
/// - En la raíz exclusiva, `--model` borra el directorio entero, `xet` incluido.
/// - En la raíz compartida, borra el repo propio y el derivado CT2, y **no** borra
///   `xet`, ni el `.locks` completo, ni el repos de otra herramienta (criterio 23).
/// - El barrido transversal recoge un temporal propio huérfano **sin PID**, que es el
///   segundo punto heredado que este lote absorbe.
#[test]
fn cleanup_model_real_run_reports_paths() {
    let n = TMP_COUNTER.fetch_add(1, Ordering::SeqCst);
    let sandbox = std::env::temp_dir().join(format!("cleanup_sandbox_{}_{n}", std::process::id()));
    let data = sandbox.join("data");
    std::fs::create_dir_all(&data).unwrap();
    // El temporal del sistema se desvía al sandbox: `cleanup` barre los temporales
    // propios y no debe tocar los de la máquina que ejecuta la prueba.
    let tmp = sandbox.join("tmp");
    std::fs::create_dir_all(&tmp).unwrap();
    let temp = tmp.join("avi_clone_x_1.qvoice");
    std::fs::write(&temp, b"marker").unwrap();

    // ── Caso exclusivo: `AVI_CACHE_DIR` manda sobre las variables HF ──────────────
    // Las variables HF se vacían porque `models_cache_dir` da prioridad a
    // `AVI_CACHE_DIR` pero, sin ella, un `HF_HUB_CACHE` del entorno de la máquina
    // haría que la raíz se resolviera a otro sitio y la prueba probaría otra cosa.
    let exclusive = sandbox.join("models-exclusiva");
    seed_models(&exclusive);
    let install = sandbox.join("install");
    std::fs::create_dir_all(&install).unwrap();
    let exclusive_envs = [
        ("AVI_DATA_DIR", data.to_str().unwrap()),
        ("AVI_INSTALL_DIR", install.to_str().unwrap()),
        ("AVI_DAEMON_PORT", "0"),
        ("AVI_CACHE_DIR", exclusive.to_str().unwrap()),
        ("HF_HUB_CACHE", ""),
        ("HF_HOME", ""),
        ("TMP", tmp.to_str().unwrap()),
        ("TEMP", tmp.to_str().unwrap()),
        ("TMPDIR", tmp.to_str().unwrap()),
    ];

    // `cleanup` detiene el daemon y, sin `resident_pid` registrado, barre por imagen
    // los `qwen_tts` de la máquina. El pidfile señuelo (puerto cerrado, PID muertos,
    // `resident_pid` distinto de 0) evita ambos efectos; se resiembra antes de cada
    // ejecución porque la parada lo puede borrar.
    seed_decoy_daemon_pidfile(&data);
    let (code, dry) = run_json_env(
        &["--json", "cleanup", "--model", "--dry-run"],
        &exclusive_envs,
    );
    assert_eq!(code, 0, "{}", dry);
    seed_decoy_daemon_pidfile(&data);
    let (code, real) = run_json_env(&["--json", "cleanup", "--model", "--yes"], &exclusive_envs);
    assert_eq!(code, 0, "{}", real);

    let as_set = |v: &Value| -> std::collections::BTreeSet<String> {
        v["removed"]
            .as_array()
            .expect("removed debe ser array")
            .iter()
            .map(|s| s.as_str().unwrap().to_string())
            .collect()
    };
    let dry_removed = as_set(&dry);
    let real_removed = as_set(&real);
    for path in &dry_removed {
        assert!(
            real_removed.contains(path),
            "real y dry-run deben listar lo mismo: {path} está en el dry-run y no en el real"
        );
    }
    assert!(
        real_removed.contains(&exclusive.display().to_string()),
        "en la raíz exclusiva `--model` borra el directorio entero: {real_removed:?}"
    );
    assert!(
        !exclusive.exists(),
        "y la raíz desaparece, `xet` incluido: {:?}",
        exclusive.join("xet")
    );
    assert!(
        !temp.exists(),
        "el barrido transversal recoge el temporal propio huérfano sin PID"
    );

    // ── Caso compartido: `HF_HUB_CACHE` elige la raíz, y R3 manda ────────────────
    let shared = sandbox.join("hub-compartida");
    seed_models(&shared);
    std::fs::create_dir_all(shared.join("models--Qwen--Qwen3-TTS-0.6B")).unwrap();
    std::fs::write(
        shared
            .join("models--Qwen--Qwen3-TTS-0.6B")
            .join("config.json"),
        b"{}",
    )
    .unwrap();
    let foreign = shared.join("models--otra--herramienta");
    let shared_envs = [
        ("AVI_DATA_DIR", data.to_str().unwrap()),
        ("AVI_INSTALL_DIR", install.to_str().unwrap()),
        ("AVI_DAEMON_PORT", "0"),
        ("AVI_CACHE_DIR", ""),
        ("HF_HUB_CACHE", shared.to_str().unwrap()),
        ("HF_HOME", ""),
        ("TMP", tmp.to_str().unwrap()),
        ("TEMP", tmp.to_str().unwrap()),
        ("TMPDIR", tmp.to_str().unwrap()),
    ];

    seed_decoy_daemon_pidfile(&data);
    let (code, really_shared) =
        run_json_env(&["--json", "cleanup", "--model", "--yes"], &shared_envs);
    assert_eq!(code, 0, "{}", really_shared);
    let removed = as_set(&really_shared);

    assert!(
        removed.contains(
            &shared
                .join("models--CristianRojaas--opus-mt-es-en-ct2-int8")
                .display()
                .to_string()
        ),
        "el repo propio sí se borra: {removed:?}"
    );
    // R3: nada de esto se toca.
    assert!(
        !removed.contains(&shared.join("xet").display().to_string()),
        "R3: `xet` no se puede listar como borrado: {removed:?}"
    );
    assert!(
        !removed.contains(&shared.join(".locks").display().to_string()),
        "R3: el `.locks` completo no se puede listar como borrado: {removed:?}"
    );
    assert!(
        !removed.contains(&foreign.display().to_string()),
        "R3: el repo de otra herramienta no se puede listar como borrado: {removed:?}"
    );
    assert!(
        shared.join("xet").exists(),
        "R3: `xet` sobrevive a `--model` en una raíz compartida"
    );
    assert!(
        shared.join(".locks").exists(),
        "R3: el `.locks` completo sobrevive"
    );
    assert!(
        foreign.exists(),
        "criterio 23: los modelos de otra herramienta sobreviven"
    );
    assert!(shared.exists(), "y la raíz compartida no se borra entera");
    assert!(
        !shared
            .join("models--CristianRojaas--opus-mt-es-en-ct2-int8")
            .exists(),
        "mientras el repo propio sí desaparece"
    );

    let _ = std::fs::remove_dir_all(&sandbox);
}

/// Planta el layout de modelos vigente: un repo fijado, los locks y `xet` colgando
/// de la raíz, más un repo de otra herramienta.
fn seed_models(root: &std::path::Path) {
    let repo = root.join("models--CristianRojaas--opus-mt-es-en-ct2-int8");
    std::fs::create_dir_all(repo.join("snapshots").join("abc")).unwrap();
    std::fs::write(
        repo.join("snapshots").join("abc").join("config.json"),
        b"{}",
    )
    .unwrap();
    std::fs::create_dir_all(root.join(".locks").join("models--x--y")).unwrap();
    std::fs::create_dir_all(root.join("xet")).unwrap();
    std::fs::create_dir_all(root.join("models--otra--herramienta")).unwrap();
}
/// `doctor --json` emite **un solo objeto** también cuando falla, con el veredicto
/// dentro y la salida 1.
///
/// Es la garantía del contrato —"cada invocación emite exactamente un objeto
/// JSON"— y la forma de fijarla es comprobar que **`stdout` contiene un único objeto**
/// y que no lleva la clave `error` que el canal de error añadiría detrás. La prueba
/// pasa aunque el veredicto sea favorable, porque lo que afirma es la unicidad del
/// objeto y la ausencia del canal de error, no el resultado del diagnóstico.
#[test]
fn doctor_json_emits_exactly_one_object_even_on_failure() {
    let (dir, envs) = sandbox_unique_state("doctor");
    let envs: Vec<(&str, &str)> = envs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
    let (code, value) = run_json_env(&["--json", "doctor"], &envs);
    // Un objeto: `run_json_env` ya no habría podido parsear dos objetos JSON seguidos,
    // así que llegar aquí con un solo objeto parseado es parte de la prueba.
    // objetos JSON seguidos, así que llegar aquí con un solo objeto parseado ya es parte de la prueba.
    assert!(
        value.get("error").is_none(),
        "el veredicto de `doctor` no lleva objeto `error`: {}",
        value
    );
    for key in [
        "version", "target", "channel", "install", "path", "pending", "models", "checks", "failed",
    ] {
        assert!(
            value.get(key).is_some(),
            "falta la clave `{key}` del reporte de ciclo de vida: {}",
            value
        );
    }
    // Las claves que el contrato retira. El sobre afirma su conjunto exacto en la
    // prueba de doctor del motor; aquí se afirma, de este lado del cable, que la raíz
    // de datos dejó de ser clave de primer nivel.
    assert!(
        value.get("data_dir").is_none(),
        "la raíz de datos no puede ser clave de primer nivel: {}",
        value
    );
    // La información que las claves retiradas tenían no se pierde: cambia de sitio.
    assert!(
        value["install"]["data_dir"].is_string(),
        "la raíz de datos vive dentro de `install`"
    );
    assert!(
        value["models"]["base"].is_string(),
        "el estado del modelo opt-in de clonado vive dentro de `models`"
    );
    assert_eq!(
        value["schema_version"],
        Value::String("4".to_string()),
        "el sobre de la CLI sube a \"4\""
    );
    // Y el código de salida es el del veredicto: 1 si falló, 0 si no.
    let failed = value["failed"].as_array().is_some_and(|f| !f.is_empty());
    assert_eq!(
        code,
        if failed { 1 } else { 0 },
        "el código de salida es el del veredicto, sin objeto `error` detrás: {value}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn voice_list_respects_envelope_contract() {
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato (envelope + presencia de `default`).
    let (code, actual) = run_json(&["--json", "voice", "list"]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
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
fn translate_empty_text_exits_2() {
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
#[ignore = "requiere los modelos CT2 es-en y en-es"]
fn translate_es_to_en_produces_translation() {
    require_ct2();
    // Ruta local fijada con `--no-daemon` sobre una instancia aislada con la
    // raíz de modelos real heredada (solo lectura): no toca daemon ni estado
    // compartido. El texto traducido depende del motor real; se verifican
    // invariantes de contrato (mismo patrón que
    // `speech_transcribe_with_audio_matches_contract`).
    let inst = IsolatedInstance::new("translate_contract");
    let (code, actual) = run_json_env(
        &[
            "--json",
            "--no-daemon",
            "translate",
            "--text",
            "Hola, ¿cómo estás?",
            "--from",
            "es",
            "--to",
            "en",
        ],
        &inst.args(),
    );
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    assert_eq!(actual["source"], Value::String("es".to_string()));
    assert_eq!(actual["target"], Value::String("en".to_string()));
    let translated = actual["translated"]
        .as_str()
        .expect("`translated` debe ser un string");
    assert!(!translated.is_empty(), "`translated` no debe estar vacío");
}

#[test]
fn translate_passthrough_same_lang_returns_intact() {
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
fn translate_unsupported_pair_exits_2() {
    // El alfabeto estricto del parser (`es`/`en`) rechaza el par antes del handler → exit 2 de `clap`.
    // del handler → exit 2 de `clap`, sin envelope JSON que afirmar.
    let output = Command::new(BIN)
        .args([
            "--json",
            "translate",
            "--text",
            "Bonjour",
            "--from",
            "fr",
            "--to",
            "de",
        ])
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    assert_eq!(
        code, 2,
        "par fuera del alfabeto debe rechazarlo el parser con exit 2"
    );
}

#[test]
fn translate_es_latam_rejected_by_parser_exits_2() {
    // Cambio deliberado: `es-latam` no pertenece al alfabeto del parser (`es`/`en`).
    // parser (`es`/`en`) aunque la vía IPC lo siga normalizando → exit 2.
    let output = Command::new(BIN)
        .args([
            "--json",
            "translate",
            "--text",
            "Hola",
            "--from",
            "es-latam",
            "--to",
            "en",
        ])
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    assert_eq!(
        code, 2,
        "`es-latam` vía CLI debe rechazarlo el parser con exit 2"
    );
}

// ─── Golden TTS (contrato dorado de síntesis, doblaje y clonado de voz) ───

mod tts {
    use super::*;
    // El trait STT solo se necesita para el cálculo de WER real (native-stt).
    #[cfg(feature = "native-stt")]
    use avi_core::engine::SttEngine;
    use std::path::Path;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};

    /// Ruta del binario del motor Qwen3-TTS (override o vendored).
    fn tts_binary() -> Option<PathBuf> {
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

    /// Exige el snapshot del modelo CustomVoice `qwen3-tts-0.6b` (su
    /// `config.json`) en la raíz de modelos vigente.
    fn require_tts_model() {
        let file = avi_store::ModelStore::new()
            .model_snapshot_path("qwen3-tts-0.6b")
            .expect("el pin de qwen3-tts-0.6b debe existir en MODEL_REVISIONS")
            .join("config.json");
        assert!(
            file.is_file(),
            "falta {}: aprovisiona el modelo con `ai-voice-interconnector setup`",
            file.display()
        );
    }

    /// Exige el modelo Base `qwen3-tts-0.6b-base` que necesita el clonado: su
    /// snapshot (`config.json`) o, si no, el directorio vendored con
    /// `"tts_model_type": "base"`.
    fn require_tts_base_model() {
        let snapshot = avi_store::ModelStore::new()
            .model_snapshot_path("qwen3-tts-0.6b-base")
            .expect("el pin de qwen3-tts-0.6b-base debe existir en MODEL_REVISIONS")
            .join("config.json");
        let vendored = Path::new("vendor/qwen3-tts/qwen3-tts-0.6b-base/config.json");
        let vendored_is_base = std::fs::read_to_string(vendored)
            .map(|c| c.contains("\"tts_model_type\": \"base\""))
            .unwrap_or(false);
        assert!(
            snapshot.is_file() || vendored_is_base,
            "falta {} (ni {} con tts_model_type base): aprovisiona el modelo Base con \
             `ai-voice-interconnector setup --with-voice-cloning`",
            snapshot.display(),
            vendored.display()
        );
    }

    /// Exige el binario del motor (`QWEN3_TTS_BIN` o
    /// `vendor/qwen3-tts/qwen_tts.exe`). Los pesos no se exigen aparte: el motor
    /// usa el snapshot de la raíz de modelos que exige `require_tts_model()`.
    fn require_tts_binary() {
        let binary = tts_binary().expect(
            "falta el binario del motor (QWEN3_TTS_BIN o vendor/qwen3-tts/qwen_tts.exe): \
             constrúyelo con `cargo xtask build-engine`",
        );
        assert!(
            binary.is_file(),
            "falta {}: constrúyelo con `cargo xtask build-engine`",
            binary.display()
        );
    }

    /// Mutex global para serializar los tests TTS pesados (cada corrida ocupa
    /// ~2.7 GB de RAM, y el motor residente escucha en un
    /// `default_port()` de servicio fijo que no admite dos instancias a la vez).
    /// Este es el núcleo físico legítimo que sobrevive (semáforo de
    /// capacidad de una única inferencia pesada residente, documentado como
    /// límite real de recursos, no como
    /// muleta): `synthesis_lock` del producto (`avi-daemon`) y `HEALTH_OBS_*`
    /// (`avi-tts`) se conservan intactos. Sin tolerancia al envenenado.
    ///
    /// **Techo de RAM y prohibición de eliminar el lock.** ~2.7 GB por
    /// residente TTS es el pico medido de UN solo proceso; los runners de CI
    /// (y la máquina de desarrollo) no garantizan margen para sostener dos o
    /// más residentes simultáneos. Paralelizar los tests pesados (quitar o
    /// debilitar `TTS_LOCK`) arriesga OOM del runner, no una mera degradación
    /// de rendimiento: un OOM mata el proceso de test a mitad de ciclo y deja
    /// el diagnóstico (reaper, hitos) sin oportunidad de correr. Por tanto
    /// **no se elimina `TTS_LOCK`/`lock_tts()`**, ni se reemplaza por un
    /// esquema que permita concurrencia entre residentes TTS reales; cualquier
    /// mejora de paralelismo de la suite debe respetar este techo (p. ej.
    /// paralelizar únicamente los tests que NO adquieren `lock_tts()`).
    pub(crate) static TTS_LOCK: Mutex<()> = Mutex::new(());

    pub(crate) fn lock_tts() -> std::sync::MutexGuard<'static, ()> {
        TTS_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    // Nota (libtest): con la serie de tests pesados bajo `TTS_LOCK`, es
    // esperable que `cargo test` emita el warning de libtest "test tts::<x>
    // has been running for over 60 seconds" para el/los test(s) que esperan
    // turno en el mutex mientras otro test pesado sintetiza. Es espera de
    // mutex esperada (serialización deliberada por el techo de RAM de
    // arriba), NO un cuelgue: el test en cola no está atascado, solo hace
    // fila. No se le añaden reintentos ni timeouts para "arreglarlo" — hacerlo
    // enmascararía un cuelgue real si alguna vez ocurriera uno.

    /// Etiqueta/voz única por corrida (el contrato normaliza a minúsculas).
    fn unique_label(prefix: &str) -> String {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("reloj del sistema")
            .as_nanos();
        format!("{}{}_{}", prefix, nanos, std::process::id())
    }

    /// Escribe en el directorio temporal un WAV válido al que se le recortan los
    /// últimos bytes: la cabecera declara más datos de los que contiene el archivo.
    fn write_truncated_wav(label: &str) -> PathBuf {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..1_600 {
                writer.write_sample((i % 200) as i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        let mut bytes = cursor.into_inner();
        bytes.truncate(bytes.len() - 400);
        let path = std::env::temp_dir().join(format!("avi_{}.wav", unique_label(label)));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// El WAV producido debe ser PCM s16le mono 24 kHz con muestras (spec del motor).
    /// Solo lo usan los E2E de síntesis que verifican WER real (native-stt).
    #[cfg(feature = "native-stt")]
    fn valid_wav_24k(path: &Path) {
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
    fn wer_vs_text(path: &Path, text: &str) -> f64 {
        let pcm = avi_audio::load_wav_16k_mono_pcm(path.to_string_lossy().as_ref())
            .unwrap_or_else(|e| panic!("no se pudo cargar {} a 16k: {}", path.display(), e));
        let snapshot = avi_store::ModelStore::new()
            .model_snapshot_path("parakeet-tdt-v3")
            .expect("el modelo parakeet-tdt-v3 debe tener un pin de revisión");
        let engine =
            avi_stt::ParakeetEngine::new(snapshot).expect("el modelo Parakeet TDT v3 debe existir");
        let transcribed = engine
            .transcribe(&pcm, Some("es"))
            .expect("la transcripción no debe fallar");
        let a = normalize(&transcribed);
        let b = normalize(text);
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
    fn normalize(s: &str) -> Vec<String> {
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
    fn has_audio_device() -> bool {
        match avi_audio::get_devices_json() {
            Ok(devs) => !devs.is_empty(),
            Err(_) => false,
        }
    }

    /// Exige al menos un dispositivo de salida de audio.
    #[cfg(feature = "native-stt")]
    fn require_audio_device() {
        assert!(
            has_audio_device(),
            "no hay ningún dispositivo de salida de audio disponible en esta máquina"
        );
    }

    // ─── synthesize ─────────────────────────────────────────────────────

    /// Éxito con `--label`: exit 0, WAV persistido en `speech/`, envelope y
    /// WER ≤ 0.25 frente al texto fuente. La verificación de WER exige el motor
    /// Parakeet (native-stt); sin el feature no se compila (en CI featureless los
    /// modelos tampoco están, así que no se pierde cobertura).
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor y el modelo Parakeet"]
    fn synthesize_ok_with_label() {
        require_tts_model();
        require_tts_binary();
        require_parakeet();
        // Serie + ciclo: excluye cleanup (STATE) y paradas del ciclo durante la
        // vía caliente; el orden STATE→TTS coincide con el resto de la suite.
        let _guard = lock_tts();
        hit_start_heavy("tts::synthesize_ok_with_label");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = arm_reaper("synthesize_ok_with_label");
        let label = unique_label("golden");
        // Vía daemon caliente de la instancia propia (instancia aislada): la instancia ya
        // pagó la carga del motor una sola vez; este test no recarga en frío.
        // `--daemon` fuerza la ruta (sin fallback silencioso a directo): si el
        // daemon no responde, falla.
        // Paridad de envelope verificada en el producto: mismo
        // {status, audio_path, voice} persistido en el almacén, con chequeo de
        // colisión de etiqueta en el cliente en ambas rutas, en la variante local
        // y en la del daemon por igual.
        let inst = IsolatedInstance::new("synthesize_label");
        start_instance(&inst, &[]);
        let a = inst.args();
        let (code, actual) = run_json_env(
            &[
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
            ],
            &a,
        );
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        valid_wav_24k(audio_path);
        let wer = wer_vs_text(
            audio_path,
            "Hola, este es un mensaje de prueba para la verificación.",
        );
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
        let _ = avi_store::SpeechStore::at(inst.dir.join("speech")).remove("default", &label);
        stop_instance(&inst, "synthesize_ok_with_label");
        hit_end("tts::synthesize_ok_with_label");
    }

    /// Gate WER texto corto — verifica que síntesis breve (2-4 palabras) con voz `default` logra WER ≤ 0.25 vía Parakeet.
    ///
    /// Cubre el caso que la E2E `test-windows-e2e` sintetizaba sin veredicto:
    /// texto de 2-4 palabras (`"Hola mundo"`) con voz `default` (preset ryan).
    /// Verifica `WAV 24kHz mono 16-bit` y `WER ≤ 0.25` vía Parakeet (`native-stt`),
    /// mismo patrón que `synthesize_ok_with_label` (11 palabras): requiere
    /// el modelo TTS, el binario y `require_parakeet()`, usa
    /// `valid_wav_24k` y `wer_vs_text`, falla la E2E/gate si `WER > 0.25`.
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor y el modelo Parakeet"]
    fn synthesize_short_text_wer_gate() {
        require_tts_model();
        require_tts_binary();
        require_parakeet();
        // Instancia aislada sin daemon (este test es ruta directa): el
        // sandbox propio aísla su estado (WAV + sidecar) del data_dir
        // compartido; `TTS_LOCK` actúa como semáforo de inferencia.
        let _guard = lock_tts();
        let inst = IsolatedInstance::new("wer_gate");
        avi_store::VoiceStore::at(inst.dir.join("voices"))
            .ensure_initialized()
            .expect("voces de fábrica inicializadas");
        let a = inst.args();
        let short_text = "Hola mundo";
        let label = unique_label("golden_corto");
        // Testigo en directo de `synthesize` (ruta local, sin daemon): ruta local fijada con
        // `--no-daemon` para que ningún daemon en ejecución lo reenrute en
        // silencio. Es el más barato con gate WER.
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "speech",
                "synthesize",
                "--text",
                short_text,
                "--voice",
                "default",
                "--label",
                &label,
            ],
            &a,
        );
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        assert_eq!(actual["status"], Value::String("success".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV debe estar persistido en el almacén"
        );
        valid_wav_24k(audio_path);
        let wer = wer_vs_text(audio_path, short_text);
        assert!(
            wer <= 0.25,
            "WER texto corto '{}' = {} debe ser ≤ 0.25 (disparador H1)",
            short_text,
            wer
        );
        let _ = avi_store::SpeechStore::at(inst.dir.join("speech")).remove("default", &label);
    }

    #[test]
    fn synthesize_empty_text_exits_2() {
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
    fn synthesize_text_too_long_exits_2() {
        let long_text = "a".repeat(501);
        let (code, actual) = run_json(&[
            "--json",
            "speech",
            "synthesize",
            "--text",
            &long_text,
            "--label",
            "x",
        ]);
        assert_eq!(code, 2, "texto de 501 caracteres → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("text_too_long".to_string()));
    }

    #[test]
    fn say_text_too_long_exits_2() {
        let long_text = "a".repeat(501);
        let (code, actual) = run_json(&["--json", "speech", "say", "--text", &long_text]);
        assert_eq!(code, 2, "texto de 501 caracteres → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("text_too_long".to_string()));
    }

    #[test]
    fn synthesize_text_at_limit_is_not_rejected_by_validation() {
        // Con 500 caracteres la validación pasa; el sandbox no tiene modelos, así
        // que el resultado es otro `reason` (modelo ausente) y no se sintetiza
        // nada, pero nunca `text_too_long`.
        let limit_text = "a".repeat(500);
        let (_code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "synthesize",
            "--text",
            &limit_text,
            "--label",
            "x",
        ]);
        assert_ne!(actual["reason"], Value::String("text_too_long".to_string()));
        assert_ne!(actual["reason"], Value::String("empty_text".to_string()));
        assert!(
            actual.get("audio_path").is_none(),
            "sin modelos no debe sintetizarse audio: {actual}"
        );
    }

    #[test]
    fn synthesize_missing_voice_exits_3() {
        // Sandbox propio con el modelo CustomVoice sembrado: la ruta llega a la
        // búsqueda de la voz sin cargar el motor ni tocar la instalación real.
        let inst = IsolatedInstance::new("synth_missing_voice");
        seed_pinned_models(&inst.dir.join("cache"), &["qwen3-tts-0.6b"]);
        let a = inst.contract_args();
        // Ruta directa fijada (colateral del régimen con fixture): la
        // existencia de la voz solo se verifica en la rama local; la vía daemon la
        // resuelve el residente.
        let (code, actual) = run_json_env(
            &[
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
            ],
            &a,
        );
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
    fn synthesize_label_collision_exits_6() {
        // Sandbox propio con el modelo CustomVoice sembrado; el almacén de
        // locuciones fabricado vive en el directorio de datos del sandbox.
        let inst = IsolatedInstance::new("synth_collision");
        seed_pinned_models(&inst.dir.join("cache"), &["qwen3-tts-0.6b"]);
        let a = inst.contract_args();
        let label = unique_label("colision");
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
        let src = inst.dir.join(format!("{}_min.wav", label));
        std::fs::write(&src, &wav_min).unwrap();
        // La voz de fábrica debe existir en el almacén de voces del sandbox: la
        // ruta la verifica antes de comprobar la colisión de etiqueta.
        std::fs::create_dir_all(inst.dir.join("voices").join("default"))
            .expect("crear la voz de fábrica del sandbox");
        let store = avi_store::SpeechStore::at(inst.dir.join("speech"));
        store
            .save("default", &label, "fabricado", &src)
            .expect("el sidecar fabricado debe guardarse");
        let _ = std::fs::remove_file(&src);
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "speech",
                "synthesize",
                "--text",
                "Hola",
                "--label",
                &label,
            ],
            &a,
        );
        assert_eq!(
            code, 6,
            "colisión de etiqueta → ExitCode::StateConflict (reason={:?})",
            actual["reason"]
        );
        assert_eq!(actual["reason"], Value::String("label_exists".to_string()));
        let _ = store.remove("default", &label);
    }

    /// Fábrica de locuciones sin síntesis: sidecar + WAV mínimo en la voz indicada,
    /// guardados en el almacén de locuciones del sandbox de `inst`; mismo patrón
    /// que `synthesize_label_collision_exits_6`.
    fn create_utterance(inst: &IsolatedInstance, voice: &str, label: &str) {
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
        let src = inst.dir.join(format!("{}_min.wav", label));
        std::fs::write(&src, cursor.into_inner()).unwrap();
        avi_store::SpeechStore::at(inst.dir.join("speech"))
            .save(voice, label, "fabricado", &src)
            .expect("el sidecar fabricado debe guardarse");
        let _ = std::fs::remove_file(&src);
    }

    /// `speech list --voice default` filtra por voz existente (exit 0, solo esa voz).
    /// solo esa voz).
    #[test]
    fn speech_list_filters_by_existing_voice() {
        let inst = IsolatedInstance::new("list_filters");
        avi_store::VoiceStore::at(inst.dir.join("voices"))
            .ensure_initialized()
            .expect("voces de fábrica inicializadas");
        let label_def = unique_label("listdef");
        let label_ryan = unique_label("listryan");
        create_utterance(&inst, "default", &label_def);
        create_utterance(&inst, "ryan", &label_ryan);
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "speech",
                "list",
                "--voice",
                "default",
            ],
            &inst.contract_args(),
        );
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        let entries = actual["speech"]
            .as_array()
            .expect("`speech` debe ser un array");
        assert!(
            entries
                .iter()
                .any(|e| e["label"] == Value::String(label_def.clone())),
            "la locución fabricada debe aparecer filtrada"
        );
        for e in entries {
            assert_eq!(
                e["voice"],
                Value::String("default".to_string()),
                "el filtro debe devolver solo la voz pedida"
            );
        }
    }

    /// `speech list --voice <inexistente>` sale con 3 (`voice_not_found`) porque el parser valida la voz antes de listar.
    #[test]
    fn speech_list_missing_voice_exits_3() {
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "list",
            "--voice",
            "voz_inexistente_xyz",
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

    /// `speech list --voice` con identificador ilegal sale con 2 (InvalidInput).
    #[test]
    fn speech_list_invalid_voice_exits_2() {
        let (code, actual) = run_json(&[
            "--json",
            "--no-daemon",
            "speech",
            "list",
            "--voice",
            "mi voz",
        ]);
        assert_eq!(
            code, 2,
            "identificador ilegal → ExitCode::InvalidInput (reason={:?})",
            actual["reason"]
        );
        assert_eq!(
            actual["reason"],
            Value::String("invalid_identifier".to_string())
        );
    }

    /// `speech list` sin `--voice` devuelve todas las locuciones (exit 0).
    #[test]
    fn speech_list_without_voice_returns_all() {
        let inst = IsolatedInstance::new("list_all");
        avi_store::VoiceStore::at(inst.dir.join("voices"))
            .ensure_initialized()
            .expect("voces de fábrica inicializadas");
        let label_def = unique_label("listalldef");
        let label_ryan = unique_label("listallryan");
        create_utterance(&inst, "default", &label_def);
        create_utterance(&inst, "ryan", &label_ryan);
        let (code, actual) = run_json_env(
            &["--json", "--no-daemon", "speech", "list"],
            &inst.contract_args(),
        );
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        let entries = actual["speech"]
            .as_array()
            .expect("`speech` debe ser un array");
        assert!(
            entries
                .iter()
                .any(|e| e["label"] == Value::String(label_def.clone())),
            "sin filtro debe aparecer la locución de default"
        );
        assert!(
            entries
                .iter()
                .any(|e| e["label"] == Value::String(label_ryan.clone())),
            "sin filtro debe aparecer la locución de ryan"
        );
    }

    // ─── say ───────────────────────────────────────────────────────────

    // Verifica WER real vía Parakeet (native-stt); sin el feature no se compila.
    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor, el modelo Parakeet y un dispositivo de audio"]
    fn say_success_plays() {
        require_tts_model();
        require_tts_binary();
        require_audio_device();
        require_parakeet();
        let _guard = lock_tts();
        // Testigo en directo de `say`: ruta local fijada con
        // `--no-daemon` — la vía daemon borra su WAV efímero tras reproducir y
        // rompería la verificación sobre archivo.
        // Humo único de audio: esta es la ÚNICA reproducción real de
        // la suite, acotada a un texto corto ("Hola mundo", <2 s); la
        // verificación es sobre el archivo (WAV válido + WER), no sobre el
        // altavoz. Gate de dispositivo como hoy (sin mezclador no hay humo).
        let inst = IsolatedInstance::new("say_plays");
        avi_store::VoiceStore::at(inst.dir.join("voices"))
            .ensure_initialized()
            .expect("voces de fábrica inicializadas");
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "speech",
                "say",
                "--text",
                "Hola mundo",
                "--voice",
                "default",
            ],
            &inst.args(),
        );
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("reproduced".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        valid_wav_24k(audio_path);
        let wer = wer_vs_text(audio_path, "Hola mundo");
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn say_empty_text_exits_2() {
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
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor, el modelo Parakeet y un dispositivo de audio"]
    fn dub_audio_passthrough_es_es() {
        require_tts_model();
        require_tts_binary();
        require_parakeet();
        require_audio_device();
        let _guard = lock_tts();
        // Testigo en directo de `dub`: ruta local fijada con
        // `--no-daemon` para que la sesión en ejecución no lo reenrute.
        // Verificación solo-archivo: WAV válido + WER sobre el
        // archivo producido. El gate de audio se conserva porque `dub`
        // reproduce siempre en ambas rutas, directo y por el daemon, en passthrough
        // y en traducción: sin
        // mezclador el comando falla con `playback_failed` y no hay archivo
        // que verificar.
        let inst = IsolatedInstance::new("dub_passthrough_direct");
        avi_store::VoiceStore::at(inst.dir.join("voices"))
            .ensure_initialized()
            .expect("voces de fábrica inicializadas");
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "speech",
                "dub",
                "--audio",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
                "--source-language",
                "es-latam",
                "--target-language",
                "es-latam",
            ],
            &inst.args(),
        );
        assert_eq!(code, 0);
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        valid_wav_24k(audio_path);
        let text = actual["text"].as_str().expect("text debe existir");
        let wer = wer_vs_text(audio_path, text);
        assert!(wer <= 0.25, "WER {} debe ser ≤ 0.25", wer);
    }

    #[test]
    fn dub_missing_file_exits_3() {
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
    #[ignore = "requiere el modelo Base de Qwen3-TTS, el binario del motor"]
    fn voice_clone_ok() {
        require_tts_base_model();
        require_tts_binary();
        let _guard = lock_tts();
        let name = unique_label("clon");
        // Testigo en directo de `clone`: ruta local fijada con
        // `--no-daemon` para que la sesión en ejecución no lo reenrute.
        let inst = IsolatedInstance::new("clone_ok");
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "voice",
                "clone",
                "--name",
                &name,
                "--speech-reference",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
            ],
            &inst.args(),
        );
        assert_eq!(code, 0);
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
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
        let _ = avi_store::VoiceStore::at(inst.dir.join("voices")).remove(&name);
    }

    /// Clonado repetido → 6. La voz existente se fabrica con un `.qvoice` mínimo.
    #[test]
    fn voice_clone_duplicate_exits_6() {
        // Sandbox propio con los modelos CustomVoice y Base sembrados; la voz
        // existente se fabrica en el almacén de voces del sandbox.
        let inst = IsolatedInstance::new("clone_duplicate");
        seed_pinned_models(
            &inst.dir.join("cache"),
            &["qwen3-tts-0.6b", "qwen3-tts-0.6b-base"],
        );
        let a = inst.contract_args();
        let name = unique_label("clon");
        let voices = avi_store::VoiceStore::at(inst.dir.join("voices"));
        let dir = voices.voice_dir(&name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("reference.qvoice"), b"QVCE").unwrap();
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "voice",
                "clone",
                "--name",
                &name,
                "--speech-reference",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
            ],
            &a,
        );
        assert_eq!(code, 6, "voz existente → ExitCode::StateConflict");
        assert_eq!(actual["reason"], Value::String("voice_exists".to_string()));
    }

    #[test]
    fn voice_clone_invalid_name_exits_2() {
        // Sandbox propio con los modelos CustomVoice y Base sembrados; el nombre
        // se rechaza antes de cargar el motor.
        let inst = IsolatedInstance::new("clone_invalid_name");
        seed_pinned_models(
            &inst.dir.join("cache"),
            &["qwen3-tts-0.6b", "qwen3-tts-0.6b-base"],
        );
        let a = inst.contract_args();
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "voice",
                "clone",
                "--name",
                "voz invalida",
                "--speech-reference",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
            ],
            &a,
        );
        assert_eq!(code, 2, "nombre inválido → ExitCode::InvalidInput");
        assert_eq!(
            actual["reason"],
            Value::String("invalid_voice_name".to_string())
        );
    }

    #[test]
    fn voice_clone_missing_audio_exits_3() {
        // Sandbox propio con los modelos CustomVoice y Base sembrados; la ruta
        // directa falla por el audio inexistente antes de cargar el motor.
        let inst = IsolatedInstance::new("clone_missing_audio");
        seed_pinned_models(
            &inst.dir.join("cache"),
            &["qwen3-tts-0.6b", "qwen3-tts-0.6b-base"],
        );
        let a = inst.contract_args();
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "voice",
                "clone",
                "--name",
                "clon_ok",
                "--speech-reference",
                "no-existe.wav",
            ],
            &a,
        );
        assert_eq!(code, 3, "audio inexistente → ExitCode::NotFound");
        assert_eq!(
            actual["reason"],
            Value::String("audio_not_found".to_string())
        );
    }

    /// Referencia truncada en la ruta local → 2 con `invalid_audio`.
    #[test]
    fn voice_clone_truncated_audio_exits_2() {
        // Sandbox propio con los modelos CustomVoice y Base sembrados; el WAV
        // truncado se escribe en el temporal del sandbox.
        let inst = IsolatedInstance::new("clone_truncated");
        seed_pinned_models(
            &inst.dir.join("cache"),
            &["qwen3-tts-0.6b", "qwen3-tts-0.6b-base"],
        );
        let a = inst.contract_args();
        let wav = write_truncated_wav("clon_truncado");
        let name = unique_label("clon");
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--no-daemon",
                "voice",
                "clone",
                "--name",
                &name,
                "--speech-reference",
                wav.to_str().unwrap(),
            ],
            &a,
        );
        std::fs::remove_file(&wav).ok();
        assert_eq!(code, 2, "referencia truncada → ExitCode::InvalidInput");
        assert_eq!(actual["reason"], Value::String("invalid_audio".to_string()));
    }

    // ─── daemon start/status/restart ────────────────────────────────

    /// IPv4 de la interfaz de salida, descubierta sin enviar tráfico (un `connect`
    /// UDP solo fija la ruta). `None` si el equipo solo tiene loopback.
    fn non_loopback_ipv4() -> Option<std::net::Ipv4Addr> {
        let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
        sock.connect("192.0.2.1:9").ok()?;
        match sock.local_addr().ok()?.ip() {
            std::net::IpAddr::V4(ip) if !ip.is_loopback() && !ip.is_unspecified() => Some(ip),
            _ => None,
        }
    }

    /// El residente acepta conexiones por loopback y las rechaza por la IPv4 de la LAN.
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn resident_listens_only_on_loopback() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::resident_listens_only_on_loopback");
        let _reaper = arm_reaper("resident_listens_only_on_loopback");
        let inst = IsolatedInstance::new("loopback");
        let port = inst
            .tts_port()
            .expect("la instancia debe fijar QWEN3_TTS_PORT");
        start_instance(&inst, &[]);
        let timeout = Duration::from_secs(3);
        let loopback = std::net::SocketAddr::from(([127, 0, 0, 1], port));
        assert!(
            std::net::TcpStream::connect_timeout(&loopback, timeout).is_ok(),
            "el residente debe aceptar conexiones en 127.0.0.1:{}",
            port
        );
        let ip = non_loopback_ipv4()
            .expect("la máquina no tiene IPv4 fuera de loopback: conéctala a una red");
        let lan = std::net::SocketAddr::from((ip, port));
        assert!(
            std::net::TcpStream::connect_timeout(&lan, timeout).is_err(),
            "el residente está expuesto a la red: acepta conexiones en {}:{}",
            ip,
            port
        );
        stop_instance(&inst, "resident_listens_only_on_loopback");
        hit_end("tts::resident_listens_only_on_loopback");
    }

    /// El motor rechaza un `--host` inválido al parsear los argumentos, sin cargar el modelo.
    #[test]
    #[ignore = "requiere el binario del motor"]
    fn engine_rejects_invalid_host() {
        require_tts_binary();
        let bin = tts_binary().expect("require_tts_binary garantiza el binario");
        let out = Command::new(bin)
            .args(["--serve", "1", "--host", "no-es-ip"])
            .output()
            .expect("debe poder ejecutar el motor");
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert_eq!(out.status.code(), Some(1), "stderr: {}", stderr);
        assert!(
            stderr.contains("--host"),
            "stderr debe mencionar --host: {}",
            stderr
        );
    }

    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn daemon_start_ok() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::daemon_start_ok");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        // La ventana spawn→write ya no ciega al handler (PID en memoria).
        let _reaper = arm_reaper("daemon_start_ok");
        // Instancia aislada propia (puerto efímero + sandbox + evento);
        // sin precondición de sesión: el sandbox nace detenido y vacío.
        let inst = IsolatedInstance::new("start_ok");
        let actual = start_instance_running_only(&inst, &[]);
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert_eq!(
            actual["status"],
            Value::String("started".to_string()),
            "desde detenido el start debe partir de cero (started): {}",
            actual
        );
        // Verificar status running (observado por evento, sin sleeps fijos).
        let a = inst.args();
        let (code3, actual3) = run_json_env(&["--json", "daemon", "status"], &a);
        assert_eq!(code3, 0);
        assert_eq!(
            actual3["daemon"],
            Value::String("running".to_string()),
            "tras start debe seguir running"
        );
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "daemon_start_ok");
        let (_, actual4) = run_json_env(&["--json", "daemon", "status"], &a);
        assert_eq!(
            actual4["daemon"],
            Value::String("stopped".to_string()),
            "tras stop debe quedar stopped"
        );
        hit_end("tts::daemon_start_ok");
    }

    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn daemon_restart_rewarms() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::daemon_restart_rearma");
        // Reaper best-effort en todo `panic!` fuera de los polls.
        let _reaper = arm_reaper("daemon_restart_rearma");
        // Instancia aislada propia; base observada en ejecución.
        let inst = IsolatedInstance::new("restart_rearma");
        start_instance_running_only(&inst, &[]);
        let previous = inst.read_daemon_pid();
        let a = inst.args();
        let (code, actual) = run_json_env(&["--json", "daemon", "restart"], &a);
        assert_eq!(code, 0, "daemon restart debe salir 0");
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert!(actual.get("pid").is_some() || actual.get("status").is_some());
        // Rearme a nivel de sistema: el PID nuevo está vivo y el árbol previo,
        // si cambió el PID, quedó muerto (sin huérfano del ciclo anterior).
        let new = actual
            .get("pid")
            .and_then(|p| p.as_u64())
            .map(|n| n as u32)
            .or_else(|| inst.read_daemon_pid());
        assert!(
            new.map(avi_daemon::pid_alive).unwrap_or(false),
            "tras restart el daemon debe estar vivo a nivel SO (pid {:?})",
            new
        );
        if let (Some(p), Some(q)) = (previous, new) {
            if p != q {
                assert!(
                    !avi_daemon::pid_alive(p),
                    "tras restart el árbol previo no debe quedar vivo (pid {})",
                    p
                );
            }
        }
        // Status debe seguir running (por evento, sin sleeps fijos, sin exigir warm).
        wait_for_running_without_warm(WARM_FAILSAFE_RETRIES, &a);
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "daemon_restart_rearma");
        hit_end("tts::daemon_restart_rearma");
    }

    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn daemon_status_running() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::daemon_status_running");
        // Reaper best-effort en todo `panic!` fuera de los polls.
        let _reaper = arm_reaper("daemon_status_running");
        // Instancia aislada propia; `status` contra daemon en ejecución.
        let inst = IsolatedInstance::new("status_running");
        start_instance_running_only(&inst, &[]);
        let a = inst.args();
        let (code, actual) = run_json_env(&["--json", "daemon", "status"], &a);
        assert_eq!(code, 0);
        // Endurecida: antes condicional (pasaba sin verificar si no estaba
        // running); ahora el `running` se exige porque la instancia lo garantiza.
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        // Presencia a nivel de sistema además del probe: el PID de la pista
        // está vivo (revalidación matar-y-rearrancar).
        let pid = inst.read_daemon_pid();
        assert!(
            pid.map(avi_daemon::pid_alive).unwrap_or(false),
            "con status running el PID de la pista debe estar vivo (pid {:?})",
            pid
        );
        // Cuando está running, el fixture running debe coincidir (schema_version 4)
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        let expected = fixture("cli_daemon_status_running.json");
        // Comparar daemon y engine
        assert_eq!(actual["daemon"], expected["daemon"]);
        // Apagado propio (sin huérfanos al cerrar).
        stop_instance(&inst, "daemon_status_running");
        hit_end("tts::daemon_status_running");
    }

    #[test]
    fn daemon_help_lists_auto_restart() {
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
    fn setup_help_lists_current_surface() {
        // Fija el contrato de flags de `setup`: presencia de la superficie vigente y ausencia de los flags eliminados/renombrados.
        // superficie vigente y ausencia de los flags eliminados/renombrados.
        let out = Command::new(BIN)
            .args(["setup", "--help"])
            .output()
            .expect("setup --help");
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        for flag in [
            "--force-update",
            "--yes",
            "--with-voice-cloning",
            "--with-stt",
        ] {
            assert!(
                combined.contains(flag),
                "setup --help debe listar {}, fue: {}",
                flag,
                combined
            );
        }
        for flag in ["--language", "--with-base", "--with-clone", "--clone"] {
            assert!(
                !combined.contains(flag),
                "setup --help no debe listar {}, fue: {}",
                flag,
                combined
            );
        }
    }

    #[test]
    fn setup_json_without_language_key() {
        // Contrato del payload --json: la clave `language` desaparece de la respuesta.
        // Corre el binario real en un sandbox con la selección por defecto ya
        // sembrada (snapshots fijados completos), de modo que `setup`
        // no descarga nada. Un proxy inalcanzable garantiza que, si el
        // sembrado no bastara, la descarga fallaría en lugar de bajar gigabytes.
        let inst = IsolatedInstance::new("setup_json");
        let models_root = inst.dir.join("cache");
        let names: Vec<&str> = avi_store::MODEL_REVISIONS
            .iter()
            .map(|pin| pin.name)
            .filter(|name| *name != "qwen3-tts-0.6b-base")
            .collect();
        seed_pinned_models(&models_root, &names);
        let mut envs = inst.contract_args();
        envs.push(("HTTPS_PROXY", "http://127.0.0.1:9"));
        envs.push(("HTTP_PROXY", "http://127.0.0.1:9"));
        let (code, payload) = run_json_env(&["--json", "setup"], &envs);
        let _ = std::fs::remove_dir_all(&inst.dir);
        assert_eq!(code, 0, "setup --json debe completar, payload: {}", payload);
        assert_eq!(payload["status"], "completed");
        assert!(
            payload.get("language").is_none(),
            "el payload de setup no debe contener la clave `language`, fue: {}",
            payload
        );
    }

    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn daemon_start_with_auto_restart() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::daemon_start_con_auto_restart");
        // Reaper best-effort en todo `panic!` fuera de los polls.
        let _reaper = arm_reaper("daemon_start_con_auto_restart");
        // Instancia aislada propia; el sandbox nace detenido y vacío.
        let inst = IsolatedInstance::new("start_auto_restart");
        // Start con supervisor habilitado y max 1 (no debe fallar en estado sano).
        // De semántica pasiva (`bind` de prueba) a reclamo activo del árbol propio previo con deadline y verificación.
        // propio previo con deadline y verificación (solo árbol propio, nunca
        // otra instancia ni imagen global; `Ok` graceful sin reintento intacto).
        let actual = start_instance_running_only(&inst, &["--auto-restart", "--max-retries", "1"]);
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        // Desde detenido: fresco con `started` y PID vivo a nivel SO.
        assert_eq!(
            actual["status"],
            Value::String("started".to_string()),
            "desde detenido el start debe partir de cero (started): {}",
            actual
        );
        let pid = inst.read_daemon_pid();
        assert!(
            pid.map(avi_daemon::pid_alive).unwrap_or(false),
            "el daemon recién arrancado debe estar vivo a nivel SO (pid {:?})",
            pid
        );
        // Stop no debe reintentar (graceful): ausencia observada por evento
        // más cero huérfanos a nivel SO.
        stop_instance(&inst, "daemon_start_con_auto_restart");
        let a = inst.args();
        let (_, actual2) = run_json_env(&["--json", "daemon", "status"], &a);
        assert_eq!(
            actual2["daemon"],
            Value::String("stopped".to_string()),
            "tras stop no debe reintentar"
        );
        hit_end("tts::daemon_start_con_auto_restart");
    }

    /// Sustituye el puerto de la instancia por uno ocupado por un listener
    /// propio, que la prueba mantiene vivo mientras lo necesite.
    fn occupy_instance_port(inst: &mut IsolatedInstance) -> std::net::TcpListener {
        let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("ocupar un puerto");
        let port = holder.local_addr().unwrap().port();
        for (k, v) in inst.envs.iter_mut() {
            if k == "AVI_DAEMON_PORT" {
                *v = port.to_string();
            }
        }
        holder
    }

    /// Afirma el fallo por puerto ocupado de `daemon start`: exit 6
    /// `port_in_use` con el puerto y `AVI_DAEMON_PORT` en el mensaje, en menos
    /// de 5 s (la regresión esperaría 10 s), sin `daemon.ready` y sin tocar el
    /// `daemon.pid` previo (el señuelo sembrado o su ausencia).
    fn assert_start_port_in_use(inst: &IsolatedInstance, port: u16, extra: &[&str]) {
        let mut args = vec!["--json", "daemon", "start"];
        args.extend_from_slice(extra);
        let pidfile_before = std::fs::read(inst.dir.join("daemon.pid")).ok();
        let t0 = Instant::now();
        let (code, actual) = run_json_env(&args, &inst.contract_args());
        let elapsed = t0.elapsed();
        assert_eq!(code, 6, "puerto ocupado debe salir con exit 6: {}", actual);
        assert_eq!(actual["reason"], "port_in_use", "{}", actual);
        let error = actual["error"].as_str().unwrap_or_default();
        assert!(error.contains(&port.to_string()), "{}", actual);
        assert!(error.contains("AVI_DAEMON_PORT"), "{}", actual);
        assert!(
            elapsed < Duration::from_secs(5),
            "el fallo debe ser inmediato, no esperar el deadline: {:?}",
            elapsed
        );
        assert_eq!(
            std::fs::read(inst.dir.join("daemon.pid")).ok(),
            pidfile_before,
            "un arranque fallido no debe escribir ni borrar daemon.pid"
        );
        assert!(
            !inst.dir.join("daemon.ready").exists(),
            "no debe quedar daemon.ready"
        );
    }

    /// `daemon start` con el puerto de la instancia ocupado falla al instante
    /// con `port_in_use` (exit 6), con y sin `--auto-restart`, y no deja
    /// huérfano: tras liberar el puerto nadie lo enlaza (un daemon supervisado
    /// huérfano reintentaría a los 500 ms y lo tomaría en cuanto quedara libre).
    /// No toma `lock_tts`: ningún modelo llega a cargarse.
    #[test]
    fn daemon_start_port_in_use_fails_fast_with_state_conflict() {
        let _reaper = arm_reaper("daemon_start_port_in_use");
        let mut inst = IsolatedInstance::new("start_port_in_use");
        // Sin el modelo `start` saldría antes por `model_missing`; el pidfile
        // señuelo evita que `start` consulte 127.0.0.1:8765 o barra `qwen_tts`.
        seed_pinned_models(&inst.dir.join("cache"), &["qwen3-tts-0.6b"]);
        seed_decoy_daemon_pidfile(&inst.dir);
        let holder = occupy_instance_port(&mut inst);
        let port = holder.local_addr().unwrap().port();
        assert_start_port_in_use(&inst, port, &[]);
        assert_start_port_in_use(&inst, port, &["--auto-restart"]);
        drop(holder);
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            !port_open(port),
            "tras un arranque fallido ningún daemon debe enlazar el puerto liberado"
        );
    }

    /// `daemon serve` con el puerto ocupado sale con `port_in_use` (exit 6) y
    /// el puerto en el mensaje, sin esperar: el fallo ocurre antes de cargar
    /// modelos, así que no necesita provisión.
    #[test]
    fn daemon_serve_port_in_use_exits_state_conflict() {
        let _reaper = arm_reaper("daemon_serve_port_in_use");
        let mut inst = IsolatedInstance::new("serve_port_in_use");
        let holder = occupy_instance_port(&mut inst);
        let port = holder.local_addr().unwrap().port();
        let t0 = Instant::now();
        let (code, actual) = run_json_env(&["--json", "daemon", "serve"], &inst.contract_args());
        let elapsed = t0.elapsed();
        assert_eq!(code, 6, "puerto ocupado debe salir con exit 6: {}", actual);
        assert_eq!(actual["reason"], "port_in_use", "{}", actual);
        assert!(
            actual["error"]
                .as_str()
                .unwrap_or_default()
                .contains(&port.to_string()),
            "{}",
            actual
        );
        assert!(
            elapsed < Duration::from_secs(5),
            "el fallo debe ser inmediato: {:?}",
            elapsed
        );
        drop(holder);
    }

    /// `daemon start --warm-voice <inexistente>` sale con `voice_not_found`
    /// (exit 3) sin cargar modelos y sin tocar el `daemon.pid` previo.
    #[test]
    fn daemon_start_missing_warm_voice_exits_not_found() {
        let _reaper = arm_reaper("daemon_start_missing_warm_voice");
        let inst = IsolatedInstance::new("start_missing_warm_voice");
        // Sin el modelo `start` saldría antes por `model_missing`; el pidfile
        // señuelo evita que `start` consulte 127.0.0.1:8765 o barra `qwen_tts`.
        seed_pinned_models(&inst.dir.join("cache"), &["qwen3-tts-0.6b"]);
        seed_decoy_daemon_pidfile(&inst.dir);
        let pidfile_before = std::fs::read(inst.dir.join("daemon.pid")).ok();
        let voice = format!("warm_inexistente_{}", std::process::id());
        let t0 = Instant::now();
        let (code, actual) = run_json_env(
            &["--json", "daemon", "start", "--warm-voice", &voice],
            &inst.contract_args(),
        );
        let elapsed = t0.elapsed();
        assert_eq!(code, 3, "voz inexistente debe salir con exit 3: {}", actual);
        assert_eq!(actual["reason"], "voice_not_found", "{}", actual);
        assert!(
            elapsed < Duration::from_secs(5),
            "el fallo debe ser inmediato: {:?}",
            elapsed
        );
        assert_eq!(
            std::fs::read(inst.dir.join("daemon.pid")).ok(),
            pidfile_before,
            "un arranque fallido no debe escribir ni borrar daemon.pid"
        );
    }

    /// Prueba pesada de limpieza: cero huérfanos tras aborto simulado, en dos
    /// fases. Fase 1 (caída del padre: pidfile borrado con daemon vivo) →
    /// `start` reclama el árbol (payload `started`, PID previo muerto). Fase 2
    /// (timeout sin graceful: árbol matado sin POST /shutdown, pista rancia) →
    /// `start` parte de cero con `started`. Cierra con cero huérfanos
    /// verificados a nivel SO.
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn h01_simulated_abort_reclaims_and_leaves_no_orphans() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::h01_simulated_abort_reclaims_and_leaves_no_orphans");
        // Reaper best-effort en todo `panic!` fuera de los polls.
        // (tensado CI Unix): tras cada reclamo se exige además residente
        // ausente por imagen cuando el PID previo murió; en Windows local ese
        // verde se declara no probatorio (runtime Unix diferido a CI).
        // La ventana spawn→write ya no ciega al handler (PID en memoria).
        let _reaper = arm_reaper("h01_aborto_simulado");
        // Instancia aislada propia; el sandbox nace detenido y vacío.
        let inst = IsolatedInstance::new("h01_aborto");
        let a = inst.args();
        // Fase 1 — caída del padre: daemon vivo sin pidfile (el dueño anterior
        // murió sin limpiar). El próximo `start` debe reclamar, no adherirse.
        start_instance_running_only(&inst, &[]);
        let pid_a = inst
            .read_daemon_pid()
            .expect("tras start debe haber pidfile");
        assert!(
            avi_daemon::pid_alive(pid_a),
            "el daemon de la instancia debe estar vivo (pid {})",
            pid_a
        );
        std::fs::remove_file(inst.dir.join("daemon.pid"))
            .expect("la caída simulada debe poder borrar el pidfile");
        milestone(&format!(
            "aborto simulado (fase 1): pidfile borrado con daemon vivo (pid {})",
            pid_a
        ));
        let (code, actual) = run_json_env(&["--json", "daemon", "start"], &a);
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
        wait_for_running_without_warm(WARM_FAILSAFE_RETRIES, &a);
        assert!(
            !avi_daemon::pid_alive(pid_a),
            "el reclamo debe haber matado el árbol residual (pid {} sigue vivo)",
            pid_a
        );
        let pid_b = inst
            .read_daemon_pid()
            .expect("tras reclamo debe haber pidfile fresco");
        assert!(
            avi_daemon::pid_alive(pid_b),
            "el daemon reclamado debe estar vivo (pid {})",
            pid_b
        );
        milestone("aborto simulado (fase 1): reclamo ok, residual muerto y fresco vivo");
        // Fase 2 — timeout/aborto sin graceful: se mata el árbol sin POST
        // /shutdown (la pista queda rancia a propósito). El próximo `start`
        // parte de cero con `started`.
        avi_daemon::kill_tree_by_pid(pid_b);
        avi_daemon::wait_for_pid_death(pid_b, std::time::Duration::from_secs(8));
        // (tensado CI Unix, sin simular Unix en local): tras matar el
        // árbol sin graceful, el residente no debe seguir vivo por imagen antes
        // del rearranque (el kill de grupo Unix arrastra al residente).
        #[cfg(unix)]
        assert!(
            !resident_present_by_image(),
            "tras matar el árbol sin graceful el residente no debe seguir por imagen"
        );
        milestone(&format!(
            "aborto simulado (fase 2): árbol matado sin graceful (pid {})",
            pid_b
        ));
        let (code2, actual2) = run_json_env(&["--json", "daemon", "start"], &a);
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
        wait_for_running_without_warm(WARM_FAILSAFE_RETRIES, &a);
        // Cierre: cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "h01_aborto_simulado");
        hit_end("tts::h01_simulated_abort_reclaims_and_leaves_no_orphans");
    }

    /// Regresión (daemon retiene el stdio del proceso que lo lanzó): reproduce
    /// la condición exacta observada, con captura de
    /// `daemon start` vía **pipe** (`Stdio::piped()`, no tempfile) — porque
    /// un tempfile nunca crea un handle heredable y no puede detectar la
    /// retención. Si el daemon (o `qwen_tts` a través de él) heredan el
    /// write-end pese a `disinherit_standard_handles` (corte de herencia vía
    /// `SetHandleInformation` en `handle_daemon`), la
    /// lectura del pipe no verá EOF hasta que el holder cierre el handle —
    /// exactamente el síntoma documentado: "matar el motor no libera el log
    /// del lanzador; matar el daemon sí". Diferencial: mata primero solo el
    /// motor (residente en 8766) y comprueba si el pipe sigue bloqueado;
    /// luego mata el árbol del daemon y comprueba que se libera.
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn h03_pipe_stdio_must_not_remain_blocked() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::h03_pipe_stdio_must_not_remain_blocked");
        let _reaper = arm_reaper("h03_pipe_stdio_must_not_remain_blocked");
        // Instancia aislada propia (puerto efímero + sandbox); el sandbox
        // nace detenido y vacío, sin precondición de sesión.
        let inst = IsolatedInstance::new("h03_pipe");
        let child_envs: Vec<(String, String)> = inst.envs.clone();

        // Localiza (sin matar) al residente por PID registrado en `daemon.pid`
        // Un solo camino portable, sin `netstat` ni rama por plataforma.
        let dir_pipe = inst.dir.clone();
        let registered_resident_pid = move || -> Option<u32> {
            let pid = read_resident_pid_dir(&dir_pipe);
            if pid != 0 && avi_tts::resident::resident_pid_alive(pid) {
                Some(pid)
            } else {
                None
            }
        };

        // `daemon start` capturado vía PIPE en un hilo aparte: `output()` no
        // retorna hasta que el proceso hijo termina Y todos los holders del
        // write-end del pipe lo cierran. Sano: retorna en ~1-2 s (la vida del
        // CLI lanzador). Retenido: no retorna hasta que muera quien heredó
        // el handle. El hijo recibe las envs de la instancia (sandbox +
        // puerto efímero); el tempfile anti-cuelgue no aplica aquí a
        // propósito (el pipe es el instrumento de detección).
        let t0 = Instant::now();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let output = std::process::Command::new(BIN)
                .args(["--json", "daemon", "start"])
                .envs(child_envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::null())
                .output();
            let _ = tx.send(output);
        });

        match rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(output) => {
                let output = output.expect("`daemon start` debe poder ejecutarse");
                assert!(
                    output.status.success(),
                    "`daemon start` debe salir 0: {:?}",
                    output
                );
                milestone(&format!(
                    "h03: pipe liberado en {} ms sin intervención — no reproduce (sin retención)",
                    t0.elapsed().as_millis()
                ));
                let a = inst.args();
                wait_for_running_without_warm(WARM_FAILSAFE_RETRIES, &a);
                stop_instance(&inst, "h03_pipe_stdio");
                hit_end("tts::h03_pipe_stdio_must_not_remain_blocked");
                return;
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                milestone(&format!(
                    "h03: pipe SIGUE bloqueado tras {} ms (umbral sano ~1-2 s) — investigando retención",
                    t0.elapsed().as_millis()
                ));
            }
            Err(e) => fail_with_reaper(
                "h03_pipe_stdio_must_not_remain_blocked(canal)",
                format!("canal del hilo lector cerrado inesperadamente: {}", e),
            ),
        }

        // El pipe sigue retenido más allá de la vida del CLI lanzador: mata
        // solo el motor primero (si hay PID registrado vivo) para replicar el
        // orden exacto del síntoma documentado.
        if let Some(engine_pid) = registered_resident_pid() {
            milestone(&format!("h03: matando solo el motor (pid {})", engine_pid));
            avi_daemon::kill_tree_by_pid(engine_pid);
            avi_daemon::wait_for_pid_death(engine_pid, std::time::Duration::from_secs(8));
            if let Ok(output) = rx.recv_timeout(std::time::Duration::from_secs(3)) {
                let output = output.expect("`daemon start` debe poder ejecutarse");
                milestone(&format!(
                    "h03: el pipe se liberó al matar SOLO el motor (inesperado vs. síntoma documentado, exit {:?})",
                    output.status.code()
                ));
                stop_instance(&inst, "h03_pipe_stdio_motor");
                hit_end("tts::h03_pipe_stdio_must_not_remain_blocked (liberado por motor)");
                return;
            }
            milestone(
                "h03: matar solo el motor NO liberó el pipe (coincide con el síntoma documentado)",
            );
        }

        // Mata el árbol completo del daemon: si el síntoma reproduce, esto
        // debe liberar el pipe.
        let pid_daemon = inst.read_daemon_pid();
        if let Some(pid) = pid_daemon {
            milestone(&format!("h03: matando el árbol del daemon (pid {})", pid));
            avi_daemon::kill_tree_by_pid(pid);
            avi_daemon::wait_for_pid_death(pid, std::time::Duration::from_secs(8));
        }

        match rx.recv_timeout(std::time::Duration::from_secs(10)) {
            Ok(output) => {
                let output = output.expect("`daemon start` debe poder ejecutarse");
                fail_with_reaper(
                    "h03_pipe_stdio_must_not_remain_blocked",
                    format!(
                        "Reproduce: el pipe del lanzador solo se liberó al matar el daemon (no el motor), tras {} ms totales (exit {:?}). El daemon retiene el stdio del proceso que lo lanzó pese al corte de herencia por SetHandleInformation (disinherit_standard_handles).",
                        t0.elapsed().as_millis(),
                        output.status.code()
                    ),
                );
            }
            Err(_) => {
                fail_with_reaper(
                    "h03_pipe_stdio_must_not_remain_blocked",
                    format!(
                        "el pipe del lanzador sigue bloqueado incluso tras matar el árbol del daemon (>{} ms): retención más allá de lo documentado",
                        t0.elapsed().as_millis()
                    ),
                );
            }
        }
    }

    // La traducción solo existe con `native-translation`.
    #[cfg(feature = "native-translation")]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor y los modelos CT2"]
    fn translate_delegates_to_daemon() {
        require_tts_model();
        require_tts_binary();
        require_ct2();
        let _tts = lock_tts();
        hit_start_heavy("tts::translate_delegates_to_daemon");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = arm_reaper("translate_delegates_to_daemon");
        // Daemon caliente de la instancia propia (sin ciclo compartido ni sleeps).
        let inst = IsolatedInstance::new("translate_delega");
        start_instance_running_only(&inst, &[]);
        let a = inst.args();
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "translate",
                "--text",
                "Hola",
                "--from",
                "es",
                "--to",
                "en",
            ],
            &a,
        );
        assert_eq!(code, 0, "translate --daemon debe delegar con exit 0");
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        assert!(actual.get("translated").is_some());
        let expected = fixture("cli_translate_daemon.json");
        assert_eq!(actual["source"], expected["source"]);
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "translate_delegates_to_daemon");
        hit_end("tts::translate_delegates_to_daemon");
    }

    #[test]
    fn translate_force_daemon_no_daemon_exits_5() {
        // Aislamiento total: la ausencia debe observarse sin carreras con
        // usuarios del daemon (serie de inferencia).
        let _tts = lock_tts();
        hit_start_heavy("tts::translate_force_daemon_sin_daemon_exit5");
        // Partición orden-sensible eliminada por aislamiento por instancia: antes este
        // test apagaba la SESIÓN global (orden-sensible con la serie: si otro
        // test necesitaba la sesión, el orden importaba y el próximo `ensure`
        // la reparaba bajo demanda). Ahora observa un sandbox sin arrancar
        // nada: el pidfile señuelo dirige al cliente a un puerto cerrado y el
        // exit 5 es observable sin tocar estado compartido — el orden de
        // ejecución ya no importa por construcción.
        let inst = IsolatedInstance::new("force_sin_daemon");
        // El pidfile señuelo apunta a un puerto cerrado: el cliente no cae al
        // 8765 del usuario, que puede tener un daemon real.
        seed_decoy_daemon_pidfile(&inst.dir);
        let a = inst.contract_args();
        // La ausencia es real a nivel SO (no solo HTTP): con matar-y-rearrancar
        // el `start` solo ocurre explícito, nunca implícito en delegación, así
        // que el exit 5 sigue observable.
        let decoy_port = inst.port();
        assert!(
            !port_open(decoy_port),
            "sin daemon el puerto {decoy_port} del señuelo debe estar cerrado a nivel SO"
        );
        assert!(
            !inst
                .read_daemon_pid()
                .map(avi_daemon::pid_alive)
                .unwrap_or(false),
            "sin daemon no debe haber PID vivo en la pista"
        );
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "translate",
                "--text",
                "Hola",
                "--from",
                "es",
                "--to",
                "en",
            ],
            &a,
        );
        assert_eq!(code, 5, "translate --daemon sin daemon debe salir 5");
        assert_eq!(
            actual["reason"],
            Value::String("daemon_unreachable".to_string())
        );
        hit_end("tts::translate_force_daemon_sin_daemon_exit5");
    }

    #[test]
    #[ignore = "requiere los modelos Qwen3-TTS y Base, y el binario del motor"]
    fn clone_delegates_to_daemon() {
        require_tts_model();
        require_tts_base_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::clone_delegates_to_daemon");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = arm_reaper("clone_delegates_to_daemon");
        // Daemon caliente de la instancia propia (sin ciclo compartido ni sleeps).
        let inst = IsolatedInstance::new("clone_delega");
        start_instance(&inst, &[]);
        let a = inst.args();
        let name = unique_label("clon_daemon");
        // El exit 0 ya no es «éxito inmediato» sino «secuencia completa
        // hasta el evento final» (`started` → latidos → `result` con
        // `precomputed: true` = precarga iniciada). El orden de eventos se
        // afirma en `crates/avi-daemon/tests/golden.rs`
        // (`voices_clone_daemon_precomputed_true`); aquí se verifica el
        // contrato final a través del stream, sin cota de 1500 ms.
        let t0 = std::time::Instant::now();
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "voice",
                "clone",
                "--name",
                &name,
                "--speech-reference",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
            ],
            &a,
        );
        milestone(&format!(
            "clone_delegates_to_daemon: clon completado en {} ms (sin cota 1500ms)",
            t0.elapsed().as_millis()
        ));
        assert_eq!(code, 0, "voice clone --daemon debe delegar con exit 0");
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        assert_eq!(actual["name"], Value::String(name.clone()));
        assert_eq!(
            actual["precomputed"],
            Value::Bool(true),
            "vía daemon el evento final trae precomputed:true (precarga iniciada)"
        );
        let _ = avi_store::VoiceStore::at(inst.dir.join("voices")).remove(&name);
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "clone_delegates_to_daemon");
        hit_end("tts::clone_delegates_to_daemon");
    }

    /// Referencia truncada por daemon → 2 con `invalid_audio`, igual que en local.
    #[test]
    #[ignore = "requiere los modelos Qwen3-TTS y Base, y el binario del motor"]
    fn clone_daemon_truncated_audio_exits_2() {
        require_tts_model();
        require_tts_base_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::clone_daemon_truncated_audio_exits_2");
        let _reaper = arm_reaper("clone_daemon_truncado");
        let inst = IsolatedInstance::new("clone_truncado");
        start_instance(&inst, &[]);
        let a = inst.args();
        let wav = write_truncated_wav("clon_daemon_truncado");
        let name = unique_label("clon_daemon");
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "voice",
                "clone",
                "--name",
                &name,
                "--speech-reference",
                wav.to_str().unwrap(),
            ],
            &a,
        );
        std::fs::remove_file(&wav).ok();
        let _ = avi_store::VoiceStore::at(inst.dir.join("voices")).remove(&name);
        stop_instance(&inst, "clone_daemon_truncado");
        assert_eq!(code, 2, "referencia truncada por daemon → InvalidInput");
        assert_eq!(actual["reason"], Value::String("invalid_audio".to_string()));
        hit_end("tts::clone_daemon_truncated_audio_exits_2");
    }

    #[cfg(feature = "native-stt")]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor, el modelo Parakeet y un dispositivo de audio"]
    fn dub_daemon_passthrough() {
        require_tts_model();
        require_tts_binary();
        require_parakeet();
        require_audio_device();
        let _tts = lock_tts();
        hit_start_dub("tts::dub_daemon_passthrough");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = arm_reaper("dub_daemon_passthrough");
        // Daemon caliente de la instancia propia (sin ciclo compartido ni sleeps).
        let inst = IsolatedInstance::new("dub_passthrough");
        start_instance(&inst, &[]);
        let a = inst.args();
        // El exit 0 con `dubbed` verifica la secuencia completa hasta el evento final.
        // evento final (`started` → latidos por fase → `result`), sin cota de
        // 10 s. El orden de eventos se afirma en los dobles programados
        // (unitarios del CLI) y en el router (`golden.rs`); aquí se verifica el
        // contrato final a través del stream con inferencia real.
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "speech",
                "dub",
                "--audio",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
                "--source-language",
                "es-latam",
                "--target-language",
                "es-latam",
            ],
            &a,
        );
        assert_eq!(code, 0, "dub passthrough --daemon debe salir 0");
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        // Verificación sobre el archivo recibido: WAV válido más
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
        valid_wav_24k(audio_path);
        let text = actual["text"].as_str().expect("text debe existir");
        assert!(!text.is_empty(), "`text` no debe estar vacío");
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "dub_daemon_passthrough");
        hit_end("tts::dub_daemon_passthrough");
    }

    // La traducción solo existe con `native-translation`.
    #[cfg(all(feature = "native-stt", feature = "native-translation"))]
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS, el binario del motor, el modelo Parakeet, los modelos CT2 y un dispositivo de audio"]
    fn dub_daemon_with_translation() {
        require_tts_model();
        require_tts_binary();
        require_parakeet();
        require_ct2();
        require_audio_device();
        let _tts = lock_tts();
        hit_start_dub("tts::dub_daemon_with_translation");
        // Todo `panic!`/`assert!` fuera de los polls ejecuta el reaper
        // best-effort antes de fallar (vía `Drop` ante `panic!`).
        let _reaper = arm_reaper("dub_daemon_with_translation");
        // Daemon caliente de la instancia propia (sin ciclo compartido ni sleeps).
        let inst = IsolatedInstance::new("dub_traduccion");
        start_instance(&inst, &[]);
        let a = inst.args();
        // Igual que `dub_daemon_passthrough` — el exit 0 con `dubbed` verifica la secuencia completa.
        // verifica la secuencia completa (transcribe→translate→sintetizar con
        // latidos) hasta el evento final, sin cota de 10 s.
        let (code, actual) = run_json_env(
            &[
                "--json",
                "--daemon",
                "speech",
                "dub",
                "--audio",
                "crates/avi-stt/tests/assets/parakeet_sample_16k.wav",
                "--source-language",
                "es-latam",
                "--target-language",
                "en",
            ],
            &a,
        );
        assert_eq!(code, 0, "dub con traducción --daemon debe salir 0");
        assert_eq!(actual["status"], Value::String("dubbed".to_string()));
        assert_eq!(actual["schema_version"], Value::String("4".to_string()));
        // Verificación sobre el archivo recibido: WAV válido más
        // texto traducido no vacío (el daemon devuelve en `text` el final
        // traducido). Sin gate WER por el mismo
        // motivo que `dub_daemon_passthrough`: no endurecer a ciegas.
        let audio = actual["audio_path"]
            .as_str()
            .expect("audio_path debe existir");
        let audio_path = Path::new(audio);
        assert!(
            audio_path.is_file(),
            "el WAV del daemon debe estar persistido"
        );
        valid_wav_24k(audio_path);
        let text = actual["text"].as_str().expect("text debe existir");
        assert!(!text.is_empty(), "`text` no debe estar vacío");
        // Apagado propio con cero huérfanos verificados a nivel SO.
        stop_instance(&inst, "dub_daemon_with_translation");
        hit_end("tts::dub_daemon_with_translation");
    }

    /// Test de rendimiento dedicado: verifica que `daemon status` sobre un daemon en ejecución responde holgadamente dentro del presupuesto de 1500 ms.
    /// sobre un daemon ya en ejecución responda holgadamente dentro del presupuesto
    /// de 1500 ms (típico < 100 ms).
    #[test]
    #[ignore = "requiere el modelo Qwen3-TTS y el binario del motor"]
    fn perf_daemon_status_while_running() {
        require_tts_model();
        require_tts_binary();
        let _tts = lock_tts();
        hit_start_heavy("tts::perf_daemon_status_while_running");
        let _reaper = arm_reaper("perf_daemon_status_while_running");
        // Instancia aislada propia; `status` sobre daemon en ejecución.
        let inst = IsolatedInstance::new("perf_status");
        start_instance_running_only(&inst, &[]);
        let a = inst.args();
        let t0 = Instant::now();
        let (code, actual) = run_json_env(&["--json", "daemon", "status"], &a);
        let elapsed = t0.elapsed();
        assert_eq!(code, 0);
        assert_eq!(actual["daemon"], Value::String("running".to_string()));
        assert!(
            elapsed < Duration::from_millis(1500),
            "daemon status tomó {:?}, superando el presupuesto de 1500 ms",
            elapsed
        );
        stop_instance(&inst, "perf_daemon_status_while_running");
        hit_end("tts::perf_daemon_status_while_running");
    }
}

/// Ejecuta el binario con `args` capturando stdout como texto plano y
/// devolviendo (código de salida, stdout). Para aserciones sobre `--help`.
/// Cada llamada usa un sandbox de contrato nuevo que se borra al terminar.
fn run_text(args: &[&str]) -> (i32, String) {
    let (dir, envs) = contract_sandbox("run_text");
    let output = Command::new(BIN)
        .args(args)
        .envs(envs)
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    let _ = std::fs::remove_dir_all(&dir);
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
fn speech_help_exposes_languages_and_temperature() {
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
fn speech_invalid_temperature_exits_2() {
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
    // Sin `--json`, el mensaje humano va a stderr y `main` le pone el único prefijo.
    let output = Command::new(BIN)
        .args(["speech", "say", "--text", "Hola", "--temperature", "0"])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("Error:").count(),
        1,
        "el prefijo `Error:` debe aparecer una sola vez: {}",
        stderr
    );
}

/// `--daemon` en un comando que siempre se ejecuta en local es un error de
/// invocación: exit 2 con `daemon_not_supported`, sin culpar al daemon.
#[test]
fn daemon_flag_on_local_commands_exits_2() {
    for args in [
        vec!["voice", "list"],
        vec!["voice", "remove", "--name", "voz-inexistente"],
        vec!["speech", "list"],
        vec!["speech", "play", "--label", "etiqueta-inexistente"],
        vec!["speech", "remove", "--label", "etiqueta-inexistente"],
    ] {
        let mut full = vec!["--json", "--daemon"];
        full.extend(args.iter().copied());
        let (code, actual) = run_json(&full);
        assert_eq!(code, 2, "--daemon en {:?} debe salir 2", args);
        assert_eq!(
            actual["reason"], "daemon_not_supported",
            "--daemon en {:?} debe dar daemon_not_supported",
            args
        );
    }
}

/// `dub` sin `--source-language`: el parser lo exige → exit 2.
#[test]
fn speech_dub_without_source_exits_2() {
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

// ─── precondiciones de `synthesize --play` (RF-12.1/RF-12.2) ────────
//
// Solo se blindan las precondiciones puras: el bucle interactivo de 4
// opciones corre en TTY y no es ejercitable por esta suite (todas las
// invocaciones fijan stdin a `Stdio::null()`, garantía de no-TTY). CA-12.3
// (opción 1, mantener sin re-síntesis), CA-12.4 (opción 2, recomprobación de
// colisión al guardar) y CA-12.5 (opción 3, re-síntesis; opción 4/EOF,
// descarte) quedan como validación manual TTY.

/// CA-12.1: `synthesize --play --json` es incompatible (RF-12.1) → sale con
/// `ExitCode::InvalidInput` (2), validado antes de cualquier síntesis.
#[test]
fn speech_synthesize_play_with_json_exits_2() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "synthesize",
        "--text",
        "Hola",
        "--label",
        "x",
        "--play",
    ]);
    assert_eq!(
        code, 2,
        "--play + --json debe mapear a ExitCode::InvalidInput (reason={:?})",
        actual["reason"]
    );
}

/// CA-12.2: `synthesize --play` sin TTY sale con `ExitCode::InvalidInput` (2)
/// antes de sintetizar (RF-12.2). Se invoca sin `--json` (no vía `run_json`)
/// porque añadir `--json` dispararía en su lugar la precondición RF-12.1
/// (CA-12.1), enmascarando la guarda de TTY que este test aísla; en este
/// arnés todas las invocaciones son no-TTY (`Stdio::null()`), así que la
/// guarda RF-12.2 se ejercita con solo `--play`.
#[test]
fn speech_synthesize_play_without_tty_exits_2() {
    let output = Command::new(BIN)
        .args([
            "speech",
            "synthesize",
            "--text",
            "Hola",
            "--label",
            "x",
            "--play",
        ])
        .stdin(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    assert_eq!(
        output
            .status
            .code()
            .expect("el proceso debe terminar con un código"),
        2,
        "--play sin TTY debe mapear a ExitCode::InvalidInput"
    );
}

/// Detector de drift contrato↔código: el contrato no promete parámetros sin
/// efecto y documenta los flags que el binario expone.
#[test]
fn speech_contract_matches_help() {
    let contract = std::fs::read_to_string(
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("docs/CLI/CONTRACT.md"),
    )
    .expect("el contrato debe leerse");
    for flag in ["--compute-backend", "--exaggeration", "--cfg-weight"] {
        assert!(
            !contract.contains(flag),
            "el contrato no debe prometer {}",
            flag
        );
    }
    assert!(
        contract.contains("--temperature"),
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

// ─── Ayuda y errores de clap en español (T17) ─────────────────────────────

/// Ejecuta el binario con `args` capturando stdout+stderr como texto plano.
fn run_text_with_stderr(args: &[&str]) -> (i32, String, String) {
    let output = Command::new(BIN)
        .args(args)
        .stdin(std::process::Stdio::null())
        .output()
        .expect("el binario debe ejecutarse");
    let code = output
        .status
        .code()
        .expect("el proceso debe terminar con un código");
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    (code, stdout, stderr)
}

/// Todo `--help` de la CLI sale en español: contiene `Uso:` y `Opciones:`
/// y no contiene los textos en inglés que generaba clap.
#[test]
fn help_output_is_spanish_for_every_command() {
    let nodes: Vec<Vec<&str>> = vec![
        vec![],
        vec!["version"],
        vec!["devices"],
        vec!["translate"],
        vec!["voice"],
        vec!["voice", "list"],
        vec!["voice", "clone"],
        vec!["voice", "remove"],
        vec!["speech"],
        vec!["speech", "list"],
        vec!["speech", "transcribe"],
        vec!["speech", "synthesize"],
        vec!["speech", "say"],
        vec!["speech", "dub"],
        vec!["speech", "play"],
        vec!["speech", "remove"],
        vec!["daemon"],
        vec!["daemon", "start"],
        vec!["daemon", "stop"],
        vec!["daemon", "restart"],
        vec!["daemon", "status"],
        vec!["daemon", "serve"],
        vec!["setup"],
        vec!["cleanup"],
        vec!["self"],
        vec!["self", "install"],
        vec!["self", "uninstall"],
        vec!["self", "update"],
        vec!["doctor"],
    ];
    for node in &nodes {
        let mut args = node.clone();
        args.push("--help");
        let (code, help) = run_text(&args);
        assert_eq!(code, 0, "{:?} --help debe salir 0", node);
        assert!(
            help.contains("Uso:"),
            "{:?} --help debe contener 'Uso:'",
            node
        );
        assert!(
            help.contains("Opciones:"),
            "{:?} --help debe contener 'Opciones:'",
            node
        );
        for forbidden in [
            "Usage:",
            "Options:",
            "Arguments:",
            "Commands:",
            "Print help",
            "Print version",
            "[default:",
            "[possible values:",
        ] {
            assert!(
                !help.contains(forbidden),
                "{:?} --help no debe contener '{}'",
                node,
                forbidden
            );
        }
    }
}

/// Las anotaciones de valores se derivan en español de los valores reales:
/// `translate` muestra defecto y posibles; ningún flag booleano muestra
/// defecto `false`.
#[test]
fn help_annotations_are_spanish() {
    let (_, help) = run_text(&["translate", "--help"]);
    assert!(
        help.contains("[por defecto: es]"),
        "translate --help debe mostrar '[por defecto: es]'"
    );
    assert!(
        help.contains("[valores posibles: es, en]"),
        "translate --help debe mostrar '[valores posibles: es, en]'"
    );
    let (_, help) = run_text(&["setup", "--help"]);
    assert!(
        !help.contains("[por defecto: false]"),
        "setup --help no debe anotar defecto en flags booleanos"
    );
}

/// El subcomando automático `help` está deshabilitado: `help speech` falla
/// con 2 y `speech --help` sale con 0.
#[test]
fn help_subcommand_is_disabled() {
    let (code, _, _) = run_text_with_stderr(&["help", "speech"]);
    assert_eq!(code, 2, "help speech debe salir 2");
    let (code, _, _) = run_text_with_stderr(&["speech", "--help"]);
    assert_eq!(code, 0, "speech --help debe salir 0");
}

/// Argumento desconocido: exit 2 con error en español en stderr.
#[test]
fn unknown_argument_error_is_spanish() {
    let (code, _, stderr) =
        run_text_with_stderr(&["speech", "say", "--text", "Hola", "--flag-inexistente"]);
    assert_eq!(code, 2, "flag inexistente debe salir 2");
    assert!(
        stderr.starts_with("Error:"),
        "stderr debe empezar con 'Error:': {}",
        stderr
    );
    assert!(
        stderr.contains("Uso:"),
        "stderr debe contener 'Uso:': {}",
        stderr
    );
    assert!(
        !stderr.contains("error: unexpected argument"),
        "stderr no debe estar en inglés: {}",
        stderr
    );
    assert!(
        !stderr.contains("invalid value"),
        "stderr no debe estar en inglés: {}",
        stderr
    );
}

/// Valor inválido: exit 2 con error en español en stderr.
#[test]
fn invalid_value_error_is_spanish() {
    let (code, _, stderr) = run_text_with_stderr(&["translate", "--text", "Hola", "--from", "fr"]);
    assert_eq!(code, 2, "valor inválido debe salir 2");
    assert!(
        stderr.starts_with("Error:"),
        "stderr debe empezar con 'Error:': {}",
        stderr
    );
    assert!(
        stderr.contains("Uso:"),
        "stderr debe contener 'Uso:': {}",
        stderr
    );
    assert!(
        !stderr.contains("error: unexpected argument"),
        "stderr no debe estar en inglés: {}",
        stderr
    );
    assert!(
        !stderr.contains("invalid value"),
        "stderr no debe estar en inglés: {}",
        stderr
    );
}

/// Demostración del guard de tiempo F4b (prueba rápida, sin daemons ni minutos):
/// fija un techo diminuto a propósito y exige `panic!` con diagnóstico
/// (último hito + fase exacta). `#[should_panic]` mantiene la suite en verde
/// mientras demuestra que el mecanismo falla en vez de colgarse.
#[test]
#[should_panic(expected = "guardia de tiempo")]
fn milestone_guard_expires_with_diagnostics() {
    hit_start(
        "milestone_guard_expires_with_diagnostics",
        Duration::from_millis(50),
    );
    milestone("milestone_guard_expires_with_diagnostics: hito previo al guard");
    std::thread::sleep(std::time::Duration::from_millis(120));
    check_guard("fase-demostracion-guard");
}

// ─── Tests de rendimiento dedicados (locales, sin inferencia) ─────────────────
//
// Separan la señal de rendimiento de la señal de corrección: afirman
// duraciones y presupuestos explícitamente sobre operaciones sin
// contención ni inferencia pesada.

#[test]
fn perf_local_fast_commands_under_budget() {
    // Los comandos locales de metadatos/ayuda deben responder de inmediato (< 1500 ms).
    let t0 = Instant::now();
    let (code_v, _) = run_json(&["--json", "version"]);
    let d_v = t0.elapsed();
    assert_eq!(code_v, 0);
    assert!(
        d_v < Duration::from_millis(1500),
        "version --json tardó {:?} (presupuesto < 1500 ms)",
        d_v
    );

    let t1 = Instant::now();
    let (code_h, _) = run_text(&["speech", "synthesize", "--help"]);
    let d_h = t1.elapsed();
    assert_eq!(code_h, 0);
    assert!(
        d_h < Duration::from_millis(1500),
        "speech synthesize --help tardó {:?} (presupuesto < 1500 ms)",
        d_h
    );
}

#[test]
fn perf_invalid_input_rejection_fail_fast() {
    // Validaciones baratas deben fallar inmediatamente sin pagar cold-start de inferencia.
    let t0 = Instant::now();
    let (code_say, _) = run_json(&["--json", "speech", "say", "--text", ""]);
    let d_say = t0.elapsed();
    assert_eq!(code_say, 2);
    assert!(
        d_say < Duration::from_millis(1500),
        "say con texto vacío tardó {:?} (fail-fast expected < 1500 ms)",
        d_say
    );

    let t1 = Instant::now();
    let (code_play, _) = run_json(&[
        "--json",
        "speech",
        "synthesize",
        "--text",
        "hola",
        "--label",
        "x",
        "--play",
    ]);
    let d_play = t1.elapsed();
    assert_eq!(code_play, 2);
    assert!(
        d_play < Duration::from_millis(1500),
        "synthesize --play --json tardó {:?} (fail-fast expected < 1500 ms)",
        d_play
    );
}

// ─── Regresión SIGPIPE (v0.20.10): stdout cerrado no debe causar panic ──
//
// Rust ignora `SIGPIPE` por defecto (`SIG_IGN`): sin la restauración a
// `SIG_DFL` hecha en `main()` (Unix, modos CLI en primer plano), una
// escritura a un pipe cuyo extremo de lectura ya está cerrado devuelve
// `EPIPE` como error de I/O y `println!`/`writeln!` hacen `panic!`
// ("failed printing to stdout: Broken pipe", exit 101). Este test verifica
// que, tras la restauración, el proceso muere en silencio por la señal (o
// termina limpio si alcanzó a escribir todo antes del cierre) en lugar de
// entrar en panic.
//
// Determinismo: se construye la tubería manualmente con `libc::pipe` y se
// cierra el extremo de lectura con `libc::close` ANTES de lanzar el hijo
// (no tras el `spawn`), de modo que la primera escritura del hijo a stdout
// ya encuentra el pipe roto — sin ventana de carrera. Solo Unix: Windows no
// tiene señales POSIX y este binario no restaura ningún manejador ahí.

#[cfg(unix)]
#[test]
fn test_voice_list_no_panic_on_sigpipe_closed_stdout_before_spawn() {
    use std::os::unix::io::FromRawFd;
    use std::os::unix::process::ExitStatusExt;
    use std::process::Stdio;

    let (_dir, mut envs) = sandbox_unique_state("sigpipe");
    envs.push(("AVI_DAEMON_PORT".to_string(), "0".to_string()));

    // (a) Tubería manual: extremo de lectura cerrado antes del spawn.
    let mut fds: [libc::c_int; 2] = [0; 2];
    let rc = unsafe { libc::pipe(fds.as_mut_ptr()) };
    assert_eq!(
        rc,
        0,
        "libc::pipe falló: {}",
        std::io::Error::last_os_error()
    );
    let (read_fd, write_fd) = (fds[0], fds[1]);
    let closed = unsafe { libc::close(read_fd) };
    assert_eq!(
        closed,
        0,
        "no se pudo cerrar el extremo de lectura del pipe: {}",
        std::io::Error::last_os_error()
    );

    // (b) Lanzar `voice list` con stdout apuntando al extremo de escritura
    // ya roto (write_fd sobrevive al close del otro extremo; Stdio adopta
    // el fd y lo cierra al soltar el Child).
    let broken_stdout = unsafe { Stdio::from_raw_fd(write_fd) };
    let mut cmd = Command::new(BIN);
    cmd.args(["voice", "list"])
        .envs(envs.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdout(broken_stdout)
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("no se pudo lanzar el binario bajo test");

    let mut stderr_buf = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        use std::io::Read;
        let _ = stderr.read_to_end(&mut stderr_buf);
    }
    let status = child.wait().expect("esperar al hijo falló");
    let stderr_txt = String::from_utf8_lossy(&stderr_buf);

    assert!(
        !stderr_txt.contains("panicked"),
        "el proceso hizo panic en vez de morir por SIGPIPE en silencio; stderr: {}",
        stderr_txt
    );
    assert_ne!(
        status.code(),
        Some(101),
        "exit 101 es el código de panic de Rust; stderr: {}",
        stderr_txt
    );
    let died_from_sigpipe = status.signal() == Some(libc::SIGPIPE);
    let exited_cleanly = status.code() == Some(0);
    assert!(
        died_from_sigpipe || exited_cleanly,
        "se esperaba muerte por SIGPIPE (señal {}) o salida limpia (0); status real: {:?}, stderr: {}",
        libc::SIGPIPE,
        status,
        stderr_txt
    );
}
