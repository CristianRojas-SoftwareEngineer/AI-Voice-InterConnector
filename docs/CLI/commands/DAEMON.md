# Comando `daemon` — ciclo de vida del daemon nativo

El estado `warm` no garantiza que siga cargada una voz distinta de la última solicitada: el residente se reemplaza al cambiar de voz.

El daemon es un servidor `Axum` (`crates/avi-daemon`) que mantiene los modelos Qwen3-TTS, Parakeet TDT v3 y opus-mt (CT2) `es↔en` en memoria, evitando la carga en cada invocación (~15–30 s). El CLI actúa como cliente HTTP sobre `127.0.0.1:8765` por defecto (`lifecycle::daemon_stop::default_addr()`, cuya reserva es `DEFAULT_ADDR` en `crates/avi-lifecycle/src/daemon_stop.rs`, la fuente única de la que también la toma el lanzador), con override por instancia vía `AVI_DAEMON_PORT` (`0` = puerto efímero, el servidor imprime la dirección realmente enlazada con `local_addr()`). El cliente CLI descubre la dirección efímera leyendo el campo `addr` del pidfile (`resolve_client_addr`); sin pidfile, o con `addr` en `null`, cae a la dirección derivada de `AVI_DAEMON_PORT` (`127.0.0.1:8765` si la variable falta o no es un puerto).

## Definición CLI

`enum DaemonCommands` (definición de subcomandos del daemon):

