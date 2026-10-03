# Canales de distribución

`ai-voice-interconnector` se distribuye por un **único canal nativo**, publicado
en cada tag `v*`: **archivos comprimidos** (`tar.gz`/`.zip`) que agrupan el
binario Rust autocontenido con los documentos de licencia GPLv3 (ver
[docs/BUILD.md](BUILD.md)). La distribución es 100 % Rust.

## Tabla de contenidos

- [El canal nativo](#el-canal-nativo)
- [Formato de los artefactos](#formato-de-los-artefactos)
- [Instalación](#instalación)
- [Por qué el one-liner evita SmartScreen/Gatekeeper](#por-qué-el-one-liner-evita-smartscreengatekeeper)
- [Publicación, tap de Homebrew y antivirus](#publicación-tap-de-homebrew-y-antivirus)
- [Flujo de publicación (CI)](#flujo-de-publicación-ci)

## El canal nativo

| | Canal nativo |
|---|---|
| **Audiencia** | Cualquier usuario final (no requiere toolchain) |
| **Instalación** | One-liner por SO (`curl \| sh` / `irm \| iex`) o Homebrew Cask (macOS) |
| **Tamaño** | Binario Rust pequeño y autocontenido (el archivo total suma además `ort-bundle` + `qwen_tts` vendido; CTranslate2 (ct2rs) enlazado estático + Parakeet vía `ort` `load-dynamic` vía `crt-static`) |
| **Dependencias del sistema** | Ninguna (autocontenido) |
| **SmartScreen / Gatekeeper** | Bloquea el primer arranque si el binario se descarga por navegador; el one-liner lo evita (ver más abajo) |
| **Actualización** | Con el binario instalado: `self update --check` (informa sin modificar) o `self update`; el bootstrap no tiene `--check`; o `brew upgrade --cask` |
| **Desinstalación** | `ai-voice-interconnector self uninstall` (todo, con `--yes` para no preguntar) o `ai-voice-interconnector cleanup --model` solo para los modelos; en Homebrew `brew uninstall --cask --zap` |
| **Publicación en CI** | `publish-release` → GitHub Release; `publish-metadata` → Cask del tap |
| **Reversibilidad de la publicación** | El Release es público al publicarse: revertir implica borrar un Release ya público |

`setup` provisiona los modelos en la raíz de modelos **exclusiva de la
aplicación** (`models_cache_dir()`: `%LOCALAPPDATA%\ai-voice-interconnector\cache\models`
en Windows, `~/Library/Caches/ai-voice-interconnector/models` en macOS y
`$XDG_CACHE_HOME/ai-voice-interconnector/models` en Linux): ningún modelo viaja
dentro del archivo, se descargan en el primer `setup`.

## Formato de los artefactos

Cada uno de los 4 targets se publica como un archivo comprimido con **layout
plano** (binario + los 4 documentos de la raíz + `ort-bundle` + `qwen_tts`
vendido, todos en la raíz del archivo):

| Target | Asset del release | Binario interno |
|---|---|---|
| `build-linux-x64` | `ai-voice-interconnector-<ver>-x86_64-linux.tar.gz` | `ai-voice-interconnector` |
| `build-linux-arm64` | `ai-voice-interconnector-<ver>-arm64-linux.tar.gz` | `ai-voice-interconnector` |
| `build-darwin-arm64` | `ai-voice-interconnector-<ver>-arm64-macos.tar.gz` | `ai-voice-interconnector` |
| `build-windows-x64` | `ai-voice-interconnector-<ver>-x86_64-windows.zip` | `ai-voice-interconnector.exe` |

Los 4 documentos incluidos son `LICENSE`, `THIRD-PARTY-LICENSES.md`,
`SOURCE-OFFER.md` (oferta de fuente GPLv3 §6) y `README.md`. Al viajar dentro
del archivo, quedan instalados junto al binario, satisfaciendo el cumplimiento
GPLv3 sin depender del bundle. `SHA256SUMS.txt` se calcula sobre los 4 archivos
comprimidos más los 2 bootstrap (`install.sh`, `install.ps1`); el release
publica 7 assets.

## Instalación

Ver [README.md](../README.md#instalación) y [USAGE.md](../USAGE.md#instalación)
para el detalle completo por SO. Las tres plataformas tienen una **instalación
de una línea** (`curl | sh` / `irm | iex`) desde los assets del release: el
bootstrap descarga el archivo de su target, verifica el checksum, comprueba que
el binario arranca y delega en `self install`, que **integra el PATH por sí
mismo** y encadena `setup` (el `setup` solo provisiona modelos):

- **Linux** — `install.sh` (`curl | sh` sobre `releases/latest/download`)
  detecta la arquitectura del host (x86_64/arm64), verifica el checksum exacto
  contra `SHA256SUMS.txt`, comprueba el arranque (`binary_incompatible` con
  diagnóstico de glibc ≥ 2.35 si el binario no arranca) y delega en
  `self install`, que crea el symlink
  `~/.local/bin/ai-voice-interconnector` y encadena `setup`.
- **macOS** — el mismo `install.sh` (POSIX, `curl | sh`): descarga el `tar.gz`
  de arm64 (Apple Silicon; Mac Intel responde `unsupported_platform`),
  verifica el checksum, comprueba el arranque y delega en `self install`, que
  además limpia la cuarentena de Gatekeeper del programa instalado.
  **Vía complementaria** para usuarios de Homebrew: el Cask del tap propio
  (`brew tap CristianRojas-SoftwareEngineer/ai-voice-interconnector && brew
  install --cask ai-voice-interconnector`), que resuelve PATH, desinstalación
  (`--zap`) y cuarentena sin intervención manual, pero exige Homebrew y no
  provisiona los modelos.
- **Windows** — `install.ps1` (`irm | iex` sobre `releases/latest/download`)
  descarga el `.zip` x86_64 (ARM64 responde `unsupported_platform`),
  verifica su checksum, comprueba el arranque y delega en `self install`, que
  registra ese directorio en
  el PATH de usuario (HKCU, sin UAC) de forma idempotente y termina con
  `ai-voice-interconnector setup`.

El porqué de que `install.ps1` no dispare SmartScreen (a diferencia de
la descarga por navegador) está explicado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar) y en la sección siguiente.

## Por qué el one-liner evita SmartScreen/Gatekeeper

El mecanismo de Mark-of-the-Web/cuarentena que dispara SmartScreen y Gatekeeper
(detallado en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar)) solo lo añade
el **navegador** a un archivo descargado. Los one-liners descargan por CLI
(`curl`, `Invoke-WebRequest`), que no aplica Mark-of-the-Web, así que el archivo
extraído no lleva la marca y ninguno de los dos sistemas de reputación se
activa. En macOS, además, `self install` limpia `com.apple.quarantine` del
programa instalado (el bootstrap no toca la cuarentena). La resolución de raíz (firma de
código y notarización) sigue pendiente; ver `docs/BUILD.md` §"Limitación
conocida: firma de código y notarización".

## Publicación, tap de Homebrew y antivirus

**Principios de publicación.** Publicar una versión nueva no requiere la aprobación
ni la revisión de un tercero, ni un pull request a un proyecto externo. Los repos
propios —el tap de Homebrew— y la automatización de CI sobre el propio repositorio
están bajo control total del proyecto y no cuentan como terceros: un `git push` a un
repositorio propio no es un PR a un proyecto externo. Esto descarta los catálogos
oficiales (`winget-pkgs`, `homebrew-cask`, Flathub, Snap Store) como vía de
publicación. Toda la automatización de publicación vive en `.circleci/config.yml`
(CI único, sin GitHub Actions) y el job `publish-release` publica el GitHub Release
directo, sin borrador: sus assets son públicos en cuanto el job termina y
`releases/latest` apunta a la versión nueva sin desfase. El tag es el punto de no
retorno, y es lo que permite que un job posterior del mismo pipeline
(`publish-metadata`) lea los assets ya públicos.

**Prerrequisitos del canal de Homebrew.** El Cask de macOS depende de dos recursos
de una sola vez, ya creados:

- El repositorio tap `homebrew-ai-voice-interconnector` (público), que aloja
  `Casks/ai-voice-interconnector.rb`.
- El context de CircleCI `homebrew-tap`, con la variable `HOMEBREW_TAP_PAT` (un PAT
  fine-grained con permiso `Contents:RW` solo sobre el tap), que autoriza el push del
  Cask actualizado.

El bootstrap de una línea no necesita ningún recurso previo. `publish-metadata` crea
o reescribe `Casks/ai-voice-interconnector.rb` en el tap en cada release, y el único
prerrequisito es que el repositorio tap exista: regenerar y re-empujar produce el
mismo resultado, así que el reintento es seguro en cualquier momento.

**Experiencia de usuario del Cask.** Homebrew autoextrae el `tar.gz`, enlaza el
binario en el prefix (`/opt/homebrew/bin`, ya en el PATH) sin `sudo` y elimina el
atributo de cuarentena, con lo que mitiga Gatekeeper. Toda la integración de `PATH`,
la desinstalación (`brew uninstall --cask --zap`) y la limpieza de cuarentena las
resuelve Homebrew; solo el modelo queda pendiente, porque el Cask no puede correr
post-install: sus `caveats` remiten a `setup` y a la licencia GPL-3.0-or-later.

**Antivirus.** El Cask de macOS es la única vía que sí limpia la cuarentena. Los
one-liners no eliminan por sí mismos las alertas de antivirus; evitan SmartScreen
porque descargan por CLI (sección anterior), pero Microsoft Defender **Antivirus** es
independiente del Mark-of-the-Web y puede marcar el binario sin firma venga de donde
venga. La marca que Windows y macOS añaden a todo archivo bajado de internet, y que
es la que activa SmartScreen/Gatekeeper, no la lleva un archivo descargado por CLI;
el detalle completo está en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar).

**Runbook de reporte a Microsoft.** La vía de remediación es el reporte a WDSI
(*Windows Defender Security Intelligence*, `microsoft.com/wdsi`), donde se reportan
los falsos positivos de Defender para que los reclasifiquen; el paso a paso está en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar). Cubre solo la **detección de
Defender Antivirus** —una firma concreta (p. ej. `Trojan:Win32/Wacatac`) que, tras
revisión de un analista, Microsoft borra globalmente para todos los Defender—. **No**
desactiva SmartScreen, que es reputación y solo la resuelve la firma de código
(Authenticode en Windows, notarización en macOS). El reporte se puede hacer con el
binario sin firmar, y firmar no borra una detección ya existente (solo el reporte lo
hace). Sin firma, la reputación se acumula por archivo, así que cada versión nueva
puede requerir un reporte propio; con firma de código, la reputación se hereda entre
versiones y esa recurrencia disminuye mucho.

El estado de esta brecha por SO (mitigada, diferida a firma de código) vive en
[brechas conocidas de la especificación del ciclo de vida](specs/sdlc-lifecycle.md#brechas-conocidas).

La firma y notarización de los binarios sigue registrada como goal a largo plazo
en [docs/GOAL.md](GOAL.md#goal-a-largo-plazo) para cuando se cumplan sus
condiciones de entrada; hasta entonces, el one-liner es la vía que evita la
fricción de SmartScreen/Gatekeeper.

## Flujo de publicación (CI)

En cada tag `v*`, tras la triple puerta de tests (`test-linux`, `test-windows`,
`test-macos`) y `coverage`, los cuatro `build-*` compilan el binario, lo empaquetan
con los documentos de licencia en el archivo comprimido de su target y lo
persisten al workspace. Luego:

1. `publish-release` estampa la versión del tag en ambos bootstrap, recoge los
   4 archivos + los 2 bootstrap, calcula `SHA256SUMS.txt` sobre los 6 ficheros
   y crea el GitHub Release (`gh release create`) con los 7 assets.
2. `publish-metadata` (depende de `publish-release`) renderiza el Cask de
   Homebrew con `cargo xtask cask` — `binary` stanza sobre el `tar.gz` de
   macOS, con el `sha256` extraído de `SHA256SUMS.txt` — y lo empuja al tap.