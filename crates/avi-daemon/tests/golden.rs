//! Harness de tests dorados del daemon.
//!
//! CLASE DECLARADA: contrato puro paralelizable sin restricción.
//! Levanta el `Router` de Axum vía [`avi_daemon::build_router_with_state`] y lo
//! ejerce con `tower::ServiceExt::oneshot` (sin abrir socket TCP real), comparando
//! cada respuesta contra fixtures fijas en `tests/golden/` de la raíz del repo.
//! Detecta regresiones de contrato (formato JSON, `schema_version`, textos de
//! estado/error) entre el runtime nativo y lo que el resto del sistema espera.
//!
//! El harness construye `DaemonState` con el motor STT real (Parakeet TDT v3 int8,
//! export de `istupakov`), que solo existe con `native-stt`. Sin el feature, el
//! harness cae a un `ParakeetEngine` construido con un directorio de modelo
//! vacío; `oneshot` no ejerce la ruta `/transcribe` en estos tests, así que el
//! motor se queda sin inicializar y el resto de la suite corre sin ONNX
//! Runtime. Los tests que sí requieren STT (golden transcribe) son gated
//! individualmente con `#[cfg(feature = "native-stt")]` en sus cuerpos.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Once;
use std::sync::OnceLock;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use base64::Engine;
use http_body_util::BodyExt;
use serde_json::Value;
use tower::ServiceExt;

use avi_daemon::{build_router_with_state, DaemonState, WarmState};
use avi_store::{SpeechStore, VoiceStore};

/// Prepara una sola vez por proceso el entorno que comparten los dos estados:
/// `AVI_DATA_DIR` apunta al directorio temporal propio del proceso para que los
/// logs del motor residente que lanza el clonado no caigan en el directorio de
/// datos del usuario, y el motor residente escucha en un puerto efímero propio
/// (`QWEN3_TTS_PORT`) para no chocar con el del daemon del usuario.
fn prepare_shared_env() {
    static PREPARED: Once = Once::new();
    PREPARED.call_once(|| {
        let tmp = std::env::temp_dir().join(format!("avi_golden_state_{}", std::process::id()));
        std::env::set_var("AVI_DATA_DIR", &tmp);
        let tts_port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .expect("reservar un puerto efímero para el motor residente")
            .port();
        std::env::set_var("QWEN3_TTS_PORT", tts_port.to_string());
    });
}

/// Estado de contrato único, reutilizable entre tests. Los almacenes de voces y
/// de habla se anclan en un directorio temporal propio del proceso. Su motor TTS
/// no tiene binario (`binary_path = None`) aunque exista vendor, `qwen_tts` en
/// el `PATH` o `QWEN3_TTS_BIN`, así que ninguna prueba de contrato puede lanzar
/// el motor real. Con `native-stt` el estado carga Parakeet, que se exige
/// antes. `warm` nace en `Warming` porque aquí no corre `run_daemon_server`
/// (sin bind ni warmup real); los tests de contrato leen el estado tal cual.
static TEST_STATE: OnceLock<Arc<DaemonState>> = OnceLock::new();

fn test_state() -> Arc<DaemonState> {
    TEST_STATE
        .get_or_init(|| {
            #[cfg(feature = "native-stt")]
            require_parakeet();
            prepare_shared_env();
            let tmp = std::env::temp_dir().join(format!("avi_golden_state_{}", std::process::id()));
            let mut state = DaemonState::with_stores(
                VoiceStore::at(tmp.join("voices")),
                SpeechStore::at(tmp.join("speech")),
            )
            .expect("estado del daemon con almacenes temporales");
            state.tts_engine.binary_path = None;
            Arc::new(state)
        })
        .clone()
}

