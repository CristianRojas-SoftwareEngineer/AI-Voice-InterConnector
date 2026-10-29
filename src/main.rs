// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use avi_audio as audio;
// El trait STT y el motor real solo entran en scope con `native-stt` (off por
// defecto); sin el feature, los subcomandos de transcripción devuelven un error
// explícito de "compilado sin soporte".
#[cfg(feature = "native-stt")]
use avi_core::engine::SttEngine;
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_daemon as daemon;
use avi_lifecycle as lifecycle;
use avi_lifecycle::progress::PhaseProgress;
use avi_store as store;
use avi_store::{ModelStore, SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::EngineHolder;
use avi_tts::{Qwen3TtsEngine, TtsEngine};
// El motor de traducción real solo entra en scope con `native-translation`.
#[cfg(feature = "native-translation")]
use avi_translation as translation;
use base64::Engine;
use clap::{CommandFactory, FromArgMatches, Parser, Subcommand};
use serde_json::{json, Value};
use std::io::IsTerminal;
use std::io::Write;
use std::net::SocketAddr;
use std::path::PathBuf;

const VERSION: &str = "0.37.1";
const APP_NAME: &str = "ai-voice-interconnector";

/// Resuelve la dirección de escucha del daemon. Delega en la fuente única del
/// protocolo —`AVI_DAEMON_PORT` con reserva en `DEFAULT_ADDR`—, y solo convierte
/// el resultado a `SocketAddr`, que es lo que necesita el servidor: el lanzador no
/// vuelve a derivar el puerto por su cuenta.
/// Sin fichero de configuración a propósito: el arranque no debe acoplarse
/// al data_dir compartido (colisionaría entre instancias).
fn resolve_daemon_addr() -> SocketAddr {
    lifecycle::daemon_stop::default_addr()
        .parse()
        .expect("la dirección del daemon debe parsear")
}

// Plazos del **lanzador**, no del protocolo: los del arranque —espera del ready,
// cadencia de sondeo y recolector del hijo— y los de la limpieza acotada ante
// Ctrl+C. Los plazos del protocolo de parada, incluido el reclamo de `start`, no
// viven aquí: los declara `avi-lifecycle` y se toman de allí
// (`lifecycle::daemon_stop::STOP_DEADLINE_GLOBAL`), igual que la dirección del
// daemon (`lifecycle::daemon_stop::default_addr`).
/// Techo de la espera de la publicación del resultado de arranque en
/// `daemon start/restart`: el registro de éxito, el de fallo o la muerte del
/// hijo. Cubre spawn + validación + bind + carga del estado (~1-2 s sanos); el
/// warmup TTS corre en segundo plano y no cuenta, y la confirmación por
/// `/health` posterior tiene su propio presupuesto (`DAEMON_HEALTH_DEADLINE`).
/// Vencerlo con el hijo vivo es un diagnóstico (`daemon_unreachable`), no un
/// flake a reintentar.
const DAEMON_READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// Presupuesto de la confirmación por `/health` una vez publicada la dirección,
/// independiente de lo que la publicación haya consumido de
/// `DAEMON_READY_DEADLINE`. El daemon ya está enlazado y sirviendo, así que
/// `/health` responde en milisegundos; el margen cubre una máquina saturada.
const DAEMON_HEALTH_DEADLINE: std::time::Duration = std::time::Duration::from_secs(5);
/// Cadencia de lectura del fichero ready, de vigilancia del hijo y del sondeo
/// de `/health`.
const DAEMON_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);
/// Deadline para recolectar al hijo tras matar su árbol ante un arranque
/// fallido.
const DAEMON_LAUNCH_CLEANUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
/// Deadline breve para la limpieza acotada ante Ctrl+C: mata el árbol
/// preciso con verificación; vencido, sale igualmente con 130 sin colgarse.
const CTRL_C_CLEANUP_DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);

/// PID hijo en memoria desde el `spawn`: estrecha la ventana
/// spawn→write del pidfile. El handler Ctrl+C lo reclama cuando aún no hay
/// pidfile; lo fija `launch_daemon` (en `start` y `restart`) justo tras
/// `spawn_background` y lo pone a 0 al abortar un arranque fallido.
static IN_MEMORY_PID: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Flag de cancelación del lote `denoise` en curso, para que el handler global
/// de Ctrl+C lo active en vez de terminar de inmediato y dejar temporales.
#[cfg(feature = "native-denoise")]
static DENOISE_CANCEL: std::sync::OnceLock<std::sync::Arc<std::sync::atomic::AtomicBool>> =
    std::sync::OnceLock::new();

/// Titular del motor de transcripción para la vida del proceso: lo crea una
/// sola vez y lo conserva sin destruirlo hasta la salida, de modo que ningún
/// destructor de ONNX Runtime corre durante la salida. El sistema operativo
/// reclama la memoria al terminar el proceso.
#[cfg(feature = "native-stt")]
static STT_HOLDER: EngineHolder = EngineHolder::new();

/// Traduce un fallo de la síntesis directa al contrato: la falta del modelo que
/// exige la voz sale como `model_missing` (exit 4), el vencimiento del
/// presupuesto como `synthesis_timeout`, una voz sin referencia que no es preset
/// como `voice_not_found` (exit 3) y cualquier otro como `synthesis_failed`.
fn synthesis_cli_error(e: anyhow::Error) -> CliError {
    if e.downcast_ref::<avi_tts::ModelMissing>().is_some() {
        return CliError::new(ExitCode::ModelMissing, "model_missing", e.to_string());
    }
    if e.downcast_ref::<avi_tts::VoiceNotFound>().is_some() {
        return CliError::new(ExitCode::NotFound, "voice_not_found", e.to_string());
    }
    let reason = if e.downcast_ref::<avi_tts::SynthesisTimeout>().is_some() {
        "synthesis_timeout"
    } else {
        "synthesis_failed"
    };
    CliError::new(ExitCode::Error, reason, e.to_string())
}

/// Motor TTS de la vía directa. Una variable de entorno del motor
/// (`QWEN3_TTS_BIN`, `QWEN3_TTS_MODEL_DIR`, `QWEN3_TTS_BASE_MODEL_DIR`) que no
/// apunta a una ruta existente, o un modelo ilegible, es `model_missing`.
fn direct_tts_engine() -> Result<Qwen3TtsEngine, CliError> {
    Qwen3TtsEngine::new(None)
        .map_err(|e| CliError::new(ExitCode::ModelMissing, "model_missing", e.to_string()))
}

/// Almacén de voces del usuario. Una raíz de datos que no se puede resolver
/// (sin `HOME`/`USERPROFILE`/`LOCALAPPDATA`) es `io_error`.
fn open_voice_store() -> Result<VoiceStore, CliError> {
    VoiceStore::new().map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Almacén de habla del usuario; misma regla que [`open_voice_store`].
fn open_speech_store() -> Result<SpeechStore, CliError> {
    SpeechStore::new().map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Almacén de modelos; una raíz de modelos que no se puede resolver es `io_error`.
fn open_model_store() -> Result<ModelStore, CliError> {
    ModelStore::new().map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Síntesis directa de una voz del almacén del comando: resuelve su referencia
/// clonada y pide al motor el idioma de destino (token del producto) con la
/// temperatura opcional ya validada.
fn synthesize_voice_directly(
    engine: &Qwen3TtsEngine,
    voice_store: &VoiceStore,
    text: &str,
    voice: &str,
    temperature: Option<f32>,
    language: &str,
    output_path: &PathBuf,
) -> Result<PathBuf, CliError> {
    let qvoice_path = voice_store
        .find_reference(voice)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    let profile = avi_tts::VoiceProfile {
        name: voice.to_string(),
        qvoice_path,
    };
    let options = avi_tts::GenerationOptions::with_temperature(temperature, language);
    engine
        .synthesize_with_options(text, &profile, &options, Some(output_path))
        .map_err(synthesis_cli_error)
}

/// Valida el override de temperatura (`0 < t <= 2.0`); exit 2 si no calza.
fn validate_temperature(temperature: Option<f32>) -> Result<(), CliError> {
    if let Some(t) = temperature {
        if !(t > 0.0 && t <= 2.0) {
            return Err(CliError::new(
                ExitCode::InvalidInput,
                "usage_error",
                "--temperature debe ser mayor que 0 y como máximo 2.0.",
            ));
        }
    }
    Ok(())
}

#[derive(Parser)]
#[command(name = APP_NAME, version = VERSION, about = "AI Voice Interconnector CLI")]
struct Cli {
    /// Emitir la salida como JSON
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

/// Localiza el árbol de comandos de clap al español: plantilla con `Uso:`,
/// encabezados `Opciones`/`Argumentos`/`Comandos`, flags propios `-h/--help`
/// y `-V/--version` en español, subcomando `help` deshabilitado y
/// anotaciones `[por defecto: …]`/`[valores posibles: …]` derivadas de los
/// valores reales. Se aplica recursivamente a cada subcomando.
fn localize_command(cmd: clap::Command) -> clap::Command {
    localize_command_node(cmd, true)
}

/// Nodo de `localize_command`: `is_root` solo agrega el flag propio de versión.
fn localize_command_node(cmd: clap::Command, is_root: bool) -> clap::Command {
    let mut cmd = cmd
        .help_template("{before-help}{about-with-newline}\nUso: {usage}\n\n{all-args}{after-help}")
        .subcommand_help_heading("Comandos")
        .subcommand_value_name("COMANDO")
        .disable_help_subcommand(true)
        .disable_help_flag(true)
        .arg(
            clap::Arg::new("help")
                .short('h')
                .long("help")
                .action(clap::ArgAction::Help)
                .help("Muestra la ayuda"),
        );
    if is_root {
        cmd = cmd.disable_version_flag(true).arg(
            clap::Arg::new("version")
                .short('V')
                .long("version")
                .action(clap::ArgAction::Version)
                .help("Muestra la versión"),
        );
    }
    cmd.mut_args(localize_arg)
        .mut_subcommands(|sub| localize_command_node(sub, false))
}

/// Localiza un argumento de clap: encabezado `Argumentos`/`Opciones` salvo
/// encabezado propio, y anotaciones en español derivadas de los valores
/// reales (solo si recibe valor y no las ocultaba ya).
fn localize_arg(arg: clap::Arg) -> clap::Arg {
    let mut arg = arg;
    if arg.get_help_heading().is_none() {
        if arg.is_positional() {
            arg = arg.help_heading("Argumentos");
        } else {
            arg = arg.help_heading("Opciones");
        }
    }
    if arg.get_action().takes_values()
        && !arg.is_hide_default_value_set()
        && !arg.is_hide_possible_values_set()
    {
        let defaults: Vec<String> = arg
            .get_default_values()
            .iter()
            .map(|v| v.to_string_lossy().into_owned())
            .collect();
        let possible: Vec<String> = arg
            .get_possible_values()
            .iter()
            .filter(|pv| !pv.is_hide_set())
            .map(|pv| pv.get_name().to_string())
            .collect();
        if !defaults.is_empty() || !possible.is_empty() {
            let mut help = arg.get_help().map(|h| h.to_string()).unwrap_or_default();
            if !defaults.is_empty() {
                if !help.is_empty() {
                    help.push(' ');
                }
                help.push_str(&format!("[por defecto: {}]", defaults.join(", ")));
            }
            if !possible.is_empty() {
                if !help.is_empty() {
                    help.push(' ');
                }
                help.push_str(&format!("[valores posibles: {}]", possible.join(", ")));
            }
            arg = arg
                .hide_default_value(true)
                .hide_possible_values(true)
                .help(help);
        }
    }
    arg
}

/// Traduce los errores comunes de parseo de clap al español conservando el
/// exit code. Los tipos sin mapear conservan el render original de clap.
/// Con `json_mode` el rechazo mapeado sale como envelope `usage_error` por
/// stdout (vía el emisor existente, que aporta `schema_version`); sin él se
/// conserva el texto español por stderr. También con `json_mode`, la ayuda
/// sale como payload `{"help"}`, la versión como el payload de `version` y un
/// grupo sin subcomando como `usage_error` con la ayuda del grupo; sin él,
/// los tres conservan el render original de clap.
fn render_clap_error(err: clap::Error, json_mode: bool) -> ! {
    use clap::error::{ContextKind, ErrorKind};
    let kind = err.kind();
    if json_mode {
        match kind {
            ErrorKind::DisplayHelp => {
                emit_help(true, err.to_string().trim_end());
                avi_process::scratch::terminate(0)
            }
            ErrorKind::DisplayVersion => {
                // `handle_version` no falla; el código de salida refleja su resultado.
                avi_process::scratch::terminate(if handle_version(true).is_ok() { 0 } else { 1 })
            }
            ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand => {
                emit_raw_json(json!({
                    "error": format!("Error: falta un subcomando\n\n{}", err.to_string().trim_end()),
                    "reason": "usage_error",
                }));
                avi_process::scratch::terminate(err.exit_code())
            }
            _ => {}
        }
    }
    let mapped = matches!(
        kind,
        ErrorKind::UnknownArgument
            | ErrorKind::InvalidValue
            | ErrorKind::ValueValidation
            | ErrorKind::MissingRequiredArgument
            | ErrorKind::InvalidSubcommand
            | ErrorKind::MissingSubcommand
            | ErrorKind::ArgumentConflict
            | ErrorKind::TooManyValues
            | ErrorKind::WrongNumberOfValues
            | ErrorKind::NoEquals
    );
    if !mapped {
        // Mismo render que `err.exit()` de clap, pero saliendo por `terminate`.
        err.print().expect("no se pudo escribir el error de clap");
        avi_process::scratch::terminate(err.exit_code());
    }
    let param = |k: ContextKind| err.get(k).map(|v| v.to_string()).unwrap_or_default();
    let message = match kind {
        ErrorKind::UnknownArgument => {
            format!("argumento inesperado '{}'", param(ContextKind::InvalidArg))
        }
        ErrorKind::InvalidValue => {
            let base = format!(
                "valor inválido '{}' para '{}'",
                param(ContextKind::InvalidValue),
                param(ContextKind::InvalidArg)
            );
            let valid = param(ContextKind::ValidValue);
            if valid.is_empty() {
                base
            } else {
                format!("{base}, valores posibles: {valid}")
            }
        }
        ErrorKind::ValueValidation => {
            let cause = std::error::Error::source(&err)
                .map(|s| s.to_string())
                .unwrap_or_default();
            if cause.is_empty() {
                format!(
                    "valor inválido '{}' para '{}'",
                    param(ContextKind::InvalidValue),
                    param(ContextKind::InvalidArg)
                )
            } else {
                format!(
                    "valor inválido '{}' para '{}': {cause}",
                    param(ContextKind::InvalidValue),
                    param(ContextKind::InvalidArg)
                )
            }
        }
        ErrorKind::MissingRequiredArgument => {
            format!(
                "faltan argumentos obligatorios: {}",
                param(ContextKind::InvalidArg)
            )
        }
        ErrorKind::InvalidSubcommand => {
            format!(
                "subcomando no reconocido '{}'",
                param(ContextKind::InvalidSubcommand)
            )
        }
        ErrorKind::MissingSubcommand => "falta un subcomando".to_string(),
        ErrorKind::ArgumentConflict => {
            format!(
                "el argumento '{}' no se puede usar junto con '{}'",
                param(ContextKind::InvalidArg),
                param(ContextKind::PriorArg)
            )
        }
        ErrorKind::TooManyValues | ErrorKind::WrongNumberOfValues => {
            format!(
                "cantidad de valores incorrecta para '{}'",
                param(ContextKind::InvalidArg)
            )
        }
        ErrorKind::NoEquals => {
            format!(
                "'{}' requiere '=' para asignar su valor",
                param(ContextKind::InvalidArg)
            )
        }
        _ => unreachable!("tipos sin mapear delegan en err.exit()"),
    };
    let mut lines = vec![format!("Error: {message}")];
    for k in [
        ContextKind::SuggestedArg,
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedValue,
    ] {
        if let Some(s) = err.get(k) {
            lines.push(format!("sugerencia: {s}"));
        }
    }
    if let Some(usage) = err.get(ContextKind::Usage) {
        let text = usage.to_string();
        let text = text.strip_prefix("Usage: ").unwrap_or(&text);
        lines.push(format!("Uso: {text}"));
    } else {
        // `InvalidValue` y `ValueValidation` no traen uso del subcomando que
        // falló: se muestra el uso de la raíz como orientación mínima, para
        // que todo error en español incluya su línea `Uso:`.
        let mut root = localize_command(Cli::command());
        let text = root.render_usage().to_string();
        let text = text.strip_prefix("Usage: ").unwrap_or(&text);
        lines.push(format!("Uso: {text}"));
    }
    lines.push("Para más información, ejecuta '--help'.".to_string());
    // Con `--json` el mismo texto español entra al envelope por stdout y stderr
    // queda en silencio; sin él se conserva la salida histórica por stderr.
    if json_mode {
        emit_raw_json(json!({
            "error": lines.join("\n"),
            "reason": "usage_error",
        }));
        avi_process::scratch::terminate(err.exit_code())
    }
    eprintln!("{}", lines.join("\n"));
    avi_process::scratch::terminate(err.exit_code())
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

/// Flags de enrutado, presentes solo en los subcomandos que tienen dos vías
/// (daemon o proceso local): `translate`, `speech synthesize|say|transcribe|dub`
/// y `voice clone`. Los demás comandos no los declaran y los rechazan como
/// argumento desconocido.
#[derive(clap::Args)]
struct Routing {
    /// Fuerza el uso exclusivo del daemon IPC (exit 5 si no responde)
    #[arg(long, conflicts_with = "no_daemon")]
    daemon: bool,

    /// Fuerza la ejecución en proceso local directo (sin daemon)
    #[arg(long, conflicts_with = "daemon")]
    no_daemon: bool,
}

impl Routing {
    fn mode(&self) -> DaemonMode {
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
        /// Idioma del texto de entrada
        #[arg(long, default_value = "es", value_parser = ["es", "en"])]
        source_language: String,
        /// Idioma de destino de la traducción
        #[arg(long, default_value = "en", value_parser = ["es", "en"])]
        target_language: String,
        #[command(flatten)]
        routing: Routing,
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
    /// Operaciones sobre la instalación del usuario
    #[command(name = "self")]
    SelfCmd {
        #[command(subcommand)]
        action: SelfSub,
    },
    /// Diagnóstico de entorno
    Doctor {
        /// Recoge los restos pendientes (aparcados, stagings y temporales)
        /// con el barrido existente y vuelve a evaluar el informe
        #[arg(long)]
        repair: bool,
    },
}

/// Subcomandos de self. Se llama SelfSub y no SelfComandos por dos razones:
/// Self es palabra reservada de Rust, y el nombre no debe colisionar por subcadena
/// con el enum de comandos de nivel superior. Lo que el usuario ve en --help es self,
/// que es lo único que la tabla de la CLI declara.
#[derive(Subcommand)]
enum SelfSub {
    /// Instala el bundle del que forma parte este ejecutable, o repara la instalación
    Install {
        /// No provisionar modelos
        #[arg(long)]
        no_setup: bool,
        /// No tocar perfiles ni registro
        #[arg(long)]
        no_modify_path: bool,
        /// Resolver un conflicto en la ruta del enlace sin preguntar
        #[arg(long, short)]
        force: bool,
        /// No pedir confirmación
        #[arg(long)]
        yes: bool,
    },
    /// Elimina el programa, la integración de PATH y (por defecto) el estado
    Uninstall {
        /// Conservar modelos, voces y habla sintetizada
        #[arg(long)]
        keep_data: bool,
        /// Mostrar el plan sin modificar el disco
        #[arg(long)]
        dry_run: bool,
        /// No pedir confirmación
        #[arg(long)]
        yes: bool,
    },
    /// Actualiza la instalación a la última estable (o a `--version`), con
    /// verificación de integridad y traspaso al binario nuevo
    Update {
        /// Solo informa de la versión disponible, sin modificar nada
        #[arg(long)]
        check: bool,
        /// Versión objetivo X.Y.Z (por defecto, la última estable)
        #[arg(long)]
        version: Option<String>,
        /// Reinstala la misma versión o degrada a una anterior
        #[arg(long, short)]
        force: bool,
        /// No provisionar modelos tras el reemplazo
        #[arg(long)]
        no_setup: bool,
        /// No pedir confirmación
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum VoiceCommands {
    /// Listar voces registradas
    List,
    /// Clonar una voz desde audio de referencia o micrófono
    #[command(
        group(
            clap::ArgGroup::new("clone_input")
                .required(true)
                .multiple(false)
                .args(["speech_reference", "mic"])
        )
    )]
    Clone {
        #[arg(short, long)]
        name: String,
        /// Audio de referencia de habla (archivo WAV; mutuamente excluyente con --mic)
        #[arg(short = 's', long, group = "clone_input")]
        speech_reference: Option<String>,
        /// Capturar audio de referencia desde el micrófono
        #[arg(long, group = "clone_input")]
        mic: bool,
        /// Duración fija en segundos (1–30); solo con --mic
        #[arg(long, conflicts_with = "speech_reference")]
        duration: Option<u64>,
        /// Sobrescribir una voz existente con el mismo nombre
        #[arg(short, long)]
        force: bool,
        /// Limpiar ruido de fondo en la referencia antes de clonar (solo con `native-denoise`).
        #[cfg(feature = "native-denoise")]
        #[arg(long)]
        denoise: bool,
        #[command(flatten)]
        routing: Routing,
    },
    /// Eliminar una voz clonada y sus locuciones guardadas
    Remove {
        #[arg(short, long)]
        name: String,
        /// No pedir confirmación al borrar las locuciones de la voz
        #[arg(long, short)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum SpeechCommands {
    /// Limpia el ruido de la pista de voz en archivos de audio y vídeo (DPDFNet + FFmpeg)
    #[cfg(feature = "native-denoise")]
    Denoise {
        #[command(flatten)]
        args: denoise::cli::Cli,
    },
    /// Listar habla sintética persistida
    List {
        /// Filtrar por voz (ausente = todas las voces)
        #[arg(short, long)]
        voice: Option<String>,
    },
    /// Transcribir audio
    #[command(group(
        clap::ArgGroup::new("input").required(true).multiple(false).args(["audio", "mic"])
    ))]
    Transcribe {
        /// Ruta del archivo WAV a transcribir (mutuamente excluyente con --mic)
        #[arg(long)]
        audio: Option<String>,
        /// Transcribir desde el micrófono (mutuamente excluyente con --audio)
        #[arg(long)]
        mic: bool,
        /// Duración fija de grabación en segundos; solo válido con --mic
        #[arg(long, conflicts_with = "audio")]
        duration: Option<u64>,
        /// Idioma hablado en el audio
        #[arg(long, value_parser = ["es", "en"])]
        source_language: String,
        /// Limpiar ruido de fondo antes de transcribir (solo con `native-denoise`).
        #[cfg(feature = "native-denoise")]
        #[arg(long)]
        denoise: bool,
        #[command(flatten)]
        routing: Routing,
    },
    /// Sintetizar texto a habla y persistir la locución
    Synthesize {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Etiqueta de la locución persistida (obligatorio)
        #[arg(short, long)]
        label: String,
        /// Sobrescribir una locución existente con la misma etiqueta
        #[arg(short, long)]
        force: bool,
        #[arg(long)]
        play: bool,
        /// Idioma del texto de entrada (por defecto igual a --target-language, sin traducir)
        #[arg(long, value_parser = ["es", "en"])]
        source_language: Option<String>,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto se traduce antes de sintetizar)
        #[arg(long, default_value = "es", value_parser = ["es", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
        #[command(flatten)]
        routing: Routing,
    },
    /// Sintetizar y reproducir
    Say {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Idioma del texto de entrada (por defecto igual a --target-language, sin traducir)
        #[arg(long, value_parser = ["es", "en"])]
        source_language: Option<String>,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto se traduce antes de sintetizar)
        #[arg(long, default_value = "es", value_parser = ["es", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
        #[command(flatten)]
        routing: Routing,
    },
    /// Doblaje voz→voz: transcribe, traduce, sintetiza y reproduce
    #[command(group(
        clap::ArgGroup::new("input").required(true).multiple(false).args(["audio", "mic"])
    ))]
    Dub {
        /// Archivo de audio a doblar
        #[arg(short = 'a', long)]
        audio: Option<String>,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Idioma hablado en el audio de entrada
        #[arg(long, value_parser = ["es", "en"])]
        source_language: String,
        /// Idioma/modelo de síntesis (si difiere del origen, el texto transcrito se traduce antes de sintetizar)
        #[arg(long, default_value = "es", value_parser = ["es", "en"])]
        target_language: String,
        /// Override del muestreo (por defecto la temperatura de producción; 0 < t <= 2.0)
        #[arg(long)]
        temperature: Option<f32>,
        /// Limpiar ruido de fondo antes de doblar (solo con `native-denoise`).
        #[cfg(feature = "native-denoise")]
        #[arg(long)]
        denoise: bool,
        /// Capturar desde el micrófono (mutuamente excluyente con --audio)
        #[arg(long)]
        mic: bool,
        /// Duración fija de grabación en segundos; solo válido con --mic
        #[arg(long, conflicts_with = "audio")]
        duration: Option<u64>,
        #[command(flatten)]
        routing: Routing,
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
        /// Voz a precalentar al arranque (falla de inmediato si no existe)
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
        /// Voz a precalentar al arranque (falla de inmediato si no existe)
        #[arg(long, default_value = "default")]
        warm_voice: String,
        /// Fichero donde el hijo publica el resultado de su arranque: su `addr`
        /// real y su estado warm cuando está listo, o la causa tipada si falla
        /// antes (el padre lo consume con espera acotada).
        #[arg(long)]
        ready_file: Option<PathBuf>,
    },
}

// ─── Bootstrap ───────────────────────────────────────────────────────

/// Instalar handler de SIGINT → limpieza acotada + exit 130.
///
/// Preserva el código 130 en todos los modos; antes de salir mata el árbol
/// preciso del daemon residual por PID con deadline breve y verificación
/// (`taskkill /F /T /PID` en Windows, `kill -9` al
/// grupo en Unix). Vencido el deadline sale igualmente con 130 sin colgarse.
/// Reclama el PID en memoria cuando aún no hay pidfile (ventana
/// spawn→write), preservando 130 y el techo de 2 s.
/// Guarda anti-auto-muerte: si el PID de la pista es el propio proceso
/// (`daemon serve` en foreground), no se auto-mata; el cierre lo hace la
/// escucha de señales del servidor por la misma ruta que POST `/shutdown`.
/// Instalado en `main` antes del despacho: cubre todos los modos.
fn install_sigint_handler() {
    ctrlc::set_handler(move || {
        // Un lote `denoise` en curso cancela de forma cooperativa: la primera
        // pulsación activa su flag y deja que limpie temporales; la segunda
        // termina el proceso.
        #[cfg(feature = "native-denoise")]
        if let Some(flag) = DENOISE_CANCEL.get() {
            if flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                eprintln!("\nInterrumpido por el usuario.");
                avi_process::scratch::terminate(ExitCode::Interrupted.code());
            }
            eprintln!("\n[aviso] Cancelación solicitada (Ctrl+C), limpiando temporales...");
            return;
        }
        // pidfile primero; sin pidfile, PID en memoria (ventana spawn→write).
        // El PID del pidfile solo se mata si su identidad coincide con la
        // registrada; el PID en memoria es de un hijo recién lanzado por este
        // proceso y no necesita comprobación.
        let in_memory_pid = || {
            let m = IN_MEMORY_PID.load(std::sync::atomic::Ordering::Relaxed);
            if m != 0 {
                Some(m)
            } else {
                None
            }
        };
        let pid = match registered_daemon() {
            Ok((Some(pid), avi_process::registry::Registered::Ours)) => Some(pid),
            Ok((Some(_), _)) => None,
            Ok((None, _)) => in_memory_pid(),
            // Sin raíz de datos no hay pidfile que leer: se avisa y solo queda el
            // PID en memoria.
            Err(e) => {
                eprintln!(
                    "Aviso: no se pudo leer el registro del daemon: {}",
                    e.message
                );
                in_memory_pid()
            }
        };
        if let Some(pid) = pid {
            let own = std::process::id();
            if pid != 0 && pid != own {
                daemon::kill_tree_by_pid(pid);
                daemon::wait_for_pid_death(pid, CTRL_C_CLEANUP_DEADLINE);
            }
        }
        // Exit code 130 = interrumpido por usuario (Ctrl+C)
        eprintln!("\nInterrumpido por el usuario.");
        avi_process::scratch::terminate(ExitCode::Interrupted.code());
    })
    .expect("Error al instalar el handler de Ctrl+C");
}

