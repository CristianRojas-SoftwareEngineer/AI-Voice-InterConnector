# Guía de Construcción

`tts-sidecar` se compila con **PyInstaller** (empaquetado de Python bytecode) para obtener un ejecutable autocontenido multiplataforma, luego se envuelve en un instalador por SO.

## Tabla de contenidos

- [1. Requisitos](#1-requisitos)
- [2. Plataformas Soportadas](#2-plataformas-soportadas)
- [3. Compilación Local](#3-compilación-local)
- [4. CI/CD con CircleCI](#4-cicd-con-circleci)
- [5. Distribución de artefactos](#5-distribución-de-artefactos)
- [6. Paquetes excluidos (bloat)](#6-paquetes-excluidos-bloat)
- [7. Notas de dependencias](#7-notas-de-dependencias)
- [8. Notas importantes](#8-notas-importantes)

---

## 1. Requisitos

- **Python 3.13+** ([python.org](https://www.python.org/downloads/))
- **PyInstaller** (`pip install pyinstaller`)

### Herramientas de empaquetado por plataforma

| Plataforma | Herramienta | Instalación |
|------------|-------------|-------------|
| Windows | Inno Setup 6 | `choco install innosetup -y --version=6.3.3` o [jrsoftware.org](https://jrsoftware.org/isdl.php) |
| Linux | appimagetool + runtime estático (type2-runtime) | Descarga automática pineada por URL + SHA-256 (`build_linux.py`); sin instalación manual |
| macOS | create-dmg | Descarga automática pineada por URL + SHA-256 del tarball del release (`build_macos.py::provision_create_dmg`); sin instalación manual |

### Política interactiva de dependencias de build

Los tres scripts de build comparten una única política, implementada en
`build_utils.ensure_build_dependency`: **verificar → avisar → preguntar →
instalar pineado o degradar**. Si una herramienta falta y hay TTY, el script
muestra el comando exacto de instalación y pregunta s/n; sin TTY (CI) no
pregunta, emite la instrucción manual y resuelve según criticidad:

- **Requeridas** (PyInstaller, sounddevice en Linux y macOS): sin ellas el
  build no tiene sentido; el script aborta si no se resuelven.
- **Empaquetadores duros** (Inno Setup, create-dmg): son dependencias duras
  del artefacto de publicación (instalador Windows y .dmg de macOS); su fallo
  de descarga/resolución aborta el build con SystemExit(1). Inno Setup se
  declara con `required=True` en `ensure_build_dependency`
  (`create_installer_windows.py`); create-dmg se provisiona pineado dentro del
  propio build (`provision_create_dmg`) y su fallo aborta.
- **Empaquetador que degrada** (appimagetool): es el único caso de degradación;
  sin él el bundle onedir sigue siendo usable y el stage AppImage degrada con
  warning (el tooling no descargable no aborta, pero un checksum que no
  coincide sí lo hace).

Las versiones pineadas viven como constantes en `scripts/build_utils.py`
(`PYINSTALLER_PIN=6.21.0`, `INNOSETUP_PIN=6.3.3`), espejo de las que instala
`.circleci/config.yml`: un build local produce el mismo artefacto que el CI.
El tooling del AppImage no se instala vía pip: `APPIMAGE_TOOLING` pinea las
URLs de release de `appimagetool` (`APPIMAGETOOL_PIN=1.9.1`) y del runtime
estático de type2-runtime (`TYPE2_RUNTIME_PIN=20251108`) con su SHA-256 por
arquitectura; `build_linux.py` los descarga a `build/appimage-tooling/`
verificando el checksum (`fetch_pinned_asset`). El runtime estático arranca
sin `libfuse2` (ausente por defecto en distros modernas), garantizando el
primer arranque del AppImage en cualquier distro. En macOS, create-dmg sigue
la misma política: `CREATE_DMG_TOOLING` pinea la URL del tarball del release
(`CREATE_DMG_PIN=1.3.0`) con su SHA-256, y `build_macos.py` lo descarga y
extrae en `build/create-dmg-tooling/` verificando el checksum.

---

## 2. Plataformas Soportadas

| Plataforma | Comando | Artefacto |
|------------|---------|-----------|
| Windows x64 | `python scripts/build_windows.py --arch x86_64` | `dist/tts-sidecar-0.1.0-x86_64-setup.exe` (instalador) |
| Linux x64 | `python scripts/build_linux.py --arch x86_64` | `dist/tts-sidecar-0.1.0-x86_64.AppImage` |
| Linux ARM64 | `python scripts/build_linux.py --arch arm64` | `dist/tts-sidecar-0.1.0-arm64.AppImage` |
| macOS arm64 (Apple Silicon) | `python scripts/build_macos.py --arch arm64` | `dist/tts-sidecar-0.1.0-arm64.dmg` |

> **Por qué Linux publica 2 arquitecturas y Windows/macOS solo 1.** Cada
> plataforma publica las arquitecturas que cumplen **a la vez** dos condiciones:
> (a) población real de usuarios y (b) wheels disponibles en el toolchain (torch,
> onnxruntime). Bajo ese criterio:
>
> - **Windows → 1 (x86_64)** por **decisión**: Windows-on-ARM es marginal en la
>   población de usuarios objetivo; el flag `--arch` solo acepta `x86_64`.
> - **macOS → 1 (arm64)** por **imposibilidad técnica**: torch≥2.3 no publica
>   wheels macOS x86_64, por lo que no es posible construir un binario Intel con
>   el toolchain actual. El artefacto se nombra por su arquitectura real (arm64).
> - **Linux → 2 (x86_64 + arm64)** porque **ambas** arquitecturas cumplen las
>   dos condiciones (usuarios reales y wheels disponibles).
>
> Los campos `os`/`cpu` de `package.json` no expresan la matriz por SO (el esquema
> no lo permite): `x64` aplica a Windows/Linux y `arm64` a Linux/macOS.

### Matriz de arquitecturas y brechas

La matriz de soporte es SO × {x86_64, arm64}. Cada SO publica las arquitecturas
que cumplen a la vez población real de usuarios y wheels disponibles en el
toolchain (torch, onnxruntime); ver el callout anterior.

| SO | x86_64 | arm64 | Artefacto |
|----|:---:|:---:|----------|
| Windows | ✅ | ❌ | `*.exe` Inno (x64) |
| Linux | ✅ | ✅ | AppImage x64 + AppImage arm64 |
| macOS | ❌ | ✅ | `.dmg` arm64 (Apple Silicon) |

> **Convención de nombres de arquitectura (unificada).** Los artefactos
> propios del proyecto usan el esquema `arm64`/`x86_64` en los tres SO, alineado
> con el flag `--arch` de los scripts de build (ya unificado a `arm64`/`x86_64`)
> y con la nomenclatura de macOS/Windows. El AppImage de Linux de ARM, que antes
> se publicaba como `*-aarch64.AppImage`, pasó a `*-arm64.AppImage`
> para eliminar la única divergencia internamente controlable.
>
> **Divergencia `aarch64` deliberadamente fuera de alcance (no es una brecha).**
> Tres cadenas `aarch64` se conservan por ser contratos externos o canónicos, no
> convenciones de naming del proyecto:
> - **URLs del tooling de AppImage** en `scripts/build_utils.py` (`appimagetool-aarch64.AppImage`,
>   `runtime-aarch64`): AppImage impone esos nombres para sus assets de upstream; el
>   mapeo vive en `APPIMAGE_TOOLING_ARCH` (`scripts/build_linux.py`) y solo resuelve el tooling.
> - **Tag de plataforma pip PEP 600** `aarch64` en `requirements-lock.txt` (p. ej.
>   `platform_machine == 'aarch64'`): canónico; pip/uv lo esperan así en Linux ARM.
> - **Salida nativa de `uname -m`** en runtime (`src/tts_sidecar/cli.py`,
>   `install-linux.sh`): es lo que devuelve Linux ARM y no debe cambiarse; el instalador
>   lo traduce a `arm64` (sufijo de asset) conservando la rama `aarch64|arm64)`.

**Arquitecturas faltantes y su justificación:**

- **Windows en ARM64 (`Windows-on-ARM`):** no soportado. Por **decisión de alcance**,
  la población de Windows-on-ARM es marginal en la audiencia objetivo (el flag
  `--arch` de `build_windows.py` solo acepta `x86_64`). Además hay un **bloqueo
  técnico**: no existen wheels estables de `torch`/`torchaudio`/`onnxruntime` para
  Windows arm64 en PyPI, así que el entorno Python del engine ni siquiera se
  resuelve. El esfuerzo del proyecto sería modesto *si existieran esos wheels*
  (añadir `arm64` a `build_windows.py`, Python arm64 en el runner, target PyInstaller
  arm64, portar el interop nativo `pycaw`/COM, `sounddevice`/WASAPI e Inno Setup
  arm64), pero hoy **no es factible** por la dependencia upstream.
- **macOS Intel (`x86_64`):** no soportado por **imposibilidad técnica**. `torch>=2.3`
  no publica wheels macOS x86_64 (foco en Apple Silicon), y un binario `universal2`
  no resolvería nada porque no hay slices x86_64 de `torch` que empaquetar. La única
  vía sería compilar `torch`/`torchaudio` desde fuente para macOS x86_64, un build
  impracticable de mantener y distribuir. Se acepta como **limitación técnica
  permanente** del toolchain actual (ver también [docs/PARITY.md](PARITY.md)).

**Divergencia aceptada en Linux arm64 (cobertura de tests):** `build-linux-arm64`
se gatea solo con el smoke `version` (importa el stack nativo en ARM y exige exit
0); no hay puerta `pytest` nativa en arm64 porque la suite es arch-independiente y
mockea el engine (ver «Simetría: 3 puertas de test vs. 4 targets de build» más
abajo). Es una **decisión consciente**;
el fast-follow de mayor ROI sería un test de integración que cargue el modelo y
sintetice en ARM, no re-correr la suite.

**Conclusión:** las dos arquitecturas faltantes no son déficits del código del
proyecto (este es arch-abstracto: Python puro + PyInstaller), sino de la
disponibilidad de wheels upstream (PyTorch/onnxruntime). macOS Intel se acepta
como limitación técnica; Windows ARM64 como decisión de alcance sumada a un bloqueo
upstream.

> Los scripts de build también generan la carpeta `--onedir` en `dist/tts-sidecar/` (o
> `dist/tts-sidecar.app/` en macOS) con el ejecutable y todas las dependencias,
> útil para pruebas directas sin pasar por el instalador.

### Matriz de SO probados y mínimos declarados

Complementa la matriz de arquitecturas: registra **en qué versiones de SO se valida
realmente** cada artefacto y qué mínimo declara. La validación efectiva ocurre en los
runners de CI; los mínimos declarados se derivan del toolchain, no de pruebas en
máquinas con esas versiones exactas.

| SO | Probado en (CI) | Mínimo declarado | Origen del mínimo |
|----|-----------------|------------------|-------------------|
| Windows | Executor `circleci/windows@5.0` (Windows Server 2022) + desarrollo en Windows 11 | Sin mínimo formal; se espera Windows 10+ x64 | Stack Inno Setup/PyInstaller sin APIs posteriores conocidas |
| Linux | `cimg/python:3.13` (base Ubuntu 22.04, glibc 2.35) en x86_64 y arm64 | glibc ≥ 2.35 | El AppImage se compila contra la glibc del runner; `install-linux.sh` advierte por debajo |
| macOS | Runner Apple Silicon con Xcode 26.4 | `LSMinimumSystemVersion` del `.dmg` | Derivado de `MACOSX_DEPLOYMENT_TARGET` del Python del build (`build_macos.py`) |

**Limitación aceptada:** los mínimos declarados **no** se prueban en máquinas
con esas versiones exactas (no hay runners de Windows 10, Ubuntu 22.04 de escritorio ni
del macOS mínimo). La matriz registra lo efectivamente probado; un reporte de fallo en
una versión mínima real reabriría esta decisión.

---

## 3. Compilación Local

### Verificación de sintaxis

Antes de compilar, verificar que el código Python no tenga errores:

```bash
python -m py_compile src/tts_sidecar/engine.py
python -m py_compile src/tts_sidecar/cli.py
python -m py_compile src/tts_sidecar/audio.py
python -m py_compile src/tts_sidecar/timing.py
python -m py_compile src/tts_sidecar/daemon/*.py
```

### Scripts de build

```bash
# Windows (requiere Inno Setup instalado)
python scripts/build_windows.py --arch x86_64
python scripts/build_windows.py --arch x86_64 --no-installer   # solo el onedir (el CI genera el instalador aparte)

# Linux (descarga appimagetool + runtime estático, pineados por SHA-256)
python scripts/build_linux.py --arch x86_64

# macOS (descarga create-dmg pineado por SHA-256)
python scripts/build_macos.py --arch arm64
```

Los scripts (`scripts/build_*.py`) ejecutan PyInstaller con `--onedir` y luego llaman
a la herramienta de empaquetado correspondiente para producir el instalador final.

**Lanzador común de PyInstaller.** Los tres scripts invocan PyInstaller a través de
`build_utils.run_pyinstaller()`, que centraliza el timeout (con kill del árbol de
procesos como red de seguridad) y, **en Windows**, reescribe la invocación para pasar
por `scripts/pyinstaller_wrapper.py`. Ese wrapper existe por un cuelgue COM: durante el
análisis, PyInstaller importa `pycaw → comtypes`, que inicializa COM en modo apartment;
en el runner headless de CI, el `CoUninitialize()` de `atexit` bloquea el shutdown del
intérprete y deja un proceso zombie que retiene el pipe de CircleCI y cuelga el job. El
wrapper arranca un bootstrap que fija `sys.coinit_flags = 0x8` (COINIT_MULTITHREADED)
**antes de cualquier import** y sale con `os._exit()`, saltándose ese cleanup. En Linux y
macOS la invocación es directa, sin wrapper.

**Fallo fatal de los empaquetadores.** Inno Setup (Windows), appimagetool (Linux) y
create-dmg (macOS) heredan la consola (su propio output es el heartbeat del step de CI) y
**su fallo aborta el build con exit ≠ 0**: un build sin instalador/AppImage/DMG nunca
reporta éxito, porque `publish-release` exige los cuatro artefactos versionados. El
instalador de Windows además emite compresión `lzma/normal` (progreso por archivo, en
lugar del `lzma2/max` silencioso que CircleCI mataba) y usa `INSTALLER_TIMEOUT` holgado.

> El entry point `bin/tts-sidecar` es la semilla que PyInstaller empaqueta. El bundle
> resultante hereda ese nombre en `dist/tts-sidecar/`. Véase `docs/DESIGN.md` para
> el detalle del entry point.

### Opciones clave de PyInstaller

```bash
python -m PyInstaller --onedir --console \
  --name tts-sidecar \
  --paths src \
  --collect-all chatterbox --collect-all transformers \
  --collect-all diffusers --collect-all torch \
  --collect-all sklearn --collect-all pandas \
  --recursive-copy-metadata chatterbox-tts \
  --copy-metadata requests \
  --exclude-module tensorflow --exclude-module gradio \
  bin/tts-sidecar
```

Los flags `--collect-all` aseguran que PyInstaller empaquete paquetes con extensiones
nativas o imports lazy que no siguen automáticamente. Los flags de metadata (`--recursive-copy-metadata`) son necesarios para que `importlib.metadata` y `pkg_resources` encuentren los metadatos de paquete en el bundle congelado.

### Verificación post-build

El **smoke test del binario congelado está automatizado en CI**: cada uno de los
4 jobs de build ejecuta `tts-sidecar version` sobre el ejecutable recién
construido (exit 0 obligatorio) **y luego `tts-sidecar voice list`** antes de
publicar el artefacto, de modo que un empaquetado roto (metadata faltante,
`--collect-all` incompleto) **o la ausencia de las voces de fábrica en el bundle**
hace fallar el job en lugar de publicarse «verde». `version` y `voice list` **no
cargan el modelo** (las voces viajan en el bundle), así que el chequeo es de
segundos: el step valida con `grep -qx '  - default'` (bash) / `-notmatch
'default'` (PowerShell) que la voz de fábrica `default` quedó empaquetada.

Queda **manual** (requiere modelo, audio real y hardware por SO): `doctor`,
`setup` y una síntesis real (`speech synthesize`). La validación end-to-end de los
instaladores por SO es por diseño **externa al pipeline** (consume mucha
cuota del runner al cargar los modelos Chatterbox y los ~6 GB de pesos (ambos modelos) en cada
build): Windows la realiza el propietario sobre su equipo local; Linux y macOS
dependen de feedback de usuarios reales que prueben la instalación y
ejecución. El smoke test automatizado en CI cubre la parte proporcional a
coste bajo: el binario congelado arranca sin errores de empaquetado. La
decisión completa está documentada en `docs/GOAL.md` §"Decisión de
validación E2E".

```bash
# Tests
pytest tests/ -v

# Ejecutable directo (carpeta onedir) — 'version' es el que corre en CI
dist/tts-sidecar/tts-sidecar.exe version
dist/tts-sidecar/tts-sidecar.exe doctor

# Provisionar los modelos es-mx-latam + en (chequeos + descarga si falta; idempotente)
dist/tts-sidecar/tts-sidecar.exe setup

# Instalador (Windows)
dist/tts-sidecar-0.1.0-x86_64-setup.exe
```

### Matriz de integración con el SO

Cada plataforma integra `tts-sidecar` en el sistema con un mecanismo distinto,
pero la experiencia resultante es homóloga (comando en el PATH + provisión
guiada + desinstalación limpia):

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| PATH | Automático: el instalador agrega `{app}` al PATH del usuario (HKCU, per-user, sin UAC) | `tts-sidecar setup` crea el symlink `~/.local/bin/tts-sidecar → $APPIMAGE` | Opt-in: `Instalar (PATH + modelos).command` del `.dmg` (symlink en `/usr/local/bin`, con sudo) |
| Guía hacia `setup` | Página informativa + casilla post-instalación que lo ejecuta en contexto de usuario | `setup` es el punto único de provisión (modelos + PATH) | El script de instalación ofrece ejecutar `setup` (sin sudo) tras enlazar |
| Desinstalación | Desinstalador de Inno Setup, sin admin (revierte la entrada de PATH en HKCU) | `tts-sidecar setup --remove-path` + borrar el `.AppImage` | `Desinstalar (quitar del PATH).command` del `.dmg` + arrastrar el `.app` a la Papelera |
| Datos provisionados | `tts-sidecar cleanup --all` (paso previo recomendado en los tres SO: elimina ambos modelos y voces de usuario antes de desinstalar el binario) | Ídem | Ídem |
| Dependencias de build | Política interactiva común (`ensure_build_dependency`) | Ídem | Ídem |

> `setup` sin `--language` descarga ambos modelos (`es-mx-latam` y `en`, el
> base de inglés para síntesis cross-lingual) a `~/.cache/huggingface/hub`; ninguno
> se empaqueta en el ejecutable. `speech synthesize` y `daemon start` fallan rápido
> remitiendo a `setup --language <idioma>` mientras el modelo requerido falte.

La tabla describe los artefactos descargados a mano. Las tres plataformas
tienen además una instalación auto-hospedada de una línea que resuelve PATH,
`setup` y desinstalación sin los pasos manuales de arriba: `install-linux.sh`
(`curl | sh`) en Linux, un Cask de Homebrew propio
(`brew install --cask tts-sidecar`, con `brew uninstall --cask` + `zap` para
la desinstalación limpia) en macOS, e `install-windows.ps1` (`irm | iex`) en Windows
(descarga el instalador, verifica el checksum y lo ejecuta en silencio, sin
UAC). Ninguno de los tres cambia el artefacto que los scripts de build
producen; todos se apoyan en el artefacto nativo tal cual. Diseño completo en
[docs/SELF-HOSTED-INSTALL.md](SELF-HOSTED-INSTALL.md).

### Limitación conocida: firma de código y notarización

Los artefactos **no están firmados ni notarizados**: en macOS, Gatekeeper
bloquea la primera apertura del `.app`/`.dmg` (clic derecho → Abrir, o
`xattr -d com.apple.quarantine`); en Windows, SmartScreen muestra la advertencia
de editor desconocido en el instalador. El porqué del mecanismo
(Mark-of-the-Web, y la firma de código como arreglo de fondo) está en
[SECURITY.md](../SECURITY.md#artefactos-sin-firmar). Firmar/notarizar con
identidad propia requiere certificados de pago (Apple Developer ID,
certificado Authenticode) y queda fuera del alcance actual del pipeline; la
ruta prevista para Windows y macOS está registrada como goal a largo plazo en
[docs/GOAL.md](GOAL.md#goal-a-largo-plazo).

Como mitigación aditiva ya implementada (no sustituye a la firma), el proyecto
publica en paralelo un **canal PyPI** (`uv tool install tts-sidecar`) que no
dispara ninguno de los dos avisos: el launcher lo genera `uv`/`pipx`
localmente, sin Mark-of-the-Web ni cuarentena. Ver
[docs/DISTRIBUTION.md](DISTRIBUTION.md) para el detalle de ambos canales y su
matriz de trade-offs.

---

## 4. CI/CD con CircleCI

El pipeline de CircleCI ejecuta los tests y, si pasan, compila el proyecto para todas las
plataformas automáticamente. Los jobs `test-linux`, `test-windows` y `test-macos` actúan
como **triple puerta simétrica**: cada build depende de los tres
(`requires: [test-linux, test-windows, test-macos]`), de modo que la suite se ejercita en
los tres SO nativos antes de compilar. A la triple puerta de la suite pytest se suman,
también como `requires` de los 4 builds, los tres smoke-tests de los instaladores de una
línea: `test-installer-linux` (bats sobre `install-linux.sh`), `test-installer-windows`
(Pester sobre `install-windows.ps1`) y `test-installer-macos` (bats sobre `install-macos.sh`).
Así, un bug específico de plataforma —Windows
(pycaw/COM, winsound, generación del `.iss`) o macOS (afplay/sounddevice, rutas y señales
POSIX)— se detecta en el gate en lugar de llegar al usuario.

Un séptimo job, `coverage`, se suma como `requires` de los 4 builds y de `publish-pypi` sin
formar parte de la triple puerta por-SO: mide y gatea la cobertura, una preocupación
**por módulo de contrato**, no por plataforma — correrlo en Linux una sola vez basta, tal
como `test-linux` fija la arquitectura de referencia arch-independiente para la suite
mockeada (ver «Por qué el runner de `test-linux` es x86_64» más abajo). Duplicarlo en los
tres SO triplicaría el coste sin aportar señal nueva sobre las líneas cubiertas.

> **Mockeo deliberado de los smoke-tests de instaladores.** Los tres jobs
> `test-installer-*` mockean por PATH las herramientas con efectos externos (`curl`,
> `sha256sum`, `hdiutil`, `xattr`, …): validan la lógica del script (parsing del
> release, verificación de checksum, guards de arquitectura y glibc, mensajes) sin
> red ni artefactos reales. Es una **decisión consciente**: ejercitar el flujo real
> exigiría descargar releases publicados en cada push (lento, frágil y circular
> durante un release). El flujo real queda validado por el propio proceso de release
> y por los smoke tests del binario congelado en los jobs de build. La cobertura es equivalente
para los tres SO **por familia de SO**: el mismo `pytest tests/` corre en cada uno. La
suite se ejercita en **una** arquitectura por SO (Linux en x86_64), no en las dos que
Linux publica; el porqué de esa asimetría se detalla en la subsección siguiente.

### Simetría: 3 puertas de test vs. 4 targets de build

Los tests (3) y los builds (4) no están desalineados: responden a **ejes distintos**.

- **Por qué 3 puertas de test y 4 builds.** Los tests son **por familia de SO**:
  validan la lógica Python (independiente de la arquitectura) más el código específico
  de cada SO (Windows: pycaw/COM, winsound, generación del `.iss`; macOS:
  afplay/sounddevice, rutas y señales POSIX; Linux: ALSA). Los builds son **por target
  de distribución**, y Linux publica **dos** arquitecturas (x86_64 + arm64). No es
  una asimetría arbitraria: son dos ejes ortogonales (SO × build-target).

- **Por qué el runner de `test-linux` es x86_64.** Es el executor Docker más barato,
  rápido y disponible. Como la suite es arch-independiente y **mockea el engine**
  (torch/onnxruntime no se ejercitan en los tests), correrla en la arquitectura más
  barata basta: un `test-linux-arm64` no aportaría señal adicional.

- **Hueco de cobertura de ARM64 (divergencia aceptada).** `build-linux-arm64` está
  *gated* por tests que solo corrieron en x86_64 → no hay una puerta `pytest` nativa en
  ARM. El riesgo arch-específico real (wheel `aarch64` faltante, segfault de una
  extensión nativa) lo cubre el **smoke test** del propio build (`tts-sidecar version`,
  que importa el stack nativo en ARM y exige exit 0), no la suite. Se documenta como
  **decisión consciente**: un `test-linux-arm64` re-correría la suite mockeada (señal
  marginal) a un coste recurrente en cada push, sin cerrar el riesgo que
  el smoke test ya cubre. Reconsiderar solo ante un bug arch-específico; el *fast-follow*
  de mayor ROI sería un test de integración que cargue el modelo y sintetice en ARM, no
  re-correr la suite.

### Arquitectura del Pipeline

```
┌────────────────────┐  ┌────────────────────┐  ┌────────────────────┐
│     test-linux     │  │    test-windows    │  │     test-macos     │
│ (pytest — Linux)   │  │ (pytest — Windows) │  │  (pytest — macOS)  │
└─────────┬──────────┘  └─────────┬──────────┘  └─────────┬──────────┘
          └──────────────────────┬┴───────────────────────┘
        ┌───────────────┬────────┴──────┬───────────────┐
        ▼               ▼               ▼               ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌──────────────────┐
│build-windows│ │build-linux- │ │build-linux- │ │ build-darwin-    │
│ -x64+ Inno  │ │    x64      │ │   arm64     │ │     arm64        │
│  Setup      │ │ + AppImage  │ │ + AppImage  │ │  + create-dmg    │
└─────────────┘ └─────────────┘ └─────────────┘ └──────────────────┘
     (cada build corre además un smoke test `version` y `voice list` del binario congelado)
```

### Jobs

| Job | Plataforma | Executor | Notas |
|-----|------------|----------|-------|
| `test-linux` | Linux x64 | docker `cimg/python:3.13` | `pytest tests/` en Linux (puerta previa) |
| `test-windows` | Windows x64 | `win/server-2022` | `pytest tests/` en Windows nativo (puerta previa) |
| `test-macos` | macOS arm64 (Apple Silicon) | macos `m4pro.medium` (Xcode 26.4.0) | `pytest tests/` en macOS nativo (puerta previa) |
| `coverage` | Linux x64 | docker `cimg/python:3.13` | `pytest tests/ --cov` + gate por módulo (`scripts/check_coverage.py`); publica `coverage.xml` como artefacto (puerta previa, independiente de la triple puerta por-SO) |
| `build-windows-x64` | Windows x64 | `win/server-2022` | **Tres steps:** «Build Windows x64 onedir via PyInstaller» (`--no-installer`, `no_output_timeout: 20m`), «Generate Windows x64 installer via Inno Setup» (`create_installer_windows.py`, `no_output_timeout: 25m`), e «Install Inno Setup via choco (pinned 6.3.3)» como step propio separado de las deps Python |
| `build-linux-x64` | Linux x64 | docker `cimg/python:3.13` (`large`) | PyInstaller onedir + AppImage |
| `build-linux-arm64` | Linux ARM64 | docker `cimg/python:3.13` (`arm.medium`) | PyInstaller onedir + AppImage |
| `build-darwin-arm64` | macOS arm64 (Apple Silicon) | macos `m4pro.medium` (Xcode 26.4.0) | PyInstaller onedir + .app + .dmg |
| `publish-release` | — (CD) | docker `cimg/base:current` | Solo en tags `v*`: recolecta los 4 artefactos por workspace, genera `SHA256SUMS.txt` y publica el GitHub Release directo (sin borrador) |

**Instalador de Windows como step separado.** En `build-windows-x64`, PyInstaller y la
generación del instalador Inno Setup son dos steps distintos: el primero corre
`build_windows.py --no-installer` (solo el onedir), y el segundo invoca directamente a
`create_installer_windows.py`. Cada uno declara su propio `no_output_timeout`, de modo que
ninguna de las dos etapas largas comparte el presupuesto de silencio de la otra ni cae en
el default de 10 min.

**Logging homogéneo `[STEP] … INICIO/FIN`.** Todos los run-steps de los 8 jobs enmarcan su
comando con marcadores `[STEP] <nombre> - INICIO` / `- FIN` (`echo` en los jobs bash,
`Write-Host` en los PowerShell). En PowerShell, cada invocación que puede fallar va seguida
de un chequeo `if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }` **antes** del marcador FIN:
sin él, el `Write-Host` final resetearía el exit code del step y lo pondría verde pese al
fallo. Los steps con riesgo de silencio prolongado (`pyenv install`, builds, instalador)
declaran `no_output_timeout` explícito.

### Cacheo de dependencias y toolchain

Los 8 jobs con Python usan `save_cache`/`restore_cache` de CircleCI para no
descargar ~1.5–2.5 GB de wheels ni recompilar CPython en cada corrida. Hay dos
tipos de cache, con claves independientes:

- **Cache de venv (dependencias instaladas).** Cada job instala sus dependencias
  en un venv (`~/.venv` en los jobs bash; `.venv` del proyecto en los PowerShell,
  que no tienen el equivalente de `$BASH_ENV` y por eso invocan
  `.venv\Scripts\python.exe` por ruta explícita en los steps posteriores). La
  clave es `venv-v1-{{ arch }}-{{ checksum "cache-key.txt" }}-{{ checksum
  "<lockfile>" }}`, donde `cache-key.txt` contiene solo la versión exacta de
  Python efectiva (`python --version`): **no** incluye pins de herramienta
  (`pytest` / `PyInstaller`). Como resultado, **la caché de venv es compartida
  entre el job de test y el job de build de cada plataforma+lockfile** —
  mismo Python, mismo lockfile, misma clave — y el build acierta HIT desde el
  test que corre antes en la misma pipeline. Un cambio de lockfile o de patch
  de Python invalida el cache; los pins de herramienta se reinstalan en cada
  corrida (después del `save_cache`, sobre el venv ya poblado), pero cuestan
  segundos al no tener dependencias pesadas. En cache hit, `pip install
  --require-hashes` sigue corriendo y re-verifica el lock como no-op rápido:
  el determinismo de instalación no se relaja.

- **Cache de CPython compilado (pyenv).** Solo los jobs macOS (`test-macos` y
  `build-darwin-arm64`) compilan CPython desde fuente vía pyenv (~8–15 min); el
  executor `macos` no ofrece una imagen con Python preinstalado equivalente a
  `cimg`. El patch se fija **exacto** (`pyenv install -s 3.13.14`, espejo del pin
  `3.13.14` de Chocolatey en los jobs Windows — versión flotante y clave de
  cache estable son incompatibles) y `~/.pyenv/versions` se cachea con clave
  `pyenv-v1-{{ arch }}-3.13.14-xcode26.4`; la clave incluye el literal de la
  versión de Xcode porque el CPython compilado depende del SDK del runner. Con el
  cache restaurado, `pyenv install -s` salta la compilación en segundos. Separar
  esta clave de la del venv evita que un cambio de lockfile fuerce recompilar
  Python. Antes de instalar, ambos jobs corren `brew update && brew upgrade
  pyenv` (el `python-build` que trae la imagen es un snapshot vendido con el
  formula de Homebrew y puede no incluir todavía la definición de un patch recién
  liberado).

  Los builds de Linux (`build-linux-x64` y `build-linux-arm64`) **no** usan
  pyenv: corren sobre `docker: cimg/python:3.13`, que ya trae Python. Esto era
  obligado en arm64 —donde el `machine` executor guarda pyenv en
  `/opt/circleci/.pyenv`, propiedad de otro usuario: ni se puede actualizar
  (`git pull` sin permiso de escritura) ni la ruta de cache `~/.pyenv/versions`
  coincidía con la de instalación (se cacheaba un directorio vacío)— y de paso lo
  vuelve simétrico con x64. El build de AppImage no necesita FUSE
  (`--appimage-extract-and-run`), así que Docker basta.

Los caches de CircleCI son **inmutables por clave**: para invalidar todo el
conjunto manualmente, incrementar el prefijo versionado (`v1-` → `v2-`) en
`.circleci/config.yml`.

### Reproducibilidad: pines por digest y sus implicaciones

Recompilar el mismo tag semanas después debe producir los mismos artefactos.
Para cerrar las fuentes de deriva controlables, el CI fija:

- **Imágenes Docker por digest**: las referencias `cimg/python:3.13` de los
  jobs Docker (`test-linux`, `coverage`, `build-linux-x64`, `build-linux-arm64`)
  llevan la forma `cimg/python:3.13@sha256:<digest>`. El digest es el del
  **manifest list** del tag (multi-arch), así que el mismo pin sirve para
  amd64 y arm64.
- **pip con versión exacta**: los ocho `pip install pip==<versión>` de los
  jobs (incluye `coverage`) reemplazan al antiguo `--upgrade pip` sin versión,
  que instalaba «lo último» en cada corrida.

**Excepciones conscientes** (documentadas en el propio config):

- `brew update && brew upgrade pyenv` (jobs macOS): Homebrew no soporta fijar
  una versión de pyenv, y el upgrade es funcionalmente necesario para que
  `python-build` conozca la definición del parche fijado (3.13.14). No afecta
  la reproducibilidad del artefacto: el CPython resultante ya está pineado.

**Costo de mantenimiento de la decisión** (asumido de forma explícita):

- Los parches de la imagen `cimg/python` (seguridad del SO base, actualizaciones
  del Python 3.13.x que trae) **dejan de llegar solos**: el digest congela la
  imagen y las actualizaciones requieren un bump manual.
- Lo mismo aplica al pin de pip: correcciones de pip llegan solo al subir el pin.

**Procedimiento de actualización** (hacerlo de forma deliberada, p. ej. al
preparar un release):

1. Obtener el digest vigente del manifest list del tag:
   `https://hub.docker.com/v2/repositories/cimg/python/tags/3.13`
   (campo `digest`), o `docker buildx imagetools inspect cimg/python:3.13`.
2. Reemplazar el digest en las **tres** referencias `image:` de
   `.circleci/config.yml` (deben quedar idénticas).
3. Para pip: consultar la versión vigente (`https://pypi.org/pypi/pip/json`) y
   reemplazarla en los **siete** `pip install pip==…` (uniforme en todos los jobs).
4. Validar con un pipeline en verde: los pines nuevos no deben romper ningún job.

El archivo de configuración completo está en `.circleci/config.yml`.

### CD: publicación del GitHub Release (`publish-release`)

Al pushear un tag `v*`, además de tests + builds corre `publish-release`
(estrategia 1, GitHub Releases). Recolecta los 4 artefactos **versionados** por
`persist_to_workspace`/`attach_workspace` (no `gh run download`: se queda dentro
del pipeline, es determinista y no requiere token de API de CircleCI), genera
`SHA256SUMS.txt`, extrae las notas de la sección `[X.Y.Z]` de `CHANGELOG.md`
(fail-fast si no existe) y publica el GitHub Release directo (sin borrador). El
detalle del runbook está en `docs/RELEASING.md`.

Requisito operativo: el context `github-release` en CircleCI con `GH_TOKEN`
(fine-grained PAT, permiso `contents: write` sobre el repo), aislado al job de
release. Para que CircleCI ejecute jobs en un tag, el job **y todas sus
dependencias** deben declarar `filters.tags`; por eso el filtro `v*` se propaga
por tests → builds → `publish-release`.

---

## 5. Distribución de artefactos

El **deliverable** que se publica a usuarios es el artefacto **empaquetado**
(instalador `.exe`, AppImage, `.dmg`), con su nombre de release **versionado
y con arch** (p. ej. `tts-sidecar-0.1.0-x86_64-setup.exe`). Estos cuatro
artefactos llegan al GitHub Release a través de `persist_to_workspace` /
`attach_workspace` (handoff entre el job de build y `publish-release`): la
publicación es **única** por el workspace, no por `store_artifacts` (el cual se
retiró del pipeline: generaba una segunda copia redundante en la pestaña
Artifacts de CircleCI, ~2 min de subida medidos en builds grandes, y
duplicaba el onedir/.app crudo cuyo contenido ya viaja dentro del
empaquetado).

El output del build en el runner vive en `dist/` y se reduce a los cuatro
artefactos versionados:

```
dist/
├── tts-sidecar-0.1.0-x86_64-setup.exe   # Windows (instalador Inno Setup)
├── tts-sidecar-0.1.0-x86_64.AppImage    # Linux x64
├── tts-sidecar-0.1.0-arm64.AppImage   # Linux ARM64
└── tts-sidecar-0.1.0-arm64.dmg          # macOS (Apple Silicon)
```

El **onedir** de PyInstaller (`dist/tts-sidecar/` o `dist/tts-sidecar-arm64.app/`)
se genera como input del empaquetado y del smoke test, pero **no** se sube a
la pestaña Artifacts de CircleCI: ya está contenido en el instalador/AppImage
correspondiente, y subirlo aparte duplica el almacenamiento sin agregar
información al Release. Los pasos `store_artifacts` quedaron retirados del
`.circleci/config.yml`; la cadena de release se mantiene íntegra (los
`attach_workspace` de `publish-release` siguen trayendo los cuatro
empaquetados).

---

## 6. Paquetes excluidos (bloat)

Los siguientes paquetes no se usan en runtime y están excluidos del bundle:

| Paquete | Razón |
|---------|--------|
| `gradio` + `gradio_client` | UI web, fuera del path TTS |
| `tensorflow`, `jax`, `flax` | Shims de transformers no cargados en runtime |

---

## 7. Notas de dependencias

### Lockfile de dependencias (`requirements-lock.txt`)

El CI y los builds **no** instalan desde `requirements.txt` (límites `>=` de
desarrollo), sino desde `requirements-lock.txt`: un **lock universal con hashes**
que fija la versión exacta de cada dependencia de runtime (directa y transitiva)
para builds reproducibles e íntegros. Los 8 jobs de CI con Python instalan su
lockfile (el universal, o el CPU-only de Linux en `test-linux`, `coverage` y
`build-linux-x64` — ver la sección siguiente) con `--require-hashes`, que
rechaza cualquier paquete cuyo contenido no coincida con el hash fijado
(barrera de supply-chain).

El lock es **universal**: un solo archivo cubre Windows, Linux y macOS mediante
marcadores de entorno (`sys_platform`, etc.), imprescindible porque el grafo de
`torch` diverge por plataforma (wheels NVIDIA/CUDA solo en Linux). Se genera con
[uv](https://github.com/astral-sh/uv), cuyo resolver universal produce esa matriz
en un único archivo (pip-tools resuelve solo para la plataforma donde corre, y no
puede hacerlo).

El hecho de que el lock universal *incluya* el stack `nvidia-*-cu12` para
`linux`/`x86_64` es **deliberado y está justificado**: es el grafo que requiere
el canal PyPI y la compilación desde código fuente para ofrecer aceleración
NVIDIA en Linux x64 (ver «Componentes propietarios redistribuibles: NVIDIA
CUDA» en `THIRD-PARTY-LICENSES.md`). No es un defecto de licencia ni de
distribución: el binario nativo distribuido en AppImage se construye desde el
lock CPU-only (sección siguiente) y por tanto **no** empaqueta esos paquetes.
La presencia de `nvidia-*` en el lock universal está acotada a la vía (PyPI /
fuente) que sí la necesita.

**Regeneración deliberada** (tras cambiar dependencias en `pyproject.toml`):

```bash
pip install uv   # si no está disponible
uv pip compile --universal --generate-hashes --python-version 3.13 \
    pyproject.toml -o requirements-lock.txt
```

Actualizar el lock es una acción **consciente**, no automática: revisar el diff
antes de commitear para auditar qué versiones y hashes cambian. Las herramientas
de build (`pyinstaller`, `pytest`) se instalan aparte con su pin exacto (`==`),
en invocaciones de pip separadas del lock.

### Lock CPU-only de Linux (`requirements-lock-linux-cpu.txt`)

El lock universal resuelve, para `sys_platform == 'linux' and platform_machine
== 'x86_64'`, el stack `nvidia-*-cu12` (~41 paquetes) que PyPI empareja por
defecto con `torch` en esa combinación de plataforma/arquitectura — el AppImage
lo arrastraba vía `--collect-all torch` aunque el proyecto no usa GPU NVIDIA.
`arm64` no se ve afectado (esos marcadores excluyen `platform_machine !=
'x86_64'`), así que instalan desde este lock alternativo los jobs de esa
plataforma: `build-linux-x64`, `test-linux` y `coverage` (la suite mockea el
engine — torch no se ejercita — y el build de la misma plataforma ya usa
exactamente este lock, así que el stack CUDA solo agregaría varios GB sin
señal adicional).
`build-linux-arm64`, `test-windows`, `test-macos` y los builds de
Windows/macOS siguen usando `requirements-lock.txt`.

`requirements-lock-linux-cpu.txt` fija `torch`/`torchaudio` a los wheels
`+cpu` del índice oficial de PyTorch en vez de los de PyPI, sin ningún paquete
`nvidia-*`. Se regenera con:

```bash
uv pip compile --generate-hashes --python-version 3.13 \
    --python-platform x86_64-unknown-linux-gnu \
    --extra-index-url https://download.pytorch.org/whl/cpu \
    --index-strategy unsafe-best-match \
    pyproject.toml -o requirements-lock-linux-cpu.txt
```

**Por qué `--index-strategy unsafe-best-match` es imprescindible (y no basta con
`--extra-index-url`).** El `index-strategy` por defecto de `uv` es
`first-index`: para cada paquete, resuelve contra el **primer** índice donde
aparece y no mira los demás. Como `torch`/`torchaudio` sí existen en el índice
por defecto (PyPI, con su build CUDA), `uv` se quedaría resolviendo ahí y jamás
llegaría a evaluar `https://download.pytorch.org/whl/cpu` — el `--extra-index-url`
por sí solo no tiene efecto en ese caso, solo importa para paquetes que PyPI no
resuelve. `unsafe-best-match` cambia la estrategia a comparar versiones entre
**todos** los índices declarados y quedarse con la mejor coincidencia, dejando
que gane el wheel `+cpu` de `download.pytorch.org` sobre el wheel CUDA de PyPI
para `torch`/`torchaudio` (el resto de paquetes —incluidas sus dependencias
transitivas— sigue resolviendo normalmente contra PyPI). Es el mismo
comportamiento (permisivo) que usa `pip` por defecto al mezclar índices; la
contrapartida documentada por `uv` es el riesgo de *dependency confusion* si
alguno de los índices no fuera de confianza — aceptable aquí porque ambos
(PyPI oficial y el índice oficial de PyTorch) lo son.

**Cuándo regenerar este lock.** Cada vez que se regenera `requirements-lock.txt`
tras editar `pyproject.toml` (nueva versión de `torch`, `chatterbox-tts`, o
cualquier dependencia de runtime), regenerar también este lock CPU-only con el
comando de arriba y revisar el diff: un cambio de versión de `torch` puede no
tener todavía wheel `+cpu` publicado para el `--python-version` fijado, lo que
haría fallar la resolución y hay que esperar a que PyTorch lo publique antes de
subir el pin.

Un usuario que necesite aceleración NVIDIA debe compilar desde código fuente
instalando el `requirements-lock.txt` universal (que sí resuelve el stack CUDA
en x86_64/Linux) en vez de usar el AppImage distribuido.

Esta exclusión de CUDA del AppImage x86_64 es una **decisión de diseño
deliberada y justificada**, no una omisión: el AppImage es el artefacto
portátil «descarga y funciona en cualquier distro», y empaquetar el stack
CUDA (varios GB) solo inflaría el bundle sin beneficio para la mayoría de
usuarios, cuya inferencia es CPU-first. Quien requiera GPU en Linux usa el
canal PyPI (o compila desde código fuente con el lock universal), donde el
stack CUDA sí se resuelve.

### `chatterbox-tts` metadata

`chatterbox/__init__.py` llama `importlib.metadata.version("chatterbox-tts")` al importar.
Sin `--recursive-copy-metadata chatterbox-tts`, el comando `doctor` reporta "NOT INSTALLED"
en el bundle congelado.

### Audio por plataforma

| Plataforma | Librería | Notas |
|------------|----------|-------|
| Windows | `pycaw` | Incluida (enumeración; reproduce `winsound`, built-in) |
| Linux | `sounddevice` | Incluida (reproducción y enumeración) |
| macOS | `afplay` (built-in) + `sounddevice` | `afplay` reproduce; `sounddevice` (incluida en el bundle) enumera dispositivos para `doctor`/`devices` |

### Paquetes recopilados con `--collect-all`

PyInstaller no sigue automáticamente imports lazy ni extensiones nativas en runtime.
Los paquetes que requieren `--collect-all` son: `chatterbox`, `transformers`,
`diffusers`, `torch`, `sklearn`, `pandas`, `s3tokenizer`, `perth`, `librosa`, `onnx`, `pycaw`.

---

## 8. Notas importantes

- **PyInstaller --onedir**: genera una carpeta con el ejecutable y todas las dependencias
  (del orden de 1-2 GB sin comprimir en Windows/macOS y en el AppImage `arm64`
  de Linux, que resuelven `torch` desde `requirements-lock.txt`). El AppImage
  `x86_64` de Linux, construido desde `requirements-lock-linux-cpu.txt` (ver
  «Lock CPU-only de Linux» más arriba), es sensiblemente más liviano al no
  arrastrar el stack `nvidia-*-cu12`; el tamaño exacto queda pendiente de medir
  en un run de CI (`build-linux-x64`) y actualizar aquí. Es el artefacto que el
  script de empaquetado consume.
- **Tiempo de build**: ~10 min en frío, ~5 min incremental.
- **Windows**: el instalador Inno Setup es el artefacto que recibe el usuario final;
  ajusta el `PATH`, muestra la página informativa de los modelos y ofrece ejecutar `setup`.
- **Linux**: el AppImage es un único archivo ejecutable, compatible con la mayoría de
  distribuciones; `tts-sidecar setup` lo integra en el PATH (symlink en `~/.local/bin`).
- **macOS**: el `.dmg` es el instalador estándar de macOS; incluye el `.app` bundle más
  los scripts de instalación (PATH + oferta de `setup`) y desinstalación.
- **Firma de código**: ver la limitación conocida en la sección 3 (artefactos sin
  firmar/notarizar: Gatekeeper y SmartScreen advierten en el primer arranque).
