#![cfg_attr(not(test), deny(clippy::let_underscore_must_use))]

use anyhow::{anyhow, Context, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, Sample, SampleFormat, SizedSample, StreamConfig};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Nombre de la variable de entorno que fija el techo de la grabación push-to-talk.
const PUSH_TO_TALK_MAX_SECS_VAR: &str = "AVI_PUSH_TO_TALK_MAX_SECS";

/// Techo de seguridad, en segundos, de la grabación push-to-talk: el valor de
/// `AVI_PUSH_TO_TALK_MAX_SECS`, o 300 si la variable no está definida. Una
/// variable definida que no sea un entero sin signo válido es un error.
pub fn push_to_talk_max_secs() -> Result<u64> {
    match std::env::var(PUSH_TO_TALK_MAX_SECS_VAR) {
        Ok(value) => parse_push_to_talk_max_secs(&value),
        Err(std::env::VarError::NotPresent) => Ok(300),
        Err(std::env::VarError::NotUnicode(_)) => Err(anyhow!(
            "{PUSH_TO_TALK_MAX_SECS_VAR} no es texto Unicode; debe ser un entero de segundos"
        )),
    }
}

/// Interpreta el valor de `AVI_PUSH_TO_TALK_MAX_SECS` como entero de segundos.
fn parse_push_to_talk_max_secs(value: &str) -> Result<u64> {
    value.trim().parse().map_err(|_| {
        anyhow!(
            "{PUSH_TO_TALK_MAX_SECS_VAR}='{value}' no es válida; debe ser un entero de segundos \
             sin signo"
        )
    })
}

/// Duración, en segundos, de un WAV leída de su cabecera sin decodificar las
/// muestras; `None` si el archivo no existe o no es un WAV legible.
pub fn wav_duration_secs(path: &Path) -> Option<f64> {
    let reader = hound::WavReader::open(path).ok()?;
    wav_reader_duration_secs(&reader)
}

/// Igual que [`wav_duration_secs`] pero sobre un WAV ya en memoria; `None` si los
/// bytes no son un WAV legible.
pub fn wav_bytes_duration_secs(bytes: &[u8]) -> Option<f64> {
    let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).ok()?;
    wav_reader_duration_secs(&reader)
}

/// Duración de la cabecera: tramas (muestras por canal) entre la frecuencia.
fn wav_reader_duration_secs<R: std::io::Read>(reader: &hound::WavReader<R>) -> Option<f64> {
    let rate = reader.spec().sample_rate;
    (rate > 0).then(|| reader.duration() as f64 / rate as f64)
}

/// Representación pública de un dispositivo de audio. `name` y `latency_ms` son
/// `None` cuando el backend no los expone: no se inventa ningún valor.
pub struct AudioDevice {
    pub id: usize,
    pub name: Option<String>,
    pub latency_ms: Option<f32>,
}

/// Latencia estimada, en milisegundos, de un buffer de salida: tamaño mínimo del
/// buffer entre la frecuencia de muestreo. `None` si el backend no informa el
/// tamaño del buffer o la frecuencia es 0.
fn estimate_latency_ms(buffer_size: &cpal::SupportedBufferSize, sample_rate: u32) -> Option<f32> {
    match buffer_size {
        cpal::SupportedBufferSize::Range { min, .. } if sample_rate > 0 => {
            Some(*min as f32 / sample_rate as f32 * 1000.0)
        }
        _ => None,
    }
}

/// Crea el callback de error de un stream: lo escribe por stderr y marca el
/// fallo en `failed` para que la espera de la operación termine con error.
fn stream_error_callback(
    label: &'static str,
    failed: &Arc<AtomicBool>,
) -> impl FnMut(cpal::Error) + Send + 'static {
    let failed = Arc::clone(failed);
    move |err| {
        eprintln!("{label}: {err}");
        failed.store(true, Ordering::SeqCst);
    }
}

/// Devuelve error si el callback de error del stream marcó un fallo.
fn ensure_stream_alive(failed: &AtomicBool, operation: &str) -> Result<()> {
    if failed.load(Ordering::SeqCst) {
        return Err(anyhow!("El stream de audio falló durante {operation}"));
    }
    Ok(())
}

/// Despacha `$body` con `$t` fijado al tipo Rust del `SampleFormat` dado. Es el
/// único sitio que enumera los formatos de muestra: reproducción y captura lo
/// comparten en lugar de repetir una rama por formato. Los formatos que cpal no
/// sabe convertir a `f32` (los DSD de 1 bit) terminan en error.
macro_rules! with_sample_type {
    ($format:expr, $t:ident => $body:expr) => {
        match $format {
            SampleFormat::I8 => {
                type $t = i8;
                $body
            }
            SampleFormat::I16 => {
                type $t = i16;
                $body
            }
            SampleFormat::I24 => {
                type $t = cpal::I24;
                $body
            }
            SampleFormat::I32 => {
                type $t = i32;
                $body
            }
            SampleFormat::I64 => {
                type $t = i64;
                $body
            }
            SampleFormat::U8 => {
                type $t = u8;
                $body
            }
            SampleFormat::U16 => {
                type $t = u16;
                $body
            }
            SampleFormat::U24 => {
                type $t = cpal::U24;
                $body
            }
            SampleFormat::U32 => {
                type $t = u32;
                $body
            }
            SampleFormat::U64 => {
                type $t = u64;
                $body
            }
            SampleFormat::F32 => {
                type $t = f32;
                $body
            }
            SampleFormat::F64 => {
                type $t = f64;
                $body
            }
            other => Err(anyhow!(
                "Formato de muestra no soportado por el dispositivo: {other:?}"
            )),
        }
    };
}

