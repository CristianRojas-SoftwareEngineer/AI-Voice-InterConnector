// This Source Code Form is subject to the terms of the Mozilla Public License, v. 2.0.
// If a copy of the MPL was not distributed with this file, You can obtain one at https://mozilla.org/MPL/2.0/.
//
// Copyright (c) 2026 Cristián Rojas Arredondo.
// SPDX-License-Identifier: MPL-2.0

#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use avi_audio as audio;
use avi_core::exit_codes::{CliError, ExitCode};
use avi_core::json_emitter::emit_raw_json;
use avi_daemon as daemon;
use avi_lifecycle as lifecycle;
use avi_store as store;
use avi_store::{Label, ModelStore, SpeechStore, VoiceName, VoiceStore};
#[cfg(feature = "native-stt")]
use avi_stt::EngineHolder;
use avi_term::progress::PhaseProgress;
use avi_term::stage::{self, Stage};
use avi_tts::Qwen3TtsEngine;
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
use std::sync::Arc;

const VERSION: &str = "0.47.0";
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
/// hijo. Cubre spawn + validación + bind + carga del estado (~1-2 s sanos); la
/// confirmación por `/health` posterior tiene su propio presupuesto
/// (`DAEMON_HEALTH_DEADLINE`) y la espera a que el motor termine de calentar
/// dispone de otro plazo de la misma duración que este.
/// Vencer la publicación con el hijo vivo es un diagnóstico
/// (`daemon_unreachable`), no un flake a reintentar; vencer la espera del
/// calentamiento no es un error, porque el daemon ya sirve: el cierre del
/// comando dice que el motor sigue calentándose.
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

/// Traduce un fallo de la síntesis directa al contrato: la falta del modelo Base
/// sale como `model_missing` (exit 4), el vencimiento del presupuesto como
/// `synthesis_timeout`, una voz sin `reference.qvoice` como `voice_not_found`
/// (exit 3) y cualquier otro, incluida una entrega cortada a mitad
/// (`SynthesisTruncated`), como `synthesis_failed`.
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
/// (`QWEN3_TTS_BIN` o `QWEN3_TTS_BASE_MODEL_DIR`) que no apunta a una ruta
/// existente, o un modelo ilegible, es `model_missing`.
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

/// Referencia de una voz del almacén con la identidad de su contenido (`None`
/// si la voz no tiene `reference.qvoice`); un fallo de E/S es `io_error`.
fn resolve_voice_reference(
    voice_store: &VoiceStore,
    voice: &VoiceName,
) -> Result<Option<avi_store::VoiceReference>, CliError> {
    voice_store
        .resolve(voice)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Perfil de voz y opciones de generación de una síntesis directa: el idioma de
/// destino (token del producto), la temperatura opcional ya validada y el seed
/// opcional (`None` usa el de producción).
fn tts_request(
    reference: Option<&avi_store::VoiceReference>,
    voice: &VoiceName,
    temperature: Option<f32>,
    seed: Option<u64>,
    language: &str,
) -> (avi_tts::VoiceProfile, avi_tts::GenerationOptions) {
    let profile = avi_tts::VoiceProfile {
        name: voice.to_string(),
        reference: reference.cloned(),
    };
    let options = avi_tts::GenerationOptions::with_temperature(temperature, language)
        .with_seed_override(seed);
    (profile, options)
}

/// Síntesis directa sin reproducción: acumula la entrega del motor y escribe el
/// WAV en `output_path`, con la voz y su referencia ya resueltas.
///
/// El texto activo de la etapa `SYNTHESIZING` ya abierta pasa a «Cargando la
/// voz…» mientras el motor arranca su residente, y vuelve a «Sintetizando…».
// Firma ancha deliberada: cada argumento mapea 1:1 a un dato de la síntesis.
#[allow(clippy::too_many_arguments)]
fn synthesize_wav_directly(
    progress: &PhaseProgress,
    engine: &Qwen3TtsEngine,
    reference: Option<&avi_store::VoiceReference>,
    text: &str,
    voice: &VoiceName,
    temperature: Option<f32>,
    seed: Option<u64>,
    language: &str,
    output_path: &std::path::Path,
) -> Result<(), CliError> {
    let (profile, options) = tts_request(reference, voice, temperature, seed, language);
    engine
        .synthesize_to_wav(
            text,
            &profile,
            &options,
            output_path,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |step| show_synthesis_step(progress, step),
        )
        .map_err(synthesis_cli_error)
}

/// Refleja en el texto activo de la etapa `SYNTHESIZING` el paso que avisa el
/// motor local: cargar la voz o sintetizar. Es la misma etapa y el mismo reloj.
fn show_synthesis_step(progress: &PhaseProgress, step: avi_tts::SynthesisStep) {
    let active = match step {
        avi_tts::SynthesisStep::LoadingVoice => stage::SYNTHESIS_LOADING_VOICE_TEXT,
        avi_tts::SynthesisStep::Synthesizing => stage::SYNTHESIZING.active,
    };
    progress.phase_with(&stage::SYNTHESIZING, active);
}

/// Datos de una síntesis directa en streaming: el motor local, la voz con su
/// referencia ya resuelta (`None` si no tiene `reference.qvoice`), el texto y el
/// idioma de destino.
struct DirectSynthesis<'a> {
    engine: &'a Qwen3TtsEngine,
    reference: Option<&'a avi_store::VoiceReference>,
    text: &'a str,
    voice: &'a VoiceName,
    temperature: Option<f32>,
    seed: Option<u64>,
    language: &'a str,
}

/// Lo que queda de una entrega en streaming ya terminada.
struct DeliveryStats {
    /// Milisegundos desde que se abre la etapa de síntesis (la del registro de
    /// ejecución), incluidas la espera de turno, el calentamiento del motor y la
    /// carga de la voz, hasta que la primera muestra llegó al dispositivo; `None`
    /// si no hubo reproducción o no llegó audio. Es la misma medida que lleva la
    /// línea de `Síntesis`.
    first_audio_ms: Option<u64>,
    /// Veces que la cola de reproducción se vació con la entrega en curso.
    stalls: u32,
    /// Audio acumulado (PCM de 16 bits, 24 kHz, mono); vacío si no se acumuló.
    pcm: Vec<i16>,
}

/// Dato de la línea de `Reproducción` mientras la síntesis sigue en marcha.
const PLAYING_IN_PARALLEL_NOTE: &str = "en paralelo con la síntesis";
/// Texto vivo mientras suena el audio y el motor sigue sintetizando.
const SYNTHESIZING_AND_PLAYING_TEXT: &str = "Sintetizando y reproduciendo…";
/// Texto vivo cuando la síntesis ya terminó y queda audio por sonar.
const PLAYING_REST_TEXT: &str = "Reproduciendo el resto…";

/// Duración desde la apertura de la última etapa de síntesis del registro hasta
/// `first_audio_at`, truncada a milisegundos. Es la única medida del primer
/// audio: la línea de `Síntesis`, el cierre y el JSON salen de ella. `None` si el
/// registro no tiene etapa de síntesis.
fn first_audio_offset(
    snapshot: &avi_term::report::Snapshot,
    first_audio_at: std::time::Instant,
) -> Option<std::time::Duration> {
    let began = snapshot
        .stages
        .iter()
        .rev()
        .find(|record| record.label == stage::SYNTHESIZING.label)?
        .start;
    let elapsed = first_audio_at.saturating_duration_since(began);
    Some(std::time::Duration::from_millis(elapsed.as_millis() as u64))
}

/// Presentación de las dos etapas solapadas: `Síntesis` y `Reproducción`. La
/// reproducción se abre con la primera muestra que llegó al dispositivo (el
/// instante exacto, no el de la CLI al enterarse) y la síntesis se cierra
/// cuando termina de verdad, aunque siga sonando audio.
#[derive(Default)]
struct SynthesisStages {
    /// `Reproducción` ya está abierta en el registro.
    playing_opened: bool,
    /// `Síntesis` ya se cerró.
    synthesis_ended: bool,
    /// `Reproducción` se abrió con la síntesis todavía viva.
    in_parallel: bool,
    /// Cortes ya reflejados en pantalla.
    stalls: u32,
}

impl SynthesisStages {
    /// Refleja las estadísticas de reproducción: abre `Reproducción` en cuanto
    /// hay primera muestra y actualiza el dato y el texto vivo si cambian los
    /// cortes.
    fn observe(&mut self, progress: &PhaseProgress, stats: audio::PlaybackStats) {
        if !self.playing_opened {
            let Some(first_audio_at) = stats.first_audio_at else {
                return;
            };
            self.playing_opened = true;
            self.in_parallel = !self.synthesis_ended;
            progress.overlap(&stage::PLAYING, first_audio_at);
            self.refresh(progress, stats.stalls);
        } else if stats.stalls != self.stalls {
            self.refresh(progress, stats.stalls);
        }
    }

    /// Escribe el dato de `Reproducción` y el texto vivo de la etapa que se ve.
    fn refresh(&mut self, progress: &PhaseProgress, stalls: u32) {
        self.stalls = stalls;
        let cuts = (stalls > 0).then(|| avi_term::fmt::count(u64::from(stalls), "corte", "cortes"));
        let note: Vec<&str> = self
            .in_parallel
            .then_some(PLAYING_IN_PARALLEL_NOTE)
            .into_iter()
            .chain(cuts.as_deref())
            .collect();
        if !note.is_empty() {
            progress.note_stage(&stage::PLAYING, &note.join(" · "));
        }
        let (target, text) = if self.synthesis_ended {
            (&stage::PLAYING, PLAYING_REST_TEXT)
        } else {
            (&stage::SYNTHESIZING, SYNTHESIZING_AND_PLAYING_TEXT)
        };
        let text = match &cuts {
            Some(cuts) => format!("{text} · {cuts}"),
            None => text.to_string(),
        };
        progress.live_text(target, &text);
    }

    /// Cierra `Síntesis` en `at`, el instante en que terminó de verdad, con el
    /// dato `primer audio X` si la primera muestra ya sonó. Si `Reproducción`
    /// está abierta pasa a ser la etapa visible.
    fn synthesis_finished(
        &mut self,
        progress: &PhaseProgress,
        stats: audio::PlaybackStats,
        at: std::time::Instant,
    ) {
        self.observe(progress, stats);
        if self.synthesis_ended {
            return;
        }
        self.synthesis_ended = true;
        if let Some(first_audio) = stats
            .first_audio_at
            .and_then(|first| first_audio_offset(&avi_term::report::snapshot(), first))
        {
            progress.note_stage(
                &stage::SYNTHESIZING,
                &format!("primer audio {}", avi_term::fmt::duration(first_audio)),
            );
        }
        progress.end_stage_at(&stage::SYNTHESIZING, at);
        if self.playing_opened {
            self.refresh(progress, stats.stalls);
        }
    }

    /// Cierra `Reproducción` en `at`, cuando `finish` devuelve, con los cortes
    /// finales.
    fn playback_finished(
        &mut self,
        progress: &PhaseProgress,
        stats: audio::PlaybackStats,
        at: std::time::Instant,
    ) {
        self.observe(progress, stats);
        if self.playing_opened {
            progress.end_stage_at(&stage::PLAYING, at);
        }
    }
}

/// Receptor de los fragmentos de audio de una síntesis en streaming. Lo usan igual
/// la ruta directa, que recibe los fragmentos del motor, y la del daemon, que los
/// recibe en eventos `audio`.
trait AudioReceiver {
    /// Entrega un fragmento. Un fallo del dispositivo es `playback_failed`.
    fn push(&mut self, pcm: &[i16]) -> Result<(), CliError>;
    /// Si ya se entregó alguna muestra al dispositivo.
    fn started(&self) -> bool;
    /// Refleja en el indicador el estado de la reproducción: abre `Reproducción`
    /// en la primera muestra y muestra los cortes. Sin reproductor no hace nada.
    fn observe(&mut self, progress: &PhaseProgress);
    /// Avisa de que la síntesis terminó de verdad: cierra `Síntesis` en este
    /// instante, aunque el audio siga sonando.
    fn synthesis_finished(&mut self, progress: &PhaseProgress);
}

/// Reparte los fragmentos de una síntesis entre un reproductor opcional y un
/// acumulador opcional. Al terminar devuelve `first_audio_ms` y `stalls`.
struct StreamDelivery {
    player: Option<audio::StreamPlayer>,
    accumulated: Option<Vec<i16>>,
    /// Presentación de las etapas solapadas de síntesis y reproducción.
    stages: SynthesisStages,
    /// Principio del mensaje de `playback_failed`, antes de la causa.
    playback_failure: String,
}

impl StreamDelivery {
    /// Estadísticas de reproducción en vivo; `None` sin reproductor.
    fn live_stats(&self) -> Option<audio::PlaybackStats> {
        self.player.as_ref().map(audio::StreamPlayer::live_stats)
    }
}

impl AudioReceiver for StreamDelivery {
    fn push(&mut self, pcm: &[i16]) -> Result<(), CliError> {
        if let Some(player) = &self.player {
            player
                .push(pcm)
                .map_err(|e| playback_error(e, &self.playback_failure))?;
        }
        if let Some(accumulated) = &mut self.accumulated {
            accumulated.extend_from_slice(pcm);
        }
        Ok(())
    }

    fn started(&self) -> bool {
        self.player.as_ref().is_some_and(|p| p.started())
    }

    fn observe(&mut self, progress: &PhaseProgress) {
        if let Some(stats) = self.live_stats() {
            self.stages.observe(progress, stats);
        }
    }

    fn synthesis_finished(&mut self, progress: &PhaseProgress) {
        let stats = self.live_stats().unwrap_or(audio::PlaybackStats {
            first_audio_at: None,
            stalls: 0,
        });
        self.stages
            .synthesis_finished(progress, stats, std::time::Instant::now());
    }
}

impl StreamDelivery {
    /// Termina la entrega: espera a que suene lo que queda en cola, repintando
    /// el indicador y reflejando la reproducción mientras suena, cierra
    /// `Reproducción` cuando termina y devuelve los datos de la reproducción y
    /// el audio acumulado. `first_audio_ms` sale del registro: es la medida de
    /// la línea de `Síntesis`.
    fn finish(mut self, progress: &PhaseProgress) -> Result<DeliveryStats, CliError> {
        let (first_audio_ms, stalls) = match self.player {
            Some(player) => {
                let stages = &mut self.stages;
                let playback = player
                    .finish(&mut |live| {
                        progress.tick();
                        stages.observe(progress, live);
                    })
                    .map_err(|e| playback_error(e, &self.playback_failure))?;
                stages.playback_finished(progress, playback, std::time::Instant::now());
                let snapshot = avi_term::report::snapshot();
                (
                    playback
                        .first_audio_at
                        .and_then(|at| first_audio_offset(&snapshot, at))
                        .map(|offset| offset.as_millis() as u64),
                    playback.stalls,
                )
            }
            None => (None, 0),
        };
        Ok(DeliveryStats {
            first_audio_ms,
            stalls,
            pcm: self.accumulated.unwrap_or_default(),
        })
    }

    /// Corta la reproducción de inmediato y descarta lo pendiente: la síntesis
    /// falló a mitad.
    fn abort(self) {
        if let Some(player) = self.player {
            player.abort();
        }
    }
}

/// Receptor para los streams que no deben traer audio (`voice clone`): un evento
/// `audio` es un protocolo inválido.
struct RejectAudio;

impl AudioReceiver for RejectAudio {
    fn push(&mut self, _pcm: &[i16]) -> Result<(), CliError> {
        Err(CliError::new(
            ExitCode::Error,
            "daemon_error",
            "El daemon envió audio en un stream que no lo admite.",
        ))
    }

    fn started(&self) -> bool {
        false
    }

    fn observe(&mut self, _progress: &PhaseProgress) {}

    fn synthesis_finished(&mut self, _progress: &PhaseProgress) {}
}

/// Error de reproducción: sin backend, sin salida o fallo del flujo de audio.
/// `failure` es el principio del mensaje del fallo genérico.
fn playback_error(e: anyhow::Error, failure: &str) -> CliError {
    audio_error(e, ExitCode::Error, "playback_failed", |e| {
        format!("{failure}: {e}")
    })
}

/// Prepara la entrega de una síntesis en streaming. Con `play` abre el
/// dispositivo de salida —antes de pedir la síntesis, de modo que sin salida de
/// audio el comando termina sin sintetizar—; con `accumulate` guarda el audio.
/// `playback_failure` encabeza el mensaje de `playback_failed`.
fn open_delivery(
    play: bool,
    accumulate: bool,
    playback_failure: impl Into<String>,
) -> Result<StreamDelivery, CliError> {
    let playback_failure = playback_failure.into();
    let player = if play {
        Some(
            audio::AudioService::new()
                .stream_player(avi_tts::ENGINE_SAMPLE_RATE)
                .map_err(|e| playback_error(e, &playback_failure))?,
        )
    } else {
        None
    };
    Ok(StreamDelivery {
        player,
        accumulated: accumulate.then(Vec::new),
        stages: SynthesisStages::default(),
        playback_failure,
    })
}

/// Escribe en `path` el audio acumulado de una entrega como WAV de 24 kHz, mono y
/// 16 bits.
fn write_take_wav(path: &std::path::Path, pcm: &[i16]) -> Result<(), CliError> {
    let wav = audio::encode_pcm_mono_wav(pcm, avi_tts::ENGINE_SAMPLE_RATE)
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
    std::fs::write(path, wav).map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))
}

