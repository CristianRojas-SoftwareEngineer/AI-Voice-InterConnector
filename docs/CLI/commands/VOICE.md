# `voice`

Gestor del registro de voces (fábrica + clonadas): listar, clonar desde audio
de referencia y eliminar. Es el único comando con despacho de 3 modos hacia el
daemon para `clone` (delegable vía `POST /voices/clone`); `list` y `remove`
son siempre locales (rechazan `--daemon` con `daemon_unreachable`, paridad con
`speech dub`/`speech play`).

Implementación: `handle_voice` (`src/main.rs:752`), apoyado en `avi-store`
(`crates/avi-store/src/lib.rs`: `VoiceStore`, `FACTORY_VOICES`,
`is_factory_name`) y, en la ruta daemon, `clone_via_daemon` (`src/main.rs:3853`)
contra `voices_clone_handler` (`crates/avi-daemon/src/lib.rs:804`).

---

## Superficie CLI

```
ai-voice-interconnector voice list
ai-voice-interconnector voice clone --name NAME --speech-reference FILE [--timbre-reference FILE] [--force]
ai-voice-interconnector voice remove --name NAME
```

`enum VoiceCommands` (`src/main.rs:213`). Los flags `--daemon`/`--no-daemon`/`--json`
son globales de `Cli` (`src/main.rs:102-116`), no propios de `voice`.

**`voice clone`** (`src/main.rs:217-229`):

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
    ├─ Sí ──► clone_via_daemon (src/main.rs:3853)
    │           lee speech/timbre a base64 → POST /voices/clone (cabeceras ≤1500ms)
    │           timeout o conexión fallida → exit 5 "daemon_unreachable"
    │           HTTP no-2xx → mapea `reason` del body a exit code (ver tabla de errores)
    │           stream NDJSON: started → latidos (500ms) / warmup → result {name, speech, timbre, precomputed:true}
    │           inactividad >1500ms o corte sin final → exit 5 "daemon_unreachable" / exit 1 "daemon_error"
    │           result → reemite con schema_version
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
              emitir {name, timbre, speech, precomputed:false}
