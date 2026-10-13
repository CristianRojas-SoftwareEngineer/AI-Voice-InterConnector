# `setup`

Provisiona el runtime: descarga los modelos pinneados desde HuggingFace Hub, incluidos los modelos de traducción, que ya vienen convertidos. Es **idempotente**: una segunda ejecución con todo presente no descarga nada.

**Implementación:** el motor es el módulo `setup` de `avi-lifecycle` (`crates/avi-lifecycle/src/setup.rs`), que aplica la semántica de provisión —selección de modelos, idempotencia por presencia del snapshot, purga de `--force-update` sobre la selección y validación de cada modelo por presencia de sus ficheros—. En `src/main.rs` quedan solo `handle_setup` y el envelope `--json`, porque el parseo de la CLI y el emisor no viven en ese crate.

---

## Definición CLI (parser)

```
ai-voice-interconnector setup [--force-update] [--yes|-y] [--json]
```

| Flag | Default | Descripción |
|---|---|---|
| `--force-update` | `false` | Purga la **selección** y la vuelve a provisionar. Es una operación destructiva: pide su propia confirmación |
| `--yes`, `-y` | `false` | Omite la confirmación de la purga y la del tamaño pendiente |
| `--json` | `false` | Global; emite el payload en stdout |

No existen `--with-stt` (`parakeet-tdt-v3` se provisiona siempre) ni `--with-voice-cloning` (el modelo Base de clonado `qwen3-tts-0.6b-base` se provisiona siempre): pasarlos falla como argumento desconocido con exit 2. Tampoco existe `--language`: el conjunto provisionado es fijo (es+en offline completo desde el primer uso), así que la provisión es determinista para la instalación por defecto.

---

## Flujo de provisión