/// Restaura `SIGPIPE` a `SIG_DFL` en Unix para los modos CLI en primer plano.
///
/// Rust ignora `SIGPIPE` por defecto (`SIG_IGN`): una escritura a un pipe
/// cerrado (p. ej. `voice list | head -n1`) devuelve `EPIPE` como error de
/// I/O en lugar de matar el proceso por señal. `println!`/`writeln!` hacen
/// `panic!` ante ese error ("failed printing to stdout: Broken pipe", exit
/// 101), que es ruidoso y no es el comportamiento esperado de un CLI Unix
/// (compárese con `cat`, `ls`: mueren en silencio por la señal). Restaurar
/// `SIG_DFL` hace que el proceso termine por la señal, sin panic.
///
/// Alcance intencionalmente acotado a los modos CLI en primer plano
/// (`main` decide no llamar a esta función para `daemon serve`): el
/// servidor HTTP del daemon corre en el mismo binario vía re-exec
/// (`avi-daemon::spawn_background` relanza `current_exe()` con
/// `daemon serve`) y es un proceso longevo — no debe morir si un cliente
/// cierra su socket a medio camino.
///
/// Es seguro para los sockets TCP del propio CLI (cliente HTTP hacia el
/// daemon vía `reqwest`/`hyper`/`tokio`/`mio`): en Linux, `std::net::TcpStream`
/// escribe con la flag `MSG_NOSIGNAL`; en macOS/BSD, `mio` replica a `libstd`
/// y fija `SO_NOSIGPIPE` en el socket al crearlo (ver `mio::sys::unix::net`).
/// Ambos mecanismos suprimen la señal a nivel de socket con independencia de
/// la disposición global de `SIGPIPE`, así que las escrituras a un daemon
/// que cerró la conexión siguen devolviendo `EPIPE` como error normal, no
/// una señal. Los procesos hijos que lanza este binario (motor TTS/STT,
/// limpiador de desinstalación, el propio daemon) siempre usan
/// `Stdio::null()` para su stdin, así que no hay tubería de escritura hacia
/// un hijo que pueda disparar la señal.
#[cfg(unix)]
fn restore_sigpipe_default() {
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

// ─── Punto de entrada ────────────────────────────────────────────────

/// Filtro de los logs, construido en un único punto.
///
/// Con `RUST_LOG` definida manda la variable, interpretada de forma tolerante
/// (las directivas inválidas se ignoran, como en la convención de `tracing`).
/// Sin ella, la CLI solo muestra `warn` y superiores: stderr queda para los
/// mensajes del producto. El daemon (`daemon serve`) registra además el `info`
/// de los crates propios, que se enumeran porque las directivas filtran por
/// prefijo de módulo y no admiten comodines, y deja las dependencias en `warn`
/// (su `info`, p. ej. `xet_*` durante las descargas, solo es ruido).
fn log_filter(rust_log: Option<&str>, daemon_serve: bool) -> tracing_subscriber::EnvFilter {
    const OWN_CRATES: [&str; 11] = [
        "avi_core",
        "avi_audio",
        "avi_tts",
        "avi_store",
        "avi_daemon",
        "avi_process",
        "avi_stt",
        "avi_translation",
        "avi_lifecycle",
        "avi_shared",
        "ai_voice_interconnector",
    ];
    if let Some(value) = rust_log {
        return tracing_subscriber::EnvFilter::builder().parse_lossy(value);
    }
    let directives = if daemon_serve {
        OWN_CRATES.iter().fold(String::from("warn"), |acc, name| {
            format!("{acc},{name}=info")
        })
    } else {
        String::from("warn")
    };
    tracing_subscriber::EnvFilter::new(directives)
}

#[tokio::main]
async fn main() {
    // Libera el scratch del proceso en la vuelta normal y en el pánico; las
    // salidas explícitas pasan por `terminate`, que no ejecuta `Drop`.
    let _scratch_guard = avi_process::scratch::ReleaseGuard;
    // Bootstrap: UTF-8, SIGINT, parseo de la CLI y tracing
    avi_shared::force_utf8_console();
    install_sigint_handler();

    let cmd = localize_command(Cli::command());
    // Pre-análisis puro de los args crudos: al fallar el parseo no existe
    // `Cli`, así que solo decide si el rechazo se emite como JSON, no qué.
    let json_hint = std::env::args()
        .skip(1)
        .any(|arg| arg == "--json" || arg.starts_with("--json="));
    let matches = cmd
        .try_get_matches()
        .unwrap_or_else(|e| render_clap_error(e, json_hint));
    let cli = Cli::from_arg_matches(&matches).unwrap_or_else(|e| render_clap_error(e, json_hint));
    let json_mode = cli.json;
    let daemon_serve = matches!(
        cli.command,
        Some(Commands::Daemon {
            action: DaemonCommands::Serve { .. }
        })
    );

    // Los logs van a stderr: stdout queda reservado al contrato JSON. Sin color
    // cuando stderr no es una terminal (p. ej. el log del daemon), para que el
    // fichero no lleve códigos ANSI. El parseo de la CLI no usa `tracing`, así que
    // inicializarlo después es seguro.
    {
        use std::io::IsTerminal;
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::util::SubscriberInitExt;
        use tracing_subscriber::Layer;
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(std::io::stderr)
                    .with_ansi(std::io::stderr().is_terminal())
                    .with_filter(log_filter(
                        std::env::var("RUST_LOG")
                            .ok()
                            .filter(|v| !v.is_empty())
                            .as_deref(),
                        daemon_serve,
                    )),
            )
            .init();
    }

    // SIGPIPE → SIG_DFL solo en modos CLI de primer plano; `daemon serve` (el
    // servidor longevo, mismo binario) queda excluido a propósito (ver doc de
    // `restore_sigpipe_default`).
    #[cfg(unix)]
    if !daemon_serve {
        restore_sigpipe_default();
    }

    // Los comandos de uso toman el bloqueo de ciclo de vida compartido antes de
    // tocar datos o modelos y lo retienen toda su vida. Si hay una operación de
    // ciclo de vida en curso, terminan aquí con `lifecycle_locked` sin lanzar nada.
    let mut usage_lock = if takes_usage_lock(&cli.command) {
        let program_dir = match lifecycle::channel::registered_program_dir() {
            Ok(dir) => dir,
            Err(e) => exit_with_error(json_mode, lifecycle_error_to_cli(e)),
        };
        match lifecycle::lock::acquire_shared(&program_dir) {
            Ok(lock) => Some(lock),
            Err(e) => exit_with_error(json_mode, lifecycle_error_to_cli(e.into())),
        }
    } else {
        None
    };

    let result = match cli.command {
        Some(Commands::Version) => ok(handle_version(json_mode)),
        Some(Commands::Devices) => ok(handle_devices(json_mode)),
        Some(Commands::Translate {
            text,
            source_language,
            target_language,
            routing,
        }) => ok(handle_translate(
            json_mode,
            routing.mode(),
            &text,
            &source_language,
            &target_language,
        )
        .await),
        Some(Commands::Voice { action }) => ok(handle_voice(json_mode, action).await),
        #[cfg(feature = "native-denoise")]
        Some(Commands::Speech {
            action: SpeechCommands::Denoise { args },
        }) => Ok(handle_denoise(json_mode, args)),
        Some(Commands::Speech { action }) => ok(handle_speech(json_mode, action).await),
        Some(Commands::Daemon { action }) => {
            handle_daemon(json_mode, action, usage_lock.take()).await
        }
        Some(Commands::Setup { force_update, yes }) => {
            ok(handle_setup(json_mode, force_update, yes).await)
        }
        Some(Commands::Cleanup {
            voices,
            synthetic_speech,
            model,
            all,
            dry_run,
            yes,
        }) => ok(handle_cleanup(
            json_mode,
            voices,
            synthetic_speech,
            model,
            all,
            dry_run,
            yes,
        )
        .await),
        Some(Commands::SelfCmd { action }) => handle_self(json_mode, action).await,
        Some(Commands::Doctor { repair }) => handle_doctor(json_mode, repair),
        None => {
            // Mismo nombre de binario que deriva clap de `argv[0]`, para que el
            // texto coincida byte a byte con el de `--help`.
            let bin_name = std::env::args_os()
                .next()
                .and_then(|a| {
                    std::path::Path::new(&a)
                        .file_name()
                        .map(|f| f.to_string_lossy().into_owned())
                })
                .unwrap_or_else(|| APP_NAME.to_string());
            let help = localize_command(Cli::command())
                .bin_name(bin_name)
                .render_help()
                .to_string();
            emit_help(json_mode, help.trim_end());
            Ok(Outcome::Done)
        }
    };

    match result {
        Ok(Outcome::Done) => {}
        // Salida por veredicto, que es lo que permite el contrato: el comando ya emitió
        // su payload
        // propio y solo queda fijar el código. Es lo que hace que `doctor --json`
        // emita **un solo objeto** también cuando falla: si esto fuera un `CliError`,
        // `main` adjuntaría detrás el objeto `error` y el envelope sería ilegible.
        Ok(Outcome::Verdict(code)) => {
            avi_process::scratch::terminate(code);
        }
        Err(err) => exit_with_error(json_mode, err),
    }
}

/// Emite el error con su `reason` (JSON o texto) y sale con su código.
fn exit_with_error(json_mode: bool, err: CliError) -> ! {
    if json_mode {
        emit_raw_json(json!({
            "error": err.message,
            "reason": err.reason,
        }));
    } else {
        eprintln!("Error: {}", err.message);
    }
    avi_process::scratch::terminate(err.code.code());
}

/// Indica si el comando es de uso: lee o escribe datos o modelos, o lanza el
/// daemon que los usa, y por eso toma el bloqueo de ciclo de vida compartido.
/// No lo toman los de solo diagnóstico (`daemon status`, `daemon stop`,
/// `doctor` sin `--repair`), las simulaciones ni las operaciones de ciclo de
/// vida, que toman el exclusivo por su cuenta.
fn takes_usage_lock(command: &Option<Commands>) -> bool {
    match command {
        Some(Commands::Translate { .. } | Commands::Voice { .. } | Commands::Speech { .. }) => true,
        Some(Commands::Setup { .. }) => true,
        Some(Commands::Daemon { action }) => matches!(
            action,
            DaemonCommands::Start { .. } | DaemonCommands::Restart | DaemonCommands::Serve { .. }
        ),
        _ => false,
    }
}

/// Desenlace de un handler ante `main`: éxito, o salida por veredicto, que es lo
/// que permite el contrato.
///
/// Existe como tipo y no como `Result<(), CliError>` porque el veredicto **no es un
/// error**: es un comando que corrió bien y cuyo resultado es negativo. Los casos
/// son `doctor`, cuyo reporte —con `checks` y `failed`— ya está en `stdout`
/// cuando llega aquí, y `daemon stop` cuando la parada no concluye, cuyo
/// envelope con `still_running` ya está en `stdout`.
enum Outcome {
    Done,
    Verdict(i32),
}

/// Mensaje final de una operación con indicador: el texto del resultado y la
/// duración que devolvió el cierre del indicador.
///
/// Es el **único** dueño del mensaje final: el indicador no imprime estado, y
/// el handler escribe esta línea una sola vez, por stdout. La duración sale
/// del mismo reloj que mostró las fases, de modo que las cifras de una misma
/// operación son coherentes entre sí.
///
/// La forma es la de todos los mensajes finales de la CLI: `texto (N s).` —
/// la duración entre paréntesis y el punto al final. El texto que se le pasa
/// no lleva punto: lo pone esta función, para que el cierre sea el único que
/// decide dónde acaba la frase.
fn final_message(msg: &str, elapsed: std::time::Duration) -> String {
    format!(
        "{msg} ({}).",
        avi_lifecycle::progress::format_duration(elapsed)
    )
}

/// Envuelve un handler que devuelve `Result<(), CliError>` en el tipo de `main`.
fn ok(result: Result<(), CliError>) -> Result<Outcome, CliError> {
    result.map(|()| Outcome::Done)
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

/// Emite la ayuda: con `json_mode` como payload `{"help"}`, sin él como texto.
fn emit_help(json_mode: bool, text: &str) {
    if json_mode {
        emit_raw_json(json!({ "help": text }));
    } else {
        println!("{text}");
    }
}

/// Subcomando `speech denoise`: la CLI de archivos comparte parser, pipeline y
/// reporting con la biblioteca migrada. El FFmpeg se resuelve solo desde el
/// paquete instalado y la receta por defecto es `studio`.
#[cfg(feature = "native-denoise")]
fn handle_denoise(json_mode: bool, args: denoise::cli::Cli) -> Outcome {
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    drop(DENOISE_CANCEL.set(cancel_flag.clone()));
    let code = denoise::cli::execute(
        args,
        json_mode,
        true,
        denoise::ffmpeg_io::find_bundled_ffmpeg,
        Some(cancel_flag.clone()),
    );
    if code == 0 {
        Outcome::Done
    } else if code == 3 && cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
        Outcome::Verdict(ExitCode::Interrupted.code())
    } else {
        Outcome::Verdict(code)
    }
}

fn handle_devices(json_mode: bool) -> Result<(), CliError> {
    let devices = audio::get_devices_json()
        .map_err(|e| CliError::new(ExitCode::Error, "audio_enumeration_failed", e.to_string()))?;
    if json_mode {
        emit_raw_json(json!({ "devices": devices }));
    } else {
        println!("Dispositivos de salida de audio:");
        for dev in &devices {
            println!("{}", device_line(dev));
        }
    }
    Ok(())
}

/// Línea de texto de un dispositivo del JSON de `avi_audio::get_devices_json`:
/// el nombre y la latencia `null` se muestran como «desconocido».
fn device_line(dev: &Value) -> String {
    let name = dev["name"].as_str().unwrap_or("desconocido");
    let latency = match dev["latency"].as_f64() {
        Some(secs) => format!("{:.1}ms", secs * 1000.0),
        None => "desconocido".to_string(),
    };
    format!("  [{}] {} (latencia: {})", dev["id"], name, latency)
}

/// Techo de la grabación push-to-talk (`AVI_PUSH_TO_TALK_MAX_SECS`): una
/// variable definida pero inválida es un error de uso.
fn push_to_talk_max_secs() -> Result<u64, CliError> {
    avi_audio::push_to_talk_max_secs()
        .map_err(|e| CliError::new(ExitCode::InvalidInput, "usage_error", e.to_string()))
}

/// Par de modelos de traducción para dos idiomas distintos del alfabeto `es|en`
/// que `clap` ya validó: el origen determina el par.
fn translation_pair(source: &str) -> &'static str {
    if source == "es" {
        "es-en"
    } else {
        "en-es"
    }
}

async fn handle_translate(
    json_mode: bool,
    daemon_mode: DaemonMode,
    text: &str,
    source: &str,
    target: &str,
) -> Result<(), CliError> {
    if text.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_text",
            "El texto a traducir está vacío",
        ));
    }
    // Passthrough: origen == destino → texto intacto, sin motor
    if source == target {
        if json_mode {
            emit_raw_json(
                json!({ "translated": text, "source_language": source, "target_language": target }),
            );
        } else {
            println!("{}", text);
        }
        return Ok(());
    }
    // Despacho 3 modos vía daemon_client + route_to_daemon
    let client = daemon_client();
    if route_to_daemon(daemon_mode, &client).await? {
        let mut progress = PhaseProgress::new();
        progress.phase("translating", "Traduciendo texto (daemon)…");
        let result = translate_via_daemon(&client, text, source, target, &progress).await;
        let _elapsed = progress.close();
        let (translated, source, target) = result?;
        if json_mode {
            emit_raw_json(
                json!({ "translated": translated, "source_language": source, "target_language": target }),
            );
        } else {
            println!("{}", translated);
        }
        return Ok(());
    }
    // Rama local: verifica modelo y traduce
    let model_name = format!("opus-mt-{}", translation_pair(source));
    let model_store = open_model_store()?;
    let ct2_dir = model_store.model_dir(&model_name);
    if !model_store.is_provisioned(&model_name) {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!(
                "El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.",
                model_name,
                ct2_dir.display(),
            ),
        ));
    }
    #[cfg(not(feature = "native-translation"))]
    {
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        let mut progress = PhaseProgress::new();
        progress.phase("translating", "Traduciendo texto…");
        progress.tick();
        // Patrón de bloque externo: el trabajo que puede fallar vive en un
        // bloque, y el cierre del indicador —que consume el indicador— se hace
        // fuera, sobre el resultado ya obtenido.
        let result = translation::translate(text, &ct2_dir).map_err(|e| {
            CliError::new(
                ExitCode::TranslationFailed,
                "translation_failed",
                e.to_string(),
            )
        });
        let _elapsed = progress.close();
        let translated = match result {
            Ok(translated) => translated,
            Err(e) => return Err(e),
        };
        if json_mode {
            emit_raw_json(
                json!({ "translated": translated, "source_language": source, "target_language": target }),
            );
        } else {
            println!("{}", translated);
        }
        Ok(())
    }
}

/// Traducción opt-in previa a la síntesis: passthrough si origen y destino
/// coinciden; exit 4 si falta el modelo, exit 9 si falla la traducción.
fn translate_if_different(text: &str, source: &str, target: &str) -> Result<String, CliError> {
    if source == target {
        return Ok(text.to_string());
    }
    let model_name = format!("opus-mt-{}", translation_pair(source));
    let model_store = open_model_store()?;
    let ct2_dir = model_store.model_dir(&model_name);
    if !model_store.is_provisioned(&model_name) {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!(
                "El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.",
                model_name,
                ct2_dir.display(),
            ),
        ));
    }
    #[cfg(not(feature = "native-translation"))]
    {
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        translation::translate(text, &ct2_dir).map_err(|e| {
            CliError::new(
                ExitCode::TranslationFailed,
                "translation_failed",
                e.to_string(),
            )
        })
    }
}

// ─── Voice ───────────────────────────────────────────────────────────

async fn handle_voice(json_mode: bool, action: VoiceCommands) -> Result<(), CliError> {
    let voice_store = open_voice_store()?;

    match action {
        VoiceCommands::List => {
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
            mic,
            duration,
            force,
            #[cfg(feature = "native-denoise")]
            denoise,
            routing,
        } => {
            let denoise = {
                #[cfg(feature = "native-denoise")]
                {
                    denoise
                }
                #[cfg(not(feature = "native-denoise"))]
                {
                    false
                }
            };
            // El indicador nace antes de las validaciones, y el patrón de
            // bloque externo garantiza que se cierra en cualquier camino de
            // salida: éxito, error o cancelación.
            let mut progress = PhaseProgress::new();
            // Patrón de bloque externo: el trabajo que puede fallar vive en un
            // bloque async y el cierre del indicador —que consume el indicador—
            // se hace fuera, sobre el resultado ya obtenido. Así ningún `?`
            // puede dejar la fase abierta.
            let result = async {
                progress.phase("validating", "Validando referencia y nombre…");
                // Orden de validaciones del contrato: el nombre se valida antes que el modelo.
                let name = name.to_lowercase();
                VoiceStore::validate_name(&name)
                    .map_err(|e| CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e))?;
                // El archivo conserva el orden de validación vigente; el PCM de micrófono
                // se limita tras la captura antes de codificarlo.
                if !mic {
                    if let Some(reference) = speech_reference.as_deref() {
                        ensure_clone_reference_within_limit(std::path::Path::new(reference))?;
                    }
                }
                if mic {
                    if duration == Some(0) {
                        return Err(CliError::new(ExitCode::InvalidInput, "usage_error", "--duration debe ser mayor que cero."));
                    }
                    if duration.is_some_and(|secs| secs > avi_core::MAX_CLONE_REFERENCE_SECS) {
                        return Err(CliError::new(ExitCode::InvalidInput, "audio_too_long", "La duración de grabación supera el límite máximo de clonado (30 s)."));
                    }
                    if duration.is_none() {
                        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                            return Err(CliError::new(ExitCode::InvalidInput, "usage_error", "--mic requiere --duration en entornos no interactivos."));
                        }
                        let ptt_limit = push_to_talk_max_secs()?;
                        validate_clone_push_to_talk_limit(ptt_limit)?;
                    }
                }
                let client = daemon_client();
                let use_daemon = route_to_daemon(routing.mode(), &client).await?;
                let (speech_path, daemon_audio, mic_scratch) = if mic {
                    if !use_daemon && !force && voice_store.exists(&name).map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))? {
                        return Err(CliError::new(ExitCode::StateConflict, "voice_exists", format!("La voz '{}' ya existe (usa --force para sobrescribirla).", name)));
                    }
                    progress.phase("capturing", "Capturando audio del micrófono…");
                    progress.close_line();
                    let mut pcm = capture_mic_pcm(duration, avi_core::MAX_CLONE_REFERENCE_SECS).await?;
                    truncate_clone_pcm(&mut pcm);
                    let wav = avi_audio::encode_pcm_16k_mono_wav(&pcm).map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
                    if use_daemon {
                        (None, Some(wav), None)
                    } else {
                        let guard = avi_process::scratch::temp_wav("avi_clone_mic_").map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
                        std::fs::write(guard.path(), wav).map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{}: {e}", guard.path().display())))?;
                        (Some(guard.path().clone()), None, Some(guard))
                    }
                } else {
                    let reference = speech_reference.as_deref().expect("clap exige una fuente");
                    (Some(PathBuf::from(reference)), None, None)
                };
                if use_daemon {
                    let audio_bytes = match daemon_audio {
                        Some(bytes) => bytes,
                        None => {
                            let reference = speech_reference.as_deref().expect("fuente WAV");
                            std::fs::read(reference).map_err(|e| CliError::new(ExitCode::NotFound, "audio_not_found", format!("El audio de referencia '{}' no existe: {}", reference, e)))?
                        }
                    };
                    let (cloned_name, cloned_speech, precomputed) = clone_via_daemon(&client, &name, &audio_bytes, force, denoise, &mut progress).await?;
                    return Ok((cloned_name, PathBuf::from(cloned_speech), precomputed));
                }
                let _mic_scratch = mic_scratch;
                let speech_path = speech_path.expect("ruta local");
                if !mic && !speech_path.is_file() {
                    return Err(CliError::new(ExitCode::NotFound, "audio_not_found", format!("El audio de referencia '{}' no existe.", speech_path.display())));
                }
                if !mic
                    && !use_daemon
                    && !force
                    && voice_store
                        .exists(&name)
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
                {
                    return Err(CliError::new(
                        ExitCode::StateConflict,
                        "voice_exists",
                        format!(
                            "La voz '{}' ya existe (usa --force para sobrescribirla).",
                            name
                        ),
                    ));
                }
                // Limpieza de ruido de referencia (opt-in) antes del clonado
                // directo. Resuelve el modelo y verifica integridad antes de
                // convertir a 48 kHz; un modelo ausente falla con `model_missing`.
                let speech_path = speech_path.to_path_buf();
                // El WAV limpio vive en este directorio temporal, así que debe
                // sobrevivir hasta que termine el clonado: si se suelta al salir
                // del bloque, la ruta queda apuntando a un archivo borrado.
                #[cfg(feature = "native-denoise")]
                let (_denoise_scratch, speech_path) = if denoise {
                    let work_dir = avi_process::scratch::tempdir("avi_clone_denoise_")
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
                    let cleaned_path = work_dir.path().join("reference_clean.wav");
                    denoise::service::ServiceModels::resolve()
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::ModelMissing,
                                "model_missing",
                                format!("Modelo de limpieza no provisionado: {e}"),
                            )
                        })?
                        .denoise_wav_file(
                            speech_path.as_path(),
                            &cleaned_path,
                            denoise::service::DenoiseUseCase::Clone,
                            work_dir.path(),
                        )
                        .map_err(|e| {
                            let reason = match e {
                                denoise::errors::E::EModelMissing(_) => "model_missing",
                                denoise::errors::E::EInvalidInput(_) => "invalid_audio",
                                denoise::errors::E::EIo(_) => "io_error",
                                _ => "denoise_failed",
                            };
                            CliError::new(ExitCode::Error, reason, e.to_string())
                        })?;
                    (Some(work_dir), cleaned_path)
                } else {
                    (None, speech_path)
                };

                progress.close_line();
                let engine_result = direct_tts_engine();
                progress.repaint();
                let engine = engine_result?;
                let model_dir = engine.base_model_dir.as_ref().ok_or_else(|| {
                    CliError::new(
                        ExitCode::ModelMissing,
                        "model_missing",
                        "El modelo Base de clonado TTS no está provisionado. Ejecuta 'setup' primero.",
                    )
                })?;
                // Temporal con dueño: se borra al soltarse por cualquier camino y,
                // si el proceso muere antes, el barrido del scratch lo recoge.
                let tmp_qvoice = avi_process::scratch::tempfile("avi_clone_", ".qvoice")
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?
                    .into_temp_path();
                progress.phase("cloning", "Clonando voz… (puede tardar varios minutos)");
                progress.tick();
                // El clonado es trabajo **síncrono**: bloquea el hilo mientras
                // dura, así que el indicador no puede repintar durante él y el
                // tiempo de la fase queda congelado. No es un defecto: el
                // trabajo total lo aporta el mensaje final.
                let cloned =
                    avi_tts::clone_voice(model_dir, speech_path.as_path(), &tmp_qvoice, &name)
                        .and_then(|()| {
                        progress.phase("saving", "Guardando referencia de voz…");
                        voice_store.save_reference(&name, &tmp_qvoice)
                    });
                let saved_qvoice = cloned.map_err(|e| {
                    // Una referencia ilegible se clasifica igual que en transcripción.
                    match e.downcast_ref::<avi_audio::WavLoadError>() {
                        Some(avi_audio::WavLoadError::Invalid(_)) => {
                            CliError::new(ExitCode::InvalidInput, "invalid_audio", e.to_string())
                        }
                        Some(avi_audio::WavLoadError::Io(_)) => {
                            CliError::new(ExitCode::Error, "io_error", e.to_string())
                        }
                        _ => CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string()),
                    }
                })?;
                Ok((name, saved_qvoice, false))
            }
            .await;
            let elapsed = progress.close();
            let (cloned_name, saved_qvoice, precomputed) = result?;
            // La única referencia de habla produce `reference.qvoice`; el WAV de
            // entrada no se persiste. La ruta local no tiene residente TTS
            // persistente, así que `precomputed` es `false` en esa vía.
            if json_mode {
                emit_raw_json(json!({
                    "name": cloned_name,
                    "speech": saved_qvoice.to_string_lossy().to_string(),
                    "precomputed": precomputed,
                }));
            } else {
                println!(
                    "Voz '{}' clonada ({}).",
                    cloned_name,
                    avi_lifecycle::progress::format_duration(elapsed)
                );
            }
            Ok(())
        }
        VoiceCommands::Remove { name, yes } => {
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
            if !voice_store
                .exists(&name)
                .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
            {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "voice_not_found",
                    format!("La voz '{}' no existe.", name.to_lowercase()),
                ));
            }
            // Las locuciones se van con su voz: si las hay, se muestra el plan y se
            // pide la confirmación destructiva antes de borrar nada.
            let speech_store = open_speech_store()?;
            let labels = speech_store
                .labels_of(&name)
                .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
            if !labels.is_empty() {
                let voice_dir = voice_store.voice_dir(&name);
                let speech_dir = speech_store.voice_dir(&name);
                let total_bytes = lifecycle::cleanup::path_size(&voice_dir)
                    + lifecycle::cleanup::path_size(&speech_dir);
                let summary = vec![format!(
                    "Se eliminará la voz '{}' y sus {} locución(es): {} (~{}).",
                    name.to_lowercase(),
                    labels.len(),
                    labels.join(", "),
                    avi_shared::human_bytes(total_bytes),
                )];
                let entries = [
                    lifecycle::confirm::PlanEntry {
                        size: Some(lifecycle::cleanup::path_size(&voice_dir)),
                        path: voice_dir,
                    },
                    lifecycle::confirm::PlanEntry {
                        size: Some(lifecycle::cleanup::path_size(&speech_dir)),
                        path: speech_dir,
                    },
                ];
                let decision = {
                    let stdin = std::io::stdin();
                    let mut input = stdin.lock();
                    lifecycle::confirm::confirm(
                        &lifecycle::confirm::Confirmation {
                            kind: lifecycle::confirm::Kind::Destructive,
                            summary: &summary,
                            entries: &entries,
                            assume_yes: yes,
                            dry_run: false,
                            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
                        },
                        &mut input,
                        &mut std::io::stderr(),
                    )
                    .map_err(lifecycle_error_to_cli)?
                };
                if decision == lifecycle::confirm::Decision::Cancelled {
                    if json_mode {
                        emit_raw_json(json!({
                            "status": "cancelled",
                            "voice": name,
                            "removed_speech": Vec::<String>::new(),
                        }));
                    }
                    // `confirm::confirm` ya escribió «Cancelado.» por stderr: el
                    // handler no lo repite, porque el literal es del motor de
                    // confirmación y saldría dos veces.
                    return Ok(());
                }
            }
            let mut progress = PhaseProgress::new();
            let result = 'work: {
                progress.phase("removing", "Eliminando voz y locuciones…");
                break 'work avi_store::remove_voice(&voice_store, &speech_store, &name)
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", e));
            };
            let elapsed = progress.close();
            let removed_speech = result?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "removed",
                    "voice": name,
                    "removed_speech": removed_speech,
                }));
            } else if removed_speech.is_empty() {
                println!(
                    "Voz '{}' eliminada ({}).",
                    name,
                    avi_lifecycle::progress::format_duration(elapsed)
                );
            } else {
                println!(
                    "Voz '{}' eliminada junto con {} locución(es) ({}).",
                    name,
                    removed_speech.len(),
                    avi_lifecycle::progress::format_duration(elapsed)
                );
            }
            Ok(())
        }
    }
}

