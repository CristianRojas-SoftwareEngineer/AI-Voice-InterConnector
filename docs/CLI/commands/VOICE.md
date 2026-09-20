# `voice`

Gestor del registro de voces (fábrica + clonadas): listar, clonar desde audio
de referencia y eliminar. Es el único comando con despacho de 3 modos hacia el
daemon para `clone` (delegable vía `POST /voices/clone`); `list` y `remove`
son siempre locales (rechazan `--daemon` con `daemon_unreachable`, paridad con
`speech dub`/`speech play`).

Implementación: `handle_voice` (`src/main.rs:698`), apoyado en `avi-store`
(`crates/avi-store/src/lib.rs`: `VoiceStore`, `FACTORY_VOICES`,
`is_factory_name`) y, en la ruta daemon, `clone_via_daemon` (`src/main.rs:3237`)
contra `voices_clone_handler` (`crates/avi-daemon/src/lib.rs:750`).

---

## Superficie CLI

```
ai-voice-interconnector voice list
ai-voice-interconnector voice clone --name NAME --speech-reference FILE [--timbre-reference FILE] [--force]
ai-voice-interconnector voice remove --name NAME
```

`enum VoiceCommands` (`src/main.rs:193`). Los flags `--daemon`/`--no-daemon`/`--json`
son globales de `Cli` (`src/main.rs:86-93`), no propios de `voice`.

**`voice clone`** (`src/main.rs:197-209`):

| Flag | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--name, -n` | string | sí | Nombre de la voz; se normaliza a minúsculas y se valida contra `VoiceStore::validate_name` |
| `--speech-reference, -s` | string (ruta) | sí | Audio de referencia de habla, obligatorio en toda ruta (local y daemon) |
| `--timbre-reference, -t` | string (ruta) | no | Audio de timbre opcional; si se omite, la referencia de habla cubre ambos roles |
| `--force, -f` | flag | no | Sobrescribe una voz existente con el mismo nombre |

**`voice list`** y **`voice remove`** no tienen flags propios más allá de `--name` en `remove` (`-n`, obligatorio) y los globales.

No existe `--daemon`/`--no-daemon` mutuamente excluyente propio de `voice`
(son globales de `Cli`); no existen banderas `--yes`/`--force-update`; no hay
subcomando `voice precompute` (el endpoint correspondiente fue purgado, ver
más abajo).

---

## Flujo de `voice clone`

```
handle_voice (Clone)
    │
    ▼
name.to_lowercase() + VoiceStore::validate_name(name)   ← exit 2 "invalid_voice_name" si falla
    │
    ▼
route_to_daemon(daemon_mode, client)?
    │
    ├─ Sí ──► clone_via_daemon (src/main.rs:3237)
    │           lee speech/timbre a base64 → POST /voices/clone (timeout 1500ms)
    │           timeout o conexión fallida → exit 5 "daemon_unreachable"
    │           HTTP no-2xx → mapea `reason` del body a exit code (ver tabla de errores)
    │           2xx → reemite {name, speech, timbre, precomputed} con schema_version
    │
    └─ No ──► require_model_provisioned()                ← exit 4 "model_missing" si falta qwen3-tts-0.6b
                │
                ▼
              validar existencia de speech_reference (y timbre_reference si se dio)  ← exit 3 "audio_not_found"
                │
                ▼
              !force && voice_store.exists(name)          ← exit 6 "voice_exists"
                │
                ▼
              Qwen3TtsEngine::new(None).base_model_dir     ← exit 4 "model_missing" si el Base de clonado no está provisionado
                │
                ▼
              avi_tts::clone_voice(model_dir, speech_path, tmp_qvoice, name, "es")
                │
                ▼
              voice_store.save_reference(name, tmp_qvoice) → <voces>/<name>/reference.qvoice
                │
                ▼
              copia speech_path → speech-reference.wav (y timbre si se dio) para compatibilidad de lectura
                │
                ▼
              emitir {name, timbre, speech, precomputed:false}
