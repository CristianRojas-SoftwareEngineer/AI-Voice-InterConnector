//! Adaptador de servicio del DSP: limpieza de audio de entrada para clonado y
//! STT (transcripción y doblaje).
//!
//! Es la única etapa que ambos consumidores comparten. No usa la CLI de
//! archivos, no aplica `studio` ni normalización de egresión y no descarga el
//! modelo: `ServiceModels::resolve` exige que `setup` lo haya provisionado y
//! verifica su integridad antes de tocar el audio.
//!
//! Las tasas internas son 16, 24 y 48 kHz. La conversión a/desde 48 kHz usa la
//! autoridad antialias de `avi-audio`, nunca interpolación lineal: DPDFNet
//! opera a 48 kHz y el resultado vuelve a la tasa de entrada para que el
//! consumidor no cambie su contrato (STT sigue recibiendo 16 kHz).

use crate::df::denoise_wav_with_params;
use crate::errors::E;
use crate::models::{AviModelsProvider, ModelPaths, ModelsProvider};
use hound::{SampleFormat, WavSpec, WavWriter};
use std::path::Path;

/// Configuración DSP privada por finalidad. Ambas parten del default probado
/// del origen (`dry_blend=0`, `pause_gate_db=-25`, `highpass=false`) y
/// evolucionan por separado solo tras medir identidad de clonado o precisión
/// STT. No se exponen por CLI ni por API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DenoiseUseCase {
    /// Referencia de clonado: conserva el timbre, sin filtros añadidos.
    Clone,
    /// Entrada de STT compartida por `/transcribe` y `/dub`.
    Stt,
}

impl DenoiseUseCase {
    fn params(self) -> (f32, f64, bool) {
        match self {
            Self::Clone | Self::Stt => (0.0, -25.0, false),
        }
    }
}

/// Modelo ya resuelto y verificado. Mantenerlo evita repetir el hash del
/// artefacto entre la resolución y la inferencia del mismo trabajo.
#[derive(Clone, Debug)]
pub struct ServiceModels {
    paths: ModelPaths,
}

impl ServiceModels {
    /// Resuelve el artefacto provisionado por AVI y comprueba su SHA256. No
    /// descarga: una ausencia o una corrupción se devuelven como
    /// `EModelMissing` y el llamante las convierte en error de modelo antes del
    /// trabajo pesado y sin continuar con audio sin limpiar.
    pub fn resolve() -> Result<Self, E> {
        let paths = AviModelsProvider.ensure_models()?;
        Ok(Self { paths })
    }

    /// Limpia un WAV de referencia arbitrario y escribe el resultado como WAV
    /// PCM16 mono 48 kHz en `output`. `work_dir` debe ser privado de este
    /// trabajo: el WAV intermedio y el resultado viven ahí hasta que el dueño
    /// del directorio los borre.
    pub fn denoise_wav_file(
        &self,
        input: &Path,
        output: &Path,
        use_case: DenoiseUseCase,
        work_dir: &Path,
    ) -> Result<(), E> {
        let pcm48 = avi_audio::load_wav_48k_mono_pcm(input).map_err(map_wav_error)?;
        self.denoise_pcm48(&pcm48, use_case, output, work_dir)
    }

    /// Limpia PCM16 mono a `sample_rate` y devuelve PCM16 mono en la misma tasa.
    /// `sample_rate` debe ser una de las tasas internas (16/24/48 kHz): el
    /// contrato de los flujos no admite otras y una tasa desconocida sería un
    /// error de entrada, no una conversión silenciosa.
    pub fn denoise_pcm16_mono(
        &self,
        pcm: &[i16],
        sample_rate: u32,
        use_case: DenoiseUseCase,
        work_dir: &Path,
    ) -> Result<Vec<i16>, E> {
        if pcm.is_empty() {
            return Err(E::EInvalidInput(
                "El audio de entrada está vacío".to_string(),
            ));
        }
        if !matches!(sample_rate, 16_000 | 24_000 | 48_000) {
            return Err(E::EInvalidInput(format!(
                "Tasa de muestreo no soportada: {sample_rate} Hz (se requiere 16, 24 o 48 kHz)"
            )));
        }

        let pcm48 = if sample_rate == 48_000 {
            pcm.to_vec()
        } else {
            resample_pcm16(pcm, sample_rate, 48_000)?
        };

        let out_wav = work_dir.join("denoise.out.wav");
        self.denoise_pcm48(&pcm48, use_case, &out_wav, work_dir)?;
        let cleaned = read_wav16_mono(&out_wav)?;
        let _ = std::fs::remove_file(&out_wav);

        if sample_rate == 48_000 {
            Ok(cleaned)
        } else {
            resample_pcm16(&cleaned, 48_000, sample_rate)
        }
    }

