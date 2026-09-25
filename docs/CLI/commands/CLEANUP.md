# `cleanup`

Comando de borrado quirúrgico granular con banderas de selección combinables
(`--voices`, `--synthetic-speech`, `--model`, `--all` como unión, `--dry-run`,
`--yes/-y`). Nunca toca la caché completa de HuggingFace ni datos de otros
proyectos, y **nunca borra binario ni PATH** — solo `uninstall` lo hace.
`cleanup` sin flags → `InvalidInput` exit 2 `usage_error`. Devuelve payload
`--json` con `removed` + `dry_run`.

**Implementación:** el handler `handle_cleanup` calcula `do_voices`/`do_speech`/`do_model`
a partir de los flags, construye la lista de candidatas existentes (defensa en
profundidad por `MODEL_REVISIONS` + `hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()`
en `avi-store`), gestiona el gate `dry-run` (lista sin borrar), la confirmación
interactiva (`--yes` la omite), y ejecuta el borrado selectivo por branch.
`stop_daemon_and_resident()` (parada unificada con deadline global de 8 s:
graceful + árbol preciso + verificación) corre como paso 0.

**Proceso de ejecución:** gate `sin flags → 2` → resolución `--all` →
construcción de candidatas (filtrado de existentes) → gate `dry-run` →
confirmación (`--yes` o `y/yes/s/si/sí`) →
branches `remove_hf_snapshot`/`remove_xet_cache`/`remove_ct2_cache`/`remove_dir_all`
por categoría → emisión JSON `removed`/`dry_run`.

---

## Definición CLI (parser)

`Commands::Cleanup` (definición del subcomando) declara los siguientes argumentos:

| Argumento | Tipo | Descripción |
|---|---|---|
| `--voices` | `bool` | Elimina voces no-fábrica y arrastra `speech/<voz>` excepto `default` |
| `--synthetic-speech` | `bool` | Elimina la raíz entera `speech/` (`default` incluida) |
| `--model` | `bool` | Elimina snapshots HF pineados + `xet` + `ct2` + `data_dir()/models` legado |
| `--all` | `bool` | Unión de `--voices` + `--synthetic-speech` + `--model` (sin binario ni PATH) |
| `--dry-run` | `bool` | Lista lo que se borraría sin borrar nada (exit 0, con `removed`/`dry_run`) |
| `--yes`, `-y` | `bool` | Omite la confirmación interactiva |
| `--json` | `bool` | Global (`Cli::json`); con `cleanup` emite `status` + `removed` + `dry_run` |

Docstring: «limpieza granular; --all = unión de --voices/--synthetic-speech/--model, sin binario ni PATH».

**Sin flags:** `Err(InvalidInput, "usage_error", "cleanup requiere al menos un flag...")` exit 2. No se borra nada.

## Banderas de selección y sus interacciones

La resolución de `handle_cleanup`:

```rust
let do_voices = voices || all;
let do_speech = synthetic_speech || all;
let do_model = model || all;
```

`--all` activa las tres categorías. Las banderas individuales son
independientes y combinables. `--all` **no delega** en `handle_uninstall`
(despacho desacoplado; solo `Uninstall` toca
`windows_install_dir`/`remove_windows_user_path`/`spawn_uninstall_helper`).

## Qué se borra por cada flag

**`--model`** — borra vía `hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()` (`avi-store`):

1. **Snapshots HF** (`MODEL_REVISIONS`): `models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice`, `models--Qwen--Qwen3-TTS-12Hz-0.6B-Base`, `models--istupakov--parakeet-tdt-0.6b-v3-onnx`, `models--Helsinki-NLP--opus-mt-es-en`, `models--Helsinki-NLP--opus-mt-en-es` dentro de `hf_cache_dir()`
2. **Cache `xet`** (`xet_cache_dir()`): `~/.cache/huggingface/xet`, y los locks de descarga `hf_cache_dir()/.locks`
3. **Cache `ct2`** (`ct2_cache_dir()`): `hf_cache_dir()/ct2` (`ct2_model_dir` por par)
4. **Índice legado** (`data_dir()/models`): limpiado si existe
5. Daemon detenido con parada unificada (`stop_daemon_and_resident()`) y temp huérfano `avi_*`/`ai-voice-interconnector-install-*`

Cada ruta se filtra por existencia antes de borrar; `--model` nunca toca `voices/` ni `speech/`.

**`--voices`** — borra dos cosas:

1. **Voces no-fábrica** (`FACTORY_VOICES`): cada subdirectorio en `data_dir()/voices` excepto `default`/`ryan`/`vivian` (`is_factory_name`)
2. **Arrastre de habla sintética:** para cada voz borrada, `data_dir()/speech/<voz>` **excepto `default`** y solo si `!do_speech` (si `do_speech` ya borrará la raíz entera, evita duplicado)