/// Rellena `data` con las muestras `f32` pendientes convertidas al tipo `T` del
/// dispositivo y avanza `idx`; lo que sobra se rellena con el silencio de `T`
/// (`EQUILIBRIUM`, que no es 0 en los formatos sin signo).
fn fill_converted<T: SizedSample + FromSample<f32>>(
    data: &mut [T],
    samples: &[f32],
    idx: &mut usize,
) {
    for out in data.iter_mut() {
        if *idx < samples.len() {
            *out = T::from_sample(samples[*idx].clamp(-1.0, 1.0));
            *idx += 1;
        } else {
            *out = T::EQUILIBRIUM;
        }
    }
}

/// Construye el stream de salida para el formato de muestra `T` del dispositivo,
/// convirtiendo cada `f32` normalizado con `T::from_sample`.
fn build_playback_stream<T: SizedSample + FromSample<f32> + 'static>(
    device: &cpal::Device,
    config: StreamConfig,
    samples: Vec<f32>,
    sample_idx: Arc<Mutex<usize>>,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream> {
    Ok(device.build_output_stream(
        config,
        move |data: &mut [T], _| {
            let mut idx = acquire_lock(&sample_idx, "índice de reproducción");
            fill_converted(data, &samples, &mut idx);
        },
        stream_error_callback("Error en stream de audio", failed),
        None,
    )?)
}

/// Construye el stream de entrada para el formato de muestra `T` del
/// dispositivo, acumulando las muestras normalizadas a `f32`.
fn build_capture_stream<T: SizedSample + 'static>(
    device: &cpal::Device,
    config: StreamConfig,
    recorded: Arc<Mutex<Vec<f32>>>,
    failed: &Arc<AtomicBool>,
) -> Result<cpal::Stream>
where
    f32: FromSample<T>,
{
    Ok(device.build_input_stream(
        config,
        move |data: &[T], _| {
            let mut buffer = acquire_lock(&recorded, "buffer de grabación");
            buffer.extend(data.iter().map(|&s| f32::from_sample(s)));
        },
        stream_error_callback("Error en captura de micrófono", failed),
        None,
    )?)
}

/// Intervalo con el que las esperas comprueban si el stream falló.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Espera `duration` comprobando cada [`POLL_INTERVAL`] que el stream siga vivo.
fn wait_checking_stream(duration: Duration, failed: &AtomicBool, operation: &str) -> Result<()> {
    let deadline = Instant::now() + duration;
    loop {
        ensure_stream_alive(failed, operation)?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(());
        }
        std::thread::sleep(remaining.min(POLL_INTERVAL));
    }
}

/// Causa por la que terminó la espera de la grabación push-to-talk.
#[derive(Debug, PartialEq, Eq)]
enum StopReason {
    /// El usuario pulsó Enter.
    Enter,
    /// Venció el techo de duración.
    Limit,
}

/// Espera el fin de una grabación push-to-talk: Enter (mensaje `Ok` del canal),
/// el vencimiento de `max` o un fallo. Es un error que falle la lectura de stdin,
/// que el hilo lector desaparezca sin avisar o que el stream marque un fallo.
fn wait_for_stop(
    rx: &std::sync::mpsc::Receiver<std::io::Result<()>>,
    max: Duration,
    failed: &AtomicBool,
) -> Result<StopReason> {
    use std::sync::mpsc::RecvTimeoutError;
    let deadline = Instant::now() + max;
    loop {
        ensure_stream_alive(failed, "la grabación")?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Ok(StopReason::Limit);
        }
        match rx.recv_timeout(remaining.min(POLL_INTERVAL)) {
            Ok(Ok(())) => return Ok(StopReason::Enter),
            Ok(Err(err)) => {
                return Err(anyhow!(
                    "No se pudo leer stdin para detener la grabación: {err}"
                ))
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(anyhow!(
                    "El lector de stdin terminó sin señalar la parada de la grabación"
                ))
            }
        }
    }
}

/// Toma el mutex de audio recuperando el veneno en lugar de propagarlo.
///
/// Un hilo que muere con el mutex tomado lo deja envenenado, y el siguiente
/// `lock()` devuelve `Err` con el guard dentro. Un `unwrap()` convertiría ese
/// estado recuperable en un pánico que tumba el proceso desde el hilo de
/// audio, así que el veneno deja traza y la operación continúa sobre el estado
/// recuperado (`PoisonError::into_inner`): lo que envenenó el mutex fue el
/// pánico del hilo anterior, no el dato que guarda.
fn acquire_lock<'a, T>(mutex: &'a Mutex<T>, label: &str) -> std::sync::MutexGuard<'a, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        eprintln!("Aviso: mutex «{label}» envenenado; se sigue sobre el estado recuperado");
        poisoned.into_inner()
    })
}

/// El sistema no tiene la librería del backend de audio, o la tiene sin alguna
/// de las funciones que se usan. En Linux, `cpal` usa ALSA, que se carga en
/// tiempo de ejecución: sin ella el resto de la app funciona y solo fallan las
/// operaciones de audio, con este error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioBackendUnavailable {
    /// Librería del sistema que falta o está incompleta.
    pub library: &'static str,
    /// Función que la librería cargada no define; `None` si la librería falta.
    pub missing: Option<String>,
}

impl std::fmt::Display for AudioBackendUnavailable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.missing {
            Some(symbol) => write!(
                f,
                "No hay audio en este sistema: la librería {} (ALSA) no define {symbol}. \
                 Actualiza el paquete de runtime de ALSA de tu distribución para reproducir \
                 o grabar audio",
                self.library
            ),
            None => write!(
                f,
                "No hay audio en este sistema: falta la librería {} (ALSA). Instala el paquete \
                 de runtime de ALSA de tu distribución para reproducir o grabar audio",
                self.library
            ),
        }
    }
}

impl std::error::Error for AudioBackendUnavailable {}

