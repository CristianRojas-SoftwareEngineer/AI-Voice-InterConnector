# Guía de construcción

`ai-voice-interconnector` se compila como un **binario Rust**
(`cargo build --release --features full`; CTranslate2 (ct2rs) compilado estático +
Parakeet vía ort load-dynamic (sin enlace en build)) y se **empaqueta por SO en un
archivo comprimido** (`tar.gz` en los 3 targets Unix, `.zip` en Windows) que
agrupa el binario con el motor TTS, ONNX Runtime y los documentos de licencia
MPL-2.0. El binario y el motor enlazan además librerías dinámicas que el archivo
no trae: ver [Dependencias de runtime por plataforma](#dependencias-de-runtime-por-plataforma).

Esta guía explica cómo se compila y por qué se compila así. Las versiones y los
pines viven en su fuente y no se copian aquí: [`packaging/pins.json`](../packaging/pins.json)
(Rust, ONNX Runtime y su SHA-256, herramientas de CI), [`packaging/bundle-manifest.json`](../packaging/bundle-manifest.json)
(contrato de cada target), [`.circleci/config.yml`](../.circleci/config.yml)
(executors y pipeline) y `[patch.crates-io]` en [`Cargo.toml`](../Cargo.toml)
(copias vendorizadas).

## Tabla de contenidos

- [1. Requisitos](#1-requisitos)
- [2. Plataformas soportadas](#2-plataformas-soportadas)
  - [Matriz de arquitecturas y brechas](#matriz-de-arquitecturas-y-brechas)
  - [SO probados y mínimos declarados](#so-probados-y-mínimos-declarados)
  - [Versión mínima de macOS](#versión-mínima-de-macos)
  - [Dependencias de runtime por plataforma](#dependencias-de-runtime-por-plataforma)
- [3. Compilación local](#3-compilación-local)
  - [Verificación rápida](#verificación-rápida)
  - [Build de distribución](#build-de-distribución)
  - [ONNX Runtime vía `load-dynamic` (sin build en compilación)](#onnx-runtime-vía-load-dynamic-sin-build-en-compilación)
  - [Verificación post-build](#verificación-post-build)
- [4. CI/CD con CircleCI](#4-cicd-con-circleci)
- [5. Distribución de artefactos](#5-distribución-de-artefactos)
- [6. Build nativo del motor TTS (Rust/qwen_tts)](#6-build-nativo-del-motor-tts-rustqwen_tts)
  - [Interfaz uniforme: `xtask build-engine`](#interfaz-uniforme-xtask-build-engine)
  - [Toolchain y flags por plataforma](#toolchain-y-flags-por-plataforma)
  - [Caché del motor en CI](#caché-del-motor-en-ci)
- [7. Descarga nativa de modelos (`hf-hub`)](#7-descarga-nativa-de-modelos-hf-hub)
  - [Parche de `hf-hub`](#parche-de-hf-hub)
  - [Ubicaciones en disco por SO](#ubicaciones-en-disco-por-so)
- [Limpieza del entorno de desarrollo](#limpieza-del-entorno-de-desarrollo)

---

## 1. Requisitos

La tabla única de requisitos vive en el código y la comprueba
`cargo xtask doctor` (solo lectura): ejecuta ese comando en tu host en lugar
de contrastar esta guía.

El toolchain de Rust basta para compilar y empaquetar; no se requiere ningún otro.

---

## 2. Plataformas soportadas

> **Por qué Linux publica 2 arquitecturas y Windows/macOS solo 1.** Cada
> plataforma publica las arquitecturas que cumplen **a la vez** dos condiciones:
> (a) población real de usuarios y (b) capacidad del toolchain Rust. Bajo ese
> criterio:
>
> - **Windows → 1 (x86_64)** por **decisión**: Windows-on-ARM es marginal en la
>   población objetivo.
> - **macOS → 1 (arm64)** por **decisión**: el único runner de macOS disponible es
>   Apple Silicon, así que es la única arquitectura que se compila, prueba y
>   publica aquí. Publicar también Intel exigiría un runner propio que no existe,
>   y su población es residual junto a la de Apple Silicon.
> - **Linux → 2 (x86_64 + arm64)** porque **ambas** arquitecturas tienen usuarios
>   y runners nativos.

### Matriz de arquitecturas y brechas

| SO | x86_64 | arm64 | Artefacto |
|----|:---:|:---:|----------|
| Windows | ✅ | ❌ | `.zip` (x64) |
| Linux | ✅ | ✅ | `tar.gz` x64 + `tar.gz` arm64 |
| macOS | ❌ | ✅ | `tar.gz` arm64 (Apple Silicon) |

Los nombres de los archivos, su formato y su contenido están en
[DISTRIBUTION.md](DISTRIBUTION.md#formato-de-los-artefactos).

**Arquitecturas faltantes y su justificación:**

- **Windows en ARM64:** no soportado por decisión de alcance (población marginal).
- **macOS Intel (x86_64):** no soportado; se acepta como limitación permanente.

### SO probados y mínimos declarados

Los mínimos que ve el usuario están en [README.md](../README.md#requisitos); el
de macOS, además, en `min_os` de `packaging/bundle-manifest.json`. El entorno en
el que CI compila y prueba cada SO está en `.circleci/config.yml`. Lo que no
está en ninguna de esas fuentes es de dónde sale cada mínimo:

- **Windows.** Binario Rust sin APIs posteriores al mínimo declarado; el motor
  TTS incrusta un manifiesto `activeCodePage` UTF-8, que las versiones
  anteriores ignoran (el motor recibiría el texto y las rutas no ASCII en la
  página de códigos ANSI).
- **Linux.** El binario `gnu` se enlaza contra la glibc de la imagen `manylinux`
  de build, y su techo de símbolos es el de `symbol_ceilings` en el manifiesto.
  `xtask package` audita ese techo y el bootstrap lo diagnostica con la
  comprobación de arranque (`binary_incompatible`). Además del build, el
  artefacto se prueba en salas limpias de distribuciones sin paquetes de
  desarrollo.
- **macOS.** `MACOSX_DEPLOYMENT_TARGET` del job `build-darwin-arm64` iguala el
  `min_os` del manifiesto; ver [Versión mínima de macOS](#versión-mínima-de-macos).

**Limitación aceptada:** los mínimos declarados **no** se prueban en máquinas
con esas versiones exactas. Un reporte de fallo reabriría la decisión.

### Versión mínima de macOS

El criterio es soportar solo versiones de macOS que reciben actualizaciones de
seguridad y que no son las siguientes en perderlas: mantener un target que está
por quedar obsoleto cuesta más de lo que aporta. La versión vigente es `min_os`
de [`packaging/bundle-manifest.json`](../packaging/bundle-manifest.json).

- **Hardware.** El mínimo corre en todos los Mac con Apple Silicon, desde los
  M1 de 2020, así que exigirlo no deja fuera ningún Mac que el producto soporte
  (Mac Intel no es target).
- **Restricciones técnicas, todas por debajo del mínimo.** El `minos` de la
  librería de ONNX Runtime empaquetada, el SDK de Xcode del runner, el target
  `aarch64-apple-darwin` de Rust, la API de LAPACK nueva de Accelerate
  (`ACCELERATE_NEW_LAPACK`) y CTranslate2 admiten desplegar desde una versión
  anterior, así que el mínimo lo fija el criterio de soporte y no la técnica.

El job `build-darwin-arm64` fija `MACOSX_DEPLOYMENT_TARGET` al `min_os` del
manifiesto (ver `.circleci/config.yml`), variable que leen rustc, el crate `cc`,
CMake y el clang del Makefile del motor, y comprueba con `cargo xtask package`
que ningún binario del bundle, incluida la librería de ONNX Runtime, declare un
`minos` mayor. El motor se compila con `-mcpu=apple-m1`: M1 es el chip más
antiguo que soporta el mínimo, y su código corre en todos los posteriores. El
Cask de Homebrew y el diagnóstico `binary_incompatible` de `install.sh` nombran
esta versión mínima.

### Dependencias de runtime por plataforma

El archivo de cada target solo depende del **sistema base** de su plataforma:
no hay que instalar nada más. El contrato (qué librerías del sistema puede
importar el bundle y qué techos de símbolos y de `minos` respeta) está declarado
por target en `packaging/bundle-manifest.json` y `cargo xtask package` lo hace
cumplir antes de comprimir (ver [Auditoría de dependencias](#auditoría-de-dependencias)).
Lo que el usuario necesita en cada SO está en [README.md](../README.md#requisitos).

En **Windows**, el binario principal y CTranslate2 enlazan el CRT estático
(`+crt-static`), el motor TTS se enlaza con `-static`, y el archivo trae las
cinco DLL del runtime de VC++ que no forman parte de Windows:
`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll` y `msvcp140_1.dll`,
que necesita ONNX Runtime, y `vcomp140.dll`, el runtime OpenMP de MSVC que el
binario importa porque CTranslate2 se compila con `/openmp`. Se empaquetan junto
al binario (*app-local deployment*, permitido por la licencia de redistribución
de Microsoft) para no exigir al usuario el *Visual C++ Redistributable*:
`cargo xtask package` las copia de System32 del runner. El Universal CRT
(`ucrtbase.dll`) ya forma parte de Windows.

En **Linux**, el binario se compila contra la glibc de la imagen `manylinux` y
no declara más librerías que las del sistema base:

- **ALSA** se carga en tiempo de ejecución: la copia vendorizada de `alsa-sys`
  (`vendor/alsa-sys-*`) resuelve cada función con `dlopen`/`dlsym` sobre
  `libasound.so.2` en el primer uso. Antes de crear el host de audio se
  resuelven todas las funciones de ALSA que alcanza el binario
  (`vendor/alsa-sys-*/src/required_symbols.rs`). La lista contiene
  exactamente esas funciones: cada envoltura guarda su dirección en un estático
  llamado `avi_alsa_fn_<función>` que el enlazador conserva solo si el binario
  la alcanza, de modo que la tabla de símbolos del binario enlazado la define.
  Se genera con `cargo xtask alsa-symbols --binary <binario Linux>` y se exige
  la igualdad exacta con `--check` en `test-linux` (binario de pruebas) y en la
  auditoría de `cargo xtask package` (binario que se publica, en x86_64 y
  arm64), allí donde cargo dice que `alsa-sys` está en el grafo del target.
  Sin ALSA, o con una ALSA a la que le
  falte alguna de esas funciones, el binario arranca y solo `devices` y las
  órdenes que reproducen o capturan audio fallan, con un error que nombra la
  librería y, si es el caso, la función ausente.
- **OpenMP y C++** se piden estáticas en origen, cada una detrás del crate
  que la necesita. La política vive solo en `.cargo/config.toml`:
  `CXXSTDLIB_<target>` y `GOMPLIB_<target>` (para `x86_64` y `aarch64`
  `unknown_linux_gnu`) valen `static:-bundle=stdc++` y `static:-bundle=gomp`.
  Cada crate declara qué necesita y emite tal cual esa petición:
  - el crate `cc`, en cada crate C++ (`cxx`, `sentencepiece-sys`,
    `esaxx-rs`), con `CXXSTDLIB_<target>`;
  - la copia vendorizada de `ct2rs` (`vendor/ct2rs-*`), que compila
    CTranslate2 con el OpenMP de GNU, con `GOMPLIB_<target>`;
  - la copia vendorizada de `onednn-src` (`vendor/onednn-src-*`), con las
    dos, según el contrato `avi-link-runtimes.txt` que escribe su script de
    CMake (`avi-link-runtimes.cmake`, inyectado con `CMAKE_PROJECT_INCLUDE`) al
    final de la configuración: `cxx=` con las librerías implícitas del
    compilador (`stdc++`) y `openmp=` con el runtime OpenMP (`gomp`). `dnnl` va
    detrás de los demás crates C++, y el original no las pedía: con las `.a`,
    en arm64 quedaban símbolos sin resolver. Si falta el contrato o una de sus
    líneas, la compilación falla nombrándolo.

  Si falta la variable que un crate necesita, su compilación falla
  nombrándola. rustc pasa cada petición entre `-Bstatic` y `-Bdynamic`, de
  modo que el enlazador solo acepta `libgomp.a` y `libstdc++.a`, que aporta el
  compilador; sin ellas el enlace falla, y `cargo xtask doctor` comprueba de
  antemano que `gcc -print-file-name` las encuentra. Solo `libonnxruntime.so`, la librería
  oficial de Microsoft, usa la `libstdc++.so.6` del sistema, dentro del techo
  `GLIBCXX` del manifiesto.
- El motor TTS (`qwen_tts`) enlaza OpenBLAS y el runtime Fortran que usa su
  LAPACK (`libgfortran` y, en x86_64, `libquadmath`) de forma estática.

En **macOS** el motor usa el framework Accelerate y el audio CoreAudio, que
forman parte del sistema.

En **Linux y macOS** la parada del daemon y su registro observan la identidad
de los procesos sin herramientas externas: en Linux, de una sola lectura de
`/proc/<pid>/stat` (`starttime` y `comm`); en macOS, con
`proc_pidinfo(PROC_PIDTBSDINFO)`.

#### Auditoría de dependencias

`cargo xtask package` lee cada ejecutable y librería del bundle montado (ELF,
Mach-O o PE, con el crate `object`) y falla, nombrando fichero y dependencia, si
importa una librería que no es del sistema base del target ni del propio
bundle, si exige una versión de símbolo por encima del techo de su familia o si
declara un `minos` mayor que el mínimo de macOS. Reporta todas las violaciones
juntas. `cargo xtask deps --check --dir <ruta>` aplica la misma auditoría a un
árbol ya montado o extraído, con el contrato del target del host.

---

## 3. Compilación local

### Verificación rápida

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test --all
cargo test --all --verbose   # pruebas de contrato (incluye el harness dorado `cli_golden`); las de recursos locales salen como ignored
```

### Build de distribución

```bash
# Binario featureless (sin STT/traducción, rápido, sin C++)
cargo build --release

# Binario completo (distribución, con STT + traducción)
cargo build --release --features full

# Ejecutar el binario recién compilado
./target/release/ai-voice-interconnector version
./target/release/ai-voice-interconnector voice list
./target/release/ai-voice-interconnector setup
./target/release/ai-voice-interconnector doctor
```

En Windows, el producto y `cargo xtask` fijan la consola a UTF-8 al arrancar
(helper compartido `avi_shared::force_utf8_console`), así que los mensajes
con tildes se ven bien aunque la PowerShell use otra página de códigos.

### ONNX Runtime vía `load-dynamic` (sin build en compilación)

El motor STT Parakeet consume **ONNX Runtime** a través del crate `ort`, en la
versión exacta y con la feature **`load-dynamic`** que declara
[`crates/avi-stt/Cargo.toml`](../crates/avi-stt/Cargo.toml): no
enlaza nada en build-time ni requiere una `.lib`, y **el build local/CI de la app
no necesita compilar ni tener presente ONNX Runtime**. Por eso las cuatro
arquitecturas compilan `--features full` sin ningún paso previo de ONNX Runtime.

En runtime, `avi-stt` carga la librería antes del primer uso de `ort`, una sola
vez y por ruta: la de `ORT_DYLIB_PATH` si está definida, que puede nombrar el
fichero de la librería o el directorio que la contiene (al que se añade el
nombre de la librería del sistema), o la que el bundle trae
en el directorio real del programa (`avi_shared::paths::program_dir()`, que
resuelve el symlink de `~/.local/bin`). No hay búsqueda de librerías del
sistema: si la librería falta o no carga, la operación falla con un error que
nombra la ruta buscada, y `doctor` lo informa en su comprobación
`onnx_runtime`. En el producto instalado basta con la librería junto al binario:
no hay `rpath` ni variables de entorno que fijar.

Desde el repositorio no hay librería junto a los ejecutables de `target/`, así
que `.cargo/config.toml` fija en `[env]` `ORT_DYLIB_PATH` al directorio
`ort-bundle/` de la raíz (que asegura `cargo xtask bootstrap`): las pruebas con
recursos y `cargo run` con STT la encuentran sin pasos manuales. Ese valor solo
rige en los procesos que lanza cargo, no se incrusta en el binario distribuido,
y una `ORT_DYLIB_PATH` exportada explícitamente lo sustituye. `cargo xtask
release` asegura `ort-bundle/` en la versión fijada y exporta la ruta de su
librería para su gate de pruebas con recursos, junto con
`AVI_ALLOW_CWD_VENDOR=1` para que los hijos CLI resuelvan el motor recién
compilado sin exigir un programa instalado.

La batería completa con `--include-ignored` se ejecuta con `-j2` y
`--test-threads=2` y sin otra carga en la máquina, por ejemplo
`cargo test --workspace --features full -j2 -- --include-ignored --test-threads=2`.
Las pruebas que arrancan un daemon real esperan su arranque con el mismo plazo
que el producto (10 s para publicar su dirección), y una CPU saturada por más
binarios de prueba en paralelo o por otra carga puede superarlo y producir
fallos que no son defectos.

#### Motor TTS desde el repo (opt-in del CWD)

Al ejecutar desde la raíz del repo (`cargo run`, pruebas sin instalar), el
motor `qwen_tts` y sus modelos viven en `vendor/qwen3-tts/`, que el producto
resuelve por la capa `<cwd>/vendor`. Esa capa está denegada por defecto: sin
`AVI_ALLOW_CWD_VENDOR=1`, el directorio de trabajo no aporta binario ni
modelos y la resolución avisa qué ignoró. El aviso sale **una sola vez por
proceso**, aunque la resolución se repita: la decisión se recalcula cada vez
—el entorno puede cambiar— y solo el mensaje deja de repetirse. Para
desarrollar desde el repo, define `AVI_ALLOW_CWD_VENDOR=1` (solo desarrollo
local; el producto instalado y CI no la necesitan).

#### Empaquetado de ONNX Runtime en los artefactos de release

El archivo de release trae ONNX Runtime: el usuario no lo instala. El bundle
lleva el asset oficial de Microsoft de su plataforma, en la versión `ort` de
[`packaging/pins.json`](../packaging/pins.json) (pareja del crate `ort`), con la
librería dinámica junto al binario y con el nombre por defecto que busca `ort`.
Lo descarga un único descargador, el de `cargo xtask package` (el mismo que usan
`cargo xtask bootstrap` y la gate de pruebas con recursos de `cargo xtask
release`), en los cuatro jobs. La descarga va por HTTPS (TLS ≥ 1.2) y el archivo
debe tener el SHA-256 que fija `pins.json` para su target (clave `ort_sha256`) y
traer la librería esperada con tamaño mayor que cero; si no, el empaquetado
falla. Los hashes son los `digest` que GitHub publica para cada asset del
release oficial de Microsoft, que no publica ficheros de checksum propios; al
subir `ort` hay que actualizarlos en el mismo cambio. `ort-bundle/` se reutiliza
solo si trae todas las librerías y el marcador `.ort-version` de la versión
fijada, que solo se escribe tras una descarga verificada.

Los instaladores extraen el archivo completo al directorio de instalación y
ejecutan el binario desde ahí, de modo que la librería llega junto al binario y
`ort` la encuentra sin cambios en los instaladores. La versión fijada no se
cambia sin revalidar el binding `ort` ↔ librería nativa.

### Verificación post-build

El **smoke test está automatizado en CI**. Cada uno de los 4 jobs de build,
antes de publicar el artefacto, ejecuta en este orden:

1. Sobre el binario recién compilado, la versión y `voice list`: en Linux y
   macOS compara `--version` con el tag; en Windows ejecuta `version` y exige a
   `version --json` los campos `name`, `version` (igual al tag) y
   `schema_version` igual a `CLI_SCHEMA_VERSION` (`cargo xtask pins --check`
   falla si el literal del humo y la constante divergen). `voice list` hace
   fallar el job si falta la voz `default`.
2. `cargo xtask build-engine --self-test`: compila el motor TTS y ejecuta su
   oráculo de kernels, sin pesos del modelo.
3. `cargo xtask package --expect-version`, que repite `version` y `voice list`
   sobre el binario, verifica el SHA-256 de ONNX Runtime si lo descarga, monta
   el bundle y audita sus dependencias contra el contrato del target (ver
   [Auditoría de dependencias](#auditoría-de-dependencias)).
4. El humo de instalación: `self install --no-setup --no-modify-path --yes` del
   archivo empaquetado en un sandbox con las cuatro raíces reubicadas, y
   `--version` del binario instalado. Los jobs permanentes comprueban además
   `speech denoise --help`, el rechazo del alias raíz `denoise` y que el FFmpeg
   instalado arranque (`-version` en Unix y Windows).
5. En Linux, la verificación de portabilidad del artefacto en salas limpias, de
   la que depende la publicación; ver [RELEASING.md](RELEASING.md#2-automático-lo-que-hace-el-ci).

La matriz temporal de cuatro targets se retiró tras transferir estos smokes a
los jobs permanentes. La última corrida de cuatro targets fue CircleCI #485
sobre `2e24b86f`; no acredita cambios posteriores ni ejecuta inferencia Denoise.

`version` y `voice list` **no cargan el modelo** (las voces de fábrica viajan
embebidas en el binario vía `crates/avi-store/assets/`), así que el chequeo es
de segundos.

Queda **manual** (requiere modelo, audio real y hardware por SO): `doctor`,
`setup` y una síntesis real (`speech synthesize`). La validación end-to-end de
los instaladores por SO es por diseño **externa al pipeline** (ver
`docs/GOAL.md` §Validación E2E).

---

## 4. CI/CD con CircleCI

La CI es un único pipeline de release (`build-all`), disparado solo por tags
`v*`. Por qué tiene esa forma y cómo se comportan sus cachés está en
[CI.md](CI.md); el flujo de publicación paso a paso, en
[RELEASING.md](RELEASING.md); la definición ejecutable, en
[`.circleci/config.yml`](../.circleci/config.yml).

---

## 5. Distribución de artefactos

El archivo de cada target lo monta de forma determinista `cargo xtask package`
(crates `tar` + `flate2` y `zip`, entradas ordenadas, marca temporal y permisos
fijos, sin herramientas de terceros), así que el mismo árbol produce el mismo
archivo en cualquier runner. Su layout es el de
[`packaging/bundle-manifest.json`](../packaging/bundle-manifest.json), la misma
lista que valida `self install`; los documentos MPL-2.0 viajan dentro del archivo
y quedan instalados junto al binario (disponibilidad de la fuente según §3.2
de la MPL-2.0 sin depender de un bundle).

Dónde está cada tema:

- Formato, nombres e instalación de los artefactos: [DISTRIBUTION.md](DISTRIBUTION.md).
- Publicación del GitHub Release: [RELEASING.md](RELEASING.md).
- Integración con el SO (`PATH`, symlink, desinstalación, `cleanup`):
  [LIFECYCLE.md](specs/LIFECYCLE.md#7-recibo-de-instalación-y-canales) y
  [LIFECYCLE.md §8](specs/LIFECYCLE.md#8-entorno-del-usuario).
- Artefactos sin firmar (SmartScreen y Gatekeeper):
  [SECURITY.md](../SECURITY.md#artefactos-sin-firmar).

---

## 6. Build nativo del motor TTS (Rust/qwen_tts)

> Esta sección documenta el toolchain C del motor Qwen3-TTS vigente.

### Interfaz uniforme: `xtask build-engine`

El motor se **compila desde fuente en CI en las 4 plataformas**; ningún
binario del motor se versiona. Los 4 jobs de build invocan una única interfaz:

```bash
cargo xtask build-engine --self-test
```

`build-engine` (`crates/xtask/src/main.rs`) oculta el mecanismo por plataforma:
en Unix invoca `make` con el entorno heredado; en Windows invoca `mingw32-make`
con el entorno MSYS2 UCRT64 augmentado (`PATH` con `ucrt64\bin` + `usr\bin` para
`cygpath`, `MSYSTEM=UCRT64`), leyendo la raíz de `MSYS2_ROOT` (default
`C:\msys64`). Pasa `SIMD=auto` por defecto (la política SIMD es autoridad única
del `Makefile`), compila con `make blas` y verifica con `--self-test` (oráculo de
kernels, sin pesos del modelo). El mismo comando sirve para CI, dev local y este
doc. En dev local sin MSYS2, Windows falla con un mensaje que guía a instalar
UCRT64 o definir `MSYS2_ROOT`.

### Toolchain y flags por plataforma

**Toolchain vigente (Windows):** MSYS2 UCRT64 con gcc, OpenBLAS y make
pineados en `packaging/pins.json` (claves `msys2_*`, espejo de los parámetros
del pipeline; comprueba tu host con `cargo xtask doctor`).
En CI se **aprovisiona pineado**: el job `build-windows-x64` extrae el release
base de MSYS2 y sincroniza las versiones pineadas, cacheando la instalación por
clave de versión (determinismo de release). El log del bootstrap captura
`pacman -Q` + `gcc --version` como evidencia de la **GCC Runtime Library
Exception** (libgfortran/libquadmath/libgcc estáticos en el `.exe`). Si la
versión instalada difiere del pin, el bootstrap registra `[WARN]` y continúa
usando la instalada como evidencia (`log on drift`, sin fallar de inmediato).

| Plataforma | `ARCH_FLAGS` | `CFLAGS_BASE` | `LDLIBS` / BLAS | Shims |
|---|---|---|---|---|
| Windows UCRT64 | `-mavx2 -mfma` (Haswell 2013+, `SIMD=auto`; `SIMD=scalar` vacía) | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `-static -L$(UCRT64_LIB) -lopenblas -lgomp -lws2_32 -lwinpthread -lm` (autocontenido) | `third_party/ingot/mingw_shim/unistd.h`, `sys/mman.h` vía `-include` |
| Linux x86_64 | `-mavx2 -mfma` | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `libopenblas.a -Wl,-Bstatic -lgfortran -lquadmath -Wl,-Bdynamic -lm -lpthread` (solo glibc dinámica) | — |
| Linux ARM64 | `-march=armv8-a` (portable; NEON baseline ARMv8-A. `SIMD=native` → `-march=native`) | idem | `libopenblas.a -Wl,-Bstatic -lgfortran -Wl,-Bdynamic -lm -lpthread` (solo glibc dinámica) | — |
| macOS (arm64) | `-mcpu=apple-m1` (portable; `SIMD=native` → `-march=native`) | idem; `MACOSX_DEPLOYMENT_TARGET` igual a `min_os` del manifiesto en CI | `-framework Accelerate` | — |

> **Baseline portable en ARM y macOS.** Las ramas ARM y Darwin del `Makefile`
> honran `SIMD` como x86: por defecto `-march=armv8-a` en Linux ARM (NEON
> garantizado en todo ARMv8-A) y `-mcpu=apple-m1` en macOS (M1, el chip más
> antiguo que soporta la versión mínima de macOS), no `-march=native`, que
> acoplaría el codegen a la microarquitectura del runner y
> haría `SIGILL` en CPUs más antiguas. `SIMD=native` recupera `-march=native`
> para compilar y ejecutar en la misma CPU.

> **OpenBLAS estático en Linux.** Debian y Ubuntu instalan `libopenblas.a`
> (paquete `libopenblas-dev`) en `/usr/lib/<multiarch>/openblas-pthread/`, fuera
> de la ruta del enlazador, y EL8 (la imagen `manylinux` de la CI, paquete
> `openblas-static`) llama `libopenblasp.a` a la variante con hilos, así que el
> `Makefile` la busca por ruta, con la variante con hilos primero (o la toma
> de `OPENBLAS_A=<ruta>`) y, si no la encuentra, el enlace del motor falla con
> el remedio en lugar de enlazar la dinámica. Su LAPACK está compilado con
> gfortran: `libgfortran.a` debe ser la de la versión mayor del `gcc` en uso
> (`libgfortran-<versión>-dev`), porque `gcc` solo la busca en su propio
> directorio. `cargo xtask doctor` comprueba ambas librerías estáticas.

### Caché del motor en CI

En CI el binario del motor (`vendor/qwen3-tts/qwen_tts[.exe]`) se cachea con una
clave derivada de `vendor/qwen3-tts/.engine-cachekey`, el agregado determinista
de `Makefile` + `*.c/*.h` (incluye `vendor/lz4.*`) + `third_party/ingot/**/*.{c,h}`
que genera el command `tts_engine_cachekey` (sin `-r` en `xargs` porque el de
macOS no admite ese flag). Si la clave exacta (sin fallback) existe tras
`restore_cache`, solo se verifica con `--self-test`; si no, se compila. El
bundle de ONNX Runtime (`ort-bundle/`) tiene su propia caché y se guarda tras
`cargo xtask package`, que es quien lo descarga y verifica. Su marcador
`ort-bundle/.ort-version` es lo que permite saber la versión: sin él es
desconocida, `cargo xtask doctor` marca `Mismatch` y `cargo xtask package`
vuelve a descargar. Las claves exactas están en `.circleci/config.yml`.

Ver `vendor/qwen3-tts/CLAUDE.md` y `crates/avi-tts/src/lib.rs` para el contrato
de invocación del motor.

---

## 7. Descarga nativa de modelos (`hf-hub`)

`setup` descarga los pesos de HuggingFace Hub de forma nativa vía el crate
**`hf-hub`** (rustls, sin OpenSSL: compila igual en los 4 targets) con validación
ETag/commit-hash del propio crate. La provisión muestra una barra por modelo con su
tamaño real y, al terminar cada uno, una línea con su tamaño, duración y velocidad.
`hf-hub` no reanuda por `Range`: una descarga interrumpida se repite completa. El crate está
vendorizado (ver `[patch.crates-io]` en [`Cargo.toml`](../Cargo.toml)): los punteros de
`snapshots/` son enlaces simbólicos en Unix y enlaces duros en Windows, por lo que el
blob no se duplica en disco; si el sistema de archivos no admite enlaces duros (FAT32,
exFAT o algunos recursos de red), se copia el blob y el disco se duplica. La descarga la
hace el propio binario.

Qué modelos se descargan, de qué repo y en qué revisión lo fija `MODEL_REVISIONS` en
[`crates/avi-shared/src/paths.rs`](../crates/avi-shared/src/paths.rs): un `&[ModelPin]`
con `name`, `repo`, `revision` y `approx_bytes` (tamaño medido de la descarga, del que sale
la cifra que anuncia `setup`). Actualizar una revisión es una acción deliberada y auditable,
y obliga a revisar también su tamaño. El comportamiento de `setup` ante cada modelo está en
[SETUP.md](CLI/commands/SETUP.md).

Los dos modelos de traducción son derivados int8 de Helsinki-NLP/opus-mt (CC-BY-4.0),
convertidos una sola vez a CTranslate2 y publicados en los repos `CristianRojaas/*-ct2-int8`.
Si una versión de `ct2rs`/CTranslate2 rompe la compatibilidad con ese formato, hay que
reconvertir con `ctranslate2` (con `--quantization int8 --copy_files source.spm target.spm`)
y publicar una revisión nueva.

### Parche de `hf-hub`

- **Qué cambia:** en la rama Windows de `create_pointer_symlink`
  (`vendor/hf-hub-*/src/cache/storage.rs`), el puntero de `snapshots/` se crea como enlace duro
  al blob en lugar de copiarlo. Si el sistema de archivos no admite enlaces
  duros, recurre a la copia, que es lo que hace el crate original. En Unix el
  puntero sigue siendo un symlink, y la ruta relativa que necesita se calcula
  solo en esa rama: el original la calcula para las dos y en Windows compila
  con un aviso de variable sin usar.
- **Por qué:** con la copia, cada archivo de modelo ocupaba el doble en disco
  (`blobs/` y `snapshots/`). Con el enlace duro, el disco ocupado iguala a la
  descarga y se conserva el layout de caché compartido con otras herramientas
  de Hugging Face.
- **Versión fijada:** la dependencia se declara con versión exacta (`=`), de modo
  que `[patch.crates-io]` no puede quedar sin efecto en silencio si el resolver
  eligiera otra versión.
- **Licencia y avisos:** la copia incluye el texto de Apache-2.0
  (`LICENSE-APACHE`) y marca con `// Patch:` cada comentario o código
  modificado en los archivos tocados (`vendor/hf-hub-*/src/cache/storage.rs`
  y `vendor/hf-hub-*/src/repository/download.rs`).
- **Cobertura:** la prueba `crates/avi-store/tests/hf_cache_layout.rs` descarga un
  archivo desde un servidor local y afirma que puntero y blob son el mismo
  archivo.

### Ubicaciones en disco por SO

La aplicación decide su caché (no depende del
fallback de `hf-hub`, que en Windows sin `HOME` caería en `<unidad>:\tmp`):
`models_cache_dir()` (`crates/avi-shared/src/paths.rs`) fija por defecto una raíz
**exclusiva de la aplicación** —la columna de modelos de la tabla siguiente—, y solo la
sustituye por la del usuario cuando este define `HF_HUB_CACHE` o, en su defecto,
`HF_HOME/hub`, que es lo que resuelve `shared_hf_root()`
(`crates/avi-shared/src/paths.rs`). `AVI_CACHE_DIR` designa el contenedor exclusivo de caché (modelos en
`$AVI_CACHE_DIR/models`, scratch en `$AVI_CACHE_DIR/tmp`) y tiene precedencia
sobre ambas, lo que permite aislar una prueba aunque el entorno tenga un
`HF_HOME` global. El cliente de
descarga se construye con `.cache_dir()` explícito, garantizando convergencia
lectura=escritura.
La provisión se decide solo por presencia del snapshot HF; no hay índice
`manifest.json` intermedio (los `manifest.json` de versiones previas que
queden en disco son inertes y los barre `cleanup`):

| SO | Modelos (`models_cache_dir()`) | Datos del usuario (`data_dir()`) |
|----|------------------------------|----------------------------------|
| Windows | `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` | `%LOCALAPPDATA%\ai-voice-interconnector\data` |
| Linux | `$XDG_CACHE_HOME/ai-voice-interconnector/models` (`~/.cache/ai-voice-interconnector/models`) | `~/.local/share/ai-voice-interconnector` |
| macOS | `~/Library/Caches/ai-voice-interconnector/models` | `~/Library/Application Support/ai-voice-interconnector` |

La raíz de modelos es **exclusiva de la aplicación** y por eso `cleanup --model` puede
borrarla como directorio entero; si el usuario define `HF_HUB_CACHE` o `HF_HOME`, esa raíz
pasa a ser **compartida** y el alcance se limita a lo atribuible a la aplicación (regla R3:
nunca `xet` ni el `.locks` completo).

`doctor` informa las dos raíces resueltas para auditoría: `models.root` (con
`models.shared_root`) y `install.data_dir` en `--json`; en texto, las imprime en las
líneas de detalle de cada chequeo. `cleanup --model/--voices/--synthetic-speech/--all` borra
selectivamente el estado del usuario (sin programa ni `PATH`; sin categoría sale con exit 2
`usage_error`); `self uninstall` es el único que añade programa y `PATH`.

---

## Limpieza del entorno de desarrollo

La limpieza de `target/`, `ort-bundle/` y demás artefactos del repositorio
(`cargo xtask clean`) está descrita en
[LIFECYCLE.md §9.6](specs/LIFECYCLE.md#96-cargo-xtask-clean); la higiene de
`target/` en el día a día, en [CONTRIBUTING.md](../CONTRIBUTING.md#higiene-de-target).
