# AI Voice InterConnector

Sistema de síntesis de voz (TTS) **100% local** con clonación de voz en **español latinoamericano**.

- **Motor**: Qwen3-TTS 0.6B CustomVoice (12 Hz, multilingüe; [QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS))
- **Clonación de voz**: Usa tu propia voz como referencia (~10 s)
- **Multiplataforma**: Windows x64, Linux x64/ARM64, macOS ARM64 (Apple Silicon)
- **Consumible via CLI**: Invocable desde cualquier lenguaje de programación
- **Binario autocontenido**: Rust (`cargo build --release --features full`), sin dependencias externas

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
- **Traducción**: `translate` es↔en (CTranslate2)
- **Daemon**: `daemon start/status/stop/restart/serve` (Axum, `127.0.0.1:8765` por defecto con override `AVI_DAEMON_PORT`, streaming NDJSON)
- **100% offline**: Sin APIs externas ni conexiones a internet (modelos en la caché de la aplicación, `~/.cache/ai-voice-interconnector/models` en Linux)
- **Binario autocontenido por plataforma**: `tar.gz` (Linux/macOS) / `.zip` (Windows) con `LICENSE`/`THIRD-PARTY-LICENSES.md`/`SOURCE-OFFER.md`
- **CLI universal**: `subprocess.run(["./ai-voice-interconnector", "speech", "say", "--text", "..."])`
- **Audio nativo**: `cpal` (WASAPI/CoreAudio/ALSA)

## Instalación

AI Voice InterConnector se distribuye por **canal nativo** (archivos comprimidos Rust, ver [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md)). La instalación es de **una línea**, sin privilegios de administrador y con verificación de checksum.

### Instalación de una línea

En **Linux y macOS** (`curl | sh`, sin `sudo`), el bootstrap detecta el target del host, verifica `SHA256SUMS.txt`, comprueba que el binario arranca y delega en `self install` (que integra el PATH y encadena `setup`):

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

En **macOS** solo Apple Silicon está soportado; la limpieza de cuarentena Gatekeeper la aplica `self install`, no el bootstrap.

En **Windows** (`irm | iex`, sin UAC), el bootstrap descarga el `.zip` x86_64, verifica su hash, comprueba el arranque y delega en `self install` (que registra `HKCU\Environment\Path`):

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
```

Ambos bootstrap abortan si el checksum no coincide con `SHA256SUMS.txt` (ver [SECURITY.md](SECURITY.md)). Cada release los publica estampados con su versión como assets propios; nada se sirve desde `main`.

**Alternativa Homebrew (macOS)**: automatiza checksum/PATH/cuarentena (exige Homebrew, no provisiona modelo):

```bash
brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector
brew install --cask ai-voice-interconnector
ai-voice-interconnector setup
```

**Ciclo de vida en un comando** (paridad con instalación). El programa se gestiona a sí mismo: no hay un `uninstall` de nivel superior.

```bash
ai-voice-interconnector self uninstall --yes        # desinstalación completa (programa + PATH + estado)
ai-voice-interconnector self update --check         # informa la versión disponible sin modificar nada
ai-voice-interconnector self update --yes           # actualiza a la última estable con verificación y traspaso
# Conservar modelos, voces y habla: ai-voice-interconnector self uninstall --keep-data --yes
# Limpieza granular sin tocar programa ni PATH: ai-voice-interconnector cleanup --all --yes
# Ver el plan sin borrar nada: ai-voice-interconnector self uninstall --dry-run
# macOS Cask: brew uninstall --cask --zap ai-voice-interconnector
```

- **Linux/macOS**: borra el enlace `~/.local/bin`, el directorio `~/.local/opt/ai-voice-interconnector/`, los bloques delimitados de los perfiles y el estado.
- **Windows**: borra `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, su entrada en el `PATH` de usuario (`HKCU\Environment`, conservando el tipo del valor) y el estado.
- **Sin `--keep-data`** se borra la **raíz de datos entera**: al desinstalar desaparece el programa y con él las voces de fábrica, así que dejar nada dentro de una raíz de propiedad exclusiva es lo que corresponde.
- Es **idempotente**: repetirla en un sistema ya limpio termina con éxito.

