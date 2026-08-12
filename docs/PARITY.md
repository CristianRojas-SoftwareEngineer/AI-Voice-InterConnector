# Paridad de experiencia entre sistemas operativos

Este documento registra el **estado de equivalencia funcional y de experiencia de usuario** del canal nativo entre Windows, Linux y macOS, y enumera **qué falta para cerrar la paridad completa**. El criterio no es la paridad tecnológica (cada SO usa sus mecanismos idiomáticos: Inno Setup, AppImage, `.dmg`/Cask — eso es aceptable por diseño), sino que el **usuario final recorra un ciclo de vida equivalente**: instalar, usar, actualizar y desinstalar con la misma cantidad de fricción, privilegios y residuo.

Fecha de corte: **v0.6.0**. Cada brecha se identifica por un **nombre descriptivo** (no por número: la numeración secuencial se vuelve inconsistente a medida que se cierran brechas). Al cerrar una brecha, actualizar la tabla y la sección correspondiente.

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
| Instalación sin privilegios de admin | ✅ per-user, sin UAC | ✅ `~/.local` | ✅ `~/.local` (one-liner y `.command` sin `sudo`) | **Sí** |
| Modelo provisionado al terminar de instalar | ✅ encadena `setup` | ✅ encadena `setup` | ✅ one-liner/`.command` encadenan `setup` (Cask: *caveat*) | **Sí** |
| Verificación de checksum automática | ✅ | ✅ | ✅ (one-liner con `shasum`; Cask sí) | **Sí** |
| Primer arranque sin advertencia de reputación | ⚠️ one-liner esquiva MOTW; `.exe` de navegador dispara SmartScreen | ✅ (no aplica) | ⚠️ one-liner/Cask limpian cuarentena; `.dmg` de navegador dispara Gatekeeper | Parcial (brecha de *firma de código*, cross-SO) |
| Uso (CLI, daemon, voces, contratos `--json`) | ✅ | ✅ | ✅ | **Sí** |
| Actualización sin residuo ni trampa | ✅ Inno reemplaza en sitio | ✅ re-ejecutar one-liner limpia AppImages viejos | ✅ `brew upgrade --cask` / re-ejecutar one-liner | **Sí** |
| Desinstalación integrada y con residuo cero | ✅ `setup --uninstall` (un comando; delega binario+PATH a Inno) | ✅ `setup --uninstall` (un comando) | ✅ `setup --uninstall` (un comando; con Homebrew difiere a `brew --zap`) | **Sí** (brecha de *desinstalación en un comando* cerrada en v0.6.0) |
| Cobertura de arquitecturas | x86_64 | x86_64 + arm64 | arm64 | Limitación de toolchain (aceptada; ver matriz en BUILD.md §2) |

**Conclusión**: con v0.6.0 la paridad es **completa** en instalación, uso, actualización y **desinstalación** en los tres SO: `setup --uninstall` es un comando único en Linux, macOS y Windows (dispatch por SO sobre un contrato compartido — datos → PATH → binario, con cancelación atómica). Queda **una sola brecha abierta**, la de **firma de código** (primer arranque en Windows y macOS, por binarios sin firmar), diferida por diseño al goal a largo plazo por depender de terceros (firma/notarización, [docs/GOAL.md](GOAL.md)); está mitigada en su síntoma por los one-liners (descarga por CLI, sin Mark-of-the-Web) y el Cask. La brecha de *desinstalación en un comando*, la última accionable del goal inmediato, se cerró a nivel de código/scripts/tests en v0.6.0. El detalle por fase, a continuación.

## Fase 1 — Instalación

