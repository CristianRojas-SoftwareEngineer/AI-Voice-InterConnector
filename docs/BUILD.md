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
- [6. Descarga nativa de modelos (`hf-hub`)](#6-descarga-nativa-de-modelos-hf-hub)
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
manifiesto (ver `.circleci/config.yml`), variable que leen rustc, el crate `cc` y
CMake, y comprueba con `cargo xtask package` que ningún binario del bundle,
incluidos la librería de ONNX Runtime y el motor TTS, declare un `minos` mayor. El
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
salen del redist de la instalación de Visual Studio que compila, la que devuelve
`vswhere -latest -products * -requires
Microsoft.VisualStudio.Component.VC.Tools.x86.x64` (la misma consulta con la
que la CI importa `vcvars64`): la versión se lee de
`VC\Auxiliary\Build\Microsoft.VCRedistVersion.default.txt`, y las DLL, de las
carpetas `Microsoft.VC*.CRT` y `Microsoft.VC*.OpenMP` de
`VC\Redist\MSVC\<versión>\x64`. Cada ejecución las copia a `ort-bundle/`, pero
solo escribe las que difieren byte a byte; si falta una DLL o una de las
carpetas no es única, el empaquetado falla nombrando la ruta. El Universal CRT
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
  librería y, si es el caso, la función ausente, el `reason`
  `audio_backend_unavailable` y exit 3. El job `verify-portability` lo
  comprueba con `devices --json` en una imagen sin ALSA.
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
- El motor TTS (`qwen_tts`) es un binario ya compilado de su propio release:
  enlaza de forma estática su OpenBLAS (BSD-3-Clause, sin OpenMP ni Fortran) y
  los objetos de arranque de GCC y de glibc, así que no importa ninguna
  librería que el sistema base no traiga. Las variables de hilos de OpenBLAS
  (`OPENBLAS_NUM_THREADS`, `GOTO_NUM_THREADS` y `OMP_NUM_THREADS`)
  no se deben definir: el contrato del motor lo exige al cliente y la aplicación
  las retira del entorno del motor en todos sus lanzamientos.

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
declara un `minos` mayor que el mínimo de macOS. En Windows comprueba además la
compatibilidad del runtime de VC++, que Microsoft exige igual o más nuevo que la
toolchain de cada binario que lo importa: falla si un PE importa una DLL de
VC++ empaquetada cuya versión mayor.menor es inferior a la de su enlazador, o
si una DLL de VC++ del bundle no trae recurso de versión. Reporta todas las
violaciones juntas. `cargo xtask deps --check --dir <ruta>` aplica la misma auditoría a un
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
(helper compartido `avi_term::line::init_console`, que además activa las secuencias de color de la consola), así que los mensajes
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
bootstrap` deja además `ffmpeg-bundle/` con el FFmpeg fijado. `cargo xtask
release` asegura `ort-bundle/` en la versión fijada y exporta la ruta de su
librería para su gate de pruebas con recursos, junto con
`QWEN3_TTS_BIN` apuntando al motor de `engine-bundle/` para que los hijos CLI
lo resuelvan sin exigir un programa instalado.

Las pruebas que codifican, remuestrean o analizan medios resuelven `ffmpeg` por
`PATH` (`find_ffmpeg`), así que desde el repositorio hay que tener el binario
fijado y anteponerlo. `cargo xtask bootstrap` ya deja `ffmpeg-bundle/` con el
artefacto que [`packaging/pins.json`](../packaging/pins.json) fija para el
target del host, verificado con su SHA-256 y con el humo de versión y de los
seis codificadores de la matriz de egresión; para un `cargo test` manual basta
con anteponer ese directorio al `PATH`. La CI usa `cargo xtask ffmpeg --dest
<directorio>`, que hace la misma operación en el directorio indicado e imprime
la ruta del ejecutable resultante. Ambos caminos y `cargo xtask package` usan el
mismo descargador, así que local y CI no pueden ejercitar artefactos distintos. La gate de `cargo xtask release` y `cargo xtask resource-gate`, que la
ejecuta sin cortar versión, hacen esto por sí mismas: aseguran el FFmpeg fijado,
lo copian junto al binario de prueba de `target/debug` (la CLI solo lo busca
junto a su ejecutable) y anteponen su carpeta al `PATH`. El paso manual solo
hace falta para un `cargo test` lanzado a mano, y no basta para la prueba de
`speech denoise` de la CLI, que exige FFmpeg junto al binario de prueba: esa
prueba se ejecuta con `cargo xtask resource-gate`.

La batería completa con `--include-ignored` se ejecuta con `-j2` y
`--test-threads=2` y sin otra carga en la máquina, por ejemplo
`cargo test --workspace --features full -j2 -- --include-ignored --test-threads=2`.
Las pruebas que arrancan un daemon real esperan su arranque con el mismo plazo
que el producto (10 s para publicar su dirección), y una CPU saturada por más
binarios de prueba en paralelo o por otra carga puede superarlo y producir
fallos que no son defectos.

#### Motor TTS desde el repo

Al ejecutar desde la raíz del repo (`cargo run`, pruebas sin instalar), el
motor `qwen_tts` vive en `engine-bundle/`, que `cargo xtask bootstrap`
aprovisiona con el artefacto fijado en `packaging/pins.json`. La resolución lo
halla por la capa `engine-bundle/` bajo el directorio de trabajo, sin
`QWEN3_TTS_BIN`. Los pesos no viven junto al motor: los aprovisiona
`ai-voice-interconnector setup` en la raíz de modelos de AVI, que es la otra
capa de su resolución.

#### Empaquetado de ONNX Runtime en los artefactos de release

El archivo de release trae ONNX Runtime: el usuario no lo instala. El bundle
lleva el asset oficial de Microsoft de su plataforma, en la versión `ort` de
[`packaging/pins.json`](../packaging/pins.json) (pareja del crate `ort`), con la
librería dinámica junto al binario y con el nombre por defecto que busca `ort`.
Lo descarga un único descargador de `xtask`, que `cargo xtask package`,
`cargo xtask bootstrap` y la gate de pruebas con recursos de `cargo xtask
release` invocan desde la misma función que asegura todos los recursos fijados
(ONNX Runtime y FFmpeg), en los cuatro jobs. La descarga va por HTTPS (TLS ≥ 1.2) y el archivo
debe tener el SHA-256 que fija `pins.json` para su target (clave `ort_sha256`) y
traer la librería esperada con tamaño mayor que cero; si no, el empaquetado
falla. Los hashes son los `digest` que GitHub publica para cada asset del
release oficial de Microsoft, que no publica ficheros de checksum propios; al
subir `ort` hay que actualizarlos en el mismo cambio. `ort-bundle/` se reutiliza
solo si trae la librería de ONNX Runtime y el marcador `.ort-version` de la
versión fijada, que solo se escribe tras una descarga verificada. La frescura
solo mira ONNX Runtime: en Windows, las DLL de VC++ se actualizan en cada
ejecución desde el redist de la toolchain, también al reutilizar el bundle.

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
2. `cargo xtask package --expect-version`, que repite `version` y `voice list`
   sobre el binario, verifica el SHA-256 de ONNX Runtime y del motor TTS si los
   descarga, monta el bundle y audita sus dependencias contra el contrato del
   target (ver [Auditoría de dependencias](#auditoría-de-dependencias)).
3. `qwen_tts --self-test` sobre el motor descargado en `engine-bundle/`: su
   oráculo de kernels, sin pesos del modelo.
4. El humo de instalación: `self install --no-setup --no-modify-path --yes` del
   archivo empaquetado en un sandbox con las cuatro raíces reubicadas, y
   `--version` del binario instalado. Los cuatro builds comprueban además
   `speech denoise --help`, el rechazo del alias raíz `denoise` y que el FFmpeg
   instalado arranque (`-version` en Unix y Windows).
5. En Linux, la verificación de portabilidad del artefacto en salas limpias,
   que ejecuta también `qwen_tts --self-test` del motor instalado y de la que
   depende la publicación; ver [RELEASING.md](RELEASING.md#2-automático-lo-que-hace-el-ci).

Los cuatro jobs de prueba aprovisionan ese mismo FFmpeg antes de su suite, con
`cargo xtask ffmpeg`: el pipeline no declara versión, URL ni hash, así que no
puede divergir de `packaging/pins.json`, y `test-windows` ejecuta la suite sobre
el target MSVC que el paquete distribuye. `ffmpeg-bundle/` se cachea por triple
en los cuatro jobs de prueba y en los cuatro builds, con una clave que depende
de `packaging/pins.json` y, como toda clave de caché, de las rutas del job
(CircleCI restaura en la ruta absoluta guardada): se restaura antes de su
primer uso y se guarda después.
Lo restaurado solo se reutiliza si su marcador `.ffmpeg-sha256` coincide con el
SHA-256 fijado y el ejecutable pasa el humo de versión y de codificadores; si
no, `xtask` lo descarga y lo verifica de nuevo. Así se evita descargar y
descomprimir el artefacto en cada corrida, a cambio de que la descarga ya no
compruebe en cada release que la URL fijada sigue sirviéndolo: lo comprueba la
primera corrida tras cambiar `packaging/pins.json` o tras expirar la caché.

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

## 6. Descarga nativa de modelos (`hf-hub`)

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
- **Qué cambia (cliente sin token):** `HFClientBuilder` gana el método
  `without_token()` (`vendor/hf-hub-*/src/client.rs`); con él, `build` no resuelve
  ni acepta token y el cliente no envía la cabecera `Authorization` en ninguna
  petición (descargas, paginación ni Xet). Sin llamarlo, el comportamiento es el
  del crate original. La aplicación construye el cliente solo desde
  `avi_store::hub_client_builder`, que lo activa.
- **Por qué:** el original busca por su cuenta un token en `HF_TOKEN`,
  `HF_TOKEN_PATH` o `$HF_HOME/token` y lo envía siempre. Los modelos fijados son
  repositorios públicos: un token del entorno caducado rompía la descarga con un
  401, y uno válido escondía en la máquina del desarrollador un modelo que hubiera
  dejado de ser público. La única alternativa del original,
  `HF_HUB_DISABLE_IMPLICIT_TOKEN`, obliga a mutar el entorno global del proceso.
- **Versión fijada:** la dependencia se declara con versión exacta (`=`), de modo
  que `[patch.crates-io]` no puede quedar sin efecto en silencio si el resolver
  eligiera otra versión.
- **Qué cambia (identidad de los grupos de descarga):** `next_download_group_id`
  (`vendor/hf-hub-*/src/xet.rs`) asigna a cada grupo de descarga Xet una
  identidad única en el proceso, y `DownloadEvent::AggregateProgress`
  (`vendor/hf-hub-*/src/progress.rs`) gana el campo `group_id` con la del grupo
  que originó cada muestra. Sin él, las muestras de descargas simultáneas no se
  pueden distinguir y el progreso agregado se mezcla; la prueba
  `concurrent_download_groups_have_distinct_ids` fija que las identidades no se
  repiten.
- **Licencia y avisos:** la copia incluye el texto de Apache-2.0
  (`LICENSE-APACHE`) y marca con `// Patch:` cada comentario o código
  modificado en los archivos tocados (`vendor/hf-hub-*/src/cache/storage.rs`,
  `vendor/hf-hub-*/src/repository/download.rs`, `vendor/hf-hub-*/src/client.rs`,
  `vendor/hf-hub-*/src/xet.rs` y `vendor/hf-hub-*/src/progress.rs`).
- **Cobertura:** la prueba `crates/avi-store/tests/hf_cache_layout.rs` descarga un
  archivo desde un servidor local y afirma que puntero y blob son el mismo
  archivo. La prueba `crates/avi-store/tests/hub_client_anonymous.rs` descarga con
  `HF_TOKEN`, `HF_TOKEN_PATH` y un archivo `token` presentes y afirma que ninguna
  petición lleva `Authorization`.

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