/// Estado de clonado, solo para las pruebas que ejercen el motor real. Usa
/// almacenes en otro directorio temporal propio del proceso y su motor TTS
/// apunta a `QWEN3_TTS_BIN` si está definida o, si no, al binario de
/// `vendor/qwen3-tts`, porque el cwd del crate no ve el vendor del workspace.
/// El clonado y el arranque del residente buscan el binario en `QWEN3_TTS_BIN`
/// y no en el campo, así que se fija también la variable; el estado de contrato
/// no la ve porque su `binary_path = None` corta `/synthesize` antes del motor.
static CLONE_STATE: OnceLock<Arc<DaemonState>> = OnceLock::new();

fn clone_state() -> Arc<DaemonState> {
    CLONE_STATE
        .get_or_init(|| {
            #[cfg(feature = "native-stt")]
            require_parakeet();
            prepare_shared_env();
            let tmp = std::env::temp_dir().join(format!("avi_golden_clone_{}", std::process::id()));
            let mut state = DaemonState::with_stores(
                VoiceStore::at(tmp.join("voices")),
                SpeechStore::at(tmp.join("speech")),
            )
            .expect("estado del daemon con almacenes temporales");
            let binary = std::env::var_os("QWEN3_TTS_BIN")
                .map(PathBuf::from)
                .unwrap_or_else(vendor_binary);
            std::env::set_var("QWEN3_TTS_BIN", &binary);
            state.tts_engine.binary_path = Some(binary);
            Arc::new(state)
        })
        .clone()
}

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
    send_to(test_state(), req).await
}

/// Envía una petición al router construido sobre el estado indicado.
async fn send_to(state: Arc<DaemonState>, req: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = build_router_with_state(state)
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

// Ceguera deliberada del harness: `send`/`get`/`post_json` ejercen el contrato
// JSON vía `oneshot` sin abrir socket, sin spawn de procesos, sin señales, sin
// puertos reales ni pidfile; por tanto nunca prueban la ausencia de huérfanos
// (árbol de procesos, PID, puertos), ni el endurecimiento del harness frente a
// fallos envenenados o al reaper corriendo fuera de los polls, ni la
// higiene de `TEST_LIMIT`, ni la ventana entre el spawn y la escritura del
// pidfile, ni el manejo de señales durante el spawn, ni un crash con el
// puerto ya ocupado y un reclamo activo del árbol previo (este último solo
// aplica en `run_supervised`): al no añadir campos nuevos a `DaemonState`, el
// doble de pruebas queda intacto, y su límite conocido es no reproducir el
// ciclo de vida real del proceso. La ausencia real de huérfanos a nivel de
// sistema operativo solo la verifica la serie de tests pesada
// (con reaper ruidoso y `verificar_cero_huerfanos`).

/// Exige `nemo128.onnx` del snapshot de Parakeet: la raíz de modelos vive fuera
/// del repo y la provisiona `ai-voice-interconnector setup`.
#[cfg(feature = "native-stt")]
fn require_parakeet() {
    let model_dir = avi_store::ModelStore::new()
        .model_snapshot_path("parakeet-tdt-v3")
        .expect("el modelo parakeet-tdt-v3 debe tener un pin de revisión");
    let preprocessor = model_dir.join("nemo128.onnx");
    std::fs::metadata(&preprocessor).unwrap_or_else(|e| {
        panic!(
            "falta {} ({e}): provisiona Parakeet con `ai-voice-interconnector setup`",
            preprocessor.display()
        )
    });
}

/// Exige `model.safetensors` del snapshot del modelo Base de clonado: lo
/// provisiona `ai-voice-interconnector setup --with-voice-cloning`.
fn require_base_model() {
    let model_dir = avi_store::ModelStore::new()
        .model_snapshot_path("qwen3-tts-0.6b-base")
        .expect("el modelo qwen3-tts-0.6b-base debe tener un pin de revisión");
    let weights = model_dir.join("model.safetensors");
    std::fs::metadata(&weights).unwrap_or_else(|e| {
        panic!(
            "falta {} ({e}): provisiona el modelo Base con `ai-voice-interconnector setup --with-voice-cloning`",
            weights.display()
        )
    });
}

/// Binario del motor en `vendor/qwen3-tts` del workspace.
fn vendor_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(if cfg!(windows) {
        "../../vendor/qwen3-tts/qwen_tts.exe"
    } else {
        "../../vendor/qwen3-tts/qwen_tts"
    })
}

