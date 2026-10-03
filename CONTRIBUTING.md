# Guía de contribución

Gracias por tu interés en contribuir a AI Voice InterConnector. Este documento describe el
flujo de desarrollo, los estándares del proyecto y cómo proponer cambios.

## Tabla de contenidos

- [Requisitos](#requisitos)
- [Configuración del entorno de desarrollo](#configuración-del-entorno-de-desarrollo)
- [Tests](#tests)
  - [Cobertura](#cobertura)
  - [Smoke-tests de instaladores](#smoke-tests-de-instaladores)
- [Dependencias y lockfile](#dependencias-y-lockfile)
- [Compilación de binarios](#compilación-de-binarios)
- [Estilo y convenciones](#estilo-y-convenciones)
- [Flujo de Pull Request](#flujo-de-pull-request)
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

La voz `default` está embebida en el binario (`crates/avi-store/assets/default/`); no requiere `src/`.

## Tests

La suite es **100% Rust** (`cargo test --all`, incluye los tests del tooling en
`crates/xtask`). Antes de abrir un PR, verifica:

```bash
cargo test --all --verbose          # tests (avi-core/audio/tts/stt/translation/store/daemon/shared/process/lifecycle/xtask/cli_golden)
cargo fmt --all --check
cargo clippy --all-targets

# Validación GPLv3 (gate de CI)
cargo xtask source-offer --check
cargo xtask licenses --check
```

- Añade tests para todo comportamiento nuevo o corregido (`#[test]` en el crate correspondiente).
- La suite se ejecuta en CI en **Linux**, **Windows** y **macOS** nativos (`test-linux`/`test-windows`/`test-macos`); evita supuestos de un SO.
- Verificación rápida de sintaxis Rust: `cargo check --all`.

### Cobertura

La cobertura es **opt-in** y usa `cargo-llvm-cov` (no `pytest-cov`). El job `coverage` de CI la mide:

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

Todo `.ps1` del repositorio (bootstrap y suite) se escribe en ASCII puro y sin BOM: comentarios sin tildes y mensajes al usuario compuestos con `[char]`. `install.tests.ps1` lo verifica. Sus pruebas de tubería (`irm | iex`) necesitan `pwsh` en el PATH para no omitirse.

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

El empaquetado de distribución (`tar.gz`/`.zip` con 4 docs GPLv3) lo hace el step `Preparar artefacto versionado` de `.circleci/config.yml` solo en tags `v*`.

## Estilo y convenciones

- **Idioma**: código, comentarios, mensajes de commit y documentación en **español**, con ortografía correcta.
- **Comentarios**: explican el *porqué*, no el *qué*; sigue la densidad del código circundante.
- **Formato/lint**: `cargo fmt --all` y `cargo clippy --all-targets` deben pasar sin diff ni warnings nuevos. `cargo xtask release` lo verifica (`cargo fmt --all --check` y `cargo clippy --all-targets -- -D warnings`) y aborta el corte si no se cumple.
- **Commits**: mensajes descriptivos en español, prefijo de tipo cuando aplique (`feat:`, `fix:`, `docs:`, `refactor:`, `test:`, `build:`), en imperativo.

## Flujo de Pull Request

1. Crea una rama a partir de `main`.
2. Implementa el cambio con sus tests y la actualización documental correspondiente (código, CI y docs sincronizados).
3. Verifica que `cargo test --all`, `cargo fmt --all --check` y `cargo clippy --all-targets` pasan.
4. Abre el PR describiendo problema, solución y cómo verificarla.
5. Enlaza el Issue si existe.

## Reporte de problemas

- **Bugs y solicitudes**: [Issues](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues).
- **Vulnerabilidades**: sigue [SECURITY.md](SECURITY.md) (no en Issue público).