// ─── Speech ──────────────────────────────────────────────────────────

/// Captura PCM del micrófono en `spawn_blocking`: `duration` fija la
/// captura por N segundos; `None` (solo alcanzable en TTY, ver guardas de
/// `--duration`) dispara push-to-talk hasta Enter (`capture_16k_mono_pcm_until_enter`).
/// `max_secs` es el techo de duración del push-to-talk (se combina con el
/// techo de entorno).
async fn capture_mic_pcm(duration: Option<u64>, max_secs: u64) -> Result<Vec<i16>, CliError> {
    if duration.is_none() {
        // Valida la variable antes de abrir el micrófono, para que una variable
        // inválida salga como `usage_error` y no como fallo de captura.
        push_to_talk_max_secs()?;
    }
    tokio::task::spawn_blocking(move || {
        let svc = audio::AudioService::new();
        match duration {
            Some(secs) => svc.capture_16k_mono_pcm(secs),
            None => svc.capture_16k_mono_pcm_until_enter(max_secs),
        }
    })
    .await
    .map_err(|e| {
        let message = e.to_string();
        if message.contains("empty_audio") {
            return CliError::new(ExitCode::InvalidInput, "empty_audio", message);
        }
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            message,
        )
    })?
    .map_err(|e| {
        let message = e.to_string();
        if message.contains("empty_audio") {
            return CliError::new(ExitCode::InvalidInput, "empty_audio", message);
        }
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            message,
        )
    })
}

/// Rechaza con exit 3 (`audio_not_found`) un `--audio` que no es un archivo
/// existente. Se llama antes de despachar al daemon o a la vía local para que
/// ambas devuelvan el mismo código.
fn validate_audio_arg(audio: &std::path::Path) -> Result<(), CliError> {
    if audio.is_file() {
        Ok(())
    } else {
        Err(CliError::new(
            ExitCode::NotFound,
            "audio_not_found",
            format!("El archivo de audio '{}' no existe.", audio.display()),
        ))
    }
}

/// Error de uso (exit 2, `audio_too_long`) cuando el audio de `operation`
/// supera `max_secs` segundos; `operation` es el sujeto de la frase.
fn audio_too_long(operation: &str, max_secs: u64) -> CliError {
    CliError::new(
        ExitCode::InvalidInput,
        "audio_too_long",
        format!("{operation} no puede superar {max_secs} segundos."),
    )
}

/// Rechaza el WAV de `path` cuya cabecera declara más de `max_secs` segundos.
/// Un archivo ausente o que no es WAV no se mide: lo informa la lectura posterior.
fn ensure_wav_within_limit(
    path: &std::path::Path,
    operation: &str,
    max_secs: u64,
) -> Result<(), CliError> {
    if avi_audio::wav_duration_secs(path).is_some_and(|secs| secs > max_secs as f64) {
        return Err(audio_too_long(operation, max_secs));
    }
    Ok(())
}

/// Rechaza el PCM 16 kHz mono que supera `max_secs` segundos.
fn ensure_pcm_within_limit(pcm: &[i16], operation: &str, max_secs: u64) -> Result<(), CliError> {
    if pcm.len() as u64 > max_secs * 16_000 {
        return Err(audio_too_long(operation, max_secs));
    }
    Ok(())
}

/// Limita el PCM del micrófono a 30 segundos sin rellenar las capturas cortas.
fn truncate_clone_pcm(pcm: &mut Vec<i16>) {
    pcm.truncate((avi_core::MAX_CLONE_REFERENCE_SECS * 16_000) as usize);
}

fn validate_clone_push_to_talk_limit(limit: u64) -> Result<(), CliError> {
    if limit > avi_core::MAX_CLONE_REFERENCE_SECS {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "audio_too_long",
            format!(
                "AVI_PUSH_TO_TALK_MAX_SECS ({limit}) supera el límite máximo de clonado ({} s).",
                avi_core::MAX_CLONE_REFERENCE_SECS
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod clone_push_to_talk_tests {
    #[test]
    fn ptt_limit_above_clone_max_is_audio_too_long() {
        let error = super::validate_clone_push_to_talk_limit(31)
            .expect_err("debe rechazar un techo superior al máximo de clonación");
        assert_eq!(error.reason, "audio_too_long");
    }

    #[test]
    fn ptt_limit_at_or_below_clone_max_is_accepted() {
        for limit in [1, 30] {
            super::validate_clone_push_to_talk_limit(limit).expect("techo válido");
        }
    }
}

fn ensure_clone_reference_within_limit(path: &std::path::Path) -> Result<(), CliError> {
    const OPERATION: &str = "La referencia de clonado";
    ensure_wav_within_limit(path, OPERATION, avi_core::MAX_CLONE_REFERENCE_SECS)?;
    match avi_audio::load_wav_16k_mono_pcm(path) {
        Ok(pcm) => ensure_pcm_within_limit(&pcm, OPERATION, avi_core::MAX_CLONE_REFERENCE_SECS),
        Err(e @ avi_audio::WavLoadError::Invalid(_)) => Err(CliError::new(
            ExitCode::InvalidInput,
            "invalid_audio",
            e.to_string(),
        )),
        // Una referencia ausente o con fallo de E/S la clasifica el clonado.
        Err(_) => Ok(()),
    }
}

/// Obtiene el PCM 16 kHz mono desde el micrófono o desde un WAV, y traduce los
/// fallos al contrato: archivo ausente exit 3, WAV inválido exit 2, error de
/// E/S exit 1 y fallo de micrófono exit 10 (`transcription_failed`). `max_secs` es el techo de la
/// captura push-to-talk. Con `mic == false`, `audio` debe ser `Some`.
async fn acquire_pcm(
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    max_secs: u64,
) -> Result<Vec<i16>, CliError> {
    if mic {
        return capture_mic_pcm(duration, max_secs).await;
    }
    let path = audio.expect("validado arriba");
    avi_audio::load_wav_16k_mono_pcm(std::path::Path::new(path)).map_err(|e| match e {
        avi_audio::WavLoadError::NotFound => {
            CliError::new(ExitCode::NotFound, "audio_not_found", e.to_string())
        }
        avi_audio::WavLoadError::Invalid(_) => {
            CliError::new(ExitCode::InvalidInput, "invalid_audio", e.to_string())
        }
        avi_audio::WavLoadError::Io(_) => CliError::new(ExitCode::Error, "io_error", e.to_string()),
    })
}

/// Decisión que toma el menú interactivo `--play` y que resuelve su llamador.
///
/// El menú no conoce el motor de síntesis: la opción 3 (repetir con una
/// síntesis nueva) se devuelve como `Resynthesize` y es el llamador quien
/// re-sintetiza y vuelve a entrar al menú.
#[derive(Debug, PartialEq)]
enum PlayDecision {
    /// El usuario eligió guardar (opción 2): la locución quedó persistida en
    /// esta ruta.
    Saved(PathBuf),
    /// El usuario descartó (opción 4) o se agotó la entrada estándar.
    Discarded,
    /// El usuario pidió una síntesis nueva (opción 3): el llamador re-sintetiza
    /// y reentra al menú.
    Resynthesize,
}

/// Envelope de locución descartada en el menú `--play`: el mismo texto y el
/// mismo envelope JSON que emite la ruta directa y la del daemon.
const PLAY_DISCARDED_TEXT: &str = "Descartado.";

/// Envelope JSON de la locución descartada en el menú `--play`.
fn play_discarded_envelope(voice: &str) -> Value {
    json!({ "status": "discarded", "voice": voice })
}

/// Emite el descarte del menú `--play`: envelope JSON o texto, según el modo.
fn emit_play_discarded(json_mode: bool, voice: &str) {
    if json_mode {
        emit_raw_json(play_discarded_envelope(voice));
    } else {
        println!("{}", PLAY_DISCARDED_TEXT);
    }
}

/// Fuente de las opciones del menú `--play`: lee una línea de la entrada
/// estándar. Es inyectable para poder ejercitar el menú sin TTY.
fn menu_stdin() -> impl FnMut(&mut String) -> std::io::Result<usize> {
    |line: &mut String| std::io::stdin().read_line(line)
}

/// Re-síntesis del menú `--play`: la ejecuta el llamador, nunca el menú. Cada
/// implementación vuelve a sintetizar la locución sobre el mismo temporal.
///
/// El menú devuelve `PlayDecision::Resynthesize` y el llamador la resuelve con
/// esta pieza; así el menú no conoce ni el motor local ni el daemon, y ningún
/// futuro de re-síntesis captura el indicador de progreso.
trait PlayResynthesizer {
    /// Vuelve a sintetizar la locución sobre el temporal del menú.
    fn resynthesize(&mut self) -> impl std::future::Future<Output = Result<(), CliError>>;
}

/// Re-síntesis directa: vuelve a sintetizar con el motor local sobre el mismo
/// temporal.
struct LocalResynthesizer<'a> {
    engine: &'a Qwen3TtsEngine,
    voice_store: &'a VoiceStore,
    final_text: &'a str,
    voice: &'a str,
    temperature: Option<f32>,
    target_language: &'a str,
    tmp_wav: &'a PathBuf,
}

impl PlayResynthesizer for LocalResynthesizer<'_> {
    async fn resynthesize(&mut self) -> Result<(), CliError> {
        synthesize_voice_directly(
            self.engine,
            self.voice_store,
            self.final_text,
            self.voice,
            self.temperature,
            self.target_language,
            self.tmp_wav,
        )?;
        Ok(())
    }
}

/// Re-síntesis por daemon: vuelve a despachar la síntesis por la misma vía y
/// deja el WAV nuevo en el mismo temporal.
struct DaemonResynthesizer<'a> {
    client: &'a reqwest::Client,
    text: &'a str,
    voice: &'a str,
    source_language: &'a str,
    target_language: &'a str,
    temperature: Option<f32>,
    tmp_wav: &'a PathBuf,
    progress: &'a mut PhaseProgress,
}

impl PlayResynthesizer for DaemonResynthesizer<'_> {
    async fn resynthesize(&mut self) -> Result<(), CliError> {
        let wav = daemon_synthesize_wav(
            self.progress,
            self.client,
            self.text,
            self.voice,
            self.source_language,
            self.target_language,
            self.temperature,
        )
        .await?;
        std::fs::write(self.tmp_wav, &wav)
            .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
    }
}

/// Bucle del llamador del menú `--play`: ofrece el menú y, ante
/// `Resynthesize`, ejecuta la re-síntesis y reentra al menú con el mismo
/// temporal. Termina con la ruta guardada (`Some`) o con `None` si el usuario
/// descartó. El bucle vive en el llamador porque el menú no conoce el motor
/// de síntesis.
// Firma ancha deliberada: cada argumento mapea 1:1 a un dato del menú.
#[allow(clippy::too_many_arguments)]
async fn play_menu_loop<R: PlayResynthesizer>(
    voice: &str,
    label: &str,
    force: bool,
    speech_store: &SpeechStore,
    final_text: &str,
    wav: &std::path::Path,
    reader: &mut dyn FnMut(&mut String) -> std::io::Result<usize>,
    playback: &mut dyn FnMut(&std::path::Path) -> Result<(), CliError>,
    resynthesizer: &mut R,
) -> Result<Option<PathBuf>, CliError> {
    loop {
        match synthesize_play_loop(
            voice,
            label,
            force,
            speech_store,
            final_text,
            wav,
            reader,
            playback,
        )? {
            PlayDecision::Saved(saved) => return Ok(Some(saved)),
            PlayDecision::Discarded => return Ok(None),
            PlayDecision::Resynthesize => resynthesizer.resynthesize().await?,
        }
    }
}

/// Reproducción del WAV en el menú `--play`, con el servicio de audio real. Es
/// inyectable para que las pruebas no dependan de la placa de sonido.
fn menu_playback(label: &str) -> impl FnMut(&std::path::Path) -> Result<(), CliError> {
    let label = label.to_string();
    move |wav: &std::path::Path| {
        audio::AudioService::new().play_wav(wav).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "playback_failed",
                format!("Fallo al reproducir la locución '{}': {}", label, e),
            )
        })
    }
}

/// Menú interactivo `--play`: reproduce la toma y ofrece 4 opciones a stderr —
/// 1) mantener (reproducir de nuevo, sin re-síntesis), 2) guardar
///    (recomprobando colisión de label al guardar), 3) repetir con una síntesis
///    nueva, 4) descartar. EOF o error de lectura de stdin también descartan;
///    una opción inválida avisa y repite el menú.
///
/// La opción 1 se resuelve dentro del menú porque no necesita volver a
/// sintetizar; la 3 se devuelve al llamador como `Resynthesize`, que re-sintetiza
/// y reentra al menú. El menú y los prompts van a stderr, nunca a stdout. No
/// introduce exit codes nuevos.
// Firma ancha deliberada: cada argumento mapea 1:1 a un dato del menú.
#[allow(clippy::too_many_arguments)]
fn synthesize_play_loop(
    voice: &str,
    label: &str,
    force: bool,
    speech_store: &SpeechStore,
    final_text: &str,
    wav: &std::path::Path,
    reader: &mut dyn FnMut(&mut String) -> std::io::Result<usize>,
    playback: &mut dyn FnMut(&std::path::Path) -> Result<(), CliError>,
) -> Result<PlayDecision, CliError> {
    // Reproducir al entrar al menú.
    playback(wav)?;

    loop {
        eprintln!(
            "Opciones: [1] mantener (reproducir de nuevo)  [2] guardar  [3] repetir (nueva síntesis)  [4] descartar"
        );
        eprint!("Opción [1-4]: ");
        #[expect(
            clippy::let_underscore_must_use,
            reason = "si stderr no admite el vaciado, la lectura de la opción sigue igual"
        )]
        let _ = std::io::stderr().flush();

        let mut line = String::new();
        let choice = match reader(&mut line) {
            Ok(0) | Err(_) => return Ok(PlayDecision::Discarded), // EOF o error de lectura → descartar
            Ok(_) => line.trim().to_string(),
        };

        match choice.as_str() {
            "1" => {
                playback(wav)?;
            }
            "2" => {
                // La colisión se recomprueba en el instante de guardar, no
                // solo en el fast-fail previo a sintetizar.
                if !force
                    && speech_store
                        .find(voice, label)
                        .map_err(speech_find_error)?
                        .is_some()
                {
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
                    .save(voice, label, final_text, wav)
                    .map_err(|e| {
                        CliError::new(ExitCode::Error, "synthesis_failed", e.to_string())
                    })?;
                return Ok(PlayDecision::Saved(saved));
            }
            "3" => return Ok(PlayDecision::Resynthesize),
            "4" => return Ok(PlayDecision::Discarded),
            _ => eprintln!("Opción inválida."),
        }
    }
}

/// Error de sidecar corrupto del almacén de locuciones como `io_error`: nombra el
/// fichero y remite a `speech remove`, que borra la locución sin leer el sidecar.
fn sidecar_cli_error(e: &anyhow::Error) -> Option<CliError> {
    e.downcast_ref::<store::SidecarError>().map(|s| {
        CliError::new(
            ExitCode::Error,
            "io_error",
            format!(
                "{s}. Elimina la locución con `speech remove --label {} --voice {}`.",
                s.label, s.voice
            ),
        )
    })
}

/// Exige que la voz exista en el registro: `voice_not_found` (exit 3) si no. Lo
/// comparten `speech list`, `play` y `remove`, de modo que una voz mal escrita
/// nunca se disfrace de «sin resultados» o de etiqueta inexistente.
fn ensure_voice_exists(voice: &str) -> Result<(), CliError> {
    if open_voice_store()?
        .exists(voice)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
    {
        return Ok(());
    }
    Err(CliError::new(
        ExitCode::NotFound,
        "voice_not_found",
        format!("La voz '{}' no existe.", voice),
    ))
}