/// Síntesis directa en streaming: el motor local entrega el audio fragmento a
/// fragmento a `delivery`, que lo reproduce, lo acumula o ambas cosas. La etapa
/// `Reproducción` se abre con la primera muestra, en paralelo con `Síntesis`, y
/// `Síntesis` se cierra cuando vuelve la síntesis, antes de esperar a que
/// termine de sonar. Si la síntesis falla a mitad, se corta la reproducción y el
/// error es el de la síntesis; un fallo del dispositivo cancela la síntesis y es
/// `playback_failed`.
///
/// La etapa `SYNTHESIZING` ya está abierta: mientras el motor arranca su
/// residente su texto activo es «Cargando la voz…». El primer audio se mide
/// desde la apertura de esa etapa en el registro.
fn synthesize_streaming_directly(
    request: &DirectSynthesis<'_>,
    progress: &PhaseProgress,
    mut delivery: StreamDelivery,
) -> Result<DeliveryStats, CliError> {
    let (profile, options) = tts_request(
        request.reference,
        request.voice,
        request.temperature,
        request.seed,
        request.language,
    );
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut playback_failure: Option<CliError> = None;
    let result = progress.while_running(|| {
        request.engine.synthesize_streaming(
            request.text,
            &profile,
            &options,
            &cancel,
            &mut |pcm| {
                if let Err(e) = delivery.push(pcm) {
                    playback_failure = Some(e);
                    return Err(anyhow::anyhow!("falló la reproducción del audio"));
                }
                // La reproducción se abre con la primera muestra, sin cerrar la
                // síntesis, que sigue hasta que vuelve el motor.
                delivery.observe(progress);
                Ok(())
            },
            &mut |step| show_synthesis_step(progress, step),
        )
    });
    match result {
        Ok(()) => {
            // La síntesis terminó de verdad: se cierra ahora, antes de esperar
            // a que suene lo que queda en cola.
            delivery.synthesis_finished(progress);
            delivery.finish(progress)
        }
        Err(e) => {
            delivery.abort();
            Err(playback_failure.unwrap_or_else(|| synthesis_cli_error(e)))
        }
    }
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
#[command(name = APP_NAME, version = VERSION, about)]
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
    // Solo en la salida de texto el prefijo `Error:` lleva color: el sobre JSON
    // no puede contener secuencias ANSI.
    lines[0] = format!(
        "{} {message}",
        avi_term::style::paint(
            avi_term::style::Stream::Stderr,
            avi_term::style::Tone::Error,
            "Error:"
        )
    );
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
    /// Enumera dispositivos de salida de audio y muestra la entrada y la salida predeterminadas
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
    /// Síntesis y audios sintetizados
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
        /// Conservar modelos, voces y audios sintetizados
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
        /// Sobrescribir una voz existente con el mismo nombre (borra también sus audios sintetizados)
        #[arg(short, long)]
        force: bool,
        /// No pedir confirmación al borrar los audios sintetizados de la voz sobrescrita
        #[arg(long, short)]
        yes: bool,
        /// Limpiar ruido de fondo en la referencia antes de clonar (solo con `native-denoise`).
        #[cfg(feature = "native-denoise")]
        #[arg(long)]
        denoise: bool,
        #[command(flatten)]
        routing: Routing,
    },
    /// Borrar una voz clonada y sus audios sintetizados guardados
    Remove {
        #[arg(short, long)]
        name: String,
        /// No pedir confirmación
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
    /// Listar los audios sintetizados guardados
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
    /// Sintetizar un texto y guardar el audio sintetizado
    Synthesize {
        #[arg(short, long)]
        text: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// Etiqueta del audio sintetizado guardado (obligatorio)
        #[arg(short, long)]
        label: String,
        /// Sobrescribir un audio sintetizado existente con la misma etiqueta
        #[arg(short, long)]
        force: bool,
        /// Conservar el audio sintetizado existente con la misma etiqueta sin sintetizar (sale con 0 y `skipped`)
        #[arg(long, conflicts_with_all = ["force", "play"])]
        skip_existing: bool,
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
    /// Reproducir un audio sintetizado guardado
    Play {
        #[arg(short, long)]
        label: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
    },
    /// Borrar un audio sintetizado guardado
    Remove {
        #[arg(short, long)]
        label: String,
        #[arg(short, long, default_value = "default")]
        voice: String,
        /// No pedir confirmación
        #[arg(long, short)]
        yes: bool,
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

/// PID del daemon que el manejador de Ctrl+C debe matar: solo el que lanzó
/// este mismo proceso (`daemon start` y `daemon restart` lo dejan en
/// `IN_MEMORY_PID`), nunca el que otro proceso registró en el pidfile. Si el
/// PID es el del propio proceso (`daemon serve` en primer plano), no se mata:
/// el cierre lo hace la escucha de señales del servidor.
fn daemon_to_kill_on_interrupt() -> Option<u32> {
    let pid = IN_MEMORY_PID.load(std::sync::atomic::Ordering::Relaxed);
    (pid != 0 && pid != std::process::id()).then_some(pid)
}

/// Pinta la interrupción desde el registro y, con `--json`, emite el envelope
/// de error `interrupted`. Quien llama tiene tomada la guarda del registro y no
/// la suelta hasta terminar el proceso, así que aquí no se toma de nuevo.
///
/// - Con una pregunta en curso o sin ninguna etapa no transitoria: deja
///   `Interrumpido; no se ha modificado nada.` y, con contexto, `stages: []`.
/// - Con etapas: marca como interrumpidas las que siguen abiertas, pinta su
///   `✗ <Nombre> <duración> · interrumpido`, el separador e `Interrumpido en <t>`.
fn report_interruption(report: &mut avi_term::report::ReportGuard) {
    use avi_term::line::SpecialClosing;

    avi_term::line::take_phase_line();
    let before = report.snapshot();
    let untouched = before.question_active || !before.has_permanent_stage();
    let closing = if untouched {
        SpecialClosing::NothingModified
    } else {
        SpecialClosing::Interrupted
    };
    avi_term::line::special_closing_with(report, closing);
    if avi_term::line::machine_mode() {
        let err = CliError::new(
            ExitCode::Interrupted,
            "interrupted",
            "Interrumpido por el usuario.",
        );
        let mut envelope = json!({
            "error": error_envelope_text(&err),
            "reason": err.reason,
        });
        let snapshot = report.snapshot();
        if untouched && snapshot.context.is_some() {
            envelope["stages"] = json!([]);
        }
        avi_core::json_emitter::emit_raw_json_with(envelope, &snapshot);
    }
}

/// Instalar handler de SIGINT → pinta la interrupción + exit 130.
///
/// Preserva el código 130 en todos los modos. Pinta la interrupción desde el
/// registro de ejecución y, con `--json`, emite su envelope de error antes de
/// cualquier espera. Después mata, si lo hay, solo el árbol del daemon que
/// lanzó este proceso (`taskkill /F /T /PID` en Windows, `kill -9` al grupo en
/// Unix) con deadline breve: un comando cliente nunca mata el daemon ajeno.
/// Vencido el deadline sale igualmente con 130 sin colgarse.
/// Instalado en `main` antes del despacho: cubre todos los modos.
fn install_sigint_handler() {
    ctrlc::set_handler(move || {
        // Un lote `denoise` en curso cancela de forma cooperativa: la primera
        // pulsación activa su flag y deja que limpie temporales; la segunda
        // termina el proceso por el camino general.
        #[cfg(feature = "native-denoise")]
        if let Some(flag) = DENOISE_CANCEL.get() {
            if !flag.swap(true, std::sync::atomic::Ordering::SeqCst) {
                // `warn` cede antes la línea de fase abierta, para no pegar el
                // aviso al indicador.
                avi_term::line::warn("Cancelación pedida; borrando los temporales…");
                return;
            }
        }
        // La guarda se conserva hasta terminar el proceso: el hilo principal no
        // puede escribir en stderr mientras se pinta la interrupción.
        let mut report = avi_term::report::lock();
        report_interruption(&mut report);
        if let Some(pid) = daemon_to_kill_on_interrupt() {
            daemon::kill_tree_by_pid(pid);
            daemon::wait_for_pid_death(pid, CTRL_C_CLEANUP_DEADLINE);
        }
        // Exit code 130 = interrumpido por usuario (Ctrl+C)
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
    avi_term::line::init_console();
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
    avi_term::line::set_machine_mode(json_mode);
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
        }) => handle_denoise(json_mode, args),
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

/// Invocación real del usuario: el nombre del programa seguido de los
/// argumentos recibidos. Los que llevan espacios van entre comillas dobles.
fn invocation_line() -> String {
    std::iter::once(APP_NAME.to_string())
        .chain(std::env::args_os().skip(1).map(|arg| {
            let arg = arg.to_string_lossy();
            if arg.contains(char::is_whitespace) {
                format!("\"{arg}\"")
            } else {
                arg.into_owned()
            }
        }))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Escribe un error en texto por stderr: `Error:` y, si el `reason` tiene
/// solución en la tabla, `Solución:` con `{cmd}` sustituido por la invocación
/// real. El detalle técnico, si lo hay, va al registro de trazas. Es el único
/// camino de escritura de `Error:` de los comandos.
fn print_error_text(err: &CliError) {
    // Tras una etapa no transitoria, el texto de error va detrás del separador.
    if avi_term::report::snapshot().has_permanent_stage() {
        avi_term::line::separator();
    }
    let solution = avi_core::exit_codes::solution_for(&err.reason)
        .map(|solution| solution.replace("{cmd}", &invocation_line()));
    avi_term::line::error(&err.message, solution.as_deref());
    if let Some(detail) = &err.detail {
        tracing::debug!(reason = %err.reason, detail = %detail, "detalle técnico del error");
    }
}

/// Texto del campo `error` del envelope: el mensaje con su punto final y, si lo
/// hay, el detalle técnico.
fn error_envelope_text(err: &CliError) -> String {
    let message = avi_term::fmt::sentence(&err.message);
    match &err.detail {
        Some(detail) => format!("{message} Detalle: {detail}"),
        None => message,
    }
}

/// Escribe el error por stderr (igual con y sin `--json`: el modo máquina solo
/// cambia stdout) y, con `--json`, emite además su envelope con el `reason`.
/// Sale con el código del error.
fn exit_with_error(json_mode: bool, err: CliError) -> ! {
    print_error_text(&err);
    if json_mode {
        emit_raw_json(json!({
            "error": error_envelope_text(&err),
            "reason": err.reason,
        }));
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

/// Cierra el indicador de un comando de voz según el resultado del trabajo: con
/// éxito escribe la línea de cierre `✓ <acción> en <total>` seguida de los
/// extras que calcula `extras` a partir del valor; con error borra la línea
/// activa sin escribir nada.
fn finish_progress<T, E>(
    progress: PhaseProgress,
    result: &Result<T, E>,
    action: &str,
    extras: impl FnOnce(&T) -> Vec<String>,
) {
    match result {
        Ok(value) => {
            progress.finish(action, &extras(value));
        }
        Err(_) => progress.fail(),
    }
}

/// Extras del cierre de un comando con síntesis en streaming: `primer audio X`,
/// si hubo reproducción, y los cortes, solo si hubo alguno. El primer audio es
/// el mismo valor que lleva la línea de `Síntesis` y el JSON.
fn streaming_closing_extras(first_audio_ms: Option<u64>, stalls: u32) -> Vec<String> {
    let first_audio = first_audio_ms.map(|ms| {
        format!(
            "primer audio {}",
            avi_term::fmt::duration(std::time::Duration::from_millis(ms))
        )
    });
    let cuts = (stalls > 0).then(|| avi_term::fmt::count(u64::from(stalls), "corte", "cortes"));
    first_audio.into_iter().chain(cuts).collect()
}

/// Ruta efectiva de un comando para su línea de contexto.
fn route_label(use_daemon: bool) -> &'static str {
    if use_daemon {
        "daemon"
    } else {
        "local"
    }
}

/// Idiomas de un comando para su línea de contexto: `es` sin traducción y
/// `es → en` con ella.
fn languages_label(source: &str, target: &str) -> String {
    if source == target {
        source.to_string()
    } else {
        format!("{source} → {target}")
    }
}

/// Tope, en segundos, de una grabación con micrófono: `--duration` o, sin ella,
/// el menor entre el techo de push-to-talk y el límite `cap` del comando.
fn mic_max_secs(duration: Option<u64>, cap: u64) -> Result<u64, CliError> {
    match duration {
        Some(secs) => Ok(secs),
        None => Ok(push_to_talk_max_secs()?.min(cap)),
    }
}

/// Fuente de audio de un comando para su línea de contexto: `archivo <nombre
/// sin carpeta>` o `micrófono (máximo m:ss)`.
fn audio_source_label(audio: Option<&str>, mic_max: Option<u64>) -> String {
    match (audio, mic_max) {
        (_, Some(max)) => format!(
            "micrófono (máximo {})",
            avi_term::fmt::clock(std::time::Duration::from_secs(max))
        ),
        (Some(path), None) => {
            let name = std::path::Path::new(path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.to_string());
            format!("archivo {name}")
        }
        (None, None) => "archivo".to_string(),
    }
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
fn handle_denoise(json_mode: bool, args: denoise::cli::Cli) -> Result<Outcome, CliError> {
    use denoise::cli::BatchOutcome;
    let cancel_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    drop(DENOISE_CANCEL.set(cancel_flag.clone()));
    // Un error de validación previo al lote sale por `exit_with_error`.
    let outcome = denoise::cli::execute(
        args,
        json_mode,
        true,
        denoise::ffmpeg_io::find_bundled_ffmpeg,
        Some(cancel_flag.clone()),
    )?;
    match outcome {
        BatchOutcome::Done => Ok(Outcome::Done),
        BatchOutcome::Cancelled => {
            if cancel_flag.load(std::sync::atomic::Ordering::SeqCst) {
                Ok(Outcome::Verdict(ExitCode::Interrupted.code()))
            } else {
                Ok(Outcome::Verdict(3))
            }
        }
        // El lote ya escribió sus líneas por archivo: en `--json` no se
        // escribe un segundo documento y en texto solo el error.
        BatchOutcome::Failed { code, error } => {
            if !json_mode {
                print_error_text(&error);
            }
            Ok(Outcome::Verdict(code))
        }
    }
}

fn handle_devices(json_mode: bool) -> Result<(), CliError> {
    let report = audio::get_devices_json().map_err(|e| {
        audio_error(e, ExitCode::Error, "audio_enumeration_failed", |e| {
            e.to_string()
        })
    })?;
    if json_mode {
        emit_raw_json(report);
    } else {
        println!("Salidas de audio:");
        let default_output = report["default_output"]["name"].as_str();
        for dev in report["devices"].as_array().into_iter().flatten() {
            println!("{}", device_line(dev, default_output));
        }
        println!(
            "{}",
            default_device_line("Entrada predeterminada", &report["default_input"])
        );
    }
    Ok(())
}

/// Línea de texto de un dispositivo predeterminado del JSON de
/// `avi_audio::get_devices_json`: `null` es «ninguna» (el sistema no tiene) y
/// un nombre `null` es «desconocido», como en `device_line`.
fn default_device_line(label: &str, device: &Value) -> String {
    let name = if device.is_null() {
        "ninguna"
    } else {
        device["name"].as_str().unwrap_or("desconocido")
    };
    format!("{label}: {name}")
}

/// Línea de texto de una salida del JSON de `avi_audio::get_devices_json`:
/// `  [<id>] <nombre> · latencia <x,y> ms`, con ` · predeterminada` si su nombre
/// es el de la salida predeterminada. El nombre `null` se muestra como
/// «desconocido» y la latencia `null` como «latencia desconocida».
fn device_line(dev: &Value, default_output: Option<&str>) -> String {
    let name = dev["name"].as_str().unwrap_or("desconocido");
    let latency = match dev["latency"].as_f64() {
        Some(secs) => format!(
            "latencia {} ms",
            format!("{:.1}", secs * 1000.0).replace('.', ",")
        ),
        None => "latencia desconocida".to_string(),
    };
    let default = if default_output.is_some_and(|d| dev["name"].as_str() == Some(d)) {
        " · predeterminada"
    } else {
        ""
    };
    format!("  [{}] {} · {}{}", dev["id"], name, latency, default)
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

/// Error `daemon_unreachable` por un fallo de conexión: el mensaje está en
/// español y el texto de la biblioteca HTTP (en inglés) va al detalle.
fn daemon_connect_error(addr: &str, cause: &dyn std::fmt::Display) -> CliError {
    CliError::new(
        ExitCode::DaemonUnreachable,
        "daemon_unreachable",
        format!("No se pudo conectar con el daemon en {addr}."),
    )
    .with_detail(cause.to_string())
}

/// Error `model_missing` del modelo de traducción del par de `source`: nombra el
/// modelo sin rutas de la caché; la orden para descargarlo es la solución.
fn translation_model_missing(source: &str) -> CliError {
    CliError::new(
        ExitCode::ModelMissing,
        "model_missing",
        format!(
            "Falta el modelo de traducción {}.",
            translation_pair(source).replace('-', " → ")
        ),
    )
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
            "El texto a traducir está vacío.",
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
    let use_daemon = route_to_daemon(daemon_mode).await?;
    // La cabecera va tras validar la invocación y antes del primer acceso a
    // recursos (modelo, daemon): un error de recurso sale con ella.
    avi_term::line::context(&format!(
        "Traducción {} · {}",
        languages_label(source, target),
        route_label(use_daemon)
    ));
    if use_daemon {
        // Única etapa del comando: no deja su línea y queda solo el cierre.
        let progress = PhaseProgress::new().single_stage();
        progress.phase(&stage::TRANSLATING);
        let result = translate_via_daemon(&client, text, source, target, &progress).await;
        if let Ok((translated, _, _)) = &result {
            progress.result(None, translated);
        }
        finish_progress(progress, &result, "Traducción completada", |_| Vec::new());
        let (translated, source, target) = result?;
        if json_mode {
            emit_raw_json(
                json!({ "translated": translated, "source_language": source, "target_language": target }),
            );
        }
        return Ok(());
    }
    // Rama local: verifica modelo y traduce
    let model_name = format!("opus-mt-{}", translation_pair(source));
    let model_store = open_model_store()?;
    #[cfg(feature = "native-translation")]
    let ct2_dir = model_store.model_dir(&model_name);
    if !model_store.is_provisioned(&model_name) {
        return Err(translation_model_missing(source));
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
        // Única etapa del comando: no deja su línea y queda solo el cierre.
        let progress = PhaseProgress::new().single_stage();
        progress.phase(&stage::TRANSLATING);
        progress.tick();
        // Patrón de bloque externo: el trabajo que puede fallar vive en un
        // bloque, y el cierre del indicador —que consume el indicador— se hace
        // fuera, sobre el resultado ya obtenido.
        let result = progress
            .while_running(|| translation::translate(text, &ct2_dir))
            .map_err(|e| {
                CliError::new(
                    ExitCode::TranslationFailed,
                    "translation_failed",
                    e.to_string(),
                )
            });
        if let Ok(translated) = &result {
            progress.result(None, translated);
        }
        finish_progress(progress, &result, "Traducción completada", |_| Vec::new());
        let translated = match result {
            Ok(translated) => translated,
            Err(e) => return Err(e),
        };
        if json_mode {
            emit_raw_json(
                json!({ "translated": translated, "source_language": source, "target_language": target }),
            );
        }
        Ok(())
    }
}

/// Traducción opt-in previa a la síntesis: passthrough si origen y destino
/// coinciden; exit 4 si falta el modelo, exit 9 si falla la traducción. La fase
/// ya la abrió quien llama; aquí solo se repinta mientras dura la traducción.
fn translate_if_different(
    text: &str,
    source: &str,
    target: &str,
    progress: &PhaseProgress,
) -> Result<String, CliError> {
    if source == target {
        return Ok(text.to_string());
    }
    let model_name = format!("opus-mt-{}", translation_pair(source));
    let model_store = open_model_store()?;
    #[cfg(feature = "native-translation")]
    let ct2_dir = model_store.model_dir(&model_name);
    if !model_store.is_provisioned(&model_name) {
        return Err(translation_model_missing(source));
    }
    #[cfg(not(feature = "native-translation"))]
    {
        let _ = progress;
        Err(CliError::new(
            ExitCode::Error,
            "translation_unsupported",
            "Este binario se compiló sin soporte de traducción (feature 'native-translation').",
        ))
    }
    #[cfg(feature = "native-translation")]
    {
        progress
            .while_running(|| translation::translate(text, &ct2_dir))
            .map_err(|e| {
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
                println!("Voces:");
                for v in &voices {
                    let tag = if v.is_factory { " (de fábrica)" } else { "" };
                    println!("  {}{}", v.name, tag);
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
            yes,
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
            // Con archivo y sin limpieza de ruido la clonación es la única etapa
            // de trabajo: no deja su línea y queda solo el cierre. Con micrófono
            // o con limpieza hay más de una etapa y cada una queda fija.
            let progress = if mic || denoise {
                PhaseProgress::new()
            } else {
                PhaseProgress::new().single_stage()
            };
            // Patrón de bloque externo: el trabajo que puede fallar vive en un
            // bloque async y el cierre del indicador —que consume el indicador—
            // se hace fuera, sobre el resultado ya obtenido. Así ningún `?`
            // puede dejar la fase abierta.
            let result = async {
                progress.phase(&stage::VALIDATING);
                // Orden de validaciones del contrato: el nombre se valida antes que el modelo.
                let name = VoiceName::parse(&name).map_err(invalid_name_cli_error)?;
                // Una voz de fábrica es el asset embebido, que la inicialización del
                // almacén restituye: no se clona encima.
                if avi_store::is_factory_name(name.as_str()) {
                    return Err(CliError::new(
                        ExitCode::InvalidInput,
                        "cannot_clone_default",
                        format!("La voz {name} es de fábrica y no se puede clonar."),
                    ));
                }
                // El archivo conserva el orden de validación vigente; el PCM de micrófono
                // se limita tras la captura antes de codificarlo.
                if !mic {
                    if let Some(reference) = speech_reference.as_deref() {
                        let reference = std::path::Path::new(reference);
                        ensure_clone_reference_within_limit(reference)?;
                        // La existencia del archivo es parte de la invocación,
                        // en las dos rutas: se comprueba antes que el almacén.
                        if !reference.is_file() {
                            return Err(CliError::new(
                                ExitCode::NotFound,
                                "audio_not_found",
                                format!(
                                    "El audio de referencia '{}' no existe.",
                                    reference.display()
                                ),
                            ));
                        }
                    }
                }
                if mic {
                    if duration == Some(0) {
                        return Err(CliError::new(
                            ExitCode::InvalidInput,
                            "usage_error",
                            "--duration debe ser mayor que cero.",
                        ));
                    }
                    if duration.is_some_and(|secs| secs > avi_core::MAX_CLONE_REFERENCE_SECS) {
                        return Err(CliError::new(
                            ExitCode::InvalidInput,
                            "audio_too_long",
                            "La duración de grabación supera el límite máximo de clonado (30 s).",
                        ));
                    }
                    if duration.is_none() {
                        if !std::io::IsTerminal::is_terminal(&std::io::stdin()) {
                            return Err(CliError::new(
                                ExitCode::InvalidInput,
                                "usage_error",
                                "--mic necesita --duration cuando no hay una terminal.",
                            ));
                        }
                        match std::env::var("AVI_PUSH_TO_TALK_MAX_SECS") {
                            Ok(value) => {
                                clone_push_to_talk_limit(Some(&value))?;
                            }
                            Err(std::env::VarError::NotPresent) => {
                                clone_push_to_talk_limit(None)?;
                            }
                            Err(std::env::VarError::NotUnicode(_)) => {
                                push_to_talk_max_secs()?;
                            }
                        }
                    }
                }
                let client = daemon_client();
                let use_daemon = route_to_daemon(routing.mode()).await?;
                let mic_max = if mic {
                    Some(mic_max_secs(duration, avi_core::MAX_CLONE_REFERENCE_SECS)?)
                } else {
                    None
                };
                // El cliente y el daemon comparten el directorio de datos, así
                // que una voz existente se rechaza antes de abrir el micrófono
                // o leer la referencia, en las dos rutas; el daemon conserva su
                // propia comprobación como defensa.
                if !force
                    && voice_store
                        .exists(&name)
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
                {
                    return Err(CliError::new(
                        ExitCode::StateConflict,
                        "voice_exists",
                        format!("La voz '{}' ya existe.", name),
                    ));
                }
                // La cabecera va tras validar la invocación y el almacén, y
                // antes de la confirmación y de cualquier recurso.
                avi_term::line::context(&format!(
                    "Clonación de la voz {} · {} · {}",
                    name,
                    audio_source_label(speech_reference.as_deref(), mic_max),
                    route_label(use_daemon)
                ));
                // Sobrescribir la voz se lleva también sus locuciones, grabadas
                // con el contenido anterior: si las hay, se pide la confirmación
                // destructiva antes de capturar o clonar, en las dos rutas.
                let speech_store = open_speech_store()?;
                if force {
                    let labels = speech_store
                        .labels_of(&name)
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
                    if !labels.is_empty() {
                        progress.yield_line();
                        let decision = confirm_voice_replacement(
                            &voice_store,
                            &speech_store,
                            &name,
                            &labels,
                            yes,
                        );
                        progress.reclaim();
                        if decision? == lifecycle::confirm::Decision::Cancelled {
                            return Ok(None);
                        }
                    }
                }
                let (speech_path, daemon_audio, mic_scratch) = if mic {
                    let capture_rate: u32 = if denoise { 48_000 } else { 24_000 };
                    let mut pcm = capture_mic_with_progress(
                        &progress,
                        capture_rate,
                        duration,
                        avi_core::MAX_CLONE_REFERENCE_SECS,
                    )
                    .await?;
                    truncate_clone_pcm(&mut pcm, capture_rate);
                    let wav = avi_audio::encode_pcm_mono_wav(&pcm, capture_rate)
                        .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
                    if use_daemon {
                        (None, Some(wav), None)
                    } else {
                        let guard =
                            avi_process::scratch::temp_wav("avi_clone_mic_").map_err(|e| {
                                CliError::new(ExitCode::Error, "io_error", e.to_string())
                            })?;
                        std::fs::write(guard.path(), wav).map_err(|e| {
                            CliError::new(
                                ExitCode::Error,
                                "io_error",
                                format!("{}: {e}", guard.path().display()),
                            )
                        })?;
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
                            std::fs::read(reference).map_err(|e| {
                                CliError::new(
                                    ExitCode::NotFound,
                                    "audio_not_found",
                                    format!(
                                        "El audio de referencia '{}' no existe: {}",
                                        reference, e
                                    ),
                                )
                            })?
                        }
                    };
                    let (cloned_name, cloned_speech, precomputed, removed_speech) =
                        clone_via_daemon(
                            &client,
                            name.as_str(),
                            &audio_bytes,
                            force,
                            denoise,
                            &progress,
                        )
                        .await?;
                    return Ok(Some((
                        cloned_name,
                        PathBuf::from(cloned_speech),
                        precomputed,
                        removed_speech,
                    )));
                }
                let _mic_scratch = mic_scratch;
                let speech_path = speech_path.expect("ruta local");
                // Limpieza de ruido de referencia (opt-in) antes del clonado
                // directo. Resuelve el modelo y verifica integridad antes de
                // convertir a 48 kHz; un modelo ausente falla con `model_missing`.
                let speech_path = speech_path.to_path_buf();
                // El WAV limpio vive en este directorio temporal, así que debe
                // sobrevivir hasta que termine el clonado: si se suelta al salir
                // del bloque, la ruta queda apuntando a un archivo borrado.
                #[cfg(feature = "native-denoise")]
                let (_denoise_scratch, speech_path) = if denoise {
                    // La limpieza es trabajo de varios segundos: tiene su etapa
                    // visible, que cierra la etapa siguiente.
                    progress.phase(&stage::DENOISING);
                    let work_dir =
                        avi_process::scratch::tempdir("avi_clone_denoise_").map_err(|e| {
                            CliError::new(ExitCode::Error, "io_error", format!("{e:#}"))
                        })?;
                    let cleaned_path = work_dir.path().join("reference_clean.wav");
                    let models =
                        denoise::service::ServiceModels::resolve().map_err(|e| e.to_cli_error())?;
                    progress
                        .while_running(|| {
                            models.denoise_wav_file(
                                speech_path.as_path(),
                                &cleaned_path,
                                24_000,
                                work_dir.path(),
                            )
                        })
                        .map_err(|e| e.to_cli_error())?;
                    (Some(work_dir), cleaned_path)
                } else {
                    (None, speech_path)
                };

                progress.yield_line();
                let engine_result = direct_tts_engine();
                progress.reclaim();
                let engine = engine_result?;
                let model_dir = engine.base_model_dir.as_ref().ok_or_else(|| {
                    CliError::new(
                        ExitCode::ModelMissing,
                        "model_missing",
                        "Falta el modelo de clonación de voz.",
                    )
                })?;
                // Temporal con dueño: se borra al soltarse por cualquier camino y,
                // si el proceso muere antes, el barrido del scratch lo recoge.
                let tmp_qvoice = avi_process::scratch::tempfile("avi_clone_", ".qvoice")
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?
                    .into_temp_path();
                progress.phase(&stage::CLONING);
                progress.tick();
                let cloned = progress
                    .while_running(|| {
                        avi_tts::clone_voice(model_dir, speech_path.as_path(), &tmp_qvoice)
                    })
                    .and_then(|()| {
                        progress.phase(&stage::SAVING);
                        avi_store::replace_voice(&voice_store, &speech_store, &name, &tmp_qvoice)
                    });
                let replaced = cloned.map_err(|e| {
                    // Una referencia ilegible se clasifica igual que en transcripción.
                    match e.downcast_ref::<avi_audio::WavLoadError>() {
                        Some(wav @ avi_audio::WavLoadError::Invalid(_)) => {
                            CliError::new(ExitCode::InvalidInput, "invalid_audio", wav.to_string())
                                .with_detail(wav.detail().unwrap_or_default())
                        }
                        Some(wav @ avi_audio::WavLoadError::Io(_)) => {
                            CliError::new(ExitCode::Error, "io_error", wav.to_string())
                                .with_detail(wav.detail().unwrap_or_default())
                        }
                        _ => CliError::new(
                            ExitCode::Error,
                            "voice_clone_failed",
                            "No se pudo clonar la voz.",
                        )
                        .with_detail(e.to_string()),
                    }
                })?;
                Ok(Some((
                    name.to_string(),
                    replaced.reference,
                    false,
                    replaced.removed_speech,
                )))
            }
            .await;
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    progress.fail();
                    return Err(e);
                }
            };
            let Some((cloned_name, saved_qvoice, precomputed, removed_speech)) = outcome else {
                progress.fail();
                // `confirm::confirm` ya escribió «Cancelado.» por stderr.
                if json_mode {
                    emit_raw_json(json!({
                        "status": "cancelled",
                        "name": name.to_lowercase(),
                        "removed_speech": Vec::<String>::new(),
                    }));
                }
                return Ok(());
            };
            let mut extras = Vec::new();
            if !removed_speech.is_empty() {
                extras.push(format!(
                    "se borraron {}",
                    avi_term::fmt::count(
                        removed_speech.len() as u64,
                        "audio sintetizado anterior",
                        "audios sintetizados anteriores"
                    )
                ));
            }
            progress.finish(&format!("Voz {cloned_name} clonada"), &extras);
            // La única referencia de habla produce `reference.qvoice`; el WAV de
            // entrada no se persiste. La ruta local no tiene residente TTS
            // persistente, así que `precomputed` es `false` en esa vía.
            if json_mode {
                emit_raw_json(json!({
                    "name": cloned_name,
                    "speech": saved_qvoice.to_string_lossy().to_string(),
                    "precomputed": precomputed,
                    "removed_speech": removed_speech,
                }));
            }
            Ok(())
        }
        VoiceCommands::Remove { name, yes } => {
            let name = VoiceName::parse(&name).map_err(invalid_name_cli_error)?;
            // Validación estructural: fábrica protegida se rechaza antes de tocar el store,
            // sin depender del mensaje de error (evita string matching frágil).
            if avi_store::is_factory_name(name.as_str()) {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "cannot_remove_default",
                    format!("La voz {name} es de fábrica y no se puede borrar."),
                ));
            }
            if !voice_store
                .exists(&name)
                .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?
            {
                return Err(CliError::new(
                    ExitCode::NotFound,
                    "voice_not_found",
                    format!("La voz '{}' no existe.", name),
                ));
            }
            // Los audios sintetizados se van con su voz. Siempre se muestra el
            // plan y se pide la confirmación destructiva antes de borrar nada,
            // también cuando la voz no tiene ninguno.
            let speech_store = open_speech_store()?;
            let labels = speech_store
                .labels_of(&name)
                .map_err(|e| CliError::new(ExitCode::Error, "io_error", e.to_string()))?;
            let voice_dir = voice_store.voice_dir(&name);
            let speech_dir = speech_store.voice_dir(&name);
            let (summary, question, entries) = if labels.is_empty() {
                (
                    vec![format!("Se borrará la voz {name}:")],
                    format!("¿Borrar la voz {name}?"),
                    vec![lifecycle::confirm::PlanEntry {
                        size: Some(lifecycle::cleanup::path_size(&voice_dir)),
                        path: voice_dir,
                    }],
                )
            } else {
                let speech_count = avi_term::fmt::count(
                    labels.len() as u64,
                    "audio sintetizado",
                    "audios sintetizados",
                );
                (
                    vec![format!("Se borrará la voz {name} y sus {speech_count}:")],
                    format!("¿Borrar la voz {name} y sus {speech_count}?"),
                    vec![
                        lifecycle::confirm::PlanEntry {
                            size: Some(lifecycle::cleanup::path_size(&voice_dir)),
                            path: voice_dir,
                        },
                        lifecycle::confirm::PlanEntry {
                            size: Some(lifecycle::cleanup::path_size(&speech_dir)),
                            path: speech_dir,
                        },
                    ],
                )
            };
            let decision = {
                let stdin = std::io::stdin();
                let mut input = stdin.lock();
                // La espera de la respuesta no cuenta como trabajo del comando.
                avi_term::report::begin_question();
                let decision = lifecycle::confirm::confirm(
                    &lifecycle::confirm::Confirmation {
                        kind: lifecycle::confirm::Kind::Destructive,
                        question: &question,
                        summary: &summary,
                        entries: &entries,
                        assume_yes: yes,
                        dry_run: false,
                        stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
                    },
                    &mut input,
                    &mut std::io::stderr(),
                );
                avi_term::report::end_question(matches!(
                    decision,
                    Ok(lifecycle::confirm::Decision::Proceed)
                ));
                decision.map_err(lifecycle_error_to_cli)?
            };
            if decision == lifecycle::confirm::Decision::Cancelled {
                if json_mode {
                    emit_raw_json(json!({
                        "status": "cancelled",
                        "voice": name,
                        "removed_speech": Vec::<String>::new(),
                    }));
                }
                // `confirm::confirm` ya escribió «Cancelado; no se ha borrado
                // nada.» por stderr: el handler no lo repite, porque el literal
                // es del motor de confirmación y saldría dos veces.
                return Ok(());
            }
            // Única etapa del comando: no deja su línea y queda solo el cierre.
            let progress = PhaseProgress::new().single_stage();
            let result = 'work: {
                progress.phase(&stage::REMOVING);
                break 'work avi_store::remove_voice(&voice_store, &speech_store, &name)
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", e));
            };
            finish_progress(
                progress,
                &result,
                &format!("Voz {name} borrada"),
                |removed_speech| {
                    if removed_speech.is_empty() {
                        Vec::new()
                    } else {
                        vec![format!(
                            "y {}",
                            avi_term::fmt::count(
                                removed_speech.len() as u64,
                                "audio sintetizado",
                                "audios sintetizados"
                            )
                        )]
                    }
                },
            );
            let removed_speech = result?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "removed",
                    "voice": name,
                    "removed_speech": removed_speech,
                }));
            }
            Ok(())
        }
    }
}

// ─── Speech ──────────────────────────────────────────────────────────

/// Captura PCM del micrófono en `spawn_blocking`: `duration` fija la
/// Captura PCM del micrófono a `target_rate` Hz (16, 24 o 48 kHz) en modo
/// duración fija o push-to-talk: `Some(secs)` captura por N segundos; `None`
/// (solo alcanzable en TTY, ver guardas de `--duration`) dispara push-to-talk
/// hasta Enter (`capture_mono_pcm_until_enter`). `max_secs` es el techo de
/// duración del push-to-talk (se combina con el techo de entorno).
async fn capture_mic_pcm(
    target_rate: u32,
    duration: Option<u64>,
    max_secs: u64,
    observer: Arc<MicObserver>,
) -> Result<Vec<i16>, CliError> {
    if duration.is_none() {
        // Valida la variable antes de abrir el micrófono, para que una variable
        // inválida salga como `usage_error` y no como fallo de captura.
        push_to_talk_max_secs()?;
    }
    tokio::task::spawn_blocking(move || {
        let svc = audio::AudioService::new();
        match duration {
            Some(secs) => svc.capture_mono_pcm(target_rate, secs, observer),
            None => svc.capture_mono_pcm_until_enter(target_rate, max_secs, observer),
        }
    })
    .await
    .map_err(|e| {
        CliError::new(
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            e.to_string(),
        )
    })?
    .map_err(|e| {
        audio_error(
            e,
            ExitCode::TranscriptionFailed,
            "transcription_failed",
            |e| e.to_string(),
        )
    })
}

