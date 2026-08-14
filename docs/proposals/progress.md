# Progreso de Migración — AI-Voice-InterConnector

**Plan de referencia:** [`PLAN-DE-MIGRACIÓN.md`](./PLAN-DE-MIGRACIÓN.md)  
**Última actualización:** 2026-08-13

---

## Resumen ejecutivo

| Fase | Descripción | Estado |
|------|-------------|--------|
| Fase 0 | Fundamentos y validación de integración | ✅ Desbloqueada |
| Fase 1 | Host Rust — almacenes, CLI, config | ✅ Completada |
| Fase 2 | Audio (CPAL) | ✅ Completada |
| Fase 3 | STT nativo (`ct2rs::Whisper`) | ✅ Completada |
| Fase 4 | Traducción nativa (`ct2rs::Translator`) + segmentación | ✅ Completada |
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

**Estado:** ✅ Desbloqueada

Ejecutada mediante la orquestación `fase0-desbloqueo`. El árbol Rust es hoy un **cargo workspace** (paquete
raíz `ai-voice-interconnector` = binario CLI en `src/main.rs`, más ocho crates bajo `crates/`:
`avi-core`, `avi-audio`, `avi-tts`, `avi-store`, `avi-config`, `avi-daemon`, `avi-stt`, `avi-translation`).
`ct2rs` está cableado en `avi-stt` y compila CTranslate2 desde fuente (backend CPU `ruy`). La verificación
de terreno real (F5) dejó la suite del workspace en verde (18 tests) y ejercitó cada pieza externa contra
pesos reales.

| Ítem | Estado | Detalle |
|------|--------|---------|
| Validar integración subprocess del motor Qwen (texto → WAV) | ✅ | Smoke real de `qwen_tts.exe`: WAV PCM 16-bit mono 24000 Hz |
| Validar `ct2rs` (Whisper + Translator) contra modelos ya convertidos | ✅ | Tests de carga en `avi-stt`: opus-mt es↔en + Whisper small, salida no vacía |
| Build nativo del motor Qwen en Windows (MinGW-w64/UCRT64) | ✅ | `qwen_tts.exe` estático (MSYS2/UCRT64, gcc 16.1.0, shims POSIX) |
| Crear workspace Rust (cargo workspace, crates por subsistema) | ✅ | Raíz + 8 crates `avi-*`; `default-members = [".", "crates/*"]` |
| Harness de tests de contrato dorados (captura del oráculo Python) | ✅ | `crates/avi-daemon/tests/golden.rs` (5) + `tests/cli_golden.rs` (6), fixtures en `tests/golden/` |

**Prerequisitos de build (F5):** CMake en el `PATH` + compilador C++ de MSVC (para el build de CTranslate2
vía `ct2rs`); `.cargo/config.toml` fuerza `+crt-static` en `x86_64-pc-windows-msvc` para alinear el CRT del
workspace con el de CTranslate2 (`/MT`).

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
| Comandos sin inferencia nativos (`voice list/remove`, `speech list/play`, `devices`, `version`) | ✅ | `src/main.rs`, `crates/avi-audio/src/lib.rs` |

---

## Fase 2 — Audio (CPAL)

**Estado:** ✅ Completada

`AudioService` unifica playback, captura y conversión en un único backend CPAL, eliminando la fragmentación por SO del stack Python. Playback y enumeración se validaron de primera mano contra el oráculo Python (paridad exacta del contrato `devices`: mismos id/name/latency/schema_version). El playback y la captura ramifican por `SampleFormat` (F32/I16/U16). La cadena de conversión (`to_mono`/`resample_linear`/`f32_to_i16`) está cubierta por tests unitarios deterministas (4/4 en verde).

| Ítem | Estado | Archivo |
|------|--------|---------|
| `AudioService::play_wav` — reproducción WAV vía CPAL + hound (F32/I16/U16) | ✅ | `crates/avi-audio/src/lib.rs` |
| `AudioService::capture_16k_mono_pcm` — captura micrófono vía CPAL (16kHz/mono/int16) | ✅ | `crates/avi-audio/src/lib.rs` |
| Helpers de conversión (`to_mono`, `resample_linear`, `f32_to_i16`) | ✅ | `crates/avi-audio/src/lib.rs` |
| Enumeración de dispositivos de salida con latencia (`list_output_devices`) | ✅ | `crates/avi-audio/src/lib.rs` |

> **Actualización (Fase 3):** `capture_16k_mono_pcm` ya está cableada a `speech transcribe --mic` (junto con `load_wav_16k_mono_pcm` para `--audio`); la captura real de micrófono se verificó de extremo a extremo contra el STT nativo. La paridad textual exacta de la transcripción contra el oráculo Python queda pendiente (ver Fase 3 más abajo).

---

## Fase 3 — STT nativo (`ct2rs::Whisper`)

**Estado:** ✅ Completada