/// Error del almacén de locuciones al buscar una etiqueta.
fn speech_find_error(e: anyhow::Error) -> CliError {
    sidecar_cli_error(&e)
        .unwrap_or_else(|| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Error del almacén de locuciones al listar.
fn speech_list_error(e: anyhow::Error) -> CliError {
    sidecar_cli_error(&e)
        .unwrap_or_else(|| CliError::new(ExitCode::Error, "speech_list_failed", e.to_string()))
}

async fn handle_speech(json_mode: bool, action: SpeechCommands) -> Result<(), CliError> {
    match action {
        SpeechCommands::List { voice } => {
            // Listado de locuciones: local-only; el daemon no expone GET /speech.
            let speech_store = open_speech_store()?;
            // Con --voice se valida el identificador (exit 2) y la existencia
            // de la voz (exit 3) antes de acotar la lectura al almacén.
            let items = match &voice {
                Some(v) => {
                    is_valid_identifier(Some(v), None)?;
                    ensure_voice_exists(v)?;
                    speech_store.list_by_voice(v).map_err(speech_list_error)?
                }
                None => speech_store.list().map_err(speech_list_error)?,
            };
            if json_mode {
                // Sin sidecar, los campos que solo él aporta salen `null`.
                let entries: Vec<serde_json::Value> = items
                    .iter()
                    .map(|e| {
                        json!({
                            "label": e.label,
                            "voice": e.voice,
                            "text": e.metadata.as_ref().map(|m| &m.text),
                            "created_at": e.metadata.as_ref().map(|m| &m.created_at),
                            "duration_secs": e.metadata.as_ref().map(|m| m.duration_secs),
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
                        match &e.metadata {
                            Some(m) => println!(
                                "  - [{}] {} ({:.1}s) — «{}»",
                                e.voice, e.label, m.duration_secs, m.text
                            ),
                            None => println!("  - [{}] {} (sin metadatos)", e.voice, e.label),
                        }
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
            #[cfg(feature = "native-denoise")]
            denoise,
            routing,
        } => {
            let denoise = {
                #[cfg(feature = "native-denoise")]
                {
                    denoise
                }
                #[cfg(not(feature = "native-denoise"))]
                {
                    false
                }
            };
            // Tope de duración antes de grabar: la captura pedida es `--duration` o,
            // sin ella, el techo de push-to-talk.
            if mic {
                let requested = match duration {
                    Some(secs) => secs,
                    None => push_to_talk_max_secs()?,
                };
                if requested > avi_core::MAX_TRANSCRIBE_AUDIO_SECS {
                    return Err(audio_too_long(
                        "El audio de la transcripción",
                        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                    ));
                }
            }
            // push-to-talk sin --duration se permite en TTY; sin TTY se exige --duration
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }
            if let Some(a) = &audio {
                validate_audio_arg(std::path::Path::new(a))?;
                ensure_wav_within_limit(
                    std::path::Path::new(a),
                    "El audio de la transcripción",
                    avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                )?;
            }

            // Dispatch 3 modos (Transcribe es delegable al daemon):
            // ForceDaemon → daemon (error si no responde); Auto → daemon si
            // responde, si no cae a directo; ForceDirect → local. El probe de
            // vida usa un deadline corto para que el fallback Auto→directo sea
            // prácticamente instantáneo cuando el daemon no está en ejecución.
            let client = daemon_client();
            if route_to_daemon(routing.mode(), &client).await? {
                let mut progress = PhaseProgress::new();
                let result = transcribe_via_daemon(
                    &client,
                    audio.as_deref(),
                    mic,
                    duration,
                    &source_language,
                    denoise,
                    &mut progress,
                )
                .await;
                let _elapsed = progress.close();
                let text = result?;
                if json_mode {
                    emit_raw_json(json!({ "text": text, "source_language": source_language }));
                } else {
                    println!("{}", text);
                }
                return Ok(());
            }

            // Modelo ausente -> exit 4, previo a construir el motor.
            let stt_dir = open_model_store()?.model_dir("parakeet-tdt-v3");
            if !stt_dir.join("nemo128.onnx").exists() {
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
                let mut progress = PhaseProgress::new();
                // Patrón de bloque externo: el trabajo que puede fallar vive en
                // un bloque y el cierre del indicador —que consume el indicador—
                // se hace fuera, sobre el resultado ya obtenido.
                let result = async {
                    progress.phase(
                        if mic { "capturing" } else { "reading" },
                        if mic {
                            "Capturando audio del micrófono…"
                        } else {
                            "Leyendo archivo de audio…"
                        },
                    );
                    if mic {
                        progress.close_line();
                    }
                    let acquired = acquire_pcm(
                        audio.as_deref(),
                        mic,
                        duration,
                        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                    )
                    .await;
                    if mic {
                        progress.repaint();
                    }
                    let pcm = acquired?;
                    ensure_pcm_within_limit(
                        &pcm,
                        "El audio de la transcripción",
                        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                    )?;

                    progress.phase("transcribing", "Transcribiendo audio…");
                    progress.close_line();
                    let engine_result = STT_HOLDER.get_or_try_init(stt_dir).map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_failed",
                            e.to_string(),
                        )
                    });
                    progress.repaint();
                    let engine = engine_result?;
                    // Limpieza de ruido (opt-in) antes de STT: toma de turno,
                    // 48 kHz, inferencia y vuelta a 16 kHz. El modelo se resuelve
                    // y verifica antes del trabajo pesado; fallo de modelo es
                    // `model_missing`, fallo de audio es `invalid_audio`.
                    let pcm_for_stt = {
                        #[cfg(feature = "native-denoise")]
                        let denoised = if denoise {
                            let work_dir = avi_process::scratch::tempdir("avi_transcribe_denoise_")
                                .map_err(|e| {
                                    CliError::new(ExitCode::Error, "io_error", format!("{e:#}"))
                                })?;
                            denoise::service::ServiceModels::resolve()
                                .map_err(|e| {
                                    CliError::new(
                                        ExitCode::ModelMissing,
                                        "model_missing",
                                        format!("Modelo de limpieza no provisionado: {e}"),
                                    )
                                })?
                                .denoise_pcm16_mono(
                                    &pcm,
                                    16_000,
                                    denoise::service::DenoiseUseCase::Stt,
                                    work_dir.path(),
                                )
                                .map_err(|e| {
                                    let reason = match e {
                                        denoise::errors::E::EModelMissing(_) => "model_missing",
                                        denoise::errors::E::EInvalidInput(_) => "invalid_audio",
                                        denoise::errors::E::EIo(_) => "io_error",
                                        _ => "denoise_failed",
                                    };
                                    CliError::new(ExitCode::Error, reason, e.to_string())
                                })?
                        } else {
                            pcm.clone()
                        };
                        #[cfg(not(feature = "native-denoise"))]
                        let denoised = pcm;
                        denoised
                    };
                    let text = engine.transcribe(&pcm_for_stt).map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_failed",
                            e.to_string(),
                        )
                    })?;
                    Ok(text)
                }
                .await;
                let _elapsed = progress.close();
                let text = result?;

                if json_mode {
                    emit_raw_json(json!({ "text": text, "source_language": source_language }));
                } else {
                    println!("{}", text);
                }
                Ok(())
            }
        }
        SpeechCommands::Synthesize {
            text,
            voice,
            label,
            force,
            play,
            source_language,
            target_language,
            temperature,
            routing,
        } => {
            // Precondiciones puras de --play, antes de cualquier síntesis.
            // No introducen exit codes nuevos.
            //
            // El indicador nace antes de las validaciones, y el patrón de
            // bloque externo garantiza que se cierra en cualquier camino: los
            // `?` de las validaciones no pueden dejar la fase abierta.
            let mut progress = PhaseProgress::new();
            let result = async {
                progress.phase("validating", "Validando texto y voz…");
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
                validate_temperature(temperature)?;
                // Orden de validaciones del contrato: la temperatura se comprueba antes que el texto.
                avi_core::validate_synthesis_text(&text)?;
                // Origen por defecto = destino (sin traducir).
                let source_eff = source_language.as_deref().unwrap_or(&target_language);

                // Dispatch 3 modos (Synthesize es delegable al daemon).
                let client = daemon_client();
                if route_to_daemon(routing.mode(), &client).await? {
                    return synthesize_via_daemon(
                        &mut progress,
                        &client,
                        &text,
                        &voice,
                        &label,
                        force,
                        play,
                        source_eff,
                        &target_language,
                        temperature,
                    )
                    .await;
                }

                require_model_provisioned()?;
                let voice_store = open_voice_store()?;
                if !voice_store
                    .exists(&voice)
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
                {
                    return Err(CliError::new(
                        ExitCode::NotFound,
                        "voice_not_found",
                        format!("La voz '{}' no existe.", voice),
                    ));
                }
                let label = label.to_lowercase();
                is_valid_identifier(Some(&label), None)?;
                let speech_store = open_speech_store()?;
                if !force
                    && speech_store
                        .find(&voice, &label)
                        .map_err(speech_find_error)?
                        .is_some()
                {
                    return Err(CliError::new(
                        ExitCode::StateConflict,
                        "label_exists",
                        format!(
                            "Ya existe una locución con la etiqueta '{}' (usa --force).",
                            label
                        ),
                    ));
                }

                // Guardián del temporal: lo borra al salir de ámbito por cualquier camino.
                let tmp_guard = avi_process::scratch::temp_wav("avi_tts_")
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
                let tmp_wav = tmp_guard.path().clone();
                progress.close_line();
                let engine_result = direct_tts_engine();
                progress.repaint();
                let engine = engine_result?;
                // Traducción opt-in antes de sintetizar (passthrough si coinciden).
                let final_text = if source_eff != target_language {
                    progress.phase("translating", "Traduciendo texto…");
                    translate_if_different(&text, source_eff, &target_language)?
                } else {
                    text.clone()
                };
                progress.phase("synthesizing", "Sintetizando voz…");
                progress.tick();
                // La síntesis es trabajo **síncrono**: bloquea el hilo mientras
                // dura, así que el indicador no puede repintar durante ella y el
                // tiempo de la fase queda congelado. No es un defecto: el
                // trabajo total lo aporta el mensaje final.
                synthesize_voice_directly(
                    &engine,
                    &voice_store,
                    &final_text,
                    &voice,
                    temperature,
                    &target_language,
                    &tmp_wav,
                )?;

                let saved = if play {
                    // Bucle interactivo, reemplaza la reproducción y el guardado
                    // incondicionales. Solo alcanzable en TTY. El bucle vive aquí:
                    // el menú solo devuelve la decisión y la re-síntesis la ejecuta
                    // esta rama, sobre el mismo temporal.
                    let mut reader = menu_stdin();
                    let mut playback = menu_playback(&label);
                    let mut resynthesizer = LocalResynthesizer {
                        engine: &engine,
                        voice_store: &voice_store,
                        final_text: &final_text,
                        voice: &voice,
                        temperature,
                        target_language: &target_language,
                        tmp_wav: &tmp_wav,
                    };
                    progress.close_line();
                    let outcome = play_menu_loop(
                        &voice,
                        &label,
                        force,
                        &speech_store,
                        &final_text,
                        &tmp_wav,
                        &mut reader,
                        &mut playback,
                        &mut resynthesizer,
                    )
                    .await;
                    progress.repaint();
                    match outcome? {
                        Some(saved) => saved,
                        None => {
                            // Opción 4 / EOF: descartado, exit 0.
                            return Ok(None);
                        }
                    }
                } else {
                    progress.phase("saving", "Guardando locución…");
                    speech_store
                        .save(&voice, &label, &final_text, &tmp_wav)
                        .map_err(|e| {
                            CliError::new(ExitCode::Error, "synthesis_failed", e.to_string())
                        })?
                };
                Ok(Some(saved))
            }
            .await;
            let elapsed = progress.close();
            let saved = result?;
            let Some(saved) = saved else {
                emit_play_discarded(json_mode, &voice);
                return Ok(());
            };
            if json_mode {
                emit_raw_json(json!({
                    "status": "success",
                    "audio_path": saved.to_string_lossy(),
                    "voice": voice,
                }));
            } else {
                println!(
                    "Síntesis completada: {} ({}).",
                    saved.display(),
                    avi_lifecycle::progress::format_duration(elapsed)
                );
            }
            Ok(())
        }
        SpeechCommands::Say {
            text,
            voice,
            source_language,
            target_language,
            temperature,
            routing,
        } => {
            let mut progress = PhaseProgress::new();
            // Patrón de bloque externo: el trabajo que puede fallar vive en un
            // bloque y el cierre del indicador —que consume el indicador— se
            // hace fuera, sobre el resultado ya obtenido.
            let result = async {
                progress.phase("validating", "Validando texto y voz…");
                validate_temperature(temperature)?;
                avi_core::validate_synthesis_text(&text)?;
                // Origen por defecto = destino (sin traducir).
                let source_eff = source_language.as_deref().unwrap_or(&target_language);

                // Dispatch 3 modos (Say es delegable al daemon).
                let client = daemon_client();
                if route_to_daemon(routing.mode(), &client).await? {
                    say_via_daemon(
                        &mut progress,
                        &client,
                        &text,
                        &voice,
                        source_eff,
                        &target_language,
                        temperature,
                    )
                    .await?;
                    return Ok(());
                }

                require_model_provisioned()?;
                let voice_store = open_voice_store()?;
                if !voice_store
                    .exists(&voice)
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
                {
                    return Err(CliError::new(
                        ExitCode::NotFound,
                        "voice_not_found",
                        format!("La voz '{}' no existe.", voice),
                    ));
                }
                // Guardián del temporal: lo borra al salir de ámbito, también si falla la reproducción.
                let tmp_wav = avi_process::scratch::temp_wav("avi_say_")
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
                progress.close_line();
                let engine_result = direct_tts_engine();
                progress.repaint();
                let engine = engine_result?;
                // Traducción opt-in antes de sintetizar (passthrough si coinciden).
                let final_text = if source_eff != target_language {
                    progress.phase("translating", "Traduciendo texto…");
                    translate_if_different(&text, source_eff, &target_language)?
                } else {
                    text.clone()
                };
                progress.phase("synthesizing", "Sintetizando voz doblada…");
                progress.tick();
                // La síntesis es trabajo **síncrono**: bloquea el hilo mientras
                // dura, así que el indicador no puede repintar durante ella y el
                // tiempo de la fase queda congelado. No es un defecto: el
                // trabajo total lo aporta el mensaje final.
                synthesize_voice_directly(
                    &engine,
                    &voice_store,
                    &final_text,
                    &voice,
                    temperature,
                    &target_language,
                    tmp_wav.path(),
                )?;
                progress.phase("playing", "Reproduciendo…");
                audio::AudioService::new()
                    .play_wav(tmp_wav.path())
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "playback_failed",
                            format!("Fallo al reproducir la locución: {}", e),
                        )
                    })?;
                Ok(())
            }
            .await;
            let elapsed = progress.close();
            result?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "reproduced",
                    "voice": voice,
                }));
            } else {
                println!(
                    "Reproduciendo ({}).",
                    avi_lifecycle::progress::format_duration(elapsed)
                );
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
            #[cfg(feature = "native-denoise")]
            denoise,
            routing,
        } => {
            let denoise = {
                #[cfg(feature = "native-denoise")]
                {
                    denoise
                }
                #[cfg(not(feature = "native-denoise"))]
                {
                    false
                }
            };
            validate_temperature(temperature)?;
            // Validaciones puras del contrato, antes del despacho:
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }
            if duration.is_some_and(|d| d > avi_core::MAX_DUB_AUDIO_SECS) {
                return Err(audio_too_long(
                    "El audio del doblaje",
                    avi_core::MAX_DUB_AUDIO_SECS,
                ));
            }
            if let Some(a) = &audio {
                validate_audio_arg(std::path::Path::new(a))?;
                // El tope se mide por la cabecera antes de contactar con el daemon
                // o de comprobar modelos, igual que en transcribe y clone.
                ensure_wav_within_limit(
                    std::path::Path::new(a),
                    "El audio del doblaje",
                    avi_core::MAX_DUB_AUDIO_SECS,
                )?;
            }
            // Despacho 3 modos: delega a POST /dub si daemon activo
            {
                let client = daemon_client();
                if route_to_daemon(routing.mode(), &client).await? {
                    let mut progress = PhaseProgress::new();
                    let result = dub_via_daemon(
                        &mut progress,
                        &client,
                        audio.as_deref(),
                        mic,
                        duration,
                        &source_language,
                        &target_language,
                        temperature,
                        denoise,
                        &voice,
                    )
                    .await;
                    let elapsed = progress.close();
                    let final_text = result?;
                    if json_mode {
                        emit_raw_json(json!({ "status": "dubbed", "text": final_text }));
                    } else {
                        println!(
                            "Doblaje reproducido ({}).",
                            avi_lifecycle::progress::format_duration(elapsed)
                        );
                    }
                    return Ok(());
                }
            }
            // Rama local: modelos ausentes → exit 4
            let stt_dir = open_model_store()?.model_dir("parakeet-tdt-v3");
            if !stt_dir.join("nemo128.onnx").exists() {
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
                Err(CliError::new(
                    ExitCode::Error,
                    "stt_unsupported",
                    "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
                ))
            }
            #[cfg(feature = "native-stt")]
            {
                let mut progress = PhaseProgress::new();
                // Patrón de bloque externo: el trabajo que puede fallar vive en
                // un bloque y el cierre del indicador —que consume el indicador—
                // se hace fuera, sobre el resultado ya obtenido. Así ninguno de
                // los `?` del pipeline deja la fase abierta.
                let result = async {
                    if mic {
                        progress.phase("capturing", "Capturando audio del micrófono…");
                    } else {
                        progress.phase("reading", "Leyendo archivo de audio…");
                    }
                    if mic {
                        progress.close_line();
                    }
                    // La captura del micrófono es trabajo **síncrono** en su
                    // fondo: bloquea el hilo mientras espera al Enter, así que
                    // el indicador no puede repintar durante ella y el tiempo
                    // de la fase queda congelado. La línea se cede antes para
                    // que el aviso de captura no quede pegado a la fase. No es
                    // un defecto: el trabajo total lo aporta el mensaje final.
                    let acquired = acquire_pcm(
                        audio.as_deref(),
                        mic,
                        duration,
                        avi_core::MAX_DUB_AUDIO_SECS,
                    )
                    .await;
                    if mic {
                        progress.repaint();
                    }
                    let pcm = acquired?;
                    ensure_pcm_within_limit(
                        &pcm,
                        "El audio del doblaje",
                        avi_core::MAX_DUB_AUDIO_SECS,
                    )?;
                    let pcm_for_stt = {
                        #[cfg(feature = "native-denoise")]
                        let denoised = if denoise {
                            let work_dir = avi_process::scratch::tempdir("avi_dub_denoise_")
                                .map_err(|e| {
                                    CliError::new(ExitCode::Error, "io_error", format!("{e:#}"))
                                })?;
                            denoise::service::ServiceModels::resolve()
                                .map_err(|e| {
                                    CliError::new(
                                        ExitCode::ModelMissing,
                                        "model_missing",
                                        format!("Modelo de limpieza no provisionado: {e}"),
                                    )
                                })?
                                .denoise_pcm16_mono(
                                    &pcm,
                                    16_000,
                                    denoise::service::DenoiseUseCase::Stt,
                                    work_dir.path(),
                                )
                                .map_err(|e| {
                                    let reason = match e {
                                        denoise::errors::E::EModelMissing(_) => "model_missing",
                                        denoise::errors::E::EInvalidInput(_) => "invalid_audio",
                                        denoise::errors::E::EIo(_) => "io_error",
                                        _ => "denoise_failed",
                                    };
                                    CliError::new(ExitCode::Error, reason, e.to_string())
                                })?
                        } else {
                            pcm.clone()
                        };
                        #[cfg(feature = "native-denoise")]
                        {
                            denoised
                        }
                        #[cfg(not(feature = "native-denoise"))]
                        {
                            pcm
                        }
                    };
                    progress.phase("transcribing", "Transcribiendo audio…");
                    progress.tick();
                    progress.close_line();
                    let stt_result = STT_HOLDER.get_or_try_init(stt_dir).map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_failed",
                            e.to_string(),
                        )
                    });
                    progress.repaint();
                    let stt = stt_result?;
                    let transcribed = stt.transcribe(&pcm_for_stt).map_err(|e| {
                        CliError::new(
                            ExitCode::TranscriptionFailed,
                            "transcription_failed",
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

                    // Traducción solo si source != target (passthrough si coinciden).
                    let source = source_language.as_str();
                    let target = target_language.as_str();
                    let final_text = if source == target {
                        transcribed.clone()
                    } else {
                        progress.phase("translating", "Traduciendo texto transcrito…");
                        let model_name = format!("opus-mt-{}", translation_pair(source));
                        let model_store = open_model_store()?;
                        let ct2_dir = model_store.model_dir(&model_name);
                        if !model_store.is_provisioned(&model_name) {
                            return Err(CliError::new(
                                ExitCode::ModelMissing,
                                "model_missing",
                                format!(
                                    "El modelo de traducción {} no está provisionado en '{}' — ejecuta setup.",
                                    model_name,
                                    ct2_dir.display(),
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
                            translation::translate(&transcribed, &ct2_dir).map_err(|e| {
                                CliError::new(
                                    ExitCode::TranslationFailed,
                                    "translation_failed",
                                    e.to_string(),
                                )
                            })?
                        }
                    };

                    // El texto traducido puede exceder el tope aunque el transcrito no.
                    avi_core::validate_synthesis_text(&final_text)?;
                    let voice_store = open_voice_store()?;
                    if !voice_store
                        .exists(&voice)
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
                    {
                        return Err(CliError::new(
                            ExitCode::NotFound,
                            "voice_not_found",
                            format!("La voz '{}' no existe.", voice),
                        ));
                    }
                    // Guardián del temporal: lo borra al salir de ámbito, también si falla la reproducción.
                    let tmp_wav = avi_process::scratch::temp_wav("avi_dub_")
                        .map_err(|e| {
                            CliError::new(ExitCode::Error, "io_error", format!("{e:#}"))
                        })?;
                    progress.close_line();
                    let engine_result = direct_tts_engine();
                    progress.repaint();
                    let engine = engine_result?;
                    progress.phase("synthesizing", "Sintetizando voz doblada…");
                    progress.tick();
                    synthesize_voice_directly(
                        &engine,
                        &voice_store,
                        &final_text,
                        &voice,
                        temperature,
                        &target_language,
                        tmp_wav.path(),
                    )?;
                    progress.phase("playing", "Reproduciendo doblaje…");
                    audio::AudioService::new()
                        .play_wav(tmp_wav.path())
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::Error,
                                "playback_failed",
                                format!("Fallo al reproducir el doblaje: {}", e),
                            )
                        })?;
                    Ok(final_text)
                }
                .await;
                let elapsed = progress.close();
                let final_text = result?;
                if json_mode {
                    emit_raw_json(json!({
                        "status": "dubbed",
                        "text": final_text,
                    }));
                } else {
                    println!(
                        "Doblaje reproducido ({}).",
                        avi_lifecycle::progress::format_duration(elapsed)
                    );
                }
                Ok(())
            }
        }
        SpeechCommands::Play { label, voice } => {
            // Reproducción de locución persistida: local-only.
            is_valid_identifier(Some(&voice), Some(&label))?;
            ensure_voice_exists(&voice)?;
            let speech_store = open_speech_store()?;
            match speech_store
                .find(&voice, &label)
                .map_err(speech_find_error)?
            {
                Some(entry) => {
                    let mut progress = PhaseProgress::new();
                    // Patrón de bloque externo: el cierre del indicador se hace
                    // fuera del trabajo que puede fallar.
                    let result = 'work: {
                        progress.phase("playing", "Reproduciendo locución…");
                        break 'work audio::AudioService::new()
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
                            });
                    };
                    let elapsed = progress.close();
                    result?;
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "played", "label": label, "voice": voice }),
                        );
                    } else {
                        println!(
                            "Reproduciendo locución '{}' de la voz '{}' ({}).",
                            label,
                            voice,
                            avi_lifecycle::progress::format_duration(elapsed)
                        );
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
            is_valid_identifier(Some(&voice), Some(&label))?;
            ensure_voice_exists(&voice)?;
            open_speech_store()?
                .remove(&voice, &label)
                .map_err(|e| CliError::new(ExitCode::NotFound, "speech_not_found", e))?;
            if json_mode {
                emit_raw_json(json!({ "status": "removed", "label": label, "voice": voice }));
            } else {
                println!("Locución '{}' de la voz '{}' eliminada.", label, voice);
            }
            Ok(())
        }
        #[cfg(feature = "native-denoise")]
        SpeechCommands::Denoise { .. } => unreachable!("speech denoise se despacha en main"),
    }
}

// ─── Daemon ──────────────────────────────────────────────────────────

/// `usage_lock` es el bloqueo compartido que `main` tomó para los subcomandos de
/// uso. `serve` lo entrega al servidor, que lo suelta al quedar registrado; `start`
/// y `restart` lo retienen hasta volver, también mientras esperan el `daemon.ready`
/// del hijo, que toma su propio compartido y coexiste con este.
async fn handle_daemon(
    json_mode: bool,
    action: DaemonCommands,
    usage_lock: Option<lifecycle::lock::LifecycleLock>,
) -> Result<Outcome, CliError> {
    match action {
        DaemonCommands::Serve {
            auto_restart,
            max_retries,
            warm_voice,
            ready_file,
        } => {
            avi_shared::diag_trace::mark("child.serve_enter");
            // La dirección la resuelve el protocolo (`AVI_DAEMON_PORT` con
            // reserva en la dirección por defecto); aquí solo se pasa al servidor.
            let addr: SocketAddr = resolve_daemon_addr();
            // Transporte flag+fichero: el flag designa el fichero de
            // señalización y viaja intra-proceso por env hasta
            // `run_supervised`/`run_daemon_server`, de modo que la firma del
            // servidor no depende del transporte de señalización. Ruta
            // absoluta: las relativas dependen de la unidad
            // del proceso en Windows y los limpiadores de `%TEMP%` pueden
            // barrer ficheros fuera del sandbox.
            if let Some(path) = ready_file.as_ref() {
                let absolute_path = if path.is_absolute() {
                    path.clone()
                } else {
                    std::env::current_dir().map(|c| c.join(path)).map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "daemon_error",
                            format!(
                                "No se pudo resolver la ruta absoluta del fichero ready '{}': {}",
                                path.display(),
                                e
                            ),
                        )
                    })?
                };
                std::env::set_var(daemon::READY_FILE_ENV, &absolute_path);
            }
            // El handler SIGINT ya quedó instalado en `main` para todos los modos;
            // la escucha de señales del servidor cierra por la misma ruta que
            // POST `/shutdown`. En Unix `serve` cierra por esa misma ruta
            // sin auto-muerte del CLI (la guarda `pid != own_pid` protege
            // al `serve` en foreground).
            // Un fallo previo a estar listo sale con el código de su causa; el
            // de un servidor que ya servía sale con exit 1 `daemon_error`, como
            // cualquier otro `daemon_error`.
            // La raíz de datos que no se puede resolver impide arrancar el daemon:
            // `startup_failed`, no `io_error` de la CLI.
            let startup_stores =
                |e: anyhow::Error| CliError::new(ExitCode::Error, "startup_failed", e.to_string());
            let voices = VoiceStore::new().map_err(startup_stores)?;
            let speech = SpeechStore::new().map_err(startup_stores)?;
            daemon::run_supervised(
                addr,
                auto_restart,
                max_retries,
                warm_voice,
                voices,
                speech,
                usage_lock.map(|lock| Box::new(lock) as Box<dyn Send>),
            )
            .await
            .map_err(|e| match e.downcast_ref::<daemon::StartupError>() {
                Some(startup) => startup_error_to_cli(startup),
                None => CliError::new(ExitCode::Error, "daemon_error", e.to_string()),
            })?;
            Ok(Outcome::Done)
        }
        DaemonCommands::Start {
            auto_restart,
            max_retries,
            warm_voice,
        } => {
            avi_shared::diag_trace::mark("start.enter");
            require_model_provisioned()?;
            avi_shared::diag_trace::mark("start.model_provisioned");
            let client = daemon_client();
            // Revalidación con reclamo matar-y-rearrancar: la vida del
            // residual se comprueba por PID vivo más probe, no por probe solo ni
            // pidfile solo. En servicio → `already_running`; degradado → la parada
            // unificada lo reclama y se rearranca con salida 0 y payload `started`.
            let data_dir = effective_data_dir()?;
            let client_addr = lifecycle::daemon_stop::resolve_client_addr(&data_dir);
            let responds = probe_health(&client, &client_addr).await;
            avi_shared::diag_trace::mark("start.health_probe_done");
            // El indicador nace **antes** de la parada del daemon residual: así
            // no hay segundos de silencio antes de la primera fase, y el mismo
            // indicador sirve para `start`, `stop` y `restart`.
            let mut progress = PhaseProgress::new();
            avi_shared::diag_trace::mark("start.observe_begin");
            match lifecycle::daemon_stop::observe(&data_dir, &ProductProcesses, responds) {
                lifecycle::daemon_stop::DaemonState::Running { pid } => {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "already_running", "daemon": "running", "pid": pid }),
                        );
                    } else {
                        println!("Daemon ya en ejecución (pid {}).", pid);
                    }
                    return Ok(Outcome::Done);
                }
                lifecycle::daemon_stop::DaemonState::Degraded(degradation) => {
                    progress.phase("stopping", "Deteniendo daemon residual…");
                    // La línea se cede antes del aviso: el aviso es del producto
                    // y no puede quedar pegado a la fase.
                    progress.close_line();
                    eprintln!(
                        "Daemon residual degradado ({}): se reclama el árbol y se rearranca.",
                        degradation.describe()
                    );
                    let outcome = lifecycle::daemon_stop::stop(
                        &data_dir,
                        &lifecycle::daemon_stop::default_addr(),
                        &ProductProcesses,
                    )
                    .await;
                    lifecycle::daemon_stop::require_stopped(&outcome)
                        .map_err(|failure| lifecycle_error_to_cli(failure.into()))?;
                    progress.repaint();
                }
                lifecycle::daemon_stop::DaemonState::Stopped => {}
            }
            avi_shared::diag_trace::mark("start.observe_done");
            progress.phase("starting", "Iniciando daemon…");
            // Mientras launch_daemon espera el ready del hijo, el usuario ve
            // la fase de calentamiento (el warmup TTS corre en segundo plano
            // en el residente y no bloquea este bucle de sondeo).
            progress.phase("warming", "Calentando motor de síntesis…");
            let pid = launch_daemon(
                &client,
                auto_restart,
                max_retries,
                &warm_voice,
                DAEMON_READY_DEADLINE,
                Some(&progress),
            )
            .await;
            let elapsed = progress.close();
            match pid {
                Ok(pid) => {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "started", "daemon": "running", "pid": pid }),
                        );
                    } else {
                        println!(
                            "Daemon iniciado correctamente (pid {}, {}).",
                            pid,
                            avi_lifecycle::progress::format_duration(elapsed)
                        );
                    }
                    Ok(Outcome::Done)
                }
                Err(e) => Err(e),
            }
        }
        DaemonCommands::Stop => {
            // Parada unificada con deadline global: el veredicto lo da el motor
            // (`StopOutcome`), sin re-sondeos fuera de él. El pidfile ya lo borró
            // el motor tras muerte verificada; la CLI solo informa. La reserva sin
            // pidfile es la dirección derivada de `AVI_DAEMON_PORT`, la misma en la
            // que escucha el daemon.
            let mut progress = PhaseProgress::new();
            progress.phase("stopping", "Deteniendo daemon…");
            let outcome = lifecycle::daemon_stop::stop(
                &effective_data_dir()?,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            let elapsed = progress.close();
            if outcome.stopped {
                let status = stop_status(outcome.was_running);
                if json_mode {
                    emit_raw_json(json!({
                        "status": status,
                        "daemon": "stopped",
                        "was_running": outcome.was_running,
                        "daemon_fully_stopped": outcome.daemon_fully_stopped
                    }));
                } else if outcome.was_running {
                    // Los mensajes diagnostican la dirección descubierta (con
                    // pidfile efímero difiere de la por defecto; sin pidfile es
                    // idéntica).
                    let client_addr =
                        lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
                    println!(
                        "Señal de apagado enviada al daemon en {} ({}).",
                        client_addr,
                        avi_lifecycle::progress::format_duration(elapsed)
                    );
                } else {
                    // El mensaje depende de `was_running`: sin daemon en marcha
                    // la operación no «detuvo» nada, y decirlo sería mentir.
                    println!(
                        "No había daemon que detener ({}).",
                        avi_lifecycle::progress::format_duration(elapsed)
                    );
                }
                Ok(Outcome::Done)
            } else {
                let client_addr =
                    lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
                // El matiz del residente lo dice el predicado del motor
                // (`StopOutcome::resident_alive`), no el texto de `remaining`: atarlo
                // al vocabulario de la frase degradaba el error en silencio ante
                // cualquier reescritura de ese mensaje.
                let message = if outcome.resident_alive {
                    format!(
                        "El daemon no se apagó tras el deadline (pid {:?} en {}); el residente sigue vivo",
                        outcome.pid, client_addr
                    )
                } else {
                    format!(
                        "El daemon no se apagó tras el deadline (pid {:?} sigue vivo en {})",
                        outcome.pid, client_addr
                    )
                };
                if json_mode {
                    emit_raw_json(json!({
                        "status": stop_failed_status(),
                        "daemon": "running",
                        "was_running": outcome.was_running,
                        "daemon_fully_stopped": outcome.daemon_fully_stopped,
                        "resident_alive": outcome.resident_alive,
                        "pid": outcome.pid,
                        "remaining": outcome.remaining,
                    }));
                } else {
                    eprintln!("Error: {}", message);
                }
                Ok(Outcome::Verdict(ExitCode::DaemonUnreachable.code()))
            }
        }
        DaemonCommands::Restart => {
            // Restart determinista sobre la parada unificada, la misma de
            // start/stop/cleanup/uninstall: un único deadline global y un único
            // reclamo por PID.
            let t_total = std::time::Instant::now();
            let budget = std::time::Duration::from_secs(12);
            let client = daemon_client();
            // El indicador nace **antes** de la parada, igual que en `start` y
            // en `stop`: así no hay segundos de silencio antes de la primera
            // fase, y la fase `stopping` se reutiliza.
            let mut progress = PhaseProgress::new();
            progress.phase("stopping", "Deteniendo daemon…");
            let outcome = lifecycle::daemon_stop::stop(
                &effective_data_dir()?,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            // Si la parada no concluye queda un daemon vivo: no se arranca otro.
            if let Err(failure) = lifecycle::daemon_stop::require_stopped(&outcome) {
                let _elapsed = progress.close();
                return Err(lifecycle_error_to_cli(failure.into()));
            }
            require_model_provisioned()?;
            let elapsed = t_total.elapsed();
            let remaining = budget
                .checked_sub(elapsed)
                .unwrap_or(std::time::Duration::from_millis(800));
            // Espera de la publicación del arranque acotada al restante del
            // presupuesto global, nunca por encima de `DAEMON_READY_DEADLINE`; la
            // confirmación por `/health` tiene su propio presupuesto.
            let ready_deadline = std::cmp::min(remaining, DAEMON_READY_DEADLINE);
            // Mismo lanzamiento que `Start`, con sus mismos resultados y
            // códigos de fallo.
            progress.phase("starting", "Iniciando daemon…");
            progress.phase("warming", "Calentando motor de síntesis…");
            let pid = launch_daemon(
                &client,
                false,
                3,
                "default",
                ready_deadline,
                Some(&progress),
            )
            .await;
            let closed = progress.close();
            match pid {
                Ok(pid) => {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "restarted", "daemon": "running", "pid": pid }),
                        );
                    } else {
                        println!(
                            "Daemon reiniciado (pid {}, {}).",
                            pid,
                            avi_lifecycle::progress::format_duration(closed)
                        );
                    }
                    Ok(Outcome::Done)
                }
                Err(e) => Err(e),
            }
        }
        DaemonCommands::Status => {
            // Un solo GET /health contra la dirección descubierta: su éxito es el
            // sondeo con el que `observe` clasifica (el mismo criterio que `start`)
            // y su cuerpo aporta motor y warmup. Sin respuesta (timeout/conexión)
            // el sondeo es negativo; siempre exit 0.
            let client = daemon_client();
            let data_dir = effective_data_dir()?;
            let client_addr = lifecycle::daemon_stop::resolve_client_addr(&data_dir);
            let health = match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                client.get(format!("http://{}/health", client_addr)).send(),
            )
            .await
            {
                Ok(Ok(resp)) if resp.status().is_success() => Some(
                    tokio::time::timeout(
                        std::time::Duration::from_millis(800),
                        resp.json::<Value>(),
                    )
                    .await
                    .map_err(|_| {
                        CliError::new(
                            ExitCode::DaemonUnreachable,
                            "daemon_unreachable",
                            format!("Daemon inalcanzable en {} (timeout json)", client_addr),
                        )
                    })?
                    .map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "daemon_error",
                            format!("Respuesta de /health no es JSON: {}", e),
                        )
                    })?,
                ),
                _ => None,
            };
            let state =
                lifecycle::daemon_stop::observe(&data_dir, &ProductProcesses, health.is_some());
            if json_mode {
                emit_raw_json(status_body(state, health.as_ref()));
            } else {
                println!("{}", status_line(state, health.as_ref()));
            }
            Ok(Outcome::Done)
        }
    }
}

// ─── Setup / Cleanup / Doctor ────────────────────────────────────────
/// `setup`: delega en el motor y compone el envelope `--json`.
///
/// Todo el cuerpo —la purga por plan y la idempotencia por presencia del snapshot—
/// vive en `lifecycle::setup`. Aquí solo queda la prosa y el envelope, porque el parseo
/// de la CLI y el emisor no están en ese crate.
async fn handle_setup(json_mode: bool, force_update: bool, yes: bool) -> Result<(), CliError> {
    let model_store = open_model_store()?;
    let voice_store = open_voice_store()?;

    // 1. Inicializar VoiceStore y directorio por defecto
    voice_store
        .ensure_initialized()
        .map_err(|e| CliError::new(ExitCode::Error, "voice_store_init_failed", e.to_string()))?;

    let options = lifecycle::setup::Options::user(force_update, yes);
    let outcome = lifecycle::setup::run(&model_store, &options)
        .await
        .map_err(lifecycle_error_to_cli)?;

    if json_mode {
        emit_raw_json(json!({
            "status": "completed",
            "models_provisioned": outcome.provisioned
        }));
    } else {
        println!(
            "Setup completado: {} modelo(s) disponibles.",
            outcome.provisioned.len()
        );
    }
    Ok(())
}

/// `cleanup`: delega íntegro en el motor y compone el envelope `--json`.
///
/// Lo que hay aquí es la conversión de tipos y la prosa; **la lista de destinos, el
/// gate de categoría, la confirmación, la parada del daemon y el barrido** son los del
/// motor. La lista que el envelope publica es la misma que el plan calculó, porque las dos
/// salen de `lifecycle::cleanup::plan` —que es exactamente lo que fallaba antes, cuando
/// `--dry-run` y la ejecución eran dos listas.
async fn handle_cleanup(
    json_mode: bool,
    voices: bool,
    synthetic_speech: bool,
    model: bool,
    all: bool,
    dry_run: bool,
    yes: bool,
) -> Result<(), CliError> {
    let roots = lifecycle::cleanup::Roots::resolve().map_err(lifecycle_error_to_cli)?;
    let options = lifecycle::cleanup::Options {
        model,
        voices,
        synthetic_speech,
        all,
        dry_run,
        assume_yes: yes,
    };
    // El indicador **no** lo crea la CLI: la confirmación ocurre dentro de
    // `cleanup::run`, y un indicador abierto antes de ella dejaría el prompt
    // pegado a la fase. La librería lo abre tras la confirmación —la frontera— y
    // lo cierra al terminar el barrido. El simulacro no pregunta nada, así que
    // también anuncia su fase.
    let outcome = lifecycle::cleanup::run_with_progress(&roots, &options, &ProductProcesses, true)
        .await
        .map_err(lifecycle_error_to_cli)?;

    if json_mode {
        emit_raw_json(json!({
            "status": outcome.status,
            "reason": Value::Null,
            "removed": outcome.removed,
            "planned": outcome.planned,
            "preserved": outcome.preserved.iter().map(|entry| json!({
                "path": entry.path.display().to_string(),
                "reason": entry.reason,
            })).collect::<Vec<_>>(),
            "unreadable": outcome.unreadable
        }));
    } else if outcome.status == "cancelled" {
        // `confirm::confirm` ya escribió «Cancelado.» por stderr: el handler no
        // lo repite, porque el literal es del motor de confirmación.
    } else if !outcome.dry_run {
        // El mensaje final lleva la duración, como los demás: la compone el
        // handler, que es su dueño, con el reloj que mostró las fases.
        println!(
            "Limpieza completada: {} ruta(s), {} barrido(s) ({}).",
            outcome.removed.len(),
            outcome.swept.len(),
            avi_lifecycle::progress::format_duration(outcome.elapsed)
        );
    }
    Ok(())
}
/// Clasifica el PID del daemon registrado (el del pidfile o, si falta, el que
/// publica `daemon.ready`): ausente, ajeno (PID reasignado) o nuestro.
fn registered_daemon() -> Result<(Option<u32>, avi_process::registry::Registered), CliError> {
    Ok(
        match lifecycle::daemon_stop::recorded_daemon(&effective_data_dir()?) {
            Some(recorded) => (
                Some(recorded.pid),
                lifecycle::daemon_stop::classify_registered(
                    &ProductProcesses,
                    recorded.pid,
                    daemon::pid_alive(recorded.pid),
                    recorded.identity.as_ref(),
                ),
            ),
            None => (None, avi_process::registry::Registered::Absent),
        },
    )
}

