# Disponibilidad del código fuente (MPL-2.0 §3.2)

**AI Voice-InterConnector 0.41.1** se distribuye bajo la licencia
**MPL-2.0** (ver `LICENSE`). Conforme a la sección 3.2 de la MPL-2.0,
el código fuente correspondiente a esta versión exacta está disponible en:

- **Código fuente (tarball del tag):**
  <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/archive/refs/tags/v0.41.1.tar.gz>
- **Release v0.41.1 (artefactos y notas):**
  <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.41.1>
- **Repositorio:** <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector>

Las atribuciones de las dependencias redistribuidas están en
`THIRD-PARTY-LICENSES.md`, junto a este archivo. Los pesos de los modelos no se
empaquetan en el binario: se descargan con `setup` y conservan sus licencias:
Qwen3-TTS (CustomVoice y Base) y DPDFNet, Apache-2.0; Parakeet TDT 0.6B v3
(ONNX) y los derivados int8 de opus-mt, CC-BY-4.0.

## Crates de terceros bajo MPL-2.0

El binario enlaza estos crates de terceros, también bajo MPL-2.0; su código fuente exacto se publica en crates.io:

- `audio_thread_priority` 0.35.1: <https://crates.io/crates/audio_thread_priority/0.35.1>
- `colored` 3.1.1: <https://crates.io/crates/colored/3.1.1>
- `option-ext` 0.2.0: <https://crates.io/crates/option-ext/0.2.0>

## FFmpeg 9.0.2

El paquete incluye **FFmpeg 9.0.2** como ejecutable independiente: la
aplicación lo invoca como proceso y no lo enlaza con el binario principal. Los
builds distribuidos son GPL-3.0-or-later. Su código fuente correspondiente está
disponible en estos assets del release v0.41.1, verificados contra los
hashes fijados en `packaging/pins.json`:

- `ffmpeg-9.0.2.tar.xz`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-9.0.2.tar.xz>
- `ffmpeg-builds-autobuild-2026-10-24-13-02.tar.gz`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-builds-autobuild-2026-10-24-13-02.tar.gz>

Los binarios proceden de estos builds (Linux: build `n9.0.2-35-g9a9b70bb8c` del
release `autobuild-2026-10-24-13-02` de BtbN):

- macOS ARM64: <https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip>
- Linux ARM64: <https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-24-13-02/ffmpeg-n9.0.2-35-g9a9b70bb8c-linuxarm64-gpl-9.0.tar.xz>
- Windows x64: <https://github.com/GyanD/codexffmpeg/releases/download/9.0.2/ffmpeg-9.0.2-essentials_build.zip>
- Linux x64: <https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-24-13-02/ffmpeg-n9.0.2-35-g9a9b70bb8c-linux64-gpl-9.0.tar.xz>

La configuración con la que se compiló cada binario figura en estos assets del
mismo release:

- `ffmpeg-9.0.2-buildconf-aarch64-apple-darwin.txt`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-9.0.2-buildconf-aarch64-apple-darwin.txt>
- `ffmpeg-9.0.2-buildconf-aarch64-unknown-linux-gnu.txt`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-9.0.2-buildconf-aarch64-unknown-linux-gnu.txt>
- `ffmpeg-9.0.2-buildconf-x86_64-pc-windows-msvc.txt`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-9.0.2-buildconf-x86_64-pc-windows-msvc.txt>
- `ffmpeg-9.0.2-buildconf-x86_64-unknown-linux-gnu.txt`: <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v0.41.1/ffmpeg-9.0.2-buildconf-x86_64-unknown-linux-gnu.txt>

## Solicitud del código fuente

Si necesitas el árbol de fuentes exacto de un build, incluidas las bibliotecas
que enlaza, o recibiste este binario sin acceso a las URLs anteriores, solicítalo
abriendo un issue en <https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues>. La oferta se mantiene
durante al menos tres años desde la entrega del binario.
