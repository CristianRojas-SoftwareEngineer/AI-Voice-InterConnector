## Recorrido

La investigación examinó la implementación completa de `doctor` explorando tres zonas del código: la definición del parser CLI (`cli.py:2653-2656`), la función handler `cmd_doctor` y sus helpers (`cli.py:1127-1400`), y el mecanismo de retorno entero en `main()` (`cli.py:2783-2804`). Se leyeron también los códigos de salida (`exit_codes.py`) y las firmas de los módulos auxiliares invocados (`model_cache.py`, `voices.py`, `translation/service.py`, `transcription/service.py`, `audio.py`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `doctor`:** Es un comando de diagnóstico sin subcomandos que ejecuta una batería de chequeos de entorno, modelos y plataforma. Emite un reporte (texto o JSON) y devuelve exit 1 si algún chequeo falló, sin que esto constituya un error de ejecución — es un **veredicto**, no una excepción.

**Implementación:** `cmd_doctor` (`cli.py:1274`) compone una lista de tuplas `(status, name, detail)` invocando helpers especializados. El handler nunca lanza excepciones; cada chequeo está envuelto en `try/except` y degrada a SKIP o FAIL interno si falla. El retorno es `None` (éxito) o un entero `EXIT_ERROR` (1) que `main()` interpreta como `sys.exit(result)`.

**Proceso de ejecución:** Acumulación de chequeos en lista mutable → conteo de FAIL/PASS → emisión de payload (texto o JSON) → retorno entero para veredicto.

**Patrón de veredicto:** `doctor` es el único comando del CLI que usa el mecanismo de retorno entero de `main()`. No genera `CliError`; en su lugar, retorna `EXIT_ERROR` (1) cuando hay fallos y `main()` lo traduce a `sys.exit(1)` sin imprimir nada adicional. Esto permite que `--json` emita un único objeto JSON limpio sin contaminar stderr.

---

## Hallazgos por tema

### Definición del parser CLI

`cli.py:2653-2656` — registro del subcomando `doctor`:

```python
doctor_parser = subparsers.add_parser("doctor", help="Ejecuta diagnósticos")
doctor_parser.add_argument("--json", action="store_true", help="Emitir JSON legible por máquina")
doctor_parser.set_defaults(func=cmd_doctor)
```

- **Subcomandos:** ninguno. Es un comando terminal (leaf command).
- **Opciones:** solo `--json` (flag booleano).
- **Handler:** `cmd_doctor` se vincula vía `set_defaults(func=...)`.

### Handler: `cmd_doctor`

Ubicación: `cli.py:1274-1400`.

El handler ejecuta este flujo:

```
checks = _environment_checks()          ← 2 chequeos base (Chatterbox + Audio)
checks += modelo Chatterbox (es + en)   ← 2 chequeos por idioma
checks += modelo de traducción           ← 1 chequeo par es<->en
checks += modelo de transcripción        ← 1 chequeo
checks += directorio de voces            ← 1 chequeo
checks += RAM (advisory)                 ← 1 chequeo
checks += AVX2 (advisory)               ← 1 chequeo
checks += OneDrive (advisory)            ← 1 chequeo

if --json → emit_json({...}) + return EXIT_ERROR si hay FAIL
else      → print reporte + return EXIT_ERROR si hay FAIL
```

### Chequeos de entorno base: `_environment_checks`

Ubicación: `cli.py:1127-1174`.

Función compartida con `setup`. Devuelve la primera tanda de chequeos:

| # | Check | Fuente | Éxito | Fallo |
|---|---|---|---|---|
| 1 | **Chatterbox TTS** | `import chatterbox` → `chatterbox.__version__` | PASS + versión | FAIL: "NO INSTALADO" |
| 2 | **Audio library** | `get_audio_devices_with_status()` (`audio.py:184`) | PASS: nombre lib + # dispositivos | FAIL: import faltante, sin dispositivos, o excepción |

El chequeo de audio usa la enumeración real de dispositivos (no solo el import), reflejando el estado efectivo del subsistema: un host headless/RDP importa la librería sin error pero falla al enumerar → FAIL con detalle específico por plataforma (`cli.py:1153-1165`).

### Chequeo de modelo Chatterbox

Ubicación: `cli.py:1282-1293`.

Itera sobre `(("es-latam", "es-mx-latam"), ("en", "en"))` y llama a `is_model_cached(model)` (`model_cache.py:166`) para cada par:

| Idioma | Modelo | Éxito | Fallo |
|---|---|---|---|
| `es-latam` | `es-mx-latam` | PASS: "{model} presente en la caché" | FAIL: "{model} no está en caché (ejecuta: tts-sidecar setup --language {lang})" |
| `en` | `en` | PASS: "{model} presente en la caché" | FAIL: "{model} no está en caché (ejecuta: tts-sidecar setup --language {lang})" |

Genera **2 chequeos** (uno por idioma), no uno consolidado.

### Chequeo de modelo de traducción

Ubicación: `cli.py:1295-1312`.

Verifica la presencia de los dos modelos CT2 (opus-mt) que se provisionan juntos:

```python
missing = [
    f"{source}->{target}" for source, target in (("es", "en"), ("en", "es"))
    if not Path(default_cache_dir(source, target)).exists()
]
```

| Condición | Resultado |
|---|---|
| Ambas direcciones presentes (`es→en` y `en→es`) | PASS: "opus-mt presente en la caché" |
| Falta una o ambas | FAIL: "falta(n) {lista} (ejecuta: tts-sidecar setup --language en)" |
| Excepción | FAIL con mensaje de error |

Es un **único chequeo lógico** — las dos direcciones se agrupan porque se provisionan juntas en `setup --language en/all` (`cli.py:1295-1297`).

### Chequeo de modelo de transcripción

Ubicación: `cli.py:1314-1326`.

Verifica la existencia del directorio de caché de `faster-whisper-small`:

```python
from .transcription import default_cache_dir as transcription_cache_dir
if Path(transcription_cache_dir()).exists():
    checks.append(("PASS", ...))
```

| Condición | Resultado |
|---|---|
| Directorio existe | PASS: "faster-whisper-small presente en la caché" |
| Directorio no existe | FAIL: "falta faster-whisper-small (ejecuta: tts-sidecar setup --with-stt)" |
| Excepción | FAIL con mensaje de error |

### Chequeo de directorio de voces

Ubicación: `cli.py:1328-1334`.

```python
voices_path = voices.voices_root()
count = len(voices.list_voices())
if os.path.exists(voices_path) or count:
    checks.append(("PASS", "Voices directory", f"{count} voz(voces) disponible(s)"))
else:
    checks.append(("SKIP", "Voices directory", "sin voces de usuario aún (opcional)"))
```

Es el único chequeo que puede retornar **SKIP** como valor normal (no por excepción). Las voces de usuario son opcionales — no having them is not a failure.

### Chequeo de RAM (advisory)

Ubicación: `cli.py:1336-1353`.

Usa `psutil.virtual_memory().total` y compara contra `RECOMMENDED_RAM_BYTES = 8 GB` (`cli.py:103`):

| Condición | Resultado |
|---|---|
| RAM ≥ 8 GB | PASS: "{X.X} GB" |
| RAM < 8 GB | **WARN**: "{X.X} GB detectados; se recomiendan 8 GB..." |
| `psutil` no disponible o error | **SKIP**: "no se pudo determinar ({error})" |

**Clave:** WARN y SKIP **no cuentan como fallo** — solo FAIL altera el exit code (`cli.py:1368-1369`).

### Chequeo de AVX2 (advisory)

Ubicación: `cli.py:1177-1221` (`_check_avx2`).

Detección best-effort sin dependencias nuevas:

| Plataforma | Método | Éxito | Fallo |
|---|---|---|---|
| ARM (aarch64, arm64) | `platform.machine()` | SKIP: "no aplica en {machine}" | — |
| Linux x86-64 | `/proc/cpuinfo` | PASS: "soportado" si "avx2" está en flags | WARN: "no detectado en /proc/cpuinfo" |
| macOS x86-64 | `sysctl -n machdep.cpu.leaf7_features` | PASS: "soportado" si "AVX2" en output | WARN: "no detectado" |
| Windows x86-64 | Sin vía estándar en stdlib | SKIP: "no verificable automáticamente en Windows" | — |
| Cualquier error | `except Exception` | SKIP: "no se pudo determinar ({error})" | — |

WARN y SKIP **no alteran el exit code**.

### Chequeo de OneDrive (advisory)

Ubicación: `cli.py:1224-1271` (`_check_onedrive`).

Verifica si `data_root()` (`paths.data_root()`) cae bajo la sincronización de OneDrive en Windows:

| Condición | Resultado |
|---|---|
| Fuera de Windows | SKIP: "no aplica fuera de Windows" |
| `data_root()` bajo raíz OneDrive (env vars `OneDrive` / `OneDriveCommercial`) | **WARN**: exponer riesgo de file locks y placeholders |
| `data_root()` contiene "onedrive" en la ruta (perfil corporativo sin env vars) | **WARN**: riesgo potencial |
| Ninguna condición | PASS: "no detectado" |

WARN **no altera el exit code**. Es puramente advisory (`cli.py:1362-1365`).

### Patrón de veredicto (exit 1 sin error)

Este es el diseño más distintivo de `doctor`. Dos mecanismos cooperan:

**1. `cmd_doctor` retorna un entero** (`cli.py:1380-1384`, `cli.py:1396-1399`):

```python
if checks_failed > 0:
    return EXIT_ERROR  # 1
```

Esto es diferente a todos los demás comandos, que retornan `None` (éxito implícito) o lanzan `CliError`.

**2. `main()` interpreta el retorno entero** (`cli.py:2798-2804`):

```python
else:
    if isinstance(result, int) and result != 0:
        sys.exit(result)
```

El comentario en `cli.py:2799-2802` lo explica explícitamente:

> "Salida por veredicto: el comando ya emitió su payload propio y pide salir con código ≠ 0 devolviendo un entero (p. ej. 'doctor' con FAIL). main() sigue siendo el único punto de salida no-cero; no se adjunta objeto 'error'."

**Diferencia con `CliError`:** Cuando un comando lanza `CliError`, `_translate_cli_error` imprime el mensaje a stderr y adjunta un objeto JSON `{"error": {...}}`. Cuando `doctor` retorna `EXIT_ERROR`, `main()` solo hace `sys.exit(1)` — no imprime nada, no adjunta JSON, porque el reporte ya se emitió como payload propio.

### Contrato JSON de `--json`

Cuando se pasa `--json`, `cmd_doctor` emite un único objeto JSON vía `emit_json()` (`cli.py:69-80`):

```json
{
  "schema_version": "3",
  "python": "3.x.x ...",
  "platform": "Windows 10",
  "checks": [
    {"status": "PASS", "name": "Chatterbox TTS", "detail": "0.x.x"},
    {"status": "FAIL", "name": "Chatterbox model (es-latam)", "detail": "es-mx-latam no está en caché..."},
    ...
  ],
  "passed": 7,
  "failed": 2
}
```

| Campo | Tipo | Descripción |
|---|---|---|
| `schema_version` | string | `"3"` — inyectado automáticamente por `emit_json` |
| `python` | string | `sys.version` completo |
| `platform` | string | `"{system} {release}"` |
| `checks` | array | Cada elemento: `{status, name, detail}` |
| `passed` | int | Conteo de chequeos con `status == "PASS"` |
| `failed` | int | Conteo de chequeos con `status == "FAIL"` |

**Nota:** WARN y SKIP no se cuentan en `passed` ni en `failed` — solo se cuentan PASS y FAIL.

### Códigos de salida

Ubicación: `exit_codes.py:1-51`.

`doctor` solo usa dos códigos:

| Código | Constante | Uso en doctor |
|---|---|---|
| `0` | `EXIT_OK` | Todos los chequeos PASS o con WARN/SKIP (sin FAIL) |
| `1` | `EXIT_ERROR` | Al menos un chequeo FAIL |

El comentario en `exit_codes.py:10` confirma: "1 error genérico (incluye chequeos fallidos de doctor)".

### Tabla resumen de todos los chequeos

| # | Nombre | Helper | Tipos posibles | Altera exit code |
|---|---|---|---|---|
| 1 | Chatterbox TTS | `_environment_checks` (`cli.py:1136-1140`) | PASS / FAIL | Sí (FAIL) |
| 2 | Audio library | `_environment_checks` (`cli.py:1147-1172`) | PASS / FAIL | Sí (FAIL) |
| 3 | Chatterbox model (es-latam) | `cmd_doctor` (`cli.py:1282-1293`) | PASS / FAIL | Sí (FAIL) |
| 4 | Chatterbox model (en) | `cmd_doctor` (`cli.py:1282-1293`) | PASS / FAIL | Sí (FAIL) |
| 5 | Translation model (es↔en) | `cmd_doctor` (`cli.py:1298-1312`) | PASS / FAIL | Sí (FAIL) |
| 6 | Transcription model (whisper-small) | `cmd_doctor` (`cli.py:1316-1326`) | PASS / FAIL | Sí (FAIL) |
| 7 | Voices directory | `cmd_doctor` (`cli.py:1329-1334`) | PASS / SKIP | No |
| 8 | RAM | `cmd_doctor` (`cli.py:1339-1353`) | PASS / WARN / SKIP | No |
| 9 | CPU AVX2 | `_check_avx2` (`cli.py:1177-1221`) | PASS / WARN / SKIP | No |
| 10 | OneDrive user-data-dir | `_check_onedrive` (`cli.py:1224-1271`) | PASS / WARN / SKIP | No |

---

## Conclusiones

`doctor` es un comando de diagnóstico estático que ejecuta 10 chequeos sin modificar el sistema (no descarga, no instala, no inicia procesos). Su diseño se distingue por tres aspectos:

1. **Patrón de veredicto:** es el único comando del CLI que retorna un entero (`EXIT_ERROR`) en vez de lanzar `CliError`. `main()` detecta el retorno entero via `isinstance(result, int)` y ejecuta `sys.exit(result)` sin imprimir ni adjuntar un objeto de error. Esto permite que el reporte (texto o JSON) sea la única salida, limpio y sin contaminación de stderr — un diseño intencional para que orquestadores consuman el JSON y distingan entre "fallo de ejecución" (excepción) y "veredicto negativo" (chequeos fallidos).

2. **Separación FAIL/WARN/SKIP:** solo FAIL cuenta como fallo. Los chequeos advisory (RAM, AVX2, OneDrive) retornan WARN y son puramente informativos — reflejan la filosofía de que el sistema puede funcionar con RAM baja o sin AVX2 verificable, pero el usuario debe tener visibilidad. SKIP se usa para plataformas donde un chequeo no aplica.

3. **Composición modular:** `_environment_checks` (`cli.py:1127`) es compartida con `setup`, evitando duplicación. Cada chequeo de modelo delega a un módulo especializado (`model_cache`, `translation`, `transcription`, `voices`) sin importar su lógica interna — doctor solo verifica existencia en caché, nunca carga ni descarga.