/// Host de audio por defecto. En Linux, comprobar antes que ALSA se puede
/// cargar y define todas las funciones que se usan evita que `cpal` aborte al
/// inicializarla o en una llamada posterior.
#[cfg(target_os = "linux")]
fn default_host() -> std::result::Result<cpal::Host, AudioBackendUnavailable> {
    if let Err(reason) = alsa_sys::availability() {
        return Err(AudioBackendUnavailable {
            library: "libasound.so.2",
            missing: match reason {
                alsa_sys::Unavailable::Library => None,
                alsa_sys::Unavailable::Symbol(name) => Some(name.to_string_lossy().into_owned()),
            },
        });
    }
    Ok(cpal::default_host())
}

/// Host de audio por defecto: fuera de Linux el backend es parte del sistema.
#[cfg(not(target_os = "linux"))]
fn default_host() -> std::result::Result<cpal::Host, AudioBackendUnavailable> {
    Ok(cpal::default_host())
}

/// Servicio de Audio sobre CPAL (Playback, Capture, Enumeración). Sin backend de
/// audio en el sistema se construye igual y cada operación falla con
/// [`AudioBackendUnavailable`].
pub struct AudioService {
    host: std::result::Result<cpal::Host, AudioBackendUnavailable>,
}

impl Default for AudioService {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioService {
    pub fn new() -> Self {
        Self {
            host: default_host(),
        }
    }

    /// Host de audio, o el error tipado si el sistema no tiene backend.
    fn host(&self) -> Result<&cpal::Host> {
        self.host.as_ref().map_err(|e| e.clone().into())
    }

    /// Listar dispositivos de salida de audio con estimación de latencia. Un fallo
    /// del host al enumerar es un error; un nombre o una latencia que el backend
    /// no expone quedan como `None`. Con `cpal` 0.18 enumerar salidas ya no abre
    /// la entrada de audio (micrófono), que en macOS sin sesión de usuario dejaba
    /// la consulta esperando para siempre a `coreaudiod`.
    pub fn list_output_devices(&self) -> Result<Vec<AudioDevice>> {
        let output_devices = self
            .host()?
            .output_devices()
            .context("El host de audio no pudo enumerar los dispositivos de salida")?;
        Ok(output_devices
            .enumerate()
            .map(|(idx, dev)| AudioDevice {
                id: idx,
                name: dev.description().ok().map(|d| d.name().to_owned()),
                latency_ms: dev
                    .default_output_config()
                    .ok()
                    .and_then(|c| estimate_latency_ms(c.buffer_size(), c.sample_rate())),
            })
            .collect())
    }

    /// Reproducir un archivo WAV sobre el dispositivo por defecto usando CPAL
    pub fn play_wav<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let reader = hound::WavReader::open(path)?;
        let spec = reader.spec();
        ensure_nonzero_sample_rate(&spec)?;

        let device = self
            .host()?
            .default_output_device()
            .ok_or_else(|| anyhow!("No hay dispositivo de salida predeterminado"))?;

        // El dispositivo puede no soportar la tasa del WAV (p. ej. 24 kHz del
        // TTS); se remuestrea a la tasa nativa del dispositivo por defecto.
        let dev_config = device.default_output_config()?;
        let device_rate = dev_config.sample_rate();

        let config = StreamConfig {
            channels: spec.channels,
            sample_rate: device_rate,
            buffer_size: cpal::BufferSize::Default,
        };

        let mut samples = read_normalized_samples(reader)
            .context("El archivo WAV está truncado o tiene muestras ilegibles")?;

        if spec.sample_rate != device_rate {
            samples = resample_linear(&samples, spec.sample_rate, device_rate);
        }

        let total_samples = samples.len();
        let sample_idx = Arc::new(Mutex::new(0usize));
        let stream_failed = Arc::new(AtomicBool::new(false));

        let stream = with_sample_type!(dev_config.sample_format(), T => {
            build_playback_stream::<T>(
                &device,
                config,
                samples,
                sample_idx.clone(),
                &stream_failed,
            )
        })?;

        stream.play()?;

        // Esperar a que finalice la reproducción de las muestras; un fallo del
        // stream la interrumpe con error.
        while *acquire_lock(&sample_idx, "índice de reproducción") < total_samples {
            ensure_stream_alive(&stream_failed, "la reproducción")?;
            std::thread::sleep(POLL_INTERVAL);
        }

        Ok(())
    }

    /// Capturar audio del micrófono por N segundos remuestreado a 16kHz/mono/int16 (Whisper compatible)
    pub fn capture_16k_mono_pcm(&self, duration_secs: u64) -> Result<Vec<i16>> {
        let device = self
            .host()?
            .default_input_device()
            .ok_or_else(|| anyhow!("No hay dispositivo de entrada predeterminado"))?;

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;

        let recorded_samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let stream_failed = Arc::new(AtomicBool::new(false));

        let stream = with_sample_type!(config.sample_format(), T => {
            build_capture_stream::<T>(
                &device,
                config.config(),
                recorded_samples.clone(),
                &stream_failed,
            )
        })?;

        stream.play()?;
        wait_checking_stream(
            Duration::from_secs(duration_secs),
            &stream_failed,
            "la captura",
        )?;
        drop(stream);

        let raw_samples = acquire_lock(&recorded_samples, "buffer de grabación").clone();
        Ok(finish(raw_samples, channels, sample_rate))
    }

