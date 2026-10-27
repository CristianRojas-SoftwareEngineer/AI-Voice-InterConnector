# Plan: reparar la provisión de FFmpeg en la CI y el reconocimiento del FFmpeg integrado

## Estado de ejecución

- **T1–T6 ejecutadas y verificadas en local** (2026-10-27), sin tocar el remoto.
- **T7 pendiente de autorización explícita**: confirmar y publicar el commit, disparar el workflow y extraer evidencia por job.
- **Enmienda aplicada durante T2 con autorización del usuario**: la descarga del artefacto de Windows pasa del sitio de paquetes del autor (`gyan.dev`, que degradaba la transferencia sostenida a ~50 KB/s hasta hacerla inútil para 110 MB) al release que ese mismo autor publica en GitHub, y `download_https` gana reintentos, límite de conexión y detector de estancamiento. El fichero es el mismo en ambos orígenes: 114.768.076 bytes y SHA-256 `60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba`, que es el valor fijado. Se aplica en el producto, así que vale para local y para los cuatro jobs de CI.
- **Corrección de un literal del plan**: la línea de versión del artefacto real de Windows es `ffmpeg version 9.0.2-essentials_build-www.gyan.dev`, no `9.0.2-full_build-www.gyan.dev`; la prueba de T1 usa el literal verificado.

## Contexto

El workflow `denoise-preintegration` de CircleCI (`.circleci/config.yml:2408`) es la puerta de preintegración de la rama `feat/denoise-migration`: nueve jobs de prueba corren en paralelo sin dependencias (`.circleci/config.yml:2410-2441`) y los cuatro jobs de construcción de los targets distribuidos dependen de los ocho gates de prueba (`.circleci/config.yml:2443-2490`). El pipeline #502, corrido sobre el commit `cb68a93e`, dejó cuatro gates rojos —`test-linux` (job 2638), `test-macos` (2639), `coverage` (2641) y `test-windows` (2644)— y, en consecuencia, los cuatro jobs `build-*` (2646-2649) quedaron `not_run`. Es decir: **ninguno de los cuatro targets distribuidos (Windows x64, Linux x64, Linux ARM64, macOS ARM64) se ha validado con la configuración vigente**.

Ninguno de los cuatro fallos es del código bajo prueba. Las tres causas están verificadas contra los artefactos fijados:

1. **`test-linux` y `coverage` mueren sin mensaje** en el paso de instalación de FFmpeg. La aserción `ffmpeg -version | grep -Eq 'ffmpeg version ([6-9]|[1-9][0-9])\.'` (`.circleci/config.yml:849` en `HEAD`, y su bloque duplicado en `coverage`) no puede coincidir: el artefacto fijado imprime `ffmpeg version n9.0.2-35-g9a9b70bb8c-20261024` —literal extraído de la sección de datos del propio binario descargado, cuyo SHA-256 `ddffcf6e...` coincide con `packaging/pins.json`—. Con `set -euo pipefail`, `grep -q` sin coincidencia devuelve 1 y el paso aborta sin imprimir nada. El diff local sin commit sustituye esa aserción por ecos de depuración: no arregla nada, solo la esconde.
2. **`test-macos` falla en la suite** (`cargo nextest run --all`, salida 100): 762 de 763 pruebas pasan y falla `denoise::test_audio_egress::test_encode_audio_codec_matrix` con `Unknown encoder 'libvorbis'`. La fórmula `ffmpeg` de Homebrew ya no depende de `libvorbis`, y el paso lo sabía: su bucle de codificadores lo excluye con un comentario que afirma que no está (`.circleci/config.yml:1167-1172`), mientras el producto mapea `.ogg` a ese codificador (`crates/denoise/src/ffmpeg_io.rs:344`). El artefacto que el producto **sí distribuye** para macOS ARM64 (Martin Riedl `1789931890_9.0.2/ffmpeg.zip`, SHA-256 `c8ed4c4e...` verificado contra `packaging/pins.json`) lo trae.
3. **`test-windows` falla al compilar** (salida 101): `aws-lc-sys` informa `Target platform: 'x86_64-pc-windows-gnu'` y el crate `cc` busca `gcc.exe`. El paso de la suite compone el PATH con `$env:ChocolateyInstall\bin` por delante de `$env:USERPROFILE\.cargo\bin` (`.circleci/config.yml:1015`), así que `cargo` resuelve al Rust GNU preinstalado en la imagen —el error cita `C:\ProgramData\chocolatey\lib\rust\tools\bin\cargo.exe`— en lugar del MSVC que fija `rust_pin`. El paso de MSYS2 con `binutils` (`.circleci/config.yml:947-971`) existe solo para servir a ese toolchain equivocado.

