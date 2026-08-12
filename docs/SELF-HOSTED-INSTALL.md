# Instalación auto-hospedada por sistema operativo

Este documento describe la mecánica interna de la **instalación auto-hospedada**
por sistema operativo: los instaladores de una línea (`curl | sh` / `irm | iex`)
y el Cask de Homebrew que envuelven el canal nativo (los artefactos PyInstaller
publicados como GitHub Release en cada tag `v*`) para dar el flujo descubrir →
instalar → comando disponible en el PATH → provisión guiada del modelo →
desinstalar.

Para la vista comparativa de canales de distribución (nativo vs. pip) y su
tabla de instalación/actualización/desinstalación, ver
[docs/DISTRIBUTION.md](DISTRIBUTION.md). Para el estado de paridad entre
sistemas operativos y las brechas cerradas, ver [docs/PARITY.md](PARITY.md).

## Tabla de contenidos

- [Principios de diseño](#principios-de-diseño)
- [Glosario](#glosario)
- [Requisitos previos de Homebrew](#requisitos-previos-de-homebrew)
- [Endurecimiento del build](#endurecimiento-del-build)
- [Instalador Linux (`curl | sh`)](#instalador-linux-curl--sh)
- [Instalador Windows (`irm | iex`)](#instalador-windows-irm--iex)
- [Cask de macOS](#cask-de-macos)
- [Instalador macOS (`curl | sh`)](#instalador-macos-curl--sh)
- [Desinstalación](#desinstalación)
- [Comportamiento frente a antivirus](#comportamiento-frente-a-antivirus)

## Principios de diseño

- **Publicación autónoma.** Publicar una versión nueva no requiere la aprobación ni
  la revisión de un tercero, ni un pull request a un proyecto externo. Los repos
  propios (el tap de Homebrew) y la automatización de CI sobre el propio repo están
  bajo control total del proyecto y no cuentan como terceros: un `git push` a un repo
  propio no es un PR a un proyecto externo. Esto descarta los catálogos oficiales
  (`winget-pkgs`, `homebrew-cask`, Flathub, Snap Store) como vía de publicación.
- **La fricción de instalación del usuario es aceptable.** Que el usuario ejecute
  comandos (`chmod +x`, `brew tap`, `brew install`) es esperable y no viola el
  principio anterior, que aplica solo a la publicación.
- **CI 100% en CircleCI.** Toda la automatización de publicación vive en
  `.circleci/config.yml`; el proyecto no usa GitHub Actions, para operar un solo
  sistema de CI.
- **Publicación directa del Release.** El job `publish-release` publica el GitHub
  Release directo, sin borrador: sus assets son públicos en cuanto el job termina, y
  `releases/latest` apunta a la versión nueva sin desfase. El tag es el punto de no
  retorno, igual que en `publish-pypi`. Esto es lo que permite que un job posterior
  del mismo pipeline lea los assets ya públicos.

**Registro de decisión (Windows).** El instalador auto-hospedado de Windows se
declaró inicialmente fuera de alcance, bajo la premisa de que todo instalador
descargado dispararía SmartScreen mientras el proyecto no tuviera firma de código
(Authenticode). La investigación empírica refutó esa premisa: la descarga por CLI
no aplica el Mark-of-the-Web (mecanismo detallado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar)), así que un instalador bajado
por script no dispara SmartScreen. El obstáculo restante era el UAC del instalador
per-machine original, eliminado al migrar el Inno Setup a **per-user**
(`PrivilegesRequired=lowest`, `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, PATH en
`HKCU\Environment`). La reserva que persiste: Microsoft Defender **Antivirus** es
independiente del MOTW y puede marcar el binario sin firma (runbook WDSI abajo);
la solución de SmartScreen para la descarga por navegador sigue siendo la firma de
código (`docs/GOAL.md`).

## Glosario

Términos externos usados en este documento:

- **AppImage / `.dmg`**: los formatos de artefacto nativo de Linux y macOS que el
  canal nativo produce. El instalador Linux y el Cask de macOS se apoyan en ellos
  tal cual, sin rehornearlos.
- **Cask**: la receta de Homebrew (`Casks/ai-voice-interconnector.rb`) que describe cómo instalar
  una aplicación distribuida como binario. Vive en un **tap**: un repositorio Git que
  Homebrew añade como fuente de recetas (`brew tap`).
- **Context de CircleCI**: un contenedor de variables de entorno secretas, visible
  solo por los jobs que lo declaran. Es el mecanismo con el que se inyectan las
  credenciales de publicación.
- **Canal pip / PyPI**: instalar con `pip`/`uv`/`pipx`. Descarga el paquete y genera
  el ejecutable en la máquina del usuario, por lo que no arrastra la marca de descarga
  ni dispara alertas del SO.
- **Gatekeeper (macOS) / SmartScreen (Windows)**: los sistemas que inspeccionan un
  archivo descargado de internet y advierten al usuario antes de ejecutarlo.
- **Mark-of-the-Web (MOTW)**: la marca que Windows y macOS añaden a todo archivo
  bajado de internet; es lo que activa a SmartScreen/Gatekeeper. Un archivo generado
  localmente (como el del canal pip) no la lleva. Detalle completo en
  [SECURITY.md](../SECURITY.md#artefactos-sin-firmar).
- **Firma de código (Authenticode en Windows, notarización en macOS)**: sellar el
  ejecutable con un certificado que prueba quién lo creó y que no fue alterado. Es lo
  que más reduce las alertas. Es un compromiso a futuro (ver `docs/GOAL.md`).
- **UPX**: un compresor de ejecutables. El malware lo usa para esconderse, así que su
  presencia eleva la sospecha del antivirus.
- **Metadata PE**: los campos de identidad (empresa, producto, versión) que un `.exe`
  de Windows puede llevar embebidos. Su ausencia hace el ejecutable más anónimo y
  sospechoso ante el clasificador de Microsoft Defender.
- **WDSI**: el portal de Microsoft (*Windows Defender Security Intelligence*,
  `microsoft.com/wdsi`) donde se reportan los falsos positivos de Defender para que
  los reclasifiquen.

## Requisitos previos de Homebrew

El Cask de macOS depende de dos recursos de una sola vez, ya creados:

- El repositorio tap `homebrew-ai-voice-interconnector` (público), que aloja
  `Casks/ai-voice-interconnector.rb`.
- El context de CircleCI `homebrew-tap`, con la variable `HOMEBREW_TAP_PAT` (un PAT
  fine-grained con permiso `Contents:RW` solo sobre el tap), que autoriza el push del
  Cask actualizado.

El instalador Linux no necesita ningún recurso previo.

## Endurecimiento del build

Los ejecutables de PyInstaller disparan la heurística de los antivirus: el patrón de
«desempaquetar y ejecutar», el bootloader genérico y la falta de señales de identidad
hacen que el clasificador los puntúe como sospechosos. El build aplica tres ajustes,
baratos y sin dependencia de terceros, que dan señales de confianza y una vía de
remediación:

- **`--noupx` en los flags compartidos** (`scripts/build_utils.py`,
  `common_pyinstaller_args()`): garantiza que el ejecutable nunca se comprima con UPX,
  aunque el servidor de CI tenga UPX instalado. Aplica a todos los builds de
  PyInstaller, incluido el bootloader del `.AppImage`.
- **Metadata PE en el `.exe` de Windows** (`scripts/build_windows.py`,
  `--version-file` con empresa, producto y versión): da al clasificador de Defender
  las señales de identidad que de otro modo faltan. Es exclusivo de Windows: el
  `.AppImage` es ELF, no PE.
- **Runbook de reporte a WDSI** (`SECURITY.md`, sección de artefactos sin firmar): una
  guía paso a paso para reportar a Microsoft cuando un release sea marcado por
  Defender. Cubre solo la **detección de Defender Antivirus** —una firma concreta
  (p. ej. `Trojan:Win32/Wacatac`) que, tras revisión de un analista, Microsoft borra
  globalmente para todos los Defender—. **No** desactiva SmartScreen, que es
  reputación y solo la resuelve la firma de código. El reporte se puede hacer con el
  binario sin firmar, y firmar no borra una detección ya existente (solo el reporte lo
  hace). Sin firma, la reputación se acumula por archivo, así que cada versión nueva
  puede requerir un reporte propio; con firma de código, la reputación se hereda entre
  versiones y esa recurrencia disminuye mucho.

## Instalador Linux (`curl | sh`)

`install-linux.sh` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-linux.sh`. Uso:
`curl -fsSL <url> | sh`.

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API (no
requiere autenticación en repos públicos) → leer `uname -m` → seleccionar el asset
`.AppImage` de la arquitectura → descargar el AppImage y `SHA256SUMS.txt` →
verificar el checksum → `chmod +x` → instalar en `~/.local/opt/ai-voice-interconnector/` →
`export APPIMAGE=<ruta>` e invocar `"$APPIMAGE" setup`, que integra el PATH y
descarga el modelo.

`_integrate_linux_path()` (`src/ai_voice_interconnector/cli.py`) activa el symlink de PATH
cuando la variable de entorno `APPIMAGE` está presente: exportar `APPIMAGE` desde
fuera es una entrada oficial y soportada, cubierta por `TestSetupLinuxPath` en
`tests/test_cli.py`. `cmd_setup()` es no interactivo, lo que permite invocarlo
directo desde el script.

**Limitaciones conocidas**: glibc < 2.35 (el script lo detecta y advierte); el PATH
no se propaga a la sesión actual que ejecutó el `curl | sh` (el CLI lo avisa; el
script no modifica `.bashrc`/`.zshrc` sin consentimiento).

## Instalador Windows (`irm | iex`)

`install-windows.ps1` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-windows.ps1`. Uso:
`irm <url> | iex`. Al no ser un `.ps1` en disco, `irm | iex` no pasa por la
Execution Policy; la alternativa inspeccionable es
`iwr <url> -OutFile install-windows.ps1; .\install-windows.ps1`.

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API →
seleccionar el asset `ai-voice-interconnector-*-x86_64-setup.exe` (solo hay build x86_64
para Windows: sin selección de arquitectura) → descargar el instalador y
`SHA256SUMS.txt` con `Invoke-WebRequest` (sin MOTW: no dispara SmartScreen) →
verificar el checksum (`Get-FileHash`; aborta si no coincide) → ejecutar el
instalador en silencio (`/VERYSILENT /SUPPRESSMSGBOXES /NORESTART`, sin
`-Verb RunAs`: la instalación es per-user, sin UAC) → recomponer el PATH de la
sesión desde el registro (el `HKCU\Environment` nuevo no llega solo a la sesión
en curso) → ejecutar `ai-voice-interconnector setup` (necesario porque `skipifsilent` omite
el checkbox de setup en instalación silenciosa; `-NoSetup` lo desactiva).

El Inno Setup generado por `scripts/create_installer_windows.py` es **per-user**:
`PrivilegesRequired=lowest`, instalación en
`%LOCALAPPDATA%\Programs\ai-voice-interconnector` (patrón convencional, p. ej. VS Code) y PATH
en `HKCU\Environment` en lugar de HKLM, con la reversión del PATH al desinstalar
sobre la misma clave. Nota de migración: quien tenga una versión per-machine
antigua debe desinstalarla primero (Panel de control, con admin); instalar la
per-user encima puede dejar dos instalaciones y PATH duplicado.

**Limitaciones conocidas**: Defender Antivirus puede marcar el binario sin firma
(independiente del MOTW; runbook WDSI arriba); el instalador descargado por
navegador sí lleva MOTW y dispara SmartScreen (lo resuelve la firma de código, no
este script).

## Cask de macOS

- `Casks/ai-voice-interconnector.rb` en el tap `homebrew-ai-voice-interconnector`, con las stanzas:
  `version`, `sha256`, `url` (al `.dmg` del release), `binary` apuntando a
  `Contents/MacOS/ai-voice-interconnector`, `livecheck` (`strategy :github_latest`),
  `zap trash:` (caché del modelo y datos de usuario) y `caveats` que sugiere
  `ai-voice-interconnector setup`.
- El job `publish-metadata` en `.circleci/config.yml`, con
  `requires: [publish-release]` y filtro de tag `only: /^v.*/`. Tras el Release
  público, lee la versión de `CIRCLE_TAG` y el `sha256` del `.dmg` desde
  `SHA256SUMS.txt` (recuperado con `gh release download "$CIRCLE_TAG" -p
  SHA256SUMS.txt`, de forma durable e idempotente, sin depender del workspace),
  reescribe el Cask y hace push al tap con el context `homebrew-tap`. Regenerar y
  re-empujar produce el mismo resultado, así que el reintento es seguro en cualquier
  momento.

**Experiencia de usuario**:
`brew tap <owner>/ai-voice-interconnector && brew install --cask ai-voice-interconnector`. Homebrew enlaza
el binario en el prefix (`/opt/homebrew/bin`, ya en el PATH) sin sudo, y elimina el
atributo de cuarentena, con lo que mitiga Gatekeeper. Toda la integración de PATH,
la desinstalación y la limpieza de cuarentena las resuelve Homebrew: el CLI no
necesita lógica específica de macOS para esta vía.

**Bootstrap**: el **primer** push del Cask al tap es manual (un paso de arranque
único, porque `publish-metadata` actualiza un Cask que ya debe existir); a partir de
ahí el job lo mantiene.

## Instalador macOS (`curl | sh`)

`install-macos.sh` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-macos.sh`. Uso:
`curl -fsSL <url> | sh`. Es la vía de una línea de macOS sin prerequisitos: ni
Homebrew (a diferencia del Cask) ni `sudo` (a diferencia del `.dmg` manual).

**Herramientas del host**: solo binarios del sistema base de macOS. No existe
`sha256sum` (se usa `shasum -a 256 -c`) ni `jq` (parseo con `grep`/`sed`, como
`install-linux.sh`); montaje con `hdiutil`, copia con `ditto`, limpieza de cuarentena
con `xattr`.

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API →
**guard de arquitectura** `uname -m` = `arm64` (Mac Intel no soportado; mensaje
claro) → seleccionar el asset `ai-voice-interconnector-*-arm64.dmg` → descargar el `.dmg` y
`SHA256SUMS.txt` → verificar el checksum con `shasum` (aborta si no coincide) →
`hdiutil attach -nobrowse -readonly -mountpoint <tmp>` → localizar el `.app` en
el volumen → copiar a `~/Applications` con `ditto` (reemplazando la versión
anterior si existe) → `hdiutil detach` → `xattr -dr com.apple.quarantine` sobre
el `.app` copiado (legítimo: el usuario ya expresó intención ejecutando el
script) → crear el symlink de PATH `~/.local/bin/ai-voice-interconnector → <app>/Contents/
MacOS/ai-voice-interconnector` → invocar `"<app>/Contents/MacOS/ai-voice-interconnector" setup`.

**Integración de PATH per-user**: `~/.local/bin` **no** está en el PATH por
defecto de zsh en macOS; el script detecta esa ausencia y emite el aviso con la
línea exacta para `~/.zshrc`, sin mutar dotfiles (mismo patrón que
`_integrate_linux_path`). Sin cambios de código en el CLI: la integración vive
en el propio script.

**Limitaciones conocidas**: solo Apple Silicon (el guard aborta en Intel); la firma
de código/notarización sigue diferida (goal a largo plazo), pero la limpieza de
cuarentena elimina la fricción de Gatekeeper para quien use el one-liner.

## Desinstalación

`ai-voice-interconnector setup --uninstall` deja el sistema idéntico a antes de instalar **en
un comando en los tres SO**: encadena `cleanup --all` (caché del modelo + datos de
usuario), revierte la integración de PATH y borra el binario, **en ese orden**
(datos independientes primero, ancla al final). Es un dispatch por SO sobre un
contrato compartido: cancelar el cleanup aborta la desinstalación sin borrar nada
(cancelación atómica, salida 0), y solo aplica al canal nativo (guard `is_frozen`;
desde fuente o pip/uv remite a `pip uninstall`).

El detalle por sistema operativo (qué hace `setup --uninstall` en Linux, macOS y
Windows, y el estado de esa paridad) vive en
[docs/PARITY.md](PARITY.md#fase-5--desinstalación).

## Comportamiento frente a antivirus

Los instaladores auto-hospedados no eliminan las alertas de antivirus por sí mismos,
salvo el Cask en macOS (que limpia la cuarentena). El porqué de que
`install-windows.ps1` no dispare SmartScreen (a diferencia de la descarga por
navegador) está explicado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar); Microsoft Defender
**Antivirus** es independiente del MOTW y puede marcar el binario sin firma venga de
donde venga (mitigado por el endurecimiento del build de arriba y el runbook WDSI).

El estado de esta brecha por SO (mitigada, diferida a firma de código) vive en
[docs/PARITY.md](PARITY.md#fase-2--primer-arranque-reputación-del-binario-sin-firmar).
