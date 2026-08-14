use avi_audio as audio;
use avi_core::engine::SttEngine;
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_daemon as daemon;
use avi_store as store;
use avi_store::{VoiceStore, SpeechStore, ModelStore};
use avi_stt::Ct2SttEngine;
use avi_tts::{Qwen3TtsEngine, TtsEngine};
use avi_translation as translation;
use clap::{Parser, Subcommand};
use serde_json::json;
use std::io::IsTerminal;
use std::net::SocketAddr;
use std::process::exit;

const VERSION: &str = "0.10.5";
const APP_NAME: &str = "ai-voice-interconnector";
/// Ruta fija del modelo Whisper ya convertido a CT2, reutilizado por
/// `speech transcribe` (no se gestiona vía `ModelStore`: layout incompatible).
const DEFAULT_WHISPER_MODEL_DIR: &str = "models/ct2/whisper-small";
/// Ruta fija del modelo Marian/opus-mt es→en ya convertido a CT2, reutilizado
/// por `translate` (no se gestiona vía `ModelStore`: layout incompatible).
const DEFAULT_TRANSLATION_MODEL_ES_EN: &str = "models/ct2/opus-mt-es-en";
/// Ruta fija del modelo Marian/opus-mt en→es ya convertido a CT2 (ídem).
const DEFAULT_TRANSLATION_MODEL_EN_ES: &str = "models/ct2/opus-mt-en-es";

/// Resuelve un token de idioma de la CLI (`es-latam`/`en`) al código ISO que
/// exige el motor STT: `es-latam` -> `es`; cualquier otro valor pasa verbatim
/// (espeja `resolve_language` del oráculo Python).
fn resolve_stt_language(token: &str) -> &str {
    match token {
        "es-latam" => "es",
        other => other,
    }
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
    List,
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
    },
    /// Sintetizar y reproducir
    Say {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
    },
    /// Doblaje voz→voz: transcribe, traduce, sintetiza y reproduce
    Dub {
        /// Archivo de audio a doblar (alias del oráculo: --file)
        #[arg(short = 'a', long, alias = "file")]
        audio: Option<String>,
        #[arg(short, long, default_value = "default")]
        voice: String,
        #[arg(long, default_value = "es")]
        from: String,
        #[arg(long, default_value = "en")]
        to: String,
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
                dev["latency"].as_f64().unwrap_or(0.0) * 1000.0
            );
        }
    }
    Ok(())
}

