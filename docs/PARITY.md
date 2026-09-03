# Paridad de experiencia entre sistemas operativos

Este documento registra el **estado de equivalencia funcional y de experiencia de usuario** del canal nativo entre Windows, Linux y macOS, y enumera **qué falta para cerrar la paridad completa**. El criterio no es la paridad tecnológica (cada SO usa sus mecanismos idiomáticos: `.zip`/`tar.gz`, symlink en `~/.local/bin`, PATH en HKCU, Cask de Homebrew — eso es aceptable por diseño), sino que el **usuario final recorra un ciclo de vida equivalente**: instalar, usar, actualizar y desinstalar con la misma cantidad de fricción, privilegios y residuo.

Fecha de corte: **Fase 7** (canal Rust por archivos comprimidos). Cada brecha se identifica por un **nombre descriptivo** (no por número: la numeración secuencial se vuelve inconsistente a medida que se cierran brechas). Al cerrar una brecha, actualizar la tabla y la sección correspondiente.

## Tabla de contenidos

- [Resumen ejecutivo](#resumen-ejecutivo)
- [Fase 1 — Instalación](#fase-1--instalación)
- [Fase 2 — Primer arranque (reputación del binario sin firmar)](#fase-2--primer-arranque-reputación-del-binario-sin-firmar)
- [Fase 3 — Uso](#fase-3--uso)
- [Fase 4 — Actualización](#fase-4--actualización)
- [Fase 5 — Desinstalación](#fase-5--desinstalación)
- [Registro de brechas](#registro-de-brechas)

## Resumen ejecutivo

| Fase | Windows | Linux | macOS | ¿Paridad? |
|---|---|---|---|---|
| Instalación de una línea sin prerequisitos | ✅ `irm \| iex` | ✅ `curl \| sh` | ✅ `curl \| sh` (`install-macos.sh`) | **Sí** |
| Instalación sin privilegios de admin | ✅ per-user, sin UAC | ✅ `~/.local` | ✅ `~/.local` (one-liner sin `sudo`) | **Sí** |
| Modelo provisionado al terminar de instalar | ✅ encadena `setup` | ✅ encadena `setup` | ✅ one-liner encadena `setup` (Cask: *caveat*) | **Sí** |
| Verificación de checksum automática | ✅ | ✅ | ✅ (one-liner con `shasum`; Cask sí) | **Sí** |
| Primer arranque sin advertencia de reputación | ⚠️ one-liner esquiva MOTW; `.zip` de navegador dispara SmartScreen | ✅ (no aplica) | ⚠️ one-liner/Cask limpian cuarentena; archivo de navegador dispara Gatekeeper | Parcial (brecha de *firma de código*, cross-SO) |
| Uso (CLI, daemon, voces, contratos `--json`) | ✅ | ✅ | ✅ | **Sí** |
| Actualización sin residuo ni trampa | ✅ re-ejecutar one-liner reemplaza en sitio | ✅ re-ejecutar one-liner limpia la versión anterior | ✅ `brew upgrade --cask` / re-ejecutar one-liner | **Sí** |
| Desinstalación integrada y con residuo cero | ✅ `ai-voice-interconnector uninstall` (HKCU + dir + `cleanup`) | ✅ `ai-voice-interconnector uninstall` (symlink + dir + `cleanup`) | ✅ `ai-voice-interconnector uninstall` o Homebrew `brew uninstall --cask --zap` | **Sí** |
| Cobertura de arquitecturas | x86_64 | x86_64 + arm64 | arm64 | Limitación de toolchain (aceptada; ver matriz en BUILD.md §2) |

**Conclusión**: la paridad es **completa** en instalación, uso, actualización y desinstalación en los tres SO: los tres one-liners descargan un archivo comprimido, verifican el checksum, extraen el binario, integran el PATH por sí mismos y encadenan `setup` (provisión del modelo). Queda **una brecha abierta**: la de **firma de código** (primer arranque en Windows y macOS, por binarios sin firmar), diferida por diseño al goal a largo plazo por depender de terceros (firma/notarización, [docs/GOAL.md](GOAL.md)) y mitigada en su síntoma por los one-liners (descarga por CLI, sin Mark-of-the-Web) y el Cask. La brecha de **desinstalación en un comando**, reabierta en la Fase 7, se **cerró** con `ai-voice-interconnector uninstall` (y `cleanup --all` como alias) multiplataforma (dispatch por SO: symlink/dir en Unix, HKCU PATH/dir en Windows, con confirmación y `--force`). El detalle por fase, a continuación.

## Fase 1 — Instalación

Mecánica interna de cada script (flujo paso a paso, checksum, dependencias del
host) en [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

### Estado

- **Windows**: `install-windows.ps1` (`irm | iex`) resuelve el release, verifica el checksum, extrae el `.zip` en `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, registra ese directorio en el PATH de usuario (HKCU, sin UAC) de forma idempotente y encadena `ai-voice-interconnector setup`. Cero prerequisitos: PowerShell viene con el SO.
- **Linux**: `install-linux.sh` (`curl | sh`) hace lo análogo: checksum, extrae el `tar.gz` en `~/.local/opt/ai-voice-interconnector/`, crea el symlink `~/.local/bin/ai-voice-interconnector` (con aviso de PATH) y encadena `setup`. Cero prerequisitos en la práctica (`curl` + coreutils).
- **macOS**: dos vías, ambas sin `sudo`:
  - **One-liner** `install-macos.sh` (`curl | sh`): descarga el `tar.gz` de arm64, verifica el checksum con `shasum`, lo extrae en `~/.local/opt/ai-voice-interconnector/`, limpia la cuarentena de Gatekeeper del binario, crea el symlink per-user en `~/.local/bin` (con aviso de PATH) y encadena `setup`. Sin prerequisitos (ni Homebrew ni `sudo`).
  - **Cask de Homebrew** (`brew tap … && brew install --cask ai-voice-interconnector`): automatiza checksum, PATH y cuarentena, pero **exige tener Homebrew instalado** — un prerequisito de terceros que la audiencia declarada del canal nativo ("usuario final sin toolchain", `docs/DISTRIBUTION.md`) no necesariamente tiene. Además **no provisiona el modelo**: Homebrew no permite post-install arbitrario, así que el Cask solo imprime un *caveat* remitiendo a `ai-voice-interconnector setup` (`scripts/render_cask.py`).

### Qué falta para la paridad

Nada pendiente en esta fase: las tres plataformas tienen one-liner sin prerequisitos, sin admin, con checksum y provisión encadenada del modelo.

## Fase 2 — Primer arranque (reputación del binario sin firmar)

### Estado

- **Windows**: el one-liner descarga por CLI (sin Mark-of-the-Web) y no dispara SmartScreen. La descarga por navegador sí, con salida de dos clics («Más información → Ejecutar de todas formas»).
- **Linux**: no existe un sistema de reputación equivalente. Sin fricción.
- **macOS**: el one-liner y el Cask limpian la cuarentena. Un archivo descargado por navegador dispara Gatekeeper, cuya salida (clic derecho → Abrir, o `xattr`) es menos descubrible que la de SmartScreen.

### Qué falta para la paridad

- **Brecha de *firma de código* [MITIGADA, diferida al goal a largo plazo, cross-SO]**: la solución de fondo es la **firma de código/notarización** (goal a largo plazo, `docs/GOAL.md`). El mecanismo por el que los one-liners y el Cask ya mitigan el síntoma —sin resolverlo— está explicado en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar): el binario de Windows y el de macOS **descargados por navegador** siguen disparando la advertencia del SO respectivo, porque ambos son sin firmar. No es una asimetría exclusiva de macOS: Windows tiene el mismo comportamiento con SmartScreen. Es cross-SO por naturaleza, y está diferida porque su fondo depende de terceros (SignPath OSS, Apple Developer).

## Fase 3 — Uso

### Estado

**Paridad completa.** Mismos comandos, mismo daemon (puerto 8765), mismos esquemas `--json` y exit codes, mismas voces de fábrica y de usuario (`data_root()` por SO), mismo fail-fast de `speech`/`daemon start` sin modelo. Las diferencias de backend de audio (por SO) son tecnologías equivalentes, no diferencias de experiencia.

Única salvedad, aceptada como limitación de toolchain y documentada en el README: la cobertura de arquitecturas no es simétrica (sin Windows ARM64, sin Mac Intel).

### Qué falta para la paridad

Nada pendiente en esta fase.

## Fase 4 — Actualización

### Estado

- **Windows**: repetir el one-liner instala la versión nueva en el mismo directorio (`Expand-Archive` reemplaza el contenido) y conserva la entrada de PATH. Limpio.
- **macOS (Cask)**: `brew upgrade --cask ai-voice-interconnector` con `livecheck` — la mejor experiencia de actualización de las tres plataformas.
- **Linux / macOS (one-liner)**: re-ejecutar el instalador con una versión nueva limpia el directorio de instalación anterior (`rm -rf` antes de extraer), extrae el binario nuevo y reapunta el symlink. Sin residuo de versiones previas.

### Qué falta para la paridad

Nada pendiente en esta fase: las tres vías reemplazan en sitio sin dejar residuo de la versión anterior.

## Fase 5 — Desinstalación

### Estado

Con la migración a Rust se reintrodujo la desinstalación en un comando: `ai-voice-interconnector uninstall` (y `ai-voice-interconnector cleanup --all` como alias) es multiplataforma y espeja `setup --uninstall` del canal Python. El flujo interno es datos primero (`cleanup` borra `data_dir()`), integración de PATH después y binario al final, con dispatch por SO y confirmación interactiva (`--force`/`--yes` la omite):

- **Windows**: `uninstall` borra el directorio `%LOCALAPPDATA%\Programs\ai-voice-interconnector` vía helper desacoplado determinista (`Wait-Process` + `Remove-Item -LiteralPath`, `crates/avi-daemon/src/spawn.rs:spawn_uninstall_helper` con `Stdio::null` + `CREATE_NO_HANDLE_INHERIT`), quita esa entrada del PATH de usuario con comparación canónica (`avi-store::canonical_path_key`, `HKCU` + `WM_SETTINGCHANGE`) y ejecuta `cleanup` para modelos/caché. **Un comando** (`ai-voice-interconnector uninstall --force`) sin aviso `Bórralo manualmente`.
- **Linux**: `uninstall` borra el symlink `~/.local/bin/ai-voice-interconnector`, el directorio `~/.local/opt/ai-voice-interconnector/` y los datos (`cleanup`). **Un comando.**
- **macOS**: en la vía one-liner, análogo a Linux (`uninstall` limpia symlink + `~/.local/opt` + `cleanup`). Con **Homebrew Cask**, `brew uninstall --cask --zap ai-voice-interconnector` sigue siendo la vía idiomática (cubre también `cleanup`).

### Qué falta para la paridad

- **Brecha de *desinstalación en un comando* [CERRADA en v0.10.8]**: se reimplementó `ai-voice-interconnector uninstall` (y `cleanup --all`) en Rust con dispatch por SO (Unix: symlink + `~/.local/opt`; Windows: HKCU PATH + `%LOCALAPPDATA%\Programs`). Cubre `data_dir()` + PATH + binario en un comando, con confirmación y `--force`, y es idempotente. El desinstalador manual por SO y `brew uninstall --cask --zap` se conservan como fallback/idiomático.

## Registro de brechas

| Brecha | Fase | SO | Estado | Nota |
|---|---|---|---|---|
| *one-liner de instalación en macOS* | Instalación | macOS | ✅ Cerrada | `install-macos.sh` (`curl \| sh`) sobre `tar.gz` |
| *Cask en el README* | Instalación | macOS | ✅ Cerrada | README con las tres plataformas + Cask |
| *instalación sin `sudo` en macOS* | Instalación | macOS | ✅ Cerrada | one-liner per-user en `~/.local/bin` |
| *acumulación de versiones anteriores* | Actualización | Linux/macOS | ✅ Cerrada | `rm -rf` del directorio de instalación antes de extraer |
| *desinstalación en un comando* | Desinstalación | Windows + Linux | ✅ Cerrada (v0.10.8) | Reimplementado `ai-voice-interconnector uninstall` (y `cleanup --all`) multiplataforma |
| *firma de código* | Primer arranque | Windows + macOS | ⚠️ Abierta (diferida, cross-SO) | Mitigada por los one-liners (CLI sin MOTW) y el Cask; fondo = firma/notarización (goal a largo plazo) |
| *publish-metadata E2E* | Publicación | macOS | ✅ Cerrada (v0.10.8) | `ruby -c` del Cask y `sha256` contra `SHA256SUMS.txt` verificados en pipeline `publish-metadata` |

Tras la Fase 8 (uninstall Rust) la instalación, el uso, la actualización y la desinstalación mantienen paridad completa en los tres SO sobre el canal nativo por archivos comprimidos. Queda **una brecha abierta**: la de **firma de código** (primer arranque, cross-SO, diferida al goal a largo plazo por depender de terceros; mitigada por los one-liners y el Cask). La validación E2E de instalación/desinstalación en Linux y macOS sigue dependiendo de feedback de usuarios reales (frontera externa al CI, ver [docs/GOAL.md](GOAL.md#validación-e2e)).
