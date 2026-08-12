## Recorrido

La investigación examinó la implementación completa de `setup` explorando tres fuentes principales: el parser CLI (`cli.py:2659-2681`), el handler principal `cmd_setup` (`cli.py:1871-2136`), y el módulo de caché de modelos (`model_cache.py`). Se leyeron en paralelo las implementaciones del handler, los helpers de integración de PATH (`_integrate_linux_path`, `_remove_linux_path`, `_path_symlink`), las tres ramas de desinstalación por SO (`_uninstall_linux`, `_uninstall_macos`, `_uninstall_windows`), la función de conversión de modelos de traducción (`_convert_translation_model`), el clasificador de fallos de provisión (`_describe_provision_failure`), los chequeos de entorno compartidos con `doctor` (`_environment_checks`), y las constantes de salida (`exit_codes.py`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `setup`:** Es un comando de provisión idempotente con tres modos mutuamente excluyentes (`--remove-path`, `--force-update`, `--uninstall`) que cortan el flujo normal. El flujo normal ejecuta: integración de PATH (solo Linux AppImage) → chequeos de entorno (FAIL de audio degrado a WARN) → descarga condicional de modelos TTS por idioma → descarga de modelos de traducción (si `--language` incluye `en`/`all`) → descarga opt-in de modelo de transcripción (`--with-stt`) → limpieza de descargas parciales huérfanas. Cada modo exclusivo implementa una operación destructiva específica sin ejecutar provisión.

**Implementación:** `cmd_setup` (linea 1871) despacha los tres modos exclusivos antes de cualquier lógica de provisión. El flujo normal delega en `model_cache.py` para inspección de caché y en `huggingface_hub.snapshot_download` para descargas con revisión fijada. `MODEL_ALLOW_PATTERNS` (definido en `model_cache.py`) filtra los archivos descargados a los checkpoints de inferencia necesarios, evitando descargar variantes no usadas (~10 GB menos para el modelo `en`). La descarga de modelos de traducción usa `snapshot_download` + conversión CT2 vía `ctranslate2.TransformersConverter`. El modelo de transcripción (`faster-whisper-small`) se descarga sin conversión. El manejo de errores clasifica las excepciones en familias (credenciales, red, permisos, disco) con códigos de salida accionables.

**Proceso de ejecución:** Impresión de banner → integración de PATH (Linux) → chequeos de entorno → resolución de modelos a descargar según `--language` → pre-chequeo de espacio en disco (6 GB/modelo) → descarga por `snapshot_download` con revisión fijada y `allow_patterns` (solo checkpoints de inferencia) → descarga de Voice Encoder si `es-mx-latam` fue provisionado → descarga de modelos de traducción (si aplica) → descarga de modelo de transcripción (si `--with-stt`) → limpieza de `.incomplete` huérfanos → emisión JSON (si `--json`).

---

## Hallazgos por tema

### Definición CLI

El parser de `setup` se define en `cli.py:2659-2681` como subcomando de nivel superior:

| Parámetro | Tipo | Default | Descripción |
|---|---|---|---|
| `--remove-path` | flag (exclusive) | False | Elimina el symlink `~/.local/bin/ai-voice-interconnector` y termina sin correr chequeos |
| `--force-update` | flag (exclusive) | False | Borra ambos modelos en caché y los vuelve a descargar |
| `--uninstall` | flag (exclusive) | False | Desinstala en un paso: encadena `cleanup --all`, revierte PATH, borra binario |
| `--yes, -y` | flag | False | Omite la confirmación interactiva del cleanup encadenado por `--uninstall` |
| `--language` | `es-latam` \| `en` \| `all` | `all` | Idioma(s) a provisionar; `all` descarga ambos modelos |
| `--with-stt` | flag | False | Provisiona el modelo de transcripción `faster-whisper-small` (opt-in) |
| `--json` | flag | False | Emite JSON legible por máquina en stdout |

Los tres flags de modo (`--remove-path`, `--force-update`, `--uninstall`) están en un `add_mutually_exclusive_group()` (`cli.py:2663`), lo que garantiza que solo uno puede activarse por invocación. El handler se asigna via `set_defaults(func=cmd_setup)` en linea 2681.

### Flujo normal de provisión

`cmd_setup` ejecuta este flujo cuando ninguno de los tres modos exclusivos está activo (`cli.py:1903-2132`):

```
Banner "=== AI Voice InterConnector Setup ==="
    │
    ▼
_integrate_linux_path()                    ← solo Linux con $APPIMAGE; no-op en otros SO
    │
    ▼
_environment_checks()                      ← compartido con doctor
    │  FAIL "Audio library" → degrado a WARN (no aborta)
    │  Cualquier otro FAIL → aborta con EXIT_PRECONDITION_FAILED (8)
    ▼
Resolución de modelos a provisionar
    │  --language all → ["es-mx-latam", "en"]
    │  --language es-latam → ["es-mx-latam"]
    │  --language en → ["en"]
    ▼
model_for(lang) → alias de modelo         ← model_cache.py:68-75
    │
    ▼
Bucle: is_model_cached(model)             ← model_cache.py:166-222
    │  cached → print [PASS], registrar en results
    │  no cached → añadir a pending
    ▼
Pre-chequeo de espacio en disco           ← 6 GB * nº modelos pendientes
    │  insuficiente → aborta con EXIT_PRECONDITION_FAILED (8)
    ▼
Bucle: snapshot_download(repo, revision, token)  ← model_cache.py:35-41, revisiones fijadas
    │
    ▼
Descarga de Voice Encoder (si es-mx-latam) ← ve.safetensors desde BASE_MODEL_REPO
    │
    ▼
_provision_translation_pairs()             ← solo si --language incluye en/all
    │  opus-mt-es-en + opus-mt-en-es → conversión CT2
    ▼
_provision_whisper_model()                  ← solo si --with-stt
    │  faster-whisper-small → descarga sin conversión
    ▼
_purge_incomplete()                         ← borra *.incomplete huérfanos
    │
    ▼
_emit_setup_json()                          ← si --json
```

### Modo `--remove-path`

**Implementación:** `_remove_linux_path()` (`cli.py:1461-1477`).

Este modo ejecuta una operación aislada: eliminar el symlink `~/.local/bin/ai-voice-interconnector` que `setup` crea en Linux. No ejecuta chequeos de entorno, ni descargas, ni任何 otra lógica de provisión. El retorno de `cmd_setup` es inmediato tras la operación (`cli.py:1897-1901`).

**Comportamiento por caso:**

| Situación | Efecto | Salida |
|---|---|---|
| Symlink existe | `unlink()`, imprime confirmación | `removed=True` |
| No existe | Imprime "No hay nada que quitar" | `removed=False` |
| Archivo regular homónimo | `raise CliError(EXIT_STATE_CONFLICT)` | Exit 6 |

Con `--json`, emite `{"remove_path": true, "removed": <bool>}` (`cli.py:1899-1900`).

### Modo `--force-update`

**Implementación:** bloque integrado en `cmd_setup` (`cli.py:1936-1954`).

Este modo NO es mutuamente excluyente con el flujo normal — ejecuta primero el borrado y luego continúa con la provisión completa. Borra quirúrgicamente las carpetas de caché de ambos modelos (acotado a `models--ResembleAI--*`) antes del gate de descarga, forzando una re-descarga limpia.

**Proceso:**
1. Itera `model_cache_dirs()` (solo carpetas del proyecto, `model_cache.py:251-262`)
2. Valida que cada ruta empiece por `models--ResembleAI--` (defensa en profundidad)
3. Calcula tamaño con `_dir_size()` (recursivo sobre archivos)
4. `shutil.rmtree()` de cada carpeta
5. Imprime espacio liberado total
6. Continúa con el flujo normal de provisión (chequeos → descarga)

### Modo `--uninstall`

**Implementación:** `_uninstall()` (`cli.py:1539-1571`) despacha a `_uninstall_linux` / `_uninstall_macos` / `_uninstall_windows`.

**Guard de canal nativo:** solo aplica cuando `paths.is_frozen()` es True (ejecutable onedir/AppImage/.app/Inno). Desde fuente o pip/uv aborta con `EXIT_NOT_APPLICABLE` (7).

**Gate `--json`/`--yes`:** `--uninstall --json` requiere `--yes` porque la confirmación interactiva del cleanup contaminaría stdout (`cli.py:1558-1561`).

**Orden unificado de desinstalación (compartido por las 3 ramas):**

| Paso | Linux (`cli.py:1574-1631`) | macOS (`cli.py:1634-1718`) | Windows (`cli.py:1721-1793`) |
|---|---|---|---|
| 1. Datos | `_uninstall_cleanup_data` → cleanup --all + data_root vacío | Idéntico | Idéntico |
| 2. PATH | unlink symlink `~/.local/bin/ai-voice-interconnector` | Idéntico | — (delegado al desinstalador) |
| 3. Binario | `shutil.rmtree(~/.local/opt/ai-voice-interconnector)` | `shutil.rmtree(.app bundle)` | `subprocess.Popen(QuietUninstallString)` (desacoplado) |

**Cancelación atómica:** si el usuario responde negativamente al cleanup (`cancelled=True`), la desinstalación aborta sin tocar PATH ni binario (`cli.py:1596-1598, 1685-1687, 1771-1773`).

**Seguridad del unlink en ejecución:** en Linux y macOS, borrar un archivo/bundle abierto es seguro — el inode sobrevive hasta que el proceso termina. En Windows, el SO mantiene un lock sobre `.exe`, así que se delega al desinstalador de Inno (`cli.py:1776-1779`).

**Detección de Homebrew Cask (macOS):** si existe `~/.homebrew/Caskroom/ai-voice-interconnector`, aborta sin tocar nada y remite a `brew uninstall --cask --zap` (`cli.py:1670-1679`).

### Descarga de modelos (HuggingFace)

El módulo `model_cache.py` proporciona la capa de detección de modelos sin dependencias de ML:

| Función | Propósito | Fuente |
|---|---|---|
| `hub_cache_path()` | Raíz de caché HF (respeta `HF_HUB_CACHE`/`HF_HOME`) | `model_cache.py:50-60` |
| `model_for(lang)` | Traduce `es-latam`→`es-mx-latam`, `en`→`en` | `model_cache.py:68-75` |
| `is_model_cached(model)` | Valida snapshot + archivos safetensors + VE | `model_cache.py:166-222` |
| `_safetensors_header_ok(path)` | Validación ligera de header (previene truncados) | `model_cache.py:78-102` |
| `_resolve_cached_snapshot(dir, rev)` | Resuelve snapshot por revisión fijada | `model_cache.py:105-135` |
| `model_cache_dirs()` | Carpetas HF de ambos repos del proyecto | `model_cache.py:251-262` |
| `purge_incomplete_downloads()` | Borra `*.incomplete` huérfanos en blobs | `model_cache.py:225-248` |

**Revisiones fijadas** (`model_cache.py:35-41`): cada modelo se descarga con un commit hash auditado, no con `main`. Un push posterior al repo no se propaga a los usuarios.

**Modelos descargados por idioma:**

| `--language` | Modelos TTS | Modelo traducción | Modelo STT |
|---|---|---|---|
| `es-latam` | `Chatterbox-Multilingual-es-mx-latam` | — | No (salvo `--with-stt`) |
| `en` | `chatterbox` + `ve.safetensors` | `opus-mt-es-en` + `opus-mt-en-es` (CT2) | No (salvo `--with-stt`) |
| `all` (default) | Ambos TTS | Ambas direcciones de traducción | No (salvo `--with-stt`) |

**Validación de integridad:** `is_model_cached` valida no solo existencia sino también headers safetensors (previene caché truncada que pasa `.exists()` pero revienta al cargar) (`model_cache.py:190-220`).

### Integración de PATH (Linux)

**Creación:** `_integrate_linux_path()` (`cli.py:1412-1458`).

Solo actúa cuando:
1. `sys.platform == "linux"` 
2. La variable de entorno `APPIMAGE` existe y apunta a un archivo existente

Crea `~/.local/bin/ai-voice-interconnector` como symlink a `$APPIMAGE`. Si ya existe un symlink, lo reemplaza. Si existe un archivo regular homónimo, lo respeta (no sobrescribe). Si `~/.local/bin` no está en el PATH de la sesión, imprime instrucciones para añadirlo al shell profile.

**Ruta del symlink:** `_path_symlink()` (`cli.py:1402-1409`) → `~/.local/bin/ai-voice-interconnector` (misma en Linux y macOS).

**Eliminación:** `_remove_linux_path()` (`cli.py:1461-1477`) — operación atómica, solo el symlink.

### Opciones `--language` y `--with-stt`

**`--language`:** controla qué modelos TTS se descargan. El default `all` garantiza offline completo es+en desde el primer uso. El flag sirve para *reducir* el alcance, no ampliarlo (`cli.py:1960-1962`). La provisión de modelos de traducción solo ocurre cuando `--language` incluye `en` o `all` (`cli.py:1998`).

**`--with-stt`:** opt-in ortogonal a `--language`. Gateado por `getattr(args, "with_stt", False)` (`cli.py:2031`). Descarga `Systran/faster-whisper-small` sin conversión CT2 (ya viene en formato CT2). Idempotente: se salta si el directorio ya existe (`cli.py:2037-2043`).

### Manejo de errores

**Clasificador de fallos de provisión** (`_describe_provision_failure`, `cli.py:1796-1852`):

| Familia de excepción | Código | Reason | Acción sugerida |
|---|---|---|---|
| `GatedRepoError` | 8 | `credentials` | Aceptar condiciones en HF o definir `HF_TOKEN` |
| `HfHubHTTPError` 401/403 | 8 | `credentials` | Verificar `HF_TOKEN` |
| `RequestException` (red) | 8 | `network` | Verificar conexión/proxy/firewall |
| `PermissionError` | 8 | `permissions` | Corregir permisos en `~/.cache/huggingface` |
| `OSError` ENOSPC | 8 | `disk_full` | Liberar espacio en disco |
| Cualquier otra | 1 | `provision_failed` | Ver detalle de la excepción |

**Chequeos de entorno** (`_environment_checks`, `cli.py:1127-1174`):

| Chequeo | PASS | FAIL en setup | FAIL en doctor |
|---|---|---|---|
| Chatterbox importable | `PASS` | `EXIT_PRECONDITION_FAILED` (8) | `EXIT_ERROR` (1) |
| Librería de audio | `PASS` | Degradado a `WARN` (continúa) | `FAIL` con salida 1 |

La degradación del FAIL de audio a WARN en setup es intencional: la síntesis a disco funciona sin subsistema de sonido (`cli.py:1886-1891`).

**Pre-chequeo de espacio en disco** (`cli.py:2070-2090`): 6 GB por modelo pendiente (`MIN_FREE_DISK_BYTES = 6 * 1024**3`, `cli.py:98`). Aborta antes de empezar a descargar si el espacio es insuficiente.

### Formato JSON de salida

Con `--json`, el payload varía según el modo ejecutado:

| Modo | Payload |
|---|---|
| Normal | `{"language": str, "models": {alias: {already_cached, downloaded}}, "cache_dir": str}` |
| `--remove-path` | `{"remove_path": true, "removed": bool}` |
| `--uninstall` | `{"uninstall": true, "removed": [str], ...}` (varía por SO) |

Los mensajes `[PASS]`/`[FAIL]`/`[WARN]` de progreso siempre van a stderr, reservando stdout para el JSON (`cli.py:1965-1972`).

---

## Conclusiones

`setup` es un comando de provisión idempotente y multi-propósito que orquesta la integración del binario en el PATH, los chequeos de entorno, y la descarga condicional de modelos TTS, de traducción y de transcripción. Su diseño es notable por: (1) los tres modos exclusivos que cubren las operaciones destructivas comunes (revertir PATH, forzar re-descarga, desinstalar) sin duplicar lógica entre sí ni con el flujo normal; (2) la defensa en profundidad — revisiones fijadas de HuggingFace, validación de headers safetensors, pre-chequeo de disco, guards de ruta en `_integrate_linux_path` y `_uninstall` — que previene estados corruptos; (3) la degradación intencional del FAIL de audio a WARN, que refleja la realidad de que la síntesis a archivo funciona sin subsistema de sonido; y (4) la cancelación atómica en `--uninstall` — si el usuario declina el cleanup, no se toca ni el PATH ni el binario, manteniendo el sistema en un estado coherente.
