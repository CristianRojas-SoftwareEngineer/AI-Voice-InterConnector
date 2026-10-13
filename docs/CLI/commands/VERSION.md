# `version`

Imprime el nombre y la versión del binario. Es el comando más simple de la
CLI: un solo path de ejecución, sin dependencias externas, sin parámetros
requeridos y sin ningún camino de error posible.

Implementación: `handle_version` (`src/main.rs`).

---

## Superficie CLI

```
ai-voice-interconnector version [--json]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

`version` es una variante sin campos del enum `Commands` (`src/main.rs`).
No tiene sub-subcomandos ni flags propios; `--json` es el flag global definido
en `struct Cli` (`src/main.rs`), compartido por todos los comandos.

La invocación **sin ningún subcomando** no es `version`: muestra la ayuda,
según la regla de ausencia de [CONTRACT §9](../CONTRACT.md#el-2-significa-lo-que-clap-quiere-decir-con-él).

### Distinción con `-V`/`--version` de clap

`#[command(version = VERSION)]` en `struct Cli` (`src/main.rs`) habilita
además el flag estándar de clap `-V`/`--version`, generado automáticamente por
el framework. Ese flag es un mecanismo **distinto** del subcomando `version`:
pasa por `render_clap_error` (`src/main.rs`): con `--json` emite el mismo
payload llamando a `handle_version`, y sin él imprime `{APP_NAME} {VERSION}`.

---

## Implementación: `handle_version`

`handle_version` (`src/main.rs`):

```rust
fn handle_version(json_mode: bool) -> Result<(), CliError> {
    if json_mode {
        emit_raw_json(json!({ "name": APP_NAME, "version": VERSION }));
    } else {
        println!("{} {}", APP_NAME, VERSION);
    }
    Ok(())
}
```

- Camino de texto plano: `{APP_NAME} {VERSION}` a stdout (p. ej.
  `ai-voice-interconnector X.Y.Z`).
- Camino JSON: payload de dos claves (`name`, `version`) pasado a
  `emit_raw_json`, que inyecta `schema_version` automáticamente.
- Siempre retorna `Ok(())`: no hay ningún `CliError` posible en este handler.

## Fuente de la versión

`VERSION` y `APP_NAME` son constantes `&str` fijas en `src/main.rs`:

```rust
const VERSION: &str = "X.Y.Z";
const APP_NAME: &str = "ai-voice-interconnector";
```

- Literales de cadena, sin mecanismo dinámico (no usan `env!("CARGO_PKG_VERSION")`
  ni ningún build script).
- `Cargo.toml` fija la misma versión en `[package]`. Ningún mecanismo de
  compilación los enlaza: los sincroniza `cargo xtask release X.Y.Z`,
  que reescribe a la vez `VERSION`, `Cargo.toml`, `Cargo.lock` y el fixture
  `tests/golden/cli_version.json` (ver [RELEASING.md](../../RELEASING.md)).

## Contrato `--json`

`emit_raw_json` (`crates/avi-core/src/json_emitter.rs`) serializa el
`Value` e inyecta `schema_version` vía `with_schema_version`
(`crates/avi-core/src/json_emitter.rs`), que usa
`CLI_SCHEMA_VERSION = "5"` (`crates/avi-core/src/json_emitter.rs`). Salida real:

```json
{
  "schema_version": "5",
  "name": "ai-voice-interconnector",
  "version": "X.Y.Z"
}
```

Fixture verificado por test: `tests/golden/cli_version.json`.

## Códigos de salida

`version` solo tiene camino de éxito: `ExitCode::Ok = 0`
(`ExitCode::Ok`, `crates/avi-core/src/exit_codes.rs`). No existe ningún
`CliError` que este handler pueda producir:

- `APP_NAME`/`VERSION` son literales, no pueden fallar.
- `println!`/`emit_raw_json` asumen stdout disponible (mismo supuesto que el
  resto de la CLI).

## Tests

| Test | Archivo:línea | Verificación |
|---|---|---|
| `version_matches_fixture` | `tests/cli_golden.rs` | `ai-voice-interconnector --json version` produce exit `0` y el JSON coincide exactamente con `tests/golden/cli_version.json` |
| `json_version_flags_match_version_command` | `tests/cli_golden.rs` | `--json --version` y `--json -V` emiten exactamente el mismo objeto que `--json version` |

---

## Ejemplos

```bash
ai-voice-interconnector version           # ai-voice-interconnector X.Y.Z
ai-voice-interconnector --json version    # payload legible por máquina con schema_version
ai-voice-interconnector -V                # flag nativo de clap: imprime `<nombre> <versión>`
ai-voice-interconnector --json -V         # mismo payload que `--json version`
```