| Subcomando | Parámetros | Descripción |
|---|---|---|
| `daemon start` | `--json` `--auto-restart` `--max-retries` (default 3) `--warm-voice` (default `default`) | Clasifica el daemon con `lifecycle::daemon_stop::observe` (PID registrado con identidad + probe + residente), el mismo criterio que `status`: en servicio → `already_running`, degradado → lo reclama la parada unificada (si no concluye, `daemon_stop_failed`, exit 16) y rearranca con `started`; si no, lanza en background con `launch_daemon` (resultado explícito: listo, fallo tipado o muerte del hijo; ante fallo no deja proceso, pidfile ni fichero ready); con `--auto-restart` los reintentos parten de reclamo activo del árbol propio previo con deadline y verificación |
| `daemon stop` | `--json` | Parada unificada con el deadline global `STOP_DEADLINE_GLOBAL` (`lifecycle::daemon_stop::stop`: graceful + árbol preciso + verificación por puerto y PID registrado; encuentra al daemon por `daemon.pid` o, si falta, por `daemon.ready`; borra `daemon.pid` y `daemon.ready` solo tras muerte verificada del daemon y del residente, exit 5 sin borrar pista si sigue vivo alguno de los dos) |
| `daemon restart` | `--json` | Parada unificada (un único deadline global y un único reclamo por PID); si la parada no concluye falla con `daemon_stop_failed` (exit 16) y no arranca otro daemon; si concluye → `start` fresco con un presupuesto único para la parada y la publicación del arranque, más la confirmación por `/health` (sin flags de supervisión heredados; calienta `default`) |
| `daemon status` | `--json` | sonda del daemon y, si responde, `GET /health` del cuerpo + `observe` → `running` con `pid`, `engine` y `warm`, o `stopped` (con `degraded` si quedan restos que `start` reclamará); siempre exit 0; en texto escribe por stdout una línea en español, sin identificadores internos: `Daemon: en marcha (pid N, puerto P, motor listo).`, `Daemon: en marcha, pero no responde: <motivo>.` o `Daemon: detenido.`; el estado del motor es `motor listo`, `motor calentándose`, `falló el calentamiento del motor` o `estado del motor desconocido`, y `engine` y `warm` conservan sus valores solo en `--json` |
| `daemon serve` | `--auto-restart` `--max-retries` (default 3) `--warm-voice` (default `default`) `--ready-file <ruta>` (fichero donde publica el resultado del arranque; lo pasa `start` al hijo) | Ejecuta servidor en foreground (`run_supervised` con escucha de Ctrl+C, y en Unix también de SIGTERM, por la misma ruta que `POST /shutdown`); un fallo previo a estar listo sale con el mismo código que en `start` (ver [Códigos de error](#códigos-de-error)) |

`start`/`serve` aceptan `--auto-restart`/`--max-retries`/`--warm-voice`; `start`/`stop`/`restart`/`status` aceptan `--json` ( `serve` sin `--json`). `start`/`restart` exigen modelo provisionado (`require_model_provisioned`), `stop`/`status` no.

**`--warm-voice <nombre>` (default `default`):** selecciona qué voz precalienta el daemon al arranque en vez de forzar `default` (el residente TTS es de una sola voz). `start` lo propaga al `serve` que respawnea (`spawn_background` anexa `--warm-voice`); `restart` calienta siempre `default`. Validación *temprana*: el arranque valida la voz y enlaza el puerto **antes de cargar los modelos**. Un nombre ilegal (también `.` o `..`) sale con exit 2 `invalid_voice_name` antes de nada; si la voz no existe sale con exit 3 `voice_not_found` (no degrada en silencio ni cae a `default`); si el puerto está ocupado sale con exit 6 `port_in_use`; en ambos casos al instante, sin esperar el deadline de arranque.

**Qué voz tiene cargada el residente y cuándo se reutiliza.** El residente TTS carga una sola voz. Cada petición de síntesis (`/synthesize`, `/dub` y el precalentamiento) resuelve la voz en el almacén con `VoiceStore::resolve`, que devuelve la referencia clonada con la identidad de su contenido: la ruta del `reference.qvoice` y su huella sha256. El residente se reutiliza solo si la voz pedida es la misma voz con la misma identidad de contenido; con cualquier otra, incluida la misma voz re-clonada con el mismo nombre (misma ruta, otra huella), se termina el residente anterior y se arranca otro con `--load-voice`. Calcular la huella lee el `reference.qvoice` completo en cada resolución: 4112 bytes en una voz `.qvoice` v6. El residente no se desaloja al eliminar una voz: la siguiente síntesis de otra voz lo reemplaza. Como cambiar de voz reinicia el residente (de 2 a 6 s), el motor lo avisa a quien llama (`SynthesisStep::LoadingVoice` y, cuando el residente está sano, `SynthesisStep::Synthesizing`): el daemon emite `progress{loading_voice}` y después `progress{synthesis}`, y la CLI lo muestra como `Cargando la voz…` dentro de la etapa de síntesis.

## Despacho del handler

`handle_daemon(json_mode, action)` (despachador principal del daemon):

```
handle_daemon
 ├── Serve  → run_supervised(dirección resuelta por `resolve_daemon_addr`, default 127.0.0.1:8765) (foreground, warmup background, Ctrl+C —y SIGTERM en Unix— por la misma ruta que POST /shutdown)
 ├── Start  → observe (PID registrado + probe + residente) → Running: already_running | Degraded: lifecycle::daemon_stop::stop → require_stopped (daemon_stop_failed, exit 16) → started : launch_daemon → write pid
 ├── Stop   → lifecycle::daemon_stop::stop (deadline global) → StopOutcome{was_running, daemon_fully_stopped, resident_alive, pid, remaining} → shutdown_sent | not_running (exit 0) : still_running con el veredicto completo (exit 5 por veredicto, sin objeto error, sin borrar pista)
 ├── Restart→ lifecycle::daemon_stop::stop → require_stopped (daemon_stop_failed, exit 16, sin arrancar) → launch_daemon (publicación dentro del presupuesto) → write pid
 │           launch_daemon: [starting] spawn_background → espera del resultado (listo | fallo tipado | muerte del hijo) → /health → [warming] espera del motor (listo | aún calentándose | fallido)
 │                          ante fallo: kill del árbol si sigue vivo + borrado de daemon.ready
 └── Status → daemon_stop::probe → (si responde) GET /health del cuerpo (plazo propio) + observe → running (pid, engine, warm) | stopped (+ degraded)
```

`serve` no usa subproceso; `start` y `restart` lanzan el daemon con `launch_daemon`, que usa `avi_daemon::spawn::spawn_background`. El handler Ctrl+C está instalado en `main` para todos los modos: limpieza acotada (`CTRL_C_CLEANUP_DEADLINE`, `src/main.rs`) sobre el árbol del pidfile —o del PID que `launch_daemon` conserva en memoria desde el spawn, en la ventana anterior a que el daemon se registre en el pidfile, tanto en `start` como en `restart`— y salida 130 preservada.

## Arquitectura del daemon

```
CLI (ai-voice-interconnector)
  ├── handle_daemon ──► spawn_background ──► daemon (Axum)
  │                       │  stdin a null, stdout/stderr al log del arranque; herencia restringida a la lista explícita (Win) / setsid (Unix)
  │                       ▼
  │                  DaemonState ──► Qwen3TtsEngine (resident qwen_tts)
  │                       ├── warm: RwLock<WarmState> (Warming/Warm/Failed)
  │                       ├── synthesis_lock: Mutex<()>
  │                       ├── inference_engines: STT y dos CT2 residentes (según features)
  │                       └── shutdown_notify: Arc<Notify>
  └── DaemonIPCClient (reqwest) ◄──► Axum Router
```

`DaemonState { synthesis_lock, voice_store, speech_store, tts_engine, inference_engines, warm, shutdown_notify }` (campos principales del estado del daemon).
El constructor de producción exige los cuatro modelos obligatorios antes de cargar STT, los motores CT2 residentes de **ambos** pares y TTS. El quinto pin, `dpdfnet8`, es opcional en el arranque: sin él el daemon sirve igual, pero las rutas opt-in de `--denoise` requieren provisionarlo y fallan con `model_missing` (salida 4), igual que `speech denoise`. Si falta un modelo, el daemon no publica el estado listo y responde `model_missing` (salida 4) con la indicación de ejecutar `setup`; si un motor no carga, también falla el arranque. El arranque no admite estados parcialmente provisionados: síntesis, clonación, traducción y transcripción exigen sus modelos.
`run_daemon_server` arranca en este orden: valida la voz de warmup, enlaza el `TcpListener`, carga el estado (`DaemonState::new`, los modelos), se registra en `daemon.pid` (`DaemonRegistry::register_daemon`: su PID, su identidad observada y la dirección real; si no puede observar su identidad (en Linux la lee de `/proc/<pid>/stat` y en macOS de `proc_pidinfo`, sin herramientas externas) o el pidfile ya registra a otro daemon vivo con identidad coincidente, el arranque falla con `startup_failed` y no sirve), anuncia la dirección realmente enlazada (`local_addr()`, con `:0` el SO asigna) y publica el registro de éxito, y sirve. El registro precede a `daemon.ready` y al warmup: quien ve el daemon listo ya lo encuentra en el pidfile; en los reinicios en proceso de `run_supervised` el PID es el mismo y el registro se tolera a sí mismo. Así un fallo de configuración se detecta antes de pagar la carga, y el puerto queda reservado durante ella (una conexión en esa ventana espera en el backlog). Tras el warmup emite el evento `avi-daemon-ready warm=<warm|warm_failed> addr=<real>` en stderr como diagnóstico redundante; el transporte que consume el padre (o el harness de pruebas) con espera acotada es el fichero designado por `--ready-file`, escrito de forma atómica con uno de dos registros: el de éxito (`addr`/`warm`/`pid`) o, si el arranque falla antes de estar listo, el de fallo (`error=port_in_use|warm_voice_missing|model_missing|startup_failed` con `port`, `voice` o `message`, nunca `addr`, porque los lectores toman `addr` como listo). Solo `run_supervised` publica el registro de fallo, justo antes de terminar. Ya en marcha: `spawn_blocking(warm_voice_engine)`, `with_graceful_shutdown` por la misma ruta desde `POST /shutdown` y desde Ctrl+C, más SIGTERM en Unix (en Windows solo se escucha Ctrl+C) (`tts_engine.shutdown()` + `notify_one()`); al terminar `serve`, por cualquier salida, `tts_engine.close()` mata y espera al residente, de modo que ni terminar ni un reinicio en proceso de `run_supervised` dejan un residente vivo sin registrar. Cierre garantizado: vigía por tubería del motor (ver «Vida del motor residente»), kill preciso del árbol por PID con verificación (función de spawn del daemon) y reclamo matar-y-rearrancar en `start` (bloque de reclamo y rearranque del binario principal; muerte del residente por PID registrado e identidad).

## Endpoints

| Endpoint | Método | Request | Response | Descripción |
|---|---|---|---|---|
| `/health` | GET | — | `{status:"ready", warm, engine, warm_error?, ct2?, stt?}` + `schema_version` | Readiness + warmup (7 rutas) |
| `/synthesize` | POST | `{text, voice, source_language, target_language, temperature?, seed?}` | NDJSON `started → progress{queued\|warming\|synthesis} → heartbeat → [progress{synthesis}] → [progress{loading_voice} → progress{synthesis}] → audio{pcm_b64}… (intercalados con heartbeat) → result{translated, work_ms}` o `error`; si los idiomas difieren, traduce fuera del runtime bajo la etapa `translate` (anunciada con `progress` y seguida de latidos) y entra después en la fase de síntesis, que emite de inmediato `progress` con la etapa real (`synthesis`, `queued` o `warming`) y `loading_voice` si arranca un residente; antes del stream, 400 `invalid_voice_name` si `voice` está vacío, tiene caracteres fuera de `[A-Za-z0-9._-]` o es `.` o `..` | Entrega en streaming: el audio (PCM s16le, 24 kHz, mono) llega en eventos `audio` según el motor lo genera; un fallo a mitad emite `error` tras los `audio` ya enviados |
| `/transcribe` | POST | `{audio_b64, source_language, denoise?}` | NDJSON `started → heartbeat/progress → [denoise_queue → denoise] → transcribe → result{text, source_language}` o `error`; los errores baratos son JSON `{status:"error", reason, message}` con código HTTP antes de que empiece el stream: 400 (`usage_error` sin audio, `invalid_audio` si el base64 no es válido, `audio_too_long` si las muestras superan `MAX_TRANSCRIBE_AUDIO_SECS`) o 413 (cuerpo mayor que el tope derivado de ese límite); un fallo dentro del stream (`empty_transcript` si el motor no reconoce voz en el audio, `transcription_failed` con `message` «No se pudo transcribir el audio.», con la causa técnica solo en el registro del daemon) llega como evento `error` | Transcripción Parakeet (feature `native-stt`) con streaming NDJSON |
| `/translate` | POST | `{text, source_language, target_language}` | `{translated, source_language, target_language}` o `error` | Traducción CT2 residente (feature `native-translation`) |
| `/voices/clone` | POST | `{name, audio_b64, force?, denoise?}` (con `force`, reemplaza la voz y sus audios sintetizados; con `denoise`, limpia el ruido de la referencia antes de clonar) | NDJSON `started → heartbeat/progress → [progress{denoise}] → progress{save} → result{name, speech, precomputed:true, removed_speech}` o `error`; antes del stream, 400 `invalid_voice_name` si `name` no es un nombre de voz válido (también `.` o `..`), 400 `audio_too_long` si la referencia supera `MAX_CLONE_REFERENCE_SECS` y 413 si el cuerpo supera el tope derivado de ese límite (topes en `crates/avi-core/src/limits.rs`) | Clonar voz (audio base64) con streaming NDJSON y warm-on-clone: la precarga sigue en segundo plano tras el `result`, no se anuncia como etapa y `precomputed:true` significa «precarga iniciada» |
| `/dub` | POST | `{audio_b64, voice, source_language, target_language, temperature?}` | NDJSON `started → heartbeat/progress → transcript{text} → [translation{text}] → audio{pcm_b64}… → result{status:"dubbed", text, translated, voice, work_ms}` o `error` (`transcript` y `translation` llevan solo `event`, `text` y `schema_version`; `translation` solo sale si los idiomas difieren); antes del stream, 400 `invalid_voice_name` si `voice` no es un nombre de voz válido (también `.` o `..`) | Pipeline transcribe→translate→synthesize con streaming NDJSON |
| `/shutdown` | POST | — | `{status:"shutting_down"}` | `shutdown_handler`: mata el árbol del residente por PID solo si su identidad observada coincide con la registrada y espera su muerte, y `notify_one()` para cierre graceful sin `process::exit` |

7 rutas públicas (podadas `GET /voices` y `POST /voices/precompute`; sin legado). Las respuestas llevan `schema_version` y la cabecera `x-schema-version` con la versión del protocolo del daemon ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)), emitida por el módulo de emisión JSON del núcleo.

