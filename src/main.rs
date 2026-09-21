use avi_audio as audio;
// El trait STT y el motor real solo entran en scope con `native-stt` (off por
// defecto); sin el feature, los subcomandos de transcripción devuelven un error
// explícito de "compilado sin soporte".
#[cfg(feature = "native-stt")]
use avi_core::engine::SttEngine;
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_daemon as daemon;
use avi_store as store;
use avi_store::{ModelStore, SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::ParakeetEngine;
use avi_tts::Qwen3TtsEngine;
// El motor de traducción real solo entra en scope con `native-translation`.
#[cfg(feature = "native-translation")]
use avi_translation as translation;
use base64::Engine;
use clap::{Parser, Subcommand};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::exit;

const VERSION: &str = "0.19.0";
const APP_NAME: &str = "ai-voice-interconnector";
/// Dirección del daemon nativo; el cliente HTTP async apunta a este address.
const DAEMON_ADDR: &str = "127.0.0.1:8765";
/// Variable de entorno para desviar el puerto del daemon por instancia
/// (permite aislamiento por instancia). Acepta `0` para puerto efímero (`:0`,
/// el SO asigna y el servidor imprime el real vía `local_addr()`).
const DAEMON_PORT_ENV: &str = "AVI_DAEMON_PORT";

/// Resuelve la dirección de escucha del daemon: override por
/// `AVI_DAEMON_PORT` (aislamiento por instancia) con fallback a `DAEMON_ADDR`.
/// No lee `avi-config::daemon_port` a propósito: el arranque no debe acoplarse
/// al `config.toml` del data_dir compartido (colisionaría entre instancias).
fn resolver_addr_daemon() -> SocketAddr {
    if let Ok(raw) = std::env::var(DAEMON_PORT_ENV) {
        if let Ok(port) = raw.trim().parse::<u16>() {
            return SocketAddr::from(([127, 0, 0, 1], port));
        }
    }
    DAEMON_ADDR
        .parse()
        .expect("la dirección por defecto del daemon debe parsear")
}
/// Techo temporal para esperar que el daemon sea alcanzable en `daemon start/restart`.
/// Dimensionado solo para spawn + bind del proceso (el warmup TTS corre en segundo
/// plano, ya no bloquea el arranque). Es el timeout diagnóstico de la espera
/// del evento (fichero ready); el intervalo es la cadencia de lectura del fichero.
/// Ningún timeout nuevo puede ser más corto que el `WARMUP_DEADLINE` (40 s) que
/// acota el warmup real: este techo solo cubre bind+publicación (~1-2 s sanos).
const DAEMON_READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// Cadencia de lectura del fichero ready (antes intervalo del sondeo).
const DAEMON_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// Deadline breve para la limpieza acotada ante Ctrl+C: mata el árbol
/// preciso con verificación; vencido, sale igualmente con 130 sin colgarse.
const CTRL_C_LIMPIEZA_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// Deadline global para la parada unificada del árbol daemon+residente
/// (graceful + árbol preciso por PID + verificación a nivel de sistema).
const STOP_DEADLINE_GLOBAL: std::time::Duration = std::time::Duration::from_secs(8);

/// PID hijo en memoria desde el `spawn`: estrecha la ventana
/// spawn→write del pidfile. El handler Ctrl+C lo reclama cuando aún no hay
/// pidfile; se fija en la secuencia `Start` justo tras `spawn_background`.
static PID_EN_MEMORIA: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

// CT2 derivado obligatorio de Marian HF en `hf_cache_dir()/ct2` (`ct2_model_dir`) → `model.bin`
// más tokenizador (`tokenizer.json`, o `source.spm`+`target.spm` autocontenidos).
// Incondicional cuando Marian está provisionado; idempotente por `mtime` solo sobre dirs
// sanos (dir roto ⇒ reconversión); escritura atómica (temporal hermano + rename).

/// Resuelve un token de idioma de la CLI (`es-latam`/`en`) al código ISO que
/// exige el motor STT: `es-latam` -> `es`; cualquier otro valor pasa verbatim
/// (espeja `resolve_language` del oráculo Python).
fn resolve_stt_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
}

/// Valida el override de temperatura (`0 < t <= 2.0`); exit 2 si no calza.
fn validar_temperature(temperature: Option<f32>) -> Result<(), CliError> {
    if let Some(t) = temperature {
        if !(t > 0.0 && t <= 2.0) {
            return Err(CliError::new(
                ExitCode::InvalidInput,
                "usage_error",
                "Error: --temperature debe ser mayor que 0 y como máximo 2.0.",
            ));
        }
    }
    Ok(())
}

#[derive(Parser)]
#[command(name = APP_NAME, version = VERSION, about = "AI Voice Interconnector CLI")]
struct Cli {
    #[arg(long, global = true)]
    json: bool,

    /// Fuerza el uso exclusivo del daemon IPC (exit 5 si no responde)
    #[arg(long, global = true, conflicts_with = "no_daemon")]
    daemon: bool,

    /// Fuerza la ejecución en proceso local directo (sin daemon)
    #[arg(long, global = true, conflicts_with = "daemon")]
    no_daemon: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonMode {
    /// Autodetección: intenta daemon, cae a directo
    Auto,
    /// Fuerza daemon exclusivo
    ForceDaemon,
    /// Fuerza ejecutor directo sin daemon
    ForceDirect,
}

impl Cli {
    pub fn daemon_mode(&self) -> DaemonMode {
        if self.daemon {
            DaemonMode::ForceDaemon
        } else if self.no_daemon {
            DaemonMode::ForceDirect
        } else {
            DaemonMode::Auto
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Muestra la versión del programa
    Version,
    /// Enumera dispositivos de salida de audio
    Devices,
    /// Traducción de texto es<->en
    Translate {
        #[arg(short, long)]
        text: String,
        #[arg(long, default_value = "es", value_parser = ["es", "en"])]
        from: String,
        #[arg(long, default_value = "en", value_parser = ["es", "en"])]
        to: String,
    },
    /// Gestión de voces clonadas
    Voice {
        #[command(subcommand)]
        action: VoiceCommands,
    },
    /// Síntesis y locuciones
    Speech {
        #[command(subcommand)]
        action: SpeechCommands,
    },
    /// Control del daemon
    Daemon {
        #[command(subcommand)]
        action: DaemonCommands,
    },
    /// Provisiona el runtime: chequeos + descarga de modelos
    Setup {
        #[arg(long)]
        with_stt: bool,
        /// Incluye el modelo Base de clonado Qwen3-TTS (~2,5 GB)
        #[arg(long)]
        with_voice_cloning: bool,
        /// Purga los snapshots pinneados y la cache xet, luego re-descarga desde cero
        #[arg(long)]
        force_update: bool,
        /// No pedir confirmación en la purga destructiva de --force-update
        #[arg(long, short)]
        yes: bool,
    },
    /// Limpia datos provisionados de forma granular; --all = unión de --voices/--synthetic-speech/--model, sin binario ni PATH
    Cleanup {
        #[arg(long)]
        voices: bool,
        #[arg(long)]
        synthetic_speech: bool,
        #[arg(long)]
        model: bool,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long, short)]
        yes: bool,
    },
    /// Desinstala el programa (datos + binario + PATH) en un comando
    Uninstall {
        /// No pedir confirmación
        #[arg(long, short)]
        force: bool,
        /// Alias de --force
        #[arg(long)]
        yes: bool,
    },
    /// Diagnóstico de entorno
    Doctor,
}

#[derive(Subcommand)]
enum VoiceCommands {
    /// Listar voces registradas
    List,
    /// Clonar una voz desde audio de referencia
    Clone {
        #[arg(short, long)]
        name: String,
        /// Audio de referencia de habla (obligatorio; paridad con el oráculo)
        #[arg(short = 's', long)]
        speech_reference: String,
        /// Audio de referencia de timbre (opcional)
        #[arg(short = 't', long)]
        timbre_reference: Option<String>,
        /// Sobrescribir una voz existente con el mismo nombre
        #[arg(short, long)]
        force: bool,
    },
    /// Eliminar una voz clonada
    Remove {
        #[arg(short, long)]
        name: String,
    },
}

#[derive(Subcommand)]
enum SpeechCommands {
    /// Listar habla sintética persistida
    List {
        /// Filtrar por voz (ausente = todas las voces)
        #[arg(short, long)]
        voice: Option<String>,
    },
    /// Transcribir audio
    Transcribe {
        /// Ruta del archivo WAV a transcribir (mutuamente excluyente con --mic)
        #[arg(long, conflicts_with = "mic")]
        audio: Option<String>,
        /// Transcribir desde el micrófono (mutuamente excluyente con --audio)
        #[arg(long)]
        mic: bool,
        /// Duración fija de grabación en segundos; solo válido con --mic
        #[arg(long)]
        duration: Option<u64>,
        /// Idioma hablado en el audio
        #[arg(long, value_parser = ["es-latam", "en"])]
        source_language: String,
    },
    /// Sintetizar texto a habla y persistir la locución
    Synthesize {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        #[arg(short, long)]
        output: Option<String>,
        /// Etiqueta de la locución persistida (obligatorio; paridad con el oráculo)
        #[arg(short, long)]
        label: String,
        /// Sobrescribir una locución existente con la misma etiqueta
        #[arg(short, long)]
        force: bool,
        #[arg(long)]
        play: bool,
        /// Idioma del texto de entrada (por defecto igual a --target-language, sin traducir)
        #[arg(long, value_parser = ["es-latam", "en"])]
        source_language: Option<String>,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto se traduce antes de sintetizar)
        #[arg(long, default_value = "es-latam", value_parser = ["es-latam", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
    },
    /// Sintetizar y reproducir
    Say {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Idioma del texto de entrada (por defecto igual a --target-language, sin traducir)
        #[arg(long, value_parser = ["es-latam", "en"])]
        source_language: Option<String>,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto se traduce antes de sintetizar)
        #[arg(long, default_value = "es-latam", value_parser = ["es-latam", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
    },
    /// Doblaje voz→voz: transcribe, traduce, sintetiza y reproduce
    Dub {
        /// Archivo de audio a doblar (alias del oráculo: --file)
        #[arg(short = 'a', long, alias = "file")]
        audio: Option<String>,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Idioma hablado en el audio de entrada
        #[arg(long, value_parser = ["es-latam", "en"])]
        source_language: String,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto transcrito se traduce antes de sintetizar)
        #[arg(long, default_value = "es-latam", value_parser = ["es-latam", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
        /// Capturar desde el micrófono (mutuamente excluyente con --audio)
        #[arg(long, conflicts_with = "audio")]
        mic: bool,
        /// Duración fija de grabación en segundos; solo válido con --mic
        #[arg(long)]
        duration: Option<u64>,
    },
    /// Reproducir una locución guardada
    Play {
        #[arg(short, long)]
        label: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
    },
    /// Eliminar una locución guardada
    Remove {
        #[arg(short, long)]
        label: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
    },
}

#[derive(Subcommand)]
enum DaemonCommands {
    /// Iniciar el daemon en segundo plano
    Start {
        /// Reiniciar automáticamente el daemon si falla
        #[arg(long)]
        auto_restart: bool,
        /// Número máximo de reintentos con reinicio automático (default 3)
        #[arg(long, default_value_t = 3)]
        max_retries: u32,
        /// Voz a precalentar al arranque (fail-fast si no existe)
        #[arg(long, default_value = "default")]
        warm_voice: String,
    },
    /// Detener el daemon
    Stop,
    /// Reiniciar el daemon
    Restart,
    /// Estado del daemon
    Status,
    /// Ejecutar el servidor HTTP del daemon en primer plano
    Serve {
        /// Reiniciar automáticamente el daemon si falla
        #[arg(long)]
        auto_restart: bool,
        /// Número máximo de reintentos con reinicio automático (default 3)
        #[arg(long, default_value_t = 3)]
        max_retries: u32,
        /// Voz a precalentar al arranque (fail-fast si no existe)
        #[arg(long, default_value = "default")]
        warm_voice: String,
        /// Fichero donde el hijo publica su `addr` real tras el bind y su
        /// estado warm tras el warmup (transporte del evento
        /// `avi-daemon-ready`; el padre lo consume con espera acotada).
        #[arg(long)]
        ready_file: Option<PathBuf>,
    },
}

// ─── Bootstrap ───────────────────────────────────────────────────────

/// Forzar UTF-8 en stdout/stderr (equivalente a bootstrap.py)
fn force_utf8() {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "chcp", "65001"])
        .output();
}

/// Instalar handler de SIGINT → limpieza acotada + exit 130.
///
/// Preserva el código 130 en todos los modos; antes de salir mata el árbol
/// preciso del daemon residual por PID con deadline breve y verificación
/// (`taskkill /F /T /PID` en Windows con `CREATE_NO_WINDOW`, `kill -9` al
/// grupo en Unix). Vencido el deadline sale igualmente con 130 sin colgarse.
/// Reclama el PID en memoria cuando aún no hay pidfile (ventana
/// spawn→write), preservando 130 y el techo de 2 s.
/// Guarda anti-auto-muerte: si el PID de la pista es el propio proceso
/// (`daemon serve` en foreground), no se auto-mata; el cierre lo hace la
/// escucha de señales del servidor por la misma ruta que POST `/shutdown`.
/// Instalado en `main` antes del despacho: cubre todos los modos.
fn install_sigint_handler() {
    ctrlc::set_handler(move || {
        // pidfile primero; sin pidfile, PID en memoria (ventana spawn→write).
        let pid = read_daemon_pid().or_else(|| {
            let m = PID_EN_MEMORIA.load(std::sync::atomic::Ordering::Relaxed);
            if m != 0 {
                Some(m)
            } else {
                None
            }
        });
        if let Some(pid) = pid {
            let propio = std::process::id();
            if pid != 0 && pid != propio {
                daemon::matar_arbol_por_pid(pid);
                daemon::esperar_muerte_pid(pid, CTRL_C_LIMPIEZA_DEADLINE);
            }
        }
        // Exit code 130 = interrumpido por usuario (Ctrl+C)
        eprintln!("\nInterrumpido por el usuario.");
        exit(130);
    })
    .expect("Error al instalar el handler de Ctrl+C");
}

/// Job Object con cierre del árbol para el daemon longevo (Windows).
///
/// Crea un Job con `KILL_ON_JOB_CLOSE` y asigna el proceso actual: al morir el
/// daemon, el SO cierra el árbol (residente incluido, que hereda el Job).
/// Best-effort silencioso: si falla, el cierre sigue garantizado por
/// `matar_arbol_por_pid` con verificación (alternativa admitida). Se llama solo
/// en la rama `Serve` (proceso longevo), nunca en el padre efímero de `Start`.
#[cfg(windows)]
fn instalar_job_con_cierre_de_arbol() {
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicLimitInformation,
        SetInformationJobObject, JOBOBJECT_BASIC_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job == 0 {
            return;
        }
        let mut info: JOBOBJECT_BASIC_LIMIT_INFORMATION = std::mem::zeroed();
        info.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectBasicLimitInformation,
            &info as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_BASIC_LIMIT_INFORMATION>() as u32,
        );
        if ok == 0 {
            CloseHandle(job);
            return;
        }
        // Pseudo-handle del proceso actual (-1): evita necesitar OpenProcess.
        let actual: isize = -1;
        if AssignProcessToJobObject(job, actual) == 0 {
            CloseHandle(job);
        }
        // Fuga intencionada del handle del Job: vive hasta la muerte del daemon.
    }
}

/// Desactiva la herencia de los 3 handles estándar del proceso actual (Windows).
///
/// Causa raíz: en Rust estable `Command::spawn` llama a `CreateProcessW`
/// con `bInheritHandles=TRUE` sin posibilidad de forzarlo a FALSE (no expuesto en
/// estable). Con ese flag TODO handle heredable de la tabla del padre se duplica
/// al hijo, no solo sus 3 handles estándar. Cuando el CLI corre bajo un pipe
/// heredable del lanzador (p. ej. `Command::output()`), su stdout es justamente
/// ese write-end: se re-hereda al daemon y de ahí al motor, y el lanzador no ve
/// EOF hasta que todos lo cierren. NO existe una creation flag para desactivar la
/// herencia (el histórico `0x02000000` es `CREATE_PRESERVE_CODE_AUTHZ_LEVEL`,
/// no-op); la herencia se controla por handle con `SetHandleInformation`.
///
/// Se quita `HANDLE_FLAG_INHERIT` de STD_IN/OUT/ERROR: corta la propagación en la
/// raíz sin matar el árbol. El stderr del motor no se ve afectado: va al
/// fichero de log vía `Stdio::from` (handle explícito con mecanismo aparte).
/// Best-effort silencioso: salta handles nulos / `INVALID_HANDLE_VALUE`.
#[cfg(windows)]
fn desheredar_handles_estandar() {
    use windows_sys::Win32::Foundation::{
        SetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::System::Console::{
        GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        for id in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let h = GetStdHandle(id);
            if h == 0 || h == INVALID_HANDLE_VALUE {
                continue;
            }
            SetHandleInformation(h, HANDLE_FLAG_INHERIT, 0);
        }
    }
}

