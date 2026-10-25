//! Tests para presets de calidad y resolución de parámetros de egress.
//!

use clap::Parser;
use denoise::cli::{resolve_egress_params, resolve_egress_params_with, Cli};
use denoise::preset::{AudioBitrate, EgressParams, Preset, DEFAULT_PARAMS};

#[derive(Parser)]
struct TestCli {
    #[command(flatten)]
    cli: Cli,
}

/// Construye un `Cli` usando `try_parse_from` para tests de resolución.
///
/// Se pasa `"dummy"` como input para satisfacer el argumento requerido.
fn make_cli(args: &[&str]) -> Cli {
    let mut full: Vec<&str> = vec!["denoise", "dummy"];
    full.extend_from_slice(args);
    TestCli::try_parse_from(&full)
        .expect("parse_from debe tener éxito")
        .cli
}

/// `Preset::studio` contiene los valores canónicos del preset.
#[test]
fn test_preset_studio_values() {
    let pp = Preset::Studio.egress_params();
    assert_eq!(pp.audio_bitrate, AudioBitrate::Auto);
    assert!(pp.highpass);
    assert_eq!(pp.target_lufs, Some(-16.0));
    assert_eq!(pp.pause_gate_db, -25.0);
    assert_eq!(pp.dry_blend, 0.0);
    assert!(!pp.stereo);
}

/// `Preset::default` conserva el perfil original de Denoise.
#[test]
fn test_preset_default_values() {
    let pp = Preset::Default.egress_params();
    assert_eq!(pp.audio_bitrate, AudioBitrate::Legacy);
    assert!(!pp.highpass);
    assert_eq!(pp.target_lufs, None);
}

/// `Preset::variants()` contiene los nombres canónicos.
#[test]
fn test_preset_variants() {
    assert!(Preset::variants().contains(&"studio"));
    assert!(Preset::variants().contains(&"default"));
}

/// `Preset::as_str()` devuelve el nombre canónico.
#[test]
fn test_preset_as_str() {
    assert_eq!(Preset::Studio.as_str(), "studio");
    assert_eq!(Preset::Default.as_str(), "default");
}

/// Sin preset y sin flags individuales → valores por defecto canónicos.
#[test]
fn test_resolve_egress_params_default() {
    let cli = make_cli(&[]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.audio_bitrate, DEFAULT_PARAMS.audio_bitrate);
    assert_eq!(p.highpass, DEFAULT_PARAMS.highpass);
    assert_eq!(p.target_lufs, DEFAULT_PARAMS.target_lufs);
    assert_eq!(p.pause_gate_db, DEFAULT_PARAMS.pause_gate_db);
    assert_eq!(p.dry_blend, DEFAULT_PARAMS.dry_blend);
    assert_eq!(p.stereo, DEFAULT_PARAMS.stereo);
}

/// `--preset studio` sin flags individuales → valores del preset `studio`.
#[test]
fn test_resolve_egress_params_studio() {
    let cli = make_cli(&["--preset", "studio"]);
    let p = resolve_egress_params(&cli);
    let sp = Preset::Studio.egress_params();
    assert_eq!(p, sp);
}

/// `--preset studio` + flag individual → flag individual gana (precedencia).
#[test]
fn test_resolve_egress_params_studio_override() {
    let cli = make_cli(&["--preset", "studio", "--audio-bitrate", "128"]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.audio_bitrate, AudioBitrate::Explicit(128));
    // El resto conserva los valores del preset studio.
    assert!(p.highpass);
    assert_eq!(p.target_lufs, Some(-16.0));
    assert_eq!(p.pause_gate_db, -25.0);
    assert_eq!(p.dry_blend, 0.0);
    assert!(!p.stereo);
}

/// Flags individuales sin preset → override sobre defaults.
#[test]
fn test_resolve_egress_params_flags_only() {
    let cli = make_cli(&["--audio-bitrate", "256", "--highpass", "--dry-blend", "0.1"]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.audio_bitrate, AudioBitrate::Explicit(256));
    assert!(p.highpass);
    assert_eq!(p.dry_blend, 0.1);
    assert_eq!(p.target_lufs, DEFAULT_PARAMS.target_lufs);
}

/// `--normalize` sin `--target-lufs` → target_lufs = Some(-16.0).
#[test]
fn test_resolve_egress_params_normalize_default() {
    let cli = make_cli(&["--normalize"]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.target_lufs, Some(-16.0));
}

/// `--normalize` + `--target-lufs -14` → flag individual gana.
#[test]
fn test_resolve_egress_params_normalize_override() {
    // Usar `=` para pasar valores negativos y evitar que clap lo interprete como flag.
    let cli = make_cli(&["--normalize", "--target-lufs=-14"]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.target_lufs, Some(-14.0));
}

/// Preset studio + `--normalize` + `--target-lufs -14` → flag individual gana sobre preset.
#[test]
fn test_resolve_egress_params_studio_normalize_override() {
    let cli = make_cli(&["--preset", "studio", "--normalize", "--target-lufs=-14"]);
    let p = resolve_egress_params(&cli);
    assert_eq!(p.target_lufs, Some(-14.0));
    assert_eq!(p.audio_bitrate, AudioBitrate::Auto);
    assert!(p.highpass);
}

/// `EgressParams` implementa `PartialEq` para comparaciones limpias en tests.
#[test]
fn test_egress_params_eq() {
    let a = Preset::Studio.egress_params();
    let b = Preset::Studio.egress_params();
    assert_eq!(a, b);
}

/// El struct `EgressParams` tiene exactamente los 6 campos definidos por el plan.
#[test]
fn test_egress_params_fields() {
    let p = EgressParams {
        audio_bitrate: AudioBitrate::Legacy,
        highpass: false,
        target_lufs: None,
        pause_gate_db: -25.0,
        dry_blend: 0.0,
        stereo: false,
    };
    assert_eq!(p.audio_bitrate, AudioBitrate::Legacy);
    assert!(!p.highpass);
    assert_eq!(p.target_lufs, None);
    assert_eq!(p.pause_gate_db, -25.0);
    assert_eq!(p.dry_blend, 0.0);
    assert!(!p.stereo);
}

/// El subcomando de AVI interpreta la ausencia de preset como `studio`, y
/// `--preset default` recupera el perfil original.
#[test]
fn test_avi_default_preset_is_studio() {
    let cli = make_cli(&[]);
    let p = resolve_egress_params_with(&cli, true);
    assert_eq!(p, Preset::Studio.egress_params());

    let cli = make_cli(&["--preset", "default"]);
    let p = resolve_egress_params_with(&cli, true);
    assert_eq!(p, Preset::Default.egress_params());
}
