# `setup`

Provisiona el runtime: descarga los modelos pinneados desde HuggingFace Hub, incluidos los modelos de traducción, que ya vienen convertidos. Es **idempotente**: una segunda ejecución con todo presente no descarga nada.

**Implementación:** el motor es el módulo `setup` de `avi-lifecycle` (`crates/avi-lifecycle/src/setup.rs`), que es una **traducción fiel, no un rediseño**: conserva la semántica que ya existía —selección por banderas, idempotencia por presencia del snapshot, purga de `--force-update` sobre la selección y validación de cada modelo por presencia de sus ficheros—. En `src/main.rs` quedan solo `handle_setup` y el envelope `--json`, porque el parseo de la CLI y el emisor no viven en ese crate.

---

## Definición CLI (parser)

```
ai-voice-interconnector setup [--with-voice-cloning] [--force-update] [--yes|-y] [--json]
```

| Flag | Default | Descripción |
|---|---|---|
| `--with-voice-cloning` | `false` | Añade a la selección el modelo Base de clonado `qwen3-tts-0.6b-base` (2,5 GB), requerido por `voice clone` |
| `--force-update` | `false` | Purga la **selección** y la vuelve a provisionar. Es una operación destructiva: pide su propia confirmación |
| `--yes`, `-y` | `false` | Omite la confirmación de la purga y la del tamaño pendiente |
| `--json` | `false` | Global; emite el payload en stdout |

No existe `--with-stt` (`parakeet-tdt-v3` se provisiona siempre): pasarlo falla como argumento desconocido con exit 2. Tampoco existe `--language`: el conjunto provisionado es fijo (es+en offline completo desde el primer uso), así que la provisión es determinista para la instalación por defecto.

---

## Flujo de provisión

1. **Inicializar el registro de voces** (`VoiceStore::ensure_initialized`), que crea el directorio de datos y materializa la voz de fábrica `default`.
2. **`--force-update`**: confirmación destructiva (salvo `--yes` o sin terminal) y purga de **la misma selección** que se va a provisionar. Purgar el modelo de clonado cuando el usuario no lo pidió dejaría la instalación sin lo que sí quiere. La purga **pasa por el plan de borrado de modelos** —las mismas reglas de propiedad, R3 entre ellas, y la misma confirmación— y no por purgas ad hoc.
3. **Calcular lo pendiente** para el resumen previo: repos sin snapshot. Ese cálculo solo alimenta el resumen y la confirmación de tamaño; no decide qué se ejecuta. Con terminal y sin `--yes`, pide confirmación con el tamaño estimado; desde `self install` **no vuelve a preguntar**, porque esa operación ya mostró su propio resumen (§8.7).
4. **Descargar** lo pendiente, repo a repo, en la revisión fijada.
5. **Envelope `--json`** o mensaje humano.

La traducción es **obligatoria**: `setup` siempre provisiona `opus-mt-es-en` y `opus-mt-en-es`, que son modelos CTranslate2 int8 ya convertidos y publicados, sin pasos locales de conversión. Cada uno se lee directo del snapshot de HuggingFace y se valida por presencia de sus cinco ficheros (`config.json`, `model.bin`, `shared_vocabulary.json`, `source.spm`, `target.spm`), todos con tamaño mayor que cero.

**La provisión no depende de nada que sobreviva a `cleanup --model`**, y por eso `setup` es el mecanismo de reintento: tras `cleanup --model` la aplicación queda operativa en cuanto `setup` vuelve a descargar.

---

## Modelos provisionados (`MODEL_REVISIONS`)

Cada modelo se fija por **commit hash** de HuggingFace, así que un push upstream no se propaga a los usuarios. Fuente: `crates/avi-shared/src/paths.rs`, donde cada entrada (`ModelPin`) lleva el nombre lógico, el repo, la revisión y el tamaño aproximado de su descarga.

| Nombre lógico | Repo HF | Rol | Selección |
|---|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Motor TTS (síntesis) | Siempre |
| `opus-mt-es-en` | `CristianRojaas/opus-mt-es-en-ct2-int8` | Traducción es→en (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) | Siempre |
| `opus-mt-en-es` | `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción en→es (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) | Siempre |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (4 artefactos int8 vía `MODEL_FILE_PATTERNS`) | Siempre |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base de clonado de voz | Opt-in `--with-voice-cloning` |

**Tamaño.** La descarga es de 3,33 GB para la selección base y de 5,85 GB con `--with-voice-cloning` (el modelo de clonado suma 2,5 GB). La cifra sale de la suma del tamaño medido de cada repo (`approx_bytes` de su `ModelPin`), y es la misma que anuncia la confirmación previa, en escala decimal y con un decimal:

```
Se descargarán 4 modelo(s), unos 3.3 GB.
```

El espacio en disco coincide con la descarga: `hf-hub` publica cada archivo de `snapshots/` como enlace simbólico en Unix y como enlace duro en Windows, así que el blob no se duplica. Si el sistema de archivos no admite enlaces duros (FAT32, exFAT o algunos recursos de red), `hf-hub` copia el blob y el espacio en disco se duplica.
**Descarga interrumpida.** `hf-hub` no reanuda por `Range`: una descarga interrumpida se repite completa desde el primer byte del archivo afectado.

**Dónde se descargan.** A la **caché exclusiva de la aplicación**, no a la caché HF del usuario: `models_cache_dir()` (`avi-store`), que es `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows, `~/Library/Caches/ai-voice-interconnector/models` en macOS y `$XDG_CACHE_HOME/ai-voice-interconnector/models` en Linux. La razón es que el borrado pueda ser de directorio entero sin tocar nada ajeno. **Si el usuario define `HF_HUB_CACHE` o `HF_HOME`, esa raíz pasa a ser compartida** y se respeta su elección: entonces `cleanup --model` limita el alcance a lo atribuible a la aplicación (regla R3) y `doctor` lo dice con `models.shared_root`.

