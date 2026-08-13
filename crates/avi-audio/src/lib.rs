use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use serde_json::{json, Value};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// Representación pública de un dispositivo de audio
pub struct AudioDevice {
    pub id: usize,
    pub name: String,
    pub latency_ms: f32,
}

/// Servicio de Audio sobre CPAL (Playback, Capture, Enumeración)
pub struct AudioService {
    host: cpal::Host,
}

impl AudioService {
    pub fn new() -> Self {
        Self {
            host: cpal::default_host(),
        }
    }

    /// Listar dispositivos de salida de audio con estimación de latencia
    pub fn list_output_devices(&self) -> Result<Vec<AudioDevice>> {
        let mut devices = Vec::new();
        if let Ok(output_devices) = self.host.output_devices() {
            for (idx, dev) in output_devices.enumerate() {
                let name = dev.name().unwrap_or_else(|_| format!("Dispositivo {}", idx));
                let latency = dev
                    .default_output_config()
                    .map(|c| {
                        let buffer_size = match c.buffer_size() {
                            cpal::SupportedBufferSize::Range { min, .. } => *min as f32,
                            cpal::SupportedBufferSize::Unknown => 512.0,
                        };
                        (buffer_size / c.sample_rate().0 as f32) * 1000.0
                    })
                    .unwrap_or(10.0);

                devices.push(AudioDevice {
                    id: idx,
                    name,
                    latency_ms: latency,
                });
            }
        }
        Ok(devices)
    }

    /// Reproducir un archivo WAV sobre el dispositivo por defecto usando CPAL
    pub fn play_wav<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let reader = hound::WavReader::open(path)?;
        let spec = reader.spec();

        let device = self
            .host
            .default_output_device()
            .ok_or_else(|| anyhow!("No hay dispositivo de salida predeterminado"))?;

        let config = StreamConfig {
            channels: spec.channels,
            sample_rate: cpal::SampleRate(spec.sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Int => {
                let max_val = (1 << (spec.bits_per_sample - 1)) as f32;
                reader
                    .into_samples::<i32>()
                    .filter_map(Result::ok)
                    .map(|s| s as f32 / max_val)
                    .collect()
            }
            hound::SampleFormat::Float => {
                reader.into_samples::<f32>().filter_map(Result::ok).collect()
            }
        };

        let total_samples = samples.len();
        let sample_idx = Arc::new(Mutex::new(0usize));

        let stream = match device.default_output_config()?.sample_format() {
            SampleFormat::F32 => {
                let idx_arc = sample_idx.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [f32], _| {
                        let mut idx = idx_arc.lock().unwrap();
                        for sample in data.iter_mut() {
                            if *idx < samples.len() {
                                *sample = samples[*idx];
                                *idx += 1;
                            } else {
                                *sample = 0.0;
                            }
                        }
                    },
                    |err| eprintln!("Error en stream de audio: {}", err),
                    None,
                )?
            }
            SampleFormat::I16 => {
                let idx_arc = sample_idx.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [i16], _| {
                        let mut idx = idx_arc.lock().unwrap();
                        for sample in data.iter_mut() {
                            if *idx < samples.len() {
                                *sample = (samples[*idx].clamp(-1.0, 1.0) * 32767.0) as i16;
                                *idx += 1;
                            } else {
                                *sample = 0;
                            }
                        }
                    },
                    |err| eprintln!("Error en stream de audio: {}", err),
                    None,
                )?
            }
            SampleFormat::U16 => {
                let idx_arc = sample_idx.clone();
                device.build_output_stream(
                    &config,
                    move |data: &mut [u16], _| {
                        let mut idx = idx_arc.lock().unwrap();
                        for sample in data.iter_mut() {
                            if *idx < samples.len() {
                                *sample = (((samples[*idx].clamp(-1.0, 1.0) + 1.0) * 0.5)
                                    * u16::MAX as f32) as u16;
                                *idx += 1;
                            } else {
                                *sample = u16::MAX / 2;
                            }
                        }
                    },
                    |err| eprintln!("Error en stream de audio: {}", err),
                    None,
                )?
            }
            _ => return Err(anyhow!("Formato de muestra no soportado para reproducción")),
        };

        stream.play()?;

        // Esperar a que finalice la reproducción de las muestras
        while *sample_idx.lock().unwrap() < total_samples {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        Ok(())
    }

    /// Capturar audio del micrófono por N segundos remuestreado a 16kHz/mono/int16 (Whisper compatible)
    pub fn capture_16k_mono_pcm(&self, duration_secs: u64) -> Result<Vec<i16>> {
        let device = self
            .host
            .default_input_device()
            .ok_or_else(|| anyhow!("No hay dispositivo de entrada predeterminado"))?;

        let config = device.default_input_config()?;
        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;

        let recorded_samples = Arc::new(Mutex::new(Vec::<f32>::new()));

        let stream = match config.sample_format() {
            SampleFormat::F32 => {
                let rec = recorded_samples.clone();
                device.build_input_stream(
                    &config.config(),
                    move |data: &[f32], _| {
                        let mut buffer = rec.lock().unwrap();
                        buffer.extend_from_slice(data);
                    },
                    |err| eprintln!("Error en captura de micrófono: {}", err),
                    None,
                )?
            }
            SampleFormat::I16 => {
                let rec = recorded_samples.clone();
                device.build_input_stream(
                    &config.config(),
                    move |data: &[i16], _| {
                        let mut buffer = rec.lock().unwrap();
                        buffer.extend(data.iter().map(|&s| s as f32 / 32768.0));
                    },
                    |err| eprintln!("Error en captura de micrófono: {}", err),
                    None,
                )?
            }
            SampleFormat::U16 => {
                let rec = recorded_samples.clone();
                device.build_input_stream(
                    &config.config(),
                    move |data: &[u16], _| {
                        let mut buffer = rec.lock().unwrap();
                        buffer.extend(
                            data.iter()
                                .map(|&s| (s as f32 / u16::MAX as f32) * 2.0 - 1.0),
                        );
                    },
                    |err| eprintln!("Error en captura de micrófono: {}", err),
                    None,
                )?
            }
            _ => return Err(anyhow!("Formato de muestra no soportado para captura")),
        };

        stream.play()?;
        std::thread::sleep(std::time::Duration::from_secs(duration_secs));
        drop(stream);

        let raw_samples = recorded_samples.lock().unwrap().clone();

        // Convertir a Mono + Resample a 16kHz + convertir a int16
        let mono_samples = to_mono(&raw_samples, channels);
        let resampled = resample_linear(&mono_samples, sample_rate, 16000);
        let pcm_i16 = f32_to_i16(&resampled);

        Ok(pcm_i16)
    }
}