## Protocolo

- `synthesize`, `voices/clone`, `dub` y `transcribe`: NDJSON `application/x-ndjson` con `schema_version` y latidos periódicos (`STREAM_HEARTBEAT`). `synthesize` y `dub` emiten además un evento `audio` por fragmento de la entrega en streaming del motor, `{event:"audio", pcm_b64}` con PCM s16le de 24 kHz mono en base64; el `result` no lleva el audio. La versión del protocolo es la `8`: toda respuesta del daemon, en stream o en JSON, lleva la cabecera `x-schema-version` con ese valor, y la CLI rechaza a un daemon que anuncie otra con `daemon_version_mismatch` (exit 5), que se resuelve con `daemon restart`.
- **Los eventos no llevan texto humano.** `started` conserva solo los datos de la petición (`voice` y `text_length` en `/synthesize`, `name` en `/voices/clone`, `voice` en `/dub`), `heartbeat` y `progress` llevan únicamente `event` y `stage`, y `result`/`error` cierran el stream (el `error` sí lleva `reason` y `message`). Los dos eventos de texto de `/dub`, `transcript` y `translation`, llevan exactamente `event`, `text` y `schema_version`: `text` es contenido del usuario (la transcripción y el texto traducido), no texto de interfaz, y el cliente lo imprime por stdout en cuanto llega, antes de que suene el audio. Solo retirar, renombrar o cambiar el significado de una clave sube `DAEMON_SCHEMA_VERSION`; un cliente que no conozca un evento añadido lo ignora. El cliente es el dueño de todos los textos de interfaz: traduce cada `stage` a una etapa de su catálogo. `/translate` sigue siendo JSON plano; el `result` de `/transcribe` lleva `text` y `source_language`.
- **El mapa de etapas.** El daemon emite diez `stage` distintos: `warming` (calentando motor de síntesis), `queued` (en cola de síntesis), `loading_voice` (el motor arranca el residente de la voz pedida), `denoise_queue` (esperando turno para limpiar el ruido), `denoise` (limpiando ruido del audio), `synthesis` (sintetizando voz), `clone` (clonando voz), `save` (guardando la voz clonada), `transcribe` (transcribiendo audio) y `translate` (traduciendo texto). Cada etapa pesada se anuncia con un evento `progress` al empezar; mientras el trabajo continua, el daemon emite eventos `heartbeat` cada ~500 ms con la etapa vigente. La fase de síntesis emite de inmediato `queued`, `warming` o `synthesis` según el lock y el estado del motor, `synthesis` al obtener el lock tras esperar, y `loading_voice` seguido de `synthesis` si el motor arranca un residente. La CLI traduce `warming`, `queued`, `loading_voice` y `synthesis` a la misma etapa de síntesis con distinto texto vivo (`Calentando el motor…`, `Esperando turno para sintetizar…`, `Cargando la voz…`, `Sintetizando…`) y un solo reloj. `/voices/clone` emite `denoise` (si se pidió) y `save` antes del `result`, pero no emite etapa de precarga. Un `stage` fuera de esa lista se ignora: no se anuncia con un texto genérico que confunda una etapa con otra. La etapa se anuncia una vez sin terminal —la actividad repetida dentro de la misma etapa no abre línea nueva— y la línea de la etapa en curso se repinta en la terminal.
- `transcribe`: PCM `i16le 16kHz mono` base64 en `audio_b64`.
- `health_body` (función interna del daemon): `Warming → Warm → Failed(causa)`; `warm_error` solo si `Failed`. `GET /health` puede incluir `ct2`/`stt` aditivas `warm/warming/warm_failed` cuando residentes, sin bump `schema_version`. Esta máquina de estados la escriben los warmups (de arranque y de clonado: `Warming` al empezar el testigo, `Warm` o `Failed` al terminar) y cada síntesis completada, que la devuelve a `Warm`; no refleja una degradación del residente posterior a la última síntesis. La salud de síntesis se observa *por petición* (antes de reutilizar el residente en la síntesis TTS del módulo de audio) y puede detectar un residente degradado aunque `warm` siga en `Warm`.
- `translate`/`dub` (etapa de traducción) exigen el modelo de traducción provisionado vía `is_provisioned` (cada fichero de `MODEL_FILE_PATTERNS` presente y de más de 0 bytes); sin él responden `model_missing` (exit 4 en CLI).