// ─── Punto de entrada ────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Bootstrap: UTF-8, tracing, SIGINT
    force_utf8();
    // Los logs van a stderr: stdout queda reservado para el contrato JSON
    // (envelope schema_version="3"), igual que el oráculo Python.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    install_sigint_handler();

    let cli = Cli::parse();
    let json_mode = cli.json;
    let daemon_mode = cli.daemon_mode();

    let result = match cli.command {
        Some(Commands::Version) => handle_version(json_mode),
        Some(Commands::Devices) => handle_devices(json_mode),
        Some(Commands::Translate { text, from, to }) => {
            handle_translate(json_mode, daemon_mode, &text, &from, &to).await
        }
        Some(Commands::Voice { action }) => handle_voice(json_mode, daemon_mode, action).await,
        Some(Commands::Speech { action }) => handle_speech(json_mode, daemon_mode, action).await,
        Some(Commands::Daemon { action }) => handle_daemon(json_mode, action).await,
        Some(Commands::Setup {
            with_stt,
            with_voice_cloning,
            force_update,
            yes,
        }) => handle_setup(json_mode, with_stt, with_voice_cloning, force_update, yes).await,
        Some(Commands::Cleanup {
            voices,
            synthetic_speech,
            model,
            all,
            dry_run,
            yes,
        }) => {
            handle_cleanup(
                json_mode,
                voices,
                synthetic_speech,
                model,
                all,
                dry_run,
                yes,
            )
            .await
        }
        Some(Commands::Uninstall { force, yes }) => handle_uninstall(json_mode, force || yes).await,
        Some(Commands::Doctor) => handle_doctor(json_mode),
        None => handle_version(json_mode),
    };

    if let Err(err) = result {
        if json_mode {
            emit_raw_json(json!({
                "error": err.message,
                "reason": err.reason,
            }));
        } else {
            eprintln!("Error: {}", err.message);
        }
        std::io::stdout().flush().ok();
        exit(err.code.code());
    }
}

// ─── Handlers ────────────────────────────────────────────────────────

fn handle_version(json_mode: bool) -> Result<(), CliError> {
    if json_mode {
        emit_raw_json(json!({ "name": APP_NAME, "version": VERSION }));
    } else {
        println!("{} {}", APP_NAME, VERSION);
    }
    Ok(())
}

fn handle_devices(json_mode: bool) -> Result<(), CliError> {
    let devices = audio::get_devices_json()
        .map_err(|e| CliError::new(ExitCode::Error, "audio_enumeration_failed", e.to_string()))?;
    if json_mode {
        emit_raw_json(json!({ "devices": devices }));
    } else {
        println!("Dispositivos de salida de audio:");
        for dev in &devices {
            println!(
                "  [{}] {} (latencia: {:.1}ms)",
                dev["id"],
                dev["name"].as_str().unwrap_or(""),
                dev["latency"].as_f64().unwrap_or(0.0) * 1000.0
            );
        }
    }
    Ok(())
}

async fn handle_translate(
    json_mode: bool,
    daemon_mode: DaemonMode,
    text: &str,
    from: &str,
    to: &str,
) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_text",
            "El texto a traducir está vacío",
        ));
    }
    let source = resolve_stt_language(from);
    let target = resolve_stt_language(to);
    // Passthrough: origen == destino tras normalizar → texto intacto, sin motor
    if source == target {
        if json_mode {
            emit_raw_json(json!({ "translated": text, "source": from, "target": to }));
        } else {
            println!("{}", text);
        }
        return Ok(());
    }
    // Par no soportado → exit 2 (validación pura, sin tocar el modelo).
    let pair_valid = matches!((source, target), ("es", "en") | ("en", "es"));
    if !pair_valid {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "unsupported_language_pair",
            format!(
                "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                source, target
            ),
        ));
    }
    // Despacho 3 modos vía daemon_client + route_to_daemon
    let client = daemon_client();
    if route_to_daemon(daemon_mode, &client).await {
        return translate_via_daemon(json_mode, &client, text, from, to).await;
    }
    // Rama local: verifica modelo y traduce
    let pair = match (source, target) {
        ("es", "en") => "es-en",
        ("en", "es") => "en-es",
        _ => unreachable!(),
    };
    let ct2_dir = store::ct2_model_dir(pair);
    if !store::is_ct2_provisioned(pair) {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!(
                "El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.",
                ct2_dir.display(),
                store::ct2_archivos_faltantes(pair).join(", "),
            ),
        ));
    }
    #[cfg(not(feature = "native-translation"))]
    {
        let _ = ct2_dir.as_os_str();
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        let translated = translation::translate(text, source, target, &ct2_dir).map_err(|e| {
            CliError::new(
                ExitCode::TranslationFailed,
                "translation_failed",
                e.to_string(),
            )
        })?;
        if json_mode {
            emit_raw_json(json!({ "translated": translated, "source": from, "target": to }));
        } else {
            println!("{}", translated);
        }
        Ok(())
    }
}

/// Traducción opt-in previa a la síntesis: passthrough si origen y destino
/// coinciden tras normalizar; exit 2 si el par no es soportado, exit 4 si
/// falta el modelo, exit 9 si falla la traducción.
fn traducir_si_difiere(
    texto: &str,
    source_token: &str,
    target_token: &str,
) -> Result<String, CliError> {
    let source = resolve_stt_language(source_token);
    let target = resolve_stt_language(target_token);
    if source == target {
        return Ok(texto.to_string());
    }
    let pair = match (source, target) {
        ("es", "en") => "es-en",
        ("en", "es") => "en-es",
        _ => {
            return Err(CliError::new(
                ExitCode::InvalidInput,
                "unsupported_language_pair",
                format!(
                    "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                    source, target
                ),
            ));
        }
    };
    let ct2_dir = store::ct2_model_dir(pair);
    if !store::is_ct2_provisioned(pair) {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!(
                "El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.",
                ct2_dir.display(),
                store::ct2_archivos_faltantes(pair).join(", "),
            ),
        ));
    }
    #[cfg(not(feature = "native-translation"))]
    {
        let _ = ct2_dir.as_os_str();
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        translation::translate(texto, source, target, &ct2_dir).map_err(|e| {
            CliError::new(
                ExitCode::TranslationFailed,
                "translation_failed",
                e.to_string(),
            )
        })
    }
}

// ─── Voice ───────────────────────────────────────────────────────────

async fn handle_voice(
    json_mode: bool,
    daemon_mode: DaemonMode,
    action: VoiceCommands,
) -> Result<(), CliError> {
    let voice_store = VoiceStore::new();

    match action {
        VoiceCommands::List => {
            // List es local-only; ForceDaemon debe fallar con DaemonUnreachable (paridad con speech dub/play)
            require_local(daemon_mode)?;
            let voices = voice_store
                .list()
                .map_err(|e| CliError::new(ExitCode::Error, "voice_list_failed", e.to_string()))?;
            if json_mode {
                let names: Vec<&str> = voices.iter().map(|v| v.name.as_str()).collect();
                emit_raw_json(json!({ "voices": names }));
            } else {
                println!("Voces registradas:");
                for v in &voices {
                    let tag = if v.is_factory { " (fábrica)" } else { "" };
                    println!("  - {}{}", v.name, tag);
                }
            }
            Ok(())
        }
        VoiceCommands::Clone {
            name,
            speech_reference,
            timbre_reference,
            force,
        } => {
            // Orden de validaciones del oráculo (cli.py:841-899): nombre antes de modelo.
            let name = name.to_lowercase();
            VoiceStore::validate_name(&name)
                .map_err(|e| CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e))?;
            // Despacho 3 modos para Clone vía POST /voices/clone
            {
                let client = daemon_client();
                if route_to_daemon(daemon_mode, &client).await {
                    return clone_via_daemon(
                        json_mode,
                        &client,
                        &name,
                        &speech_reference,
                        timbre_reference.as_deref(),
                        force,
                    )
                    .await;
                }
            }
            require_model_provisioned()?;
            let speech_path = std::path::Path::new(&speech_reference);
            if !speech_path.is_file() {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "audio_not_found",
                    format!("El audio de referencia '{}' no existe.", speech_reference),
                ));
            }
            if let Some(t) = &timbre_reference {
                if !std::path::Path::new(t).is_file() {
                    return Err(CliError::new(
                        ExitCode::NotFound,
                        "audio_not_found",
                        format!("El audio de timbre '{}' no existe.", t),
                    ));
                }
            }
            if !force && voice_store.exists(&name) {
                return Err(CliError::new(
                    ExitCode::StateConflict,
                    "voice_exists",
                    format!(
                        "La voz '{}' ya existe (usa --force para sobrescribirla).",
                        name
                    ),
                ));
            }

            let engine = Qwen3TtsEngine::new(None);
            let model_dir = engine.base_model_dir.as_ref().ok_or_else(|| {
                CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "El modelo Base de clonado TTS no está provisionado. Ejecuta 'setup' primero.",
                )
            })?;
            let tmp_qvoice = std::env::temp_dir().join(format!("{}.qvoice", name));
            avi_tts::clone_voice(model_dir, speech_path, &tmp_qvoice, &name, "es")
                .map_err(|e| CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string()))?;
            let saved_qvoice = voice_store
                .save_reference(&name, &tmp_qvoice)
                .map_err(|e| CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string()))?;
            // El timbre y el habla quedan fundidos en `reference.qvoice` durante el
            // clonado; no se persisten WAV de referencia separados (nadie los lee).
            if json_mode {
                // La ruta local no tiene residente TTS persistente (el motor es
                // efímero por proceso): no hay nada que precalentar en caliente,
                // así que `precomputed` es siempre `false` aquí. El warm-on-clone
                // (`precomputed: true`) solo aplica a la ruta daemon.
                emit_raw_json(json!({
                    "name": name,
                    "speech": saved_qvoice.to_string_lossy().to_string(),
                    "precomputed": false,
                }));
            } else {
                println!("Voz '{}' clonada.", name);
            }
            Ok(())
        }
        VoiceCommands::Remove { name } => {
            require_local(daemon_mode)?;
            VoiceStore::validate_name(&name)
                .map_err(|e| CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e))?;
            // Validación estructural: fábrica protegida se rechaza antes de tocar el store,
            // sin depender del mensaje de error (evita string matching frágil).
            if avi_store::is_factory_name(&name) {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "cannot_remove_default",
                    format!("La voz '{}' no se puede eliminar.", name.to_lowercase()),
                ));
            }
            voice_store
                .remove(&name)
                .map_err(|e| CliError::new(ExitCode::NotFound, "voice_not_found", e))?;
            if json_mode {
                emit_raw_json(json!({ "status": "removed", "voice": name }));
            } else {
                println!("Voz '{}' eliminada.", name);
            }
            Ok(())
        }
    }
}

// ─── Speech ──────────────────────────────────────────────────────────

/// Captura PCM del micrófono en `spawn_blocking`: `duration` fija la
/// captura por N segundos; `None` (solo alcanzable en TTY, ver guardas de
/// `--duration`) dispara push-to-talk hasta Enter (`capture_16k_mono_pcm_until_enter`).
/// Único punto de selección, reusado por las 4 vías de captura (directa y daemon,
/// transcribe y dub) para no duplicar el `match` ni el manejo de `spawn_blocking`.
async fn capture_mic_pcm(duration: Option<u64>) -> Result<Vec<i16>, CliError> {
    tokio::task::spawn_blocking(move || {
        let svc = audio::AudioService::new();
        match duration {
            Some(secs) => svc.capture_16k_mono_pcm(secs),
            None => svc.capture_16k_mono_pcm_until_enter(),
        }
    })
    .await
    .map_err(|e| {
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_error",
            e.to_string(),
        )
    })?
    .map_err(|e| {
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_error",
            e.to_string(),
        )
    })
}

/// Bucle interactivo `--play` (RF-12.3–12.5): reproduce la toma en memoria y
/// ofrece 4 opciones a stderr — [1] mantener (reproducir de nuevo, sin
/// re-síntesis), [2] guardar (recomprobando colisión de label al guardar,
/// RF-12.4), [3] repetir con una síntesis nueva (misma vía, vía `resynthesize`)
/// y reproducir, [4] descartar. EOF o error de lectura de stdin también
/// descartan; una opción inválida avisa y repite el menú. Devuelve la ruta
/// guardada si el usuario eligió [2], o `None` si descartó (RF-12.3, exit 0 en
/// ambos casos). Reusado por la ruta directa y la daemon (P6, client-side); el
/// menú y los prompts van a stderr, nunca a stdout (P3). No introduce exit
/// codes nuevos.
async fn synthesize_play_loop<F, Fut>(
    voice: &str,
    label: &str,
    force: bool,
    speech_store: &SpeechStore,
    texto_final: &str,
    mut tmp_wav: PathBuf,
    mut resynthesize: F,
) -> Result<Option<PathBuf>, CliError>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<PathBuf, CliError>>,
{
    let reproducir = |wav: &PathBuf| -> Result<(), CliError> {
        audio::AudioService::new().play_wav(wav).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "playback_failed",
                format!("Fallo al reproducir la locución '{}': {}", label, e),
            )
        })
    };

    // RF-12.5: reproducir al entrar al bucle.
    reproducir(&tmp_wav)?;

    loop {
        eprintln!(
            "Opciones: [1] mantener (reproducir de nuevo)  [2] guardar  [3] repetir (nueva síntesis)  [4] descartar"
        );
        eprint!("Opción [1-4]: ");
        let _ = std::io::stderr().flush();

        let mut linea = String::new();
        let opcion = match std::io::stdin().read_line(&mut linea) {
            Ok(0) | Err(_) => return Ok(None), // EOF o error de lectura → descartar
            Ok(_) => linea.trim().to_string(),
        };

        match opcion.as_str() {
            "1" => {
                reproducir(&tmp_wav)?;
            }
            "2" => {
                // RF-12.4: recomprobar colisión en el instante de guardar, no
                // solo en el fast-fail previo a sintetizar.
                if !force && speech_store.find(voice, label).is_some() {
                    return Err(CliError::new(
                        ExitCode::StateConflict,
                        "label_exists",
                        format!(
                            "Ya existe una locución con la etiqueta '{}' (usa --force).",
                            label
                        ),
                    ));
                }
                let saved = speech_store
                    .save(voice, label, texto_final, &tmp_wav)
                    .map_err(|e| {
                        CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
                    })?;
                return Ok(Some(saved));
            }
            "3" => {
                tmp_wav = resynthesize().await?;
                reproducir(&tmp_wav)?;
            }
            "4" => return Ok(None),
            _ => eprintln!("Opción inválida."),
        }
    }
}