```

`route_to_daemon` (`src/main.rs:3314`): `ForceDaemon` siempre delega (el POST
falla con `daemon_unreachable` si no hay daemon corriendo); `ForceDirect`
nunca delega; `Auto` delega solo si `GET /health` responde en ≤500 ms.

**Precarga en caliente (warm-on-clone), solo en la ruta daemon.** El campo
`precomputed` del envelope depende de la ruta: la **ruta daemon** dispara, tras
clonar, el precalentamiento en segundo plano de la voz recién clonada (evicciona
la voz caliente previa; el residente TTS es de una sola voz) y responde
`precomputed: true` con la semántica «precarga en caliente iniciada» —la
completitud real se refleja en `GET /health` (`warm`). La **ruta local** responde
始终 `precomputed: false`: su motor TTS es efímero por proceso, no hay
residente persistente que calentar. El antiguo endpoint `POST /voices/precompute`
fue purgado (`crates/avi-daemon/src/lib.rs` expone 7 rutas públicas, sin
`/voices/precompute` ni `GET /voices`); la precarga vive ahora en el propio
handler de clonado.

---

## Ruta daemon: `POST /voices/clone`

`clone_via_daemon` (`src/main.rs:3853`) codifica los audios a base64 y envía:

```json
{ "name": "...", "audio_b64": "...", "force": false, "timbre_b64": "..." }
```

`voices_clone_handler` (`crates/avi-daemon/src/lib.rs:804`):

1. `VoiceStore::validate_name(name)` → `400` `invalid_voice_name` si falla.
2. `!force && voice_store.exists(name)` → `409` `voice_exists`.
3. Falta `audio_b64` → `400` `audio_missing`; no decodifica base64 → `400` `audio_decode_error`.
4. `tts_engine.base_model_dir` ausente (Base de clonado no provisionado) → `404` `model_missing`.
5. Tras superar las validaciones baratas anteriores, el handler abre una respuesta streaming NDJSON (`application/x-ndjson`) emitiendo el evento inicial `{"event":"started", "name": "..."}`.
6. El clonado pesado corre en `spawn_blocking(avi_tts::clone_voice)` envuelto en `con_latidos`: el daemon emite latidos periódicos (`{"event":"heartbeat", "stage":"clone"}`) cada 500 ms (`STREAM_HEARTBEAT`). Si el cliente se desconecta, `AbortHandle` aborta la inferencia.
7. Al completar el clonado, `voice_store.save_reference(name, tmp_qvoice)` persiste `reference.qvoice` (único archivo que el motor consulta; sin copia de los WAV de entrada).
8. Lanza en segundo plano el warm-on-clone de la voz recién clonada (`precalentar_voz`, emitiendo `{"event":"progress", "stage":"warmup"}`) y emite el evento final `{"event":"result", "name": "...", "speech": "...", "timbre": ..., "precomputed": true}` + `schema_version`. El calentamiento (~18-40 s) no bloquea el flujo: `precomputed: true` significa «precarga en caliente iniciada». Si el clonado falla, emite `{"event":"error", "reason":"voice_clone_failed", "message": "..."}`.

El cliente (`clone_via_daemon`) consume el stream mediante `consumir_stream_ndjson` con un timeout de inactividad entre latidos de 1500 ms (`STREAM_INACTIVITY_TIMEOUT`) y un deadline failsafe de 120 s (`STREAM_TOTAL_DEADLINE`), mapeando `reason` a exit code:

| `reason` del daemon | Exit code |
|---|---|
| `invalid_voice_name` | 2 (`InvalidInput`) |
| `voice_exists` | 6 (`StateConflict`) |
| `model_missing` | 4 (`ModelMissing`) |
| `audio_missing` / `audio_decode_error` | 2 (`InvalidInput`) |
| `voice_clone_failed` | 1 (`Error`) |
| otro / desconocido | 1 (`Error`) |

---

## `voice list`

`require_local(daemon_mode)` rechaza `--daemon` explícito con exit 5 antes de
tocar el store. Delega en `VoiceStore::list()` (`crates/avi-store/src/lib.rs:115`):
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
(`src/main.rs:863-885`):

1. `VoiceStore::validate_name(name)` → exit 2 `invalid_voice_name`.
2. `is_factory_name(name)` → exit 2 `cannot_remove_default` (nota: pese al
   nombre de la razón, protege las tres voces de fábrica —`default`, `ryan`,
   `vivian`—, no solo `default`).
3. `voice_store.remove(name)` (`crates/avi-store/src/lib.rs:174`): normaliza a
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
| `voice clone` | `{ "name": "...", "timbre": "..."\|null, "speech": "<ruta a reference.qvoice>", "precomputed": <bool> }` |
| `voice remove` | `{ "status": "removed", "voice": "<nombre>" }` |

`precomputed` depende de la ruta: `true` en la ruta daemon (warm-on-clone
iniciado; completitud en `GET /health`) y `false` en la ruta local (motor
efímero, sin residente que calentar). `schema_version` (`"3"`) lo añade
`emit_raw_json` (`crates/avi-core/src/json_emitter.rs:5`) en el CLI, y `with_sv`
en las respuestas del daemon.

---

## Almacenamiento

`VoiceStore` (`crates/avi-store/src/lib.rs:75`) usa un único nivel físico en
`<data_dir>/voices/<nombre>/`, sin separación fábrica/usuario en disco: las
tres voces de fábrica (`FACTORY_VOICES = ["default", "ryan", "vivian"]`,
`crates/avi-store/src/lib.rs:25`) se materializan como directorios normales en
`ensure_initialized()`, y `is_factory_name` es lo único que las distingue de
una voz clonada al listar o al intentar eliminarlas.

**Estructura de una voz clonada:**

```
<nombre>/
└── reference.qvoice        ← graft binario (speaker embedding + pesos Base); único archivo persistido
```

Los WAV de entrada (`--speech-reference`/`--timbre-reference`) se consumen
para producir el graft y no se copian al almacén: `reference.qvoice` es lo
único que el motor de síntesis consulta
(`VoiceStore::find_reference`, `crates/avi-store/src/lib.rs:190`): su presencia
determina la rama «clonada» en `avi_tts::resolve_voice_motor`; sin él, la voz
resuelve como preset del motor. `default` es una voz de fábrica *clonada*
(trae su propio `reference.qvoice` embebido en el binario,
`FACTORY_DEFAULT_QVOICE`) para garantizar una tasa de error de palabra baja en
textos cortos; `ryan`/`vivian` son presets puros del motor Qwen3-TTS, sin
`reference.qvoice`.

`voice_store.save_reference` (`crates/avi-store/src/lib.rs:206`) escribe con
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
| Daemon inalcanzable (inactividad 1500 ms / conexión fallida) en ruta `--daemon`/`Auto`-daemon | clone | 5 | `daemon_unreachable` |
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