## Gestión del ciclo de vida

**El progreso por stderr.** `daemon start`, `stop` y `restart` abren con la línea de contexto `Daemon · <host:puerto>`, pintan sus etapas por stderr y escriben lo mismo con y sin `--json`: `--json` solo cambia stdout. Las etapas son `Parada del daemon` (`stopping`), `Arranque` (`starting`) y `Calentamiento` (`warming`), numeradas `[n/N]` cuando hay dos o más. `daemon start` solo tiene `Parada` si encuentra un daemon degradado: lo detiene, escribe bajo esa etapa el aviso `⚠ El daemon que estaba en marcha no respondía bien; se detiene y se arranca de nuevo.` y sigue con el arranque. Cada etapa del arranque se abre cuando ocurre su trabajo: `Arranque` («Iniciando el daemon…») dura lo que dura el arranque real —el proceso, la publicación de su dirección y la respuesta de `/health`—; `Calentamiento` («Calentando el motor…») empieza solo después, cuando el daemon ya responde, y se cierra cuando `/health` informa el motor como `warm`, la misma condición con la que `daemon status` dice «motor listo». La espera del calentamiento es de hasta 10 s contados desde que el daemon responde. Si el calentamiento falla o no termina en ese plazo, el daemon sigue en marcha, la etapa queda como `○ Calentamiento · sigue calentándose` (o `· falló el calentamiento`) con un `⚠` debajo («El daemon está en marcha, pero el motor sigue calentándose; `ai-voice-interconnector daemon status` dirá cuándo está listo.» o «El daemon está en marcha, pero falló el calentamiento del motor; consulta `ai-voice-interconnector daemon status`.»), y el cierre pasa de `Daemon listo` a `Daemon en marcha`. Ningún comando de ciclo de vida del daemon escribe resultados por stdout: el cierre va por stderr y lo dice **una sola vez**, con la duración de la operación. `daemon start` cierra con `✓ Daemon listo en <total> · pid N` con el motor listo, `✓ Daemon en marcha en <total> · pid N` con el calentamiento sin terminar o fallido, y `✓ El daemon ya estaba en marcha · pid N` (sin etapas) si ya lo estaba. `daemon restart` comprueba antes de parar que el modelo está provisionado; sus etapas son `Parada del daemon`, `Arranque` y `Calentamiento` en dos fases (`Parada → Arranque`), y sin daemon en marcha la primera queda como `○ Parada del daemon · no había ningún daemon en marcha`; cierra con `✓ Daemon reiniciado en <total> · pid N` y el desglose por fase. `daemon stop` tiene una sola etapa, así que no escribe su línea: el cierre es `✓ Daemon detenido en <total>`, o `✓ No había ningún daemon en marcha.` sin etapa —porque, sin daemon en marcha, decir que se detuvo algo sería mentir—. Si la parada no concluye, `stop` muestra `✗ Parada del daemon`, el separador y el `Error:` con su `Solución:`. `daemon status` es la excepción: su línea de estado es un resultado y va por stdout.

`AVI_NO_PROGRESS=1` silencia solo las etapas por stderr: la línea de contexto, el cierre, el envelope `--json` y los códigos de salida no cambian.

