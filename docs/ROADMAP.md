# Roadmap: estado actual y camino al Goal inmediato

Este documento es el **registro vivo** del estado de implementación de
`tts-sidecar` y del trabajo pendiente para alcanzar el **Goal inmediato**, cuya
especificación, criterios de aceptación y condición de finalización viven en
[docs/GOAL.md](GOAL.md). El **Goal a largo plazo** (firma de código /
notarización) también se define en
[docs/GOAL.md](GOAL.md#goal-a-largo-plazo).

Mantener el estado y el roadmap aquí (separados de la especificación ideal)
permite que [docs/GOAL.md](GOAL.md) siga siendo la fuente de verdad del *qué* y
el *cuándo está hecho*, mientras este archivo responde al *dónde estamos* y *qué
falta*.

## Tabla de contenidos

- [Estado actual](#estado-actual)
- [Trabajo pendiente (roadmap al Goal inmediato)](#trabajo-pendiente-roadmap-al-goal-inmediato)
- [Plan técnico: brecha de *desinstalación en un comando* — EJECUTADO (v0.6.0)](#plan-técnico-brecha-de-desinstalación-en-un-comando--ejecutado-v060)
- [Hacia el Goal inmediato](#hacia-el-goal-inmediato)

## Estado actual

**Implementado y verificable en el repo** (la validación end-to-end de los
instaladores por SO es externa al pipeline por diseño; ver la «Decisión de
validación E2E» en
[docs/GOAL.md](GOAL.md#validación-e2e)):

- Motor Chatterbox Multilingual V3 implementado (Python)
- Sistema de audio playback nativo por SO (pycaw/winsound/sounddevice/afplay)
- Daemon mode con IPC HTTP (FastAPI, puerto 8765)
- Optimizaciones de síntesis (n_cfm=4, max_new_tokens=500)
- Bypass del watermark PerthNet: el audio generado no lleva marca de agua (ver «Uso ético y responsable» en README/USAGE)
- Scripts de build PyInstaller por SO (Windows/Linux/macOS)
- **Canal PyPI** (`uv tool install tts-sidecar` / `pipx install tts-sidecar`), publicado automáticamente en cada tag `v*` junto al canal nativo (ver [docs/DISTRIBUTION.md](DISTRIBUTION.md))
- Descarga automática del modelo Chatterbox desde HuggingFace
- CLI completa con todos los comandos
- **Instalación auto-hospedada de una línea por SO** (Linux y Cask de macOS en v0.3.0; Windows en v0.4.0; one-liner macOS `install-macos.sh` en v0.5.0): `install-linux.sh` (`curl | sh`) en Linux, `install-macos.sh` (`curl | sh`, sin Homebrew ni `sudo`) y el Cask de Homebrew propio en macOS, e `install-windows.ps1` (`irm | iex`) en Windows (instalador Inno Setup per-user, sin UAC; entró en alcance al refutarse la premisa de SmartScreen — la descarga por CLI no aplica el Mark-of-the-Web). Todos los canales publican de forma autónoma, sin aprobación ni pull request a terceros. Ver [docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md)
- **Paridad de experiencia entre los 3 SO** (v0.6.0): cerradas a nivel de código/scripts/tests **las siete** brechas accionables de [docs/PARITY.md](PARITY.md) — las seis de v0.5.0 (one-liner macOS, `.command` sin `sudo`, limpieza de AppImages, `setup --uninstall` en Linux, `zap` del Cask completo, README con las tres plataformas) más la de *desinstalación en un comando* (`setup --uninstall` multiplataforma en macOS/Windows, cerrada en v0.6.0). Queda **una sola brecha abierta**, la de *firma de código* (SmartScreen/Gatekeeper, binarios sin firmar, cross-SO), diferida al goal a largo plazo
- Tests pytest (572 tests: timing, protocolo, daemon, CLI, voces, rutas, caché de modelo, audio, Cask y utilidades de build), más los smoke-tests de instaladores (bats Linux/macOS y Pester Windows) en CI
- Documentación sincronizada

## Trabajo pendiente (roadmap al Goal inmediato)

**Todas** las brechas accionables de [docs/PARITY.md](PARITY.md) están cerradas
a nivel de código/scripts/tests: las seis de v0.5.0 más la de *desinstalación en
un comando* (`setup --uninstall` multiplataforma en macOS/Windows), cerrada en
v0.6.0 (plan técnico ejecutado, ver la sección siguiente). Solo la brecha de
*firma de código* (cross-SO) sigue diferida al goal a largo plazo. **No queda
código pendiente del goal inmediato**: la **marca de los criterios de aceptación
10, 1-3 y 9** depende ahora solo de la validación por feedback de usuarios reales
en Linux y macOS (la validación E2E automatizable ya corre en CI; ver
«Validación E2E» en [docs/GOAL.md](GOAL.md#validación-e2e)).

## Plan técnico: brecha de *desinstalación en un comando* — EJECUTADO (v0.6.0)

> **Estado: ejecutado en v0.6.0.** `setup --uninstall` es ya multiplataforma:
> el despachador `_uninstall` y las ramas `_uninstall_macos`/`_uninstall_windows`
> viven en `src/tts_sidecar/cli.py`, la rama Linux se reordenó al contrato
> compartido, y la suite `TestSetupUninstall` cubre los tres SO. Esta sección se
> conserva como registro del diseño implementado (la «Definición de hecho» a pie
> de sección quedó satisfecha a nivel de código; la marca del criterio 10 sigue
> pendiente de feedback E2E).

`tts-sidecar setup --uninstall` debe pasar de ser un comando **solo-Linux** a
uno **multiplataforma** que espeje la instalación one-line de cada SO:
desinstalación equivalente en esfuerzo a la instalación, con residuo cero, en un
único comando. El objetivo, redactado como especificación ideal, vive en
[docs/GOAL.md §Desinstalación en un comando](GOAL.md#desinstalación-en-un-comando);
esta sección es el plan de implementación prescriptivo para alcanzarlo desde el
estado actual.

### Estado del código

Linux es la **plantilla de referencia, ya implementada**. `_uninstall_linux`
(`src/tts_sidecar/cli.py`) ejecuta el contrato completo: guard de SO → gate
`--json` (exige `--yes`) → tres pasos → payload JSON. El parser de `setup` (grupo
mutuamente excluyente `--uninstall` / `--remove-path` / `--force-update`, más
`--yes` y `--json`) ya sirve a las tres plataformas y **no cambia
estructuralmente**; sí cambian sus **textos de superficie**, hoy anclados a
Linux: los `help` de `--uninstall` y `--yes` (describen la desinstalación como
solo-Linux y citan `~/.local/opt/tts-sidecar/`) y el docstring de `cmd_setup`
(«--uninstall (solo Linux)») deben reescribirse en clave multiplataforma. Lo que
falta es reemplazar el guard `if sys.platform != "linux"` por un **dispatch por
SO** y añadir las dos ramas que hoy no existen (`darwin`, `win32`).

**Reordenamiento de la rama Linux**: hoy `_uninstall_linux` borra en el orden
symlink → directorio de instalación → `cleanup --all` (binario **antes** que
datos). El contrato compartido (abajo) impone el orden inverso —datos
independientes primero, componente ancla al final—, así que `_uninstall_linux`
debe reordenarse a `cleanup --all` → symlink → directorio de instalación. El
reordenamiento trae un cambio de comportamiento deliberado: con los datos como
primer paso, cancelar el cleanup aborta la desinstalación completa sin borrar
nada (ver el punto 2 del contrato compartido), cosa imposible con el orden
antiguo (el binario ya había caído cuando llegaba la pregunta). Hay que revisar
que los tests de `TestSetupUninstall` no afirmen un orden estricto de mensajes.

### Arquitectura del cierre

El cierre es un **dispatch por SO sobre un contrato compartido**, no tres
implementaciones independientes. `cmd_setup` sigue delegando en una función de
desinstalación; esa función pasa de ser `_uninstall_linux` a un despachador:

```
_uninstall(args)
├── guard de canal nativo (is_frozen)      # común a los 3 SO
├── gate --json requiere --yes             # común (heredado de Linux)
├── sys.platform == "linux"   → _uninstall_linux(args)     # ya existe
├── sys.platform == "darwin"  → _uninstall_macos(args)     # NUEVO
├── sys.platform == "win32"   → _uninstall_windows(args)   # NUEVO
└── otro                      → EXIT_INVALID_INPUT
```

El gate `--json`/`--yes` y el guard `is_frozen` viven **solo en el
despachador**: al extraerlos, el gate inline actual de `_uninstall_linux`
(`cli.py:696-702`) se elimina de la rama (no se duplica).

#### Contrato compartido (invariante en los 3 SO)

Toda rama respeta las mismas reglas:

1. **Orden unificado: independientes primero, dependiente al final.** Las tres
   ramas borran en el mismo orden lógico:
   1. **Datos independientes** — `cleanup --all` (modelo + voces): componentes de
      filesystem que no dependen del binario.
   2. **Integración de PATH** — symlink (`~/.local/bin/tts-sidecar` en Unix) o su
      equivalente (PATH de HKCU en Windows).
   3. **Componente ancla (binario)** — el directorio de instalación / `.app` /
      `%LOCALAPPDATA%\Programs\tts-sidecar`, borrado al final.

   El motivo no es solo simetría, sino **atomicidad ante fallos**: borrar primero
   el residuo pesado e independiente (el modelo, ~4 GB) hace que una interrupción
   a media desinstalación deje atrás solo el binario —barato y fácil de borrar a
   mano—, nunca gigabytes huérfanos. Windows **obliga** a este orden (no puede
   borrar su `.exe` en ejecución, ver su rama); Unix lo adopta por consistencia y
   por el mejor modo de fallo, aunque técnicamente podría autoborrar el binario en
   vivo. La única asimetría que sobrevive es irreducible: *cómo* se borra el
   binario (en proceso en Unix; delegado a Inno en Windows), no *cuándo*.
2. **Encadenar `cleanup --all` con resultado observable y cancelación atómica**:
   construir el `argparse.Namespace` (`model=False, voices=False, all=True,
   dry_run=False, yes=<args.yes>, json=False,
   cleanup_parser=<args.cleanup_parser>`) e invocar `cmd_cleanup`. Borra solo
   las dos carpetas `models--ResembleAI--*` (`model_cache_dirs`) y las voces de
   usuario — es idéntico en las tres plataformas y es siempre el **primer** paso
   del orden unificado. Exige un cambio de contrato interno: hoy `cmd_cleanup`
   retorna `None` en todos sus caminos, indistinguible entre éxito y cancelación
   y sin exponer las rutas borradas; debe pasar a devolver el resultado (rutas
   eliminadas + bandera de cancelación). Con ese resultado, una cancelación del
   cleanup (respuesta negativa o stdin cerrado, misma política que
   `cmd_cleanup`) **aborta la desinstalación completa sin borrar nada** — ni
   PATH ni binario: es la atomicidad de cara al usuario que el orden unificado
   habilita. La cancelación termina con **salida 0** y el mensaje de cancelación
   —misma política que `cmd_cleanup` standalone; no es un código de error porque
   bajo automatización la cancelación es inalcanzable (`--json` exige `--yes`, y
   `--yes` omite la pregunta)—. El camino «no hay nada que limpiar» (ninguna
   ruta objetivo existe, p. ej. tras un `cleanup --all` previo) **no es una
   cancelación**: retorna rutas vacías con la bandera de cancelación apagada y
   la desinstalación continúa con el PATH y el binario. Tras un cleanup exitoso
   (no cancelado), el uninstall elimina además el **directorio raíz de datos**
   (`data_root()`) si quedó vacío, atestiguándolo en `removed`: `cleanup` solo
   borra `voices/` en su interior —y `voices_root()` lo re-crea al resolverse
   (`paths.py`, `mkdir` incondicional)—, así que sin este paso quedaría un
   directorio vacío en los tres SO, contra el residuo cero de la spec y contra
   el alcance del `zap` del Cask, que borra ese directorio entero. Este borrado
   vive en el uninstall, no en `cmd_cleanup` (que se usa standalone).
3. **Gate `--json`/`--yes`**: `--json` exige `--yes` (la confirmación
   interactiva del cleanup contaminaría stdout); bajo `--json` el stdout del
   cleanup encadenado se redirige a stderr con `contextlib.redirect_stdout`.
4. **Payload JSON**: `{schema_version, uninstall: true, removed: [rutas]}`. Como
   los datos se borran siempre en proceso, sus rutas se atestiguan en las tres
   plataformas — vía el valor de retorno de `cmd_cleanup` del punto 2 (la
   implementación Linux actual **no** las incluye en `removed`; hay que
   corregirlo al reordenar la rama). Lo que varía es el binario: en Unix su ruta
   se atestigua en `removed` (borrado en proceso); en Windows **no entra en
   `removed`** — su borrado queda a cargo del desinstalador de Inno y la ruta se
   reporta en un campo adicional `delegated` (aditivo al esquema, sin bump de
   `schema_version`; ver su rama).
5. **Guard de canal nativo**: `setup --uninstall` solo aplica al **canal nativo**
   (AppImage / `.app` / Inno). Antes del dispatch, exigir `paths.is_frozen()`;
   si el proceso corre desde fuente o desde una instalación pip/uv, abortar con
   `EXIT_INVALID_INPUT` remitiendo a `pip uninstall tts-sidecar`. (La rama Linux
   actual se apoya de forma implícita en que `~/.local/opt` exista; el guard
   `is_frozen` lo hace explícito y protege a macOS/Windows.)
6. **Pre-imports antes de borrar el componente ancla**: todos los imports
   perezosos que la rama necesite después del borrado (`json`, `shutil`,
   `contextlib`) deben resolverse **antes** de eliminar el binario. En Linux el
   patrón actual (importar `json` al final) funciona por accidente: el AppImage
   corre desde un squashfs montado que sobrevive al unlink del archivo. En el
   `.app` onedir de macOS no hay esa indirección: tras el `rmtree` del bundle,
   un import perezoso (p. ej. el módulo C de `json`) se resuelve por ruta contra
   archivos ya inexistentes y puede fallar en pleno happy path de `--json`.

#### Rama macOS — `_uninstall_macos` (nueva)

Espeja `install-macos.sh` y los `.command` del `.dmg`. Los pasos siguen el orden
unificado (datos → PATH → binario):

1. **Localizar el `.app` desde `sys.executable`**, sin adivinar la carpeta: en
   modo congelado el binario corre en `<app>/Contents/MacOS/tts-sidecar`, así que
   la raíz del bundle es `Path(sys.executable).resolve().parents[2]`. El
   `resolve()` es obligatorio: si el proceso se invocó vía el symlink
   `~/.local/bin/tts-sidecar`, `_NSGetExecutablePath` puede devolver la ruta del
   symlink y `parents[2]` apuntaría a `$HOME`. Guard estructural adicional: si
   la ruta resultante no termina en `.app`, abortar con `EXIT_INVALID_INPUT` —
   es el análogo del guard de ruta exacta de Linux, que aquí no puede ser exacto
   porque hay tres ubicaciones válidas. Esto cubre las tres vías de instalación
   (`~/Applications` del one-liner, `/Applications` del `.dmg` y del Cask) con
   una sola expresión. (Se resuelve al inicio, pero se borra al final, según el
   orden unificado.)
2. **Detección de Homebrew Cask por metadata, no por ruta**: el artefacto `app`
   del Cask (`scripts/render_cask.py`) **mueve** el bundle a
   `/Applications/tts-sidecar-arm64.app` — la misma ruta que deja la vía `.dmg`
   manual, así que la ubicación del `.app` no distingue las dos vías. La
   detección fiable es la existencia del directorio de metadata del Caskroom:
   `$HOMEBREW_PREFIX/Caskroom/tts-sidecar` (con `/opt/homebrew` como default
   arm64 si la variable no está definida), sin subprocesos. Si existe, **abortar
   la desinstalación completa** (sin cleanup, sin symlink, sin `.app`) con
   `EXIT_INVALID_INPUT` —el comando no aplica a esa instalación, igual que el
   guard de canal nativo— y remitir
   a `brew uninstall --cask --zap tts-sidecar`, cuyo `zap` ya cubre los datos:
   borrar el `.app` a mano dejaría la metadata del Caskroom inconsistente, y una
   desinstalación parcial dejaría un estado híbrido difícil de razonar. En las
   otras dos vías, continuar con el borrado.
3. **`cleanup --all`** (datos independientes) — primer borrado, según el contrato
   compartido.
4. **Eliminar el symlink `~/.local/bin/tts-sidecar`** (misma ruta que Linux:
   reutilizar/renombrar `_linux_path_symlink` a un helper neutro de SO).
5. **Eliminar el `.app`** (componente ancla, al final) con `shutil.rmtree`,
   re-asertando inmediatamente antes el guard estructural del paso 1 (la ruta
   termina en `.app`) como defensa en profundidad — el guard de ruta exacta de
   Linux no es aplicable aquí porque hay tres ubicaciones de instalación
   válidas. Borrar el bundle en ejecución es seguro en macOS (el inode mapeado
   en memoria sobrevive hasta que el proceso termina, igual que el unlink de
   Linux).
6. **Emitir el payload** según el contrato compartido.

#### Rama Windows — `_uninstall_windows` (nueva)

Windows es la plataforma que **origina** el orden unificado: el SO mantiene un
lock sobre el `tts-sidecar.exe` en ejecución, así que el propio proceso no puede
borrar su binario ni esperar a un desinstalador que necesita borrarlo. De ahí que
el componente ancla se borre al final y de forma delegada:

1. **Leer y validar `QuietUninstallString` primero, sin efectos**: leer la clave
   `HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{E8A1B2C3-D4F5-6789-ABCD-EF0123456789}_is1`
   con `winreg` (stdlib, solo Windows → import perezoso para no romper el import
   del módulo en Linux/macOS ni en los tests). El `AppId` está fijado en
   `scripts/create_installer_windows.py`. Si la clave o el valor faltan
   (instalación no hecha por el instalador nativo), abortar con
   `EXIT_INVALID_INPUT` **antes de haber borrado nada** — es el análogo Windows
   de resolver el `.app` al inicio en macOS: el ancla se valida primero y se
   borra al final, para que un aborto nunca sea destructivo (atomicidad del
   contrato compartido).
2. **`cleanup --all` en proceso** (datos independientes: modelo + voces),
   mientras el proceso vive y el contrato `--json`/`--yes` se cumple.
3. **Lanzar el desinstalador desacoplado** con `subprocess.Popen` (sin
   `-Wait`/`.wait()`), pasando el `QuietUninstallString` ya validado tal cual
   (ya incluye las comillas y el flag `/SILENT`): Inno se autocopia a `%TEMP%` y
   se relanza para poder borrar `{app}`. El desinstalador ya revierte el PATH de
   HKCU (`CurUninstallStepChanged`) y borra `%LOCALAPPDATA%\Programs\tts-sidecar`
   — en ese orden: `usUninstall` dispara **antes** del borrado de archivos, así
   que el tramo delegado también respeta el PATH → binario del orden unificado;
   no hay que replicar nada de eso en Python.
4. **Emitir el payload y retornar de inmediato** (ver el punto 5 sobre la
   asimetría del `removed`) para liberar el lock del `.exe` y dejar que Inno lo
   borre tras la salida del proceso. La carrera resultante es benigna en la
   práctica —el CLI muere en milisegundos y el arranque de Inno (autocopia a
   `%TEMP%` + relanzamiento) tarda más—, pero debe quedar documentada en el
   docstring.
5. **Payload**: como el borrado del binario ocurre *después* de la muerte del
   proceso, `removed` atestigua solo las rutas de datos (borradas en proceso); el
   directorio de instalación **no entra en `removed`** —afirmarlo sería falso:
   aún existe cuando se emite el payload— sino en el campo adicional `delegated`
   del punto 4 del contrato compartido. Documentar esta asimetría en el
   docstring.

### Plan de tests (`tests/test_cli.py::TestSetupUninstall`)

La clase ya cubre Linux (`monkeypatch` de `Path.home`, `sys.platform`, caché HF
sintética y voces falsas) y el gate `--json`/`--yes` y la exclusión mutua del
parser. Dos partes de la suite existente **no** se heredan sin cambio y hay que
adaptarlas:

- Los tests Linux actuales no simulan modo congelado; con el guard `is_frozen`
  fallarían. Añadirles `monkeypatch.setattr(sys, "frozen", True,
  raising=False)` (p. ej. dentro de `_fake_home_linux`).
- `test_uninstall_no_linux_falla` usa `darwin` como plataforma inválida; con el
  dispatch, `darwin` pasa a ser rama válida. Reescribirlo contra una plataforma
  realmente fuera del dispatch (p. ej. `"freebsd"`).

Añadir:

- **Guard de canal nativo**: con `paths.is_frozen()` falso, `--uninstall` aborta
  con `EXIT_INVALID_INPUT` remitiendo a `pip uninstall`.
- **Cancelación atómica** (rama Linux como representante del contrato): cleanup
  respondido con `n` (o stdin cerrado) → nada borrado: datos, symlink y
  directorio de instalación intactos.
- **Camino «no hay nada que limpiar»** (rama Linux como representante del
  contrato): sin caché ni voces preexistentes, la desinstalación **continúa** —
  symlink y directorio de instalación borrados (rutas vacías con la bandera de
  cancelación apagada no son una cancelación; una regresión que las confundiera
  abortaría la desinstalación dejando PATH y binario atrás).
- **Payload con rutas de datos**: el `removed` del `--json` incluye las rutas
  borradas por el cleanup encadenado (vía el nuevo valor de retorno de
  `cmd_cleanup`).
- **Directorio raíz de datos**: tras el cleanup encadenado, `data_root()` vacío
  queda eliminado y atestiguado en `removed` (cubre el `mkdir` incondicional de
  `data_root()` que lo re-crea al resolverse `voices_root()`).
- **macOS** (`sys.platform="darwin"`, `sys.frozen=True`, `sys.executable`
  apuntando a un `.app` falso bajo `tmp_path`): borrado del bundle, del symlink y
  encadenado de cleanup; `sys.executable` apuntando vía symlink → el `resolve()`
  localiza el `.app` real; ejecutable fuera de un `.app` → `EXIT_INVALID_INPUT`;
  caso Homebrew Cask (directorio de Caskroom falso presente) → aborta **sin
  borrar nada** con `EXIT_INVALID_INPUT` y remite a `brew`.
- **Windows** (`sys.platform="win32"`, mock de `winreg` devolviendo un
  `QuietUninstallString` falso, mock de `subprocess.Popen`): la lectura del
  registro se valida primero (paso 1); luego `cleanup` corre en proceso y el
  desinstalador se lanza **desacoplado** (sin espera); clave de registro ausente
  → `EXIT_INVALID_INPUT` **con los datos intactos** (la validación precede al
  cleanup, nada se borra); el payload `--json` atestigua las rutas de datos en
  `removed` y el directorio de instalación en `delegated` (nunca en `removed`).

### Sincronización documental

Al cerrar el código, actualizar en el mismo ciclo: `README.md` (el bloque de
desinstalación por SO, hoy con macOS en tres pasos manuales y Windows sin
mención de `setup --uninstall`), `USAGE.md` (§Desinstalación,
las tres vías de un comando), `docs/SELF-HOSTED-INSTALL.md` (diseño del
`--uninstall` multiplataforma), `CLAUDE.md` (§Comandos CLI), `docs/GOAL.md`
(criterio 10 y condición de finalización de paridad) y `docs/PARITY.md` (cerrar
la brecha de *desinstalación en un comando* en el registro y el resumen
ejecutivo, dejando solo la brecha de *firma de código* abierta).

### Definición de hecho

La brecha se considera cerrada a nivel de código cuando: `_uninstall_macos` y
`_uninstall_windows` existen y respetan el contrato compartido; el guard
`is_frozen` protege las tres ramas; la suite `TestSetupUninstall` cubre los tres
SO en verde; y la documentación queda sincronizada. **Satisfecho en v0.6.0**: las
tres ramas y el despachador viven en `cli.py`, la suite pasa (572 tests en verde)
y los siete documentos quedaron sincronizados. La **marca** del criterio de
aceptación 10 depende además de la validación por feedback de usuarios reales en
Linux y macOS (frontera E2E externa al CI; ver abajo).

## Hacia el Goal inmediato

El objetivo, los [Criterios de aceptación](GOAL.md#criterios-de-aceptación) y la
[Condición de finalización](GOAL.md#condición-de-finalización) que definen la
meta están en [docs/GOAL.md](GOAL.md). La brecha de *desinstalación en un
comando* quedó **cerrada en código** (v0.6.0), así que **no queda trabajo de
implementación pendiente del Goal inmediato**: solo falta marcar los criterios
pendientes vía feedback de usuarios reales en Linux y macOS. Cuando eso ocurra,
el Goal inmediato se considera cumplido y solo la brecha de *firma de código*
(cross-SO) queda como pieza diferida al Goal a largo plazo.