/// Exige el binario del motor, salvo que `QWEN3_TTS_BIN` ya apunte a otro. Lo
/// construye `cargo xtask build-engine`.
fn require_clone_binary() {
    if std::env::var_os("QWEN3_TTS_BIN").is_some() {
        return;
    }
    let binary = vendor_binary();
    assert!(
        binary.is_file(),
        "falta {}: constrúyelo con `cargo xtask build-engine`",
        binary.display()
    );
}

#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn health_matches_fixture() {
    // Los almacenes del estado deben anclarse bajo un directorio temporal propio
    // y no en el directorio de datos del usuario; se afirma antes de escribir.
    let tmp = std::env::temp_dir().join(format!("avi_golden_state_{}", std::process::id()));
    let state = test_state();
    assert!(
        state.voice_store.root().starts_with(&tmp),
        "raíz de voces fuera del temporal: {}",
        state.voice_store.root().display()
    );
    assert!(
        state.speech_store.root().starts_with(&tmp),
        "raíz de habla fuera del temporal: {}",
        state.speech_store.root().display()
    );
    let (status, bytes) = send(get("/health")).await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let fixture_val = fixture("daemon_health.json");
    assert_eq!(actual["status"], fixture_val["status"]);
    assert_eq!(actual["warm"], fixture_val["warm"]);
    assert_eq!(actual["schema_version"], Value::String("4".to_string()));
    // Tolerar claves aditivas ct2/stt sin bump schema_version
    assert_eq!(actual["engine"], fixture_val["engine"]);
}

#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_matches_fixture() {
    require_parakeet();
    // Payload `{}` (campo audio_b64 ausente) → rama de error de campo ausente
    // diseñada a propósito (no un stub `transcription_pending`), con estado 400.
    let (status, bytes) = send(post_json("/transcribe", serde_json::json!({}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_transcribe.json"));
}

#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn synthesize_empty_text_is_contract_error() {
    let (status, bytes) = send(post_json("/synthesize", serde_json::json!({ "text": "" }))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

/// Texto de 501 caracteres: el daemon lo rechaza con 400 y `text_too_long`.
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn synthesize_text_too_long_is_contract_error() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "a".repeat(501) }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_too_long.json"));
}

/// Los campos de idioma y temperatura son opcionales: un payload que solo trae
/// `text` y `voice` se valida igual (mismo error de contrato ante texto vacío).
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn synthesize_old_payload_without_new_fields() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual, fixture("daemon_synthesize_empty.json"));
}

/// Temperatura fuera de rango (`0`): el stream NDJSON termina en error con
/// motivo `usage_error`, sin llegar a la síntesis.
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn synthesize_invalid_temperature_is_usage_error() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "hola", "voice": "default", "temperature": 0.0 }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    let final_event = events.last().expect("debe haber al menos un evento");
    assert_eq!(final_event["event"], Value::String("error".to_string()));
    assert_eq!(
        final_event["reason"],
        Value::String("usage_error".to_string())
    );
}