**`start` (handler de inicio):** clasifica el daemon con `lifecycle::daemon_stop::observe`, el mismo criterio que `status`, a partir del daemon registrado (por `daemon.pid` o, si falta, por `daemon.ready`, solo si su identidad observada coincide con la registrada), del sondeo de `/health` y del residente registrado. En servicio (daemon propio vivo que responde) → `already_running` con salida 0. Degradado (daemon propio vivo que no responde, algo que responde sin daemon propio registrado, o residente registrado vivo sin daemon) → lo reclama la parada unificada `lifecycle::daemon_stop::stop`, la misma de `stop` y `restart` (árbol preciso por PID e identidad y residente por su PID registrado, nunca por imagen, con verificación), y rearranca desde cero con salida 0 y payload `started` (nunca `already_running` ciego); si la parada no concluye, sale con `daemon_stop_failed` (exit 16) sin arrancar otro daemon. Si no hay residual, `launch_daemon` lanza el hijo con `spawn_background` (función de spawn del daemon) con stdin nulo, stdout y stderr redirigidos al log del arranque (ver «Log por arranque») + `CREATE_NO_WINDOW|CREATE_NEW_PROCESS_GROUP` (Win; el daemon tiene una consola oculta propia que heredan los hijos de consola que lanza, así que no abren ventanas) / `setsid` (Unix); el hijo hereda solo la lista explícita (entrada nula y log del arranque): ningún otro handle del lanzador —incluido el `pipe` de `cargo test`— viaja al daemon por construcción. Luego espera, hasta `DAEMON_READY_DEADLINE` (`src/main.rs`), el resultado explícito del arranque, vigilando a la vez el fichero ready (`--ready-file`) y al propio hijo (`try_wait`): **listo** (registro de éxito con la `addr` efímera, seguido de la confirmación por `/health` con su propio presupuesto, `DAEMON_HEALTH_DEADLINE`, y siempre al menos una sonda aunque la publicación haya agotado el deadline), **fallo tipado** (registro de fallo: `port_in_use` exit 6, `voice_not_found` exit 3, `daemon_error` exit 1) o **muerte** del hijo sin registro (exit 1 `daemon_error` con su estado de salida). Si el hijo sigue vivo sin publicar al vencer el deadline, o `/health` no responde tras publicar, sale con exit 5 `daemon_unreachable`. Ante cualquier fallo mata el árbol del hijo si sigue vivo, lo recolecta y borra `daemon.ready`: no quedan proceso, pidfile ni fichero ready. La CLI no escribe el pidfile: lo escribe el propio daemon antes de publicar `daemon.ready` (`data_dir()/daemon.pid`, con su `pid`, su identidad, la `addr` efímera real y `started_at`, que el cliente lee luego vía `resolve_client_addr`). `launch_daemon` abre la etapa `starting` al entrar y la etapa `warming` tras el `/health` correcto, y devuelve junto al PID el estado del motor (listo, aún calentándose o con el calentamiento fallido), con el que el handler forma el cierre.

**`stop` (handler de parada):** parada unificada `lifecycle::daemon_stop::stop` (función unificada de parada) con deadline global (`STOP_DEADLINE_GLOBAL`, plazos en `crates/avi-lifecycle/src/daemon_stop.rs`): graceful (`POST /shutdown` + espera de `/health` down) si responde, árbol preciso por PID (`taskkill /F /T /PID` en Windows, invocado por su ruta fija `%SystemRoot%\System32\taskkill.exe` y con plazo acotado; `kill -9` al grupo en Unix; con guarda anti-auto-muerte) cuando sigue vivo, más liquidación del residente por su PID registrado (`read_resident_pid`, `kill_tree_resident_by_pid`), y verificación a nivel de sistema (probe + `pid_alive` + residente registrado muerto). El daemon se busca con `recorded_daemon`: el del pidfile y, si este falta o es ilegible, el que publica `daemon.ready` (`pid`, `start=`, `image=` y `addr`), de modo que un pidfile borrado a mano no hace que `stop` diga «No había ningún daemon en marcha.» con el daemon vivo. Sin dirección registrada, el sondeo y el `POST /shutdown` van a la dirección derivada de `AVI_DAEMON_PORT` (`default_addr()`), la misma en la que escucha el daemon; `restart` usa la misma reserva. La parada no se da por completa mientras el residente registrado en el pidfile (`resident_pid`) siga vivo, aunque el daemon haya muerto. El pidfile solo se borra tras muerte verificada; si el árbol o el residente siguen vivos se conserva la pista y la parada sale por veredicto: envelope con `status` `still_running` y el veredicto completo (`daemon_fully_stopped: false`, `resident_alive`, `pid`, `remaining`), con exit 5 y sin objeto `error` (en `self install`, `self uninstall`, `self update` y `cleanup`, que reutilizan la parada, el fallo es `daemon_stop_failed`). Con la parada completa se borra también `daemon.ready`, pero solo si el PID que contiene el propio fichero no está vivo; un `daemon.ready` ilegible, sin PID o con el PID del propio proceso es obsoleto y se borra. `daemon.ready` guarda el PID del daemon, no el del residente, y su identidad en las líneas `start=` e `image=` (hora de creación y nombre de imagen; en Linux se lee de `/proc/<pid>/stat` y en macOS de `proc_pidinfo`). Un PID registrado, en el pidfile o en el ready, cuya identidad no coincide con la del proceso que hoy lo tiene es un proceso ajeno (el sistema reasignó el número): se trata como muerto, no se mata y no bloquea la parada; lo mismo vale para el reclamo de `start` y para Ctrl+C. Un fichero sin identidad registrada es ilegible: se descarta con aviso por stderr y se propaga como ausencia, sin degradar a «PID solo». Un ready sin las líneas `start=` e `image=` no declara a su PID como propio: cuenta como muerto, no bloquea la parada y se borra por obsoleto. Límite: sin pidfile el daemon se encuentra por `daemon.ready`, pero no hay `resident_pid` que reclamar ni verificar: un residente vivo sin registrar no se mata y el veredicto no lo ve. Sin kill por imagen; el residente muere solo cuando muere su daemon; `netstat` y `pkill` quedan eliminados.

**El envelope de `stop`.** El binario compone el envelope con el `StopOutcome` del motor
y no vuelve a sondear por su cuenta: `status` es `not_running` cuando
`was_running` es falso y `shutdown_sent` cuando hubo un daemon que se verificó
detenido; en los dos casos el envelope lleva `daemon: "stopped"`, `was_running` y
`daemon_fully_stopped` —el veredicto completo de la verificación a nivel de
sistema— con salida 0. Si la parada no concluye, el `status` es `still_running`
con `daemon: "running"` y el veredicto completo (`was_running`,
`daemon_fully_stopped: false`, `resident_alive`, `pid`, `remaining`), con salida
5 por veredicto y sin objeto `error`: quien automatiza distingue «sigue el árbol»
de «solo quedó el residente» sin leer el texto humano. El texto humano es el cierre
`✓ Daemon detenido en <total>`, `✓ No había ningún daemon en marcha.` o, con el
veredicto `still_running`, un `Error:` con el matiz del residente, según el mismo
dato. Con un daemon detenido el envelope lleva `context` (`addr`), `total_ms`,
`stages` (la etapa `stopping`) y `warnings`; con `not_running` lleva solo `context`,
sin etapas. El envelope de `stop` bindea el veredicto del motor (`StopOutcome`, calculado antes de tocar nada): una firma `shutdown_sent` sin daemon previo no es un veredicto válido.

