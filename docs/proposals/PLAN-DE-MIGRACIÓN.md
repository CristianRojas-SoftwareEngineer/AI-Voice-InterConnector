# Plan de Migración — AI-Voice-InterConnector: de runtime Python a runtime nativo Rust

**Fecha:** 2026-08-12
**Estado:** Especificación de diseño definitiva y plan de migración por fases.

## Introducción

AI-Voice-InterConnector es un componente de síntesis de voz (con clonación de voz), transcripción y
traducción, consumible como CLI desde cualquier lenguaje y opcionalmente como daemon local.
Hoy está implementado en Python y embarca motores nativos (Chatterbox para TTS, CTranslate2
para STT/traducción), lo que arrastra un intérprete de Python —y, en Windows, una ruta WSL2
para el motor de referencia— en producción.

Este documento es la **especificación de diseño definitiva** de su reescritura hacia un
runtime nativo en Rust que orquesta esos motores sin Python ni WSL, junto con el **plan por
fases** para llegar allí sin romper a sus consumidores. No narra el proceso de decisión:
describe el sistema objetivo y el camino de migración ya resueltos. Está organizado en tres
secciones —estado actual, sistema objetivo y proceso por fases— precedidas por el propósito,
las dos decisiones que el plan mantiene separadas y el invariante que protege a los
consumidores durante toda la transición.

## Tabla de contenidos

