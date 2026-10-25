# Fase 3 — Denoise opt-in en clonado, `/dub` y `/transcribe` (pasos 10-11)

Rama AVI: `feat/denoise-migration`. Fecha: 2026-10-24.
Criterio: preprocesamiento DPDFNet opt-in (`denoise: true` / `--denoise`) en
clonado, doblaje y transcripción, con puerta de concurrencia (1 activo + 1
esperando ≤30 s, 503 si está saturado), sin omitir la limpieza en silencio.

## 1. Qué se implementó

### Crate `denoise` (`crates/denoise/`)
- `src/service.rs` (nuevo): `DenoiseUseCase::{Clone,Stt}`,
  `ServiceModels::resolve()`, `denoise_wav_file()`, `denoise_pcm16_mono()`,
  `ResolvedProvider` (evita doble hash), `resample_pcm16`, `write_wav16_mono`,
  `read_wav16_mono`. Config privada por caso de uso:
  `dry_blend=0.0`, `pause_gate_db=-25.0`, `highpass=false`.
- `src/lib.rs`: `pub mod service;`.
- `Cargo.toml`: dependencia `avi-audio`.

### Daemon (`crates/avi-daemon/`)
- `src/denoise_adapter.rs` (nuevo): `DENOISE_QUEUE_TIMEOUT=30s`,
  `DenoiseGate` (`Mutex<GateInner>` + `Notify`), `GateBegin::{Acquired,Waiting,Busy}`,
  `DenoisePermit` (Drop libera y despierta), `DenoiseWaiter::wait_timeout`
  (Drop libera plaza al cancelar), tests unitarios de admisión/espera/timeout/cancelación.
- `src/lib.rs`: módulo/imports `#[cfg(feature = "native-denoise")]`;
  campo `denoise_gate: Arc<DenoiseGate>`; helpers `optional_denoise`,
  `denoise_unsupported_response` (501), `denoise_busy_response` (503),
  `denoise_failure` (model_missing 404 / invalid_audio 400 / io_error 500 /
  denoise_failed 500), `denoise_error_response`.
- `/voices/clone`: parse `denoise`; sin feature → 501; con feature, toma de
  turno antes de escribir el WAV de referencia, denoise dentro del stream
  NDJSON, turno liberado tras DSP (antes de TTS), error `model_missing`/
  `invalid_audio`/`io_error`/`denoise_failed` en evento NDJSON.
- `/dub`: parse `denoise`; sin feature → 501; con feature, denoise del PCM
  antes de STT dentro del stream NDJSON, turno liberado tras DSP.
- `/transcribe`: parse `denoise`; sin feature → 501; con feature, denoise del
  PCM antes de STT, turno liberado tras DSP, error mapeado como respuesta HTTP
  directa.
- `Cargo.toml`: dep opcional `denoise` + feature `native-denoise`.

### Workspace raíz (`Cargo.toml`)
- `native-denoise = ["dep:denoise", "avi-daemon/native-denoise"]`.

### CLI (`src/main.rs`)
- `--denoise` en `VoiceCommands::Clone`, `SpeechCommands::Transcribe`,
  `SpeechCommands::Dub` (`#[cfg(feature = "native-denoise")]`).
- Propagación `"denoise": true` en JSON a `/voices/clone`, `/dub`, `/transcribe`.
- Ruta directa (ForceDirect/sin daemon): denoise del WAV de referencia antes
  de `clone_voice`; denoise del PCM antes de `engine.transcribe` en transcribe.

### `crates/avi-core/src/exit_codes.rs`
- `REASON_CODES`: `denoise_busy` → `StateConflict` (6),
  `denoise_unsupported` → `Error` (1), `denoise_failed` → `Error` (1).
- `ORACLE` test actualizado (mismas entradas). Pruebas de conformidad pasan.

## 2. Pruebas ejecutadas

| Comando | Resultado |
|---|---|
| `cargo check` (default) | ✓ |
| `cargo check --features native-stt` | ✓ |
| `cargo check --features native-denoise,native-stt` | ✓ |
| `cargo test -p avi-core` | ✓ 23 passed |
| `cargo test -p avi-daemon` | ✓ 21 passed, 6 ignored |
| `cargo test -p avi-daemon --features native-denoise,native-stt` | ✓ 25 passed, 9 ignored |
| `cargo fmt --all -- --check` | ✓ limpio |
| `cargo clippy --all-targets --features native-denoise,native-stt` | ✓ 1 aviso esperado (`denoise_unsupported_response` sin feature) |

### Nuevas pruebas de contrato (`crates/avi-daemon/tests/golden.rs`)
- `denoise_no_booleano_es_usage_error`: `denoise="si"` → 400 `usage_error` en
  `/transcribe`, `/voices/clone`, `/dub` (requiere `native-stt`).
- `denoise_true_sin_feature_es_unsupported`: `denoise:true` sin
  `native-denoise` → 501 `denoise_unsupported` en `/voices/clone`.
- `denoise_true_con_feature_sin_modelo_es_model_missing`: `denoise:true` con
  `native-denoise` y sin modelo → evento NDJSON `error`/`model_missing`.

## 3. Paso 11 — mediciones E2E (pendiente de entorno)

El paso 11 requiere: modelo DPDFNet provisionado, motor Qwen3-TTS compilado,
medios de audio reales y ejecución E2E para medir latencia frío/caliente,
límite de cola de 30 s, pico de memoria por petición y concurrencia bajo cola
acotada. No se ejecutó en esta sesión por falta de modelos/entorno E2E.
Cuando se disponga del entorno, ejecutar:
- `cargo xtask build-engine` + provisionar modelo DPDFNet.
- Ejecutar clonado/doblaje/transcripción con y sin `--denoise` sobre audio
  ruidoso/limpio; medir latencia, WER de STT, identidad de clon y uso de
  memoria.
- Verificar que `denoise_busy` (503) se emite cuando la cola está saturada y
  que `denoise_unsupported` (501) se emite sin la feature.

## 4. Archivos modificados

- `crates/avi-core/src/exit_codes.rs` — razones y oráculo.
- `crates/avi-daemon/Cargo.toml` — dep/feature `native-denoise`.
- `crates/avi-daemon/src/denoise_adapter.rs` — puerta de concurrencia (nuevo).
- `crates/avi-daemon/src/lib.rs` — estado, handlers, helpers denoise.
- `crates/avi-daemon/tests/golden.rs` — 3 pruebas de contrato (nuevas).
- `crates/denoise/Cargo.toml` — dep `avi-audio`.
- `crates/denoise/src/lib.rs` — `pub mod service`.
- `crates/denoise/src/service.rs` — adaptador DSP (nuevo).
- `Cargo.toml` — feature `native-denoise` encadenada.
- `src/main.rs` — flags `--denoise`, propagación JSON, rutas directas.

## 5. Veredicto

El paso 10 está completo: código implementado, compilado y probado con y sin
`native-denoise`, `cargo fmt`/`clippy`/`test` en verde. El paso 11 (mediciones
E2E) está listo para ejecutarse cuando haya modelos y entorno de audio
disponibles.