/// Clasificador único de los errores de las operaciones de audio (captura,
/// reproducción y enumeración). El entorno se clasifica por tipo: sin backend
/// de audio, sin entrada o sin salida predeterminada es un recurso ausente
/// (exit 3) y una captura sin muestras es entrada inválida (exit 2), sea cual
/// sea la operación. Lo demás conserva el `code`, el `reason` y el mensaje
/// propios de la operación.
fn audio_error(
    e: anyhow::Error,
    code: ExitCode,
    reason: &str,
    message: impl FnOnce(&anyhow::Error) -> String,
) -> CliError {
    if e.downcast_ref::<avi_audio::AudioBackendUnavailable>()
        .is_some()
    {
        CliError::new(
            ExitCode::NotFound,
            "audio_backend_unavailable",
            e.to_string(),
        )
    } else if e.downcast_ref::<avi_audio::NoInputDevice>().is_some() {
        CliError::new(ExitCode::NotFound, "no_input_device", e.to_string())
    } else if e.downcast_ref::<avi_audio::NoOutputDevice>().is_some() {
        CliError::new(ExitCode::NotFound, "no_output_device", e.to_string())
    } else if e.downcast_ref::<avi_audio::EmptyAudio>().is_some() {
        CliError::new(ExitCode::InvalidInput, "empty_audio", e.to_string())
    } else {
        CliError::new(code, reason, message(&e))
    }
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
fn audio_too_long(operation: &str, actual_secs: f64, max_secs: u64) -> CliError {
    CliError::new(
        ExitCode::InvalidInput,
        "audio_too_long",
        format!(
            "{operation} dura {} s y el máximo es {max_secs} s.",
            actual_secs.ceil() as u64
        ),
    )
}

/// Rechaza el WAV de `path` cuya cabecera declara más de `max_secs` segundos.
/// Un archivo ausente o que no es WAV no se mide: lo informa la lectura posterior.
fn ensure_wav_within_limit(
    path: &std::path::Path,
    operation: &str,
    max_secs: u64,
) -> Result<(), CliError> {
    match avi_audio::wav_duration_secs(path) {
        Some(secs) if secs > max_secs as f64 => Err(audio_too_long(operation, secs, max_secs)),
        _ => Ok(()),
    }
}

/// Rechaza el PCM mono a `sample_rate` Hz que supera `max_secs` segundos.
fn ensure_pcm_within_limit(
    pcm: &[i16],
    sample_rate: u32,
    operation: &str,
    max_secs: u64,
) -> Result<(), CliError> {
    if pcm.len() as u64 > max_secs * u64::from(sample_rate) {
        return Err(audio_too_long(
            operation,
            pcm.len() as f64 / f64::from(sample_rate),
            max_secs,
        ));
    }
    Ok(())
}

/// Limita el PCM del micrófono a 30 segundos a `sample_rate` Hz sin rellenar las capturas cortas.
fn truncate_clone_pcm(pcm: &mut Vec<i16>, sample_rate: u32) {
    pcm.truncate((avi_core::MAX_CLONE_REFERENCE_SECS * u64::from(sample_rate)) as usize);
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

fn clone_push_to_talk_limit(value: Option<&str>) -> Result<u64, CliError> {
    let limit = match value {
        None => avi_core::MAX_CLONE_REFERENCE_SECS,
        Some(value) => value.trim().parse::<u64>().map_err(|_| {
            CliError::new(ExitCode::InvalidInput, "usage_error", format!("AVI_PUSH_TO_TALK_MAX_SECS='{value}' no es válida; debe ser un entero de segundos sin signo"))
        })?,
    };
    validate_clone_push_to_talk_limit(limit)?;
    Ok(limit)
}

#[cfg(test)]
mod clone_push_to_talk_tests {
    #[test]
    fn absent_ptt_limit_uses_clone_maximum() {
        assert_eq!(
            super::clone_push_to_talk_limit(None).expect("techo predeterminado"),
            30
        );
    }

    #[test]
    fn explicit_ptt_limit_is_validated() {
        for value in ["1", "30"] {
            assert_eq!(
                super::clone_push_to_talk_limit(Some(value)).expect("techo válido"),
                value.parse::<u64>().unwrap()
            );
        }
        assert_eq!(
            super::clone_push_to_talk_limit(Some("31"))
                .unwrap_err()
                .reason,
            "audio_too_long"
        );
        assert_eq!(
            super::clone_push_to_talk_limit(Some("inválido"))
                .unwrap_err()
                .reason,
            "usage_error"
        );
    }

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
    // La cabecera ya acotó la duración y un WAV que se decodifica entero tiene
    // las muestras que declara: aquí solo se comprueba que el archivo está íntegro.
    match avi_audio::validate_wav_samples(path) {
        Ok(()) => Ok(()),
        Err(e @ avi_audio::WavLoadError::Invalid(_)) => {
            Err(
                CliError::new(ExitCode::InvalidInput, "invalid_audio", e.to_string())
                    .with_detail(e.detail().unwrap_or_default()),
            )
        }
        // Una referencia ausente o con fallo de E/S la clasifica el clonado.
        Err(_) => Ok(()),
    }
}

/// Cuántos niveles recientes dibuja el medidor de la línea de grabación.
const METER_LEVELS: usize = 5;

/// Estado de una grabación con micrófono, que el hilo de captura actualiza y el
/// lado asíncrono lee para pintar la línea viva.
#[derive(Default)]
struct MicCaptureState {
    /// Tiempo grabado.
    elapsed: std::time::Duration,
    /// Tope de la grabación; cero mientras no ha empezado.
    max: std::time::Duration,
    /// Últimos niveles, en dBFS, del más antiguo al más reciente.
    levels: std::collections::VecDeque<f32>,
    /// La grabación la detiene el usuario con Enter.
    until_enter: bool,
    /// La grabación terminó porque venció su tope.
    limit_reached: bool,
}

/// Observador de la captura del micrófono: solo guarda el estado. No toca el
/// indicador, porque la biblioteca de audio lo llama desde su hilo bloqueante.
#[derive(Default)]
struct MicObserver {
    state: std::sync::Mutex<MicCaptureState>,
}

impl MicObserver {
    fn lock(&self) -> std::sync::MutexGuard<'_, MicCaptureState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Línea viva de la grabación, o `None` mientras no ha empezado:
    /// `● Grabando m:ss / m:ss  <medidor>` y, si la detiene Enter,
    /// `  Enter para detener`. El `●` va en rojo.
    fn live_line(&self) -> Option<String> {
        let state = self.lock();
        if state.max.is_zero() {
            return None;
        }
        let levels: Vec<f32> = state.levels.iter().copied().collect();
        let mut line = format!(
            "{} Grabando {} / {}  {}",
            avi_term::style::paint(
                avi_term::style::Stream::Stderr,
                avi_term::style::Tone::Error,
                avi_term::style::REC
            ),
            avi_term::fmt::clock(state.elapsed),
            avi_term::fmt::clock(state.max),
            avi_term::fmt::level_meter(&levels)
        );
        if state.until_enter {
            line.push_str("  Enter para detener");
        }
        Some(line)
    }

    fn limit_reached(&self) -> bool {
        self.lock().limit_reached
    }
}

impl audio::CaptureObserver for MicObserver {
    fn started(&self, max: std::time::Duration, until_enter: bool) {
        let mut state = self.lock();
        state.max = max;
        state.until_enter = until_enter;
    }

    fn progress(&self, elapsed: std::time::Duration, level_dbfs: f32) {
        let mut state = self.lock();
        state.elapsed = elapsed;
        if state.levels.len() == METER_LEVELS {
            state.levels.pop_front();
        }
        state.levels.push_back(level_dbfs);
    }

    fn limit_reached(&self) {
        self.lock().limit_reached = true;
    }
}

/// Cadencia con la que la línea de grabación se actualiza.
const CAPTURE_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

/// Valida el PCM adquirido: un PCM vacío se rechaza con exit 2
/// (`empty_audio`) y uno que supera `max_secs` segundos con exit 2
/// (`audio_too_long`); `subject` es el sujeto de la frase del límite.
fn validate_acquired_pcm(
    pcm: &[i16],
    sample_rate: u32,
    max_secs: u64,
    subject: &str,
) -> Result<(), CliError> {
    if pcm.is_empty() {
        return Err(CliError::new(
            ExitCode::InvalidInput,
            "empty_audio",
            "No se capturó audio; pulsa Enter después de hablar o verifica el micrófono.",
        ));
    }
    ensure_pcm_within_limit(pcm, sample_rate, subject, max_secs)
}

/// Captura PCM del micrófono abriendo la etapa `CAPTURING`. Con terminal, la
/// línea viva muestra el tiempo grabado, el tope y el nivel; sin terminal, el
/// texto de la etapa dice el tope una sola vez. Al terminar la captura cierra la
/// etapa y avisa si se alcanzó el límite de grabación o si no se detectó voz.
/// Los fallos son los de [`capture_mic_pcm`].
async fn capture_mic_with_progress(
    progress: &PhaseProgress,
    target_rate: u32,
    duration: Option<u64>,
    max_secs: u64,
) -> Result<Vec<i16>, CliError> {
    let shown_max = mic_max_secs(duration, max_secs)?;
    progress.phase_with(
        &stage::CAPTURING,
        &format!(
            "Grabando (máximo {})…",
            avi_term::fmt::clock(std::time::Duration::from_secs(shown_max))
        ),
    );
    let observer = Arc::new(MicObserver::default());
    let mut capture = Box::pin(capture_mic_pcm(
        target_rate,
        duration,
        max_secs,
        observer.clone(),
    ));
    let captured = loop {
        match tokio::time::timeout(CAPTURE_REPAINT_INTERVAL, &mut capture).await {
            Ok(result) => break result,
            Err(_elapsed) => {
                if let Some(line) = observer.live_line() {
                    progress.live(line);
                }
                progress.tick();
            }
        }
    };
    let pcm = captured?;
    // Una grabación detenida con Enter desde una terminal deja el eco del
    // Enter bajo la línea viva: se sube antes de que el indicador la limpie.
    if duration.is_none() && !observer.limit_reached() && std::io::stdin().is_terminal() {
        progress.climb_echoed_enter();
    }
    // Cierra la etapa de grabación antes de los avisos, para que su línea
    // terminada salga primero.
    progress.phase(&stage::VALIDATING);
    if observer.limit_reached() {
        avi_term::line::warn(format!(
            "Se alcanzó el límite de grabación ({}); se detiene",
            avi_term::fmt::clock(std::time::Duration::from_secs(shown_max))
        ));
    }
    let level = audio::loudest_window_dbfs(&pcm, target_rate);
    if level < audio::SILENCE_THRESHOLD_DBFS {
        avi_term::line::warn(format!(
            "No se detectó voz en la grabación (nivel máximo {level:.0} dBFS); comprueba que el micrófono no esté silenciado"
        ));
    }
    Ok(pcm)
}

/// Obtiene el PCM 16 kHz mono desde el micrófono o desde un WAV pintando la
/// etapa `CAPTURING` o `READING`, y lo valida con
/// [`validate_acquired_pcm`] (vacío y límite de `max_secs` con el sujeto
/// `subject`). Con `mic == false`, `audio` debe ser `Some`. Traduce los fallos
/// al contrato: archivo ausente exit 3, WAV inválido exit 2, error de E/S
/// exit 1; en la captura, sin entrada predeterminada o sin backend de audio
/// exit 3, audio vacío exit 2 y el resto exit 10 (`transcription_failed`).
/// `max_secs` es también el techo de la captura push-to-talk.
async fn acquire_pcm_with_progress(
    progress: &PhaseProgress,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    max_secs: u64,
    subject: &str,
) -> Result<Vec<i16>, CliError> {
    let pcm = if mic {
        capture_mic_with_progress(progress, 16_000, duration, max_secs).await?
    } else {
        progress.phase(&stage::READING);
        read_wav_pcm(audio.expect("validado arriba"))?
    };
    validate_acquired_pcm(&pcm, 16_000, max_secs, subject)?;
    Ok(pcm)
}

/// Lee un WAV como PCM 16 kHz mono y traduce los fallos al contrato.
fn read_wav_pcm(path: &str) -> Result<Vec<i16>, CliError> {
    avi_audio::load_wav_16k_mono_pcm(std::path::Path::new(path)).map_err(|e| {
        let error = match e {
            avi_audio::WavLoadError::NotFound => CliError::new(
                ExitCode::NotFound,
                "audio_not_found",
                format!("El archivo de audio '{path}' no existe."),
            ),
            avi_audio::WavLoadError::Invalid(_) => {
                CliError::new(ExitCode::InvalidInput, "invalid_audio", e.to_string())
            }
            avi_audio::WavLoadError::Io(_) => {
                CliError::new(ExitCode::Error, "io_error", e.to_string())
            }
        };
        match e.detail() {
            Some(detail) => error.with_detail(detail),
            None => error,
        }
    })
}

/// Decisión que toma el menú interactivo `--play` y que resuelve su llamador.
///
/// El menú no conoce el motor de síntesis: la opción 3 (repetir con una
/// síntesis nueva) se devuelve como `Resynthesize` y es el llamador quien
/// re-sintetiza y vuelve a entrar al menú.
#[derive(Debug, PartialEq)]
enum PlayDecision {
    /// El usuario eligió guardar (opción 2): este es el desenlace del guardado.
    Saved(avi_store::Persisted),
    /// El usuario descartó (opción 4) o se agotó la entrada estándar.
    Discarded,
    /// El usuario pidió una síntesis nueva (opción 3): el llamador re-sintetiza
    /// y reentra al menú.
    Resynthesize,
}

/// Desenlace de `speech synthesize`, con un `status` registrado por variante.
#[derive(Debug, PartialEq)]
enum SynthesizeOutcome {
    /// La locución se guardó en esta ruta (`success`).
    Saved(PathBuf),
    /// Ya existía en esta ruta y `--skip-existing` pidió conservarla (`skipped`).
    Skipped(PathBuf),
    /// El usuario la descartó en el menú `--play` (`discarded`).
    Discarded,
}

impl From<avi_store::Persisted> for SynthesizeOutcome {
    fn from(persisted: avi_store::Persisted) -> Self {
        match persisted {
            avi_store::Persisted::Saved(path) => Self::Saved(path),
            avi_store::Persisted::Skipped(path) => Self::Skipped(path),
        }
    }
}

/// Política de colisión de `speech synthesize` a partir de sus flags, que clap
/// hace mutuamente excluyentes. Es el único punto de conversión.
fn collision_policy(force: bool, skip_existing: bool) -> avi_store::CollisionPolicy {
    match (force, skip_existing) {
        (true, _) => avi_store::CollisionPolicy::Overwrite,
        (false, true) => avi_store::CollisionPolicy::Skip,
        (false, false) => avi_store::CollisionPolicy::Fail,
    }
}

/// Único punto de emisión de `speech synthesize`: traduce el desenlace a su
/// envelope JSON, o en texto a la ruta del WAV por stdout y la línea de cierre
/// por stderr. Cierra el indicador.
fn emit_synthesize(
    json_mode: bool,
    voice: &str,
    outcome: &SynthesizeOutcome,
    progress: PhaseProgress,
) {
    match outcome {
        SynthesizeOutcome::Saved(path) => {
            if json_mode {
                emit_raw_json(json!({
                    "status": "success",
                    "audio_path": path.to_string_lossy(),
                    "voice": voice,
                }));
            }
            progress.result(None, &path.display().to_string());
            // La duración del audio sale de las muestras del WAV guardado.
            let extras: Vec<String> = audio::wav_duration_secs(path)
                .map(|secs| {
                    format!(
                        "{} de audio",
                        avi_term::fmt::duration(std::time::Duration::from_secs_f64(secs))
                    )
                })
                .into_iter()
                .collect();
            progress.finish("Síntesis completada", &extras);
        }
        SynthesizeOutcome::Skipped(path) => {
            if json_mode {
                emit_raw_json(json!({
                    "status": "skipped",
                    "audio_path": path.to_string_lossy(),
                    "voice": voice,
                }));
            }
            progress.result(None, &path.display().to_string());
            progress.close();
            avi_term::line::ok("Ya existía; se conserva sin sintetizar de nuevo.");
        }
        SynthesizeOutcome::Discarded => {
            progress.fail();
            emit_play_discarded(json_mode, voice);
        }
    }
}

/// Envelope JSON de la locución descartada en el menú `--play`.
fn play_discarded_envelope(voice: &str) -> Value {
    json!({ "status": "discarded", "voice": voice })
}

/// Emite el descarte del menú `--play`: envelope JSON o línea de cierre por
/// stderr, según el modo.
fn emit_play_discarded(json_mode: bool, voice: &str) {
    if json_mode {
        emit_raw_json(play_discarded_envelope(voice));
    } else {
        avi_term::line::ok("Audio descartado; no se ha guardado.");
    }
}

/// Fuente de las opciones del menú `--play`: lee una línea de la entrada
/// estándar. Es inyectable para poder ejercitar el menú sin TTY.
fn menu_stdin() -> impl FnMut(&mut String) -> std::io::Result<usize> {
    |line: &mut String| std::io::stdin().read_line(line)
}

/// Seeds de las regeneraciones del menú `--play`: cada toma nueva usa un seed
/// que no se ha usado antes en este menú, y nunca el de producción, que es el
/// de la primera toma. El azar sale de la biblioteca estándar (`RandomState`
/// se inicializa con claves aleatorias del sistema por instancia).
struct RetakeSeeds {
    used: Vec<u64>,
}

impl RetakeSeeds {
    fn new() -> Self {
        Self {
            used: vec![avi_tts::PRODUCTION_SEED],
        }
    }

    /// Sortea un seed en `0..=MAX_ENGINE_SEED` ausente de la lista de usados, lo
    /// registra y lo devuelve.
    fn next(&mut self) -> u64 {
        use std::hash::{BuildHasher, Hasher};
        loop {
            let hasher = std::collections::hash_map::RandomState::new().build_hasher();
            let seed = hasher.finish() % (avi_tts::MAX_ENGINE_SEED + 1);
            if !self.used.contains(&seed) {
                self.used.push(seed);
                return seed;
            }
        }
    }
}

/// Re-síntesis del menú `--play`: la ejecuta el llamador, nunca el menú. Cada
/// implementación vuelve a sintetizar la locución sobre el mismo temporal.
///
/// El menú devuelve `PlayDecision::Resynthesize` y el llamador la resuelve con
/// esta pieza; así el menú no conoce ni el motor local ni el daemon. El
/// llamador del menú entrega el seed de cada regeneración.
trait PlayResynthesizer {
    /// Vuelve a sintetizar la locución sobre el temporal del menú con el seed
    /// indicado.
    fn resynthesize(
        &mut self,
        seed: u64,
    ) -> impl std::future::Future<Output = Result<(), CliError>>;
}

/// Re-síntesis directa: vuelve a sintetizar con el motor local, reproduciendo la
/// toma nueva mientras se sintetiza, y la deja en el mismo temporal. Se encarga
/// de su propio progreso.
struct LocalResynthesizer<'a> {
    engine: &'a Qwen3TtsEngine,
    reference: Option<&'a avi_store::VoiceReference>,
    final_text: &'a str,
    voice: &'a VoiceName,
    label: &'a Label,
    temperature: Option<f32>,
    target_language: &'a str,
    tmp_wav: &'a PathBuf,
    progress: &'a PhaseProgress,
}

impl PlayResynthesizer for LocalResynthesizer<'_> {
    async fn resynthesize(&mut self, seed: u64) -> Result<(), CliError> {
        self.progress.phase(&stage::SYNTHESIZING);
        self.progress.tick();
        let result = open_delivery(
            true,
            true,
            format!(
                "No se pudo reproducir el audio sintetizado '{}'",
                self.label
            ),
        )
        .and_then(|delivery| {
            synthesize_streaming_directly(
                &DirectSynthesis {
                    engine: self.engine,
                    reference: self.reference,
                    text: self.final_text,
                    voice: self.voice,
                    temperature: self.temperature,
                    seed: Some(seed),
                    language: self.target_language,
                },
                self.progress,
                delivery,
            )
        });
        // La etapa se cierra antes de que escriba el menú, también si la
        // síntesis falla: así su duración no incluye lo que tarde el usuario.
        self.progress.end_stage();
        write_take_wav(self.tmp_wav, &result?.pcm)
    }
}

/// Re-síntesis por daemon: vuelve a despachar la síntesis por la misma vía,
/// reproduce la toma nueva mientras llega y la deja en el mismo temporal.
struct DaemonResynthesizer<'a> {
    client: &'a reqwest::Client,
    text: &'a str,
    voice: &'a str,
    label: &'a Label,
    source_language: &'a str,
    target_language: &'a str,
    temperature: Option<f32>,
    tmp_wav: &'a PathBuf,
    progress: &'a PhaseProgress,
}

impl PlayResynthesizer for DaemonResynthesizer<'_> {
    async fn resynthesize(&mut self, seed: u64) -> Result<(), CliError> {
        // La fase «Sintetizando…» la pinta el consumo del stream del daemon.
        let result = match open_delivery(
            true,
            true,
            format!(
                "No se pudo reproducir el audio sintetizado '{}'",
                self.label
            ),
        ) {
            Ok(delivery) => {
                daemon_synthesize_streaming(
                    self.progress,
                    self.client,
                    self.text,
                    self.voice,
                    self.source_language,
                    self.target_language,
                    self.temperature,
                    Some(seed),
                    delivery,
                )
                .await
            }
            Err(e) => Err(e),
        };
        // La etapa se cierra antes de que escriba el menú, también si la
        // síntesis falla: así su duración no incluye lo que tarde el usuario.
        self.progress.end_stage();
        write_take_wav(self.tmp_wav, &result?.stats.pcm)
    }
}

/// Bucle del llamador del menú `--play`: ofrece el menú y, ante
/// `Resynthesize`, ejecuta la re-síntesis y reentra al menú con el mismo
/// temporal. Termina con el desenlace del guardado o con `Discarded` si el
/// usuario descartó. El bucle vive en el llamador porque el menú no conoce el
/// motor de síntesis.
// Firma ancha deliberada: cada argumento mapea 1:1 a un dato del menú.
#[allow(clippy::too_many_arguments)]
async fn play_menu_loop<R: PlayResynthesizer>(
    voice: &VoiceName,
    label: &Label,
    policy: avi_store::CollisionPolicy,
    voice_store: &VoiceStore,
    speech_store: &SpeechStore,
    reference: Option<&avi_store::VoiceReference>,
    final_text: &str,
    wav: &std::path::Path,
    reader: &mut dyn FnMut(&mut String) -> std::io::Result<usize>,
    playback: &mut dyn FnMut(&std::path::Path) -> Result<(), CliError>,
    resynthesizer: &mut R,
) -> Result<SynthesizeOutcome, CliError> {
    let mut seeds = RetakeSeeds::new();
    loop {
        match synthesize_play_loop(
            voice,
            label,
            policy,
            voice_store,
            speech_store,
            reference,
            final_text,
            wav,
            reader,
            playback,
        )? {
            PlayDecision::Saved(persisted) => return Ok(persisted.into()),
            PlayDecision::Discarded => return Ok(SynthesizeOutcome::Discarded),
            PlayDecision::Resynthesize => resynthesizer.resynthesize(seeds.next()).await?,
        }
    }
}

/// Reproducción del WAV en el menú `--play`, con el servicio de audio real. Es
/// inyectable para que las pruebas no dependan de la placa de sonido.
fn menu_playback<'a>(
    label: &Label,
    progress: &'a PhaseProgress,
) -> impl FnMut(&std::path::Path) -> Result<(), CliError> + 'a {
    let label = label.to_string();
    move |wav: &std::path::Path| {
        // La opción ya se leyó con su salto de línea, así que la etapa se pinta
        // en una línea libre; al terminar se cierra para que la pregunta del
        // menú no quede pegada a ella.
        progress.phase(&stage::PLAYING);
        let played = audio::AudioService::new()
            .play_wav(wav, &mut || progress.tick())
            .map_err(|e| {
                audio_error(e, ExitCode::Error, "playback_failed", |e| {
                    format!(
                        "No se pudo reproducir el audio sintetizado '{}': {}",
                        label, e
                    )
                })
            });
        progress.end_stage();
        played
    }
}

/// Menú interactivo `--play`: sobre la toma que ya sonó mientras se sintetizaba,
/// ofrece 4 opciones a stderr —
/// 1) reproducir otra vez (sin re-síntesis), 2) aceptar y guardar con
///    `persist_locution`, que recomprueba la identidad de la voz y la colisión
///    de label al guardar, 3) rechazar y regenerar
///    (una síntesis nueva), 4) rechazar y descartar. EOF o error de lectura de
///    stdin también descartan; una opción inválida avisa y repite el menú.
///
/// La opción 1 reproduce la toma completa ya guardada y se resuelve dentro del
/// menú porque no necesita volver a sintetizar; la 3 se devuelve al llamador como `Resynthesize`, que re-sintetiza
/// y reentra al menú. El menú y los prompts van a stderr, nunca a stdout. No
/// introduce exit codes nuevos.
// Firma ancha deliberada: cada argumento mapea 1:1 a un dato del menú.
#[allow(clippy::too_many_arguments)]
fn synthesize_play_loop(
    voice: &VoiceName,
    label: &Label,
    policy: avi_store::CollisionPolicy,
    voice_store: &VoiceStore,
    speech_store: &SpeechStore,
    reference: Option<&avi_store::VoiceReference>,
    final_text: &str,
    wav: &std::path::Path,
    reader: &mut dyn FnMut(&mut String) -> std::io::Result<usize>,
    playback: &mut dyn FnMut(&std::path::Path) -> Result<(), CliError>,
) -> Result<PlayDecision, CliError> {
    loop {
        eprintln!("¿Qué quieres hacer con esta toma?");
        eprintln!("  1) Reproducir otra vez");
        eprintln!("  2) Aceptar y guardar");
        eprintln!("  3) Rechazar y regenerar");
        eprintln!("  4) Rechazar y descartar");
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
                // La identidad de la voz y la colisión se recomprueban en el
                // instante de guardar, no solo en el fast-fail previo a sintetizar.
                let saved = avi_store::persist_locution(
                    voice_store,
                    speech_store,
                    reference,
                    voice,
                    label,
                    final_text,
                    wav,
                    policy,
                )
                .map_err(persist_cli_error)?;
                return Ok(PlayDecision::Saved(saved));
            }
            "3" => return Ok(PlayDecision::Resynthesize),
            "4" => return Ok(PlayDecision::Discarded),
            _ => eprintln!("Opción inválida."),
        }
    }
}

