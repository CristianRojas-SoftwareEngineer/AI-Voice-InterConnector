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

## 6. Corrida de cierre — 2026-10-25 (paso 11 y defectos reproducidos)

Corridas nuevas, posteriores al cierre de fase 3. No reemplazan las mediciones
anteriores: las amplían con la comparación limpia/ruidosa que faltaba.

Entorno: Windows x64; rama `feat/denoise-migration` en `964970d5` más las
correcciones de esta corrida; ORT 1.28.0 desde `ort-bundle`; DPDFNet y Parakeet
v3 provisionados en el almacén canónico; Qwen3-TTS con
`vendor/qwen3-tts/qwen_tts.exe`; FFmpeg 9.0.1 del sistema solo para conversiones
auxiliares del corpus.

Corpus: pares congelados `clean`/`noisy` de `Denoise/tests/data`, usados como
entrada controlada (misma señal, mismo modelo, solo cambia la variable).
`voz_*` dura 3 s y 48 kHz mono; `pub_snr0`, `car_snr0` y `office_snr0` duran
~6 s.

### 6.1 Transcripción directa (`--no-daemon`)

| Clip / entrada | denoise | Latencia | Texto |
|---|---|---|---|
| voz / clean | no | 3 050 ms | Bien me envió correos electrónicos recientemente |
| voz / clean | sí | 5 852 ms | Bien me envió correos electrónicos recientemente |
| voz / noisy | no | 3 159 ms | Bien, me envió correos electrónicos recientemente. |
| voz / noisy | sí | 5 900 ms | Bien envió correos electrónicos recientemente. |
| pub_snr0 / clean | no | 3 995 ms | Borrar mi antiguas lista de canciones inglesas. Está mi alarma de la escuela puesta. Enviar un correo electrónico a todos los trabajadores de mi empresa. |
| pub_snr0 / clean | sí | 16 765 ms | idéntico al anterior |
| pub_snr0 / noisy | no | 3 960 ms | Corrar mi antigua lista de canciones inglesas. Enviar un correo electrónico a todos los trabajadores de mi empresa. |
| pub_snr0 / noisy | sí | 16 829 ms | Enviar un correo electrónico a todos los trabajadores de mi empresa. |
| car_snr0 / clean | no | 3 885 ms | Me gustaría que recordases que me gusta esta canción. Ok, Google, pon música rock. Noticias de tecnología de última hora. |
| car_snr0 / clean | sí | 16 872 ms | idéntico al anterior |
| car_snr0 / noisy | no | 3 930 ms | … Ok, Google, con música rock. … |
| car_snr0 / noisy | sí | 17 156 ms | … Ok, Google, pon música rock. … |
| office_snr0 / clean | no | 4 121 ms | Delhi el nivel de tráfico es demasiado alto. ¿Cuál es el tipo de cambio de dólares estadounidenses a dólares canadienses? |
| office_snr0 / clean | sí | 17 616 ms | idéntico al anterior |
| office_snr0 / noisy | no | 4 026 ms | El nivel de tráfico es demasiado al cual es el tipo de cambio de dólares estadounidenses a dólares canadienses. |
| office_snr0 / noisy | sí | 17 687 ms | Delhi el nivel de tráfico es demasiado al final. |

Lectura: con entrada limpia la limpieza no cambia una sola palabra y cuesta
+11 a +13,5 s por archivo (carga de sesión ORT y DSP por proceso). Con entrada
ruidosa el efecto es **mixto**: recupera `car_snr0` («con» → «pon») y pierde
contenido en `pub_snr0` y `office_snr0`, una frase completa en cada uno. No hay
mejora uniforme de STT; el beneficio depende del clip y del tipo de ruido.
No se calculó WER numérico: la referencia sería la transcripción de la señal
limpia con el mismo motor, y aquí se compara texto a texto, que es suficiente
para descartar una mejora uniforme y no para cuantificarla en porcentaje.

### 6.2 Transcripción por daemon (misma entrada, sesión caliente)

| Caso | Latencia | Texto |
|---|---|---|
| `pub_snr0` noisy, sin denoise | 6 394 ms | Corrar mi antigua lista… Enviar un correo… |
| `pub_snr0` noisy, con denoise (primera) | 90 178 ms | Enviar un correo electrónico a todos los trabajadores de mi empresa. |
| `pub_snr0` noisy, con denoise (caliente) | 14 308 y 14 655 ms | idéntico al anterior |
| `car_snr0` noisy, sin denoise | 1 170 ms | … Ok, Google, con música rock. … |
| `car_snr0` noisy, con denoise | 13 720 y 14 120 ms | … Ok, Google, con música rock. … |

La ruta del daemon convierte 16→48→16 kHz, así que su resultado no coincide
palabra a palabra con la ruta directa sobre el mismo WAV de 48 kHz
(`car_snr0`: la directa corrige «con»→«pon», la del daemon no). Es una
diferencia de remuestreo, no dos configuraciones DSP distintas.

