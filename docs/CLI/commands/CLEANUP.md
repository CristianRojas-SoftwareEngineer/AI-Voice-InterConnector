## Recorrido

La investigación examinó la implementación Rust del comando `cleanup` explorando tres fuentes principales: el parser CLI (`src/main.rs:132` `Commands::Cleanup` con 6 flags), el handler granular (`src/main.rs:1579` `handle_cleanup` con gates `sin flags→2`, `dry-run`, `yes`/confirmación y branches selectivos) y el despacho desacoplado (`src/main.rs:327` `Cleanup` → `handle_cleanup`, solo `Uninstall` toca binario/PATH). Se complementó con el almacén `avi-store` (`crates/avi-store/src/lib.rs:497` `hf_cache_dir`, `517` `xet_cache_dir`, `537` `ct2_cache_dir`, `745` `remove_hf_snapshot`/`remove_xet_cache`) y las constantes de salida (`crates/avi-core/src/exit_codes.rs`). Oráculo Python `cli.py:2684` se conserva solo como referencia histórica; la fuente vigente es Rust.

---

## Respuestas a los objetivos

**Diseño de `cleanup`:** Es un comando de borrado quirúrgico granular con banderas de selección combinables (`--voices`, `--synthetic-speech`, `--model`, `--all` como unión, `--dry-run`, `--yes/-y`). Nunca toca la caché completa de HuggingFace ni datos de otros proyectos, y **nunca borra binario ni PATH** — solo `uninstall` lo hace (`src/main.rs:318`). `cleanup` sin flags → `InvalidInput` exit 2 `usage_error` (`src/main.rs:1589`). Devuelve payload `--json` con `removed` + `dry_run` (`src/main.rs:1678`).

**Implementación:** El handler `handle_cleanup` (`src/main.rs:1579`) calcula `do_voices/do_speech/do_model` a partir de los flags (`src/main.rs:1596`), construye la lista de candidatas existentes (defensa en profundidad por `MODEL_REVISIONS` + `hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()`), gestiona gate `dry-run` (lista sin borrar), confirmación interactiva (`--yes` la omite), y ejecuta borrado selectivo por branch. `stop_daemon_and_resident()` (`src/main.rs:1847`) es el paso 0.

**Proceso de ejecución:** Gate `sin flags → 2` → resolución `--all` → construcción de candidatas (filtrado de existentes) → gate `dry-run` → confirmación (`--yes` o `y/yes/s/si/sí`) → branches `remove_hf_snapshot`/`remove_xet_cache`/`remove_ct2_cache`/`remove_dir_all` por categoría → emisión JSON `removed`/`dry_run`.

---

## Hallazgos por tema

### Definición CLI (parser)

El parser se define en `src/main.rs:132-146` con los siguientes argumentos:

| Argumento | Tipo | Descripción |
|---|---|---|
| `--voices` | `bool` | Elimina voces no-fábrica y arrastra `speech/<voz>` excepto `default` |
| `--synthetic-speech` | `bool` | Elimina la raíz entera `speech/` (`default` incluida) |
| `--model` | `bool` | Elimina snapshots HF pineados + `xet` + `ct2` + `data_dir()/models` legado |
| `--all` | `bool` | Unión de `--voices` + `--synthetic-speech` + `--model` (sin binario ni PATH) |
| `--dry-run` | `bool` | Lista lo que se borraría sin borrar nada (exit 0, con `removed`/`dry_run`) |
| `--yes`, `-y` | `bool` | Omite la confirmación interactiva |
| `--json` | `bool` | Global (`Cli::json`); con `cleanup` emite `status` + `removed` + `dry_run` |

Docstring: «limpieza granular; --all = unión de --voices/--synthetic-speech/--model, sin binario ni PATH» (`src/main.rs:132`).

**Sin flags:** `src/main.rs:1589` → `Err(InvalidInput, "usage_error", "cleanup requiere al menos un flag...")` exit 2. No se borra nada.

### Banderas de selección y sus interacciones

La resolución ocurre en `src/main.rs:1596-1598`:

