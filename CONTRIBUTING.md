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

- **Rust 1.96.0** (ver `rust_version` en `.circleci/config.yml`; `rustup` recomendado).
- **Cargo** (con Rust).
- **CMake ≥ 3.20** + **pkg-config** (para `whisper-rs`/`ct2rs`).
- En Linux: `libasound2-dev` y `libclang-dev` (`sudo apt install libasound2-dev libclang-dev pkg-config cmake`).
- Git. El proyecto es 100% Rust: sin Python.

## Configuración del entorno de desarrollo

```bash
git clone https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector.git
cd AI-Voice-InterConnector

# Verificar toolchain Rust
rustc --version  # 1.96.0
cargo --version

# Ejecutar el CLI desde el código fuente (sin instalar)
cargo run -- version
cargo run -- doctor
cargo run -- voice list
```

La voz `default` está embebida en el binario (`crates/avi-store/assets/default/`); no requiere `src/` ni Python.

## Tests

La suite es **100% Rust** (`cargo test --all`, incluye los tests del tooling en
`crates/xtask`). Antes de abrir un PR, verifica:

```bash
cargo test --all --verbose          # tests (avi-core/audio/tts/store/daemon/xtask/cli_golden)
cargo fmt --all --check
cargo clippy --all-targets

# Validación GPLv3 (gate de CI)
cargo run -p xtask -- source-offer --check
cargo run -p xtask -- licenses --check
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

Además de `cargo test`, los one-liners tienen smoke-tests en `tests/installer/`, que corren **en CI, no en `cargo test`**:

- `install-linux.bats` — `install-linux.sh` (Linux), con [bats-core](https://github.com/bats-core/bats-core) (`bats tests/installer/install-linux.bats`).
- `install-macos.bats` — `install-macos.sh` (macOS), también con bats.
- `install-windows.tests.ps1` — `install-windows.ps1` (Windows), con **Pester v5** (`Invoke-Pester tests/installer/install-windows.tests.ps1 -CI`).

Si modificas un instalador, actualiza su smoke-test; los tres jobs (`test-installer-*`) son puerta de los 4 builds en CI.

## Dependencias y lockfile

La fuente de verdad es `Cargo.toml` + `Cargo.lock` (workspace Rust). Tras modificar `Cargo.toml`:

```bash
cargo update          # regenera Cargo.lock
cargo test --all      # verifica
```

Revisa el diff de `Cargo.lock` antes de commitear. Si cambian crates empaquetados, actualiza `THIRD-PARTY-LICENSES.md` (ver su §Regeneración; `cargo-license` o `cargo metadata`).

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
- **Formato/lint**: `cargo fmt --all` y `cargo clippy --all-targets` deben pasar sin diff ni warnings nuevos.
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