    /// Núcleo del servicio: PCM ya en 48 kHz, sin conversiones adicionales.
    fn denoise_pcm48(
        &self,
        pcm48: &[i16],
        use_case: DenoiseUseCase,
        output: &Path,
        work_dir: &Path,
    ) -> Result<(), E> {
        if pcm48.is_empty() {
            return Err(E::EInvalidInput(
                "El audio de entrada está vacío".to_string(),
            ));
        }
        let in_wav = work_dir.join("denoise.in.wav");
        write_wav16_mono(&in_wav, 48_000, pcm48)?;

        let provider = ResolvedProvider {
            paths: self.paths.clone(),
        };
        let (dry_blend, pause_gate_db, highpass) = use_case.params();
        let result = denoise_wav_with_params(
            &in_wav,
            output,
            &provider,
            dry_blend,
            pause_gate_db,
            highpass,
            &|_, _| {},
        );
        let _ = std::fs::remove_file(&in_wav);
        result
    }
}

/// Proveedor que devuelve el modelo ya resuelto y verificado por
/// `ServiceModels::resolve`: evita un segundo hash dentro de cada inferencia.
struct ResolvedProvider {
    paths: ModelPaths,
}

impl ModelsProvider for ResolvedProvider {
    fn ensure_models(&self) -> Result<ModelPaths, E> {
        Ok(self.paths.clone())
    }
}

fn map_wav_error(e: avi_audio::WavLoadError) -> E {
    match e {
        avi_audio::WavLoadError::Invalid(detail) => E::EInvalidInput(detail),
        avi_audio::WavLoadError::NotFound => E::EIo(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "El archivo de audio no existe",
        )),
        avi_audio::WavLoadError::Io(e) => E::EIo(e),
    }
}

fn resample_pcm16(pcm: &[i16], from_rate: u32, to_rate: u32) -> Result<Vec<i16>, E> {
    let samples: Vec<f32> = pcm.iter().map(|&s| s as f32 / 32768.0).collect();
    let resampled =
        avi_audio::resample_antialias(&samples, from_rate, to_rate).map_err(map_wav_error)?;
    Ok(resampled
        .iter()
        .map(|&s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
        .collect())
}

fn write_wav16_mono(path: &Path, sample_rate: u32, pcm: &[i16]) -> Result<(), E> {
    let mut writer = WavWriter::create(
        path,
        WavSpec {
            channels: 1,
            sample_rate,
            bits_per_sample: 16,
            sample_format: SampleFormat::Int,
        },
    )
    .map_err(|e| E::EIo(std::io::Error::other(e)))?;
    for &sample in pcm {
        writer
            .write_sample(sample)
            .map_err(|e| E::EIo(std::io::Error::other(e)))?;
    }
    writer
        .finalize()
        .map_err(|e| E::EIo(std::io::Error::other(e)))
}

fn read_wav16_mono(path: &Path) -> Result<Vec<i16>, E> {
    let reader = hound::WavReader::open(path)
        .map_err(|e| E::EInvalidInput(format!("Error abriendo WAV limpio: {e}")))?;
    let spec = reader.spec();
    if spec.channels != 1 || spec.sample_rate != 48_000 || spec.bits_per_sample != 16 {
        return Err(E::EInvalidInput(format!(
            "WAV limpio con formato inesperado: {} canales, {} Hz, {} bits",
            spec.channels, spec.sample_rate, spec.bits_per_sample
        )));
    }
    reader
        .into_samples::<i16>()
        .collect::<Result<Vec<i16>, _>>()
        .map_err(|e| E::EInvalidInput(format!("Error leyendo WAV limpio: {e}")))
}
