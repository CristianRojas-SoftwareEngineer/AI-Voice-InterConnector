# Diseño del Sistema AI Voice InterConnector (Rust + Qwen3-TTS)

## Tabla de contenidos

- [Resumen ejecutivo](#resumen-ejecutivo)
- [Arquitectura](#arquitectura)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Entry point `src/main.rs`](#entry-point-srcmainrs)
- [Motor Qwen3-TTS 0.6B (Rust)](#motor-qwen3-tts-06b-rust)
- [Traducción cross-lingual (opus-mt / CTranslate2)](#traducción-cross-lingual-opus-mt--ctranslate2)
- [Flujo de síntesis](#flujo-de-síntesis)
- [Modelo de voces de dos niveles](#modelo-de-voces-de-dos-niveles)
- [Comandos CLI](#comandos-cli)
- [Compilación Rust (cargo)](#compilación-rust-cargo)
- [Extensibilidad](#extensibilidad)
- [Referencias](#referencias)

## Resumen ejecutivo

AI Voice InterConnector es un motor de síntesis de voz (TTS) **100% local** que usa **Qwen3-TTS 0.6B CustomVoice** para clonación de voz en español latinoamericano. El usuario puede clonar su propia voz a partir de ~10 segundos de audio y generar narración de alta calidad.

- **Licencia**: GPL-3.0-or-later (código del proyecto); modelo y dependencias conservan sus licencias permisivas (MIT/BSD/Apache), salvo traducción `opus-mt` (CC-BY-4.0)
- **Idiomas**: 23+ incluyendo Español (es)
- **Clonación**: `speech-reference.wav` obligatorio (≥10s); `timbre-reference.wav` opcional
- **Parámetros del modelo**: 0.6B (Qwen3-TTS)
- **Hardware**: CPU, CUDA, MPS (Apple Silicon) — inferencia local sin APIs externas

---

## Arquitectura

```
┌─────────────────────────────────────────────────────────────┐
│              ai-voice-interconnector (binario Rust)         │
│   Instalador por SO (one-liners curl|sh / irm|iex + Cask)  │
│   Binario autocontenido (cargo build --release --features full) │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│                 Motor TTS (Qwen3-TTS 0.6B)                  │
│   Modelo: qwen3-tts-0.6b / qwen3-tts-0.6b-base (clonado)    │
│   Licencia: MIT / Apache-2.0                               │
│   Runtime: Parakeet TDT v3 int8 (ort load-dynamic ONNX Runtime 1.28.0) + CTranslate2 (ct2rs) │
└─────────────────────────────────────────────────────────────┘
                        │
                        ▼
┌─────────────────────────────────────────────────────────────┐
│           Reproducción de audio (APIs nativas)              │
│   Windows: cpal (WASAPI) · Linux: cpal (ALSA) · macOS: cpal (CoreAudio) │
└─────────────────────────────────────────────────────────────┘
```

El binario expone el CLI (`src/main.rs` + crates) y gestiona el daemon HTTP en `127.0.0.1:8765`. Los modelos se provisionan vía `ai-voice-interconnector setup` en `~/.cache/huggingface/hub` y `data_dir()` por SO.

## Estructura del proyecto

```
AI-Voice-InterConnector/
├── src/
│   └── main.rs                         # CLI (clap), dispatch daemon/directo, handle_*
├── crates/
│   ├── avi-core/                       # Tipos, exit codes, json emitter
│   ├── avi-audio/                      # AudioService (cpal, hound)
│   ├── avi-tts/                        # Qwen3TtsEngine, GenerationOptions, resident
│   ├── avi-store/                      # VoiceStore, SpeechStore, ModelStore (hf-hub + indicatif; MODEL_REVISIONS; cache_dir propio)
│   │   └── assets/default/             # speech-reference.wav + timbre-reference.wav embebidos
│   ├── avi-daemon/                     # Servidor HTTP del daemon (axum)
│   ├── avi-stt/                        # ParakeetEngine (ort, load-dynamic)
│   ├── avi-translation/                # MarianTranslator (CTranslate2)
│   └── avi-config/                     # Configuración
├── vendor/
│   └── qwen3-tts/                      # Binario y pesos Qwen3-TTS (no commiteados todos)
└── crates/xtask/src/main.rs            # cask / source-offer / licenses (tooling Rust)
├── install-linux.sh                    # One-liner Linux (curl|sh)
├── install-macos.sh                    # One-liner macOS (curl|sh)
├── install-windows.ps1                 # One-liner Windows (irm|iex)
└── tests/
    ├── cli_golden.rs                   # Harness dorado del CLI
    └── installer/                      # bats/Pester de one-liners
        ├── install-linux.bats
        ├── install-macos.bats
        └── install-windows.tests.ps1
├── Cargo.toml                          # Workspace Rust (version = 0.15.1)
├── Cargo.lock
├── .circleci/config.yml                # Pipeline Rust (cargo test/build + publish-release)
└── docs/
    ├── DESIGN.md                       # Este documento
    ├── BUILD.md                        # Build y distribución Rust
    ├── DISTRIBUTION.md                 # Canales de distribución (tar.gz/zip)
    ├── PARITY.md                       # Paridad multiplataforma
    └── SELF-HOSTED-INSTALL.md          # One-liners
```

> Las voces de **fábrica** `default` están embebidas en el binario (`crates/avi-store/assets/default/`) y se materializan en `data_dir()/voices/default/` en `ensure_initialized()`. Las voces de **usuario** viven en `data_dir()/voices/<nombre>/` (user-data-dir por SO).

## Entry point `src/main.rs`

`src/main.rs` es el **punto de entrada único** del binario. Usa `clap` para parsear los subcomandos (`version`, `devices`, `translate`, `voice`, `speech`, `daemon`, `setup`, `cleanup`, `uninstall`, `doctor`) y hace dispatch a los crates (`avi-tts`, `avi-store`, etc.) o al daemon vía HTTP. Es también la **fuente de verdad de la versión** (`const VERSION = "0.15.1"`, espejo de `Cargo.toml`).

En desarrollo se invoca como `cargo run -- <args>` o `./target/release/ai-voice-interconnector <args>`.

## Motor Qwen3-TTS 0.6B (Rust)

| Aspecto | Detalle |
|---------|---------|
| **Modelos** | `qwen3-tts-0.6b` (síntesis) y `qwen3-tts-0.6b-base` (clonado) |
| **Licencia** | MIT / Apache-2.0 |
| **Parámetros** | 0.6B |
| **Clonación de voz** | `speech-reference.wav` obligatorio (≥10s); `timbre-reference.wav` opcional |
| **Inferencia** | CPU, CUDA, MPS (vía ONNX/CTranslate2) |
| **Opciones de generación** | `GenerationOptions::produccion()` temp 0.35 seed 4 |

Ver `crates/avi-tts/src/lib.rs` y `vendor/qwen3-tts/CLAUDE.md` para el contrato de invocación (`--int4 -j 4 --stream`, residente HTTP).

## Traducción cross-lingual (opus-mt / CTranslate2)

Subsistema `crates/avi-translation` que traduce `es<->en` antes de la síntesis (`--source-language`/`--target-language` en `speech say`/`synthesize`) o de forma aislada (`translate`). Usa `Helsinki-NLP/opus-mt-es-en` / `opus-mt-en-es` (CC-BY-4.0) convertidos a CT2 en `setup`.

## Transcripción STT (Parakeet TDT v3 int8 / ort)

Subsistema `crates/avi-stt` que transcribe WAV vía `speech transcribe` (audio→texto). Usa `ParakeetEngine` vía `ort` `load-dynamic` (ONNX Runtime 1.28.0) con 4 artefactos acotados (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), incluido por defecto (setup base).

## Flujo de síntesis

```
1. Usuario: ai-voice-interconnector speech say --text "Hola" -v mi_voz
                     │
                     ▼
2. CLI parsea args y resuelve VoiceStore (default embebida o clonada)
                     │
                     ▼
3. Qwen3TtsEngine::synthesize(text, voice) → resolve_voice_motor
   - default → Preset "ryan" (sin necesidad de .wav, pero wavs embebidos para paridad)
   - clonada → Clonada(PathBuf) con reference.qvoice
                     │
                     ▼
4. Intento residente HTTP (127.0.0.1:8766) → fallback subprocess --stdout (PCM 24kHz)
                     │
                     ▼
5. PCM → WAV (hound) → AudioService::play_wav (cpal) o guardado en SpeechStore
                     │
                     ▼
6. Usuario escucha habla en español con voz clonada
```

## Modelo de voces de dos niveles

Las voces se resuelven por nombre con precedencia **usuario→fábrica** (`avi-store/src/lib.rs`):

- **Fábrica**: `voices/default/` embebida en el binario (`include_bytes!`); se materializa en `data_dir()/voices/default/` en `ensure_initialized()`. Solo lectura, protege `remove("default")`.
- **Usuario**: `data_dir()/voices/<nombre>/` (escribible), registradas con `voice clone`. Homónima sobrescribe a la de fábrica.

Sin `--voice` la CLI usa `default`.

## Comandos CLI

La referencia de comandos y flags vive en [USAGE.md](../USAGE.md) y el contrato normativo en [CLI/CONTRACT.md](CLI/CONTRACT.md). Invocación desde otros lenguajes (`subprocess`, `child_process`, `std::process`) en el [README](../README.md).

Principales:

- `setup [--language es] [--with-stt]` — provisiona modelos
- `voice clone/list/remove` — gestión de voces
- `speech say/synthesize/transcribe/dub/play/list/remove` — síntesis y audio
- `daemon start/stop/restart/status/serve` — daemon HTTP
- `cleanup [--all]` / `uninstall [--force]` — limpieza y desinstalación en un comando
- `doctor` — diagnóstico de entorno

## Compilación Rust (cargo)

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all
cargo build --release --features full
./target/release/ai-voice-interconnector version
./target/release/ai-voice-interconnector voice list
```

El pipeline CI ejecuta `cargo test --all` en Linux/Windows/macOS, `cargo llvm-cov`, `validate-licenses` y los smoke tests de one-liners, y luego 4 builds `cargo build --release --features full` con staging `tar.gz`/`.zip`. Ver `docs/BUILD.md`.

## Extensibilidad

Para añadir un nuevo motor TTS:

1. Crear nuevo crate en `crates/` o módulo en `crates/avi-tts`
2. Mantener la interfaz `TtsEngine` y el dispatch en `src/main.rs`
3. Recompilar con `cargo build --release --features full` y validar con `cargo test --all`

## Referencias

- [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)
- [CTranslate2](https://github.com/OpenNMT/CTranslate2)
- [ONNX Runtime](https://onnxruntime.ai/)