#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn synthesize_emits_contract_ndjson_stream() {
    let (status, bytes) = send(post_json(
        "/synthesize",
        serde_json::json!({ "text": "hola", "voice": "default" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);

    // El cuerpo es NDJSON: una línea JSON por evento (start/progress/result|error).
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    assert!(!events.is_empty(), "debe haber al menos un evento");

    // Invariante de envelope: schema_version=4 en todo evento.
    for e in &events {
        assert_eq!(
            e["schema_version"],
            Value::String("4".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }

    // Invariante: el primer evento es `start`.
    assert_eq!(
        events.first().unwrap()["event"],
        Value::String("start".to_string()),
        "el stream NDJSON debe comenzar con `start`"
    );

    // Invariante: evento final `error` por falta de motor.
    // El estado de contrato no tiene binario del motor, así que el evento final
    // es `error` con `reason` `model_missing`. La síntesis real con audio
    // verdadero se verifica por separado contra el motor.
    let final_event = events.last().unwrap();
    assert_eq!(
        final_event["event"],
        Value::String("error".to_string()),
        "evento final insuficiente: {:?}",
        final_event
    );
    assert_eq!(
        final_event["reason"],
        Value::String("model_missing".to_string()),
        "evento final insuficiente: {:?}",
        final_event
    );
}

/// Audio largo (~22 s, concatenación de 4 corpus): Parakeet no necesita chunking VAD
/// (RTF ~0.11 lineal); se transcribe de una sola pasada y se verifica el texto unido.
#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_long_audio_transcribes_in_one_pass() {
    require_parakeet();
    let assets = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../avi-stt/tests/assets");
    let corpus = [
        "corpus_sintesis_16k.wav",
        "corpus_watermark_16k.wav",
        "corpus_respuestas_16k.wav",
        "parakeet_sample_16k.wav",
    ];
    let mut pcm: Vec<i16> = Vec::new();
    for wav in corpus {
        let seg = avi_audio::load_wav_16k_mono_pcm(assets.join(wav))
            .expect("el WAV corpus debe cargarse");
        pcm.extend_from_slice(&seg);
    }
    assert!(
        pcm.len() > 240_000,
        "el audio concatenado debe superar los 15 s de una sola pasada"
    );

    let audio_bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let (status, bytes) = send(post_json(
        "/transcribe",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio_bytes),
            "source_language": "es-latam",
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    let text = actual["text"].as_str().unwrap_or("");
    let norm = text.to_lowercase();
    // Frases de cada corpus que el modelo transcribe bien (el sintético
    // `sintesis` tiene pronunciación defectuosa → se usan palabras estables).
    // "hola" se descuenta: el fixture `parakeet_sample` (saludo breve) se emite
    // en inglés por el TDT, por lo que no aparece en el texto
    // unido aunque el resto del audio (watermark/sintesis/respuestas) sí se
    // transcribe en español. "voz" no se exige estricta: "esténtesis" puede
    // dropearla.
    for phrase in ["marca de agua", "usuario", "espajo"] {
        assert!(
            norm.contains(phrase),
            "el texto unido debe contener {:?}: {text:?}",
            phrase
        );
    }
}

/// Audio de ~70 s (PCM 16 kHz mono, ~3 MB de cuerpo en base64): supera el límite
/// de cuerpo por defecto de 2 MiB pero cabe en el tope de transcripción de 300 s,
/// así que `/transcribe` debe procesarlo y responder 200.
#[cfg(feature = "native-stt")]
#[tokio::test]
#[ignore = "requiere Parakeet"]
async fn transcribe_audio_over_default_body_limit_is_accepted() {
    require_parakeet();
    let silence = vec![0u8; 70 * 16_000 * 2];
    let (status, bytes) = send(post_json(
        "/transcribe",
        serde_json::json!({
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&silence),
            "source_language": "es-latam",
        }),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
}

/// WAV PCM 16-bit mono 16 kHz de silencio con la duración indicada.
fn silent_wav(secs: u32) -> Vec<u8> {
    let data_len = secs * 16_000 * 2;
    let mut wav = Vec::<u8>::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    wav.resize(wav.len() + data_len as usize, 0);
    wav
}

/// Un cuerpo de más de 2 MiB con nombre de voz inválido se valida y responde 400
/// `invalid_voice_name`: el límite de cuerpo de la ruta no puede cortar antes con 413.
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn voices_clone_oversized_body_reports_invalid_name_not_413() {
    let oversized = vec![0u8; 3 * 1024 * 1024];
    let (status, bytes) = send(post_json(
        "/voices/clone",
        serde_json::json!({
            "name": "../invalida",
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&oversized),
        }),
    ))
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], "invalid_voice_name");
}

/// Una referencia WAV de 31 s supera el tope de 30 s y se rechaza con 400
/// `audio_too_long` antes de buscar el modelo Base. El estado se construye sin
/// modelo Base para que, mientras falte el tope, la prueba no pueda lanzar el
/// clonado real aunque el modelo esté provisionado en el equipo.
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn voices_clone_reference_over_limit_is_audio_too_long() {
    #[cfg(feature = "native-stt")]
    require_parakeet();
    prepare_shared_env();
    let tmp = std::env::temp_dir().join(format!("avi_golden_reflimit_{}", std::process::id()));
    let mut state = DaemonState::with_stores(
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
    )
    .expect("estado del daemon con almacenes temporales");
    state.tts_engine.binary_path = None;
    state.tts_engine.base_model_dir = None;
    let (status, bytes) = send_to(
        Arc::new(state),
        post_json(
            "/voices/clone",
            serde_json::json!({
                "name": "referencia_larga",
                "force": true,
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(silent_wav(31)),
            }),
        ),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::BAD_REQUEST,
        "cuerpo: {}",
        String::from_utf8_lossy(&bytes)
    );
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["reason"], "audio_too_long");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Warm-on-clone: `POST /voices/clone` por daemon sirve un stream
/// NDJSON (`started` → latidos → `result`), y el evento final conserva
/// `precomputed: true` («precarga en caliente iniciada»; la completitud se
/// refleja en `/health`). Invierte el «éxito inmediato»: el test solo pasa si
/// la secuencia completa llega hasta el final.
/// Requiere el modelo Base de clonado: sin él el handler retorna
/// `model_missing` en vez de clonar.
#[tokio::test]
#[ignore = "requiere el modelo Base de clonado y el binario del motor"]
async fn voices_clone_daemon_precomputed_true() {
    require_base_model();
    require_clone_binary();
    let wav = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../avi-stt/tests/assets/parakeet_sample_16k.wav");
    let audio_bytes = std::fs::read(&wav).expect("el WAV de muestra debe leerse");
    let name = format!("clon_warm_{}", std::process::id());
    let (status, bytes) = send_to(
        clone_state(),
        post_json(
            "/voices/clone",
            serde_json::json!({
                "name": name,
                "force": true,
                "audio_b64": base64::engine::general_purpose::STANDARD.encode(&audio_bytes),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    // El cuerpo es NDJSON: una línea JSON por evento.
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let events: Vec<Value> = text
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("cada línea debe ser JSON"))
        .collect();
    // Un clonado con éxito deja en segundo plano una precarga que arranca el
    // motor residente; se espera a que termine y se detiene el residente antes
    // de las aserciones para que no sobreviva al proceso de pruebas.
    if events.last().is_some_and(|e| e["event"] == "result") {
        let state = clone_state();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(300);
        while matches!(*state.warm.read().unwrap(), WarmState::Warming) {
            assert!(
                std::time::Instant::now() < deadline,
                "la precarga del clon no terminó en 300 s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }
        state.tts_engine.shutdown();
    }
    assert!(!events.is_empty(), "debe haber al menos un evento");
    // Invariante de envelope: schema_version=4 en todo evento.
    for e in &events {
        assert_eq!(
            e["schema_version"],
            Value::String("4".to_string()),
            "todo evento NDJSON lleva schema_version"
        );
    }
    // Aceptación inmediata: el primer evento es `started` con el nombre.
    assert_eq!(
        events.first().unwrap()["event"],
        Value::String("started".to_string()),
        "el stream debe comenzar con `started`"
    );
    assert_eq!(events.first().unwrap()["name"], Value::String(name.clone()));
    // Evento final `result` con la forma contractual actual (`precomputed: true`).
    let final_event = events.last().unwrap();
    assert_eq!(
        final_event["event"],
        Value::String("result".to_string()),
        "el stream debe terminar con `result`: {:?}",
        final_event
    );
    assert_eq!(final_event["name"], Value::String(name.clone()));
    assert_eq!(final_event["precomputed"], Value::Bool(true));
    // Intermedios: solo latidos o progreso (nunca un segundo `started`/`result`).
    if events.len() > 2 {
        for e in &events[1..events.len() - 1] {
            let ev = e["event"].as_str().unwrap_or("");
            assert!(
                ev == "heartbeat" || ev == "progress",
                "evento intermedio debe ser latido o progreso: {:?}",
                e
            );
        }
    }
    let _ = clone_state().voice_store.remove(&name);
}

/// Una referencia WAV truncada (la cabecera declara más datos de los que hay)
/// se rechaza en la carga, antes de invocar el motor de clonado, con un evento
/// `error` de razón `invalid_audio`.
/// Requiere el modelo Base de clonado: sin él el handler responde
/// `model_missing` antes de llegar a la carga del audio.
#[tokio::test]
#[ignore = "requiere el modelo Base de clonado"]
async fn voices_clone_truncated_wav_emits_invalid_audio() {
    require_base_model();
    // WAV PCM 16-bit mono 16 kHz: cabecera de 44 bytes que declara 3200 bytes de
    // datos y solo 1000 presentes.
    let mut wav = Vec::<u8>::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36u32 + 3200).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&16_000u32.to_le_bytes());
    wav.extend_from_slice(&32_000u32.to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&3200u32.to_le_bytes());
    wav.extend_from_slice(&[0u8; 1000]);
    let name = format!("clon_truncado_{}", std::process::id());
    let (status, bytes) = send(post_json(
        "/voices/clone",
        serde_json::json!({
            "name": name,
            "force": true,
            "audio_b64": base64::engine::general_purpose::STANDARD.encode(&wav),
        }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let text = String::from_utf8(bytes).expect("NDJSON debe ser UTF-8");
    let last: Value = serde_json::from_str(text.lines().rfind(|l| !l.trim().is_empty()).unwrap())
        .expect("cada línea debe ser JSON");
    assert_eq!(last["event"], Value::String("error".to_string()));
    assert_eq!(last["reason"], Value::String("invalid_audio".to_string()));
    let _ = test_state().voice_store.remove(&name);
}

/// Warm-voice configurable: `run_daemon_server` con una `--warm-voice`
/// inexistente aborta con `StartupError::WarmVoiceMissing`, porque la voz se
/// valida antes de enlazar y de cargar modelos (por eso esta prueba no
/// necesita modelos provisionados), y una voz existente pasa la validación de
/// arranque. La rama de aceptación se verifica sobre el mismo predicado que
/// usa la guarda (`find_reference(...).is_some()`): arrancar el servidor real
/// con voz válida bloquearía sirviendo, así que no se invoca.
#[tokio::test]
async fn warm_voice_fail_fast_and_acceptance() {
    // Fail-fast: voz inexistente → Err antes del bind.
    let addr: std::net::SocketAddr = "127.0.0.1:0".parse().unwrap();
    let nonexistent_voice = format!("warm_inexistente_{}", std::process::id());
    let tmp = std::env::temp_dir().join(format!("avi_golden_warm_missing_{}", std::process::id()));
    let res = avi_daemon::run_daemon_server(
        addr,
        nonexistent_voice.clone(),
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
    )
    .await;
    let err = res.expect_err("una --warm-voice inexistente debe abortar el arranque");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::WarmVoiceMissing {
            voice: nonexistent_voice.clone()
        })
    );
    assert!(
        err.to_string().contains("--warm-voice"),
        "el error fail-fast debe mencionar --warm-voice: {err}"
    );

    // Aceptación: una voz existente satisface el predicado de la guarda.
    let store = VoiceStore::at(
        std::env::temp_dir().join(format!("avi_golden_warm_ok_{}", std::process::id())),
    );
    let tmp = std::env::temp_dir().join(format!("warm_ok_{}.qvoice", std::process::id()));
    std::fs::write(&tmp, b"qvoice-fixture").expect("escribir fixture qvoice");
    let name = format!("warm_ok_{}", std::process::id());
    store
        .save_reference(&name, &tmp)
        .expect("registrar voz de prueba");
    let _ = std::fs::remove_file(&tmp);
    assert!(
        store.find_reference(&name).is_some(),
        "la voz registrada debe pasar la validación de --warm-voice"
    );
    let _ = store.remove(&name);
}

/// Puerto ocupado: `run_daemon_server` devuelve `StartupError::PortInUse` con
/// el puerto pedido en menos de 2 s, porque enlaza antes de cargar los
/// modelos. No necesita modelos provisionados.
#[tokio::test]
async fn port_in_use_fails_before_loading_models() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("ocupar un puerto");
    let busy = holder.local_addr().unwrap();
    let start = std::time::Instant::now();
    let tmp = std::env::temp_dir().join(format!("avi_golden_port_busy_{}", std::process::id()));
    let err = avi_daemon::run_daemon_server(
        busy,
        "default".into(),
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
    )
    .await
    .expect_err("un puerto ocupado debe abortar el arranque");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::PortInUse { port: busy.port() })
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "el fallo por puerto ocupado debe ser inmediato: {:?}",
        start.elapsed()
    );
}

/// La supervisión no reintenta un fallo previo a estar listo: con el puerto
/// ocupado, `run_supervised` con `--auto-restart` devuelve `PortInUse` en
/// menos de 2 s. Un reintento costaría al menos el primer backoff (500 ms) más
/// la espera de puerto libre (hasta 5 s).
#[tokio::test]
async fn supervised_startup_failure_is_terminal() {
    let holder = std::net::TcpListener::bind("127.0.0.1:0").expect("ocupar un puerto");
    let busy = holder.local_addr().unwrap();
    let start = std::time::Instant::now();
    let tmp = std::env::temp_dir().join(format!("avi_golden_supervised_{}", std::process::id()));
    let err = avi_daemon::run_supervised(
        busy,
        true,
        3,
        "default".into(),
        VoiceStore::at(tmp.join("voices")),
        SpeechStore::at(tmp.join("speech")),
    )
    .await
    .expect_err("un puerto ocupado debe abortar la supervisión");
    assert_eq!(
        err.downcast_ref::<avi_daemon::StartupError>(),
        Some(&avi_daemon::StartupError::PortInUse { port: busy.port() })
    );
    assert!(
        start.elapsed() < std::time::Duration::from_secs(2),
        "la supervisión no debe reintentar un fallo de arranque: {:?}",
        start.elapsed()
    );
}

/// Par de idiomas no soportado vía IPC → 400 con `unsupported_language_pair`.
/// La guarda de par corre antes de tocar el modelo: sin CT2 ni TCP.
#[cfg(feature = "native-translation")]
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn translate_unsupported_pair_returns_400() {
    let (status, bytes) = send(post_json(
        "/translate",
        serde_json::json!({ "text": "Bonjour", "from": "fr", "to": "de" }),
    ))
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(
        actual["reason"],
        Value::String("unsupported_language_pair".to_string())
    );
}

/// La vía daemon conserva la normalización `es-latam`→`es`:
/// passthrough con texto intacto aunque el CLI ya rechace ese token.
#[cfg(feature = "native-translation")]
#[tokio::test]
#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]
async fn translate_es_latam_passthrough_ipc_returns_intact_text() {
    let (status, bytes) = send(post_json(
        "/translate",
        serde_json::json!({ "text": "Hola", "from": "es-latam", "to": "es" }),
    ))
    .await;
    assert_eq!(status, StatusCode::OK);
    let actual: Value = serde_json::from_slice(&bytes).expect("respuesta JSON");
    assert_eq!(actual["translated"], Value::String("Hola".to_string()));
}