/// Conflicto de una locución como error de la CLI: un cambio de la voz durante la
/// síntesis es `voice_changed` y una etiqueta ocupada es `label_exists`, los dos
/// exit 6.
fn conflict_cli_error(conflict: &avi_store::LocutionConflict) -> CliError {
    let reason = match conflict {
        avi_store::LocutionConflict::VoiceChanged { .. } => "voice_changed",
        avi_store::LocutionConflict::LabelExists { .. } => "label_exists",
    };
    CliError::new(ExitCode::StateConflict, reason, conflict.to_string())
}

/// Error de `persist_locution` como error de la CLI: sus conflictos se traducen
/// con [`conflict_cli_error`] y cualquier otro fallo al guardar es
/// `synthesis_failed`.
fn persist_cli_error(e: anyhow::Error) -> CliError {
    match e.downcast_ref::<avi_store::LocutionConflict>() {
        Some(conflict) => conflict_cli_error(conflict),
        None => CliError::new(ExitCode::Error, "synthesis_failed", e.to_string()),
    }
}

/// Error de metadatos dañados de un audio sintetizado guardado, como
/// `speech_metadata_corrupt`: nombra la etiqueta y la voz; la solución de la
/// tabla remite a `speech remove`, que borra el audio sin leer los metadatos.
/// El texto de la causa y el fichero van al detalle.
fn sidecar_cli_error(e: &anyhow::Error) -> Option<CliError> {
    e.downcast_ref::<store::SidecarError>().map(|s| {
        CliError::new(
            ExitCode::Error,
            "speech_metadata_corrupt",
            format!(
                "Los metadatos del audio sintetizado {} de la voz {} están dañados.",
                s.label, s.voice
            ),
        )
        .with_detail(format!("{}: {}", s.path.display(), s.reason))
    })
}

/// Traduce un nombre ilegal al contrato con exit 2 y un `reason` por tipo:
/// `invalid_voice_name` para una voz e `invalid_identifier` para una etiqueta.
fn invalid_name_cli_error(e: avi_store::InvalidName) -> CliError {
    let reason = match e.kind {
        avi_store::NameKind::Voice => "invalid_voice_name",
        avi_store::NameKind::Label => "invalid_identifier",
    };
    CliError::new(ExitCode::InvalidInput, reason, e.to_string())
}

