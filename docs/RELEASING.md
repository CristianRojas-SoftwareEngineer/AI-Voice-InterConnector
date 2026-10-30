# Publicación de una versión (RELEASING.md)

`ai-voice-interconnector` publica sus releases de forma **automática**. Al pushear un
tag `v*`, CircleCI ejecuta el pipeline `build-all`:

1. Corre las puertas: tests, cobertura, instaladores, licencias y CHANGELOG.
2. Compila los 4 artefactos nativos.
3. Publica el GitHub Release directamente sobre el tag, sin borrador, con los
   artefactos y `SHA256SUMS.txt`.
4. Actualiza el Cask de Homebrew.

La distribución es solo de archivos comprimidos con el binario Rust (ver
[docs/DISTRIBUTION.md](DISTRIBUTION.md)). Los binarios no están firmados, así que
la integridad se verifica cotejando los checksums SHA-256.

## Tabla de contenidos

- [Prerrequisitos](#prerrequisitos)
  - [Configuración única](#configuración-única)
  - [Antes de cada corte](#antes-de-cada-corte)
  - [Aprovisionamiento de los recursos antes de cortar](#aprovisionamiento-de-los-recursos-antes-de-cortar)
- [1. Corte: crear y publicar el tag](#1-corte-crear-y-publicar-el-tag)
- [2. Automático: lo que hace el CI](#2-automático-lo-que-hace-el-ci)
- [3. Verificación post-publicación](#3-verificación-post-publicación)
- [4. Verificación del usuario final](#4-verificación-del-usuario-final)

## Prerrequisitos

### Configuración única

- **Context `github-release`** en CircleCI (Organization Settings → Contexts).
  Contiene la variable `GH_TOKEN`, un PAT fine-grained con permiso
  `contents: write` sobre el repo. Lo usan solo `publish-release` y
  `publish-metadata`.
- **Context `homebrew-tap`** en CircleCI. Contiene la variable
  `HOMEBREW_TAP_PAT`, un PAT fine-grained con permiso `Contents:RW` solo sobre el
  repositorio tap público `homebrew-ai-voice-interconnector`. Lo usa solo
  `publish-metadata`, que en cada release crea o reescribe
  `Casks/ai-voice-interconnector.rb` en el tap. El diseño del canal está en
  [docs/DISTRIBUTION.md](DISTRIBUTION.md).

### Antes de cada corte

- **Sección `## [No publicado]` curada en `CHANGELOG.md`.** Se escribe a mano
  mientras avanza el desarrollo (Keep a Changelog) y no conserva marcadores
  `TODO: curar`. El corte no genera contenido: **promueve** esa sección a la
  versión que se publica.
- **Árbol de trabajo limpio y al menos un commit desde el último tag.** Los
  cambios previos al bump —incluida la curación del CHANGELOG— deben estar
  commiteados antes de invocar `xtask release`; el comando aborta si no se cumple.
- **`THIRD-PARTY-LICENSES.md` en sincronía con `Cargo.lock`.** Cada vez que
  cambia `Cargo.lock`, hay que regenerar el inventario con
  `cargo xtask licenses` y revisar el diff. La comprobación
  `cargo xtask licenses --check` compara el inventario completo:
  nombre, versión, licencia, familia y totales.
- **Revisiones de los modelos auditadas.** Los seis modelos fijados (Qwen3-TTS,
  dos pares opus-mt, Parakeet y DPDFNet) se descargan con
  `ai-voice-interconnector setup` y no se empaquetan. Para incorporar una
  revisión nueva de alguno:
  1. consultar el `sha` vigente en `https://huggingface.co/api/models/<repo>`;
  2. auditar el diff de esa revisión en HuggingFace;
  3. verificarla con `setup` y `doctor`.
- **La publicación es irreversible.** El Release se publica directamente sobre
  el tag. La versión y el CHANGELOG deben estar correctos **antes** de crear el
  tag, porque corregirlos después exige borrar un Release público.

### Aprovisionamiento de los recursos antes de cortar

La puerta de pruebas con recursos exige FFmpeg y ONNX Runtime de los pines,
el motor TTS compilado y los seis modelos completos. Los cinco modelos
obligatorios bloquean el arranque del daemon; `dpdfnet8` es el sexto pin y es
opcional para el arranque, pero requerido por las superficies `--denoise` y
`speech denoise`.

**Ensayo del bootstrap desde cero.** `clean --all` no arranca sin binario del
producto: su capa app delega en `self uninstall`. Usa la siguiente secuencia para
probar la desinstalación y, después, arrancar el bootstrap realmente en frío:

```bash
cargo xtask clean --repo --dry-run             # revisar artefactos del repositorio
cargo xtask clean --repo --yes                 # primera limpieza; no necesita binario de producto
cargo build --package ai-voice-interconnector  # semilla para self uninstall
cargo xtask clean --app --dry-run              # revisar el plan de desinstalación
cargo xtask clean --app --yes                  # exigir resultado `uninstalled`, no aceptar `not_installed` como prueba
cargo xtask clean --repo --dry-run             # retirar también la semilla compilada
cargo xtask clean --repo --yes                 # deja target/ y bundles fuera para el cold start
cargo xtask ffmpeg                             # FFmpeg del pin, verificado por SHA-256
cargo xtask bootstrap --models --yes           # ORT, motor y seis modelos
```

La primera capa `--repo` puede compilar `xtask` para ejecutar el limpiador; su
propio ejecutable se retira al terminar. La compilación semilla posterior crea
el binario que necesita `--app`; la segunda capa `--repo` elimina los artefactos
del producto antes del aprovisionamiento. Así `ffmpeg` y `bootstrap` arrancan sin
el binario del producto, bundles nativos ni binario del motor. `xtask` deberá
recompilarse para ejecutar cada orden Cargo que lo invoque. Cierra
`rust-analyzer` y procesos del producto o de `xtask` antes de cada borrado de
repo. Al pedir `clean --repo --yes`, el propio `xtask.exe` puede quedar en un
borrado diferido hasta que el comando salga. En Windows, no continúes mientras
exista `target/` o `target.removal-failed.log`; identifica el proceso que retiene
el DLL y comprueba que la retirada terminó. En las pruebas de este plan, la
sesión del editor mantuvo `async_trait-*.dll` abierto a través de `rust-analyzer`:
cierra el editor, verifica que `target/` desapareció y vuelve a abrir la terminal
antes de seguir. Si `--app` o el borrado diferido fallan,
detente el procedimiento y resuelve la causa. Ninguno de estos comandos toca
`CARGO_HOME`, rustup, MSYS2 ni `CARGO_HOME/bin`.

**Preparación normal del gate.** Si no se ensaya desde cero, conserva recursos
ya verificados; ejecuta `cargo xtask ffmpeg` y
`cargo xtask bootstrap --models --yes` cuando falte o esté dañado algún requisito.

La descarga acota el silencio por dos vías: el cliente corta la lectura tras
sesenta segundos sin bytes y el vigilante de `bootstrap` aborta `setup` tras diez
minutos sin recibir el latido de bytes de `TransferObserver`. El latido viene del
avance de archivo HTTP y de los eventos agregados de xet: las muestras repetidas
sin bytes nuevos no lo renuevan. El avance se compara por archivo y por grupo
xet, sin mezclar contadores. No depende del tamaño del árbol, que puede quedar
estable porque hf-hub preextiende los blobs. El plazo es sin progreso, no total:
una transferencia lenta puede durar más de diez minutos si sigue informando bytes.
Un fallo al leer el stderr del hijo interrumpe la supervisión e intenta terminar
y recoger el proceso; si no se puede confirmar la terminación o la recogida, el
error lo indica. Un fallo al mostrar un diagnóstico no detiene la lectura de los
latidos.

**Nunca esperes a un `setup` que «parece lento».** La selección completa son
5,9 GB y a 30-60 MB/s se completa en minutos. Si el progreso queda mudo durante
el límite, el proceso falla explícitamente en vez de seguir esperando.

**Cómo se mide el avance, si hay que mirarlo.** El tamaño del directorio de la
caché **no** sirve como señal: `hf-hub` preextiende algunos ficheros y el total
puede quedarse plano durante minutos con la transferencia viva. Las señales
fiables son los bytes escritos por el proceso
(`(Get-CimInstance Win32_Process -Filter 'ProcessId=<pid>').WriteTransferCount`
en Windows) y el tamaño y la fecha de cada `blobs/*.incomplete`. Una sola muestra
no concluye nada: compara el mismo valor en dos muestras separadas por varios
minutos.

**Después de cualquier interrupción, verifica independientemente los recursos.**
`doctor` es el diagnóstico integral de una instalación y no la gate de recursos
de un bootstrap local. Ejecuta la comprobación específica de recursos:

```bash
cargo xtask verify-resources --json
```

Exige exit code 0 y un único objeto JSON con `ok: true`, los seis modelos
íntegros, bundles FFmpeg/ONNX Runtime verificados y sin artefactos incompletos.
En Windows, desde Git Bash, comprueba además que no queden cuerpos `.incomplete`:

```bash
find "$(cygpath -u "$LOCALAPPDATA")/ai-voice-interconnector/cache/models" -name '*.incomplete' | wc -l  # debe dar 0
```

Usa la raíz de modelos que informa la propia gate. Para validar la integración
real del usuario final, ejecuta después el recorrido de instalación publicada
y exige que `doctor` integral termine con exit code 0 desde una terminal nueva.

Si el estado es dudoso o quedan restos, repara mediante el producto:

```bash
cargo run -q -- setup --force-update --yes   # purga snapshots y caché xet, y re-descarga
```

Un `setup` normal repara un snapshot al que le faltan ficheros del contrato —un
pin sin sus pesos no cuenta como provisionado—; `--force-update` es la opción
para descartar todo el estado de modelos y empezar de nuevo.

`bootstrap --models` puede informar que falta `sccache` en el paso `doctor`; sin
`--system`, es una recomendación informativa. La condición de éxito es código 0
y que las verificaciones independientes confirmen todos los recursos.

## 1. Corte: crear y publicar el tag

El repo es **trunk-based sobre `main`**. Los cambios previos al bump se
commitean primero; `cargo xtask release` exige árbol limpio y commits desde el
último tag, luego modifica los portadores. El commit del bump es posterior.

```bash
cargo xtask release X.Y.Z   # puertas + bump de portadores; todavía no crea commit/tag
git add -A
git commit -m "release: vX.Y.Z"      # conventional-commits
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin main --tags           # sin --tags el tag no dispara build-all
```

Antes del commit/tag, revisa el diff, en especial el CHANGELOG. La fecha debe
aparecer en `## [X.Y.Z] — AAAA-MM-DD`; el índice debe apuntar al ancla de GitHub
y debe existir el enlace de comparación con el tag anterior. Después de
`cargo test --all`, no repitas `cargo xtask changelog --check` en los fuentes
locales: esa comprobación exige que `[No publicado]` haya desaparecido y se
reserva para el tag. El gate local `cargo xtask release` ya validó el CHANGELOG
promovido.

`cargo xtask release X.Y.Z` ejecuta estas puertas antes de mutar archivos:

1. La versión tiene formato `X.Y.Z`.
2. `THIRD-PARTY-LICENSES.md` está en sincronía con `Cargo.lock`.
3. `cargo fmt --all --check` y `cargo clippy --all-targets -- -D warnings` pasan.
4. El árbol está limpio y hay commits desde el último tag.
5. Pasa `cargo test --workspace --features full -- --include-ignored`, la
   puerta más lenta: incluye pruebas con modelos, motor e instalación real.
   Requiere recursos aprovisionados con `cargo xtask ffmpeg` y
   `cargo xtask bootstrap --models --yes`. Las pruebas de recursos de
   `avi-daemon/tests/golden.rs` consultan esa caché heredada en solo lectura;
   el caso HTTP que exige DPDFNet ausente corre en un target de integración con
   una caché temporal privada. La gate asegura
   `ort-bundle/` en la versión fijada y la impone con `ORT_DYLIB_PATH`, para que
   STT use el runtime incluido. Un fallo aborta antes del bump.

En el commit local del bump, `cargo test --all` usa el fixture de versión que el
propio `cargo xtask release` actualizó. La comprobación estricta
`cargo xtask changelog --check` corre en CI contra el commit del tag, cuya versión
`Cargo.toml` ya es la nueva y cuyo CHANGELOG promovido debe haber eliminado
`[No publicado]`.

Si se reproduce la puerta a mano, el entorno es obligatorio:

```bash
AVI_ALLOW_CWD_VENDOR=1 ORT_DYLIB_PATH=<ruta absoluta del repo>/ort-bundle/onnxruntime.dll \
  cargo test --workspace --features full -- --include-ignored
```

`AVI_ALLOW_CWD_VENDOR=1` permite a los hijos CLI resolver el motor en
`vendor/qwen3-tts/` sin una instalación. `ORT_DYLIB_PATH` debe ser absoluto:
los tests corren desde directorios de paquetes miembros donde una ruta relativa
a `ort-bundle/` no resuelve.

El bump versiona `src/main.rs`, `Cargo.toml`, `Cargo.lock`,
`tests/golden/cli_version.json` y `SOURCE-OFFER.md`; regenera
`THIRD-PARTY-LICENSES.md` y promueve `## [No publicado]` a la sección de versión
fechada. Las post-comprobaciones verifican licencias, `SOURCE-OFFER.md` y
CHANGELOG. Si una post-comprobación falla, los archivos ya se modificaron:
corrígelos de forma explícita o revierte únicamente esos cambios y vuelve a
validar. No avances al tag hasta que todas las comprobaciones pasen.

## 2. Automático: lo que hace el CI

`build-all` corre solo en tags `v*` (`branches: ignore: /.*/`). La justificación
del diseño y las cachés está en [docs/CI.md](CI.md), y la definición ejecutable
en `.circleci/config.yml`. Con el tag publicado, ejecuta sin intervención:

1. **Puertas.** Todas son requisito de los builds; si alguna falla, no se compila
   ni se publica.

   | Puerta | Qué comprueba |
   |---|---|
   | `test-linux`, `test-windows`, `test-macos` | `cargo nextest run --all` en cada SO nativo |
   | `coverage` | Cobertura de la suite |
   | `test-bootstrap-linux`, `test-bootstrap-windows`, `test-bootstrap-macos` | Suites de `tests/bootstrap/`: bats contra `openssl s_server` y Pester contra `Serve.ps1` |
   | `validate-licenses` | `SOURCE-OFFER.md` coincide con su render y el inventario de terceros con `Cargo.lock` |
   | `validate-changelog` | Sección `[X.Y.Z]`, entrada del índice/ancla, enlace de comparación, sin `TODO: curar` ni `[No publicado]` |

2. **Builds.** `build-windows-x64`, `build-linux-x64`, `build-linux-arm64` y
   `build-darwin-arm64`:
   - verifican versión del binario contra el tag y `voice list` con `default`;
   - compilan el motor TTS y ejecutan `--self-test`;
   - empaquetan con `cargo xtask package --expect-version ...`, descargan ORT
     validando SHA-256 y auditan dependencias/ABI del target;
   - ejecutan el smoke de instalación `self install --no-setup --no-modify-path`
     en sandbox y persisten el artefacto en el workspace.

   Los cuatro artefactos son Windows x64, Linux x64, Linux arm64 y macOS arm64.
   Se añaden `install.sh`, `install.ps1` y `SHA256SUMS.txt` al Release.

3. **`publish-release`** estampa la versión en ambos bootstrap, genera los
   checksums y publica las notas extraídas del CHANGELOG con los enlaces de la
   fuente (MPL-2.0 §3.2).
4. **`publish-metadata`** obtiene los checksums, actualiza el Cask de Homebrew y
   lo empuja al tap. Si el Cask no cambió, no empuja nada.

## 3. Verificación post-publicación

```bash
gh release view vX.Y.Z --json tagName,assets
```

Verifica que el Release `vX.Y.Z` esté publicado y marcado como *latest*:

- Tiene los **7 assets**: cuatro artefactos, dos bootstrap y
  `SHA256SUMS.txt`.
- Las notas coinciden con la sección `[X.Y.Z]` del CHANGELOG e incluyen el
  enlace al tarball fuente `archive/refs/tags/vX.Y.Z.tar.gz`.
- Opcionalmente, compara los hashes con los emitidos por los builds.

**Recuperación ante fallas:**

- Si falla una puerta/build y no hubo publicación, borra el tag local y remoto,
  corrige y vuelve a crear el tag sobre el commit corregido:
  `git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`.
- Si el Release publicado tiene un defecto, elimina también el Release con
  `gh release delete vX.Y.Z --yes` antes de borrar el tag y republicar.

**Cierre:** después de verificar el Release, cierra daemon, `rust-analyzer` y
otros procesos que retengan archivos. Revisa y ejecuta la capa repo oficial:

```bash
cargo xtask clean --repo --dry-run
cargo xtask clean --repo --yes
```

Comprueba que `target/` desapareció y que no queda
`target.removal-failed.log`. El corte es un commit directo en `main`, así que no
dispara `post-merge`; esta limpieza limita los artefactos acumulados al ciclo
entre releases.

## 4. Verificación del usuario final

El usuario final verifica la integridad de su descarga contra
`SHA256SUMS.txt`, publicado en el Release, con los comandos de
[Descargar binario pre-compilado](../README.md#descargar-binario-pre-compilado).
El modelo de amenaza y la nota sobre binarios sin firmar están en
[SECURITY.md](../SECURITY.md).