```rust
let do_voices = voices || all;
let do_speech = synthetic_speech || all;
let do_model = model || all;
```

`--all` activa las tres categorías. Las banderas individuales son independientes y combinables. `--all` **no delega** en `handle_uninstall` (`src/main.rs:327` desacoplado; solo `Uninstall` toca `windows_install_dir`/`remove_windows_user_path`/`spawn_uninstall_helper`).

### Qué se borra por cada flag

**`--model`** (`src/main.rs:1603-1626`, `crates/avi-store/src/lib.rs:497-554,745-773`) — borra vía `hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()`:

1. **Snapshots HF** (`MODEL_REVISIONS` `crates/avi-store/src/lib.rs:432`): `models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice`, `models--Qwen--Qwen3-TTS-12Hz-0.6B-Base`, `models--istupakov--parakeet-tdt-0.6b-v3-onnx`, `models--Helsinki-NLP--opus-mt-es-en`, `models--Helsinki-NLP--opus-mt-en-es` dentro de `hf_cache_dir()`
2. **Cache `xet`** (`xet_cache_dir()` `crates/avi-store/src/lib.rs:517`): `~/.cache/huggingface/xet` + `.locks` limpiado atómicamente
3. **Cache `ct2`** (`ct2_cache_dir()` `crates/avi-store/src/lib.rs:537`): `hf_cache_dir()/ct2` (`ct2_model_dir` por par)
4. **Índice legado** (`data_dir()/models`): limpiado si existe
5. Daemon detenido graceful (`stop_daemon_and_resident()` `src/main.rs:1847`) y temp huérfano `avi_*`/`ai-voice-interconnector-install-*` (`src/main.rs:1659`)

Cada ruta se filtra por existencia antes de borrar; `--model` nunca toca `voices/` ni `speech/`.

**`--voices`** (`src/main.rs:1628-1650`) — borra dos cosas:

1. **Voces no-fábrica** (`FACTORY_VOICES` `crates/avi-store/src/lib.rs:16`): cada subdirectorio en `data_dir()/voices` excepto `default`/`ryan`/`vivian` (`is_factory_name`)
2. **Arrastre de habla sintética** (`src/main.rs:1640`): para cada voz borrada, `data_dir()/speech/<voz>` **excepto `default`** y solo si `!do_speech` (si `do_speech` ya borrará la raíz entera, evita duplicado)

**`--synthetic-speech`** (`src/main.rs:1652-1657`) — borra:

1. **Raíz entera** `data_dir()/speech` — todas las locuciones, `default` incluida

### Interacción `--voices` / `--synthetic-speech`

La lógica de arrastre es condicional (`src/main.rs:1640`):

- Si `--synthetic-speech` (o `--all`) está activo, se borra la raíz completa (no hay iteración por namespace)
- Si solo `--voices` está activo, se itera `voices/` y se arrastra cada `speech/<voz>` excepto `default`

Esto garantiza que `--voices` nunca elimina `default`, incluso con locuciones asociadas.

### Modo dry-run

El gate está en `src/main.rs:1678-1696`:

```rust
if dry_run {
    emit_raw_json(json!({"status":"cleanup_complete","removed":removed_display,"dry_run":true}));
    return Ok(());
}
```

**Comportamiento:**
- Lista las rutas candidatas existentes (filtradas por flag)
- No ejecuta `remove_dir_all`/`remove_hf_snapshot`/`remove_xet_cache`/`remove_ct2_cache`
- Emite JSON con `removed` (candidatas) + `dry_run:true`
- Retorna `Ok(())` exit 0; en modo humano imprime `Dry-run: se eliminarían N ruta(s):` o `Nada para limpiar (dry-run).`

En `--json`, los listados no contaminan stdout (payload único vía `emit_raw_json`).

### Lógica de confirmación

La confirmación ocurre en `src/main.rs:1699-1714` (patrón de `handle_uninstall`):