**`HF_XET_CACHE` no se decide en dos sitios.** Toda la provisión pasa por `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos antes de construir el cliente. Fijarla también desde el motor duplicaría la decisión.

---

## Integridad e idempotencia

- `is_provisioned(name)` valida no solo la existencia del snapshot sino también los ficheros críticos con `size > 0`, lo que evita una caché truncada que pasa `.exists()` y revienta al cargar.
- `ensure_downloaded` valida y hace rollback de snapshot y blobs ante una descarga parcial.
- `Pending::is_empty()` es la condición de idempotencia: si no hay nada pendiente, no se descarga nada y no se pregunta el tamaño.

---

## `setup` al final de una instalación: `setup_failed`

`self install` aplica **la misma provisión que `setup`** (descarga de la selección según el estado real del almacén) en el mismo proceso (salvo `--no-setup`), sin repetir la confirmación ni escribir la selección, y su fallo **no es un fallo de la instalación**:

| | Valor |
|---|---|
| `status` | `installed` — el programa **queda instalado** |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Causa del fallo de provisión | Anidada en `models_cause.reason`: `network_error` |
| Qué hacer | Reintentar con `setup` |

Es un **éxito parcial**, y por eso el envelope de `self install` sale por veredicto y no con el objeto `error` detrás. El detalle está en [`SELF.md`](SELF.md) y en §11 de [`../CONTRACT.md`](../CONTRACT.md).

Cuando `setup` se invoca **directamente**, en cambio, sí es un error: un fallo de descarga sale con `network_error` y **20**, y un fallo de conversión con `setup_failed` y **11**.

---

## Selección persistida y poda, vigentes

**La selección persistida en configuración y la poda de las revisiones obsoletas de los repos propios** —las dos cosas que §8.7 pide— **están vigentes**, porque las necesita una actualización, no una instalación.

La selección vive en `setup-selection.json` bajo la raíz de datos vigente (honra `AVI_DATA_DIR`), con esquema `{schema_version: 1, with_voice_cloning: bool}` extensible a futuros opcionales. La lectura es tolerante (fichero ausente o ilegible → conjunto base) y la escritura es atómica (temporal + renombrado, como el recibo). Sobrevive a los updates porque el reemplazo no toca la raíz de datos; se pierde al desinstalar, lo cual es correcto. El `setup` invocado por el traspaso lee la selección guardada, no los flags.

Tras el reemplazo, el `setup` de la versión nueva provisiona los modelos cuyo pin cambió y poda las revisiones propias obsoletas (`selection`/`purge_targets`/`purge`), con R3 en raíz compartida. La confirmación de tamaño y `called_from_lifecycle` siguen vigentes, la idempotencia se conserva y las migraciones hacia delante corren antes de provisionar.

Consecuencia práctica: la selección es **la guardada en la instalación**, no la de los flags de esta invocación. Un `setup` posterior sin `--with-voice-cloning` no purga el modelo Base (la purga es sobre la selección), pero tampoco lo vuelve a descargar si ya está.

---

## Contrato `--json`

| Clave | Tipo | Significado |
|---|---|---|
| `status` | string | `"completed"` |
| `models_provisioned` | array de strings | Los modelos de la selección disponibles tras la ejecución |

No hay clave `language`. Los mensajes de progreso, la purga y los avisos van a stderr, reservando stdout para el JSON; `schema_version` lo inyecta el emisor y vale **`"5"`**.

---

## Errores

| Situación | `reason` | Código |
|---|---|---|
| No se pudo inicializar el registro de voces | `voice_store_init_failed` | 1 |
| Fallo de descarga de un snapshot (invocación directa) | `network_error` | 20 |
| Fallo de descarga, invocado desde `self install` | `setup_failed` (11) con la causa en `models_cause` | 11 |

---

## Ejemplos

```bash
ai-voice-interconnector setup                          # descarga la selección base (idempotente)
ai-voice-interconnector setup --with-voice-cloning     # incluye el Base de clonado (2,5 GB)
ai-voice-interconnector setup --force-update           # purga la selección y re-descarga (confirma en TTY)
ai-voice-interconnector setup --force-update --yes     # ídem, no interactivo
ai-voice-interconnector --json setup                   # payload legible por máquina
```
