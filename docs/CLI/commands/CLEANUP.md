## Recorrido

La investigación examinó la implementación completa del comando `cleanup` explorando tres fuentes principales: el handler CLI (`src/main.rs:319-326` `handle_cleanup`), el almacén `avi-store` (`crates/avi-store/src/lib.rs:675-703` `hf_cache_dir`, `xet_cache_dir`, `remove_hf_snapshot`, `remove_xet_cache`) y las constantes de salida (`crates/avi-core/src/exit_codes.rs`). Se complementó con `VoiceStore`/`SpeechStore` (`crates/avi-store/src/lib.rs`) y el helper que encadena `cleanup --all` durante la desinstalación (`src/main.rs:1387`).

---

## Respuestas a los objetivos

**Diseño de `cleanup`:** Es un comando de borrado quirúrgico con banderas de selección combinables (`--model`, `--voices`, `--synthetic-speech`, `--all`). Nunca toca la caché completa de HuggingFace ni datos de otros proyectos. Devuelve un `CleanupResult` (NamedTuple) para que `_uninstall_cleanup_data` pueda distinguir borrado exitoso de cancelación atómica.

**Implementación:** El handler `cmd_cleanup` (`cli.py:2157`) calcula los targets a partir de las banderas, filtra solo los que existen, imprime el listado, gestiona confirmación/dry-run, y ejecuta `shutil.rmtree` por cada ruta. No hay helpers auxiliares fuera de `_emit_cleanup_json`; toda la lógica vive en una sola función.

**Proceso de ejecución:** Resolución de banderas → construcción de lista de targets (con defensa en profundidad) → filtrado de existentes → impresión del listado → gate dry-run → confirmación interactiva (o `--yes`) → `shutil.rmtree` por ruta → emisión JSON condicional.

---

## Hallazgos por tema

### Definición CLI (parser)

El parser se define en `cli.py:2684-2705` con los siguientes argumentos:

| Argumento | Tipo | Descripción |
|---|---|---|
| `--model` | `store_true` | Elimina carpetas de caché HF de los dos repos del proyecto + modelo de traducción + modelo de transcripción |
| `--voices` | `store_true` | Elimina el directorio de voces de usuario (arrastra habla sintética de esas voces, excluye `default`) |
| `--synthetic-speech` | `store_true` | Elimina la raíz entera de habla sintética (`default` incluida) |
| `--all` | `store_true` | Equivale a `--model --voices --synthetic-speech` |
| `--dry-run` | `store_true` | Lista lo que se borraría sin borrar nada |
| `--yes`, `-y` | `store_true` | Omite la confirmación interactiva |
| `--json` | `store_true` | Emite JSON legible por máquina (requiere `--yes` o `--dry-run`) |

**Sin flags:** se muestra la ayuda del subcomando (`cli.py:2196-2204`). No se borra nada.

### Banderas de selección y sus interacciones

La resolución de banderas ocurre en `cli.py:2192-2194`:

```python
do_model = getattr(args, "model", False) or getattr(args, "all", False)
do_voices = getattr(args, "voices", False) or getattr(args, "all", False)
do_synthetic = getattr(args, "synthetic_speech", False) or getattr(args, "all", False)
```

`--all` activa las tres categorías. Las banderas individuales son independientes entre sí y se pueden combinar libremente.

### Qué se borra por cada flag

**`--model`** (`src/main.rs:319-326` + `crates/avi-store/src/lib.rs:675-703`) — borra snapshots HF pineados vía `hf_cache_dir()`/`xet_cache_dir()`:

1. **Snapshots HF** (`MODEL_REVISIONS` `crates/avi-store/src/lib.rs:381`): `models--Qwen--Qwen3-TTS-12Hz-0.6B-CustomVoice`, `models--Qwen--Qwen3-TTS-12Hz-0.6B-Base`, `models--istupakov--parakeet-tdt-0.6b-v3-onnx` (4 artefactos), `models--Helsinki-NLP--opus-mt-es-en`, `models--Helsinki-NLP--opus-mt-en-es` dentro de `hf_cache_dir()`
2. **Cache `xet`** (`xet_cache_dir()` `crates/avi-store/src/lib.rs:446`): `~/.cache/huggingface/xet` (`shard-cache` + `stage` + `.locks`) limpiado atómicamente con `hub`
3. **Índice de compatibilidad** (`data_dir()/models/<name>/manifest.json`): limpiado si existe
4. Daemon detenido graceful (`src/main.rs:1387`) y `daemon.pid` borrado antes del borrado

Cada ruta se valida con defensa en profundidad: solo se aceptan prefijos `models--Qwen--`, `models--istupakov--`, `models--Helsinki-NLP--`, `xet` derivados de `MODEL_REVISIONS` y `hf_cache_dir()`. Cualquier ruta inesperada no se borra.

**`--voices`** (`cli.py:2232-2253`) — borra dos cosas:

1. **Directorio de voces de usuario** (`voices.py:56-58`): `{data_root}/voices` — todo el directorio, no Voice individual
2. **Arrastre de habla sintética** (`cli.py:2240-2253`): para cada subdirectorio en `{data_root}/synthetic-speech/`, lo borra **excepto `default`** — voz de fábrica de solo lectura que `--voices` nunca toca

**`--synthetic-speech`** (`cli.py:2235-2239`) — borra:

1. **Raíz entera de habla sintética** (`synthetic_speech.py:28-30`): `{data_root}/synthetic-speech` — todas las locuciones, `default` incluida

### Interacción `--voices` / `--synthetic-speech`

La lógica de arrastre es condicional (`cli.py:2240`):

- Si `--synthetic-speech` está activo, se borra la raíz completa (rama `if do_synthetic`, línea 2235)
- Si solo `--voices` está activo (sin `--synthetic-speech`), se activa la rama `elif do_voices` (línea 2240) que itera los namespaces y borra cada uno excepto `default`

