# `setup`

Provisiona el runtime: chequeos de entorno + descarga de los modelos pinneados
desde HuggingFace y derivación del modelo CT2 de traducción. Es idempotente: una
segunda ejecución solo registra los snapshots ya presentes.

Implementación: `handle_setup` (`src/main.rs`), apoyado en `avi-store`
(`crates/avi-store/src/lib.rs`: `MODEL_REVISIONS`, `ensure_downloaded`,
`is_provisioned`, `remove_hf_snapshot`, `remove_xet_cache`, `is_ct2_provisioned`).

---

## Superficie CLI

```
ai-voice-interconnector setup [--with-voice-cloning] [--with-stt] [--force-update] [--yes|-y] [--json]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--with-voice-cloning` | flag | `false` | Provisiona además el modelo Base de clonado `qwen3-tts-0.6b-base` (~2,5 GB), requerido por `voice clone` |
| `--with-stt` | flag | `false` | Aceptado por compatibilidad; **redundante**: `parakeet-tdt-v3` ya se provisiona siempre. Solo emite un aviso informativo |
| `--force-update` | flag | `false` | Purga los snapshots pinneados (respetando la selección de clonado) y la caché xet, luego re-descarga desde cero. Confirma antes de purgar salvo `--yes` o entrada no interactiva |
| `--yes`, `-y` | flag | `false` | Omite la confirmación destructiva de `--force-update`. Sin `--force-update` es un no-op inocuo |
| `--json` | flag global | `false` | Emite JSON legible por máquina en stdout |

No existe `--language`: el conjunto de modelos provisionados es fijo (es+en
offline completo desde el primer uso).

---

## Flujo de provisión

```
handle_setup
    │
    ▼
VoiceStore::ensure_initialized        ← crea el directorio de datos y materializa la voz `default`
    │
    ▼
[--force-update] purga incondicional  ← confirmación destructiva salvo --yes / no-TTY
    │  remove_hf_snapshot(name) por cada modelo de la selección
    │  remove_xet_cache() una vez
    ▼
Bucle sobre MODEL_REVISIONS (filtrado por selección de clonado)
    │  is_provisioned(name) == true  → skip (snapshot HF ya presente)
    │  is_provisioned(name) == false → ensure_downloaded(name) (hf-hub, revisión fijada)
    ▼
Derivación CT2 de traducción (es-en, en-es)
    │  para cada par con Marian HF provisionado:
    │    is_ct2_provisioned + gate por mtime (ct2 > hf) → skip
    │    en otro caso → convert_marian_to_ct2 (escritura atómica, reconversión del dir roto)
    ▼
Salida (JSON o mensaje humano)
```

La purga de `--force-update` no re-deriva el CT2 explícitamente: al re-descargar
los snapshots Marian con `mtime` nuevo, el gate por `mtime` de la sección de
derivación dispara la reconversión por sí solo.

---

## Modelos provisionados (`MODEL_REVISIONS`)

Cada modelo se fija por **commit hash** de HuggingFace (reproducible; un push
upstream no se propaga a los usuarios). Fuente: `crates/avi-store/src/lib.rs`.

| Nombre lógico | Repo HF | Rol | Selección |
|---|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Motor TTS (síntesis) | Siempre |
| `marian-es-en` | `Helsinki-NLP/opus-mt-es-en` | Traducción es→en (derivado a CT2) | Siempre |
| `marian-en-es` | `Helsinki-NLP/opus-mt-en-es` | Traducción en→es (derivado a CT2) | Siempre |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (4 artefactos int8 vía `MODEL_FILE_PATTERNS`) | Siempre |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Modelo Base de clonado de voz | Opt-in `--with-voice-cloning` |

Peso aproximado: ~9 GB base, ~11,5 GB con `--with-voice-cloning`. Los modelos
se descargan a la caché HF del usuario (`hf_cache_dir()`, respeta
`HF_HUB_CACHE`/`HF_HOME` → `~/.cache/huggingface/hub`).

El derivado CT2 de traducción (`model.bin` + `source.spm`+`target.spm`) vive en
`hf_cache_dir/ct2` y se convierte desde los snapshots Marian con `ctranslate2`.

---

## Integridad e idempotencia

- `is_provisioned(name)` valida no solo la existencia del snapshot sino también
  los ficheros críticos con `size>0`, evitando caché truncada que pasa
  `.exists()` pero revienta al cargar.
- `ensure_downloaded` valida y hace rollback de snapshot+blobs ante descarga parcial.
- La conversión CT2 es atómica (dir temporal hermano + `rename`); un fallo nunca
  deja un parcial que el gate acepte, y un dir roto se sustituye por reconversión.

---

## Contrato `--json`

Con `--json`, la salida en stdout es:

| Clave | Tipo | Significado |
|---|---|---|
| `status` | string | `"completed"` (o `"cancelled"` si se declina la confirmación de `--force-update`) |
| `with_stt` | boolean | Espejo del flag `--with-stt` |
| `models_provisioned` | array de strings | Nombres lógicos registrados en esta ejecución |

No hay clave `language`. Los mensajes de progreso y la purga van a stderr,
reservando stdout para el JSON.

---

## Errores

| Reason | Código | Causa |
|---|---|---|
| `voice_store_init_failed` | Error | No se pudo inicializar el `VoiceStore`/directorio de datos |
| `model_download_failed` | Error | Falló la descarga de un snapshot HF (red, credenciales, disco) |
| `model_provision_failed` | Error | Falló el registro del snapshot en el índice |
| `ct2_conversion_failed` | Error | Snapshot Marian no resoluble/ausente, o falló la conversión CT2 (falta `ctranslate2`) |

---

## Ejemplos

```bash
ai-voice-interconnector setup                          # descarga los 4 base (idempotente)
ai-voice-interconnector setup --with-voice-cloning     # incluye el Base de clonado (~2,5 GB)
ai-voice-interconnector setup --force-update           # purga + re-descarga (confirma en TTY)
ai-voice-interconnector setup --force-update --yes     # ídem, no interactivo
ai-voice-interconnector --json setup                   # payload legible por máquina
```
