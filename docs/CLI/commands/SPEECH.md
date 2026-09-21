# `speech`

Síntesis, transcripción y doblaje voz→voz, más el CRUD de las locuciones
persistidas. Es el comando más grande de la CLI: reúne siete subcomandos y es
el único punto de la superficie donde conviven los tres motores nativos
(`avi-tts`, `avi-stt`, `avi-translation`) y el despacho tri-modal contra el
daemon.

Implementación: `handle_speech`, enum `SpeechCommands`. Helpers de despacho al daemon: `route_to_daemon`, `transcribe_via_daemon`,
`daemon_synthesize_wav`, `synthesize_via_daemon`, `say_via_daemon`, `dub_via_daemon`, `dub_compose_via_daemon` (fallback si
el daemon responde 404 en `/dub`, es decir, un binario viejo sin esa ruta).

---

## Superficie CLI

```
ai-voice-interconnector [--daemon|--no-daemon] [--json] speech <subcomando> [flags]
```

`--daemon`/`--no-daemon` son flags globales de `Cli` (mutuamente excluyentes) que fijan el `DaemonMode` (`ForceDaemon`/`ForceDirect`/
`Auto` sin ninguno de los dos). No son flags de `speech`; se anteponen al
comando raíz.

| Subcomando | Delegable al daemon | Local-only |
|---|---|---|
| `speech list` | No | Sí (`require_local`) |
| `speech transcribe` | Sí | — |
| `speech synthesize` | Sí | — |
| `speech say` | Sí | — |
| `speech dub` | Sí | — |
| `speech play` | No | Sí (`require_local`) |
| `speech remove` | No | Sí (`require_local`) |

`require_local` hace que `list`/`play`/`remove` con
`--daemon` forzado fallen con `daemon_unreachable` (exit 5) en vez de
ejecutarse localmente: son operaciones sobre `SpeechStore`, que el daemon no
expone por HTTP.

---

## Despacho tri-modal (subcomandos delegables)

`route_to_daemon`:

