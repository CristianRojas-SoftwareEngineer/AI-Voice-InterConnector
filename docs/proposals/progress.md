# Progreso de Migración — AI-Voice-InterConnector

**Plan de referencia:** [`PLAN-DE-MIGRACIÓN.md`](./PLAN-DE-MIGRACIÓN.md)  
**Última actualización:** 2026-08-12

---

## Resumen ejecutivo

| Fase | Descripción | Estado |
|------|-------------|--------|
| Fase 0 | Fundamentos y validación de integración | ⏳ Pendiente |
| Fase 1 | Host Rust — almacenes, CLI, config | ✅ Completada |
| Fase 2 | Audio (CPAL) | ✅ Completada |
| Fase 3 | STT nativo (`ct2rs::Whisper`) | ⏳ Pendiente |
| Fase 4 | Traducción nativa (`ct2rs::Translator`) + segmentación | ⏳ Pendiente |
| Fase 5 | TTS nativo (Qwen3-TTS subprocess) | 🔶 Parcial |
| Fase 6 | Daemon (Axum) + streaming + warmup | 🔶 Parcial |
| Fase 7 | Empaquetado, cutover y retiro de Python | 🔶 Parcial |
| Transversales | Tracing, UTF-8, SIGINT, despacho de modos | ✅ Completadas |

---

## Preocupaciones transversales

| Ítem | Estado | Archivo |
|------|--------|---------|
| Inicializar `tracing_subscriber` en main | ✅ | `src/main.rs` |
| Forzar UTF-8 en stdout/stderr (`force_utf8()`) | ✅ | `src/main.rs` |
| Handler de SIGINT → exit code 130 (`ctrlc`) | ✅ | `src/main.rs` |
| Despacho de tres modos (`--daemon` / `--no-daemon` / auto) | ✅ | `src/main.rs` |

---

## Fase 0 — Fundamentos y validación de integración

**Estado:** ⏳ Pendiente

Fase de andamiaje. No se ha ejecutado formalmente; algunas tareas dependientes están siendo cubiertas de manera incremental en otras fases, pero la validación de integración formal no ha comenzado.

| Ítem | Estado |
|------|--------|
| Validar integración subprocess del motor Qwen (HTTP / PCM stdout) | ⏳ |
| Validar `ct2rs` (Whisper + Translator) contra modelos ya convertidos | ⏳ |
| Build nativo del motor Qwen en Windows (MinGW-w64/UCRT64) | ⏳ |
| Crear workspace Rust (cargo workspace, crates por subsistema) | ⏳ |
| Harness de tests de contrato dorados (captura del oráculo Python) | ⏳ |

---

## Fase 1 — Host Rust (paridad de superficie, motores aún delegados)

**Estado:** ✅ Completada

La infraestructura de almacenes y configuración está implementada. La superficie CLI completa (clap, 9 grupos de comandos), la taxonomía de errores (`thiserror`) y el emisor JSON único (`schema_version = "3"`) también están completos y verificados con `cargo build`/`cargo test` (4/4) y smokes reales de CLI y daemon.

| Ítem | Estado | Archivo |
|------|--------|---------|
| `VoiceStore` — registro de voces usuario/fábrica, layout en disco | ✅ | `src/store.rs` |
| `SpeechStore` — WAV + sidecar de metadatos por (voz, etiqueta) | ✅ | `src/store.rs` |
| `ModelStore` — gestión de modelos, revisiones pinneadas | ✅ | `src/store.rs` |
| `ModelStore::register_provisioned` | ✅ | `src/store.rs` |
| `AppConfig` — configuración TOML (`serde` + `toml`) | ✅ | `src/config.rs` |
| Superficie CLI completa con `clap` (9 grupos de comandos) | ✅ | `src/main.rs` |
| Taxonomía de errores → exit codes (`thiserror`) | ✅ | `src/exit_codes.rs` |
| Emisor JSON único (`schema_version = "3"`) | ✅ | `src/json_emitter.rs`, `src/daemon.rs` |
| Comandos sin inferencia nativos (`voice list/remove`, `speech list/play`, `devices`, `version`) | ✅ | `src/main.rs`, `src/audio.rs` |

---

## Fase 2 — Audio (CPAL)

**Estado:** ✅ Completada

`AudioService` unifica playback, captura y conversión en un único backend CPAL, eliminando la fragmentación por SO del stack Python.

| Ítem | Estado | Archivo |
|------|--------|---------|
| `AudioService::Playback` — reproducción WAV vía CPAL + hound | ✅ | `src/audio.rs` |
| `AudioService::Capture` — captura micrófono vía CPAL (16kHz/mono/int16) | ✅ | `src/audio.rs` |
| `AudioConverter` — conversión a mono, resample lineal, f32_to_i16 | ✅ | `src/audio.rs` |
| Latencia real de dispositivos en `list_output_devices` | ✅ | `src/audio.rs` |

---

## Fase 3 — STT nativo (`ct2rs::Whisper`)

**Estado:** ⏳ Pendiente

Bloqueada por la necesidad de configurar el entorno de build de `ct2rs` (CMake + CTranslate2).

| Ítem | Estado |
|------|--------|
| Verificar disponibilidad de `ct2rs` / CMake / CTranslate2 en entorno build | ⏳ |
| Implementar `Ct2SttEngine` sobre `ct2rs::Whisper` | ⏳ |
| Integrar con `AudioConverter` (pipeline captura → transcripción) | ⏳ |
| Validar contra oráculo Python (WER ≈ 0 sobre corpus de referencia) | ⏳ |

