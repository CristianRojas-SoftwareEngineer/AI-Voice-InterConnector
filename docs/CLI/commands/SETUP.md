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
| `--force-update` | `false` | Purga la **selección** y la vuelve a provisionar. Es una operación destructiva: su confirmación única lo advierte |
| `--yes`, `-y` | `false` | Omite la confirmación única (la destructiva de `--force-update` o la de tamaño) |
| `--json` | `false` | Global; emite el payload en stdout |

No existen `--with-stt` (`parakeet-tdt-v3` se provisiona siempre) ni `--with-voice-cloning` (el modelo Base de clonado `qwen3-tts-0.6b-base` se provisiona siempre): pasarlos falla como argumento desconocido con exit 2. Tampoco existe `--language`: el conjunto provisionado es fijo (es+en offline completo desde el primer uso), así que la provisión es determinista para la instalación por defecto.

---

## Flujo de provisión

1. **Inicializar el registro de voces** (`VoiceStore::ensure_initialized`), que crea el directorio del registro con el directorio de la voz de fábrica (`default`) y escribe el `reference.qvoice` embebido si falta o si el instalado difiere de él, en cuyo caso borra antes los audios sintetizados de `default` (ver [el modelo de voces](../../DESIGN.md#modelo-de-voces)).
2. **Calcular lo pendiente** para el resumen previo: repos sin snapshot. Ese cálculo solo alimenta el resumen y la confirmación; no decide qué se ejecuta. Con `--force-update`, la selección completa es lo que se purga y se vuelve a descargar.
3. **Cabecera y una sola confirmación.** Lo primero que sale por stderr es la cabecera `Setup · N modelos` (con ` · descarga forzada` si se pasa `--force-update`); la pregunta llega después y el tiempo que el usuario tarda en contestarla no cuenta en el reloj del cierre. Con `--force-update` la confirmación es destructiva: su resumen incluye «Se purgarán los modelos descargados y se volverán a descargar.» y la línea de tamaño de la selección completa. Sin él es no destructiva: solo muestra el tamaño y se omite si no hay nada que descargar o con `--yes`; sin terminal procede. La confirmación destructiva sin terminal y sin `--yes` es un error (`confirmation_required`, exit 2), igual que en `cleanup`. Desde `self install` **no vuelve a preguntar**, porque esa operación ya mostró su propio resumen (ver [`setup` en el ciclo de vida](../../specs/LIFECYCLE.md#87-setup-en-el-ciclo-de-vida)). Si el usuario cancela, no se purga ni se descarga nada: tras el separador de 50 `─` solo aparece «Cancelado.» (o «Cancelado; no se ha borrado nada.» con `--force-update`) y con `--json` el envelope es `cancelled`, con `models_provisioned` vacío, `context` y `stages: []`, y exit 0.
4. **Purga** (solo con `--force-update`) de **la misma selección** que se va a provisionar: todos los modelos, el de clonado incluido. Es la etapa `Purga`, que precede a las descargas. La purga **pasa por el plan de borrado de modelos** —las mismas reglas de propiedad, R3 entre ellas— y no por purgas ad hoc.
5. **Descargar** lo pendiente, repo a repo, en la revisión fijada, con una etapa `Descarga` por modelo (ver «Progreso» más abajo), y **podar** después las revisiones propias obsoletas. En la carpeta de modelos **exclusiva** la poda retira además todo repo `models--*` que la versión ya no fija, con su carpeta de `.locks`. `setup` no para el daemon: no hay etapa de parada.
6. **Cierre y envelope `--json`**: separador y cierre por stderr, y con `--json` el envelope por stdout.

La traducción es **obligatoria**: `setup` siempre provisiona `opus-mt-es-en` y `opus-mt-en-es`, que son modelos CTranslate2 int8 ya convertidos y publicados, sin pasos locales de conversión. Cada uno se lee directo del snapshot de HuggingFace y se valida por presencia de sus cinco ficheros (`config.json`, `model.bin`, `shared_vocabulary.json`, `source.spm`, `target.spm`), todos con tamaño mayor que cero.

**La provisión no depende de nada que sobreviva a `cleanup --model`**, y por eso `setup` es el mecanismo de reintento: tras `cleanup --model` la aplicación queda operativa en cuanto `setup` vuelve a descargar.

---

## Modelos provisionados (`MODEL_REVISIONS`)

Cada modelo se fija por **commit hash** de HuggingFace, así que un push upstream no se propaga a los usuarios. Fuente: `crates/avi-shared/src/paths.rs`, donde cada entrada (`ModelPin`) lleva el nombre lógico, el repo, la revisión y el tamaño aproximado de su descarga.

| Nombre lógico | Repo HF | Rol |
|---|---|---|
| `opus-mt-es-en` | `CristianRojaas/opus-mt-es-en-ct2-int8` | Traducción es→en (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) |
| `opus-mt-en-es` | `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción en→es (CTranslate2 int8, derivado de Helsinki-NLP/opus-mt, CC-BY-4.0) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (4 artefactos int8 vía `MODEL_FILE_PATTERNS`) |
| `dpdfnet8` | `Ceva-IP/DPDFNet` | Limpieza de ruido; opcional para arrancar el daemon, necesario para `--denoise` |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base: clona voces y sintetiza todas, la de fábrica `default` incluida |

Todos los modelos se provisionan siempre: no hay selección opcional.

**Tamaño.** El tamaño de cada descarga es el `approx_bytes` de su `ModelPin` en [`MODEL_REVISIONS`](../../../crates/avi-shared/src/paths.rs). La confirmación previa anuncia la suma de los repos pendientes, en escala decimal y con un decimal separado por coma:

```
Se descargarán 5 modelos, unos X,Y GB.
```

La pregunta de confirmación es `¿Descargar los modelos?` (o `¿Borrar los modelos descargados y descargarlos de nuevo?` con `--force-update`).

**Progreso.** Cada modelo pendiente es una etapa numerada `Descarga` (y con `--force-update` la etapa `Purga` va antes, como `[1/N]`). En la terminal, la línea viva de la etapa en curso (`●`) lleva el nombre del modelo, el porcentaje, el tamaño descargado y la velocidad; al terminar, la etapa pasa a `✓` con su duración y la nota `<tamaño> · <velocidad>/s`. Si falla, la etapa queda en `✗`. Sin terminal no hay línea viva: cada etapa se anuncia una vez como `[n/N] ● Descargando <modelo>…` y, si dura más, deja un latido cada 10 s. Las muestras de la sección «Muestras de salida» recogen ambas formas.

El cierre es `✓ Setup completado en <duración> · N modelos, <tamaño> descargados`; con todo ya descargado, `setup` cierra con `✓ Los N modelos ya estaban descargados.` y no hay etapas. El cierre lo escribe el comando (`setup` o `self install`), no la provisión.

El watchdog interno de `cargo xtask bootstrap` recibe un latido separado (`AVI_SETUP_HEARTBEAT` con `AVI_SETUP_WATCHDOG`) cuando se observa avance de bytes, como máximo una vez cada cinco segundos por modelo; las muestras repetidas sin avance no renuevan el plazo. El latido sigue activo incluso si `AVI_NO_PROGRESS` silencia la salida visible.

El espacio en disco coincide con la descarga: `hf-hub` publica cada archivo de `snapshots/` como enlace simbólico en Unix y como enlace duro en Windows, así que el blob no se duplica. Si el sistema de archivos no admite enlaces duros (FAT32, exFAT o algunos recursos de red), `hf-hub` copia el blob y el espacio en disco se duplica.
**Descarga interrumpida.** `hf-hub` no reanuda por `Range`: una descarga interrumpida se repite completa desde el primer byte del archivo afectado.

**Dónde se descargan.** A la **caché exclusiva de la aplicación**, no a la caché HF del usuario: `models_cache_dir()` ([`paths.rs`](../../../crates/avi-shared/src/paths.rs)); la ruta por sistema operativo está en la [arquitectura](../../DESIGN.md#arquitectura). La razón es que el borrado pueda ser de directorio entero sin tocar nada ajeno. **Si el usuario define `HF_HUB_CACHE` o `HF_HOME`, esa raíz pasa a ser compartida** y se respeta su elección: entonces `cleanup --model` limita el alcance a lo atribuible a la aplicación (regla R3) y `doctor` lo dice con `models.shared_root`.

**`HF_XET_CACHE` no se decide en dos sitios.** Toda la provisión pasa por `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos antes de construir el cliente. Fijarla también desde el motor duplicaría la decisión.

---

## Integridad e idempotencia

- `is_provisioned(name)` valida no solo la existencia del snapshot sino también los ficheros críticos con `size > 0`, lo que evita una caché truncada que pasa `.exists()` y revienta al cargar.
- `ModelStore::ensure_downloaded(name, observer)` valida y hace rollback de snapshot y blobs ante una descarga parcial; el progreso lo entrega a `observer` sin dibujar nada.
- `Pending::is_empty()` es la condición de idempotencia: si no hay nada pendiente, no se descarga nada y no se pregunta el tamaño.

---

## `setup` al final de una instalación: `setup_failed`

`self install` aplica **la misma provisión que `setup`** (descarga de la selección según el estado real del almacén) en el mismo proceso (salvo `--no-setup`), sin repetir la confirmación, y su fallo **no es un fallo de la instalación**:

| | Valor |
|---|---|
| `status` | `installed` — el programa **queda instalado** |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Causa del fallo de provisión | Anidada en `models_cause.reason`: `network_error` (fallo de transporte o del servidor) o `model_unavailable` (Hugging Face rechaza el modelo fijado) |
| Qué hacer | Con `network_error`, reintentar con `setup`; con `model_unavailable`, reintentar no lo resuelve y hace falta una versión que vuelva a fijar el modelo |

Es un **éxito parcial**, y por eso el envelope de `self install` sale por veredicto y no con el objeto `error` detrás. El detalle está en [`SELF.md`](SELF.md) y en el [contrato de `setup`](../CONTRACT.md#setup).

Cuando `setup` se invoca **directamente**, en cambio, sí es un error: un fallo de transporte sale con `network_error` y **20**, y un rechazo del modelo fijado con `model_unavailable` y **1**.

---

## Descarga sin credenciales

`setup` no requiere cuenta de Hugging Face: los modelos fijados son repositorios públicos y se descargan de forma anónima. La descarga **ignora** `HF_TOKEN`, `HF_TOKEN_PATH` y el token guardado por `huggingface-cli login` (el archivo `token` de `HF_HOME`), de modo que un token caducado o ajeno no rompe la descarga ni esconde un modelo que haya dejado de ser público. `HF_ENDPOINT`, `HF_HUB_CACHE` y `HF_HOME` se siguen respetando.

---

## Poda de revisiones obsoletas

**La poda de las revisiones obsoletas de los repos propios** —lo que pide [`setup` en el ciclo de vida](../../specs/LIFECYCLE.md#87-setup-en-el-ciclo-de-vida)— **está vigente**, porque la necesita una actualización, no una instalación.

La selección es siempre la tabla de pines completa: no hay modelos opcionales, así que no se persiste ninguna selección. Tras el reemplazo, el `setup` de la versión nueva provisiona los modelos cuyo pin cambió y poda las revisiones propias obsoletas (`selection`/`purge`), con R3 en raíz compartida. **En la carpeta de modelos exclusiva**, que es de la aplicación y solo contiene modelos fijados, la poda borra además todo directorio `models--*` que no corresponda a ningún pin de la tabla y su carpeta en `.locks`: un modelo que una versión deja de fijar (por ejemplo, el CustomVoice que dejaron las instalaciones anteriores a la 0.46.0) se retira en el siguiente `setup`, que también encadenan `self install` y `self update`. No toca `xet`, el resto de `.locks` ni entradas que no empiecen por `models--`. **En una caché compartida** solo se podan las revisiones de los repos fijados (R3). La confirmación de tamaño y `called_from_lifecycle` siguen vigentes y la idempotencia se conserva.

---

## Contrato `--json`

| Clave | Tipo | Significado |
|---|---|---|
| `status` | string | `"completed"`; `"already_provisioned"` si los modelos ya estaban todos descargados; o `"cancelled"` si el usuario cancela la confirmación (exit 0, `models_provisioned` vacío y nada purgado ni descargado) |
| `models_provisioned` | array de strings | Los modelos de la selección disponibles tras la ejecución |
| `context` | objeto | `models` (número de modelos de la selección) y `force_update` |
| `total_ms`, `stages`, `warnings` | número, array, array | Las claves comunes del envelope; solo cuando hay etapas: `already_provisioned` no las lleva y `cancelled` lleva `stages: []` |

No hay clave `language`. La cabecera, las etapas, la purga, los avisos y el cierre van a stderr, igual con y sin `--json`, reservando stdout para el JSON; `schema_version` lo inyecta el emisor ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)).

---

## Errores

| Situación | `reason` | Código |
|---|---|---|
| `--force-update` sin terminal y sin `--yes`: la purga destructiva exige confirmación | `confirmation_required` | 2 |
| No se pudo inicializar el registro de voces | `voice_store_init_failed` | 1 |
| Fallo de transporte al descargar un snapshot: sin red, 5xx o límite de peticiones tras los reintentos (invocación directa; reintentar puede resolverlo) | `network_error` | 20 |
| Hugging Face rechaza el modelo fijado con 401, 403 o 404: el repositorio, la revisión o el archivo no existe o dejó de ser público (invocación directa; reintentar no lo resuelve) | `model_unavailable` | 1 |
| Fallo de descarga, invocado desde `self install` | `setup_failed` (11) con la causa en `models_cause` (`network_error` o `model_unavailable`) | 11 |
| Hay una operación de ciclo de vida en curso (invocación directa; no llega a ejecutarse) | `lifecycle_locked` | 17 |

---

## Ejemplos

```bash
ai-voice-interconnector setup                          # descarga todos los modelos (idempotente)
ai-voice-interconnector setup --force-update           # purga la selección y re-descarga (confirma en TTY)
ai-voice-interconnector setup --force-update --yes     # ídem, no interactivo
ai-voice-interconnector --json setup                   # payload legible por máquina
```

---

## Muestras de salida

Salidas reales del binario. Las duraciones, tamaños y velocidades son orientativas y cambian con la red y el equipo.

**Descarga sin terminal** (`ai-voice-interconnector setup --yes 2> setup.log`). Sin terminal cada etapa se anuncia una vez; el texto de abajo es el contenido de `setup.log`:

```
Setup · 5 modelos
Se descargarán 2 modelos, unos 97,4 MB.
  [1/2] ● Descargando opus-mt-en-es…
  [1/2] ✓ opus-mt-en-es  6,0 s · 82,5 MB · 13,8 MB/s
  [2/2] ● Descargando dpdfnet8…
  [2/2] ✓ dpdfnet8       5,0 s · 14,9 MB · 3,0 MB/s
──────────────────────────────────────────────────
✓ Setup completado en 11,0 s · 2 modelos, 97,4 MB descargados
```

**Modelos ya descargados** (en la terminal):

```
Setup · 5 modelos
✓ Los 5 modelos ya estaban descargados.
```

**Con `--json`** (stdout; stderr lleva lo mismo que en el modo de texto):

```json
{
  "context": {
    "force_update": false,
    "models": 5
  },
  "models_provisioned": [
    "opus-mt-es-en",
    "opus-mt-en-es",
    "parakeet-tdt-v3",
    "dpdfnet8",
    "qwen3-tts-0.6b-base"
  ],
  "schema_version": "7",
  "status": "already_provisioned"
}
```

**`--force-update` con la descarga fallida** (`ai-voice-interconnector setup --force-update --yes 2> setup.log`, sin red; sale con 20). La etapa `Purga` termina y la primera `Descarga` queda en `✗`; el `ERROR` intermedio es la traza de los reintentos del cliente de descarga:

```
Setup · 5 modelos · descarga forzada
Se purgarán los modelos descargados y se volverán a descargar.
Se descargarán 5 modelos, unos 3,4 GB.
  [1/6] ● Purgando los modelos…
  [1/6] ✓ Purga     0,6 s
  [2/6] ● Descargando opus-mt-es-en…
  [2/6] ● opus-mt-es-en · 0 B · 0 B/s 10,0 s
2026-05-12T10:30:00.000000Z ERROR hf_hub::retry: retry exhausted url=https://huggingface.co/api/models/CristianRojaas/opus-mt-es-en-ct2-int8/tree/6eabf2f7d9f92dd52e38fe95c9da29f219613c19?recursive=true max_attempts=5
  [2/6] ✗ opus-mt-es-en        17,0 s
──────────────────────────────────────────────────
Error: opus-mt-es-en: HTTP request error: error sending request for url (https://huggingface.co/api/models/CristianRojaas/opus-mt-es-en-ct2-int8/tree/6eabf2f7d9f92dd52e38fe95c9da29f219613c19?recursive=true) (https://huggingface.co/api/models/CristianRojaas/opus-mt-es-en-ct2-int8/tree/6eabf2f7d9f92dd52e38fe95c9da29f219613c19?recursive=true).
Solución: Comprueba la conexión a Internet y repite la orden.
```