Además, la misma clase de defecto está en el producto: `verify_ffmpeg` (`crates/denoise/src/ffmpeg_io.rs:74`) exige `ffmpeg version (\d+)\.` y solo admite como respaldo los prefijos `N-` y `git-`, así que rechaza ese mismo FFmpeg integrado. Como `src/main.rs:1195` resuelve con `find_bundled_ffmpeg`, en Linux `speech denoise` aborta antes de procesar —y `speech denoise --version` imprime `ffmpeg missing` (`crates/denoise/src/cli.rs:604` usa `.ok()`)— pese a que el paquete trae el ejecutable. Windows y macOS no lo manifiestan porque sus artefactos imprimen el número de release desnudo.

Resultado buscado: los cuatro gates en verde, los cuatro jobs de construcción ejecutados y verificados sobre el mismo commit, y el producto reconociendo el FFmpeg que distribuye en las tres plataformas.

## Decisiones de diseño vigentes

- **El aprovisionamiento de las suites reutiliza el código del producto, no lo reimplementa.** `crates/xtask/src/package.rs` ya hace todo lo necesario y de forma portable: `ensure_ffmpeg_bundle` (línea 582) elige el artefacto por triple con `ffmpeg_download` (188), verifica el SHA-256 fijado con `pinned_ffmpeg_sha256` (162) y `verify_sha256` (243), extrae con `zip`/`lzma-rs`+`tar` (`extract_ffmpeg_binary`, 528), aplica permisos 0755 en Unix y ejecuta `smoke_ffmpeg` (628), que exige la línea de versión y **los seis codificadores** `libmp3lame libvorbis libopus aac flac pcm_s16le`. `xtask` es Rust puro (sin compilación de C) y la CI ya lo invoca en otras puertas (`pins --check`, `alsa-symbols --check`, `source-offer --check`).
- Se descartó reconstruir ese aprovisionamiento en YAML con `curl`, `sha256sum`/`shasum` y `unzip`: obligaría a duplicar la URL por target y la lista de codificadores, a extender la puerta de pines con una tabla nueva, y a resolver a mano la portabilidad del hash y de la extracción (la imagen de macOS no trae `sha256sum`).
- **`test-windows` prueba el target que se distribuye**: se alinea con el patrón ya verde de `build-windows-x64` (`.circleci/config.yml:1577-1588`) y se retira el andamiaje de MSYS2 que solo servía al toolchain GNU.
- **El defecto de producto entra en el mismo trabajo**: es la misma clase y la misma causa que el fallo de CI.
- **Sin retrocompatibilidad ni narrativa histórica**: se corrige el contrato, se actualiza el código y la documentación, y se retiran los restos (recetas por sistema operativo, aserciones frágiles, ecos de depuración, paso de MSYS2 sin propósito, comentarios que afirmen que un codificador no existe donde el artefacto distribuido sí lo trae).

## Alcance por target

| Target | Jobs de prueba que lo cubren | Job de construcción | FFmpeg implicado |
|---|---|---|---|
| Windows x64 (`x86_64-pc-windows-msvc`) | `test-windows` | `build-windows-x64` | Gyan `9.0.2-essentials_build.zip` |
| Linux x64 (`x86_64-unknown-linux-gnu`) | `test-linux`, `coverage` | `build-linux-x64` | BtbN `linux64-gpl-9.0` |
| Linux ARM64 (`aarch64-unknown-linux-gnu`) | **ninguno** | `build-linux-arm64` | BtbN `linuxarm64-gpl-9.0` (solo vía `xtask package`) |
| macOS ARM64 (`aarch64-apple-darwin`) | `test-macos` | `build-darwin-arm64` | Martin Riedl `9.0.2` arm64 |