async fn handle_speech(
    json_mode: bool,
    daemon_mode: DaemonMode,
    action: SpeechCommands,
) -> Result<(), CliError> {
    let speech_store = SpeechStore::new();

    match action {
        SpeechCommands::List { voice } => {
            // Listado de locuciones: local-only; el daemon no expone GET /speech.
            require_local(daemon_mode)?;
            // Con --voice se valida el identificador (exit 2) y la existencia
            // de la voz (exit 3) antes de acotar la lectura al almacén.
            let items = match &voice {
                Some(v) => {
                    es_identificador_valido(Some(v), None)?;
                    if !VoiceStore::new().exists(v) {
                        return Err(CliError::new(
                            ExitCode::NotFound,
                            "voice_not_found",
                            format!("La voz '{}' no existe.", v),
                        ));
                    }
                    speech_store.list_by_voice(v).map_err(|e| {
                        CliError::new(ExitCode::Error, "speech_list_failed", e.to_string())
                    })?
                }
                None => speech_store.list().map_err(|e| {
                    CliError::new(ExitCode::Error, "speech_list_failed", e.to_string())
                })?,
            };
            if json_mode {
                let entries: Vec<serde_json::Value> = items
                    .iter()
                    .map(|e| {
                        json!({
                            "label": e.metadata.label,
                            "voice": e.metadata.voice,
                            "text": e.metadata.text,
                            "created_at": e.metadata.created_at,
                            "duration_secs": e.metadata.duration_secs,
                        })
                    })
                    .collect();
                emit_raw_json(json!({ "speech": entries }));
            } else {
                println!("Habla sintética albergada:");
                if items.is_empty() {
                    println!("  (ninguna locución guardada)");
                } else {
                    for e in &items {
                        println!(
                            "  - [{}] {} ({:.1}s) — «{}»",
                            e.metadata.voice,
                            e.metadata.label,
                            e.metadata.duration_secs,
                            e.metadata.text
                        );
                    }
                }
            }
            Ok(())
        }
        SpeechCommands::Transcribe {
            audio,
            mic,
            duration,
            source_language,
        } => {
            // Validación de argumentos: --audio/--mic mutuamente excluyentes, uno
            // requerido; --duration solo válido con --mic.
            if audio.is_none() && !mic {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "Debe especificarse --audio o --mic.",
                ));
            }
            // push-to-talk sin --duration se permite en TTY; sin TTY se exige --duration
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }

            // Dispatch 3 modos (Transcribe es delegable al daemon):
            // ForceDaemon → daemon (error si no responde); Auto → daemon si
            // responde, si no cae a directo; ForceDirect → local. El probe de
            // vida usa un deadline corto para que el fallback Auto→directo sea
            // prácticamente instantáneo cuando el daemon no está en ejecución.
            let client = daemon_client();
            if route_to_daemon(daemon_mode, &client).await {
                return transcribe_via_daemon(
                    json_mode,
                    &client,
                    audio.as_deref(),
                    mic,
                    duration,
                    &source_language,
                )
                .await;
            }

            // Modelo ausente -> exit 4, previo a construir el motor.
            if !ModelStore::new()
                .model_dir("parakeet-tdt-v3")
                .join("nemo128.onnx")
                .exists()
            {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "El modelo de transcripción no está provisionado (parakeet-tdt-v3, se descarga con 'setup').",
                ));
            }

            // Compilado sin soporte STT (feature off): rama de error explícita.
            // La validación de argumentos y la ausencia de modelo (exit 4) son puras
            // y ya se ejecutaron arriba; aquí solo se corta la ejecución del motor.
            #[cfg(not(feature = "native-stt"))]
            {
                Err(CliError::new(
                    ExitCode::Error,
                    "stt_unsupported",
                    "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
                ))
            }
            #[cfg(feature = "native-stt")]
            {
                let pcm = if mic {
                    capture_mic_pcm(duration).await?
                } else {
                    avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(
                        |e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_error",
                                e.to_string(),
                            )
                        },
                    )?
                };

                let engine = ParakeetEngine::new(ModelStore::new().model_dir("parakeet-tdt-v3"))
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_error",
                            e.to_string(),
                        )
                    })?;
                let language = resolve_stt_language(&source_language);
                let text = engine.transcribe(&pcm, Some(language)).map_err(|e| {
                    CliError::new(
                        ExitCode::TranscriptionFailed,
                        "transcription_error",
                        e.to_string(),
                    )
                })?;

                if json_mode {
                    emit_raw_json(json!({ "text": text, "source": source_language }));
                } else {
                    println!("{}", text);
                }
                Ok(())
            }
        }
        SpeechCommands::Synthesize {
            text,
            voice,
            output,
            label,
            force,
            play,
            source_language,
            target_language,
            temperature,
        } => {
            // RF-12.1 / RF-12.2: precondiciones puras de --play, antes de
            // cualquier síntesis (P1). No introducen exit codes nuevos.
            if play && json_mode {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--play es incompatible con --json.",
                ));
            }
            if play && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--play requiere una terminal interactiva (TTY).",
                ));
            }
            validar_temperature(temperature)?;
            // Orden de validaciones del oráculo (cli.py:659-667).
            if text.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto a sintetizar está vacío",
                ));
            }
            // Origen por defecto = destino (sin traducir).
            let source_eff = source_language.as_deref().unwrap_or(&target_language);

            // Dispatch 3 modos (Synthesize es delegable al daemon).
            let client = daemon_client();
            if route_to_daemon(daemon_mode, &client).await {
                return synthesize_via_daemon(
                    json_mode,
                    &client,
                    &text,
                    &voice,
                    &label,
                    force,
                    play,
                    &output,
                    source_eff,
                    &target_language,
                    temperature,
                )
                .await;
            }

            require_model_provisioned()?;
            let voice_store = VoiceStore::new();
            if !voice_store.exists(&voice) {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "voice_not_found",
                    format!("La voz '{}' no existe.", voice),
                ));
            }
            let label = label.to_lowercase();
            es_identificador_valido(Some(&label), None)?;
            let speech_store = SpeechStore::new();
            if !force && speech_store.find(&voice, &label).is_some() {
                return Err(CliError::new(
                    ExitCode::StateConflict,
                    "label_exists",
                    format!(
                        "Ya existe una locución con la etiqueta '{}' (usa --force).",
                        label
                    ),
                ));
            }

            let tmp_wav = std::env::temp_dir().join(format!("avi_tts_{}.wav", label));
            let engine = Qwen3TtsEngine::new(None);
            // Traducción opt-in antes de sintetizar (passthrough si coinciden).
            let texto_final = traducir_si_difiere(&text, source_eff, &target_language)?;
            engine
                .synthesize_with_temperature(&texto_final, &voice, temperature, Some(&tmp_wav))
                .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;

            let saved = if play {
                // RF-12.3–12.5: bucle interactivo, reemplaza la reproducción y
                // el guardado incondicionales. Solo alcanzable en TTY (RF-12.2).
                let resultado = synthesize_play_loop(
                    &voice,
                    &label,
                    force,
                    &speech_store,
                    &texto_final,
                    tmp_wav.clone(),
                    || async {
                        engine
                            .synthesize_with_temperature(
                                &texto_final,
                                &voice,
                                temperature,
                                Some(&tmp_wav),
                            )
                            .map_err(|e| {
                                CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
                            })?;
                        Ok(tmp_wav.clone())
                    },
                )
                .await?;
                match resultado {
                    Some(saved) => saved,
                    None => {
                        // Opción 4 / EOF: descartado, exit 0 (RF-12.3).
                        if json_mode {
                            emit_raw_json(json!({ "status": "discarded", "voice": voice }));
                        } else {
                            println!("Descartado.");
                        }
                        return Ok(());
                    }
                }
            } else {
                speech_store
                    .save(&voice, &label, &texto_final, &tmp_wav)
                    .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?
            };
            if let Some(out) = &output {
                std::fs::copy(&saved, out).map_err(|e| {
                    CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
                })?;
            }
            if json_mode {
                emit_raw_json(json!({
                    "status": "success",
                    "audio_path": saved.to_string_lossy(),
                    "voice": voice,
                }));
            } else {
                println!("Síntesis completada: {}", saved.display());
            }
            Ok(())
        }
        SpeechCommands::Say {
            text,
            voice,
            source_language,
            target_language,
            temperature,
        } => {
            validar_temperature(temperature)?;
            if text.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto a sintetizar está vacío",
                ));
            }
            // Origen por defecto = destino (sin traducir).
            let source_eff = source_language.as_deref().unwrap_or(&target_language);

            // Dispatch 3 modos (Say es delegable al daemon).
            let client = daemon_client();
            if route_to_daemon(daemon_mode, &client).await {
                return say_via_daemon(
                    json_mode,
                    &client,
                    &text,
                    &voice,
                    source_eff,
                    &target_language,
                    temperature,
                )
                .await;
            }

            require_model_provisioned()?;
            let voice_store = VoiceStore::new();
            if !voice_store.exists(&voice) {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "voice_not_found",
                    format!("La voz '{}' no existe.", voice),
                ));
            }
            let tmp_wav = std::env::temp_dir().join(format!("avi_say_{}.wav", std::process::id()));
            let engine = Qwen3TtsEngine::new(None);
            // Traducción opt-in antes de sintetizar (passthrough si coinciden).
            let texto_final = traducir_si_difiere(&text, source_eff, &target_language)?;
            engine
                .synthesize_with_temperature(&texto_final, &voice, temperature, Some(&tmp_wav))
                .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
            // Divergencia 5 corregida: `say` reproduce de verdad.
            audio::AudioService::new().play_wav(&tmp_wav).map_err(|e| {
                CliError::new(
                    ExitCode::Error,
                    "playback_failed",
                    format!("Fallo al reproducir la locución: {}", e),
                )
            })?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "reproduced",
                    "audio_path": tmp_wav.to_string_lossy(),
                    "voice": voice,
                }));
            } else {
                println!("Reproduciendo: {}", tmp_wav.display());
            }
            Ok(())
        }
        SpeechCommands::Dub {
            audio,
            mic,
            duration,
            voice,
            source_language,
            target_language,
            temperature,
        } => {
            validar_temperature(temperature)?;
            // Validaciones puras del oráculo (cli.py:562-624) — antes del despacho
            if duration.is_some() && !mic {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--duration solo es válido con --mic.",
                ));
            }
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }
            if audio.is_none() && !mic {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "Debe especificarse --audio o --mic.",
                ));
            }
            if let Some(a) = &audio {
                if !std::path::Path::new(a).is_file() {
                    return Err(CliError::new(
                        ExitCode::NotFound,
                        "audio_not_found",
                        format!("El archivo de audio '{}' no existe.", a),
                    ));
                }
            }
            // Despacho 3 modos: delega a POST /dub si daemon activo
            {
                let client = daemon_client();
                if route_to_daemon(daemon_mode, &client).await {
                    return dub_via_daemon(
                        json_mode,
                        &client,
                        audio.as_deref(),
                        mic,
                        duration,
                        &source_language,
                        &target_language,
                        temperature,
                        &voice,
                    )
                    .await;
                }
            }
            // Rama local: modelos ausentes → exit 4
            if !ModelStore::new()
                .model_dir("parakeet-tdt-v3")
                .join("nemo128.onnx")
                .exists()
            {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "El modelo de transcripción no está provisionado (parakeet-tdt-v3, se descarga con 'setup').",
                ));
            }
            require_model_provisioned()?;

            // Doblaje = transcribe→traduce→sintetiza: sin soporte STT (feature off)
            // el pipeline no puede arrancar; rama de error explícita tras las
            // validaciones puras (usage, audio existente, modelos ausentes → exit 4).
            #[cfg(not(feature = "native-stt"))]
            {
                let _ = (&voice, &source_language, &target_language);
                Err(CliError::new(
                    ExitCode::Error,
                    "stt_unsupported",
                    "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
                ))
            }
            #[cfg(feature = "native-stt")]
            {
                let pcm = if mic {
                    capture_mic_pcm(duration).await?
                } else {
                    avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(
                        |e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_error",
                                e.to_string(),
                            )
                        },
                    )?
                };
                let stt = ParakeetEngine::new(ModelStore::new().model_dir("parakeet-tdt-v3"))
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_error",
                            e.to_string(),
                        )
                    })?;
                let transcribed = stt
                    .transcribe(&pcm, Some(resolve_stt_language(&source_language)))
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_error",
                            e.to_string(),
                        )
                    })?;
                if transcribed.trim().is_empty() {
                    return Err(CliError::new(
                        ExitCode::InvalidInput,
                        "empty_text",
                        "El texto transcrito está vacío",
                    ));
                }

                // Traducción solo si source != target tras normalizar (passthrough si coinciden).
                let source = resolve_stt_language(&source_language);
                let target = resolve_stt_language(&target_language);
                let final_text = if source == target {
                    transcribed.clone()
                } else {
                    let pair = match (source, target) {
                        ("es", "en") => "es-en",
                        ("en", "es") => "en-es",
                        _ => {
                            return Err(CliError::new(
                                ExitCode::InvalidInput,
                                "unsupported_language_pair",
                                format!(
                                    "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                                    source, target
                                ),
                            ));
                        }
                    };
                    let ct2_dir = store::ct2_model_dir(pair);
                    if !store::is_ct2_provisioned(pair) {
                        return Err(CliError::new(
                            ExitCode::ModelMissing,
                            "model_missing",
                            format!(
                                "El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.",
                                ct2_dir.display(),
                                store::ct2_archivos_faltantes(pair).join(", "),
                            ),
                        ));
                    }
                    // Sin soporte de traducción (feature off) el par no-passthrough no
                    // puede resolverse: se corta con un error explícito (type `!`).
                    #[cfg(not(feature = "native-translation"))]
                    {
                        return Err(CliError::new(
                            ExitCode::Error,
                            "translation_unsupported",
                            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
                        ));
                    }
                    #[cfg(feature = "native-translation")]
                    {
                        translation::translate(&transcribed, source, target, &ct2_dir).map_err(
                            |e| {
                                CliError::new(
                                    ExitCode::TranslationFailed,
                                    "translation_failed",
                                    e.to_string(),
                                )
                            },
                        )?
                    }
                };

                let voice_store = VoiceStore::new();
                if !voice_store.exists(&voice) {
                    return Err(CliError::new(
                        ExitCode::NotFound,
                        "voice_not_found",
                        format!("La voz '{}' no existe.", voice),
                    ));
                }
                let tmp_wav =
                    std::env::temp_dir().join(format!("avi_dub_{}.wav", std::process::id()));
                let engine = Qwen3TtsEngine::new(None);
                engine
                    .synthesize_with_temperature(&final_text, &voice, temperature, Some(&tmp_wav))
                    .map_err(|e| {
                        CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
                    })?;
                audio::AudioService::new().play_wav(&tmp_wav).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "playback_failed",
                        format!("Fallo al reproducir el doblaje: {}", e),
                    )
                })?;
                if json_mode {
                    emit_raw_json(json!({
                        "status": "dubbed",
                        "text": final_text,
                        "audio_path": tmp_wav.to_string_lossy(),
                    }));
                } else {
                    println!("Doblaje reproducido: {}", tmp_wav.display());
                }
                Ok(())
            }
        }
        SpeechCommands::Play { label, voice } => {
            // Reproducción de locución persistida: local-only.
            require_local(daemon_mode)?;
            es_identificador_valido(Some(&voice), Some(&label))?;
            match speech_store.find(&voice, &label) {
                Some(entry) => {
                    audio::AudioService::new()
                        .play_wav(&entry.audio_path)
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::Error,
                                "playback_failed",
                                format!(
                                    "Fallo al reproducir la locución '{}' de la voz '{}': {}",
                                    label, voice, e
                                ),
                            )
                        })?;
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "played", "label": label, "voice": voice }),
                        );
                    } else {
                        println!("Reproduciendo locución '{}' de la voz '{}'.", label, voice);
                    }
                    Ok(())
                }
                None => Err(CliError::new(
                    ExitCode::NotFound,
                    "speech_not_found",
                    format!("La locución '{}' de la voz '{}' no existe.", label, voice),
                )),
            }
        }
        SpeechCommands::Remove { label, voice } => {
            // Borrado de locución: local-only.
            require_local(daemon_mode)?;
            es_identificador_valido(Some(&voice), Some(&label))?;
            speech_store
                .remove(&voice, &label)
                .map_err(|e| CliError::new(ExitCode::NotFound, "speech_not_found", e))?;
            if json_mode {
                emit_raw_json(json!({ "status": "removed", "label": label, "voice": voice }));
            } else {
                println!("Locución '{}' de la voz '{}' eliminada.", label, voice);
            }
            Ok(())
        }
    }
}

// ─── Daemon ──────────────────────────────────────────────────────────