### Descargar binario pre-compilado

Desde [Releases](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases) (4 archivos + 2 bootstrap + `SHA256SUMS.txt`: 7 assets):

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
> Linux requiere **glibc ≥ 2.35** (Ubuntu 22.04+); el bootstrap comprueba que el binario arranca y, si no lo hace, aborta con `binary_incompatible` remitiendo a `docs/BUILD.md`.

Cada Release publica `SHA256SUMS.txt` sobre los 6 ficheros (4 archivos + 2 bootstrap); verifica con `sha256sum -c` o `Get-FileHash` antes de ejecutar.

### Primer arranque: SmartScreen / Gatekeeper

Al ejecutar por primera vez un binario **descargado por navegador**, es esperable el bloqueo del SO (Mark-of-the-Web). Los **one-liners no lo disparan** (descarga por CLI sin MOTW; la limpieza de cuarentena en macOS la aplica `self install`). Detalle en [SECURITY.md](SECURITY.md#artefactos-sin-firmar).

- **Windows**: *Más información* → *Ejecutar de todas formas*.
- **macOS**: clic derecho → *Abrir* (o `xattr -d com.apple.quarantine`).

La firma Authenticode/Apple notarization es goal a largo plazo (`docs/GOAL.md`).

### Provisión del/los modelo(s) (`setup`)

Cinco modelos pinneados (4 + 1 opt-in) no vienen en el binario: `qwen3-tts-0.6b` (2,5 GB),
`opus-mt-es-en`/`opus-mt-en-es` (0,08 GB cada uno), `parakeet-tdt-v3` (0,7 GB, int8) y `qwen3-tts-0.6b-base` (2,5 GB, opt-in con `setup --with-voice-cloning`). Se descargan a la
**caché exclusiva de la aplicación** (`~/.cache/ai-voice-interconnector/models` en Linux,
`~/Library/Caches/ai-voice-interconnector/models` en macOS,
`%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows; si defines `HF_HUB_CACHE` o
`HF_HOME`, esa raíz se respeta y pasa a ser compartida) vía `setup` (3,3 GB base, 5,85 GB con `--with-voice-cloning`; el disco ocupa lo mismo que la descarga):

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
ai-voice-interconnector setup [--with-voice-cloning] [--with-stt] [--force-update] [-y|--yes]
ai-voice-interconnector cleanup [--voices|--synthetic-speech|--model|--all] [--dry-run] [-y|--yes]
ai-voice-interconnector self install [--no-setup] [--no-modify-path] [-f|--force] [-y|--yes]
ai-voice-interconnector self update [--check] [--version X.Y.Z] [-f|--force] [--no-setup] [-y|--yes]
ai-voice-interconnector self uninstall [--keep-data] [--dry-run] [-y|--yes]
```

Contrato estable (`--json` `schema_version="4"`, exit codes `0-22/130`) en `docs/CLI/CONTRACT.md`. El protocolo del daemon va por `schema_version="4"`: es un contrato independiente.

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
- [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) - Canal nativo (`tar.gz`/`.zip`), one-liners, Homebrew Cask y antivirus
- [docs/specs/sdlc-lifecycle.md](docs/specs/sdlc-lifecycle.md) - Especificación del ciclo de vida y paridad Windows/Linux/macOS
- [docs/RELEASING.md](docs/RELEASING.md) - Publicación de Releases
- [docs/MANUAL-VALIDATION.md](docs/MANUAL-VALIDATION.md) - Validación manual CLI

## Comunidad y soporte

- [CHANGELOG.md](CHANGELOG.md) - Historial
- [CONTRIBUTING.md](CONTRIBUTING.md) - Cómo contribuir (Rust)
- [SECURITY.md](SECURITY.md) - Seguridad
- [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues)