Los otros cuatro gates (`validate-licenses`, `test-bootstrap-linux|windows|macos`) bloquean igualmente los cuatro builds y no tocan FFmpeg; se vigilan en la corrida única pero no requieren cambios. Linux ARM64 no tiene job de prueba: su FFmpeg solo se ejerce por el humo de `xtask package` en `build-linux-arm64`, y así se declara.

## Tareas

### T1 — Reconocer el FFmpeg integrado (`crates/denoise/src/ffmpeg_io.rs`)

- Sustituir en `verify_ffmpeg` (línea 64) el reconocimiento por una función pura nueva, contigua, que devuelva la versión mayor de la primera línea de `ffmpeg -version` admitiendo el prefijo de `git describe` y el número de release desnudo; mantener el rechazo por debajo de 6 con el error y el mensaje que ya usa la resolución (`errors.rs:16`), conservando el respaldo para las formas `N-`/`git-`.
- Actualizar el comentario de documentación de la función para declarar qué formas acepta y por qué el FFmpeg integrado de Linux llega con el prefijo de `git describe` (comentario autocontenido, en español).
- Añadir pruebas en `mod tests` (línea 541) con los literales reales por plataforma: `9.0.2-essentials_build-www.gyan.dev`, `n9.0.2-35-g9a9b70bb8c-20261024`, `9.0.2` de macOS, una forma `git-`, una versión `5.1.4` (rechazo) y una línea no reconocible.
- Hoy no existe ninguna prueba de `verify_ffmpeg`: este es el punto ciego que permitió el defecto.
- Reversión: restaurar el reconocimiento anterior; la resolución vuelve a exigir el número de release desnudo.

### T2 — Subcomando de aprovisionamiento (`crates/xtask/src/package.rs`, `crates/xtask/src/main.rs`)

- Parametrizar el destino en `ensure_ffmpeg_bundle` (línea 582): recibir el directorio del bundle en lugar de componerlo siempre como `<raíz>/ffmpeg-bundle` (`FFMPEG_BUNDLE_REL`, línea 38); actualizar su única llamada (`package.rs:951`) y conservar el marcador `.ffmpeg-sha256` (`FFMPEG_MARKER`, línea 42) y el humo `smoke_ffmpeg` como autoridad única de versión y codificadores.
- Añadir una función de entrada `run_ffmpeg(dest)` que resuelva el triple del host con `host_triple()` (línea 87), cargue los pines con la misma vía que `package::run` y aprovisione, informando por salida estándar la ruta del ejecutable resultante para que quien la invoque no tenga que adivinar rutas. Ese resolutor ya cubre exactamente los cuatro targets de este plan (`x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin`) y falla con mensaje en plataformas no soportadas, así que el subcomando no necesita parámetro de triple ni puede aprovisionar un artefacto que no corresponda al host.
- Declarar la variante en el enum `Commands` (`main.rs:308`) con ayuda en español, y despacharla junto a `Commands::Package` (`main.rs:604-638`).
- Cubrir con pruebas: destino explícito, reutilización de un bundle fresco (marcador coincidente) y reconstrucción cuando el marcador diverge.
- Este subcomando sustituye la necesidad de extender la puerta de pines: la versión, los hashes y las URLs se leen del manifiesto en tiempo de ejecución, así que no pueden divergir de `packaging/pins.json`.
- Reversión: retirar la variante y devolver la firma anterior; el empaquetado queda como está hoy.

### T3 — Comandos compartidos de aprovisionamiento (`.circleci/config.yml`)

- Añadir dos comandos en la sección `commands:`, contiguos a la instalación de Rust: uno Unix y otro Windows, que invoquen `cargo xtask ffmpeg --dest <directorio>` y publiquen ese directorio en el PATH (en Unix, en el shell en curso **y** en `$BASH_ENV`; en Windows, en el PATH del paso, ya que no hay `$BASH_ENV`).
- Sustituir las cuatro recetas por el comando correspondiente: `test-linux` y `coverage` (los dos bloques duplicados), `test-macos` (retirando además la instalación condicional con Homebrew y su silenciamiento de avisos) y `test-windows` (retirando la instalación por Chocolatey).
- Descartar el diff local de depuración: los pasos nuevos lo reemplazan por completo, de modo que no queden ecos de depuración ni aserciones frágiles duplicadas.
- Actualizar el encabezado del workflow `denoise-preintegration` para describir el aprovisionamiento único desde el manifiesto de pines.
- Reversión: restaurar los pasos anteriores de los cuatro jobs desde `cb68a93e` y relanzar.

