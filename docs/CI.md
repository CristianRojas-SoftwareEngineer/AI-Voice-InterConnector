# Análisis del pipeline de CI (CI.md)

Este documento explica **por qué** el pipeline de CircleCI tiene la forma que tiene y cómo se comportan sus cachés. No describe cada job ni el flujo de publicación: la definición ejecutable (claves exactas, pines, parámetros y comentarios de cada caché) está en [`.circleci/config.yml`](../.circleci/config.yml), las versiones fijadas en [`packaging/pins.json`](../packaging/pins.json) y el flujo de release paso a paso en [`docs/RELEASING.md`](RELEASING.md).

## Tabla de contenidos

- [Análisis del pipeline de CI (CI.md)](#análisis-del-pipeline-de-ci-cimd)
  - [Tabla de contenidos](#tabla-de-contenidos)
  - [1. Forma del pipeline](#1-forma-del-pipeline)
    - [Por qué un pipeline y no dos](#por-qué-un-pipeline-y-no-dos)
    - [Feedback pre-release](#feedback-pre-release)
    - [Simetría de puertas y builds](#simetría-de-puertas-y-builds)
    - [Hueco de cobertura de ARM64](#hueco-de-cobertura-de-arm64)
  - [2. Cachés](#2-cachés)
    - [Qué se restaura y qué se recompila](#qué-se-restaura-y-qué-se-recompila)
    - [Clave normalizada de `Cargo.lock`](#clave-normalizada-de-cargolock)
    - [Parches vendorizados en la clave de `target/`](#parches-vendorizados-en-la-clave-de-target)
    - [Guardado inmutable frente a rolling](#guardado-inmutable-frente-a-rolling)
    - [Cobertura de `sccache`](#cobertura-de-sccache)
  - [3. Invalidación](#3-invalidación)
    - [Escenarios de invalidación](#escenarios-de-invalidación)
    - [Causas ajenas al repositorio](#causas-ajenas-al-repositorio)
  - [4. Determinismo de releases](#4-determinismo-de-releases)

## 1. Forma del pipeline

### Por qué un pipeline y no dos

La publicación es irreversible, así que debe estar condicionada a los tests por una dependencia real, no por confianza. CircleCI no encadena pipelines distintas: un job solo puede depender de otro dentro del mismo workflow. La única forma de que los tests **bloqueen** el release es tenerlos en el grafo de `requires:` de los builds, es decir, en el mismo pipeline (`build-all`, disparado solo por tags `v*`). Las puertas (tests por SO, cobertura, suites de bootstrap y validaciones de licencias y changelog) son `requires:` de todos los builds nativos.

Se descartó un pipeline de rama `validate`. Como el corte empuja commit y tag juntos, ese workflow disparaba una segunda pipeline que re-ejecutaba la misma suite sobre el mismo commit, en paralelo con `build-all`: no adelantaba la señal ni aportaba cobertura única. Con las puertas dentro del pipeline de release, los tests corren una vez por release y bloquean de verdad la publicación.

### Feedback pre-release

El repo es trunk-based sobre `main`, sin ramas de larga vida, y los commits de rama no disparan CI. La red de seguridad es la puerta al taggear: si la suite falla, los builds no corren y nada se publica (se corrige hacia adelante o se revierte y se vuelve a taggear). No existe un estado de `main` sin tag que publique, así que no hace falta *branch protection* estricta para sostener la garantía «commit taggeado probado».

### Simetría de puertas y builds

Las puertas de test y los builds responden a **ejes distintos**:

- Los tests son **por familia de SO** y validan lógica específica de cada una (en Windows, el lanzamiento con lista explícita de handles, el borrado diferido, `PATH` en HKCU y WASAPI; en macOS, CoreAudio y la cuarentena de Gatekeeper; en Linux y macOS, `setsid`, señales e identidad de proceso por `/proc` y `proc_pidinfo`; en Linux, ALSA).
- Los builds son **por target de distribución**, y Linux publica dos arquitecturas.

Son ejes ortogonales (SO × target). El runner de `test-linux` es x86_64 porque es el executor Docker más barato y rápido; la suite es independiente de la arquitectura y mockea el motor, así que basta la más barata.

### Hueco de cobertura de ARM64

Es una divergencia aceptada: `build-linux-arm64` está condicionado por tests en x86_64, y en ARM solo corre el smoke test de [Verificación post-build](BUILD.md#verificación-post-build): arrancar el binario (lo que exige resolver sus librerías dinámicas), `voice list`, el `--self-test` de los kernels del motor y el humo de `self install`. Ni la traducción (CTranslate2) ni la transcripción (ONNX Runtime) se ejecutan en ARM dentro del pipeline.

## 2. Cachés

Las familias de caché, sus contenidos y sus claves exactas están comentadas en la cabecera y en el bloque «Cachés compartidas» de [`.circleci/config.yml`](../.circleci/config.yml). Aquí se explica el razonamiento.

### Qué se restaura y qué se recompila

Intervienen dos mecanismos independientes. Distinguirlos explica por qué un cambio de código siempre llega al binario publicado y, aun así, el pipeline puede ser rápido.

1. **Qué restaura CircleCI.** Ninguna clave depende del código fuente del workspace: dependen de las dependencias de `Cargo.lock`, de la versión de Rust, del SO, de la variante de compilación y de los pines de herramientas. Editar `src/` o `crates/*` no cambia ninguna clave, así que se restaura el mismo `target/` y la misma caché de `sccache`.
2. **Qué recompila cargo dentro de lo restaurado.** Cargo decide unidad por unidad con su *fingerprint*:
   - Las dependencias de crates.io se identifican por versión y checksum; si no cambiaron quedan `Fresh`. Eso incluye la cadena nativa C++ (CTranslate2, oneDNN, SentencePiece, aws-lc), la parte más cara.
   - Los crates del workspace se identifican por la fecha de modificación (`mtime`). El checkout de CI les da la fecha actual, posterior a la de `target/`, así que quedan `Dirty` y se recompilan en cada corrida, se hayan editado o no.
   - `sccache` atiende esas recompilaciones por hash de contenido: si el archivo y los flags no cambiaron, devuelve el objeto guardado casi al instante. Los crates `bin` (el binario final, `xtask` y los build scripts) no son cacheables por `sccache` y siempre pasan por `rustc`.

Un cambio en el código fuente no invalida ninguna caché y siempre se compila; su costo es solo el de recompilar los crates afectados. Lo que alarga el pipeline es un cambio de clave de la caché de `target/` (dependencias reales o parche vendorizado), porque entonces `target/` parte vacío y las dependencias externas se recompilan con la ayuda de `sccache`.

El `variant` de la clave (`test`, `cov`, `full`) separa conjuntos de features distintos y el `os` separa los targets: sin ellos, jobs distintos compartirían una entrada incompatible.

### Clave normalizada de `Cargo.lock`

Las claves de `cargo-v2` y de la caché de `target/` no usan `Cargo.lock` directamente, sino `Cargo.lock.cachekey`, que genera el command `cargo_lock_cachekey` como primer paso de la restauración. Reemplaza la versión del propio crate por `0.0.0` con `perl` (disponible en los tres executors con `shell: bash`). Cada release cambia esa versión; sin la normalización, la clave cambiaría en cada corte aunque las dependencias fueran las mismas, y la caché de `target/`, que no tiene fallback, partiría siempre en frío.

El paso falla si la sustitución no ocurre exactamente una vez, para que un `Cargo.lock` con otro formato no degrade la caché en silencio. Se usa un transform de texto y no `xtask` para no compilar `xtask` y su árbol de dependencias antes de restaurar la caché.

Esto sostiene una invariante de orden en todos los jobs que usan cargo: ningún paso compila antes de restaurar las cachés. Una restauración extraída sobre un `target/` ya poblado deja el estado de dos ejecuciones, y el enlazado puede reutilizar objetos que esperan símbolos que el código actual ya no define. Las puertas `validate_pins` y `validate_features` son la primera compilación del workspace y ocurren tras restaurar el registry y las cachés. La vigila `test_cargo_jobs_compile_only_after_cache_restore`.

### Parches vendorizados en la clave de `target/`

El `Cargo.toml` raíz reemplaza varios crates por copias parcheadas locales bajo `vendor/` (`[patch.crates-io]`). Al ser paquetes locales, cargo los identifica por `mtime`, y el `mtime` nuevo del checkout los marcaría `Dirty` en cada corrida. Para `cmake` eso arrastraría la recompilación de toda la cadena nativa que depende de él, y para `ct2rs` y `onednn-src` la de CTranslate2 y oneDNN, aunque su contenido no hubiera cambiado.

Para evitarlo, la restauración:

1. Toma como parches los directorios que cumplen el glob `vendor/*/Cargo.toml`, en orden `LC_ALL=C` y sin analizar TOML, de modo que un parche nuevo entra en la clave sin tocar el pipeline.
2. Escribe en `.vendor-patches.tree` el tree hash git de cada uno (`git rev-parse HEAD:<directorio>`) y lo incorpora a la clave de `target/`.
3. Como esa clave no tiene fallback, un acierto garantiza que `target/` se compiló con el mismo contenido de los parches, y es seguro fijarles un `mtime` antiguo para que cargo los vea `Fresh`. Si un parche cambia, cambia la clave y `target/` parte vacío.

El paso aborta si `vendor/` no contiene ningún crate o si alguna línea no tiene 40 caracteres hexadecimales, para no cachear nunca bajo una clave degenerada. Las pruebas de `xtask` exigen que el conjunto coincida con los paquetes de ruta ajenos al workspace que resuelve `cargo metadata --all-features` (detectan parches sin usar, parches fuera de `vendor/` y crates vendorizados sin `[patch]`) y que ninguna regla de ignorado alcance sus ficheros, que se compilan enteros desde ahí.

Se usa git y no `find | sort` porque en el executor de Windows `sort` resuelve al `sort.exe` de System32, y el tree hash es además inmune a la conversión de fin de línea del checkout.

### Guardado inmutable frente a rolling

- **La caché de `target/` es inmutable.** `save_cache` solo escribe si la clave no existe, así que la entrada refleja la primera corrida que la creó. Los builds la guardan solo si el job tuvo éxito, para no persistir un `target/` a medio compilar bajo una clave que ya no se puede reescribir. Los jobs de test y `coverage` la guardan siempre, porque ahí un fallo suele ser un test y no una compilación incompleta.
- **`sccache` es rolling.** Cada job pesado guarda una entrada nueva en cada corrida, incluso con fallos, y la restauración toma la más reciente. No hay guardado condicional: los fallos de una corrida son objetos que `sccache` acaba de compilar, y omitir el guardado haría que la siguiente los volviera a fallar. Cada cambio de `Cargo.lock` agrega objetos que ya nadie usa, así que el tamaño se acota con el parámetro de pipeline `sccache_cache_size`, dimensionado por encima del working set del job más grande. Al superarlo, `sccache` desaloja por LRU: un acierto actualiza el `mtime` de la entrada, también en una caché recién restaurada desde el tar de CircleCI, así que se desalojan los objetos sin uso y no los vigentes.

### Cobertura de `sccache`

`sccache` envuelve tres tipos de compilación: `rustc` (`RUSTC_WRAPPER=sccache`); los nativos que compila el crate `cc` (`aws-lc-sys`, `ring`, `blake3`…), que detecta `RUSTC_WRAPPER` y lo usa como wrapper; y, en los builds de release, los proyectos CMake (`ct2rs`/CTranslate2, `onednn-src`, `sentencepiece-sys`) mediante `CMAKE_C_COMPILER_LAUNCHER` y `CMAKE_CXX_COMPILER_LAUNCHER`. CMake ≥ 3.17 lee esas variables del entorno al configurar, así que no se toca ningún `build.rs`. En Linux y macOS se exportan a `$BASH_ENV`. En Windows hacen falta tres piezas más, todas en el paso de compilación de `build-windows-x64`:

- **Ninja como generador** (`CMAKE_GENERATOR=Ninja`): el generador Visual Studio, el que el crate `cmake` elige por defecto con MSVC, ignora los launchers. Ninja se descarga en cada corrida con la versión fijada por el parámetro `ninja_pin`, fuera de toda caché, y se verifica su versión.
- **Entorno de MSVC**: Ninja no carga `INCLUDE`/`LIB` por sí mismo (el generador Visual Studio sí), así que el paso localiza Visual Studio con `vswhere`, importa `vcvars64.bat` y solo después antepone Ninja y `.cargo\bin` al `PATH`.
- **`CC`/`CXX` con la ruta absoluta de `cl.exe`**: el crate `cc` solo usa `RUSTC_WRAPPER` como wrapper en MSVC cuando el compilador es la ruta de un ejecutable existente.

`CARGO_INCREMENTAL=0` está en el `environment:` de cada job que usa cargo, porque `sccache` no cachea compilaciones incrementales y el estado incremental no debe viajar en la caché de `target/`; lo vigila `test_cargo_jobs_disable_incremental_at_job_level`. En `test-windows`, `RUSTFLAGS` también vive ahí: entra en el fingerprint de cargo y reemplaza los `rustflags` del target de `.cargo/config.toml` en todo el job, de modo que las puertas y la suite comparten flags y `target/`. Como el reemplazo es completo, repite `+crt-static` para que las pruebas enlacen el CRT estático igual que el binario de release; `rust-lld` solo acelera el enlace.

El parche `vendor/cmake-*` fija `CMAKE_<LANG>_FLAGS_RELEASE` con los flags que calcula el crate `cc` (runtime estático `/MT`) más `/O2 /Ob2 /DNDEBUG`, tanto sin generador explícito como con Ninja. La rama de Ninja es necesaria porque los proyectos con la política CMP0091 en OLD (oneDNN) heredarían `/MD` del valor por defecto de CMake mientras Rust enlaza con `/MT`, y el enlace fallaría con `LNK2038`. Así, los objetos C++ de Windows se compilan con los mismos flags de optimización y runtime que con Visual Studio, lo que se verifica con el diagnóstico de la sonda (`CMakeCache.txt` y `build.ninja`) del workflow `native-cache-probe`, documentado en la cabecera de `.circleci/config.yml`.

El mismo parche fija `CMAKE_INSTALL_LIBDIR=lib` salvo que el llamador lo defina: en distribuciones tipo RHEL de 64 bits (la imagen `manylinux_2_28`), `GNUInstallDirs` instala las librerías en `lib64`, y los build scripts que solo buscan en `<out>/lib` (`onednn-src`) no las encontrarían.

## 3. Invalidación

### Escenarios de invalidación

De menor a mayor costo en los `build-*`:

| Escenario | Cachés que se invalidan | Efecto |
|-----------|-------------------------|--------|
| Cambio en el código fuente del workspace, bump de versión de una release, documentación o `CHANGELOG.md` | Ninguna | Todas aciertan. Cargo recompila los crates del workspace (siempre `Dirty` por `mtime`): `sccache` devuelve los que no cambiaron y compila de verdad los editados y el binario final. Es el caso más barato. |
| Cambio de `ort_pin` (con su SHA-256) | `ort-bundle` | Una descarga nueva de ONNX Runtime en cada build, que cachea su propio triple. |
| Cambio de fuentes del motor TTS | `tts` | Solo se recompila el motor TTS; el resto acierta. |
| Cambio de los pines de MSYS2, gcc, openblas o make | `msys2` y, con gcc u openblas, `tts` | Reinstalación de MSYS2 y recompilación del motor TTS en Windows. |
| Cambio real de dependencias Rust en `Cargo.lock` sin tocar nativos | `target/` (sin fallback) y `cargo-v2` (cae a su fallback por prefijo) | `target/` parte vacío, pero `sccache` sirve por contenido Rust y C/C++: costo moderado en Unix y notablemente mayor en Windows, el executor más lento. |
| Edición, alta o baja de un parche vendorizado, o cambio de versión de un crate nativo (`ct2rs`, `onednn-src`, `sentencepiece-sys`, `aws-lc-sys`…) | `target/`; `sccache` falla solo para las unidades cuyo fuente o flags cambiaron (si el parche de cmake cambia los flags, afecta a toda la cadena CMake) | Recompilación de la cadena nativa afectada. Si alcanza a oneDNN o CTranslate2, el costo se acerca al frío total. |
| Cambio de `rust_pin` | `toolchain`, `cargo-v2`, `target/` y `sccache` (todas llevan la versión en la clave) | Frío total: se reinstala Rust y se recompila todo, Rust y C/C++, sin ayuda de `sccache`. Es el caso más caro. |

### Causas ajenas al repositorio

Invalidan sin que cambie ningún archivo:

- **Cambio de `{{ arch }}` del executor.** Todas las claves llevan `{{ arch }}`, y en los executors Linux x86-64 ese valor incluye la familia y el modelo de CPU. Si CircleCI asigna un hardware distinto, todas las familias de ese job fallan y la corrida es fría.
- **Expiración por retención.** CircleCI borra las cachés según la política de retención de la organización. Tras un período sin corridas más largo que la retención, la siguiente es fría. Las familias rolling (`sccache`) se renuevan en cada corrida; las inmutables (`target/`) solo existen mientras no expiren.
- **Actualización de la imagen del executor.** Una versión nueva del compilador (MSVC, Xcode/clang, gcc) cambia la identidad del compilador que `sccache` hashea: la clave restaura, pero la primera corrida no obtiene aciertos C/C++ y vuelve a sembrar la caché. Ninja no se cachea: se descarga en cada corrida con la versión de `ninja_pin`.

## 4. Determinismo de releases

Restaurar un `target/` de una release anterior nunca produce un binario con la versión vieja, y no hace falta ningún paso de limpieza:

- **El crate raíz siempre se recompila.** El checkout deja `src/main.rs` y `Cargo.toml` con un `mtime` posterior al de cualquier artefacto restaurado, y el bump de versión cambia además la `-C metadata` con la que cargo nombra la unidad de compilación.
- **`sccache` no puede servir un binario viejo.** No cachea crates `--crate-type bin`: el binario final, los build scripts y `xtask` siempre pasan por `rustc`.
- **El smoke test lo comprueba.** Cada build compara la versión del binario con `CIRCLE_TAG` y falla si no coinciden.
