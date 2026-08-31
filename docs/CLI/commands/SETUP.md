## Recorrido

La investigación examinó la implementación completa de `setup` explorando tres fuentes principales: el handler CLI (`src/main.rs:460-777`), el almacén nativo (`crates/avi-store/src/lib.rs:381-801` `MODEL_REVISIONS`, `hf_cache_dir`, `xet_cache_dir`) y el descargador `hf-hub`/`ct2rs`/`ort`. Se leyeron en paralelo los helpers de integración de PATH, las tres ramas de desinstalación por SO, la provisión de modelos de traducción y el clasificador de fallos de provisión (`src/main.rs`), los chequeos de entorno compartidos con `doctor` y las constantes de salida (`crates/avi-core/src/exit_codes.rs`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `setup`:** Es un comando de provisión idempotente con tres modos mutuamente excluyentes (`--remove-path`, `--force-update`, `--uninstall`) que cortan el flujo normal. El flujo normal ejecuta: integración de PATH (solo Linux AppImage) → chequeos de entorno (FAIL de audio degrado a WARN) → descarga condicional de modelos TTS por idioma → descarga de modelos de traducción (si `--language` incluye `en`/`all`) → descarga opt-in de modelo de transcripción (`--with-stt`) → limpieza de descargas parciales huérfanas. Cada modo exclusivo implementa una operación destructiva específica sin ejecutar provisión.

**Implementación:** `cmd_setup` (`src/main.rs:460-777`) despacha vía `avi-store` (`crates/avi-store/src/lib.rs:381` `MODEL_REVISIONS`, `lib.rs:634` `ensure_downloaded`) con `hf-hub`/`ct2rs`/`ort`. `MODEL_FILE_PATTERNS` (`crates/avi-store/src/lib.rs:421`) filtra los 4 artefactos Parakeet (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`, acotado a ~600 MB en vez de ~40 GB), evitando descargar variantes no usadas. La descarga de traducción usa `ct2rs`; `ParakeetEngine` usa `ort` load-dynamic (stack Rust nativo sin Python). El manejo de errores clasifica las excepciones en familias (credenciales, red, permisos, disco) con códigos de salida accionables.

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
| `--with-stt` | flag | False | Provisiona el modelo de transcripción `parakeet-tdt-0.6b-v3` int8 (`ParakeetEngine`/`ort` load-dynamic, opt-in) |
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
MODEL_REVISIONS → alias de modelo         ← crates/avi-store/src/lib.rs:381
    │
    ▼
Bucle: is_provisioned(model)             ← crates/avi-store/src/lib.rs:550
    │  cached → print [PASS], registrar en results
    │  no cached → añadir a pending
    ▼
Pre-chequeo de espacio en disco           ← 6 GB * nº modelos pendientes
    │  insuficiente → aborta con EXIT_PRECONDITION_FAILED (8)
    ▼
Bucle: ensure_downloaded(repo, revision)  ← crates/avi-store/src/lib.rs:713, hf-hub + hf_cache_dir()
    │
    ▼
Descarga de Voice Encoder (si es-mx-latam) ← ve.safetensors desde BASE_MODEL_REPO
    │
    ▼
_provision_translation_pairs()             ← solo si --language incluye en/all
     │  opus-mt-es-en + opus-mt-en-es → conversión CT2 vía `ct2rs`
     ▼
ParakeetEngine::ensure_downloaded           ← solo si --with-stt
     │  `parakeet-tdt-0.6b-v3` int8 (4 artefactos) → `ort` load-dynamic vía `hf_cache_dir()`
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

Este modo NO es mutuamente excluyente con el flujo normal — ejecuta primero el borrado y luego continúa con la provisión completa. Borra quirúrgicamente las carpetas de caché de los modelos pinneados (`models--Qwen--*`, `models--istupakov--*`, `models--Helsinki-NLP--*`, `xet`) antes del gate de descarga, forzando una re-descarga limpia.

**Proceso:**
1. Itera snapshots HF vía `hf_cache_dir()` (`crates/avi-store/src/lib.rs:446,675-703` `remove_hf_snapshot`/`remove_xet_cache`)
2. Valida que cada ruta sea pin en `MODEL_REVISIONS` (defensa en profundidad)
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

El módulo `avi-store` (`crates/avi-store/src/lib.rs`) proporciona la capa de detección sin Python:

| Función | Propósito | Fuente |
|---|---|---|
| `hf_cache_dir()` | Raíz de caché HF (respeta `HF_HUB_CACHE`/`HF_HOME` → `~/.cache/huggingface/hub`) | `crates/avi-store/src/lib.rs:446` |
| `MODEL_REVISIONS` | Pines `(nombre, repo, revisión)` auditables | `crates/avi-store/src/lib.rs:381` |
| `is_provisioned(model)` | Valida snapshot + ficheros críticos `MODEL_FILE_PATTERNS` con `size>0` | `crates/avi-store/src/lib.rs:550` |
| `model_snapshot_path(model)` | Resuelve snapshot por revisión fijada (`snapshots/<hash>`) | `crates/avi-store/src/lib.rs:524` |
| `remove_hf_snapshot` / `remove_xet_cache` | Borrado quirúrgico `hub` + `xet` | `crates/avi-store/src/lib.rs:675-703` |
| `ensure_downloaded(model)` | Descarga vía `hf-hub` con `HF_CACHE_DIR` explícito + validación/rollback | `crates/avi-store/src/lib.rs:713` |

**Revisiones fijadas** (`crates/avi-store/src/lib.rs:381` `MODEL_REVISIONS`): cada modelo se descarga con un commit hash auditado, no con `main`. Un push posterior al repo no se propaga a los usuarios.

**Modelos descargados por idioma:**

| `--language` | Modelos TTS | Modelo traducción | Modelo STT |
|---|---|---|---|
| `es-latam` | `qwen3-tts-0.6b` (`Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice`) | — | `parakeet-tdt-0.6b-v3` int8 (4 artefactos, opt-in `--with-stt`) |
| `en` | `qwen3-tts-0.6b` + Base opt-in | `opus-mt-es-en` + `opus-mt-en-es` (CT2) | idem |
| `all` (default) | 4 base | Ambas direcciones de traducción | idem |

**Validación de integridad:** `is_provisioned` valida no solo existencia sino también ficheros críticos con `size>0` (`crates/avi-store/src/lib.rs:550`) y `ensure_downloaded` valida/rollback de snapshot+blobs, previniendo caché truncada que pasa `.exists()` pero revienta al cargar.

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

**`--with-stt`:** opt-in ortogonal a `--language`. Descarga `parakeet-tdt-0.6b-v3` int8 (4 artefactos `MODEL_FILE_PATTERNS` `crates/avi-store/src/lib.rs:421`) vía `hf-hub` con `ort` load-dynamic (`crates/avi-stt/src/parakeet.rs`). Idempotente: se salta si `hf_cache_dir()` ya tiene el snapshot pinneado (`8f23f0c`).

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
| Qwen3-TTS/Parakeet importable (`hf_cache_dir()` snapshots) | `PASS` | `EXIT_PRECONDITION_FAILED` (8) | `EXIT_ERROR` (1) |
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
