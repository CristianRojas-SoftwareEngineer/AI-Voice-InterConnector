# AI Voice InterConnector

Sistema de síntesis de voz (TTS) **100% local** con clonación de voz en **español latinoamericano**.

- **Motor**: Qwen3-TTS 0.6B CustomVoice (12 Hz, multilingüe; [QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS))
- **Clonación de voz**: Usa tu propia voz como referencia (~10 s)
- **Multiplataforma**: Windows x64, Linux x64/ARM64, macOS ARM64 (Apple Silicon)
- **Consumible via CLI**: Invocable desde cualquier lenguaje de programación
- **Binario autocontenido**: Rust (`cargo build --release --features full`), sin Python ni dependencias externas

## Tabla de contenidos

- [Uso ético y responsable](#uso-ético-y-responsable)
- [Características](#características)
- [Instalación](#instalación)
- [Uso Rápido](#uso-rápido)
- [Invocación desde cualquier lenguaje](#invocación-desde-cualquier-lenguaje)
- [Arquitectura](#arquitectura)
- [Licencia](#licencia)
- [Documentación](#documentación)
- [Comunidad y soporte](#comunidad-y-soporte)

## Uso ético y responsable

AI Voice InterConnector clona voces arbitrarias y **el audio que genera no lleva marca de
agua** (el motor Qwen3-TTS no incorpora watermarker), por lo que no es distinguible
por medios técnicos de una grabación real. Esto exige un uso responsable:

- **Consentimiento**: clona únicamente voces para las que tengas permiso explícito
  de la persona titular. No clones la voz de nadie sin su autorización.
- **No suplantación**: no uses la herramienta para hacerte pasar por otra persona,
  cometer fraude, difamar, ni producir contenido engañoso.
- **Divulgación**: al publicar o compartir audio sintetizado, indícalo como tal.
  Recuerda que el audio no contiene marca de agua que lo identifique.
- **Reporte**: si detectas un uso indebido de este proyecto, repórtalo abriendo un
  [Issue](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues).

El proyecto no impone barreras técnicas (fácilmente sorteables en software libre):
la responsabilidad del uso legítimo recae en quien lo emplea.

## Características

- **Clonación de voz**: ~10 segundos de audio de referencia (`speech-reference.wav` obligatorio, `timbre-reference.wav` opcional)
- **Síntesis cross-lingual**: reutiliza el timbre de una voz clonada para hablar en español o en inglés (`--target-language`)
- **Transcripción STT**: `speech transcribe` (Parakeet TDT 0.6B v3 int8, ONNX Runtime)
- **Traducción**: `translate` es↔en (CTranslate2, opt-in)
- **Daemon**: `daemon start/status/stop/restart/serve` (Axum, `127.0.0.1:8765`, streaming NDJSON)
- **100% offline**: Sin APIs externas ni conexiones a internet (modelos en `~/.cache/huggingface/hub`)
- **Binario autocontenido por plataforma**: `tar.gz` (Linux/macOS) / `.zip` (Windows) con `LICENSE`/`THIRD-PARTY-LICENSES.md`/`SOURCE-OFFER.md`
- **CLI universal**: `subprocess.run(["./ai-voice-interconnector", "speech", "say", "--text", "..."])`
- **Audio nativo**: `cpal` (WASAPI/CoreAudio/ALSA)

## Instalación

AI Voice InterConnector se distribuye por **canal nativo** (archivos comprimidos Rust, ver [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md)). La instalación es de **una línea**, sin privilegios de administrador y con verificación de checksum.

### Instalación de una línea

En **Linux** (`curl | sh`), descarga el `tar.gz` de tu arquitectura, verifica `SHA256SUMS.txt`, extrae en `~/.local/opt/ai-voice-interconnector/`, crea el symlink `~/.local/bin/ai-voice-interconnector` y encadena `setup`:

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-linux.sh | sh
```

En **macOS Apple Silicon** (`curl | sh`, sin `sudo` ni Homebrew), análogo a Linux con `shasum` y limpieza de cuarentena Gatekeeper (`xattr`):

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-macos.sh | sh
```

En **Windows** (`irm | iex`, sin UAC), descarga el `.zip` x86_64, verifica `Get-FileHash` y registra `HKCU\Environment\Path`:

```powershell
irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 | iex
```

Los tres scripts abortan si el checksum no coincide con `SHA256SUMS.txt` (ver [SECURITY.md](SECURITY.md)).

**Alternativa Homebrew (macOS)**: automatiza checksum/PATH/cuarentena (exige Homebrew, no provisiona modelo):

```bash
brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector
brew install --cask ai-voice-interconnector
ai-voice-interconnector setup
```

**Desinstalación en un comando** (paridad con instalación):

```bash
ai-voice-interconnector uninstall --force   # desinstalación completa (datos + binario + PATH)
# Limpieza de datos sin binario/PATH: ai-voice-interconnector cleanup --all --yes  # unión Modelo+voces+habla
# macOS Cask: brew uninstall --cask --zap ai-voice-interconnector
```

- **Linux/macOS**: borra `~/.local/bin` symlink + `~/.local/opt/ai-voice-interconnector/` + datos.
- **Windows**: borra `%LOCALAPPDATA%\Programs\ai-voice-interconnector` + entrada `HKCU` PATH + datos.

### Descargar binario pre-compilado

Desde [Releases](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases) (4 artefactos + `SHA256SUMS.txt`):

```bash
# Linux x64 (sustituye X.Y.Z por la versión del Release)
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz -o ai.tar.gz
tar -xzf ai.tar.gz && ./ai-voice-interconnector setup

# macOS arm64
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz -o ai.tar.gz
tar -xzf ai.tar.gz && ./ai-voice-interconnector setup

# Windows x64 (PowerShell)
Invoke-WebRequest https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-x86_64-windows.zip -OutFile ai.zip
Expand-Archive ai.zip -Force; .\ai-voice-interconnector.exe setup
```

> Mac Intel (x86_64) y Windows ARM64 no están soportados (limitación de toolchain aceptada, ver `docs/BUILD.md`).
> Linux requiere **glibc ≥ 2.35** (Ubuntu 22.04+); el instalador advierte si es menor.

Cada Release publica `SHA256SUMS.txt`; verifica con `sha256sum -c` o `Get-FileHash` antes de ejecutar.

### Primer arranque: SmartScreen / Gatekeeper

Al ejecutar por primera vez un binario **descargado por navegador**, es esperable el bloqueo del SO (Mark-of-the-Web). Los **one-liners no lo disparan** (descarga por CLI sin MOTW, `xattr` en macOS). Detalle en [SECURITY.md](SECURITY.md#artefactos-sin-firmar).

- **Windows**: *Más información* → *Ejecutar de todas formas*.
- **macOS**: clic derecho → *Abrir* (o `xattr -d com.apple.quarantine`).

La firma Authenticode/Apple notarization es goal a largo plazo (`docs/GOAL.md`).

### Provisión del/los modelo(s) (`setup`)

Cinco modelos pinneados (4 + 1 opt-in) no vienen en el binario: `qwen3-tts-0.6b` (~4,7 GB),
`marian-es-en`/`marian-en-es` (~3 GB), `parakeet-tdt-v3` (~600 MB, int8) y `qwen3-tts-0.6b-base` (~2,5 GB, opt-in con `setup --with-base`). Se descargan a
`~/.cache/huggingface/hub` vía `setup` (~9 GB base, ~11,5 GB con `--with-base`):

```bash
ai-voice-interconnector setup
ai-voice-interconnector doctor
```

Hasta provisionar, `speech synthesize`/`daemon start` fallan con exit 4 remitiendo a `setup`.

### Compilar desde código (Rust)

Requisitos: Rust 1.96.0, `cmake`, `pkg-config`, `libasound2-dev` (Linux) y `libclang-dev` solo con `--features native-translation/full` (traducción). Ver `docs/BUILD.md`.

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all
cargo build --release --features full
./target/release/ai-voice-interconnector version
./target/release/ai-voice-interconnector voice list
```

## Uso Rápido

### Clonación de voz

```bash
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav
ai-voice-interconnector speech say --text "Hola mundo" -v mi_voz
ai-voice-interconnector speech synthesize --text "Hola mundo" -v mi_voz --label saludo
```

### Síntesis básica

```bash
ai-voice-interconnector speech say --text "Hola mundo"                    # voz default
ai-voice-interconnector speech say --text "Hola mundo" --voice mi_voz
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo
```

Sin `--voice` usa `default` (embebida en el binario, `crates/avi-store/assets/default/`).

### Modelo de voces

Dos niveles, precedencia usuario→fábrica (`avi-store/src/lib.rs`):

- **Fábrica**: embebida en el binario (`include_bytes!`), materializada en `data_dir()/voices/default/`; `default` no se puede borrar.
- **Usuario**: `data_dir()/voices/<nombre>/` (escribible), `voice clone`.

### Comandos disponibles

```bash
ai-voice-interconnector speech say --text "..."                    # reproducir sin persistir
ai-voice-interconnector speech synthesize --text "..." --label L   # persistir
ai-voice-interconnector speech transcribe --audio file.wav --source-language es-latam
ai-voice-interconnector speech dub --mic --source-language es-latam --target-language en -v mi_voz
ai-voice-interconnector voice clone --name X --timbre-reference ref.wav --speech-reference speech.wav
ai-voice-interconnector voice list / remove --name X
ai-voice-interconnector translate --text "Hola" --from es --to en
ai-voice-interconnector devices / doctor / version
ai-voice-interconnector daemon start / status / stop / restart / serve
ai-voice-interconnector setup [--with-base] [--with-stt] / cleanup [--voices|--synthetic-speech|--model|--all] [--dry-run] [-y|--yes] / uninstall --force
```

Contrato estable (`--json` `schema_version="3"`, exit codes `0-10/130`) en `docs/CLI/CONTRACT.md`.

## Invocación desde cualquier lenguaje

```bash
./ai-voice-interconnector speech say --text "Hola mundo"
subprocess.run(["./ai-voice-interconnector", "speech", "say", "--text", "Hola mundo"])
child_process.spawn("./ai-voice-interconnector", ["speech", "say", "--text", "Hola mundo"])
std::process::Command::new("./ai-voice-interconnector").args(["speech", "say", "--text", "Hola"]).output()?;
exec.Command("./ai-voice-interconnector", "speech", "say", "--text", "Hola")
new ProcessBuilder("./ai-voice-interconnector", "speech", "say", "--text", "Hola").start()
```

## Arquitectura

```
┌─────────────────────────────────────────────────────┐
│  ai-voice-interconnector (binario Rust)             │
│  src/main.rs (clap) + crates/* + tokio/axum/cpal    │
└──────────────────────┬──────────────────────────────┘
                       ▼
┌─────────────────────────────────────────────────────┐
│  Qwen3-TTS 0.6B (C, subprocess/HTTP) + Parakeet TDT  │
│  Modelos: qwen3-tts-0.6b, parakeet-tdt-v3 (HF Hub)   │
└─────────────────────────────────────────────────────┘
```

Ver `docs/DESIGN.md` y `docs/BUILD.md`.

## Licencia

Copyright © 2026 Cristián Rojas Arredondo — **GPL-3.0-or-later** ([LICENSE](LICENSE)).
Motor Qwen3-TTS MIT/Apache-2.0; dependencias en [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) (inventario Rust `Cargo.lock`). Oferta de fuente GPLv3 §6 en [SOURCE-OFFER.md](SOURCE-OFFER.md).

## Documentación

- [docs/GOAL.md](docs/GOAL.md) - Meta y criterios de aceptación
- [docs/DESIGN.md](docs/DESIGN.md) - Diseño técnico (Rust)
- [docs/BUILD.md](docs/BUILD.md) - Guía de compilación Rust
- [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) - Canal nativo (`tar.gz`/`.zip`)
- [docs/PARITY.md](docs/PARITY.md) - Paridad Windows/Linux/macOS
- [docs/SELF-HOSTED-INSTALL.md](docs/SELF-HOSTED-INSTALL.md) - One-liners
- [docs/RELEASING.md](docs/RELEASING.md) - Publicación de Releases
- [docs/MANUAL-VALIDATION.md](docs/MANUAL-VALIDATION.md) - Validación manual CLI

## Comunidad y soporte

- [CHANGELOG.md](CHANGELOG.md) - Historial
- [CONTRIBUTING.md](CONTRIBUTING.md) - Cómo contribuir (Rust)
- [SECURITY.md](SECURITY.md) - Seguridad
- [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues)