```json
{
  "context": { "addr": "127.0.0.1:8765" },
  "daemon": "stopped",
  "daemon_fully_stopped": true,
  "schema_version": "7",
  "stages": [
    { "end_ms": 500, "phase": "stop", "stage": "stopping", "start_ms": 0 }
  ],
  "status": "shutdown_sent",
  "total_ms": 500,
  "warnings": [],
  "was_running": true
}
```

```json
{
  "context": { "addr": "127.0.0.1:8765" },
  "daemon": "stopped",
  "daemon_fully_stopped": true,
  "schema_version": "7",
  "status": "not_running",
  "was_running": false
}
```

Con `still_running` el envelope lleva además el veredicto: `"status": "still_running"`, `"daemon": "running"`, `"was_running": true`, `"daemon_fully_stopped": false`, `"resident_alive"`, `"pid"` y `"remaining"`, junto a `context`, `stages` (con `failed: true` en `stopping`) y `warnings`.

**Vida del motor residente.** El daemon (y la CLI, en el clonado por proceso local) lanza el motor `qwen_tts` con `--watch-stdin` y mantiene abierto el extremo de escritura de una tubería conectada a su entrada estándar mientras el motor vive. Con ese flag el motor arranca, tras parsear los argumentos y antes de cargar el modelo, un hilo que lee su entrada estándar hasta fin de fichero y entonces termina el proceso (`_exit(0)`), tanto en modo residente como en clonado. Si el proceso que lo lanzó muere, de forma ordenada o con un kill duro, el sistema operativo cierra la tubería y el motor termina solo en pocos segundos (medido: 2-3 s), sin depender de un grupo de procesos del sistema ni de un barrido. Sin el flag, el motor no cambia. En macOS, si el daemon lanza dos procesos a la vez, uno podría heredar el extremo de escritura de la tubería del otro y retrasar su cierre como mucho lo que dura un clonado; es un riesgo teórico, sin verificar en esa plataforma, que se acepta. Cuando el lanzador suelta el residente de forma ordenada (al cambiar de voz o al terminar la CLI directa), lo mata y lo recolecta en el acto, porque mientras lo gestiona retiene la tubería y el motor no puede terminar solo.

**`restart` (handler de reinicio):** parada unificada, con un único deadline global (`STOP_DEADLINE_GLOBAL`) y un único reclamo por PID; si no concluye, `daemon_stop_failed` y fin → `launch_daemon` fresco, la misma secuencia que `start` con los mismos códigos de fallo y la misma limpieza, con la espera de la publicación acotada al restante del presupuesto único del reinicio (nunca por encima de `DAEMON_READY_DEADLINE`) y la confirmación por `/health` con su presupuesto aparte (`DAEMON_HEALTH_DEADLINE`, ambas en `src/main.rs`) → `write pid` con payload `restarted` (sin `/restart` dedicado).

**Log por arranque.** Cada `start` y `restart` crea `data/logs/daemon_<token>_<ms>.log` (`<ms>` la marca de creación en milisegundos) y le redirige stdout y stderr del daemon. `<token>` (`<pid>-<inicio>`) es el del daemon hijo, no el de la CLI lanzadora: el fichero nace con el del lanzador porque su handle se entrega al spawn, y en cuanto se conoce el del hijo se renombra para que el nombre identifique al proceso exacto que escribe y la poda pueda reconocer por identidad el log de un daemon vivo. El renombrado nunca pisa un fichero: si ya existe uno con el nombre de destino, queda intacto y el arranque se aborta con `io_error`. Si el arranque falla (hijo caído, fallo publicado como `port_in_use` u otro, deadline vencido o `/health` sin respuesta), el mensaje de error de la CLI termina con `Log del daemon: <ruta>`, sin cambiar `reason` ni el exit code. Si el log no se puede crear, el error es `daemon_error` con «No se pudo crear el log del daemon». Se conservan los 10 logs `daemon_*` más recientes (por el `<ms>` del nombre); la poda se hace al crear uno nuevo y no toca otras familias ni ficheros ajenos, y un log cuyo propietario sigue vivo o no es observable se conserva siempre y no cuenta para la retención. Nivel por defecto del daemon: `info` de los crates propios y `warn` de las dependencias, salvo que `RUST_LOG` esté definida: manda si lo está (sintaxis de `tracing`, tolerante a directivas inválidas) y, sin ella o vacía, rige el nivel por defecto; es distinto del de la CLI, que emite `warn` y superiores. El motor Qwen3-TTS residente (y el de `voice clone`) escribe en `data/logs/qwen3-tts_<token>_<ms>.log`, con `<token>` el del proceso que lo crea (el daemon, o la CLI en la vía directa); la salida del clonado nunca se vuelca en la terminal, y se conservan los 10 logs más recientes por familia.

**`status` (handler de estado):** primero la sonda del daemon (`daemon_stop::probe`, la misma de `start`), cuyo rechazo es inmediato sin daemon; solo si responde, un `GET /health` con plazo propio cuyo cuerpo aporta `engine`, `warm` y `warm_error`. El éxito de ese `GET` es el sondeo con el que `observe` clasifica: un daemon que muere entre la sonda y el `GET` cuenta como «no responde». `daemon` vale `running` solo con un daemon propio en servicio, y entonces lleva su `pid`; en cualquier otro caso vale `stopped`, y si quedan restos añade `degraded` con su código: `unresponsive` (el daemon registrado sigue vivo pero no responde), `unregistered` (algo responde en la dirección del daemon sin un daemon registrado) u `orphan_resident` (el residente sigue vivo sin daemon). Son los casos que `start` reclama antes de arrancar y que `stop` retira. Siempre sale 0, con `schema_version` ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema); fixtures `tests/golden/cli_daemon_status.json` y `cli_daemon_status_running.json`):

