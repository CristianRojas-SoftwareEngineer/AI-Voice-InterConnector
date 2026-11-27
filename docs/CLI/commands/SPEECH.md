# `speech`

Síntesis, transcripción, doblaje voz→voz y limpieza local de archivos de audio y vídeo, más el CRUD de los audios sintetizados persistidos. Es el comando más grande de la CLI: reúne varios subcomandos y los motores de voz (`avi-tts`, `avi-stt`, `avi-translation`, `denoise`); sus subcomandos de síntesis, transcripción y doblaje admiten el
[despacho al daemon](../CONTRACT.md#5-el-despacho-al-daemon).

Implementación: enum `SpeechCommands`, `handle_speech` para las demás acciones y `handle_denoise` desde `main` para el veredicto de archivos. Helpers de despacho al daemon: `route_to_daemon`, `transcribe_via_daemon`,
`daemon_synthesize_streaming`, `synthesize_via_daemon`, `say_via_daemon`, `dub_via_daemon`.

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
| `speech denoise` | No | Sí; archivos locales de audio/vídeo |

`denoise` solo está disponible con `native-denoise`, incluido en `full`. Usa DPDFNet provisionado por `setup` y FFmpeg incluido en el paquete; no llama al daemon ni procesa la imagen del vídeo. `--json` global genera JSONL de progreso y resultados. No se aceptan `--model-dir` ni `--ffmpeg-path`: el modelo y FFmpeg tienen resolución única de AVI. Los flags opt-in `--denoise` de transcripción, doblaje y clonado son rutas separadas de este pipeline de archivos; la evaluación humana de preservación de identidad vocal del clonado sigue pendiente.

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

En `synthesize`, `say` y `dub`, el motor tiene un único perfil de precisión, siempre activo: no hay variable ni opción para elegirlo.

**Invariante de captura de audio:** en `transcribe`/`dub`, la captura o
lectura del WAV ocurre siempre en el cliente (`AudioService::capture_mono_pcm(16_000, ...)`
o `avi_audio::load_wav_16k_mono_pcm`); el daemon nunca recibe una ruta de
archivo, solo PCM `i16` little-endian 16 kHz mono codificado en base64 en
`audio_b64`.

---

## `speech list`

```
ai-voice-interconnector speech list [--voice <nombre>]
```

Lista los audios sintetizados persistidos en `SpeechStore`
(`crates/avi-store`), local-only (brazo `List` de `SpeechCommands`).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--voice`, `-v` | string | — (todas las voces) | Opcional, sin default. Con valor, acota la lectura al directorio de esa voz (`SpeechStore::list_by_voice`, filtro normalizado a minúsculas); sin el flag, lista todas (`SpeechStore::list`) |

Definición: `SpeechCommands::List { voice: Option<String> }`. Con `--voice`, el handler construye el nombre de voz
validado (`VoiceName::parse`: exit 2 `invalid_voice_name`, también con `.` o `..`) y
comprueba la existencia de la voz con `VoiceStore::exists` (exit 3 `voice_not_found`)
antes de leer; sin `--voice` no valida nada y devuelve todo.

Salida humana (stdout): la cabecera `Audios sintetizados guardados:` y una
línea por audio, `  <voz>  <etiqueta>  <duración>  «<texto>»`; un audio sin
sidecar de metadatos se muestra como `  <voz>  <etiqueta>  (sin metadatos)`.
Sin ningún audio, una sola línea: `No hay audios sintetizados guardados.`.
Salida `--json`: `{"speech": [{"label", "voice", "text", "created_at",
"duration_secs"}, ...]}`; `text`, `created_at` y `duration_secs` son `null`
cuando el audio no tiene sidecar.

El WAV es el registro: `list` enumera los WAV, y el sidecar `.json` aporta los
metadatos opcionales. Un sidecar ilegible o corrupto hace fallar el comando con
exit 1 `speech_metadata_corrupt`; el mensaje nombra la etiqueta y la voz, y la
línea `Solución:` remite a `speech remove`.

---

## `speech denoise`

Limpia archivos locales de audio o la pista de audio de archivos de vídeo; la imagen del vídeo se conserva y no se procesa. Es una operación local-only: no envía rutas al daemon ni expone un endpoint genérico de limpieza.

```text
ai-voice-interconnector [--json] speech denoise <entrada>... [opciones]
```

Requiere el feature `native-denoise` (incluido en `full`), el modelo DPDFNet provisionado con `ai-voice-interconnector setup` y el FFmpeg verificado del paquete AVI. No busca FFmpeg en `PATH`. El binario integrado debe declarar versión 6 o superior en la primera línea de `ffmpeg -version`, con o sin el prefijo de `git describe` que anteponen los builds de Linux; una versión anterior, o una línea que no declare ninguna, se rechaza con un error que nombra lo que encontró, y `--version` añade la línea de FFmpeg cuando esta se reconoce e informa `ffmpeg missing` cuando no.

| Opción | Función |
|---|---|
| `<entrada>...` | Uno o más archivos o directorios de entrada |
| `--output-dir <directorio>` | Directorio de salida |
| `--output-name <nombre>` | Nombre explícito para una entrada |
| `--prefix <texto>` / `--suffix <texto>` | Ajusta nombres de salida |
| `--recursive` | Recorre directorios de forma recursiva |
| `--overwrite` / `--skip-existing` | Política de colisión de salidas |
| `--dry-run` | Muestra el plan sin procesar archivos |
| `--preset studio|default` | `studio` por omisión; `default` selecciona el perfil sin normalización |
| `--audio-bitrate <kbps>` | Bitrate de audio, sujeto a compatibilidad con el códec de salida |
| `--highpass` | Activa el filtro pasa-altos del DSP |
| `--normalize` / `--target-lufs <LUFS>` | Normaliza la egresión al objetivo de sonoridad |
| `--pause-gate-db <dB>` / `--dry-blend <0..1>` | Ajusta el gate de pausas o la mezcla de señal seca |
| `--stereo` | Conserva/procesa salida estéreo |
| `--verbose` | Aumenta el detalle de progreso |
| `--json` global | Emite registros JSONL por archivo y resumen terminal directamente, sin envelope adicional; la ejecución real publica el WAV/medio indicado por `output` |

El orden de resolución es defaults → preset → flags individuales. `studio` aplica solo a exportación de archivos; no altera audio de entrada de transcripción, doblaje o clonado. Para vídeo, se produce MP4 con el primer stream de vídeo copiado y audio limpio AAC; los formatos de audio siguen la matriz admitida por el pipeline migrado. No se amplía la matriz de contenedores ni códecs.

Con Ctrl+C el proceso termina con el código de interrupción 130; el pipeline elimina temporales y no publica una salida parcial. **Límite verificado:** la señal se atiende al cerrar, no interrumpe el DSP en curso: en las corridas medidas con el paquete instalado el archivo se completó y publicó antes de salir con 130. Los códigos de fallo conservan la paridad con el motor de origen: entrada inválida, destino ya existente o entrada sin audio devuelven 2; ORT no disponible, FFmpeg ausente o fallido y error de E/S devuelven 1; el modelo DPDFNet ausente devuelve 4 (`model_missing`); entrada ausente devuelve 3. La cancelación pedida por la biblioteca devuelve 3. En lotes, los resultados individuales conservan los estados `ok`/`skipped`/`failed`; el código final refleja si el lote completó o tuvo fallos.

Cada error se construye como un error común de la CLI, con un único mapeo de error a `reason`: entrada inválida o sin audio es `invalid_audio`, destino ya existente es `output_exists`, el modelo ausente es `model_missing`, el error de E/S es `io_error` y el resto `denoise_failed`. Los errores de validación previos al lote (un bitrate fuera de rango o incompatible con el códec, `--dry-blend`, `--pause-gate-db`, `--target-lufs`, un prefijo o sufijo inválido, entradas que no existen o ffmpeg ausente) salen como un único error `Error:` más `Solución:` en texto, o como un único envelope `{error, reason}` con `--json`, sin registros JSONL; los de uso son `usage_error` (2).

**Salida en texto.**

- stderr abre con el contexto `Limpieza de ruido · N archivos` y muestra una etapa por archivo con su avance: `… [i/n] <nombre>  <porcentaje> %  <etapa del pipeline>  <duración>`. Cada archivo terminado deja `✓ [i/n] <nombre>  <duración> · <tamaño de la salida>`; uno omitido, `– [i/n] <nombre>: ya existe la salida; se omite.`; y uno fallido, `✗ [i/n] <nombre>: <mensaje>`. Sin terminal, cada archivo escribe `… [i/n] <nombre>` una vez y un latido cada 10 s.
- stdout lleva la ruta completa de cada archivo generado, una por línea.
- El cierre es `✓ Limpieza de ruido completada en <total> · N archivos limpios`, más `, N omitidos` si los hay. Si algún archivo falló, el comando escribe `Error: No se pudo limpiar N de M archivos.` y sale con el peor código del lote; con `--json` no se escribe un segundo documento.
- `--dry-run` escribe por stdout `<entrada> → <salida> (<se procesaría | se sobrescribiría | se omitiría | fallaría: ya existe la salida>)` por archivo, y por stderr `Simulación: no se ha modificado nada.`.
- `--verbose` añade líneas de detalle atenuadas por stderr, y una salida duplicada se avisa con `Aviso:`. Los decimales llevan coma y no queda texto en inglés ni línea `Summary:`.

`--model-dir` y `--ffmpeg-path` no forman parte de esta interfaz y se rechazan como flags desconocidos: provisiona el modelo con `setup` y usa el FFmpeg integrado. El único comando público para limpiar archivos es `speech denoise`.

---

## `speech transcribe`

```
ai-voice-interconnector speech transcribe (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es|en> [--denoise] [--daemon|--no-daemon]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio` | string | — | Ruta del WAV a transcribir. Forma con `--mic` un grupo obligatorio y excluyente |
| `--mic` | flag | `false` | Captura desde el micrófono en vez de leer un archivo |
| `--duration` | u64 | — | Duración fija de grabación en segundos. Solo válido con `--mic` (el parser lo declara incompatible con `--audio`): con `--audio` sale con exit 2 `usage_error` |
| `--source-language` | `es`\|`en` | — | Obligatorio. Idioma hablado en el audio |
| `--denoise` | flag | `false` | Limpia el audio antes de transcribir; requiere `native-denoise` y el modelo DPDFNet provisionado |
| `--daemon` / `--no-daemon` | flag | — | Despacho tri-modal; mutuamente excluyentes |

El parser exige exactamente uno de `--audio` o `--mic` y rechaza
`--duration` sin `--mic`; en los tres casos (ninguno, ambos, `--duration` sin
`--mic`) sale con exit 2 `usage_error`.

Validaciones puras antes de despachar:
- `--mic` sin `--duration` **y sin TTY** (`stdin.is_terminal` es `false`,
 p. ej. en un pipe o en CI): exit 2 `usage_error` ("`--mic` necesita
 `--duration` cuando no hay una terminal.").
- `--mic` sin `--duration` **con TTY**: graba en modo **push-to-talk**
 (`crates/avi-audio`, primitiva de captura hasta Enter): al iniciar la
 grabación muestra por stderr `● Grabando m:ss / m:ss`, un medidor de nivel y
 `Enter para detener` (sin terminal, escribe una sola vez
 `Grabando (máximo m:ss)…`; nunca espera en silencio) y
 captura hasta que el usuario presiona Enter. Si el tramo más fuerte de la
 grabación no supera −43 dBFS, avisa que no se detectó voz y continúa. Hay un techo de seguridad
 configurable con la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS`
 (el default es el de [`push_to_talk_max_secs`](../../../crates/avi-audio/src/lib.rs) si no está definida): al alcanzarlo, detiene la grabación,
 avisa por stderr y devuelve lo grabado hasta ese punto con exit **0** (no es
 un error). Una variable definida que no sea un entero de segundos sin signo
 sale con exit 2 `usage_error` y un mensaje que la nombra. Si falla la lectura
 de stdin o el stream queda invalidado, la grabación termina con error; los
 fallos transitorios del motor no se registran en stderr y la captura continúa.
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
 `daemon_error`, y la línea `Solución:` indica reiniciar el daemon con `ai-voice-interconnector daemon restart`.
2. Rama directa: verifica que `parakeet-tdt-v3/nemo128.onnx` exista (si no,
 exit 4 `model_missing` antes de instanciar nada); si el binario se compiló
 sin el feature `native-stt`, exit 1 `stt_unsupported`; si el feature está
 activo, instancia `ParakeetEngine` (`crates/avi-stt/src/lib.rs`) y transcribe
 con `avi_core::engine::transcribe_speech`, la misma función que usa el daemon.

`ParakeetEngine` solo transcribe, nunca traduce: el idioma declarado selecciona el modelo y la respuesta, pero la transcripción es directa sin traducción intermedia.

Un audio en el que el motor no reconoce voz no es un éxito con texto vacío: en
las dos vías (directa y daemon) sale con exit 2 `empty_transcript`, con el
mensaje «No se reconoció voz en el audio.» y la línea `Solución:`. Con `--mic`,
una grabación en silencio conserva primero su aviso de captura
(`No se detectó voz en la grabación`) y después sale con ese mismo error. Si el
motor falla, sale con exit 10 `transcription_failed` y el mensaje «No se pudo
transcribir el audio.» (o «No se pudo cargar el modelo de transcripción.» si
falla la carga del modelo); la causa técnica de la biblioteca va solo en el
detalle de `--json`.

### Contrato `--json`

```json
{ "text": "<texto transcrito>", "source_language": "<valor de --source-language>" }
```

(Mismo envelope en la rama local y vía daemon.)

---

## `speech synthesize`

```
ai-voice-interconnector speech synthesize --text <texto> --label <etiqueta> [--voice <nombre>] [--force | --skip-existing] [--play] [--source-language <es|en>] [--target-language <es|en>] [--temperature <0<t<=2.0>] [--daemon|--no-daemon]
```

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--text`, `-t` | string | — | Obligatorio. Texto a sintetizar; no vacío y de hasta 1000 caracteres Unicode (si no, exit 2 `empty_text` / `text_too_long`, antes de cualquier despacho) |
| `--voice`, `-v` | string | `default` | Voz a usar |
| `--label`, `-l` | string | — | Obligatorio. Etiqueta bajo la que se persiste el audio sintetizado (se normaliza a minúsculas) |
| `--force`, `-f` | flag | `false` | Sobrescribe un audio sintetizado existente con la misma etiqueta |
| `--skip-existing` | flag | `false` | Conserva el audio sintetizado existente con la misma etiqueta sin sintetizar: sale con exit 0 y `status` `skipped`. Incompatible con `--force` y con `--play` (exit 2 `usage_error`) |
| `--play` | flag | `false` | Reproduce la toma y abre el menú interactivo; solo persiste al aceptar |
| `--source-language` | `es`\|`en` | = `target-language` | Idioma del texto de entrada |
| `--target-language` | `es`\|`en` | `es` | Idioma/modelo de síntesis; si difiere del origen, el texto se traduce antes de sintetizar |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo; rango `0 < t <= 2.0` |

**`--play` ofrece el bucle interactivo de 4 opciones** (reproducir de nuevo /
aceptar y guardar / rechazar y regenerar / rechazar y descartar), con menú y
prompts por stderr. Sin `--force`, la colisión de etiqueta se decide dos
veces: antes de sintetizar (fast-fail, exit 6 sin pagar la síntesis) y de nuevo al
guardar, por si la etiqueta quedó ocupada mientras se sintetizaba o el bucle
esperaba respuesta — si colisiona en ese instante, también exit 6. Rechazar y
descartar (opción 4) o Ctrl-D terminan con exit 0 sin persistir nada; solo
aceptar (opción 2) persiste la toma que sonó. La primera toma usa el seed de
producción; cada regeneración (opción 3) usa un seed nuevo, distinto de los
anteriores, y vuelve a mostrar la etapa de síntesis (en las dos rutas) sin dejar la línea de progreso
pegada al menú. `--play` es incompatible con
`--json` (exit 2 si se combinan) y exige terminal interactiva en la entrada
estándar (exit 2 sin TTY, antes de sintetizar).

**Política de colisión.** Los flags se convierten en un único punto en
`CollisionPolicy`: sin flag es `Fail` (exit 6 `label_exists`), `--force` es
`Overwrite` y `--skip-existing` es `Skip`. La decisión mira solo el WAV, que es el
registro: con la ficha `.json` corrupta y sin `--force` sale `label_exists`, no un
`io_error`. Con `--skip-existing` y la etiqueta ocupada, el comando termina con
exit 0 y `status` `skipped` sin sintetizar ni tocar el audio sintetizado: imprime
su ruta por stdout y cierra con `✓ Ya existía; se conserva sin sintetizar de nuevo.`; como la colisión se decide antes que los
recursos, responde así aunque el modelo falte o el daemon esté caído.

**Orden de validación.** `synthesize`, `say` y `dub` comparten la preparación
`prepare_synthesis`, con el orden «invocación → almacén → recursos» en las dos
rutas, de modo que con dos fallos a la vez gana el del paso anterior:

1. Invocación (exit 2): las reglas de `--play`, `validate_temperature` (fuera de
 `(0, 2.0]`), el texto (`empty_text` / `text_too_long`), la etiqueta
 (`Label::parse`: `invalid_identifier`) y la voz (`VoiceName::parse`:
 `invalid_voice_name`); los nombres rechazan también `.` y `..`.
2. Almacén: la voz debe existir en `VoiceStore` (exit 3 `voice_not_found`) y se
 aplica la política de colisión con `check_locution` (exit 6 `label_exists`, o
 exit 0 `skipped`).
3. Recursos, ya decidida la ruta: el modelo (exit 4) o el daemon (exit 5).

La línea de contexto se escribe entre el paso 2 y el 3: un error de invocación o
de almacén sale sin ella, y uno de recursos o de trabajo sale con ella. La única
excepción es la sonda que decide la ruta en modo automático, que va antes porque
la línea nombra la ruta y nunca falla.

Flujo local:
1. `source_eff = source_language.unwrap_or(target_language)` — sin
 `--source-language`, origen = destino (passthrough, no se traduce).
2. Despacho: si aplica, vía daemon; si no, rama directa:
 `require_model_provisioned` (exit 4 si falta `qwen3-tts-0.6b-base`) → `translate_if_different`
 (passthrough si `source == target`; si no, exige el modelo de traducción
 provisionado, exit 4 si falta) → entrega en streaming con `Qwen3TtsEngine`: sin
 `--play`, acumula el audio, escribe el WAV temporal y persiste; con `--play`, la
 toma suena mientras se sintetiza, se acumula en el WAV temporal y entra al bucle
 de 4 opciones, que persiste solo al aceptar y no repite la reproducción al
 entrar. La CLI imprime por stdout la ruta calculada del WAV persistido y cierra
por stderr con `✓ Síntesis completada en <total> · <duración> de audio`; con
`--play`, la opción de descartar cierra con `✓ Audio descartado; no se ha guardado.`.
Antes de la primera etapa escribe el contexto `Síntesis <idiomas> · voz <voz> ·
etiqueta <etiqueta> · local|daemon`.

Sin `--play` y sin traducción (origen y destino coinciden), la síntesis es la única
etapa de trabajo y no deja su línea `✓ Voz sintetizada`: queda solo el cierre. Con
`--play` o con traducción las etapas conservan sus líneas. Mientras la síntesis
espera, su línea viva cambia de texto sin reiniciar el reloj: `Esperando turno para
sintetizar…` (otra síntesis tiene el turno), `Calentando el motor…` (el daemon aún
calienta el motor) o `Cargando la voz…` (el motor arranca el residente de la voz,
por el daemon o en local, y tarda de 2 a 6 s), y vuelve a `Sintetizando…` cuando
el residente está listo. Nunca cierran la síntesis como hecha. Sin terminal solo se
escribe el anuncio inicial de la etapa y sus latidos de 10 s.

**Persistencia.** Todo audio sintetizado se guarda con `avi_store::persist_locution`
(`crates/avi-store/src/lib.rs`), la única forma de escribir en `SpeechStore`, en
las dos rutas, con y sin `--play`. La CLI resuelve la identidad de la voz
(`VoiceStore::resolve`: la ruta del `reference.qvoice` y su huella sha256) antes
de sintetizar, y `persist_locution` la comprueba justo
antes de escribir, antes que la colisión de etiqueta:

- Si la voz cambió entre la resolución y el guardado (se re-clonó o se eliminó
  mientras se sintetizaba), no se guarda nada y sale con exit 6
  `voice_changed`: la toma se grabó con un contenido que la voz ya no tiene.
- Si la etiqueta ya existe, la política decide: `Fail` sale con exit 6
  `label_exists`, `Skip` conserva la existente (`skipped`) y `Overwrite`
  sobrescribe.

El exit 6 es siempre un fallo: no se guardó nada.

El WAV y su ficha `.json` se publican cada uno con temporal + `rename`. Entre la
comprobación y la escritura queda una ventana de milisegundos que se acepta.

La rama vía daemon pasa por la misma preparación en el cliente —nombres, voz y
colisión, contra el almacén compartido— y resuelve la identidad de la voz antes de
despachar la síntesis al daemon (`POST /synthesize`, consume
el NDJSON: acumula el PCM de los eventos `audio`, que con `--play` suena según
llega, y lee `translated` del evento `result`; un `result` sin `translated` es
`synthesis_failed`, y un evento `audio` inválido, `daemon_error`), guarda el WAV
temporal, aplica el mismo bucle de `--play` (o persiste directo sin él; cada
regeneración envía un `seed` explícito) y expone la ruta del WAV persistido.
El sidecar guarda el texto realmente sintetizado (`translated`), igual que la
rama local — el mismo contrato de salida y de persistencia. Cuando los idiomas difieren, el daemon traduce con la misma segmentación (oraciones y párrafos, por lotes) que la ruta directa, así que la misma entrada produce el mismo texto por las dos rutas, y la CLI muestra la fase `translating` también por esta vía.

### Contrato `--json`

```json
{ "status": "success", "audio_path": "<ruta del WAV persistido>", "voice": "<voz>" }
{ "status": "skipped", "audio_path": "<ruta del audio sintetizado existente>", "voice": "<voz>" }
{ "status": "discarded", "voice": "<voz>" }
```

`skipped` lo emite `--skip-existing` sobre una etiqueta ocupada y `discarded` el
descarte del bucle `--play`. (Idéntico vía daemon y en la rama local.) El
envelope de `synthesize` no lleva `first_audio_ms` ni `stalls`: `--play`, el único
modo en que la toma suena, es incompatible con `--json`. Si la síntesis falla o
vence su plazo con la voz de `--play` ya sonando, se corta la reproducción, la
toma no se ofrece en el menú ni se guarda, y el comando sale con 1
`synthesis_failed` o `synthesis_timeout`.

---

## `speech say`

```
ai-voice-interconnector speech say --text <texto> [--voice <nombre>] [--source-language <es|en>] [--target-language <es|en>] [--temperature <0<t<=2.0>] [--daemon|--no-daemon]
```

Sintetiza y reproduce a la vez, en streaming, sin persistir en `SpeechStore` (no tiene `--label`): la voz empieza a sonar en cuanto hay audio suficiente, sin esperar a que termine la síntesis.
El texto debe ser no vacío y de hasta 1000 caracteres Unicode (exit 2
`empty_text` / `text_too_long`, validado antes de cualquier despacho). La
síntesis directa se acota al presupuesto `synthesis_budget` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)); al vencer, exit 1
`synthesis_timeout`. Mismas reglas de `--source-language`/`--target-language`/`--temperature` que
`synthesize`. No escribe ningún archivo: reproduce el audio según llega con
`AudioService::stream_player`, que espera 300 ms de audio en cola antes de sonar y,
si la cola se vacía con la entrega en curso, pausa y reanuda al recuperarlos; a
diferencia de `synthesize`, no hay flag `--play` porque la reproducción es
incondicional. El dispositivo de salida se abre antes de pedir la síntesis: sin
salida de audio el comando sale con 3 sin sintetizar. Si la síntesis falla o vence
su plazo con la voz ya sonando, se corta la reproducción y sale con 1
`synthesis_failed` o `synthesis_timeout`; un fallo del dispositivo durante la
reproducción es `playback_failed` y cancela la síntesis.

Valida con la misma preparación que `synthesize` («invocación → almacén →
recursos»): la temperatura y el texto, el nombre de voz (exit 2
`invalid_voice_name`) y la existencia de la voz (exit 3 `voice_not_found`), en las
dos rutas y antes del modelo o del daemon.

Despacho: `say_via_daemon` si aplica, o rama directa con
`require_model_provisioned` + `translate_if_different` +
`Qwen3TtsEngine::synthesize_streaming` + `AudioService::stream_player`. Con idiomas distintos, ambas rutas muestran la fase `translating` y traducen igual.

### Contrato `--json`

```json
{ "status": "reproduced", "voice": "<voz>", "first_audio_ms": 1180, "stalls": 0 }
```

(Mismo envelope en la rama local y vía daemon.) `first_audio_ms` son los
milisegundos desde que se abre la etapa de síntesis hasta la primera muestra
que suena, e incluyen la espera de turno, el calentamiento del motor y la carga
de la voz (`Esperando turno para sintetizar…`, `Calentando el motor…` y
`Cargando la voz…` son textos vivos de esa misma etapa, con un solo reloj). Es
una sola medida: el dato `· primer audio` de la etapa `Síntesis`, el extra
`primer audio` del cierre y `first_audio_ms` dan el mismo valor. `stalls` el número de veces que la cola de reproducción se vació con la entrega en
curso.

**No hay `audio_path`.** El audio no pasa por ningún archivo: suena según llega.
El texto humano cierra con `✓ Reproducción completada en <total> · primer audio <t>`
por stderr, tras la línea de contexto `Lectura en voz alta <idiomas> · voz <voz> · local|daemon`,
y sin ruta; stdout queda vacío. Quien
quiera el fichero en disco usa `speech synthesize`, que sí lo persiste y sí
devuelve su `audio_path`.
`say` es humo de punta a punta, sin puerta de calidad sobre lo que sonó; la
paridad de inteligibilidad la cubre `synthesize`.

---

## `speech dub`

```
ai-voice-interconnector speech dub (--audio <archivo.wav> | --mic) [--duration <segs>] --source-language <es|en> [--target-language <es|en>] [--voice <nombre>] [--temperature <0<t<=2.0>] [--denoise] [--daemon|--no-daemon]
```

Pipeline voz→voz: transcribe → traduce (si `source != target`) → sintetiza →
reproduce. Informa la transcripción y el texto sintetizado (ver «Contrato
`--json` (CLI)» más abajo).

| Flag | Tipo | Default | Descripción |
|---|---|---|---|
| `--audio`, `-a` | string | — | Archivo de audio a doblar. Forma con `--mic` un grupo obligatorio y excluyente |
| `--voice`, `-v` | string | `default` | Voz destino de la síntesis |
| `--source-language` | `es`\|`en` | — | Obligatorio. Idioma hablado en el audio de entrada |
| `--target-language` | `es`\|`en` | `es` | Idioma/modelo de síntesis; dispara traducción si difiere del origen |
| `--temperature` | f32 | producción (`0.35`) | Override de muestreo |
| `--mic` | flag | `false` | Captura desde micrófono. Mutuamente excluyente con `--audio` |
| `--duration` | u64 | — | Duración fija de grabación; solo válido con `--mic` (el parser lo declara incompatible con `--audio`) |
| `--denoise` | flag | `false` | Limpia el audio antes de doblar; en ruta local alimenta el PCM limpio a STT, y en daemon transmite WAV limpio. Requiere `native-denoise` y DPDFNet provisionado |
| `--daemon` / `--no-daemon` | flag | — | Despacho tri-modal; mutuamente excluyentes |

El parser exige exactamente uno de `--audio` o `--mic` y rechaza
`--duration` sin `--mic`; en los tres casos (ninguno, ambos, `--duration` sin
`--mic`) sale con exit 2 `usage_error`.

Validaciones puras (en este orden):
1. `validate_temperature`.
2. `--mic` sin `--duration` sin TTY: exit 2 `usage_error`. **Con TTY, mismo
 push-to-talk que en `transcribe`**: contador `● Grabando m:ss / m:ss` y
 medidor de nivel por stderr, captura hasta Enter o hasta el techo `AVI_PUSH_TO_TALK_MAX_SECS` (exit 0
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
5. Con la invocación validada, la misma preparación que `synthesize`: el nombre
 de voz (exit 2 `invalid_voice_name`) y su existencia, antes de abrir el
 micrófono o leer el audio y antes del modelo o del daemon, en las dos vías:
 una voz inexistente sale con exit 3 `voice_not_found` sin capturar nada (el
 daemon conserva su propia comprobación como defensa).
6. El texto traducido, que es el que se sintetiza, no supera 1000 caracteres
 Unicode: si no, exit 2 `text_too_long`.

La línea de contexto se escribe tras estas validaciones y antes de comprobar los
modelos, abrir el micrófono o contactar con el daemon: los errores anteriores
salen sin ella y los de recursos y de trabajo con ella.

Despacho: si aplica, `dub_via_daemon` hace `POST /dub`
consumiendo el stream NDJSON con un timeout de inactividad entre latidos
(`STREAM_INACTIVITY_TIMEOUT`) y un deadline total failsafe
(`STREAM_TOTAL_DEADLINE`, el techo `REQUEST_FAILSAFE` de [`limits.rs`](../../../crates/avi-core/src/limits.rs)).
Todo error de la ruta (404 incluido) se traduce por su `reason` con la tabla
única, sin degradar a otra vía: 404 `voice_not_found` sale con exit 3 y 404
`model_missing` con exit 4. Un 404 sin `reason` ni `error` sale con exit 1
`daemon_error` y el mensaje «El daemon no expone esta ruta (HTTP 404): es
anterior a este programa o no incluye la función.», con la línea `Solución:` de
reiniciar el daemon con `ai-voice-interconnector daemon restart`.

Rama directa (requiere feature `native-stt`; sin
ella, exit 1 `stt_unsupported`): verifica `parakeet-tdt-v3` provisionado (exit
4) y el modelo de síntesis provisionado (`require_model_provisioned`) →
captura/lee PCM → `transcribe_speech` (exit 2 `empty_transcript` si el texto
transcrito queda vacío) → si `source != target`, exige el modelo de
traducción provisionado (exit 4 `model_missing` si no); sin el feature
`native-translation`, exit 1 `translation_unsupported` →
`Qwen3TtsEngine::synthesize_streaming` →
`AudioService::stream_player`: el doblaje suena mientras se sintetiza.

### `POST /dub` (daemon)

Handler `dub_handler`. Acepta
`{audio_b64, voice, source_language, target_language, temperature?}`
(todos obligatorios salvo `temperature`, todos `es`/`en` en idiomas).
Tras validar barato en JSON
plano (audio, formatos, modelos e idiomas), abre una respuesta streaming NDJSON
(`application/x-ndjson`) emitiendo `{"event":"started", "voice":"..."}`, sin `operation` ni `message`: el cliente es el dueño de los textos y muestra su propia etapa.

Pipeline interno con latidos (`with_heartbeats`, periodo `STREAM_HEARTBEAT` entre `heartbeat`; ante desconexión,
`AbortHandle` descarta el resultado de la transcripción y la traducción, que terminan en segundo
plano; la cancelación del trabajo en el motor solo aplica a la síntesis).
La limpieza de ruido de `--denoise` la hace el cliente antes de despachar y
se muestra como la fase `denoising`, igual que en la ruta directa:
1. Transcribe en `spawn_blocking` con el motor STT residente del estado de inferencia (fase `"transcribe"`) y, al terminar, emite `{"event":"transcript","text":"<transcripción>"}`.
2. Si `source != target`, traduce en `spawn_blocking` con el motor CT2 residente del par; nunca carga modelos bajo demanda (latidos `"translate"`, con la misma segmentación de oraciones y párrafos, por lotes, que la ruta directa) y, al terminar, emite `{"event":"translation","text":"<texto traducido>"}`.
3. Sintetiza bajo `state.synthesis_lock` (reloj de trabajo propio arrancado tras adquirir el lock) con `GenerationOptions::with_temperature` sobre `spawn_blocking`. Al entrar en la fase emite de inmediato un `progress` con la etapa real: `synthesis` si consigue el lock al instante; si no, `warming` (el daemon aún calienta el motor) o `queued` (otra síntesis retiene el lock) con latidos de esa etapa, y `progress{synthesis}` al obtenerlo. Si el motor debe arrancar un residente (primer uso, cambio de voz o residente degradado), emite `progress{loading_voice}` y los latidos pasan a esa etapa hasta que el residente está sano, momento en que vuelve `progress{synthesis}`; los latidos usan siempre la etapa vigente. El plazo es el presupuesto `synthesis_budget` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) del texto traducido, más un margen interno; si vence, emite evento `error` con `synthesis_timeout` y cancela el trabajo en el motor TTS residente, que queda caliente para la siguiente petición. Si el cliente se desconecta durante la síntesis, el trabajo se cancela igual.

Antes del stream, un audio que supera `MAX_DUB_AUDIO_SECS` se rechaza con HTTP 400 `audio_too_long`; un texto traducido que supera `MAX_TEXT_LENGTH` ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) emite un evento `error` `text_too_long`.

Los eventos `transcript` y `translation` llevan exactamente las claves `event`, `text` y `schema_version`; `text` es contenido del usuario, no texto de interfaz. Salen antes del primer `audio`, de modo que el cliente muestra `Transcripción: …` y `Traducción: …` antes de que suene el doblaje. Son eventos nuevos sin cambio de versión del esquema del daemon; un cliente que no los conozca los ignora.

Durante la síntesis emite un evento `audio` por cada fragmento del motor, intercalado con los latidos:

```json
{ "event": "audio", "pcm_b64": "<PCM s16le, 24 kHz, mono, en base64>" }
```

Al finalizar, emite el evento `result`, que ya no lleva el audio:

```json
{ "event": "result", "status": "dubbed", "text": "<transcrito>", "translated": "<texto final tras traducir/passthrough>", "voice": "<voz>", "work_ms": 1234 }
```

Un fallo a mitad de la entrega emite `error` (`synthesis_failed` o `synthesis_timeout`) después de los eventos `audio` ya enviados.

En caso de error en cualquier etapa, emite `{"event":"error", "reason": "<motivo>", "message": "..."}` con el mapeo contractual correspondiente (`usage_error`/`invalid_audio`/`audio_too_long`/`empty_text`/`empty_transcript`/`text_too_long` → exit 2, `model_missing` → exit 4, `voice_not_found` → exit 3, `transcription_failed` → exit 10, `translation_failed` → exit 9, `synthesis_failed`/`synthesis_timeout`/`stt_unsupported`/`translation_unsupported` → exit 1).

### Contrato `--json` (CLI)

```json
{ "status": "dubbed", "text": "<transcripción>", "translated": "<texto sintetizado>", "first_audio_ms": 1180, "stalls": 0 }
```

(Mismo envelope en las dos rutas —local y vía `/dub`—.) Los nombres son los del
evento `result` del daemon: `text` es la transcripción del audio de entrada y
`translated` el texto que se sintetizó, que coincide con `text` cuando el
idioma de origen y el de destino son el mismo. `translated` está siempre
presente. `first_audio_ms` y `stalls` son los de la entrega en streaming, con el mismo
significado que en `say`. En la vía daemon, si al evento `result` le falta `text` o
`translated`, la CLI sale con exit 1 `daemon_error`, sin valores por defecto; un evento
`audio` inválido sale igual con `daemon_error`.

En modo texto (sin `--json`), `dub` escribe por stdout `Transcripción: <texto>`
en cuanto termina de transcribir y, solo si los idiomas difieren,
`Traducción: <texto>` en cuanto termina de traducir; ambas salen antes de que
suene el audio. Por stderr abre con el contexto `Doblaje <idiomas> · voz <voz> ·
<fuente> · local|daemon` y cierra con
`✓ Doblaje completado en <total> · primer audio <t>`.

**No hay `audio_path`, por la misma razón que en `say`.** El audio doblado no
pasa por ningún archivo, ni en el daemon ni en la CLI: suena según llega. Quien
quiera conservar el audio del doblaje usa `speech synthesize` con el texto
traducido. Como `say`, `dub` es
humo sin puerta de calidad sobre lo que sonó.

---

## `speech play`

```
ai-voice-interconnector speech play --label <etiqueta> [--voice <nombre>]
```

Local-only. Busca el audio sintetizado en `SpeechStore` por
`(voice, label)` y reproduce el WAV persistido. Valida los nombres al construir
`VoiceName` y `Label` (exit 2 `invalid_voice_name` para la voz e
`invalid_identifier` para la etiqueta, también con `.` o `..`) y la existencia de la voz con
`VoiceStore::exists` (exit 3 `voice_not_found`) antes de buscar la etiqueta; con
la voz válida, una etiqueta inexistente es exit 3 `speech_not_found`. La existencia la decide el WAV; un sidecar corrupto sale con
exit 1 `speech_metadata_corrupt` (el mensaje nombra la etiqueta y la voz, y la línea `Solución:` remite a `speech remove`).

Por stderr abre con el contexto `Reproducción del audio sintetizado <etiqueta> · voz <voz>`, avanza el contador de la etapa `Reproduciendo…` hasta el final y cierra con `✓ Reproducción completada en <total>`; stdout queda vacío. Es su única etapa, así que no deja la línea `✓ Reproducido`.

### Contrato `--json`

```json
{ "status": "played", "label": "<etiqueta>", "voice": "<voz>" }
```

---

## `speech remove`

```
ai-voice-interconnector speech remove --label <etiqueta> [--voice <nombre>] [--yes]
```

Local-only. Elimina el WAV y el sidecar del audio sintetizado de `SpeechStore` sin
interpretar el sidecar (también elimina un audio con sidecar corrupto). Valida
los nombres (exit 2 `invalid_voice_name` para la voz e `invalid_identifier` para la
etiqueta, también con `.` o `..`, sin tocar ningún archivo) y la existencia de la voz (exit 3
`voice_not_found`) antes de buscar la etiqueta; con la voz válida, si no existe
ninguno de los dos ficheros, exit 3 `speech_not_found`. Los audios sintetizados de una voz
se borran con la propia voz (`voice remove`), así que todo audio guardado
pertenece a una voz existente.

**Confirmación.** Borrar es destructivo: con terminal muestra por stderr
`Se borrará el audio sintetizado <etiqueta> de la voz <voz>:` con la ruta del WAV
y pregunta `¿Borrar este audio sintetizado? [s/N]`. `--yes`/`-y` omite la pregunta.
Sin terminal y sin `--yes` sale con exit 2 `confirmation_required`, y la línea
`Solución:` muestra la orden repetida con `--yes`. Si el usuario responde que no,
escribe `Cancelado; no se ha borrado nada.`, sale con exit 0 y, con `--json`,
emite `{ "status": "cancelled", "label", "voice" }`. Si el audio no existe, la
pregunta no se hace y el comando sale con `speech_not_found`. Al borrar, cierra con
`✓ Audio sintetizado <etiqueta> de la voz <voz> borrado.` por stderr.

### Contrato `--json`

```json
{ "status": "removed", "label": "<etiqueta>", "voice": "<voz>" }
{ "status": "cancelled", "label": "<etiqueta>", "voice": "<voz>" }
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

## El progreso por stderr

Cada subcomando de este grupo abre con una línea de contexto por stderr (`Doblaje es → en · voz default · archivo a.wav · daemon`) y pinta sus etapas por stderr. Una etapa permanente deja, al terminar, `✓ <texto>  <duración>`; la etapa en curso se repinta debajo como `… <texto> <duración>`, también durante el trabajo bloqueante de la ruta directa, la grabación y la reproducción. Los textos de las etapas son idénticos en la ruta directa y en la del daemon, y ninguna lleva un sufijo de ruta. Los avisos pasan por un canal de consola que borra la línea de etapa abierta antes de escribir (`Aviso: …`); el indicador la repinta en su siguiente repintado.

Las etapas de los subcomandos de voz son estas; las marcadas como transitorias se borran al terminar y no dejan línea:

| Etapa | Texto activo | Texto terminado |
|---|---|---|
| validación | `Validando…` | transitoria |
| envío al daemon | `Enviando al daemon…` | transitoria |
| espera de turno de la limpieza de ruido | `Esperando turno para limpiar el ruido…` | transitoria |
| lectura del archivo | `Leyendo el audio…` | `Audio leído` |
| captura | `Grabando…` | `Audio grabado` |
| limpieza de ruido | `Limpiando el ruido…` | `Ruido limpiado` |
| transcripción | `Transcribiendo…` | `Transcrito` |
| traducción | `Traduciendo…` | `Traducido` |
| síntesis | `Sintetizando…` | `Primer audio` si después se reproduce; si no, `Voz sintetizada` |
| guardado | `Guardando…` | transitoria |
| reproducción | `Reproduciendo…` | `Reproducido` |
| clonado | `Clonando la voz… (puede tardar varios minutos)` | `Voz clonada` |

Las esperas de la síntesis no son etapas propias, sino textos vivos de la etapa de síntesis, con su mismo reloj, que nunca la cierran como hecha: `Esperando turno para sintetizar…` (otra síntesis tiene el turno), `Calentando el motor…` (el daemon aún calienta el motor) y `Cargando la voz…` (el motor arranca el residente de la voz pedida). Cuando un comando solo tiene una etapa de trabajo (`speech play` y `speech synthesize` sin `--play` y sin traducción), esa etapa no deja su línea y queda solo el cierre; con `--json` la regla no actúa. El **resultado** —el texto, la ruta o el envelope `--json`— va por stdout en cuanto existe, y el cierre (`✓ <acción> en <duración total>`) va por stderr, una sola vez.

`AVI_NO_PROGRESS=1` silencia solo las etapas por stderr —línea activa, líneas terminadas y latidos—, y nada más: el contexto, los resultados por stdout, el cierre, el envelope `--json` y los códigos de salida no cambian. `NO_COLOR` desactiva el color; la salida redirigida nunca lleva color.

---

## Errores

| Reason | Exit | Origen |
|---|---|---|
| `usage_error` | 2 | `--skip-existing` con `--force` o con `--play`, falta `--audio`/`--mic`, `--audio` y `--mic` a la vez, `--duration` sin `--mic`, `--mic` sin `--duration` y sin TTY, `--temperature` fuera de rango, `--daemon` junto a `--no-daemon`, `--daemon`/`--no-daemon` en un subcomando que no los declara (`list`/`play`/`remove`) |
| `empty_text` | 2 | Texto a sintetizar vacío |
| `empty_transcript` | 2 | La transcripción (`transcribe`) o el doblaje (`dub`), directos o por el daemon, no reconocieron voz: la transcripción resultante está vacía |
| `text_too_long` | 2 | El texto a sintetizar (o el traducido en `dub`) supera `MAX_TEXT_LENGTH` caracteres Unicode ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) |
| `audio_too_long` | 2 | El audio de `dub` supera `MAX_DUB_AUDIO_SECS`, o `--duration` lo supera; el de `transcribe` supera `MAX_TRANSCRIBE_AUDIO_SECS`, o `--duration` o `AVI_PUSH_TO_TALK_MAX_SECS` lo superan ([`limits.rs`](../../../crates/avi-core/src/limits.rs)) |
| `invalid_audio` | 2 | El audio no es un WAV decodificable, o está truncado (`--audio` de transcribe y dub, referencia de `voice clone`) |
| `empty_audio` | 2 | La captura de micrófono (`--mic` de transcribe y dub) terminó sin ninguna muestra |
| `invalid_voice_name` | 2 | El nombre de voz está vacío, tiene caracteres fuera de `[A-Za-z0-9._-]` o es `.` o `..` (todos los subcomandos que toman `--voice`) |
| `invalid_identifier` | 2 | La etiqueta está vacía, tiene caracteres fuera de `[A-Za-z0-9._-]` o es `.` o `..` (`synthesize`, `play` y `remove`) |
| `audio_not_found` | 3 | `--audio` en `transcribe` o `dub` apunta a un archivo inexistente |
| `voice_not_found` | 3 | La voz indicada no existe en `VoiceStore` (todos los subcomandos que toman `--voice`) |
| `speech_not_found` | 3 | `play`/`remove` sobre una etiqueta inexistente de una voz que existe |
| `no_input_device` | 3 | `--mic` de transcribe y dub sin dispositivo de entrada predeterminado en el sistema |
| `no_output_device` | 3 | La reproducción (`say`, `play`, `dub` y `synthesize --play`) sin dispositivo de salida predeterminado en el sistema |
| `audio_backend_unavailable` | 3 | Captura o reproducción en un sistema sin backend de audio (en Linux, falta ALSA, `libasound.so.2`, o no define alguna función que se usa) |
| `model_missing` | 4 | Falta `parakeet-tdt-v3`, `qwen3-tts-0.6b-base` o el modelo de traducción del par (`opus-mt-<par>`) |
| `daemon_unreachable` | 5 | `--daemon` forzado sin daemon activo |
| `label_exists` | 6 | `synthesize` sin `--force` ni `--skip-existing` sobre una etiqueta ya usada |
| `voice_changed` | 6 | `synthesize`: la voz se re-clonó o se eliminó mientras se sintetizaba; el audio sintetizado no se guarda |
| `speech_metadata_corrupt` | 1 | `list`/`play`: el sidecar de metadatos de un audio sintetizado es ilegible; `remove` lo borra sin leerlo |
| `confirmation_required` | 2 | `remove` sin terminal y sin `--yes` |
| `output_exists` | 2 | `denoise`: la salida ya existe y no se pidió `--overwrite` ni `--skip-existing` (el daemon lo responde con HTTP 400) |
| `lifecycle_locked` | 17 | Todos los subcomandos: hay una operación de ciclo de vida en curso (`self install`, `self update`, `self uninstall`, `cleanup`, `doctor --repair`) y el comando no llega a ejecutarse |
| `stt_unsupported` | 1 | Binario compilado sin el feature `native-stt` (transcribe/dub) |
| `translation_unsupported` | 1 | Binario compilado sin el feature `native-translation`, con par no-passthrough |
| `transcription_failed` | 10 | Fallo del motor Parakeet o de la captura no atribuible al entorno (la falta de micrófono o de backend sale con 3 y la captura vacía con 2; la lectura de un `--audio` ausente, corrupto o con error de E/S sale con 3, 2 y 1) |
| `translation_failed` | 9 | Fallo del motor CT2 |
| `synthesis_failed` / `playback_failed` | 1 | Fallo del motor Qwen3-TTS (incluida una entrega cortada a mitad) o de una reproducción no atribuible al entorno (la falta de salida o de backend sale con 3). Un fallo con la voz ya sonando corta la reproducción |
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
ai-voice-interconnector speech synthesize --text "Hola mundo" --label saludo --skip-existing
ai-voice-interconnector speech transcribe --no-daemon --audio entrada.wav --source-language es
ai-voice-interconnector speech play --label saludo
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector --json speech list
ai-voice-interconnector --json speech list --voice mi_voz
```

---

## Muestras de salida

Los **resultados** van por stdout, en cuanto existen. La línea de contexto, las etapas y el cierre van por stderr.

```text
# speech synthesize (ruta local)
stdout: <ruta del WAV persistido>
stderr: Síntesis es · voz default · etiqueta saludo · local
        ✓ Síntesis completada en 2,1 s · 3,4 s de audio

# speech say
stderr: Lectura en voz alta es · voz default · local
        ✓ Primer audio  1,2 s
        ✓ Reproducido  3,0 s
        ✓ Reproducción completada en 4,3 s · primer audio 1,2 s

# speech transcribe
stdout: <el texto transcrito>
stderr: Transcripción es · archivo entrada.wav · local
        ✓ Audio leído  0,1 s
        ✓ Transcrito  1,4 s
        ✓ Transcripción completada en 1,6 s

# speech dub
stdout: Transcripción: <el texto transcrito>
        Traducción: <el texto traducido>
stderr: Doblaje en → es · voz default · archivo entrevista.wav · daemon
        ✓ Audio leído  0,1 s
        ✓ Transcrito  2,0 s
        ✓ Traducido  0,6 s
        ✓ Primer audio  1,1 s
        ✓ Reproducido  4,2 s
        ✓ Doblaje completado en 8,2 s · primer audio 1,1 s

# speech play
stderr: Reproducción del audio sintetizado saludo · voz default
        ✓ Reproducción completada en 2,0 s
```

Con terminal, la etapa en curso aparece debajo de las ya terminadas como `… Reproduciendo… 1,3 s` y se repinta; sin terminal escribe `… <texto>` una vez, un latido cada 10 s y la línea `✓` al terminar.

Con `--json`, stdout lleva **solo** el envelope —`{"status": "success"|"skipped", "audio_path", "voice"}`, `{"status": "reproduced", "voice"}`, `{"text", "source_language"}`, `{"status": "dubbed", "text", "translated"}`— y no salen ni el contexto, ni los resultados de texto, ni el cierre. Las etapas por stderr son las mismas.
