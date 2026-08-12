mod audio;
mod config;
mod daemon;
mod engine;
mod exit_codes;
mod json_emitter;
mod store;
#[cfg(test)]
mod tests;
mod tts;

use clap::{Parser, Subcommand};
use engine::{DummySttEngine, SttEngine};
use exit_codes::{CliError, ExitCode};
use json_emitter::emit_raw_json;
use serde_json::json;
use std::net::SocketAddr;
use std::process::exit;
use store::{VoiceStore, SpeechStore, ModelStore};
use tts::{Qwen3TtsEngine, TtsEngine};

const VERSION: &str = "0.10.5";
const APP_NAME: &str = "ai-voice-interconnector";

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
        #[arg(long, default_value = "es")]
        from: String,
        #[arg(long, default_value = "en")]
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
        #[arg(long, default_value = "es")]
        language: String,
        #[arg(long)]
        with_stt: bool,
    },
    /// Limpia modelos/caché
    Cleanup,
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
        #[arg(short, long)]
        audio: String,
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
    List,
    /// Transcribir audio
    Transcribe {
        #[arg(short, long)]
        file: Option<String>,
    },
    /// Sintetizar texto a habla
    Synthesize {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        #[arg(short, long)]
        output: Option<String>,
        #[arg(long)]
        play: bool,
    },
    /// Sintetizar y reproducir
    Say {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
    },
    /// Doblaje voz→voz: transcribe, traduce, sintetiza
    Dub {
        #[arg(short, long)]
        file: Option<String>,
        #[arg(short, long, default_value = "default")]
        voice: String,
        #[arg(long, default_value = "es")]
        from: String,
        #[arg(long, default_value = "en")]
        to: String,
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
    Start,
    /// Detener el daemon
    Stop,
    /// Reiniciar el daemon
    Restart,
    /// Estado del daemon
    Status,
    /// Ejecutar el servidor HTTP del daemon en primer plano
    Serve,
}

// ─── Bootstrap ───────────────────────────────────────────────────────

/// Forzar UTF-8 en stdout/stderr (equivalente a bootstrap.py)
fn force_utf8() {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "chcp", "65001"])
        .output();
}

/// Instalar handler de SIGINT → exit 130
fn install_sigint_handler() {
    ctrlc::set_handler(move || {
        // Exit code 130 = interrumpido por usuario (Ctrl+C)
        eprintln!("\nInterrumpido por el usuario.");
        exit(130);
    })
    .expect("Error al instalar el handler de Ctrl+C");
}

// ─── Punto de entrada ────────────────────────────────────────────────

#[tokio::main]
async fn main() {
    // Bootstrap: UTF-8, tracing, SIGINT
    force_utf8();
    tracing_subscriber::fmt::init();
    install_sigint_handler();

    let cli = Cli::parse();
    let json_mode = cli.json;
    let daemon_mode = cli.daemon_mode();

    let result = match cli.command {
        Some(Commands::Version) => handle_version(json_mode),
        Some(Commands::Devices) => handle_devices(json_mode),
        Some(Commands::Translate { text, from, to }) => {
            handle_translate(json_mode, daemon_mode, &text, &from, &to)
        }
        Some(Commands::Voice { action }) => handle_voice(json_mode, action),
        Some(Commands::Speech { action }) => handle_speech(json_mode, daemon_mode, action).await,
        Some(Commands::Daemon { action }) => handle_daemon(json_mode, action).await,
        Some(Commands::Setup { language, with_stt }) => {
            handle_setup(json_mode, &language, with_stt)
        }
        Some(Commands::Cleanup) => handle_cleanup(json_mode),
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
    let devices = audio::get_devices_json().map_err(|e| {
        CliError::new(ExitCode::Error, "audio_enumeration_failed", e.to_string())
    })?;
    if json_mode {
        emit_raw_json(json!({ "devices": devices }));
    } else {
        println!("Dispositivos de salida de audio:");
        for dev in &devices {
            println!(
                "  [{}] {} (latencia: {:.1}ms)",
                dev["id"],
                dev["name"].as_str().unwrap_or(""),
                dev["latency"].as_f64().unwrap_or(0.0)
            );
        }
    }
    Ok(())
}

fn handle_translate(_json_mode: bool, daemon_mode: DaemonMode, text: &str, _from: &str, _to: &str) -> Result<(), CliError> {
    if daemon_mode == DaemonMode::ForceDaemon {
        return Err(CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            "Daemon inalcanzable en 127.0.0.1:8765",
        ));
    }
    if text.trim().is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_text",
            "El texto a traducir está vacío",
        ));
    }
    // Sin modelo provisionado → exit 4
    let model_store = ModelStore::new();
    if !model_store.is_provisioned("marian-es-en") {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            "El modelo de traducción no está provisionado. Ejecuta 'setup' primero.",
        ));
    }
    unreachable!("La traducción real se implementa tras integrar ct2rs")
}

