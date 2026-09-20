# `speech`

Síntesis, transcripción y doblaje voz→voz, más el CRUD de las locuciones
persistidas. Es el comando más grande de la CLI: reúne siete subcomandos y es
el único punto de la superficie donde conviven los tres motores nativos
(`avi-tts`, `avi-stt`, `avi-translation`) y el despacho tri-modal contra el
daemon.

Implementación: `handle_speech` (`src/main.rs:846`), enum `SpeechCommands`
(`src/main.rs:217-315`). Helpers de despacho al daemon: `route_to_daemon`
(`src/main.rs:2875`), `transcribe_via_daemon` (`src/main.rs:2898`),
`daemon_synthesize_wav` (`src/main.rs:3056`), `synthesize_via_daemon`
(`src/main.rs:3152`), `say_via_daemon` (`src/main.rs:3202`), `dub_via_daemon`
(`src/main.rs:3336`), `dub_compose_via_daemon` (`src/main.rs:3490`, fallback si
el daemon responde 404 en `/dub`, es decir, un binario viejo sin esa ruta).

---

## Superficie CLI

```
ai-voice-interconnector [--daemon|--no-daemon] [--json] speech <subcomando> [flags]
```

`--daemon`/`--no-daemon` son flags globales de `Cli` (`src/main.rs:83-96`,
mutuamente excluyentes) que fijan el `DaemonMode` (`ForceDaemon`/`ForceDirect`/
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

`require_local` (`src/main.rs:2884`) hace que `list`/`play`/`remove` con
`--daemon` forzado fallen con `daemon_unreachable` (exit 5) en vez de
ejecutarse localmente: son operaciones sobre `SpeechStore`, que el daemon no
expone por HTTP.

---

## Despacho tri-modal (subcomandos delegables)

`route_to_daemon` (`src/main.rs:2875`):

| `DaemonMode` | Comportamiento |
|---|---|
| `ForceDaemon` (`--daemon`) | Siempre intenta el daemon; si el `POST` falla, exit 5 `daemon_unreachable` |
| `ForceDirect` (`--no-daemon`) | Nunca sondea el daemon; ejecuta el motor local |
| `Auto` (sin flags) | Sondea `GET /health` (`daemon_activo`, `src/main.rs:2868`) con deadline corto; si responde, delega; si no, cae a directo |

**Invariante de captura de audio:** en `transcribe`/`dub`, la captura o
lectura del WAV ocurre siempre en el cliente (`AudioService::capture_16k_mono_pcm`
o `avi_audio::load_wav_16k_mono_pcm`); el daemon nunca recibe una ruta de
archivo, solo PCM `i16` little-endian 16 kHz mono codificado en base64 en
`audio_b64`.

---

## `speech list`

```
ai-voice-interconnector speech list
```

Sin flags de filtrado. Lista todas las locuciones persistidas en
`SpeechStore` (`crates/avi-store`), local-only (`src/main.rs:854-891`).

**`speech list --voice/-v` NO existe.** El contrato histórico (`CONTRACT.md`)
prometía un filtro por voz; el `enum SpeechCommands::List` es una variante
unitaria sin campos (`src/main.rs:220`) y no acepta ningún flag. Es un drift
pendiente (H-10, `docs/reviews/2026-09-10-hallazgos-pendientes-consolidado.md`):
no lo documentes como funcional ni lo invoques con `--voice`, fallaría con un
error de parseo de `clap`.

Salida humana: una línea por locución con voz, etiqueta, duración y texto.
Salida `--json`: `{"speech": [{"label", "voice", "text", "created_at",
"duration_secs"}, ...]}` (`src/main.rs:861-873`).

---

## `speech transcribe`

```
ai-voice-interconnector speech transcribe (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es-latam|en>
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio` | string | — | Ruta del WAV a transcribir. Mutuamente excluyente con `--mic` (`conflicts_with`, `src/main.rs:224`) |
| `--mic` | flag | `false` | Captura desde el micrófono en vez de leer un archivo |
| `--duration` | u64 | — | Duración fija de grabación en segundos. Solo tiene efecto con `--mic` |
| `--source-language` | `es-latam`\|`en` | — | Obligatorio. Idioma hablado en el audio |

Validaciones puras antes de despachar (`src/main.rs:898-914`):
- Ni `--audio` ni `--mic`: exit 2 `usage_error`.
- `--mic` sin `--duration` **y sin TTY** (`stdin().is_terminal()` es `false`,
  p. ej. en un pipe o en CI): exit 2 `usage_error` ("`--mic` requiere
  `--duration` en este host").
- `--mic` sin `--duration` **con TTY**: la validación lo deja pasar (el
  comentario en el código la llama "push-to-talk permitido en TTY"), pero
  **no existe implementación de push-to-talk** — `AudioService::capture_16k_mono_pcm`
  (`crates/avi-audio/src/lib.rs:179`) solo acepta una duración fija en
  segundos, nunca "grabar hasta Enter". El camino llega a
  `duration.expect("validado arriba")` (`src/main.rs:923` en la rama daemon,
  y de nuevo en la rama directa) con `duration == None`, y **entra en panic**
  con stack trace, no con un `CliError` disciplinado. Este es el hallazgo
  H-08 (pendiente): documenta el comportamiento real — en TTY sin `--duration`
  el proceso revienta — y no prometas grabación interactiva funcional.

Despacho (`src/main.rs:916-1005`):
1. `route_to_daemon` → si aplica, `transcribe_via_daemon` (`src/main.rs:2898`):
   codifica el PCM a `audio_b64`, hace `POST /transcribe` y emite el mismo
   envelope que la rama local.
2. Rama directa: verifica que `parakeet-tdt-v3/nemo128.onnx` exista (si no,
   exit 4 `model_missing` antes de instanciar nada); si el binario se compiló
   sin el feature `native-stt`, exit 1 `stt_unsupported`; si el feature está
   activo, instancia `ParakeetEngine` (`crates/avi-stt/src/lib.rs`) y llama
   `engine.transcribe(&pcm, Some(language))`.

`resolve_stt_language` (`src/main.rs:59-64`) mapea `es-latam` → `es`; `en`
pasa verbatim. `ParakeetEngine` solo transcribe, nunca traduce.

### Contrato `--json`

```json
{ "text": "<texto transcrito>", "source": "<source_language tal cual se pasó>" }
```

(`src/main.rs:1000` local, `src/main.rs:2961` vía daemon — mismo envelope en
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

**`--play` NO tiene el bucle interactivo de 4 opciones que el contrato
histórico prometía** (reproducir / aceptar / regenerar / descartar). El
código real (`src/main.rs:1088-1099`) reproduce el WAV incondicionalmente
—si `--play` está presente— y **siempre** persiste la locución en
`SpeechStore` a continuación, sin preguntar nada. Es el hallazgo H-12
(pendiente): documenta `--play` como "reproduce y guarda", no como un flujo
interactivo.

Validaciones y flujo local (`src/main.rs:1007-1114`):
1. `validar_temperature` (`src/main.rs:67-78`): exit 2 si el override está
   fuera de `(0, 2.0]`.
2. Texto vacío tras `trim()`: exit 2 `empty_text`.
3. `source_eff = source_language.unwrap_or(target_language)` — sin
   `--source-language`, origen = destino (passthrough, no se traduce).
4. Despacho: si aplica, `synthesize_via_daemon` (`src/main.rs:3152`); si no,
   rama directa: `require_model_provisioned` (exit 4 si falta
   `qwen3-tts-0.6b`) → la voz debe existir en `VoiceStore` (exit 3
   `voice_not_found`) → `es_identificador_valido` sobre la etiqueta
   normalizada (exit 2 `invalid_identifier`, regex `^[A-Za-z0-9._-]+$`) →
   si ya existe una locución con esa etiqueta y no hay `--force`, exit 6
   `label_exists` → `traducir_si_difiere` (`src/main.rs:643-676`, passthrough
   si `source == target`; si no, exige el derivado CT2 sano, exit 4 si falta)
   → `Qwen3TtsEngine::synthesize_with_temperature` → si `--play`, reproduce
   → `SpeechStore::save` → si `--output`, copia el WAV persistido a esa ruta.

La rama vía daemon (`synthesize_via_daemon`, `src/main.rs:3152-3199`) repite
las mismas validaciones de etiqueta/duplicado en el cliente antes de llamar a
`daemon_synthesize_wav` (`POST /synthesize`, consume el NDJSON y decodifica
`audio_b64` del evento `result`), guarda el WAV temporal, reproduce si
`--play`, persiste en `SpeechStore` y copia a `--output` si corresponde — el
mismo contrato de salida que la rama local.

### Contrato `--json`

```json
{ "status": "success", "audio_path": "<ruta del WAV persistido>", "voice": "<voz>" }
```

(`src/main.rs:1046-1051` vía daemon, `src/main.rs:1105-1110` local — idéntico.)

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
incondicional (`src/main.rs:1116-1183`).

Despacho: `say_via_daemon` (`src/main.rs:3202`) si aplica, o rama directa con
`require_model_provisioned` + `VoiceStore::exists` + `traducir_si_difiere` +
`Qwen3TtsEngine::synthesize_with_temperature` + `AudioService::play_wav`.

### Contrato `--json`

```json
{ "status": "reproduced", "audio_path": "<ruta temporal del WAV>", "voice": "<voz>" }
```

(`src/main.rs:1174-1178` local, `src/main.rs:3224-3229` vía daemon.)

---

## `speech dub`

```
ai-voice-interconnector speech dub (--audio <archivo.wav>|--file <archivo.wav> | --mic) [--duration <segs>] --source-language <es-latam|en> [--target-language <es-latam|en>] [--voice <nombre>] [--temperature <0<t<=2.0>]
```

Pipeline voz→voz: transcribe → traduce (si `source != target`) → sintetiza →
reproduce. `--file` es alias de `--audio` (`alias = "file"`, `src/main.rs:281`,
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

Validaciones puras (`src/main.rs:1193-1224`, en este orden):
1. `validar_temperature`.
2. `--duration` sin `--mic`: exit 2 `usage_error`.
3. `--mic` sin `--duration` sin TTY: exit 2 `usage_error`. **Con TTY, mismo
   panic que en `transcribe`** (H-08): la validación lo permite pero
   `duration.expect("validado arriba")` revienta más adelante porque no hay
   push-to-talk implementado.
4. Ni `--audio` ni `--mic`: exit 2 `usage_error`.
5. Si `--audio` apunta a un archivo inexistente: exit 3 `audio_not_found`.

Despacho: si aplica, `dub_via_daemon` (`src/main.rs:3336`) hace `POST /dub`
con timeout de 10 s (`src/main.rs:3385`, "dub puede tardar por síntesis").
Si el daemon responde `404` (binario viejo sin esa ruta), degrada
automáticamente a `dub_compose_via_daemon` (`src/main.rs:3490`): transcribe
vía `POST /transcribe`, traduce localmente con `avi_translation::translate` si
`source != target`, y sintetiza vía `POST /synthesize` (`daemon_synthesize_wav`).

Rama directa (`src/main.rs:1244-1406`, requiere feature `native-stt`; sin
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

Handler `dub_handler` (`crates/avi-daemon/src/lib.rs:896`). Acepta
`{audio_b64, voice?, from|source_language?, to|target_language?, temperature?}`
(`from`/`source_language` son alias del mismo campo, igual `to`/`target_language`;
default `voice="default"`, default idiomas `"es"`). Pipeline interno:
transcribe con `state.stt_engine` → traduce con el CT2 residente
(`state.ct2_engine`) si está cargado, si no con `avi_translation::translate`
como fallback → sintetiza bajo `state.synthesis_lock` con
`GenerationOptions::con_temperatura` y un deadline `SYNTH_DEADLINE` sobre
`spawn_blocking` (mismo mecanismo que `/synthesize`, emite `synthesis_timeout`
sin matar el residente si vence). Respuesta de éxito:

```json
{ "status": "dubbed", "text": "<transcrito>", "translated": "<texto final tras traducir/passthrough>", "audio_b64": "<WAV base64>", "voice": "<voz>" }
```

En error, `{"status":"error", "reason": "<motivo>", "message": "..."}` con el
código HTTP correspondiente (`audio_missing`/`audio_decode_error` → 400,
`model_missing`/`voice_not_found` → 404, `transcription_failed`/
`translation_failed`/`synthesis_failed`/`synthesis_timeout`/`io_error` → 500,
`stt_unsupported`/`translation_unsupported` → 501 si el binario del daemon se
compiló sin esos features).

### Contrato `--json` (CLI)

```json
{ "status": "dubbed", "text": "<texto final, traducido o passthrough>", "audio_path": "<ruta temporal del WAV reproducido>" }
```

(`src/main.rs:1396-1401` local, `src/main.rs:3478-3483` vía `/dub`,
`src/main.rs:3611-3616` vía composición — mismo envelope en las tres rutas;
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
antes de buscar (`src/main.rs:1408-1441`).

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
`speech_not_found` (`src/main.rs:1442-1455`).

### Contrato `--json`

```json
{ "status": "removed", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## Idiomas soportados

Conjunto cerrado `{es-latam, en}` en todos los flags de idioma (`value_parser`
de `clap`); internamente se normalizan a `{es, en}` vía `resolve_stt_language`
(`src/main.rs:59-64`, CLI) / `resolve_translation_language`
(`crates/avi-daemon/src/lib.rs:626-631`, daemon) — ambas funciones mapean
`es-latam` → `es` y pasan cualquier otro valor verbatim. Los únicos pares de
traducción provisionados por `setup` son `es-en` y `en-es`
(`crates/avi-store/src/lib.rs`, `MODEL_REVISIONS`); cualquier otro par
resulta en exit 2 `unsupported_language_pair`.

---

## Errores

| Reason | Exit | Origen |
|---|---|---|
| `usage_error` | 2 | Falta `--audio`/`--mic`, `--duration` sin `--mic`, `--mic` sin `--duration` y sin TTY, `--temperature` fuera de rango |
| `empty_text` | 2 | Texto a sintetizar o transcripción resultante vacíos |
| `invalid_identifier` | 2 | Etiqueta o voz no cumple `^[A-Za-z0-9._-]+$` |
| `unsupported_language_pair` | 2 | Traducción fuera de `{es-en, en-es}` |
| `audio_not_found` | 3 | `--audio` en `dub` apunta a un archivo inexistente |
| `voice_not_found` | 3 | La voz indicada no existe en `VoiceStore` |
| `speech_not_found` | 3 | `play`/`remove` sobre una etiqueta inexistente |
| `model_missing` | 4 | Falta `parakeet-tdt-v3`, `qwen3-tts-0.6b` o el derivado CT2 del par de traducción |
| `daemon_unreachable` | 5 | `--daemon` forzado sin daemon activo, o local-only (`list`/`play`/`remove`) con `--daemon` |
| `label_exists` | 6 | `synthesize` sin `--force` sobre una etiqueta ya usada |
| `stt_unsupported` | 1 | Binario compilado sin el feature `native-stt` (transcribe/dub) |
| `translation_unsupported` | 1 | Binario compilado sin el feature `native-translation`, con par no-passthrough |
| `transcription_error` / `transcription_failed` | 10 | Fallo de captura/lectura de audio o del motor Parakeet |
| `translation_failed` | 9 | Fallo del motor CT2 |
| `synthesis_error` / `playback_failed` | 1 | Fallo del motor Qwen3-TTS o de reproducción |
| — (panic, sin `CliError`) | — | H-08: `--mic` sin `--duration` en TTY, en `transcribe` y `dub` |

---

## Hallazgos de drift pendientes (no implementar como funcional)

Referencia: `docs/reviews/2026-09-10-hallazgos-pendientes-consolidado.md`.

- **H-08** — `--mic` sin `--duration` en TTY hace panic (`duration.expect(...)`)
  en vez de fallar con un exit code disciplinado; no hay push-to-talk.
- **H-10** — `speech list` no acepta `--voice`/`-v`; el filtro por voz
  prometido en el contrato histórico no existe.
- **H-12** — `speech synthesize --play` reproduce y guarda incondicionalmente;
  no hay el bucle interactivo de 4 opciones (reproducir/aceptar/regenerar/
  descartar) que el contrato histórico describía.

---

## Ejemplos

```bash
ai-voice-interconnector speech list
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
```
