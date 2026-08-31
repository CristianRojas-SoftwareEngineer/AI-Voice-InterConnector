# Instalación auto-hospedada por sistema operativo

Este documento describe la mecánica interna de la **instalación auto-hospedada**
por sistema operativo: los instaladores de una línea (`curl | sh` / `irm | iex`)
y el Cask de Homebrew que envuelven el canal nativo (los **archivos comprimidos**
—`tar.gz`/`.zip`— que agrupan el binario Rust autocontenido con los documentos de
licencia GPLv3, publicados como GitHub Release en cada tag `v*`) para dar el flujo
descubrir → instalar → comando disponible en el PATH → provisión guiada del
modelo → desinstalar.

Para la vista de los canales de distribución y su tabla de
instalación/actualización/desinstalación, ver
[docs/DISTRIBUTION.md](DISTRIBUTION.md). Para el estado de paridad entre
sistemas operativos y las brechas abiertas, ver [docs/PARITY.md](PARITY.md).

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
  retorno. Esto es lo que permite que un job posterior del mismo pipeline
  (`publish-metadata`) lea los assets ya públicos.

**Registro de decisión (Windows).** El instalador auto-hospedado de Windows se
declaró inicialmente fuera de alcance, bajo la premisa de que todo instalador
descargado dispararía SmartScreen mientras el proyecto no tuviera firma de código
(Authenticode). La investigación empírica refutó esa premisa: la descarga por CLI
no aplica el Mark-of-the-Web (mecanismo detallado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar)), así que un artefacto bajado
por script no dispara SmartScreen. Con la migración a Rust ya no hay instalador Inno
per-machine (ni su UAC): el artefacto es un `.zip` que el propio `install-windows.ps1`
extrae en `%LOCALAPPDATA%\Programs\ai-voice-interconnector` y registra en el PATH de
usuario (`HKCU\Environment`), sin privilegios de admin. La reserva que persiste:
Microsoft Defender **Antivirus** es independiente del MOTW y puede marcar el binario
sin firma (runbook WDSI abajo); la solución de SmartScreen para la descarga por
navegador sigue siendo la firma de código (`docs/GOAL.md`).

## Glosario

Términos externos usados en este documento:

- **Archivo comprimido (`tar.gz` / `.zip`)**: el formato de artefacto nativo que el
  canal produce por target. Agrupa, en layout plano, el binario Rust autocontenido y
  los 4 documentos de licencia GPLv3. Los instaladores lo descargan y lo extraen; el
  Cask de macOS lo autoextrae.
- **Cask**: la receta de Homebrew (`Casks/ai-voice-interconnector.rb`) que describe cómo instalar
  una aplicación distribuida como binario. Vive en un **tap**: un repositorio Git que
  Homebrew añade como fuente de recetas (`brew tap`).
- **Context de CircleCI**: un contenedor de variables de entorno secretas, visible
  solo por los jobs que lo declaran. Es el mecanismo con el que se inyectan las
  credenciales de publicación.
- **Gatekeeper (macOS) / SmartScreen (Windows)**: los sistemas que inspeccionan un
  archivo descargado de internet y advierten al usuario antes de ejecutarlo.
