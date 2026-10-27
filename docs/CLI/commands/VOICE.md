# `voice`

Gestor del registro de voces (fábrica + clonadas): listar, clonar desde audio
de referencia y eliminar. `voice clone` admite el
[despacho al daemon](../CONTRACT.md#5-el-despacho-al-daemon) (delegable vía
`POST /voices/clone`); `list` y `remove`
son siempre locales: no declaran `--daemon` ni `--no-daemon` y el parser los
rechaza como argumento desconocido (exit 2 `usage_error`), igual que
`speech list`/`speech play`/`speech remove`.

Implementación: `handle_voice` (`src/main.rs`), apoyado en `avi-store`
(`crates/avi-store/src/lib.rs`: `VoiceStore`, `FACTORY_VOICES`,
`is_factory_name`) y, en la ruta daemon, `clone_via_daemon` (`src/main.rs`)
contra `voices_clone_handler` (`crates/avi-daemon/src/lib.rs`).

---

## Superficie CLI

```
ai-voice-interconnector voice list
ai-voice-interconnector voice clone --name NAME --speech-reference FILE [--force] [--daemon|--no-daemon]
ai-voice-interconnector voice remove --name NAME [--yes]
```

`enum VoiceCommands` (`src/main.rs`). `--json` es global de `Cli`
(`src/main.rs`), no propio de `voice`. `--daemon`/`--no-daemon` son propios de
`voice clone` (mutuamente excluyentes) y se escriben tras el subcomando.

**`voice clone`** (`src/main.rs`):

| Flag | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--name, -n` | string | sí | Nombre de la voz; se normaliza a minúsculas y se valida contra `VoiceStore::validate_name` |
| `--speech-reference, -s` | string (ruta) | sí | Audio de referencia de habla, obligatorio en toda ruta (local y daemon) |
| `--denoise` | flag | no | Limpia la referencia antes de extraer la huella vocal; requiere `native-denoise` y el modelo DPDFNet provisionado. La opción no garantiza por sí misma preservación de identidad vocal; esa calidad no ha sido evaluada en una prueba humana controlada |
| `--force, -f` | flag | no | Sobrescribe una voz existente con el mismo nombre |
| `--daemon` / `--no-daemon` | flag | no | Despacho tri-modal; mutuamente excluyentes |

**`voice remove`** (`src/main.rs`):

| Flag | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--name, -n` | string | sí | Nombre de la voz a eliminar |
| `--yes, -y` | flag | no | Omite la confirmación cuando la voz tiene locuciones guardadas |

**`voice list`** no tiene flags propios más allá del global `--json`.

No existe `--force-update`; no hay
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
    ├─ Sí ──► clone_via_daemon
    │           (previo al POST: referencia > MAX_CLONE_REFERENCE_SECS → exit 2 "audio_too_long")
    │           lee speech a base64 → POST /voices/clone (cabeceras con timeout)
    │           timeout o conexión fallida → exit 5 "daemon_unreachable"
    │           HTTP no-2xx → mapea `reason` del body a exit code (ver tabla de errores)
    │           stream NDJSON: started → latidos / warmup → result {name, speech, precomputed:true}
    │           inactividad (`STREAM_INACTIVITY_TIMEOUT`) o corte sin final → exit 5 "daemon_unreachable" / exit 1 "daemon_error"
    │           result → reemite con schema_version
    │
    └─ No ──► require_model_provisioned()                ← exit 4 "model_missing" si falta qwen3-tts-0.6b
                │
                ▼
              validar existencia de speech_reference  ← exit 3 "audio_not_found"
                │
                ▼
              !force && voice_store.exists(name)          ← exit 6 "voice_exists"
                │
                ▼
              Qwen3TtsEngine::new(None).base_model_dir     ← exit 4 "model_missing" si el Base de clonado no está provisionado
                │
                ▼
              avi_tts::clone_voice(model_dir, speech_path, tmp_qvoice, name)
                ← el motor se lanza con --xvector-only: guarda solo la huella de timbre;
                  sin transcripción ni idioma, y sin depender del STT (funciona igual con o sin native-stt)
                │
                ▼
              voice_store.save_reference(name, tmp_qvoice) → <voces>/<name>/reference.qvoice
                │
                ▼
              emitir {name, speech, precomputed:false}