Mecánica interna de cada script (flujo paso a paso, checksum, dependencias del
host) en [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

### Estado

- **Windows**: `install-windows.ps1` (`irm | iex`) resuelve el release, verifica el checksum, instala en silencio per-user (sin UAC, PATH en HKCU) y encadena `ai-voice-interconnector setup`. Cero prerequisitos: PowerShell viene con el SO.
- **Linux**: `install-linux.sh` (`curl | sh`) hace lo análogo: checksum, instala en `~/.local/opt/ai-voice-interconnector/`, exporta `APPIMAGE` y encadena `setup` (que crea el symlink de PATH en `~/.local/bin`). Cero prerequisitos en la práctica (`curl` + coreutils).
- **macOS**: **no existe one-liner equivalente.** Las dos vías actuales:
  - **Cask de Homebrew** (`brew tap … && brew install --cask ai-voice-interconnector`): automatiza checksum, PATH y cuarentena, pero **exige tener Homebrew instalado** — un prerequisito de terceros que la audiencia declarada del canal nativo ("usuario final sin Python", `docs/DISTRIBUTION.md`) no necesariamente tiene. Además **no provisiona el modelo**: Homebrew no permite post-install arbitrario, así que el Cask solo imprime un *caveat* remitiendo a `ai-voice-interconnector setup` (`scripts/render_cask.py`).
  - **`.dmg` manual**: montar, arrastrar el `.app`, ejecutar `Instalar (PATH + modelo).command`, **teclear la contraseña de administrador** (`sudo` para el symlink en `/usr/local/bin`, `scripts/build_macos.py::_path_install_script`) y responder el prompt de descarga del modelo. Es la única vía de instalación del proyecto que pide privilegios elevados, y el checksum queda a cargo del usuario.

Asimetría documental (cerrada en v0.5.0): el README ahora se titula «Instalación de una línea» y documenta los tres SO más el Cask como alternativa de macOS.

### Estado tras v0.5.0

Las tres brechas de esta fase quedaron **cerradas**:

- **Brecha de *one-liner de instalación en macOS* [CERRADA]**: existe `install-macos.sh` (`curl | sh`): descarga el `.dmg` del release, verifica el checksum con `shasum -a 256 -c`, monta con `hdiutil attach`, copia el `.app` a `~/Applications` (per-user, sin `sudo`), limpia la cuarentena con `xattr -dr com.apple.quarantine`, crea el symlink per-user en `~/.local/bin` (con aviso de PATH) y encadena `setup`. Cierra de una vez: one-liner, sin admin, checksum y provisión; mitiga Gatekeeper (ver brecha de *firma de código*). Smoke-test `bats` en el job CI `test-installer-macos`.
- **Brecha de *Cask en el README* [CERRADA]**: el README documenta la instalación de una línea para los tres SO y presenta el Cask como alternativa de macOS para usuarios de Homebrew.
- **Brecha de *instalación sin `sudo` en macOS* [CERRADA]**: los scripts `.command` del `.dmg` crean el symlink **per-user** en `~/.local/bin` (con aviso de PATH), sin `sudo`. Ninguna vía de instalación del proyecto pide ya privilegios de administrador.

## Fase 2 — Primer arranque (reputación del binario sin firmar)

### Estado

- **Windows**: el one-liner descarga por CLI (sin Mark-of-the-Web) y no dispara SmartScreen. La descarga por navegador sí, con salida de dos clics («Más información → Ejecutar de todas formas»).
- **Linux**: no existe un sistema de reputación equivalente. Sin fricción.
- **macOS**: el Cask limpia la cuarentena. El `.dmg` descargado por navegador dispara Gatekeeper, cuya salida (clic derecho → Abrir, o `xattr`) es menos descubrible que la de SmartScreen.

### Qué falta para la paridad

- **Brecha de *firma de código* [MITIGADA, diferida al goal a largo plazo, cross-SO]**: la solución de fondo es la **firma de código/notarización** (goal a largo plazo, `docs/GOAL.md`). El mecanismo por el que los one-liners y el Cask ya mitigan el síntoma —sin resolverlo— está explicado en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar): el `.exe` de Windows y el `.dmg` de macOS **descargados por navegador** siguen disparando la advertencia del SO respectivo, porque ambos binarios son sin firmar. No es una asimetría exclusiva de macOS: Windows tiene el mismo comportamiento con SmartScreen. Es cross-SO por naturaleza, y está diferida porque su fondo depende de terceros (SignPath OSS, Apple Developer).