---

## Fase 4 — Traducción nativa (`ct2rs::Translator`) + segmentación

**Estado:** ⏳ Pendiente

Depende del entorno CT2 establecido en la Fase 3.

| Ítem | Estado |
|------|--------|
| Implementar `Ct2TranslationEngine` sobre `ct2rs::Translator` (Marian) | ⏳ |
| Reimplementar `sacremoses` en Rust (normalización / truecase) | ⏳ |
| Implementar segmentador determinista Rust (reemplaza pysbd) | ⏳ |
| Validar segmentador contra corpus pysbd | ⏳ |
| Pipeline completo: segmentar → traducir → ensamblar con passthrough | ⏳ |

---

## Fase 5 — TTS nativo (Qwen3-TTS subprocess)

**Estado:** 🔶 Parcial

Las abstracciones de dominio (traits, tipos) están definidas. La integración real con el subproceso de inferencia y el build nativo Windows están pendientes.

| Ítem | Estado | Archivo |
|------|--------|---------|
| `TtsEngine` trait y tipos públicos (`VoiceProfile`, `GenerationOptions`, `ProsodyOptions`) | ✅ | `src/tts.rs` |
| Estructura e IPC: cliente HTTP local + subprocess PCM por stdout | ✅ | `src/tts.rs` |
| `EmotionOptions` (API por extensibilidad, no-op en modelo 0.6B) | ✅ | `src/tts.rs` |
| Integración real con subproceso de inferencia Qwen3-TTS (weights) | ⏳ | |
| Build nativo Windows del motor (MinGW-w64/UCRT64, shims POSIX) | ⏳ | |
| Migrar clonado de voz (timbre → `.qvoice`) | ⏳ | |
| Portar bypass de watermark y su documentación ética | ⏳ | |

---

## Fase 6 — Daemon (Axum) + streaming + warmup

**Estado:** 🔶 Parcial

El servidor Axum con las rutas principales está operativo. La precarga + warmup al arranque es la pieza faltante.

| Ítem | Estado | Archivo |
|------|--------|---------|
| Servidor HTTP Axum en `127.0.0.1:8765` | ✅ | `src/daemon.rs` |
| `GET /voices` | ✅ | `src/daemon.rs` |
| `POST /voices/precompute` | ✅ | `src/daemon.rs` |
| `POST /shutdown` | ✅ | `src/daemon.rs` |
| `POST /transcribe` | ✅ | `src/daemon.rs` |
| `POST /synthesize` con streaming NDJSON de progreso | ✅ | `src/daemon.rs` |
| Handshake de `schema_version` estricto | ✅ | `src/daemon.rs` |
| Serialización de síntesis (`synthesis_lock`) | ✅ | `src/daemon.rs` |
| Recepción de PCM int16 en base64 del cliente | ✅ | `src/daemon.rs` |
| Precarga de pesos + warmup de inferencia al arranque | ⏳ | |

---

## Fase 7 — Empaquetado, cutover y retiro de Python

**Estado:** 🔶 Parcial

El pipeline de empaquetado y la provisión de modelos nativa están operativos. CI multi-SO y retiro Python son los ítems pendientes.

| Ítem | Estado | Archivo |
|------|--------|---------|
| Pipeline de empaquetado nativo (binarios release + dist/) | ✅ | `scripts/build_release_native.py` |
| Provisión de modelos nativa en `setup` (manifiestos `manifest.json`) | ✅ | `src/main.rs`, `src/store.rs` |
| Preservar GPLv3, `THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md` | ✅ | |
| CI para 3 SO Tier 1 (Windows, Linux, macOS) | ⏳ | |
| Retiro formal del código Python | ⏳ | |

---

## Dependencias de Rust añadidas

| Crate | Propósito |
|-------|-----------|
| `tokio` | Runtime async |
| `axum` | Servidor HTTP del daemon |
| `cpal` | Audio I/O unificado (WASAPI / CoreAudio / ALSA) |
| `hound` | Codificación/decodificación WAV |
| `serde` / `serde_json` | Serialización y emisión NDJSON |
| `toml` | Lectura de configuración TOML |
| `reqwest` | Cliente HTTP bloqueante (IPC con daemon y motor Qwen) |
| `tokio-stream` | Streaming NDJSON asíncrono |
| `ctrlc` | Handler de SIGINT → exit 130 |
| `windows-sys` | Forzar UTF-8 (`chcp 65001`) en Windows |
| `tracing` / `tracing-subscriber` | Logging y diagnósticos estructurados |
| `thiserror` / `anyhow` | Taxonomía de errores de dominio |
| `clap` | Superficie CLI |

---

## Próximos pasos

1. **Fase 0 formal:** ejecutar la validación de integración de `ct2rs` y del motor Qwen antes de avanzar a Fases 3–5.
2. **Fase 3:** configurar entorno CMake/CTranslate2 e implementar `Ct2SttEngine`.
3. **Fase 4:** implementar segmentador Rust y `Ct2TranslationEngine`.
4. **Fase 5:** integración real del motor Qwen3-TTS y build nativo Windows.
5. **Fase 6:** implementar precarga + warmup al arranque del daemon.
6. **Fase 7:** CI multi-SO y retiro del código Python.