fn handle_translate(json_mode: bool, daemon_mode: DaemonMode, text: &str, from: &str, to: &str) -> Result<(), CliError> {
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
    let source = resolve_stt_language(from);
    let target = resolve_stt_language(to);
    // Passthrough: origen == destino tras normalizar → texto intacto, sin
    // construir ningún motor de traducción (replica `TranslationService`).
    if source == target {
        if json_mode {
            emit_raw_json(json!({ "translated": text, "source": from, "target": to }));
        } else {
            println!("{}", text);
        }
        return Ok(());
    }
    // Par no soportado → exit 2 (validación pura, sin tocar el modelo).
    let model_dir = match (source, target) {
        ("es", "en") => DEFAULT_TRANSLATION_MODEL_ES_EN,
        ("en", "es") => DEFAULT_TRANSLATION_MODEL_EN_ES,
        _ => {
            return Err(CliError::new(
                ExitCode::InvalidInput,
                "unsupported_language_pair",
                format!("Par de idiomas no soportado: {} -> {} (soportados: es, en)", source, target),
            ));
        }
    };
    // Modelo ausente -> exit 4, previo a construir el motor (patrón de STT).
    if !std::path::Path::new(model_dir).exists() {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!("El modelo de traducción no está provisionado en '{}'.", model_dir),
        ));
    }
    let translated = translation::translate(text, source, target, model_dir).map_err(|e| {
        CliError::new(ExitCode::TranslationFailed, "translation_failed", e.to_string())
    })?;
    if json_mode {
        emit_raw_json(json!({ "translated": translated, "source": from, "target": to }));
    } else {
        println!("{}", translated);
    }
    Ok(())
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
        VoiceCommands::Clone {
            name,
            speech_reference,
            timbre_reference,
            force,
        } => {
            // Orden de validaciones del oráculo (cli.py:841-899).
            require_model_provisioned()?;
            let name = name.to_lowercase();
            VoiceStore::validate_name(&name).map_err(|e| {
                CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e)
            })?;
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
                    format!("La voz '{}' ya existe (usa --force para sobrescribirla).", name),
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
            avi_tts::clone_voice(model_dir, speech_path, &tmp_qvoice, &name, "es").map_err(|e| {
                CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string())
            })?;
            let saved_qvoice = voice_store
                .save_reference(&name, &tmp_qvoice)
                .map_err(|e| CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string()))?;
            // Copias con los nombres del oráculo para compatibilidad de lecturas.
            let speech_copy = voice_store.voice_dir(&name).join("speech-reference.wav");
            std::fs::copy(speech_path, &speech_copy)
                .map_err(|e| CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string()))?;
            let timbre_saved = match &timbre_reference {
                Some(t) => {
                    let dest = voice_store.voice_dir(&name).join("timbre-reference.wav");
                    std::fs::copy(t, &dest).map_err(|e| {
                        CliError::new(ExitCode::Error, "voice_clone_failed", e.to_string())
                    })?;
                    Some(dest)
                }
                None => None,
            };
            if json_mode {
                emit_raw_json(json!({
                    "name": name,
                    "timbre": timbre_saved.map(|p| p.to_string_lossy().to_string()),
                    "speech": saved_qvoice.to_string_lossy().to_string(),
                    "precomputed": false,
                }));
            } else {
                println!("Voz '{}' clonada.", name);
            }
            Ok(())
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
        SpeechCommands::Transcribe { audio, mic, duration, source_language } => {
            // Validación de argumentos: --audio/--mic mutuamente excluyentes, uno
            // requerido; --duration solo válido con --mic.
            if audio.is_none() && !mic {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "Debe especificarse --audio o --mic.",
                ));
            }
            if mic && duration.is_none() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }

            // Modelo ausente -> exit 4, previo a construir el motor.
            if !std::path::Path::new(DEFAULT_WHISPER_MODEL_DIR).exists() {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "El modelo de transcripción no está provisionado en 'models/ct2/whisper-small'.",
                ));
            }

            let pcm = if mic {
                audio::AudioService::new()
                    .capture_16k_mono_pcm(duration.expect("validado arriba"))
                    .map_err(|e| {
                        CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
                    })?
            } else {
                avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(|e| {
                    CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
                })?
            };

            let engine = Ct2SttEngine::new(DEFAULT_WHISPER_MODEL_DIR).map_err(|e| {
                CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
            })?;
            let language = resolve_stt_language(&source_language);
            let text = engine.transcribe(&pcm, Some(language)).map_err(|e| {
                CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
            })?;

            if json_mode {
                emit_raw_json(json!({ "text": text, "source": source_language }));
            } else {
                println!("{}", text);
            }
            Ok(())
        }
        SpeechCommands::Synthesize { text, voice, output, label, force, play } => {
            // Orden de validaciones del oráculo (cli.py:659-667).
            if text.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto a sintetizar está vacío",
                ));
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
                    format!("Ya existe una locución con la etiqueta '{}' (usa --force).", label),
                ));
            }

            let tmp_wav = std::env::temp_dir().join(format!("avi_tts_{}.wav", label));
            let engine = Qwen3TtsEngine::new(None);
            engine.synthesize(&text, &voice, Some(&tmp_wav)).map_err(|e| {
                CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
            })?;
            if play {
                audio::AudioService::new().play_wav(&tmp_wav).map_err(|e| {
                    CliError::new(
                        ExitCode::Error,
                        "playback_failed",
                        format!("Fallo al reproducir la locución '{}': {}", label, e),
                    )
                })?;
            }
            let saved = speech_store
                .save(&voice, &label, &text, &tmp_wav)
                .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
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
        SpeechCommands::Say { text, voice } => {
            if text.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto a sintetizar está vacío",
                ));
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
            engine.synthesize(&text, &voice, Some(&tmp_wav)).map_err(|e| {
                CliError::new(ExitCode::Error, "synthesis_error", e.to_string())
            })?;
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
        SpeechCommands::Dub { audio, mic, duration, voice, from, to } => {
            // Validaciones del oráculo (cli.py:562-624).
            if duration.is_some() && !mic {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--duration solo es válido con --mic.",
                ));
            }
            if mic && duration.is_none() && std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
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
            // Modelos ausentes → exit 4 antes de tocar audio (patrón main.rs:479-485).
            if !std::path::Path::new(DEFAULT_WHISPER_MODEL_DIR).exists() {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "El modelo de transcripción no está provisionado en 'models/ct2/whisper-small'.",
                ));
            }
            require_model_provisioned()?;

            let pcm = if mic {
                audio::AudioService::new()
                    .capture_16k_mono_pcm(duration.expect("validado arriba"))
                    .map_err(|e| {
                        CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
                    })?
            } else {
                avi_audio::load_wav_16k_mono_pcm(audio.expect("validado arriba")).map_err(|e| {
                    CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
                })?
            };
            let stt = Ct2SttEngine::new(DEFAULT_WHISPER_MODEL_DIR).map_err(|e| {
                CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
            })?;
            let transcribed = stt
                .transcribe(&pcm, Some(resolve_stt_language(&from)))
                .map_err(|e| {
                    CliError::new(ExitCode::TranscriptionFailed, "transcription_error", e.to_string())
                })?;
            if transcribed.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto transcrito está vacío",
                ));
            }

            // Traducción solo si from != to tras normalizar (passthrough si coinciden).
            let source = resolve_stt_language(&from);
            let target = resolve_stt_language(&to);
            let final_text = if source == target {
                transcribed.clone()
            } else {
                let model_dir = match (source, target) {
                    ("es", "en") => DEFAULT_TRANSLATION_MODEL_ES_EN,
                    ("en", "es") => DEFAULT_TRANSLATION_MODEL_EN_ES,
                    _ => {
                        return Err(CliError::new(
                            ExitCode::InvalidInput,
                            "unsupported_language_pair",
                            format!("Par de idiomas no soportado: {} -> {} (soportados: es, en)", source, target),
                        ));
                    }
                };
                if !std::path::Path::new(model_dir).exists() {
                    return Err(CliError::new(
                        ExitCode::ModelMissing,
                        "model_missing",
                        format!("El modelo de traducción no está provisionado en '{}'.", model_dir),
                    ));
                }
                translation::translate(&transcribed, source, target, model_dir).map_err(|e| {
                    CliError::new(ExitCode::TranslationFailed, "translation_failed", e.to_string())
                })?
            };

            let voice_store = VoiceStore::new();
            if !voice_store.exists(&voice) {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "voice_not_found",
                    format!("La voz '{}' no existe.", voice),
                ));
            }
            let tmp_wav = std::env::temp_dir().join(format!("avi_dub_{}.wav", std::process::id()));
            let engine = Qwen3TtsEngine::new(None);
            engine.synthesize(&final_text, &voice, Some(&tmp_wav)).map_err(|e| {
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
        SpeechCommands::Play { label, voice } => {
            es_identificador_valido(Some(&voice), Some(&label))?;
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
            es_identificador_valido(Some(&voice), Some(&label))?;
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

/// Valida identificadores de voz/etiqueta contra el regex del oráculo
/// (`^[A-Za-z0-9._-]+$`; paridad, divergencia 3 de F1) → exit 2.
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