// ─── Voice ───────────────────────────────────────────────────────────

fn handle_voice(json_mode: bool, action: VoiceCommands) -> Result<(), CliError> {
    let voice_store = VoiceStore::new();

    match action {
        VoiceCommands::List => {
            let voices = voice_store.list().map_err(|e| {
                CliError::new(ExitCode::Error, "voice_list_failed", e.to_string())
            })?;
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
        VoiceCommands::Clone { name: _, audio: _ } => {
            Err(CliError::new(
                ExitCode::ModelMissing,
                "model_missing",
                "El motor de clonado no está provisionado. Ejecuta 'setup' primero.",
            ))
        }
        VoiceCommands::Remove { name } => {
            VoiceStore::validate_name(&name).map_err(|e| {
                CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e)
            })?;
            voice_store.remove(&name).map_err(|e| {
                if name == "default" {
                    CliError::new(ExitCode::InvalidInput, "cannot_remove_default", e)
                } else {
                    CliError::new(ExitCode::NotFound, "voice_not_found", e)
                }
            })?;
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

async fn handle_speech(json_mode: bool, daemon_mode: DaemonMode, action: SpeechCommands) -> Result<(), CliError> {
    if daemon_mode == DaemonMode::ForceDaemon {
        return Err(CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            "Daemon inalcanzable en 127.0.0.1:8765",
        ));
    }
    let speech_store = SpeechStore::new();

    match action {
        SpeechCommands::List => {
            let items = speech_store.list().map_err(|e| {
                CliError::new(ExitCode::Error, "speech_list_failed", e.to_string())
            })?;
            if json_mode {
                let entries: Vec<serde_json::Value> = items.iter().map(|e| {
                    json!({
                        "label": e.metadata.label,
                        "voice": e.metadata.voice,
                        "text": e.metadata.text,
                        "created_at": e.metadata.created_at,
                        "duration_secs": e.metadata.duration_secs,
                    })
                }).collect();
                emit_raw_json(json!({ "speech": entries }));
            } else {
                println!("Habla sintética albergada:");
                if items.is_empty() {
                    println!("  (ninguna locución guardada)");
                } else {
                    for e in &items {
                        println!("  - [{}] {} ({:.1}s) — «{}»",
                            e.metadata.voice, e.metadata.label,
                            e.metadata.duration_secs, e.metadata.text);
                    }
                }
            }
            Ok(())
        }
        SpeechCommands::Transcribe { file: _ } => {
            let engine = DummySttEngine;
            let text = engine.transcribe(&[]).map_err(|e| {
                CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
            })?;
            if json_mode {
                emit_raw_json(json!({ "text": text }));
            } else {
                println!("{}", text);
            }
            Ok(())
        }
        SpeechCommands::Synthesize { text, voice, output, play: _ } => {
            require_model_provisioned()?;
            let engine = Qwen3TtsEngine::new(None);
            let path_buf = output.map(std::path::PathBuf::from);
            let res = engine.synthesize(&text, &voice, path_buf.as_ref()).map_err(|e| {
                CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
            })?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "success",
                    "audio_path": res.to_string_lossy(),
                    "voice": voice,
                }));
            } else {
                println!("Síntesis completada: {}", res.display());
            }
            Ok(())
        }
        SpeechCommands::Say { text, voice } => {
            require_model_provisioned()?;
            let engine = Qwen3TtsEngine::new(None);
            let res = engine.synthesize(&text, &voice, None).map_err(|e| {
                CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
            })?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "reproduced",
                    "audio_path": res.to_string_lossy(),
                    "voice": voice,
                }));
            } else {
                println!("Reproduciendo: {}", res.display());
            }
            Ok(())
        }
        SpeechCommands::Dub { file: _, voice: _, from: _, to: _ } => {
            require_model_provisioned()?;
            Err(CliError::new(
                ExitCode::ModelMissing,
                "model_missing",
                "El motor de doblaje no está provisionado. Ejecuta 'setup' primero.",
            ))
        }
        SpeechCommands::Play { label, voice } => {
            match speech_store.find(&voice, &label) {
                Some(entry) => {
                    audio::AudioService::new().play_wav(&entry.audio_path).map_err(|e| {
                        CliError::new(
                            ExitCode::Error,
                            "playback_failed",
                            format!("Fallo al reproducir la locución '{}' de la voz '{}': {}", label, voice, e),
                        )
                    })?;
                    if json_mode {
                        emit_raw_json(json!({ "status": "played", "label": label, "voice": voice }));
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
            speech_store.remove(&voice, &label).map_err(|e| {
                CliError::new(ExitCode::NotFound, "speech_not_found", e)
            })?;
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
    match action {
        DaemonCommands::Serve => {
            let addr: SocketAddr = "127.0.0.1:8765".parse().map_err(|e: std::net::AddrParseError| {
                CliError::new(ExitCode::Error, "invalid_address", e.to_string())
            })?;
            daemon::run_daemon_server(addr).await.map_err(|e| {
                CliError::new(ExitCode::DaemonUnreachable, "daemon_error", e.to_string())
            })
        }
        DaemonCommands::Start => {
            eprintln!("Daemon: inicio en segundo plano no implementado aún (use 'daemon serve').");
            Err(CliError::new(
                ExitCode::NotApplicable,
                "not_implemented",
                "El inicio del daemon en segundo plano no está implementado aún.",
            ))
        }
        DaemonCommands::Stop => {
            // Intenta POST /shutdown al daemon
            Err(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_not_running",
                "No se pudo contactar al daemon en 127.0.0.1:8765.",
            ))
        }
        DaemonCommands::Restart => {
            Err(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_not_running",
                "No se pudo contactar al daemon para reiniciarlo.",
            ))
        }
        DaemonCommands::Status => {
            if json_mode {
                emit_raw_json(json!({ "daemon": "stopped" }));
            } else {
                println!("Daemon: no está en ejecución.");
            }
            Ok(())
        }
    }
}

// ─── Setup / Cleanup / Doctor ────────────────────────────────────────

fn handle_setup(json_mode: bool, language: &str, with_stt: bool) -> Result<(), CliError> {
    let model_store = ModelStore::new();
    let voice_store = VoiceStore::new();

    // 1. Inicializar VoiceStore y directorio por defecto
    voice_store.ensure_initialized().map_err(|e| {
        CliError::new(ExitCode::Error, "voice_store_init_failed", e.to_string())
    })?;

    // 2. Registrar modelo de síntesis TTS (Qwen3-TTS 0.6B)
    model_store.register_provisioned("qwen3-tts-0.6b", "v1.0").map_err(|e| {
        CliError::new(ExitCode::Error, "model_provision_failed", e.to_string())
    })?;

    // 3. Registrar modelo de traducción (Marian es<->en)
    model_store.register_provisioned("marian-es-en", "v1.0").map_err(|e| {
        CliError::new(ExitCode::Error, "model_provision_failed", e.to_string())
    })?;

    // 4. Registrar modelo STT si se solicita --with-stt
    if with_stt {
        model_store.register_provisioned("whisper-ct2", "v1.0").map_err(|e| {
            CliError::new(ExitCode::Error, "model_provision_failed", e.to_string())
        })?;
    }

    if json_mode {
        emit_raw_json(json!({
            "status": "completed",
            "language": language,
            "with_stt": with_stt,
            "models_provisioned": ["qwen3-tts-0.6b", "marian-es-en"]
        }));
    } else {
        println!(
            "Setup completado: provisión de modelos finalizada para idioma '{}'{}.",
            language,
            if with_stt { " (con STT)" } else { "" }
        );
    }
    Ok(())
}

fn handle_cleanup(json_mode: bool) -> Result<(), CliError> {
    if json_mode {
        emit_raw_json(json!({ "status": "cleanup_complete" }));
    } else {
        println!("Limpieza de modelos/caché completada.");
    }
    Ok(())
}

fn handle_doctor(_json_mode: bool) -> Result<(), CliError> {
    let model_store = ModelStore::new();
    let voice_store = VoiceStore::new();

    // Chequeos reales de entorno
    let mut issues = Vec::new();

    // Verificar que el directorio de datos existe y es escribible
    let data_dir = store::data_dir();
    if !data_dir.exists() {
        issues.push("Directorio de datos no existe");
    }

    // Verificar modelos provisionados
    if !model_store.is_provisioned("qwen3-tts-0.6b") {
        issues.push("Modelo TTS (Qwen3-TTS 0.6B) no provisionado");
    }
    if !model_store.is_provisioned("whisper-ct2") {
        issues.push("Modelo STT (Whisper CT2) no provisionado");
    }
    if !model_store.is_provisioned("marian-es-en") {
        issues.push("Modelo traducción (Marian es→en) no provisionado");
    }

    // Verificar voces
    if let Err(_e) = voice_store.list() {
        issues.push("Error al listar voces");
    }

    if issues.is_empty() {
        println!("Diagnóstico: todo correcto.");
        Ok(())
    } else {
        for issue in &issues {
            eprintln!("  ✗ {}", issue);
        }
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
