# `setup`

Provisiona el runtime: descarga los modelos pinneados desde HuggingFace Hub y convierte el derivado CT2 de traducción. Es **idempotente**: una segunda ejecución con todo presente no descarga nada.

**Implementación:** el motor es el módulo `setup` de `avi-lifecycle` (`crates/avi-lifecycle/src/setup.rs`), que es una **traducción fiel, no un rediseño**: conserva la semántica que ya existía —selección por banderas, idempotencia por presencia del snapshot, purga de `--force-update` sobre la selección y conversión del derivado con directorio temporal atómico y el mismo gate que la acepta—. En `src/main.rs` quedan solo `handle_setup` y el sobre `--json`, porque el parseo de la CLI y el emisor no viven en ese crate.

---

## Definición CLI (parser)

```
ai-voice-interconnector setup [--with-voice-cloning] [--with-stt] [--force-update] [--yes|-y] [--json]
```

| Flag | Default | Descripción |
|---|---|---|
| `--with-voice-cloning` | `false` | Añade a la selección el modelo Base de clonado `qwen3-tts-0.6b-base` (~2,5 GB), requerido por `voice clone` |
| `--with-stt` | `false` | **Redundante**: `parakeet-tdt-v3` ya se provisiona siempre. Solo emite un aviso informativo por stderr |
| `--force-update` | `false` | Purga la **selección** y la vuelve a provisionar. Es una operación destructiva: pide su propia confirmación |
| `--yes`, `-y` | `false` | Omite la confirmación de la purga y la del tamaño pendiente |
| `--json` | `false` | Global; emite el payload en stdout |

No existe `--language`: el conjunto provisionado es fijo (es+en offline completo desde el primer uso), así que la provisión es determinista para la instalación por defecto.

---

## Flujo de provisión

1. **Inicializar el registro de voces** (`VoiceStore::ensure_initialized`), que crea el directorio de datos y materializa la voz de fábrica `default`.
2. **`--with-stt`**: aviso informativo, nada más.
3. **`--force-update`**: confirmación destructiva (salvo `--yes` o sin terminal) y purga de **la misma selección** que se va a provisionar. Purgar el modelo de clonado cuando el usuario no lo pidió dejaría la instalación sin lo que sí quiere. La purga **pasa por el plan de borrado de modelos** —las mismas reglas de propiedad, R3 entre ellas, y la misma confirmación— y no por purgas ad hoc.
4. **Calcular lo pendiente** antes de descargar: repos sin snapshot y derivados CT2 que haya que convertir o revalidar. Con terminal y sin `--yes`, pide confirmación con el tamaño estimado; desde `self install` **no vuelve a preguntar**, porque esa operación ya mostró su propio resumen (§8.7).
5. **Descargar** lo pendiente, repo a repo, en la revisión fijada.
6. **Convertir los derivados CT2** de los pares `es-en` y `en-es` cuyo repo esté provisionado y cuyo derivado no pase el gate.
7. **Sobre `--json`** o mensaje humano.

### La conversión del derivado CT2

El derivado es **obligatorio**, no opcional: sin él la traducción no funciona. Vive en `avi_store::ct2_model_dir(pair)` = `<models_cache_dir>/ct2/opus-mt-<pair>/` y se genera con `ctranslate2` desde el snapshot de Marian.

- **Atómica**: se convierte en un directorio temporal **hermano** del destino y se publica con `rename`. Hermano y no dentro, porque el renombrado final tiene que ser del mismo volumen: si el temporal estuviera dentro del destino, la escritura no sería atómica porque el destino se borra antes.
- **Verificada antes de declarar éxito**: se comprueba con el mismo criterio del gate que usa el loader (`ct2_dir_missing_files`), de modo que un temporal incompleto nunca se publica.
- **Los `.spm` se aseguran**: si el conversor no los depositó —una versión sin `--copy_files`— se copian desde el snapshot pinneado, porque sin ellos el derivado no puede tokenizar. Si el snapshot tampoco los trae, es un fallo con diagnóstico.
- **Idempotente por fecha**: solo se reconvierte cuando el `model.bin` del derivado es más viejo que el snapshot. Si alguna de las dos fechas no se puede leer, el derivado se da por bueno: reconvertir porque no se pudo leer una fecha convertiría en fallo lo que es un no-op.

**La provisión no depende de nada que sobreviva a `cleanup --model`**, y por eso `setup` es el mecanismo de reintento: tras `cleanup --model` la aplicación queda operativa en cuanto `setup` vuelve a descargar.

---

## Modelos provisionados (`MODEL_REVISIONS`)