/// Exige que la voz exista en el registro: `voice_not_found` (exit 3) si no. Lo
/// comparten `speech list`, `play` y `remove`, de modo que una voz mal escrita
/// nunca se disfrace de «sin resultados» o de etiqueta inexistente.
fn ensure_voice_exists(voice: &VoiceName) -> Result<(), CliError> {
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

/// Preparación común de `speech synthesize`, `say` y `dub`, por las dos rutas.
///
/// El orden es contrato y va de lo barato y local a lo costoso:
///
/// 1. **Invocación** (exit 2): las comprobaciones propias del comando, que recibe
///    `invocation`, y el nombre de la voz.
/// 2. **Almacén**: que la voz exista (exit 3) y, en `synthesize`, la colisión de
///    la etiqueta, que decide `collision` y que puede terminar la orden sin
///    sintetizar (`--skip-existing`).
/// 3. **Recursos**, después de la preparación y ya decidida la ruta: el modelo
///    (exit 4) o el daemon (exit 5).
///
/// Con dos fallos a la vez gana el del paso anterior, en los tres comandos y en
/// las dos rutas: el cliente y el daemon comparten el directorio de datos, así
/// que el almacén se consulta aquí aunque la síntesis la haga el daemon.
fn prepare_synthesis<T, C>(
    invocation: impl FnOnce() -> Result<T, CliError>,
    voice: &str,
    collision: impl FnOnce(&VoiceName, &T) -> Result<C, CliError>,
) -> Result<(VoiceName, T, C), CliError> {
    let checked = invocation()?;
    let voice = VoiceName::parse(voice).map_err(invalid_name_cli_error)?;
    ensure_voice_exists(&voice)?;
    let collided = collision(&voice, &checked)?;
    Ok((voice, checked, collided))
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
                    let v = VoiceName::parse(v).map_err(invalid_name_cli_error)?;
                    ensure_voice_exists(&v)?;
                    speech_store.list_by_voice(&v).map_err(speech_list_error)?
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
                if items.is_empty() {
                    println!("No hay audios sintetizados guardados.");
                } else {
                    println!("Audios sintetizados guardados:");
                    for e in &items {
                        match &e.metadata {
                            Some(m) => println!(
                                "  {}  {}  {}  «{}»",
                                e.voice,
                                e.label,
                                avi_term::fmt::duration(
                                    std::time::Duration::try_from_secs_f64(m.duration_secs)
                                        .unwrap_or_default()
                                ),
                                m.text
                            ),
                            None => println!("  {}  {}  (sin metadatos)", e.voice, e.label),
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
                        requested as f64,
                        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                    ));
                }
            }
            // push-to-talk sin --duration se permite en TTY; sin TTY se exige --duration
            if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                return Err(CliError::new(
                    ExitCode::InvalidInput,
                    "usage_error",
                    "--mic necesita --duration cuando no hay una terminal.",
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
            // vida es la sonda del daemon (`daemon_stop::probe`): sin daemon, el
            // rechazo es inmediato en todas las plataformas y el fallback
            // Auto→directo no espera.
            let client = daemon_client();
            let use_daemon = route_to_daemon(routing.mode()).await?;
            let mic_max = if mic {
                Some(mic_max_secs(duration, avi_core::MAX_TRANSCRIBE_AUDIO_SECS)?)
            } else {
                None
            };
            let context = format!(
                "Transcripción {} · {} · {}",
                source_language,
                audio_source_label(audio.as_deref(), mic_max),
                route_label(use_daemon)
            );
            // La cabecera va tras validar la invocación y antes del primer
            // acceso a recursos (modelo, micrófono, daemon): un error de
            // recurso sale con ella y uno de invocación, sin ella.
            avi_term::line::context(&context);
            if use_daemon {
                let progress = PhaseProgress::new();
                let result = transcribe_via_daemon(
                    &client,
                    audio.as_deref(),
                    mic,
                    duration,
                    &source_language,
                    denoise,
                    &progress,
                )
                .await;
                return finish_transcription(json_mode, progress, result, &source_language);
            }

            // Modelo ausente -> exit 4, previo a construir el motor.
            let stt_dir = open_model_store()?.model_dir("parakeet-tdt-v3");
            if !stt_dir.join("nemo128.onnx").exists() {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "Falta el modelo de transcripción.",
                ));
            }

            // Compilado sin soporte STT (feature off): rama de error explícita.
            // La validación de argumentos y la ausencia de modelo (exit 4) ya se
            // comprobaron arriba; aquí solo se corta la ejecución del motor.
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
                let progress = PhaseProgress::new();
                // Patrón de bloque externo: el trabajo que puede fallar vive en
                // un bloque y el cierre del indicador —que consume el indicador—
                // se hace fuera, sobre el resultado ya obtenido.
                let result = async {
                    let pcm = acquire_pcm_with_progress(
                        &progress,
                        audio.as_deref(),
                        mic,
                        duration,
                        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
                        "El audio de la transcripción",
                    )
                    .await?;

                    progress.phase(&stage::TRANSCRIBING);
                    // La carga del modelo también cuenta en la etapa y deja
                    // avanzar su contador.
                    let engine = progress
                        .while_running(|| STT_HOLDER.get_or_try_init(stt_dir))
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_failed",
                                "No se pudo cargar el modelo de transcripción.",
                            )
                            .with_detail(format!("{e:#}"))
                        })?;
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
                            let models = denoise::service::ServiceModels::resolve()
                                .map_err(|e| e.to_cli_error())?;
                            progress
                                .while_running(|| {
                                    models.denoise_pcm16_mono(&pcm, 16_000, work_dir.path())
                                })
                                .map_err(|e| e.to_cli_error())?
                        } else {
                            pcm.clone()
                        };
                        #[cfg(not(feature = "native-denoise"))]
                        let denoised = pcm;
                        denoised
                    };
                    let text = progress.while_running(|| {
                        avi_core::engine::transcribe_speech(engine, &pcm_for_stt)
                    })?;
                    Ok(text)
                }
                .await;
                finish_transcription(json_mode, progress, result, &source_language)
            }
        }
        SpeechCommands::Synthesize {
            text,
            voice,
            label,
            force,
            skip_existing,
            play,
            source_language,
            target_language,
            temperature,
            routing,
        } => {
            let policy = collision_policy(force, skip_existing);
            // Precondiciones puras de --play, antes de cualquier síntesis.
            // No introducen exit codes nuevos.
            //
            // El indicador nace antes de las validaciones, y el patrón de
            // bloque externo garantiza que se cierra en cualquier camino: los
            // `?` de las validaciones no pueden dejar la fase abierta.
            //
            // Sin `--play` y sin traducción (origen igual al destino) la síntesis
            // es la única etapa de trabajo: no deja su línea y queda solo el
            // cierre, que ya da la duración del audio. Se decide con los
            // argumentos, porque el indicador nace antes de las validaciones.
            let source_is_target =
                source_language.as_deref().unwrap_or(&target_language) == target_language;
            let progress = if play || !source_is_target {
                PhaseProgress::new()
            } else {
                PhaseProgress::new().single_stage()
            };
            let result = async {
                progress.phase(&stage::VALIDATING);
                let (voice, label, existing) = prepare_synthesis(
                    || {
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
                        // La temperatura se comprueba antes que el texto.
                        avi_core::validate_synthesis_text(&text)?;
                        Label::parse(&label).map_err(invalid_name_cli_error)
                    },
                    &voice,
                    |voice, label| {
                        avi_store::check_locution(&open_speech_store()?, voice, label, policy)
                            .map_err(|conflict| conflict_cli_error(&conflict))
                    },
                )?;
                // `--skip-existing` sobre una etiqueta ocupada termina aquí, antes
                // de comprobar el modelo o el daemon: no hace falta sintetizar.
                if let Some(existing) = existing {
                    return Ok(SynthesizeOutcome::Skipped(existing));
                }
                // Origen por defecto = destino (sin traducir).
                let source_eff = source_language.as_deref().unwrap_or(&target_language);

                // Dispatch 3 modos (Synthesize es delegable al daemon).
                let client = daemon_client();
                let use_daemon = route_to_daemon(routing.mode()).await?;
                avi_term::line::context(&format!(
                    "Síntesis {} · voz {} · etiqueta {} · {}",
                    languages_label(source_eff, &target_language),
                    voice,
                    label,
                    route_label(use_daemon)
                ));
                if use_daemon {
                    return synthesize_via_daemon(
                        &progress,
                        &client,
                        &text,
                        &voice,
                        &label,
                        policy,
                        play,
                        source_eff,
                        &target_language,
                        temperature,
                    )
                    .await;
                }

                require_model_provisioned()?;
                let voice_store = open_voice_store()?;
                let speech_store = open_speech_store()?;

                // Guardián del temporal: lo borra al salir de ámbito por cualquier camino.
                let tmp_guard = avi_process::scratch::temp_wav("avi_tts_")
                    .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
                let tmp_wav = tmp_guard.path().clone();
                progress.yield_line();
                let engine_result = direct_tts_engine();
                progress.reclaim();
                let engine = engine_result?;
                // Traducción opt-in antes de sintetizar (passthrough si coinciden).
                let final_text = if source_eff != target_language {
                    progress.phase(&stage::TRANSLATING);
                    translate_if_different(&text, source_eff, &target_language, &progress)?
                } else {
                    text.clone()
                };
                progress.phase(&stage::SYNTHESIZING);
                progress.tick();
                // La identidad de la voz se resuelve antes de sintetizar: la
                // locución solo se guarda si la voz la conserva al guardarla.
                let reference = resolve_voice_reference(&voice_store, &voice)?;
                if play {
                    // La toma suena mientras se sintetiza y se acumula en el
                    // temporal para el menú y el guardado.
                    let delivery = open_delivery(
                        true,
                        true,
                        format!("No se pudo reproducir el audio sintetizado '{}'", label),
                    )?;
                    let take = synthesize_streaming_directly(
                        &DirectSynthesis {
                            engine: &engine,
                            reference: reference.as_ref(),
                            text: &final_text,
                            voice: &voice,
                            temperature,
                            seed: None,
                            language: &target_language,
                        },
                        &progress,
                        delivery,
                    )?;
                    write_take_wav(&tmp_wav, &take.pcm)?;
                } else {
                    progress.while_running(|| {
                        synthesize_wav_directly(
                            &progress,
                            &engine,
                            reference.as_ref(),
                            &final_text,
                            &voice,
                            temperature,
                            None,
                            &target_language,
                            &tmp_wav,
                        )
                    })?;
                }

                let saved = if play {
                    // Bucle interactivo, reemplaza la reproducción y el guardado
                    // incondicionales. Solo alcanzable en TTY. El bucle vive aquí:
                    // el menú solo devuelve la decisión y la re-síntesis la ejecuta
                    // esta rama, sobre el mismo temporal.
                    let mut reader = menu_stdin();
                    let mut playback = menu_playback(&label, &progress);
                    // La etapa de reproducción termina antes del menú: su
                    // duración no incluye lo que tarde el usuario en elegir.
                    progress.end_stage();
                    let mut resynthesizer = LocalResynthesizer {
                        engine: &engine,
                        reference: reference.as_ref(),
                        final_text: &final_text,
                        voice: &voice,
                        label: &label,
                        temperature,
                        target_language: &target_language,
                        tmp_wav: &tmp_wav,
                        progress: &progress,
                    };
                    // El menú devuelve el desenlace: guardado o descartado (opción 4 o
                    // EOF), que sale con 0.
                    let outcome = play_menu_loop(
                        &voice,
                        &label,
                        policy,
                        &voice_store,
                        &speech_store,
                        reference.as_ref(),
                        &final_text,
                        &tmp_wav,
                        &mut reader,
                        &mut playback,
                        &mut resynthesizer,
                    )
                    .await;
                    outcome?
                } else {
                    progress.phase(&stage::SAVING);
                    avi_store::persist_locution(
                        &voice_store,
                        &speech_store,
                        reference.as_ref(),
                        &voice,
                        &label,
                        &final_text,
                        &tmp_wav,
                        policy,
                    )
                    .map_err(persist_cli_error)?
                    .into()
                };
                Ok(saved)
            }
            .await;
            let outcome = match result {
                Ok(outcome) => outcome,
                Err(e) => {
                    progress.fail();
                    return Err(e);
                }
            };
            emit_synthesize(json_mode, &voice, &outcome, progress);
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
            let progress = PhaseProgress::new();
            // Patrón de bloque externo: el trabajo que puede fallar vive en un
            // bloque y el cierre del indicador —que consume el indicador— se
            // hace fuera, sobre el resultado ya obtenido.
            let result: Result<DeliveryStats, CliError> = async {
                progress.phase(&stage::VALIDATING);
                let (voice, (), ()) = prepare_synthesis(
                    || {
                        validate_temperature(temperature)?;
                        avi_core::validate_synthesis_text(&text)
                    },
                    &voice,
                    |_, _| Ok(()),
                )?;
                // Origen por defecto = destino (sin traducir).
                let source_eff = source_language.as_deref().unwrap_or(&target_language);

                // Dispatch 3 modos (Say es delegable al daemon).
                let client = daemon_client();
                let use_daemon = route_to_daemon(routing.mode()).await?;
                avi_term::line::context(&format!(
                    "Lectura en voz alta {} · voz {} · {}",
                    languages_label(source_eff, &target_language),
                    voice,
                    route_label(use_daemon)
                ));
                if use_daemon {
                    return say_via_daemon(
                        &progress,
                        &client,
                        &text,
                        voice.as_str(),
                        source_eff,
                        &target_language,
                        temperature,
                    )
                    .await;
                }

                require_model_provisioned()?;
                let voice_store = open_voice_store()?;
                progress.yield_line();
                let engine_result = direct_tts_engine();
                progress.reclaim();
                let engine = engine_result?;
                // Traducción opt-in antes de sintetizar (passthrough si coinciden).
                let final_text = if source_eff != target_language {
                    progress.phase(&stage::TRANSLATING);
                    translate_if_different(&text, source_eff, &target_language, &progress)?
                } else {
                    text.clone()
                };
                progress.phase(&stage::SYNTHESIZING);
                progress.tick();
                let reference = resolve_voice_reference(&voice_store, &voice)?;
                // El dispositivo se abre antes de pedir la síntesis: sin salida de
                // audio el comando termina sin sintetizar.
                let delivery =
                    open_delivery(true, false, "No se pudo reproducir el audio sintetizado")?;
                synthesize_streaming_directly(
                    &DirectSynthesis {
                        engine: &engine,
                        reference: reference.as_ref(),
                        text: &final_text,
                        voice: &voice,
                        temperature,
                        seed: None,
                        language: &target_language,
                    },
                    &progress,
                    delivery,
                )
            }
            .await;
            finish_progress(progress, &result, "Reproducción completada", |stats| {
                streaming_closing_extras(stats.first_audio_ms, stats.stalls)
            });
            let stats = result?;
            if json_mode {
                emit_raw_json(json!({
                    "status": "reproduced",
                    "voice": voice,
                    "first_audio_ms": stats.first_audio_ms,
                    "stalls": stats.stalls,
                }));
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
            // El cliente y el daemon comparten el directorio de datos, así que
            // una voz inexistente se rechaza antes de abrir el micrófono o leer
            // el audio, en las dos rutas; el daemon conserva su propia
            // comprobación como defensa.
            let (voice, (), ()) = prepare_synthesis(
                || {
                    validate_temperature(temperature)?;
                    if mic && duration.is_none() && !std::io::stdin().is_terminal() {
                        return Err(CliError::new(
                            ExitCode::InvalidInput,
                            "usage_error",
                            "--mic necesita --duration cuando no hay una terminal.",
                        ));
                    }
                    if let Some(d) = duration.filter(|d| *d > avi_core::MAX_DUB_AUDIO_SECS) {
                        return Err(audio_too_long(
                            "El audio del doblaje",
                            d as f64,
                            avi_core::MAX_DUB_AUDIO_SECS,
                        ));
                    }
                    if let Some(a) = &audio {
                        validate_audio_arg(std::path::Path::new(a))?;
                        // El tope se mide por la cabecera antes de contactar con el
                        // daemon o de comprobar modelos, igual que en transcribe y clone.
                        ensure_wav_within_limit(
                            std::path::Path::new(a),
                            "El audio del doblaje",
                            avi_core::MAX_DUB_AUDIO_SECS,
                        )?;
                    }
                    Ok(())
                },
                &voice,
                |_, _| Ok(()),
            )?;
            // Despacho 3 modos: delega a POST /dub si daemon activo
            let mic_max = if mic {
                Some(mic_max_secs(duration, avi_core::MAX_DUB_AUDIO_SECS)?)
            } else {
                None
            };
            let context = |use_daemon: bool| {
                format!(
                    "Doblaje {} · voz {} · {} · {}",
                    languages_label(&source_language, &target_language),
                    voice,
                    audio_source_label(audio.as_deref(), mic_max),
                    route_label(use_daemon)
                )
            };
            {
                let client = daemon_client();
                let use_daemon = route_to_daemon(routing.mode()).await?;
                // La cabecera va tras validar la invocación y antes del primer
                // acceso a recursos (modelos, micrófono, daemon).
                avi_term::line::context(&context(use_daemon));
                if use_daemon {
                    let progress = PhaseProgress::new();
                    let result = dub_via_daemon(
                        &progress,
                        &client,
                        audio.as_deref(),
                        mic,
                        duration,
                        &source_language,
                        &target_language,
                        temperature,
                        denoise,
                        voice.as_str(),
                    )
                    .await;
                    return finish_dub(json_mode, progress, result);
                }
            }
            // Rama local: modelos ausentes → exit 4
            let stt_dir = open_model_store()?.model_dir("parakeet-tdt-v3");
            if !stt_dir.join("nemo128.onnx").exists() {
                return Err(CliError::new(
                    ExitCode::ModelMissing,
                    "model_missing",
                    "Falta el modelo de transcripción.",
                ));
            }
            require_model_provisioned()?;

            // Doblaje = transcribe→traduce→sintetiza: sin soporte STT (feature off)
            // el pipeline no puede arrancar; rama de error explícita tras las
            // comprobaciones (usage, audio existente, modelos ausentes → exit 4).
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
                let progress = PhaseProgress::new();
                // Patrón de bloque externo: el trabajo que puede fallar vive en
                // un bloque y el cierre del indicador —que consume el indicador—
                // se hace fuera, sobre el resultado ya obtenido. Así ninguno de
                // los `?` del pipeline deja la fase abierta.
                let result = async {
                    let pcm = acquire_pcm_with_progress(
                        &progress,
                        audio.as_deref(),
                        mic,
                        duration,
                        avi_core::MAX_DUB_AUDIO_SECS,
                        "El audio del doblaje",
                    )
                    .await?;
                    let pcm_for_stt = {
                        #[cfg(feature = "native-denoise")]
                        let denoised = if denoise {
                            let work_dir = avi_process::scratch::tempdir("avi_dub_denoise_")
                                .map_err(|e| {
                                    CliError::new(ExitCode::Error, "io_error", format!("{e:#}"))
                                })?;
                            let models = denoise::service::ServiceModels::resolve()
                                .map_err(|e| e.to_cli_error())?;
                            progress.phase(&stage::DENOISING);
                            progress
                                .while_running(|| {
                                    models.denoise_pcm16_mono(&pcm, 16_000, work_dir.path())
                                })
                                .map_err(|e| e.to_cli_error())?
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
                    progress.phase(&stage::TRANSCRIBING);
                    progress.tick();
                    // La carga del modelo también cuenta en la etapa y deja
                    // avanzar su contador.
                    let stt = progress
                        .while_running(|| STT_HOLDER.get_or_try_init(stt_dir))
                        .map_err(|e| {
                            CliError::new(
                                ExitCode::TranscriptionFailed,
                                "transcription_failed",
                                "No se pudo cargar el modelo de transcripción.",
                            )
                            .with_detail(format!("{e:#}"))
                        })?;
                    let transcribed = progress.while_running(|| {
                        avi_core::engine::transcribe_speech(stt, &pcm_for_stt)
                    })?;
                    // La transcripción sale por stdout en cuanto existe, antes de
                    // que suene el audio.
                    progress.result(Some("Transcripción"), &transcribed);

                    // Traducción solo si source != target (passthrough si coinciden).
                    let source = source_language.as_str();
                    let target = target_language.as_str();
                    let final_text = if source == target {
                        transcribed.clone()
                    } else {
                        progress.phase(&stage::TRANSLATING);
                        let model_name = format!("opus-mt-{}", translation_pair(source));
                        let model_store = open_model_store()?;
                        let ct2_dir = model_store.model_dir(&model_name);
                        if !model_store.is_provisioned(&model_name) {
                            return Err(translation_model_missing(source));
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
                            progress
                                .while_running(|| translation::translate(&transcribed, &ct2_dir))
                                .map_err(|e| {
                                CliError::new(
                                    ExitCode::TranslationFailed,
                                    "translation_failed",
                                    e.to_string(),
                                )
                            })?
                        }
                    };

                    if source != target {
                        progress.result(Some("Traducción"), &final_text);
                    }

                    // El texto traducido puede exceder el tope aunque el transcrito no.
                    avi_core::validate_synthesis_text(&final_text)?;
                    let voice_store = open_voice_store()?;
                    progress.yield_line();
                    let engine_result = direct_tts_engine();
                    progress.reclaim();
                    let engine = engine_result?;
                    progress.phase(&stage::SYNTHESIZING);
                    progress.tick();
                    let reference = resolve_voice_reference(&voice_store, &voice)?;
                    // El dispositivo se abre antes de pedir la síntesis.
                    let delivery = open_delivery(true, false, "Fallo al reproducir el doblaje")?;
                    let stats = synthesize_streaming_directly(
                        &DirectSynthesis {
                            engine: &engine,
                            reference: reference.as_ref(),
                            text: &final_text,
                            voice: &voice,
                            temperature,
                            seed: None,
                            language: &target_language,
                        },
                        &progress,
                        delivery,
                    )?;
                    Ok(DubOutcome {
                        transcript: transcribed,
                        translated: final_text,
                        first_audio_ms: stats.first_audio_ms,
                        stalls: stats.stalls,
                    })
                }
                .await;
                finish_dub(json_mode, progress, result)
            }
        }
        SpeechCommands::Play { label, voice } => {
            // Reproducción de locución persistida: local-only.
            let voice = VoiceName::parse(&voice).map_err(invalid_name_cli_error)?;
            let label = Label::parse(&label).map_err(invalid_name_cli_error)?;
            ensure_voice_exists(&voice)?;
            let speech_store = open_speech_store()?;
            match speech_store
                .find(&voice, &label)
                .map_err(speech_find_error)?
            {
                Some(entry) => {
                    avi_term::line::context(&format!(
                        "Reproducción del audio sintetizado {label} · voz {voice}"
                    ));
                    // Única etapa del comando: no deja su línea y queda solo el cierre.
                    let progress = PhaseProgress::new().single_stage();
                    // Patrón de bloque externo: el cierre del indicador se hace
                    // fuera del trabajo que puede fallar.
                    let result = 'work: {
                        progress.phase(&stage::PLAYING);
                        break 'work audio::AudioService::new()
                            .play_wav(&entry.audio_path, &mut || progress.tick())
                            .map_err(|e| {
                                audio_error(e, ExitCode::Error, "playback_failed", |e| {
                                    format!(
                                        "No se pudo reproducir el audio sintetizado '{}' de la voz '{}': {}",
                                        label, voice, e
                                    )
                                })
                            });
                    };
                    finish_progress(progress, &result, "Reproducción completada", |()| {
                        Vec::new()
                    });
                    result?;
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "played", "label": label, "voice": voice }),
                        );
                    }
                    Ok(())
                }
                None => Err(CliError::new(
                    ExitCode::NotFound,
                    "speech_not_found",
                    format!(
                        "El audio sintetizado '{}' de la voz '{}' no existe.",
                        label, voice
                    ),
                )),
            }
        }
        SpeechCommands::Remove { label, voice, yes } => {
            // Borrado de audio sintetizado: local-only.
            let voice = VoiceName::parse(&voice).map_err(invalid_name_cli_error)?;
            let label = Label::parse(&label).map_err(invalid_name_cli_error)?;
            ensure_voice_exists(&voice)?;
            let speech_store = open_speech_store()?;
            // Se pregunta antes de borrar. El plan nombra el WAV sin leer los
            // metadatos, de modo que un audio con metadatos dañados también se
            // puede borrar. Si el WAV no existe, `remove` informa del fallo.
            let wav = speech_store.audio_path(&voice, &label);
            if wav.is_file() {
                let summary = vec![format!(
                    "Se borrará el audio sintetizado {label} de la voz {voice}:"
                )];
                let entries = [lifecycle::confirm::PlanEntry::of(&wav)];
                let decision = {
                    let stdin = std::io::stdin();
                    let mut input = stdin.lock();
                    // La espera de la respuesta no cuenta como trabajo del comando.
                    avi_term::report::begin_question();
                    let decision = lifecycle::confirm::confirm(
                        &lifecycle::confirm::Confirmation {
                            kind: lifecycle::confirm::Kind::Destructive,
                            question: "¿Borrar este audio sintetizado?",
                            summary: &summary,
                            entries: &entries,
                            assume_yes: yes,
                            dry_run: false,
                            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
                        },
                        &mut input,
                        &mut std::io::stderr(),
                    );
                    avi_term::report::end_question(matches!(
                        decision,
                        Ok(lifecycle::confirm::Decision::Proceed)
                    ));
                    decision.map_err(lifecycle_error_to_cli)?
                };
                if decision == lifecycle::confirm::Decision::Cancelled {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "cancelled", "label": label, "voice": voice }),
                        );
                    }
                    // `confirm::confirm` ya escribió «Cancelado; no se ha borrado
                    // nada.» por stderr.
                    return Ok(());
                }
            }
            speech_store
                .remove(&voice, &label)
                .map_err(|e| CliError::new(ExitCode::NotFound, "speech_not_found", e))?;
            if json_mode {
                emit_raw_json(json!({ "status": "removed", "label": label, "voice": voice }));
            } else {
                avi_term::line::ok(&format!(
                    "Audio sintetizado {label} de la voz {voice} borrado."
                ));
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
            let warm_voice = VoiceName::parse(&warm_voice).map_err(invalid_name_cli_error)?;
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
            VoiceName::parse(&warm_voice).map_err(invalid_name_cli_error)?;
            require_model_provisioned()?;
            avi_shared::diag_trace::mark("start.model_provisioned");
            // Revalidación con reclamo matar-y-rearrancar: la vida del
            // residual se comprueba por PID vivo más probe, no por probe solo ni
            // pidfile solo. En servicio → `already_running`; degradado → la parada
            // unificada lo reclama y se rearranca con salida 0 y payload `started`.
            let data_dir = effective_data_dir()?;
            let client_addr = lifecycle::daemon_stop::resolve_client_addr(&data_dir);
            let responds = lifecycle::daemon_stop::probe(&client_addr).await;
            avi_shared::diag_trace::mark("start.health_probe_done");
            avi_term::line::context(&format!("Daemon · {client_addr}"));
            // El indicador nace **antes** de la parada del daemon residual: así
            // no hay segundos de silencio antes de la primera fase, y el mismo
            // indicador sirve para `start`, `stop` y `restart`.
            let progress = PhaseProgress::new();
            avi_shared::diag_trace::mark("start.observe_begin");
            match lifecycle::daemon_stop::observe(&data_dir, &ProductProcesses, responds) {
                lifecycle::daemon_stop::DaemonState::Running { pid } => {
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "already_running", "daemon": "running", "pid": pid }),
                        );
                    } else {
                        avi_term::line::ok(&format!("El daemon ya estaba en marcha · pid {pid}"));
                    }
                    return Ok(Outcome::Done);
                }
                lifecycle::daemon_stop::DaemonState::Degraded(_) => {
                    // El aviso es del producto y no puede quedar pegado a la
                    // fase: `warn` cede la línea antes de escribir.
                    avi_term::line::warn(
                        "El daemon que estaba en marcha no respondía bien; se detiene y se arranca de nuevo.",
                    );
                    progress.phase(&stage::STOPPING);
                    let outcome = lifecycle::daemon_stop::stop(
                        &data_dir,
                        &lifecycle::daemon_stop::default_addr(),
                        &ProductProcesses,
                    )
                    .await;
                    lifecycle::daemon_stop::require_stopped(&outcome)
                        .map_err(|failure| lifecycle_error_to_cli(failure.into()))?;
                    progress.done_as("Daemon detenido");
                    progress.reclaim();
                }
                lifecycle::daemon_stop::DaemonState::Stopped => {}
            }
            avi_shared::diag_trace::mark("start.observe_done");
            let launched = launch_daemon(
                auto_restart,
                max_retries,
                &warm_voice,
                DAEMON_READY_DEADLINE,
                Some(&progress),
            )
            .await;
            match launched {
                Ok((pid, warm)) => {
                    let (action, extras) =
                        daemon_launch_closing("Daemon listo", "Daemon en marcha", pid, &warm);
                    progress.finish(&action, &extras);
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "started", "daemon": "running", "pid": pid }),
                        );
                    }
                    Ok(Outcome::Done)
                }
                Err(e) => {
                    progress.fail();
                    Err(e)
                }
            }
        }
        DaemonCommands::Stop => {
            // Parada unificada con deadline global: el veredicto lo da el motor
            // (`StopOutcome`), sin re-sondeos fuera de él. El pidfile ya lo borró
            // el motor tras muerte verificada; la CLI solo informa. La reserva sin
            // pidfile es la dirección derivada de `AVI_DAEMON_PORT`, la misma en la
            // que escucha el daemon.
            // La dirección se descubre antes de la parada, porque el pidfile
            // efímero desaparece al detener el daemon; los mensajes diagnostican
            // esa dirección (sin pidfile es la de por defecto).
            let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
            avi_term::line::context(&format!("Daemon · {client_addr}"));
            let progress = PhaseProgress::new();
            progress.phase(&stage::STOPPING);
            let outcome = lifecycle::daemon_stop::stop(
                &effective_data_dir()?,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            if outcome.stopped {
                let status = stop_status(outcome.was_running);
                if outcome.was_running {
                    progress.finish("Daemon detenido", &[]);
                } else {
                    // El mensaje depende de `was_running`: sin daemon en marcha
                    // la operación no «detuvo» nada, y decirlo sería mentir.
                    progress.close();
                    avi_term::line::ok("No había ningún daemon en marcha.");
                }
                if json_mode {
                    emit_raw_json(json!({
                        "status": status,
                        "daemon": "stopped",
                        "was_running": outcome.was_running,
                        "daemon_fully_stopped": outcome.daemon_fully_stopped
                    }));
                }
                Ok(Outcome::Done)
            } else {
                progress.close();
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
                    // El fallo no tiene `reason` propio y ninguna solución de la
                    // tabla encaja: el error sale sin `Solución:`.
                    avi_term::line::error(&message, None);
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
            // El indicador nace **antes** de la parada, igual que en `start` y
            // en `stop`: así no hay segundos de silencio antes de la primera
            // fase, y la fase `stopping` se reutiliza.
            avi_term::line::context(&format!(
                "Daemon · {}",
                lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?)
            ));
            let progress = PhaseProgress::new();
            progress.phase(&stage::STOPPING);
            let outcome = lifecycle::daemon_stop::stop(
                &effective_data_dir()?,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            // Si la parada no concluye queda un daemon vivo: no se arranca otro.
            if let Err(failure) = lifecycle::daemon_stop::require_stopped(&outcome) {
                progress.fail();
                return Err(lifecycle_error_to_cli(failure.into()));
            }
            // La etapa solo deja línea terminada si había un daemon que detener.
            if outcome.was_running {
                progress.done_as("Daemon detenido");
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
            let launched =
                launch_daemon(false, 3, "default", ready_deadline, Some(&progress)).await;
            match launched {
                Ok((pid, warm)) => {
                    let (action, extras) =
                        daemon_launch_closing("Daemon reiniciado", "Daemon reiniciado", pid, &warm);
                    progress.finish(&action, &extras);
                    if json_mode {
                        emit_raw_json(
                            json!({ "status": "restarted", "daemon": "running", "pid": pid }),
                        );
                    }
                    Ok(Outcome::Done)
                }
                Err(e) => {
                    progress.fail();
                    Err(e)
                }
            }
        }
        DaemonCommands::Status => {
            // Primero la sonda única del daemon contra la dirección descubierta
            // (la misma de `start`), cuyo rechazo es inmediato sin daemon; solo
            // si responde se pide el cuerpo de GET /health, que aporta motor y
            // warmup. El éxito de ese GET es el sondeo con el que `observe`
            // clasifica: un daemon que muere entre la sonda y el GET cuenta como
            // «no responde». Siempre exit 0.
            let data_dir = effective_data_dir()?;
            let client_addr = lifecycle::daemon_stop::resolve_client_addr(&data_dir);
            let health = if !lifecycle::daemon_stop::probe(&client_addr).await {
                None
            } else {
                match tokio::time::timeout(
                    std::time::Duration::from_millis(500),
                    daemon_client()
                        .get(format!("http://{}/health", client_addr))
                        .send(),
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
                                format!("El daemon en {} no respondió a tiempo.", client_addr),
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
                }
            };
            let state =
                lifecycle::daemon_stop::observe(&data_dir, &ProductProcesses, health.is_some());
            if json_mode {
                emit_raw_json(status_body(state, health.as_ref()));
            } else {
                println!("{}", status_line(state, health.as_ref(), &client_addr));
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
///
/// Si el usuario rechaza la confirmación, el motor no toca el disco: en modo
/// texto no se imprime nada más (el «Cancelado.» ya lo escribió la
/// confirmación, y el estado se dice una sola vez) y en `--json` el envelope
/// lleva `status:"cancelled"` sin modelos. En ambos casos sale con 0.
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

    if outcome.cancelled {
        if json_mode {
            emit_raw_json(json!({
                "status": "cancelled",
                "models_provisioned": []
            }));
        }
    } else if json_mode {
        emit_raw_json(json!({
            "status": "completed",
            "models_provisioned": outcome.provisioned
        }));
    } else if let Some(stats) = outcome.stats {
        // El cierre sale por stderr; los resultados de `setup` son sus envelopes.
        avi_term::line::ok(&format!(
            "Setup completado en {} · {}, {} descargados",
            avi_term::fmt::duration(stats.elapsed),
            avi_term::fmt::count(outcome.downloaded as u64, "modelo", "modelos"),
            avi_term::fmt::bytes(stats.bytes)
        ));
    } else {
        avi_term::line::ok(&format!(
            "Los {}.",
            avi_term::fmt::count(
                outcome.provisioned.len() as u64,
                "modelo ya estaba descargado",
                "modelos ya estaban descargados"
            )
        ));
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
    // Sin recibo (descarga manual, binario de desarrollo o Cask de Homebrew) `--all` deja el
    // estado en cero, porque ningún `self uninstall` lo retirará.
    let registered_program_dir =
        lifecycle::channel::registered_program_dir().map_err(lifecycle_error_to_cli)?;
    let receipt =
        lifecycle::receipt::read_from(&registered_program_dir).map_err(lifecycle_error_to_cli)?;
    let options = lifecycle::cleanup::Options {
        model,
        voices,
        synthetic_speech,
        all,
        dry_run,
        assume_yes: yes,
        without_installation: receipt.is_none(),
    };
    // El indicador **no** lo crea la CLI: la confirmación ocurre dentro de
    // `cleanup::run`, y un indicador abierto antes de ella dejaría el prompt
    // pegado a la fase. La librería lo abre tras la confirmación —la frontera— y
    // lo cierra al terminar el barrido. La simulación no abre ninguna etapa.
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
    }
    // En texto no queda nada por escribir: la librería, dueña del indicador,
    // escribe el plan de la simulación por stdout y el cierre por stderr, y
    // `confirm::confirm` ya escribió «Cancelado.» si el usuario canceló.
    Ok(())
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

/// Identidad del proceso que retiene el aparcado del reemplazo, por cuyo término
/// espera su retirada. Lo fija `self update` al lanzar el traspaso (variable con
/// su PID, cuyo inicio se observa aquí); sin traspaso (variable ausente) es el
/// propio proceso, que termina enseguida. Una variable presente con un valor que no es
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
            let parked_holder = handover_wait_token()?;
            let exe = std::env::current_exe().map_err(|e| {
                CliError::new(ExitCode::Error, "self_install_failed", e.to_string())
            })?;
            let env = lifecycle::install::Env::from_current_exe(exe, VERSION, parked_holder)
                .map_err(lifecycle_error_to_cli)?;
            let options = lifecycle::install::Options {
                assume_yes: yes,
                no_setup,
                no_modify_path,
                force,
            };
            let outcome =
                lifecycle::install::install(&env, &options, &ProductProcesses, &PlatformRemoval)
                    .await
                    .map_err(lifecycle_error_to_cli)?;
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
                // el `reason` real de la causa (`network_error` o `model_unavailable`)
                // sin perderlo. Solo existe si
                // hubo fallo: un envelope estable es más fácil de leer que uno con nulos.
                if let lifecycle::install::ModelsState::Failed { cause } = &outcome.models {
                    on["models_cause"] = json!({
                        "reason": cause.reason,
                        "message": cause.message
                    });
                }
                emit_raw_json(on);
            } else if let Some((closing, details)) = outcome.summary.split_first() {
                // El cierre y los detalles salen por stderr; los avisos de después,
                // con `warn`. Si el usuario canceló, el resumen está vacío.
                avi_term::line::ok(closing);
                for line in details {
                    avi_term::line::detail(line);
                }
                for warning in &outcome.warnings {
                    avi_term::line::warn(warning);
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
                &PlatformRemoval,
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
            } else if outcome.status == "not_installed" {
                // El literal de `status` es del contrato `--json` y no sale en el
                // texto. La simulación y la ejecución real escriben su plan y su
                // cierre en la librería, que es dueña del indicador.
                avi_term::line::ok("No había nada que desinstalar.");
            }
            match kept {
                Some(failure) => {
                    // El envelope ya salió por stdout: en `--json` no se escribe
                    // nada más; en texto, el error con su solución.
                    if !json_mode {
                        print_error_text(&CliError::new(
                            ExitCode::from_reason(failure.reason),
                            failure.reason,
                            failure.message.clone(),
                        ));
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
            // El indicador nace antes de la resolución y es el único de todo el
            // comando: un solo reloj hasta el cierre. El patrón de bloque
            // externo garantiza que se cierra en cualquier camino: los `?` de
            // la resolución no pueden dejar la fase `checking` abierta.
            if !check {
                avi_term::line::context(&format!(
                    "Actualización desde la versión {}",
                    receipt.version
                ));
            }
            let progress = PhaseProgress::new();
            let result = async {
                progress.phase(&stage::CHECKING);
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
            let (client, resolution) = match result {
                Ok(resolved) => resolved,
                Err(e) => {
                    progress.fail();
                    return Err(e);
                }
            };

            // ── `--check`: informa sin cambios de actualización ──────────────
            // La recuperación del paso 1 ya pudo barrer huérfanos pendientes;
            // "sin cambios" no incluye ese barrido.
            if check {
                let available = resolution.verdict != lifecycle::update_resolve::Verdict::UpToDate;
                drop(lock);
                // El resultado sale por stdout; en `--json` el envelope lo
                // sustituye y `result` no escribe nada.
                if available {
                    progress.result(
                        None,
                        &format!(
                            "Hay una versión nueva: {} → {}.",
                            receipt.version, resolution.target
                        ),
                    );
                } else {
                    progress.result(
                        None,
                        &format!("Ya tienes la última versión ({}).", receipt.version),
                    );
                }
                progress.close();
                if json_mode {
                    emit_raw_json(json!({
                        "status": "check",
                        "reason": Value::Null,
                        "current": receipt.version,
                        "latest": resolution.target,
                        "update_available": available,
                        "channel": channel.as_str(),
                    }));
                }
                return Ok(Outcome::Done);
            }

            // Iguales sin `--force`: `already_up_to_date`, éxito con 0.
            if resolution.verdict == lifecycle::update_resolve::Verdict::UpToDate {
                drop(lock);
                progress.result(
                    None,
                    &format!("Ya tienes la última versión ({}).", receipt.version),
                );
                progress.close();
                if json_mode {
                    emit_raw_json(compose_update_status_envelope(
                        "already_up_to_date",
                        &receipt.version,
                        &resolution.target,
                        channel,
                    ));
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
            let question = if resolution.destructive {
                format!(
                    "¿Instalar la versión {}, anterior a la instalada?",
                    resolution.target
                )
            } else {
                format!("¿Actualizar a la versión {}?", resolution.target)
            };
            // La comprobación ya terminó: se cierra su etapa antes de preguntar,
            // y la pregunta no queda pegada a ninguna línea de fase.
            progress.end_stage();
            progress.yield_line();
            let decision = {
                let stdin = std::io::stdin();
                let mut entry = stdin.lock();
                lifecycle::confirm::confirm(
                    &lifecycle::confirm::Confirmation {
                        kind,
                        question: &question,
                        summary: &summary,
                        entries: &entries,
                        assume_yes: yes,
                        dry_run: false,
                        stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
                    },
                    &mut entry,
                    &mut std::io::stderr(),
                )
                .map_err(lifecycle_error_to_cli)?
            };
            progress.reclaim();
            if decision == lifecycle::confirm::Decision::Cancelled {
                progress.close();
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
                progress.phase_with(
                    &stage::DOWNLOADING,
                    &format!("Descargando la versión {}…", resolution.target),
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
                progress.done_as(&format!("Versión {} descargada", resolution.target));

                progress.phase(&stage::VERIFYING);
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
            let staged = match result {
                Ok(staged) => staged,
                Err(e) => {
                    progress.fail();
                    return Err(e);
                }
            };

            // ── Paso 8. Parar el daemon con el binario actual ────────────────
            // Si no se detiene, `daemon_stop_failed` sin tocar nada: la
            // instalación sigue intacta y el staging se retira.
            progress.phase(&stage::STOPPING);
            let daemon = lifecycle::daemon_stop::stop(
                &roots.data_dir,
                &lifecycle::daemon_stop::default_addr(),
                &ProductProcesses,
            )
            .await;
            if let Err(failure) = lifecycle::daemon_stop::require_stopped(&daemon) {
                progress.fail();
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
            // La etapa de parada solo deja línea terminada si había un daemon
            // que detener.
            if daemon.was_running {
                progress.done_as("Daemon detenido");
            }
            progress.phase(&stage::REPLACING);
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
            lifecycle::update::cleanup_staging(&staged.staging, &PlatformRemoval, &progress);
            progress.finish(
                &format!("Actualizado de {} a {}", receipt.version, resolution.target),
                &[],
            );

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
                if let lifecycle::update::HandoverOutcome::PartialSetupFailed { cause } = &handover
                {
                    avi_term::line::warn(lifecycle::update::partial_setup_message(cause));
                }
                if !recovery.kept.is_empty() {
                    avi_term::line::warn(format!(
                        "No se pudo limpiar {} de una operación anterior; \
                         `ai-voice-interconnector doctor` informa de ello",
                        avi_term::fmt::count(recovery.kept.len() as u64, "recurso", "recursos")
                    ));
                }
                if daemon.was_running {
                    avi_term::line::warn(
                        "El daemon estaba en marcha y se detuvo; vuelve a iniciarlo con \
                         `ai-voice-interconnector daemon start`",
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
        // Única etapa del comando: no deja su línea y queda solo el cierre.
        let progress = PhaseProgress::new().single_stage();
        // Patrón de bloque externo: el cierre del indicador —que consume el
        // indicador— se hace fuera del trabajo que puede fallar.
        let repair_result = 'work: {
            progress.phase(&stage::REPAIRING);
            progress.tick();
            break 'work lifecycle::recovery::recover(env.roots.recovery_roots())
                .map_err(lifecycle_error_to_cli);
        };
        match repair_result {
            Ok(_) => {
                // El cierre va por stderr; las comprobaciones, por stdout.
                progress.finish("Reparación completada", &[]);
            }
            Err(e) => {
                progress.fail();
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
    } else {
        // Todas las comprobaciones salen por stdout; el veredicto, por stderr.
        println!("{}", doctor_checks_text(&report));
        if failed {
            // El reporte ya está en stdout: el error se escribe sin segundo
            // documento y se sale por veredicto.
            print_error_text(&CliError::new(
                ExitCode::Error,
                "doctor_failed",
                format!(
                    "{}.",
                    avi_term::fmt::count(
                        report.failed.len() as u64,
                        "comprobación falla",
                        "comprobaciones fallan"
                    )
                ),
            ));
        } else if !repair {
            avi_term::line::ok("Diagnóstico sin problemas.");
        }
    }

    if failed {
        Ok(Outcome::Verdict(ExitCode::Error.code()))
    } else {
        Ok(Outcome::Done)
    }
}

/// Etiqueta en español de una comprobación de `doctor`. El identificador
/// interno solo sale en `--json`.
fn doctor_check_label(name: &str) -> &str {
    match name {
        "install_receipt" => "Recibo de instalación",
        "path_resolves" => "Comando en el PATH",
        "path_duplicates" => "PATH sin duplicados",
        "pending_artifacts" => "Restos pendientes",
        "models_provisioned" => "Modelos",
        "onnx_runtime" => "ONNX Runtime",
        other => other,
    }
}

/// Indicación de una comprobación fallida, con la orden concreta que la
/// resuelve, si la hay.
fn doctor_check_hint(name: &str) -> Option<&'static str> {
    match name {
        "models_provisioned" => Some("ejecuta `ai-voice-interconnector setup`"),
        "pending_artifacts" => Some("ejecuta `ai-voice-interconnector doctor --repair`"),
        _ => None,
    }
}

/// Texto de las comprobaciones de `doctor`: la cabecera y una línea por
/// comprobación, con la indicación al final de las que fallan.
fn doctor_checks_text(report: &lifecycle::doctor::Report) -> String {
    let mut lines = vec!["Diagnóstico:".to_string()];
    for check in &report.checks {
        let label = doctor_check_label(&check.name);
        if check.ok {
            lines.push(format!("  ✓ {label}: {}", check.detail));
        } else {
            let hint = doctor_check_hint(&check.name)
                .map(|hint| format!(" → {hint}"))
                .unwrap_or_default();
            lines.push(format!("  ✗ {label}: {}{hint}", check.detail));
        }
    }
    for path in &report.pending.unreadable {
        lines.push(format!("  No se puede listar {path}"));
    }
    lines.join("\n")
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

/// Mecanismo de borrado de plataforma del ciclo de vida, el único que hay.
///
/// En Unix es `remove_dir_all`. En Windows, si el ejecutable en uso retiene la ruta,
/// el borrado directo es imposible y se programa con el limpiador propio
/// (desacoplado si el anfitrión lo permite). El motor decide cuándo se borra ya y
/// cuándo se programa, tanto para la basura del ciclo (staging y aparcado) como para
/// el directorio de programa en uso, que cambia el desenlace a `removal_scheduled`.
struct PlatformRemoval;

impl lifecycle::removal::PathRemover for PlatformRemoval {
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
    ) -> anyhow::Result<avi_process::Scheduling> {
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

impl lifecycle::uninstall::ProgramDirRemover for PlatformRemoval {
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
}
// ─── Helpers ─────────────────────────────────────────────────────────

fn require_model_provisioned() -> Result<(), CliError> {
    let model_store = open_model_store()?;
    if !model_store.is_provisioned(avi_shared::paths::CLONING_MODEL) {
        return Err(CliError::new(
            ExitCode::ModelMissing,
            "model_missing",
            "Falta el modelo de síntesis.",
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
/// - listo: el hijo publicó su dirección real y `/health` responde → su PID y el
///   estado del motor (`EngineWarm`);
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
///
/// Abre la etapa de inicio al entrar y la de calentamiento tras el `/health`
/// correcto, de modo que cada una dura lo que dura su trabajo. Si la espera del
/// calentamiento vence o falla, descarta esa etapa y devuelve el estado; solo en
/// modo máquina, donde no hay cierre, lo avisa por stderr.
async fn launch_daemon(
    auto_restart: bool,
    max_retries: u32,
    warm_voice: &str,
    deadline: std::time::Duration,
    progress: Option<&PhaseProgress>,
) -> Result<(u32, EngineWarm), CliError> {
    let start = std::time::Instant::now();
    if let Some(progress) = progress {
        progress.phase(&stage::STARTING);
    }
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
    // Recién ahora empieza el calentamiento: el proceso ya publicó y responde.
    if let Some(progress) = progress {
        progress.phase(&stage::WARMING);
    }
    // El arranque no termina hasta que `/health` informa el motor como
    // `warm`, la misma condición con la que `daemon status` dice «motor
    // listo»: que el daemon responda solo significa que sirve.
    let warm = match await_engine_warm(&addr, DAEMON_READY_DEADLINE, DAEMON_POLL_INTERVAL, progress)
        .await
    {
        Ok(true) => EngineWarm::Ready,
        Ok(false) => {
            // El calentamiento fallido no derriba el daemon (la primera
            // petición paga el arranque en frío), pero la etapa no puede
            // decir «Motor calentado».
            if let Some(progress) = progress {
                progress.discard_stage();
            }
            if avi_term::line::machine_mode() {
                avi_term::line::warn(
                        "El daemon está en marcha, pero falló el calentamiento del motor; consulta `ai-voice-interconnector daemon status`.",
                    );
            }
            EngineWarm::Failed
        }
        Err(_) => {
            // El daemon ya sirve y acota su propio calentamiento, que acaba en
            // `warm` o `warm_failed`; en un equipo lento puede pasar del plazo
            // de esta espera. No se derriba un daemon sano por eso: la etapa
            // no dice «Motor calentado» y el cierre remite a `daemon status`.
            avi_shared::diag_trace::mark("launch_daemon.warm_deadline_expired");
            if let Some(progress) = progress {
                progress.discard_stage();
            }
            if avi_term::line::machine_mode() {
                avi_term::line::warn(
                        "El daemon está en marcha, pero el motor sigue calentándose; `ai-voice-interconnector daemon status` dirá cuándo está listo.",
                    );
            }
            EngineWarm::StillWarming
        }
    };
    Ok((pid, warm))
}

/// Estado del motor cuando el daemon ya sirve.
enum EngineWarm {
    /// `/health` informó el motor como `warm`.
    Ready,
    /// La espera del calentamiento venció con el motor aún calentándose.
    StillWarming,
    /// El calentamiento terminó en `warm_failed`.
    Failed,
}

/// Línea de cierre de `daemon start` y `daemon restart`: la acción y los extras
/// según el estado del motor. Con el motor listo la acción es `action_ready`;
/// si la espera venció o el calentamiento falló, `action_running` y el propio
/// cierre lo dice, sin un aviso aparte.
fn daemon_launch_closing(
    action_ready: &str,
    action_running: &str,
    pid: u32,
    warm: &EngineWarm,
) -> (String, Vec<String>) {
    let pid = format!("pid {pid}");
    match warm {
        EngineWarm::Ready => (action_ready.to_string(), vec![pid]),
        EngineWarm::StillWarming => (
            action_running.to_string(),
            vec![
                pid,
                "el motor sigue calentándose; `ai-voice-interconnector daemon status` dirá cuándo está listo"
                    .to_string(),
            ],
        ),
        EngineWarm::Failed => (
            action_running.to_string(),
            vec![
                pid,
                "falló el calentamiento del motor; consulta `ai-voice-interconnector daemon status`"
                    .to_string(),
            ],
        ),
    }
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
            avi_term::line::warn(format!(
                "No se pudo retirar el fichero ready '{}': {}",
                ready_path.display(),
                e
            ));
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
    addr: &str,
    deadline: std::time::Duration,
    interval: std::time::Duration,
    progress: Option<&PhaseProgress>,
) -> anyhow::Result<()> {
    let start = std::time::Instant::now();
    loop {
        if lifecycle::daemon_stop::probe(addr).await {
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

/// Etiqueta `warm` que informa `GET /health`, o `None` si el daemon no
/// responde o no la trae. Es la misma que `daemon status` traduce a «motor
/// listo» o «motor calentándose».
async fn fetch_warm_label(addr: &str) -> Option<String> {
    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(500),
        daemon_client()
            .get(format!("http://{}/health", addr))
            .send(),
    )
    .await
    .ok()?
    .ok()?;
    let body: Value =
        tokio::time::timeout(std::time::Duration::from_millis(800), resp.json::<Value>())
            .await
            .ok()?
            .ok()?;
    body.get("warm")
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Espera, repintando la etapa, a que `/health` informe el motor como `warm`
/// (el «motor listo» de `daemon status`). Devuelve `true` con `warm` y `false`
/// con `warm_failed`, que ya no cambiará; agotar `deadline` por reloj de pared
/// es un error. Hace siempre al menos una consulta.
async fn await_engine_warm(
    addr: &str,
    deadline: std::time::Duration,
    interval: std::time::Duration,
    progress: Option<&PhaseProgress>,
) -> anyhow::Result<bool> {
    let start = std::time::Instant::now();
    loop {
        match fetch_warm_label(addr).await.as_deref() {
            Some("warm") => return Ok(true),
            Some("warm_failed") => return Ok(false),
            _ => {}
        }
        if start.elapsed() >= deadline {
            break;
        }
        tokio::time::sleep(interval).await;
        if let Some(progress) = progress {
            progress.tick();
        }
    }
    anyhow::bail!("El motor no terminó de calentar tras {:?}", start.elapsed())
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

/// Descripción en español del estado del motor que informa `/health`; un valor
/// que no es de contrato se muestra como desconocido. El identificador del
/// motor solo sale en `--json`.
fn warm_description(label: &str) -> &'static str {
    match label {
        "warming" => "motor calentándose",
        "warm" => "motor listo",
        "warm_failed" => "falló el calentamiento del motor",
        _ => "estado del motor desconocido",
    }
}

/// Línea de texto de `daemon status`, con los mismos casos que [`status_body`].
/// `addr` es la dirección `host:puerto` del daemon, de la que sale el puerto.
fn status_line(
    state: lifecycle::daemon_stop::DaemonState,
    health: Option<&Value>,
    addr: &str,
) -> String {
    use lifecycle::daemon_stop::DaemonState;
    let port = addr.rsplit(':').next().unwrap_or(addr);
    let warm = health
        .and_then(|h| h.get("warm"))
        .and_then(Value::as_str)
        .unwrap_or("");
    match state {
        DaemonState::Running { pid } => format!(
            "Daemon: en marcha (pid {pid}, puerto {port}, {}).",
            warm_description(warm)
        ),
        DaemonState::Degraded(degradation) => {
            format!(
                "Daemon: en marcha, pero no responde: {}.",
                degradation.describe()
            )
        }
        DaemonState::Stopped => "Daemon: detenido.".to_string(),
    }
}

// ─── Cliente HTTP async del daemon ────────────────────────────────────────

/// Cliente `reqwest` hacia el daemon (HTTP, sin TLS: basta para
/// localhost). La dirección destino la resuelve cada llamada el protocolo
/// (`daemon_stop::resolve_client_addr`). Solo sirve a las peticiones de trabajo
/// y al cuerpo de `daemon status`; la sonda de vida es `daemon_stop::probe`. El
/// timeout de conexión breve acota el `daemon_unreachable` de una petición
/// forzada al daemon (`--daemon`) cuando no está en ejecución.
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
/// un intervalo largo haría que el tiempo parezca congelado entre repintados
/// si no hay hilo de repintado, y uno corto gastaría la CPU en pintar sin que
/// nadie lo note. Durante el trabajo bloqueante un hilo auxiliar repinta con
/// la misma cadencia mediante `PhaseProgress::while_running`.
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

/// Probe de vida sobre la dirección descubierta (pidfile o, sin él, la dirección
/// por defecto del protocolo); `false` habilita el fallback Auto→local.
async fn daemon_active() -> Result<bool, CliError> {
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    Ok(lifecycle::daemon_stop::probe(&client_addr).await)
}

/// Decide si una acción delegable se despacha al daemon:
/// ForceDaemon → siempre (el POST fallará con DaemonUnreachable si no corre);
/// Auto → solo si el daemon responde; ForceDirect → nunca.
async fn route_to_daemon(mode: DaemonMode) -> Result<bool, CliError> {
    match mode {
        DaemonMode::ForceDaemon => Ok(true),
        DaemonMode::ForceDirect => Ok(false),
        DaemonMode::Auto => daemon_active().await,
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
    progress: &PhaseProgress,
) -> Result<String, CliError> {
    // El POST apunta a la dirección descubierta (fallback idéntico sin
    // pidfile).
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let pcm = acquire_pcm_with_progress(
        progress,
        audio,
        mic,
        duration,
        avi_core::MAX_TRANSCRIBE_AUDIO_SECS,
        "El audio de la transcripción",
    )
    .await?;
    progress.phase(&stage::TRANSCRIBING);
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    let mut payload =
        serde_json::json!({ "audio_b64": audio_b64, "source_language": source_language });
    if denoise {
        payload["denoise"] = serde_json::json!(true);
    }
    // La espera se repinta con el indicador: `/transcribe` devuelve JSON plano
    // y no emite eventos, así que sin el repintado la fase se quedaría
    // con el mismo tiempo. `await_with_progress` repinta cada
    // `PROGRESS_REPAINT_INTERVAL` mientras la petición está pendiente.
    let resp = await_with_progress(
        progress,
        client
            .post(format!("http://{}/transcribe", client_addr))
            .json(&payload)
            .send(),
    )
    .await
    .map_err(|e| daemon_connect_error(&client_addr, &e))?;
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
    // y no emite eventos, así que sin el repintado la fase se quedaría
    // con el mismo tiempo. `await_with_progress` repinta cada
    // `PROGRESS_REPAINT_INTERVAL` mientras la petición está pendiente. No hay
    // un tope total propio: la petición incluye la inferencia, cuya duración
    // crece con el texto, así que sigue el mismo criterio que `/transcribe`
    // (el timeout de conexión y el failsafe global del cliente del daemon).
    let resp = await_with_progress(progress, fut)
        .await
        .map_err(|e| daemon_connect_error(&client_addr, &e))?;
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

/// Resultado de una síntesis por daemon: el texto realmente sintetizado (ya
/// traducido si los idiomas difieren) y los datos de la entrega del audio.
struct DaemonSynthesis {
    translated: String,
    stats: DeliveryStats,
}

/// POST /synthesize al daemon y consumo de su stream NDJSON: cada evento `audio`
/// va a `delivery` según llega, y el evento `result` aporta `translated`. El seed
/// opcional fija la toma; sin él el daemon usa el de producción. Si el stream falla
/// después de empezar a sonar, se corta la reproducción y se propaga el error.
// Cada argumento mapea 1:1 a un campo de la petición y el indicador es del llamador.
#[allow(clippy::too_many_arguments)]
async fn daemon_synthesize_streaming(
    progress: &PhaseProgress,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
    seed: Option<u64>,
    mut delivery: StreamDelivery,
) -> Result<DaemonSynthesis, CliError> {
    let mut payload = serde_json::json!({ "text": text, "voice": voice, "source_language": source_language, "target_language": target_language });
    if let Some(t) = temperature {
        payload["temperature"] = serde_json::json!(t);
    }
    if let Some(s) = seed {
        payload["seed"] = serde_json::json!(s);
    }
    // El POST apunta a la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    progress.phase(&stage::STARTED);
    let streamed = async {
        let resp = await_with_progress(
            progress,
            client
                .post(format!("http://{}/synthesize", client_addr))
                .json(&payload)
                .send(),
        )
        .await
        .map_err(|e| daemon_connect_error(&client_addr, &e))?;
        if !resp.status().is_success() {
            return Err(daemon_error_from_response(resp).await);
        }
        consume_ndjson_stream(resp, StreamResult::Synthesize, progress, &mut delivery).await
    }
    .await;
    let val = match streamed {
        Ok(val) => val,
        Err(e) => {
            delivery.abort();
            return Err(e);
        }
    };
    let Some(translated) = val["translated"].as_str().map(str::to_string) else {
        delivery.abort();
        return Err(CliError::new(
            ExitCode::Error,
            "synthesis_failed",
            "El daemon devolvió result sin translated.",
        ));
    };
    Ok(DaemonSynthesis {
        translated,
        stats: delivery.finish(progress)?,
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

/// Espera el siguiente fragmento del stream hasta `wait` como máximo,
/// repintando el indicador cada `PROGRESS_REPAINT_INTERVAL` mientras no llega.
/// Devuelve `None` si agotó `wait` sin fragmento; el plazo total es el mismo
/// que con una sola espera. En cada repintado llama también a `on_repaint`, para
/// que quien consume refleje la reproducción sin esperar al siguiente evento.
async fn next_chunk_repainting<P: ProgressTicker + ?Sized>(
    resp: &mut reqwest::Response,
    wait: std::time::Duration,
    progress: &P,
    on_repaint: &mut dyn FnMut(),
) -> Option<Result<Option<String>, reqwest::Error>> {
    let end = tokio::time::Instant::now() + wait;
    loop {
        let left = end.checked_duration_since(tokio::time::Instant::now())?;
        let slice = std::cmp::min(PROGRESS_REPAINT_INTERVAL, left);
        if let Ok(result) = tokio::time::timeout(slice, resp.chunk()).await {
            return Some(
                result.map(|chunk| chunk.map(|bytes| String::from_utf8_lossy(&bytes).into_owned())),
            );
        }
        progress.repaint();
        on_repaint();
    }
}

/// Consume un stream NDJSON del daemon (`started` → latidos → eventos `audio` →
/// `result`/`error`) con timeout de inactividad + failsafe total. Cada evento
/// `audio` (PCM de 16 bits, 24 kHz, mono, en base64) va a `audio` según llega, y la
/// etapa visible pasa a la de reproducción en cuanto suena la primera muestra.
/// Retorna el evento final `result`; el evento `error` se mapea a `CliError` con la
/// tabla única.
/// Sin `result` (stream truncado, NDJSON inválido, inactividad o failsafe) el
/// fallo es ruidoso: nunca se reemite un éxito parcial.
async fn consume_ndjson_stream(
    mut resp: reqwest::Response,
    expected: StreamResult,
    progress: &PhaseProgress,
    audio: &mut dyn AudioReceiver,
) -> Result<Value, CliError> {
    let deadline = tokio::time::Instant::now() + STREAM_TOTAL_DEADLINE;
    let command_stage = expected.stage();
    let mut rest = String::new();
    // Los diagnósticos nombran la dirección descubierta.
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    loop {
        let Some(remaining) = deadline.checked_duration_since(tokio::time::Instant::now()) else {
            return Err(CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!(
                    "El daemon en {} no terminó la operación {} en el límite total de {} s.",
                    client_addr,
                    command_stage,
                    STREAM_TOTAL_DEADLINE.as_secs()
                ),
            ));
        };
        let wait = std::cmp::min(STREAM_INACTIVITY_TIMEOUT, remaining);
        let chunk =
            next_chunk_repainting(&mut resp, wait, progress, &mut || audio.observe(progress))
                .await
                .ok_or_else(|| {
                    CliError::new(
                        ExitCode::DaemonUnreachable,
                        "daemon_unreachable",
                        format!(
                            "El daemon en {} dejó de enviar eventos durante la operación {}.",
                            client_addr, command_stage
                        ),
                    )
                })?
                .map_err(|e| daemon_connect_error(&client_addr, &e))?;
        // Un fragmento recién llegado (latido o evento) también repinta: el
        // contador no debe esperar al siguiente fragmento para avanzar.
        progress.tick();
        let bytes = match chunk {
            Some(b) => b,
            None => {
                // Fin de stream: procesar el resto acumulado (línea sin `\n`
                // final) y exigir el evento final.
                let line = std::mem::take(&mut rest);
                if !line.trim().is_empty() {
                    if let Some(v) = process_stream_line(&line)? {
                        audio.synthesis_finished(progress);
                        return Ok(v);
                    }
                }
                return Err(CliError::new(
                    ExitCode::Error,
                    "daemon_error",
                    format!(
                        "El stream del daemon terminó sin evento final en {}.",
                        command_stage
                    ),
                ));
            }
        };
        rest.push_str(&bytes);
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
            // Presentación: el primer evento de la síntesis, sea espera o
            // trabajo, abre la etapa `Síntesis`, cuyo reloj es el del primer
            // audio: la espera cuenta. Una vez que suena, los latidos de la
            // síntesis no cambian la etapa visible: la reproducción corre en
            // paralelo.
            if let Some((event_stage, active)) = stream_event_phase(&ev) {
                if !(audio.started() && event_stage.label == stage::SYNTHESIZING.label) {
                    progress.phase_with(event_stage, active);
                }
            }
            // Protocolo: cada evento por su tipo.
            match ev.get("event").and_then(|v| v.as_str()) {
                Some("audio") => {
                    audio.push(&decode_audio_event(&ev)?)?;
                    audio.observe(progress);
                }
                // Los textos del doblaje salen por stdout en cuanto llegan, antes
                // de que suene el audio. Cada uno cierra la etapa que lo produjo.
                Some(kind @ ("transcript" | "translation"))
                    if matches!(expected, StreamResult::Dub) =>
                {
                    let text = ev["text"].as_str().ok_or_else(|| {
                        CliError::new(
                            ExitCode::Error,
                            "daemon_error",
                            format!("Evento {kind} inválido del daemon: falta text"),
                        )
                    })?;
                    let label = if kind == "transcript" {
                        "Transcripción"
                    } else {
                        "Traducción"
                    };
                    progress.result(Some(label), text);
                }
                // La síntesis terminó de verdad: `Síntesis` se cierra aquí, aunque
                // quede audio por sonar.
                Some("result") => {
                    audio.synthesis_finished(progress);
                    return Ok(ev);
                }
                Some("error") => return Err(stream_error_event(&ev)),
                _ => {}
            }
        }
    }
}

/// Decodifica el PCM de un evento `audio` del daemon. Un evento sin `pcm_b64`, con
/// base64 inválido o con un número impar de bytes es un protocolo inválido.
fn decode_audio_event(ev: &Value) -> Result<Vec<i16>, CliError> {
    let invalid = |detail: String| {
        CliError::new(
            ExitCode::Error,
            "daemon_error",
            format!("Evento audio inválido del daemon: {detail}"),
        )
    };
    let b64 = ev["pcm_b64"]
        .as_str()
        .ok_or_else(|| invalid("falta pcm_b64".to_string()))?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(b64)
        .map_err(|e| invalid(format!("pcm_b64 no decodable: {e}")))?;
    if bytes.len() % 2 != 0 {
        return Err(invalid(format!(
            "pcm_b64 con {} bytes, no es PCM de 16 bits",
            bytes.len()
        )));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect())
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
            "El daemon no expone esta ruta (HTTP 404): es anterior a este programa o no incluye la función."
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

/// Traduce un evento intermedio del stream a su etapa del catálogo, sin escribir
/// ni consultar el reloj. La etiqueta de la etapa es la clave de racionado:
/// `progress` y `heartbeat` de una misma etapa comparten etiqueta. Solo se leen
/// `event` y `stage`; los eventos y las etapas desconocidos no anuncian fase.
///
/// Devuelve la etapa del catálogo y su texto activo. Etapa del daemon → etapa:
/// `queued`, `warming`, `loading_voice` y `synthesis` son la misma etapa
/// `SYNTHESIZING` con distinto texto activo (esperar turno, calentar el motor,
/// cargar la voz o sintetizar), de modo que la espera comparte el reloj de la
/// síntesis; `denoise_queue` se conserva; `denoise` → `DENOISING`, `clone` →
/// `CLONING`, `transcribe` → `TRANSCRIBING`, `translate` → `TRANSLATING`; el
/// evento `started` → `STARTED`.
fn stream_event_phase(ev: &Value) -> Option<(&'static Stage, &'static str)> {
    let with_active = |stage: &'static Stage| Some((stage, stage.active));
    match ev.get("event").and_then(Value::as_str) {
        Some("started") => with_active(&stage::STARTED),
        Some("progress" | "heartbeat") => match ev.get("stage").and_then(Value::as_str) {
            Some("queued") => Some((&stage::SYNTHESIZING, stage::SYNTHESIS_QUEUED_TEXT)),
            Some("warming") => Some((&stage::SYNTHESIZING, stage::SYNTHESIS_WARMING_TEXT)),
            Some("loading_voice") => {
                Some((&stage::SYNTHESIZING, stage::SYNTHESIS_LOADING_VOICE_TEXT))
            }
            Some("synthesis") => with_active(&stage::SYNTHESIZING),
            Some("denoise_queue") => with_active(&stage::DENOISE_QUEUE),
            Some("denoise") => with_active(&stage::DENOISING),
            Some("clone") => with_active(&stage::CLONING),
            Some("transcribe") => with_active(&stage::TRANSCRIBING),
            Some("translate") => with_active(&stage::TRANSLATING),
            _ => None,
        },
        _ => None,
    }
}

/// `synthesize` vía daemon: persiste el WAV con `persist_locution` y respeta
/// --label/--play, devolviendo la ruta del WAV persistido (paralelo al
/// handler local para que el envelope JSON de salida coincida). La identidad
/// de la voz se resuelve en el almacén compartido antes de pedir la síntesis,
/// así que una voz que cambia mientras el daemon sintetiza no se guarda.
// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech synthesize`.
#[allow(clippy::too_many_arguments)]
async fn synthesize_via_daemon(
    progress: &PhaseProgress,
    client: &reqwest::Client,
    text: &str,
    voice: &VoiceName,
    label: &Label,
    policy: avi_store::CollisionPolicy,
    play: bool,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<SynthesizeOutcome, CliError> {
    let speech_store = open_speech_store()?;
    let voice_store = open_voice_store()?;
    let reference = resolve_voice_reference(&voice_store, voice)?;
    // La toma suena mientras llega solo con --play; siempre se acumula para guardarla.
    let delivery = open_delivery(
        play,
        true,
        format!("No se pudo reproducir el audio sintetizado '{}'", label),
    )?;
    let first_take = daemon_synthesize_streaming(
        progress,
        client,
        text,
        voice.as_str(),
        source_language,
        target_language,
        temperature,
        None,
        delivery,
    )
    .await?;
    // Texto realmente sintetizado: el que ven el menú y el sidecar (paridad
    // con la ruta directa, que guarda el traducido).
    let final_text = first_take.translated.as_str();
    // Guardián del temporal: lo borra al salir de ámbito por cualquier camino.
    let tmp_guard = avi_process::scratch::temp_wav("avi_tts_")
        .map_err(|e| CliError::new(ExitCode::Error, "io_error", format!("{e:#}")))?;
    let tmp = tmp_guard.path().clone();
    write_take_wav(&tmp, &first_take.stats.pcm)?;

    let saved = if play {
        // Bucle interactivo en el cliente; el bucle vive en el llamador y la
        // opción 3 re-despacha la síntesis por la misma vía daemon.
        let mut reader = menu_stdin();
        let mut playback = menu_playback(label, progress);
        // La etapa de reproducción termina antes del menú: su duración no
        // incluye lo que tarde el usuario en elegir.
        progress.end_stage();
        let mut resynthesizer = DaemonResynthesizer {
            client,
            text,
            voice: voice.as_str(),
            label,
            source_language,
            target_language,
            temperature,
            tmp_wav: &tmp,
            progress,
        };
        let outcome = play_menu_loop(
            voice,
            label,
            policy,
            &voice_store,
            &speech_store,
            reference.as_ref(),
            final_text,
            &tmp,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await;
        outcome?
    } else {
        progress.phase(&stage::SAVING);
        avi_store::persist_locution(
            &voice_store,
            &speech_store,
            reference.as_ref(),
            voice,
            label,
            final_text,
            &tmp,
            policy,
        )
        .map_err(persist_cli_error)?
        .into()
    };
    Ok(saved)
}

/// `say` vía daemon: reproduce el audio según llega en los eventos `audio` y
/// devuelve los datos de la entrega.
// Cada argumento representa un dato de la orden y el indicador pertenece al llamador.
#[allow(clippy::too_many_arguments)]
async fn say_via_daemon(
    progress: &PhaseProgress,
    client: &reqwest::Client,
    text: &str,
    voice: &str,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
) -> Result<DeliveryStats, CliError> {
    // El dispositivo se abre antes de pedir la síntesis.
    let delivery = open_delivery(true, false, "No se pudo reproducir el audio sintetizado")?;
    Ok(daemon_synthesize_streaming(
        progress,
        client,
        text,
        voice,
        source_language,
        target_language,
        temperature,
        None,
        delivery,
    )
    .await?
    .stats)
}

/// Confirmación destructiva de sobrescribir la voz `name`, que tiene las
/// locuciones `labels`: se borrarán con su contenido anterior. Sin terminal y sin
/// `--yes` falla con `confirmation_required`; una cancelación no cambia nada.
fn confirm_voice_replacement(
    voice_store: &VoiceStore,
    speech_store: &SpeechStore,
    name: &VoiceName,
    labels: &[String],
    yes: bool,
) -> Result<lifecycle::confirm::Decision, CliError> {
    let voice_dir = voice_store.voice_dir(name);
    let speech_dir = speech_store.voice_dir(name);
    let summary = vec![format!(
        "Se sobrescribirá la voz '{}' y se borrarán sus {}, grabados con su contenido anterior: {} (~{}).",
        name,
        avi_term::fmt::count(
            labels.len() as u64,
            "audio sintetizado",
            "audios sintetizados"
        ),
        labels.join(", "),
        avi_term::fmt::bytes(lifecycle::cleanup::path_size(&speech_dir)),
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
    let question = format!(
        "¿Sobrescribir la voz {} y borrar sus {}?",
        name,
        avi_term::fmt::count(
            labels.len() as u64,
            "audio sintetizado",
            "audios sintetizados"
        )
    );
    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    // La espera de la respuesta no cuenta como trabajo del comando.
    avi_term::report::begin_question();
    let decision = lifecycle::confirm::confirm(
        &lifecycle::confirm::Confirmation {
            kind: lifecycle::confirm::Kind::Destructive,
            question: &question,
            summary: &summary,
            entries: &entries,
            assume_yes: yes,
            dry_run: false,
            stdin_is_terminal: std::io::IsTerminal::is_terminal(&stdin),
        },
        &mut input,
        &mut std::io::stderr(),
    );
    avi_term::report::end_question(matches!(
        decision,
        Ok(lifecycle::confirm::Decision::Proceed)
    ));
    decision.map_err(lifecycle_error_to_cli)
}

async fn clone_via_daemon(
    client: &reqwest::Client,
    name: &str,
    audio_bytes: &[u8],
    force: bool,
    denoise: bool,
    progress: &PhaseProgress,
) -> Result<(String, String, bool, Vec<String>), CliError> {
    // La etapa de clonación la abre el stream del daemon.
    progress.phase(&stage::STARTED);
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
    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        await_with_progress(progress, fut),
    )
    .await
    .map_err(|_| {
        CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("El daemon en {} no respondió a tiempo.", client_addr),
        )
    })?
    .map_err(|e| daemon_connect_error(&client_addr, &e))?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    let val: Value =
        consume_ndjson_stream(resp, StreamResult::Clone, progress, &mut RejectAudio).await?;
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
    let removed_speech = val["removed_speech"]
        .as_array()
        .and_then(|labels| {
            labels
                .iter()
                .map(|label| label.as_str().map(str::to_string))
                .collect::<Option<Vec<String>>>()
        })
        .ok_or_else(|| {
            CliError::new(
                ExitCode::Error,
                "daemon_error",
                "El daemon no devolvió 'removed_speech' en la clonación.",
            )
        })?;
    Ok((
        cloned_name.to_string(),
        cloned_speech.to_string(),
        precomputed,
        removed_speech,
    ))
}

/// Resultado de un doblaje, igual en la ruta local y en la del daemon.
#[derive(Debug)]
struct DubOutcome {
    /// Texto reconocido en el audio de entrada (el STT).
    transcript: String,
    /// Texto que se sintetiza: la traducción, o la propia transcripción cuando
    /// el idioma de origen y el de destino coinciden.
    translated: String,
    /// Milisegundos hasta la primera muestra entregada al dispositivo.
    first_audio_ms: Option<u64>,
    /// Veces que la cola de reproducción se vació con la entrega en curso.
    stalls: u32,
}

/// Envelope `--json` de `speech dub`: `text` es la transcripción y `translated`
/// el texto sintetizado.
fn dub_envelope(outcome: &DubOutcome) -> Value {
    json!({
        "status": "dubbed",
        "text": outcome.transcript,
        "translated": outcome.translated,
        "first_audio_ms": outcome.first_audio_ms,
        "stalls": outcome.stalls,
    })
}

/// Cierra `speech dub` en las dos rutas, que son su único punto de salida. Los
/// textos de la transcripción y la traducción ya salieron por stdout en cuanto
/// existieron; aquí se escribe la línea de cierre por stderr o, en JSON, el
/// envelope. Un fallo borra la línea activa sin cierre.
fn finish_dub(
    json_mode: bool,
    progress: PhaseProgress,
    result: Result<DubOutcome, CliError>,
) -> Result<(), CliError> {
    finish_progress(progress, &result, "Doblaje completado", |outcome| {
        streaming_closing_extras(outcome.first_audio_ms, outcome.stalls)
    });
    let outcome = result?;
    if json_mode {
        emit_raw_json(dub_envelope(&outcome));
    }
    Ok(())
}

/// Lee `text` y `translated` del evento `result` del daemon y les añade los datos de
/// la entrega. Un campo ausente o que no es texto es un error del daemon: no hay
/// valor por defecto.
fn dub_outcome_from_result(val: &Value, stats: &DeliveryStats) -> Result<DubOutcome, CliError> {
    let field = |name: &str| {
        val[name].as_str().map(str::to_string).ok_or_else(|| {
            CliError::new(
                ExitCode::Error,
                "daemon_error",
                format!("El daemon no devolvió '{name}' en el doblaje."),
            )
        })
    };
    Ok(DubOutcome {
        transcript: field("text")?,
        translated: field("translated")?,
        first_audio_ms: stats.first_audio_ms,
        stalls: stats.stalls,
    })
}

/// Envelope `--json` de `speech transcribe`.
fn transcription_envelope(text: &str, source_language: &str) -> Value {
    json!({ "text": text, "source_language": source_language })
}

/// Cierra `speech transcribe` en las dos rutas, que son su único punto de
/// salida: con éxito, el texto por stdout y la línea de cierre por stderr (o el
/// envelope en JSON); con fallo, borra la línea activa sin cierre.
fn finish_transcription(
    json_mode: bool,
    progress: PhaseProgress,
    result: Result<String, CliError>,
    source_language: &str,
) -> Result<(), CliError> {
    if let Ok(text) = &result {
        progress.result(None, text);
    }
    finish_progress(progress, &result, "Transcripción completada", |_| {
        Vec::new()
    });
    let text = result?;
    if json_mode {
        emit_raw_json(transcription_envelope(&text, source_language));
    }
    Ok(())
}

// Firma ancha deliberada: cada argumento mapea 1:1 a un flag de `speech dub`.
#[allow(clippy::too_many_arguments)]
async fn dub_via_daemon(
    progress: &PhaseProgress,
    client: &reqwest::Client,
    audio: Option<&str>,
    mic: bool,
    duration: Option<u64>,
    source_language: &str,
    target_language: &str,
    temperature: Option<f32>,
    denoise: bool,
    voice: &str,
) -> Result<DubOutcome, CliError> {
    // Captura/lectura PCM y encode a base64 para POST /dub
    let pcm = acquire_pcm_with_progress(
        progress,
        audio,
        mic,
        duration,
        avi_core::MAX_DUB_AUDIO_SECS,
        "El audio del doblaje",
    )
    .await?;
    // El dispositivo se abre antes de pedir la síntesis.
    let mut delivery = open_delivery(true, false, "Fallo al reproducir el doblaje")?;
    progress.phase(&stage::STARTED);
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
    let resp = tokio::time::timeout(
        std::time::Duration::from_millis(1500),
        await_with_progress(progress, fut),
    )
    .await
    .map_err(|_| {
        CliError::new(
            ExitCode::DaemonUnreachable,
            "daemon_unreachable",
            format!("El daemon en {} no respondió a tiempo.", client_addr),
        )
    })?
    .map_err(|e| daemon_connect_error(&client_addr, &e))?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    // Los eventos `audio` llevan el audio doblado; el evento final trae la forma
    // contractual {status:"dubbed", text, translated, voice}, donde `text` es la
    // transcripción y `translated` el texto que se sintetiza. Los eventos `error`
    // del stream se traducen con la misma tabla única; si llegan con la voz ya
    // sonando, se corta la reproducción.
    let val: Value =
        match consume_ndjson_stream(resp, StreamResult::Dub, progress, &mut delivery).await {
            Ok(val) => val,
            Err(e) => {
                delivery.abort();
                return Err(e);
            }
        };
    dub_outcome_from_result(&val, &delivery.finish(progress)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ctrl+C solo mata el daemon que lanzó este proceso: sin PID en memoria,
    /// o con el propio, no mata nada.
    #[test]
    fn daemon_to_kill_on_interrupt_only_returns_the_spawned_pid() {
        use std::sync::atomic::Ordering;

        IN_MEMORY_PID.store(0, Ordering::Relaxed);
        assert_eq!(daemon_to_kill_on_interrupt(), None);
        IN_MEMORY_PID.store(std::process::id(), Ordering::Relaxed);
        assert_eq!(daemon_to_kill_on_interrupt(), None);
        let foreign = std::process::id().wrapping_add(1);
        IN_MEMORY_PID.store(foreign, Ordering::Relaxed);
        assert_eq!(daemon_to_kill_on_interrupt(), Some(foreign));
        IN_MEMORY_PID.store(0, Ordering::Relaxed);
    }

    // Las pruebas de integración de `transcribe` y `dub` exigen modelos y un
    // dispositivo de audio; estas pruebas puras son la fuente determinista del
    // contrato de salida de ambos comandos, igual en la ruta local y en la del daemon.

    #[test]
    fn dub_envelope_carries_transcript_as_text_and_translation() {
        let outcome = DubOutcome {
            transcript: "hola mundo".to_string(),
            translated: "hello world".to_string(),
            first_audio_ms: Some(1234),
            stalls: 0,
        };
        assert_eq!(
            dub_envelope(&outcome),
            json!({
                "status": "dubbed",
                "text": "hola mundo",
                "translated": "hello world",
                "first_audio_ms": 1234,
                "stalls": 0,
            })
        );
    }

    /// Los textos del doblaje salen por stdout con su etiqueta cuando no hay
    /// terminal, y el cierre no los repite.
    #[test]
    fn dub_texts_are_labelled_results_on_stdout() {
        assert_eq!(
            avi_term::progress::result_line(false, false, 2, Some("Transcripción"), "hola mundo"),
            "Transcripción: hola mundo"
        );
        assert_eq!(
            avi_term::progress::result_line(false, false, 2, Some("Traducción"), "hello world"),
            "Traducción: hello world"
        );
    }

    #[test]
    fn dub_outcome_from_result_reads_text_and_translated() {
        let event =
            json!({ "event": "result", "status": "dubbed", "text": "hola", "translated": "hello" });
        let stats = DeliveryStats {
            first_audio_ms: Some(950),
            stalls: 2,
            pcm: Vec::new(),
        };
        let outcome = dub_outcome_from_result(&event, &stats).expect("evento completo");
        assert_eq!(outcome.transcript, "hola");
        assert_eq!(outcome.translated, "hello");
        assert_eq!(outcome.first_audio_ms, Some(950));
        assert_eq!(outcome.stalls, 2);
    }

    #[test]
    fn dub_outcome_from_result_rejects_missing_fields_without_fallback() {
        for event in [
            json!({ "status": "dubbed", "text": "hola" }),
            json!({ "status": "dubbed", "translated": "hello" }),
            json!({ "status": "dubbed", "text": 1, "translated": "hello" }),
        ] {
            let stats = DeliveryStats {
                first_audio_ms: None,
                stalls: 0,
                pcm: Vec::new(),
            };
            let err =
                dub_outcome_from_result(&event, &stats).expect_err("campo ausente o inválido");
            assert_eq!(err.reason, "daemon_error", "{event}");
        }
    }

    #[test]
    fn transcription_envelope_has_text_and_source_language() {
        assert_eq!(
            transcription_envelope("hola", "es"),
            json!({ "text": "hola", "source_language": "es" })
        );
    }

    #[test]
    fn clone_pcm_truncates_at_thirty_seconds_without_padding() {
        for sample_rate in [24_000u32, 48_000] {
            let limit = (avi_core::MAX_CLONE_REFERENCE_SECS * u64::from(sample_rate)) as usize;
            let mut over = vec![7; limit + 1];
            super::truncate_clone_pcm(&mut over, sample_rate);
            assert_eq!(over.len(), limit, "tasa {sample_rate}");

            let mut exact = vec![7; limit];
            super::truncate_clone_pcm(&mut exact, sample_rate);
            assert_eq!(exact.len(), limit, "tasa {sample_rate}");

            let mut short = vec![7; limit - 1];
            super::truncate_clone_pcm(&mut short, sample_rate);
            assert_eq!(short.len(), limit - 1, "tasa {sample_rate}");
        }
    }

    #[test]
    fn clone_pcm_wav_base64_round_trip_preserves_known_samples() {
        let samples = [0, 1_234, -2_345, i16::MAX, i16::MIN];
        let wav = avi_audio::encode_pcm_mono_wav(&samples, 24_000).expect("codificar WAV");
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wav);
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("decodificar base64");
        let mut reader = hound::WavReader::new(std::io::Cursor::new(decoded)).expect("leer WAV");
        assert_eq!(reader.spec().sample_rate, 24_000);
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
        let samples = vec![123; 48_000 * 30];
        let started = std::time::Instant::now();
        let wav = avi_audio::encode_pcm_mono_wav(&samples, 48_000).expect("codificar WAV");
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
            device_line(&json!({ "id": 1, "name": null, "latency": null }), None),
            "  [1] desconocido · latencia desconocida"
        );
        assert_eq!(
            device_line(
                &json!({ "id": 0, "name": "Altavoces", "latency": 0.0105 }),
                None
            ),
            "  [0] Altavoces · latencia 10,5 ms"
        );
    }

    /// La salida predeterminada se marca en la lista por su nombre; un nombre
    /// `null` nunca coincide.
    #[test]
    fn device_line_marks_the_default_output() {
        let dev = json!({ "id": 0, "name": "Altavoces", "latency": 0.0105 });
        assert_eq!(
            device_line(&dev, Some("Altavoces")),
            "  [0] Altavoces · latencia 10,5 ms · predeterminada"
        );
        assert_eq!(
            device_line(&dev, Some("Auriculares")),
            "  [0] Altavoces · latencia 10,5 ms"
        );
        assert_eq!(
            device_line(&json!({ "id": 1, "name": null, "latency": null }), None),
            "  [1] desconocido · latencia desconocida"
        );
    }

    /// Un predeterminado ausente se muestra como «ninguna», uno sin nombre como
    /// «desconocido» y uno con nombre, tal cual.
    #[test]
    fn default_device_line_shows_none_unknown_and_name() {
        assert_eq!(
            default_device_line("Entrada predeterminada", &Value::Null),
            "Entrada predeterminada: ninguna"
        );
        assert_eq!(
            default_device_line("Salida predeterminada", &json!({ "name": null })),
            "Salida predeterminada: desconocido"
        );
        assert_eq!(
            default_device_line("Salida predeterminada", &json!({ "name": "Altavoces" })),
            "Salida predeterminada: Altavoces"
        );
    }

    /// Los fallos de entorno se clasifican por tipo en cualquier operación; el
    /// resto conserva el código, el `reason` y el mensaje de la operación.
    #[test]
    fn audio_error_classifies_environment_by_type() {
        let backend = avi_audio::AudioBackendUnavailable {
            library: "libasound.so.2",
            missing: None,
        };
        let cases: [(anyhow::Error, i32, &str); 4] = [
            (backend.into(), 3, "audio_backend_unavailable"),
            (avi_audio::NoInputDevice.into(), 3, "no_input_device"),
            (avi_audio::NoOutputDevice.into(), 3, "no_output_device"),
            (avi_audio::EmptyAudio.into(), 2, "empty_audio"),
        ];
        for (error, code, reason) in cases {
            let cli = audio_error(error, ExitCode::Error, "playback_failed", |_| {
                unreachable!("el entorno no usa el mensaje de la operación")
            });
            assert_eq!((cli.code.code(), cli.reason.as_str()), (code, reason));
        }

        let other = audio_error(
            anyhow::anyhow!("stream roto"),
            ExitCode::Error,
            "playback_failed",
            |e| format!("Fallo al reproducir: {e}"),
        );
        assert_eq!(
            (
                other.code.code(),
                other.reason.as_str(),
                other.message.as_str()
            ),
            (1, "playback_failed", "Fallo al reproducir: stream roto")
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
        let res = await_daemon_ready(&addr, deadline, interval, None).await;
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

    /// Sirve en loopback respuestas de `/health` con las etiquetas `warm` dadas,
    /// una por conexión; la última se repite.
    async fn serve_health_sequence(labels: Vec<&'static str>) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind efímero");
        let addr = listener.local_addr().expect("local_addr").to_string();
        tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            let mut served = 0usize;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let mut buf = [0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let label = labels[served.min(labels.len() - 1)];
                served += 1;
                let body = format!(r#"{{"status":"ready","warm":"{label}"}}"#);
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            }
        });
        addr
    }

    /// El arranque espera a que `/health` pase de `warming` a `warm`, la misma
    /// condición con la que `daemon status` dice «motor listo».
    #[tokio::test]
    async fn await_engine_warm_waits_until_health_reports_warm() {
        let addr = serve_health_sequence(vec!["warming", "warming", "warm"]).await;
        let started = std::time::Instant::now();
        let res = await_engine_warm(
            &addr,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(50),
            None,
        )
        .await;
        assert!(matches!(res, Ok(true)), "{res:?}");
        assert!(started.elapsed() >= std::time::Duration::from_millis(100));
    }

    /// El cierre del arranque dice en una sola línea cómo quedó el motor: listo,
    /// aún calentándose o con el calentamiento fallido.
    #[test]
    fn daemon_launch_closing_reports_the_engine_state() {
        let (action, extras) =
            daemon_launch_closing("Daemon listo", "Daemon en marcha", 42, &EngineWarm::Ready);
        assert_eq!(action, "Daemon listo");
        assert_eq!(extras, vec!["pid 42".to_string()]);

        let (action, extras) = daemon_launch_closing(
            "Daemon listo",
            "Daemon en marcha",
            42,
            &EngineWarm::StillWarming,
        );
        assert_eq!(action, "Daemon en marcha");
        assert_eq!(
            extras,
            vec![
                "pid 42".to_string(),
                "el motor sigue calentándose; `ai-voice-interconnector daemon status` dirá cuándo está listo"
                    .to_string(),
            ]
        );

        let (action, extras) = daemon_launch_closing(
            "Daemon reiniciado",
            "Daemon reiniciado",
            7,
            &EngineWarm::Failed,
        );
        assert_eq!(action, "Daemon reiniciado");
        assert_eq!(
            extras,
            vec![
                "pid 7".to_string(),
                "falló el calentamiento del motor; consulta `ai-voice-interconnector daemon status`"
                    .to_string(),
            ]
        );
    }

    /// Un calentamiento fallido deja de esperar sin darlo por logrado, y uno
    /// que no termina agota el plazo con error.
    #[tokio::test]
    async fn await_engine_warm_reports_failure_and_deadline() {
        let failed = serve_health_sequence(vec!["warming", "warm_failed"]).await;
        let res = await_engine_warm(
            &failed,
            std::time::Duration::from_secs(5),
            std::time::Duration::from_millis(50),
            None,
        )
        .await;
        assert!(matches!(res, Ok(false)), "{res:?}");

        let stuck = serve_health_sequence(vec!["warming"]).await;
        let res = await_engine_warm(
            &stuck,
            std::time::Duration::from_millis(300),
            std::time::Duration::from_millis(50),
            None,
        )
        .await;
        assert!(res.is_err(), "{res:?}");
    }

    /// La falta del modelo Base sale como `model_missing` (exit 4), igual que un
    /// motor sin provisionar, y no como `synthesis_failed`.
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

    /// Una voz sin `reference.qvoice` sale como `voice_not_found` (exit 3), sin
    /// necesitar el motor real.
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
            status_line(
                DaemonState::Running { pid: 1234 },
                health.as_ref(),
                "127.0.0.1:8765",
            )
        };
        let cases = [
            (Some("warming"), "motor calentándose"),
            (Some("warm"), "motor listo"),
            (Some("warm_failed"), "falló el calentamiento del motor"),
            (Some("otro"), "estado del motor desconocido"),
        ];
        for (warm, text) in cases {
            let expected = format!("Daemon: en marcha (pid 1234, puerto 8765, {text}).");
            assert_eq!(line(warm), expected);
            assert!(!line(warm).contains("rust_native"));
            assert!(!line(warm).contains("warm_"));
        }
        let absent = status_line(DaemonState::Running { pid: 1234 }, None, "127.0.0.1:8765");
        assert_eq!(
            absent,
            "Daemon: en marcha (pid 1234, puerto 8765, estado del motor desconocido)."
        );
        assert_eq!(
            status_line(DaemonState::Stopped, None, "127.0.0.1:8765"),
            "Daemon: detenido."
        );
        let degraded = status_line(
            DaemonState::Degraded(Degradation::Unresponsive),
            None,
            "127.0.0.1:8765",
        );
        assert!(degraded.starts_with("Daemon: en marcha, pero no responde: "));
        assert!(degraded.ends_with('.'));
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
            r#"{"event":"progress","stage":"loading_voice"}"#,
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
        // La orden de reinicio ya no va en el mensaje: es la solución de `daemon_error`.
        assert!(err.message.contains("HTTP 404"), "mensaje: {}", err.message);
        assert!(!err.message.contains("daemon restart"), "{}", err.message);
        assert!(avi_core::exit_codes::solution_for(&err.reason)
            .is_some_and(|s| s.contains("ai-voice-interconnector daemon restart")));
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
        let progress = PhaseProgress::new();
        let val = consume_ndjson_stream(resp, StreamResult::Clone, &progress, &mut RejectAudio)
            .await
            .expect("la secuencia de éxito entrega el final");
        assert_eq!(val["event"], "result");
        assert_eq!(val["name"], "v");
        assert_eq!(val["precomputed"], true);
    }

    /// El doblaje recibe `transcript` y `translation` entre los eventos de
    /// progreso y los consume sin error, y el final sigue siendo el `result`.
    #[tokio::test]
    async fn consume_ndjson_stream_dub_accepts_transcript_and_translation() {
        let addr = serve_programmed_sequence(
            vec![
                r#"{"event":"started"}"#.to_string(),
                r#"{"event":"progress","stage":"transcribe"}"#.to_string(),
                r#"{"event":"transcript","text":"hola mundo"}"#.to_string(),
                r#"{"event":"progress","stage":"translate"}"#.to_string(),
                r#"{"event":"translation","text":"hello world"}"#.to_string(),
                r#"{"event":"result","status":"dubbed","text":"hola mundo","translated":"hello world"}"#
                    .to_string(),
            ],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/dub", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let progress = PhaseProgress::new();
        let val = consume_ndjson_stream(
            resp,
            StreamResult::Dub,
            &progress,
            &mut CollectingAudio::default(),
        )
        .await
        .expect("la secuencia del doblaje entrega el final");
        assert_eq!(val["event"], "result");
        assert_eq!(val["translated"], "hello world");
        let _elapsed = progress.close();
    }

    /// Un `transcript` sin `text` es un protocolo inválido del daemon.
    #[tokio::test]
    async fn consume_ndjson_stream_dub_rejects_transcript_without_text() {
        let addr = serve_programmed_sequence(
            vec![
                r#"{"event":"started"}"#.to_string(),
                r#"{"event":"transcript"}"#.to_string(),
            ],
            None,
        )
        .await;
        let client = daemon_client();
        let resp = client
            .post(format!("http://{}/dub", addr))
            .send()
            .await
            .expect("el doble debe responder");
        let progress = PhaseProgress::new();
        let err = consume_ndjson_stream(
            resp,
            StreamResult::Dub,
            &progress,
            &mut CollectingAudio::default(),
        )
        .await
        .expect_err("falta el texto");
        assert_eq!(err.reason, "daemon_error");
        let _elapsed = progress.close();
    }

    /// Receptor de pruebas: guarda los fragmentos y dice que ya suena tras el primero.
    #[derive(Default)]
    struct CollectingAudio {
        chunks: Vec<Vec<i16>>,
        /// Orden de las llamadas al receptor: `push` y `finished` (fin de la
        /// síntesis).
        calls: Vec<&'static str>,
    }

    impl AudioReceiver for CollectingAudio {
        fn push(&mut self, pcm: &[i16]) -> Result<(), CliError> {
            self.calls.push("push");
            self.chunks.push(pcm.to_vec());
            Ok(())
        }

        fn started(&self) -> bool {
            !self.chunks.is_empty()
        }

        fn observe(&mut self, _progress: &PhaseProgress) {}

        fn synthesis_finished(&mut self, _progress: &PhaseProgress) {
            self.calls.push("finished");
        }
    }

    /// Línea NDJSON de un evento `audio` con estas muestras.
    fn audio_event(samples: &[i16]) -> String {
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        json!({
            "event": "audio",
            "pcm_b64": base64::engine::general_purpose::STANDARD.encode(bytes),
        })
        .to_string()
    }

    /// Consume la secuencia programada de `/synthesize` con un receptor dado.
    async fn consume_synthesis(
        lines: Vec<String>,
        audio: &mut dyn AudioReceiver,
    ) -> Result<Value, CliError> {
        let addr = serve_programmed_sequence(lines, None).await;
        let resp = daemon_client()
            .post(format!("http://{addr}/synthesize"))
            .send()
            .await
            .expect("el doble debe responder");
        let progress = PhaseProgress::new();
        let result = consume_ndjson_stream(resp, StreamResult::Synthesize, &progress, audio).await;
        let _elapsed = progress.close();
        result
    }

    /// Los eventos `audio` llegan al receptor en orden mientras la respuesta sigue
    /// abierta, y el resultado final ya no lleva el audio.
    #[tokio::test]
    async fn consume_ndjson_stream_synthesis_delivers_audio_events_in_order() {
        let mut audio = CollectingAudio::default();
        let val = consume_synthesis(
            vec![
                r#"{"event":"started","voice":"v"}"#.to_string(),
                r#"{"event":"heartbeat","stage":"warming"}"#.to_string(),
                r#"{"event":"progress","stage":"synthesis"}"#.to_string(),
                audio_event(&[1, 2, 3]),
                r#"{"event":"heartbeat","stage":"synthesis"}"#.to_string(),
                audio_event(&[-4, 5]),
                r#"{"event":"result","translated":"hola","work_ms":10}"#.to_string(),
            ],
            &mut audio,
        )
        .await
        .expect("la secuencia termina con result");
        assert_eq!(val["translated"], "hola");
        assert!(val.get("audio_b64").is_none());
        assert_eq!(audio.chunks, vec![vec![1, 2, 3], vec![-4, 5]]);
        assert_eq!(
            audio.calls.last(),
            Some(&"finished"),
            "el evento result cierra la síntesis para el receptor"
        );
    }

    /// Una espera de turno es texto vivo de la etapa de síntesis: no la cierra
    /// como hecha, y la síntesis solo se da por terminada al llegar `result`,
    /// después de todos los fragmentos.
    #[tokio::test]
    async fn consume_ndjson_stream_queued_wait_does_not_finish_synthesis() {
        let addr = serve_programmed_sequence(
            vec![
                r#"{"event":"started","voice":"v"}"#.to_string(),
                r#"{"event":"progress","stage":"queued"}"#.to_string(),
                r#"{"event":"heartbeat","stage":"queued"}"#.to_string(),
                r#"{"event":"progress","stage":"synthesis"}"#.to_string(),
                audio_event(&[1, 2, 3]),
                audio_event(&[4]),
                r#"{"event":"result","translated":"hola","work_ms":10}"#.to_string(),
            ],
            None,
        )
        .await;
        let resp = daemon_client()
            .post(format!("http://{addr}/synthesize"))
            .send()
            .await
            .expect("el doble debe responder");
        let progress = PhaseProgress::new();
        let mut audio = CollectingAudio::default();
        consume_ndjson_stream(resp, StreamResult::Synthesize, &progress, &mut audio)
            .await
            .expect("la secuencia termina con result");
        assert_eq!(audio.calls, vec!["push", "push", "finished"]);
        let _elapsed = progress.close();
    }

    /// Un `error` tras eventos `audio` ya entregados es un error ruidoso con su
    /// `reason`: nunca se da por buena una entrega parcial.
    #[tokio::test]
    async fn consume_ndjson_stream_error_after_audio_events_is_reported() {
        let mut audio = CollectingAudio::default();
        let err = consume_synthesis(
            vec![
                r#"{"event":"started","voice":"v"}"#.to_string(),
                audio_event(&[1, 2]),
                r#"{"event":"error","reason":"synthesis_failed","message":"se cortó"}"#.to_string(),
            ],
            &mut audio,
        )
        .await
        .expect_err("el error del stream se propaga");
        assert_eq!(err.reason, "synthesis_failed");
        assert_eq!(
            audio.chunks,
            vec![vec![1, 2]],
            "lo recibido antes ya se entregó"
        );
    }

    /// Un `pcm_b64` que no es base64, un evento sin `pcm_b64` o un PCM de longitud
    /// impar son un protocolo inválido del daemon.
    #[tokio::test]
    async fn consume_ndjson_stream_rejects_invalid_audio_events() {
        for event in [
            r#"{"event":"audio","pcm_b64":"%%%no-base64%%%"}"#,
            r#"{"event":"audio"}"#,
            r#"{"event":"audio","pcm_b64":"AAAA"}"#,
        ] {
            let mut audio = CollectingAudio::default();
            let err = consume_synthesis(
                vec![
                    r#"{"event":"started","voice":"v"}"#.to_string(),
                    event.to_string(),
                    r#"{"event":"result","translated":"hola"}"#.to_string(),
                ],
                &mut audio,
            )
            .await
            .expect_err("el evento audio inválido es un error");
            assert_eq!(err.reason, "daemon_error", "{event}");
            assert!(audio.chunks.is_empty(), "{event}");
        }
    }

    /// Un stream que no debe traer audio (clonado) rechaza el evento `audio`.
    #[tokio::test]
    async fn consume_ndjson_stream_without_audio_receiver_rejects_audio_events() {
        let addr = serve_programmed_sequence(
            vec![
                r#"{"event":"started","name":"v"}"#.to_string(),
                audio_event(&[1, 2]),
                r#"{"event":"result","name":"v"}"#.to_string(),
            ],
            None,
        )
        .await;
        let resp = daemon_client()
            .post(format!("http://{addr}/voices/clone"))
            .send()
            .await
            .expect("el doble debe responder");
        let progress = PhaseProgress::new();
        let err = consume_ndjson_stream(resp, StreamResult::Clone, &progress, &mut RejectAudio)
            .await
            .expect_err("el clonado no admite audio");
        assert_eq!(err.reason, "daemon_error");
        let _elapsed = progress.close();
    }

    /// Sin reproductor, la entrega acumula los fragmentos y no informa de
    /// reproducción: ni primer sonido ni cortes.
    #[test]
    fn stream_delivery_without_player_accumulates_the_audio() {
        let _g = avi_term::report::test_guard();
        let mut delivery =
            open_delivery(false, true, "Fallo al reproducir").expect("sin reproductor");
        assert!(!delivery.started());
        delivery.push(&[1, 2]).expect("acumular");
        delivery.push(&[3]).expect("acumular");
        let stats = delivery.finish(&PhaseProgress::new()).expect("terminar");
        assert_eq!(stats.pcm, vec![1, 2, 3]);
        assert_eq!(stats.first_audio_ms, None);
        assert_eq!(stats.stalls, 0);
    }

    /// Registro de la última etapa abierta con la etiqueta de `stage`.
    fn record_of(
        snapshot: &avi_term::report::Snapshot,
        stage: &stage::Stage,
    ) -> avi_term::report::StageRecord {
        snapshot
            .stages
            .iter()
            .rev()
            .find(|record| record.label == stage.label)
            .cloned()
            .expect("la etapa está en el registro")
    }

    /// El primer audio es una sola medida, `first_audio_at − inicio de Síntesis`:
    /// `Reproducción` empieza en la primera muestra, `Síntesis` se cierra al
    /// terminar la síntesis y el dato de la línea, el extra del cierre y el
    /// valor del JSON son el mismo número.
    #[test]
    fn overlapped_stages_share_a_single_first_audio_measure() {
        use std::time::Duration;
        let _g = avi_term::report::test_guard();
        let progress = PhaseProgress::new();
        progress.phase(&stage::SYNTHESIZING);
        let began = record_of(&avi_term::report::snapshot(), &stage::SYNTHESIZING).start;
        let first_audio_at = began + Duration::from_millis(600);
        let synthesis_end = began + Duration::from_millis(1400);
        let playback_end = began + Duration::from_millis(3500);
        let sounding = |stalls| audio::PlaybackStats {
            first_audio_at: Some(first_audio_at),
            stalls,
        };

        let mut stages = SynthesisStages::default();
        stages.observe(&progress, sounding(0));
        stages.synthesis_finished(&progress, sounding(0), synthesis_end);
        stages.playback_finished(&progress, sounding(1), playback_end);

        let snapshot = avi_term::report::snapshot();
        let synthesis = record_of(&snapshot, &stage::SYNTHESIZING);
        let playing = record_of(&snapshot, &stage::PLAYING);
        assert_eq!(synthesis.end, Some(synthesis_end));
        assert_eq!(synthesis.note.as_deref(), Some("primer audio 0,6 s"));
        assert_eq!(playing.start, first_audio_at);
        assert_eq!(playing.end, Some(playback_end));
        assert_eq!(
            playing.note.as_deref(),
            Some("en paralelo con la síntesis · 1 corte")
        );
        let first_audio = first_audio_offset(&snapshot, first_audio_at);
        assert_eq!(first_audio, Some(Duration::from_millis(600)));
        assert_eq!(
            streaming_closing_extras(first_audio.map(|d| d.as_millis() as u64), 1),
            vec!["primer audio 0,6 s".to_string(), "1 corte".to_string()],
            "el cierre repite el dato de la línea de Síntesis"
        );
        let _elapsed = progress.close();
    }

    /// Si la síntesis termina antes de que suene la primera muestra,
    /// `Reproducción` se abre después de cerrarla y no lleva el dato «en
    /// paralelo con la síntesis».
    #[test]
    fn playback_opened_after_the_synthesis_has_no_parallel_note() {
        use std::time::Duration;
        let _g = avi_term::report::test_guard();
        let progress = PhaseProgress::new();
        progress.phase(&stage::SYNTHESIZING);
        let began = record_of(&avi_term::report::snapshot(), &stage::SYNTHESIZING).start;
        let first_audio_at = began + Duration::from_millis(900);
        let synthesis_end = began + Duration::from_millis(500);
        let silent = audio::PlaybackStats {
            first_audio_at: None,
            stalls: 0,
        };

        let mut stages = SynthesisStages::default();
        stages.synthesis_finished(&progress, silent, synthesis_end);
        let sounding = audio::PlaybackStats {
            first_audio_at: Some(first_audio_at),
            stalls: 0,
        };
        stages.playback_finished(&progress, sounding, began + Duration::from_millis(2000));

        let snapshot = avi_term::report::snapshot();
        let synthesis = record_of(&snapshot, &stage::SYNTHESIZING);
        let playing = record_of(&snapshot, &stage::PLAYING);
        assert_eq!(synthesis.end, Some(synthesis_end));
        assert_eq!(
            synthesis.note, None,
            "aún no había primer audio al cerrarla"
        );
        assert_eq!(playing.start, first_audio_at);
        assert_eq!(playing.note, None);
        let _elapsed = progress.close();
    }

    /// La traducción pura fija la etapa del catálogo y el texto activo de cada
    /// etapa del daemon; `progress` y `heartbeat` de una misma etapa comparten
    /// etiqueta (clave de racionado) y texto. Las esperas de la síntesis son la
    /// misma etapa `SYNTHESIZING` con otro texto.
    #[test]
    fn stream_event_phase_maps_stages_and_rate_limits_by_stage() {
        let cases = [
            (
                "warming",
                &stage::SYNTHESIZING,
                stage::SYNTHESIS_WARMING_TEXT,
            ),
            ("queued", &stage::SYNTHESIZING, stage::SYNTHESIS_QUEUED_TEXT),
            (
                "loading_voice",
                &stage::SYNTHESIZING,
                stage::SYNTHESIS_LOADING_VOICE_TEXT,
            ),
            (
                "synthesis",
                &stage::SYNTHESIZING,
                stage::SYNTHESIZING.active,
            ),
            (
                "denoise_queue",
                &stage::DENOISE_QUEUE,
                stage::DENOISE_QUEUE.active,
            ),
            ("denoise", &stage::DENOISING, stage::DENOISING.active),
            ("clone", &stage::CLONING, stage::CLONING.active),
            (
                "transcribe",
                &stage::TRANSCRIBING,
                stage::TRANSCRIBING.active,
            ),
            ("translate", &stage::TRANSLATING, stage::TRANSLATING.active),
        ];
        for (daemon_stage, expected_stage, expected_text) in cases {
            let expected = Some((expected_stage, expected_text));
            let heartbeat = json!({ "event": "heartbeat", "stage": daemon_stage });
            let progress_event = json!({ "event": "progress", "stage": daemon_stage });
            assert_eq!(
                stream_event_phase(&heartbeat),
                expected,
                "latido de {daemon_stage}"
            );
            assert_eq!(
                stream_event_phase(&progress_event),
                stream_event_phase(&heartbeat),
                "ambos eventos racionan por etapa"
            );
        }
        assert!(stream_event_phase(&json!({ "event": "heartbeat", "stage": "unknown" })).is_none());
        assert!(stream_event_phase(&json!({ "event": "progress", "stage": "unknown" })).is_none());
        assert!(stream_event_phase(&json!({ "event": "result" })).is_none());
    }

    /// El evento `started` anuncia la etapa de inicio, sin leer ninguna clave de
    /// la operación: la línea de contexto ya dice qué comando corre.
    #[test]
    fn started_maps_to_the_started_stage_for_every_command() {
        let event = json!({ "event": "started", "operation": "otra cosa" });
        assert_eq!(
            stream_event_phase(&event),
            Some((&stage::STARTED, stage::STARTED.active))
        );
    }

    /// El daemon es un detalle de ruta: ningún texto del catálogo lleva un
    /// sufijo de ruta y todos terminan en «…» o en el aviso de duración.
    #[test]
    fn no_phase_text_mentions_the_route() {
        let all = [
            &stage::VALIDATING,
            &stage::STARTED,
            &stage::DENOISE_QUEUE,
            &stage::SAVING,
            &stage::CHECKING,
            &stage::READING,
            &stage::CAPTURING,
            &stage::DENOISING,
            &stage::TRANSCRIBING,
            &stage::TRANSLATING,
            &stage::SYNTHESIZING,
            &stage::PLAYING,
            &stage::CLONING,
            &stage::WARMING,
            &stage::REMOVING,
            &stage::STOPPING,
            &stage::STARTING,
            &stage::DOWNLOADING,
            &stage::VERIFYING,
            &stage::REPLACING,
            &stage::COPYING,
            &stage::PATH_ADD,
            &stage::PATH_REVERT,
            &stage::RECEIPT,
            &stage::STATE,
            &stage::PROGRAM,
            &stage::LOCKS,
            &stage::CLEANING,
            &stage::REPAIRING,
        ];
        for stage in all {
            assert!(
                !stage.active.contains("(daemon)"),
                "texto con sufijo de ruta: {}",
                stage.active
            );
            assert!(
                stage.active.ends_with('…') || stage.active.ends_with("minutos)"),
                "texto sin cierre «…»: {}",
                stage.active
            );
        }
        for text in [
            stage::SYNTHESIS_QUEUED_TEXT,
            stage::SYNTHESIS_WARMING_TEXT,
            stage::SYNTHESIS_LOADING_VOICE_TEXT,
        ] {
            assert!(
                !text.contains("(daemon)"),
                "texto con sufijo de ruta: {text}"
            );
            assert!(text.ends_with('…'), "texto sin cierre «…»: {text}");
        }
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
        let progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &progress, &mut RejectAudio)
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
        let progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &progress, &mut RejectAudio)
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
        let progress = PhaseProgress::new();
        let e = consume_ndjson_stream(resp, StreamResult::Clone, &progress, &mut RejectAudio)
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

    /// Prepara almacenes de voces y de locuciones aislados y un WAV temporal
    /// válido, y los devuelve con el directorio temporal que los respalda.
    fn menu_fixture() -> (tempfile::TempDir, VoiceStore, SpeechStore, PathBuf) {
        let dir = tempfile::Builder::new()
            .prefix(avi_shared::paths::MENU_SANDBOX_PREFIX)
            .tempdir()
            .expect("directorio temporal de prueba");
        let voices = VoiceStore::at(dir.path().join("voices"));
        let store = SpeechStore::at(dir.path().join("speech"));
        let wav = dir.path().join("toma.wav");
        write_test_wav(&wav);
        (dir, voices, store, wav)
    }

    /// La opción 1 reproduce de nuevo el WAV y vuelve a ofrecer el menú; la 2
    /// guarda la locución y devuelve la ruta persistida.
    #[test]
    fn play_menu_option_one_replays_and_option_two_saves() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["1", "2"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        // Solo la de la opción 1: la toma inicial ya sonó mientras se sintetizaba.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 1);
        match decision {
            PlayDecision::Saved(avi_store::Persisted::Saved(saved)) => {
                // La ruta guardada es la del almacén, no la del temporal.
                assert_eq!(
                    saved,
                    store.audio_path(
                        &VoiceName::parse("voz").unwrap(),
                        &Label::parse("etiqueta").unwrap()
                    )
                );
                assert!(
                    saved.is_file(),
                    "el audio sintetizado guardado debe existir"
                );
            }
            other => panic!("se esperaba guardar, no: {other:?}"),
        }
        // El texto de la locución guardada es el texto final, no el de la WAV.
        let entry = store
            .find(
                &VoiceName::parse("voz").unwrap(),
                &Label::parse("etiqueta").unwrap(),
            )
            .expect("buscar el audio sintetizado guardado")
            .expect("el audio sintetizado debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// La opción 4 descarta: no persiste nada y devuelve `Discarded`.
    #[test]
    fn play_menu_option_four_discards_without_saving() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["4"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Discarded);
        // El menú no reproduce al entrar: la toma ya sonó mientras se sintetizaba.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            store
                .find(
                    &VoiceName::parse("voz").unwrap(),
                    &Label::parse("etiqueta").unwrap()
                )
                .expect("buscar el audio sintetizado")
                .is_none(),
            "descartar no debe guardar el audio sintetizado"
        );
    }

    /// La opción 3 no se resuelve en el menú: se devuelve `Resynthesize` para
    /// que la ejecute el llamador.
    #[test]
    fn play_menu_option_three_returns_resynthesize_to_caller() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Resynthesize);
        // El menú no reproduce nada al entrar: la re-síntesis, con su propia
        // reproducción en streaming, es del llamador.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(
            store
                .find(
                    &VoiceName::parse("voz").unwrap(),
                    &Label::parse("etiqueta").unwrap()
                )
                .expect("buscar el audio sintetizado")
                .is_none(),
            "re-sintetizar no debe persistir nada"
        );
    }

    /// El fin de la entrada estándar descarta: el menú se ofrece, la lectura
    /// devuelve EOF y la locución no se persiste.
    #[test]
    fn play_menu_end_of_input_discards() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec![]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert_eq!(decision, PlayDecision::Discarded);
        assert!(
            store
                .find(
                    &VoiceName::parse("voz").unwrap(),
                    &Label::parse("etiqueta").unwrap()
                )
                .expect("buscar el audio sintetizado")
                .is_none(),
            "el fin de la entrada no debe guardar el audio sintetizado"
        );
    }

    /// Una opción inválida avisa y repite el menú; la siguiente opción válida
    /// se atiende con normalidad.
    #[test]
    fn play_menu_invalid_option_repeats_menu() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["9", "2"]);
        let mut playback = fake_playback(&plays);

        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("el menú no debe fallar");

        assert!(matches!(decision, PlayDecision::Saved(_)));
        // Ninguna reproducción: la opción inválida no reproduce.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 0);
    }

    /// La colisión de etiqueta se recomprueba al guardar (opción 2) y falla con
    /// `label_exists` (exit 6) si la locución ya existe y no hay `--force`.
    #[test]
    fn play_menu_option_two_detects_label_collision() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut playback = fake_playback(&plays);
        // La locución ya existe en el almacén cuando se ofrece el menú.
        avi_store::persist_locution(
            &voices,
            &store,
            None,
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            "texto previo",
            &wav,
            avi_store::CollisionPolicy::Fail,
        )
        .expect("guardar el audio sintetizado previo");

        let mut reader = scripted_reader(vec!["2"]);
        let e = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect_err("la colisión debe fallar");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "label_exists");
    }

    /// Si la voz ya no tiene la identidad con la que se sintetizó la toma, la
    /// opción 2 falla con `voice_changed` (exit 6) y no guarda nada.
    #[test]
    fn play_menu_option_two_rejects_changed_voice() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut playback = fake_playback(&plays);
        let stale = avi_store::VoiceReference {
            path: voices
                .voice_dir(&VoiceName::parse("voz").unwrap())
                .join("reference.qvoice"),
            fingerprint: "contenido-anterior".to_string(),
        };

        let mut reader = scripted_reader(vec!["2"]);
        let e = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Overwrite,
            &voices,
            &store,
            Some(&stale),
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect_err("una voz cambiada no debe guardarse");
        assert_eq!(e.code, ExitCode::StateConflict);
        assert_eq!(e.reason, "voice_changed");
        assert!(store
            .find(
                &VoiceName::parse("voz").unwrap(),
                &Label::parse("etiqueta").unwrap()
            )
            .unwrap()
            .is_none());
    }

    /// Con `--force` la colisión no impide guardar: la locución previa se
    /// sustituye por la nueva.
    #[test]
    fn play_menu_option_two_saves_over_existing_label_with_force() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut playback = fake_playback(&plays);
        avi_store::persist_locution(
            &voices,
            &store,
            None,
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            "texto previo",
            &wav,
            avi_store::CollisionPolicy::Fail,
        )
        .expect("guardar el audio sintetizado previo");

        let mut reader = scripted_reader(vec!["2"]);
        let decision = synthesize_play_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Overwrite,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
        )
        .expect("con --force no debe fallar");

        assert!(matches!(decision, PlayDecision::Saved(_)));
        let entry = store
            .find(
                &VoiceName::parse("voz").unwrap(),
                &Label::parse("etiqueta").unwrap(),
            )
            .expect("buscar el audio sintetizado guardado")
            .expect("el audio sintetizado debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// Re-síntesis de mentira: cuenta las re-síntesis, registra los seeds
    /// recibidos y no toca el motor.
    struct CountingResynthesizer {
        counter: std::sync::Arc<std::sync::atomic::AtomicUsize>,
        seeds: Vec<u64>,
    }

    impl CountingResynthesizer {
        fn new(counter: std::sync::Arc<std::sync::atomic::AtomicUsize>) -> Self {
            Self {
                counter,
                seeds: Vec::new(),
            }
        }
    }

    impl PlayResynthesizer for CountingResynthesizer {
        async fn resynthesize(&mut self, seed: u64) -> Result<(), CliError> {
            self.counter
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            self.seeds.push(seed);
            Ok(())
        }
    }

    /// Varias regeneraciones consecutivas del bucle entregan seeds en rango,
    /// distintos entre sí y distintos del seed de producción.
    #[tokio::test]
    async fn play_menu_loop_hands_distinct_seeds_to_each_retake() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3", "3", "3", "3", "4"]);
        let mut playback = fake_playback(&plays);
        let mut resynthesizer =
            CountingResynthesizer::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)));

        play_menu_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await
        .expect("el bucle no debe fallar");

        let seeds = &resynthesizer.seeds;
        assert_eq!(seeds.len(), 4);
        for (i, seed) in seeds.iter().enumerate() {
            assert!(*seed <= avi_tts::MAX_ENGINE_SEED);
            assert_ne!(*seed, avi_tts::PRODUCTION_SEED);
            assert!(!seeds[..i].contains(seed), "seed repetido: {seed}");
        }
    }

    /// `RetakeSeeds::next` no repite ni devuelve el seed de producción en muchas
    /// iteraciones y se mantiene dentro del rango del motor.
    #[test]
    fn retake_seeds_never_repeat_nor_use_production_seed() {
        let mut seeds = RetakeSeeds::new();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..2000 {
            let seed = seeds.next();
            assert!(seed <= avi_tts::MAX_ENGINE_SEED);
            assert_ne!(seed, avi_tts::PRODUCTION_SEED);
            assert!(seen.insert(seed), "seed repetido: {seed}");
        }
    }

    /// El bucle del llamador: ante `Resynthesize` re-sintetiza y vuelve a
    /// ofrecer el menú; la locución final se persiste con la etiqueta correcta.
    #[tokio::test]
    async fn play_menu_loop_resynthesizes_and_offers_menu_again() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let resyntheses = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3", "2"]);
        let mut playback = fake_playback(&plays);
        let mut resynthesizer = CountingResynthesizer::new(resyntheses.clone());

        let outcome = play_menu_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await
        .expect("el bucle no debe fallar");

        assert_eq!(resyntheses.load(std::sync::atomic::Ordering::SeqCst), 1);
        // El menú no reproduce nada: cada toma ya sonó mientras se sintetizaba.
        assert_eq!(plays.load(std::sync::atomic::Ordering::SeqCst), 0);
        let SynthesizeOutcome::Saved(saved) = outcome else {
            panic!("el audio sintetizado debe quedar guardado: {outcome:?}");
        };
        assert_eq!(
            saved,
            store.audio_path(
                &VoiceName::parse("voz").unwrap(),
                &Label::parse("etiqueta").unwrap()
            )
        );
        let entry = store
            .find(
                &VoiceName::parse("voz").unwrap(),
                &Label::parse("etiqueta").unwrap(),
            )
            .expect("buscar el audio sintetizado guardado")
            .expect("el audio sintetizado debe existir");
        assert_eq!(entry.metadata.expect("sidecar").text, "texto final");
    }

    /// El bucle del llamador termina en descarte cuando el usuario elige la 4
    /// tras re-sintetizar.
    #[tokio::test]
    async fn play_menu_loop_ends_on_discard_after_resynthesis() {
        let (_dir, voices, store, wav) = menu_fixture();
        let plays = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut reader = scripted_reader(vec!["3", "4"]);
        let mut playback = fake_playback(&plays);
        let mut resynthesizer =
            CountingResynthesizer::new(std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)));

        let outcome = play_menu_loop(
            &VoiceName::parse("voz").unwrap(),
            &Label::parse("etiqueta").unwrap(),
            avi_store::CollisionPolicy::Fail,
            &voices,
            &store,
            None,
            "texto final",
            &wav,
            &mut reader,
            &mut playback,
            &mut resynthesizer,
        )
        .await
        .expect("el bucle no debe fallar");

        assert_eq!(
            outcome,
            SynthesizeOutcome::Discarded,
            "descartar no debe guardar nada"
        );
        assert!(
            store
                .find(
                    &VoiceName::parse("voz").unwrap(),
                    &Label::parse("etiqueta").unwrap()
                )
                .expect("buscar el audio sintetizado")
                .is_none(),
            "descartar no debe guardar el audio sintetizado"
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

    /// Mientras el daemon no envía nada, la espera del siguiente fragmento
    /// repinta el indicador al menos cada 250 ms y se rinde al agotar el plazo.
    #[tokio::test]
    async fn next_chunk_repainting_repaints_while_stream_is_silent() {
        let addr = serve_programmed_sequence(vec![], Some(3)).await;
        let mut resp = daemon_client()
            .post(format!("http://{}/synthesize", addr))
            .send()
            .await
            .expect("el doble debe responder cabeceras");
        let progress = CountingProgress::default();
        let mut observed = 0usize;
        let chunk = next_chunk_repainting(
            &mut resp,
            std::time::Duration::from_millis(1100),
            &progress,
            &mut || observed += 1,
        )
        .await;
        assert!(chunk.is_none(), "sin fragmentos debe agotar el plazo");
        assert!(
            progress.repaints() >= 3,
            "debe repintar cada 250 ms: {}",
            progress.repaints()
        );
        assert_eq!(
            observed,
            progress.repaints(),
            "cada repintado también refleja la reproducción"
        );
    }

    /// Un futuro que tarda un rato en terminar.
    ///
    /// Duerme un tiempo concreto, así que sigue pendiente durante ese tiempo
    /// y el ayudante agota su espera para repintar. Las pruebas que lo usan
    /// corren con el reloj de tokio detenido (`start_paused`): el tiempo
    /// avanza solo cuando el ejecutor queda ocioso, así que ni la carga de la
    /// máquina ni una pausa del hilo alteran el orden de los plazos.
    async fn slow_future(millis: u64) -> u32 {
        tokio::time::sleep(std::time::Duration::from_millis(millis)).await;
        0
    }

    /// El intervalo con el que el indicador se repinta durante una espera.
    const TEST_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_millis(5);

    /// Espera a un futuro reponiendo el indicador mientras no termina: el
    /// futuro inmediato no llega a consumirse, así que no hay repintado.
    #[tokio::test(start_paused = true)]
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
    /// Con el reloj detenido el recuento es exacto: el futuro dura 50 ms y el
    /// intervalo es de 5 ms, así que el repintado ocurre a los 5, 10, …, 45
    /// ms (nueve veces). A los 50 ms vencen a la vez el futuro y el plazo del
    /// intervalo, y gana el futuro porque `timeout` lo evalúa primero.
    #[tokio::test(start_paused = true)]
    async fn await_with_progress_repaints_while_the_future_is_pending() {
        let progress = CountingProgress::default();
        let value =
            await_with_progress_every(&progress, TEST_REPAINT_INTERVAL, slow_future(50)).await;
        assert_eq!(value, 0);
        assert_eq!(
            progress.repaints(),
            9,
            "un futuro lento se repinta cada intervalo mientras espera"
        );
    }

    /// Sin terminal y sin `--yes`, la confirmación de `self update` sale como
    /// `confirmation_required` con código 2, no como `lifecycle_failed`/1.
    #[test]
    fn update_confirmation_without_terminal_is_confirmation_required() {
        let error: anyhow::Error =
            lifecycle::LifecycleError::confirmation_required("falta confirmación").into();
        let cli = lifecycle_error_to_cli(error);
        assert_eq!(cli.reason, "confirmation_required");
        assert_eq!(cli.code.code(), 2);
    }

    /// El indicador real no repinta si no hay etapa abierta: `tick` solo
    /// consulta el estado, y sin etapa no hay línea que repintar.
    ///
    /// Es la regla que hace que el repintado no invente actividad: la etapa
    /// tiene que existir para que el tiempo visible avance. El indicador de
    /// producción no expone su salida al binario, así que aquí solo se fija
    /// que acepta la llamada sin etapa y no escribe nada.
    #[test]
    fn tick_without_a_phase_does_not_repaint() {
        let progress = PhaseProgress::new();
        // Sin fase, el indicador no tiene línea que devolver: `tick` es un
        // no-op y el tiempo visible no avanza por su cuenta.
        progress.tick();
    }

    /// La espera asíncrona sí pide el repintado: es el mecanismo que hace que
    /// el tiempo visible avance mientras un futuro sigue pendiente.
    #[tokio::test(start_paused = true)]
    async fn asynchronous_work_asks_for_the_repaint() {
        let progress = CountingProgress::default();
        await_with_progress_every(&progress, TEST_REPAINT_INTERVAL, slow_future(50)).await;
        assert!(
            progress.repaints() >= 1,
            "la espera asíncrona sí pide repintar: {}",
            progress.repaints()
        );
    }

    /// La línea viva de la grabación no existe hasta que empieza, y después
    /// muestra el tiempo, el tope, el medidor y, solo con Enter, la indicación.
    #[test]
    fn mic_observer_live_line_follows_the_capture_state() {
        use avi_audio::CaptureObserver;
        let observer = MicObserver::default();
        assert_eq!(observer.live_line(), None, "sin empezar no hay línea");
        observer.started(std::time::Duration::from_secs(40), true);
        for level in [-90.0, -50.0, -20.0] {
            observer.progress(std::time::Duration::from_secs(5), level);
        }
        let line = observer.live_line().expect("la grabación empezó");
        assert!(line.contains("Grabando 0:05 / 0:40  ▁▃█"), "{line}");
        assert!(line.ends_with("  Enter para detener"), "{line}");
        assert!(!observer.limit_reached());
        CaptureObserver::limit_reached(&observer);
        assert!(observer.limit_reached());
        let timed = MicObserver::default();
        timed.started(std::time::Duration::from_secs(10), false);
        let line = timed.live_line().expect("la grabación empezó");
        assert!(!line.contains("Enter"), "{line}");
    }

    /// El medidor conserva solo los últimos niveles.
    #[test]
    fn mic_observer_keeps_only_the_last_levels() {
        use avi_audio::CaptureObserver;
        let observer = MicObserver::default();
        observer.started(std::time::Duration::from_secs(10), false);
        for level in [-90.0, -90.0, -90.0, -90.0, -90.0, -20.0] {
            observer.progress(std::time::Duration::ZERO, level);
        }
        let line = observer.live_line().expect("la grabación empezó");
        assert!(line.ends_with("▁▁▁▁█"), "{line}");
    }

    /// El tope mostrado es `--duration` o el menor entre el techo de entorno
    /// y el límite del comando.
    #[test]
    fn mic_max_secs_prefers_the_duration() {
        assert_eq!(mic_max_secs(Some(7), 40).unwrap(), 7);
    }

    /// Las fuentes de audio de la línea de contexto.
    #[test]
    fn audio_source_label_names_the_file_or_the_microphone() {
        // La ruta se construye con el separador del sistema: una ruta con
        // barras invertidas solo se descompone en Windows.
        let path = std::path::Path::new("datos").join("voz.wav");
        assert_eq!(audio_source_label(path.to_str(), None), "archivo voz.wav");
        assert_eq!(
            audio_source_label(None, Some(40)),
            "micrófono (máximo 0:40)"
        );
    }

    /// Un PCM vacío se rechaza como `empty_audio`.
    #[test]
    fn empty_pcm_is_rejected_as_empty_audio() {
        let error = validate_acquired_pcm(&[], 16_000, 10, "El audio")
            .expect_err("un PCM vacío debe rechazarse");
        assert_eq!(error.reason, "empty_audio");
    }

    /// Un PCM que supera el límite falla con la razón de límite existente.
    #[test]
    fn pcm_over_the_limit_is_rejected_as_audio_too_long() {
        let pcm = vec![1i16; 2 * 16_000 + 1];
        let error = validate_acquired_pcm(&pcm, 16_000, 2, "El audio")
            .expect_err("un PCM demasiado largo debe rechazarse");
        assert_eq!(error.reason, "audio_too_long");
        validate_acquired_pcm(&pcm[..2 * 16_000], 16_000, 2, "El audio")
            .expect("el límite exacto es válido");
    }

    /// Escribe `pcm` en un WAV temporal y devuelve su guarda.
    fn temp_wav_with(pcm: &[i16]) -> avi_shared::temp_wav::TempWav {
        let guard = avi_process::scratch::temp_wav("avi_acquire_test_").expect("WAV temporal");
        let wav = avi_audio::encode_pcm_mono_wav(pcm, 16_000).expect("codificar WAV");
        std::fs::write(guard.path(), wav).expect("escribir WAV");
        guard
    }

    /// El ayudante completo lee un WAV, pinta la fase de lectura y devuelve
    /// el PCM sin necesidad de micrófono.
    #[tokio::test]
    async fn acquire_reads_a_wav_without_a_microphone() {
        let pcm: Vec<i16> = (0..1_600).map(|i| (i % 100) as i16 + 1).collect();
        let guard = temp_wav_with(&pcm);
        let path = guard.path().to_string_lossy().into_owned();
        let progress = PhaseProgress::new();
        let acquired =
            acquire_pcm_with_progress(&progress, Some(&path), false, None, 10, "El audio")
                .await
                .expect("lectura válida");
        assert_eq!(acquired.len(), pcm.len());
    }

    /// El ayudante completo rechaza un WAV vacío y uno que supera el límite.
    #[tokio::test]
    async fn acquire_rejects_empty_and_too_long_wavs() {
        let progress = PhaseProgress::new();
        let empty = temp_wav_with(&[]);
        let empty_path = empty.path().to_string_lossy().into_owned();
        let error =
            acquire_pcm_with_progress(&progress, Some(&empty_path), false, None, 10, "El audio")
                .await
                .expect_err("un WAV vacío debe rechazarse");
        assert_eq!(error.reason, "empty_audio");

        let long = temp_wav_with(&vec![1i16; 2 * 16_000 + 1]);
        let long_path = long.path().to_string_lossy().into_owned();
        let error =
            acquire_pcm_with_progress(&progress, Some(&long_path), false, None, 2, "El audio")
                .await
                .expect_err("un WAV demasiado largo debe rechazarse");
        assert_eq!(error.reason, "audio_too_long");
    }
}