```json
{ "schema_version": "7", "daemon": "running", "pid": 1234, "engine": "rust_native", "warm": "warm" }
{ "schema_version": "7", "daemon": "stopped" }
{ "schema_version": "7", "daemon": "stopped", "degraded": "unresponsive" }
```

Esta clasificación es la detección de residual *al arrancar*; es un mecanismo distinto de la salud observada *por petición* dentro de una sesión ya viva que verifica la salud del residente antes de reutilizarlo en la síntesis TTS — no debe leerse como que esa verificación ya estaba resuelta por esta vía.

## Esquema de `daemon.pid`

`data_dir()/daemon.pid` persiste un objeto JSON: `{ "pid": u32, "resident_pid": u32, "addr": string, "started_at": string, "identity": { "start": string, "image": string }, "resident_identity": { "start": string, "image": string }, "resident_port": u16 }`. Se escribe de forma atómica (temporal hermano y renombrado) desde el estado en memoria del daemon. Todas las claves son obligatorias: tienen que estar aunque el valor sea `null` (`addr`, `started_at`, las identidades y `resident_port` lo admiten), y un fichero al que le falte alguna es ilegible y se descarta con un aviso que nombra la clave. `identity` y `resident_identity` guardan la hora de creación y el nombre de imagen observados al registrar al daemon y al residente. Con `AVI_DATA_DIR` el pidfile es por instancia.

El residente TTS se registra en cuanto el daemon lo lanza y antes de su comprobación de salud: `resident_pid` es su PID (`0` sin residente), `resident_identity` su identidad y `resident_port` el puerto efímero de loopback que el lanzador reservó para él; cada arranque de residente reserva el suyo, sin puerto fijo ni variable de entorno, de modo que la salud de un residente nunca la responde otro. Si no se puede observar su identidad o registrarlo, se mata lo lanzado y el arranque del residente falla. El cierre ordenado del daemon mata al residente solo si la identidad observada ahora coincide con la registrada, espera su muerte y anula el registro. La parada unificada (`stop`, `restart` y el reclamo de `start`) lo mata por su PID registrado. El `daemon.ready` de éxito lleva además las líneas `start=` e `image=` con la identidad del daemon.

## Decisiones de diseño

- **Transporte HTTP (no stdio)**: contrato del transporte HTTP estable para clientes externos.
- **Captura siempre de cliente**: el daemon recibe PCM base64, nunca rutas ni dispositivos.
- **Una carpeta de datos, un daemon**: el estado del daemon vive en un único `daemon.pid` por carpeta de datos, y la CLI descubre la dirección leyéndolo. Dos daemons que compartan carpeta con puertos distintos (`AVI_DAEMON_PORT`) no están soportados: el segundo no arranca, porque el daemon rechaza registrarse (`startup_failed`) cuando el pidfile nombra a otro daemon vivo con identidad coincidente. Para varias instancias hay que darle a cada una su propia carpeta con `AVI_DATA_DIR`, como hacen las pruebas aisladas.
- **Un solo escritor del registro**: `daemon.pid` lo escribe únicamente el daemon, completo y de forma atómica, justo después de enlazar el puerto y antes de publicar `daemon.ready` y de cualquier warmup; si no puede observar su propia identidad no arranca. La CLI solo lo lee y lo borra tras una parada verificada. Si el pidfile se borra con el daemon vivo, `stop` y el reclamo de `start` lo encuentran por `daemon.ready` (`pid`, identidad y dirección), y el daemon recrea el pidfile en su siguiente registro.
- **Residente en puerto efímero**: el TTS habla con su propio servidor Qwen3-TTS en un puerto efímero de loopback gestionado por `avi-tts`: cada arranque de residente reserva el suyo y el daemon lo anota en `resident_port` de `daemon.pid`. Ese puerto es el canal de servicio, no la identidad del proceso. El daemon registra al residente (`resident_pid`, `resident_identity`, `resident_port`) en cuanto lo lanza y antes de su comprobación de salud, y la parada, el reclamo y la cosecha por identidad liquidan su proceso por su identidad estable: el `resident_pid` registrado (kill y verificación por viveza de PID); sin PID registrado no hay residente que reclamar y nada se mata por imagen. El cierre ordenado del daemon mata y espera al residente antes de terminar y antes de cualquier reinicio en proceso; la vía directa no registra residente y nunca toca `daemon.pid`. No se descubre ni se verifica el cierre sondeando puertos. Un PID cuya identidad actual no coincide con la registrada se ha reasignado a otro programa, cuenta como muerto y no se mata; un fichero sin identidad es ilegible (se descarta con aviso y cuenta como ausente), sin degradar a «PID solo».

## Códigos de error

Los fallos del arranque se traducen a código en un único punto, compartido por `start`, `restart` y `serve`, así que la misma causa da el mismo código en los tres. Con `--json` el objeto de error lleva `reason` y `error`.

| `reason` | Exit | Causa | Comandos |
|---|---|---|---|
| `model_missing` | 4 | Falta algún modelo de los cuatro obligatorios (síntesis, clonado, traducción o transcripción): el daemon no arranca y pide ejecutar `setup` | `start`, `restart`, `serve` |
| `port_in_use` | 6 | El puerto del daemon está en uso o reservado por otro proceso. Remedio: liberar el puerto o arrancar en otro con `AVI_DAEMON_PORT=<puerto>` (`0` = puerto efímero) | `start`, `restart`, `serve` |
| `invalid_voice_name` | 2 | El nombre de `--warm-voice` no es un nombre de voz válido | `start`, `serve` |
| `voice_not_found` | 3 | La voz de `--warm-voice` no existe | `start`, `serve` |
| `daemon_error` | 1 | Fallo al lanzar el hijo, muerte del hijo durante el arranque (el mensaje incluye su estado de salida), fallo no clasificado previo a estar listo o fallo del servidor ya en marcha | `start`, `restart`, `serve` |
| `daemon_unreachable` | 5 | El hijo sigue vivo sin publicar su dirección al vencer el deadline, `/health` no responde tras publicarla | `start`, `restart` |
| `daemon_stop_failed` | 16 | La parada previa no concluyó (sigue vivo el daemon o su residente): no se arranca otro daemon y el mensaje lleva lo que quedó vivo | `start` (al reclamar un residual degradado), `restart` |
| `daemon_version_mismatch` | 5 | El daemon en marcha es de otra versión del protocolo; se resuelve con `daemon restart` | los comandos que usan el daemon (`speech`, `translate`, `voice clone`) |
| `lifecycle_locked` | 17 | Hay una operación de ciclo de vida en curso: el comando no llega a ejecutarse ni lanza nada. `serve` toma el bloqueo compartido y lo suelta al quedar registrado en `daemon.pid`; `start` y `restart` lo retienen mientras esperan el `daemon.ready` del hijo | `start`, `restart`, `serve` |