```

`route_to_daemon` (`src/main.rs:2875`): `ForceDaemon` siempre delega (el POST
falla con `daemon_unreachable` si no hay daemon corriendo); `ForceDirect`
nunca delega; `Auto` delega solo si `GET /health` responde en ≤500 ms.

**No hay precómputo de conditionals en ningún camino.** El campo `precomputed`
del envelope es siempre `false`: ni la ruta local ni la ruta daemon calculan
conditionals por adelantado. El endpoint `POST /voices/precompute` que existía
en versiones previas fue purgado del router (`crates/avi-daemon/src/lib.rs`
expone 7 rutas públicas, sin `/voices/precompute` ni `GET /voices`; ver
`docs/reviews/2026-09-10-hallazgos-pendientes-consolidado.md`, hallazgo H-07).
La primera síntesis (`speech synthesize --voice <nombre>`) es la que resuelve
los conditionals bajo demanda a partir de `reference.qvoice`.

---

## Ruta daemon: `POST /voices/clone`

`clone_via_daemon` (`src/main.rs:3237`) codifica los audios a base64 y envía:

```json
{ "name": "...", "audio_b64": "...", "force": false, "timbre_b64": "..." }
```

`voices_clone_handler` (`crates/avi-daemon/src/lib.rs:750`):

1. `VoiceStore::validate_name(name)` → `400` `invalid_voice_name` si falla.
2. `!force && voice_store.exists(name)` → `409` `voice_exists`.
3. Falta `audio_b64` → `400` `audio_missing`; no decodifica base64 → `400` `audio_decode_error`.
4. `tts_engine.base_model_dir` ausente (Base de clonado no provisionado) → `404` `model_missing`.
5. Escribe el audio a un WAV temporal, `avi_tts::clone_voice(base_model_dir, tmp_wav, tmp_qvoice, name, "es")`
   (constante `DEFAULT_CLONE_LANGUAGE = "es"`, `crates/avi-daemon/src/lib.rs:37`) → `500` `voice_clone_failed` si falla.
6. `voice_store.save_reference(name, tmp_qvoice)` → `reference.qvoice`; copia `speech-reference.wav`; si vino `timbre_b64`, copia `timbre-reference.wav` (mejor esfuerzo, sin abortar si falla).
7. Responde `200` con `{name, speech, precomputed:false}` + `schema_version`.

El CLI (`clone_via_daemon`) mapea la `reason` del cuerpo de error a exit code:

| `reason` del daemon | Exit code |
|---|---|
| `invalid_voice_name` | 2 (`InvalidInput`) |
| `voice_exists` | 6 (`StateConflict`) |
| `model_missing` | 4 (`ModelMissing`) |
| `audio_missing` / `audio_decode_error` | 2 (`InvalidInput`) |
| otro / desconocido | 1 (`Error`) |

---

## `voice list`

`require_local(daemon_mode)` rechaza `--daemon` explícito con exit 5 antes de
tocar el store. Delega en `VoiceStore::list()` (`crates/avi-store/src/lib.rs:122`):
escanea `<data_dir>/voices/`, marca `is_factory` con `is_factory_name` y
ordena fábrica primero (`default`, `ryan`, `vivian`), luego clonadas
alfabéticamente. `ensure_initialized()` materializa las voces de fábrica (y el
`reference.qvoice` embebido de `default`) antes de listar, por lo que
`default` siempre aparece.

Salida humana: `Voces registradas:` con sufijo ` (fábrica)` para las tres
voces base. Salida `--json`: `{ "voices": [...] }` (solo nombres, sin
metadatos de ruta).

---

## `voice remove`

`require_local(daemon_mode)` rechaza `--daemon` con exit 5. Flujo
(`src/main.rs:818-840`):

1. `VoiceStore::validate_name(name)` → exit 2 `invalid_voice_name`.
2. `is_factory_name(name)` → exit 2 `cannot_remove_default` (nota: pese al
   nombre de la razón, protege las tres voces de fábrica —`default`, `ryan`,
   `vivian`—, no solo `default`).
3. `voice_store.remove(name)` (`crates/avi-store/src/lib.rs:181`): normaliza a
   minúsculas, vuelve a rechazar nombres de fábrica, y falla si el directorio
   no existe → exit 3 `voice_not_found`. Si existe, `remove_dir_all`
   incondicional (sin distinguir «archivo en uso»: en este árbol no hay una
   rama de manejo específico para `PermissionError` de Windows).
4. Salida `--json`: `{ "status": "removed", "voice": "<nombre>" }`.

---

## Contrato `--json`

| Subcomando | Payload (más `schema_version`) |
|---|---|
| `voice list` | `{ "voices": ["default", "ryan", "vivian", ...] }` |
| `voice clone` | `{ "name": "...", "timbre": "..."\|null, "speech": "<ruta a reference.qvoice>", "precomputed": false }` |
| `voice remove` | `{ "status": "removed", "voice": "<nombre>" }` |

`precomputed` es siempre `false`: no existe ninguna ruta (local o daemon) que
la ponga en `true`. `schema_version` (`"3"`) lo añade `emit_raw_json`
(`crates/avi-core/src/json_emitter.rs:5`) en el CLI, y `with_sv` en las
respuestas del daemon.

---

## Almacenamiento

`VoiceStore` (`crates/avi-store/src/lib.rs:66`) usa un único nivel físico en
`<data_dir>/voices/<nombre>/`, sin separación fábrica/usuario en disco: las
tres voces de fábrica (`FACTORY_VOICES = ["default", "ryan", "vivian"]`,
`crates/avi-store/src/lib.rs:16`) se materializan como directorios normales en
`ensure_initialized()`, y `is_factory_name` es lo único que las distingue de
una voz clonada al listar o al intentar eliminarlas.

**Estructura de una voz clonada:**

```
<nombre>/
├── reference.qvoice        ← graft binario (speaker embedding + pesos Base); usado por el motor
├── speech-reference.wav    ← copia del audio de origen, solo para compatibilidad de lectura
└── timbre-reference.wav    ← copia del audio de timbre, si se proporcionó
```

`reference.qvoice` es lo único que el motor de síntesis consulta
(`VoiceStore::find_reference`, `crates/avi-store/src/lib.rs:197`): su presencia
determina la rama «clonada» en `avi_tts::resolve_voice_motor`; sin él, la voz
resuelve como preset del motor. `default` es una voz de fábrica *clonada*
(trae su propio `reference.qvoice` embebido en el binario,
`FACTORY_DEFAULT_QVOICE`) para garantizar una tasa de error de palabra baja en
textos cortos; `ryan`/`vivian` son presets puros del motor Qwen3-TTS, sin
`reference.qvoice`.

`voice_store.save_reference` (`crates/avi-store/src/lib.rs:213`) escribe con
temporal + `rename` (sin dejar un `.qvoice` parcial ante fallo a mitad de
copia).

---

## Errores

| Condición | Subcomando | Exit code | `reason` |
|---|---|---|---|
| Nombre inválido (regex/longitud/`..`/separadores) | clone, remove | 2 | `invalid_voice_name` |
| Audio de referencia (o timbre) inexistente | clone (ruta local) | 3 | `audio_not_found` |
| Modelo de síntesis (`qwen3-tts-0.6b`) no provisionado | clone (ruta local) | 4 | `model_missing` |
| Voz ya existe sin `--force` | clone | 6 | `voice_exists` |
| Modelo Base de clonado no provisionado | clone (ruta local) | 4 | `model_missing` |
| Falla `avi_tts::clone_voice` o `save_reference` | clone | 1 | `voice_clone_failed` |
| Daemon inalcanzable (timeout 1500 ms) en ruta `--daemon`/`Auto`-daemon | clone | 5 | `daemon_unreachable` |
| Error mapeado desde el body del daemon (ver tabla de la ruta daemon) | clone | 2/4/6/1 | según `reason` recibida |
| Voz de fábrica (`default`/`ryan`/`vivian`) | remove | 2 | `cannot_remove_default` |
| Voz no encontrada | remove | 3 | `voice_not_found` |
| `--daemon` explícito en `list`/`remove` | list, remove | 5 | `daemon_unreachable` |

---

## Ejemplos

```bash
ai-voice-interconnector voice list
ai-voice-interconnector --json voice list
ai-voice-interconnector voice clone --name locutor --speech-reference ref.wav
ai-voice-interconnector voice clone -n locutor -s ref.wav -t timbre.wav --force
ai-voice-interconnector --daemon voice clone -n locutor -s ref.wav   # exige daemon vivo; exit 5 si no responde
ai-voice-interconnector --no-daemon voice clone -n locutor -s ref.wav  # fuerza ruta local, sin sondear el daemon
ai-voice-interconnector voice remove --name locutor
```