    /// Capturar audio del micrófono en modo push-to-talk: graba hasta que el
    /// usuario pulse Enter, con un techo de seguridad configurable por
    /// `AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s) que detiene la grabación y
    /// devuelve lo capturado hasta ese punto (no es un error). `max_secs` es un
    /// techo adicional que fija quien llama; rige el menor entre `max_secs` y el
    /// del entorno (`u64::MAX` deja solo el del entorno). Comparte
    /// formato de salida (16 kHz, mono, int16) con `capture_16k_mono_pcm`.
    /// Devuelve error si la variable de entorno es inválida, si falla la
    /// lectura de stdin o si el stream de audio notifica un fallo.
    pub fn capture_16k_mono_pcm_until_enter(&self, max_secs: u64) -> Result<Vec<i16>> {
        let max_secs = max_secs.min(push_to_talk_max_secs()?);
        let device = self
            .host()?
            .default_input_device()
            .ok_or_else(|| anyhow!("No hay dispositivo de entrada predeterminado"))?;

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate();
        let channels = config.channels() as usize;

        let recorded_samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let stream_failed = Arc::new(AtomicBool::new(false));

        let stream = with_sample_type!(config.sample_format(), T => {
            build_capture_stream::<T>(
                &device,
                config.config(),
                recorded_samples.clone(),
                &stream_failed,
            )
        })?;

        stream.play()?;
        eprintln!("Grabando… pulsa Enter para detener.");

        // Espera de una línea de stdin con límite temporal: se lee en un hilo
        // aparte y se recibe con `recv_timeout` para poder aplicar el deadline
        // de captura sin bloquear indefinidamente en la lectura de stdin. Un fallo
        // de lectura viaja por el canal y termina la operación con error.
        let (tx, rx) = std::sync::mpsc::channel::<std::io::Result<()>>();
        std::thread::spawn(move || {
            let mut line = String::new();
            let read = std::io::stdin().read_line(&mut line).map(|_| ());
            #[expect(
                clippy::let_underscore_must_use,
                reason = "si el receptor ya terminó (límite o fallo), nadie espera el mensaje"
            )]
            let _ = tx.send(read);
        });

        if wait_for_stop(&rx, Duration::from_secs(max_secs), &stream_failed)? == StopReason::Limit {
            eprintln!(
                "Límite de grabación alcanzado ({} s); deteniendo.",
                max_secs
            );
        }

        drop(stream);

        let raw_samples = acquire_lock(&recorded_samples, "buffer de grabación").clone();
        Ok(finish(raw_samples, channels, sample_rate))
    }
}

/// Post-procesado compartido de captura de micrófono: mono → resample a
/// 16 kHz → int16. Usado por `capture_16k_mono_pcm` y
/// `capture_16k_mono_pcm_until_enter` para no duplicar el pipeline.
fn finish(recorded: Vec<f32>, channels: usize, sample_rate: u32) -> Vec<i16> {
    let mono_samples = to_mono(&recorded, channels);
    let resampled = resample_linear(&mono_samples, sample_rate, 16000);
    f32_to_i16(&resampled)
}

/// Error tipado de la carga de un WAV, para que quien llama distinga la causa
/// sin inspeccionar mensajes: archivo inexistente, contenido que no es un WAV
/// válido o fallo de lectura del sistema.
#[derive(Debug)]
pub enum WavLoadError {
    /// El archivo no existe.
    NotFound,
    /// El contenido no es un WAV válido o su formato no está soportado.
    Invalid(String),
    /// Fallo de E/S distinto de «no existe».
    Io(std::io::Error),
}

impl std::fmt::Display for WavLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WavLoadError::NotFound => write!(f, "El archivo de audio no existe"),
            WavLoadError::Invalid(detail) => {
                write!(f, "El archivo de audio no es un WAV válido: {detail}")
            }
            WavLoadError::Io(err) => write!(f, "No se pudo leer el archivo de audio: {err}"),
        }
    }
}

impl std::error::Error for WavLoadError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            WavLoadError::Io(err) => Some(err),
            _ => None,
        }
    }
}

/// Mensaje con el que `hound` reporta una lectura que se queda sin bytes.
const HOUND_SHORT_READ_MESSAGE: &str = "Failed to read enough bytes.";

impl From<hound::Error> for WavLoadError {
    fn from(err: hound::Error) -> Self {
        match err {
            hound::Error::IoError(io) => match io.kind() {
                std::io::ErrorKind::NotFound => WavLoadError::NotFound,
                // Un archivo truncado o vacío es contenido inválido, no un fallo del sistema.
                std::io::ErrorKind::UnexpectedEof => WavLoadError::Invalid(io.to_string()),
                // `hound` señala el fin de archivo prematuro con `ErrorKind::Other` y
                // este texto fijo, no con `UnexpectedEof`; un WAV truncado es inválido.
                std::io::ErrorKind::Other if io.to_string() == HOUND_SHORT_READ_MESSAGE => {
                    WavLoadError::Invalid(io.to_string())
                }
                _ => WavLoadError::Io(io),
            },
            other => WavLoadError::Invalid(other.to_string()),
        }
    }
}

/// Rechaza como WAV inválido el que declara una frecuencia de muestreo nula:
/// `hound` no la valida y el remuestreo dividiría por cero.
fn ensure_nonzero_sample_rate(spec: &hound::WavSpec) -> Result<(), WavLoadError> {
    if spec.sample_rate == 0 {
        return Err(WavLoadError::Invalid(
            "la frecuencia de muestreo es 0".to_string(),
        ));
    }
    Ok(())
}

/// Cargar un WAV arbitrario (`hound`, cualquier tasa/canales/formato) y normalizarlo
/// a PCM `i16` mono a 16 kHz, mismo formato que exige Parakeet y que ya produce
/// `capture_16k_mono_pcm` para el micrófono. Los fallos se devuelven como
/// `WavLoadError`: `NotFound` si el archivo no existe, `Invalid` si no es un WAV
/// válido o soportado (incluido un WAV truncado, cuyas muestras declaradas no están
/// en el archivo) e `Io` para el resto de fallos de lectura.
pub fn load_wav_16k_mono_pcm(path: impl AsRef<Path>) -> Result<Vec<i16>, WavLoadError> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    ensure_nonzero_sample_rate(&spec)?;

    let samples = read_normalized_samples(reader)?;

    let mono = to_mono(&samples, spec.channels as usize);
    let resampled = resample_linear(&mono, spec.sample_rate, 16_000);
    Ok(f32_to_i16(&resampled))
}

