//! Módulo `preset.rs` — definición de presets de calidad para empaquetado y post-procesamiento.
//!
//! Un preset **no modifica el núcleo DSP** (el modelo DPDFNet es siempre el mismo).
//! Solo agrupa una combinación coherente de opciones de empaquetado y
//! post-procesamiento en la frontera de entrada (CLI).
//!
//! Precedencia de resolución (`src/cli.rs::resolve_egress_params`):
//! ```text
//! defaults (hardcodeados aquí)
//!     ↓ sobrescritos por
//! preset (si --preset se pasa)
//!     ↓ sobrescritos por
//! flags individuales (si el usuario las pasa explícitamente)
//! ```

use clap::ValueEnum;

/// Bitrate de audio solicitado en la frontera de salida.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioBitrate {
    /// Perfil original: 192 kbps en códecs con pérdida; sin pérdida no lleva.
    Legacy,
    /// Receta `studio`: automático por códec (MP3/AAC 320, Vorbis 192, Opus 256).
    Auto,
    /// Override explícito del usuario, validado contra el códec antes de ffmpeg.
    Explicit(u32),
}

/// Valores por defecto actuales de los parámetros de egress/post-procesamiento.
///
/// Son el punto de partida antes de aplicar un preset o flags individuales.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DefaultParams {
    /// Bitrate de audio solicitado; el códec decide si aplica.
    pub audio_bitrate: AudioBitrate,
    /// Nivel de atenuación de pausas sin voz en dB (-40 a 0; 0 desactiva el gate).
    pub pause_gate_db: f64,
    /// Fracción de mezcla con audio original (0.0 = 100% limpio, 1.0 = 100% original).
    pub dry_blend: f32,
    /// Nivel objetivo de sonoridad en LUFS (-36 a -12). None = sin normalizar.
    pub target_lufs: Option<f64>,
    /// Filtro pasa-altos a 70 Hz para suprimir rumble subgrave.
    pub highpass: bool,
    /// Preservar audio estéreo de 2 canales.
    pub stereo: bool,
}

/// Valores por defecto canónicos del CLI (`denoise` sin flags).
pub const DEFAULT_PARAMS: DefaultParams = DefaultParams {
    audio_bitrate: AudioBitrate::Legacy,
    pause_gate_db: -25.0,
    dry_blend: 0.0,
    target_lufs: None,
    highpass: false,
    stereo: false,
};

/// Parámetros de empaquetado y post-procesamiento resueltos.
///
/// Reúne los 6 valores que afectan a la frontera de salida (egress)
/// sin tocar el núcleo DSP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EgressParams {
    /// Bitrate de audio de salida; el códec decide si aplica y cuál es el
    /// automático (MP3/AAC 320, Vorbis 192, Opus 256).
    pub audio_bitrate: AudioBitrate,
    /// Activar filtro pasa-altos a 70 Hz.
    pub highpass: bool,
    /// Nivel objetivo de sonoridad en LUFS (-36 a -12). None = sin normalizar.
    pub target_lufs: Option<f64>,
    /// Nivel de atenuación de pausas sin voz en dB (-40 a 0; 0 desactiva el gate).
    pub pause_gate_db: f64,
    /// Fracción de mezcla con audio original (0.0 = 100% limpio, 1.0 = 100% original).
    pub dry_blend: f32,
    /// Preservar audio estéreo de 2 canales.
    pub stereo: bool,
}

/// Presets de calidad disponibles en la CLI.
///
/// `None` (= flag `--preset` ausente) conserva los defaults del perfil original;
/// el subcomando de AVI interpreta la ausencia como `studio` y `--preset
/// default` recupera el perfil original.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum Preset {
    /// Perfil original de Denoise: 192 kbps, sin normalización ni highpass.
    #[value(name = "default")]
    Default,
    /// Preset `studio`: empaquetado y post-procesamiento de calidad máxima.
    ///
    /// - `audio_bitrate`: automático por códec (MP3/AAC 320, Vorbis 192, Opus 256).
    /// - `highpass`: activado (filtro pasa-altos 70 Hz contra rumble subgrave).
    /// - `target_lufs`: -16.0 LUFS (normalización EBU R128 web/podcast).
    /// - `pause_gate_db`: -25.0 dB (atenuación de pausas sin voz).
    /// - `dry_blend`: 0.0 (100% señal procesada).
    /// - `stereo`: false (seguir la entrada; no forzar estéreo).
    Studio,
}

impl Preset {
    /// Nombre canónico del preset, usado en CLI y documentación.
    pub fn as_str(&self) -> &'static str {
        match self {
            Preset::Default => "default",
            Preset::Studio => "studio",
        }
    }

    /// Lista canónica de nombres de presets disponibles.
    pub fn variants() -> &'static [&'static str] {
        &["default", "studio"]
    }

    /// Devuelve los parámetros de egress/post-procesamiento del preset.
    ///
    /// El preset **no modifica el núcleo DSP** (DPDFNet es siempre el mismo);
    /// solo define empaquetado y post-procesamiento.
    pub fn egress_params(&self) -> EgressParams {
        match self {
            Preset::Default => EgressParams {
                audio_bitrate: AudioBitrate::Legacy,
                highpass: DEFAULT_PARAMS.highpass,
                target_lufs: DEFAULT_PARAMS.target_lufs,
                pause_gate_db: DEFAULT_PARAMS.pause_gate_db,
                dry_blend: DEFAULT_PARAMS.dry_blend,
                stereo: DEFAULT_PARAMS.stereo,
            },
            Preset::Studio => EgressParams {
                audio_bitrate: AudioBitrate::Auto,
                highpass: true,
                target_lufs: Some(-16.0),
                pause_gate_db: -25.0,
                dry_blend: 0.0,
                stereo: false,
            },
        }
    }
}
