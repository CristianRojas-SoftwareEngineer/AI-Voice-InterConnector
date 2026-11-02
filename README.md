# AI Voice-InterConnector

Sistema de voz con IA **100% local** en español e inglés: transcripción, texto a voz con clonación de voz y traducción de voz a voz. Corre en CPU, sin GPU ni APIs externas, y funciona sin conexión una vez descargados los modelos. La síntesis usa el motor Qwen3-TTS 0.6B ([QwenLM/Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)), la transcripción Parakeet TDT 0.6B v3 y la traducción opus-mt. Se distribuye como un único archivo por plataforma, para Windows x64, Linux x64/ARM64 y macOS con Apple Silicon.

## Tabla de contenidos

- [Uso ético y responsable](#uso-ético-y-responsable)
- [Requisitos](#requisitos)
- [Instalación](#instalación)
  - [Windows](#windows)
  - [Linux](#linux)
  - [macOS](#macos)
  - [Descargar binario pre-compilado](#descargar-binario-pre-compilado)
    - [Primer arranque: SmartScreen / Gatekeeper](#primer-arranque-smartscreen--gatekeeper)
- [Primeros pasos](#primeros-pasos)
  - [Modelos](#modelos)
  - [Síntesis](#síntesis)
  - [Clonación de voz](#clonación-de-voz)
  - [Transcripción y traducción](#transcripción-y-traducción)
  - [Traducción de voz a voz](#traducción-de-voz-a-voz)
  - [Limpieza de audio y vídeo](#limpieza-de-audio-y-vídeo)
  - [Invocación desde cualquier lenguaje](#invocación-desde-cualquier-lenguaje)
- [Actualizar y desinstalar](#actualizar-y-desinstalar)
- [Arquitectura](#arquitectura)
- [Documentación](#documentación)
  - [Para usuarios](#para-usuarios)
  - [Para contribuidores](#para-contribuidores)
- [Licencia](#licencia)
- [Comunidad y soporte](#comunidad-y-soporte)

## Uso ético y responsable

AI Voice-InterConnector puede clonar cualquier voz, y **el audio que genera no lleva marca de agua**: no se distingue por medios técnicos de una grabación real. Úsalo de forma responsable:

- **Consentimiento**: clona solo voces para las que tengas permiso explícito de su titular.
- **No suplantación**: no lo uses para hacerte pasar por otra persona, cometer fraude, difamar ni producir contenido engañoso.
- **Divulgación**: al publicar o compartir audio sintetizado, indica que lo es.
- **Reporte**: si detectas un uso indebido del proyecto, abre un [Issue](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues).

El proyecto no impone barreras técnicas, que serían fáciles de sortear en software libre: la responsabilidad del uso legítimo recae en quien lo emplea.

## Requisitos

| Sistema | Versión mínima | Nada más que instalar porque… |
|---|---|---|
| Windows | Windows 10 1903 o posterior, x64 | el archivo trae las DLL del runtime de VC++ |
| Linux | glibc ≥ 2.28 (Debian 10+, Ubuntu 20.04+, RHEL/Rocky/Alma 8+, Fedora 29+), x64 o ARM64 | solo usa las librerías de C y C++ que trae toda distribución con glibc |
| macOS | macOS 26 (Tahoe) o posterior, Apple Silicon (M1 en adelante) | el archivo trae todo lo necesario |

No están soportados los Mac Intel ni Windows ARM64. En Linux, la reproducción y la captura de audio usan ALSA, que traen las distribuciones de escritorio; sin ALSA (servidores o contenedores mínimos), el programa funciona, pero los comandos que reproducen o graban audio fallan con un error que lo explica.

Toda la inferencia corre en CPU:

- **CPU**: x86-64 o ARM64 moderna; en procesadores antiguos la síntesis es más lenta.
- **RAM**: 8 GB recomendados, 4 GB mínimo. Con menos memoria funciona, pero los textos largos se ralentizan. `doctor` no comprueba la CPU ni la RAM.
- **Disco**: unos 5,9 GB para los modelos (los dos de Qwen3-TTS suman 5 GB; Parakeet, opus-mt y DPDFNet, el resto) y entre 150 y 320 MB para el programa, según la plataforma. En sistemas de archivos sin enlaces duros (FAT32, exFAT y algunos recursos de red), el espacio de los modelos se duplica.

Detalle por plataforma en [docs/BUILD.md](docs/BUILD.md#dependencias-de-runtime-por-plataforma).

## Instalación

La instalación recomendada es de una línea: no pide privilegios de administrador, verifica el checksum del archivo contra `SHA256SUMS.txt`, añade el programa al PATH y descarga los modelos. Cada release publica sus propios instaladores; nada se sirve desde `main`. Más detalle sobre los canales en [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md).

### Windows

En PowerShell:

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
```

### Linux

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

El instalador detecta si el equipo es x64 o ARM64 y comprueba que el binario arranca antes de instalarlo.

### macOS

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

Como alternativa, con Homebrew:

```bash
brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector
brew install --cask ai-voice-interconnector
ai-voice-interconnector setup
```

Homebrew verifica el checksum, deja el binario en el PATH y quita la cuarentena de Gatekeeper, pero no descarga los modelos: por eso hay que ejecutar `setup`.

### Descargar binario pre-compilado

Si prefieres no usar el instalador, descarga el archivo de tu plataforma y `SHA256SUMS.txt` desde [Releases](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases), verifica el checksum **antes de ejecutar nada** e instala con `self install`, que hace lo mismo que el instalador de una línea: copia el programa, lo añade al PATH y descarga los modelos. Sustituye `X.Y.Z` por la versión que descargas.

Windows (PowerShell):

```powershell
$V = "X.Y.Z"; $F = "ai-voice-interconnector-$V-x86_64-windows.zip"
$U = "https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v$V"
Invoke-WebRequest "$U/$F" -OutFile $F; Invoke-WebRequest "$U/SHA256SUMS.txt" -OutFile SHA256SUMS.txt
$esperado = ((Get-Content SHA256SUMS.txt) -match " $F$" -split " ")[0]
if ((Get-FileHash $F -Algorithm SHA256).Hash -ne $esperado) { throw "El checksum no coincide" }
Expand-Archive $F -DestinationPath ai-voice-interconnector
.\ai-voice-interconnector\ai-voice-interconnector.exe self install
```

Linux (`x86_64` o `arm64`):

```bash
V=X.Y.Z; F=ai-voice-interconnector-$V-x86_64-linux.tar.gz
U=https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v$V
curl -fsSLO "$U/$F" && curl -fsSLO "$U/SHA256SUMS.txt"
sha256sum -c SHA256SUMS.txt --ignore-missing
mkdir ai-voice-interconnector && tar -xzf "$F" -C ai-voice-interconnector
./ai-voice-interconnector/ai-voice-interconnector self install
```

macOS:

```bash
V=X.Y.Z; F=ai-voice-interconnector-$V-arm64-macos.tar.gz
U=https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/download/v$V
curl -fsSLO "$U/$F" && curl -fsSLO "$U/SHA256SUMS.txt"
grep " $F\$" SHA256SUMS.txt | shasum -a 256 -c
mkdir ai-voice-interconnector && tar -xzf "$F" -C ai-voice-interconnector
./ai-voice-interconnector/ai-voice-interconnector self install
```

El archivo trae, además del binario, el motor TTS, ONNX Runtime, FFmpeg y los avisos de licencia; por eso se extrae en un directorio propio, que puedes borrar después de `self install`.

#### Primer arranque: SmartScreen / Gatekeeper

Solo afecta a Windows y macOS, y solo si descargaste el archivo **con el navegador**: los binarios no están firmados ni notarizados, y el sistema bloquea la primera ejecución de un ejecutable descargado sin historial. No significa que contenga malware; el porqué está en [SECURITY.md](SECURITY.md#artefactos-sin-firmar). Las descargas desde la terminal, como las de arriba y las del instalador de una línea, no lo provocan. Linux no tiene un bloqueo equivalente.

Verifica el checksum antes de aceptar. Si un antivirus pone el archivo en cuarentena, restáuralo y añade una exclusión solo después de verificarlo.

- **Windows (SmartScreen)**: en «Windows protegió tu PC», pulsa **Más información** → **Ejecutar de todas formas**. Si el navegador bloqueó la descarga, consérvala desde su menú de descargas: **Conservar** → **Conservar de todas formas**.
- **macOS (Gatekeeper)**: haz clic derecho sobre el binario → **Abrir** y confirma, o quita la cuarentena desde la terminal:

  ```bash
  xattr -d com.apple.quarantine ai-voice-interconnector
  ```

La firma Authenticode y la notarización de Apple son un objetivo a largo plazo ([docs/GOAL.md](docs/GOAL.md)).

## Primeros pasos

### Modelos

Los modelos no vienen en el programa. El instalador de una línea y `self install` los descargan al terminar; si instalaste con Homebrew o se interrumpió la descarga, ejecuta:

```bash
ai-voice-interconnector setup
ai-voice-interconnector doctor
```

`setup` no necesita cuenta y, si los modelos ya están, termina al instante. `doctor` comprueba que todo está listo. Hasta descargar los modelos, los comandos que los usan fallan con un mensaje que remite a `setup`. Detalle de cada modelo en [USAGE.md](USAGE.md#modelos-setup).

### Síntesis

Síntesis en español o en inglés con [`speech say`](USAGE.md#speech-say) (reproducción al momento) o [`speech synthesize`](USAGE.md#speech-synthesize) (guardado como WAV).

```bash
ai-voice-interconnector speech say --text "Hola mundo"
ai-voice-interconnector speech say --text "Hello world" --target-language en
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo
```

`speech say` reproduce sin guardar; `speech synthesize` guarda el WAV y muestra su ruta. Sin `--voice` se usa la voz de fábrica `default`, que viene incluida en el programa.

### Clonación de voz

[`voice clone`](USAGE.md#voice-clone) crea una voz a partir de unos 10 segundos de audio (archivo WAV o micrófono) y úsala para hablar en español o en inglés.

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference referencia.wav
ai-voice-interconnector voice clone --name mi_voz --mic --duration 10
ai-voice-interconnector speech say --text "Hola mundo" -v mi_voz
```

La referencia ideal es un solo audio de unos 10 segundos de habla limpia, en archivo WAV o grabado con `--mic`. `--duration` fija la grabación entre 1 y 30 segundos; sin ella, en una terminal interactiva se graba hasta que pulsas Enter. El recorrido completo está en [USAGE.md](USAGE.md#clonación-de-voz-recorrido-completo).

### Transcripción y traducción

[`speech transcribe`](USAGE.md#speech-transcribe) convierte voz a texto desde un archivo WAV o el micrófono. [`translate`](USAGE.md#translate) traduce texto entre español e inglés.

```bash
ai-voice-interconnector speech transcribe --audio grabacion.wav --source-language es
ai-voice-interconnector speech transcribe --mic --source-language en
ai-voice-interconnector translate --text "Hola, ¿cómo estás?" --source-language es --target-language en
```

### Traducción de voz a voz

[`speech dub`](USAGE.md#speech-dub) transcribe lo que dices, lo traduce y lo reproduce con una voz clonada; no genera un archivo.

```bash
ai-voice-interconnector speech dub --mic --source-language es --target-language en -v mi_voz
ai-voice-interconnector speech dub --audio grabacion.wav --source-language en --target-language es
```

`speech dub` transcribe lo que dices, lo traduce si los idiomas difieren y lo reproduce con la voz elegida (`default` si no pasas `-v`). Muestra la transcripción y la traducción, y no guarda ningún archivo.

### Limpieza de audio y vídeo

[`speech denoise`](USAGE.md#speech-denoise) limpia archivos de audio y la pista de audio de un vídeo, sin tocar su imagen.

```bash
ai-voice-interconnector speech denoise entrevista.mp4
ai-voice-interconnector speech denoise voz.wav --preset default
```

`speech denoise` limpia archivos de audio o la pista de audio de un vídeo, sin procesar la imagen. Usa el preset `studio` por omisión; `--preset default` aplica el perfil original, sin normalización. Opciones, formatos y límites en la [referencia de `speech denoise`](docs/CLI/commands/SPEECH.md#speech-denoise).

### Invocación desde cualquier lenguaje

Cada comando acepta `--json` para leer su resultado desde otro programa, y un daemon opcional mantiene los motores cargados entre invocaciones. El programa se invoca como cualquier proceso externo. Con la opción global `--json`, cada comando escribe su resultado como JSON en stdout, de modo que no hace falta interpretar texto. Por ejemplo, en Python:

```python
import json, subprocess

salida = subprocess.run(
    ["ai-voice-interconnector", "--json", "speech", "synthesize", "--text", "Hola mundo", "--label", "saludo"],
    capture_output=True, text=True, check=True,
).stdout
print(json.loads(salida)["audio_path"])
```

Todos los comandos y sus salidas `--json` están en [USAGE.md](USAGE.md); el contrato de la CLI, en [docs/CLI/README.md](docs/CLI/README.md).

## Actualizar y desinstalar

El programa se gestiona a sí mismo:

```bash
ai-voice-interconnector self update --check               # informa si hay una versión nueva, sin cambiar nada
ai-voice-interconnector self update --yes                 # actualiza a la última versión estable, con verificación
ai-voice-interconnector self uninstall --yes              # desinstala el programa, el PATH y los datos
ai-voice-interconnector self uninstall --keep-data --yes  # conserva los modelos, las voces y los audios
ai-voice-interconnector cleanup --all --yes               # borra los datos sin tocar el programa ni el PATH
```

Si instalaste con Homebrew, usa sus órdenes; `--zap` borra también los modelos y los datos:

```bash
brew upgrade --cask ai-voice-interconnector
brew uninstall --cask --zap ai-voice-interconnector
```

Detalle en [USAGE.md](USAGE.md#ciclo-de-vida-de-la-instalación-self).

## Arquitectura

Un único binario Rust expone la CLI. La síntesis la hace el motor Qwen3-TTS, que corre como proceso residente y se comunica por HTTP en loopback; la transcripción (Parakeet, ONNX Runtime) y la traducción (CTranslate2) corren dentro del proceso. Un daemon opcional mantiene los motores cargados entre invocaciones. Detalle en [docs/DESIGN.md](docs/DESIGN.md#arquitectura).

## Documentación

### Para usuarios

- [USAGE.md](USAGE.md) — guía de uso: comandos, modelos, ciclo de vida, daemon y solución de problemas.
- [docs/CLI/README.md](docs/CLI/README.md) — referencia por comando de la CLI y códigos de salida.
- [docs/CLI/CONTRACT.md](docs/CLI/CONTRACT.md) — contrato normativo de la CLI y de la salida `--json`.
- [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) — canales de distribución, Homebrew y antivirus.
- [SECURITY.md](SECURITY.md) — seguridad y reporte de vulnerabilidades.
- [CHANGELOG.md](CHANGELOG.md) — historial de cambios.

### Para contribuidores

- [CONTRIBUTING.md](CONTRIBUTING.md) — cómo contribuir: entorno, tests y convenciones.
- [docs/DESIGN.md](docs/DESIGN.md) — diseño técnico y arquitectura.
- [docs/BUILD.md](docs/BUILD.md) — compilación y pipeline.
- [docs/RELEASING.md](docs/RELEASING.md) — publicación de releases.
- [docs/CI.md](docs/CI.md) — integración continua.
- [docs/GOAL.md](docs/GOAL.md) — meta y criterios de aceptación.
- [docs/specs/LIFECYCLE.md](docs/specs/LIFECYCLE.md) — especificación del ciclo de vida y paridad entre sistemas.
- [docs/specs/VOICE_CLONING.md](docs/specs/VOICE_CLONING.md) — especificación funcional de la clonación de voz.
- [docs/MANUAL-VALIDATION.md](docs/MANUAL-VALIDATION.md) — validación manual de la CLI.
- [docs/dev/CONTEXT-OPTIMIZATION.md](docs/dev/CONTEXT-OPTIMIZATION.md) — optimización de contexto para agentes de código.
- [AGENTS.md](AGENTS.md) — guías para agentes de código.
- [SOURCE-OFFER.md](SOURCE-OFFER.md) — disponibilidad de la fuente (MPL-2.0 §3.2).
- [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) — inventario y avisos de licencias de terceros.
- [docs/issues/README.md](docs/issues/README.md) — registro de defectos en investigación.

## Licencia

Copyright © 2026 Cristián Rojas Arredondo — **MPL-2.0** ([LICENSE](LICENSE), avisos en [NOTICE](NOTICE)). Modelos descargados con `setup` bajo sus propias licencias: Qwen3-TTS y DPDFNet Apache-2.0; Parakeet TDT 0.6B v3 y el par de traducción `opus-mt` (Helsinki-NLP) CC-BY-4.0. Los avisos de licencia de las dependencias y de los componentes del paquete están en [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md), y la disponibilidad de la fuente (MPL-2.0 §3.2), incluida la de FFmpeg, en [SOURCE-OFFER.md](SOURCE-OFFER.md).

## Comunidad y soporte

- [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues)
- Vulnerabilidades: reporte privado según [SECURITY.md](SECURITY.md).
