## Recorrido

La investigación examinó la implementación completa de `version` explorando cuatro fuentes principales: la definición del parser CLI (`cli.py:2745-2747`), el handler `cmd_version` (`cli.py:1035-1042`), la constante `__version__` en `__init__.py:14`, y la tabla de códigos de salida (`exit_codes.py`). Se leyeron en paralelo las implementaciones del handler, el helper `emit_json` (`cli.py:69-80`), el módulo de bootstrap (`bootstrap.py`), y los casos de prueba (`test_cli.py:340-360`, `test_cli.py:2549-2557`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `version`:** Es el comando más simple de la CLI — un solo path de ejecución sin dependencias externas, sin parámetros requeridos, y sin posibilidad de fallo. Su único option flag es `--json` para salida legible por máquina. No interactúa con el daemon, no carga modelos, y no requiere bootstrap más allá de la capa UTF-8.

**Implementación:** El handler `cmd_version` (line 1035) importa `__version__` vía import perezoso dentro del handler (no a nivel de módulo), garantizando que `--help` y otros comandos rápidos no arrastren dependencias pesadas. Emite JSON a través del helper `emit_json` que inyecta automáticamente `schema_version`.

**Proceso de ejecución:** Bootstrap UTF-8 → parseo de args → import de `__version__` → impresión a stdout (texto plano o JSON).

---

## Hallazgos por tema

### Definición CLI

`version_parser` se registra en `cli.py:2745-2747`:

```python
version_parser = subparsers.add_parser("version", help="Muestra la versión")
version_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
version_parser.set_defaults(func=cmd_version)
```

- Es un subparser directo bajo `subparsers` (no tiene sub-subcomandos).
- El único flag es `--json` (store_true, default False).
- `set_defaults(func=cmd_version)` vincula el handler sin lógica intermedia.

### Handler: cmd_version

`cmd_version` (`cli.py:1035-1042`):

```python
def cmd_version(args):
    """Muestra la versión de ai-voice-interconnector."""
    from . import __version__

    if getattr(args, "json", False):
        emit_json({"name": "ai-voice-interconnector", "version": __version__})
    else:
        print(f"ai-voice-interconnector {__version__}")
```

- Importa `__version__` localmente (no a nivel de módulo) — consistente con el patrón de imports perezosos del paquete.
- Usa `getattr(args, "json", False)` en vez de `args.json`, tolerante a attrs faltantes.
- Camino de texto plano: `ai-voice-interconnector {__version__}` a stdout.
- Camino JSON: payload de dos claves (`name`, `version`) pasado a `emit_json`.

### Fuente de la versión

`__init__.py:14`:

```python
__version__ = "0.10.0"
```

- Literal de cadena, sin mecanismo dinámico (no usa `importlib.metadata` ni `setuptools-scm`).
- `pyproject.toml:64` sincroniza la versión del paquete distribuible: `version = {attr = "ai_voice_interconnector.__version__"}`.
- El autor es `Cristián Rojas Arredondo` (`__init__.py:15`), licencia `GPL-3.0-or-later` (`__init__.py:16`).

### Payload JSON

El helper `emit_json` (`cli.py:69-80`) serializa el dict y le inyecta `schema_version` automáticamente:

```json
{
  "name": "ai-voice-interconnector",
  "version": "0.10.0",
  "schema_version": "3"
}
```

- `SCHEMA_VERSION = "3"` (`cli.py:66`) — contrato legible por máquina, campo aditivo.
- `emit_json` usa `payload.setdefault("schema_version", SCHEMA_VERSION)` — no sobrescribe si el caller ya la trae.
- Garantiza exactamente un objeto JSON por invocación (una sola llamada a `print(json.dumps(...))`).

### Códigos de salida

`version` solo tiene path de éxito — no lanza `CliError` en ninguna circunstancia. El código de salida implícito es `EXIT_OK = 0` (`exit_codes.py:23`).

No hay caminos de error posibles:
- `__version__` es un literal, no puede fallar.
- `getattr(args, "json", False)` es seguro ante attrs faltantes.
- `print()` y `emit_json()` asumen stdout disponible.

### Bootstrap

`main()` (`cli.py:2764-2766`) invoca `bootstrap.apply()` antes de construir el parser. Esto configura UTF-8 en stdout/stderr, silencia warnings selectivos, y configura variables de entorno para HuggingFace. Para `version`, el único efecto relevante es la reconfiguración UTF-8 — no hay imports pesados.

### Tests

| Test | Archivo:linea | Verificación |
|---|---|---|
| `test_cmd_version_human` | `test_cli.py:341-347` | Salida contiene `"ai-voice-interconnector"` |
| `test_cmd_version_json` | `test_cli.py:349-360` | JSON exacto: `{schema_version, name, version}` |
| `test_version_json_includes_schema_version` | `test_cli.py:2549-2557` | `schema_version == SCHEMA_VERSION` y `name == "ai-voice-interconnector"` |

Los tests usan `MockArgs(json=True)` y `capsys` de pytest para capturar stdout.

---

## Conclusiones

`version` es el comando más minimalista de la CLI: un handler de 7 líneas sin dependencias externas, sin interacción con el daemon, y sin caminos de error. Su diseño es notable por: (1) la coherencia con el patrón de imports perezosos — `__version__` se importa dentro del handler, no a nivel de módulo, evitando arrastre de dependencias incluso en este caso trivial; (2) la adherencia al contrato JSON global — emite `schema_version` automáticamente vía `emit_json`, igual que todos los comandos que aceptan `--json`; y (3) la sincronización de versión — `__version__` en `__init__.py` es la fuente única, referenciada tanto por el CLI como por `pyproject.toml` para el paquete distribuible.