## Fase 3 — Uso

### Estado

**Paridad completa.** Mismos comandos, mismo daemon (puerto 8765), mismos esquemas `--json` y exit codes, mismas voces de fábrica y de usuario (`data_root()` por SO), mismo fail-fast de `speech say`/`daemon start` sin modelo. Las diferencias de backend de audio (pycaw/sounddevice/afplay) son tecnologías equivalentes, no diferencias de experiencia.

Única salvedad, aceptada como limitación de toolchain y documentada en el README: la cobertura de arquitecturas no es simétrica (sin Windows ARM64, sin Mac Intel).

### Qué falta para la paridad

Nada pendiente en esta fase.

## Fase 4 — Actualización

### Estado

- **Windows**: repetir el one-liner (o el instalador nuevo); Inno reemplaza la instalación per-user en el mismo directorio y conserva el PATH. Limpio.
- **macOS (Cask)**: `brew upgrade --cask ai-voice-interconnector` con `livecheck` — la mejor experiencia de actualización de las tres plataformas.
- **Linux**: re-ejecutar `install-linux.sh` con una versión nueva instala el AppImage nuevo, reapunta el symlink y **elimina los AppImages anteriores** del directorio de instalación (cerrado en v0.5.0). En la vía manual, reemplazar el archivo sin re-correr `setup` sigue dejando el symlink de PATH colgante (trampa documentada en `USAGE.md`), pero la vía recomendada (re-ejecutar el one-liner) ya no la tiene.

### Qué falta para la paridad

- **Brecha de *acumulación de AppImages* [CERRADA]**: `install-linux.sh` elimina las versiones anteriores tras instalar y dar permisos al AppImage nuevo: un bucle POSIX borra los `ai-voice-interconnector-*.AppImage` previos de `~/.local/opt/ai-voice-interconnector/` (de su propiedad exclusiva), dejando exactamente un AppImage. Cubierto por un test `bats` de actualización.

## Fase 5 — Desinstalación

### Estado

El contrato (`USAGE.md` §"Desinstalación completa") es: datos primero (`cleanup --all`), integración de PATH después, binario al final. `setup --uninstall` es un **comando único en los tres SO** (v0.6.0), un dispatch por SO sobre ese contrato compartido con cancelación atómica (cancelar el cleanup aborta sin borrar nada, salida 0) y guard de canal nativo (`is_frozen`; desde fuente o pip/uv remite a `pip uninstall`). Lo único que cambia por SO es el paso del binario:

- **Windows**: `setup --uninstall` borra los datos en proceso y **delega** el binario y la reversión del PATH (HKCU) al desinstalador de Inno, lanzado desacoplado con el `QuietUninstallString` del registro (el SO mantiene el lock del `.exe`); el directorio de instalación se reporta en `delegated`, no en `removed`. **Un comando.** La vía idiomática (Configuración → Aplicaciones) sigue como alternativa.
- **Linux**: `setup --uninstall` encadena `cleanup --all`, quita el symlink de PATH y borra `~/.local/opt/ai-voice-interconnector/`. **Un comando.** (`setup --remove-path` se conserva como reversión fina del symlink.)
- **macOS**: `setup --uninstall` encadena `cleanup --all`, quita el symlink per-user y borra el `.app` (localizado desde `sys.executable`, cubre `~/Applications`, `/Applications` y el Cask). **Un comando.** Con **Homebrew Cask** lo detecta por la metadata del Caskroom y **difiere a `brew uninstall --cask --zap`** sin borrar nada (para no dejar el Caskroom inconsistente).

### Qué falta para la paridad