/// Lee todas las muestras de un WAV normalizadas a `f32` en [-1, 1]. Las enteras
/// se dividen entre el fondo de escala de su profundidad de bits, calculado en
/// `i64` para que 32 bits no desborde.
fn read_normalized_samples<R: std::io::Read>(
    reader: hound::WavReader<R>,
) -> Result<Vec<f32>, hound::Error> {
    match reader.spec().sample_format {
        hound::SampleFormat::Int => {
            let full_scale = (1i64 << (reader.spec().bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .map(|s| s.map(|s| s as f32 / full_scale))
                .collect()
        }
        hound::SampleFormat::Float => reader.into_samples::<f32>().collect(),
    }
}

/// Carga un WAV arbitrario y lo normaliza a PCM `i16` mono a 24 kHz, el formato
/// que exige el clonado del motor Qwen3-TTS en `--ref-audio`. Simétrica a
/// `load_wav_16k_mono_pcm` (que sirve a Whisper), pero a 24 kHz. Los fallos se
/// devuelven como `WavLoadError` con la misma clasificación: un WAV truncado o que
/// no es un WAV válido es `Invalid`.
pub fn load_wav_24k_mono_pcm(path: impl AsRef<Path>) -> Result<Vec<i16>, WavLoadError> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    ensure_nonzero_sample_rate(&spec)?;

    let samples = read_normalized_samples(reader)?;

    let mono = to_mono(&samples, spec.channels as usize);
    let resampled = resample_24k_antialias(&mono, spec.sample_rate)?;
    Ok(f32_to_i16(&resampled))
}

/// Resamplea `mono` a 24 kHz con `rubato::FftFixedIn` (FFT polyphase con filtro
/// anti-alias). Se usa en la ruta de clonado porque la interpolación lineal no
/// filtra al decimar (p. ej. 48→24 kHz), contamina el espectro y degrada el
/// x-vector (timbre).
/// No-op cuando ya está a 24 kHz (preserva el paso intacto de una referencia ya
/// normalizada). Procesa en chunks fijos y vacía el remanente con `process_partial`.
/// La salida mide exactamente `round(len * 24000 / from_rate)` muestras: se
/// descarta el retardo propio del filtro (`output_delay`) para que el audio no
/// quede desplazado, y se recorta el relleno de ceros del último bloque.
/// Un fallo del remuestreador (ratio no construible o error de proceso) es un
/// `WavLoadError::Invalid`: no hay degradación a otro método.
fn resample_24k_antialias(mono: &[f32], from_rate: u32) -> Result<Vec<f32>, WavLoadError> {
    use rubato::{FftFixedIn, Resampler};
    if from_rate == 24_000 || mono.is_empty() {
        return Ok(mono.to_vec());
    }
    let fail = |e: &dyn std::fmt::Display| {
        WavLoadError::Invalid(format!(
            "no se pudo remuestrear de {from_rate} Hz a 24000 Hz: {e}"
        ))
    };
    const CHUNK: usize = 1024;
    let mut resampler =
        FftFixedIn::<f32>::new(from_rate as usize, 24_000, CHUNK, 2, 1).map_err(|e| fail(&e))?;
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + CHUNK <= mono.len() {
        let w = resampler
            .process(&[&mono[pos..pos + CHUNK]], None)
            .map_err(|e| fail(&e))?;
        out.extend_from_slice(&w[0]);
        pos += CHUNK;
    }
    // Remanente final + frames retenidos por el filtro. Un remanente vacío se
    // entrega como `None`: el remuestreador rechaza una rebanada vacía.
    let tail = &mono[pos..];
    let w = resampler
        .process_partial((!tail.is_empty()).then_some(&[tail][..]), None)
        .map_err(|e| fail(&e))?;
    out.extend_from_slice(&w[0]);
    // Longitud exacta de la salida y retardo del filtro a compensar.
    let expected =
        ((mono.len() as u128 * 24_000 + from_rate as u128 / 2) / from_rate as u128) as usize;
    let delay = resampler.output_delay();
    // Si el último bloque no alcanzó, se sigue vaciando el filtro con ceros.
    while out.len() < delay + expected {
        let w = resampler
            .process_partial(None::<&[&[f32]]>, None)
            .map_err(|e| fail(&e))?;
        out.extend_from_slice(&w[0]);
    }
    out.drain(..delay);
    out.truncate(expected);
    Ok(out)
}

// ─── Helpers de AudioConverter ───────────────────────────────────────

pub(crate) fn to_mono(samples: &[f32], channels: usize) -> Vec<f32> {
    if channels == 1 {
        return samples.to_vec();
    }
    samples
        .chunks(channels)
        .map(|chunk| chunk.iter().sum::<f32>() / channels as f32)
        .collect()
}

pub(crate) fn resample_linear(samples: &[f32], from_rate: u32, to_rate: u32) -> Vec<f32> {
    if from_rate == to_rate || samples.is_empty() {
        return samples.to_vec();
    }
    let ratio = from_rate as f64 / to_rate as f64;
    let target_len = (samples.len() as f64 / ratio) as usize;
    let mut output = Vec::with_capacity(target_len);

    for i in 0..target_len {
        let src_idx = i as f64 * ratio;
        let idx_floor = src_idx.floor() as usize;
        let idx_ceil = (idx_floor + 1).min(samples.len() - 1);
        let frac = src_idx - idx_floor as f64;

        let sample = samples[idx_floor] * (1.0 - frac as f32) + samples[idx_ceil] * (frac as f32);
        output.push(sample);
    }
    output
}

pub(crate) fn f32_to_i16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&s| {
            let clamped = s.clamp(-1.0, 1.0);
            (clamped * 32767.0) as i16
        })
        .collect()
}

/// Representación JSON de un dispositivo: `name` y `latency` (en segundos) son
/// `null` cuando el backend no los expone.
fn device_to_json(device: &AudioDevice) -> Value {
    json!({
        "id": device.id,
        "name": device.name,
        "latency": device.latency_ms.map(|ms| ms / 1000.0)
    })
}

pub fn get_devices_json() -> Result<Vec<Value>> {
    let service = AudioService::new();
    let devs = service.list_output_devices()?;
    Ok(devs.iter().map(device_to_json).collect())
}

