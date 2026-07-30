# TTS Sidecar

Sistema de síntesis de voz (TTS) **100% local** con clonación de voz en **español latinoamericano**.

- **Motor**: Chatterbox Multilingual V3 ([modelo MIT](https://huggingface.co/ResembleAI/Chatterbox-Multilingual-es-mx-latam), 23+ idiomas)
- **Clonación de voz**: Usa tu propia voz como referencia
- **Multiplataforma**: Windows x64, Linux x64/ARM64, macOS ARM64 (Apple Silicon)
- **Consumible via CLI**: Invocable desde cualquier lenguaje de programación

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

TTS Sidecar clona voces arbitrarias y **el audio que genera no lleva marca de
agua** (el watermark de PerthNet está desactivado), por lo que no es distinguible
por medios técnicos de una grabación real. Esto exige un uso responsable:

- **Consentimiento**: clona únicamente voces para las que tengas permiso explícito
  de la persona titular. No clones la voz de nadie sin su autorización.
- **No suplantación**: no uses la herramienta para hacerte pasar por otra persona,
  cometer fraude, difamar, ni producir contenido engañoso.
- **Divulgación**: al publicar o compartir audio sintetizado, indícalo como tal.
  Recuerda que el audio no contiene marca de agua que lo identifique.
- **Reporte**: si detectas un uso indebido de este proyecto, repórtalo abriendo un
  [Issue](https://github.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/issues).

El proyecto no impone barreras técnicas (fácilmente sorteables en software libre):
la responsabilidad del uso legítimo recae en quien lo emplea.

## Características

- **Clonación de voz**: ~10 segundos de audio de referencia
- **100% offline**: Sin APIs externas ni conexiones a internet
- **Instalador por plataforma**: Un instalador único por SO que despliega el bundle PyInstaller `--onedir` (carpeta de la aplicación)
- **CLI universal**: `subprocess.run(["./tts-sidecar", "speech", "say", "--text", "..."])`
- **Audio nativo**: APIs nativas del sistema operativo

## Instalación

TTS Sidecar se distribuye por **dos canales** (detalle completo y matriz de
trade-offs en [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md)): el binario
pre-compilado por SO (audiencia general, sin Python) y el paquete PyPI
(audiencia técnica con Python 3.13+).

### Instalación de una línea

En Linux, `install-linux.sh` automatiza la Opción 1 completa: resuelve el
último Release, descarga el `.AppImage` de tu arquitectura, verifica su
checksum contra `SHA256SUMS.txt`, lo instala en `~/.local/opt/tts-sidecar/`
(eliminando la versión anterior si existe) y ejecuta `setup` (integra el PATH
y ofrece descargar el modelo):

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/main/install-linux.sh | sh
```

En macOS (Apple Silicon), `install-macos.sh` hace lo análogo sin `sudo` ni
Homebrew: descarga el `.dmg` de arm64, verifica su checksum, monta el volumen,
copia el `.app` a `~/Applications`, limpia la cuarentena de Gatekeeper, crea el
symlink de PATH en `~/.local/bin` y ejecuta `setup`:

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/main/install-macos.sh | sh
```

En Windows, `install-windows.ps1` hace lo análogo desde PowerShell: descarga
el instalador del último Release, verifica su checksum, lo ejecuta en
silencio (instalación per-user, sin UAC) y corre `tts-sidecar setup`:

```powershell
irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/main/install-windows.ps1 | iex
```

Los tres scripts abortan la instalación si el checksum descargado no coincide
con `SHA256SUMS.txt` (ver [SECURITY.md](SECURITY.md#nota-sobre-los-instaladores-de-una-línea)).

**Alternativa para usuarios de Homebrew (macOS)**: el Cask del tap propio
automatiza checksum, PATH y cuarentena (pero exige tener Homebrew y no
provisiona el modelo: hay que correr `tts-sidecar setup` aparte):

```bash
brew tap CristianRojas-SoftwareEngineer/tts-sidecar
brew install --cask tts-sidecar
```

**Desinstalación**: `tts-sidecar setup --uninstall` lo hace en **un comando** en
los tres SO (encadena `cleanup --all`, revierte la integración de PATH y borra el
binario, en ese orden). Añade `--yes` para omitir la confirmación del cleanup.

- **Linux**: quita el symlink de PATH y borra `~/.local/opt/tts-sidecar/`.
- **macOS**: quita el symlink de `~/.local/bin` y borra el `.app`. Si instalaste
  con Homebrew, usa en su lugar `brew uninstall --cask --zap tts-sidecar` (el
  comando lo detecta y te remite ahí para no dejar el Caskroom inconsistente).
- **Windows**: borra los datos y delega el binario y el PATH al desinstalador de
  Inno Setup. La vía idiomática (Configuración → Aplicaciones, sin admin) sigue
  disponible como alternativa.

### Opción 1: Descargar binario pre-compilado

Descarga el ejecutable para tu plataforma desde [Releases](https://github.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/releases):

```bash
# Windows: ejecuta el instalador tts-sidecar-<versión>-x86_64-setup.exe.
# Instala en tu perfil (%LOCALAPPDATA%\Programs\tts-sidecar) y escribe el PATH
# de usuario (HKCU), sin privilegios de administrador ni prompt de UAC.
# Agrega tts-sidecar al PATH, muestra una página informativa sobre el modelo y
# ofrece una casilla para descargarlo (ejecuta 'tts-sidecar setup') al terminar.

# Linux: descarga el AppImage de tu arquitectura, hazlo ejecutable y provisiona
chmod +x tts-sidecar-<versión>-x86_64.AppImage    # o -arm64.AppImage en ARM64
./tts-sidecar-<versión>-x86_64.AppImage setup
# → setup también crea el symlink ~/.local/bin/tts-sidecar para invocarlo por nombre

# macOS (Apple Silicon): monta tts-sidecar-<versión>-arm64.dmg, arrastra el .app
# a Aplicaciones y ejecuta el script "Instalar (PATH + modelo).command" incluido
# en el volumen. El script pide tu contraseña de administrador (sudo) para crear
# el symlink en /usr/local/bin y luego ofrece ejecutar 'setup' como tu usuario.
```

> Mac Intel (x86_64) no está soportado: el toolchain actual (torch≥2.3) no
> publica wheels macOS x86_64.

> El AppImage de Linux requiere **glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+,
> Fedora 36+ o equivalente): es la versión mínima que soportan los wheels
> manylinux de las dependencias empaquetadas (torch, onnxruntime). En una
> distro más antigua, `tts-sidecar` falla al arrancar con un error del tipo
> `GLIBC_2.35 not found` (ver solución de problemas en [USAGE.md](USAGE.md)).

Cada Release publica un `SHA256SUMS.txt` con el hash de los 4 artefactos;
verifica tu descarga contra él antes de ejecutar el instalador (ver
[SECURITY.md](SECURITY.md) y [docs/RELEASING.md](docs/RELEASING.md)).

### Primer arranque: SmartScreen / Gatekeeper

Al ejecutar por primera vez el instalador **descargado desde el navegador**,
**es esperable** que el sistema lo bloquee con una advertencia («Windows
protegió tu PC» / «no se puede verificar el desarrollador»). **No indica
malware**: los binarios distribuidos no están firmados con un certificado de
código (los certificados son de pago y el proyecto aún no los financia), así
que el sistema muestra «editor desconocido» y, al ser cada release un archivo
nuevo sin historial de descargas, carece de reputación acumulada ante
SmartScreen. La advertencia depende del *Mark-of-the-Web*, que solo aplica el
navegador: el instalador de una línea (`irm | iex`) descarga por CLI, sin la
marca, y no la dispara.

- **Windows (SmartScreen)**: pulsa **Más información** → **Ejecutar de todas
  formas**.
- **macOS (Gatekeeper)**: haz clic derecho sobre el `.app`/`.dmg` → **Abrir** y
  confirma (o quita la cuarentena con `xattr`).

> El canal PyPI (`uv tool install tts-sidecar`, ver [Opción 2](#opción-2-instalar-desde-pypi-uv--pipx))
> no dispara ninguno de los dos avisos: el launcher lo genera `uv`/`pipx`
> localmente, sin Mark-of-the-Web ni cuarentena. Detalle en
> [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md).

Solo ocurre en el primer arranque. La forma objetiva de confiar en el artefacto
es **verificar su SHA-256** contra el `SHA256SUMS.txt` del Release (ver
[SECURITY.md](SECURITY.md)). Está previsto firmar los binarios vía
[SignPath Foundation](https://signpath.org/) (firma de código gratuita para
proyectos open source) en una versión futura. Detalle paso a paso en
[USAGE.md](USAGE.md#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar).

### Provisión del modelo (`setup`)

El modelo de voz **`es-mx-latam`** (~4 GB) **no** viene incluido en
el ejecutable: se descarga una sola vez a la caché de HuggingFace de tu usuario
(`~/.cache/huggingface/hub`). Esto es homólogo en los 3 SO:

- **Windows**: el instalador ofrece una casilla post-instalación que ejecuta
  `setup` por ti, en tu contexto de usuario.
- **Linux / macOS**: ejecuta `tts-sidecar setup` manualmente tras instalar.

`setup` corre los chequeos de entorno (igual que `doctor`) y descarga el modelo
solo si falta; si ya está cacheado, termina al instante sin descargar. Hasta que
el modelo esté provisionado, `speech synthesize` y `daemon start` **fallan de inmediato** y te
remiten a `tts-sidecar setup` (nunca disparan una descarga silenciosa).

### Opción 2: Instalar desde PyPI (uv / pipx)

Para audiencia técnica con Python 3.13+ ya instalado:

```bash
uv tool install tts-sidecar
# o: pipx install tts-sidecar

tts-sidecar setup     # provisiona el modelo, idéntico al canal nativo
tts-sidecar speech say --text "Hola mundo"
```

> Linux: `sounddevice` requiere la librería del sistema `libportaudio2` para
> reproducir audio (`sudo apt install libportaudio2` / `sudo dnf install
> portaudio`); no es necesaria si solo usas `speech synthesize` para persistir
> a archivo. Ver
> [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) para la matriz completa de
> trade-offs entre canales y el flujo de actualización/desinstalación.

### Opción 3: Compilar desde código

```bash
# Instalar dependencias de build
pip install -r requirements.txt

# Compilar
python scripts/build_windows.py    # Windows
python scripts/build_linux.py     # Linux
python scripts/build_macos.py     # macOS
```

## Uso Rápido

### Clonación de voz

```bash
# Clonar tu voz (requiere ~10 segundos de audio en español)
# Se necesitan DOS archivos: reference (timbre) y speech (conditioning)
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav

# Sintetizar y reproducir con tu voz clonada
tts-sidecar speech say --text "Hola mundo" -v mi_voz

# Sintetizar y guardar en el almacén bajo una etiqueta (--label es requerido)
tts-sidecar speech synthesize --text "Hola mundo" -v mi_voz --label saludo
```

### Síntesis básica

```bash
# Sintetizar y reproducir con la voz de fábrica 'default' (no requiere audios)
tts-sidecar speech say --text "Hola mundo"

# Sobrescribir la voz por defecto con una voz registrada
tts-sidecar speech say --text "Hola mundo" --voice mi_voz

# Sintetizar y guardar en el almacén bajo una etiqueta (--label es requerido)
tts-sidecar speech synthesize --text "Hola mundo" --label saludo
```

> Sin `--voice` ni audios explícitos, `speech synthesize` y `speech say`
> usan la voz de fábrica **`default`** (empaquetada, de solo lectura). Ver
> [Modelo de voces](#modelo-de-voces).

### Modelo de voces

Las voces se resuelven en **dos niveles**, con precedencia usuario→fábrica:

- **Fábrica**: empaquetadas en el ejecutable (solo lectura), incluida la voz
  `default`. Idénticas en desarrollo y en cualquier instalación.
- **Usuario**: registradas con `voice clone`, escribibles, guardadas en el
  directorio de datos de usuario por SO (estables entre ejecuciones).

Registrar una voz de usuario con el mismo nombre que una de fábrica la sobrescribe.

### Comandos disponibles

```bash
tts-sidecar speech say --text "..."                    # Sintetizar y reproducir sin persistir
tts-sidecar speech synthesize --text "..." --label L   # Sintetizar y persistir en el almacén
tts-sidecar voice clone --name X --timbre-reference ref.wav --speech-reference speech.wav  # Clonar voz
tts-sidecar voice remove --name X       # Eliminar voz
tts-sidecar voice list                  # Listar voces (--json disponible)
tts-sidecar devices                     # Dispositivos de audio (--json disponible)
tts-sidecar doctor                      # Diagnóstico (--json disponible)
tts-sidecar setup                       # Provisionar: chequeos + descargar modelo si falta
tts-sidecar version                     # Versión (--json disponible)
```

## Invocación desde cualquier lenguaje

```bash
# Bash/shell
./tts-sidecar speech say --text "Hola mundo"

# Python
subprocess.run(["./tts-sidecar", "speech", "say", "--text", "Hola mundo"])

# Node.js
child_process.spawn("./tts-sidecar", ["speech", "say", "--text", "Hola mundo"])

# Rust
std::process::Command::new("./tts-sidecar")
    .args(["speech", "say", "--text", "Hola"])
    .output()?;

# Go
exec.Command("./tts-sidecar", "speech", "say", "--text", "Hola")

# Java
new ProcessBuilder("./tts-sidecar", "speech", "say", "--text", "Hola").start()
```

## Arquitectura

```
┌─────────────────────────────────────────────────────┐
│              tts-sidecar (binario CLI)              │
│   Compilado con PyInstaller --onedir (carpeta de app) │
└──────────────────────┬──────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────┐
│           Chatterbox Multilingual V3                 │
│   Modelo: es-mx-latam (caché de HuggingFace)        │
│   Licencia: MIT                                     │
│   Idiomas: 23+ (incl. español es)                    │
└─────────────────────────────────────────────────────┘
```

## Licencia

Copyright © 2026 Cristián Rojas Arredondo.

**GPL-3.0-or-later** — el código de `tts-sidecar` se distribuye bajo la GNU General Public
License v3 (ver [LICENSE](LICENSE)). Es software libre y de uso comercial permitido, con la
condición copyleft de que los trabajos derivados que se distribuyan se liberen bajo la misma
licencia.

El **modelo de voz** Chatterbox Multilingual V3 se distribuye bajo licencia
[**MIT**](https://huggingface.co/ResembleAI/Chatterbox-Multilingual-es-mx-latam) (verificada
en HuggingFace), al igual que el modelo base
[`ResembleAI/chatterbox`](https://huggingface.co/ResembleAI/chatterbox). Las **dependencias**
empaquetadas conservan sus propias licencias, en su mayoría permisivas (MIT/BSD/Apache 2.0),
con algunas de copyleft compatible con GPLv3 (MPL-2.0, LGPL-2.1+, GPLv3+). Los runtimes
propietarios de NVIDIA CUDA que lista el lockfile universal **no se incluyen en ningún
artefacto distribuido** (todos los builds son CPU-only); solo aplican a instalaciones
desde código fuente con ese lock. El detalle completo y verificado está en
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).

## Documentación

- [docs/GOAL.md](docs/GOAL.md) - Meta del proyecto
- [docs/ROADMAP.md](docs/ROADMAP.md) - Estado actual y roadmap al goal inmediato
- [docs/DESIGN.md](docs/DESIGN.md) - Diseño técnico
- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) - Arquitectura del sistema
- [docs/DAEMON-MODE.md](docs/DAEMON-MODE.md) - Daemon mode (servidor persistente)
- [docs/BUILD.md](docs/BUILD.md) - Guía de compilación PyInstaller
- [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) - Canales de distribución (nativo + PyPI)
- [docs/PARITY.md](docs/PARITY.md) - Estado de paridad de experiencia entre Windows, Linux y macOS
- [docs/MANUAL-VALIDATION.md](docs/MANUAL-VALIDATION.md) - Recorrido manual de validación de la superficie de la CLI

## Comunidad y soporte

- [CHANGELOG.md](CHANGELOG.md) - Historial de cambios por versión
- [CONTRIBUTING.md](CONTRIBUTING.md) - Cómo contribuir (setup, tests, estilo, flujo de PR)
- [SECURITY.md](SECURITY.md) - Política de seguridad y cómo reportar vulnerabilidades
- [Issues](https://github.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/issues) - Reporte de bugs y solicitudes de función
