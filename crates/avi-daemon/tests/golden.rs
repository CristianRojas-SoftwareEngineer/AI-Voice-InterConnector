//! Harness de tests dorados del daemon (Tarea 8 del desbloqueo de Fase 0).
//!
//! Levanta el `Router` de Axum vía [`avi_daemon::build_router`] y lo ejercita con
//! `tower::ServiceExt::oneshot` (sin abrir un socket TCP real), comparando cada
//! respuesta contra fixtures fijas en `tests/golden/` de la raíz del repo. Detecta
//! regresiones de contrato (formato JSON, `schema_version`, textos de estado/error)
//! entre el runtime nativo y lo que el resto del sistema espera.
//!
//! Los handlers del daemon son deterministas y no requieren pesos reales, salvo
//! `/voices`, cuyo contenido depende del `data_dir` del usuario: para esa ruta se
//! verifican los invariantes de contrato (envelope + presencia de la voz `default`)
//! en lugar de una igualdad exacta.

use std::path::PathBuf;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::build_router;

/// Carga una fixture dorada desde `tests/golden/` en la raíz del workspace.
fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tests/golden")
        .join(name);
    let content = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("no se pudo leer la fixture {}: {}", path.display(), e));
    serde_json::from_str(&content)
        .unwrap_or_else(|e| panic!("fixture {} no es JSON válido: {}", name, e))
}

/// Envía una petición al router y devuelve (status, cuerpo crudo en bytes).
async fn send(req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = build_router()
        .oneshot(req)
        .await
        .expect("el router debe responder");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("el cuerpo debe poder leerse")
        .to_bytes()
        .to_vec();
    (status, bytes)
}

/// Petición GET simple.
fn get(uri: &str) -> Request<Body> {
    Request::builder()
        .method("GET")
        .uri(uri)
        .body(Body::empty())
        .unwrap()
}

/// Petición POST con cuerpo JSON.
fn post_json(uri: &str, body: Value) -> Request<Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn health_coincide_con_fixture() {
    let (status, bytes) = send(get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_health.json"));
}

#[tokio::test]
async fn transcribe_coincide_con_fixture() {
    let (status, bytes) = send(post_json("/transcribe", serde_json::json!({}))).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_transcribe.json"));
}

#[tokio::test]
async fn synthesize_texto_vacio_es_error_de_contrato() {
    let (status, bytes) = send(post_json("/synthesize", serde_json::json!({ "text": "" }))).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

#[tokio::test]
async fn synthesize_emite_stream_ndjson_de_contrato() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "hola", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

    // El cuerpo es NDJSON: una línea JSON por evento (start/progress/error).
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let eventos: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();

    assert_eq!(Value::Array(eventos), fixture("daemon_synthesize_stream.json"));
}

#[tokio::test]
async fn voices_respeta_el_contrato_de_envelope() {
    // El contenido exacto depende del `data_dir` del usuario; se verifican los
    // invariantes de contrato en vez de una igualdad exacta.
    let (status, bytes) = send(get("/voices")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");

    assert_eq!(actual["schema_version"], Value::String("3".to_string()));
    let voices = actual["voices"].as_array().expect("`voices` debe ser un array");
    let default = voices
        .iter()
        .find(|v| v["name"] == Value::String("default".to_string()))
        .expect("debe existir la voz de fábrica `default`");
    assert_eq!(default["is_factory"], Value::Bool(true));
    assert!(default.get("has_reference").is_some(), "contrato: `has_reference` presente");
}