### T4 — Toolchain MSVC de `test-windows` (`.circleci/config.yml`)

- Eliminar el paso «Instalar MSYS2 UCRT64 (mingw-w64 para dlltool)» (947-971) y su comentario.
- En el paso de la suite (1010-1046): importar el entorno de MSVC con `vswhere` y `vcvars64.bat` **antes** de anteponer directorios, fijar `CC` y `CXX` a la ruta absoluta de `cl.exe`, y ordenar el PATH con el directorio del FFmpeg aprovisionado y `~/.cargo/bin` por delante de `$env:ChocolateyInstall\bin`.
- Si el paso de aprovisionamiento de FFmpeg necesita el mismo entorno (compila `xtask` con `cargo`), extraer esa preparación a un comando compartido que usen ambos pasos, en lugar de duplicarla. **No lo necesita**: `xtask` no compila ningún crate con C y `rustc` localiza el `link.exe` del MSVC instalado sin `vcvars64`, así que el paso de aprovisionamiento solo antepone `~\.cargo\bin`. La condición se cumple por la negativa y así queda escrito en el comentario del comando.
- Declarar en el comentario de cabecera del job que la suite corre sobre el target MSVC que el paquete distribuye y que el orden del PATH es un invariante, no un detalle.
- Reversión: restaurar el PATH y el paso retirado desde `cb68a93e` y relanzar.

### T5 — Verificación local antes de gastar cuota

- `cargo xtask pins --check` (los pines siguen en sincronía; el subcomando nuevo no introduce literales).
- `cargo test -p xtask` (subcomando nuevo y pruebas de empaquetado) y la suite de `denoise` **usando el artefacto recién aprovisionado** por el subcomando: esto convierte la precondición de FFmpeg de las pruebas de audio, hoy dependiente del equipo, en algo reproducible en local.
- `cargo fmt --all --check` y los lints del proyecto sobre todos los targets.
- Informar por separado cualquier verificación que el entorno local no pueda sostener, sin presentarla como fallo del producto. (`nextest` no está instalado en el host de desarrollo: se ejecuta `cargo test` sobre los mismos binarios de prueba.)

### T6 — Propagación canónica

- `CHANGELOG.md`: sección `[No publicado]` — entrada de corrección del reconocimiento del FFmpeg integrado y entrada de la capacidad de aprovisionamiento del FFmpeg fijado para desarrollo y pruebas.
- `docs/BUILD.md`: nombrar el aprovisionamiento de FFmpeg desde el manifiesto en el bloque de smokes de los jobs permanentes, retirando la referencia a la matriz temporal y precisando qué verifica el humo; y documentar el uso local (`cargo xtask ffmpeg` con el directorio en `PATH` antes de `cargo test`).
- `docs/CLI/CONTRACT.md` (232, 259) y `docs/CLI/commands/SPEECH.md` (34, 99, 121, 123): precisar qué formas de versión acepta el producto y qué ocurre con un binario no reconocido, manteniendo la resolución única desde el paquete.
- `docs/DESIGN.md:150` y `USAGE.md:669`: mantener la afirmación de resolución única y precisar el reconocimiento de versiones.
- `crates/xtask/src/main.rs:84-95` y `SOURCE-OFFER.md`: la procedencia de FFmpeg que emite `source-offer` debe coincidir con lo que descarga `ffmpeg_download`.
- `.work/plans/denoise-closure-plan.md`: Puerta B, Puerta D y registro de continuidad — causa raíz por job, arreglos y estado de los cuatro targets.
- `.work/plans/ci-ffmpeg-provision-plan.md`: este plan.
- No se toca `packaging/pins.json`: es la fuente, no el objeto del cambio.

### T7 — Una sola corrida de CI hacia los cuatro targets