- **Mark-of-the-Web (MOTW)**: la marca que Windows y macOS añaden a todo archivo
  bajado de internet; es lo que activa a SmartScreen/Gatekeeper. Un archivo generado
  o descargado por CLI (como el de los one-liners) no la lleva. Detalle completo en
  [SECURITY.md](../SECURITY.md#artefactos-sin-firmar).
- **Firma de código (Authenticode en Windows, notarización en macOS)**: sellar el
  ejecutable con un certificado que prueba quién lo creó y que no fue alterado. Es lo
  que más reduce las alertas. Es un compromiso a futuro (ver `docs/GOAL.md`).
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

El artefacto es un **binario Rust autocontenido**, enlazado estáticamente al CRT
(`+crt-static`; CTranslate2 (ct2rs) compilado estático + Parakeet vía `ort`
`load-dynamic`), no un bundle empaquetado: desaparece el patrón
«desempaquetar y ejecutar» que elevaba la
puntuación heurística del clasificador. Aun así, el binario **sigue sin firma de
código**, y Microsoft Defender **Antivirus** puede marcarlo por reputación baja,
independiente del MOTW.

La vía de remediación que se conserva es el **runbook de reporte a WDSI**
(`SECURITY.md`, sección de artefactos sin firmar): una guía paso a paso para reportar
a Microsoft cuando un release sea marcado por Defender. Cubre solo la **detección de
Defender Antivirus** —una firma concreta (p. ej. `Trojan:Win32/Wacatac`) que, tras
revisión de un analista, Microsoft borra globalmente para todos los Defender—. **No**
desactiva SmartScreen, que es reputación y solo la resuelve la firma de código. El
reporte se puede hacer con el binario sin firmar, y firmar no borra una detección ya
existente (solo el reporte lo hace). Sin firma, la reputación se acumula por archivo,
así que cada versión nueva puede requerir un reporte propio; con firma de código, la
reputación se hereda entre versiones y esa recurrencia disminuye mucho.

## Instalador Linux (`curl | sh`)

`install-linux.sh` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-linux.sh`. Uso:
`curl -fsSL <url> | sh`.

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API (no
requiere autenticación en repos públicos) → leer `uname -m` → seleccionar el asset
`ai-voice-interconnector-*-<arch>-linux.tar.gz` de la arquitectura → descargar el
`tar.gz` y `SHA256SUMS.txt` → verificar el checksum (`sha256sum`; aborta si no
coincide) → extraer en `~/.local/opt/ai-voice-interconnector/` (limpiando la versión
anterior) → crear el symlink `~/.local/bin/ai-voice-interconnector` al binario
extraído → invocar `ai-voice-interconnector setup`, que descarga el modelo.

La **integración de PATH la hace el propio script** (crea el symlink en
`~/.local/bin` y avisa si ese directorio no está en el PATH), porque el `setup` del
binario Rust ya no la realiza: `setup` solo provisiona el modelo. El script no muta
`.bashrc`/`.zshrc` sin consentimiento.

**Limitaciones conocidas**: glibc < 2.35 (el script lo detecta y advierte; el target
`gnu` no enlaza glibc estáticamente aunque el resto sea `crt-static`); el PATH no se
propaga a la sesión actual que ejecutó el `curl | sh` (el script lo avisa con la
línea exacta a añadir).

**Modo `--check`**: ejecutar `sh install-linux.sh --check` compara la versión
instalada (`ai-voice-interconnector --version`) con la última publicada en
GitHub y reporta la transición (`1.0.0 → 2.0.0`), `Ya estás en la versión`, o
`no instalado → 2.0.0` sin descargar ni extraer nada.

**Wrapper de auto-actualización**: `upgrade-ai-voice-interconnector.sh`
detecta el SO (`uname -s`), obtiene la versión corriente y la última, reporta
la transición y, salvo que se pase `--check`, re-ejecuta el one-liner del SO
correspondiente (`install-linux.sh` o `install-macos.sh`). Es el punto de
entrada unificado para la auto-actualización desde shell.

## Instalador Windows (`irm | iex`)

`install-windows.ps1` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-windows.ps1`. Uso:
`irm <url> | iex`. Al no ser un `.ps1` en disco, `irm | iex` no pasa por la
Execution Policy; la alternativa inspeccionable es
`iwr <url> -OutFile install-windows.ps1; .\install-windows.ps1`.

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API →
seleccionar el asset `ai-voice-interconnector-*-x86_64-windows.zip` (solo hay build
x86_64 para Windows: sin selección de arquitectura) → descargar el `.zip` y
`SHA256SUMS.txt` con `Invoke-WebRequest` (sin MOTW: no dispara SmartScreen) →
verificar el checksum (`Get-FileHash`; aborta si no coincide) → extraer con
`Expand-Archive` en `%LOCALAPPDATA%\Programs\ai-voice-interconnector` (reemplazando la
versión anterior) → registrar ese directorio en el PATH de usuario
(`HKCU\Environment`) de forma idempotente, sin `-Verb RunAs` (per-user, sin UAC) →
recomponer el PATH de la sesión desde el registro (el `HKCU\Environment` nuevo no
llega solo a la sesión en curso) → ejecutar `ai-voice-interconnector setup` (`-NoSetup`
lo desactiva).

La **extracción y el registro de PATH los hace el propio script**: no hay instalador
Inno ni post-install del binario (el `setup` de Rust solo provisiona el modelo).
Nota de migración: quien tenga una versión Inno per-machine antigua debe
desinstalarla primero (Panel de control, con admin); el script avisa si detecta esa
entrada de PATH legada per-machine, para evitar un PATH duplicado.

**Limitaciones conocidas**: Defender Antivirus puede marcar el binario sin firma
(independiente del MOTW; runbook WDSI arriba); el `.zip` descargado por navegador sí
lleva MOTW y dispara SmartScreen (lo resuelve la firma de código, no este script).

**Modo `-Check`**: ejecutar `install-windows.ps1 -Check` compara la
versión instalada (`ai-voice-interconnector --version`) con la última
publicada en GitHub y reporta la transición (`1.0.0 -> 2.0.0`), `Ya estás
en la versión`, o `no instalado -> 2.0.0` sin descargar ni extraer nada.

**Wrapper de auto-actualización**: `upgrade-ai-voice-interconnector.ps1`
detecta la versión corriente, la compara con la última, reporta la
transición, y, salvo que se pase `-Check`, re-ejecuta `install-windows.ps1`.
Es el punto de entrada unificado para la auto-actualización desde PowerShell.

## Cask de macOS

- `Casks/ai-voice-interconnector.rb` en el tap `homebrew-ai-voice-interconnector`, con las stanzas:
  `version`, `sha256`, `url` (al `tar.gz` de arm64 del release),
  `binary "ai-voice-interconnector"` (el binario en la raíz del archivo extraído, sin
  `.app`), `livecheck` (`strategy :github_latest`), `zap trash:` (caché del modelo y
  datos de usuario) y `caveats` que informa la licencia GPL-3.0-or-later (ubicación de
  `SOURCE-OFFER.md`/`THIRD-PARTY-LICENSES.md` dentro del `staged_path`) y sugiere
  `ai-voice-interconnector setup` para provisionar el modelo.
- El job `publish-metadata` en `.circleci/config.yml`, con
  `requires: [publish-release]` y filtro de tag `only: /^v.*/`. Tras el Release
  público, lee la versión de `CIRCLE_TAG` y el `sha256` del `tar.gz` de macOS desde
  `SHA256SUMS.txt` (recuperado con `gh release download "$CIRCLE_TAG" -p
  SHA256SUMS.txt`, de forma durable e idempotente, sin depender del workspace),
  reescribe el Cask con `cargo xtask cask` y hace push al tap con el context
  `homebrew-tap`. Regenerar y re-empujar produce el mismo resultado, así que el
  reintento es seguro en cualquier momento.

**Experiencia de usuario**:
`brew tap <owner>/ai-voice-interconnector && brew install --cask ai-voice-interconnector`. Homebrew
autoextrae el `tar.gz`, enlaza el binario en el prefix (`/opt/homebrew/bin`, ya en el
PATH) sin sudo, y elimina el atributo de cuarentena, con lo que mitiga Gatekeeper.
Toda la integración de PATH, la desinstalación (`--zap`) y la limpieza de cuarentena
las resuelve Homebrew; solo el modelo queda pendiente (el Cask no puede correr
post-install: lo remite a `setup` en las caveats).

**Bootstrap**: el **primer** push del Cask al tap es manual (un paso de arranque
único, porque `publish-metadata` actualiza un Cask que ya debe existir); a partir de
ahí el job lo mantiene.

## Instalador macOS (`curl | sh`)

`install-macos.sh` vive en la raíz del repo, servido desde
`raw.githubusercontent.com/<owner>/AI-Voice-InterConnector/main/install-macos.sh`. Uso:
`curl -fsSL <url> | sh`. Es la vía de una línea de macOS sin prerequisitos: ni
Homebrew (a diferencia del Cask) ni `sudo`.

**Herramientas del host**: solo binarios del sistema base de macOS. No existe
`sha256sum` (se usa `shasum -a 256`) ni `jq` (parseo con `grep`/`sed`, como
`install-linux.sh`); extracción con `tar`, limpieza de cuarentena con `xattr`. Ya no
intervienen `hdiutil` ni `ditto` (no hay `.dmg` ni `.app`).

**Flujo del script**: resolver `releases/latest` de la GitHub Releases API →
**guard de arquitectura** `uname -m` = `arm64` (Mac Intel no soportado; mensaje
claro que remite a compilar desde fuente, `docs/BUILD.md`) → seleccionar el asset
`ai-voice-interconnector-*-arm64-macos.tar.gz` → descargar el `tar.gz` y
`SHA256SUMS.txt` → verificar el checksum con `shasum` (aborta si no coincide) →
extraer en `~/.local/opt/ai-voice-interconnector/` (reemplazando la versión anterior)
→ `xattr -dr com.apple.quarantine` sobre el binario extraído (legítimo: el usuario ya
expresó intención ejecutando el script) → crear el symlink de PATH
`~/.local/bin/ai-voice-interconnector` → invocar `ai-voice-interconnector setup`.

**Integración de PATH per-user**: `~/.local/bin` **no** está en el PATH por
defecto de zsh en macOS; el script detecta esa ausencia y emite el aviso con la
línea exacta para `~/.zshrc`, sin mutar dotfiles. La integración vive en el propio
script (el `setup` de Rust solo provisiona el modelo).

**Limitaciones conocidas**: solo Apple Silicon (el guard aborta en Intel); la firma
de código/notarización sigue diferida (goal a largo plazo), pero la limpieza de
cuarentena elimina la fricción de Gatekeeper para quien use el one-liner.

## Desinstalación

`ai-voice-interconnector uninstall` (y `ai-voice-interconnector cleanup --all` como alias) es el **desinstalador en un comando** multiplataforma que cierra la paridad con `setup --uninstall` de Python. El flujo interno es parada de daemon primero (`POST /shutdown` graceful con `timeout 5s`, fallback `taskkill`/`kill`), datos después (`data_dir()` con `daemon.pid`), `hub`+`xet` (`~/.cache/huggingface/hub` y `~/.cache/huggingface/xet` con `shard-cache`/`logs` y `.locks`) y `temp` (`avi_*`, `ai-voice-interconnector-install-*`), integración de PATH después y binario al final, con confirmación interactiva (`--force`/`--yes` la omite) e idempotencia:

- **Linux / macOS (one-liner)**: `ai-voice-interconnector uninstall --force` para el daemon, borra el symlink `~/.local/bin/ai-voice-interconnector`, el directorio `~/.local/opt/ai-voice-interconnector/`, los datos (`cleanup`) y `hub`+`xet`+`logs`.
- **Windows**: `ai-voice-interconnector uninstall --force` para el daemon (`qwen_tts.exe` incluido), borra `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, quita esa entrada del PATH de usuario (`HKCU\Environment` + `WM_SETTINGCHANGE`), borra los datos y `hub`+`xet`+`logs`+`temp`. Si el binario está en uso, avisa y deja el borrado final para después de cerrar la terminal.
- **macOS (Cask)**: `brew uninstall --cask --zap ai-voice-interconnector` sigue siendo la vía idiomática (binario, PATH y, con `--zap`, `data_dir` + `hub` + `xet`).

`cleanup` (sin `--all`) hace lo mismo sin borrar binario/PATH: para el daemon, borra `models/speech/voices` + `daemon.pid`, purga `hub`+`xet` y `temp`, dejando el binario reintentable (`setup` descarga limpio). El contrato de instalación es `binario siempre + aviso`: `install-*.sh/ps1` instala el binario aunque `setup` falle por red, deja `doctor --json` en `failed` y es reintentable con `ai-voice-interconnector setup` + verificación `doctor --json` (no aborta instalación).

El procedimiento manual por SO (borrado/registry) queda como fallback y para auditoría. El estado de paridad (brecha cerrada en v0.10.8) vive en [docs/PARITY.md](PARITY.md#fase-5--desinstalación).

## Comportamiento frente a antivirus

Los instaladores auto-hospedados no eliminan las alertas de antivirus por sí mismos,
salvo el Cask en macOS (que limpia la cuarentena). El porqué de que
`install-windows.ps1` no dispare SmartScreen (a diferencia de la descarga por
navegador) está explicado en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar); Microsoft Defender
**Antivirus** es independiente del MOTW y puede marcar el binario sin firma venga de
donde venga (mitigado por el runbook WDSI de arriba).

El estado de esta brecha por SO (mitigada, diferida a firma de código) vive en
[docs/PARITY.md](PARITY.md#fase-2--primer-arranque-reputación-del-binario-sin-firmar).