1. **Inicializar el registro de voces** (`VoiceStore::ensure_initialized`), que crea el directorio del registro con un directorio por voz de fábrica (`default`, `ryan`, `vivian`) y escribe el `reference.qvoice` embebido solo en `default` si aún no existe; `ryan` y `vivian` quedan sin archivo, porque son presets del motor (ver [el modelo de voces de dos niveles](../../DESIGN.md#modelo-de-voces-de-dos-niveles)).
2. **`--force-update`**: confirmación destructiva (salvo `--yes` o sin terminal) y purga de **la misma selección** que se va a provisionar: todos los modelos, el de clonado incluido. La purga **pasa por el plan de borrado de modelos** —las mismas reglas de propiedad, R3 entre ellas, y la misma confirmación— y no por purgas ad hoc.
3. **Calcular lo pendiente** para el resumen previo: repos sin snapshot. Ese cálculo solo alimenta el resumen y la confirmación de tamaño; no decide qué se ejecuta. Con terminal y sin `--yes`, pide confirmación con el tamaño estimado; desde `self install` **no vuelve a preguntar**, porque esa operación ya mostró su propio resumen (ver [`setup` en el ciclo de vida](../../specs/LIFECYCLE.md#87-setup-en-el-ciclo-de-vida)).
4. **Descargar** lo pendiente, repo a repo, en la revisión fijada.
5. **Envelope `--json`** o mensaje humano.

La traducción es **obligatoria**: `setup` siempre provisiona `opus-mt-es-en` y `opus-mt-en-es`, que son modelos CTranslate2 int8 ya convertidos y publicados, sin pasos locales de conversión. Cada uno se lee directo del snapshot de HuggingFace y se valida por presencia de sus cinco ficheros (`config.json`, `model.bin`, `shared_vocabulary.json`, `source.spm`, `target.spm`), todos con tamaño mayor que cero.

**La provisión no depende de nada que sobreviva a `cleanup --model`**, y por eso `setup` es el mecanismo de reintento: tras `cleanup --model` la aplicación queda operativa en cuanto `setup` vuelve a descargar.

---

## Modelos provisionados (`MODEL_REVISIONS`)

Cada modelo se fija por **commit hash** de HuggingFace, así que un push upstream no se propaga a los usuarios. Fuente: `crates/avi-shared/src/paths.rs`, donde cada entrada (`ModelPin`) lleva el nombre lógico, el repo, la revisión y el tamaño aproximado de su descarga.

| Nombre lógico | Repo HF | Rol |
|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Motor TTS (síntesis de los presets) |
| `opus-mt-es-en` | `CristianRojaas/opus-mt-es-en-ct2-int8` | Traducción es→en (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) |
| `opus-mt-en-es` | `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción en→es (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (4 artefactos int8 vía `MODEL_FILE_PATTERNS`) |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base de clonado de voz: sintetiza las voces clonadas, la de fábrica `default` incluida |

Todos los modelos se provisionan siempre: no hay selección opcional.

**Tamaño.** El tamaño de cada descarga es el `approx_bytes` de su `ModelPin` en [`MODEL_REVISIONS`](../../../crates/avi-shared/src/paths.rs). La confirmación previa anuncia la suma de los repos pendientes, en escala decimal y con un decimal:

```
Se descargarán N modelo(s), unos X.Y GB.
```

El espacio en disco coincide con la descarga: `hf-hub` publica cada archivo de `snapshots/` como enlace simbólico en Unix y como enlace duro en Windows, así que el blob no se duplica. Si el sistema de archivos no admite enlaces duros (FAT32, exFAT o algunos recursos de red), `hf-hub` copia el blob y el espacio en disco se duplica.
**Descarga interrumpida.** `hf-hub` no reanuda por `Range`: una descarga interrumpida se repite completa desde el primer byte del archivo afectado.

**Dónde se descargan.** A la **caché exclusiva de la aplicación**, no a la caché HF del usuario: `models_cache_dir()` ([`paths.rs`](../../../crates/avi-shared/src/paths.rs)); la ruta por sistema operativo está en la [arquitectura](../../DESIGN.md#arquitectura). La razón es que el borrado pueda ser de directorio entero sin tocar nada ajeno. **Si el usuario define `HF_HUB_CACHE` o `HF_HOME`, esa raíz pasa a ser compartida** y se respeta su elección: entonces `cleanup --model` limita el alcance a lo atribuible a la aplicación (regla R3) y `doctor` lo dice con `models.shared_root`.

**`HF_XET_CACHE` no se decide en dos sitios.** Toda la provisión pasa por `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos antes de construir el cliente. Fijarla también desde el motor duplicaría la decisión.

---

## Integridad e idempotencia

- `is_provisioned(name)` valida no solo la existencia del snapshot sino también los ficheros críticos con `size > 0`, lo que evita una caché truncada que pasa `.exists()` y revienta al cargar.
- `ensure_downloaded` valida y hace rollback de snapshot y blobs ante una descarga parcial.
- `Pending::is_empty()` es la condición de idempotencia: si no hay nada pendiente, no se descarga nada y no se pregunta el tamaño.

---

## `setup` al final de una instalación: `setup_failed`

`self install` aplica **la misma provisión que `setup`** (descarga de la selección según el estado real del almacén) en el mismo proceso (salvo `--no-setup`), sin repetir la confirmación, y su fallo **no es un fallo de la instalación**:

| | Valor |
|---|---|
| `status` | `installed` — el programa **queda instalado** |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Causa del fallo de provisión | Anidada en `models_cause.reason`: `network_error` |
| Qué hacer | Reintentar con `setup` |

Es un **éxito parcial**, y por eso el envelope de `self install` sale por veredicto y no con el objeto `error` detrás. El detalle está en [`SELF.md`](SELF.md) y en el [contrato de `setup`](../CONTRACT.md#setup).

Cuando `setup` se invoca **directamente**, en cambio, sí es un error: un fallo de descarga sale con `network_error` y **20**.

---

## Poda de revisiones obsoletas

**La poda de las revisiones obsoletas de los repos propios** —lo que pide [`setup` en el ciclo de vida](../../specs/LIFECYCLE.md#87-setup-en-el-ciclo-de-vida)— **está vigente**, porque la necesita una actualización, no una instalación.

La selección es siempre la tabla de pines completa: no hay modelos opcionales, así que no se persiste ninguna selección. Tras el reemplazo, el `setup` de la versión nueva provisiona los modelos cuyo pin cambió y poda las revisiones propias obsoletas (`selection`/`purge`), con R3 en raíz compartida. La confirmación de tamaño y `called_from_lifecycle` siguen vigentes y la idempotencia se conserva.

---

## Contrato `--json`

| Clave | Tipo | Significado |
|---|---|---|
| `status` | string | `"completed"` |
| `models_provisioned` | array de strings | Los modelos de la selección disponibles tras la ejecución |

No hay clave `language`. Los mensajes de progreso, la purga y los avisos van a stderr, reservando stdout para el JSON; `schema_version` lo inyecta el emisor ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)).

---

## Errores

| Situación | `reason` | Código |
|---|---|---|
| No se pudo inicializar el registro de voces | `voice_store_init_failed` | 1 |
| Fallo de descarga de un snapshot (invocación directa) | `network_error` | 20 |
| Fallo de descarga, invocado desde `self install` | `setup_failed` (11) con la causa en `models_cause` | 11 |
| Hay una operación de ciclo de vida en curso (invocación directa; no llega a ejecutarse) | `lifecycle_locked` | 17 |

---

## Ejemplos

```bash
ai-voice-interconnector setup                          # descarga todos los modelos (idempotente)
ai-voice-interconnector setup --force-update           # purga la selección y re-descarga (confirma en TTY)
ai-voice-interconnector setup --force-update --yes     # ídem, no interactivo
ai-voice-interconnector --json setup                   # payload legible por máquina
```
