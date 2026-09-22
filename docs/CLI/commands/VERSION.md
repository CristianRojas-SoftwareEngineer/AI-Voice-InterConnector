# `version`

Imprime el nombre y la versión del binario. Es el comando más simple de la
CLI: un solo path de ejecución, sin dependencias externas, sin parámetros
requeridos y sin ningún camino de error posible.

Implementación: `handle_version` (`src/main.rs:575-581`).

---

## Superficie CLI

```
ai-voice-interconnector version [--json]
ai-voice-interconnector [--json]              # sin subcomando: mismo handler
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

`version` es una variante sin campos del enum `Commands` (`src/main.rs:143`).
No tiene sub-subcomandos ni flags propios; `--json` es el flag global definido
en `struct Cli` (`src/main.rs:102-104`), compartido por todos los comandos.

Cuando la CLI se invoca **sin ningún subcomando**, `Cli::command` es `None` y
el despacho en `main` cae también en `handle_version(json_mode)`
(`src/main.rs:556`) — mismo comportamiento que invocar `version` explícitamente.

### Distinción con `-V`/`--version` de clap

`#[command(version = VERSION)]` en `struct Cli` (`src/main.rs:100`) habilita
además el flag estándar de clap `-V`/`--version`, generado automáticamente por
el framework. Ese flag es un mecanismo **distinto** del subcomando `version`:
imprime únicamente `{APP_NAME} {VERSION}` a stdout y termina el proceso vía la
salida propia de clap, sin soportar `--json` ni pasar por `handle_version`.

---

## Implementación: `handle_version`

`src/main.rs:575-581`:

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
  `ai-voice-interconnector 0.20.6`).
- Camino JSON: payload de dos claves (`name`, `version`) pasado a
  `emit_raw_json`, que inyecta `schema_version` automáticamente.
- Siempre retorna `Ok(())`: no hay ningún `CliError` posible en este handler.

## Fuente de la versión

`VERSION` y `APP_NAME` son constantes `&str` fijas en `src/main.rs:27-28`:

```rust
const VERSION: &str = "0.20.6";
const APP_NAME: &str = "ai-voice-interconnector";
```

- Literales de cadena, sin mecanismo dinámico (no usan `env!("CARGO_PKG_VERSION")`
  ni ningún build script).
- `Cargo.toml:3` fija `version = "0.20.6"` para el paquete — debe mantenerse
  sincronizado manualmente con la constante `VERSION`, ya que no hay
  generación automática que los enlace.

## Contrato `--json`

`emit_raw_json` (`crates/avi-core/src/json_emitter.rs:19-25`) serializa el
`Value` e inyecta `schema_version` vía `with_schema_version`
(`crates/avi-core/src/json_emitter.rs:7-17`), que usa
`SCHEMA_VERSION = "3"` (`crates/avi-core/src/json_emitter.rs:4`). Salida real:

```json
{
  "schema_version": "3",
  "name": "ai-voice-interconnector",
  "version": "0.20.6"
}
```

Fixture verificado por test: `tests/golden/cli_version.json`.

## Códigos de salida

`version` solo tiene camino de éxito: `ExitCode::Ok = 0`
(`ExitCode::Ok`, `crates/avi-core/src/exit_codes.rs:6`). No existe ningún
`CliError` que este handler pueda producir:

- `APP_NAME`/`VERSION` son literales, no pueden fallar.
- `println!`/`emit_raw_json` asumen stdout disponible (mismo supuesto que el
  resto de la CLI).

## Tests

| Test | Archivo:línea | Verificación |
|---|---|---|
| `version_coincide_con_fixture` | `tests/cli_golden.rs` | `ai-voice-interconnector --json version` produce exit `0` y el JSON coincide exactamente con `tests/golden/cli_version.json` |

---

## Ejemplos

```bash
ai-voice-interconnector version           # ai-voice-interconnector 0.20.6
ai-voice-interconnector --json version    # payload legible por máquina con schema_version
ai-voice-interconnector                   # sin subcomando: mismo handler que `version`
ai-voice-interconnector -V                # flag nativo de clap; sin --json, ruta distinta a handle_version
```