/// Escribe en `path` un WAV PCM 16-bit mono con `sample_rate` 0 y 100 muestras.
/// Solo es para pruebas: `hound` no permite escribir frecuencia 0, así que la
/// cabecera se arma a mano.
#[cfg(any(test, feature = "test-support"))]
pub fn write_zero_rate_wav(path: &Path) {
    let data_len = 200u32;
    let mut wav = Vec::<u8>::with_capacity(44 + data_len as usize);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&0u32.to_le_bytes());
    wav.extend_from_slice(&0u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(wav.len() + data_len as usize, 1);
    std::fs::write(path, wav).expect("escribir el WAV de frecuencia nula");
}

#[cfg(test)]
mod tests {
    /// Reserva un `.wav` temporal con dueño: el fichero se borra al soltar la
    /// ruta devuelta, también si la prueba falla.
    fn temp_wav_path(name: &str) -> tempfile::TempPath {
        tempfile::Builder::new()
            .prefix(&format!("avi_audio_test_{name}_"))
            .suffix(".wav")
            .tempfile()
            .unwrap()
            .into_temp_path()
    }

    /// Un mutex de audio envenenado no tumba el proceso: el recover entrega
    /// el estado que el hilo muerto dejó puesto y la operación sigue sobre él.
    #[test]
    fn test_acquire_lock_recovers_poisoned_mutex() {
        let mutex = std::sync::Arc::new(std::sync::Mutex::new(7usize));
        // Un hilo muere con el mutex tomado: eso es lo que lo envenena.
        let dying = std::sync::Arc::clone(&mutex);
        let _ = std::thread::spawn(move || {
            let _guard = dying.lock().expect("el hilo toma el mutex antes de morir");
            panic!("el hilo muere con el mutex tomado");
        })
        .join();
        assert!(mutex.is_poisoned(), "el mutex debe quedar envenenado");

        // Un `unwrap()` habría tumbado el test aquí; el recover no.
        assert_eq!(*crate::acquire_lock(&mutex, "índice de reproducción"), 7);
        // El estado recuperado sigue siendo utilizable.
        *crate::acquire_lock(&mutex, "índice de reproducción") += 1;
        assert_eq!(*mutex.lock().unwrap_or_else(|p| p.into_inner()), 8);
    }

    #[test]
    fn test_audio_resampling_linear() {
        let input = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let resampled = crate::resample_linear(&input, 44100, 16000);
        // 5 muestras a razón 44100/16000: ⌊5 / 2,75625⌋ = 1.
        assert_eq!(resampled.len(), 1);
        assert_eq!(resampled[0], 0.0);
    }

    #[test]
    fn test_to_mono_downmix_stereo() {
        let input = vec![1.0, 0.0, 0.5, 0.5, 0.0, 1.0];
        let mono = crate::to_mono(&input, 2);
        assert_eq!(mono, vec![0.5, 0.5, 0.5]);

        let direct_mono = vec![0.1, 0.2, 0.3];
        let result = crate::to_mono(&direct_mono, 1);
        assert_eq!(result, direct_mono);
    }