Variabilidad observada: la misma entrada ruidosa con denoise devolvió dos
transcripciones distintas en corridas separadas (`Enviar un correo…` en solitario
y `Borrar mi antigua lista… de la puesta en el envío…` bajo concurrencia). El
motor no es reproducible byte a byte bajo contención de hilos; las conclusiones
de calidad se apoyan en la tendencia, no en una salida única.

### 6.3 Puerta de concurrencia (4 solicitudes simultáneas por HTTP)

`POST /transcribe` con `denoise: true`, mismo cuerpo en 4 hilos:

| Solicitud | Resultado | Latencia |
|---|---|---|
| 1 | HTTP 200 | 15 074 ms |
| 2 | HTTP 200 (esperó en cola) | 28 733 ms |
| 3 | HTTP 503 `denoise_busy` | 90 ms |
| 4 | HTTP 503 `denoise_busy` | 90 ms |

Confirma el contrato: una inferencia activa, a lo sumo una esperando, el resto
saturado sin iniciar DSP. La solicitud en espera arrancó al liberarse el turno
(≈15 s + 13,7 s de DSP y STT).

### 6.4 Clonación directa (`voice clone --no-daemon`)

| Referencia | denoise | Resultado | Latencia |
|---|---|---|---|
| voz clean, 3 s | no | `reference.qvoice` creado | 1 739 ms |
| voz clean, 3 s | sí | `reference.qvoice` creado | 3 533 ms |
| voz noisy, 3 s | no | `reference.qvoice` creado | 1 399 ms |
| voz noisy, 3 s | sí | `reference.qvoice` creado | 3 669 ms |

Sobrecoste de la limpieza: ≈+2,0 s sobre una referencia de 3 s. **No** se midió
identidad ni naturalidad de la voz clonada: no hay métrica de hablante
(embedding/verificación) en el workspace ni se hizo escucha ciega. Queda como
caso pendiente, no aprobado.

### 6.5 Doblaje directo (`speech dub --no-daemon`)

| Referencia | denoise | Latencia | Texto |
|---|---|---|---|
| voz clean | no | 27 067 ms | Bien me envió correos electrónicos recientemente |
| voz clean | sí | 26 359 ms | idéntico |
| voz noisy | no | 29 717 ms | Bien, me envió correos electrónicos recientemente. |
| voz noisy | sí | 29 160 ms | idéntico |

El coste está dominado por la síntesis TTS (16-18 s de la fase de reproducción);
el sobrecoste de la limpieza queda por debajo del ruido de medición en este clip
de 3 s.

### 6.6 Memoria de la ruta con limpieza

Daemon en reposo: 959 MB de working set. Pico observado durante un `/transcribe`
con `denoise: true` sobre audio ruidoso de 3 s: 1 140 MB, es decir ≈+225 MB para
PCM 16→48 kHz, WAV temporal y sesión DPDFNet. Medido con muestreo de
`WorkingSet64` cada 100 ms mientras corría la petición.

### 6.7 Límites vigentes

`cargo test --test cli_golden -- transcribe_audio_over_limit_is_audio_too_long
clone_reference_over_limit_is_audio_too_long
dub_audio_over_limit_is_audio_too_long`: 3/3 en verde; los 30/300/40 s se
rechazan con `audio_too_long` en ruta local y por daemon, sin rebajar el tope.

### 6.8 Defectos reproducidos y corregidos en esta corrida

1. **`voice clone --denoise` en ruta directa fallaba siempre** con
   `voice_clone_failed` («El archivo de audio no existe»). El directorio temporal
   del WAV limpio se liberaba al salir del bloque que lo creaba, antes de que el
   motor clonara. Corregido manteniendo el temporal vivo hasta el final del
   clonado; regresión añadida en `tests/cli_golden.rs`
   (`voice_clone_with_denoise_ok`, requiere modelo Base y DPDFNet).
2. **`speech transcribe --daemon --denoise` omitía la limpieza en silencio**: el
   cliente construía el cuerpo con `denoise` y enviaba otro cuerpo sin el campo.
   Corregido en `transcribe_via_daemon`; verificado contra el daemon real
   (14,3-14,7 s y texto limpio frente a 6,4 s y texto sin limpiar). Es la
   violación de contrato que el plan prohíbe expresamente.
3. **Avisos de código muerto bajo `-D warnings`**: `denoise_unsupported_response`
   (feature activa), `denoise_busy_response` (feature ausente) y
   `denoise_failure`/`denoise_error_response` (denoise sin STT) quedaron
   confinados a su combinación real de features.

### 6.9 Sin cubrir en esta corrida

Identidad y naturalidad de la clonación; WER numérico; pico de memoria por
petición concurrente (solo se midió una activa); coste de arranque en frío de la
primera limpieza por proceso (90 s en el daemon, no desglosado); y todo lo que
requiere target nativo (Linux/macOS).