Esto garantiza que `--voices` nunca elimina la voz de fábrica `default`, incluso si tiene locuciones sintéticas asociadas.

### Modo dry-run

El gate de dry-run está en `cli.py:2266-2269`:

```python
if getattr(args, "dry_run", False):
    print("\n(dry-run) No se borró nada.", file=info_out)
    _emit_cleanup_json([p for p, _kind in existing])
    return CleanupResult([], False)
```

**Comportamiento:**
- Lista las rutas que existen y se borrarían
- No ejecuta `shutil.rmtree` en ninguna ruta
- Emite JSON con las rutas candidatas (no vacío)
- Retorna `CleanupResult([], False)` — `removed` vacío, `cancelled=False`

En modo `--json`, los listados informativos van a `stderr` (`cli.py:2173-2175`) para no contaminar el stdout reservado al JSON.

### Lógica de confirmación

La confirmación ocurre en `cli.py:2271-2282`:

1. Si `--yes` está activo: se omite la confirmación, se procede al borrado
2. Si no hay `--yes`: se muestra `input("\n¿Eliminar estas rutas? (s/n): ")`
3. Acepta: `s`, `si`, `sí`, `y`, `yes` (normalizado a minúsculas)
4. Cualquier otra respuesta: `"Cancelado: no se borró nada."` → `CleanupResult([], True)`
5. `EOFError` (stdin cerrado, típicamente subprocess sin `--yes`): misma cancelación, sin traceback (`cli.py:2275-2279`)

**Invariante:** `cancelled=True` solo cuando el usuario declinó. El camino "no hay nada que limpiar" y el dry-run **no** son cancelaciones (`cancelled=False`).

### Contrato JSON (`--json`)

El gate de validación está en `cli.py:2177-2183`:

```python
if json_mode and not (getattr(args, "yes", False) or getattr(args, "dry_run", False)):
    raise CliError(EXIT_INVALID_INPUT, "usage_error",
        "Error: cleanup --json requiere --yes o --dry-run ...")
```

**Razón:** la confirmación interactiva (`input()`) escribiría a stdout, contaminando el payload JSON.

**Payload emitido** por `_emit_cleanup_json` (`cli.py:2185-2190`):

```json
{
  "removed": ["path/to/dir1", "path/to/dir2"],
  "dry_run": true
}
```

Se inyecta `schema_version` automáticamente vía `emit_json` (`cli.py:69-78`). Exactamente un objeto JSON por invocación.

**Excepciones:**
- Sin flags de selección + `--json`: emite `usage` a stderr + JSON vacío (`cli.py:2199-2202`)
- Nada que limpiar + `--json`: emite JSON con `removed: []`

### Estructura de retorno: `CleanupResult`

Definido en `cli.py:2142-2154` como `NamedTuple`:

| Campo | Tipo | Significado |
|---|---|---|
| `removed` | `list` | Rutas efectivamente eliminadas (vacío en dry-run, cancelación, o "nada que limpiar") |
| `cancelled` | `bool` | `True` solo si el usuario declinó la confirmación interactiva |

Este tipo es el contrato interno para `_uninstall_cleanup_data` (`cli.py:1489`): con `cancelled=True`, la desinstalación aborta atómicamente sin borrar PATH ni binario.

### Manejo de errores

| Condición | Código exit | Razón | Fuente |
|---|---|---|---|
| `--json` sin `--yes` ni `--dry-run` | 2 | `usage_error` | `cli.py:2178-2183` |
| Ruta fuera del proyecto (defensa en profundidad) | — | `RuntimeError` | `cli.py:2212`, `cli.py:2218` |
| Sin flags de selección | 0 | Ayuda mostrada, nada borrado | `cli.py:2196-2204` |
| Nada que limpiar | 0 | Mensaje informativo | `cli.py:2257-2260` |
| Dry-run | 0 | Listado sin borrado | `cli.py:2266-2269` |
| Cancelación del usuario | 0 | `cancelled=True` | `cli.py:2280-2282` |
| `EOFError` en input | 0 | Cancelación por stdin cerrado | `cli.py:2275-2279` |

### Integración con `_uninstall_cleanup_data`

`_uninstall_cleanup_data` (`cli.py:1489-1536`) es el único consumidor interno de `cmd_cleanup`. Lo invoca con:

```python
cleanup_args = argparse.Namespace(
    model=False, voices=False, all=True,
    dry_run=False, yes=...,
    json=False, cleanup_parser=...
)
```

Siempre ejecuta `cleanup --all` (el primer borrado del orden unificado de desinstalación). Tras un cleanup no cancelado, elimina `data_root()` si quedó vacío (`cli.py:1530-1534`). Con `cancelled=True`, retorna `( [], True )` y la desinstalación completa aborta.

---

## Conclusiones

El comando `cleanup` implementa un borrado quirúrgico con tres categorías de datos (modelos, voces, habla sintética) que se pueden combinar libremente. Su diseño se distingue por: (1) la defensa en profundidad — cada ruta se valida antes de agregarla a targets, impidiendo borrados accidentales fuera del proyecto; (2) la distinción semántica entre `--voices` (arrastre parcial, preserva `default`) y `--synthetic-speech` (raíz completa, incluye `default`); (3) el contrato `CleanupResult` que permite al uninstall encadenado distinguir cancelación atómica de éxito; y (4) la separación estricta stdout/diagnóstico en modo `--json`, con el gate que impide `--json` sin `--yes` o `--dry-run`. La implementación es monocapa — toda la lógica vive en `cmd_cleanup` sin helpers auxiliares — lo que facilita el razonamiento sobre el estado y las transiciones del comando.
