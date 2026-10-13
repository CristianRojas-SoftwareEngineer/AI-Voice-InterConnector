# `speech`

Síntesis, transcripción y doblaje voz→voz, más el CRUD de las locuciones
persistidas. Es el comando más grande de la CLI: reúne varios subcomandos y los
tres motores nativos (`avi-tts`, `avi-stt`, `avi-translation`); sus
subcomandos de síntesis, transcripción y doblaje admiten el
[despacho al daemon](../CONTRACT.md#5-el-despacho-al-daemon).

Implementación: `handle_speech`, enum `SpeechCommands`. Helpers de despacho al daemon: `route_to_daemon`, `transcribe_via_daemon`,
`daemon_synthesize_wav`, `synthesize_via_daemon`, `say_via_daemon`, `dub_via_daemon`.

---

## Superficie CLI

```
ai-voice-interconnector [--json] speech <subcomando> [flags]
```

`--daemon`/`--no-daemon` (mutuamente excluyentes) fijan el `DaemonMode`
(`ForceDaemon`/`ForceDirect`/`Auto` sin ninguno de los dos). Son flags de los
subcomandos delegables (`transcribe`, `synthesize`, `say` y `dub`), no de
`speech` ni globales: se escriben tras el subcomando
(`speech say --daemon …`).

| Subcomando | Delegable al daemon | Local-only |
|---|---|---|
| `speech list` | No | Sí |
| `speech transcribe` | Sí | — |
| `speech synthesize` | Sí | — |
| `speech say` | Sí | — |
| `speech dub` | Sí | — |
| `speech play` | No | Sí |
| `speech remove` | No | Sí |

`list`/`play`/`remove` son operaciones sobre `SpeechStore`, que el daemon no
expone por HTTP: no declaran `--daemon` ni `--no-daemon` y, si se les pasa
alguno, el parser lo rechaza como argumento desconocido (exit 2
`usage_error`).

---

## Despacho tri-modal (subcomandos delegables)

`route_to_daemon`:

| `DaemonMode` | Comportamiento |
|---|---|
| `ForceDaemon` (`--daemon`) | Siempre intenta el daemon; si el `POST` falla, exit 5 `daemon_unreachable` |
| `ForceDirect` (`--no-daemon`) | Nunca sondea el daemon; ejecuta el motor local |
| `Auto` (sin flags) | Sondea `GET /health` (`daemon_active`) con deadline corto; si responde, delega; si no, cae a directo |

**Invariante de captura de audio:** en `transcribe`/`dub`, la captura o
lectura del WAV ocurre siempre en el cliente (`AudioService::capture_16k_mono_pcm`
o `avi_audio::load_wav_16k_mono_pcm`); el daemon nunca recibe una ruta de
archivo, solo PCM `i16` little-endian 16 kHz mono codificado en base64 en
`audio_b64`.

---

## `speech list`

```
ai-voice-interconnector speech list [--voice <nombre>]
```

Lista las locuciones persistidas en `SpeechStore`
(`crates/avi-store`), local-only (brazo `List` de `SpeechCommands`).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--voice`, `-v` | string | — (todas las voces) | Opcional, sin default. Con valor, acota la lectura al directorio de esa voz (`SpeechStore::list_by_voice`, filtro normalizado a minúsculas); sin el flag, lista todas (`SpeechStore::list`) |

Definición: `SpeechCommands::List { voice: Option<String> }`. Con `--voice`, el handler valida el identificador
con `is_valid_identifier` (exit 2 `invalid_identifier`) y la existencia
de la voz con `VoiceStore::exists` (exit 3 `voice_not_found`) antes de leer;
sin `--voice` no valida nada y devuelve todo.

Salida humana: una línea por locución con voz, etiqueta, duración y texto; una
locución sin sidecar de metadatos se muestra como `(sin metadatos)`.
Salida `--json`: `{"speech": [{"label", "voice", "text", "created_at",
"duration_secs"}, ...]}`; `text`, `created_at` y `duration_secs` son `null`
cuando la locución no tiene sidecar.

El WAV es el registro: `list` enumera los WAV, y el sidecar `.json` aporta los
metadatos opcionales. Un sidecar ilegible o corrupto hace fallar el comando con
exit 1 `io_error`; el mensaje nombra el fichero y remite a `speech remove`.

---

## `speech transcribe`

```
ai-voice-interconnector speech transcribe (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es|en> [--daemon|--no-daemon]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio` | string | — | Ruta del WAV a transcribir. Forma con `--mic` un grupo obligatorio y excluyente |
| `--mic` | flag | `false` | Captura desde el micrófono en vez de leer un archivo |
| `--duration` | u64 | — | Duración fija de grabación en segundos. Solo válido con `--mic` (el parser lo declara incompatible con `--audio`): con `--audio` sale con exit 2 `usage_error` |
| `--source-language` | `es`\|`en` | — | Obligatorio. Idioma hablado en el audio |
| `--daemon` / `--no-daemon` | flag | — | Despacho tri-modal; mutuamente excluyentes |

El parser exige exactamente uno de `--audio` o `--mic` y rechaza
`--duration` sin `--mic`; en los tres casos (ninguno, ambos, `--duration` sin
`--mic`) sale con exit 2 `usage_error`.

Validaciones puras antes de despachar:
- `--mic` sin `--duration` **y sin TTY** (`stdin.is_terminal` es `false`,
 p. ej. en un pipe o en CI): exit 2 `usage_error` ("`--mic` requiere
 `--duration` en este host").
- `--mic` sin `--duration` **con TTY**: graba en modo **push-to-talk**
 (`crates/avi-audio`, primitiva de captura hasta Enter): al iniciar la
 grabación emite un aviso mínimo por stderr (nunca espera en silencio) y
 captura hasta que el usuario presiona Enter. Hay un techo de seguridad
 configurable con la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS`
 (el default es el de [`push_to_talk_max_secs`](../../../crates/avi-audio/src/lib.rs) si no está definida): al alcanzarlo, detiene la grabación,
 avisa por stderr y devuelve lo grabado hasta ese punto con exit **0** (no es
 un error). Una variable definida que no sea un entero de segundos sin signo
 sale con exit 2 `usage_error` y un mensaje que la nombra. Si falla la lectura
 de stdin o el stream de audio, la grabación termina con error. No hay
 panic por `duration` ausente: el modo push-to-talk cubre ese caso.
- Duración del audio: el audio de entrada dura como máximo `MAX_TRANSCRIBE_AUDIO_SECS`
 ([`limits.rs`](../../../crates/avi-core/src/limits.rs)), igual en la
 vía local y en la del daemon. Un WAV que lo supera sale con exit 2
 `audio_too_long`; la duración se mide por la cabecera del WAV y luego por el
 PCM decodificado, y solo se admiten WAV. Con `--mic`, un `--duration` mayor
 que ese máximo y un `AVI_PUSH_TO_TALK_MAX_SECS` mayor que él (push-to-talk) salen
 con el mismo `audio_too_long` antes de grabar (y, en el segundo caso, antes
 de exigir terminal). Esta comprobación precede a la de `model_missing`, a la
 de `stt_unsupported` y al contacto con el daemon.

Despacho:
1. `route_to_daemon` → si aplica, `transcribe_via_daemon`:
 codifica el PCM a `audio_b64`, hace `POST /transcribe` y emite el mismo
 envelope que la rama local. Un error HTTP del daemon se traduce por su
 `reason` con la tabla única; un 404 sin `reason` ni `error` sale con exit 1
 `daemon_error` y el mensaje de reiniciar el daemon con `avi daemon restart`.
2. Rama directa: verifica que `parakeet-tdt-v3/nemo128.onnx` exista (si no,
 exit 4 `model_missing` antes de instanciar nada); si el binario se compiló
 sin el feature `native-stt`, exit 1 `stt_unsupported`; si el feature está
 activo, instancia `ParakeetEngine` (`crates/avi-stt/src/lib.rs`) y llama
 `engine.transcribe(&pcm)`.

`ParakeetEngine` solo transcribe, nunca traduce: el idioma declarado selecciona el modelo y la respuesta, pero la transcripción es directa sin traducción intermedia.

### Contrato `--json`

```json
{ "text": "<texto transcrito>", "source_language": "<valor de --source-language>" }
```

(Mismo envelope en la rama local y vía daemon.)

---

## `speech synthesize`

```
ai-voice-interconnector speech synthesize --text <texto> --label <etiqueta> [--voice <nombre>] [--force] [--play] [--source-language <es|en>] [--target-language <es|en>] [--temperature <0<t<=2.0>] [--daemon|--no-daemon]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--text`, `-t` | string | — | Obligatorio. Texto a sintetizar; no vacío y de hasta 500 caracteres Unicode (si no, exit 2 `empty_text` / `text_too_long`, antes de cualquier despacho) |
| `--voice`, `-v` | string | `default` | Voz a usar |
| `--label`, `-l` | string | — | Obligatorio. Etiqueta bajo la que se persiste la locución (se normaliza a minúsculas) |
| `--force`, `-f` | flag | `false` | Sobrescribe una locución existente con la misma etiqueta |
| `--play` | flag | `false` | Reproduce el WAV tras sintetizar |
| `--source-language` | `es`\|`en` | = `target-language` | Idioma del texto de entrada |
| `--target-language` | `es`\|`en` | `es` | Idioma/modelo de síntesis; si difiere del origen, el texto se traduce antes de sintetizar |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo; rango `0 < t <= 2.0` |

**`--play` ofrece el bucle interactivo de 4 opciones** (reproducir de nuevo /
aceptar y guardar / rechazar y regenerar / rechazar y descartar), con menú y
prompts por stderr. Sin `--force`, la colisión de etiqueta se comprueba dos
veces: antes de sintetizar (fast-fail, exit 6 sin pagar la síntesis) y de nuevo al
aceptar (opción 2), por si la etiqueta quedó ocupada mientras el bucle
esperaba respuesta — si colisiona en ese instante, también exit 6. Rechazar y
descartar (opción 4) o Ctrl-D terminan con exit 0 sin persistir nada; solo
aceptar (opción 2) persiste la toma que sonó. `--play` es incompatible con
`--json` (exit 2 si se combinan) y exige terminal interactiva en la entrada
estándar (exit 2 sin TTY, antes de sintetizar).

Validaciones y flujo local:
1. `validate_temperature`: exit 2 si el override está fuera de `(0, 2.0]`.
2. Texto vacío tras `trim`: exit 2 `empty_text`.
3. `source_eff = source_language.unwrap_or(target_language)` — sin
 `--source-language`, origen = destino (passthrough, no se traduce).
4. Despacho: si aplica, vía daemon; si no, rama directa:
 `require_model_provisioned` (exit 4 si falta `qwen3-tts-0.6b`) → la voz
 debe existir en `VoiceStore` (exit 3 `voice_not_found`) →
 `is_valid_identifier` sobre la etiqueta normalizada (exit 2
 `invalid_identifier`, regex `^[A-Za-z0-9._-]+$`) → comprobación fast-fail
 de colisión de etiqueta (exit 6 sin `--force`) → `traducir_si_difiere`
 (passthrough si `source == target`; si no, exige el modelo de traducción
 provisionado, exit 4 si falta) → `Qwen3TtsEngine::synthesize_with_options` → sin
 `--play`, persiste directamente; con `--play`, entra al bucle de 4
 opciones y persiste solo al aceptar (con recomprobación de colisión).
 La CLI imprime la ruta calculada del WAV persistido.

La rama vía daemon repite las mismas validaciones de etiqueta/duplicado en el
cliente antes de despachar la síntesis al daemon (`POST /synthesize`, consume
el NDJSON y decodifica `audio_b64` del evento `result`), guarda el WAV
temporal, aplica el mismo bucle de `--play` (o persiste directo sin él) y
expone la ruta del WAV persistido — el mismo contrato de salida que la rama
local.

### Contrato `--json`

```json
{ "status": "success", "audio_path": "<ruta del WAV persistido>", "voice": "<voz>" }
```

(Idéntico vía daemon y en la rama local.)

---

## `speech say`

```
ai-voice-interconnector speech say --text <texto> [--voice <nombre>] [--source-language <es|en>] [--target-language <es|en>] [--temperature <0<t<=2.0>] [--daemon|--no-daemon]
```

Sintetiza y reproduce sin persistir en `SpeechStore` (no tiene `--label`).
El texto debe ser no vacío y de hasta 500 caracteres Unicode (exit 2
`empty_text` / `text_too_long`, validado antes de cualquier despacho). La
síntesis directa se acota al presupuesto `synthesis_budget` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)); al vencer, exit 1
`synthesis_timeout`. Mismas reglas de `--source-language`/`--target-language`/`--temperature` que
`synthesize`. Escribe el WAV en un archivo temporal
(`avi_say_<pid>.wav`) y siempre lo reproduce (`AudioService::play_wav`); a
diferencia de `synthesize`, no hay flag `--play` porque la reproducción es
incondicional.