| `DaemonMode` | Comportamiento |
|---|---|
| `ForceDaemon` (`--daemon`) | Siempre intenta el daemon; si el `POST` falla, exit 5 `daemon_unreachable` |
| `ForceDirect` (`--no-daemon`) | Nunca sondea el daemon; ejecuta el motor local |
| `Auto` (sin flags) | Sondea `GET /health` (`daemon_activo`, ``) con deadline corto; si responde, delega; si no, cae a directo |

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
(`crates/avi-store`), local-only (brazo `List` en ``).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--voice`, `-v` | string | — (todas las voces) | Opcional, sin default. Con valor, acota la lectura al directorio de esa voz (`SpeechStore::list_by_voice`, ``, filtro normalizado a minúsculas); sin el flag, lista todas (`SpeechStore::list`, ``) |

Definición: `SpeechCommands::List { voice: Option<String> }`. Con `--voice`, el handler valida el identificador
con `es_identificador_valido` (exit 2 `invalid_identifier`,
``) y la existencia de la voz con `VoiceStore::exists`
(exit 3 `voice_not_found`, ``) antes de leer;
sin `--voice` no valida nada y devuelve todo.

Salida humana: una línea por locución con voz, etiqueta, duración y texto.
Salida `--json`: `{"speech": [{"label", "voice", "text", "created_at",
"duration_secs"}, ...]}`.

---

## `speech transcribe`

```
ai-voice-interconnector speech transcribe (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es-latam|en>
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio` | string | — | Ruta del WAV a transcribir. Mutuamente excluyente con `--mic` (`conflicts_with`, ``) |
| `--mic` | flag | `false` | Captura desde el micrófono en vez de leer un archivo |
| `--duration` | u64 | — | Duración fija de grabación en segundos. Solo tiene efecto con `--mic` |
| `--source-language` | `es-latam`\|`en` | — | Obligatorio. Idioma hablado en el audio |

Validaciones puras antes de despachar:
- Ni `--audio` ni `--mic`: exit 2 `usage_error`.
- `--mic` sin `--duration` **y sin TTY** (`stdin.is_terminal` es `false`,
 p. ej. en un pipe o en CI): exit 2 `usage_error` ("`--mic` requiere
 `--duration` en este host").
- `--mic` sin `--duration` **con TTY**: graba en modo **push-to-talk**
 (`crates/avi-audio`, primitiva de captura hasta Enter): al iniciar la
 grabación emite un aviso mínimo por stderr (nunca espera en silencio) y
 captura hasta que el usuario presiona Enter. Hay un techo de seguridad
 configurable con la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS`
 (default 300 s): al alcanzarlo, detiene la grabación, avisa por stderr y
 devuelve lo grabado hasta ese punto con exit **0** (no es un error). No hay
 panic por `duration` ausente: el modo push-to-talk cubre ese caso.

Despacho:
1. `route_to_daemon` → si aplica, `transcribe_via_daemon`:
 codifica el PCM a `audio_b64`, hace `POST /transcribe` y emite el mismo
 envelope que la rama local.
2. Rama directa: verifica que `parakeet-tdt-v3/nemo128.onnx` exista (si no,
 exit 4 `model_missing` antes de instanciar nada); si el binario se compiló
 sin el feature `native-stt`, exit 1 `stt_unsupported`; si el feature está
 activo, instancia `ParakeetEngine` (`crates/avi-stt/src/lib.rs`) y llama
 `engine.transcribe(&pcm, Some(language))`.

`resolve_stt_language` mapea `es-latam` → `es`; `en`
pasa verbatim. `ParakeetEngine` solo transcribe, nunca traduce.

### Contrato `--json`

```json
{ "text": "<texto transcrito>", "source": "<source_language tal cual se pasó>" }
```

(`` local, `` vía daemon — mismo envelope en
ambas rutas.) (`` local, `` vía daemon — mismo envelope en
ambas rutas.)

---

## `speech synthesize`

```
ai-voice-interconnector speech synthesize --text <texto> --label <etiqueta> [--voice <nombre>] [--output <ruta>] [--force] [--play] [--source-language <es-latam|en>] [--target-language <es-latam|en>] [--temperature <0<t<=2.0>]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--text`, `-t` | string | — | Obligatorio. Texto a sintetizar |
| `--voice`, `-v` | string | `default` | Voz a usar |
| `--output`, `-o` | string | — | Copia adicional del WAV persistido a esta ruta |
| `--label`, `-l` | string | — | Obligatorio. Etiqueta bajo la que se persiste la locución (se normaliza a minúsculas) |
| `--force`, `-f` | flag | `false` | Sobrescribe una locución existente con la misma etiqueta |
| `--play` | flag | `false` | Reproduce el WAV tras sintetizar |
| `--source-language` | `es-latam`\|`en` | = `target-language` | Idioma del texto de entrada |
| `--target-language` | `es-latam`\|`en` | `es-latam` | Idioma/modelo de síntesis; si difiere del origen, el texto se traduce antes de sintetizar |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo; rango `0 < t <= 2.0` |

**`--play` ofrece el bucle interactivo de 4 opciones** (reproducir de nuevo /
aceptar y guardar / rechazar y regenerar / rechazar y descartar), con menú y
prompts por stderr. Sin `--force`, la colisión de etiqueta se comprueba dos
veces: antes de sintetizar (fast-fail, exit 6 sin gastar GPU) y de nuevo al
aceptar (opción 2), por si la etiqueta quedó ocupada mientras el bucle
esperaba respuesta — si colisiona en ese instante, también exit 6. Rechazar y
descartar (opción 4) o Ctrl-D terminan con exit 0 sin persistir nada; solo
aceptar (opción 2) persiste la toma que sonó. `--play` es incompatible con
`--json` (exit 2 si se combinan) y exige terminal interactiva en la entrada
estándar (exit 2 sin TTY, antes de sintetizar).

Validaciones y flujo local:
1. `validar_temperature`: exit 2 si el override está fuera de `(0, 2.0]`.
2. Texto vacío tras `trim`: exit 2 `empty_text`.
3. `source_eff = source_language.unwrap_or(target_language)` — sin
 `--source-language`, origen = destino (passthrough, no se traduce).
4. Despacho: si aplica, vía daemon; si no, rama directa:
 `require_model_provisioned` (exit 4 si falta `qwen3-tts-0.6b`) → la voz
 debe existir en `VoiceStore` (exit 3 `voice_not_found`) →
 `es_identificador_valido` sobre la etiqueta normalizada (exit 2
 `invalid_identifier`, regex `^[A-Za-z0-9._-]+$`) → comprobación fast-fail
 de colisión de etiqueta (exit 6 sin `--force`) → `traducir_si_difiere`
 (passthrough si `source == target`; si no, exige el derivado CT2 sano,
 exit 4 si falta) → `Qwen3TtsEngine::synthesize_with_temperature` → sin
 `--play`, persiste directamente; con `--play`, entra al bucle de 4
 opciones y persiste solo al aceptar (con recomprobación de colisión) →
 si `--output`, copia el WAV persistido a esa ruta.

La rama vía daemon repite las mismas validaciones de etiqueta/duplicado en el
cliente antes de despachar la síntesis al daemon (`POST /synthesize`, consume
el NDJSON y decodifica `audio_b64` del evento `result`), guarda el WAV
temporal, aplica el mismo bucle de `--play` (o persiste directo sin él) y
copia a `--output` si corresponde — el mismo contrato de salida que la rama
local.

### Contrato `--json`

```json
{ "status": "success", "audio_path": "<ruta del WAV persistido>", "voice": "<voz>" }
```

(`` vía daemon, `` local — idéntico.) (`` vía daemon, `` local — idéntico.)

---

## `speech say`

```
ai-voice-interconnector speech say --text <texto> [--voice <nombre>] [--source-language <es-latam|en>] [--target-language <es-latam|en>] [--temperature <0<t<=2.0>]
```

Sintetiza y reproduce sin persistir en `SpeechStore` (no tiene `--label`).
Mismas reglas de `--source-language`/`--target-language`/`--temperature` que
`synthesize`. Escribe el WAV en un archivo temporal
(`avi_say_<pid>.wav`) y siempre lo reproduce (`AudioService::play_wav`); a
diferencia de `synthesize`, no hay flag `--play` porque la reproducción es
incondicional.

Despacho: `say_via_daemon` si aplica, o rama directa con
`require_model_provisioned` + `VoiceStore::exists` + `traducir_si_difiere` +
`Qwen3TtsEngine::synthesize_with_temperature` + `AudioService::play_wav`.

### Contrato `--json`

```json
{ "status": "reproduced", "audio_path": "<ruta temporal del WAV>", "voice": "<voz>" }
```

(`` local, `` vía daemon.) (`` local, `` vía daemon.)

---

## `speech dub`

```
ai-voice-interconnector speech dub (--audio <archivo.wav>|--file <archivo.wav> | --mic) [--duration <segs>] --source-language <es-latam|en> [--target-language <es-latam|en>] [--voice <nombre>] [--temperature <0<t<=2.0>]
```

Pipeline voz→voz: transcribe → traduce (si `source != target`) → sintetiza →
reproduce. `--file` es alias de `--audio` (`alias = "file"`, ``,
paridad con el oráculo Python retirado).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio`, `-a` (alias `--file`) | string | — | Archivo de audio a doblar. Mutuamente excluyente con `--mic` |
| `--voice`, `-v` | string | `default` | Voz destino de la síntesis |
| `--source-language` | `es-latam`\|`en` | — | Obligatorio. Idioma hablado en el audio de entrada |
| `--target-language` | `es-latam`\|`en` | `es-latam` | Idioma/modelo de síntesis; dispara traducción si difiere del origen |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo |
| `--mic` | flag | `false` | Captura desde micrófono. Mutuamente excluyente con `--audio` |
| `--duration` | u64 | — | Duración fija de grabación; solo válido con `--mic` |

Validaciones puras (en este orden):
1. `validar_temperature`.
2. `--duration` sin `--mic`: exit 2 `usage_error`.
3. `--mic` sin `--duration` sin TTY: exit 2 `usage_error`. **Con TTY, mismo
 push-to-talk que en `transcribe`**: aviso mínimo por stderr al iniciar,
 captura hasta Enter o hasta el techo `AVI_PUSH_TO_TALK_MAX_SECS` (default
 300 s, exit 0 al vencer). No hay panic por `duration` ausente en este caso.
4. Ni `--audio` ni `--mic`: exit 2 `usage_error`.
5. Si `--audio` apunta a un archivo inexistente: exit 3 `audio_not_found`.

Despacho: si aplica, `dub_via_daemon` hace `POST /dub`
esperando cabeceras de respuesta en ≤1500 ms y consumiendo el stream NDJSON
con un timeout de inactividad entre latidos de 1500 ms (`STREAM_INACTIVITY_TIMEOUT`)
y un deadline total failsafe de 120 s (`STREAM_TOTAL_DEADLINE`).
Si el daemon responde `404` (binario viejo sin esa ruta), degrada
automáticamente a `dub_compose_via_daemon`: transcribe
vía `POST /transcribe`, traduce localmente con `avi_translation::translate` si
`source != target`, y sintetiza vía `POST /synthesize` (`daemon_synthesize_wav`).

Rama directa (requiere feature `native-stt`; sin
ella, exit 1 `stt_unsupported`): verifica `parakeet-tdt-v3` provisionado (exit
4) y el modelo de síntesis provisionado (`require_model_provisioned`) →
captura/lee PCM → `ParakeetEngine::transcribe` → si el texto transcrito está
vacío, exit 2 `empty_text` → si `source != target`, exige el par
`{es-en, en-es}` (si no, exit 2 `unsupported_language_pair`) y el derivado CT2
sano (exit 4 `model_missing` con los ficheros faltantes si no); sin el feature
`native-translation`, exit 1 `translation_unsupported` → la voz debe existir
(exit 3 `voice_not_found`) → `Qwen3TtsEngine::synthesize_with_temperature` →
`AudioService::play_wav`.

### `POST /dub` (daemon)

Handler `dub_handler`. Acepta
`{audio_b64, voice?, from|source_language?, to|target_language?, temperature?}`
(`from`/`source_language` son alias del mismo campo, igual `to`/`target_language`;
default `voice="default"`, default idiomas `"es"`). Tras validar barato en JSON
plano (audio, formatos, modelos e idiomas), abre una respuesta streaming NDJSON
(`application/x-ndjson`) emitiendo `{"event":"started", "voice":"..."}`.

Pipeline interno con latidos (`con_latidos`, 500 ms entre `heartbeat` y `AbortHandle`
ante desconexión):
1. Transcribe en `spawn_blocking` con `state.stt_engine` (fase `"transcribe"`).
2. Si `source != target`, traduce en `spawn_blocking` con el CT2 residente (`state.ct2_engine`) o `avi_translation::translate` (fase `"translate"`).
3. Sintetiza bajo `state.synthesis_lock` (reloj de trabajo propio arrancado tras adquirir el lock) con `GenerationOptions::con_temperatura` y deadline `SYNTH_DEADLINE` (8 s) sobre `spawn_blocking` (fase `"synthesis"`). Si el deadline vence, emite evento `error` con `synthesis_timeout` sin derribar el residente.

Al finalizar, emite el evento `result`:

```json
{ "event": "result", "status": "dubbed", "text": "<transcrito>", "translated": "<texto final tras traducir/passthrough>", "audio_b64": "<WAV base64>", "voice": "<voz>", "work_ms": 1234 }
```

En caso de error en cualquier etapa, emite `{"event":"error", "reason": "<motivo>", "message": "..."}` con el mapeo contractual correspondiente (`audio_missing`/`audio_decode_error`/`empty_text`/`unsupported_language_pair` → exit 2, `model_missing` → exit 4, `voice_not_found` → exit 3, `transcription_failed` → exit 10, `translation_failed` → exit 9, `synthesis_failed`/`synthesis_timeout`/`stt_unsupported`/`translation_unsupported` → exit 1).

### Contrato `--json` (CLI)

```json
{ "status": "dubbed", "text": "<texto final, traducido o passthrough>", "audio_path": "<ruta temporal del WAV reproducido>" }
```

(`` local, `` vía `/dub`,
`` vía composición — mismo envelope en las tres rutas;
nótese que el campo del CLI se llama `text`, aunque el handler del daemon
distingue internamente `text`/`translated`.) (`` local, `` vía `/dub`,
`` vía composición — mismo envelope en las tres rutas;
nótese que el campo del CLI se llama `text`, aunque el handler del daemon
distingue internamente `text`/`translated`.) (`` local, `` vía `/dub`,
`` vía composición — mismo envelope en las tres rutas;
nótese que el campo del CLI se llama `text`, aunque el handler del daemon
distingue internamente `text`/`translated`.)

---

## `speech play`

```
ai-voice-interconnector speech play --label <etiqueta> [--voice <nombre>]
```

Local-only (`require_local`). Busca la locución en `SpeechStore` por
`(voice, label)` y reproduce el WAV persistido. Si no existe, exit 3
`speech_not_found`. Valida los identificadores con `es_identificador_valido`
antes de buscar.

### Contrato `--json`

```json
{ "status": "played", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## `speech remove`

```
ai-voice-interconnector speech remove --label <etiqueta> [--voice <nombre>]
```

Local-only. Elimina la locución de `SpeechStore`; si no existe, exit 3
`speech_not_found`.

### Contrato `--json`

```json
{ "status": "removed", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## Idiomas soportados

Conjunto cerrado `{es-latam, en}` en todos los flags de idioma (`value_parser`
de `clap`); internamente se normalizan a `{es, en}` vía `resolve_stt_language`
(CLI) / `resolve_translation_language`
(daemon) — ambas funciones mapean
`es-latam` → `es` y pasan cualquier otro valor verbatim. Los únicos pares de
traducción provisionados por `setup` son `es-en` y `en-es`
(`crates/avi-store/src/lib.rs`, `MODEL_REVISIONS`); cualquier otro par
resulta en exit 2 `unsupported_language_pair`.

**Divergencia deliberada con `translate`.** El conjunto `{es-latam, en}` es
la taxonomía del grupo `speech`; el comando `translate` usa un alfabeto
estricto `{es, en}` en `--from`/`--to` (`value_parser = ["es", "en"]`,
``) y rechaza `es-latam` por parser con exit 2. `es-latam`
solo sigue vivo fuera del CLI en la vía IPC del daemon, que lo normaliza a
`es`.

---

## Errores

| Reason | Exit | Origen |
|---|---|---|
| `usage_error` | 2 | Falta `--audio`/`--mic`, `--duration` sin `--mic`, `--mic` sin `--duration` y sin TTY, `--temperature` fuera de rango |
| `empty_text` | 2 | Texto a sintetizar o transcripción resultante vacíos |
| `invalid_identifier` | 2 | Etiqueta o voz no cumple `^[A-Za-z0-9._-]+$` (incluye `speech list --voice` ilegal) |
| `unsupported_language_pair` | 2 | Traducción fuera de `{es-en, en-es}` |
| `audio_not_found` | 3 | `--audio` en `dub` apunta a un archivo inexistente |
| `voice_not_found` | 3 | La voz indicada no existe en `VoiceStore` (incluye `speech list --voice` sobre voz inexistente) |
| `speech_not_found` | 3 | `play`/`remove` sobre una etiqueta inexistente |
| `model_missing` | 4 | Falta `parakeet-tdt-v3`, `qwen3-tts-0.6b` o el derivado CT2 del par de traducción |
| `daemon_unreachable` | 5 | `--daemon` forzado sin daemon activo, o local-only (`list`/`play`/`remove`) con `--daemon` |
| `label_exists` | 6 | `synthesize` sin `--force` sobre una etiqueta ya usada |
| `stt_unsupported` | 1 | Binario compilado sin el feature `native-stt` (transcribe/dub) |
| `translation_unsupported` | 1 | Binario compilado sin el feature `native-translation`, con par no-passthrough |
| `transcription_error` / `transcription_failed` | 10 | Fallo de captura/lectura de audio o del motor Parakeet |
| `translation_failed` | 9 | Fallo del motor CT2 |
| `synthesis_error` / `playback_failed` | 1 | Fallo del motor Qwen3-TTS o de reproducción |

---

## Ejemplos

```bash
ai-voice-interconnector speech list
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech transcribe --audio entrada.wav --source-language es-latam
ai-voice-interconnector speech transcribe --mic --duration 5 --source-language en
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo --voice default
ai-voice-interconnector speech synthesize --text "Hello" --label greeting --source-language en --target-language es-latam --play
ai-voice-interconnector speech say --text "Probando" --voice default
ai-voice-interconnector speech dub --audio entrevista.wav --source-language en --target-language es-latam --voice default
ai-voice-interconnector --daemon speech synthesize --text "Vía daemon" --label demo
ai-voice-interconnector --no-daemon speech transcribe --audio entrada.wav --source-language es-latam
ai-voice-interconnector speech play --label saludo
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector --json speech list
ai-voice-interconnector --json speech list --voice mi_voz
```
