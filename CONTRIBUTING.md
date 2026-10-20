# Guía de contribución

Gracias por tu interés en contribuir a AI Voice-InterConnector. Este documento describe el
flujo de desarrollo, los estándares del proyecto y cómo proponer cambios.

## Tabla de contenidos

- [Requisitos](#requisitos)
- [Configuración del entorno de desarrollo](#configuración-del-entorno-de-desarrollo)
  - [Hooks de git](#hooks-de-git-corehookspath-opt-in-por-clon)
  - [Worktrees](#worktrees)
  - [Higiene de `target/`](#higiene-de-target)
- [Tests](#tests)
  - [Cobertura](#cobertura)
  - [Smoke-tests de instaladores](#smoke-tests-de-instaladores)
- [Dependencias y lockfile](#dependencias-y-lockfile)
- [Compilación de binarios](#compilación-de-binarios)
- [Estilo y convenciones](#estilo-y-convenciones)
- [Flujo de Pull Request](#flujo-de-pull-request)
  - [Ramas](#ramas)
  - [Actualizar e integrar](#actualizar-e-integrar-siempre-merge-nunca-rebase)
  - [Tags y releases](#tags-y-releases)
  - [Pasos](#pasos)
- [Reporte de problemas](#reporte-de-problemas)

## Requisitos

La tabla única de requisitos vive en el código: comprueba tu host con
`cargo xtask doctor` (solo lectura; informa la deriva del entorno).

- Git. El proyecto es 100% Rust.

## Configuración del entorno de desarrollo

```bash
git clone https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector.git
cd AI-Voice-InterConnector

# Verificar toolchain Rust (fijado por rust-toolchain.toml)
rustc --version  # 1.96.0
cargo --version

# Poner el entorno al día: componentes de Rust, ONNX Runtime y motor TTS
cargo xtask bootstrap

# Comprobar los requisitos del host (solo lectura)
cargo xtask doctor

# Ejecutar el CLI desde el código fuente (sin instalar)
cargo run -- version
cargo run -- doctor
cargo run -- voice list
```

`bootstrap` es convergente: repetirlo cuando nada cambia no hace trabajo. Con
`--models` provisiona además los pesos, y `--system` es la única vía que ejecuta
los gestores del sistema (con `sudo` o UAC), así que solo se usa cuando se pide
de forma explícita.

El origen de las voces se explica en [docs/DESIGN.md](docs/DESIGN.md#modelo-de-voces-de-dos-niveles).

### Hooks de git (`core.hooksPath`, opt-in por clon)

Los hooks viven versionados en `.githooks/` y no se activan solos. Actívalos una vez por clon (y por cada worktree que los necesite):

```bash
git config core.hooksPath .githooks
```

- `pre-commit`: puertas de idioma, comentarios, pines y features; falla el commit si no pasan.
- `post-merge`: poda fina de `target/` tras integrar en `main`, solo si el merge tocó dependencias o toolchain y `target/` supera el umbral definido en la cabecera del hook. Es best-effort y nunca falla el merge.

### Worktrees

Cada worktree lleva su propia rama transitoria y resuelve su propia raíz con `git rev-parse --show-toplevel`, así que el hook y `xtask` operan por worktree. Para no multiplicar el coste en disco, cada worktree compila con su propio `target-dir` o comparte el principal por turnos (nunca dos builds concurrentes contra el mismo `target/`), y cada uno usa su propio `data-dir` y puertos del daemon.

### Higiene de `target/`

`target/` crece sin límite. La poda fina (`cargo xtask clean --prune`, con `--dry-run` para ver qué borraría) y su relación con `doctor` y el hook están descritas en [docs/specs/LIFECYCLE.md §9.6](docs/specs/LIFECYCLE.md#96-cargo-xtask-clean); los parámetros vigentes, con `cargo xtask clean --help`.

Los temporales de todo lo que lanza Cargo (pruebas, `cargo run`, `cargo xtask`) quedan confinados dentro de `target/`: `.cargo/config.toml` fija `TMP`, `TEMP` y `TMPDIR` a `target`, de modo que una prueba abortada no deja restos en el directorio temporal del sistema y `cargo clean` o `cargo xtask clean` los retiran con el resto. Los temporales de `xtask` llevan el prefijo `xtask-`. Si usas un intermediario del compilador (`RUSTC_WRAPPER`, como sccache), crea `target/` antes del primer comando de cargo en un checkout limpio o tras `cargo clean`: cargo lo invoca antes de crear `target/` y el intermediario aborta al no encontrar su temporal.

Las cachés globales de Cargo (`registry` y `git` bajo `CARGO_HOME`) y la caché de paquetes de pacman en Windows no forman parte de `--repo` ni de `--all`: se limpian solo con `cargo xtask clean --global` (usa `--dry-run` para verlas antes). `--global` nunca toca `CARGO_HOME/bin`, rustup, sccache ni la instalación de MSYS2. Si el borrado diferido de `target/` falla, deja `target.removal-failed.log` en la raíz del repositorio (ignorado por git); `cargo xtask clean` lo retira cuando `target/` ya no existe.

## Tests

La suite es **100% Rust** (`cargo test --all`, incluye los tests del tooling en
`crates/xtask`). Antes de abrir un PR, verifica:

```bash
cargo test --all --verbose          # tests (avi-core/audio/tts/stt/translation/store/daemon/shared/process/lifecycle/xtask/cli_golden)
cargo fmt --all --check
cargo clippy --all-targets

# Validación de licencias MPL-2.0 (gate de CI)
cargo xtask source-offer --check
cargo xtask licenses --check
```

- Añade tests para todo comportamiento nuevo o corregido (`#[test]` en el crate correspondiente).
- La suite se ejecuta en CI en **Linux**, **Windows** y **macOS** nativos (`test-linux`/`test-windows`/`test-macos`); evita supuestos de un SO.
- Verificación rápida de sintaxis Rust: `cargo check --all`.
- Las pruebas pesadas de inferencia TTS se serializan con el file-lock `lock_tts()` de `tests/cli_golden.rs`: cada residente ocupa varios GB de RAM, y quitar o debilitar el lock para paralelizar arriesga un OOM del runner. Que un test de esa serie parezca detenido más de 60 s es espera en cola y no un cuelgue: no se le añaden reintentos ni timeouts. El razonamiento completo está en los comentarios de `lock_tts()` en ese archivo.
- Las pruebas con recursos (`#[ignore = "requiere …"]`) no se omiten ni aprovisionan por su cuenta: con `--include-ignored` fallan con un mensaje que nombra el comando que aprovisiona lo que falta. No descargan modelos, porque el runner de CI no tiene el motor ni los pesos locales y la descarga costaría gigabytes en cada corrida sin aportar cobertura. `cargo xtask release` las ejecuta antes del bump.

### Cobertura

La cobertura es **opt-in** y usa `cargo-llvm-cov`. El job `coverage` de CI la mide:

```bash
cargo install cargo-llvm-cov --locked
cargo llvm-cov --workspace --lcov --output-path lcov.info
cargo llvm-cov --workspace --summary-only
```

No hay gate de porcentaje aún; el job valida que la instrumentación no rompa la suite.

### Smoke-tests de instaladores

Además de `cargo test`, los bootstrap tienen suites en `tests/bootstrap/`, que corren **en CI, no en `cargo test`**:

- `install.bats` — `packaging/bootstrap/install.sh` (Linux y macOS), con [bats-core](https://github.com/bats-core/bats-core) (`bats tests/bootstrap/install.bats`). Necesita `bats-core`, `openssl` y `curl`: el servidor falso es `openssl s_server` por HTTPS con una CA de prueba que genera la propia suite.
- `install.tests.ps1` — `packaging/bootstrap/install.ps1` (Windows), con **Pester v5** (`Invoke-Pester tests/bootstrap/install.tests.ps1 -CI`). Necesita `pwsh`; su servidor falso HTTP es `support/Serve.ps1`.

**Ejecución local.** La suite bats es para Linux y macOS: no corre en Git Bash de Windows. En una máquina Windows se ejecuta en WSL o en un contenedor Linux con el repositorio montado, por ejemplo (Ubuntu 24.04, bats-core en la versión del parámetro `bats_pin` de `.circleci/config.yml`):

```sh
docker run --rm -v "$PWD":/src:ro ubuntu:24.04 bash -c '
  apt-get update -qq && apt-get install -y -qq git curl openssl ca-certificates >/dev/null &&
  git clone -q --depth 1 --branch v1.14.0 https://github.com/bats-core/bats-core.git /tmp/bats &&
  /tmp/bats/install.sh /usr/local >/dev/null &&
  mkdir /work && cp -r /src/tests /src/packaging /work && cd /work && bats tests/bootstrap/install.bats'
```

En Git Bash de Windows, anteponer `MSYS_NO_PATHCONV=1` a `docker run` para que no reescriba las rutas del contenedor. La suite Pester corre en Windows con `pwsh -Command "Invoke-Pester tests/bootstrap/install.tests.ps1 -CI"`.

Todo `.ps1` del repositorio (bootstrap y suite) se escribe en ASCII puro y sin BOM: comentarios sin tildes y mensajes al usuario compuestos con `[char]`. `install.tests.ps1` lo verifica. Sus pruebas de tubería (`irm | iex`) necesitan `pwsh` en el PATH para no omitirse. La suite cubre también el entorno con `PSModulePath` heredado de PowerShell 7: la prueba construye esa contaminación a propósito (el directorio `Modules` de `pwsh` delante del `PSModulePath`), así que no depende del anfitrión que lance la suite.

Si modificas un bootstrap, actualiza su suite; los tres jobs (`test-bootstrap-*`) son puerta de los 4 builds en CI.

## Dependencias y lockfile

La fuente de verdad es `Cargo.toml` + `Cargo.lock` (workspace Rust). Tras modificar `Cargo.toml`:

```bash
cargo update          # regenera Cargo.lock
cargo test --all      # verifica
```

Revisa el diff de `Cargo.lock` antes de commitear. Si cambia, regenera el inventario de `THIRD-PARTY-LICENSES.md` con `cargo xtask licenses` y revisa su diff (ver su §Regeneración).

`THIRD-PARTY-LICENSES.md` y `SOURCE-OFFER.md` viajan dentro de los `tar.gz`/`.zip`; el gate `validate-licenses` falla si divergen.

## Compilación de binarios

Ver [docs/BUILD.md](docs/BUILD.md) para el detalle por plataforma. Resumen:

```bash
cargo build --release --features full   # binario completo (STT + traducción)
cargo build --release                   # featureless (rápido, sin C++)

./target/release/ai-voice-interconnector version
./target/release/ai-voice-interconnector voice list
./target/release/ai-voice-interconnector setup
```

El empaquetado de distribución lo hace `cargo xtask package` en los jobs de build de los tags `v*`; ver [docs/BUILD.md](docs/BUILD.md).

## Estilo y convenciones

- **Idioma**: identificadores de código en inglés; comentarios, documentación, mensajes de commit y textos de usuario en **español**, con ortografía correcta.
- **Comentarios**: explican el *porqué*, no el *qué*; sigue la densidad del código circundante.
- **Formato/lint**: `cargo fmt --all` y `cargo clippy --all-targets` deben pasar sin diff ni warnings nuevos. `cargo xtask release` lo verifica (`cargo fmt --all --check` y `cargo clippy --all-targets -- -D warnings`) y aborta el corte si no se cumple.
- **Commits**: mensajes descriptivos en español, prefijo de tipo cuando aplique (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `build:`), en imperativo.

## Flujo de Pull Request

### Ramas

`main` contiene siempre el estado vigente y todo cambio entra por ramas transitorias de vida corta; no hay ramas de desarrollo, release ni soporte. `main` no se commitea directamente salvo retoques triviales.

- Nombres: `fix/<sintoma>`, `feat/<capacidad>`, `hotfix/<asunto>` o `docs/<tema>`.
- Una rama es un cambio revisable y se elimina al integrarse.

### Actualizar e integrar (siempre merge, nunca rebase)

```bash
# Poner la rama al día con main
git checkout fix/mi-cambio
git merge main

# Integrarla en main con commit de merge explícito, para que el historial conserve qué commits agrupó cada flujo
git checkout main
git merge --no-ff fix/mi-cambio
git branch -d fix/mi-cambio
```

No se rebasea lo ya publicado: el rebase reescribe hashes y rompe la trazabilidad que el merge conserva.

### Tags y releases

Las releases se cortan con tags `v*` sobre `main`; el procedimiento completo, incluida la verificación posterior, está en [docs/RELEASING.md](docs/RELEASING.md).

### Pasos

1. Crea una rama transitoria a partir de `main`.
2. Implementa el cambio con sus tests y la actualización documental correspondiente (código, CI y docs sincronizados).
3. Verifica que `cargo test --all`, `cargo fmt --all --check` y `cargo clippy --all-targets` pasan.
4. Abre el PR describiendo problema, solución y cómo verificarla.
5. Enlaza el Issue si existe.

## Reporte de problemas

- **Bugs y solicitudes**: [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues).
- **Vulnerabilidades**: sigue [SECURITY.md](SECURITY.md) (no en Issue público).