1. Si `--yes`/`-y` está activo: se omite la confirmación
2. Si `--dry-run`: ya retornó antes (no hay confirmación)
3. Si `stdin.is_terminal()` es falso: se omite (no interactivo; procede sin preguntar, coherente con `handle_uninstall`)
4. Si hay TTY: `eprint!("¿Continuar? [y/N]: ")` + `read_line`; acepta `s`, `si`, `sí`, `y`, `yes` (case-insensitive)
5. Cualquier otra respuesta (incl. vacío, `n`, `no`) o `EOF`: `{"status":"cancelled"}` con `--json` o `Cancelado.` en humano, exit 0 sin borrar

**Invariante:** `cancelled`/`Cancelado` solo cuando el usuario declinó. `dry-run` y "nada que limpiar" no son cancelaciones (exit 0 con `removed`/`dry_run`).

### Contrato JSON (`--json`)

Payload emitido en `src/main.rs:1679` (dry-run) y `src/main.rs:1820` (real):

```json
{
  "status": "cleanup_complete",
  "removed": ["path/to/dir1", "path/to/dir2"],
  "dry_run": true
}
```

`schema_version="3"` lo inyecta `emit_raw_json`. Exactamente un objeto JSON por invocación.

**Casos:**
- `cancelled` por declinar confirmación: `{"status":"cancelled"}` exit 0 (`src/main.rs:1707`)
- Sin flags + `--json`: `{"error":"cleanup requiere al menos un flag...","reason":"usage_error"}` exit 2 (`src/main.rs:1590`)
- Nada que limpiar: `removed: []` con `dry_run:false` (o `true` en dry-run)

Nota de divergencia con oráculo `cli.py:2177`: el oráculo exigía `--json` con `--yes`/`--dry-run` (exit 2 `usage_error`); Rust no impone ese gate — `--json` solo, sin `--yes`, procede en no-TTY y pide confirmación en TTY sin contaminar stdout (stderr para prompt, stdout para JSON).

### Manejo de errores

| Condición | Código exit | Razón | Fuente |
|---|---|---|---|
| Sin flags de categoría | 2 | `usage_error` | `src/main.rs:1589` |
| Sin flags + `--json` | 2 | `usage_error` | `src/main.rs:1589` (mismo gate) |
| Nada que limpiar | 0 | `removed: []` | `src/main.rs:1686`/`1820` |
| Dry-run | 0 | `dry_run:true` + `removed` candidatas | `src/main.rs:1678` |
| Cancelación del usuario | 0 | `cancelled` | `src/main.rs:1707` |
| `EOFError`/sin TTY | 0 | Cancelación o procede sin prompt | `src/main.rs:1699-1714` |

### Integración con `handle_uninstall`

`handle_uninstall` (`src/main.rs:1880`) es el **único** que borra binario y PATH. `cleanup --all` no lo invoca; expande a los tres flags y borra solo datos (`src/main.rs:327`). `uninstall` reutiliza `stop_daemon_and_resident()` y luego borra `data_dir()` entero + snapshots `MODEL_REVISIONS` + `xet` + temp + integración por SO (`windows_install_dir`/`remove_windows_user_path`/`spawn_uninstall_helper` en Windows, symlink/dir en Unix). Tolerancias del oráculo `CleanupResult`/`_uninstall_cleanup_data` no existen en Rust: `handle_cleanup` retorna `Result<(), CliError>` y `handle_uninstall` gestiona su propio flujo.

---

## Conclusiones

El comando `cleanup` Rust restablece el borrado granular del oráculo `7542962` con semántica de unión para `--all`, gates `sin flags→2` y `dry-run` sin side-effects, confirmación `s/si/sí/y/yes` y payload `removed`/`dry_run`/`cancelled`. Se distingue por: (1) defensa en profundidad por `MODEL_REVISIONS` + `hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()`; (2) distinción `--voices` (arrastre parcial, preserva `default`/`ryan`/`vivian`) vs `--synthetic-speech` (raíz completa); (3) `Handle_cleanup` desacoplado de `handle_uninstall` — `--all` no toca binario/PATH; (4) `stop_daemon_and_resident()` compartido como paso 0. La implementación es granular por branches, no monocapa, y el contrato `CONTRACT.md §11` es la fuente de verdad.