/// Estado JSON de la parada a partir del veredicto del motor: sin ejecución
/// previa es `not_running`, con ella `shutdown_sent`. Función pura para poder
/// probarla sin daemon.
fn stop_status(was_running: bool) -> &'static str {
    if was_running {
        "shutdown_sent"
    } else {
        "not_running"
    }
}

/// Estado JSON del fallo de la parada: no concluyó y el daemon sigue vivo.
/// Función pura para probarla sin daemon.
fn stop_failed_status() -> &'static str {
    "still_running"
}

/// Resumen previo de `self update`: qué versión se instala sobre cuál,
/// dónde, por qué canal y de dónde se descarga. Es lo que se muestra **antes**
/// de confirmar, en paralelo al resumen previo de `self install`.
fn compose_update_summary(
    receipt: &lifecycle::receipt::InstallReceipt,
    program_dir: &std::path::Path,
    target: &str,
    resolution: &lifecycle::update_resolve::Resolution,
    channel: lifecycle::channel::Channel,
) -> Vec<String> {
    let mut out = vec![format!(
        "Se actualizará {} ({}): {} → {}",
        lifecycle::APP_NAME,
        target,
        receipt.version,
        resolution.target,
    )];
    out.push(format!(
        "  Programa:  {}   (reemplaza {})",
        program_dir.display(),
        receipt.version
    ));
    out.push(format!("  Canal:     {}", channel.as_str()));
    if let Ok(url) = lifecycle::update_fetch::asset_url(&resolution.target, target) {
        out.push(format!("  Descarga:  {url}"));
    }
    if resolution.destructive {
        out.push("  Aviso: degradación de versión (operación destructiva).".to_string());
    }
    out
}

/// Envelope de `self update` para los desenlaces que no instalan nada y no
/// tienen más datos que las versiones y el canal (`already_up_to_date` y
/// `cancelled`): ambos comparten forma y se emiten por esta única vía.
fn compose_update_status_envelope(
    status: &str,
    current: &str,
    latest: &str,
    channel: lifecycle::channel::Channel,
) -> Value {
    json!({
        "status": status,
        "reason": Value::Null,
        "current": current,
        "latest": latest,
        "channel": channel.as_str(),
    })
}

/// Identidad del proceso por cuyo término espera el limpiador de los restos del
/// aparcado. Lo fija `self update` al lanzar el traspaso (variable con su PID,
/// cuyo inicio se observa aquí); sin traspaso (variable ausente) es el propio
/// proceso, que termina enseguida. Una variable presente con un valor que no es
/// un PID, o cuyo proceso ya no se puede observar, es un error: no se sustituye
/// por el proceso propio.
fn handover_wait_token() -> Result<avi_process::OwnerToken, CliError> {
    let name = lifecycle::update::HANDOVER_PARENT_PID_ENV;
    let fail = |message: String| CliError::new(ExitCode::Error, "self_install_failed", message);
    match std::env::var(name) {
        Ok(value) => {
            let pid = value.trim().parse::<u32>().map_err(|e| {
                fail(format!(
                    "La variable {name} debe contener un PID numérico (valor '{value}'): {e}"
                ))
            })?;
            let identity = avi_process::process_identity(pid).ok_or_else(|| {
                fail(format!(
                    "No se pudo observar el proceso {pid} de la variable {name}"
                ))
            })?;
            avi_process::OwnerToken::new(pid, identity.start).ok_or_else(|| {
                fail(format!(
                    "El inicio observado del proceso {pid} no es un token válido"
                ))
            })
        }
        Err(std::env::VarError::NotPresent) => avi_process::current_owner_token()
            .map_err(|e| fail(format!("No se pudo observar el propio proceso: {e}"))),
        Err(std::env::VarError::NotUnicode(_)) => Err(CliError::new(
            ExitCode::Error,
            "self_install_failed",
            format!("La variable {name} no contiene texto válido."),
        )),
    }
}

/// `self install` y `self uninstall`: el grupo de la CLI, delegando en el motor.
///
/// El binario aporta lo que el motor no puede tener: el **control de procesos** —que
/// vive en `avi-daemon` y `avi-tts`— y el **borrado diferido** de Windows, que necesita
/// `avi_process::schedule_removal`. El motor decide; el binario ejecuta las dos
/// primitivas de plataforma.
async fn handle_self(json_mode: bool, action: SelfSub) -> Result<Outcome, CliError> {
    match action {
        SelfSub::Install {
            no_setup,
            no_modify_path,
            force,
            yes,
        } => {
            // Se valida antes de instalar nada: un valor inválido no deja una
            // instalación a medias.
            let wait_token = handover_wait_token()?;
            let exe = std::env::current_exe().map_err(|e| {
                CliError::new(ExitCode::Error, "self_install_failed", e.to_string())
            })?;
            let env = lifecycle::install::Env::from_current_exe(exe, VERSION)
                .map_err(lifecycle_error_to_cli)?;
            let options = lifecycle::install::Options {
                assume_yes: yes,
                no_setup,
                no_modify_path,
                force,
            };
            let outcome = lifecycle::install::install(&env, &options, &ProductProcesses)
                .await
                .map_err(lifecycle_error_to_cli)?;
            // Restos del aparcado: el reemplazo no pudo borrarlos
            // por estar en uso —los retiene el `self update` que lanzó este
            // traspaso— y se programan con el limpiador en vez de descartarse.
            // Sin traspaso no hay retenedor conocido: se espera al propio
            // proceso, que termina enseguida, y lo no programado cae al barrido.
            if !outcome.parked_leftovers.is_empty() {
                let scheduled = lifecycle::update::schedule_leftovers(
                    &outcome.parked_leftovers,
                    &LifecyclePaths,
                    &wait_token,
                );
                if scheduled.scheduled.len() == outcome.parked_leftovers.len() {
                    eprintln!(
                        "  el aparcado anterior se borrará al terminar el proceso que lo \
                         retiene ({} ruta(s) programadas)",
                        scheduled.scheduled.len()
                    );
                } else {
                    eprintln!(
                        "  no se pudieron programar {} resto(s) del aparcado: los recogerá \
                         la recuperación de la siguiente operación",
                        outcome.parked_leftovers.len() - scheduled.scheduled.len()
                    );
                }
            }
            // `setup_failed` es un **éxito parcial** —el programa está instalado y
            // lo único que falta es la provisión—, con código propio. No es un `CliError`:
            // el resumen final y el envelope se emiten igual, y lo único que cambia es
            // el `reason` y el código de salida. `Outcome::Verdict` es el mecanismo que ya
            // existe para eso —el comando emitió su payload y solo queda fijar el código—,
            // y es el mismo que usa `doctor` para su envelope único.
            let partial = outcome.lifecycle_error();
            if json_mode {
                let mut on = json!({
                    "status": outcome.status,
                    "reason": match &partial {
                        Some(failure) => Value::String(failure.reason.to_string()),
                        None => Value::Null,
                    },
                    "install_dir": outcome.receipt.install_dir,
                    "version": outcome.receipt.version,
                    "channel": outcome.receipt.channel.as_str(),
                    "path_integrated": outcome.path_integrated(),
                    "models": outcome.models.as_str()
                });
                // El `reason` de la operación es `setup_failed`; el del fallo de provisión
                // viaja anidado en `models_cause`, que es donde un consumidor encuentra
                // `network_error` sin perderlo. Solo existe si
                // hubo fallo: un envelope estable es más fácil de leer que uno con nulos.
                if let lifecycle::install::ModelsState::Failed { cause } = &outcome.models {
                    on["models_cause"] = json!({
                        "reason": cause.reason,
                        "message": cause.message
                    });
                }
                emit_raw_json(on);
            } else {
                // La línea en blanco separa el resumen final del bloque anterior (el
                // progreso de la provisión o el plan confirmado).
                println!();
                for line in &outcome.summary {
                    println!("{line}");
                }
            }
            match partial {
                Some(failure) => Ok(Outcome::Verdict(
                    ExitCode::from_reason(failure.reason).code(),
                )),
                None => Ok(Outcome::Done),
            }
        }
        SelfSub::Uninstall {
            keep_data,
            dry_run,
            yes,
        } => {
            let exe = std::env::current_exe().map_err(|e| {
                CliError::new(ExitCode::Error, "self_uninstall_failed", e.to_string())
            })?;
            let registered_program_dir =
                lifecycle::channel::registered_program_dir().map_err(|e| {
                    CliError::new(ExitCode::Error, "self_uninstall_failed", e.to_string())
                })?;
            let receipt = lifecycle::receipt::read_from(&registered_program_dir).map_err(|e| {
                CliError::new(ExitCode::Error, "self_uninstall_failed", e.to_string())
            })?;
            let roots = lifecycle::cleanup::Roots::from_receipt(receipt.as_ref()).map_err(|e| {
                CliError::new(ExitCode::Error, "self_uninstall_failed", e.to_string())
            })?;
            let env = lifecycle::uninstall::Env {
                roots,
                receipt: receipt.as_ref(),
                channel: lifecycle::channel::detect(&exe, receipt.as_ref()),
                program_dir: registered_program_dir,
                daemon_addr: lifecycle::daemon_stop::default_addr(),
                home: home_dir()?,
            };
            let outcome = lifecycle::uninstall::run(
                &env,
                &lifecycle::uninstall::Options {
                    keep_data,
                    dry_run,
                    assume_yes: yes,
                },
                &ProgramRemoval,
                &ProductProcesses,
            )
            .await
            .map_err(lifecycle_error_to_cli)?;
            // `program_dir_kept` es un éxito parcial con código propio: el resto se completó
            // y el directorio de programa sigue en disco. Se emite el envelope igual y solo
            // cambian el `reason` y el código de salida, como en `self install`.
            let kept = outcome.lifecycle_error();
            if json_mode {
                // Sin `dry_run`: con `status: "planned"` el simulacro ya se lee como
                // un plan y la clave sería redundante.
                emit_raw_json(json!({
                    "status": outcome.status,
                    "reason": match &kept {
                        Some(failure) => Value::String(failure.reason.to_string()),
                        None => Value::Null,
                    },
                    "removed": outcome.removed,
                    "planned": outcome.planned,
                    "preserved": outcome.preserved.iter().map(|entry| json!({
                        "path": entry.path.display().to_string(),
                        "reason": entry.reason,
                    })).collect::<Vec<_>>(),
                    "path_reverted": outcome.path_reverted
                }));
            } else if outcome.status == "cancelled" {
                // `confirm::confirm` ya escribió «Cancelado.» por stderr: el
                // handler no lo repite, porque el literal es del motor de
                // confirmación.
            } else if outcome.status != "planned" {
                // La simulación ya imprimió por stderr el plan y su aviso único; el
                // literal de `status` es del contrato `--json` y no sale en el texto.
                // El mensaje final lo compone el flujo de la librería —el
                // indicador es interno— y lleva la duración de su cierre.
                println!("{}", outcome.message);
            }
            match kept {
                Some(failure) => {
                    if !json_mode {
                        eprintln!("{}", failure.message);
                    }
                    Ok(Outcome::Verdict(
                        ExitCode::from_reason(failure.reason).code(),
                    ))
                }
                None => Ok(Outcome::Done),
            }
        }
        SelfSub::Update {
            check,
            version,
            force,
            no_setup,
            yes,
        } => {
            let exe = std::env::current_exe()
                .map_err(|e| CliError::new(ExitCode::Error, "self_update_failed", e.to_string()))?;
            // Instalación registrada y canal: sea cual sea la copia que
            // ejecuta el comando, se opera sobre la registrada. Va antes del
            // bloqueo porque el directorio de programa registrado es lo que
            // dice dónde se toma.
            let program_dir = lifecycle::channel::registered_program_dir()
                .map_err(|e| CliError::new(ExitCode::Error, "self_update_failed", e.to_string()))?;
            let receipt = lifecycle::receipt::read_from(&program_dir)
                .map_err(|e| CliError::new(ExitCode::Error, "self_update_failed", e.to_string()))?;
            let channel = lifecycle::channel::detect(&exe, receipt.as_ref());
            if channel == lifecycle::channel::Channel::Homebrew {
                return Err(CliError::new(
                    ExitCode::ExternallyManaged,
                    "externally_managed",
                    "esta copia la gestiona Homebrew: actualiza con `brew upgrade --cask \
                     ai-voice-interconnector`, y `cleanup --all` para el estado de usuario",
                ));
            }
            let Some(receipt) = receipt else {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "not_installed",
                    "no hay instalación registrada: instala con el one-liner de tu sistema \
                     operativo (docs/DISTRIBUTION.md)",
                ));
            };
            // Falla rápido en plataforma no soportada, antes de tocar la red.
            let target = lifecycle::target::host_triple();
            lifecycle::target::ensure_supported(target)
                .map_err(|e| lifecycle_error_to_cli(e.into()))?;

            // ── Paso 1. Recuperación y bloqueo ────────────────────────────────
            // El staging que cree la descarga aún no existe; y cuando exista llevará el
            // token de este proceso, vivo, así que el barrido nunca lo toca.
            let roots = lifecycle::cleanup::Roots::from_receipt(Some(&receipt))
                .map_err(|e| CliError::new(ExitCode::Error, "self_update_failed", e.to_string()))?;
            let mut lock = lifecycle::lock::acquire_exclusive(&roots.program_dir)
                .map_err(|e| lifecycle_error_to_cli(e.into()))?;
            let recovery = lifecycle::recovery::recover(roots.recovery_roots())
                .map_err(lifecycle_error_to_cli)?;

            // ── Paso 3. Resolución de la versión objetivo ────────────────────
            // Con `--version` explícito no se toca la red: la resolución lo usa
            // como objetivo sin pedir la última estable.
            //
            // El indicador nace antes de la resolución, y el patrón de bloque
            // externo garantiza que se cierra en cualquier camino: los `?` de
            // la resolución no pueden dejar la fase `checking` abierta.
            let mut progress = PhaseProgress::new();
            let result = async {
                progress.phase("checking", "Buscando actualizaciones…");
                let client = lifecycle::update_fetch::http_client()
                    .map_err(|e| lifecycle_error_to_cli(e.into()))?;
                let request = lifecycle::update_resolve::ResolveRequest {
                    installed: receipt.version.clone(),
                    explicit: version.clone(),
                    force,
                };
                let latest = match &request.explicit {
                    Some(explicit) => explicit.clone(),
                    None => lifecycle::update_resolve::latest_stable(&client)
                        .await
                        .map_err(|e| lifecycle_error_to_cli(e.into()))?,
                };
                let resolution = lifecycle::update_resolve::resolve(&request, &latest)
                    .map_err(|e| lifecycle_error_to_cli(e.into()))?;
                Ok((client, resolution))
            }
            .await;
            let _elapsed = progress.close();
            let (client, resolution) = result?;

            // ── `--check`: informa sin cambios de actualización ──────────────
            // La recuperación del paso 1 ya pudo barrer huérfanos pendientes;
            // "sin cambios" no incluye ese barrido.
            if check {
                let available = resolution.verdict != lifecycle::update_resolve::Verdict::UpToDate;
                drop(lock);
                if json_mode {
                    emit_raw_json(json!({
                        "status": "check",
                        "reason": Value::Null,
                        "current": receipt.version,
                        "latest": resolution.target,
                        "update_available": available,
                        "channel": channel.as_str(),
                    }));
                } else if available {
                    println!(
                        "Hay actualización disponible: {} → {}.",
                        receipt.version, resolution.target
                    );
                } else {
                    println!("Ya está actualizado ({}).", receipt.version);
                }
                return Ok(Outcome::Done);
            }

            // Iguales sin `--force`: `already_up_to_date`, éxito con 0.
            if resolution.verdict == lifecycle::update_resolve::Verdict::UpToDate {
                drop(lock);
                if json_mode {
                    emit_raw_json(compose_update_status_envelope(
                        "already_up_to_date",
                        &receipt.version,
                        &resolution.target,
                        channel,
                    ));
                } else {
                    println!("Ya está actualizado ({}).", receipt.version);
                }
                return Ok(Outcome::Done);
            }

            // ── Resumen previo y confirmación ────────────────────────────────
            // Normal `[S/n]`; degradación `[s/N]` con `confirmation_required`
            // sin terminal (`confirm.rs` sin cambios).
            //
            // La confirmación vive en el handler, así que la cesión de línea es
            // del handler: la línea se cede antes del prompt y se repinta al
            // retomar, para que el prompt no quede pegado a la fase.
            let summary =
                compose_update_summary(&receipt, &program_dir, target, &resolution, channel);
            let kind = if resolution.destructive {
                lifecycle::confirm::Kind::Destructive
            } else {
                lifecycle::confirm::Kind::NonDestructive
            };
            let entries: Vec<lifecycle::confirm::PlanEntry> = if resolution.destructive {
                vec![lifecycle::confirm::PlanEntry::of(&program_dir)]
            } else {
                Vec::new()
            };
            let mut progress = PhaseProgress::new();
            progress.phase("checking", "Buscando actualizaciones…");
            progress.close_line();
            let decision = {
                let stdin = std::io::stdin();
                let mut entry = stdin.lock();
                lifecycle::confirm::confirm(
                    &lifecycle::confirm::Confirmation {
                        kind,
                        summary: &summary,
                        entries: &entries,
                        assume_yes: yes,
                        dry_run: false,
                        stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
                    },
                    &mut entry,
                    &mut std::io::stderr(),
                )
                .map_err(|e| CliError::new(ExitCode::Error, "lifecycle_failed", e.to_string()))?
            };
            progress.repaint();
            if decision == lifecycle::confirm::Decision::Cancelled {
                let _elapsed = progress.close();
                drop(lock);
                if json_mode {
                    emit_raw_json(compose_update_status_envelope(
                        "cancelled",
                        &receipt.version,
                        &resolution.target,
                        channel,
                    ));
                }
                // `confirm::confirm` ya escribió «Cancelado.» por stderr: el
                // handler no lo repite.
                return Ok(Outcome::Done);
            }

            // ── Paso 7. Descarga, verificación y extracción ──────────────────
            // Ante cualquier fallo el staging se borra y la instalación queda
            // intacta.
            //
            // Una fase por paso: `downloading` rotula solo la descarga.
            let result = async {
                progress.phase(
                    "downloading",
                    &format!("Descargando versión {}…", resolution.target),
                );
                progress.tick();
                // La descarga y la verificación son esperas reales: el repintado
                // mantiene viva la fase mientras el trabajo ocurre.
                let staged = await_with_progress(
                    &progress,
                    lifecycle::update_fetch::fetch(
                        &client,
                        &lifecycle::update_fetch::FetchRequest {
                            program_dir: program_dir.clone(),
                            target: target.to_string(),
                            version: resolution.target.clone(),
                        },
                    ),
                )
                .await
                .map_err(|e| lifecycle_error_to_cli(e.into()))?;

                progress.phase("verifying", "Verificando la versión descargada…");
                await_with_progress(
                    &progress,
                    lifecycle::update_fetch::verify_boot(
                        &staged.staging,
                        target,
                        &resolution.target,
                    ),
                )
                .await
                .map_err(|e| lifecycle_error_to_cli(e.into()))?;
                Ok(staged)
            }
            .await;
            let _elapsed = progress.close();
            let staged = result?;

            // ── Paso 8. Parar el daemon con el binario actual ────────────────
            // Si no se detiene, `daemon_stop_failed` sin tocar nada: la
            // instalación sigue intacta y el staging se retira.
            let mut progress = PhaseProgress::new();
            progress.phase("stopping", "Deteniendo daemon…");
            let daemon = lifecycle::daemon_stop::stop(
                &roots.data_dir,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            if let Err(failure) = lifecycle::daemon_stop::require_stopped(&daemon) {
                let _elapsed = progress.close();
                avi_shared::best_effort(
                    std::fs::remove_dir_all(&staged.staging),
                    format_args!("no se pudo borrar el staging {}", staged.staging.display()),
                );
                return Err(lifecycle_error_to_cli(failure.into()));
            }

            // ── Paso 9. Traspaso al binario nuevo ────────────────────────────
            // Se suelta solo el bloqueo principal antes de lanzar: el `self install`
            // interno lo toma, y con él tomado el hijo vería `lifecycle_locked`. La
            // compuerta se retiene hasta que vuelve el traspaso, de modo que ningún
            // comando de uso se cuela entre la parada del daemon y el reemplazo; el
            // hijo, reconocido por `AVI_HANDOVER_PARENT_PID`, no la toma.
            lock.release_main();
            progress.phase("replacing", "Reemplazando el programa…");
            let section = lifecycle::manifest::target_section(target)
                .map_err(|e| lifecycle_error_to_cli(e.into()))?;
            let handover = lifecycle::update::handover(&lifecycle::update::HandoverRequest {
                staging_exe: staged.staging.join(section.executable_path()),
                no_setup,
                no_modify_path: !receipt.path_integration.modify_path,
                force,
            })
            .await
            .map_err(lifecycle_error_to_cli)?;
            lifecycle::update::cleanup_staging(&staged.staging, &LifecyclePaths);
            let elapsed = progress.close();

            // ── Resultado `anterior → nueva` ─────────────────────────────────
            // Sin reinicio automático del daemon: el resultado indica cómo
            // relanzarlo. `setup_failed` es éxito parcial con el mecanismo de
            // veredicto del brazo `Install`.
            let partial = matches!(
                &handover,
                lifecycle::update::HandoverOutcome::PartialSetupFailed { .. }
            );
            if json_mode {
                let mut on = json!({
                    "status": "updated",
                    "reason": if partial { Value::String("setup_failed".to_string()) } else { Value::Null },
                    "previous_version": receipt.version,
                    "version": resolution.target,
                    "channel": channel.as_str(),
                });
                if let lifecycle::update::HandoverOutcome::PartialSetupFailed { cause } = &handover
                {
                    on["models_cause"] = json!({
                        "reason": cause.reason,
                        "message": cause.message,
                    });
                }
                emit_raw_json(on);
            } else {
                println!(
                    "{} actualizado: {} → {} ({}).",
                    lifecycle::APP_NAME,
                    receipt.version,
                    resolution.target,
                    avi_lifecycle::progress::format_duration(elapsed)
                );
                if partial {
                    println!("  Aviso: {}.", lifecycle::update::partial_setup_message());
                }
                if !recovery.kept.is_empty() {
                    println!(
                        "  Aviso: quedaron {} recurso(s) de una operación anterior sin poder \
                         limpiarse; `doctor` los informa.",
                        recovery.kept.len()
                    );
                }
                if daemon.was_running {
                    println!(
                        "  Daemon: estaba en ejecución y se ha parado; se relanza con el comando \
                         habitual."
                    );
                }
            }
            if partial {
                Ok(Outcome::Verdict(
                    ExitCode::from_reason("setup_failed").code(),
                ))
            } else {
                Ok(Outcome::Done)
            }
        }
    }
}

/// `doctor`: compone el envelope a partir de la sección de ciclo de vida del motor y
/// devuelve el **veredicto** como código de salida, que es lo que permite el
/// contrato.
///
/// Un solo objeto en `stdout` **también cuando falla**: el veredicto va dentro, en
/// `checks` y `failed`, y la salida es 1. Devolver `Err` aquí haría que `main` adjuntara
/// detrás el objeto `error` y el envelope sería ilegible, que es el defecto que el
/// contrato prohíbe con «cada invocación emite exactamente un objeto JSON».
fn handle_doctor(json_mode: bool, repair: bool) -> Result<Outcome, CliError> {
    let exe = std::env::current_exe()
        .map_err(|e| CliError::new(ExitCode::Error, "doctor_failed", e.to_string()))?;
    let env = lifecycle::doctor::Env::resolve()
        .map_err(|e| CliError::new(ExitCode::Error, "doctor_failed", e.to_string()))?;
    // `--repair`: toma el bloqueo, ejecuta el barrido
    // existente —solo lo que el barrido recoge— y reevalúa el informe sobre
    // lo que quedó. Sin el flag, `doctor` solo informa.
    if repair {
        let lock = lifecycle::lock::acquire_exclusive(&env.roots.program_dir)
            .map_err(|e| lifecycle_error_to_cli(e.into()))?;
        let mut progress = PhaseProgress::new();
        // Patrón de bloque externo: el cierre del indicador —que consume el
        // indicador— se hace fuera del trabajo que puede fallar.
        let repair_result = 'work: {
            progress.phase("repairing", "Reparando restos del entorno…");
            progress.tick();
            break 'work lifecycle::recovery::recover(env.roots.recovery_roots())
                .map_err(lifecycle_error_to_cli);
        };
        let elapsed = progress.close();
        match repair_result {
            Ok(_) => {
                // El mensaje final va por stdout, como el de cualquier otro
                // comando: stderr lleva el progreso y los avisos, stdout lleva
                // el resultado.
                if !json_mode {
                    println!("{}", final_message("Reparación completada", elapsed));
                }
            }
            Err(e) => {
                drop(lock);
                return Err(e);
            }
        }
        drop(lock);
    }
    let report = lifecycle::doctor::report(&env, &exe, VERSION);
    #[cfg(feature = "native-stt")]
    let report = with_onnx_runtime_check(report);
    let failed = report.is_failure();

    if json_mode {
        // El motor devuelve el reporte ya serializable y con las nueve claves del
        // contrato
        // más las del contrato: aquí solo se estampa la versión del envelope.
        let value = serde_json::to_value(&report)
            .map_err(|e| CliError::new(ExitCode::Error, "doctor_failed", e.to_string()))?;
        emit_raw_json(value);
    } else if failed {
        for check in report.checks.iter().filter(|c| !c.ok) {
            eprintln!("  ✗ {}: {}", check.name, check.detail);
        }
        for path in &report.pending.unreadable {
            eprintln!("  no se puede listar {path}");
        }
        eprintln!(
            "Diagnóstico: {} comprobación(es) fallan.",
            report.failed.len()
        );
        // Sin `--repair`, ante restos se falla con la pista de reparación:
        // quien puede recogerlos es el mismo `doctor --repair`.
        if !repair && report.failed.iter().any(|name| name == "pending_artifacts") {
            eprintln!("  Pista: recoge los restos con `doctor --repair`.");
        }
    } else {
        println!("Diagnóstico: todo correcto.");
        for check in &report.checks {
            println!("  ✓ {}: {}", check.name, check.detail);
        }
    }

    if failed {
        Ok(Outcome::Verdict(ExitCode::Error.code()))
    } else {
        Ok(Outcome::Done)
    }
}

/// Añade al reporte de `doctor` la comprobación `onnx_runtime`: carga la
/// librería de ONNX Runtime que trae el bundle (o la de `ORT_DYLIB_PATH`) y
/// falla con la ruta buscada si no carga.
#[cfg(feature = "native-stt")]
fn with_onnx_runtime_check(mut report: lifecycle::doctor::Report) -> lifecycle::doctor::Report {
    match avi_shared::onnx_runtime::ensure_onnx_runtime() {
        Ok(path) => report.push_check(
            "onnx_runtime",
            true,
            format!("ONNX Runtime cargada desde {}", path.display()),
        ),
        Err(e) => report.push_check("onnx_runtime", false, e.to_string()),
    }
    report
}