Tras un fallo de `start` o `restart` no quedan proceso hijo, `daemon.pid` ni `daemon.ready`.

## Foreground vs background

| Aspecto | `daemon start` | `daemon serve` |
|---|---|---|
| Proceso | Proceso hijo lanzado con `spawn_background` | Mismo proceso CLI (si el daemon muere, el residente termina solo gracias al vigía por tubería) |
| PID | `data_dir()/daemon.pid`, escrito solo por el daemon, con `pid`, `resident_pid`, `resident_port` y sus identidades `identity` y `resident_identity`; `resident_port` es el puerto efímero de loopback que reservó el lanzador del residente | Se registra en el mismo `daemon.pid`, aunque el proceso CLI permanezca en foreground |
| `--json` | Sí (`started` tras arranque o reclamo / `already_running` solo si está en servicio) | No |
| Warmup | background `spawn_blocking` | igual |
| Señales | Ctrl+C del CLI con limpieza acotada (`CTRL_C_CLEANUP_DEADLINE`) (exit 130 preservado, con PID en memoria si aún no hay pidfile) | Ctrl+C (todos los SO) y SIGTERM (solo Unix; en Windows no hay equivalente escuchado) en `run_daemon_server` por la misma ruta que `POST /shutdown` (cobertura `serve` Unix sin auto-muerte del CLI) |

Supervisión configurable: `start`/`serve` con `--auto-restart` habilitan `run_supervised` (función de supervisión del daemon) con contador `retries`, backoff exponencial acotado, hasta `max_retries` (default 3), y un **watchdog de supervisión** (`SUPERVISION_PROGRESS_MIN`, `SUPERVISION_STREAK_MAX`, en `crates/avi-daemon/src/lib.rs`): si el daemon sufre `SUPERVISION_STREAK_MAX` caídas rápidas seguidas con vida menor a `SUPERVISION_PROGRESS_MIN` (sin progreso), aborta con error explícito evitando bucles de reinicio ciegos. Un fallo previo a que el daemon haya estado listo alguna vez (puerto ocupado, voz inexistente, fallo al cargar el estado) es de configuración: es terminal, se publica en el fichero ready y no se reintenta; la supervisión solo recupera caídas de un daemon que llegó a servir. Antes de cada reintento hay reclamo activo del árbol propio previo con deadline y verificación (muerte + puerto libre en el log; el `Drop` previo ya mató el árbol preciso, sin kill global ni otra instancia); el reclamo activo matar-y-rearrancar ante otra instancia vive en `daemon start`, nunca en `serve`. Un apagado graceful vía `POST /shutdown` (`shutdown_notify`) no reintenta; solo los crashes reintentan. Sin `--auto-restart`, el daemon es `fail-stop`. No hay flags de idioma en `start`/`serve`: `--source-language`/`--target-language` son locales a `translate` y al grupo `speech`, los motores no reciben parámetro de idioma, y la transcripción y la traducción en el daemon dependen de las features de compilación `native-stt` y `native-translation`.

---

## Muestras de salida

La línea de contexto, las etapas y el cierre van por stderr; solo `daemon status` escribe por stdout. Las duraciones son orientativas: dependen del equipo.

```text
# daemon start
Daemon · 127.0.0.1:8765
  [1/2] ✓ Arranque       4,0 s
  [2/2] ✓ Calentamiento  4,0 s
──────────────────────────────────────────────────
✓ Daemon listo en 8,0 s · pid 6148

# daemon start, con el daemon ya en marcha
Daemon · 127.0.0.1:8765
✓ El daemon ya estaba en marcha · pid 6148

# daemon stop, con el daemon en marcha
Daemon · 127.0.0.1:8765
✓ Daemon detenido en 0,5 s

# daemon stop, sin daemon en marcha
Daemon · 127.0.0.1:8765
✓ No había ningún daemon en marcha.

# daemon restart, con el daemon en marcha
Daemon · 127.0.0.1:8765
Parada → Arranque

Parada
  [1/3] ✓ Parada del daemon  0,5 s
Arranque
  [2/3] ✓ Arranque           3,5 s
  [3/3] ✓ Calentamiento      4,0 s
──────────────────────────────────────────────────
✓ Daemon reiniciado en 8,0 s · pid 4812
  Parada 0,5 s · Arranque 7,5 s

# daemon restart, sin daemon en marcha
Daemon · 127.0.0.1:8765
Parada → Arranque

Parada
  [1/3] ○ Parada del daemon · no había ningún daemon en marcha
Arranque
  [2/3] ✓ Arranque           3,5 s
  [3/3] ✓ Calentamiento      4,0 s
──────────────────────────────────────────────────
✓ Daemon reiniciado en 7,5 s · pid 5236
  Parada 0,0 s · Arranque 7,5 s

# daemon status
Daemon: en marcha (pid 7320, puerto 8765, motor listo).
```

Con `--json`, stdout lleva **solo** el envelope (`started`, `already_running`, `restarted`, `shutdown_sent`, `not_running` o `still_running`, con las claves comunes) y stderr es el mismo que sin `--json`. Por ejemplo, `--json daemon start` con el daemon ya en marcha:

```json
{
  "context": { "addr": "127.0.0.1:8765", "auto_restart": false, "warm_voice": "default" },
  "daemon": "running",
  "pid": 6148,
  "schema_version": "7",
  "status": "already_running"
}
```

y `--json daemon status` con el daemon en marcha:

```json
{
  "daemon": "running",
  "engine": "rust_native",
  "pid": 6148,
  "schema_version": "7",
  "warm": "warm"
}
```
