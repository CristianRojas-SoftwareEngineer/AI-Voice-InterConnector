# Canales de distribución

`ai-voice-interconnector` se distribuye por un **único canal nativo**, publicado
en cada tag `v*`: **archivos comprimidos** (`tar.gz`/`.zip`) que agrupan el
binario Rust autocontenido con los documentos de licencia GPLv3 (ver
[docs/BUILD.md](BUILD.md)). El canal **PyPI** (`pip`/`uv tool`/`pipx`) fue
**retirado en la Fase 7**: el motor real ya es Rust, así que el repositorio
quedó 100 % Rust en su distribución.

## Tabla de contenidos

- [El canal nativo](#el-canal-nativo)
- [Formato de los artefactos](#formato-de-los-artefactos)
- [Instalación](#instalación)
- [Por qué el one-liner evita SmartScreen/Gatekeeper](#por-qué-el-one-liner-evita-smartscreengatekeeper)
- [Canal PyPI retirado](#canal-pypi-retirado)
- [Flujo de publicación (CI)](#flujo-de-publicación-ci)

## El canal nativo

| | Canal nativo |
|---|---|
| **Audiencia** | Cualquier usuario final (no requiere Python ni toolchain) |
| **Instalación** | One-liner por SO (`curl \| sh` / `irm \| iex`) o Homebrew Cask (macOS) |
| **Tamaño** | Binario pequeño y autocontenido (~13-42 MB; CTranslate2 (ct2rs) enlazado estático + Parakeet vía `ort` `load-dynamic` vía `crt-static`) |
| **Dependencias del sistema** | Ninguna (autocontenido) |
| **SmartScreen / Gatekeeper** | Bloquea el primer arranque si el binario se descarga por navegador; el one-liner lo evita (ver más abajo) |
| **Actualización** | Re-ejecutar el one-liner por SO con `--check` (reporta la transición sin instalar), `upgrade-ai-voice-interconnector.{sh,ps1}` (wrapper), o `brew upgrade --cask` |
| **Desinstalación** | Eliminar el directorio de instalación + la entrada de PATH; `ai-voice-interconnector cleanup` para los modelos; en Homebrew `brew uninstall --cask --zap` |
| **Publicación en CI** | `publish-release` → GitHub Release; `publish-metadata` → Cask del tap |
| **Reversibilidad de la publicación** | El Release es público al publicarse: revertir implica borrar un Release ya público |

`setup` (default `--language es`) provisiona los modelos en la caché de
HuggingFace del usuario (`~/.cache/huggingface/hub`): ningún modelo viaja
dentro del archivo, se descargan en el primer `setup`.

## Formato de los artefactos

Cada uno de los 4 targets se publica como un archivo comprimido con **layout
plano** (binario + los 4 documentos de la raíz, todos en la raíz del archivo):

| Target | Asset del release | Binario interno |
|---|---|---|
| `build-linux-x64` | `ai-voice-interconnector-<ver>-x86_64-linux.tar.gz` | `ai-voice-interconnector` |
| `build-linux-arm64` | `ai-voice-interconnector-<ver>-arm64-linux.tar.gz` | `ai-voice-interconnector` |
| `build-darwin-arm64` | `ai-voice-interconnector-<ver>-arm64-macos.tar.gz` | `ai-voice-interconnector` |
| `build-windows-x64` | `ai-voice-interconnector-<ver>-x86_64-windows.zip` | `ai-voice-interconnector.exe` |

Los 4 documentos incluidos son `LICENSE`, `THIRD-PARTY-LICENSES.md`,
`SOURCE-OFFER.md` (oferta de fuente GPLv3 §6) y `README.md`. Al viajar dentro
del archivo, quedan instalados junto al binario, satisfaciendo el cumplimiento
GPLv3 sin depender del bundle. `SHA256SUMS.txt` se calcula sobre los archivos
comprimidos.

## Instalación

Ver [README.md](../README.md#instalación) y [USAGE.md](../USAGE.md#instalación)
para el detalle completo por SO. Las tres plataformas tienen una **instalación
auto-hospedada de una línea** (`curl | sh` / `irm | iex`), con un script por SO
que descarga, verifica el checksum, extrae el archivo e **integra el PATH por sí
mismo** (dado que el `setup` del binario Rust ya no lo hace: solo provisiona
modelos):

- **Linux** — `install-linux.sh` (`curl | sh`) selecciona el `tar.gz` de la
  arquitectura del host, verifica el checksum (`sha256sum`), lo extrae en
  `~/.local/opt/ai-voice-interconnector/` (limpiando la versión anterior), crea
  el symlink `~/.local/bin/ai-voice-interconnector` y encadena `setup`.
- **macOS** — `install-macos.sh` (`curl | sh`) descarga el `tar.gz` de arm64,
  verifica el checksum (`shasum`), lo extrae en
  `~/.local/opt/ai-voice-interconnector/`, limpia la cuarentena de Gatekeeper
  del binario, crea el symlink per-user en `~/.local/bin` y encadena `setup`.
  **Vía complementaria** para usuarios de Homebrew: el Cask del tap propio
  (`brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector && brew
  install --cask ai-voice-interconnector`), que resuelve PATH, desinstalación
  (`--zap`) y cuarentena sin intervención manual, pero exige Homebrew y no
  provisiona los modelos.
- **Windows** — `install-windows.ps1` (`irm | iex`) descarga el `.zip` x86_64,
  verifica su checksum, lo extrae en
  `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, registra ese directorio en
  el PATH de usuario (HKCU, sin UAC) de forma idempotente y termina con
  `ai-voice-interconnector setup`.

El porqué de que `install-windows.ps1` no dispare SmartScreen (a diferencia de
la descarga por navegador) está explicado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar); diseño completo de los
tres instaladores en [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

## Por qué el one-liner evita SmartScreen/Gatekeeper

El mecanismo de Mark-of-the-Web/cuarentena que dispara SmartScreen y Gatekeeper
(detallado en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar)) solo lo añade
el **navegador** a un archivo descargado. Los one-liners descargan por CLI
(`curl`, `Invoke-WebRequest`), que no aplica Mark-of-the-Web, así que el archivo
extraído no lleva la marca y ninguno de los dos sistemas de reputación se
activa. En macOS, además, `install-macos.sh` limpia explícitamente
`com.apple.quarantine` del binario extraído. La resolución de raíz (firma de
código y notarización) sigue pendiente; ver `docs/BUILD.md` §"Limitación
conocida: firma de código y notarización".

## Canal PyPI retirado

**Contexto histórico**: tras el release `v0.1.1`, los binarios nativos seguían
sin firma de código, disparando SmartScreen/Gatekeeper en cada primer arranque.
Se adoptaron dos estrategias no excluyentes: **A** (añadir el canal PyPI, sin el
problema de Mark-of-the-Web) y **B** (firmar/notarizar los binarios nativos).

Con la migración a Rust, el motor real dejó de ser Python: el paquete PyPI pasó
a envolver un binario Rust y perdió su razón de ser. En la **Fase 7** se
**retiró el canal PyPI** (job `publish-pypi`, su nodo en el workflow y el
context `pypi-publish`), dejando la distribución 100 % Rust por archivos
comprimidos. La estrategia **B** (firma/notarización) sigue registrada como goal
a largo plazo en [docs/GOAL.md](GOAL.md#goal-a-largo-plazo) para cuando se
cumplan sus condiciones de entrada; hasta entonces, el one-liner es la vía que
evita la fricción de SmartScreen/Gatekeeper.

## Flujo de publicación (CI)

En cada tag `v*`, tras la triple puerta de tests (`test-linux`, `test-windows`,
`test-macos`) y `coverage`, los cuatro `build-*` compilan el binario, lo empaquetan
con los documentos de licencia en el archivo comprimido de su target y lo
persisten al workspace. Luego:

1. `publish-release` recoge los 4 archivos, calcula `SHA256SUMS.txt` sobre ellos
   y crea el GitHub Release (`gh release create`) con los archivos + el checksum.
2. `publish-metadata` (depende de `publish-release`) renderiza el Cask de
   Homebrew con `cargo xtask cask` — `binary` stanza sobre el `tar.gz` de
   macOS, con el `sha256` extraído de `SHA256SUMS.txt` — y lo empuja al tap.

No hay job de publicación a PyPI: fue retirado en la Fase 7 (ver arriba).
