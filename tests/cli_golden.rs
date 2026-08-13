//! Harness de tests dorados del CLI (Tarea 8 del desbloqueo de Fase 0).
//!
//! Invoca el binario compilado con argumentos fijos y compara `stdout` (JSON) y el
//! código de salida contra fixtures en `tests/golden/`, replicando el contrato que
//! cubrían los scripts Python eliminados: `schema_version == "3"` (vía
//! `avi_core::json_emitter`) y los códigos de salida de `avi_core::exit_codes`.
//!
//! Se ubica como test de integración del paquete raíz (y no dentro de `src/main.rs`)
//! porque capturar `stdout` + exit code con fidelidad exige ejecutar el binario real,
//! y `CARGO_BIN_EXE_*` solo está disponible para tests de integración.

use std::path::PathBuf;
use std::process::Command;

use serde_json::Value;

/// Ruta al binario bajo test, inyectada por Cargo en tests de integración.
const BIN: &str = env!("CARGO_BIN_EXE_ai-voice-interconnector");

/// Carga una fixture dorada desde `tests/golden/`.
fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("fixture {} no es JSON válido: {}", name, e))
}

/// Ejecuta el binario con `args` y devuelve (código de salida, stdout parseado a JSON).
fn run_json(args: &[&str]) -> (i32, Value) {
    let output = Command::new(BIN)
        .args(args)
        .output()
        .expect("el binario debe ejecutarse");
    let code = output.status.code().expect("el proceso debe terminar con un código");
    let stdout = String::from_utf8(output.stdout).expect("stdout debe ser UTF-8");
    let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!("stdout no es JSON válido ({}): {:?}", e, stdout)
    });
    (code, json)
}

#[test]
fn version_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "version"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_version.json"));
}

#[test]
fn speech_transcribe_con_audio_cumple_contrato() {
    let (code, actual) = run_json(&[
        "--json",
        "speech",
        "transcribe",
        "--audio",
        "crates/avi-stt/tests/assets/whisper_sample_16k.wav",
        "--source-language",
        "es-latam",
    ]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    assert_eq!(actual["source"], Value::String("es-latam".to_string()));
    let text = actual["text"].as_str().expect("`text` debe ser un string");
    assert!(!text.is_empty(), "`text` no debe estar vacío");
}

#[test]
fn speech_transcribe_sin_audio_ni_mic_sale_con_codigo_2() {
    let output = Command::new(BIN)
        .args(["--json", "speech", "transcribe", "--source-language", "es-latam"])
        .output()
        .expect("el binario debe ejecutarse");
    let code = output.status.code().expect("el proceso debe terminar con un código");
    assert_eq!(code, 2, "omitir --audio y --mic debe mapear a ExitCode::InvalidInput");
}

#[test]
fn daemon_status_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "daemon", "status"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_daemon_status.json"));
}

#[test]
fn cleanup_coincide_con_fixture() {
    let (code, actual) = run_json(&["--json", "cleanup"]);
    assert_eq!(code, 0);
    assert_eq!(actual, fixture("cli_cleanup.json"));
}

#[test]
fn voice_list_respeta_el_contrato_de_envelope() {
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato (envelope + presencia de `default`).
    let (code, actual) = run_json(&["--json", "voice", "list"]);
    assert_eq!(code, 0);
    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    let voices = actual["voices"].as_array().expect("`voices` debe ser un array");
    assert!(
        voices.iter().any(|v| v == &Value::String("default".to_string())),
        "debe listarse la voz de fábrica `default`"
    );
}

#[test]
fn translate_texto_vacio_sale_con_codigo_2() {
    // Entrada inválida → ExitCode::InvalidInput (2), con el envelope de error del CLI.
    let (code, actual) = run_json(&["--json", "translate", "--text", ""]);
    assert_eq!(code, 2, "texto vacío debe mapear a ExitCode::InvalidInput");
    assert_eq!(actual, fixture("cli_translate_empty.json"));
}