async fn handle_daemon(json_mode: bool, action: DaemonCommands) -> Result<(), CliError> {
    // Corta en la raíz la herencia de los handles estándar antes de spawnear
    // ningún hijo del rol daemon. Cubre el CLI (`Start`/`Restart` → `spawn_background`)
    // y el propio daemon (`Serve` → motor), incluido `serve` lanzado bajo un pipe.
    #[cfg(windows)]
    desheredar_handles_estandar();
    match action {
        DaemonCommands::Serve {
            auto_restart,
            max_retries,
            warm_voice,
            ready_file,
        } => {
            // La dirección se resuelve por `AVI_DAEMON_PORT` (aislamiento por
            // instancia) con fallback al literal (reversión: restaurar el
            // literal directo).
            let addr: SocketAddr = resolver_addr_daemon();
            // Transporte flag+fichero: el flag designa el fichero de
            // señalización y viaja intra-proceso por env hasta
            // `run_daemon_server` (la firma del servidor queda intacta para
            // no tocar `crates/avi-daemon/tests/golden.rs`, fuera del
            // alcance). Ruta absoluta: las relativas dependen de la unidad
            // del proceso en Windows y los limpiadores de `%TEMP%` pueden
            // barrer ficheros fuera del sandbox.
            if let Some(ruta) = ready_file.as_ref() {
                let absoluta = if ruta.is_absolute() {
                    ruta.clone()
                } else {
                    std::env::current_dir()
                        .map(|c| c.join(ruta))
                        .unwrap_or_else(|_| ruta.clone())
                };
                std::env::set_var(daemon::READY_FILE_ENV, &absoluta);
            }
            // Job con cierre del árbol en el proceso longevo (Windows).
            // El handler SIGINT ya quedó instalado en `main` para todos los modos;
            // la escucha de señales del servidor cierra por la misma ruta que
            // POST `/shutdown`. En Unix `serve` cierra por esa misma ruta
            // sin pidfile ni auto-muerte del CLI (la guarda `pid != propio`
            // protege al `serve` en foreground).
            #[cfg(windows)]
            instalar_job_con_cierre_de_arbol();
            daemon::run_supervised(addr, auto_restart, max_retries, warm_voice)
                .await
                .map_err(|e| {
                    CliError::new(ExitCode::DaemonUnreachable, "daemon_error", e.to_string())
                })
        }
        DaemonCommands::Start {
            auto_restart,
            max_retries,
            warm_voice,
        } => {
            require_model_provisioned()?;
            let client = daemon_client();
            // Revalidación con reclamo matar-y-rearrancar: la vida del
            // residual se comprueba por PID vivo más probe, no por probe solo ni
            // pidfile solo. Sano → `already_running`; degradado → se reclama el
            // árbol y se rearranca con salida 0 y payload `started`.
            match clasificar_residual(&client).await {
                EstadoResidual::Sano(pid) => {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "already_running", "daemon": "running", "pid": pid }),
                        );
                    } else {
                        println!("Daemon ya en ejecución (pid {}).", pid);
                    }
                    return Ok(());
                }
                EstadoResidual::Degradado { pid, motivo } => {
                    eprintln!(
                        "Daemon residual degradado ({}): se reclama el árbol y se rearranca.",
                        motivo
                    );
                    reclamar_residual_degradado(&client, pid).await;
                }
                EstadoResidual::Parado => {}
            }
            // El hijo publica su `addr` real en el fichero ready de la
            // instancia (ruta absoluta bajo el `data_dir` vigente); el padre
            // la espera, la verifica por probe y la persiste en el pidfile.
            // Reversión: `None` en el spawn y literal en `write_daemon_pid`.
            let ready_path = ruta_fichero_ready();
            if let Some(parent) = ready_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "daemon_error",
                        format!("No se pudo crear el dir del fichero ready: {}", e),
                    )
                })?;
            }
            // Invalidar la señal previa: el fichero es determinista por
            // instancia y un contenido rancio se leería como evento válido.
            let _ = std::fs::remove_file(&ready_path);
            let pid =
                daemon::spawn_background(auto_restart, max_retries, &warm_voice, Some(&ready_path))
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "daemon_error",
                            format!("No se pudo lanzar el daemon: {}", e),
                        )
                    })?;
            // Conservar el PID hijo en memoria desde el spawn para que el
            // handler Ctrl+C lo reclame aunque aún no haya pidfile (ventana
            // spawn → await → write).
            PID_EN_MEMORIA.store(pid, std::sync::atomic::Ordering::Relaxed);
            let addr_real = esperar_addr_fichero_ready(&ready_path, DAEMON_READY_DEADLINE)
                .await
                .map_err(|e| {
                    CliError::new(
                        ExitCode::DaemonUnreachable,
                        "daemon_unreachable",
                        e.to_string(),
                    )
                })?;
            await_daemon_ready(
                &client,
                &addr_real,
                Some(&ready_path),
                DAEMON_READY_DEADLINE,
                DAEMON_POLL_INTERVAL,
            )
            .await
            .map_err(|e| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    e.to_string(),
                )
            })?;
            write_daemon_pid(pid, &addr_real, 0).map_err(|e| {
                CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!("No se pudo escribir daemon.pid: {}", e),
                )
            })?;
            if json_mode {
                emit_raw_json(json!({ "status": "started", "daemon": "running", "pid": pid }));
            } else {
                println!("Daemon iniciado correctamente (pid {}).", pid);
            }
            Ok(())
        }
        DaemonCommands::Stop => {
            // Parada unificada con deadline global (C1): graceful + árbol preciso
            // por PID + verificación a nivel de sistema. El pidfile solo se borra
            // tras muerte verificada (probe down + PID muerto/ausente); si el árbol
            // sigue vivo se conserva la pista y se falla con exit 5.
            let client = daemon_client();
            stop_daemon_and_resident().await;
            let pid = read_daemon_pid();
            let vivo = pid.map(daemon::pid_vivo).unwrap_or(false);
            let activo = daemon_activo(&client).await;
            // Los mensajes diagnostican la dirección descubierta (con
            // pidfile efímero difiere del literal; sin pidfile es idéntica).
            let addr_cli = resolver_addr_cliente();
            if !activo && !vivo {
                let _ = remove_daemon_pid_file();
                if json_mode {
                    emit_raw_json(json!({ "status": "shutdown_sent", "daemon": "stopped" }));
                } else {
                    println!("Señal de apagado enviada al daemon en {}.", addr_cli);
                }
                Ok(())
            } else {
                Err(CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    format!(
                        "El daemon no se apagó tras el deadline (pid {:?} sigue vivo en {})",
                        pid, addr_cli
                    ),
                ))
            }
        }
        DaemonCommands::Restart => {
            // Restart determinista sobre el ayudante único (C2/C3): sin doble techo
            // `timeout(5s, wait_health_down(5s))` ni kill por PID duplicado; la
            // parada unificada (deadline global) sirve a start/restart/cleanup/uninstall.
            let t_total = std::time::Instant::now();
            let budget = std::time::Duration::from_secs(12);
            let client = daemon_client();
            stop_daemon_and_resident().await;
            require_model_provisioned()?;
            // Igual que `Start`: fichero ready propio de la instancia,
            // espera de la `addr` real y persistencia en el pidfile.
            let ready_path = ruta_fichero_ready();
            if let Some(parent) = ready_path.parent() {
                std::fs::create_dir_all(parent).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "daemon_error",
                        format!("No se pudo crear el dir del fichero ready: {}", e),
                    )
                })?;
            }
            // Invalidar la señal previa (ver `Start`): sin esto el
            // `esperar` leería la `addr` de la instancia detenida.
            let _ = std::fs::remove_file(&ready_path);
            let pid =
                daemon::spawn_background(false, 3, "default", Some(&ready_path)).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "daemon_error",
                        format!("No se pudo lanzar el daemon: {}", e),
                    )
                })?;
            let elapsed = t_total.elapsed();
            let remaining = budget
                .checked_sub(elapsed)
                .unwrap_or(std::time::Duration::from_millis(800));
            // ready acotado al restante del presupuesto global — nunca >10s vigente
            let ready_deadline = std::cmp::min(remaining, DAEMON_READY_DEADLINE);
            let addr_real = esperar_addr_fichero_ready(&ready_path, ready_deadline)
                .await
                .map_err(|e| {
                    CliError::new(
                        ExitCode::DaemonUnreachable,
                        "daemon_unreachable",
                        e.to_string(),
                    )
                })?;
            let ready_res = await_daemon_ready(
                &client,
                &addr_real,
                Some(&ready_path),
                ready_deadline,
                DAEMON_POLL_INTERVAL,
            )
            .await;
            ready_res.map_err(|e| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    e.to_string(),
                )
            })?;
            write_daemon_pid(pid, &addr_real, 0).map_err(|e| {
                CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!("No se pudo escribir daemon.pid: {}", e),
                )
            })?;
            if json_mode {
                emit_raw_json(json!({ "status": "restarted", "daemon": "running", "pid": pid }));
            } else {
                println!("Daemon reiniciado (pid {}).", pid);
            }
            Ok(())
        }
        DaemonCommands::Status => {
            // GET /health → running; sin respuesta (timeout/conexión) → stopped
            // (exit 0), conservando el contrato de la fixture `cli_daemon_status.json`.
            // El probe apunta a la dirección descubierta (fallback idéntico
            // sin pidfile).
            let client = daemon_client();
            let addr_cli = resolver_addr_cliente();
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                client.get(format!("http://{}/health", addr_cli)).send(),
            )
            .await
            {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    let val: Value = tokio::time::timeout(
                        std::time::Duration::from_millis(800),
                        resp.json::<Value>(),
                    )
                    .await
                    .map_err(|_| {
                        CliError::new(
                            ExitCode::DaemonUnreachable,
                            "daemon_unreachable",
                            format!("Daemon inalcanzable en {} (timeout json)", addr_cli),
                        )
                    })?
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "daemon_error",
                            format!("Respuesta de /health no es JSON: {}", e),
                        )
                    })?;
                    let engine = val.get("engine").and_then(|e| e.as_str());
                    let warm_label = val.get("warm").and_then(|w| w.as_str());
                    let warm_error = val
                        .get("warm_error")
                        .and_then(|e| e.as_str())
                        .map(|s| s.to_string());
                    if json_mode {
                        let warm = warm_label.map(|l| (l, warm_error));
                        emit_raw_json(status_body(true, engine, warm));
                    } else {
                        println!(
                            "Daemon: en ejecución (motor: {}, warm: {}).",
                            engine.unwrap_or("desconocido"),
                            warm_label.unwrap_or("desconocido")
                        );
                    }
                    Ok(())
                }
                _ => {
                    if json_mode {
                        emit_raw_json(status_body(false, None, None));
                    } else {
                        println!("Daemon: no está en ejecución.");
                    }
                    Ok(())
                }
            }
        }
    }
}

// ─── Setup / Cleanup / Doctor ────────────────────────────────────────

async fn handle_setup(
    json_mode: bool,
    with_stt: bool,
    with_voice_cloning: bool,
    force_update: bool,
    yes: bool,
) -> Result<(), CliError> {
    let model_store = ModelStore::new();
    let voice_store = VoiceStore::new();

    // 1. Inicializar VoiceStore y directorio por defecto
    voice_store
        .ensure_initialized()
        .map_err(|e| CliError::new(ExitCode::Error, "voice_store_init_failed", e.to_string()))?;

    // 1b. --force-update: purga incondicional de snapshots pinneados + cache xet
    // antes de re-provisionar. Respeta la selección de clonado (mismo filtro que
    // el bucle de provisión). Confirmación destructiva salvo --yes/no-TTY.
    if force_update {
        if !yes && std::io::stdin().is_terminal() {
            eprint!(
                "Esto purgará los modelos descargados (~9–11,5 GB) y los re-descargará. ¿Continuar? [y/N]: "
            );
            let _ = std::io::stderr().flush();
            let mut input = String::new();
            if std::io::stdin().read_line(&mut input).is_ok() {
                let t = input.trim().to_ascii_lowercase();
                if t != "y" && t != "yes" && t != "s" && t != "si" && t != "sí" {
                    if json_mode {
                        emit_raw_json(json!({ "status": "cancelled" }));
                    } else {
                        println!("Cancelado.");
                    }
                    return Ok(());
                }
            }
        }
        for name in store::MODEL_REVISIONS
            .iter()
            .map(|(n, _, _)| *n)
            .filter(|n| *n != "qwen3-tts-0.6b-base" || with_voice_cloning)
        {
            match model_store.remove_hf_snapshot(name) {
                Ok(true) => eprintln!("Snapshot {} purgado.", name),
                Ok(false) => {}
                Err(e) => eprintln!("  ✗ No se pudo purgar {}: {}", name, e),
            }
        }
        match store::ModelStore::remove_xet_cache() {
            Ok(true) => eprintln!("Cache xet purgada."),
            Ok(false) => {}
            Err(e) => eprintln!("  ✗ No se pudo purgar cache xet: {}", e),
        }
    }

    // 2. Descargar y registrar modelos pinneados. Base es opt-in (--with-voice-cloning).
    if with_stt {
        tracing::info!("--with-stt es redundante: parakeet-tdt-v3 ya está incluido en setup");
    }
    let mut provisioned = Vec::new();
    for name in store::MODEL_REVISIONS
        .iter()
        .map(|(n, _, _)| *n)
        .filter(|n| *n != "qwen3-tts-0.6b-base" || with_voice_cloning)
    {
        // Idempotente: la presencia del snapshot HF (`is_provisioned`) es el
        // único criterio; `ensure_downloaded` resuelve desde cache sin red si ya
        // está. No hay índice `manifest.json` que escribir.
        if !model_store.is_provisioned(name) {
            store::ModelStore::ensure_downloaded(name)
                .await
                .map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "model_download_failed",
                        format!("{}: {}", name, e),
                    )
                })?;
        }
        provisioned.push(name.to_string());
    }

    // 2b. CT2 es derivado obligatorio de Marian HF en `hf_cache_dir/ct2`.
    // Incondicional cuando Marian está provisionado; idempotente por mtime solo sobre dirs
    // sanos (gate nuevo en falso ⇒ reconversión aunque ct2 > hf).
    // Determinista: fallo de conversión → setup falla con `ct2_conversion_failed`.
    for pair in &["es-en", "en-es"] {
        let hf_name = format!("marian-{}", pair);
        if !model_store.is_provisioned(&hf_name) {
            continue;
        }
        let Some(hf_snapshot) = model_store.model_snapshot_path(&hf_name) else {
            return Err(CliError::new(
                ExitCode::Error,
                "ct2_conversion_failed",
                format!(
                    "No se pudo convertir CT2 {}: snapshot HF de '{}' no resoluble — limpia la cache HF y reintenta setup",
                    pair, hf_name
                ),
            ));
        };
        if !hf_snapshot.is_dir() {
            return Err(CliError::new(
                ExitCode::Error,
                "ct2_conversion_failed",
                format!(
                    "No se pudo convertir CT2 {}: snapshot HF de '{}' ausente en '{}' — limpia la cache HF y reintenta setup",
                    pair,
                    hf_name,
                    hf_snapshot.display()
                ),
            ));
        }
        let ct2_dir = store::ct2_model_dir(pair);
        if store::is_ct2_provisioned(pair) {
            let ct2_mtime = std::fs::metadata(ct2_dir.join("model.bin"))
                .and_then(|m| m.modified())
                .ok();
            let hf_mtime = std::fs::metadata(hf_snapshot.join("pytorch_model.bin"))
                .or_else(|_| std::fs::metadata(hf_snapshot.join("model.safetensors")))
                .and_then(|m| m.modified())
                .ok();
            if let (Some(ct2_t), Some(hf_t)) = (ct2_mtime, hf_mtime) {
                if ct2_t > hf_t {
                    tracing::info!("CT2 {} ya convertido ({}), skip", pair, ct2_dir.display());
                    continue;
                }
            } else {
                tracing::info!("CT2 {} ya existe, skip", pair);
                continue;
            }
        }
        convert_marian_to_ct2(&hf_snapshot, &ct2_dir).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "ct2_conversion_failed",
                format!(
                    "No se pudo convertir CT2 {}: {} — instala ctranslate2 (pip install ctranslate2) y reintenta setup",
                    pair, e
                ),
            )
        })?;
        tracing::info!("CT2 {} convertido en {}", pair, ct2_dir.display());
    }

    if json_mode {
        emit_raw_json(json!({
            "status": "completed",
            "with_stt": with_stt,
            "models_provisioned": provisioned
        }));
    } else {
        println!(
            "Setup completado: {} modelo(s) disponibles.",
            provisioned.len()
        );
    }
    Ok(())
}

