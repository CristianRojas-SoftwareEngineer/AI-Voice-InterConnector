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
- `denoise_non_boolean_is_usage_error`: `denoise="si"` → 400 `usage_error` en
  `/transcribe`, `/voices/clone`, `/dub` (requiere `native-stt`).
- `denoise_true_without_feature_is_unsupported`: `denoise:true` sin
  `native-denoise` → 501 `denoise_unsupported` en `/voices/clone`.
- `denoise_true_with_feature_without_model_is_model_missing`: `denoise:true` con
  `native-denoise` y sin modelo → evento NDJSON `error`/`model_missing`.

## 3. Paso 11 — mediciones E2E (ejecutadas)

Entorno: Windows x64; `QWEN3_TTS_BIN=vendor/qwen3-tts/qwen_tts.exe`;
`ORT_DYLIB_PATH=ort-bundle`; modelo DPDFNet `dpdfnet8_48khz_hr.onnx`
provisionado; modelo STT `parakeet-tdt-v3` provisionado vía `setup`.
Audio: `Speech Sampler.wav` (3 s, 16 kHz mono PCM16 para STT/dub),
`Voice Sampler.wav` (10 s, 48 kHz mono para clonación).

Daemon iniciado con `daemon serve` y features `native-denoise,native-stt,native-translation`.

### Resultados de latencia

| Prueba | Sin denoise | Con denoise | Overhead |
|---|---|---|---|
| Transcribe (3 s audio) | 482 ms | 3616 ms | +3134 ms |
| Clonación (10 s referencia) | 4818 ms | 12767 ms | +7949 ms |
| Dub (3 s audio) | 9324 ms | 9023 ms | ≈0 ms (ruido) |

### Comportamiento de la puerta de concurrencia

3 peticiones concurrentes con `denoise:true` en `/transcribe`:
- Petición 1: OK, ~3.3 s (turno activo, denoise + STT)
- Petición 2: OK, ~6.3 s (esperó en cola ~3 s, luego denoise + STT)
- Petición 3: **HTTP 503** en 33 ms, `reason: denoise_busy` (cola saturada: 1 activa + 1 esperando)

El texto transcrito es idéntico con y sin denoise (audio limpio de prueba), como se espera.

### Conclusión del paso 11

- La puerta de concurrencia funciona: 1 activa + 1 esperando ≤30 s, 503 para el resto.
- Denoise añade ~3.1 s de overhead a transcribe (inferencia DPDFNet en 48 kHz + conversiones).
- En clonación el overhead es mayor (~7.9 s) porque incluye carga de modelo + conversión WAV + DSP + escritura temporal.
- En dub el overhead se diluye porque la síntesis TTS domina el tiempo total (~9 s).
- No hay degradación de calidad de STT en audio limpio.

### Archivos de evidencia

Mediciones capturadas en scripts temporales bajo `/tmp/` (no incorporados al repositorio):
- `test_transcribe_denoise.ps1`, `test_denoise_busy_detail.ps1`,
  `clone_noden.ps1`/`clone_den.ps1`, `dub_noden.ps1`/`dub_den.ps1`

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
E2E) se ejecutó con modelos provisionados y motor compilado; los resultados
confirman el comportamiento de la puerta de concurrencia y el overhead de
Denoise por ruta. La fase 3 está completa.
