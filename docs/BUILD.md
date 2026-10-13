# Guía de construcción

`ai-voice-interconnector` se compila como un **binario Rust**
(`cargo build --release --features full`; CTranslate2 (ct2rs) compilado estático +
Parakeet vía ort load-dynamic (sin enlace en build)) y se **empaqueta por SO en un
archivo comprimido** (`tar.gz` en los 3 targets Unix, `.zip` en Windows) que
agrupa el binario con el motor TTS, ONNX Runtime y los documentos de licencia
GPLv3. El binario y el motor enlazan además librerías dinámicas que el archivo
no trae: ver [Dependencias de runtime por plataforma](#dependencias-de-runtime-por-plataforma).

## Tabla de contenidos

- [1. Requisitos](#1-requisitos)
- [2. Plataformas soportadas](#2-plataformas-soportadas)
  - [Matriz de arquitecturas y brechas](#matriz-de-arquitecturas-y-brechas)
  - [Matriz de SO probados y mínimos declarados](#matriz-de-so-probados-y-mínimos-declarados)
  - [Versión mínima de macOS](#versión-mínima-de-macos)
  - [Dependencias de runtime por plataforma](#dependencias-de-runtime-por-plataforma)
- [3. Compilación local](#3-compilación-local)
  - [Verificación rápida](#verificación-rápida)
  - [Build de distribución](#build-de-distribución)
  - [ONNX Runtime vía `load-dynamic` (sin build en compilación)](#onnx-runtime-vía-load-dynamic-sin-build-en-compilación)
  - [Verificación post-build](#verificación-post-build)
- [4. CI/CD con CircleCI](#4-cicd-con-circleci)
  - [Arquitectura del pipeline](#arquitectura-del-pipeline)
  - [Jobs](#jobs)
  - [Simetría: 3 puertas de test vs. 4 targets de build](#simetría-3-puertas-de-test-vs-4-targets-de-build)
  - [CD: publicación del GitHub Release (`publish-release`)](#cd-publicación-del-github-release-publish-release)
  - [Cacheo de dependencias y toolchain](#cacheo-de-dependencias-y-toolchain)
  - [Reproducibilidad: pines por digest y sus implicaciones](#reproducibilidad-pines-por-digest-y-sus-implicaciones)
  - [Guardarraíles durables de la suite de tests (`tests/cli_golden.rs`)](#guardarraíles-durables-de-la-suite-de-tests-testscli_goldenrs)
- [5. Distribución de artefactos](#5-distribución-de-artefactos)
  - [Empaquetado por plataforma (archivos comprimidos)](#empaquetado-por-plataforma-archivos-comprimidos)
  - [Integración con el SO](#integración-con-el-so)
  - [Limitación conocida: firma de código y notarización](#limitación-conocida-firma-de-código-y-notarización)
- [6. Build nativo del motor TTS (Rust/qwen_tts)](#6-build-nativo-del-motor-tts-rustqwen_tts)
  - [Interfaz uniforme: `xtask build-engine`](#interfaz-uniforme-xtask-build-engine)
  - [Toolchain y flags por plataforma](#toolchain-y-flags-por-plataforma)
  - [Caché del motor en CI](#caché-del-motor-en-ci)
- [7. Descarga nativa de modelos (`hf-hub`)](#7-descarga-nativa-de-modelos-hf-hub)
  - [Ubicaciones en disco por SO](#ubicaciones-en-disco-por-so)
- [8. Limpieza del entorno de desarrollo](#8-limpieza-del-entorno-de-desarrollo)

---

## 1. Requisitos

La tabla única de requisitos vive en el código y la comprueba
`cargo xtask doctor` (solo lectura): ejecuta ese comando en tu host en lugar
de contrastar esta guía.

El toolchain de Rust basta para compilar y empaquetar; no se requiere ningún otro.

---

## 2. Plataformas soportadas

| Plataforma | Compilación | Artefacto (archivo comprimido) |
|------------|-------------|-------------------------------|
| Windows x64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-x86_64-windows.zip` |
| Linux x64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-x86_64-linux.tar.gz` |
| Linux ARM64 | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-arm64-linux.tar.gz` |
| macOS arm64 (Apple Silicon) | `cargo build --release --features full` | `ai-voice-interconnector-<ver>-arm64-macos.tar.gz` |

> **Por qué Linux publica 2 arquitecturas y Windows/macOS solo 1.** Cada
> plataforma publica las arquitecturas que cumplen **a la vez** dos condiciones:
> (a) población real de usuarios y (b) capacidad del toolchain Rust. Bajo ese
> criterio:
>
> - **Windows → 1 (x86_64)** por **decisión**: Windows-on-ARM es marginal en la
>   población objetivo.
> - **macOS → 1 (arm64)** por **decisión**: el único runner de macOS disponible es
>   Apple Silicon (`macos: xcode 26.4.0` con `resource_class: m4pro.medium`), así que es
>   la única arquitectura que se compila, prueba y publica aquí. Publicar también Intel
>   exigiría un runner propio que no existe, y su población es residual junto a la de
>   Apple Silicon.
> - **Linux → 2 (x86_64 + arm64)** porque **ambas** arquitecturas tienen usuarios
>   y runners nativos (`arm.medium`).

### Matriz de arquitecturas y brechas

| SO | x86_64 | arm64 | Artefacto |
|----|:---:|:---:|----------|
| Windows | ✅ | ❌ | `.zip` (x64) |
| Linux | ✅ | ✅ | `tar.gz` x64 + `tar.gz` arm64 |
| macOS | ❌ | ✅ | `tar.gz` arm64 (Apple Silicon) |

**Arquitecturas faltantes y su justificación:**

- **Windows en ARM64:** no soportado por decisión de alcance (población marginal).
- **macOS Intel (x86_64):** no soportado; se acepta como limitación permanente.

### Matriz de SO probados y mínimos declarados

| SO | Probado en (CI) | Mínimo declarado | Origen del mínimo |
|----|-----------------|------------------|-------------------|
| Windows | Executor `circleci/windows@5.0` (Windows Server 2022) | Windows 10 1903+ x64 | Binario Rust sin APIs posteriores; el motor TTS incrusta un manifiesto `activeCodePage` UTF-8, que las versiones anteriores ignoran (el motor recibiría el texto y las rutas no ASCII en la página de códigos ANSI) |
| Linux | Imagen `manylinux_2_28` (AlmaLinux 8, glibc 2.28) para compilar; sala limpia en `almalinux:8` y `debian:stable-slim` | glibc ≥ 2.28, sin librerías aparte (ver [Dependencias de runtime](#dependencias-de-runtime-por-plataforma)) | Binario `gnu` enlazado contra la glibc 2.28 de la imagen de build; `xtask package` audita el techo de símbolos y el bootstrap lo diagnostica con la comprobación de arranque (`binary_incompatible`) |
| macOS | Runner Apple Silicon Xcode 26.4 | macOS 26+ (Tahoe), cualquier Mac con Apple Silicon (M1 en adelante) | `MACOSX_DEPLOYMENT_TARGET=26.0` en `build-darwin-arm64`; ver [Versión mínima de macOS](#versión-mínima-de-macos) |

**Limitación aceptada:** los mínimos declarados **no** se prueban en máquinas
con esas versiones exactas. Un reporte de fallo reabriría la decisión.

### Versión mínima de macOS

El artefacto de macOS exige **macOS 26 (Tahoe)** y corre en cualquier Mac con
Apple Silicon. El criterio es soportar solo versiones de macOS que reciben
actualizaciones de seguridad y que no son las siguientes en perderlas: mantener
un target que está por quedar obsoleto cuesta más de lo que aporta.

- **Soporte de Apple (octubre de 2026).** Apple publica parches de seguridad
  para la versión actual de macOS y las dos anteriores: macOS 27, 26 (Tahoe) y
  15 (Sequoia). macOS 14 (Sonoma) dejó de recibirlos en 2026, y macOS 15 es la
  siguiente en perderlos cuando salga la próxima versión mayor. macOS 26 es la
  más antigua que no está en esa situación.
- **Hardware.** macOS 26 corre en todos los Mac con Apple Silicon, desde los M1
  de 2020, así que exigirlo no deja fuera ningún Mac que el producto soporte
  (Mac Intel no es target).
- **Restricciones técnicas, todas por debajo de 26.0.** La
  `libonnxruntime.dylib` 1.28 que empaquetamos declara `minos 14.0`; el SDK de
  Xcode 26.4 admite desplegar desde macOS 11; el target `aarch64-apple-darwin`
  de Rust, desde macOS 11; el motor usa la API de LAPACK nueva de Accelerate
  (`ACCELERATE_NEW_LAPACK`), disponible desde macOS 13.3; CTranslate2 no fija
  mínimo propio.

El job `build-darwin-arm64` fija `MACOSX_DEPLOYMENT_TARGET=26.0`, que leen
rustc, el crate `cc`, CMake y el clang del Makefile del motor, y comprueba con
`cargo xtask package` que ningún binario del bundle, incluida la librería de
ONNX Runtime, declare un `minos` mayor que el `min_os` del manifiesto (26.0).
El motor se compila
con `-mcpu=apple-m1`: M1 es el chip más antiguo que soporta macOS 26, y su
código corre en todos los posteriores. El Cask de Homebrew declara
`depends_on macos: ">= :tahoe"`, y `install.sh` nombra esta versión mínima en
su diagnóstico `binary_incompatible`.

### Dependencias de runtime por plataforma

El archivo de cada target solo depende del **sistema base** de su plataforma:
no hay que instalar nada más. El contrato está declarado por target en
`packaging/bundle-manifest.json` y `cargo xtask package` lo hace cumplir antes
de comprimir (ver [Auditoría de dependencias](#auditoría-de-dependencias)):

| Target | Sistema base que puede usar el bundle |
|---|---|
| Windows x64 | DLL de Windows (`kernel32.dll`, `ntdll.dll`, el Universal CRT `api-ms-win-*` y las demás de la lista del manifiesto) y las que trae el propio archivo |
| Linux x64 y arm64 | glibc ≥ 2.28 (`libc`, `libm` y su `libmvec`, `libdl`, `librt`, `libpthread` y el cargador) más `libstdc++.so.6` y `libgcc_s.so.1`, que toda distribución con glibc trae; ningún símbolo por encima de `GLIBC_2.28` ni de `GLIBCXX_3.4.21` |
| macOS arm64 | `/usr/lib` (libSystem, libc++) y los frameworks de `/System`, con `minos` ≤ 26.0 |

En **Windows**, el binario principal y CTranslate2 enlazan el CRT estático
(`+crt-static`), el motor TTS se enlaza con `-static`, y el archivo trae las
cinco DLL del runtime de VC++ que no forman parte de Windows:
`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll` y `msvcp140_1.dll`,
que necesita ONNX Runtime, y `vcomp140.dll`, el runtime OpenMP de MSVC que el
binario importa porque CTranslate2 se compila con `/openmp`.

En **Linux**, el binario se compila contra glibc 2.28 (imagen `manylinux_2_28`)
y no declara más librerías que las del sistema base:

- **ALSA** se carga en tiempo de ejecución: la copia vendorizada de `alsa-sys`
  (`vendor/alsa-sys-0.4.0`) resuelve cada función con `dlopen`/`dlsym` sobre
  `libasound.so.2` en el primer uso. Antes de crear el host de audio se
  resuelven todas las funciones de ALSA que alcanza el binario
  (`vendor/alsa-sys-0.4.0/src/required_symbols.rs`). La lista contiene
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
  - la copia vendorizada de `ct2rs` (`vendor/ct2rs-0.10.0`), que compila
    CTranslate2 con el OpenMP de GNU, con `GOMPLIB_<target>`;
  - la copia vendorizada de `onednn-src` (`vendor/onednn-src-0.1.15`), con las
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
  oficial de Microsoft, usa la `libstdc++.so.6` del sistema, con
  `GLIBCXX_3.4.21` como máximo.
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
cargo test --all --verbose   # pruebas de contrato (≈70 tests en cli_golden); las de recursos locales salen como ignored
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

El motor STT Parakeet consume **ONNX Runtime** a través del crate
`ort =2.0.0-rc.13` en modo **`load-dynamic`** (`crates/avi-stt/Cargo.toml`): no
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
modelos y la resolución avisa qué ignoró. Para desarrollar desde el repo,
define `AVI_ALLOW_CWD_VENDOR=1` (solo desarrollo local; el producto instalado
y CI no la necesitan).

#### Empaquetado en los artefactos de release

El archivo de release trae ONNX Runtime: el usuario no lo instala. El bundle
lleva el asset oficial de Microsoft de **ONNX Runtime 1.28.0** (pareja de `ort`
rc.13) de su plataforma, con la librería dinámica junto al binario y con el nombre
por defecto que busca `ort`. Lo descarga un único descargador, el de
`cargo xtask package` (el mismo que usan `cargo xtask bootstrap` y la gate de
pruebas con recursos de `cargo xtask release`), en los cuatro jobs. La descarga
va por HTTPS (TLS ≥ 1.2) y el archivo debe tener el SHA-256 que fija
`packaging/pins.json` para su target (clave `ort_sha256`) y traer la librería
esperada con tamaño mayor que cero; si no, el empaquetado falla. Los hashes son
los `digest` que GitHub publica para cada asset del release oficial de
Microsoft, que no publica ficheros de checksum propios; al subir `ort_pin` hay
que actualizarlos en el mismo cambio. `ort-bundle/` se reutiliza solo si trae
todas las librerías y el marcador `.ort-version` de la versión fijada, que solo
se escribe tras una descarga verificada; cada `build-*` lo conserva entre
corridas con su caché `ort-v4`.

| Job | Asset MS v1.28.0 | Librería empaquetada |
|---|---|---|
| build-windows-x64 | `onnxruntime-win-x64-1.28.0.zip` | `onnxruntime.dll` |
| build-linux-x64 | `onnxruntime-linux-x64-1.28.0.tgz` | `libonnxruntime.so` |
| build-linux-arm64 | `onnxruntime-linux-aarch64-1.28.0.tgz` | `libonnxruntime.so` |
| build-darwin-arm64 | `onnxruntime-osx-arm64-1.28.0.tgz` | `libonnxruntime.dylib` |

En **Windows**, la `onnxruntime.dll` de Microsoft es `/MD` y depende del runtime
de VC++ (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`,
`msvcp140_1.dll`), y el binario principal importa `vcomp140.dll`, el runtime
OpenMP de MSVC que usa CTranslate2. Para no exigir al usuario instalar el
*Visual C++ Redistributable*, esas cinco
DLLs se empaquetan junto al binario (*app-local deployment*, permitido por la
licencia de redistribución de MS): `cargo xtask package` las copia de System32
del runner. El Universal CRT (`ucrtbase.dll`) ya forma parte de Windows 10/11.
En Linux/macOS la librería depende del runtime C++ del SO
(`libstdc++` / `libc++`), que forma parte del sistema base (ver
[dependencias de runtime](#dependencias-de-runtime-por-plataforma)).

Los instaladores extraen el archivo completo al directorio de instalación y
ejecutan el binario desde ahí, de modo que la librería llega junto al binario y
`ort` la encuentra sin cambios en los instaladores. La versión
1.28.0 no se cambia sin revalidar el binding `ort` ↔ librería nativa.

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
   `--version` del binario instalado.
5. En Linux, la verificación de portabilidad (`verify-portability`): el artefacto, sin compilar
   nada, en `almalinux:8` y `debian:stable-slim` (sin paquetes de desarrollo,
   ALSA, `libgomp` ni `procps`): `--version`, `voice list`, `self install` en
   un sandbox, `--version` por el symlink instalado y la comprobación
   `onnx_runtime` de `doctor`. `publish-release` depende de ella.

`version` y `voice list` **no cargan el modelo** (las voces de fábrica viajan
embebidas en el binario vía `crates/avi-store/assets/`), así que el chequeo es
de segundos.

Queda **manual** (requiere modelo, audio real y hardware por SO): `doctor`,
`setup` y una síntesis real (`speech synthesize`). La validación end-to-end de
los instaladores por SO es por diseño **externa al pipeline** (ver
`docs/GOAL.md` §Validación E2E).

---

## 4. CI/CD con CircleCI

La CI es **un solo pipeline de release** (`build-all`), disparado **solo en
tags `v*`** (todos sus jobs con `filters.tags: /^v.*/` y `branches: ignore /.*/`).
La **triple puerta simétrica** `test-linux` + `test-windows` + `test-macos`
(cada uno `cargo nextest run --all` en su SO nativo, con `cargo-nextest`
pineado) es `requires:` de los 4 builds, de
modo que la publicación queda **mecánicamente condicionada** al resultado
completo de la suite sobre el commit taggeado, dentro de la **misma** pipeline.

- **Por qué un pipeline y no dos.** La publicación (`publish-release`) es
  irreversible, así que el release debe estar condicionado a los tests por una
  dependencia real, no por confianza. CircleCI **no encadena pipelines
  distintas**: un job solo puede depender de otro dentro del mismo workflow. La
  única forma de que los tests **bloqueen** el release es tenerlos en el grafo
  de `requires:` de los builds — es decir, en la misma pipeline. Por eso la
  triple puerta vive aquí y no en un workflow de rama separado.

- **Por qué se descartó el pipeline de rama `validate`.** Como el corte empuja
  commit+tag juntos (`git push origin main --tags`), un workflow `validate` en
  `main` disparaba una **segunda** pipeline que re-ejecutaba la **misma** triple
  suite sobre el **mismo** commit, en paralelo con `build-all`. Eso no adelantaba
  la señal (corría en paralelo, no antes) ni aportaba cobertura única: era
  duplicación pura. Al mover la triple puerta al pipeline de release, los tests
  corren **una vez** por release y bloquean de verdad la publicación.

- **Gates completos.** Además de la triple puerta: `coverage` (cargo-llvm-cov,
  **solo aquí**), las tres suites de bootstrap (`test-bootstrap-linux`
  bats, `test-bootstrap-windows` Pester, `test-bootstrap-macos` bats, en
  `tests/bootstrap/`; bats sirve los assets con `openssl s_server` por HTTPS
  y Pester con `support/Serve.ps1` por HTTP) y los gates
  `validate-licenses` (SOURCE-OFFER/THIRD-PARTY) y `validate-changelog`. Esos
  **9 gates** son `requires:` de los 4 builds nativos, que compilan las 4
  plataformas en modo release (validación de compilación por plataforma).
  Los runners bats están pineados: bats-core se instala desde el tag de git
  según el parámetro `bats_pin` (1.14.0, verificado con `bats --version`),
  la misma versión en Linux y macOS, y `test-bootstrap-linux` corre sobre
  `cimg/base` pineada por digest. `test-bootstrap-windows` instala Pester según el parámetro `pester_pin` (5.8.0)
  desde PSGallery con `$ErrorActionPreference = "Stop"`, de modo que un fallo de
  instalación o de importación hace fallar el paso. Además instala PowerShell 7
  según el parámetro `pwsh_pin` (7.6.6, zip del release oficial verificado con
  su versión) para ejecutar `irm | iex` en 5.1 y 7.

**Feedback pre-release.** El repo es trunk-based sobre `main` (flujo de un solo
desarrollador, sin ramas de larga vida); los commits de rama **no** disparan CI
(`build-all` es tags-only, `branches: ignore: /.*/`). La red de seguridad es el
gate al taggear: si la triple suite falla, los builds no corren y **nada se
publica** (fix-forward/revert y re-tag). No hay estado de `main` sin tag que
publique, así que no se necesita *branch protection* airtight para sostener la
garantía "commit taggeado probado".

### Arquitectura del pipeline

#### Pipeline de release `build-all`

Único pipeline de release; solo se dispara con tags `v*`:

```
┌────────────────────┐  ┌────────────────────┐  ┌────────────────────┐
│     test-linux     │  │    test-windows    │  │     test-macos     │
│ (cargo nextest)    │  │ (cargo nextest)    │  │ (cargo nextest)    │
└─────────┬──────────┘  └─────────┬──────────┘  └─────────┬──────────┘
          │        (triple puerta = gate mecánico)         │
          └──────────────────────┬────────────────────────┘
        ┌───────────────┬────────┴──────┬───────────────┬──────────────┐
        │   coverage    │ validate-     │ validate-     │ test-bootstrap-* (×3)  │
        │(cargo llvm-cov)│ licenses     │ changelog     │ (bats/Pester)          │
        └───────┬───────┴──────┬────────┴──────┬────────┴──────┬────────────────┘
                └──────────────┴───────┬───────┴───────────────┘
                          (9 gates = requires: de cada build)
         ┌───────────────┬─────────────┴─┬───────────────┐
         ▼               ▼               ▼               ▼
┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌──────────────────┐
│build-windows│ │build-linux- │ │build-linux- │ │ build-darwin-    │
│ -x64        │ │    x64      │ │   arm64     │ │     arm64        │
└─────────────┘ └─────────────┘ └─────────────┘ └──────────────────┘
     (cada build: versión + `voice list`, `build-engine --self-test`, `cargo xtask package --expect-version` y humo de `self install` en sandbox + `--version`)
                              │
                              ▼   publish-release → publish-metadata
```

Cada `build-*` compila con `cargo build --timings` y publica el desglose por crate como artefacto `cargo-timings` (`target/cargo-timings`).

#### Workflow de sonda `native-cache-probe`

Herramienta de diagnóstico que nunca publica. Mide los `build-*` en los executors reales sin cortar una release. Solo se activa por API, con el parámetro de pipeline `native_cache_probe`:

```bash
curl -X POST https://circleci.com/api/v2/project/gh/<org>/<repo>/pipeline \
  -H "Circle-Token: $CIRCLE_TOKEN" -H "Content-Type: application/json" \
  -d '{"branch":"main","parameters":{"native_cache_probe":true}}'
```

Ejecuta los 4 `build-*` con `probe: true`, sin `requires` ni filtros de tags, y excluye `build-all` mientras el parámetro está activo. En modo sonda cada `build-*`:

- no restaura ni guarda `target-*`: compila siempre con `target/` frío y nunca escribe una clave inmutable de producción;
- omite el staging versionado, el SHA-256 y `persist_to_workspace` (exigen `CIRCLE_TAG`);
- ejecuta el smoke test y guarda `sccache` en la variante `full` (por contenido, sin riesgo de contaminar la producción, y así siembra la caché para la siguiente release);
- imprime el diagnóstico de CMake: versiones de `cmake`/`ninja`, número de CPU y, por cada `CMakeCache.txt`, generador, compilador, launcher, tipo de build, runtime de MSVC y flags efectivos (en Windows, además, una línea de compilación real de `onednn-src` y de `ct2rs`);
- arranca el servidor `sccache` con `SCCACHE_ERROR_LOG` y `SCCACHE_LOG=debug` y publica ese log como artefacto `sccache-error-log`: identifica las unidades que `sccache` no puede compilar o cachear con `target/` frío, cuya salida cargo oculta dentro de los build scripts. Los builds de release no activan el log.

Los tests de topología de `xtask` fallan si el workflow de sonda llega a contener un `publish-*`, si el modo sonda toca `target-*` o si `build-all` puede correr junto a la sonda.

### Jobs

| Job | Pipeline | Plataforma | Executor | Notas |
|-----|----------|------------|----------|-------|
| `test-linux` | `build-all` (gate) | Linux x64 | docker `cimg/rust:1.96.0` | `cargo nextest run --all` (+ `cargo build` dev para la comprobación de ALSA) |
| `test-windows` | `build-all` (gate) | Windows x64 | `win/server-2022` | `cargo nextest run --all` en Windows nativo |
| `test-macos` | `build-all` (gate) | macOS arm64 | macos `m4pro.medium` | `cargo nextest run --all` en macOS nativo |
| `coverage` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo llvm-cov --workspace --lcov` genera `lcov.info` (artefacto `coverage-lcov`) y `cargo llvm-cov report` imprime el resumen sin re-ejecutar la suite; sin umbral de % |
| `validate-licenses` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo xtask source-offer --check` + `licenses --check` |
| `validate-changelog` | `build-all` | Linux x64 | docker `cimg/rust:1.96.0` | `cargo xtask changelog --check` |
| `test-bootstrap-*` | `build-all` | por SO | bats/Pester | Suites de los bootstrap (`tests/bootstrap/`; bats contra `openssl s_server`, Pester contra `Serve.ps1`) |
| `build-windows-x64` | `build-all` | Windows x64 | `win/server-2022` | `cargo build --release --features full` + staging `.zip` |
| `build-linux-x64` | `build-all` | Linux x64 | docker `manylinux_2_28_x86_64` (`large`) | `cargo build --release --features full` + staging `tar.gz` |
| `build-linux-arm64` | `build-all` | Linux ARM64 | docker `manylinux_2_28_aarch64` (`arm.medium`) | idem, nativo aarch64 |
| `verify-portability` | `build-all` | Linux x64 y ARM64 | docker `almalinux:8` y `debian:stable-slim` | Verificación de portabilidad del artefacto Linux (4 instancias con nombre corto); `publish-release` depende de ella |
| `build-darwin-arm64` | `build-all` | macOS arm64 | macos `m4pro.medium` | idem, Xcode 26.4 |
| `publish-release` | `build-all` (CD) | Linux x64 | docker `cimg/base:2026.09` (pineada por digest) | Solo en tags `v*`: estampa los bootstrap, genera `SHA256SUMS.txt` sobre los 6 ficheros y publica el GitHub Release con 7 assets |
| `publish-metadata` | `build-all` (CD) | Linux x64 | docker `cimg/rust:1.96.0` | Solo en tags `v*`: renderiza Cask con `cargo xtask cask` y empuja al tap |

### Simetría: 3 puertas de test vs. 4 targets de build

Las 3 puertas de test y los 4 builds (ambos en `build-all`) responden a **ejes
distintos**.

- **Por qué 3 puertas de test y 4 builds.** Los tests son **por familia de SO**:
  validan lógica Rust por SO (Windows: lanzamiento con lista explícita de
  handles, limpiador diferido, `PATH` en HKCU y WASAPI; macOS: CoreAudio y
  cuarentena de Gatekeeper; Linux y macOS: `setsid`, señales e identidad de
  proceso por `/proc` y `proc_pidinfo`; Linux: ALSA). Los builds son **por target de
  distribución**, y Linux publica **dos**
  arquitecturas. Son dos ejes ortogonales (SO × build-target).

- **Por qué el runner de `test-linux` es x86_64.** Es el executor Docker más
  barato/rápido. La suite es arch-independiente y mockea el engine, así que
  basta la arquitectura más barata.

- **Hueco de cobertura de ARM64 (divergencia aceptada).** `build-linux-arm64`
  está *gated* por tests en x86_64; en ARM solo corre el smoke test de
  [Verificación post-build](#verificación-post-build): arrancar el binario
  (`--version`, que exige resolver sus librerías dinámicas), `voice list`, el
  `--self-test` de los kernels del motor y el humo de `self install`. Ni la
  traducción (CTranslate2) ni la transcripción (ONNX Runtime) se ejecutan en
  ARM dentro del pipeline.

### CD: publicación del GitHub Release (`publish-release`)

Al pushear un tag `v*`, además de tests + builds corre `publish-release`
(estrategia GitHub Releases). Recolecta los 4 artefactos **versionados** por
`persist_to_workspace`/`attach_workspace`, estampa la versión del tag en ambos
bootstrap (una sustitución verificada por fichero), genera `SHA256SUMS.txt`
sobre los 6 ficheros, extrae las
notas de la sección `[X.Y.Z]` de `CHANGELOG.md` (fail-fast si no existe) y
publica el GitHub Release directo (sin borrador) con 7 assets.

### Cacheo de dependencias y toolchain

#### Qué se restaura y qué se recompila

En CI intervienen dos mecanismos independientes. Distinguirlos es lo que explica
por qué un cambio de código siempre llega al binario publicado y, aun así, el
pipeline puede tardar pocos minutos.

1. **Qué restaura CircleCI.** Cada caché se restaura por una clave. Ninguna
   clave depende del código fuente del workspace: dependen de las dependencias
   de `Cargo.lock`, de la versión de Rust, del SO, de la variante de
   compilación y de los pines de herramientas. Editar `src/` o `crates/*` no
   cambia ninguna clave, así que se restaura el mismo `target/` y la misma
   caché de `sccache`.
2. **Qué recompila cargo dentro de lo restaurado.** Cargo decide unidad por
   unidad con su *fingerprint*:
   - **Dependencias de crates.io:** se identifican por versión y checksum. Si no
     cambiaron, quedan `Fresh` y no se recompilan. Esto incluye la cadena
     nativa C++ (CTranslate2, oneDNN, SentencePiece, aws-lc), la parte más cara.
   - **Crates del workspace** (`src/`, `crates/*`): se identifican por la fecha
     de modificación (`mtime`) de sus archivos. El checkout de CI les da la
     fecha actual, posterior a la de `target/`, así que quedan `Dirty` y se
     recompilan en cada corrida, se hayan editado o no.
   - **`sccache`** atiende esas recompilaciones por hash de contenido: si el
     archivo y los flags no cambiaron devuelve el objeto guardado en
     milisegundos; si cambiaron, compila de verdad. Los crates `bin` (el
     binario final, `xtask` y los build scripts) no son cacheables por
     `sccache` y siempre pasan por `rustc`.

En consecuencia, un cambio en el código fuente no invalida ninguna caché y
siempre se compila: su costo es solo el de recompilar los crates afectados. Lo
que alarga el pipeline es un cambio de clave de la caché de `target/` (dependencias reales
o el parche de CMake), porque entonces `target/` parte vacío y las dependencias
externas vuelven a compilarse, con la ayuda de `sccache`. La tabla de
[escenarios](#escenarios-de-invalidación-y-su-costo) resume cada caso.

#### Familias de caché

| Caché | Qué guarda | Jobs | Clave (se invalida cuando cambia…) |
|-------|------------|------|------------------------------------|
| `cargo-v2` (registry) | `~/.cargo/registry` y `~/.cargo/git`: las fuentes descargadas de crates | todos los que compilan | `arch`, `rust_pin` y el checksum de `Cargo.lock.cachekey`. Tiene fallback por prefijo. |
| `target-v5` | `target/` completo: dependencias compiladas y los `OUT_DIR` de los proyectos CMake | `test-linux`, `test-windows`, `test-macos` (`variant: test`), `coverage` (`cov`) y los 4 `build-*` (`full`) | `arch`, `os`, `rust_pin`, `variant`, el checksum de `Cargo.lock.cachekey` y el tree hash git de cada crate de `vendor/*/Cargo.toml`. Sin fallback. |
| `sccache-v1` | objetos Rust, C y C++ indexados por contenido, con tamaño acotado por el parámetro `sccache_cache_size` (`3G`, que `sccache` lee como 3 GiB, unos 3,2 GB) | los mismos jobs que la caché de `target/`; `validate-licenses`, `validate-changelog` y `publish-metadata` solo la restauran | `arch`, `os`, `rust_pin` y `variant`, más `{{ epoch }}` al guardar: se restaura la entrada más reciente del prefijo. |
| `toolchain-v1` | `~/.rustup` y `~/.cargo/bin`: el Rust instalado con `rustup` y las herramientas instaladas con `cargo install` | `test-windows`, `test-macos`, `build-windows-x64` y `build-darwin-arm64`, que instalan Rust; `coverage`, cuya imagen Docker ya trae Rust, la usa para conservar `llvm-tools-preview` y `cargo-llvm-cov` | `arch`, `os` y `rust_pin`. |
| `msys2-v1` | la instalación de MSYS2 | `build-windows-x64` | los pines de MSYS2: release base, gcc, openblas y make. |
| `tts-v1` | el motor `qwen_tts[.exe]` compilado | los 4 `build-*` | `vendor/qwen3-tts/.engine-cachekey` (`Makefile`, `*.c`/`*.h` y `third_party/ingot`); más los pines de gcc y openblas en Windows y `os`+`arch` en Unix. Sin fallback. |
| `ort-v4` | `ort-bundle/` tal como lo deja `cargo xtask package`: la librería oficial de ONNX Runtime (con su SHA-256 verificado), las cuatro DLL del runtime de VC++ en Windows y el marcador `.ort-version` | los 4 `build-*` | `ort_pin` y triple de target (`ort-v4-<triple>-<pin>`). |

`variant` separa los conjuntos de features (`test` sin features, `cov`
instrumentado, `full` con C++), y `os` separa los targets: sin esos
componentes, jobs distintos compartirían una entrada incompatible.

#### Clave normalizada de `Cargo.lock`

Las claves de `cargo-v2` y de la caché de `target/` no usan `Cargo.lock` directamente sino
`Cargo.lock.cachekey`, que genera el command `cargo_lock_cachekey`, primer paso
de `cargo_restore_caches` y de `cargo_restore_registry` (los jobs que solo
restauran `cargo-v2`). Lo hace con `perl` (en `shell: bash`, disponible en los
tres executors), reemplazando la versión del propio crate
(`ai-voice-interconnector`) por `0.0.0`. Cada release cambia esa versión; sin la
normalización, la clave cambiaría en cada corte aunque las dependencias fueran
las mismas, y la caché de `target/`, que no tiene fallback, partiría siempre en
frío. Por eso el paso falla si la sustitución no ocurre exactamente una vez: un
`Cargo.lock` con otro formato no puede degradar la caché en silencio. Se usa un
transform de texto en lugar de `xtask` para no compilar `xtask` y su árbol de
dependencias antes de restaurar la caché.

Esa es la invariante de orden de los 11 jobs que usan cargo: ningún paso
compila antes de restaurar las cachés. Una restauración extraída sobre un
`target/` ya poblado deja el estado de dos ejecuciones, y el enlazado puede
reutilizar objetos que esperan símbolos que el código actual ya no define. Las
puertas `validate_pins` y `validate_features` son la primera compilación del
workspace, juntas y tras activar `sccache`, y compilan `xtask` ya con el
registry y las cachés del job. La vigila
`test_cargo_jobs_compile_only_after_cache_restore`.

#### Parches vendorizados en la clave de `target/`

El `Cargo.toml` raíz reemplaza cinco crates por copias parcheadas locales
(`[patch.crates-io]`): `cmake` (`vendor/cmake-0.1.58`), `hf-hub`
(`vendor/hf-hub-1.0.0`), `alsa-sys` (`vendor/alsa-sys-0.4.0`), `ct2rs`
(`vendor/ct2rs-0.10.0`) y `onednn-src` (`vendor/onednn-src-0.1.15`). Al ser
paquetes locales, cargo los identifica por
`mtime`, y el `mtime` nuevo del checkout los marcaría `Dirty` en cada corrida.
Para `cmake` eso arrastraría la recompilación de toda la cadena nativa que
depende de él (`aws-lc-sys`, `onednn-src`, `sentencepiece-sys`, `ct2rs`), y para
`ct2rs` y `onednn-src` la de CTranslate2 y oneDNN, aunque su contenido no
hubiera cambiado.

Para evitarlo, `cargo_restore_caches` toma como parches los directorios que
cumplen el glob `vendor/*/Cargo.toml`, en orden `LC_ALL=C` (sin analizar TOML:
un parche nuevo entra en la clave sin tocar el pipeline), escribe en
`.vendor-patches.tree` el tree hash git
de cada uno (`git rev-parse HEAD:<directorio>`, una línea por parche) y lo
incorpora a la clave de `target/`. Como la clave no tiene fallback, un acierto
garantiza que `target/` se compiló con el mismo contenido de los parches, y es
seguro fijarles un `mtime` antiguo (`touch -t 200001010000`) para que cargo los
vea `Fresh`. Si un parche cambia, cambia la clave y `target/` parte vacío. El
paso aborta si `vendor/` no contiene ningún crate o si alguna línea no tiene 40
caracteres hexadecimales, para no cachear nunca bajo una clave degenerada. Las
pruebas de `xtask` exigen que ese conjunto coincida con los paquetes de ruta
ajenos al workspace que resuelve `cargo metadata --all-features` (detectan
parches sin usar, parches fuera de `vendor/` y crates vendorizados sin
`[patch]`) y que ninguna regla de ignorado alcanza sus ficheros, que se
compilan enteros desde ahí. Se usa git y no `find | sort` porque en
el executor de Windows `sort` resuelve al `sort.exe` de System32, y el tree hash
es además inmune a la conversión de fin de línea del checkout.

##### Parche de `hf-hub`

- **Qué cambia:** en la rama Windows de `create_pointer_symlink`
  (`vendor/hf-hub-1.0.0/src/cache/storage.rs`), el puntero de `snapshots/` se crea como enlace duro
  al blob en lugar de copiarlo. Si el sistema de archivos no admite enlaces
  duros (FAT32, exFAT o algunos recursos de red), recurre a la copia, que es lo
  que hace el crate original. En Unix el puntero sigue siendo un symlink, y la
  ruta relativa que necesita se calcula solo en esa rama: el original la
  calcula para las dos y en Windows compila con un aviso de variable sin usar.
- **Por qué:** con la copia, cada archivo de modelo ocupaba el doble en disco
  (`blobs/` y `snapshots/`). Con el enlace duro, el disco ocupado iguala a la
  descarga y se conserva el layout de caché compartido con otras herramientas
  de Hugging Face.
- **Versión fijada:** la dependencia se declara como `=1.0.0`, de modo que
  `[patch.crates-io]` no puede quedar sin efecto en silencio si el resolver
  eligiera otra versión.
- **Licencia y avisos:** la copia incluye el texto de Apache-2.0
  (`LICENSE-APACHE`) y marca con `// Patch:` cada comentario o código
  modificado en los archivos tocados (`vendor/hf-hub-1.0.0/src/cache/storage.rs`
  y `vendor/hf-hub-1.0.0/src/repository/download.rs`).
- **Cobertura:** la prueba `crates/avi-store/tests/hf_cache_layout.rs` descarga un
  archivo desde un servidor local y afirma que puntero y blob son el mismo
  archivo.

#### Guardado

- **La caché de `target/` es inmutable:** `save_cache` solo escribe si la clave no existe,
  así que la entrada refleja la primera corrida que la creó. Los 4 `build-*` la
  guardan con `when: on_success`, para no persistir un `target/` a medio
  compilar bajo una clave que ya no se puede reescribir. Los jobs de test y
  `coverage` la guardan con `when: always`, porque ahí un fallo suele ser un
  test y no una compilación incompleta.
- **`sccache-v1` es rolling:** cada job pesado guarda una entrada nueva en cada
  corrida (`when: always`), y la restauración toma la más reciente. No hay
  guardado condicional: los fallos de una corrida son objetos que `sccache`
  acaba de compilar, y omitir el guardado haría que la siguiente corrida los
  volviera a fallar. Cada cambio de `Cargo.lock` agrega objetos que ya nadie
  usa, así que el tamaño se acota con `SCCACHE_CACHE_SIZE`, fijado por el
  parámetro de pipeline `sccache_cache_size` (`3G`: 3 GiB, unos 3,2 GB, más del doble del working
  set de `build-windows-x64`, la variante más grande). Al superarlo, `sccache`
  desaloja por LRU: un acierto actualiza el `mtime` de la entrada, también en
  una caché recién restaurada desde el tar de CircleCI, así que se desalojan los
  objetos sin uso y no los vigentes (comprobado con `sccache` 0.8.2).

#### Cobertura de `sccache`

`sccache` envuelve tres tipos de compilación: `rustc` (`RUSTC_WRAPPER=sccache`); los nativos que compila el crate `cc` (`aws-lc-sys`, `ring`, `blake3`…), que detecta `RUSTC_WRAPPER` y usa `sccache` como wrapper; y, en los 4 `build-*`, los proyectos CMake (`ct2rs`/CTranslate2, `onednn-src`, `sentencepiece-sys`), vía `CMAKE_C_COMPILER_LAUNCHER`/`CMAKE_CXX_COMPILER_LAUNCHER=sccache`. CMake ≥ 3.17 lee esas variables del entorno al configurar, así que no se toca ningún `build.rs`. En Linux y macOS el comando `native_sccache_setup_unix` las exporta a `$BASH_ENV`. En Windows hacen falta tres piezas más, todas en el paso de compilación de `build-windows-x64`:

- **Ninja como generador** (`CMAKE_GENERATOR=Ninja`): el generador Visual Studio, el que el crate `cmake` elige por defecto con MSVC, ignora los launchers. `native_sccache_setup_windows` descarga en cada corrida la release oficial fijada por el parámetro `ninja_pin` a un directorio temporal fuera de toda caché, y verifica su versión.
- **Entorno de MSVC**: Ninja no carga `INCLUDE`/`LIB` por sí mismo (el generador Visual Studio sí), así que el paso localiza Visual Studio con `vswhere`, importa `vcvars64.bat` y solo después antepone Ninja y `.cargo\bin` al `PATH`.
- **`CC`/`CXX` con la ruta absoluta de `cl.exe`**: el crate `cc` solo usa `RUSTC_WRAPPER` como wrapper en MSVC cuando el compilador es la ruta de un ejecutable existente.

`CARGO_INCREMENTAL=0` está en el `environment:` de cada uno de los 11 jobs que usan cargo, porque `sccache` no cachea compilaciones incrementales y el estado incremental no debe viajar en la caché de `target/`; lo vigila `test_cargo_jobs_disable_incremental_at_job_level`. En `test-windows` también está ahí `RUSTFLAGS=-C linker=rust-lld -C target-feature=+crt-static`: entra en el fingerprint de cargo y reemplaza los `rustflags` del target de `.cargo/config.toml` en todo el job, de modo que las puertas y la suite comparten flags y `target/`. Como el reemplazo es completo, repite `+crt-static` para que las pruebas enlacen el CRT estático igual que el binario de release; rust-lld solo acelera el enlace.

El entorno de `sccache` (`RUSTC_WRAPPER`, `SCCACHE_DIR` y el límite `SCCACHE_CACHE_SIZE`) lo exporta `sccache_setup_unix` a `$BASH_ENV` en Linux y macOS (y solo esas tres variables); en Windows, sin `$BASH_ENV`, `RUSTC_WRAPPER` y `SCCACHE_DIR` se definen inline en los pasos de compilación de `test-windows` y `build-windows-x64`, y `SCCACHE_CACHE_SIZE` en el `environment:` de ambos jobs. `sccache --show-stats` muestra el límite vigente como `Max cache size`.

El parche `vendor/cmake-0.1.58` fija `CMAKE_<LANG>_FLAGS_RELEASE` con los flags que calcula el crate `cc` (runtime estático `/MT`) más `/O2 /Ob2 /DNDEBUG`, tanto sin generador explícito como con Ninja. La rama de Ninja es necesaria porque los proyectos con la política CMP0091 en OLD (oneDNN) heredarían `/MD` del valor por defecto de CMake mientras Rust enlaza con `/MT`, y el enlace fallaría con `LNK2038`. Así, los objetos C++ de Windows se compilan con los mismos flags de optimización y runtime que con Visual Studio, lo que se verifica con el diagnóstico de la sonda (`CMakeCache.txt` y `build.ninja`). Medición en una compilación local del binario de Windows (`--features full`, 4 hilos, mismo MSVC 14.44 que el executor): `16 min` con `target/` y `sccache` fríos frente a `4 min 45 s` con `target/` frío y `sccache` caliente (809 de 825 unidades C/C++ servidas por `sccache`). El executor de Windows compila unas 2,35 veces más lento que esa máquina, de modo que tras un cambio de clave de `target/` sin cambios nativos se espera `build-windows-x64` en torno a 11–15 min.

El mismo parche fija `CMAKE_INSTALL_LIBDIR=lib` salvo que el llamador lo
defina: en distribuciones tipo RHEL de 64 bits (la imagen `manylinux_2_28`),
`GNUInstallDirs` instala las librerías en `lib64`, y los build scripts que solo
buscan en `<out>/lib` (`onednn-src`) no las encontrarían.

#### Escenarios de invalidación y su costo

Qué cambio invalida qué caché y cuánto alarga los `build-*`, de menor a mayor
costo:

| Escenario | Cachés que se invalidan | Efecto en los `build-*` |
|-----------|-------------------------|-------------------------|
| Cambio en el código fuente del workspace (`src/`, `crates/*`), bump de versión de una release, documentación o `CHANGELOG.md` | Ninguna | Todas las cachés aciertan. Cargo recompila los crates del workspace (siempre `Dirty` por `mtime`): `sccache` devuelve al instante los que no cambiaron y compila de verdad los editados y el binario final. `cargo build --release` tarda unos 20 s en linux-x64. |
| Cambio de `ort_pin` (con sus `ort_sha256`) | `ort-v4` | Una descarga nueva de ONNX Runtime en los 4 `build-*` (cada uno cachea su triple). |
| Cambio de fuentes del motor TTS (`vendor/qwen3-tts`, `third_party/ingot`) | `tts-v1` | Solo se recompila el motor TTS en cada `build-*`; el resto acierta. |
| Cambio de los pines de MSYS2, gcc, openblas o make | `msys2-v1` y, con gcc u openblas, `tts-v1` | Reinstalación de MSYS2 y recompilación del motor TTS en `build-windows-x64`. |
| Cambio real de dependencias Rust en `Cargo.lock` (añadir, quitar o actualizar crates sin tocar nativos) | La caché de `target/` (sin fallback) y `cargo-v2` (cae a su fallback por prefijo) | `target/` parte vacío, pero `sccache` sirve por contenido Rust y C/C++: 2–5 min por job en Unix y ~11–15 min previstos en windows-x64. |
| Edición, alta o baja de un crate de `vendor/*/Cargo.toml` (parche vendorizado), o cambio de versión de un crate nativo (`ct2rs`, `onednn-src`, `sentencepiece-sys`, `aws-lc-sys`…) | La caché de `target/`; `sccache` falla solo para las unidades cuyo fuente o flags cambiaron (si el parche de cmake cambia los flags, afecta a toda la cadena CMake) | Recompilación de la cadena nativa afectada. Si alcanza a oneDNN o CTranslate2, el costo se acerca al frío total. |
| Cambio de `rust_pin` | `toolchain-v1`, `cargo-v2`, la caché de `target/` y `sccache-v1` (todas llevan la versión en la clave) | Frío total: se reinstala Rust y se recompila todo, Rust y C/C++, sin ayuda de `sccache`. Referencia de `cargo build --release` en frío: `1969 s` en windows-x64, `1569 s` en linux-arm64, `956 s` en linux-x64 y `149 s` en darwin-arm64. |

Causas ajenas al repositorio, que invalidan sin que cambie ningún archivo:

- **Cambio de `{{ arch }}` del executor.** Todas las claves llevan `{{ arch }}`, y en los executors Linux x86-64 ese valor incluye la familia y el modelo de CPU (por ejemplo `linux-amd64-6_85`). Si CircleCI asigna un hardware distinto, todas las familias de ese job fallan y la corrida es fría.
- **Expiración por retención.** CircleCI borra las cachés según la política de retención de la organización (15 días por defecto). Tras un período sin corridas más largo que la retención, la siguiente es fría. Las familias rolling (`sccache`) se renuevan en cada corrida; las inmutables (la caché de `target/`) solo existen mientras no expiren.
- **Actualización de la imagen del executor.** Una versión nueva del compilador (MSVC, Xcode/clang, gcc) cambia la identidad del compilador que `sccache` hashea: la clave restaura, pero la primera corrida no obtiene aciertos C/C++ y vuelve a sembrar la caché. Ninja no se cachea: se descarga en cada corrida con la versión fijada en `ninja_pin`.

#### Determinismo de releases (binario obsoleto)

Restaurar un `target/` de una release anterior nunca produce un binario con la
versión vieja, y no hace falta ningún paso de limpieza:

- **El crate raíz siempre se recompila.** El checkout deja `src/main.rs` y
  `Cargo.toml` con un `mtime` posterior al de cualquier artefacto restaurado, y
  el bump de versión cambia además la `-C metadata` con la que cargo nombra la
  unidad de compilación.
- **`sccache` no puede servir un binario viejo.** No cachea crates
  `--crate-type bin`: el binario final, los build scripts y `xtask` siempre
  pasan por `rustc`.
- **El smoke test lo comprueba.** Cada `build-*` compara la versión del
  binario con `CIRCLE_TAG` (`--version` en Linux y macOS, `version --json` en
  Windows) y falla si no coinciden.

Las claves exactas de cada familia están en `.circleci/config.yml`.

### Reproducibilidad: pines por digest y sus implicaciones

Las imágenes (`cimg/rust:1.96.0`, `manylinux_2_28` de los build-linux-* y las
de la sala limpia) van pineadas por digest (`@sha256:...`), y
`rust_pin` + `ort_pin` son parámetros únicos del pipeline (`pipeline.parameters`).
Ver `.circleci/config.yml` §Reproducibilidad para procedimiento de bump (bumpear
`ort_pin` invalida `ort-v4` en el siguiente tag).

El archivo de configuración completo está en `.circleci/config.yml`.

### Guardarraíles durables de la suite de tests (`tests/cli_golden.rs`)

Tres invariantes de la suite dorada son fáciles de "optimizar" por error y
quedan documentados aquí de forma autocontenida:

- **Serialización de residentes TTS (`TTS_LOCK`/`lock_tts()`).** Cada
  residente TTS activo ocupa un pico medido de ~2.7 GB de RAM. La suite
  serializa deliberadamente sus tests pesados de inferencia con un mutex
  global (`TTS_LOCK`, junto a `lock_tts()` en `tests/cli_golden.rs`) para
  garantizar que nunca haya más de un residente TTS vivo a la vez. Quitar o
  debilitar ese lock para paralelizar los tests pesados arriesga un OOM del
  runner de CI, no solo una degradación de tiempo: el lock no se elimina.
- **El warning de libtest ">60 seconds" no es un cuelgue.** Con la
  serialización vigente (`TTS_LOCK`), es esperable que `cargo test` reporte que un test
  de la serie pesada "has been running for over 60 seconds" mientras espera
  turno para adquirir `TTS_LOCK`. Es espera de mutex esperada, no trabajo
  atascado: no se le añaden reintentos ni timeouts para silenciarlo.
- **Las pruebas con recursos no se omiten ni aprovisionan.** Las pruebas pesadas
  llevan `#[ignore = "requiere …"]`: `cargo test --all` las muestra como
  *ignored* y, con `--include-ignored`, fallan con un mensaje que nombra el
  archivo que falta y el comando que lo aprovisiona
  (`ai-voice-interconnector setup` o
  `cargo xtask build-engine`). Ninguna descarga nada por su cuenta: hacerlo
  costaría 5,85 GB en cada corrida de CI sin obtener cobertura, porque el runner
  de tests no tiene el binario del motor ni los pesos locales. `cargo xtask
  release` las ejecuta antes del bump.

---

## 5. Distribución de artefactos

El **deliverable** que se publica a usuarios es el **archivo comprimido** de
cada target (binario Rust + los 4 documentos de licencia + la librería de ONNX
Runtime, con las DLL de VC++ en Windows + el motor `qwen_tts`), con su nombre de
release **versionado y con arch** (p. ej.
`ai-voice-interconnector-<ver>-x86_64-windows.zip`). Estos cuatro archivos, junto a los dos bootstrap estampados, llegan
al GitHub Release a través de `persist_to_workspace` / `attach_workspace` (7 assets con `SHA256SUMS.txt`).

El output del empaquetado en el runner vive en `artifacts/`:

```
artifacts/
├── ai-voice-interconnector-<ver>-x86_64-windows.zip   # Windows x64 (.zip)
├── ai-voice-interconnector-<ver>-x86_64-linux.tar.gz  # Linux x64
├── ai-voice-interconnector-<ver>-arm64-linux.tar.gz   # Linux ARM64
└── ai-voice-interconnector-<ver>-arm64-macos.tar.gz   # macOS (Apple Silicon)
```

`publish-release` estampa ambos bootstrap, recoge los 4 archivos más los 2 bootstrap por `attach_workspace`, calcula
`SHA256SUMS.txt` sobre los 6 ficheros y crea el GitHub Release (7 assets). El
layout de cada archivo es el de `packaging/bundle-manifest.json`: en la raíz,
el binario, los 4 documentos y la librería de ONNX Runtime (más las cuatro DLL de
VC++ en Windows); el motor, en `vendor/qwen3-tts/qwen_tts[.exe]`.

### Empaquetado por plataforma (archivos comprimidos)

El empaquetado **no requiere herramientas de terceros**: lo hace el propio
`cargo xtask package` en Rust, con los crates `tar` + `flate2` y `zip`, de forma
determinista (entradas ordenadas, marca temporal y permisos fijos), así que el
mismo árbol produce el mismo archivo en cualquier runner.

| Plataforma | Formato | Compresión en `xtask package` |
|------------|---------|-------------------------------|
| Linux x64 / ARM64 | `tar.gz` | crates `tar` + `flate2` |
| macOS arm64 | `tar.gz` | crates `tar` + `flate2` |
| Windows x64 | `.zip` | crate `zip` (Deflate) |

Cada `build-*` en `.circleci/config.yml` empaqueta con `cargo xtask
package --expect-version "${CIRCLE_TAG#v}"` (puerta tag-versión fail-fast:
`const VERSION` de `src/main.rs` vs `CIRCLE_TAG`) y después corre el humo de
instalación (`self install --no-setup --no-modify-path` en sandbox con raíces
reubicadas, y `--version` del binario instalado). El subcomando `package`
(`crates/xtask/src/package.rs`) exige el binario ya compilado en
`target/release/` y el motor en `vendor/qwen3-tts/`, ejecuta `version` y
`voice list` sobre el binario, asegura `ort-bundle/` y monta el bundle desde
`packaging/bundle-manifest.json` —la misma lista que valida `self install`—:
el binario renombrado a `ai-voice-interconnector[.exe]` (sin sufijo de
arquitectura) más los 4 documentos de la raíz (`LICENSE`,
`THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`, `README.md`) y la librería de
ONNX Runtime (con las DLL de VC++ en Windows) en la raíz, y el motor en
`vendor/qwen3-tts/`. Lo comprime al archivo del target con el nombre de
`release_asset_name`. Los documentos GPLv3 viajan así **dentro del archivo** y
quedan instalados junto al binario (cumplimiento §6 de la GPLv3 sin depender de
un bundle).

### Integración con el SO

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| PATH | El bootstrap `install.ps1` delega en `self install`, que registra `%LOCALAPPDATA%\Programs\ai-voice-interconnector` en HKCU (sin UAC); `self uninstall` lo revierte conservando el tipo del valor | El bootstrap `install.sh` (POSIX) delega en `self install`, que crea el symlink `~/.local/bin/ai-voice-interconnector → ~/.local/opt/ai-voice-interconnector/ai-voice-interconnector`; `self uninstall` lo retira | El mismo `install.sh` en macOS (Apple Silicon, `~/.local/bin`); Cask `brew install --cask` enlaza en `/opt/homebrew/bin` |
| Guía hacia `setup` | El one-liner encadena `setup` tras instalar | Ídem | Ídem (Cask no encadena; caveat remite a `setup`) |
| Desinstalación | `ai-voice-interconnector self uninstall --yes` (programa + PATH + estado, con R2) o manual | `ai-voice-interconnector self uninstall --yes` (enlace + dir + estado) | `ai-voice-interconnector self uninstall --yes` o `brew uninstall --cask --zap` |
| Datos provisionados | `ai-voice-interconnector cleanup --voices` / `--synthetic-speech` / `--model` / `--all` (limpieza granular sin programa ni PATH; sin categoría → exit 2) | Ídem | Ídem |

### Limitación conocida: firma de código y notarización

Los artefactos **no están firmados ni notarizados**: en macOS, Gatekeeper
bloquea la primera apertura del binario descargado por navegador; en Windows,
SmartScreen muestra advertencia de editor desconocido si el `.zip` se baja por
navegador. El mecanismo (Mark-of-the-Web) y la firma como arreglo de fondo
están en [SECURITY.md](../SECURITY.md#artefactos-sin-firmar). Firmar requiere
certificados de pago (Apple Developer ID, Authenticode vía SignPath OSS) y queda
registrado como goal a largo plazo en [docs/GOAL.md](GOAL.md#goal-a-largo-plazo).

Como mitigación, los **one-liners descargan por CLI** (`curl`/`Invoke-WebRequest`),
que no aplica Mark-of-the-Web, así que el archivo extraído no dispara
SmartScreen/Gatekeeper. Ver [docs/DISTRIBUTION.md](DISTRIBUTION.md).

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
base de MSYS2 y sincroniza las versiones pineadas, cacheando `C:\msys64` por
clave de versión (determinismo de release). El log del bootstrap captura
`pacman -Q` + `gcc
--version` como evidencia de la **GCC Runtime Library Exception** (libgfortran/
libquadmath/libgcc estáticos en el `.exe`). Si la versión instalada difiere del
pin, el bootstrap registra `[WARN]` y continúa usando la instalada como evidencia
(`log on drift`, no `fail-fast`). WSL oráculo: Ubuntu gcc
**15.2.0-16ubuntu1**, `libopenblas-dev 0.3.32+ds-5`.

| Plataforma | `ARCH_FLAGS` | `CFLAGS_BASE` | `LDLIBS` / BLAS | Shims |
|---|---|---|---|---|
| Windows UCRT64 | `-mavx2 -mfma` (Haswell 2013+, `SIMD=auto`; `SIMD=scalar` vacía) | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `-static -L$(UCRT64_LIB) -lopenblas -lgomp -lws2_32 -lwinpthread -lm` (33 MB autocontenido) | `third_party/ingot/mingw_shim/unistd.h`, `sys/mman.h` vía `-include` |
| Linux x86_64 | `-mavx2 -mfma` | `-Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math` | `libopenblas.a -Wl,-Bstatic -lgfortran -lquadmath -Wl,-Bdynamic -lm -lpthread` (solo glibc dinámica) | — |
| Linux ARM64 | `-march=armv8-a` (portable; NEON baseline ARMv8-A. `SIMD=native` → `-march=native`) | idem | `libopenblas.a -Wl,-Bstatic -lgfortran -Wl,-Bdynamic -lm -lpthread` (solo glibc dinámica) | — |
| macOS (arm64) | `-mcpu=apple-m1` (portable; `SIMD=native` → `-march=native`) | idem; `MACOSX_DEPLOYMENT_TARGET=26.0` en CI | `-framework Accelerate` | — |

> **Baseline portable en ARM y macOS.** Las ramas ARM y Darwin del `Makefile`
> honran `SIMD` como x86: por defecto `-march=armv8-a` en Linux ARM (NEON
> garantizado en todo ARMv8-A) y `-mcpu=apple-m1` en macOS (M1, el chip más
> antiguo que soporta la versión mínima de macOS), no `-march=native`, que
> acoplaría el codegen a la microarquitectura del runner (un M4 Pro en CI) y
> haría `SIGILL` en CPUs más antiguas. `SIMD=native` recupera `-march=native`
> para compilar y ejecutar en la misma CPU.

> **OpenBLAS estático en Linux.** Debian y Ubuntu instalan `libopenblas.a`
> (paquete `libopenblas-dev`) en `/usr/lib/<multiarch>/openblas-pthread/`, fuera
> de la ruta del enlazador, y EL8 (la imagen `manylinux_2_28` de la CI, paquete
> `openblas-static`) llama `libopenblasp.a` a la variante con hilos, así que el
> `Makefile` la busca por ruta, con la variante con hilos primero (o la toma
> de `OPENBLAS_A=<ruta>`) y, si no la encuentra, el enlace del motor falla con
> el remedio en lugar de enlazar la dinámica. Su LAPACK está compilado con
> gfortran: `libgfortran.a` debe ser la de la versión mayor del `gcc` en uso
> (`libgfortran-<versión>-dev`), porque `gcc` solo la busca en su propio
> directorio. `cargo xtask doctor` comprueba ambas librerías estáticas.

### Caché del motor en CI

En CI el binario del motor (`vendor/qwen3-tts/qwen_tts[.exe]`) se cachea con
clave `tts-v1-<segmento>-{{ checksum "vendor/qwen3-tts/.engine-cachekey" }}` donde
`.engine-cachekey` es el agregado determinista de `Makefile` + `*.c/*.h` (incluye `vendor/lz4.*`) + `third_party/ingot/**/*.{c,h}`
(lo genera el command `tts_engine_cachekey`, sin `-r` en `xargs` porque el de macOS no admite ese flag).
El segmento es `{{ arch }}-gcc<gcc>-ob<openblas>` en Windows y `linux-el8-{{ arch }}` o
`macos-{{ arch }}` en Unix: `{{ arch }}` solo discrimina la arquitectura del executor,
y sin el `os` el ARM64 de Linux y el de macOS colisionarían.
Si existe tras `restore_cache` (clave exacta sin fallback) solo se verifica con `--self-test`;
si no, se compila. El bundle ONNX Runtime (`ort-bundle/`) se cachea con
`ort-v4-<triple>-<< pipeline.parameters.ort_pin >>` (`win-x64`, `linux-x64`,
`linux-arm64`, `darwin-arm64`) y se guarda tras
`cargo xtask package`, que es quien lo descarga y verifica.
El bundle incluye el marcador `ort-bundle/.ort-version`: sin él la versión es
desconocida, `cargo xtask doctor` marca `Mismatch` y `cargo xtask package`
vuelve a descargar.

Ver `vendor/qwen3-tts/CLAUDE.md` y `crates/avi-tts/src/lib.rs` para el contrato
de invocación (`-j 4 --stream`, sin `--int4`; `GenerationOptions::production()` temp
0.35 seed 4).

---

## 7. Descarga nativa de modelos (`hf-hub`)

`setup` descarga los pesos de HuggingFace Hub de forma nativa vía el crate
**`hf-hub`** (rustls, sin OpenSSL: compila igual en los 4 targets) con barra de
progreso **`indicatif`** y validación ETag/commit-hash del propio crate. `hf-hub`
no reanuda por `Range`: una descarga interrumpida se repite completa. El crate está
vendorizado y fijado a `=1.0.0` (`vendor/hf-hub-1.0.0`): los punteros de `snapshots/`
son enlaces simbólicos en Unix y enlaces duros en Windows, por lo que el blob no se
duplica en disco; si el sistema de archivos no admite enlaces duros (FAT32, exFAT o algunos
recursos de red), se copia el blob y el disco se duplica. La descarga la hace el propio binario (crate `hf-hub`).

| Modelo lógico | Repo HF | Contenido |
|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Pesos TTS (síntesis) |
| `opus-mt-es-en` | `CristianRojaas/opus-mt-es-en-ct2-int8` | Traducción es→en (CTranslate2 int8, 82.536.565 bytes) |
| `opus-mt-en-es` | `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción en→es (CTranslate2 int8, 82.536.565 bytes) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT Parakeet TDT v3 int8 (671,1 MB, 4 artefactos: encoder-model.int8.onnx, decoder_joint-model.int8.onnx, nemo128.onnx, vocab.txt) |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base: clonado de voz y síntesis de las voces clonadas |

Los pines viven en `MODEL_REVISIONS` (`crates/avi-shared/src/paths.rs`): un `&[ModelPin]`
con `name`, `repo`, `revision` y `approx_bytes` (tamaño medido de la descarga, del que sale la
cifra que anuncia `setup`). Actualizar una revisión es una acción deliberada y auditable, y
obliga a revisar también su tamaño.

Los dos modelos de traducción son derivados int8 de Helsinki-NLP/opus-mt (CC-BY-4.0),
convertidos una sola vez a CTranslate2 y publicados en los repos `CristianRojaas/*-ct2-int8`.
Si una versión de `ct2rs`/CTranslate2 rompe la compatibilidad con ese formato, hay que
reconvertir con `ctranslate2` (4.8.1 en la conversión actual, con `--quantization int8
--copy_files source.spm target.spm`) y publicar una revisión nueva.

### Ubicaciones en disco por SO

La aplicación decide su caché (no depende del
fallback de `hf-hub`, que en Windows sin `HOME` caería en `<unidad>:\tmp`):
`models_cache_dir()` (`crates/avi-shared/src/paths.rs:212`) fija por defecto una raíz
**exclusiva de la aplicación** —la columna de modelos de la tabla siguiente—, y solo la
sustituye por la del usuario cuando este define `HF_HUB_CACHE` o, en su defecto,
`HF_HOME/hub`, que es lo que resuelve `shared_hf_root()`
(`crates/avi-shared/src/paths.rs:184`). `AVI_CACHE_DIR` tiene precedencia sobre ambas, y es
lo que permite aislar una prueba aunque el entorno tenga un `HF_HOME` global. El cliente de
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

## 8. Limpieza del entorno de desarrollo

Compilar, probar e instalar deja artefactos en dos capas que crecen sin límite.
Cada capa tiene su comando:

| Capa | Qué contiene | Comando |
|------|--------------|---------|
| Proyecto (repo) | `target/`, `ort-bundle/`, `artifacts/`, cobertura, binario/objetos/`output.wav` del motor en `vendor/qwen3-tts` | `cargo xtask clean --repo` (capa por defecto) |
| App (perfil de usuario) | Instalación, `data_dir()`, raíz de modelos y temporales `avi_*`/`avi-*`/`*.qvoice` | `cargo xtask clean --app` (delega en `ai-voice-interconnector self uninstall --yes`, o `cleanup --all --yes` en canal `homebrew`) |
| Ambas | Aplicación y después repositorio | `cargo xtask clean --all` |
| Global compartida | `~/.cargo/registry`, `~/.cargo/git`, caché de `sccache` | Manual: la comparten otros proyectos |

```bash
# Ver qué se borraría y cuánto ocupa, sin borrar nada
cargo xtask clean --repo --dry-run

# Poda fina: solo cachés, cruces ajenos y PDBs viejos, sin tocar la
# compilación vigente (el siguiente build sigue siendo incremental)
cargo xtask clean --prune --dry-run
cargo xtask clean --prune --yes

# Borrar el repositorio (pide confirmación [s/N]; --yes la omite, obligatorio sin TTY)
cargo xtask clean --repo --yes

# Aplicación y después repositorio
cargo xtask clean --all --yes
```

Tras integrar una rama en `main`, el hook `post-merge` de `.githooks/`
(opt-in por clon con `git config core.hooksPath .githooks`) ejecuta la poda
fina automáticamente cuando el merge invalida cachés y `target/` supera el
umbral; `cargo xtask doctor` avisa con la fila `target-hygiene` cuando hay
exceso. La estrategia de ramas y esta automatización se describen en
`docs/BRANCHING.md`.

`clean` se ejecuta desde la raíz del repo. Antes de borrar, la capa `--repo`
detiene el daemon lanzado desde `target/`; la capa `--app` delega en el binario
instalado. `~/.cargo`, la caché de `sccache`, los paquetes del sistema y MSYS2
**nunca** se tocan: se informa de ellos, no se borran. Nunca borra código fuente
versionado. En Windows, el propio `xtask.exe` en ejecución se borra con un
proceso auxiliar al terminar.

Tras `clean`, el entorno arranca de cero: `cargo build` recompila todo (con
`sccache` el costo baja), `cargo xtask build-engine` reconstruye el
motor y `setup` vuelve a descargar los modelos pineados.