Cada modelo se fija por **commit hash** de HuggingFace, así que un push upstream no se propaga a los usuarios. Fuente: `crates/avi-store/src/lib.rs`.

| Nombre lógico | Repo HF | Rol | Selección |
|---|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Motor TTS (síntesis) | Siempre |
| `marian-es-en` | `Helsinki-NLP/opus-mt-es-en` | Traducción es→en (derivado a CT2) | Siempre |
| `marian-en-es` | `Helsinki-NLP/opus-mt-en-es` | Traducción en→es (derivado a CT2) | Siempre |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (4 artefactos int8 vía `MODEL_FILE_PATTERNS`) | Siempre |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base de clonado de voz | Opt-in `--with-voice-cloning` |

Peso aproximado: ~9 GB la selección base, ~11,5 GB con `--with-voice-cloning`.

**Dónde se descargan.** A la **caché exclusiva de la aplicación**, no a la caché HF del usuario: `models_cache_dir()` (`avi-store`), que es `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows, `~/Library/Caches/ai-voice-interconnector/models` en macOS y `$XDG_CACHE_HOME/ai-voice-interconnector/models` en Linux. La razón es que el borrado pueda ser de directorio entero sin tocar nada ajeno. **Si el usuario define `HF_HUB_CACHE` o `HF_HOME`, esa raíz pasa a ser compartida** y se respeta su elección: entonces `cleanup --model` limita el alcance a lo atribuible a la aplicación (regla R3) y `doctor` lo dice con `models.shared_root`.

**`HF_XET_CACHE` no se decide en dos sitios.** Toda la provisión pasa por `ModelStore::new()`, que ya la fija al subdirectorio `xet` de la raíz de modelos antes de construir el cliente. Fijarla también desde el motor duplicaría la decisión.

---

## Integridad e idempotencia

- `is_provisioned(name)` valida no solo la existencia del snapshot sino también los ficheros críticos con `size > 0`, lo que evita una caché truncada que pasa `.exists()` y revienta al cargar.
- `ensure_downloaded` valida y hace rollback de snapshot y blobs ante una descarga parcial.
- La conversión CT2 es atómica y verificada (§«La conversión del derivado CT2»).
- `Pending::is_empty()` es la condición de idempotencia: si no hay nada pendiente, no se descarga nada y no se pregunta el tamaño.

---

## `setup` al final de una instalación: `setup_failed`

`self install` ejecuta `setup` **en el mismo proceso** (salvo `--no-setup`), y su fallo **no es un fallo de la instalación**:

| | Valor |
|---|---|
| `status` | `installed` — el programa **queda instalado** |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Causa del fallo de provisión | Anidada en `models_cause.reason`: `network_error` o `ct2_conversion_failed` |
| Qué hacer | Reintentar con `setup` |

Es un **éxito parcial**, y por eso el sobre de `self install` sale por veredicto y no con el objeto `error` detrás. El detalle está en [`SELF.md`](SELF.md) y en §11 de [`../CONTRACT.md`](../CONTRACT.md).

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
| `with_stt` | boolean | Espejo del flag `--with-stt` |
| `models_provisioned` | array de strings | Los modelos de la selección disponibles tras la ejecución |

No hay clave `language`. Los mensajes de progreso, la purga y los avisos van a stderr, reservando stdout para el JSON; `schema_version` lo inyecta el emisor y vale **`"4"`**.

---

## Errores

| Situación | `reason` | Código |
|---|---|---|
| No se pudo inicializar el registro de voces | `voice_store_init_failed` | 1 |
| Fallo de descarga de un snapshot (invocación directa) | `network_error` | 20 |
| Fallo de conversión de un derivado (invocación directa) | `setup_failed` | 11 |
| El mismo fallo, invocado desde `self install` | `setup_failed` (11) con la causa en `models_cause` | 11 |

`ct2_conversion_failed` **no es un `reason` de primer nivel de la invocación directa**: es el `reason` anidado que viaja en `models_cause` cuando el fallo lo sufre `self install`. Su valor declarado es **1**, el del error genérico, y el proceso sale con el de la operación.

---

## Ejemplos

```bash
ai-voice-interconnector setup                          # descarga la selección base (idempotente)
ai-voice-interconnector setup --with-voice-cloning     # incluye el Base de clonado (~2,5 GB)
ai-voice-interconnector setup --force-update           # purga la selección y re-descarga (confirma en TTY)
ai-voice-interconnector setup --force-update --yes     # ídem, no interactivo
ai-voice-interconnector --json setup                   # payload legible por máquina
```