Despacho: `say_via_daemon` si aplica, o rama directa con
`require_model_provisioned` + `VoiceStore::exists` + `traducir_si_difiere` +
`Qwen3TtsEngine::synthesize_with_options` + `AudioService::play_wav`.

### Contrato `--json`

```json
{ "status": "reproduced", "voice": "<voz>" }
```

(Mismo envelope en la rama local y vía daemon.)

**No hay `audio_path`.** La síntesis va a un WAV temporal con dueño
(`TempWav`, `crates/avi-shared/src/temp_wav.rs`), que lo borra al salir de
ámbito: al terminar la reproducción, pero también si la reproducción falla, que
era el camino por el que el fichero se quedaba. El texto humano dice
«Reproduciendo.» sin ruta. Quien quiera el fichero en disco usa
`speech synthesize`, que sí lo persiste y sí devuelve su `audio_path`.
`say` es humo de punta a punta, sin puerta de calidad sobre lo que sonó; la
paridad de inteligibilidad la cubre `synthesize`.

---

## `speech dub`

```
ai-voice-interconnector speech dub (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es|en> [--target-language <es|en>] [--voice <nombre>] [--temperature <0<t<=2.0>] [--daemon|--no-daemon]
```

Pipeline voz→voz: transcribe → traduce (si `source != target`) → sintetiza →
reproduce.

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio`, `-a` | string | — | Archivo de audio a doblar. Forma con `--mic` un grupo obligatorio y excluyente |
| `--voice`, `-v` | string | `default` | Voz destino de la síntesis |
| `--source-language` | `es`\|`en` | — | Obligatorio. Idioma hablado en el audio de entrada |
| `--target-language` | `es`\|`en` | `es` | Idioma/modelo de síntesis; dispara traducción si difiere del origen |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo |
| `--mic` | flag | `false` | Captura desde micrófono. Mutuamente excluyente con `--audio` |
| `--duration` | u64 | — | Duración fija de grabación; solo válido con `--mic` (el parser lo declara incompatible con `--audio`) |
| `--daemon` / `--no-daemon` | flag | — | Despacho tri-modal; mutuamente excluyentes |

El parser exige exactamente uno de `--audio` o `--mic` y rechaza
`--duration` sin `--mic`; en los tres casos (ninguno, ambos, `--duration` sin
`--mic`) sale con exit 2 `usage_error`.

Validaciones puras (en este orden):
1. `validate_temperature`.
2. `--mic` sin `--duration` sin TTY: exit 2 `usage_error`. **Con TTY, mismo
 push-to-talk que en `transcribe`**: aviso mínimo por stderr al iniciar,
 captura hasta Enter o hasta el techo `AVI_PUSH_TO_TALK_MAX_SECS` (exit 0
 al vencer). No hay panic por `duration` ausente en este caso.
3. Si `--audio` apunta a un archivo inexistente: exit 3 `audio_not_found`.
4. Duración del audio: el audio de entrada dura como máximo `MAX_DUB_AUDIO_SECS`
 ([`limits.rs`](../../../crates/avi-core/src/limits.rs)). Un WAV que
 lo supera, o un `--duration` mayor, sale con exit 2 `audio_too_long`;
 el push-to-talk se corta a ese máximo (además del techo
 `AVI_PUSH_TO_TALK_MAX_SECS`). La duración del `--audio` se mide por la
 cabecera del WAV antes de contactar con el daemon, de comprobar los modelos y
 de `stt_unsupported`, igual en las dos vías. Un WAV corrupto sale con exit 2
 `invalid_audio` y un fallo de E/S al leerlo con exit 1 `io_error`.
5. El texto traducido, que es el que se sintetiza, no supera 500 caracteres
 Unicode: si no, exit 2 `text_too_long`.

Despacho: si aplica, `dub_via_daemon` hace `POST /dub`
consumiendo el stream NDJSON con un timeout de inactividad entre latidos
(`STREAM_INACTIVITY_TIMEOUT`) y un deadline total failsafe
(`STREAM_TOTAL_DEADLINE`, el techo `REQUEST_FAILSAFE` de [`limits.rs`](../../../crates/avi-core/src/limits.rs)).
Todo error de la ruta (404 incluido) se traduce por su `reason` con la tabla
única, sin degradar a otra vía: 404 `voice_not_found` sale con exit 3 y 404
`model_missing` con exit 4. Un 404 sin `reason` ni `error` sale con exit 1
`daemon_error` y el mensaje «El daemon no expone esta ruta (HTTP 404):
reinícialo con el binario actual ejecutando `avi daemon restart`, o usa un
build que incluya la función.»

Rama directa (requiere feature `native-stt`; sin
ella, exit 1 `stt_unsupported`): verifica `parakeet-tdt-v3` provisionado (exit
4) y el modelo de síntesis provisionado (`require_model_provisioned`) →
captura/lee PCM → `ParakeetEngine::transcribe` → si el texto transcrito está
vacío, exit 2 `empty_text` → si `source != target`, exige el modelo de
traducción provisionado (exit 4 `model_missing` si no); sin el feature
`native-translation`, exit 1 `translation_unsupported` → la voz debe existir
(exit 3 `voice_not_found`) → `Qwen3TtsEngine::synthesize_with_options` →
`AudioService::play_wav`.

### `POST /dub` (daemon)

Handler `dub_handler`. Acepta
`{audio_b64, voice, source_language, target_language, temperature?}`
(todos obligatorios salvo `temperature`, todos `es`/`en` en idiomas).
Tras validar barato en JSON
plano (audio, formatos, modelos e idiomas), abre una respuesta streaming NDJSON
(`application/x-ndjson`) emitiendo `{"event":"started", "voice":"..."}`.

Pipeline interno con latidos (`with_heartbeats`, periodo `STREAM_HEARTBEAT` entre `heartbeat`; ante desconexión,
`AbortHandle` descarta el resultado de la transcripción y la traducción, que terminan en segundo
plano; la cancelación del trabajo en el motor solo aplica a la síntesis):
1. Transcribe en `spawn_blocking` con `state.stt_engine` (fase `"transcribe"`).
2. Si `source != target`, traduce en `spawn_blocking` con el CT2 residente (`state.ct2_engine`) o `avi_translation::translate` (fase `"translate"`).
3. Sintetiza bajo `state.synthesis_lock` (reloj de trabajo propio arrancado tras adquirir el lock) con `GenerationOptions::with_temperature` sobre `spawn_blocking` (fase `"synthesis"`). Mientras espera el lock emite latidos `warming` (el daemon aún calienta el motor) o `queued` (otra síntesis retiene el lock). El plazo es el presupuesto `synthesis_budget` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) del texto traducido, más un margen interno; si vence, emite evento `error` con `synthesis_timeout` y cancela el trabajo en el motor TTS residente, que queda caliente para la siguiente petición. Si el cliente se desconecta durante la síntesis, el trabajo se cancela igual.

Antes del stream, un audio que supera `MAX_DUB_AUDIO_SECS` se rechaza con HTTP 400 `audio_too_long`; un texto traducido que supera `MAX_TEXT_LENGTH` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) emite un evento `error` `text_too_long`.

Al finalizar, emite el evento `result`:

```json
{ "event": "result", "status": "dubbed", "text": "<transcrito>", "translated": "<texto final tras traducir/passthrough>", "audio_b64": "<WAV base64>", "voice": "<voz>", "work_ms": 1234 }
```

En caso de error en cualquier etapa, emite `{"event":"error", "reason": "<motivo>", "message": "..."}` con el mapeo contractual correspondiente (`usage_error`/`invalid_audio`/`audio_too_long`/`empty_text`/`text_too_long` → exit 2, `model_missing` → exit 4, `voice_not_found` → exit 3, `transcription_failed` → exit 10, `translation_failed` → exit 9, `synthesis_failed`/`synthesis_timeout`/`stt_unsupported`/`translation_unsupported` → exit 1).

### Contrato `--json` (CLI)

```json
{ "status": "dubbed", "text": "<texto final, traducido o passthrough>" }
```

(Mismo envelope en las dos rutas —local y vía `/dub`—;
nótese que el campo del CLI se llama `text`, aunque el handler del daemon
distingue internamente `text`/`translated`.)

**No hay `audio_path`, por la misma razón que en `say`.** El WAV temporal que
el daemon sintetiza para el doblaje lo borra su dueño al salir de ámbito, y en
el CLI el que reproduce es otro temporal con el mismo mecanismo. En el lado del
daemon el borrado cubre también el caso de que falle la lectura del WAV
sintetizado: el temporal se iba antes. Quien quiera conservar el audio del
doblaje usa `speech synthesize` con el texto traducido. Como `say`, `dub` es
humo sin puerta de calidad sobre lo que sonó.

---

## `speech play`

```
ai-voice-interconnector speech play --label <etiqueta> [--voice <nombre>]
```

Local-only. Busca la locución en `SpeechStore` por
`(voice, label)` y reproduce el WAV persistido. Valida los identificadores con
`is_valid_identifier` (exit 2 `invalid_identifier`) y la existencia de la voz con
`VoiceStore::exists` (exit 3 `voice_not_found`) antes de buscar la etiqueta; con
la voz válida, una etiqueta inexistente es exit 3 `speech_not_found`. La existencia la decide el WAV; un sidecar corrupto sale con
exit 1 `io_error` (el mensaje nombra el fichero y remite a `speech remove`).

### Contrato `--json`

```json
{ "status": "played", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## `speech remove`

```
ai-voice-interconnector speech remove --label <etiqueta> [--voice <nombre>]
```

Local-only. Elimina el WAV y el sidecar de la locución de `SpeechStore` sin
interpretar el sidecar (también elimina una locución con sidecar corrupto). Valida
los identificadores (exit 2 `invalid_identifier`) y la existencia de la voz (exit 3
`voice_not_found`) antes de buscar la etiqueta; con la voz válida, si no existe
ninguno de los dos ficheros, exit 3 `speech_not_found`. Las locuciones de una voz
se borran con la propia voz (`voice remove`), así que toda locución guardada
pertenece a una voz existente.

### Contrato `--json`

```json
{ "status": "removed", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## Idiomas soportados

Conjunto cerrado `{es, en}` en todos los flags de idioma (`value_parser`
de `clap`). Los únicos pares de
traducción provisionados por `setup` son `es-en` y `en-es`
(`crates/avi-shared/src/paths.rs`, `MODEL_REVISIONS`).

**Idioma de síntesis.** El idioma de destino (`--target-language`) es también el
idioma que recibe el motor de síntesis, tanto en la CLI como en el daemon: el
token del producto se traduce al nombre del motor (`es` → `Spanish`,
`en` → `English`) y las tres voces de fábrica hablan ambos. `POST /synthesize`
rechaza un `source_language`/`target_language` distinto de `es` y `en` con `usage_error`.

---

## Errores

| Reason | Exit | Origen |
|---|---|---|
| `usage_error` | 2 | Falta `--audio`/`--mic`, `--audio` y `--mic` a la vez, `--duration` sin `--mic`, `--mic` sin `--duration` y sin TTY, `--temperature` fuera de rango, `--daemon` junto a `--no-daemon`, `--daemon`/`--no-daemon` en un subcomando que no los declara (`list`/`play`/`remove`) |
| `empty_text` | 2 | Texto a sintetizar o transcripción resultante vacíos |
| `text_too_long` | 2 | El texto a sintetizar (o el traducido en `dub`) supera `MAX_TEXT_LENGTH` caracteres Unicode ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) |
| `audio_too_long` | 2 | El audio de `dub` supera `MAX_DUB_AUDIO_SECS`, o `--duration` lo supera; el de `transcribe` supera `MAX_TRANSCRIBE_AUDIO_SECS`, o `--duration` o `AVI_PUSH_TO_TALK_MAX_SECS` lo superan ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) |
| `invalid_audio` | 2 | El audio no es un WAV decodificable, o está truncado (`--audio` de transcribe y dub, referencia de `voice clone`) |
| `invalid_identifier` | 2 | Etiqueta o voz no cumple `^[A-Za-z0-9._-]+$` (incluye `list --voice`, `play` y `remove` con voz ilegal) |
| `audio_not_found` | 3 | `--audio` en `transcribe` o `dub` apunta a un archivo inexistente |
| `voice_not_found` | 3 | La voz indicada no existe en `VoiceStore` (incluye `list --voice`, `play` y `remove` sobre voz inexistente) |
| `speech_not_found` | 3 | `play`/`remove` sobre una etiqueta inexistente de una voz que existe |
| `model_missing` | 4 | Falta `parakeet-tdt-v3`, `qwen3-tts-0.6b` o el modelo de traducción del par (`opus-mt-<par>`); una voz clonada exige además `qwen3-tts-0.6b-base` |
| `daemon_unreachable` | 5 | `--daemon` forzado sin daemon activo |
| `label_exists` | 6 | `synthesize` sin `--force` sobre una etiqueta ya usada |
| `lifecycle_locked` | 17 | Todos los subcomandos: hay una operación de ciclo de vida en curso (`self install`, `self update`, `self uninstall`, `cleanup`, `doctor --repair`) y el comando no llega a ejecutarse |
| `stt_unsupported` | 1 | Binario compilado sin el feature `native-stt` (transcribe/dub) |
| `translation_unsupported` | 1 | Binario compilado sin el feature `native-translation`, con par no-passthrough |
| `transcription_failed` | 10 | Fallo de captura del micrófono o del motor Parakeet (la lectura de un `--audio` ausente, corrupto o con error de E/S sale con 3, 2 y 1) |
| `translation_failed` | 9 | Fallo del motor CT2 |
| `synthesis_failed` / `playback_failed` | 1 | Fallo del motor Qwen3-TTS o de reproducción |
| `synthesis_timeout` | 1 | La síntesis superó su presupuesto (`synthesis_budget`, [`limits.rs`](../../../crates/avi-core/src/limits.rs)) |
| `io_error` | 1 | Fallo de E/S al leer el WAV (`--audio` de transcribe y dub, referencia de `voice clone`) |

---

## Ejemplos

```bash
ai-voice-interconnector speech list
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech transcribe --audio entrada.wav --source-language es
ai-voice-interconnector speech transcribe --mic --duration 5 --source-language en
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo --voice default
ai-voice-interconnector speech synthesize --text "Hello" --label greeting --source-language en --target-language es --play
ai-voice-interconnector speech say --text "Probando" --voice default
ai-voice-interconnector speech dub --audio entrevista.wav --source-language en --target-language es --voice default
ai-voice-interconnector speech synthesize --daemon --text "Vía daemon" --label demo
ai-voice-interconnector speech transcribe --no-daemon --audio entrada.wav --source-language es
ai-voice-interconnector speech play --label saludo
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector --json speech list
ai-voice-interconnector --json speech list --voice mi_voz
```