fn convert_marian_to_ct2(
    hf_snapshot: &std::path::Path,
    ct2_dir: &std::path::Path,
) -> anyhow::Result<()> {
    // Escritura atómica: el conversor vuelca en un dir temporal hermano y solo
    // tras verificar el derivado completo se renombra sobre el destino. Así un
    // fallo nunca deja un parcial que el gate acepte, y el dir previo roto se
    // sustituye entero (reparación por reconversión).
    let tmp_dir = ct2_dir.with_extension(format!("tmp-{}", std::process::id()));
    if tmp_dir.exists() {
        std::fs::remove_dir_all(&tmp_dir)?;
    }
    std::fs::create_dir_all(&tmp_dir)?;
    let convertir = || -> anyhow::Result<()> {
        // Conversión determinista a CT2 int8 vía `python -m ctranslate2.converters.transformers`,
        // con `--copy_files` para que el derivado quede autocontenido (tokenizador
        // dentro del dir CT2, no solo en el snapshot).
        let try_converter = |bin: &str| {
            std::process::Command::new(bin)
                .args([
                    "-m",
                    "ctranslate2.converters.transformers",
                    "--model",
                    &hf_snapshot.to_string_lossy(),
                    "--output_dir",
                    &tmp_dir.to_string_lossy(),
                    "--quantization",
                    "int8",
                    "--copy_files",
                    "source.spm",
                    "target.spm",
                    "--force",
                ])
                .status()
        };
        match try_converter("python") {
            Ok(s) if s.success() => {}
            Ok(s) => anyhow::bail!("converter python exit {}", s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => match try_converter("python3") {
                Ok(s) if s.success() => {}
                Ok(s) => anyhow::bail!("converter python3 exit {}", s),
                Err(e2) => anyhow::bail!("python no encontrado: {} / {}", e, e2),
            },
            Err(e) => anyhow::bail!("fallo al ejecutar converter: {}", e),
        }
        // Copia posterior verificada: si el conversor no depositó los `.spm`
        // (versión sin `--copy_files`), se copian desde el snapshot pinneado.
        for spm in ["source.spm", "target.spm"] {
            if !tmp_dir.join(spm).is_file() {
                let origen = hf_snapshot.join(spm);
                if !origen.is_file() {
                    anyhow::bail!(
                        "el snapshot {} no contiene {} (revisión inesperada) — limpia la cache HF y reintenta setup",
                        hf_snapshot.display(),
                        spm
                    );
                }
                std::fs::copy(&origen, tmp_dir.join(spm))?;
            }
        }
        // Verificación con el mismo criterio del gate antes de declarar éxito.
        let faltan = store::ct2_dir_faltantes(&tmp_dir);
        if !faltan.is_empty() {
            anyhow::bail!(
                "derivado CT2 incompleto (faltan: {}) — limpia la cache HF y reintenta setup",
                faltan.join(", ")
            );
        }
        Ok(())
    };
    if let Err(e) = convertir() {
        let _ = std::fs::remove_dir_all(&tmp_dir);
        return Err(e);
    }
    if ct2_dir.exists() {
        std::fs::remove_dir_all(ct2_dir)?;
    }
    std::fs::rename(&tmp_dir, ct2_dir)?;
    Ok(())
}

async fn handle_cleanup(
    json_mode: bool,
    voices: bool,
    synthetic_speech: bool,
    model: bool,
    all: bool,
    dry_run: bool,
    yes: bool,
) -> Result<(), CliError> {
    // Gate sin flags → InvalidInput exit 2 sin borrar (paridad oráculo 7542962, CONTRACT §11)
    if !voices && !synthetic_speech && !model && !all {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "usage_error",
            "cleanup requiere al menos un flag: --voices, --synthetic-speech, --model o --all",
        ));
    }
    let do_voices = voices || all;
    let do_speech = synthetic_speech || all;
    let do_model = model || all;

    // Construir lista de rutas candidatas existentes para --dry-run y payload removed
    let mut candidates: Vec<PathBuf> = Vec::new();
    // --model: snapshots HF + xet + ct2 + legacy data_dir/models
    if do_model {
        for (_, repo, _) in store::MODEL_REVISIONS {
            let p = store::hf_cache_dir().join(format!("models--{}", repo.replace('/', "--")));
            if p.exists() {
                candidates.push(p);
            }
        }
        let xet = store::xet_cache_dir();
        if xet.exists() {
            candidates.push(xet);
        }
        let locks = store::hf_cache_dir().join(".locks");
        if locks.exists() {
            candidates.push(locks);
        }
        let ct2 = store::ct2_cache_dir();
        if ct2.exists() {
            candidates.push(ct2);
        }
        let legacy_models = store::data_dir().join("models");
        if legacy_models.exists() {
            candidates.push(legacy_models);
        }
    }
    // --voices: voces no-fábrica + arrastre speech/<voz> excepto default
    if do_voices {
        let voice_base = store::data_dir().join("voices");
        if voice_base.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&voice_base) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let name = entry.file_name().to_string_lossy().to_lowercase();
                        if store::is_factory_name(&name) {
                            continue;
                        }
                        candidates.push(entry.path());
                        // Arrastre speech/<voz> solo si no se va a borrar speech entero
                        if !do_speech && name != "default" {
                            let sp = store::data_dir().join("speech").join(&name);
                            if sp.exists() {
                                candidates.push(sp);
                            }
                        }
                    }
                }
            }
        }
    }
    // --synthetic-speech: speech/ entero (subsumido si ya hay arrastre, deduplicado arriba)
    if do_speech {
        let sp_root = store::data_dir().join("speech");
        if sp_root.exists() {
            candidates.push(sp_root);
        }
    }
    // Temp huérfano siempre candidato auxiliar (no categoría, pero se limpia con cualquier cleanup)
    {
        let tmp = std::env::temp_dir();
        if let Ok(entries) = std::fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("avi_") || name.starts_with("ai-voice-interconnector-install-")
                {
                    candidates.push(entry.path());
                }
            }
        }
    }
    // Deduplicar candidatos por display (evita duplicar speech/<voz> bajo speech/ root)
    {
        let mut seen = std::collections::HashSet::new();
        candidates.retain(|p| seen.insert(p.display().to_string()));
    }
    let removed_display: Vec<String> = candidates.iter().map(|p| p.display().to_string()).collect();

    // Gate --dry-run: listar sin borrar, exit 0, payload con removed/dry_run
    if dry_run {
        if json_mode {
            emit_raw_json(json!({
                "status": "cleanup_complete",
                "removed": removed_display,
                "dry_run": true
            }));
        } else {
            if candidates.is_empty() {
                println!("Nada para limpiar (dry-run).");
            } else {
                println!("Dry-run: se eliminarían {} ruta(s):", candidates.len());
                for p in &candidates {
                    println!("  {}", p.display());
                }
            }
        }
        return Ok(());
    }

    // Confirmación si no es --yes y hay TTY (patrón handle_uninstall:1688)
    if !yes && std::io::stdin().is_terminal() {
        eprint!("Esto eliminará datos seleccionados. ¿Continuar? [y/N]: ");
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            let t = input.trim().to_ascii_lowercase();
            if t != "y" && t != "yes" && t != "s" && t != "si" && t != "sí" {
                if json_mode {
                    emit_raw_json(json!({ "status": "cancelled" }));
                } else {
                    println!("Cancelado.");
                }
                return Ok(());
            }
        }
    }

    // 0. Parar daemon graceful si está vivo (libera puerto; reutiliza helper compartido)
    stop_daemon_and_resident().await;

    let mut actually_removed: Vec<String> = Vec::new();

    // Branch --model
    if do_model {
        let model_store = ModelStore::new();
        for name in store::MODEL_REVISIONS.iter().map(|(n, _, _)| *n) {
            match model_store.remove_hf_snapshot(name) {
                Ok(true) => {
                    eprintln!("Snapshot {} eliminado.", name);
                    actually_removed.push(format!("hf:{}", name));
                }
                Ok(false) => {}
                Err(e) => eprintln!("  ✗ No se pudo borrar {}: {}", name, e),
            }
        }
        match store::ModelStore::remove_xet_cache() {
            Ok(true) => {
                eprintln!("Cache xet eliminada.");
                actually_removed.push(store::xet_cache_dir().display().to_string());
            }
            Ok(false) => {}
            Err(e) => eprintln!("  ✗ No se pudo borrar cache xet: {}", e),
        }
        match store::remove_ct2_cache() {
            Ok(true) => {
                eprintln!("Cache CT2 eliminada.");
                actually_removed.push(store::ct2_cache_dir().display().to_string());
            }
            Ok(false) => {}
            Err(e) => eprintln!("  ✗ No se pudo borrar cache CT2: {}", e),
        }
        let legacy_models = store::data_dir().join("models");
        if legacy_models.exists() {
            let _ = std::fs::remove_dir_all(&legacy_models);
            actually_removed.push(legacy_models.display().to_string());
        }
    }
    // Branch --voices (con arrastre speech/<voz> excepto default; preserva FACTORY_VOICES)
    if do_voices {
        let voice_base = store::data_dir().join("voices");
        if voice_base.is_dir() {
            if let Ok(entries) = std::fs::read_dir(&voice_base) {
                for entry in entries.flatten() {
                    if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                        let name = entry.file_name().to_string_lossy().to_lowercase();
                        if store::is_factory_name(&name) {
                            continue;
                        }
                        let p = entry.path();
                        if p.exists() {
                            let _ = std::fs::remove_dir_all(&p);
                            actually_removed.push(p.display().to_string());
                        }
                        if !do_speech && name != "default" {
                            let sp = store::data_dir().join("speech").join(&name);
                            if sp.exists() {
                                let _ = std::fs::remove_dir_all(&sp);
                                actually_removed.push(sp.display().to_string());
                            }
                        }
                    }
                }
            }
        }
    }
    // Branch --synthetic-speech: speech/ entero
    if do_speech {
        let sp_root = store::data_dir().join("speech");
        if sp_root.exists() {
            let _ = std::fs::remove_dir_all(&sp_root);
            actually_removed.push(sp_root.display().to_string());
        }
    }
    // Temp huérfano (siempre que haya borrado selectivo)
    {
        let tmp = std::env::temp_dir();
        if let Ok(entries) = std::fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("avi_") || name.starts_with("ai-voice-interconnector-install-")
                {
                    let p = entry.path();
                    let disp = p.display().to_string();
                    if p.is_dir() {
                        let _ = std::fs::remove_dir_all(&p);
                    } else {
                        let _ = std::fs::remove_file(&p);
                    }
                    // Solo registrar si existía (ya estaba en candidates)
                    if removed_display.contains(&disp) {
                        actually_removed.push(disp);
                    }
                }
            }
        }
    }
    // Si no hay payload previo de dry-run, emitir removed real o candidates como fallback
    let final_removed = if actually_removed.is_empty() {
        removed_display.clone()
    } else {
        actually_removed
    };
    if json_mode {
        emit_raw_json(json!({
            "status": "cleanup_complete",
            "removed": final_removed,
            "dry_run": false
        }));
    } else {
        println!("Limpieza de modelos/caché completada.");
    }
    Ok(())
}

/// Estado del residual al arrancar: la vida real se determina por PID vivo más
/// probe, no por probe solo ni pidfile solo.
enum EstadoResidual {
    /// Probe responde y el PID de la pista está vivo: instancia sana única.
    Sano(u32),
    /// Probe y PID discrepan (colgado, pista rancia o sin pista): reclama y rearranca.
    Degradado {
        pid: Option<u32>,
        motivo: &'static str,
    },
    /// Sin probe ni proceso: vía libre para arranque fresco.
    Parado,
}

/// Predicado puro (testeable sin daemon): ante un `Parado` por probe/PID
/// (sin probe ni proceso del daemon), hay residente-solo si el residente sigue
/// vivo (por su PID registrado) — entonces no hay vía libre, sino degradado
/// para reclamo.
#[allow(dead_code)]
fn parado_con_residente_es_degradado(
    probe_daemon: bool,
    pid_vivo: bool,
    residente_vivo: bool,
) -> bool {
    !probe_daemon && !pid_vivo && residente_vivo
}

/// Clasifica el residual del daemon: sano, degradado o parado.
/// Ante `Parado` se busca al residente por su PID registrado antes de declarar
/// vía libre; con residente vivo es degradado residente-solo para reclamo.
async fn clasificar_residual(client: &reqwest::Client) -> EstadoResidual {
    // Recuperación de reclamo: con pidfile perdido (padre caído sin
    // limpiar), el `daemon.ready` sobrevive y conserva el PID del árbol
    // efímero. El fallback solo aplica sin pidfile; `pid_vivo` gatea después,
    // así que un ready rancio con PID muerto sigue cayendo a `Parado`.
    let pid = read_daemon_pid().or_else(|| leer_pid_ready(&ruta_fichero_ready()));
    // El probe apunta a la dirección descubierta (fallback idéntico sin
    // pidfile; vía nueva solo con pidfile vivo de addr efímera).
    let addr_cli = resolver_addr_cliente();
    let probe = probe_health(client, &addr_cli).await;
    // `Parado` con residente vivo no es vía libre.
    if !probe && !pid.map(daemon::pid_vivo).unwrap_or(false) && residente_vivo_por_pid() {
        return EstadoResidual::Degradado {
            pid,
            motivo: "residente vivo sin daemon (Parado con resident_pid vivo)",
        };
    }
    match pid {
        Some(p) if probe && daemon::pid_vivo(p) => EstadoResidual::Sano(p),
        None if !probe => EstadoResidual::Parado,
        Some(p) if !probe && !daemon::pid_vivo(p) => EstadoResidual::Parado,
        _ => {
            let motivo = if probe && pid.is_none() {
                "probe responde sin pidfile"
            } else if probe {
                "probe responde con PID muerto"
            } else {
                "PID vivo sin probe (colgado)"
            };
            EstadoResidual::Degradado { pid, motivo }
        }
    }
}

/// ¿Sigue vivo el residente registrado en `daemon.pid`? Identidad estable del
/// residente: el `resident_pid` del pidfile por instancia. Sin PID registrado
/// (pidfile perdido tras aborto duro) devuelve `false`; en ese caso la
/// ausencia se confirma con el barrido por imagen de último recurso
/// (`avi_tts::resident::barrer_residente_por_imagen`), no con este predicado.
fn residente_vivo_por_pid() -> bool {
    let pid = read_resident_pid();
    pid != 0 && avi_tts::resident::pid_vivo_residente(pid)
}

/// Predicado puro del reclamo Unix ante líder muerto (testeable sin
/// plataforma): el reclamo queda verificado cuando el probe del daemon está
/// caído, el PID del líder está muerto y el residente ya no está vivo (por su
/// PID registrado, tras el barrido por imagen cuando no hay PID).
#[allow(dead_code)]
fn reclamo_unix_verificado(probe_daemon: bool, pid_vivo: bool, residente_vivo: bool) -> bool {
    !probe_daemon && !pid_vivo && !residente_vivo
}

/// Reclamo matar-y-rearrancar ante residual degradado: mata el árbol
/// preciso por PID con verificación y deja vía libre para rearrancar desde cero.
/// Sin kill por imagen para el daemon (comparte imagen con el CLI: se auto-mataría);
/// el residente `qwen_tts` se reclama por su PID registrado en `daemon.pid`
/// (árbol preciso + verificación por `resident_pid` muerto) y, sin PID
/// registrado, por el barrido por imagen de último recurso
/// (`barrer_residente_por_imagen`, seguro por imagen propia del residente).
/// No emite payload.
///
/// En Unix el daemon nace líder de sesión (`setsid`) y el residente hereda
/// su grupo; muerto el líder, el grupo se disuelve y el residente reparentado
/// sobrevive fuera del alcance del reclamo solo-por-PID-vivo. Ante líder muerto
/// se reclama además por grupo (`kill -9 -<pgid>` vía `matar_arbol_por_pid`,
/// que ya mata al grupo en Unix) con verificación por residente muerto más PID
/// sin viveza. El runtime Unix se verifica en CI; aquí quedan compilación,
/// revisión lógica y unitarios no-plataformeros (prohibido simular Unix).
/// Ante `Parado` con residente vivo (PID registrado o imagen) se reclama su
/// árbol por PID registrado —o por imagen si no hay PID— antes de declarar
/// fresco (imagen del daemon prohibida; sólo el residente tiene imagen propia).
async fn reclamar_residual_degradado(client: &reqwest::Client, pid: Option<u32>) {
    let inicio = std::time::Instant::now();
    // Graceful y verificación contra la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    // 1) Graceful breve si el probe responde (no hereda el timeout de 120 s).
    if probe_health(client, &addr_cli).await {
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(1500),
            client.post(format!("http://{}/shutdown", addr_cli)).send(),
        )
        .await;
        let restante = STOP_DEADLINE_GLOBAL
            .checked_sub(inicio.elapsed())
            .unwrap_or(std::time::Duration::from_millis(500));
        let espera = std::cmp::min(restante, std::time::Duration::from_secs(3));
        let _ = tokio::time::timeout(espera, wait_health_down(client, espera)).await;
    }
    // 2) Árbol preciso por PID con guarda anti-auto-muerte (imagen compartida).
    if let Some(p) = pid {
        if p != 0 && p != std::process::id() && daemon::pid_vivo(p) {
            daemon::matar_arbol_por_pid(p);
        }
    }
    // 2b) En Unix: ante líder muerto con posible residente reparentado vivo,
    // reclamar por grupo aunque el PID ya esté muerto (reutiliza la primitiva
    // de grupo de `matar_arbol_por_pid`; la vía feliz Windows queda intacta).
    #[cfg(unix)]
    if let Some(p) = pid {
        if p != 0 && p != std::process::id() && !daemon::pid_vivo(p) {
            daemon::matar_arbol_por_pid(p);
        }
    }
    // 2c) Residente por identidad estable: si hay PID registrado vivo se mata
    // su árbol preciso; sin PID (pidfile perdido) el último recurso es el
    // barrido por imagen `qwen_tts` (seguro por imagen propia). La verificación
    // por `resident_pid` muerto vive en el paso 3.
    let residente = read_resident_pid();
    if residente != 0
        && residente != std::process::id()
        && avi_tts::resident::pid_vivo_residente(residente)
    {
        avi_tts::resident::matar_arbol_residente_por_pid(residente);
    } else if residente == 0 {
        avi_tts::resident::barrer_residente_por_imagen();
    }
    // 3) Verificación con el restante del deadline global (probe down + PID muerto;
    // en Unix además residente muerto ante líder muerto).
    while inicio.elapsed() < STOP_DEADLINE_GLOBAL {
        let vivo = pid.map(daemon::pid_vivo).unwrap_or(false);
        let probe = probe_health(client, &addr_cli).await;
        #[cfg(unix)]
        {
            if reclamo_unix_verificado(probe, vivo, residente_vivo_por_pid()) {
                break;
            }
        }
        #[cfg(not(unix))]
        {
            if !probe && !vivo {
                break;
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
}

/// Fuente única de verdad para detener el daemon residente y `qwen_tts` huérfano,
/// al servicio de `start` (reclamo), `restart`, `cleanup` y `uninstall`.
///
/// Parada unificada con deadline global (`STOP_DEADLINE_GLOBAL`): graceful
/// (`POST /shutdown` acotado + espera de `/health` down) si el daemon responde;
/// árbol preciso por PID (`taskkill /F /T /PID` en Windows, `kill -9` al grupo
/// en Unix) con guarda anti-auto-muerte cuando sigue vivo; sin PID del
/// daemon pero con residente vivo se reclama su árbol por PID registrado, o por
/// imagen `qwen_tts` como último recurso sin PID (imagen del daemon prohibida:
/// la comparte con el CLI); verificación posterior a nivel de sistema (probe +
/// `pid_vivo` + `resident_pid` muerto).
///
/// El pidfile solo se borra tras muerte verificada o pista rancia reconciliada
/// (PID muerto + probe down); si el árbol sigue vivo se conserva la pista.
/// Sin kill por imagen para el daemon (imagen compartida con el CLI); el
/// residente `qwen_tts` (imagen propia) se reclama en `avi-tts` por PID
/// registrado, con barrido por imagen como último recurso sin PID.
///
/// Por qué el fallback es por PID y nunca por imagen: daemon y CLI comparten la misma
/// imagen `ai-voice-interconnector.exe` (el daemon es el mismo binario lanzado con
/// `daemon serve`), así que `taskkill /IM` no puede distinguirlos y mataba al propio
/// invocador (bug v0.18.10–v0.18.25 en `uninstall --force`). La guarda
/// `pid != process::id()` previene la auto-muerte incluso si el PID leído fuera el del
/// propio proceso.
async fn stop_daemon_and_resident() {
    let inicio = std::time::Instant::now();
    let client = daemon_client();
    // Graceful contra la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    // 1) Graceful acotado si responde (no hereda el timeout de 120 s).
    if daemon_activo(&client).await {
        let _ = tokio::time::timeout(
            std::time::Duration::from_millis(1500),
            client.post(format!("http://{}/shutdown", addr_cli)).send(),
        )
        .await;
        let restante = STOP_DEADLINE_GLOBAL
            .checked_sub(inicio.elapsed())
            .unwrap_or(std::time::Duration::from_millis(500));
        let espera = std::cmp::min(restante, std::time::Duration::from_secs(3));
        let _ = tokio::time::timeout(espera, wait_health_down(&client, espera)).await;
    }
    // 2) Árbol preciso por PID si sigue vivo (ambas plataformas, con guarda).
    let pid = read_daemon_pid();
    let vivo = pid.map(daemon::pid_vivo).unwrap_or(false);
    let sigue_activo = daemon_activo(&client).await;
    if sigue_activo || vivo {
        if let Some(p) = pid {
            if p != 0 && p != std::process::id() {
                daemon::matar_arbol_por_pid(p);
            }
        }
    }
    // 2b) Residente por identidad estable. Si el PID registrado sigue vivo se
    // mata su árbol preciso; sin PID (pidfile perdido) el último recurso es el
    // barrido por imagen `qwen_tts` (seguro por imagen propia; imagen del daemon
    // prohibida). La verificación por `resident_pid` muerto vive en el paso 3.
    let residente = read_resident_pid();
    if residente != 0
        && residente != std::process::id()
        && avi_tts::resident::pid_vivo_residente(residente)
    {
        avi_tts::resident::matar_arbol_residente_por_pid(residente);
    } else if residente == 0 {
        avi_tts::resident::barrer_residente_por_imagen();
    }
    // 3) Verificación con el restante del deadline global.
    while inicio.elapsed() < STOP_DEADLINE_GLOBAL {
        let vivo_ahora = read_daemon_pid().map(daemon::pid_vivo).unwrap_or(false);
        if !daemon_activo(&client).await && !vivo_ahora && !residente_vivo_por_pid() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    // 4) Borrado solo tras muerte verificada o pista rancia reconciliada.
    let pid_final = read_daemon_pid();
    let vivo_final = pid_final.map(daemon::pid_vivo).unwrap_or(false);
    if !daemon_activo(&client).await && !vivo_final {
        let _ = remove_daemon_pid_file();
    }
}

async fn handle_uninstall(json_mode: bool, force: bool) -> Result<(), CliError> {
    // Confirmación interactiva si no es --force/--yes y hay TTY
    if !force && std::io::stdin().is_terminal() {
        eprint!("Esto eliminará datos (modelos, voces, locuciones), el binario y la integración PATH. ¿Continuar? [y/N]: ");
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let mut input = String::new();
        if std::io::stdin().read_line(&mut input).is_ok() {
            let t = input.trim().to_ascii_lowercase();
            if t != "y" && t != "yes" && t != "s" && t != "si" {
                if json_mode {
                    emit_raw_json(json!({ "status": "cancelled" }));
                } else {
                    println!("Cancelado.");
                }
                return Ok(());
            }
        }
    }

    // 0. Parar daemon graceful si está vivo (helper compartido con `cleanup`; el
    // fallback de daemon colgado mata por PID, nunca por imagen compartida con el CLI)
    stop_daemon_and_resident().await;

    // 1. Datos de usuario (incluye modelos, voces, locuciones)
    let data = store::data_dir();
    if data.exists() {
        std::fs::remove_dir_all(&data).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "uninstall_failed",
                format!("No se pudo borrar {}: {}", data.display(), e),
            )
        })?;
    }

    // 1b. Snapshots HF de los modelos pinneados (~/.cache/huggingface/hub)
    {
        let model_store = ModelStore::new();
        for name in store::MODEL_REVISIONS.iter().map(|(n, _, _)| *n) {
            if let Err(e) = model_store.remove_hf_snapshot(name) {
                eprintln!("  ✗ No se pudo borrar snapshot {}: {}", name, e);
            }
        }
        // 1c. Cache xet + locks
        match store::ModelStore::remove_xet_cache() {
            Ok(true) => eprintln!("Cache xet eliminada."),
            Ok(false) => {}
            Err(e) => eprintln!("  ✗ No se pudo borrar cache xet: {}", e),
        }
        // Temp huérfano
        {
            let tmp = std::env::temp_dir();
            if let Ok(entries) = std::fs::read_dir(&tmp) {
                for entry in entries.flatten() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    if name.starts_with("avi_")
                        || name.starts_with("ai-voice-interconnector-install-")
                    {
                        let p = entry.path();
                        if p.is_dir() {
                            let _ = std::fs::remove_dir_all(&p);
                        } else {
                            let _ = std::fs::remove_file(&p);
                        }
                    }
                }
            }
        }
    }

    // 2. Integración por SO (binario + PATH)
    #[cfg(unix)]
    {
        let home = PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()));
        let link = home.join(".local/bin/ai-voice-interconnector");
        // `is_symlink` requiere `symlink_metadata`; basta con intentar borrar si existe
        if link.exists() || std::fs::symlink_metadata(&link).is_ok() {
            let _ = std::fs::remove_file(&link);
        }
        let install_dir = home.join(".local/opt/ai-voice-interconnector");
        if install_dir.exists() {
            let _ = std::fs::remove_dir_all(&install_dir);
        }
        // Fallback: si el binario se ejecuta desde otro prefijo, intenta borrar su directorio padre
        if let Ok(exe) = std::env::current_exe() {
            if let Some(parent) = exe.parent() {
                if parent != install_dir && parent.join("ai-voice-interconnector").exists() {
                    let _ = std::fs::remove_dir_all(parent);
                }
            }
        }
    }
    #[cfg(windows)]
    {
        // Fuente canónica única — espejo de `install-windows.ps1:Get-InstallDir`
        // y `avi-store::windows_install_dir`. Sin `join("Programs/ai-...")` mixto.
        let install_dir = store::windows_install_dir();
        // H2 determinista: sin `let _ =`, si falla propaga `path_cleanup_failed`
        remove_windows_user_path(&install_dir).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "path_cleanup_failed",
                format!(
                    "No se pudo limpiar PATH de {}: {}",
                    install_dir.display(),
                    e
                ),
            )
        })?;
        // H4 determinista: si el `exe` vivo está dentro de `install_dir`,
        // no se puede `remove_dir_all` sin `PermissionDenied` — se delega a
        // helper desacoplado `Wait-Process PID` + `Remove-Item -LiteralPath`.
        // Si el `exe` no está dentro (sandbox de `cargo test`), borrado
        // síncrono determinista. Sin aviso `Bórralo manualmente`.
        if install_dir.exists() {
            let inside = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.canonicalize().ok())
                .and_then(|exe| {
                    install_dir
                        .canonicalize()
                        .ok()
                        .map(|dir| exe.starts_with(dir))
                })
                .unwrap_or(false);
            if inside {
                // Corta la herencia de los handles estándar antes de spawnear el
                // helper: `spawn_uninstall_helper` vive fuera de
                // `handle_daemon`, así que replica aquí el corte para que el `.ps1`
                // no retenga el stdio del proceso que lanzó el uninstall.
                desheredar_handles_estandar();
                daemon::spawn_uninstall_helper(&install_dir, std::process::id()).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "uninstall_failed",
                        format!(
                            "No se pudo programar el borrado de {}: {}",
                            install_dir.display(),
                            e
                        ),
                    )
                })?;
            } else {
                std::fs::remove_dir_all(&install_dir).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "uninstall_failed",
                        format!("No se pudo borrar {}: {}", install_dir.display(), e),
                    )
                })?;
            }
        }
    }

    if json_mode {
        emit_raw_json(json!({ "status": "uninstalled" }));
    } else {
        println!("Desinstalación completada.");
    }
    Ok(())
}