- **Brecha de *`zap` completo del Cask* [CERRADA]**: el `zap` del Cask incluye el repo base del modelo (`~/.cache/huggingface/hub/models--ResembleAI--chatterbox`) además del multilingüe — corrección en `_CASK_TEMPLATE` de `scripts/render_cask.py`, cubierta por test; se propaga al tap con el release v0.5.0 vía `publish-metadata`.
- **Brecha de *desinstalador de Linux* [CERRADA]**: `ai-voice-interconnector setup --uninstall` desinstala Linux en un paso: quita el symlink, borra `~/.local/opt/ai-voice-interconnector/` y encadena `cleanup --all` (con confirmación o `--yes`), con contrato `--json` y tests pytest.
- **Brecha de *desinstalación en un comando* [CERRADA a nivel de código en v0.6.0]**: `setup --uninstall` es ahora multiplataforma — un dispatch por SO (`_uninstall` en `cli.py`) sobre un contrato compartido: guard `is_frozen`, gate `--json`/`--yes`, orden unificado datos → PATH → binario y cancelación atómica. La rama Linux se reordenó a ese orden (el reorden habilita cancelar sin borrar nada); macOS resuelve el `.app` desde `sys.executable` con deferral a Homebrew por metadata del Caskroom; Windows valida el `QuietUninstallString` primero y delega el binario+PATH al desinstalador de Inno desacoplado, con el directorio de instalación en `delegated`. Cubierto por la suite `TestSetupUninstall` de los tres SO. La marca del criterio de aceptación 10 depende además de la validación por feedback de usuarios reales en Linux y macOS (frontera E2E externa al CI, ver [docs/GOAL.md](GOAL.md#validación-e2e)).

## Registro de brechas

| Brecha | Fase | SO | Estado | Cierre |
|---|---|---|---|---|
| *one-liner de instalación en macOS* | Instalación | macOS | ✅ Cerrada (v0.5.0) | `install-macos.sh` (`curl \| sh`) |
| *Cask en el README* | Instalación | macOS | ✅ Cerrada (v0.5.0) | README con las tres plataformas + Cask |
| *instalación sin `sudo` en macOS* | Instalación | macOS | ✅ Cerrada (v0.5.0) | `.command` per-user en `~/.local/bin` |
| *acumulación de AppImages* | Actualización | Linux | ✅ Cerrada (v0.5.0) | Limpieza de versiones en `install-linux.sh` |
| *`zap` completo del Cask* | Desinstalación | macOS | ✅ Cerrada (v0.5.0) | Repo base añadido a `_CASK_TEMPLATE` |
| *desinstalador de Linux* | Desinstalación | Linux | ✅ Cerrada (v0.5.0) | `setup --uninstall` |
| *desinstalación en un comando* | Desinstalación | macOS + Windows | ✅ Cerrada a nivel de código (v0.6.0) | `setup --uninstall` multiplataforma (dispatch por SO) |
| *firma de código* | Primer arranque | Windows + macOS | ⚠️ Abierta (diferida, cross-SO) | Mitigada por los one-liners (CLI sin MOTW) y el Cask; fondo = firma/notarización (goal a largo plazo) |

Con v0.6.0 quedan cerradas a nivel de código/scripts/tests las **siete** brechas accionables: las seis de v0.5.0 más la de *desinstalación en un comando* (`setup --uninstall` multiplataforma). Queda **una sola brecha abierta**, la de **firma de código** (primer arranque, cross-SO), **diferida al goal a largo plazo** porque su fondo (firma/notarización) depende de terceros; está mitigada en su síntoma por los one-liners (descarga por CLI sin Mark-of-the-Web) y el Cask, pero no resuelta en código. Ningún cierre rompió compatibilidad ni re-publicó artefactos existentes: los de v0.6.0 se materializan en ese release. La marca del criterio de aceptación 10 depende además de la validación por feedback de usuarios reales en Linux y macOS (frontera E2E externa al CI).
