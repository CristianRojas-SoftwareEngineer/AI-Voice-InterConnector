# AI Voice-InterConnector

Sistema de síntesis de voz (TTS) **100% local** con clonación de voz en **español**, sobre el motor Qwen3-TTS 0.6B (12 Hz, multilingüe; [QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)). Se distribuye como un único archivo por plataforma que trae el binario, el motor TTS y ONNX Runtime.

- **Síntesis local**: texto a voz sin APIs externas.
- **Clonación de voz**: usa tu propia voz como referencia con ~10 s de audio.
- **Síntesis cross-lingual**: reutiliza el timbre de una voz clonada para hablar en español o en inglés.
- **Transcripción**: voz a texto con Parakeet TDT 0.6B v3.
- **Traducción**: es↔en con opus-mt.
- **Daemon opcional**: mantiene los motores calientes entre invocaciones.
- **100% offline** tras descargar los modelos con `setup`.
- **Multiplataforma**: Windows x64, Linux x64/ARM64, macOS ARM64 (Apple Silicon).
- **Invocable por CLI** desde cualquier lenguaje de programación.

## Tabla de contenidos

- [Uso ético y responsable](#uso-ético-y-responsable)
- [Instalación](#instalación)
  - [Requisitos](#requisitos)
    - [Requisitos en Windows](#requisitos-en-windows)
    - [Requisitos en Linux](#requisitos-en-linux)
    - [Requisitos en macOS](#requisitos-en-macos)
  - [Instalación de una línea](#instalación-de-una-línea)
    - [Instalar en Windows](#instalar-en-windows)
    - [Instalar en Linux](#instalar-en-linux)
    - [Instalar en macOS](#instalar-en-macos)
  - [Descargar binario pre-compilado](#descargar-binario-pre-compilado)
    - [Descarga manual en Windows](#descarga-manual-en-windows)
    - [Descarga manual en Linux](#descarga-manual-en-linux)
    - [Descarga manual en macOS](#descarga-manual-en-macos)
  - [Primer arranque: SmartScreen / Gatekeeper](#primer-arranque-smartscreen--gatekeeper)
    - [Primer arranque en Windows](#primer-arranque-en-windows)
    - [Primer arranque en macOS](#primer-arranque-en-macos)
  - [Provisión de los modelos (`setup`)](#provisión-de-los-modelos-setup)
- [Primeros pasos](#primeros-pasos)
  - [Síntesis](#síntesis)
  - [Clonación de voz](#clonación-de-voz)
  - [Invocación desde cualquier lenguaje](#invocación-desde-cualquier-lenguaje)
- [Actualizar y desinstalar](#actualizar-y-desinstalar)
- [Arquitectura](#arquitectura)
- [Documentación](#documentación)
  - [Para usuarios](#para-usuarios)
  - [Para contribuidores](#para-contribuidores)
- [Licencia](#licencia)
- [Comunidad y soporte](#comunidad-y-soporte)

## Uso ético y responsable

AI Voice-InterConnector clona voces arbitrarias y **el audio que genera no lleva marca de
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

## Instalación

**Elige tu sistema**:

| Sistema | Requisitos | Instalación de una línea | Descarga manual | Primer arranque |
|---|---|---|---|---|
| Windows | [Requisitos en Windows](#requisitos-en-windows) | [Instalar en Windows](#instalar-en-windows) | [Descarga manual en Windows](#descarga-manual-en-windows) | [Primer arranque en Windows](#primer-arranque-en-windows) |
| Linux | [Requisitos en Linux](#requisitos-en-linux) | [Instalar en Linux](#instalar-en-linux) | [Descarga manual en Linux](#descarga-manual-en-linux) | — |
| macOS | [Requisitos en macOS](#requisitos-en-macos) | [Instalar en macOS](#instalar-en-macos) | [Descarga manual en macOS](#descarga-manual-en-macos) | [Primer arranque en macOS](#primer-arranque-en-macos) |

AI Voice-InterConnector se distribuye por **canal nativo** (archivos comprimidos Rust, ver [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md)). La instalación es de **una línea**, sin privilegios de administrador y con verificación de checksum.

### Requisitos

La síntesis corre en CPU por defecto (sin GPU). Requisitos orientativos:

- **CPU**: x86-64 o ARM64 moderna. Toda la inferencia corre en CPU, así que en
  procesadores antiguos la síntesis es más lenta.
- **RAM**: **8 GB recomendados**, **4 GB mínimo**. Con menos memoria la síntesis
  funciona pero puede paginar (ralentizarse) en textos largos. `doctor` no mide
  ni la CPU ni la RAM.
- **Disco**: 5,85 GB para los modelos descargados (Qwen3-TTS 2,5 GB + modelo Base
  de clonado 2,5 GB + opus-mt es↔en 0,17 GB + Parakeet TDT v3 0,7 GB). El disco
  ocupa lo mismo que la descarga, salvo en sistemas de archivos sin enlaces duros (FAT32,
  exFAT o algunos recursos de red), donde `hf-hub` copia cada archivo y el espacio se
  duplica. El programa instalado (binario, motor TTS y ONNX Runtime) ocupa ~124 MB en Windows.
- **GPU (opcional)**: el motor usa CPU por defecto; no es necesaria para el
  funcionamiento.
- **Plataformas no soportadas**: Mac Intel (x86_64) y Windows ARM64 (limitación de toolchain aceptada).

Detalle en [docs/BUILD.md](docs/BUILD.md#dependencias-de-runtime-por-plataforma).

#### Requisitos en Windows

- **Windows 10 1903 o posterior** (x64); nada que instalar: el
  archivo trae las DLL del runtime de VC++ que necesitan el binario y ONNX
  Runtime.

#### Requisitos en Linux

- **glibc ≥ 2.28** (Debian 10+, Ubuntu 20.04+, RHEL, Rocky y Alma 8+,
  Fedora 29+ o equivalente), sin nada más que instalar: el binario solo usa la
  glibc y las librerías de C y C++ que toda distribución con glibc trae, y el
  motor TTS lleva OpenBLAS enlazado estático. El bootstrap comprueba que el binario
  arranca y, si no lo hace, aborta con `binary_incompatible`.
- **Audio**: la reproducción y la captura usan ALSA, que traen las
  distribuciones de escritorio. En un sistema sin ALSA (servidores o
  contenedores mínimos) el binario arranca igual: solo `devices` y las órdenes
  que reproducen o capturan audio fallan, con un error que nombra la librería
  de ALSA ausente.

#### Requisitos en macOS

- **macOS 26 (Tahoe) o posterior**, en cualquier Mac con Apple Silicon
  (M1 en adelante).

### Instalación de una línea

Ambos bootstrap abortan si el checksum no coincide con `SHA256SUMS.txt` (ver [SECURITY.md](SECURITY.md)). Cada release los publica estampados con su versión como assets propios; nada se sirve desde `main`.

#### Instalar en Windows

En **Windows** (`irm | iex`, sin UAC), el bootstrap descarga el `.zip` x86_64, verifica su hash, comprueba el arranque y delega en `self install` (que integra el PATH de usuario y encadena `setup`):

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
```

#### Instalar en Linux

En **Linux y macOS** (`curl | sh`, sin `sudo`), el bootstrap detecta el target del host, verifica `SHA256SUMS.txt`, comprueba que el binario arranca y delega en `self install` (que integra el PATH y encadena `setup`):

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

#### Instalar en macOS

El one-liner es el mismo que en [Linux](#instalar-en-linux):

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

En **macOS** solo Apple Silicon está soportado; la limpieza de cuarentena Gatekeeper la aplica `self install`, no el bootstrap.

**Alternativa Homebrew (macOS)**: automatiza checksum/PATH/cuarentena (exige Homebrew, no provisiona modelo):

```bash
brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector
brew install --cask ai-voice-interconnector
ai-voice-interconnector setup
```

Con Homebrew, para actualizar y desinstalar:

```bash
brew upgrade --cask ai-voice-interconnector
brew uninstall --cask --zap ai-voice-interconnector
```

### Descargar binario pre-compilado

Desde [Releases](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases) (sustituye X.Y.Z por la versión del Release).

Cada Release publica `SHA256SUMS.txt` con el hash de cada archivo y de cada bootstrap. Antes de ejecutar, descárgalo junto al archivo y verifica el hash con la orden de tu sistema.

#### Descarga manual en Windows

```powershell
# Windows x64 (PowerShell)
Invoke-WebRequest https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-x86_64-windows.zip -OutFile ai.zip
Expand-Archive ai.zip -Force; .\ai-voice-interconnector.exe setup
```

```powershell
# Windows (PowerShell): compara el resultado con la línea del archivo en SHA256SUMS.txt
Get-FileHash .\ai-voice-interconnector-X.Y.Z-x86_64-windows.zip -Algorithm SHA256
```

#### Descarga manual en Linux

```bash
# Linux x64
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz -o ai.tar.gz
tar -xzf ai.tar.gz && ./ai-voice-interconnector setup
```

```bash
# Linux / macOS
sha256sum -c SHA256SUMS.txt --ignore-missing
```

#### Descarga manual en macOS

```bash
# macOS arm64
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz -o ai.tar.gz
tar -xzf ai.tar.gz && ./ai-voice-interconnector setup
```

```bash
# Linux / macOS
sha256sum -c SHA256SUMS.txt --ignore-missing
```

### Primer arranque: SmartScreen / Gatekeeper

Al ejecutar por primera vez un binario **descargado por navegador**, es esperable que el SO lo bloquee (Mark-of-the-Web): los binarios no están firmados ni notarizados, y SmartScreen y Gatekeeper tratan todo ejecutable sin historial como no confiable. No significa que el archivo contenga malware; el porqué está en [SECURITY.md](SECURITY.md#artefactos-sin-firmar). Los **one-liners no lo disparan** (descarga por CLI sin Mark-of-the-Web; la limpieza de cuarentena en macOS la aplica `self install`).

Antes de aceptar, verifica el hash contra `SHA256SUMS.txt` como se indica en
[Descargar binario pre-compilado](#descargar-binario-pre-compilado). Esto solo ocurre
en el primer arranque de cada versión. Si un antivirus de terceros pone el archivo en
cuarentena, restáuralo y añade una exclusión **solo después** de verificar el hash.
La firma Authenticode y la notarización de Apple son un objetivo a largo plazo
([docs/GOAL.md](docs/GOAL.md)).

#### Primer arranque en Windows

**SmartScreen**: en el diálogo «Windows protegió tu PC», pulsa
**Más información** → **Ejecutar de todas formas**. Si el navegador ya
bloqueó la descarga, consérvala desde el menú de descargas: **Conservar** →
**Conservar de todas formas**.

#### Primer arranque en macOS

**Gatekeeper**: haz clic derecho sobre el binario → **Abrir** y confirma; o
quita la cuarentena desde una terminal:

```bash
xattr -d com.apple.quarantine ai-voice-interconnector
```

### Provisión de los modelos (`setup`)

Los modelos no vienen en el binario: `setup` descarga los modelos pinneados a la caché
de la aplicación (el espacio necesario está en [Requisitos](#requisitos)). Los one-liners
lo ejecutan al terminar la instalación; tras una descarga manual o con Homebrew, ejecútalo tú:

```bash
ai-voice-interconnector setup
ai-voice-interconnector doctor
```

Hasta provisionar, `speech synthesize`/`daemon start` fallan con exit 4 remitiendo a `setup`.
Detalle de cada modelo y de la caché en [USAGE.md](USAGE.md#modelos-setup).

## Primeros pasos

### Síntesis

```bash
ai-voice-interconnector speech say --text "Hola mundo"                    # voz default
ai-voice-interconnector speech say --text "Hola mundo" --voice mi_voz
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo
```

Sin `--voice` se usa la voz de fábrica `default`, que viene embebida en el binario. `speech say` reproduce sin guardar; `speech synthesize` guarda el WAV en el almacén `(voz, etiqueta)` y muestra su ruta; con `--json`, la ruta llega en el campo `audio_path`.

### Clonación de voz

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav
ai-voice-interconnector speech say --text "Hola mundo" -v mi_voz
ai-voice-interconnector speech synthesize --text "Hola mundo" -v mi_voz --label saludo
```

La referencia es un único audio de ~10 segundos de habla limpia.

### Invocación desde cualquier lenguaje

```bash
./ai-voice-interconnector speech say --text "Hola mundo"
subprocess.run(["./ai-voice-interconnector", "speech", "say", "--text", "Hola mundo"])
child_process.spawn("./ai-voice-interconnector", ["speech", "say", "--text", "Hola mundo"])
std::process::Command::new("./ai-voice-interconnector").args(["speech", "say", "--text", "Hola"]).output()?;
exec.Command("./ai-voice-interconnector", "speech", "say", "--text", "Hola")
new ProcessBuilder("./ai-voice-interconnector", "speech", "say", "--text", "Hola").start()
```

Todos los comandos, la salida `--json` y la transcripción y traducción se detallan en [USAGE.md](USAGE.md); el contrato de la CLI está en [docs/CLI/README.md](docs/CLI/README.md).

## Actualizar y desinstalar

El programa se gestiona a sí mismo: no hay un `uninstall` de nivel superior.

```bash
ai-voice-interconnector self update --check             # informa la versión disponible sin modificar nada
ai-voice-interconnector self update --yes               # actualiza a la última estable con verificación
ai-voice-interconnector self uninstall --yes            # desinstalación completa (programa + PATH + datos)
ai-voice-interconnector self uninstall --keep-data --yes  # conserva modelos, voces y habla
ai-voice-interconnector cleanup --all --yes             # borra los datos sin tocar programa ni PATH
```

Con Homebrew, las órdenes para actualizar y desinstalar están en [Instalar en macOS](#instalar-en-macos).

La desinstalación es completa (programa, integración de PATH y datos) e idempotente: repetirla en un sistema ya limpio termina con éxito. Detalle en [USAGE.md](USAGE.md#ciclo-de-vida-de-la-instalación-self).

## Arquitectura

Un único binario Rust expone la CLI. La síntesis la hace el motor Qwen3-TTS, que corre
como proceso residente y se comunica por HTTP en loopback; la transcripción (Parakeet,
ONNX Runtime) y la traducción (CTranslate2) corren dentro del proceso. Un daemon opcional
mantiene los motores calientes entre invocaciones. Detalle en
[docs/DESIGN.md](docs/DESIGN.md#arquitectura).

## Documentación

### Para usuarios

- [USAGE.md](USAGE.md) — guía de uso con el programa instalado: comandos, modelos, ciclo de vida, daemon y solución de problemas.
- [docs/CLI/README.md](docs/CLI/README.md) — referencia por comando de la CLI (cada comando tiene su página en `docs/CLI/commands/`) y códigos de salida.
- [docs/CLI/CONTRACT.md](docs/CLI/CONTRACT.md) — contrato normativo de la CLI y de la salida `--json`, que es el contrato con el plugin.
- [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) — canal de distribución nativo, Homebrew Cask y antivirus.
- [SECURITY.md](SECURITY.md) — seguridad y reporte de vulnerabilidades.
- [CHANGELOG.md](CHANGELOG.md) — historial de cambios.

### Para contribuidores

- [CONTRIBUTING.md](CONTRIBUTING.md) — cómo contribuir: entorno, tests y convenciones.
- [docs/DESIGN.md](docs/DESIGN.md) — diseño técnico y arquitectura.
- [docs/BUILD.md](docs/BUILD.md) — compilación y pipeline.
- [docs/RELEASING.md](docs/RELEASING.md) — publicación de releases.
- [docs/CI.md](docs/CI.md) — integración continua.
- [docs/GOAL.md](docs/GOAL.md) — meta y criterios de aceptación.
- [docs/specs/LIFECYCLE.md](docs/specs/LIFECYCLE.md) — especificación del ciclo de vida y paridad entre SO.
- [docs/MANUAL-VALIDATION.md](docs/MANUAL-VALIDATION.md) — validación manual de la CLI.
- [docs/dev/CONTEXT-OPTIMIZATION.md](docs/dev/CONTEXT-OPTIMIZATION.md) — optimización de contexto para agentes de código.
- [AGENTS.md](AGENTS.md) — guías para agentes de código.
- [SOURCE-OFFER.md](SOURCE-OFFER.md) — disponibilidad de la fuente (MPL-2.0 §3.2).
- [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) — inventario de licencias de terceros.
- [docs/issues/README.md](docs/issues/README.md) — registro transitorio de defectos en investigación.

## Licencia

Copyright © 2026 Cristián Rojas Arredondo — **MPL-2.0** ([LICENSE](LICENSE), avisos en [NOTICE](NOTICE)).
Motor Qwen3-TTS MIT/Apache-2.0; par de traducción `opus-mt` (Helsinki-NLP) CC-BY-4.0; dependencias en [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) (inventario Rust `Cargo.lock`). Disponibilidad de la fuente (MPL-2.0 §3.2) en [SOURCE-OFFER.md](SOURCE-OFFER.md).

## Comunidad y soporte

- [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues)
- Vulnerabilidades: reporte privado según [SECURITY.md](SECURITY.md).