#[cfg(windows)]
fn remove_windows_user_path(dir: &std::path::Path) -> Result<(), String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let env = hkcu
        .open_subkey_with_flags(
            "Environment",
            winreg::enums::KEY_READ | winreg::enums::KEY_WRITE,
        )
        .map_err(|e| e.to_string())?;
    let path: String = env.get_value("Path").unwrap_or_default();
    let target_key = store::canonical_path_key(dir);
    let filtered: Vec<String> = path
        .split(';')
        .filter(|s| {
            if s.is_empty() {
                return false;
            }
            store::canonical_path_key(std::path::Path::new(s)) != target_key
        })
        .map(|s| s.to_string())
        .collect();
    let new_path = filtered.join(";");
    if new_path != path {
        env.set_value("Path", &new_path)
            .map_err(|e| e.to_string())?;
        // Notificar al sistema del cambio de entorno (WM_SETTINGCHANGE)
        unsafe {
            use windows_sys::Win32::UI::WindowsAndMessaging::{
                SendMessageTimeoutW, HWND_BROADCAST, WM_SETTINGCHANGE,
            };
            let wide: Vec<u16> = "Environment\0".encode_utf16().collect();
            // SMTO_ABORTIFHUNG = 0x0002
            SendMessageTimeoutW(
                HWND_BROADCAST,
                WM_SETTINGCHANGE,
                0,
                wide.as_ptr() as isize,
                2,
                5000,
                std::ptr::null_mut(),
            );
        }
    }
    Ok(())
}

fn handle_doctor(json_mode: bool) -> Result<(), CliError> {
    let model_store = ModelStore::new();
    let voice_store = VoiceStore::new();
    // Ruta de caché resuelta: auditable (la app decide, no el fallback de hf-hub)
    let hf_cache = store::hf_cache_dir();

    // Chequeos reales de entorno
    let mut issues = Vec::new();

    // Verificar que el directorio de datos existe y es escribible
    let data_dir = store::data_dir();
    if !data_dir.exists() {
        issues.push("Directorio de datos no existe");
    }

    // Verificar los 4 modelos pinneados (snapshot HF en hf_cache_dir) y su derivado CT2 obligatorio
    if !model_store.is_provisioned("qwen3-tts-0.6b") {
        issues.push("Modelo TTS (Qwen3-TTS 0.6B) no provisionado");
    }
    if !model_store.is_provisioned("parakeet-tdt-v3") {
        issues.push("Modelo STT (Parakeet TDT v3) no provisionado");
    }
    if !model_store.is_provisioned("marian-es-en") {
        issues.push("Modelo traducción es→en (Marian) no provisionado");
    } else if !store::is_ct2_provisioned("es-en") {
        issues.push("Modelo CT2 es→en incompleto en 'hf_cache_dir/ct2/opus-mt-es-en' (exige model.bin más tokenizer.json o source.spm+target.spm) — ejecuta setup");
    }
    if !model_store.is_provisioned("marian-en-es") {
        issues.push("Modelo traducción en→es (Marian) no provisionado");
    } else if !store::is_ct2_provisioned("en-es") {
        issues.push("Modelo CT2 en→es incompleto en 'hf_cache_dir/ct2/opus-mt-en-es' (exige model.bin más tokenizer.json o source.spm+target.spm) — ejecuta setup");
    }
    // Base opt-in: WARN si falta, no FAIL
    let base_ready = model_store.is_provisioned("qwen3-tts-0.6b-base");
    let base_status = if base_ready {
        "ready"
    } else {
        "missing_opt_in"
    };

    // Verificar voces
    if let Err(_e) = voice_store.list() {
        issues.push("Error al listar voces");
    }

    if json_mode {
        emit_raw_json(json!({
            "status": if issues.is_empty() { "ok" } else { "failed" },
            "data_dir": data_dir.to_string_lossy(),
            "hf_cache": hf_cache.to_string_lossy(),
            "issues": issues,
            "base_status": base_status,
        }));
        if issues.is_empty() {
            Ok(())
        } else {
            Err(CliError::new(
                ExitCode::Error,
                "doctor_checks_failed",
                "Chequeos de entorno fallaron",
            ))
        }
    } else if issues.is_empty() {
        if base_ready {
            println!("Diagnóstico: todo correcto.");
        } else {
            println!("Diagnóstico: todo correcto. [WARN] Modelo Base de clonado no provisionado (usa setup --with-voice-cloning).");
        }
        println!("Cache HF: {}", hf_cache.display());
        Ok(())
    } else {
        for issue in &issues {
            eprintln!("  ✗ {}", issue);
        }
        if !base_ready {
            eprintln!("  ⚠ [WARN] Modelo Base de clonado no provisionado (usa setup --with-voice-cloning).");
        }
        eprintln!("Cache HF: {}", hf_cache.display());
        Err(CliError::new(
            ExitCode::Error,
            "doctor_checks_failed",
            "Chequeos de entorno fallaron",
        ))
    }
}

// ─── Helpers ─────────────────────────────────────────────────────────

fn require_model_provisioned() -> Result<(), CliError> {
    let model_store = ModelStore::new();
    if !model_store.is_provisioned("qwen3-tts-0.6b") {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            "El modelo de síntesis TTS no está provisionado. Ejecuta 'setup' primero.",
        ));
    }
    Ok(())
}

/// Valida identificadores de voz/etiqueta contra el regex del oráculo
/// (`^[A-Za-z0-9._-]+$`; paridad con el oráculo) → exit 2.
fn es_identificador_valido(ids: Option<&str>, mas: Option<&str>) -> Result<(), CliError> {
    for id in ids.into_iter().chain(mas) {
        if id.is_empty()
            || !id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        {
            return Err(CliError::new(
                ExitCode::InvalidInput,
                "invalid_identifier",
                format!("Identificador inválido: '{}'.", id),
            ));
        }
    }
    Ok(())
}

fn daemon_pid_path() -> PathBuf {
    store::data_dir().join("daemon.pid")
}

/// Escribe el pidfile de forma atómica (escritura tardía pero atómica por
/// rename); el handler Ctrl+C ya no depende solo de él gracias al PID en memoria.
/// El pidfile extiende el esquema plano con `resident_pid`. Al arrancar solo se
/// conoce el PID del daemon y el residente es 0/desconocido; el daemon lo
/// actualiza en disco al arrancar el residente (`arrancar_residente`).
/// `addr` es la dirección REAL publicada por el hijo en el fichero ready:
/// con puerto efímero difiere del literal `DAEMON_ADDR`.
fn write_daemon_pid(pid: u32, addr: &str, resident_pid: u32) -> anyhow::Result<()> {
    let path = daemon_pid_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pid.tmp");
    let content = serde_json::json!({
        "pid": pid,
        "addr": addr,
        "started_at": chrono::Utc::now().to_rfc3339(),
        "resident_pid": resident_pid
    });
    std::fs::write(&tmp, serde_json::to_string_pretty(&content)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

fn read_daemon_pid() -> Option<u32> {
    let path = daemon_pid_path();
    let content = std::fs::read_to_string(&path).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    v.get("pid")?.as_u64().map(|n| n as u32)
}

/// Lee el PID del residente registrado en el pidfile. Lectura tolerante:
/// esquema viejo sin el campo, fichero ausente o valor inválido = 0/desconocido.
fn read_resident_pid() -> u32 {
    let path = daemon_pid_path();
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

/// Lee la `addr` publicada en el pidfile. Lectura tolerante: fichero
/// ausente, ilegible o esquema viejo sin el campo = `None` (el llamante cae
/// al default, nunca falla).
fn leer_addr_pidfile() -> Option<String> {
    let content = std::fs::read_to_string(daemon_pid_path()).ok()?;
    let v: Value = serde_json::from_str(&content).ok()?;
    let addr = v.get("addr")?.as_str()?.trim();
    if addr.is_empty() {
        return None;
    }
    Some(addr.to_string())
}

/// Resuelve la dirección del cliente CLI: `addr` del pidfile cuando existe; sin pidfile usa
/// `DAEMON_ADDR` con comportamiento idéntico al actual. Solo el caso
/// "pidfile vivo con addr efímera" toma la vía nueva.
fn resolver_addr_cliente() -> String {
    leer_addr_pidfile().unwrap_or_else(|| DAEMON_ADDR.to_string())
}

/// Lee el `pid` publicado en el fichero ready (recuperación de reclamo):
/// habilita reclamar el árbol de un daemon efímero cuyo pidfile se perdió
/// (padre caído sin limpiar), única pista de PID cuando `addr` ya no es
/// descubrible. Tolerante: fichero ausente, ilegible, sin campo o `0` = `None`.
/// Reversión: quitar esta función y su uso en `clasificar_residual`.
fn leer_pid_ready(ruta: &std::path::Path) -> Option<u32> {
    let contenido = std::fs::read_to_string(ruta).ok()?;
    for linea in contenido.lines() {
        if let Some(v) = linea.trim().strip_prefix("pid=") {
            if let Ok(p) = v.trim().parse::<u32>() {
                if p != 0 {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Ruta del fichero ready de esta instancia: absoluta bajo el
/// `data_dir` vigente, de modo que cada sandbox (`AVI_DATA_DIR`) posee el
/// suyo sin depender de la unidad del proceso ni de `%TEMP%`.
fn ruta_fichero_ready() -> PathBuf {
    store::data_dir().join("daemon.ready")
}

/// Lee la `addr` del fichero ready de forma tolerante (lado producto):
/// ausente o a medio escribir = aún-no-listo (`None`), nunca error fatal.
fn leer_addr_ready(ruta: &std::path::Path) -> Option<String> {
    let contenido = std::fs::read_to_string(ruta).ok()?;
    for linea in contenido.lines() {
        if let Some(v) = linea.trim().strip_prefix("addr=") {
            let v = v.trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Espera async acotada de la `addr` en el fichero ready: poll con
/// cadencia `DAEMON_POLL_INTERVAL`; al vencer el deadline falla con
/// diagnóstico del último contenido (timeout = bug, no flake).
async fn esperar_addr_fichero_ready(
    ruta: &std::path::Path,
    deadline: std::time::Duration,
) -> anyhow::Result<String> {
    let inicio = std::time::Instant::now();
    let mut ultimo = String::new();
    while inicio.elapsed() < deadline {
        if let Some(addr) = leer_addr_ready(ruta) {
            return Ok(addr);
        }
        ultimo = std::fs::read_to_string(ruta).unwrap_or_default();
        tokio::time::sleep(DAEMON_POLL_INTERVAL).await;
    }
    anyhow::bail!(
        "el fichero ready {} no publicó addr válida tras {:?} (último contenido: {:?})",
        ruta.display(),
        deadline,
        ultimo
    )
}

fn remove_daemon_pid_file() -> std::io::Result<()> {
    let p = daemon_pid_path();
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// Espera acotada a que el daemon sea alcanzable (`/health` responde) tras el
/// spawn+bind. Con fichero ready (`Some`) espera el evento (la `addr`
/// publicada) y luego verifica por probe una sola vez — el timeout es bug a
/// diagnosticar, no flake a reintentar. Sin fichero (`None`: `serve` manual sin
/// flag o el unitario hermético) conserva el sondeo clásico como reversión
/// declarada. Con el warmup en segundo plano, «alcanzable = listo».
async fn await_daemon_ready(
    client: &reqwest::Client,
    addr: &str,
    ready: Option<&std::path::Path>,
    deadline: std::time::Duration,
    interval: std::time::Duration,
) -> anyhow::Result<()> {
    if let Some(ruta) = ready {
        // Vía evento: la `addr` publicada manda (debe coincidir con `addr`;
        // si difiere se diagnostica pero se verifica la publicada).
        let publicada = esperar_addr_fichero_ready(ruta, deadline).await?;
        let objetivo = if publicada == addr { addr } else { &publicada };
        if probe_health(client, objetivo).await {
            return Ok(());
        }
        anyhow::bail!(
            "el daemon publicó {} pero /health no responde tras el evento",
            publicada
        );
    }
    let start = std::time::Instant::now();
    while start.elapsed() < deadline {
        if probe_health(client, addr).await {
            return Ok(());
        }
        tokio::time::sleep(interval).await;
    }
    anyhow::bail!("El daemon no respondió a /health tras {:?}", deadline)
}

/// Construye el cuerpo JSON de `daemon status`. Función pura (testeable sin daemon):
/// `stopped` cuando no es alcanzable (fixture intacta, sin campos extra; el
/// `stopped` por probe incluye en `clasificar_residual` la búsqueda del residente
/// por su PID registrado antes de declarar vía libre, sin cambiar este contrato); si es
/// alcanzable, `running` con `engine` y `warm` (más `warm_error` cuando el warmup
/// falló) leídos de `/health`. El `schema_version` lo añade `emit_raw_json`.
fn status_body(
    reachable: bool,
    engine: Option<&str>,
    warm: Option<(&str, Option<String>)>,
) -> Value {
    if !reachable {
        return json!({ "daemon": "stopped" });
    }
    let mut body = json!({ "daemon": "running" });
    if let Some(eng) = engine {
        body["engine"] = Value::String(eng.to_string());
    }
    if let Some((label, error)) = warm {
        body["warm"] = Value::String(label.to_string());
        if let Some(err) = error {
            body["warm_error"] = Value::String(err);
        }
    }
    body
}

async fn wait_health_down(
    client: &reqwest::Client,
    timeout: std::time::Duration,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !daemon_activo(client).await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    anyhow::bail!("El daemon no se apagó tras {:?}", timeout)
}

// ─── Cliente HTTP async del daemon ────────────────────────────────────────

/// Cliente `reqwest` hacia el daemon (HTTP, sin TLS: basta para
/// localhost). La dirección destino la resuelve cada llamada con
/// `resolver_addr_cliente()`. Timeout de conexión breve para que el probe
/// Auto→local sea rápido cuando el daemon no está en ejecución.
fn daemon_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(500))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .expect("construir el cliente HTTP del daemon")
}

/// Probe de vida (GET /health) con deadline corto contra `addr`; `false` en
/// cualquier fallo (connection-refused incluido).
async fn probe_health(client: &reqwest::Client, addr: &str) -> bool {
    match tokio::time::timeout(
        std::time::Duration::from_millis(500),
        client.get(format!("http://{}/health", addr)).send(),
    )
    .await
    {
        Ok(Ok(resp)) => resp.status().is_success(),
        _ => false,
    }
}

/// Probe de vida sobre la dirección descubierta (pidfile o fallback a
/// `DAEMON_ADDR`); `false` habilita el fallback Auto→local.
async fn daemon_activo(client: &reqwest::Client) -> bool {
    let addr_cli = resolver_addr_cliente();
    probe_health(client, &addr_cli).await
}

/// Decide si una acción delegable se despacha al daemon:
/// ForceDaemon → siempre (el POST fallará con DaemonUnreachable si no corre);
/// Auto → solo si el daemon responde; ForceDirect → nunca.
async fn route_to_daemon(mode: DaemonMode, client: &reqwest::Client) -> bool {
    match mode {
        DaemonMode::ForceDaemon => true,
        DaemonMode::ForceDirect => false,
        DaemonMode::Auto => daemon_activo(client).await,
    }
}

/// Acciones local-only rechazan ForceDaemon con DaemonUnreachable.
fn require_local(daemon_mode: DaemonMode) -> Result<(), CliError> {
    if daemon_mode == DaemonMode::ForceDaemon {
        Err(CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("Daemon inalcanzable en {}", resolver_addr_cliente()),
        ))
    } else {
        Ok(())
    }
}

/// POST /transcribe al daemon: codifica PCM i16 LE 16 kHz mono a base64 y devuelve
/// el texto transcrito, emitiendo el mismo envelope local ({text, source}).
async fn transcribe_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    source_language: &str,
) -> Result<(), CliError> {
    // El POST apunta a la dirección descubierta (fallback idéntico sin
    // pidfile).
    let addr_cli = resolver_addr_cliente();
    let pcm: Vec<i16> = if mic {
        capture_mic_pcm(duration).await?
    } else {
        avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(|e| {
            CliError::new(
                ExitCode::TranscriptionFailed,
                "transcription_error",
                e.to_string(),
            )
        })?
    };
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let resp = client
        .post(format!("http://{}/transcribe", addr_cli))
        .json(&serde_json::json!({ "audio_b64": audio_b64, "source_language": source_language }))
        .send()
        .await
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("El daemon devolvió {}", resp.status()),
        ));
    }
    let val: Value = resp.json().await.map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("Respuesta del daemon no es JSON: {}", e),
        )
    })?;
    let text = val["text"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            "El daemon no devolvió 'text'.",
        )
    })?;
    if json_mode {
        emit_raw_json(json!({ "text": text, "source": source_language }));
    } else {
        println!("{}", text);
    }
    Ok(())
}