/// Cargar un WAV arbitrario (`hound`, cualquier tasa/canales/formato) y normalizarlo
/// a PCM `i16` mono a 16 kHz, mismo formato que exige Whisper y que ya produce
/// `capture_16k_mono_pcm` para el micrófono.
pub fn load_wav_16k_mono_pcm(path: impl AsRef<Path>) -> Result<Vec<i16>> {
    let reader = hound::WavReader::open(path)?;
    let spec = reader.spec();

    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Int => {
            let max_val = (1 << (spec.bits_per_sample - 1)) as f32;
            reader
                .into_samples::<i32>()
                .filter_map(Result::ok)
                .map(|s| s as f32 / max_val)
                .collect()
        }
        hound::SampleFormat::Float => {
            reader.into_samples::<f32>().filter_map(Result::ok).collect()
        }
    };

    let mono = to_mono(&samples, spec.channels as usize);
    let resampled = resample_linear(&mono, spec.sample_rate, 16_000);
    Ok(f32_to_i16(&resampled))
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

pub fn get_devices_json() -> Result<Vec<Value>> {
    let service = AudioService::new();
    let devs = service.list_output_devices()?;
    let json_devs = devs
        .into_iter()
        .map(|d| {
            json!({
                "id": d.id,
                "name": d.name,
                "latency": d.latency_ms / 1000.0
            })
        })
        .collect();
    Ok(json_devs)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_audio_resampling_linear() {
        let input = vec![0.0, 0.5, 1.0, 0.5, 0.0];
        let resampled = crate::resample_linear(&input, 44100, 16000);
        assert!(!resampled.is_empty());
        assert!(resampled.len() < input.len());
    }

    #[test]
    fn test_to_mono_downmix_estereo() {
        let input = vec![1.0, 0.0, 0.5, 0.5, 0.0, 1.0];
        let mono = crate::to_mono(&input, 2);
        assert_eq!(mono, vec![0.5, 0.5, 0.5]);

        let mono_directo = vec![0.1, 0.2, 0.3];
        let resultado = crate::to_mono(&mono_directo, 1);
        assert_eq!(resultado, mono_directo);
    }

    #[test]
    fn test_f32_to_i16_escala_y_clamp() {
        let input = vec![0.0, 0.5, -0.5, 1.5, -2.0];
        let output = crate::f32_to_i16(&input);
        assert_eq!(output[0], (0.0f32 * 32767.0) as i16);
        assert_eq!(output[1], (0.5f32 * 32767.0) as i16);
        assert_eq!(output[2], (-0.5f32 * 32767.0) as i16);
        assert_eq!(output[3], 32767);
        assert_eq!(output[4], -32767);
    }

    #[test]
    fn test_load_wav_16k_mono_pcm_normaliza_wav_sintetico() {
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

        let tmp_path = std::env::temp_dir().join("avi_audio_test_load_wav_16k_mono_pcm.wav");
        std::fs::write(&tmp_path, cursor.into_inner()).unwrap();

        let pcm = crate::load_wav_16k_mono_pcm(&tmp_path).expect("debe cargar y normalizar el WAV");
        std::fs::remove_file(&tmp_path).ok();

        // 300 muestras estéreo -> 150 muestras mono @48kHz -> ~50 muestras @16kHz.
        assert!(!pcm.is_empty());
        assert!(pcm.len() < 150, "debe quedar remuestreado a una tasa menor");
    }

    #[test]
    fn test_round_trip_conversion_48k_estereo_a_16k_mono_i16() {
        // Onda de rampa determinista intercalada en 2 canales @ 48kHz
        let input: Vec<f32> = (0..300)
            .map(|i| ((i % 100) as f32 / 100.0) * 2.0 - 1.0)
            .collect();

        let mono = crate::to_mono(&input, 2);
        let resampled = crate::resample_linear(&mono, 48000, 16000);
        let pcm = crate::f32_to_i16(&resampled);

        let esperado = mono.len() / 3;
        assert!(
            (pcm.len() as i64 - esperado as i64).abs() <= 1,
            "longitud inesperada: {} vs ~{}",
            pcm.len(),
            esperado
        );
    }
}