- Solicitar autorización explícita y confirmar y publicar el commit (mensaje en español según las convenciones del proyecto).
- Disparar el workflow y seguir los nueve gates; al quedar verdes, los cuatro `build-*` arrancan por dependencia.
- Extraer evidencia por job: del paso de aprovisionamiento, la URL, el SHA-256 verificado, el triple y la versión impresa; de la suite, su resumen; de cada `build-*`, el artefacto por target con su hash y el humo de instalación (que ya ejecuta el `ffmpeg` instalado).
- Registrar el resultado en `.work/plans/denoise-closure-plan.md` (Puerta B) o el bloqueo con su causa. Si un target falla, no se declara la puerta satisfecha.

## Archivos críticos

- `crates/denoise/src/ffmpeg_io.rs` — reconocimiento de versión y sus pruebas (T1).
- `crates/xtask/src/package.rs` — `ensure_ffmpeg_bundle`, `ffmpeg_download`, `smoke_ffmpeg`, `host_triple`, `download_https` (T2).
- `crates/xtask/src/main.rs` — enum `Commands`, despacho (T2) y texto de procedencia (T6).
- `.circleci/config.yml` — comandos compartidos y los cuatro jobs de prueba (T3, T4).
- `CHANGELOG.md`, `docs/BUILD.md`, `docs/CLI/CONTRACT.md`, `docs/CLI/commands/SPEECH.md`, `docs/DESIGN.md`, `USAGE.md`, `SOURCE-OFFER.md` (T6).
- `.work/plans/denoise-closure-plan.md`, `.work/plans/ci-ffmpeg-provision-plan.md` (T6, T7).

## Riesgos y mitigaciones

- **Compilar `xtask` antes de la suite**: el job ya compila la workspace completa para `nextest`, así que el coste marginal es de ordenación. En Windows el paso de aprovisionamiento no necesita el entorno MSVC (no hay crates con C en el árbol de `xtask`), pero la suite sí lo importa por su cuenta.
- **Descarga por job**: 110 MB en Windows, 144 MB en Linux y 28 MB en macOS, una vez por job. La CI no cachea el directorio del bundle, y no debe hacerlo mientras la descarga sea de segundos: medido en este host, el artefacto de Windows baja en 3,5-9 s por el release del autor. Cachearlo duplicaría el SHA-256 que el marcador y el manifiesto ya llevan.
- **`curl` es requisito de `download_https`**: presente en las tres imágenes, y ya ejercitado por `cargo xtask package` en `build-windows-x64`.
- **Cadena de gates**: un gate ajeno a FFmpeg (`test-bootstrap-*`, `validate-licenses`) rojo bloquea los cuatro builds; se vigilan en la corrida única.
- **`test-bootstrap-macos` asevera la forma de `openssl version`** (`^OpenSSL 3\.`) mientras Homebrew ya publica `openssl@4`: hoy pasa, pero es la misma clase de fragilidad. Se vigila en la corrida y no se cambia sin evidencia de fallo.
- **Tiempo de la primera corrida**: cachés frías en los gates de prueba; el paso de la suite en Windows declara `no_output_timeout: 25m` porque la primera compilación con MSVC puede pasar del silencio por defecto.

## Verificación

1. **Local**: `cargo xtask ffmpeg --dest <directorio>` (imprime la ruta del binario, verifica el SHA-256 fijado y pasa el humo de versión y codificadores); después, con ese directorio en el PATH, la suite de `denoise` pasa la matriz de codificadores, incluido el caso `.ogg`; `cargo test -p xtask`; `cargo xtask pins --check`; `cargo fmt --all --check` y lints.
2. **End-to-end**: una corrida autorizada del workflow `denoise-preintegration` sobre el commit. Criterio de éxito: los nueve gates en verde y los cuatro `build-*` (`build-windows-x64`, `build-linux-x64`, `build-linux-arm64`, `build-darwin-arm64`) en verde, con el artefacto por target, el humo de instalación y el `ffmpeg` instalado ejecutándose.
3. **Evidencia por job**: URL, SHA-256 y versión del FFmpeg aprovisionado; resumen de la suite; nombre y hash del artefacto por target. Se registra en `.work/plans/denoise-closure-plan.md`.

## Autorizaciones

Confirmar, publicar y disparar el pipeline requieren consentimiento explícito y se solicitan solo en T7. Ninguna tarea anterior a T7 toca el remoto.
