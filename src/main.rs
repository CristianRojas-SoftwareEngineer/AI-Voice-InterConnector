use avi_audio as audio;
// El trait STT y el motor real solo entran en scope con `native-stt` (off por
// defecto); sin el feature, los subcomandos de transcripción devuelven un error
// explícito de "compilado sin soporte" (ver plan R1/T6).
#[cfg(feature = "native-stt")]
use avi_core::engine::SttEngine;
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_daemon as daemon;
use avi_store as store;
use avi_store::{ModelStore, SpeechStore, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::ParakeetEngine;
use avi_tts::{Qwen3TtsEngine, TtsEngine};
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

const VERSION: &str = "0.18.14";
const APP_NAME: &str = "ai-voice-interconnector";
/// Dirección del daemon nativo (T7: cliente HTTP async contra este address).
const DAEMON_ADDR: &str = "127.0.0.1:8765";
/// Techo temporal para esperar que el daemon sea alcanzable en `daemon start/restart`.
/// Dimensionado solo para spawn + bind del proceso (el warmup TTS corre en segundo
/// plano, ya no bloquea el arranque); no es un tiempo de espera fijo: el sondeo
/// retorna en cuanto `/health` responde.
const DAEMON_READY_DEADLINE: std::time::Duration = std::time::Duration::from_secs(10);
/// Intervalo entre reintentos del sondeo de readiness.
const DAEMON_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(250);

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
        /// Incluye el modelo Base de clonado Qwen3-TTS (~2,5 GB)
        #[arg(long, alias = "with-clone", alias = "clone")]
        with_base: bool,
    },
    /// Limpia modelos/caché (usa --all para desinstalación completa)
    Cleanup {
        /// Elimina también binario y PATH (desinstalación completa, alias de `uninstall`)
        #[arg(long)]
        all: bool,
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
        Some(Commands::Voice { action }) => handle_voice(json_mode, daemon_mode, action).await,
        Some(Commands::Speech { action }) => handle_speech(json_mode, daemon_mode, action).await,
        Some(Commands::Daemon { action }) => handle_daemon(json_mode, action).await,
        Some(Commands::Setup { language, with_stt, with_base }) => {
            handle_setup(json_mode, &language, with_stt, with_base).await
        }
        Some(Commands::Cleanup { all }) => {
            if all {
                handle_uninstall(json_mode, true).await
            } else {
                handle_cleanup(json_mode).await
            }
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

fn handle_translate(
    json_mode: bool,
    daemon_mode: DaemonMode,
    text: &str,
    from: &str,
    to: &str,
) -> Result<(), CliError> {
    // T7: el daemon nativo aún no expone /translate (el contrato NDJSON de esta
    // fase cubre solo synthesize/transcribe). En ForceDaemon se rechaza con
    // DaemonUnreachable; en Auto/ForceDirect se ejecuta local, preservando el
    // fallback intacto. La ruta daemon se habilitará cuando el daemon sirva
    // /translate.
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
                format!(
                    "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                    source, target
                ),
            ));
        }
    };
    // Modelo ausente -> exit 4, previo a construir el motor (patrón de STT).
    if !std::path::Path::new(model_dir).exists() {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            format!(
                "El modelo de traducción no está provisionado en '{}'.",
                model_dir
            ),
        ));
    }
    // Compilado sin soporte de traducción (feature off): rama de error explícita;
    // toda la validación previa (par soportado, modelo presente) es pura y corre igual.
    #[cfg(not(feature = "native-translation"))]
    {
        let _ = model_dir;
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        let translated = translation::translate(text, source, target, model_dir).map_err(|e| {
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

// ─── Voice ───────────────────────────────────────────────────────────

async fn handle_voice(json_mode: bool, daemon_mode: DaemonMode, action: VoiceCommands) -> Result<(), CliError> {
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
            // Tarea 2: despacho 3-modos para voice clone (S3-02). List/Remove son local-only
            // y ya validaron require_local; Clone respeta ForceDaemon/ForceDirect/Auto:
            // ForceDaemon exige daemon activo (exit 5), ForceDirect evita sondeo, Auto
            // sondea y si responde avisa por stderr pero mantiene clonado local (no hay
            // endpoint clone en daemon, solo precompute post-registro).
            {
                let client = daemon_client();
                match daemon_mode {
                    DaemonMode::ForceDaemon => {
                        if !daemon_activo(&client).await {
                            return Err(CliError::new(
                                ExitCode::DaemonUnreachable,
                                "daemon_unreachable",
                                "Daemon inalcanzable en 127.0.0.1:8765",
                            ));
                        }
                    }
                    DaemonMode::Auto => {
                        // Best-effort: si el daemon responde, el precompute posterior será más rápido (modelo caliente)
                        if daemon_activo(&client).await {
                            eprintln!("Daemon activo: el precompute de '{}' usará modelo caliente.", name);
                        }
                    }
                    DaemonMode::ForceDirect => {}
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
            require_local(daemon_mode)?;
            VoiceStore::validate_name(&name)
                .map_err(|e| CliError::new(ExitCode::InvalidInput, "invalid_voice_name", e))?;
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

async fn handle_speech(
    json_mode: bool,
    daemon_mode: DaemonMode,
    action: SpeechCommands,
) -> Result<(), CliError> {
    let speech_store = SpeechStore::new();

    match action {
        SpeechCommands::List => {
            // Listado de locuciones: local-only; el daemon no expone GET /speech.
            require_local(daemon_mode)?;
            let items = speech_store
                .list()
                .map_err(|e| CliError::new(ExitCode::Error, "speech_list_failed", e.to_string()))?;
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
            // T4: push-to-talk sin --duration permitido en TTY (S2-01); sin TTY se exige --duration
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic requiere --duration en este host.",
                ));
            }

            // T7 — dispatch 3 modos (Transcribe es delegable al daemon):
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
            if !ModelStore::new().model_dir("parakeet-tdt-v3")
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
                    audio::AudioService::new()
                        .capture_16k_mono_pcm(duration.expect("validado arriba"))
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_error",
                                e.to_string(),
                            )
                        })?
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

                let engine = ParakeetEngine::new(ModelStore::new().model_dir("parakeet-tdt-v3")).map_err(|e| {
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
        } => {
            // Orden de validaciones del oráculo (cli.py:659-667).
            if text.trim().is_empty() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "empty_text",
                    "El texto a sintetizar está vacío",
                ));
            }

            // T7 — dispatch 3 modos (Synthesize es delegable al daemon).
            let client = daemon_client();
            if route_to_daemon(daemon_mode, &client).await {
                let saved =
                    synthesize_via_daemon(&client, &text, &voice, &label, force, play, &output)
                        .await?;
                if json_mode {
                    emit_raw_json(json!({
                        "status": "success",
                        "audio_path": saved,
                        "voice": voice,
                    }));
                } else {
                    println!("Síntesis completada: {}", saved);
                }
                return Ok(());
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
            engine
                .synthesize(&text, &voice, Some(&tmp_wav))
                .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
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

            // T7 — dispatch 3 modos (Say es delegable al daemon).
            let client = daemon_client();
            if route_to_daemon(daemon_mode, &client).await {
                return say_via_daemon(json_mode, &client, &text, &voice).await;
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
            engine
                .synthesize(&text, &voice, Some(&tmp_wav))
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
            from,
            to,
        } => {
            // Doblaje es un pipeline compuesto (transcribe→traduce→sintetiza) que
            // el daemon no expone como ruta única; se mantiene local-only.
            // T7: ForceDaemon → DaemonUnreachable (ruta no delegable).
            require_local(daemon_mode)?;
            // Validaciones del oráculo (cli.py:562-624).
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
            if !ModelStore::new().model_dir("parakeet-tdt-v3")
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
                let _ = (&voice, &from, &to);
                Err(CliError::new(
                    ExitCode::Error,
                    "stt_unsupported",
                    "Este binario se compiló sin soporte de transcripción (feature 'native-stt').",
                ))
            }
            #[cfg(feature = "native-stt")]
            {
                let pcm = if mic {
                    audio::AudioService::new()
                        .capture_16k_mono_pcm(duration.expect("validado arriba"))
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_error",
                                e.to_string(),
                            )
                        })?
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
                let stt = ParakeetEngine::new(ModelStore::new().model_dir("parakeet-tdt-v3")).map_err(|e| {
                    CliError::new(
                        ExitCode::TranscriptionFailed,
                        "transcription_error",
                        e.to_string(),
                    )
                })?;
                let transcribed = stt
                    .transcribe(&pcm, Some(resolve_stt_language(&from)))
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
                                format!(
                                    "Par de idiomas no soportado: {} -> {} (soportados: es, en)",
                                    source, target
                                ),
                            ));
                        }
                    };
                    if !std::path::Path::new(model_dir).exists() {
                        return Err(CliError::new(
                            ExitCode::ModelMissing,
                            "model_missing",
                            format!(
                                "El modelo de traducción no está provisionado en '{}'.",
                                model_dir
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
                        translation::translate(&transcribed, source, target, model_dir).map_err(
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
                    .synthesize(&final_text, &voice, Some(&tmp_wav))
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
    match action {
        DaemonCommands::Serve => {
            let addr: SocketAddr =
                "127.0.0.1:8765"
                    .parse()
                    .map_err(|e: std::net::AddrParseError| {
                        CliError::new(ExitCode::Error, "invalid_address", e.to_string())
                    })?;
            daemon::run_daemon_server(addr).await.map_err(|e| {
                CliError::new(ExitCode::DaemonUnreachable, "daemon_error", e.to_string())
            })
        }
        DaemonCommands::Start => {
            require_model_provisioned()?;
            let client = daemon_client();
            if daemon_activo(&client).await {
                let pid = read_daemon_pid().unwrap_or(0);
                if json_mode {
                    emit_raw_json(json!({ "status": "already_running", "daemon": "running", "pid": pid }));
                } else {
                    println!("Daemon ya en ejecución (pid {}).", pid);
                }
                return Ok(());
            }
            let pid = daemon::spawn_background().map_err(|e| {
                CliError::new(ExitCode::Error, "daemon_error", format!("No se pudo lanzar el daemon: {}", e))
            })?;
            await_daemon_ready(&client, DAEMON_ADDR, DAEMON_READY_DEADLINE, DAEMON_POLL_INTERVAL)
                .await
                .map_err(|e| {
                    CliError::new(ExitCode::DaemonUnreachable, "daemon_unreachable", e.to_string())
                })?;
            write_daemon_pid(pid).map_err(|e| {
                CliError::new(ExitCode::Error, "daemon_error", format!("No se pudo escribir daemon.pid: {}", e))
            })?;
            if json_mode {
                emit_raw_json(json!({ "status": "started", "daemon": "running", "pid": pid }));
            } else {
                println!("Daemon iniciado correctamente (pid {}).", pid);
            }
            Ok(())
        }
        DaemonCommands::Stop => {
            let client = daemon_client();
            let resp = client
                .post(format!("http://{}/shutdown", DAEMON_ADDR))
                .send()
                .await;
            match resp {
                Ok(r) if r.status().is_success() => {
                    let _ = remove_daemon_pid_file();
                    if json_mode {
                        emit_raw_json(json!({ "status": "shutdown_sent", "daemon": "stopped" }));
                    } else {
                        println!("Señal de apagado enviada al daemon en {}.", DAEMON_ADDR);
                    }
                    Ok(())
                }
                Ok(r) => Err(CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!("El daemon devolvió el código {}", r.status()),
                )),
                Err(_e) => {
                    // Idempotencia: si no responde pero hay pid stale, limpiar
                    let _ = remove_daemon_pid_file();
                    Err(CliError::new(
                        ExitCode::DaemonUnreachable,
                        "daemon_unreachable",
                        format!("Daemon inalcanzable en {}", DAEMON_ADDR),
                    ))
                }
            }
        }
        DaemonCommands::Restart => {
            // Restart = Stop + spawn_background (orquestado CLI, sin ruta /restart)
            let client = daemon_client();
            let was_running = daemon_activo(&client).await;
            if was_running {
                let _ = client
                    .post(format!("http://{}/shutdown", DAEMON_ADDR))
                    .send()
                    .await;
                let _ = wait_health_down(&client, std::time::Duration::from_secs(5)).await;
                let _ = remove_daemon_pid_file();
            } else {
                let _ = remove_daemon_pid_file();
            }
            require_model_provisioned()?;
            let pid = daemon::spawn_background().map_err(|e| {
                CliError::new(ExitCode::Error, "daemon_error", format!("No se pudo lanzar el daemon: {}", e))
            })?;
            await_daemon_ready(&client, DAEMON_ADDR, DAEMON_READY_DEADLINE, DAEMON_POLL_INTERVAL)
                .await
                .map_err(|e| {
                    CliError::new(ExitCode::DaemonUnreachable, "daemon_unreachable", e.to_string())
                })?;
            write_daemon_pid(pid).map_err(|e| {
                CliError::new(ExitCode::Error, "daemon_error", format!("No se pudo escribir daemon.pid: {}", e))
            })?;
            if json_mode {
                emit_raw_json(json!({ "status": "restarted", "daemon": "running", "pid": pid }));
            } else {
                println!("Daemon reiniciado (pid {}).", pid);
            }
            Ok(())
        }
        DaemonCommands::Status => {
            // T7: GET /health → running; sin respuesta (timeout/conexión) → stopped
            // (exit 0), conservando el contrato de la fixture `cli_daemon_status.json`.
            let client = daemon_client();
            match tokio::time::timeout(
                std::time::Duration::from_millis(500),
                client.get(format!("http://{}/health", DAEMON_ADDR)).send(),
            )
            .await
            {
                Ok(Ok(resp)) if resp.status().is_success() => {
                    let val: Value = resp.json().await.map_err(|e| {
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

async fn handle_setup(json_mode: bool, language: &str, with_stt: bool, with_base: bool) -> Result<(), CliError> {
    let model_store = ModelStore::new();
    let voice_store = VoiceStore::new();

    // 1. Inicializar VoiceStore y directorio por defecto
    voice_store
        .ensure_initialized()
        .map_err(|e| CliError::new(ExitCode::Error, "voice_store_init_failed", e.to_string()))?;

    // 2. Descargar y registrar modelos pinneados. Base es opt-in (--with-base).
    if with_stt {
        tracing::info!("--with-stt es redundante: parakeet-tdt-v3 ya está incluido en setup");
    }
    let mut provisioned = Vec::new();
    for name in store::MODEL_REVISIONS
        .iter()
        .map(|(n, _, _)| *n)
        .filter(|n| *n != "qwen3-tts-0.6b-base" || with_base)
    {
        // Idempotente: snapshot HF presente → solo registrar índice.
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
        model_store
            .register_provisioned(name, "hf-snapshot")
            .map_err(|e| {
                CliError::new(
                    ExitCode::Error,
                    "model_provision_failed",
                    format!("{}: {}", name, e),
                )
            })?;
        provisioned.push(name.to_string());
    }

    if json_mode {
        emit_raw_json(json!({
            "status": "completed",
            "language": language,
            "with_stt": with_stt,
            "models_provisioned": provisioned
        }));
    } else {
        println!(
            "Setup completado: {} modelo(s) disponibles para idioma '{}'.",
            provisioned.len(),
            language
        );
    }
    Ok(())
}

async fn handle_cleanup(json_mode: bool) -> Result<(), CliError> {
    // 0. Parar daemon graceful si está vivo (libera puerto y permite borrar data_dir)
    {
        let client = daemon_client();
        if daemon_activo(&client).await {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client
                    .post(format!("http://{}/shutdown", DAEMON_ADDR))
                    .send(),
            )
            .await;
            let _ = wait_health_down(&client, std::time::Duration::from_secs(5)).await;
        }
        let _ = remove_daemon_pid_file();
        // Fallback Windows: matar qwen_tts huérfano si quedó
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "qwen_tts.exe"])
                .output();
        }
    }
    // 1. Limpieza real de datos: borra subdirectorios conocidos + daemon.pid
    let data = store::data_dir();
    if data.exists() {
        for sub in &["models", "speech", "voices"] {
            let p = data.join(sub);
            if p.exists() {
                let _ = std::fs::remove_dir_all(&p);
            }
        }
        let _ = std::fs::remove_file(data.join("daemon.pid"));
        // Si quedó vacío, intenta borrar el propio data_dir
        let _ = std::fs::remove_dir(&data);
        // Si sigue existiendo (daemon.pid huérfano), forzar remove_dir_all
        if data.exists() {
            if let Ok(mut entries) = std::fs::read_dir(&data) {
                if entries.next().is_none() {
                    let _ = std::fs::remove_dir(&data);
                }
            }
        }
    }
    // 2. Snapshots HF de los modelos pinneados (~/.cache/huggingface/hub/models--*)
    let model_store = ModelStore::new();
    for name in store::MODEL_REVISIONS.iter().map(|(n, _, _)| *n) {
        match model_store.remove_hf_snapshot(name) {
            Ok(removed) if removed => eprintln!("Snapshot {} eliminado.", name),
            Ok(_) => {}
            Err(e) => eprintln!("  ✗ No se pudo borrar {}: {}", name, e),
        }
    }
    // 3. Purga xet shard-cache + logs + .locks para coherencia hub/xet
    match store::ModelStore::remove_xet_cache() {
        Ok(true) => eprintln!("Cache xet eliminada."),
        Ok(false) => {}
        Err(e) => eprintln!("  ✗ No se pudo borrar cache xet: {}", e),
    }
    // 4. Temp huérfano
    {
        let tmp = std::env::temp_dir();
        if let Ok(entries) = std::fs::read_dir(&tmp) {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                if name.starts_with("avi_") || name.starts_with("ai-voice-interconnector-install-") {
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
    if json_mode {
        emit_raw_json(json!({ "status": "cleanup_complete" }));
    } else {
        println!("Limpieza de modelos/caché completada.");
    }
    Ok(())
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

    // 0. Parar daemon graceful si está vivo
    {
        let client = daemon_client();
        if daemon_activo(&client).await {
            let _ = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client
                    .post(format!("http://{}/shutdown", DAEMON_ADDR))
                    .send(),
            )
            .await;
            let _ = wait_health_down(&client, std::time::Duration::from_secs(5)).await;
        }
        let _ = remove_daemon_pid_file();
        #[cfg(windows)]
        {
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "qwen_tts.exe"])
                .output();
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/IM", "ai-voice-interconnector.exe"])
                .output();
        }
    }

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
                    if name.starts_with("avi_") || name.starts_with("ai-voice-interconnector-install-") {
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
        let install_dir = {
            let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
            PathBuf::from(local).join("Programs/ai-voice-interconnector")
        };
        // Quitar del PATH de usuario (HKCU\Environment)
        let _ = remove_windows_user_path(&install_dir);
        // Borrar directorio de instalación (puede fallar si el exe está en uso)
        if install_dir.exists() {
            match std::fs::remove_dir_all(&install_dir) {
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::PermissionDenied => {
                    eprintln!("Aviso: no se pudo borrar {} (binario en uso). Bórralo manualmente tras cerrar la terminal.", install_dir.display());
                }
                Err(e) => {
                    return Err(CliError::new(
                        ExitCode::Error,
                        "uninstall_failed",
                        format!("No se pudo borrar {}: {}", install_dir.display(), e),
                    ));
                }
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
    let target = dir.to_string_lossy().to_string();
    let filtered: Vec<String> = path
        .split(';')
        .filter(|s| !s.is_empty() && !s.eq_ignore_ascii_case(&target))
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

    // Verificar los 4 modelos pinneados (snapshot HF en hf_cache_dir)
    if !model_store.is_provisioned("qwen3-tts-0.6b") {
        issues.push("Modelo TTS (Qwen3-TTS 0.6B) no provisionado");
    }
    if !model_store.is_provisioned("parakeet-tdt-v3") {
        issues.push("Modelo STT (Parakeet TDT v3) no provisionado");
    }
    if !model_store.is_provisioned("marian-es-en") {
        issues.push("Modelo traducción es→en (Marian) no provisionado");
    }
    if !model_store.is_provisioned("marian-en-es") {
        issues.push("Modelo traducción en→es (Marian) no provisionado");
    }
    // Base opt-in: WARN si falta, no FAIL
    let base_ready = model_store.is_provisioned("qwen3-tts-0.6b-base");
    let base_status = if base_ready { "ready" } else { "missing_opt_in" };

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
            println!("Diagnóstico: todo correcto. [WARN] Modelo Base de clonado no provisionado (usa setup --with-base).");
        }
        println!("Cache HF: {}", hf_cache.display());
        Ok(())
    } else {
        for issue in &issues {
            eprintln!("  ✗ {}", issue);
        }
        if !base_ready {
            eprintln!("  ⚠ [WARN] Modelo Base de clonado no provisionado (usa setup --with-base).");
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

fn daemon_pid_path() -> PathBuf {
    store::data_dir().join("daemon.pid")
}

fn write_daemon_pid(pid: u32) -> anyhow::Result<()> {
    let path = daemon_pid_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("pid.tmp");
    let content = serde_json::json!({
        "pid": pid,
        "addr": DAEMON_ADDR,
        "started_at": chrono::Utc::now().to_rfc3339()
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

fn remove_daemon_pid_file() -> std::io::Result<()> {
    let p = daemon_pid_path();
    if p.exists() {
        std::fs::remove_file(p)?;
    }
    Ok(())
}

/// Espera acotada a que el daemon sea alcanzable (`/health` responde) tras el
/// spawn+bind. Con el warmup movido a segundo plano, «alcanzable = listo»: el
/// sondeo retorna en cuanto `probe_health` es `true` o falla al vencer el deadline.
async fn await_daemon_ready(
    client: &reqwest::Client,
    addr: &str,
    deadline: std::time::Duration,
    interval: std::time::Duration,
) -> anyhow::Result<()> {
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
/// `stopped` cuando no es alcanzable (fixture intacta, sin campos extra); si es
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

async fn wait_health_down(client: &reqwest::Client, timeout: std::time::Duration) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if !daemon_activo(client).await {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    anyhow::bail!("El daemon no se apagó tras {:?}", timeout)
}

// ─── Cliente HTTP async del daemon (T7) ────────────────────────────────

/// Cliente `reqwest` hacia el daemon en `DAEMON_ADDR` (HTTP, sin TLS: basta para
/// localhost). Timeout de conexión breve para que el probe Auto→local sea rápido
/// cuando el daemon no está en ejecución.
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

/// Probe de vida sobre `DAEMON_ADDR`; `false` habilita el fallback Auto→local.
async fn daemon_activo(client: &reqwest::Client) -> bool {
    probe_health(client, DAEMON_ADDR).await
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
            "Daemon inalcanzable en 127.0.0.1:8765",
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
    let pcm: Vec<i16> = if mic {
        audio::AudioService::new()
            .capture_16k_mono_pcm(duration.expect("validado arriba"))
            .map_err(|e| {
                CliError::new(
                    ExitCode::TranscriptionFailed,
                    "transcription_error",
                    e.to_string(),
                )
            })?
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
        .post(format!("http://{}/transcribe", DAEMON_ADDR))
        .json(&serde_json::json!({ "audio_b64": audio_b64, "source_language": source_language }))
        .send()
        .await
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", DAEMON_ADDR, e),
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

/// POST /synthesize al daemon, consume el stream NDJSON y decodifica `audio_b64`
/// del evento `result`, devolviendo los bytes WAV del motor (24 kHz s16le mono).
async fn daemon_synthesize_wav(
    client: &reqwest::Client,
    text: &str,
    voice: &str,
) -> Result<Vec<u8>, CliError> {
    let resp = client
        .post(format!("http://{}/synthesize", DAEMON_ADDR))
        .json(&serde_json::json!({ "text": text, "voice": voice }))
        .send()
        .await
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", DAEMON_ADDR, e),
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

/// `synthesize` vía daemon: persiste el WAV en `SpeechStore` y respeta
/// --label/--output/--play, devolviendo la ruta del WAV persistido (paralelo al
/// handler local para que el envelope JSON de salida coincida).
async fn synthesize_via_daemon(
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    label: &str,
    force: bool,
    play: bool,
    output: &Option<String>,
) -> Result<String, CliError> {
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
    let wav = daemon_synthesize_wav(client, text, voice).await?;
    let tmp = std::env::temp_dir().join(format!("avi_tts_{}.wav", label_l));
    std::fs::write(&tmp, &wav)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    if play {
        audio::AudioService::new().play_wav(&tmp).map_err(|e| {
            CliError::new(
                ExitCode::Error,
                "playback_failed",
                format!("Fallo al reproducir la locución '{}': {}", label_l, e),
            )
        })?;
    }
    let saved = speech_store
        .save(voice, &label_l, text, &tmp)
        .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
    let _ = std::fs::remove_file(&tmp);
    if let Some(out) = output {
        std::fs::copy(&saved, out)
            .map_err(|e| CliError::new(ExitCode::Error, "synthesis_error", e.to_string()))?;
    }
    Ok(saved.to_string_lossy().to_string())
}

/// `say` vía daemon: reproduce el WAV decodificado y expone una copia efímera.
async fn say_via_daemon(
    json_mode: bool,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
) -> Result<(), CliError> {
    let wav = daemon_synthesize_wav(client, text, voice).await?;
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
        let res = await_daemon_ready(&daemon_client(), &addr, deadline, interval).await;
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
}