**`--synthetic-speech`** — borra:

1. **Raíz entera** `data_dir()/speech` — todas las locuciones, `default` incluida

## Interacción `--voices` / `--synthetic-speech`

La lógica de arrastre es condicional:

- Si `--synthetic-speech` (o `--all`) está activo, se borra la raíz completa (no hay iteración por namespace)
- Si solo `--voices` está activo, se itera `voices/` y se arrastra cada `speech/<voz>` excepto `default`

Esto garantiza que `--voices` nunca elimina `default`, incluso con locuciones asociadas.

## Modo dry-run

El gate en `handle_cleanup`:

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

## Lógica de confirmación

La confirmación (mismo patrón que `handle_uninstall`):

1. Si `--yes`/`-y` está activo: se omite la confirmación
2. Si `--dry-run`: ya retornó antes (no hay confirmación)
3. Si `stdin.is_terminal()` es falso: se omite (no interactivo; procede sin preguntar, coherente con `handle_uninstall`)
4. Si hay TTY: `eprint!("¿Continuar? [y/N]: ")` + `read_line`; acepta `s`, `si`, `sí`, `y`, `yes` (case-insensitive)
5. Cualquier otra respuesta (incl. vacío, `n`, `no`) o `EOF`: `{"status":"cancelled"}` con `--json` o `Cancelado.` en humano, exit 0 sin borrar

**Invariante:** `cancelled`/`Cancelado` solo cuando el usuario declinó. `dry-run` y "nada que limpiar" no son cancelaciones (exit 0 con `removed`/`dry_run`).

## Contrato JSON (`--json`)

Payload (dry-run o real):

```json
{
  "status": "cleanup_complete",
  "removed": ["path/to/dir1", "path/to/dir2"],
  "dry_run": true
}
```

`schema_version="3"` lo inyecta `emit_raw_json`. Exactamente un objeto JSON por invocación.

**Casos:**
- `cancelled` por declinar confirmación: `{"status":"cancelled"}` exit 0
- Sin flags + `--json`: `{"error":"cleanup requiere al menos un flag...","reason":"usage_error"}` exit 2
- Nada que limpiar: `removed: []` con `dry_run:false` (o `true` en dry-run)

Nota de divergencia con el oráculo Python: el oráculo exigía `--json` con
`--yes`/`--dry-run` (exit 2 `usage_error`); Rust no impone ese gate — `--json`
solo, sin `--yes`, procede en no-TTY y pide confirmación en TTY sin
contaminar stdout (stderr para prompt, stdout para JSON).

## Manejo de errores

| Condición | Código exit | Razón |
|---|---|---|
| Sin flags de categoría | 2 | `usage_error` |
| Sin flags + `--json` | 2 | `usage_error` (mismo gate) |
| Nada que limpiar | 0 | `removed: []` |
| Dry-run | 0 | `dry_run:true` + `removed` candidatas |
| Cancelación del usuario | 0 | `cancelled` |
| `EOFError`/sin TTY | 0 | Cancelación o procede sin prompt |

## Integración con `handle_uninstall`

`handle_uninstall` es el **único** que borra binario y PATH. `cleanup --all`
no lo invoca; expande a los tres flags y borra solo datos. `uninstall`
reutiliza la parada unificada `stop_daemon_and_resident()` (deadline 8 s con
verificación) y luego borra `data_dir()` entero + snapshots
`MODEL_REVISIONS` + `xet` + `.locks` + `ct2` + temp + integración por SO
(`windows_install_dir`/`remove_windows_user_path`/`spawn_uninstall_helper` en
Windows, symlink/dir en Unix).

---

`cleanup` restablece el borrado granular del oráculo Python con semántica de
unión para `--all`, gates `sin flags→2` y `dry-run` sin side-effects,
confirmación `s/si/sí/y/yes` y payload `removed`/`dry_run`/`cancelled`. Se
distingue por: (1) defensa en profundidad por `MODEL_REVISIONS` +
`hf_cache_dir()`/`xet_cache_dir()`/`ct2_cache_dir()`; (2) distinción
`--voices` (arrastre parcial, preserva `default`/`ryan`/`vivian`) vs
`--synthetic-speech` (raíz completa); (3) `handle_cleanup` desacoplado de
`handle_uninstall` — `--all` no toca binario/PATH; (4)
`stop_daemon_and_resident()` compartido como paso 0. La implementación es
granular por branches, no monocapa, y `CONTRACT.md §11` es la fuente de
verdad del contrato.