- [Introducción](#introducción)
- [Propósito y encuadre](#propósito-y-encuadre)
  - [Dos decisiones ortogonales (no confundirlas)](#dos-decisiones-ortogonales-no-confundirlas)
  - [Invariante rector: los contratos públicos se preservan](#invariante-rector-los-contratos-públicos-se-preservan)
- [Sección 1 — Estado actual (arquitectura y especificaciones funcionales)](#sección-1--estado-actual-arquitectura-y-especificaciones-funcionales)
  - [1.1 Identidad y empaquetado](#11-identidad-y-empaquetado)
  - [1.2 Superficie funcional (CLI)](#12-superficie-funcional-cli)
  - [1.3 Contratos públicos congelados](#13-contratos-públicos-congelados)
  - [1.4 Arquitectura interna por subsistema](#14-arquitectura-interna-por-subsistema)
  - [1.5 Dependencias de runtime declaradas](#15-dependencias-de-runtime-declaradas)
  - [1.6 Calidad, tests y CI](#16-calidad-tests-y-ci)
- [Sección 2 — Sistema objetivo (arquitectura, especificaciones, diseño e implementación)](#sección-2--sistema-objetivo-arquitectura-especificaciones-diseño-e-implementación)
  - [2.1 Principios rectores](#21-principios-rectores)
  - [2.2 Arquitectura objetivo](#22-arquitectura-objetivo)
  - [2.3 Stack tecnológico Rust](#23-stack-tecnológico-rust)
  - [2.4 Diseño por subsistema](#24-diseño-por-subsistema)
  - [2.5 Qué se preserva y qué cambia](#25-qué-se-preserva-y-qué-cambia)
- [Sección 3 — Proceso de migración por fases](#sección-3--proceso-de-migración-por-fases)
  - [Estrategia](#estrategia)
  - [Fase 0 — Fundamentos y validación de integración](#fase-0--fundamentos-y-validación-de-integración)
  - [Fase 1 — Host Rust (paridad de superficie, motores aún delegados)](#fase-1--host-rust-paridad-de-superficie-motores-aún-delegados)
  - [Fase 2 — Audio (CPAL)](#fase-2--audio-cpal)
  - [Fase 3 — STT (`ct2rs::Whisper`)](#fase-3--stt-ct2rswhisper)
  - [Fase 4 — Traducción (`ct2rs::Translator`) + segmentación](#fase-4--traducción-ct2rstranslator--segmentación)
  - [Fase 5 — TTS (Qwen3-TTS por subprocess)](#fase-5--tts-qwen3-tts-por-subprocess)
  - [Fase 6 — Daemon (Axum) + streaming + warmup](#fase-6--daemon-axum--streaming--warmup)
  - [Fase 7 — Empaquetado, cutover y retiro de Python](#fase-7--empaquetado-cutover-y-retiro-de-python)
  - [Preocupaciones transversales](#preocupaciones-transversales)
  - [Registro de riesgos (resumen)](#registro-de-riesgos-resumen)
- [Nota de alcance y honestidad](#nota-de-alcance-y-honestidad)

## Propósito y encuadre

Este documento define un plan **completo y accionable** para transformar AI-Voice-InterConnector
desde su implementación actual (aplicación Python que embarca motores nativos) hacia
un **runtime nativo en Rust** que orquesta motores de inferencia nativos, sin
intérprete de Python ni WSL en producción.

El plan integra dos cambios (detallados en «Dos decisiones ortogonales»): el **cambio de
motor** TTS (Chatterbox → Qwen3-TTS), respaldado por un benchmark de rendimiento en CPU
cuya evidencia se resume en §2.4, y el **cambio de lenguaje** del host (Python → Rust),
conservando los motores en sus backends nativos (Qwen3-TTS vía C, Whisper + traducción vía
CTranslate2, audio vía CPAL).

La Sección 1 describe el estado actual **verificado contra el código** del repositorio; la
Sección 2, el sistema objetivo; la Sección 3, el proceso de migración por fases.

### Dos decisiones ortogonales (no confundirlas)

Se trata de dos cambios independientes que conviene no confundir; el plan los mantiene
separados:

| Decisión | Naturaleza |
|---|---|
| **A. Cambio de motor** Chatterbox → Qwen3-TTS | Producto / calidad |
| **B. Cambio de lenguaje** Python → Rust | Arquitectura / plataforma |

B **no** requiere A: el host puede migrarse a Rust conservando incluso el motor
Chatterbox tras un worker/subprocess. El plan ejecuta ambos cambios, pero los mantiene en
fases distintas: A condiciona la fase de TTS por **riesgo de integración** (build nativo del
motor en Windows), y por eso el motor TTS es la última fase de la migración.

### Invariante rector: los contratos públicos se preservan

AI-Voice-InterConnector ya expone contratos congelados y estables entre lenguajes y SO. Son la
**red de seguridad** de toda la migración: cada componente Rust se acepta cuando
reproduce el contrato que hoy cumple su equivalente Python, con la versión Python como
**oráculo de comportamiento**. Los invariantes son:

- La superficie de comandos de la CLI y su semántica.
- El contrato de salida: datos por stdout, diagnósticos por stderr, UTF-8 forzado.
- Los **códigos de salida** `0–10` y `130` (contrato público documentado en `cli.py`).
- El **esquema JSON** `schema_version = "3"` de las salidas `--json`.
- El contrato HTTP del daemon (`127.0.0.1:8765`, rutas, streaming NDJSON, handshake de
  `schema_version`).
- El layout en disco del almacén de voces y de habla sintética.

---

## Sección 1 — Estado actual (arquitectura y especificaciones funcionales)

### 1.1 Identidad y empaquetado

| Aspecto | Detalle |
|---|---|
| Lenguaje | Python `>=3.13` |
| Licencia | GPL-3.0-or-later (con `SOURCE-OFFER.md` y `THIRD-PARTY-LICENSES.md` embarcados) |
| Build | `setuptools` / `pyproject.toml`; distribución binaria vía **PyInstaller `--onedir`** |
| Instalador | Windows vía Inno Setup (`scripts/create_installer_windows.py`) |
| Plataformas | Windows x64, Linux x64/ARM64, macOS ARM64 |
| Entry point | `ai-voice-interconnector = ai_voice_interconnector.cli:main` |
| Naturaleza | CLI consumible desde cualquier lenguaje vía `subprocess`; daemon opcional |

### 1.2 Superficie funcional (CLI)

La superficie es de **9 grupos de comandos** (verificada en el `argparse` de `cli.py`):

| Comando | Sub-acciones | Función |
|---|---|---|
| `speech` | `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `transcribe` | Síntesis, reproducción, doblaje voz→voz, almacén de locuciones, transcripción |
| `voice` | `list`, `clone`, `remove` | Gestión de voces clonadas (timbre + habla) |
| `translate` | — | Traducción texto→texto `es<->en` (sin audio) |
| `devices` | — | Enumera dispositivos de salida de audio |
| `doctor` | — | Diagnósticos de entorno |
| `setup` | — | Provisiona el runtime: chequeos + descarga de modelos (`--language`, `--with-stt`) |
| `cleanup` | — | Limpia modelos/caché |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon |
| `version` | — | Versión |

**Flujos compuestos relevantes:**

- `speech dub`: transcribe (archivo o micrófono) → traduce si el idioma hablado difiere
  del de síntesis → sintetiza con la voz → reproduce. Reutiliza las tres máquinas.
- `speech synthesize --play`: bucle interactivo (reproducir/aceptar/regenerar/descartar)
  antes de persistir.
- **Despacho de tres modos** (síntesis y transcripción): `--daemon` (exige daemon, exit 5
  si no está), `--no-daemon` (fuerza directo), sin flags (autodetección).

### 1.3 Contratos públicos congelados

**Códigos de salida** (un orquestador distingue la causa sin parsear texto):

| Code | Significado | | Code | Significado |
|---|---|---|---|---|
| 0 | éxito | | 6 | conflicto de estado (recurso ocupado, colisión) |
| 1 | error genérico | | 7 | no aplicable al contexto (solo lectura, plataforma) |
| 2 | entrada inválida | | 8 | precondición de entorno (credenciales, red, disco) |
| 3 | recurso no encontrado | | 9 | fallo del pipeline de traducción |
| 4 | modelo no provisionado (→ `setup`) | | 10 | fallo del pipeline de transcripción |
| 5 | daemon inalcanzable | | 130 | interrupción por el usuario (Ctrl+C) |

**Esquema JSON:** todos los payloads `--json` llevan `schema_version = "3"`, inyectado por
el único emisor `emit_json()`. Campo aditivo: añadir claves no incrementa la versión.

**Salida:** datos por stdout, diagnósticos/errores por stderr, stdout/stderr forzados a
UTF-8 en toda plataforma.

### 1.4 Arquitectura interna por subsistema

```
src/ai_voice_interconnector/
├── cli.py            # superficie CLI, validación cliente, despacho de tres modos
├── engine.py         # ChatterboxEngine (façade + composition root)
├── synthesis.py      # SynthesisOrchestrator (flujo de síntesis, ciclo del progress_cb)
├── model_loader.py   # carga de pesos (es-mx-latam / multilingüe base)
├── model_cache.py    # aliases, revisiones pinneadas, detección de caché HF
├── conditionals.py   # ConditionalsPreparer (precómputo/carga de conditionals.pt)
├── compute_backend.py# ComputeBackendResolver (auto/cpu/cuda/mps)
├── audio.py          # AudioPlayer, AudioRecorder, enumeración de dispositivos
├── audio_writer.py   # ensamblado WAV
├── voices.py         # registro de voces (usuario/fábrica), validación de nombres
├── synthetic_speech.py# almacén de locuciones (WAV + sidecar de metadatos)
├── translation/      # segmenter, translator, assembler, service, model_loader
├── transcription/    # model_loader, transcriber, service
├── daemon/           # protocol, ipc, server, daemon, run
├── timing.py         # Spinner, StageTimer, SynthesisMetrics/Result, progress events
├── bootstrap.py      # UTF-8, warnings, env vars
├── exceptions.py     # taxonomía de errores de dominio
└── exit_codes.py     # códigos + CliError
```

#### TTS — `ChatterboxEngine`

- Motor: **Chatterbox Multilingual V3**, language pack **es-mx-latam** + inglés base.
- Arquitectura: **T3** (autoregresivo, ~0.4B) + **S3Gen** (vocoder de flow matching) +
  **VoiceEncoder**. Stack: PyTorch, SafeTensors, HuggingFace Hub.
- Parámetros optimizados propios: `max_new_tokens=500`, `n_cfm_timesteps=4`,
  `exaggeration=0.75`; defaults de síntesis por idioma (`SYNTHESIS_DEFAULTS`).
- **Bypass del watermark PerthNet** (decisión ética documentada, no una optimización).
- **Caché de motor en memoria** a nivel de clase (`get_instance`), reutilizado entre
  llamadas y por el daemon.
- **Conditionals precomputados a disco** (`conditionals.pt`) por voz; carga en vez de
  recomputar. Backends `cpu`/`cuda`/`mps`; configuración fina de PyTorch para CPU
  (`set_num_threads`, `OMP/MKL_NUM_THREADS`, oneDNN/MKLDNN, `flush_denormal`).
- Progreso por sub-etapa vía monkeypatch de `t3.inference`/`s3gen.inference` y shim de
  `tqdm` para conteo de tokens; cancelación cooperativa (`SynthesisCancelled`).

#### STT — transcripción

- Cadena: **faster-whisper `>=1.2.1` → CTranslate2 `>=4.8.1` → Whisper**.
- `WhisperTranscriber` recibe un modelo ya cargado y hace `transcribe(task="transcribe")`:
  **solo transcribe, nunca traduce**. Runtime de producción **exclusivamente CTranslate2**.
- Captura de micrófono vía **miniaudio (CFFI)**, 16 kHz / mono / int16; helper de
  remuestreo a 16 kHz para audio de archivo.

#### Traducción

- Cadena: **Marian / OPUS-MT → CTranslate2** (no usa un LLM). Par `es<->en`.
- Pipeline: valida → **segmenta (pysbd)** → traduce por segmento → ensambla; con
  **passthrough** cuando origen == destino (sin cargar modelo).
- Tokenización: SentencePiece crudo (`sentencepiece.SentencePieceProcessor` con el `.spm` del
  par) + token `</s>` anexado manualmente; `transformers`/`sacremoses` son dependencias
  declaradas, no parte del camino de ejecución (no se invoca `MarianTokenizer`).
- Modelos convertidos opus-mt→CT2 durante `setup` (`ctranslate2.converters.TransformersConverter`).

#### Segmentación

- **pysbd `>=0.3.4`** (segmentación de oraciones). Sin equivalente Rust maduro directo.

#### Audio (matriz por función y SO)

| Función | Windows | macOS | Linux |
|---|---|---|---|
| **Playback** | `winsound` (built-in) | `afplay` (built-in) | `sounddevice` → PortAudio |
| **Captura** | `miniaudio` (CFFI, backend único multiplataforma) | idem | idem |
| **Enumeración** | `pycaw` → Core Audio/COM | `sounddevice` → PortAudio | idem |

Es el subsistema más fragmentado (5–6 dependencias, ramas por SO) y el candidato más
claro a simplificación.

#### Daemon

- **FastAPI + Uvicorn**, HTTP sobre **loopback `127.0.0.1:8765`** (puerto fijo, sin
  `--port`; correr dos daemons no está soportado por diseño).
- Rutas: `GET /health`, `POST /synthesize` (progreso **streaming NDJSON**), `GET /voices`,
  `POST /voices/precompute`, `POST /shutdown`, `POST /transcribe`.
- Cliente IPC (`DaemonIPCClient`) vía `requests`; **handshake de `schema_version` exacto**
  (un daemon de otra versión se trata como no utilizable) y validación estricta del cuerpo
  de `/health` contra el modelo Pydantic (para no confundir otro servicio en el puerto).
- **Precarga de pesos + warmup de inferencia al arrancar** (paga la init perezosa del
  runtime). Síntesis **serializada** por `_synthesis_lock`. La captura de audio es
  **siempre de cliente**: el daemon recibe muestras PCM int16 en base64, nunca rutas.

#### Provisión de modelos y almacenes

- `setup`: chequeos de entorno + descarga desde HF Hub (revisiones **pinneadas**) +
  conversión opus-mt→CT2. Umbrales de disco/RAM advisory en `doctor`.
- **Registro de voces** (`voices.py`): voces de usuario vs fábrica, validación
  anti-escape de nombres, `default` de fábrica.
- **Almacén de habla sintética** (`synthetic_speech.py`): WAV + sidecar de metadatos por
  `(voz, etiqueta)`.

### 1.5 Dependencias de runtime declaradas

`chatterbox-tts`, `peft`, `numpy<2.5`, `torch` (transitiva), `ctranslate2`,
`faster-whisper`, `transformers`, `sentencepiece`, `sacremoses`, `pysbd`, `miniaudio`,
`sounddevice`, `pycaw` (win32), `fastapi`, `uvicorn`, `pydantic`, `requests`, `psutil`.

### 1.6 Calidad, tests y CI

- Suite de **~765+ tests** (pytest), con **gate de cobertura por módulo**
  (`scripts/check_coverage.py`; el `fail_under` global de coverage.py es insuficiente).
- CI con job de cobertura en Docker Linux; build PyInstaller; instalador Windows.
- El código ya separa explícitamente **engine / orchestrator / loaders / audio /
  transcripción / traducción / caché / CLI**: hay límites arquitectónicos reales que la
  migración puede conservar en vez de reescribir un monolito.

---

## Sección 2 — Sistema objetivo (arquitectura, especificaciones, diseño e implementación)

### 2.1 Principios rectores

1. **Nativo primero, no dogmático.** Rust para dominio/plataforma/orquestación; C/C++
   para cómputo intensivo (motores). No se reimplementan modelos en Rust.
2. **Preservación de contratos.** La superficie CLI, los exit codes, el esquema JSON y el
   contrato HTTP del daemon se conservan **byte-a-byte** salvo donde se declare lo
   contrario. Esto habilita migración incremental con la versión Python como oráculo.
3. **API modelada por conceptos, no por parámetros de Chatterbox.** El tipo público de
   síntesis expone `VoiceProfile`, `GenerationOptions`, `ProsodyOptions`,
   `EmotionOptions` — no `exaggeration`/`cfg_weight` crudos. El motor mapea a su
   semántica (reference audio, x-vector/ICL, temperature, top-k/p, seed, rate).
   **Restricción del modelo 0.6B:** en el motor C de Qwen, `--emotion` es *no-op* en el
   modelo 0.6B — la emoción no es una palanca de inferencia sino **una propiedad de la voz
   clonada** (se clona de un audio emocional y la voz hereda las emociones). Por tanto
   `EmotionOptions` no aplica al 0.6B; se conserva en la API por extensibilidad (1.7B), pero
   no debe prometer control emocional en el modelo objetivo.
4. **Un runtime nativo menos por eje.** STT y traducción comparten un único runtime
   CTranslate2 dentro del proceso Rust; el audio se unifica en un backend.

### 2.2 Arquitectura objetivo

```
                        ┌──────────────────────────────┐
                        │        AI-Voice-InterConnector (Rust)     │
                        │  CLI (clap) · Daemon (Axum)   │
                        │  Config · Model/Voice manager │
                        │  Audio manager · Streaming    │
                        │  Error taxonomy → exit codes  │
                        └───────────────┬──────────────┘
                                        │  (traits de dominio)
        ┌───────────────────────────────┼───────────────────────────────┐
        ▼                               ▼                               ▼
     AUDIO                             STT / Traducción                TTS
     CPAL                              ct2rs → CTranslate2         subprocess (HTTP/PCM)
        │                               │         │                    │
   WASAPI/CoreAudio/                 Whisper    Marian            Qwen3-TTS 0.6B
   ALSA/PipeWire                      (STT)   (es<->en)          (motor C, binario)
```

### 2.3 Stack tecnológico Rust

| Responsabilidad | Tecnología | Reemplaza |
|---|---|---|
| Runtime / async | Rust + Tokio | Python + asyncio |
| CLI | `clap` | `argparse` |
| HTTP (daemon) | `axum` | FastAPI + Uvicorn |
| Audio I/O (captura/playback/enum) | **CPAL** | miniaudio + winsound + afplay + sounddevice + pycaw |
| Decodificación/escritura audio | `hound` (WAV) / `symphonia` (si hiciera falta) | `wave` + numpy |
| Resampling / conversión | `rubato` o convertidor propio | remuestreo numpy |
| STT | `ct2rs::Whisper` → CTranslate2 | faster-whisper |
| Traducción | `ct2rs::Translator` → CTranslate2 (Marian) | transformers + CT2 |
| Segmentación | Segmentador determinista en Rust | pysbd |
| TTS | Qwen3-TTS 0.6B (motor C) + **subprocess** (HTTP / PCM por stdout) | ChatterboxEngine (PyTorch) |
| Serialización | `serde` / `serde_json` (NDJSON) | Pydantic + json |
| Config | `serde` + TOML | — |
| Logging / progreso | `tracing` | `timing.py` |
| Errores | `thiserror` (dominio) + `anyhow` (bordes) | `exceptions.py` |
| Tests | test stack nativo + tests de contrato dorados | pytest |
| Packaging | instaladores nativos por plataforma | PyInstaller + Inno Setup |

### 2.4 Diseño por subsistema

#### Host / runtime

- **Traits de dominio** que congelan las fronteras (permiten migrar motor por motor):
  `TtsEngine`, `SttEngine`, `TranslationEngine`, `Segmenter`, `AudioInput`,
  `AudioOutput`, `DeviceEnumerator`, `VoiceStore`, `ModelStore`.
- **Taxonomía de errores** (`thiserror`) que mapea 1:1 a los exit codes `0–10/130`. Es la
  traducción directa de `CliError`/`exit_codes.py`.
- **Emisor JSON único** que inyecta `schema_version` y garantiza un objeto por invocación
  (equivalente a `emit_json`).

#### TTS — `Qwen3TtsEngine`

- **Motor:** el proyecto de referencia es
  [`gabriele-mastrapasqua/qwen3-tts`](https://github.com/gabriele-mastrapasqua/qwen3-tts) —
  motor **C puro + BLAS**, pesos safetensors BF16 memory-mapped, modelos **0.6B/1.7B**,
  con **streaming a altavoz, servidor HTTP (`--serve`), clonado de voz y diseño de voz**
  en un solo binario (~200 KB). Correlación **0.999996** con la referencia Python en el
  pipeline completo. Modelo base: `Qwen/Qwen3-TTS-12Hz-0.6B-Base`.
- **GPU:** el motor incorpora backends **Metal y CUDA** (opt-in, `make metal` / `make cuda`);
  salir de PyTorch **no** implica quedarse solo en CPU. El objetivo CPU sigue siendo el caso
  base (donde Qwen ya domina el benchmark); cada ruta GPU se valida en la Fase 5.
- **Integración: subprocess.** El `Makefile` del upstream **no produce ninguna librería
  propia** (`.so`/`.dll`/`.a`): todos los targets compilan ejecutables. Al no existir
  `libqwen`, FFI exigiría mantener aguas arriba un target de librería y una API C. En cambio
  el binario ya ofrece IPC de fábrica: **servidor HTTP** (`/v1/tts`, `/v1/tts/stream`,
  `/v1/audio/speech`) y **PCM crudo por `--stdout`**. El `Qwen3TtsEngine` de Rust habla con
  el motor por **subprocess** (HTTP local o stdout) en los tres SO. El trait `TtsEngine`
  aísla esta decisión: si el upstream expusiera una librería, se migra a FFI sin tocar el host.
- **Rendimiento (evidencia que respalda el cambio de motor):** un benchmark propio,
  solo-CPU sobre la rama es-latam (AMD Ryzen 7 5825U 8c/16t, mediana de 3 corridas, warmup
  descartado, ejecución serializada), midió a Qwen3-TTS dominando todos los ejes de
  producción frente a Chatterbox:

  | Eje | Qwen3-TTS | Chatterbox | Ventaja |
  |---|---|---|---|
  | RTF (menor es mejor) | 1.31–1.73 | 6.56–7.97 | ~5× más rápido |
  | RAM pico | ~2.7 GB | ~7.1 GB | ~2.6× más liviano |
  | TTFA (primer audio) | ~1.0–1.6 s (streaming) | — (sin streaming) | latencia baja |
  | Footprint en disco | 2.38 GB | 3.06 GB | menor |
  | Clonado de voz (frío) | ~5 s | ~1.5 s | Chatterbox (coste único por sesión) |
  | Estabilidad | 12/12 | 12/12 | empate (24/24 OK) |

  El upstream reporta además RTF **0.52 (int4) / 0.69 (int8)** en M1 CPU (sub-tiempo real),
  consistente con el orden de magnitud. La medición comparó "motor listo para usar" (Qwen
  `--int4 --icl-only` vs Chatterbox por defecto), no misma precisión numérica. El **veredicto
  de calidad de voz** (naturalidad, timbre, acento, comportamiento cross-lingual) se decide
  por **escucha humana A/B**, fuera del alcance del benchmark de rendimiento.
- Separa **clonado** (captura de timbre, agnóstico al idioma, `.qvoice`) de **síntesis**
  (idioma como parámetro). El `VoiceProfile` de Rust encapsula el `.qvoice`; el
  `GenerationOptions` fija idioma y prosodia.
- Conserva la filosofía actual: **motor residente + reutilización de estado** (equivalente
  al caché de motor y al warmup del daemon).

##### Build nativo por plataforma

El motor compila **nativo en Linux y macOS (ARM/x86)** con OpenBLAS/Accelerate. En Windows
el `Makefile` del upstream es POSIX-only (`uname`, `-lpthread`, `-mavx2`) y solo documenta
una ruta WSL2; **WSL2 no se usa en producción**. La distribución en Windows se construye
**nativa con MinGW-w64/UCRT64 (MSYS2)**, con estas propiedades de diseño:

- **Shims POSIX acotados bajo `#ifdef _WIN32`** (vendoring local del motor, sin fork público;
  la licencia MIT lo permite): `mmap`/`munmap`/`pread`, `setenv`,
  `posix_fadvise`/`posix_madvise` (no-ops seguros, solo hints de caché) y `posix_memalign`
  (en `qwen_tts_kernels.h` y `third_party/ingot/src/cpu.c`).
- **Limitación conocida:** ni msvcrt ni UCRT exportan `aligned_alloc`, y el motor libera los
  buffers alineados con `free()` plano (`ingot_aligned_free`), lo que descarta
  `_aligned_malloc` (incompatible con `free()`); el shim cae a `malloc` simple — pierde la
  alineación a 64B como **optimización**, no como requisito de correctitud.
- **Binario autocontenido:** se enlaza **100% estático** (`-static -fopenmp`, con
  `libopenblas.a` de UCRT64 + `vendor/lz4.o`), embebiendo `libopenblas`, `libwinpthread-1`,
  `libgomp-1`, `libgfortran-5`, `libgcc_s_seh-1` y `libquadmath-0`. El `.exe` resultante solo
  depende de DLLs del propio Windows (`ntdll`, `KERNEL32`, `KERNELBASE`, `ucrtbase`) — cero
  dependencias de MSYS2/MinGW. Costo: ~33 MB frente a ~1.1 MB del build dinámico.
- **Compatibilidad de licencias:** el static linking es compatible con la licencia
  GPL-3.0-or-later del proyecto. Qwen3-TTS y `third_party/ingot` (MIT) y OpenBLAS
  (BSD-3-Clause) son permisivos; `libgcc`/`libwinpthread` llevan la **GCC Runtime Library
  Exception** (embebido estático explícitamente autorizado); `libgfortran`/`libquadmath` son
  LGPL sin esa excepción, pero su exigencia de re-linking queda cubierta por la oferta de
  fuente que GPLv3 ya obliga a proveer (`SOURCE-OFFER.md`). Las nuevas piezas —Qwen3-TTS
  (MIT), `ingot` (MIT), OpenBLAS (BSD-3-Clause), LZ4 (BSD-2-Clause) y el par LGPL
  (`libgfortran`/`libquadmath`)— se registran en `THIRD-PARTY-LICENSES.md` en la Fase 7.

#### STT y Traducción — runtime CTranslate2 compartido

- `ct2rs` (MIT, requiere CMake en el build) expone `Translator`, `Generator` y `Whisper`
  sobre CTranslate2, con streaming y backends `mkl`/`dnnl`/`cuda`. STT (Whisper) y traducción
  (Marian) **comparten el mismo runtime CT2** en el proceso Rust.
- STT mantiene `task="transcribe"` estricto. Traducción mantiene el par `es<->en` y el
  passthrough.
- **Tokenización:** `ct2rs` cubre **SentencePiece** de fábrica; el token `</s>` se anexa
  manualmente al texto de origen, replicando el runtime del oráculo (SentencePiece crudo +
  `</s>` manual en `model_loader.py`; el `.spm` embebido ya aplica la normalización
  `nmt_nfkc`, idéntica en Python y en Rust vía `ct2rs`). El pre/post-procesado de
  `sacremoses` NO se reimplementa: el oráculo no lo usa en su camino de ejecución, pieza
  descartada por decisión cerrada (ver Fase 4). No queda dependencia Python en la ruta de
  traducción en runtime.

#### Segmentación (objetivo)

- Segmentador determinista en Rust (reglas de puntuación + abreviaturas). La segmentación
  que el pipeline necesita es mucho más simple que el motor de ML; **no es razón para
  conservar Python**. Se valida contra la salida de pysbd sobre un corpus fijo.

#### Audio — CPAL unificado

- Un `AudioService` con `Capture` y `Playback` sobre **CPAL** (WASAPI/CoreAudio/
  ALSA/PipeWire/PulseAudio). Elimina `miniaudio`, `sounddevice`, `winsound`, `afplay`,
  `pycaw` de golpe y **borra la mayoría de las ramas por SO**.
- **Diferencia de diseño frente a hoy:** miniaudio entrega 16 kHz/mono/int16 directamente;
  CPAL entrega el formato nativo del dispositivo (p. ej. 48 kHz/estéreo/f32) y exige un
  **`AudioConverter`** propio (sample-format + downmix + resampler + ring buffer) antes de
  Whisper. Es trabajo acotado, no una limitación.
- `rminiaudio` (bindings Rust sobre miniaudio C) queda como **plan B** si en la
  implementación aparece una capacidad de miniaudio imprescindible.

#### Daemon — Axum

- Se conserva el **contrato HTTP** (`127.0.0.1:8765`, mismas rutas, streaming NDJSON,
  handshake de `schema_version`) reimplementado sobre **Axum + Tokio**. El cliente
  programático externo no nota el cambio.
- Se conserva la precarga + warmup al arranque y la serialización de síntesis.
- **Transporte:** se **conserva Axum-HTTP** fiel al contrato actual; el cliente IPC existente
  no cambia y encaja con que el propio motor Qwen también hable HTTP. Un transporte
  NDJSON-local queda como posible optimización futura, fuera de alcance.

#### Provisión de modelos y almacenes (objetivo)

- **Descargador nativo** (equivalente a `snapshot_download` con revisiones pinneadas) o
  reutilización del cliente HF por CLI en `setup`. La **conversión opus-mt→CT2** es un **paso
  offline de setup/build**: los converters de CTranslate2 (`ct2-opus-mt-converter`,
  `ct2-transformers-converter`) son **solo Python**, pero sus artefactos son **compatibles
  entre versiones CT2**, así que se convierte fuera de línea (o se embarcan modelos ya
  convertidos) y el runtime Rust carga el artefacto sin ningún Python.
- **Almacén de voces y de habla sintética**: se preserva el **layout en disco exacto**
  (mismas rutas, `conditionals.pt`/`.qvoice`, sidecars de metadatos) para compatibilidad
  y para permitir corridas mixtas Python/Rust durante la transición.

### 2.5 Qué se preserva y qué cambia

| Se preserva intacto (invariante) | Cambia |
|---|---|
| Superficie CLI y semántica de comandos | Lenguaje del host (Python → Rust) |
| Exit codes `0–10/130` | Motor TTS (Chatterbox → Qwen3-TTS) |
| Esquema JSON `schema_version=3` | Runtime STT/traducción (faster-whisper → ct2rs) |
| Contrato HTTP del daemon (puerto, rutas, NDJSON) | Stack de audio (5–6 libs → CPAL) |
| Contrato stdout/stderr + UTF-8 | Segmentación (pysbd → Rust) |
| Layout en disco de voces/locuciones | Empaquetado (PyInstaller → nativo) |
| Decisión ética (sin watermark) y su documentación | Framework HTTP (FastAPI → Axum) |

---

## Sección 3 — Proceso de migración por fases

### Estrategia

**Strangler fig, no big-bang.** El host Rust nace envolviendo/absorbiendo la
funcionalidad componente por componente. Durante la transición conviven la
implementación Python (oráculo) y la Rust (candidata), validadas contra el mismo set de
**vectores de contrato dorados** (mismos inputs → mismos exit codes, mismo JSON, mismos
bytes de audio dentro de tolerancia).

Cada fase declara: **objetivo**, **trabajo**, **criterio de verificación ejecutable** y
**rollback**. El orden minimiza riesgo: primero lo de bajo riesgo y alto valor
estructural (host, audio), luego los runtimes CT2 (STT/traducción), y **al final** el TTS
(máximo riesgo, gated).

### Fase 0 — Fundamentos y validación de integración

- **Objetivo:** montar el andamiaje de la migración y validar los puntos de integración
  nativos antes de escribir código de producto.
- **Trabajo:**
  - Validar la **integración por subprocess** del motor Qwen (HTTP local y/o PCM por
    `--stdout`) produciendo audio válido.
  - Validar **`ct2rs`** (Whisper + Translator) contra los modelos ya convertidos; confirmar
    la versión de CTranslate2 embebida frente a la de los artefactos.
  - Producir el **build nativo del motor en Windows** (MinGW-w64/UCRT64, §2.4 «Build nativo
    por plataforma») e integrarlo al pipeline de empaquetado.
  - Crear el **workspace Rust** (cargo workspace, crates por subsistema).
  - Construir el **harness de tests de contrato dorados**: capturar de la versión Python
    actual, para un corpus fijo de invocaciones, los `(exit_code, stdout_json, stderr,
    audio_hash)` que servirán de oráculo.
- **Verificar:** la integración por subprocess produce audio válido; `ct2rs` compila y
  produce salida plausible; el `.exe` nativo de Windows es autocontenido; el harness
  reproduce contra Python un baseline verde.
- **Rollback:** los runtimes CT2/subprocess conservan el worker Python como respaldo por
  componente; el runtime Python en Windows es el respaldo de rollback del motor TTS.

### Fase 1 — Host Rust (paridad de superficie, motores aún delegados)

- **Objetivo:** un binario Rust que reproduce la CLI, config, logging, exit codes, JSON y
  los almacenes en disco — **sin** motores nativos aún (delega a workers Python o stubs).
- **Trabajo:** `clap` con la superficie completa; taxonomía de errores → exit codes;
  emisor JSON; `VoiceStore`/`ModelStore`/`synthetic_speech` sobre el mismo layout; los
  comandos que no tocan inferencia (`voice list/remove`, `speech list/remove/play`,
  `devices`, `version`, parte de `doctor`) ya nativos.
- **Verificar:** todos los vectores de contrato de comandos **sin inferencia** pasan
  contra el oráculo; los de inferencia pasan delegando al worker Python.
- **Rollback:** el binario Python sigue siendo el de release; el Rust es opt-in.

### Fase 2 — Audio (CPAL)

- **Objetivo:** unificar playback + captura + enumeración en CPAL.
- **Trabajo:** `AudioService` (Capture/Playback) + `AudioConverter` (formato nativo →
  16 kHz/mono para Whisper); eliminar `winsound`/`afplay`/`sounddevice`/`miniaudio`/`pycaw`
  de la ruta Rust.
- **Verificar:** `devices` casa con el oráculo; round-trip de captura produce 16 kHz/mono
  int16 equivalente al de miniaudio; playback de un WAV conocido suena correcto en los 3
  SO; los WAV capturados dan la **misma transcripción** que hoy.
- **Rollback:** conmutar la ruta de audio al worker Python.

### Fase 3 — STT (`ct2rs::Whisper`)

- **Objetivo:** transcripción nativa sobre CTranslate2, sin faster-whisper.
- **Trabajo:** `SttEngine` sobre `ct2rs`; `task="transcribe"` estricto; reutilizar los
  modelos Whisper ya convertidos a CT2; conectar con el `AudioConverter` de la Fase 2.
- **Verificar (✅ completada):** corpus de 4 audios (el WAV sintético existente + 3 audios
  nuevos generados con el motor Qwen3-TTS del repositorio, remuestreados a 16 kHz/mono) con
  fixtures de transcripción emitidas por el oráculo Python (`faster-whisper-small`, provisionado
  vía `setup --with-stt`); el test de paridad (antes `#[ignore]`) compara por WER ≤ 0.05 por
  ítem — 4/4 en verde; `speech transcribe` cumple el contrato JSON y el exit 10 en fallos.
- **Rollback:** worker Python de STT.

### Fase 4 — Traducción (`ct2rs::Translator`) + segmentación

- **Objetivo:** traducción `es<->en` nativa compartiendo el runtime CT2 de la Fase 3.
- **Trabajo:** motor `Ct2TranslationEngine` sobre `ct2rs::Translator` (Marian, ambas direcciones
  es↔en); **segmentador jerárquico `HierarchicalSegmenter`** en `avi-core` (reemplaza pysbd:
  párrafo → oración → puntuación fuerte → tokens, con `max_length`); pipeline
  `segmentar → traducir → ensamblar` con passthrough intacto; cableado al comando `translate`
  con contrato JSON (`schema_version = "3"`) y exit codes 0/2/4/9.
- **Tokenización (premisa corregida):** el oráculo Python NO usa `sacremoses` en su camino de
  ejecución — tokeniza con SentencePiece crudo y añade el token `</s>` manualmente (verificado
  en `model_loader.py`); la pieza «`sacremoses` reimplementado en Rust» del alcance original
  quedó **descartada** por decisión cerrada (Decisión #6 de la orquestación de la Fase 4,
  respaldada por la evidencia de F1). La tokenización real se cubre con SentencePiece vía
  `ct2rs` + `</s>` manual, replicando el runtime del oráculo.
- **Verificar (✅ completada):** los modelos `opus-mt-{es-en,en-es}` se reconvirtieron a CT2
  **int8** replicando el flujo de conversión del oráculo (`_convert_translation_model`; pesos
  byte-idénticos a su deployment — antes eran float32, 311 MB vs 79 MB); corpus de paridad de
  11 pares `{input, expected}` (5 es→en, 6 en→es) sobre textos reales del repositorio, emitidos
  por el `TranslationService` de producción. **Paridad FUNCIONAL, no byte a byte** (decisión del
  equipo: la migración busca calidad y eficiencia, no clonar el oráculo): WER medio de corpus
  ≤ 0.35 (real 0.19 — varianza de paráfrasis válida, p. ej. «Don't» vs «Do not»), tope por ítem
  ≤ 0.6, checks funcionales (no vacío, sin `</s>`, sin `<unk>`); 5/5 en verde. Mejora de
  calidad: `disable_unk=true` en el engine (suprime `<unk>` crudo en la salida). `translate`
  cumple contrato JSON (`schema_version = "3"`) y exit codes 0/2/4/9.
- **Rollback:** worker Python de traducción/segmentación.

### Fase 5 — TTS (Qwen3-TTS por subprocess)

- **Objetivo:** reemplazar `ChatterboxEngine` por `Qwen3TtsEngine` nativo.
- **Trabajo:** integrar el motor C por **subprocess** (servidor HTTP local `/v1/tts*` o PCM
  por `--stdout`); `VoiceProfile`/`GenerationOptions`/`ProsodyOptions` mapeando a la semántica
  de Qwen (reference audio, ICL/x-vector, temperature, top-k/p, seed, rate); motor residente
  + warmup; **migrar el clonado** (timbre → `.qvoice`); portar el bypass de watermark y su
  documentación ética. En Windows usa el build nativo MinGW-w64/UCRT64 del motor (§2.4).
- **Verificar:** la calidad de las locuciones se sostiene con la integración real; RTF/RAM en el rango
  del benchmark; `speech say/synthesize/dub` cumplen contrato (exit codes, JSON, streaming de
  progreso); el almacén de voces migra o coexiste.
- **Rollback:** conservar Chatterbox tras worker Python como motor alternativo hasta estabilizar.

### Fase 6 — Daemon (Axum) + streaming + warmup

- **Objetivo:** el daemon nativo reemplaza a FastAPI/Uvicorn conservando su contrato.
- **Trabajo:** Axum en `127.0.0.1:8765`; rutas `health/synthesize/voices/precompute/
  shutdown/transcribe`; **streaming NDJSON** de progreso; handshake de `schema_version`;
  serialización de síntesis; precarga + warmup. Transporte: **Axum-HTTP** (§2.4).
- **Verificar:** un cliente IPC **sin cambios** (el actual, en Python) habla con el daemon
  Rust y pasa los vectores de contrato del daemon; el skew de versiones se comporta igual.
- **Rollback:** daemon Python.

### Fase 7 — Empaquetado, cutover y retiro de Python

- **Objetivo:** distribución nativa y jubilación del runtime Python.
- **Trabajo:** instaladores nativos por plataforma (reemplazan PyInstaller + Inno Setup);
  provisión de modelos y librerías compartidas nativas; CI que compila los 3 SO Tier 1;
  migrar/portar la suite de tests al harness de contrato + tests unitarios Rust; cumplir
  **GPLv3** (oferta de fuente y THIRD-PARTY para las nuevas dependencias nativas).
- **Verificar:** instalación limpia en cada SO Tier 1 → `setup` → síntesis/transcripción/
  doblaje end-to-end sin Python ni WSL; paridad completa de contratos; playbook de
  operación reproducido.
- **Rollback:** mantener el release Python como canal paralelo hasta declarar paridad.

### Preocupaciones transversales

- **Licenciamiento (GPLv3).** Cada dependencia nativa nueva (CPAL, ct2rs, motor Qwen,
  CTranslate2) exige revisión de compatibilidad de licencia y actualización de
  `THIRD-PARTY-LICENSES.md` / `SOURCE-OFFER.md`. El **motor Qwen y su cadena de build en
  Windows** son compatibles con GPLv3 (detalle en §2.4 «Build nativo por plataforma»):
  Qwen3-TTS/`ingot` (MIT) y OpenBLAS (BSD-3-Clause) son permisivos; `libgcc`/`libwinpthread`
  llevan la GCC Runtime Library Exception; `libgfortran`/`libquadmath` son LGPL cubiertos por
  la oferta de fuente que GPLv3 ya impone. Sin dependencias copyleft fuerte incompatibles en
  esa cadena.
- **Migración de tests.** Los ~765 tests Python no se traducen 1:1: los de contrato pasan
  al harness dorado (independiente del lenguaje); la lógica interna se recubre con tests
  Rust nativos. El harness dorado es el que garantiza no-regresión durante toda la
  transición.
- **GPU (resuelto en lo esencial).** Salir de PyTorch no implica perder GPU: el motor C de
  Qwen ya trae backends **Metal y CUDA**, y `ct2rs` expone la feature `cuda`. Queda como
  trabajo de integración validar cada ruta en Fase 5; el objetivo CPU sigue siendo el caso
  base (donde Qwen ya domina el benchmark).
- **Coexistencia.** Preservar el layout en disco permite corridas mixtas Python/Rust por
  fase, imprescindible para el rollback por componente.

### Registro de riesgos (resumen)

| Riesgo | Fase | Severidad | Mitigación |
|---|---|---|---|
| Regresión de calidad en la integración real / int4 | 5 | Media | Clips de referencia; re-escucha tras integrar; fallback Chatterbox |
| Build nativo del motor en Windows (upstream solo documenta WSL2) | 0/5 | Baja | Build MinGW-w64/UCRT64 con shims POSIX acotados bajo `_WIN32` y static linking autocontenido (§2.4); fallback Python como respaldo de rollback |
| `ct2rs` sin paridad de versión CT2 vs modelos convertidos | 3 | Resuelto | CTranslate2 embebido = 4.8.1, misma versión que el oráculo (ctranslate2 4.8.1); pesos reconvertidos a int8 byte-idénticos al deployment del oráculo |
| Divergencia de tokenización Marian (`sacremoses`) — resuelto por evidencia (F1) | 4 | Resuelto | El oráculo no usa `sacremoses` en runtime (SentencePiece crudo + `</s>` manual en `model_loader.py`); pieza descartada por decisión cerrada (Decisión #6) — ver Fase 4 |
| Empates numéricos de beam search entre builds de CT2 | 4 | Baja | En empates casi exactos («Don't» vs «Do not») cada build puede elegir una hipótesis válida distinta; la paridad es funcional (WER medio de corpus ≤ 0.35), no byte a byte — ver Fase 4 |
| Calidad de segmentación Rust < pysbd | 4 | Resuelto | Validación estructural contra corpus pysbd (6 tests); paridad funcional de traducción en verde sobre el corpus de 11 ítems — ver Fase 4 |
| Reescritura de packaging por SO | 7 | Media | Instaladores por plataforma; CI temprana |
| Coste/oportunidad (trabajo Python reciente) | Todas | Media | Strangler incremental; releases paralelos |

---

## Nota de alcance y honestidad

Este plan describe una **migración arquitectónica por componentes**, no una reescritura
desde cero: la viabilidad se apoya en que el repositorio ya tiene fronteras limpias
(engine / orchestrator / loaders / audio / STT / traducción / caché / CLI) y contratos
públicos congelados. Aun así es un esfuerzo grande y de riesgo real, concentrado en la
integración por subprocess del motor TTS y en su build nativo en Windows. Las estimaciones de
esfuerzo en persona-mes **no** se incluyen a propósito: dependen de la validación de
integración de la Fase 0 (subprocess, `ct2rs`).