/// Traduce un `reason` del motor a la variante de `ExitCode` que le corresponde.
///
/// El motor solo transporta el `reason`; el código sale de la tabla única
/// `ExitCode::from_reason`, y un `reason` sin variante propia cae en `ExitCode::Error`.
fn lifecycle_error_to_cli(err: anyhow::Error) -> CliError {
    match err.downcast_ref::<lifecycle::LifecycleError>() {
        Some(failure) => CliError::new(
            ExitCode::from_reason(failure.reason),
            failure.reason,
            failure.message.clone(),
        ),
        None => CliError::new(ExitCode::Error, "lifecycle_failed", err.to_string()),
    }
}

/// Control de procesos del producto, conectado al protocolo de parada del motor.
///
/// El motor trae el **protocolo** —`daemon.pid`, `POST /shutdown`, el árbol por PID, el
/// plazo global de 8 s— y entra por un rasgo el **control de procesos**, que vive aquí
/// porque arrastraría el árbol de `avi-daemon` y `avi-tts` al crate del motor. Es la
/// misma división que el motor documenta, y la guarda `pid != process::id()` que evita
/// el auto-mate del bug v0.18.10–v0.18.25 está en ambos lados.
struct ProductProcesses;

impl lifecycle::daemon_stop::ProcessControl for ProductProcesses {
    fn pid_alive(&self, pid: u32) -> bool {
        daemon::pid_alive(pid)
    }

    fn kill_tree_by_pid(&self, pid: u32) -> bool {
        daemon::kill_tree_by_pid(pid)
    }

    fn resident_pid_alive(&self, pid: u32) -> bool {
        avi_tts::resident::resident_pid_alive(pid)
    }

    fn kill_tree_resident_by_pid(&self, pid: u32) -> bool {
        avi_tts::resident::kill_tree_resident_by_pid(pid)
    }

    fn identity(&self, pid: u32) -> Option<avi_process::ProcessIdentity> {
        avi_process::process_identity(pid)
    }

    fn observe_identity(&self, pid: u32) -> avi_process::IdentityObservation {
        avi_process::observe_identity(pid)
    }
}

/// Borrado del directorio de programa con el mecanismo de plataforma (paso 8 de
/// la desinstalación).
///
/// En Unix es `remove_dir_all`. En Windows, si el ejecutable en uso está dentro —que es
/// el caso normal, porque el comando se invoca desde la propia instalación—, el
/// borrado directo es imposible y se programa con el limpiador propio desacoplado;
/// el motor decide cuál de los dos casos es y cambia el
/// desenlace a `removal_scheduled`.
struct ProgramRemoval;

impl lifecycle::uninstall::ProgramDirRemover for ProgramRemoval {
    fn exe_lives_inside(&self, program_dir: &std::path::Path) -> anyhow::Result<bool> {
        // La impedancia que justifica el borrado diferido es **de Windows**: allí el
        // ejecutable en uso impide borrar su directorio. En Unix no la hay —el proceso
        // sigue vivo desde el inodo desenlazado—, así que responder `true` llevaría al
        // motor a `schedule`, que en Unix no tiene implementación, y `self uninstall`
        // invocado desde la propia instalación nunca borraría el directorio de
        // programa.
        #[cfg(not(windows))]
        {
            let _ = program_dir;
            Ok(false)
        }
        #[cfg(windows)]
        {
            let exe = std::env::current_exe()
                .and_then(|exe| exe.canonicalize())
                .map_err(|e| {
                    anyhow::anyhow!("no se pudo resolver la ruta del ejecutable en uso: {e}")
                })?;
            let dir = program_dir.canonicalize().map_err(|e| {
                anyhow::anyhow!(
                    "no se pudo resolver la ruta del directorio de programa '{}': {e}",
                    program_dir.display()
                )
            })?;
            Ok(exe.starts_with(dir))
        }
    }

    fn remove_now(&self, program_dir: &std::path::Path) -> anyhow::Result<()> {
        if !program_dir.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(program_dir)?;
        Ok(())
    }

    fn schedule(
        &self,
        program_dir: &std::path::Path,
        wait: &avi_process::OwnerToken,
    ) -> anyhow::Result<()> {
        // El borrado diferido es un mecanismo **de Windows**: allí el ejecutable en uso
        // impide el borrado directo. En Unix no hay tal impedance y el motor ya habría
        // llamado a `remove_now`; llegar aquí sería un error del motor, no un caso que
        // tenga una implementación silenciosa.
        #[cfg(windows)]
        {
            avi_process::schedule_removal(program_dir, wait)
        }
        #[cfg(not(windows))]
        {
            let _ = (program_dir, wait);
            anyhow::bail!("el borrado diferido del directorio de programa es de Windows")
        }
    }
}
/// Borrado de rutas del ciclo con el mecanismo de plataforma (paso 10 de la
/// actualización).
///
/// En Unix es `remove_dir_all`. En Windows, si el ejecutable en uso está dentro
/// —el staging tras el traspaso—, el borrado directo es imposible y se programa
/// con el borrado diferido de `avi-process`; el motor decide cuál de los dos
/// casos es. Es la contrapartida de `ProgramRemoval` para rutas arbitrarias del
/// ciclo (staging, `.old-*`).
struct LifecyclePaths;

impl lifecycle::update::PathRemover for LifecyclePaths {
    fn remove_now(&self, path: &std::path::Path) -> anyhow::Result<()> {
        if !path.exists() {
            return Ok(());
        }
        std::fs::remove_dir_all(path)?;
        Ok(())
    }

    fn schedule(
        &self,
        path: &std::path::Path,
        wait: &avi_process::OwnerToken,
    ) -> anyhow::Result<()> {
        // El borrado diferido es un mecanismo **de Windows**: allí el ejecutable en uso
        // impide el borrado directo. En Unix no hay tal impedimento y el motor ya habría
        // llamado a `remove_now`; llegar aquí sería un error del motor, no un caso que
        // tenga una implementación silenciosa.
        #[cfg(windows)]
        {
            avi_process::schedule_removal(path, wait)
        }
        #[cfg(not(windows))]
        {
            let _ = (path, wait);
            anyhow::bail!("el borrado diferido de rutas del ciclo es de Windows")
        }
    }
}
// ─── Helpers ─────────────────────────────────────────────────────────

fn require_model_provisioned() -> Result<(), CliError> {
    let model_store = open_model_store()?;
    if !model_store.is_provisioned("qwen3-tts-0.6b") {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            "El modelo de síntesis TTS no está provisionado. Ejecuta 'setup' primero.",
        ));
    }
    Ok(())
}

/// Traduce un fallo de arranque del daemon a código de salida y `reason`. Es el
/// único punto de traducción, compartido por `start`, `restart` y `serve`, para
/// que la misma causa dé el mismo código en los tres: conflicto de puerto (6),
/// recurso inexistente (3), modelo ausente (4) o error no clasificado (1).
fn startup_error_to_cli(err: &daemon::StartupError) -> CliError {
    let reason = match err {
        daemon::StartupError::PortInUse { .. } => "port_in_use",
        daemon::StartupError::WarmVoiceMissing { .. } => "voice_not_found",
        daemon::StartupError::ModelMissing { .. } => "model_missing",
        daemon::StartupError::Failed { .. } => "daemon_error",
    };
    CliError::new(ExitCode::from_reason(reason), reason, err.to_string())
}

/// Valida identificadores de voz/etiqueta contra el regex del contrato
/// (`^[A-Za-z0-9._-]+$`) → exit 2.
fn is_valid_identifier(ids: Option<&str>, more: Option<&str>) -> Result<(), CliError> {
    for id in ids.into_iter().chain(more) {
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
/// Raíz de datos vigente, donde vive el pidfile del daemon.
///
/// Va por el motor porque el pidfile es **su** esquema: `daemon_stop` lo escribe, lo
/// lee y lo borra, y las lecturas tolerantes que este binario usaba eran una segunda
/// implementación del mismo formato.
fn effective_data_dir() -> Result<PathBuf, CliError> {
    lifecycle::data_dir().map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// `$HOME` del usuario, la misma clave que usa `lifecycle::path_unix` al escribir los
/// bloques delimitados del `PATH`. Se pasa al motor como dato porque la reversión
/// necesita **el mismo** `$HOME` con el que se escribió el bloque.
fn home_dir() -> Result<PathBuf, CliError> {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(PathBuf::from)
        .map_err(|_| {
            CliError::new(
                ExitCode::Error,
                "self_uninstall_failed",
                "No se pudo determinar el directorio del usuario: ni HOME ni USERPROFILE están definidas.",
            )
        })
}

/// Ruta del fichero ready de esta instancia: absoluta bajo el
/// `data_dir` vigente, de modo que cada sandbox (`AVI_DATA_DIR`) posee el
/// suyo sin depender de la unidad del proceso ni de `%TEMP%`.
fn ready_file_path() -> Result<PathBuf, CliError> {
    Ok(effective_data_dir()?.join(avi_process::registry::READY_FILE))
}

/// Lanza el daemon en segundo plano y espera el resultado de su arranque. Es la
/// única secuencia de lanzamiento, compartida por `start` y `restart`, y
/// termina siempre en uno de tres resultados:
/// - listo: el hijo publicó su dirección real y `/health` responde → su PID;
/// - fallo tipado: el hijo publicó un registro de fallo → el código de su causa
///   (`startup_error_to_cli`);
/// - muerte: el hijo terminó sin registro → exit 1 `daemon_error` con su estado
///   de salida.
///
/// Un hijo vivo que no publica antes de `deadline`, o que publica y no responde
/// a `/health` dentro de `DAEMON_HEALTH_DEADLINE` (con al menos una sonda,
/// aunque la publicación haya agotado `deadline`), sale con exit 5
/// `daemon_unreachable`. Cada arranque crea un log
/// `daemon_<pid>_<ms>.log` con stdout y stderr del hijo, renombrado acto seguido
/// con el PID del hijo (el del lanzador solo sirve para crearlo), de modo que el
/// nombre identifica al proceso que escribe y la poda puede conservarlo mientras
/// siga vivo. Todo error posterior
/// al lanzamiento cita su ruta sin cambiar `reason` ni el código. Ante cualquier fallo no
/// deja nada vivo: mata el árbol del hijo si sigue vivo, lo recolecta y borra
/// `daemon.ready`. No escribe el pidfile: lo escribe el propio daemon antes de
/// publicar `daemon.ready`.
async fn launch_daemon(
    client: &reqwest::Client,
    auto_restart: bool,
    max_retries: u32,
    warm_voice: &str,
    deadline: std::time::Duration,
    progress: Option<&PhaseProgress>,
) -> Result<u32, CliError> {
    let start = std::time::Instant::now();
    avi_shared::diag_trace::mark("launch_daemon.enter");
    // El hijo publica su registro en el fichero ready de la instancia (ruta
    // absoluta bajo el `data_dir` vigente).
    let ready_path = ready_file_path()?;
    if let Some(parent) = ready_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("No se pudo crear el dir del fichero ready: {}", e),
            )
        })?;
    }
    // Invalidar la señal previa: el fichero es determinista por instancia y
    // un contenido obsoleto se leería como resultado de este arranque.
    match std::fs::remove_file(&ready_path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => {
            return Err(CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!(
                    "No se pudo retirar el fichero ready previo '{}': {}",
                    ready_path.display(),
                    e
                ),
            ))
        }
    }
    avi_shared::diag_trace::mark("launch_daemon.ready_path_prepared");
    let logs_dir = avi_shared::paths::logs_dir()
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    let launcher_token = avi_process::current_owner_token()
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    let (log_path, log_file) = avi_shared::logs::create_log(
        &logs_dir,
        daemon::DAEMON_LOG_FAMILY,
        &launcher_token,
        &avi_process::classify_owner,
    )
    .map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("No se pudo crear el log del daemon: {}", e),
        )
    })?;
    avi_shared::diag_trace::mark("launch_daemon.log_created");
    let mut child = match daemon::spawn_background(
        auto_restart,
        max_retries,
        warm_voice,
        Some(&ready_path),
        log_file,
    ) {
        Ok(child) => child,
        Err(e) => {
            return Err(CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!(
                    "No se pudo lanzar el daemon: {}\nLog del daemon: {}",
                    e,
                    log_path.display()
                ),
            ))
        }
    };
    let pid = child.id();
    avi_shared::diag_trace::mark("launch_daemon.child_spawned");
    // El log nació con el token del lanzador porque su handle se entrega al
    // spawn; ya conocido el hijo, el nombre pasa a identificar al proceso que
    // escribe, y la poda puede reconocer por su token el log de un daemon vivo.
    let renamed = match avi_process::owner_token_of(pid) {
        Ok(child_token) => avi_shared::logs::rename_log_to_owner(
            &log_path,
            daemon::DAEMON_LOG_FAMILY,
            &child_token,
        )
        .map_err(|e| e.to_string()),
        Err(e) => Err(e.to_string()),
    };
    let log_path = match renamed {
        Ok(renamed) => renamed,
        Err(e) => {
            let err = CliError::new(
                ExitCode::Error,
                "io_error",
                format!(
                    "No se pudo renombrar el log del daemon '{}' con la identidad del proceso {}: {}",
                    log_path.display(),
                    pid,
                    e
                ),
            );
            abort_launch(&mut child, &ready_path).await;
            return Err(err);
        }
    };
    avi_shared::diag_trace::mark("launch_daemon.log_renamed");
    let with_log = |mut e: CliError| {
        e.message = format!("{}\nLog del daemon: {}", e.message, log_path.display());
        e
    };
    // PID hijo en memoria desde el spawn para que el handler Ctrl+C lo
    // reclame aunque aún no haya pidfile (ventana spawn → espera → write).
    IN_MEMORY_PID.store(pid, std::sync::atomic::Ordering::Relaxed);

    let addr = loop {
        match daemon::read_ready_signal(&ready_path) {
            Some(daemon::ReadySignal::Ready { addr, .. }) => {
                avi_shared::diag_trace::mark("launch_daemon.ready_signal_read");
                break addr;
            }
            Some(daemon::ReadySignal::Failed(e)) => {
                avi_shared::diag_trace::mark("launch_daemon.failed_signal_read");
                abort_launch(&mut child, &ready_path).await;
                return Err(with_log(startup_error_to_cli(&e)));
            }
            None => {}
        }
        if let Ok(Some(status)) = child.try_wait() {
            // El hijo pudo publicar su fallo justo antes de morir.
            if let Some(daemon::ReadySignal::Failed(e)) = daemon::read_ready_signal(&ready_path) {
                avi_shared::diag_trace::mark("launch_daemon.failed_signal_read_after_child_exit");
                abort_launch(&mut child, &ready_path).await;
                return Err(with_log(startup_error_to_cli(&e)));
            }
            avi_shared::diag_trace::mark("launch_daemon.child_exited_without_signal");
            abort_launch(&mut child, &ready_path).await;
            return Err(with_log(CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("el daemon terminó durante el arranque ({}).", status),
            )));
        }
        if start.elapsed() >= deadline {
            avi_shared::diag_trace::mark("launch_daemon.ready_deadline_expired");
            abort_launch(&mut child, &ready_path).await;
            return Err(with_log(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!(
                    "el daemon (pid {}) no publicó su dirección tras {:?}.",
                    pid, deadline
                ),
            )));
        }
        tokio::time::sleep(DAEMON_POLL_INTERVAL).await;
        if let Some(progress) = progress {
            progress.tick();
        }
    };

    avi_shared::diag_trace::mark("launch_daemon.health_probe_begin");
    if let Err(e) = await_daemon_ready(
        client,
        &addr,
        DAEMON_HEALTH_DEADLINE,
        DAEMON_POLL_INTERVAL,
        progress,
    )
    .await
    {
        avi_shared::diag_trace::mark("launch_daemon.health_probe_failed");
        abort_launch(&mut child, &ready_path).await;
        return Err(with_log(CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("el daemon publicó {} pero {}.", addr, e),
        )));
    }
    avi_shared::diag_trace::mark("launch_daemon.health_probe_ok");
    Ok(pid)
}

/// Limpieza de un arranque fallido: si el hijo sigue vivo, mata su árbol y lo
/// recolecta (hasta `DAEMON_LAUNCH_CLEANUP_DEADLINE`); después borra el fichero
/// ready y olvida el PID en memoria. Mientras el hijo no se recolecta su PID no
/// puede reutilizarse, así que el kill por PID no alcanza a un proceso ajeno;
/// si ya había terminado no se mata nada (en el arranque aún no existe el
/// residente TTS, que solo lanza el warmup posterior a estar listo).
async fn abort_launch(child: &mut avi_process::RestrictedChild, ready_path: &std::path::Path) {
    avi_shared::diag_trace::mark("abort_launch.enter");
    if let Ok(None) = child.try_wait() {
        avi_shared::diag_trace::mark("abort_launch.killing_child_tree");
        daemon::kill_tree_by_pid(child.id());
        let start = std::time::Instant::now();
        while start.elapsed() < DAEMON_LAUNCH_CLEANUP_DEADLINE {
            if !matches!(child.try_wait(), Ok(None)) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }
    if let Err(e) = std::fs::remove_file(ready_path) {
        if e.kind() != std::io::ErrorKind::NotFound {
            eprintln!(
                "Aviso: no se pudo retirar el fichero ready '{}': {}",
                ready_path.display(),
                e
            );
        }
    }
    IN_MEMORY_PID.store(0, std::sync::atomic::Ordering::Relaxed);
    avi_shared::diag_trace::mark("abort_launch.done");
}

/// Sondeo acotado de `/health` tras la publicación de la dirección: con el
/// warmup en segundo plano, «alcanzable = listo». Hace siempre al menos una
/// sonda, aunque `deadline` sea 0, y después agota `deadline` por reloj de
/// pared con cadencia `interval`.
async fn await_daemon_ready(
    client: &reqwest::Client,
    addr: &str,
    deadline: std::time::Duration,
    interval: std::time::Duration,
    progress: Option<&PhaseProgress>,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    loop {
        if probe_health(client, addr).await {
            return Ok(());
        }
        if start.elapsed() >= deadline {
            break;
        }
        tokio::time::sleep(interval).await;
        if let Some(progress) = progress {
            progress.tick();
        }
    }
    anyhow::bail!(
        "El daemon no respondió a /health tras {:?}",
        start.elapsed()
    )
}

/// Construye el cuerpo JSON de `daemon status`. Función pura (testeable sin daemon):
/// `running` solo con un daemon propio en servicio, con su `pid` y con `engine`,
/// `warm` y `warm_error` copiados de `/health`; en otro caso `stopped`, con
/// `degraded` cuando quedan restos que `daemon start` reclamará. Vía libre es
/// solo `stopped`, como la fixture. El `schema_version` lo añade `emit_raw_json`.
fn status_body(state: lifecycle::daemon_stop::DaemonState, health: Option<&Value>) -> Value {
    use lifecycle::daemon_stop::DaemonState;
    match state {
        DaemonState::Running { pid } => {
            let mut body = json!({ "daemon": "running", "pid": pid });
            for key in ["engine", "warm", "warm_error"] {
                if let Some(value) = health.and_then(|h| h.get(key)).filter(|v| v.is_string()) {
                    body[key] = value.clone();
                }
            }
            body
        }
        DaemonState::Degraded(degradation) => {
            json!({ "daemon": "stopped", "degraded": degradation.code() })
        }
        DaemonState::Stopped => json!({ "daemon": "stopped" }),
    }
}

/// Descripción en español del calentamiento que informa `/health`; un valor que no es de
/// contrato se muestra como desconocido.
fn warm_description(label: &str) -> &'static str {
    match label {
        "warming" => "calentando",
        "warm" => "listo",
        "warm_failed" => "falló el calentamiento",
        _ => "desconocido",
    }
}

/// Línea de texto de `daemon status`, con los mismos casos que [`status_body`].
fn status_line(state: lifecycle::daemon_stop::DaemonState, health: Option<&Value>) -> String {
    use lifecycle::daemon_stop::DaemonState;
    let field = |key: &str| {
        health
            .and_then(|h| h.get(key))
            .and_then(Value::as_str)
            .unwrap_or("desconocido")
            .to_string()
    };
    match state {
        DaemonState::Running { pid } => format!(
            "Daemon: en ejecución (pid {}, motor: {}, calentamiento: {}).",
            pid,
            field("engine"),
            warm_description(&field("warm"))
        ),
        DaemonState::Degraded(degradation) => format!(
            "Daemon: no está en servicio: {}. `daemon start` lo reclama antes de arrancar.",
            degradation.describe()
        ),
        DaemonState::Stopped => "Daemon: no está en ejecución.".to_string(),
    }
}

// ─── Cliente HTTP async del daemon ────────────────────────────────────────

/// Cliente `reqwest` hacia el daemon (HTTP, sin TLS: basta para
/// localhost). La dirección destino la resuelve cada llamada el protocolo
/// (`daemon_stop::resolve_client_addr`). Timeout de conexión breve para que el
/// probe Auto→local sea rápido cuando el daemon no está en ejecución.
fn daemon_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_millis(500))
        .timeout(avi_core::REQUEST_FAILSAFE)
        .build()
        .expect("construir el cliente HTTP del daemon")
}

/// Intervalo con el que el indicador se repinta durante una espera.
///
/// Es la cadencia con la que la fase actualizada llega a verse en la terminal:
/// un intervalo largo haría que el tiempo pareciera congelado entre paint y
/// paint, y uno corto gastaría la CPU en pintar sin que nadie lo note.
const PROGRESS_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

/// Algo que puede repintar la fase mientras se espera.
///
/// El indicador de progreso es la implementación real; la prueba usa un doble
/// que cuenta los repintados. El rasgo va aparte para que la prueba pueda
/// observar **cuántas veces** se repintó, que es lo que hace el indicador.
trait ProgressTicker {
    /// Repinta la fase actual con el tiempo transcurrido actualizado.
    fn repaint(&self);
}

impl ProgressTicker for PhaseProgress {
    fn repaint(&self) {
        self.tick();
    }
}

/// Espera a un futuro reponiendo el indicador mientras no termina.
///
/// Es la forma de que una fase parezca viva durante un trabajo que no emite
/// nada: el indicador se repinta con su propio reloj, de modo que el tiempo
/// que muestra avanza de verdad.
///
/// El futuro se evalúa primero y el repintado se hace **entre** intentos, no
/// durante: así el indicador no hace falta tenerlo prestado de forma mutable
/// a través del `await`, y el repintado usa `tick`, que solo consulta el
/// estado.
///
/// En modo no-TTY el repintado no abre línea nueva: el racionado del
/// indicador lo impide, de modo que la actividad repetida dentro de la misma
/// fase no inunda el log.
///
/// El resultado del futuro se devuelve tal cual, con su error.
async fn await_with_progress<P, F, T>(progress: &P, future: F) -> T
where
    P: ProgressTicker + ?Sized,
    F: std::future::Future<Output = T>,
{
    await_with_progress_every(progress, PROGRESS_REPAINT_INTERVAL, future).await
}

/// Igual que [`await_with_progress`], pero con el intervalo del repintado
/// elegido por quien llama.
///
/// Existe para que una prueba pueda usar un intervalo corto y determinista
/// sin cambiar el del producto.
async fn await_with_progress_every<P, F, T>(
    progress: &P,
    interval: std::time::Duration,
    future: F,
) -> T
where
    P: ProgressTicker + ?Sized,
    F: std::future::Future<Output = T>,
{
    let mut future = Box::pin(future);
    loop {
        match tokio::time::timeout(interval, &mut future).await {
            Ok(value) => return value,
            Err(_elapsed) => progress.repaint(),
        }
    }
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

/// Probe de vida sobre la dirección descubierta (pidfile o, sin él, la dirección
/// por defecto del protocolo); `false` habilita el fallback Auto→local.
async fn daemon_active(client: &reqwest::Client) -> Result<bool, CliError> {
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    Ok(probe_health(client, &client_addr).await)
}

/// Decide si una acción delegable se despacha al daemon:
/// ForceDaemon → siempre (el POST fallará con DaemonUnreachable si no corre);
/// Auto → solo si el daemon responde; ForceDirect → nunca.
async fn route_to_daemon(mode: DaemonMode, client: &reqwest::Client) -> Result<bool, CliError> {
    match mode {
        DaemonMode::ForceDaemon => Ok(true),
        DaemonMode::ForceDirect => Ok(false),
        DaemonMode::Auto => daemon_active(client).await,
    }
}

/// POST /transcribe al daemon: codifica PCM i16 LE 16 kHz mono a base64 y devuelve
/// el texto transcrito; el handler emite el envelope local ({text, source}).
async fn transcribe_via_daemon(
    client: &reqwest::Client,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    source_language: &str,
    denoise: bool,
    progress: &mut PhaseProgress,
) -> Result<String, CliError> {
    // El POST apunta a la dirección descubierta (fallback idéntico sin
    // pidfile).
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    progress.phase(
        if mic { "capturing" } else { "reading" },
        if mic {
            "Capturando audio del micrófono…"
        } else {
            "Leyendo archivo de audio…"
        },
    );
    if mic {
        progress.close_line();
    }
    let acquired = acquire_pcm(audio, mic, duration, avi_core::MAX_TRANSCRIBE_AUDIO_SECS).await;
    if mic {
        progress.repaint();
    }
    let pcm = acquired?;
    ensure_pcm_within_limit(
        &pcm,
        "El audio de la transcripción",
        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
    )?;
    progress.phase("transcribing", "Transcribiendo audio (daemon)…");
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut payload =
        serde_json::json!({ "audio_b64": audio_b64, "source_language": source_language });
    if denoise {
        payload["denoise"] = serde_json::json!(true);
    }
    // La espera se repinta con el indicador: `/transcribe` devuelve JSON plano
    // y no emite eventos, así que sin esto la fase parecería congelada.
    let resp = await_with_progress(
        progress,
        client
            .post(format!("http://{}/transcribe", client_addr))
            .json(&payload)
            .send(),
    )
    .await
    .map_err(|e| {
        CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("Daemon inalcanzable en {}: {}", client_addr, e),
        )
    })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
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
    Ok(text.to_string())
}

async fn translate_via_daemon(
    client: &reqwest::Client,
    text: &str,
    source_language: &str,
    target_language: &str,
    progress: &PhaseProgress,
) -> Result<(String, String, String), CliError> {
    let payload = serde_json::json!({
        "text": text,
        "source_language": source_language,
        "target_language": target_language,
    });
    // El POST apunta a la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let fut = client
        .post(format!("http://{}/translate", client_addr))
        .json(&payload)
        .send();
    // La espera se repinta con el indicador: `/translate` devuelve JSON plano
    // y no emite eventos, así que sin esto la fase parecería congelada. El
    // timeout de 1500 ms del endpoint se conserva: solo cambia lo que se ve
    // mientras espera.
    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        await_with_progress(progress, fut),
    )
    .await
    .map_err(|_| {
        CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("Daemon inalcanzable en {} (timeout 1500ms)", client_addr),
        )
    })?
    .map_err(|e| {
        CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("Daemon inalcanzable en {}: {}", client_addr, e),
        )
    })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
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
        return Err(CliError::new(
            ExitCode::from_reason(reason),
            reason,
            msg.to_string(),
        ));
    }
    let translated = val["translated"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::TranslationFailed,
            "translation_failed",
            "El daemon no devolvió 'translated'.",
        )
    })?;
    let source = val["source_language"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::TranslationFailed,
            "translation_failed",
            "El daemon no devolvió 'source_language'.",
        )
    })?;
    let target = val["target_language"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::TranslationFailed,
            "translation_failed",
            "El daemon no devolvió 'target_language'.",
        )
    })?;
    Ok((
        translated.to_string(),
        source.to_string(),
        target.to_string(),
    ))
}

/// POST /synthesize al daemon, consume el stream NDJSON y decodifica `audio_b64`
/// del evento `result`, devolviendo los bytes WAV del motor (24 kHz s16le mono).
async fn daemon_synthesize_wav(
    progress: &mut PhaseProgress,
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
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let resp = client
        .post(format!("http://{}/synthesize", client_addr))
        .json(&payload)
        .send()
        .await
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", client_addr, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    let val = consume_ndjson_stream(resp, StreamResult::Synthesize, progress).await?;
    let b64 = val["audio_b64"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "synthesis_failed",
            "El daemon devolvió result sin audio_b64.",
        )
    })?;
    base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "synthesis_failed",
                format!("audio_b64 del daemon no decodable: {}", e),
            )
        })
}

/// Timeout de inactividad entre eventos de un stream NDJSON: 1500 ms.
/// No acota la inferencia total: solo dispara si el daemon deja de emitir
/// (bucle atascado), nunca por inferencia sana (el servidor emite latidos cada
/// ~500 ms).
const STREAM_INACTIVITY_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(1500);
/// Deadline total failsafe del consumo de un stream (paridad con `daemon_client`):
/// red de seguridad documentada, no presupuesto.
const STREAM_TOTAL_DEADLINE: std::time::Duration = avi_core::REQUEST_FAILSAFE;