    #[test]
    fn test_f32_to_i16_scale_and_clamp() {
        let input = vec![0.0, 0.5, -0.5, 1.5, -2.0];
        let output = crate::f32_to_i16(&input);
        assert_eq!(output[0], (0.0f32 * 32767.0) as i16);
        assert_eq!(output[1], (0.5f32 * 32767.0) as i16);
        assert_eq!(output[2], (-0.5f32 * 32767.0) as i16);
        assert_eq!(output[3], 32767);
        assert_eq!(output[4], -32767);
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_normalizes_synthetic_wav() {
        // WAV sintético en memoria: 48 kHz estéreo, para verificar que
        // `load_wav_16k_mono_pcm` lo normaliza a 16 kHz mono.
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..300 {
                let sample = (((i % 100) as f32 / 100.0) * 2.0 - 1.0) * 32767.0;
                writer.write_sample(sample as i16).unwrap();
            }
            writer.finalize().unwrap();
        }

        let tmp_path = temp_wav_path("load_wav_16k_mono_pcm");
        std::fs::write(&tmp_path, cursor.into_inner()).unwrap();

        let pcm = crate::load_wav_16k_mono_pcm(&tmp_path).expect("debe cargar y normalizar el WAV");

        // 300 muestras estéreo -> 150 muestras mono @48kHz -> 50 muestras @16kHz.
        assert_eq!(pcm.len(), 50);
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_missing_file_is_not_found() {
        // Ruta inexistente: se reserva y se cierra, de modo que no queda fichero.
        let reserved = temp_wav_path("never_exists");
        let path = reserved.to_path_buf();
        reserved.close().unwrap();
        let err = crate::load_wav_16k_mono_pcm(&path).unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::NotFound),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_garbage_bytes_is_invalid() {
        let path = temp_wav_path("garbage");
        std::fs::write(&path, b"esto no es un archivo wav, solo bytes basura").unwrap();
        let result = crate::load_wav_16k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_24k_mono_pcm_oversamples_reference_16k() {
        // WAV sintético en memoria: 16 kHz mono (el caso real de la referencia de
        // clonado), para verificar que `load_wav_24k_mono_pcm` lo sube a 24 kHz.
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..160 {
                let sample = (((i % 100) as f32 / 100.0) * 2.0 - 1.0) * 32767.0;
                writer.write_sample(sample as i16).unwrap();
            }
            writer.finalize().unwrap();
        }

        let tmp_path = temp_wav_path("load_wav_24k_mono_pcm");
        std::fs::write(&tmp_path, cursor.into_inner()).unwrap();

        let pcm = crate::load_wav_24k_mono_pcm(&tmp_path).expect("debe cargar y normalizar el WAV");

        // 160 muestras mono @16kHz -> 240 muestras @24kHz (factor 1,5).
        assert_eq!(pcm.len(), 240);
    }

    /// Escribe en un temporal con dueño un WAV válido de 200 muestras al que se le
    /// recortan los últimos bytes, de modo que la cabecera declara más datos de los
    /// que contiene el archivo.
    fn write_truncated_wav(name: &str) -> tempfile::TempPath {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for i in 0..200 {
                writer.write_sample((i * 100) as i16).unwrap();
            }
            writer.finalize().unwrap();
        }
        let mut bytes = cursor.into_inner();
        bytes.truncate(bytes.len() - 100);
        let path = temp_wav_path(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Escribe el WAV de frecuencia nula en un archivo temporal propio de la prueba.
    fn zero_rate_wav_in_temp(name: &str) -> tempfile::TempPath {
        let path = temp_wav_path(name);
        crate::write_zero_rate_wav(&path);
        path
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_zero_sample_rate_is_invalid() {
        let path = zero_rate_wav_in_temp("rate0_16k");
        let result = crate::load_wav_16k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_24k_mono_pcm_zero_sample_rate_is_invalid() {
        let path = zero_rate_wav_in_temp("rate0_24k");
        let result = crate::load_wav_24k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_truncated_is_invalid() {
        let path = write_truncated_wav("truncated_16k");
        let result = crate::load_wav_16k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_24k_mono_pcm_truncated_is_invalid() {
        let path = write_truncated_wav("truncated_24k");
        let result = crate::load_wav_24k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_load_wav_24k_mono_pcm_garbage_bytes_is_invalid() {
        let path = temp_wav_path("garbage_24k");
        std::fs::write(&path, b"esto no es un archivo wav, solo bytes basura").unwrap();
        let result = crate::load_wav_24k_mono_pcm(&path);
        let err = result.unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    #[test]
    fn test_round_trip_conversion_48k_stereo_to_16k_mono_i16() {
        // Onda de rampa determinista intercalada en 2 canales @ 48kHz
        let input: Vec<f32> = (0..300)
            .map(|i| ((i % 100) as f32 / 100.0) * 2.0 - 1.0)
            .collect();

        let mono = crate::to_mono(&input, 2);
        let resampled = crate::resample_linear(&mono, 48000, 16000);
        let pcm = crate::f32_to_i16(&resampled);

        // 300 muestras estéreo -> 150 mono @48kHz -> 50 @16kHz.
        assert_eq!(pcm.len(), 50);
    }

    /// Serializa un WAV de 32 bits enteros con las muestras dadas.
    fn wav_32bit_int_bytes(samples: &[i32]) -> Vec<u8> {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 16_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Int,
        };
        let mut cursor = std::io::Cursor::new(Vec::<u8>::new());
        {
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for &s in samples {
                writer.write_sample(s).unwrap();
            }
            writer.finalize().unwrap();
        }
        cursor.into_inner()
    }

    /// El fondo de escala de 32 bits no desborda: los extremos enteros quedan en
    /// ±1 con su signo, no invertidos.
    #[test]
    fn test_read_normalized_samples_32bit_keeps_sign_and_scale() {
        let bytes = wav_32bit_int_bytes(&[i32::MAX, i32::MIN, 0, 1 << 30]);
        let reader = hound::WavReader::new(std::io::Cursor::new(bytes)).unwrap();
        let samples = crate::read_normalized_samples(reader).unwrap();
        assert_eq!(samples, vec![1.0, -1.0, 0.0, 0.5]);
    }

    /// Una muestra positiva de 32 bits llega positiva al PCM i16 final.
    #[test]
    fn test_load_wav_16k_mono_pcm_32bit_positive_stays_positive() {
        let path = temp_wav_path("pcm32_16k");
        std::fs::write(&path, wav_32bit_int_bytes(&[1 << 30; 8])).unwrap();
        let pcm = crate::load_wav_16k_mono_pcm(&path).unwrap();
        assert_eq!(pcm, vec![16383; 8]);
    }

    /// Una frecuencia de muestreo nula se rechaza antes de tocar el dispositivo,
    /// así que la prueba no depende de que haya audio en la máquina.
    #[test]
    fn test_play_wav_zero_sample_rate_is_error_not_panic() {
        let path = zero_rate_wav_in_temp("rate0_play");
        let err = crate::AudioService::new().play_wav(&path).unwrap_err();
        assert!(
            err.to_string().contains("frecuencia de muestreo"),
            "obtenido: {err}"
        );
    }

    /// Un ratio que el remuestreador no puede construir es `Invalid`, sin
    /// degradar a interpolación lineal.
    #[test]
    fn test_resample_24k_unbuildable_ratio_is_invalid() {
        let err = crate::resample_24k_antialias(&[0.1; 2048], 0).unwrap_err();
        assert!(
            matches!(err, crate::WavLoadError::Invalid(_)),
            "obtenido: {err:?}"
        );
    }

    /// La salida mide exactamente `round(len * 24000 / from_rate)`, para tamaños
    /// menores que un bloque, exactos y no múltiplos, en 16, 44,1 y 48 kHz.
    #[test]
    fn test_resample_24k_output_length_is_exact() {
        // (frecuencia, muestras de entrada, muestras de salida esperadas)
        let cases: [(u32, usize, usize); 12] = [
            (16_000, 100, 150),
            (16_000, 1024, 1536),
            (16_000, 2500, 3750),
            (16_000, 3000, 4500),
            (44_100, 100, 54),
            (44_100, 1024, 557),
            (44_100, 2500, 1361),
            (44_100, 4410, 2400),
            (48_000, 100, 50),
            (48_000, 1024, 512),
            (48_000, 2500, 1250),
            (48_000, 4800, 2400),
        ];
        for (rate, len, expected) in cases {
            let out = crate::resample_24k_antialias(&vec![0.1; len], rate).unwrap();
            assert_eq!(out.len(), expected, "{len} muestras a {rate} Hz");
        }
    }

    /// El retardo del filtro se compensa: una senoide remuestreada coincide con la
    /// senoide ideal a 24 kHz, sin desplazamiento.
    #[test]
    fn test_resample_24k_does_not_shift_the_signal() {
        let sine =
            |rate: f32, n: usize| (2.0 * std::f32::consts::PI * 440.0 * n as f32 / rate).sin();
        for rate in [16_000u32, 48_000] {
            let input: Vec<f32> = (0..4000).map(|n| sine(rate as f32, n)).collect();
            let out = crate::resample_24k_antialias(&input, rate).unwrap();
            for (n, &sample) in out.iter().enumerate().skip(200).take(out.len() - 400) {
                let ideal = sine(24_000.0, n);
                assert!(
                    (sample - ideal).abs() < 0.05,
                    "desfase en la muestra {n} a {rate} Hz: {sample} vs {ideal}"
                );
            }
        }
    }

    #[test]
    fn test_resample_24k_same_rate_is_identity() {
        let input = vec![0.25, -0.5, 0.75];
        assert_eq!(
            crate::resample_24k_antialias(&input, 24_000).unwrap(),
            input
        );
    }

    #[test]
    fn test_push_to_talk_max_secs_parsing() {
        assert_eq!(crate::parse_push_to_talk_max_secs("45").unwrap(), 45);
        for invalid in ["", "abc", "-3", "1.5", "10s"] {
            let err = crate::parse_push_to_talk_max_secs(invalid).unwrap_err();
            assert!(
                err.to_string().contains("AVI_PUSH_TO_TALK_MAX_SECS"),
                "el error debe nombrar la variable: {err}"
            );
        }
    }

    /// La conversión genérica normaliza y rellena con el silencio de cada
    /// formato, sin dispositivo: el equilibrio de los sin signo no es 0.
    #[test]
    fn test_fill_converted_converts_and_pads_with_equilibrium() {
        let samples = [0.0f32, 1.0, -1.0, 2.0];
        let mut idx = 0usize;
        let mut out = [7i16; 6];
        crate::fill_converted(&mut out, &samples, &mut idx);
        assert_eq!(idx, 4);
        assert_eq!(&out[..2], &[0, i16::MAX]);
        assert_eq!(out[2], i16::MIN);
        assert_eq!(out[3], i16::MAX, "2.0 se recorta a 1.0");
        assert_eq!(&out[4..], &[0, 0]);

        let mut idx = 0usize;
        let mut out = [0u16; 5];
        crate::fill_converted(&mut out, &samples[..1], &mut idx);
        assert_eq!(out[0], 32768);
        assert_eq!(&out[1..], &[32768; 4]);

        let mut idx = 0usize;
        let mut out = [1i32; 3];
        crate::fill_converted(&mut out, &samples[..2], &mut idx);
        assert_eq!(&out, &[0, i32::MAX, 0]);
    }

    #[test]
    fn test_estimate_latency_ms() {
        let range = cpal::SupportedBufferSize::Range {
            min: 480,
            max: 4096,
        };
        assert_eq!(crate::estimate_latency_ms(&range, 48_000), Some(10.0));
        assert_eq!(crate::estimate_latency_ms(&range, 0), None);
        assert_eq!(
            crate::estimate_latency_ms(&cpal::SupportedBufferSize::Unknown, 48_000),
            None
        );
    }

    #[test]
    fn test_device_to_json_uses_null_for_unknown_fields() {
        let unknown = crate::AudioDevice {
            id: 3,
            name: None,
            latency_ms: None,
        };
        assert_eq!(
            crate::device_to_json(&unknown),
            serde_json::json!({ "id": 3, "name": null, "latency": null })
        );
        let known = crate::AudioDevice {
            id: 0,
            name: Some("Altavoces".to_string()),
            latency_ms: Some(10.0),
        };
        assert_eq!(
            crate::device_to_json(&known),
            serde_json::json!({ "id": 0, "name": "Altavoces", "latency": 0.01f32 })
        );
    }

    #[test]
    fn test_wait_for_stop_enter_limit_and_failures() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::mpsc::channel;
        use std::time::Duration;
        let alive = AtomicBool::new(false);

        let (tx, rx) = channel();
        tx.send(Ok(())).unwrap();
        let enter = crate::wait_for_stop(&rx, Duration::from_secs(5), &alive).unwrap();
        assert_eq!(enter, crate::StopReason::Enter);

        let (_tx, rx) = channel();
        let limit = crate::wait_for_stop(&rx, Duration::from_millis(60), &alive).unwrap();
        assert_eq!(limit, crate::StopReason::Limit);

        // Un fallo de lectura de stdin no se confunde con Enter.
        let (tx, rx) = channel();
        tx.send(Err(std::io::Error::other("stdin roto"))).unwrap();
        let err = crate::wait_for_stop(&rx, Duration::from_secs(5), &alive).unwrap_err();
        assert!(err.to_string().contains("stdin roto"), "obtenido: {err}");

        // El lector que desaparece sin avisar es un fallo.
        let (tx, rx) = channel::<std::io::Result<()>>();
        drop(tx);
        assert!(crate::wait_for_stop(&rx, Duration::from_secs(5), &alive).is_err());

        // Un fallo del stream interrumpe la espera.
        let (_tx, rx) = channel();
        let failed = AtomicBool::new(false);
        failed.store(true, Ordering::SeqCst);
        let err = crate::wait_for_stop(&rx, Duration::from_secs(5), &failed).unwrap_err();
        assert!(
            err.to_string().contains("stream de audio"),
            "obtenido: {err}"
        );
    }

    #[test]
    fn test_wait_checking_stream_fails_when_stream_fails() {
        use std::sync::atomic::AtomicBool;
        use std::time::Duration;
        let alive = AtomicBool::new(false);
        crate::wait_checking_stream(Duration::from_millis(60), &alive, "la captura").unwrap();
        let failed = AtomicBool::new(true);
        assert!(
            crate::wait_checking_stream(Duration::from_secs(5), &failed, "la captura").is_err()
        );
    }
}