```

**La salida del motor no se vuelca en la terminal**, ni en la ruta directa ni en la del daemon: el motor de clonado escribe en `data/logs/qwen3-tts_<token>_<ms>.log` (`<token>` identifica al proceso que lo crea: la CLI en la ruta directa, el daemon en la otra), con retención de los 10 más recientes. Cuando el clonado falla, el error `voice_clone_failed` termina con `Log del motor: <ruta>`.

`route_to_daemon` (`src/main.rs`): `ForceDaemon` siempre delega (el POST
falla con `daemon_unreachable` si no hay daemon corriendo); `ForceDirect`
nunca delega; `Auto` delega solo si `GET /health` responde dentro del plazo de `probe_health` (`src/main.rs`).

**Precarga en caliente (warm-on-clone), solo en la ruta daemon.** El campo
`precomputed` del envelope depende de la ruta: la **ruta daemon** dispara, tras
clonar, el precalentamiento en segundo plano de la voz recién clonada (evicciona
la voz caliente previa; el residente TTS es de una sola voz) y responde
`precomputed: true` con la semántica «precarga en caliente iniciada» —la
completitud real se refleja en `GET /health` (`warm`). La **ruta local** responde
siempre `precomputed: false`: su motor TTS es efímero por proceso, no hay
residente persistente que calentar. La precarga vive en el propio handler de
clonado: el daemon no expone ninguna ruta aparte para precargar voces.

---

## Ruta daemon: `POST /voices/clone`

`clone_via_daemon` (`src/main.rs`) codifica la referencia a base64 y envía:

```json
{ "name": "...", "audio_b64": "...", "force": false }
```

La referencia dura como máximo `MAX_CLONE_REFERENCE_SECS` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)), solo se admiten WAV, y la CLI lo comprueba, midiendo la cabecera y luego el PCM decodificado, antes de contactar con el daemon y antes de comprobar el modelo; un exceso sale con exit 2 `audio_too_long`, igual en la vía local. El cuerpo de la petición admite hasta el tope que calcula `body_limit_for` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) a partir de esa duración: un WAV más pesado recibe 413 aunque dure menos.

`voices_clone_handler` (`crates/avi-daemon/src/lib.rs`):

1. `VoiceStore::validate_name(name)` → `400` `invalid_voice_name` si falla.
2. `!force && voice_store.exists(name)` → `409` `voice_exists`.
3. Falta `audio_b64` → `400` `usage_error`; no decodifica base64 → `400` `invalid_audio`; la referencia supera `MAX_CLONE_REFERENCE_SECS` (medida por su cabecera WAV) → `400` `audio_too_long`.
4. `tts_engine.base_model_dir` ausente (Base de clonado no provisionado) → `404` `model_missing`.
5. Tras superar las validaciones baratas anteriores, el handler abre una respuesta streaming NDJSON (`application/x-ndjson`) emitiendo el evento inicial `{"event":"started", "operation": "clonación", "name": "..."}`. La clave `operation` nombra el trabajo, de modo que el cliente anuncia «Iniciando clonación…» en vez de una operación genérica.
6. El clonado pesado corre en `spawn_blocking(avi_tts::clone_voice)` envuelto en `with_heartbeats`: el daemon emite latidos periódicos (`{"event":"heartbeat", "stage":"clone"}`) con la cadencia de `STREAM_HEARTBEAT` (`crates/avi-daemon/src/lib.rs`). Si el cliente se desconecta, el daemon descarta el resultado; el clonado termina en segundo plano. Si la referencia no se puede cargar, el clonado se interrumpe y emite `{"event":"error", "reason":"invalid_audio", "message": "..."}` (WAV inválido o truncado) o `{"event":"error", "reason":"io_error", "message": "..."}` (fallo de lectura); si falla el propio clonado o el guardado, emite `{"event":"error", "reason":"voice_clone_failed", "message": "..."}`.
7. Al completar el clonado, `voice_store.save_reference(name, tmp_qvoice)` persiste `reference.qvoice` (único archivo que el motor consulta; sin copia de los WAV de entrada).
8. Lanza en segundo plano el warm-on-clone de la voz recién clonada (`warm_voice_engine`, emitiendo `{"event":"progress", "stage":"warmup"}`) y emite el evento final `{"event":"result", "name": "...", "speech": "...", "precomputed": true}` + `schema_version`. El calentamiento (acotado por el arranque del residente más el presupuesto del testigo) no bloquea el flujo: `precomputed: true` significa «precarga en caliente iniciada».

El cliente (`clone_via_daemon`) consume el stream mediante `consume_ndjson_stream` con un timeout de inactividad entre latidos (`STREAM_INACTIVITY_TIMEOUT`) y un deadline failsafe (`STREAM_TOTAL_DEADLINE`, el `REQUEST_FAILSAFE` de [`limits.rs`](../../../crates/avi-core/src/limits.rs)), mapeando `reason` a exit code con la tabla única
`ExitCode::from_reason`, tanto en las respuestas no 2xx como en los eventos
`error` del stream:

| `reason` del daemon | Exit code |
|---|---|
| `invalid_voice_name` | 2 (`InvalidInput`) |
| `voice_exists` | 6 (`StateConflict`) |
| `model_missing` | 4 (`ModelMissing`) |
| `usage_error` / `invalid_audio` | 2 (`InvalidInput`) |
| `audio_too_long` | 2 (`InvalidInput`) |
| `io_error` | 1 (`Error`) |
| `voice_clone_failed` | 1 (`Error`) |
| otro / desconocido | 1 (`Error`) |

---

## `voice list`

Siempre local: no declara `--daemon` ni `--no-daemon`. Delega en `VoiceStore::list()` (`crates/avi-store/src/lib.rs`):
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

Siempre local: no declara `--daemon` ni `--no-daemon`. Flujo
(`src/main.rs`):

1. `VoiceStore::validate_name(name)` → exit 2 `invalid_voice_name`.
2. `is_factory_name(name)` → exit 2 `cannot_remove_default` (nota: pese al
   nombre de la razón, protege las tres voces de fábrica —`default`, `ryan`,
   `vivian`—, no solo `default`).
3. La voz no existe en el registro → exit 3 `voice_not_found`, antes de
   preguntar nada.
4. **Las locuciones se van con su voz.** Se leen las etiquetas del namespace de
   habla de la voz (`SpeechStore::labels_of`, `<data_dir>/speech/<voz>/`; el WAV es
   el registro y no se lee ningún sidecar). Si hay alguna, el comando es
   destructivo y aplica la misma confirmación que `cleanup`: imprime a stderr el
   plan (la voz y sus locuciones, con rutas y tamaños) y, con terminal, pregunta
   `[s/N]` (por defecto, no). Sin terminal exige `--yes`; sin él sale con
   exit 2 `confirmation_required` y no borra nada. Responder que no sale con
   exit 0, imprime `Cancelado.` —por stderr, una sola vez, y solo el motor de
   confirmación lo escribe— y con `--json` emite `status: "cancelled"`. Una
   voz sin locuciones se borra sin preguntar.
5. `avi_store::remove_voice` (`crates/avi-store/src/lib.rs`) borra primero el
   namespace de habla de la voz y después su directorio del registro, de modo
   que una interrupción entre ambos pasos nunca deja locuciones de una voz
   inexistente. Es el mismo criterio de propiedad que `cleanup --voices`, que
   ubica `speech/<voz>` con el mismo `SpeechStore`. El borrado es
   `remove_dir_all` incondicional (sin distinguir «archivo en uso»: en este
   árbol no hay una rama de manejo específico para `PermissionError` de
   Windows); un fallo de E/S sale con exit 1 `io_error`.
6. Salida `--json`: `{ "status": "removed", "voice": "<nombre>", "removed_speech": ["<etiqueta>", ...] }`.

---

## Contrato `--json`

| Subcomando | Payload (más `schema_version`) |
|---|---|
| `voice list` | `{ "voices": ["default", "ryan", "vivian", ...] }` |
| `voice clone` | `{ "name": "...", "speech": "<ruta a reference.qvoice>", "precomputed": <bool> }` |
| `voice remove` | `{ "status": "removed", "voice": "<nombre>", "removed_speech": ["<etiqueta>", ...] }`; `removed_speech` es `[]` si la voz no tenía locuciones. Cancelada la confirmación: `{ "status": "cancelled", "voice": "<nombre>", "removed_speech": [] }` |

`precomputed` depende de la ruta: `true` en la ruta daemon (warm-on-clone
iniciado; completitud en `GET /health`) y `false` en la ruta local (motor
efímero, sin residente que calentar). `schema_version` (cada contrato con su versión, ver
[las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)) lo añade `emit_raw_json`
(`crates/avi-core/src/json_emitter.rs`) en el CLI, y `with_sv` en las respuestas
del daemon.

---

## Almacenamiento

`VoiceStore` (`crates/avi-store/src/lib.rs`) usa un único nivel físico en
`<data_dir>/voices/<nombre>/`, sin separación fábrica/usuario en disco: las
tres voces de fábrica (`FACTORY_VOICES = ["default", "ryan", "vivian"]`,
`crates/avi-store/src/lib.rs`) se materializan como directorios normales en
`ensure_initialized()`, y `is_factory_name` es lo único que las distingue de
una voz clonada al listar o al intentar eliminarlas.

**Estructura de una voz clonada:**

```
<nombre>/
└── reference.qvoice        ← perfil binario con solo la huella de timbre (speaker embedding, x-vector); único archivo persistido
```

El WAV de entrada (`--speech-reference`) se consume
para producir el perfil y no se copia al almacén: `reference.qvoice` es lo
único que el motor de síntesis consulta
(`VoiceStore::find_reference`, `crates/avi-store/src/lib.rs`): su presencia
determina la rama «clonada» en `avi_tts::resolve_voice_engine`; sin él, la voz
resuelve como preset del motor. Toda voz clonada se sintetiza con el modelo
Base (`qwen3-tts-0.6b-base`) y `--graft`, que aplica solo la huella de timbre y
deja la prosodia al modelo; los presets se sintetizan con el modelo CustomVoice
(`qwen3-tts-0.6b`). Sin el modelo Base provisionado, sintetizar con una voz
clonada (`default` incluida) sale con `model_missing` (exit 4; en el daemon, como evento `error` del stream de
`/synthesize` y `/dub`), sin caer a CustomVoice. `default` es una voz de fábrica *clonada*
(trae su propio `reference.qvoice` embebido en el binario,
`FACTORY_DEFAULT_QVOICE`, también con solo la huella de timbre). Se generó con la
ruta de clonación del producto a partir de `assets/audios/Speech Sampler.wav`
(~10 s), copiando al asset el `reference.qvoice` que deja en el almacén
`ai-voice-interconnector voice clone --no-daemon --name default --force --speech-reference "assets/audios/Speech Sampler.wav"`
(`--force` porque `default` ya existe como voz de fábrica).
`ryan`/`vivian` son presets puros del motor Qwen3-TTS, sin
`reference.qvoice`. Las tres hablan `es` y `en`: el idioma nativo de un
preset (`ryan`: inglés; `vivian`: chino) es informativo, porque los nueve
locutores del motor son multilingües.

`voice_store.save_reference` (`crates/avi-store/src/lib.rs`) escribe con
temporal + `rename` (sin dejar un `.qvoice` parcial ante fallo a mitad de
copia).

---

## El progreso por stderr

`voice clone` y `voice remove` pintan la fase del trabajo por stderr (`validating`, `cloning`, `saving`, `removing`) y ceden la línea antes de que el producto escriba la confirmación. El **resultado** va por stdout, y lo dice **una sola vez**, con la duración de la operación.

`AVI_NO_PROGRESS=1` silencia todo el progreso por stderr, y nada más: el resultado por stdout, el envelope `--json` y los códigos de salida no cambian.

## Errores

| Condición | Subcomando | Exit code | `reason` |
|---|---|---|---|
| Nombre inválido (regex/longitud/`..`/separadores) | clone, remove | 2 | `invalid_voice_name` |
| Audio de referencia inexistente | clone (ruta local) | 3 | `audio_not_found` |
| Modelo de síntesis (`qwen3-tts-0.6b`) no provisionado | clone (ruta local) | 4 | `model_missing` |
| Voz ya existe sin `--force` | clone | 6 | `voice_exists` |
| Modelo Base de clonado no provisionado | clone (ruta local) | 4 | `model_missing` |
| Referencia que no es un WAV válido, está truncada o declara una frecuencia de muestreo nula | clone | 2 | `invalid_audio` |
| Fallo de E/S al leer la referencia | clone | 1 | `io_error` |
| Falla el motor de `avi_tts::clone_voice` o `save_reference` (el mensaje termina con `Log del motor: <ruta>` a `data/logs/qwen3-tts_*.log`) | clone | 1 | `voice_clone_failed` |
| Daemon inalcanzable (inactividad / conexión fallida) en ruta `--daemon`/`Auto`-daemon | clone | 5 | `daemon_unreachable` |
| Error mapeado desde el body del daemon (ver tabla de la ruta daemon) | clone | 2/4/6/1 | según `reason` recibida |
| Voz de fábrica (`default`/`ryan`/`vivian`) | remove | 2 | `cannot_remove_default` |
| Voz no encontrada | remove | 3 | `voice_not_found` |
| La voz tiene locuciones, no hay terminal y falta `--yes` (no se borra nada) | remove | 2 | `confirmation_required` |
| Fallo de E/S al borrar la voz o sus locuciones | remove | 1 | `io_error` |
| Hay una operación de ciclo de vida en curso (el comando no llega a ejecutarse) | list, clone, remove | 17 | `lifecycle_locked` |
| `--daemon` o `--no-daemon` en `list`/`remove` (argumento desconocido), o juntos en `clone` | list, remove, clone | 2 | `usage_error` |

---

## Ejemplos

```bash
ai-voice-interconnector voice list
ai-voice-interconnector --json voice list
ai-voice-interconnector voice clone --name locutor --speech-reference ref.wav
ai-voice-interconnector voice clone -n locutor -s ref.wav --force
ai-voice-interconnector voice clone --daemon -n locutor -s ref.wav   # exige daemon vivo; exit 5 si no responde
ai-voice-interconnector voice clone --no-daemon -n locutor -s ref.wav  # fuerza ruta local, sin sondear el daemon
ai-voice-interconnector voice remove --name locutor
ai-voice-interconnector voice remove --name locutor --yes   # sin preguntar, también las locuciones de la voz
```

---

## Muestras de salida

El **resultado** va por stdout y lo dice una sola vez, con la duración de la operación. El progreso —la fase— va por stderr, con la forma `… <mensaje> (<tiempo>)`.

```text
# voice clone (ruta local)
stdout: Voz 'mi_voz' clonada (24 s).
stderr: … Validando referencia y nombre… (0 s)
        … Clonando voz… (puede tardar varios minutos) (1 s)
        … Guardando referencia de voz… (24 s)

# voice remove
stdout: Voz 'mi_voz' eliminada (1 s).
stderr: … Eliminando voz y locuciones… (0 s)
```

Cancelar la confirmación de `voice remove` imprime `Cancelado.` **por stderr y una sola vez** —solo lo escribe el motor de confirmación— y sale con 0. Con `--json`, stdout lleva **solo** el envelope —`{"name", "speech", "precomputed"}`, `{"status": "removed", "voice", "removed_speech"}`— y el resultado en texto no sale.