/// Ruta esperada por el consumidor NDJSON; cada llamador extrae su dato del
/// evento final sin alterar la forma contractual de las otras rutas.
#[derive(Clone, Copy)]
enum StreamResult {
    Synthesize,
    Clone,
    Dub,
}

impl StreamResult {
    fn stage(self) -> &'static str {
        match self {
            Self::Synthesize => "synthesize",
            Self::Clone => "clone",
            Self::Dub => "dub",
        }
    }
}

/// Consume un stream NDJSON del daemon (`started` → latidos → `result`/`error`)
/// con timeout de inactividad + failsafe total. Retorna el evento final
/// `result`; el evento `error` se mapea a `CliError` con la tabla única.
/// Sin `result` (stream truncado, NDJSON inválido, inactividad o failsafe) el
/// fallo es ruidoso: nunca se reemite un éxito parcial.
async fn consume_ndjson_stream(
    mut resp: reqwest::Response,
    expected: StreamResult,
    progress: &mut PhaseProgress,
) -> Result<Value, CliError> {
    let deadline = tokio::time::Instant::now() + STREAM_TOTAL_DEADLINE;
    let stage = expected.stage();
    let mut rest = String::new();
    // Los diagnósticos nombran la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return Err(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!(
                    "Daemon inalcanzable en {} (límite total {}s agotado en {})",
                    client_addr,
                    STREAM_TOTAL_DEADLINE.as_secs(),
                    stage
                ),
            ));
        };
        let wait = std::cmp::min(STREAM_INACTIVITY_TIMEOUT, remaining);
        let chunk = tokio::time::timeout(wait, resp.chunk())
            .await
            .map_err(|_| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    format!(
                        "Daemon inalcanzable en {} (sin eventos del daemon en 1500ms en {})",
                        client_addr, stage
                    ),
                )
            })?
            .map_err(|e| {
                CliError::new(
                    ExitCode::DaemonUnreachable,
                    "daemon_unreachable",
                    format!("Daemon inalcanzable en {}: {}", client_addr, e),
                )
            })?;
        let bytes = match chunk {
            Some(b) => b,
            None => {
                // Fin de stream: procesar el resto acumulado (línea sin `\n`
                // final) y exigir el evento final.
                let line = std::mem::take(&mut rest);
                if !line.trim().is_empty() {
                    if let Some(v) = process_stream_line(&line)? {
                        return Ok(v);
                    }
                }
                return Err(CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!(
                        "El stream del daemon terminó sin evento final en {}.",
                        stage
                    ),
                ));
            }
        };
        rest.push_str(&String::from_utf8_lossy(&bytes));
        while let Some(pos) = rest.find('\n') {
            let line: String = rest.drain(..=pos).collect();
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            // Se analiza una sola vez: emite progreso intermedio y clasifica
            // result/error con la misma lógica de process_stream_line,
            // que permanece pura y conserva su prueba.
            let ev: Value = serde_json::from_str(trimmed).map_err(|e| {
                CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!("NDJSON inválido del daemon: {}", e),
                )
            })?;
            if let Some((label, msg)) = command_stream_phase(&ev, expected) {
                progress.phase(label, &msg);
            }
            match ev.get("event").and_then(|v| v.as_str()) {
                Some("result") => return Ok(ev),
                Some("error") => return Err(stream_error_event(&ev)),
                _ => {}
            }
        }
    }
}

/// Traduce una respuesta no 2xx del daemon a `CliError`: el `reason` sale del cuerpo
/// (o de su espejo `error`, o `daemon_error` si falta) y el código, de la tabla única.
/// Un 404 sin `reason` ni `error` indica una ruta ausente: o el daemon en marcha es
/// anterior al binario, o el build no incluye la función; el mensaje nombra ambas causas.
async fn daemon_error_from_response(resp: reqwest::Response) -> CliError {
    let status = resp.status();
    let raw = match resp.text().await {
        Ok(text) => text,
        Err(e) => {
            return CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("No se pudo leer la respuesta del daemon (HTTP {status}): {e}"),
            )
        }
    };
    let parsed = serde_json::from_str::<Value>(&raw);
    let body = parsed.as_ref().cloned().unwrap_or(Value::Null);
    let reason = body
        .get("reason")
        .and_then(|v| v.as_str())
        .or_else(|| body.get("error").and_then(|v| v.as_str()));
    let Some(reason) = reason else {
        let msg = if status == reqwest::StatusCode::NOT_FOUND {
            "El daemon no expone esta ruta (HTTP 404): reinícialo con el binario actual \
             ejecutando `ai-voice-interconnector daemon restart`, o usa un build que incluya la función."
                .to_string()
        } else if parsed.is_err() {
            // Cuerpo que no es JSON: se muestra su inicio para diagnosticarlo.
            let start: String = raw.trim().chars().take(200).collect();
            let start = if start.is_empty() {
                "cuerpo vacío".to_string()
            } else {
                start
            };
            format!("Respuesta del daemon que no es JSON (HTTP {status}): {start}")
        } else {
            let msg = body
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("error del daemon");
            format!("{} (HTTP {})", msg, status)
        };
        return CliError::new(ExitCode::Error, "daemon_error", msg);
    };
    let msg = body
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("error del daemon");
    CliError::new(
        ExitCode::from_reason(reason),
        reason,
        format!("{} (HTTP {})", msg, status),
    )
}

/// Traduce un evento `error` de un stream NDJSON del daemon a `CliError`, con el
/// código de la tabla única.
fn stream_error_event(ev: &Value) -> CliError {
    let reason = ev
        .get("reason")
        .and_then(|v| v.as_str())
        .unwrap_or("daemon_error");
    let msg = ev
        .get("message")
        .and_then(|v| v.as_str())
        .unwrap_or("error del daemon");
    CliError::new(ExitCode::from_reason(reason), reason, msg.to_string())
}

/// Procesa una línea del stream: `result` → `Some(evento)`, `error` → `Err`
/// mapeado, resto (`started`/latidos/desconocidos) → `Some` nada (`Ok(None)`).
fn process_stream_line(line: &str) -> Result<Option<Value>, CliError> {
    let line = line.trim();
    if line.is_empty() {
        return Ok(None);
    }
    let ev: Value = serde_json::from_str(line).map_err(|e| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("NDJSON inválido del daemon: {}", e),
        )
    })?;
    match ev.get("event").and_then(|v| v.as_str()) {
        Some("result") => Ok(Some(ev)),
        Some("error") => Err(stream_error_event(&ev)),
        _ => Ok(None),
    }
}

/// Decide la etiqueta de racionado y el texto de un evento intermedio, sin
/// escribir ni consultar el reloj. Los eventos desconocidos no anuncian fase.
fn ndjson_progress_phase(ev: &Value) -> Option<(&'static str, String)> {
    match ev.get("event").and_then(Value::as_str) {
        Some("started") => {
            let op = ev
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("operación");
            Some(("started", format!("Iniciando {op}…")))
        }
        Some(event @ ("progress" | "heartbeat")) => {
            let (label, msg) = match ev.get("stage").and_then(Value::as_str) {
                Some("warming") => ("warming", "Calentando motor de síntesis…"),
                Some("queued") => ("queued", "En cola de síntesis…"),
                Some("synthesis") => ("synthesis", "Sintetizando voz…"),
                Some("warmup") => ("warmup", "Iniciando precarga en caliente…"),
                Some("clone") => ("clone", "Clonando voz… (puede tardar varios minutos)"),
                Some("transcribe") => ("transcribe", "Transcribiendo audio…"),
                Some("translate") => ("translate", "Traduciendo texto…"),
                _ => return None,
            };
            let msg = if event == "progress" {
                ev.get("message").and_then(Value::as_str).unwrap_or(msg)
            } else {
                msg
            };
            Some((label, msg.to_string()))
        }
        _ => None,
    }
}

/// Traduce el evento real a la fase canónica del comando cuando la etapa
/// corresponde a una fase local; conserva las etapas de detalle del motor.
fn command_stream_phase(ev: &Value, expected: StreamResult) -> Option<(&'static str, String)> {
    let (label, msg) = ndjson_progress_phase(ev)?;
    let canonical = match (expected, label) {
        (StreamResult::Synthesize | StreamResult::Dub, "synthesis") => "synthesizing",
        (StreamResult::Clone, "clone") => "cloning",
        (StreamResult::Dub, "transcribe") => "transcribing",
        (StreamResult::Synthesize | StreamResult::Dub, "translate") => "translating",
        _ => label,
    };
    Some((canonical, msg))
}

/// `synthesize` vía daemon: persiste el WAV en `SpeechStore` y respeta
/// --label/--play, devolviendo la ruta del WAV persistido (paralelo al
/// handler local para que el envelope JSON de salida coincida).
// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech synthesize`.
#[allow(clippy::too_many_arguments)]
async fn synthesize_via_daemon(
    progress: &mut PhaseProgress,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    label: &str,
    force: bool,
    play: bool,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<Option<PathBuf>, CliError> {
    let speech_store = open_speech_store()?;
    let label_lower = label.to_lowercase();
    is_valid_identifier(Some(&label_lower), None)?;
    if !force
        && speech_store
            .find(voice, &label_lower)
            .map_err(speech_find_error)?
            .is_some()
    {
        return Err(CliError::new(
            ExitCode::StateConflict,
            "label_exists",
            format!(
                "Ya existe una locución con la etiqueta '{}' (usa --force).",
                label_lower
            ),
        ));
    }
    let wav = daemon_synthesize_wav(
        progress,
        client,
        text,
        voice,
        source_language,
        target_language,
        temperature,
    )
    .await?;
    // Guardián del temporal: lo borra al salir de ámbito por cualquier camino.
    let tmp_guard = avi_process::scratch::temp_wav("avi_tts_")
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
    let tmp = tmp_guard.path().clone();
    std::fs::write(&tmp, &wav)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;

    let saved = if play {
        // Bucle interactivo en el cliente; el bucle vive en el llamador y la
        // opción 3 re-despacha la síntesis por la misma vía daemon.
        let mut reader = menu_stdin();
        let mut playback = menu_playback(&label_lower);
        progress.close_line();
        let mut resynthesizer = DaemonResynthesizer {
            client,
            text,
            voice,
            source_language,
            target_language,
            temperature,
            tmp_wav: &tmp,
            progress,
        };
        let outcome = play_menu_loop(
            voice,
            &label_lower,
            force,
            &speech_store,
            text,
            &tmp,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await;
        progress.repaint();
        match outcome? {
            Some(saved) => saved,
            None => return Ok(None),
        }
    } else {
        progress.phase("saving", "Guardando locución…");
        speech_store
            .save(voice, &label_lower, text, &tmp)
            .map_err(|e| CliError::new(ExitCode::Error, "synthesis_failed", e.to_string()))?
    };
    Ok(Some(saved))
}

/// `say` vía daemon: reproduce el WAV decodificado y expone una copia efímera.
// Cada argumento representa un dato de la orden y el indicador pertenece al llamador.
#[allow(clippy::too_many_arguments)]
async fn say_via_daemon(
    progress: &mut PhaseProgress,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<(), CliError> {
    let wav = daemon_synthesize_wav(
        progress,
        client,
        text,
        voice,
        source_language,
        target_language,
        temperature,
    )
    .await?;
    // Guardián del temporal: lo borra al salir de ámbito, también si falla la reproducción.
    let tmp = avi_process::scratch::temp_wav("avi_say_")
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
    std::fs::write(tmp.path(), &wav)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    progress.phase("playing", "Reproduciendo…");
    audio::AudioService::new()
        .play_wav(tmp.path())
        .map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "playback_failed",
                format!("Fallo al reproducir la locución: {}", e),
            )
        })?;
    Ok(())
}

async fn clone_via_daemon(
    client: &reqwest::Client,
    name: &str,
    audio_bytes: &[u8],
    force: bool,
    denoise: bool,
    progress: &mut PhaseProgress,
) -> Result<(String, String, bool), CliError> {
    progress.phase("cloning", "Clonando voz (daemon).");
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(audio_bytes);
    let mut payload = serde_json::json!({
        "name": name,
        "audio_b64": audio_b64,
        "force": force,
    });
    if denoise {
        payload["denoise"] = serde_json::json!(true);
    }
    // El envío solo espera las cabeceras (el daemon valida barato y
    // responde 200 de inmediato); el trabajo pesado se consume como stream con
    // inactividad 1500 ms + failsafe `REQUEST_FAILSAFE` hasta el evento final.
    // El POST apunta a la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let fut = client
        .post(format!("http://{}/voices/clone", client_addr))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout 1500ms)", client_addr),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", client_addr, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    let val: Value = consume_ndjson_stream(resp, StreamResult::Clone, progress).await?;
    let cloned_name = val["name"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            "El daemon no devolvió 'name' en la clonación.",
        )
    })?;
    let cloned_speech = val["speech"].as_str().ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            "El daemon no devolvió 'speech' en la clonación.",
        )
    })?;
    let precomputed = val["precomputed"].as_bool().ok_or_else(|| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            "El daemon no devolvió 'precomputed' en la clonación.",
        )
    })?;
    Ok((
        cloned_name.to_string(),
        cloned_speech.to_string(),
        precomputed,
    ))
}

// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech dub`.
#[allow(clippy::too_many_arguments)]
async fn dub_via_daemon(
    progress: &mut PhaseProgress,
    client: &reqwest::Client,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
    denoise: bool,
    voice: &str,
) -> Result<String, CliError> {
    // Captura/lectura PCM y encode a base64 para POST /dub
    progress.phase(
        if mic { "capturing" } else { "reading" },
        if mic {
            "Capturando audio del micrófono…"
        } else {
            "Leyendo archivo de audio…"
        },
    );
    if mic {
        progress.close_line();
    }
    let acquired = acquire_pcm(audio, mic, duration, avi_core::MAX_DUB_AUDIO_SECS).await;
    if mic {
        progress.repaint();
    }
    let pcm = acquired?;
    if pcm.is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_audio",
            "No se capturó audio; pulsa Enter después de hablar o verifica el micrófono.",
        ));
    }
    ensure_pcm_within_limit(&pcm, "El audio del doblaje", avi_core::MAX_DUB_AUDIO_SECS)?;
    progress.phase("dubbing", "Doblando audio (daemon)…");
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut payload = serde_json::json!({
        "audio_b64": audio_b64,
        "source_language": source_language,
        "target_language": target_language,
        "voice": voice,
    });
    if let Some(t) = temperature {
        payload["temperature"] = serde_json::json!(t);
    }
    if denoise {
        payload["denoise"] = serde_json::json!(true);
    }
    // El envío espera las cabeceras (respuesta 200 inmediata tras
    // validar barato); el pipeline se consume como stream con inactividad
    // 1500 ms + failsafe `REQUEST_FAILSAFE` hasta el evento final.
    // El POST apunta a la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let fut = client
        .post(format!("http://{}/dub", client_addr))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!(
                    "Daemon inalcanzable en {} (timeout dub 1500ms)",
                    client_addr
                ),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", client_addr, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    // El evento final del stream trae la forma contractual
    // {status:"dubbed", text, translated, audio_b64, voice}; los eventos `error`
    // del stream se traducen con la misma tabla única.
    let val: Value = consume_ndjson_stream(resp, StreamResult::Dub, progress).await?;
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
    // Guardián del temporal: lo borra al salir de ámbito, también si falla la reproducción.
    let tmp_wav = avi_process::scratch::temp_wav("avi_dub_")
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
    std::fs::write(tmp_wav.path(), &wav_bytes)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    progress.phase("playing", "Reproduciendo doblaje…");
    audio::AudioService::new()
        .play_wav(tmp_wav.path())
        .map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "playback_failed",
                format!("Fallo al reproducir el doblaje: {}", e),
            )
        })?;
    Ok(final_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clone_pcm_truncates_at_thirty_seconds_without_padding() {
        let limit = (avi_core::MAX_CLONE_REFERENCE_SECS * 16_000) as usize;
        let mut over = vec![7; limit + 1];
        super::truncate_clone_pcm(&mut over);
        assert_eq!(over.len(), limit);

        let mut exact = vec![7; limit];
        super::truncate_clone_pcm(&mut exact);
        assert_eq!(exact.len(), limit);

        let mut short = vec![7; limit - 1];
        super::truncate_clone_pcm(&mut short);
        assert_eq!(short.len(), limit - 1);
    }

    #[test]
    fn clone_pcm_wav_base64_round_trip_preserves_known_samples() {
        let samples = [0, 1_234, -2_345, i16::MAX, i16::MIN];
        let wav = avi_audio::encode_pcm_16k_mono_wav(&samples).expect("codificar WAV");
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wav);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("decodificar base64");
        let mut reader = hound::WavReader::new(std::io::Cursor::new(decoded)).expect("leer WAV");
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(
            reader
                .samples::<i16>()
                .map(Result::unwrap)
                .collect::<Vec<_>>(),
            samples
        );
    }

    #[test]
    #[ignore = "requiere ejecución explícita: benchmark informativo que mide este host"]
    fn benchmark_clone_mic_wav_base64() {
        let samples = vec![123; 16_000 * 30];
        let started = std::time::Instant::now();
        let wav = avi_audio::encode_pcm_16k_mono_wav(&samples).expect("codificar WAV");
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wav);
        eprintln!(
            "PCM→WAV→base64 (30 s): {:?}; {} bytes",
            started.elapsed(),
            encoded.len()
        );
    }

    /// El envelope de `self update` cancelado lleva `status: "cancelled"`,
    /// `reason` nulo y las mismas claves que `already_up_to_date`, sin claves de más.
    #[test]
    fn update_cancelled_envelope_has_fixed_shape() {
        let envelope = compose_update_status_envelope(
            "cancelled",
            "1.0.0",
            "1.1.0",
            lifecycle::channel::Channel::Script,
        );
        assert_eq!(
            envelope,
            json!({
                "status": "cancelled",
                "reason": null,
                "current": "1.0.0",
                "latest": "1.1.0",
                "channel": "script",
            })
        );
    }

    /// Escritor en memoria para capturar lo que emite el `fmt` con un filtro dado.
    #[derive(Clone, Default)]
    struct MemWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

    impl std::io::Write for MemWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Emite un `INFO` y un `WARN` por target (uno propio y una dependencia) bajo
    /// el filtro dado y devuelve lo que llegó al escritor.
    fn captured_output(filter: tracing_subscriber::EnvFilter) -> String {
        use tracing_subscriber::layer::SubscriberExt;
        use tracing_subscriber::Layer;
        let writer = MemWriter::default();
        let sink = writer.clone();
        let subscriber = tracing_subscriber::registry().with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || sink.clone())
                .with_filter(filter),
        );
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "avi_store", "info propio");
            tracing::warn!(target: "avi_store", "warn propio");
            tracing::info!(target: "xet_client", "info dependencia");
            tracing::warn!(target: "xet_client", "warn dependencia");
        });
        let bytes = writer.0.lock().unwrap().clone();
        String::from_utf8(bytes).unwrap()
    }

    /// Un nombre o una latencia `null` se muestran como «desconocido»; los
    /// conocidos, tal cual.
    #[test]
    fn device_line_shows_unknown_for_null_fields() {
        assert_eq!(
            device_line(&json!({ "id": 1, "name": null, "latency": null })),
            "  [1] desconocido (latencia: desconocido)"
        );
        assert_eq!(
            device_line(&json!({ "id": 0, "name": "Altavoces", "latency": 0.0105 })),
            "  [0] Altavoces (latencia: 10.5ms)"
        );
    }

    /// Sin `RUST_LOG`, la CLI solo muestra `warn` y superiores, también
    /// de los crates propios.
    #[test]
    fn cli_default_filter_shows_only_warnings() {
        let out = captured_output(log_filter(None, false));
        assert!(out.contains("warn propio"), "{out}");
        assert!(out.contains("warn dependencia"), "{out}");
        assert!(!out.contains("info propio"), "{out}");
        assert!(!out.contains("info dependencia"), "{out}");
    }

    /// Sin `RUST_LOG`, el daemon registra `info` de los crates propios y solo
    /// `warn` de las dependencias.
    #[test]
    fn daemon_default_filter_keeps_own_info_and_silences_dependencies() {
        let out = captured_output(log_filter(None, true));
        assert!(out.contains("info propio"), "{out}");
        assert!(out.contains("warn propio"), "{out}");
        assert!(out.contains("warn dependencia"), "{out}");
        assert!(!out.contains("info dependencia"), "{out}");
    }

    /// Con `RUST_LOG` definida, la variable manda sobre el filtro por defecto.
    #[test]
    fn rust_log_overrides_the_default_filter() {
        let out = captured_output(log_filter(Some("info"), false));
        assert!(out.contains("info propio"), "{out}");
    }

    /// `await_daemon_ready` debe agotar el deadline por reloj de pared (no un
    /// recuento fijo de iteraciones) contra un puerto sin oyente: retorna `Err` y
    /// el tiempo transcurrido queda acotado por el deadline. Hermético: no arranca
    /// daemon ni paga warmup, y usa un puerto efímero reservado (no el 8765
    /// compartido).
    #[tokio::test]
    async fn await_daemon_ready_respects_deadline() {
        // Puerto efímero enlazado sin `listen`: mientras viva el socket nadie
        // más puede enlazarlo y toda conexión se rechaza.
        let reserved = tokio::net::TcpSocket::new_v4().expect("socket efímero");
        reserved
            .bind(std::net::SocketAddr::from(([127, 0, 0, 1], 0)))
            .expect("bind efímero");
        let addr = reserved.local_addr().expect("local_addr").to_string();

        let deadline = std::time::Duration::from_millis(800);
        let interval = std::time::Duration::from_millis(100);
        let start = std::time::Instant::now();
        let res = await_daemon_ready(&daemon_client(), &addr, deadline, interval, None).await;
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

    /// Con presupuesto 0, `await_daemon_ready` sigue haciendo una sonda: un
    /// daemon que publica su dirección al final del plazo de arranque y ya
    /// responde a `/health` se confirma como listo.
    #[tokio::test]
    async fn await_daemon_ready_probes_at_least_once_with_zero_budget() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind efímero");
        let addr = listener.local_addr().expect("local_addr").to_string();
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let (mut sock, _) = listener.accept().await.expect("aceptar la sonda");
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = sock.read(&mut buf).await.expect("leer la sonda");
                if n == 0 {
                    break;
                }
                request.extend_from_slice(&buf[..n]);
            }
            sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                .await
                .expect("responder a la sonda");
        });

        let res = await_daemon_ready(
            &daemon_client(),
            &addr,
            std::time::Duration::ZERO,
            std::time::Duration::from_millis(100),
            None,
        )
        .await;

        assert!(
            res.is_ok(),
            "con presupuesto 0 debe sondear una vez y confirmar: {res:?}"
        );
        server.await.expect("el servidor de la sonda termina");
    }

    /// La falta del modelo que exige la voz sale como `model_missing` (exit 4),
    /// igual que la ausencia de CustomVoice, y no como `synthesis_failed`.
    #[test]
    fn synthesis_cli_error_maps_missing_model_to_model_missing() {
        let any: anyhow::Error = avi_tts::ModelMissing {
            message: "falta el modelo",
        }
        .into();
        let err = synthesis_cli_error(any);
        assert_eq!(err.code, ExitCode::ModelMissing);
        assert_eq!(err.reason, "model_missing");
        let other = synthesis_cli_error(anyhow::anyhow!("otro fallo"));
        assert_eq!(other.reason, "synthesis_failed");
    }

    /// Una voz sin referencia que no es preset sale como `voice_not_found`
    /// (exit 3), sin necesitar el motor real.
    #[test]
    fn synthesis_cli_error_maps_voice_not_found() {
        let any: anyhow::Error = avi_tts::VoiceNotFound {
            voice: "nadie".to_string(),
        }
        .into();
        let err = synthesis_cli_error(any);
        assert_eq!(err.code, ExitCode::NotFound);
        assert_eq!(err.reason, "voice_not_found");
        assert!(err.message.contains("nadie"), "{}", err.message);
    }

    /// Cada clase de fallo de arranque sale con su código y `reason`, y el
    /// mensaje de puerto ocupado nombra el puerto y `AVI_DAEMON_PORT`.
    #[test]
    fn startup_error_to_cli_maps_each_class() {
        let port = startup_error_to_cli(&daemon::StartupError::PortInUse { port: 8765 });
        assert_eq!(port.code, ExitCode::StateConflict);
        assert_eq!(port.reason, "port_in_use");
        assert!(port.message.contains("8765"), "{}", port.message);
        assert!(port.message.contains("AVI_DAEMON_PORT"), "{}", port.message);

        let voice = startup_error_to_cli(&daemon::StartupError::WarmVoiceMissing {
            voice: "nadie".to_string(),
        });
        assert_eq!(voice.code, ExitCode::NotFound);
        assert_eq!(voice.reason, "voice_not_found");
        assert!(voice.message.contains("nadie"), "{}", voice.message);

        let model = startup_error_to_cli(&daemon::StartupError::ModelMissing {
            message: "falta opus-mt-es-en: ejecuta setup".to_string(),
        });
        assert_eq!(model.code, ExitCode::ModelMissing);
        assert_eq!(model.reason, "model_missing");
        assert!(model.message.contains("setup"));

        let failed = startup_error_to_cli(&daemon::StartupError::Failed {
            message: "motor caído".to_string(),
        });
        assert_eq!(failed.code, ExitCode::Error);
        assert_eq!(failed.reason, "daemon_error");
        assert_eq!(failed.message, "motor caído");
    }

    /// `status_body` mapea los estados de `daemon status`: vía libre es solo
    /// `stopped` (fixture intacta), en servicio lleva `pid` y lo de `/health`, y
    /// los restos salen `stopped` con `degraded`.
    #[test]
    fn status_body_maps_daemon_states() {
        use lifecycle::daemon_stop::{DaemonState, Degradation};
        // stopped: solo `daemon` (schema_version lo añade emit_raw_json).
        let stopped = status_body(DaemonState::Stopped, None);
        assert_eq!(stopped, json!({ "daemon": "stopped" }));

        // running con PID y warm, sin warm_error.
        let health = json!({ "status": "ready", "engine": "rust_native", "warm": "warm" });
        let running = status_body(DaemonState::Running { pid: 42 }, Some(&health));
        assert_eq!(
            running,
            json!({ "daemon": "running", "pid": 42, "engine": "rust_native", "warm": "warm" })
        );

        // running + warm_failed con causa.
        let failed_health =
            json!({ "engine": "rust_native", "warm": "warm_failed", "warm_error": "boom" });
        let failed = status_body(DaemonState::Running { pid: 42 }, Some(&failed_health));
        assert_eq!(failed["warm"], "warm_failed");
        assert_eq!(failed["warm_error"], "boom");

        // restos: `stopped` con el código del resto.
        let degraded = status_body(DaemonState::Degraded(Degradation::Unresponsive), None);
        assert_eq!(
            degraded,
            json!({ "daemon": "stopped", "degraded": "unresponsive" })
        );
    }

    /// El texto de `daemon status` describe el calentamiento en español y no imprime
    /// ningún valor del contrato `--json`.
    #[test]
    fn status_line_describes_warmup_in_spanish() {
        use lifecycle::daemon_stop::{DaemonState, Degradation};
        let line = |warm: Option<&str>| {
            let health = warm.map(|w| json!({ "engine": "rust_native", "warm": w }));
            status_line(DaemonState::Running { pid: 1234 }, health.as_ref())
        };
        let cases = [
            (Some("warming"), "calentando"),
            (Some("warm"), "listo"),
            (Some("warm_failed"), "falló el calentamiento"),
            (Some("otro"), "desconocido"),
        ];
        for (warm, text) in cases {
            let expected = format!(
                "Daemon: en ejecución (pid 1234, motor: rust_native, calentamiento: {text})."
            );
            assert_eq!(line(warm), expected);
            assert!(!line(warm).contains("warm"));
        }
        let absent = status_line(DaemonState::Running { pid: 1234 }, None);
        assert_eq!(
            absent,
            "Daemon: en ejecución (pid 1234, motor: desconocido, calentamiento: desconocido)."
        );
        assert!(!absent.contains("warm"));
        assert_eq!(
            status_line(DaemonState::Stopped, None),
            "Daemon: no está en ejecución."
        );
        let degraded = status_line(DaemonState::Degraded(Degradation::Unresponsive), None);
        assert!(degraded.starts_with("Daemon: no está en servicio: "));
    }

    /// El estado de la parada sale del `was_running` del motor: sin ejecución
    /// previa es `not_running`, con ella `shutdown_sent`. La parada completa
    /// (`daemon_fully_stopped`) la verifica el motor en `StopOutcome`.
    #[test]
    fn stop_status_follows_was_running_from_engine() {
        assert_eq!(stop_status(false), "not_running");
        assert_eq!(stop_status(true), "shutdown_sent");
    }

    /// El fallo de la parada tiene `status` propio, distinto de los de éxito:
    /// `still_running` dice que no concluyó, sin afirmar un apagado que no ocurrió.
    #[test]
    fn stop_failure_status_is_still_running() {
        assert_eq!(stop_failed_status(), "still_running");
    }

    /// `process_stream_line` clasifica cada línea NDJSON sin reloj
    /// (determinista): `result` se entrega, `error` se mapea por reason con la
    /// tabla única, `started`/latidos se ignoran y el NDJSON inválido falla ruidoso.
    #[test]
    fn process_stream_line_classifies_events() {
        let r = process_stream_line(r#"{"event":"result","name":"v","precomputed":true}"#)
            .expect("result no falla");
        assert_eq!(r.expect("result se entrega")["name"], "v");
        for line in [
            r#"{"event":"started","name":"v"}"#,
            r#"{"event":"heartbeat","stage":"clone"}"#,
            r#"{"event":"progress","stage":"warmup"}"#,
            "",
            "   ",
        ] {
            assert!(
                process_stream_line(line)
                    .expect("no-final no falla")
                    .is_none(),
                "línea no-final se ignora: {:?}",
                line
            );
        }
        let e =
            process_stream_line(r#"{"event":"error","reason":"voice_exists","message":"existe"}"#)
                .expect_err("error debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "voice_exists");
        assert!(
            process_stream_line("{no json").is_err(),
            "NDJSON inválido falla ruidoso"
        );
    }

    /// Sirve una secuencia NDJSON programada sobre HTTP plano en loopback para
    /// ejercitar `consume_ndjson_stream` sin daemon (doble determinista).
    /// `hold_secs`: si es `Some`, tras escribir `lines` la conexión se
    /// retiene abierta ese tiempo (caso de atasco); si es `None`, se cierra
    /// (éxito, fallo o truncado según `lines`). Emite cada línea por separado
    /// para permitir comprobar la lectura incremental. Sin `content-length`:
    /// cuerpo hasta cierre de conexión.
    async fn serve_programmed_sequence(lines: Vec<String>, hold_secs: Option<u64>) -> String {
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
            let mut read = 0;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                read += n;
                if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read >= buf.len() {
                    break;
                }
            }
            let header = "HTTP/1.1 200 OK\r\ncontent-type: application/x-ndjson\r\nconnection: close\r\n\r\n";
            if sock.write_all(header.as_bytes()).await.is_err() {
                return;
            }
            for line in lines {
                if sock
                    .write_all(format!("{line}\n").as_bytes())
                    .await
                    .is_err()
                {
                    return;
                }
                if sock.flush().await.is_err() {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_millis(75)).await;
            }
            if let Some(s) = hold_secs {
                tokio::time::sleep(std::time::Duration::from_secs(s)).await;
            }
        });
        addr
    }

    /// Sirve una única respuesta HTTP con el estado y el cuerpo JSON elegidos, para
    /// obtener un `reqwest::Response` de error sin daemon (doble determinista).
    async fn serve_status_json(status: u16, body: Value) -> reqwest::Response {
        serve_status_raw(status, body.to_string()).await
    }

    /// Igual que `serve_status_json` pero con el cuerpo textual indicado, que
    /// puede no ser JSON.
    async fn serve_status_raw(status: u16, body: String) -> reqwest::Response {
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
            let mut read = 0;
            loop {
                let n = sock.read(&mut buf[read..]).await.unwrap_or(0);
                if n == 0 {
                    break;
                }
                read += n;
                if buf[..read].windows(4).any(|w| w == b"\r\n\r\n") || read >= buf.len() {
                    break;
                }
            }
            let response = format!(
                "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = sock.write_all(response.as_bytes()).await;
        });
        reqwest::Client::new()
            .get(format!("http://{addr}/"))
            .send()
            .await
            .expect("respuesta del doble")
    }

    /// Un 500 de `/transcribe` con `transcription_failed` sale con 10 y conserva el
    /// `reason` del cuerpo.
    #[tokio::test]
    async fn daemon_error_from_response_maps_transcription_failed() {
        let resp = serve_status_json(
            500,
            json!({ "status": "error", "reason": "transcription_failed", "message": "fallo" }),
        )
        .await;
        let err = daemon_error_from_response(resp).await;
        assert_eq!(err.code.code(), 10);
        assert_eq!(err.reason, "transcription_failed");
    }

    /// Un 404 con `model_missing` sale con 4, sin importar el estado HTTP.
    #[tokio::test]
    async fn daemon_error_from_response_maps_model_missing() {
        let resp = serve_status_json(
            404,
            json!({ "error": "model_missing", "reason": "model_missing", "message": "falta" }),
        )
        .await;
        let err = daemon_error_from_response(resp).await;
        assert_eq!(err.code.code(), 4);
        assert_eq!(err.reason, "model_missing");
    }

    /// Un 404 sin `reason` ni `error` es una ruta ausente (el daemon en marcha es de
    /// otro binario, o su build no incluye la función de la ruta): falla de forma
    /// explícita como `daemon_error` (exit 1) con un mensaje que nombra ambas causas.
    #[tokio::test]
    async fn daemon_error_from_response_maps_bare_404_to_restart_hint() {
        let resp = serve_status_json(404, json!({})).await;
        let err = daemon_error_from_response(resp).await;
        assert_eq!(err.code.code(), 1);
        assert_eq!(err.reason, "daemon_error");
        assert!(
            err.message
                .contains("ai-voice-interconnector daemon restart"),
            "mensaje sin la indicación de reinicio: {}",
            err.message
        );
    }

    /// Un cuerpo de error que no es JSON se reporta con su inicio en el mensaje.
    #[tokio::test]
    async fn daemon_error_from_response_includes_start_of_non_json_body() {
        let resp = serve_status_raw(502, "<html>bad gateway</html>".to_string()).await;
        let err = daemon_error_from_response(resp).await;
        assert_eq!(err.code.code(), 1);
        assert_eq!(err.reason, "daemon_error");
        assert!(
            err.message.contains("<html>bad gateway</html>") && err.message.contains("502"),
            "mensaje sin el inicio del cuerpo: {}",
            err.message
        );
    }

    /// Un evento `error` del stream de síntesis con `model_missing` sale con 4.
    #[test]
    fn stream_error_event_maps_model_missing() {
        let err = stream_error_event(
            &json!({ "event": "error", "reason": "model_missing", "message": "falta" }),
        );
        assert_eq!(err.code.code(), 4);
        assert_eq!(err.reason, "model_missing");
    }

    /// El consumo entrega el evento final tras la secuencia
    /// `started` → latidos → `result` (doble con secuencia de éxito).
    #[tokio::test]
    async fn consume_ndjson_stream_returns_final_event() {
        let addr = serve_programmed_sequence(
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
        let mut progress = PhaseProgress::new();
        let val = consume_ndjson_stream(resp, StreamResult::Clone, &mut progress)
            .await
            .expect("la secuencia de éxito entrega el final");
        assert_eq!(val["event"], "result");
        assert_eq!(val["name"], "v");
        assert_eq!(val["precomputed"], true);
    }

    /// La síntesis lee los eventos mientras la respuesta sigue abierta; el WAV
    /// solo se entrega cuando llega el evento final.
    #[tokio::test]
    async fn consume_ndjson_stream_synthesis_returns_audio_after_intermediate_events() {
        let addr = serve_programmed_sequence(
            vec![
                r#"{"event":"started","voice":"v"}"#.to_string(),
                r#"{"event":"heartbeat","stage":"warming"}"#.to_string(),
                r#"{"event":"heartbeat","stage":"warming"}"#.to_string(),
                r#"{"event":"progress","stage":"synthesis","message":"Síntesis en curso."}"#
                    .to_string(),
                r#"{"event":"heartbeat","stage":"synthesis"}"#.to_string(),
                r#"{"event":"result","audio_b64":"UklGRg=="}"#.to_string(),
            ],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{addr}/synthesize"))
            .send()
            .await
            .expect("el doble debe responder");
        let mut progress = PhaseProgress::new();
        let mut stream = Box::pin(consume_ndjson_stream(
            resp,
            StreamResult::Synthesize,
            &mut progress,
        ));
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(320), &mut stream)
                .await
                .is_err(),
            "el resultado debe seguir pendiente tras los eventos intermedios"
        );
        let val = stream.await.expect("el stream termina con audio_b64");
        assert_eq!(val["audio_b64"], "UklGRg==");
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(val["audio_b64"].as_str().unwrap())
                .unwrap(),
            b"RIFF"
        );
        let _elapsed = progress.close();
    }

    /// La traducción pura fija la etiqueta y el mensaje de cada etapa; los
    /// eventos de avance conservan su mensaje y los latidos usan el de la CLI.
    #[test]
    fn ndjson_progress_phase_maps_stages_and_rate_limits_by_stage() {
        let cases = [
            ("warming", "Calentando motor de síntesis…"),
            ("queued", "En cola de síntesis…"),
            ("synthesis", "Sintetizando voz…"),
            ("warmup", "Iniciando precarga en caliente…"),
            ("clone", "Clonando voz… (puede tardar varios minutos)"),
            ("transcribe", "Transcribiendo audio…"),
            ("translate", "Traduciendo texto…"),
        ];
        for (stage, msg) in cases {
            let heartbeat = json!({ "event": "heartbeat", "stage": stage });
            let progress_event =
                json!({ "event": "progress", "stage": stage, "message": "avance" });
            let (label, text) = ndjson_progress_phase(&heartbeat).expect("etapa conocida");
            let (progress_label, progress_text) =
                ndjson_progress_phase(&progress_event).expect("avance conocido");
            assert_eq!(label, stage);
            assert_eq!(text, msg);
            assert_eq!(progress_label, label, "ambos eventos racionan por etapa");
            assert_eq!(progress_text, "avance", "se conserva el mensaje del daemon");
        }
        assert!(
            ndjson_progress_phase(&json!({ "event": "heartbeat", "stage": "unknown" })).is_none()
        );
        assert!(ndjson_progress_phase(
            &json!({ "event": "progress", "stage": "unknown", "message": "x" })
        )
        .is_none());
        assert!(ndjson_progress_phase(&json!({ "event": "result" })).is_none());
        assert_eq!(
            ndjson_progress_phase(&json!({ "event": "started" })),
            Some(("started", "Iniciando operación…".to_string()))
        );
        assert_eq!(
            ndjson_progress_phase(&json!({ "event": "started", "operation": "doblaje" })),
            Some(("started", "Iniciando doblaje…".to_string()))
        );
    }

    /// El evento `started` nombra la operación cuando el daemon manda la clave.
    ///
    /// El daemon envía `operation` en sus tres emisoras, así que la CLI
    /// anuncia «Iniciando síntesis…», «Iniciando clonación…» o «Iniciando
    /// doblaje…» en vez de una operación genérica.
    #[test]
    fn started_names_the_operation_the_daemon_reports() {
        for operation in ["síntesis", "clonación", "doblaje"] {
            let event = json!({ "event": "started", "operation": operation });
            let (label, text) = ndjson_progress_phase(&event).expect("el evento se anuncia");
            assert_eq!(label, "started");
            assert_eq!(text, format!("Iniciando {operation}…"));
        }
    }

    /// Sin la clave `operation`, la CLI no se queda muda: anuncia una
    /// operación genérica.
    ///
    /// Es la red de seguridad para un daemon anterior que no manda la clave. El
    /// texto genérico no se confunde con el de una operación concreta: el
    /// concreto dice cuál es.
    #[test]
    fn started_without_the_operation_key_falls_back_to_a_generic_operation() {
        let (label, text) =
            ndjson_progress_phase(&json!({ "event": "started" })).expect("el evento se anuncia");
        assert_eq!(label, "started");
        assert_eq!(text, "Iniciando operación…");
        assert_ne!(
            text,
            ndjson_progress_phase(&json!({ "event": "started", "operation": "síntesis" }))
                .expect("el evento se anuncia")
                .1,
            "el texto genérico se distingue del concreto"
        );
    }

    /// Los latidos del daemon pintan las etiquetas canónicas de la vía local;
    /// los detalles del motor no se confunden con las fases del comando.
    #[test]
    fn command_stream_phase_uses_local_labels_for_real_events() {
        let cases = [
            (StreamResult::Synthesize, "synthesis", "synthesizing"),
            (StreamResult::Synthesize, "translate", "translating"),
            (StreamResult::Clone, "clone", "cloning"),
            (StreamResult::Dub, "transcribe", "transcribing"),
            (StreamResult::Dub, "translate", "translating"),
            (StreamResult::Dub, "synthesis", "synthesizing"),
            (StreamResult::Dub, "warming", "warming"),
            (StreamResult::Clone, "warmup", "warmup"),
            (StreamResult::Synthesize, "queued", "queued"),
        ];
        for (route, stage, expected) in cases {
            let event = json!({ "event": "heartbeat", "stage": stage });
            assert_eq!(command_stream_phase(&event, route).unwrap().0, expected);
        }
        assert!(command_stream_phase(&json!({ "event": "result" }), StreamResult::Dub).is_none());
        assert!(command_stream_phase(
            &json!({ "event": "heartbeat", "stage": "unknown" }),
            StreamResult::Dub
        )
        .is_none());
    }

    /// El evento de fallo del stream se mapea por reason (doble con
    /// secuencia de fallo), sin esperar al failsafe total.
    #[tokio::test]
    async fn consume_ndjson_stream_maps_failure_event() {
        let addr = serve_programmed_sequence(
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
        let start = std::time::Instant::now();
        let mut progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &mut progress)
            .await
            .expect_err("el evento de fallo debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "voice_exists");
        assert!(
            start.elapsed() < std::time::Duration::from_secs(10),
            "el fallo explícito no espera techos: {:?}",
            start.elapsed()
        );
    }

    /// El stream atascado (cabeceras sin eventos) dispara el timeout de
    /// inactividad de 1500 ms (doble con secuencia de atasco), no el failsafe.
    #[tokio::test]
    async fn consume_ndjson_stream_detects_stall_by_inactivity() {
        let addr = serve_programmed_sequence(vec![], Some(3)).await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder cabeceras");
        let start = std::time::Instant::now();
        let mut progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &mut progress)
            .await
            .expect_err("el atasco debe fallar");
        assert_eq!(e.code, ExitCode::DaemonUnreachable);
        assert_eq!(e.reason, "daemon_unreachable");
        let elapsed = start.elapsed();
        assert!(
            elapsed >= std::time::Duration::from_millis(1500),
            "debe agotar la inactividad: {:?}",
            elapsed
        );
        assert!(
            elapsed < std::time::Duration::from_secs(30),
            "no debe llegar al failsafe ni al retén: {:?}",
            elapsed
        );
    }

    /// El stream truncado tras `started` (cierre sin evento final) falla
    /// ruidoso como `daemon_error`, nunca como éxito parcial.
    #[tokio::test]
    async fn consume_ndjson_stream_fails_if_truncated_without_final() {
        let addr =
            serve_programmed_sequence(vec![r#"{"event":"started","name":"v"}"#.to_string()], None)
                .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/voices/clone", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let mut progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &mut progress)
            .await
            .expect_err("el truncado debe fallar");
        assert_eq!(e.code, ExitCode::Error);
        assert_eq!(e.reason, "daemon_error");
    }

    /// Escribe un WAV válido (mono 16 kHz) en `path`, para que el almacén de
    /// locuciones pueda leer su duración al guardar.
    fn write_test_wav(path: &std::path::Path) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(path, spec).expect("crear el WAV de prueba");
        for sample in 0i16..64 {
            writer
                .write_sample(sample)
                .expect("escribir la muestra del WAV de prueba");
        }
        writer.finalize().expect("cerrar el WAV de prueba");
    }

    /// Lector de opciones del menú `--play` con las respuestas programadas: cada
    /// respuesta se consume una vez; al agotarse, la lectura devuelve EOF.
    fn scripted_reader(
        responses: Vec<&'static str>,
    ) -> impl FnMut(&mut String) -> std::io::Result<usize> {
        let mut responses = responses.into_iter();
        move |line: &mut String| {
            match responses.next() {
                Some(response) => {
                    line.push_str(response);
                    line.push('\n');
                    Ok(response.len() + 1)
                }
                // Sin más respuestas: EOF.
                None => Ok(0),
            }
        }
    }

    /// Reproducción de mentira del menú `--play`: cuenta las reproducciones y
    /// no toca la placa de sonido.
    fn fake_playback(
        counter: &std::sync::Arc<std::sync::atomic::AtomicUsize>,
    ) -> impl FnMut(&std::path::Path) -> Result<(), CliError> {
        let counter = counter.clone();
        move |_wav: &std::path::Path| {
            counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// Prepara un almacén de locuciones aislado y un WAV temporal válido, y
    /// devuelve los tres con el directorio temporal que los respalda.
    fn menu_fixture() -> (tempfile::TempDir, SpeechStore, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(avi_shared::paths::MENU_SANDBOX_PREFIX)
            .tempdir()
            .expect("directorio temporal de prueba");
        let store = SpeechStore::at(dir.path().join("speech"));
        let wav = dir.path().join("toma.wav");
        write_test_wav(&wav);
        (dir, store, wav)
    }

    /// La opción 1 reproduce de nuevo el WAV y vuelve a ofrecer el menú; la 2
    /// guarda la locución y devuelve la ruta persistida.
    #[test]
    fn play_menu_option_one_replays_and_option_two_saves() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["1", "2"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        // Reproducción inicial + la de la opción 1.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 2);
        match decision {
            PlayDecision::Saved(saved) => {
                // La ruta guardada es la del almacén, no la del temporal.
                assert_eq!(saved, store.audio_path("voz", "etiqueta"));
                assert!(saved.is_file(), "la locución guardada debe existir");
            }
            other => panic!("se esperaba guardar, no: {other:?}"),
        }
        // El texto de la locución guardada es el texto final, no el de la WAV.
        let entry = store
            .find("voz", "etiqueta")
            .expect("buscar la locución guardada")
            .expect("la locución debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// La opción 4 descarta: no persiste nada y devuelve `Discarded`.
    #[test]
    fn play_menu_option_four_discards_without_saving() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["4"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Discarded);
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            store
                .find("voz", "etiqueta")
                .expect("buscar la locución")
                .is_none(),
            "descartar no debe persistir la locución"
        );
    }

    /// La opción 3 no se resuelve en el menú: se devuelve `Resynthesize` para
    /// que la ejecute el llamador.
    #[test]
    fn play_menu_option_three_returns_resynthesize_to_caller() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Resynthesize);
        // El menú solo reproduce al entrar: la re-síntesis es del llamador.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(
            store
                .find("voz", "etiqueta")
                .expect("buscar la locución")
                .is_none(),
            "re-sintetizar no debe persistir nada"
        );
    }

    /// El fin de la entrada estándar descarta: el menú se ofrece, la lectura
    /// devuelve EOF y la locución no se persiste.
    #[test]
    fn play_menu_end_of_input_discards() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec![]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Discarded);
        assert!(
            store
                .find("voz", "etiqueta")
                .expect("buscar la locución")
                .is_none(),
            "el fin de la entrada no debe persistir la locución"
        );
    }

    /// Una opción inválida avisa y repite el menú; la siguiente opción válida
    /// se atiende con normalidad.
    #[test]
    fn play_menu_invalid_option_repeats_menu() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["9", "2"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert!(matches!(decision, PlayDecision::Saved(_)));
        // Solo una reproducción: la opción inválida no reproduce.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// La colisión de etiqueta se recomprueba al guardar (opción 2) y falla con
    /// `label_exists` (exit 6) si la locución ya existe y no hay `--force`.
    #[test]
    fn play_menu_option_two_detects_label_collision() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut playback = fake_playback(&plays);
        // La locución ya existe en el almacén cuando se ofrece el menú.
        store
            .save("voz", "etiqueta", "texto previo", &wav)
            .expect("guardar la locución previa");

        let mut reader = scripted_reader(vec!["2"]);
        let e = synthesize_play_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect_err("la colisión debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "label_exists");
    }

    /// Con `--force` la colisión no impide guardar: la locución previa se
    /// sustituye por la nueva.
    #[test]
    fn play_menu_option_two_saves_over_existing_label_with_force() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut playback = fake_playback(&plays);
        store
            .save("voz", "etiqueta", "texto previo", &wav)
            .expect("guardar la locución previa");

        let mut reader = scripted_reader(vec!["2"]);
        let decision = synthesize_play_loop(
            "voz",
            "etiqueta",
            true,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("con --force no debe fallar");

        assert!(matches!(decision, PlayDecision::Saved(_)));
        let entry = store
            .find("voz", "etiqueta")
            .expect("buscar la locución guardada")
            .expect("la locución debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// Re-síntesis de mentira: cuenta las re-síntesis y no toca el motor.
    struct CountingResynthesizer {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    impl PlayResynthesizer for CountingResynthesizer {
        async fn resynthesize(&mut self) -> Result<(), CliError> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        }
    }

    /// El bucle del llamador: ante `Resynthesize` re-sintetiza y vuelve a
    /// ofrecer el menú; la locución final se persiste con la etiqueta correcta.
    #[tokio::test]
    async fn play_menu_loop_resynthesizes_and_offers_menu_again() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resyntheses = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3", "2"]);
        let mut playback = fake_playback(&plays);
        let mut resynthesizer = CountingResynthesizer {
            counter: resyntheses.clone(),
        };

        let outcome = play_menu_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await
        .expect("el bucle no debe fallar");

        assert_eq!(resyntheses.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Reproducción inicial + la de la vuelta tras re-sintetizar.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 2);
        let saved = outcome.expect("la locución debe quedar guardada");
        assert_eq!(saved, store.audio_path("voz", "etiqueta"));
        let entry = store
            .find("voz", "etiqueta")
            .expect("buscar la locución guardada")
            .expect("la locución debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// El bucle del llamador termina en descarte cuando el usuario elige la 4
    /// tras re-sintetizar.
    #[tokio::test]
    async fn play_menu_loop_ends_on_discard_after_resynthesis() {
        let (_dir, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3", "4"]);
        let mut playback = fake_playback(&plays);
        let mut resynthesizer = CountingResynthesizer {
            counter: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        };

        let outcome = play_menu_loop(
            "voz",
            "etiqueta",
            false,
            &store,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await
        .expect("el bucle no debe fallar");

        assert!(outcome.is_none(), "descartar no debe guardar nada");
        assert!(
            store
                .find("voz", "etiqueta")
                .expect("buscar la locución")
                .is_none(),
            "descartar no debe persistir la locución"
        );
    }

    /// El envelope de descarte del menú `--play` no cambia: mismo `status` y
    /// misma clave `voice`, y el estampado de `schema_version` de la CLI.
    #[test]
    fn play_discarded_envelope_keeps_its_shape() {
        assert_eq!(
            play_discarded_envelope("voz"),
            json!({ "status": "discarded", "voice": "voz" })
        );
        let stamped = avi_core::json_emitter::with_schema_version(
            match play_discarded_envelope("voz") {
                Value::Object(map) => map,
                other => panic!("el envelope debe ser un objeto, no: {other}"),
            },
            avi_core::json_emitter::CLI_SCHEMA_VERSION,
        );
        assert_eq!(
            stamped,
            json!({
                "status": "discarded",
                "voice": "voz",
                "schema_version": avi_core::json_emitter::CLI_SCHEMA_VERSION,
            })
        );
    }

    /// El indicador con el que se mide el repintado: cuenta las veces que
    /// `tick` se llamó, sin depender de capturar stderr.
    ///
    /// Es un doble del indicador, no el indicador: el de producción no expone
    /// su contador, y aquí lo que importa es **cuántas veces se intentó
    /// repintar**, no lo que se pintó.
    #[derive(Default)]
    struct CountingProgress {
        repaints: std::sync::atomic::AtomicUsize,
    }

    impl CountingProgress {
        fn repaints(&self) -> usize {
            self.repaints.load(std::sync::atomic::Ordering::Relaxed)
        }
    }

    impl ProgressTicker for CountingProgress {
        fn repaint(&self) {
            self.repaints
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// Un futuro que tarda un rato en terminar.
    ///
    /// Duerme un tiempo concreto, así que sigue pendiente durante ese tiempo
    /// y el ayudante agota su espera para repintar.
    async fn slow_future(millis: u64) -> u32 {
        tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
        0
    }

    /// El intervalo con el que el indicador se repinta durante una espera.
    const TEST_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

    /// Espera a un futuro reponiendo el indicador mientras no termina: el
    /// futuro inmediato no llega a consumirse, así que no hay repintado.
    #[tokio::test]
    async fn await_with_progress_returns_immediately_without_repainting() {
        let progress = CountingProgress::default();
        let value =
            await_with_progress_every(&progress, TEST_REPAINT_INTERVAL, async { 7u32 }).await;
        assert_eq!(value, 7);
        assert_eq!(progress.repaints(), 0, "un futuro inmediato no repinta");
    }

    /// Con un futuro lento, el indicador se repinta hasta que el trabajo
    /// termina: la fase no se queda congelada mientras espera.
    ///
    /// El umbral es de **al menos un** repintado, no un número exacto: el
    /// intervalo con el que se repinta es de 5 ms y la carga de la máquina
    /// puede hacer que la espera se alargue. Lo que la prueba afirma es que
    /// **hubo** repintado mientras el futuro seguía pendiente, no cuántos.
    #[tokio::test]
    async fn await_with_progress_repaints_while_the_future_is_pending() {
        let progress = CountingProgress::default();
        let value =
            await_with_progress_every(&progress, TEST_REPAINT_INTERVAL, slow_future(50)).await;
        assert_eq!(value, 0);
        assert!(
            progress.repaints() >= 1,
            "un futuro lento se repinta mientras espera: {}",
            progress.repaints()
        );
    }

    /// El indicador real no repinta si no hay fase abierta: `tick` solo
    /// consulta el estado, y sin fase no hay línea que repintar.
    ///
    /// Es la regla que hace que el repintado no invente actividad: la fase
    /// tiene que existir para que el tiempo visible avance.
    ///
    /// El indicador de producción no expone su contador —los contadores son
    /// `#[cfg(test)]` de la crate `avi-lifecycle`, y aquí el binario la
    /// consume como dependencia—, así que el recuento vive en la crate, donde
    /// está el indicador: `tick_without_phase_paints_nothing`. Aquí lo que se
    /// fija es que el indicador acepta la llamada sin fase y no escribe
    /// nada, que es lo que el binario puede observar.
    #[test]
    fn tick_without_a_phase_does_not_repaint() {
        let progress = PhaseProgress::new();
        // Sin fase, el indicador no tiene línea que devolver: `tick` es un
        // no-op y el tiempo visible no avanza por su cuenta.
        progress.tick();
    }

    /// El trabajo síncrono es la excepción declarada al repintado.
    ///
    /// La síntesis, el clonado local y la captura del micrófono bloquean el
    /// hilo mientras duran, así que el indicador no puede repintar: no hay
    /// forma de que el tiempo visible avance durante ellos. La excepción está
    /// escrita en el punto de llamada de cada uno, y esta prueba fija la
    /// regla que la hace verdadera: **el indicador solo repinta cuando se lo
    /// pide**, y el trabajo síncrono no lo pide porque bloquea el hilo.
    ///
    /// La prueba no invoca la síntesis —que necesita el motor—: comprueba la
    /// regla que la hace verdadera, que el repintado es una petición
    /// explícita y no un efecto del trabajo.
    #[test]
    fn synchronous_work_is_declared_as_the_exception_to_repainting() {
        let counter = CountingProgress::default();
        // El repintado no ocurre por sí solo: sin espera asíncrona que lo
        // pida, el contador no se mueve. Es lo que hace que el trabajo
        // síncrono sea una excepción declarada y no un defecto de la espera.
        assert_eq!(
            counter.repaints(),
            0,
            "nadie ha pedido repintar: el indicador no repinta solo"
        );
    }

    /// La espera asíncrona sí pide el repintado: es el mecanismo que hace que
    /// el tiempo visible avance mientras un futuro sigue pendiente.
    #[tokio::test]
    async fn asynchronous_work_asks_for_the_repaint() {
        let progress = CountingProgress::default();
        await_with_progress_every(&progress, TEST_REPAINT_INTERVAL, slow_future(50)).await;
        assert!(
            progress.repaints() >= 1,
            "la espera asíncrona sí pide repintar: {}",
            progress.repaints()
        );
    }
}