async fn translate_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    text: &str,
    from: &str,
    to: &str,
) -> Result<(), CliError> {
    let payload = serde_json::json!({ "text": text, "from": from, "to": to });
    // El POST apunta a la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    let fut = client
        .post(format!("http://{}/translate", addr_cli))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout 1500ms)", addr_cli),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(json!({}));
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon_error");
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("error del daemon");
        let code = match reason {
            "empty_text" | "unsupported_language_pair" => ExitCode::InvalidInput,
            "model_missing" => ExitCode::ModelMissing,
            "translation_failed" => ExitCode::TranslationFailed,
            _ => ExitCode::Error,
        };
        return Err(CliError::new(
            code,
            reason,
            format!("{} (HTTP {})", msg, status),
        ));
    }
    let val: Value = resp.json().await.map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("Respuesta del daemon no es JSON: {}", e),
        )
    })?;
    if let Some(err) = val.get("error").and_then(|v| v.as_str()) {
        let reason = val.get("reason").and_then(|v| v.as_str()).unwrap_or(err);
        let msg = val.get("message").and_then(|v| v.as_str()).unwrap_or(err);
        let code = match reason {
            "empty_text" | "unsupported_language_pair" => ExitCode::InvalidInput,
            "model_missing" => ExitCode::ModelMissing,
            "translation_failed" => ExitCode::TranslationFailed,
            _ => ExitCode::Error,
        };
        return Err(CliError::new(code, reason, msg.to_string()));
    }
    let translated = val["translated"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::TranslationFailed,
            "translation_failed",
            "El daemon no devolvió 'translated'.",
        )
    })?;
    let source = val["source"].as_str().unwrap_or(from);
    let target = val["target"].as_str().unwrap_or(to);
    if json_mode {
        emit_raw_json(json!({ "translated": translated, "source": source, "target": target }));
    } else {
        println!("{}", translated);
    }
    Ok(())
}

/// POST /synthesize al daemon, consume el stream NDJSON y decodifica `audio_b64`
/// del evento `result`, devolviendo los bytes WAV del motor (24 kHz s16le mono).
async fn daemon_synthesize_wav(
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<Vec<u8>, CliError> {
    let mut payload = serde_json::json!({ "text": text, "voice": voice, "source_language": source_language, "target_language": target_language });
    if let Some(t) = temperature {
        payload["temperature"] = serde_json::json!(t);
    }
    // El POST apunta a la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    let resp = client
        .post(format!("http://{}/synthesize", addr_cli))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("El daemon devolvió {}", resp.status()),
        ));
    }
    let bytes = resp.bytes().await.map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("Error leyendo la respuesta del daemon: {}", e),
        )
    })?;
    let body = String::from_utf8_lossy(&bytes);
    let mut wav: Option<Vec<u8>> = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let ev: Value = serde_json::from_str(line).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("NDJSON inválido del daemon: {}", e),
            )
        })?;
        match ev["event"].as_str() {
            Some("result") => {
                let b64 = ev["audio_b64"].as_str().ok_or_else(|| {
                    CliError::new(
                        ExitCode::Error,
                        "synthesis_error",
                        "El daemon devolvió result sin audio_b64.",
                    )
                })?;
                wav = Some(
                    base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::Error,
                                "synthesis_error",
                                format!("audio_b64 del daemon no decodable: {}", e),
                            )
                        })?,
                );
            }
            Some("error") => {
                let reason = ev["reason"].as_str().unwrap_or("daemon_error").to_string();
                let msg = ev["message"].as_str().unwrap_or("").to_string();
                return Err(CliError::new(ExitCode::Error, reason, msg));
            }
            _ => {}
        }
    }
    wav.ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "synthesis_error",
            "El daemon no devolvió audio_b64.".to_string(),
        )
    })
}

/// Timeout de inactividad entre eventos de un stream NDJSON: 1500 ms.
/// Presupuesto histórico reinterpretado — ya no acota la inferencia total, solo
/// dispara si el daemon deja de emitir (bucle atascado), nunca por inferencia sana
/// (el servidor emite latidos cada ~500 ms).
const STREAM_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// Deadline total failsafe del consumo de un stream (paridad con `daemon_client`):
/// red de seguridad documentada, no presupuesto.
const STREAM_TOTAL_DEADLINE: std::time::Duration = std::time::Duration::from_secs(120);

/// Consume un stream NDJSON del daemon (`started` → latidos → `result`/`error`)
/// con timeout de inactividad + failsafe total. Retorna el evento final
/// `result`; el evento `error` se mapea a `CliError` con `codigo_de(reason)`.
/// Sin `result` (stream truncado, NDJSON inválido, inactividad o failsafe) el
/// fallo es ruidoso: nunca se reemite un éxito parcial.
async fn consumir_stream_ndjson(
    mut resp: reqwest::Response,
    etapa: &str,
    codigo_de: impl Fn(&str) -> ExitCode,
) -> Result<Value, CliError> {
    let inicio = std::time::Instant::now();
    let mut resto = String::new();
    // Los diagnósticos nombran la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    loop {
        if inicio.elapsed() >= STREAM_TOTAL_DEADLINE {
            return Err(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!(
                    "Daemon inalcanzable en {} (límite total 120s agotado en {})",
                    addr_cli, etapa
                ),
            ));
        }
        let espera = std::cmp::min(
            STREAM_INACTIVITY_TIMEOUT,
            STREAM_TOTAL_DEADLINE - inicio.elapsed(),
        );
        let chunk = tokio::time::timeout(espera, resp.chunk())
            .await
            .map_err(|_| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    format!(
                        "Daemon inalcanzable en {} (sin eventos del daemon en 1500ms en {})",
                        addr_cli, etapa
                    ),
                )
            })?
            .map_err(|e| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    format!("Daemon inalcanzable en {}: {}", addr_cli, e),
                )
            })?;
        let bytes = match chunk {
            Some(b) => b,
            None => {
                // Fin de stream: procesar el resto buffered (línea sin `\n`
                // final) y exigir el evento final.
                let linea = std::mem::take(&mut resto);
                if !linea.trim().is_empty() {
                    if let Some(v) = procesar_linea_stream(&linea, &codigo_de)? {
                        return Ok(v);
                    }
                }
                return Err(CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!(
                        "El stream del daemon terminó sin evento final en {}.",
                        etapa
                    ),
                ));
            }
        };
        resto.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = resto.find('\n') {
            let linea: String = resto.drain(..=pos).collect();
            if let Some(v) = procesar_linea_stream(linea.trim(), &codigo_de)? {
                return Ok(v);
            }
        }
    }
}

/// Procesa una línea del stream: `result` → `Some(evento)`, `error` → `Err`
/// mapeado, resto (`started`/latidos/desconocidos) → `Some` nada (`Ok(None)`).
fn procesar_linea_stream(
    linea: &str,
    codigo_de: &impl Fn(&str) -> ExitCode,
) -> Result<Option<Value>, CliError> {
    let linea = linea.trim();
    if linea.is_empty() {
        return Ok(None);
    }
    let ev: Value = serde_json::from_str(linea).map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("NDJSON inválido del daemon: {}", e),
        )
    })?;
    match ev.get("event").and_then(|v| v.as_str()) {
        Some("result") => Ok(Some(ev)),
        Some("error") => {
            let reason = ev
                .get("reason")
                .and_then(|v| v.as_str())
                .unwrap_or("daemon_error");
            let msg = ev
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("error del daemon");
            Err(CliError::new(codigo_de(reason), reason, msg.to_string()))
        }
        _ => Ok(None),
    }
}

/// `synthesize` vía daemon: persiste el WAV en `SpeechStore` y respeta
/// --label/--output/--play, devolviendo la ruta del WAV persistido (paralelo al
/// handler local para que el envelope JSON de salida coincida).
// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech synthesize`.
#[allow(clippy::too_many_arguments)]
async fn synthesize_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    label: &str,
    force: bool,
    play: bool,
    output: &Option<String>,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<(), CliError> {
    let speech_store = SpeechStore::new();
    let label_l = label.to_lowercase();
    es_identificador_valido(Some(&label_l), None)?;
    if !force && speech_store.find(voice, &label_l).is_some() {
        return Err(CliError::new(
            ExitCode::StateConflict,
            "label_exists",
            format!(
                "Ya existe una locución con la etiqueta '{}' (usa --force).",
                label_l
            ),
        ));
    }
    let wav = daemon_synthesize_wav(
        client,
        text,
        voice,
        source_language,
        target_language,
        temperature,
    )
    .await?;
    let tmp = std::env::temp_dir().join(format!("avi_tts_{}.wav", label_l));
    std::fs::write(&tmp, &wav)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;

    let saved = if play {
        // RF-12.3–12.5: bucle interactivo client-side (P6); la opción 3
        // re-despacha la síntesis por la misma vía daemon.
        let resultado = synthesize_play_loop(
            voice,
            &label_l,
            force,
            &speech_store,
            text,
            tmp.clone(),
            || async {
                let wav = daemon_synthesize_wav(
                    client,
                    text,
                    voice,
                    source_language,
                    target_language,
                    temperature,
                )
                .await?;
                std::fs::write(&tmp, &wav)
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
                Ok(tmp.clone())
            },
        )
        .await?;
        match resultado {
            Some(saved) => saved,
            None => {
                // Opción 4 / EOF: descartado, exit 0 (RF-12.3).
                let _ = std::fs::remove_file(&tmp);
                if json_mode {
                    emit_raw_json(json!({ "status": "discarded", "voice": voice }));
                } else {
                    println!("Descartado.");
                }
                return Ok(());
            }
        }
    } else {
        speech_store
            .save(voice, &label_l, text, &tmp)
            .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?
    };
    let _ = std::fs::remove_file(&tmp);
    if let Some(out) = output {
        std::fs::copy(&saved, out)
            .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
    }
    if json_mode {
        emit_raw_json(json!({
            "status": "success",
            "audio_path": saved.to_string_lossy(),
            "voice": voice,
        }));
    } else {
        println!("Síntesis completada: {}", saved.display());
    }
    Ok(())
}

/// `say` vía daemon: reproduce el WAV decodificado y expone una copia efímera.
async fn say_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<(), CliError> {
    let wav = daemon_synthesize_wav(
        client,
        text,
        voice,
        source_language,
        target_language,
        temperature,
    )
    .await?;
    let tmp = std::env::temp_dir().join(format!("avi_say_{}.wav", std::process::id()));
    std::fs::write(&tmp, &wav)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    audio::AudioService::new().play_wav(&tmp).map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "playback_failed",
            format!("Fallo al reproducir la locución: {}", e),
        )
    })?;
    if json_mode {
        emit_raw_json(json!({
            "status": "reproduced",
            "audio_path": tmp.to_string_lossy(),
            "voice": voice,
        }));
    } else {
        println!("Reproduciendo: {}", tmp.display());
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(())
}

async fn clone_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    name: &str,
    speech_reference: &str,
    timbre_reference: Option<&str>,
    force: bool,
) -> Result<(), CliError> {
    let speech_bytes = std::fs::read(speech_reference).map_err(|e| {
        CliError::new(
            ExitCode::NotFound,
            "audio_not_found",
            format!(
                "El audio de referencia '{}' no existe: {}",
                speech_reference, e
            ),
        )
    })?;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&speech_bytes);
    let timbre_b64 = if let Some(t) = timbre_reference {
        let b = std::fs::read(t).map_err(|e| {
            CliError::new(
                ExitCode::NotFound,
                "audio_not_found",
                format!("El audio de timbre '{}' no existe: {}", t, e),
            )
        })?;
        Some(base64::engine::general_purpose::STANDARD.encode(&b))
    } else {
        None
    };
    let mut payload = serde_json::json!({
        "name": name,
        "audio_b64": audio_b64,
        "force": force,
    });
    if let Some(tb) = timbre_b64 {
        payload["timbre_b64"] = Value::String(tb);
    }
    // El envío solo espera las cabeceras (el daemon valida barato y
    // responde 200 de inmediato); el trabajo pesado se consume como stream con
    // inactividad 1500 ms + failsafe 120 s hasta el evento final.
    // El POST apunta a la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    let fut = client
        .post(format!("http://{}/voices/clone", addr_cli))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout 1500ms)", addr_cli),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body: Value = resp.json().await.unwrap_or(json!({}));
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .or_else(|| body.get("error").and_then(|v| v.as_str()))
            .unwrap_or("daemon_error");
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("error del daemon");
        let code = match reason {
            "invalid_voice_name" => ExitCode::InvalidInput,
            "voice_exists" => ExitCode::StateConflict,
            "model_missing" => ExitCode::ModelMissing,
            "audio_missing" | "audio_decode_error" => ExitCode::InvalidInput,
            _ => ExitCode::Error,
        };
        return Err(CliError::new(
            code,
            reason,
            format!("{} (HTTP {})", msg, status),
        ));
    }
    let val: Value = consumir_stream_ndjson(resp, "clone", |reason| match reason {
        "invalid_voice_name" => ExitCode::InvalidInput,
        "voice_exists" => ExitCode::StateConflict,
        "model_missing" => ExitCode::ModelMissing,
        "audio_missing" | "audio_decode_error" => ExitCode::InvalidInput,
        _ => ExitCode::Error,
    })
    .await?;
    if json_mode {
        emit_raw_json(json!({
            "name": val["name"].as_str().unwrap_or(name),
            "speech": val["speech"].as_str().unwrap_or(""),
            "timbre": val.get("timbre").cloned().unwrap_or(Value::Null),
            "precomputed": val.get("precomputed").and_then(|v| v.as_bool()).unwrap_or(false),
        }));
    } else {
        println!("Voz '{}' clonada.", name);
    }
    Ok(())
}

// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech dub`.
#[allow(clippy::too_many_arguments)]
async fn dub_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
    voice: &str,
) -> Result<(), CliError> {
    // Captura/lectura PCM y encode a base64 para POST /dub
    let pcm: Vec<i16> = if mic {
        capture_mic_pcm(duration).await?
    } else {
        avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(|e| {
            CliError::new(
                ExitCode::TranscriptionFailed,
                "transcription_error",
                e.to_string(),
            )
        })?
    };
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut payload = serde_json::json!({
        "audio_b64": audio_b64,
        "from": source_language,
        "to": target_language,
        "source_language": source_language,
        "target_language": target_language,
        "voice": voice,
    });
    if let Some(t) = temperature {
        payload["temperature"] = serde_json::json!(t);
    }
    // El envío espera las cabeceras (respuesta 200 inmediata tras
    // validar barato); el pipeline se consume como stream con inactividad
    // 1500 ms + failsafe 120 s hasta el evento final.
    // El POST apunta a la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    let fut = client
        .post(format!("http://{}/dub", addr_cli))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout dub 1500ms)", addr_cli),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        let status = resp.status();
        // Si 404, el daemon es viejo sin /dub → degradar a composición
        if status == reqwest::StatusCode::NOT_FOUND {
            return dub_compose_via_daemon(
                json_mode,
                client,
                Some(pcm),
                source_language,
                target_language,
                temperature,
                voice,
            )
            .await;
        }
        let body: Value = resp.json().await.unwrap_or(json!({}));
        let reason = body
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("daemon_error");
        let msg = body
            .get("message")
            .and_then(|v| v.as_str())
            .unwrap_or("error del daemon");
        let code = match reason {
            "audio_missing" | "audio_decode_error" | "empty_text" | "unsupported_language_pair" => {
                ExitCode::InvalidInput
            }
            "model_missing" => ExitCode::ModelMissing,
            "transcription_failed" => ExitCode::TranscriptionFailed,
            "translation_failed" => ExitCode::TranslationFailed,
            "voice_not_found" => ExitCode::NotFound,
            "synthesis_failed" | "synthesis_timeout" => ExitCode::Error,
            "stt_unsupported" | "translation_unsupported" => ExitCode::Error,
            _ => ExitCode::Error,
        };
        return Err(CliError::new(
            code,
            reason,
            format!("{} (HTTP {})", msg, status),
        ));
    }
    // El evento final del stream trae la forma contractual
    // {status:"dubbed", text, translated, audio_b64, voice}; los mapeos
    // reason→exit se preservan también para los eventos `error` del stream.
    let val: Value = consumir_stream_ndjson(resp, "dub", |reason| match reason {
        "audio_missing" | "audio_decode_error" | "empty_text" | "unsupported_language_pair" => {
            ExitCode::InvalidInput
        }
        "model_missing" => ExitCode::ModelMissing,
        "transcription_failed" => ExitCode::TranscriptionFailed,
        "translation_failed" => ExitCode::TranslationFailed,
        "voice_not_found" => ExitCode::NotFound,
        "synthesis_failed" | "synthesis_timeout" => ExitCode::Error,
        "stt_unsupported" | "translation_unsupported" => ExitCode::Error,
        _ => ExitCode::Error,
    })
    .await?;
    // Respuesta esperada {status:"dubbed", text, translated, audio_b64}
    let audio_b64_resp = val["audio_b64"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            "El daemon no devolvió audio_b64.",
        )
    })?;
    let wav_bytes = base64::engine::general_purpose::STANDARD
        .decode(audio_b64_resp)
        .map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("audio_b64 del daemon no decodable: {}", e),
            )
        })?;
    let final_text = val["translated"]
        .as_str()
        .or_else(|| val["text"].as_str())
        .unwrap_or("")
        .to_string();
    let tmp_wav = std::env::temp_dir().join(format!("avi_dub_{}.wav", std::process::id()));
    std::fs::write(&tmp_wav, &wav_bytes)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    audio::AudioService::new().play_wav(&tmp_wav).map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "playback_failed",
            format!("Fallo al reproducir el doblaje: {}", e),
        )
    })?;
    if json_mode {
        emit_raw_json(json!({
            "status": "dubbed",
            "text": final_text,
            "audio_path": tmp_wav.to_string_lossy(),
        }));
    } else {
        println!("Doblaje reproducido: {}", tmp_wav.display());
    }
    Ok(())
}

async fn dub_compose_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    pcm_opt: Option<Vec<i16>>,
    from: &str,
    to: &str,
    temperature: Option<f32>,
    voice: &str,
) -> Result<(), CliError> {
    // Transcribe vía daemon (reusa PCM ya capturado)
    let pcm = pcm_opt.expect("pcm ya capturado");
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    // El POST apunta a la dirección descubierta.
    let addr_cli = resolver_addr_cliente();
    let fut = client
        .post(format!("http://{}/transcribe", addr_cli))
        .json(&serde_json::json!({ "audio_b64": audio_b64, "source_language": from }))
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout 1500ms)", addr_cli),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", addr_cli, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("El daemon devolvió {}", resp.status()),
        ));
    }
    let val: Value = resp.json().await.map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("Respuesta del daemon no es JSON: {}", e),
        )
    })?;
    let transcribed = val["text"]
        .as_str()
        .ok_or_else(|| {
            CliError::new(
                ExitCode::TranscriptionFailed,
                "transcription_failed",
                "El daemon no devolvió 'text'.",
            )
        })?
        .to_string();
    if transcribed.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_text",
            "El texto transcrito está vacío",
        ));
    }
    let source = resolve_stt_language(from);
    let target = resolve_stt_language(to);
    let final_text = if source == target {
        transcribed.clone()
    } else {
        let pair = match (source, target) {
            ("es", "en") => "es-en",
            ("en", "es") => "en-es",
            _ => {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "unsupported_language_pair",
                    format!(
                        "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                        source, target
                    ),
                ));
            }
        };
        let ct2_dir = store::ct2_model_dir(pair);
        if !store::is_ct2_provisioned(pair) {
            return Err(CliError::new(
                ExitCode::ModelMissing,
                "model_missing",
                format!("El modelo de traducción no está provisionado en '{}' (faltan: {}) — ejecuta setup.", ct2_dir.display(), store::ct2_archivos_faltantes(pair).join(", ")),
            ));
        }
        #[cfg(not(feature = "native-translation"))]
        {
            let _ = ct2_dir.as_os_str();
            return Err(CliError::new(
                ExitCode::Error,
                "translation_unsupported",
                "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
            ));
        }
        #[cfg(feature = "native-translation")]
        {
            translation::translate(&transcribed, source, target, &ct2_dir).map_err(|e| {
                CliError::new(
                    ExitCode::TranslationFailed,
                    "translation_failed",
                    e.to_string(),
                )
            })?
        }
    };
    let voice_store = VoiceStore::new();
    if !voice_store.exists(voice) {
        return Err(CliError::new(
            ExitCode::NotFound,
            "voice_not_found",
            format!("La voz '{}' no existe.", voice),
        ));
    }
    let wav_bytes =
        daemon_synthesize_wav(client, &final_text, voice, target, target, temperature).await?;
    let tmp_wav = std::env::temp_dir().join(format!("avi_dub_{}.wav", std::process::id()));
    std::fs::write(&tmp_wav, &wav_bytes)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    audio::AudioService::new().play_wav(&tmp_wav).map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "playback_failed",
            format!("Fallo al reproducir el doblaje: {}", e),
        )
    })?;
    if json_mode {
        emit_raw_json(json!({
            "status": "dubbed",
            "text": final_text,
            "audio_path": tmp_wav.to_string_lossy(),
        }));
    } else {
        println!("Doblaje reproducido: {}", tmp_wav.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `await_daemon_ready` debe agotar el deadline por reloj de pared (no un
    /// recuento fijo de iteraciones) contra un puerto cerrado: retorna `Err` y el
    /// tiempo transcurrido queda acotado por el deadline. Hermético: no arranca
    /// daemon ni paga warmup, y usa un puerto efímero cerrado (no el 8765 compartido).
    #[tokio::test]
    async fn await_daemon_ready_respeta_deadline() {
        // Puerto efímero: enlazamos, capturamos la dirección y dropeamos el
        // listener para garantizar que el puerto queda cerrado (connection-refused).
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind efímero");
        let addr = listener.local_addr().expect("local_addr").to_string();
        drop(listener);

        let deadline = std::time::Duration::from_millis(800);
        let interval = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();
        // Sin fichero: ejercita el sondeo clásico (reversión declarada).
        let res = await_daemon_ready(&daemon_client(), &addr, None, deadline, interval).await;
        let elapsed = start.elapsed();

        assert!(res.is_err(), "esperado Err contra puerto cerrado");
        assert!(
            elapsed >= deadline,
            "debe respetar el deadline: elapsed={:?} < deadline={:?}",
            elapsed,
            deadline
        );
        assert!(
            elapsed < std::time::Duration::from_millis(1500),
            "no debe exceder holgadamente el deadline: elapsed={:?}",
            elapsed
        );
    }

    /// `status_body` mapea los tres casos del contrato de `daemon status`:
    /// `stopped` (fixture intacta, sin campos extra), `running` + `warm`, y
    /// `running` + `warm_error` cuando el warmup falló.
    #[test]
    fn status_body_mapea_stopped_running_y_warm() {
        // stopped: solo `daemon` (schema_version lo añade emit_raw_json).
        let stopped = status_body(false, None, None);
        assert_eq!(stopped, json!({ "daemon": "stopped" }));

        // running + warm, sin warm_error.
        let running = status_body(true, Some("rust_native"), Some(("warm", None)));
        assert_eq!(running["daemon"], "running");
        assert_eq!(running["engine"], "rust_native");
        assert_eq!(running["warm"], "warm");
        assert!(running.get("warm_error").is_none());

        // running + warm_failed con causa.
        let failed = status_body(
            true,
            Some("rust_native"),
            Some(("warm_failed", Some("boom".to_string()))),
        );
        assert_eq!(failed["warm"], "warm_failed");
        assert_eq!(failed["warm_error"], "boom");
    }

    /// El predicado puro del reclamo Unix ante líder muerto solo verifica
    /// con probe caído + PID muerto + residente muerto (no-plataformero, hermético).
    #[test]
    fn reclamo_unix_verificado_exige_triple_cierre() {
        assert!(reclamo_unix_verificado(false, false, false));
        assert!(!reclamo_unix_verificado(true, false, false));
        assert!(!reclamo_unix_verificado(false, true, false));
        assert!(!reclamo_unix_verificado(false, false, true));
    }

    /// `Parado` con residente vivo (resident_pid vivo) es degradado para
    /// reclamo, no vía libre (no-plataformero, hermético).
    #[test]
    fn parado_con_residente_vivo_es_degradado() {
        assert!(parado_con_residente_es_degradado(false, false, true));
        assert!(!parado_con_residente_es_degradado(false, false, false));
        assert!(!parado_con_residente_es_degradado(true, false, true));
        assert!(!parado_con_residente_es_degradado(false, true, true));
    }

    /// `procesar_linea_stream` clasifica cada línea NDJSON sin reloj
    /// (determinista): `result` se entrega, `error` se mapea por reason,
    /// `started`/latidos se ignoran y el NDJSON inválido falla ruidoso.
    #[test]
    fn procesar_linea_stream_clasifica_eventos() {
        let codigo = |reason: &str| match reason {
            "voice_exists" => ExitCode::StateConflict,
            _ => ExitCode::Error,
        };
        let r = procesar_linea_stream(
            r#"{"event":"result","name":"v","precomputed":true}"#,
            &codigo,
        )
        .expect("result no falla");
        assert_eq!(r.expect("result se entrega")["name"], "v");
        for linea in [
            r#"{"event":"started","name":"v"}"#,
            r#"{"event":"heartbeat","stage":"clone"}"#,
            r#"{"event":"progress","stage":"warmup"}"#,
            "",
            "   ",
        ] {
            assert!(
                procesar_linea_stream(linea, &codigo)
                    .expect("no-final no falla")
                    .is_none(),
                "línea no-final se ignora: {:?}",
                linea
            );
        }
        let e = procesar_linea_stream(
            r#"{"event":"error","reason":"voice_exists","message":"existe"}"#,
            &codigo,
        )
        .expect_err("error debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "voice_exists");
        assert!(
            procesar_linea_stream("{no json", &codigo).is_err(),
            "NDJSON inválido falla ruidoso"
        );
    }

    /// Sirve una secuencia NDJSON programada sobre HTTP plano en loopback para
    /// ejercitar `consumir_stream_ndjson` sin daemon (doble determinista).
    /// `retener_secs`: si es `Some`, tras escribir `lineas` la conexión se
    /// retiene abierta ese tiempo (caso de atasco); si es `None`, se cierra
    /// (éxito, fallo o truncado según `lineas`). Sin `content-length`: cuerpo
    /// hasta cierre de conexión.
    async fn servir_secuencia_programada(lineas: Vec<String>, retener_secs: Option<u64>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind efímero");
        let addr = listener.local_addr().expect("addr").to_string();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut buf = vec![0u8; 8192];
            let mut leido = 0;
            loop {
                let n = sock.read(&mut buf[leido..]).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                leido += n;
                if buf[..leido].windows(4).any(|w| w == b"\r\n\r\n") || leido >= buf.len() {
                    break;
                }
            }
            let cuerpo: String = lineas.iter().map(|l| format!("{}\n", l)).collect();
            let cabecera = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\nconnection: close\r\n\r\n";
            if sock.write_all(cabecera.as_bytes()).await.is_err() {
                return;
            }
            if sock.write_all(cuerpo.as_bytes()).await.is_err() {
                return;
            }
            if let Some(s) = retener_secs {
                tokio::time::sleep(std::time::Duration::from_secs(s)).await;
            }
        });
        addr
    }

    /// El consumo entrega el evento final tras la secuencia
    /// `started` → latidos → `result` (doble con secuencia de éxito).
    #[tokio::test]
    async fn consumir_stream_ndjson_devuelve_evento_final() {
        let addr = servir_secuencia_programada(
            vec![
                r#"{"event":"started","name":"v"}"#.to_string(),
                r#"{"event":"heartbeat","stage":"clone"}"#.to_string(),
                r#"{"event":"result","name":"v","speech":"s","precomputed":true}"#.to_string(),
            ],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let val = consumir_stream_ndjson(resp, "clone", |_| ExitCode::Error)
            .await
            .expect("la secuencia de éxito entrega el final");
        assert_eq!(val["event"], "result");
        assert_eq!(val["name"], "v");
        assert_eq!(val["precomputed"], true);
    }

    /// El evento de fallo del stream se mapea por reason (doble con
    /// secuencia de fallo), sin esperar al failsafe total.
    #[tokio::test]
    async fn consumir_stream_ndjson_mapea_evento_de_fallo() {
        let addr = servir_secuencia_programada(
            vec![
                r#"{"event":"started","name":"v"}"#.to_string(),
                r#"{"event":"error","reason":"voice_exists","message":"existe"}"#.to_string(),
            ],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let inicio = std::time::Instant::now();
        let e = consumir_stream_ndjson(resp, "clone", |reason| match reason {
            "voice_exists" => ExitCode::StateConflict,
            _ => ExitCode::Error,
        })
        .await
        .expect_err("el evento de fallo debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "voice_exists");
        assert!(
            inicio.elapsed() < std::time::Duration::from_secs(10),
            "el fallo explícito no espera techos: {:?}",
            inicio.elapsed()
        );
    }

    /// El stream atascado (cabeceras sin eventos) dispara el timeout de
    /// inactividad de 1500 ms (doble con secuencia de atasco), no el failsafe.
    #[tokio::test]
    async fn consumir_stream_ndjson_detecta_atasco_por_inactividad() {
        let addr = servir_secuencia_programada(vec![], Some(30)).await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder cabeceras");
        let inicio = std::time::Instant::now();
        let e = consumir_stream_ndjson(resp, "clone", |_| ExitCode::Error)
            .await
            .expect_err("el atasco debe fallar");
        assert_eq!(e.code, ExitCode::DaemonUnreachable);
        assert_eq!(e.reason, "daemon_unreachable");
        let transcurrido = inicio.elapsed();
        assert!(
            transcurrido >= std::time::Duration::from_millis(1500),
            "debe agotar la inactividad: {:?}",
            transcurrido
        );
        assert!(
            transcurrido < std::time::Duration::from_secs(30),
            "no debe llegar al failsafe ni al retén: {:?}",
            transcurrido
        );
    }

    /// El stream truncado tras `started` (cierre sin evento final) falla
    /// ruidoso como `daemon_error`, nunca como éxito parcial.
    #[tokio::test]
    async fn consumir_stream_ndjson_falla_si_trunca_sin_final() {
        let addr = servir_secuencia_programada(
            vec![r#"{"event":"started","name":"v"}"#.to_string()],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let e = consumir_stream_ndjson(resp, "clone", |_| ExitCode::Error)
            .await
            .expect_err("el truncado debe fallar");
        assert_eq!(e.code, ExitCode::Error);
        assert_eq!(e.reason, "daemon_error");
    }
}