`Ct2SttEngine` (sobre `ct2rs::Whisper`) está implementado y cableado a `speech transcribe`, con superficie
CLI `--audio`/`--mic`/`--duration`/`--source-language`, contrato JSON `{text, source}` (envuelto en
`schema_version = "3"`) y exit codes 2 (argumentos inválidos), 4 (modelo ausente) y 10 (fallo de
transcripción). La paridad contra el oráculo Python quedó cerrada: se provisionó el modelo del oráculo
(`setup --with-stt` → `faster-whisper-small`) y se construyó un corpus de 4 pares `(WAV, fixture)` con
audios reales — el WAV sintético existente más 3 audios nuevos generados con el motor Qwen3-TTS del
propio repositorio (remuestreados a 16 kHz mono int16) — y transcripciones emitidas por el oráculo
(`TranscriptionService` de producción). El test de paridad (antes `#[ignore]`) quedó activo: WER ≤ 0.05
por ítem, 4/4 en verde.

| Ítem | Estado |
|------|--------|
| Verificar disponibilidad de `ct2rs` / CMake / CTranslate2 en entorno build | ✅ |
| Implementar `Ct2SttEngine` sobre `ct2rs::Whisper` | ✅ |
| Integrar con captura/carga de audio (pipeline `--mic`/`--audio` → transcripción) | ✅ |
| Validar contra oráculo Python (WER ≤ 0.05, corpus de 4 audios) | ✅ |

---

## Fase 4 — Traducción nativa (`ct2rs::Translator`) + segmentación

**Estado:** ✅ Completada

`Ct2TranslationEngine` (sobre `ct2rs::Translator`) está implementado y cableado a `translate`, con
segmentación jerárquica `HierarchicalSegmenter` en `avi-core`, pipeline `segmentar → traducir
por lotes por párrafo (tope 10) → ensamblar` con passthrough intacto, contrato JSON (envuelto
en `schema_version = "3"`) y exit codes 2, 4 y 9. La paridad funcional contra el oráculo Python quedó cerrada: los modelos `opus-mt-{es-en,en-es}` se
reconvirtieron a CT2 int8 replicando el flujo de conversión del oráculo (`_convert_translation_model`,
pesos byte-idénticos a su deployment; no commiteados, regenerables vía `setup`), y se construyó un corpus
de 11 pares `{input, expected}` (5 es→en, 6 en→es) sobre textos reales del repositorio, emitidos por el
`TranslationService` de producción. El test de paridad (antes `#[ignore]`) quedó activo con criterio
FUNCIONAL (decisión del equipo: la migración busca calidad y eficiencia, no clonar bytes del oráculo):
WER medio de corpus ≤ 0.35, tope por ítem ≤ 0.6 y checks de calidad (salida no vacía, sin `</s>`, sin
`<unk>`). 5/5 en verde; WER medio real 0.19, atribuible a varianza de paráfrasis válida (p. ej. «Don't»
vs «Do not», «a watermark» vs «any watermark»), no a divergencia funcional. Mejora de calidad aplicada
sobre el default de ct2rs: `disable_unk=true` suprime `<unk>` crudo en la salida (default sano del
oráculo). La pieza original «reimplementar `sacremoses` en Rust» quedó descartada: el oráculo Python no
la usa en su camino de ejecución (SentencePiece crudo + token `</s>` manual).

Optimización de rendimiento (lote por párrafo): cada párrafo se traduce en una sola llamada
`translate_batch` (constante `MAX_ORACIONES_POR_LOTE = 10`; los párrafos de más de 10 oraciones se
parten en grupos de 10), con el mismo segmentador, el mismo reensamblado y la misma API pública —
salida idéntica a la anterior. Speedup medido a nivel motor (release, modelo cargado una vez,
mediana de 5): **2.71x** en 5 oraciones y **~3.6x** en 10. El speedup no es de nivel pipeline:
`translate` construye un `Ct2TranslationEngine` nuevo por llamada (recarga del modelo ~200 ms,
preexistente, fuera de alcance).

| Ítem | Estado |
|------|--------|
| Implementar `Ct2TranslationEngine` sobre `ct2rs::Translator` (Marian, es↔en) | ✅ |
| Implementar segmentador jerárquico Rust (`HierarchicalSegmenter`, reemplaza pysbd) | ✅ |
| Validar segmentador contra corpus pysbd (estructural, 6 tests) | ✅ |
| Pipeline completo: segmentar → traducir por lotes por párrafo (tope 10) → ensamblar con passthrough | ✅ |
| Paridad funcional contra oráculo Python (WER medio corpus 0.19 ≤ 0.35, 11 ítems) | ✅ |
| Optimización de traducción por lotes por párrafo (tope 10): speedup a nivel motor 2.71x (5 oraciones) y ~3.6x (10) | ✅ |

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
| `ct2rs` | Bindings de CTranslate2 (STT/traducción, backend CPU `ruy`); compila desde fuente |

---

## Próximos pasos

1. **Fase 5:** integración real del motor Qwen3-TTS y build nativo Windows.
2. **Fase 6:** implementar precarga + warmup al arranque del daemon.
3. **Fase 7:** CI multi-SO y retiro del código Python.
