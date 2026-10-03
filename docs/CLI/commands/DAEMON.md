# Comando `daemon` — ciclo de vida del daemon nativo

El daemon es un servidor `Axum` (`crates/avi-daemon`) que mantiene los modelos Qwen3-TTS, Parakeet TDT v3 y opus-mt (CT2) `es↔en` en memoria, evitando la carga en cada invocación (~15–30 s). El CLI actúa como cliente HTTP sobre `127.0.0.1:8765` por defecto (`DAEMON_ADDR`), con override por instancia vía `AVI_DAEMON_PORT` (`0` = puerto efímero, el servidor imprime la dirección realmente enlazada con `local_addr()`). El cliente CLI descubre la dirección efímera leyendo el campo `addr` del pidfile (`resolve_client_addr`); sin pidfile, o con pidfile de esquema viejo sin ese campo, cae a la dirección derivada de `AVI_DAEMON_PORT` (`127.0.0.1:8765` si la variable falta o no es un puerto).

## Definición CLI

`enum DaemonCommands` (definición de subcomandos del daemon):

| Subcomando | Parámetros | Descripción |
|---|---|---|
| `daemon start` | `--json` `--auto-restart` `--max-retries` (default 3) `--warm-voice` (default `default`) | Revalida el residual (PID vivo + probe): sano → `already_running`, degradado → reclama el árbol y rearranca con `started`; si no, lanza en background con `launch_daemon` (resultado explícito: listo, fallo tipado o muerte del hijo; ante fallo no deja proceso, pidfile ni fichero ready); con `--auto-restart` los reintentos parten de reclamo activo del árbol propio previo con deadline y verificación |
| `daemon stop` | `--json` | Parada unificada con deadline global de 8 s (`lifecycle::daemon_stop::stop`: graceful + árbol preciso + verificación por puerto y PID registrado; borra `daemon.pid` solo tras muerte verificada, exit 5 sin borrar pista si sigue vivo) |
| `daemon restart` | `--json` | Ayudante único de parada (sin doble techo ni kill duplicado) → `start` fresco con presupuesto de 12 s (sin flags de supervisión heredados; calienta `default`) |
| `daemon status` | `--json` | `GET /health` → `running`/`stopped` + `warm` |
| `daemon serve` | `--auto-restart` `--max-retries` (default 3) `--warm-voice` (default `default`) | Ejecuta servidor en foreground (`run_supervised` con escucha de Ctrl+C/SIGTERM por la misma ruta que `POST /shutdown`); un fallo previo a estar listo sale con el mismo código que en `start` (ver [Códigos de error](#códigos-de-error)) |

`start`/`serve` aceptan `--auto-restart`/`--max-retries`/`--warm-voice`; `start`/`stop`/`restart`/`status` aceptan `--json` ( `serve` sin `--json`). `start`/`restart` exigen modelo provisionado (`require_model_provisioned`), `stop`/`status` no.

**`--warm-voice <nombre>` (default `default`):** selecciona qué voz precalienta el daemon al arranque en vez de forzar `default` (el residente TTS es de una sola voz). `start` lo propaga al `serve` que respawnea (`spawn_background` anexa `--warm-voice`); `restart` calienta siempre `default`. Validación *fail-fast*: el arranque valida la voz y enlaza el puerto **antes de cargar los modelos**. Si la voz no existe sale con exit 3 `voice_not_found` (no degrada en silencio ni cae a `default`); si el puerto está ocupado sale con exit 6 `port_in_use`; en ambos casos al instante, sin esperar el deadline de arranque.

## Despacho del handler

`handle_daemon(json_mode, action)` (despachador principal del daemon):

```
handle_daemon
 ├── Serve  → Job KILL_ON_JOB_CLOSE (Windows) → run_supervised(dirección resuelta por `resolve_daemon_addr`, default 127.0.0.1:8765) (foreground, warmup background, Ctrl+C/SIGTERM por la misma ruta que POST /shutdown)
 ├── Start  → classify_residual (PID vivo + probe) → Sano: already_running | Degradado: reclamar árbol + rearrancar → started : launch_daemon → write pid
 ├── Stop   → lifecycle::daemon_stop::stop (deadline 8 s) → muerte verificada? → remove pid + shutdown_sent : exit 5 sin borrar pista
 ├── Restart→ lifecycle::daemon_stop::stop → launch_daemon (presupuesto 12 s) → write pid
 │           launch_daemon: spawn_background → espera del resultado (listo | fallo tipado | muerte del hijo) → /health
 │                          ante fallo: kill del árbol si sigue vivo + borrado de daemon.ready
 └── Status → GET /health (500ms timeout) → running/stopped + warm/engine
```

`serve` no usa subproceso; `start` y `restart` lanzan el daemon con `launch_daemon`, que usa `avi_daemon::spawn::spawn_background`. El handler Ctrl+C está instalado en `main` para todos los modos: limpieza acotada de 2 s sobre el árbol del pidfile —o del PID que `launch_daemon` conserva en memoria desde el spawn, en la ventana antes de escribir el pidfile, tanto en `start` como en `restart`— y salida 130 preservada.

## Arquitectura del daemon

```
CLI (ai-voice-interconnector)
  ├── handle_daemon ──► spawn_background ──► daemon (Axum)
  │                       │  stdin a null, stdout/stderr al log del arranque; corte de herencia por SetHandleInformation (Win) / setsid (Unix)
  │                       ▼
  │                  DaemonState ──► Qwen3TtsEngine (resident qwen_tts)
  │                       ├── warm: RwLock<WarmState> (Warming/Warm/Failed)
  │                       ├── synthesis_lock: Mutex<()>
  │                       ├── ct2_engine: Option<HashMap<String,Ct2TranslationEngine>> (native-translation)
  │                       └── shutdown_notify: Arc<Notify>
  └── DaemonIPCClient (reqwest) ◄──► Axum Router
```

`DaemonState { synthesis_lock, voice_store, speech_store, tts_engine, stt_engine, ct2_engine, warm, shutdown_notify }` (campos del estado del daemon).
`ct2_engine` carga al arrancar el motor residente de cada par provisionado (`is_provisioned`, el mismo gate que usa el loader); si falta o no carga, es `None` y la petición de traducción responde `model_missing` sin derribar el servidor (`DaemonState::new`).
`run_daemon_server` arranca en este orden: valida la voz de warmup, enlaza el `TcpListener`, carga el estado (`DaemonState::new`, los modelos), anuncia la dirección realmente enlazada (`local_addr()`, con `:0` el SO asigna) y publica el registro de éxito, y sirve. Así un fallo de configuración se detecta antes de pagar la carga, y el puerto queda reservado durante ella (una conexión en esa ventana espera en el backlog). Tras el warmup emite el evento `avi-daemon-ready warm=<warm|warm_failed> addr=<real>` en stderr como diagnóstico redundante; el transporte que consume el padre (o el harness de pruebas) con espera acotada es el fichero designado por `--ready-file`, escrito de forma atómica con uno de dos registros: el de éxito (`addr`/`warm`/`pid`) o, si el arranque falla antes de estar listo, el de fallo (`error=port_in_use|warm_voice_missing|startup_failed` con `port`, `voice` o `message`, nunca `addr`, porque los lectores toman `addr` como listo). Solo `run_supervised` publica el registro de fallo, justo antes de terminar. Ya en marcha: `spawn_blocking(warm_voice_engine)`, `with_graceful_shutdown` por la misma ruta desde `POST /shutdown` y desde Ctrl+C/SIGTERM (`tts_engine.shutdown()` preciso-primero + `notify_one()`). Cierre garantizado: Job `KILL_ON_JOB_CLOSE` en `Serve` (Windows), kill preciso del árbol por PID con verificación (función de spawn del daemon) y reclamo matar-y-rearrancar en `start` (bloque de reclamo y rearranque del binario principal, en Unix ante líder muerto además por grupo con verificación por 8766 cerrado; muerte del residente por PID registrado).

## Endpoints

| Endpoint | Método | Request | Response | Descripción |
|---|---|---|---|---|
| `/health` | GET | — | `{status:"ready", warm, engine, warm_error?, ct2?, stt?}` + `schema_version="4"` | Readiness + warmup (7 rutas) |
| `/synthesize` | POST | `{text, voice}` | NDJSON `start → progress → result{audio_b64}` o `error` | Síntesis streaming 24 kHz |
| `/transcribe` | POST | `{audio_b64, source_language}` | `{text}` + `schema_version="4"`; error con `{status:"error", reason, message}`: 400 (`usage_error` sin audio, `invalid_audio` si el base64 no es válido, `audio_too_long` si hay más de 300 s de muestras), 413 (cuerpo de más de 13 848 576 B) o 500 (`transcription_failed`) | Transcripción Parakeet (feature `native-stt`) |
| `/translate` | POST | `{text, from, to}` | `{translated, source, target}` o `error` | Traducción CT2 residente (feature `native-translation`) |
| `/voices/clone` | POST | `{name, audio_b64, force?}` | NDJSON `started → heartbeat/progress → result{name, speech, precomputed:true}` o `error`; antes del stream, 400 `audio_too_long` si la referencia dura más de 30 s y 413 si el cuerpo supera 31 768 576 B | Clonar voz (audio base64) con streaming NDJSON y warm-on-clone (`precomputed:true` = precarga iniciada) |
| `/dub` | POST | `{audio_b64, from, to, voice}` | NDJSON `started → heartbeat → result{status:"dubbed", text, translated, audio_b64, voice, work_ms}` o `error` | Pipeline transcribe→translate→synthesize con streaming NDJSON |
| `/shutdown` | POST | — | `{status:"shutting_down"}` | `shutdown_handler`: mata el árbol preciso del residente por PID registrado + `notify_one()` para cierre graceful sin `process::exit` |

7 rutas públicas (podadas `GET /voices` y `POST /voices/precompute`; sin legado). Prefijo `x-schema-version: 3` (módulo de emisión JSON del núcleo).

## Protocolo

- `synthesize`, `voices/clone` y `dub`: NDJSON `application/x-ndjson` con `schema_version` y latidos periódicos cada 500 ms (`STREAM_HEARTBEAT`).
- `transcribe`: PCM `i16le 16kHz mono` base64 en `audio_b64`.
- `health_body` (función interna del daemon): `Warming → Warm → Failed(causa)`; `warm_error` solo si `Failed`. `GET /health` puede incluir `ct2`/`stt` aditivas `warm/warming/warm_failed` cuando residentes, sin bump `schema_version`. Esta máquina de estados la escriben los warmups (de arranque y de clonado: `Warming` al empezar el testigo, `Warm` o `Failed` al terminar) y cada síntesis completada, que la devuelve a `Warm`; no refleja una degradación del residente posterior a la última síntesis. La salud de síntesis se observa *por petición* (antes de reutilizar el residente en la síntesis TTS del módulo de audio) y puede detectar un residente degradado aunque `warm` siga en `Warm`.
- `translate`/`dub` (etapa de traducción) exigen el modelo de traducción provisionado vía `is_provisioned` (cada fichero de `MODEL_FILE_PATTERNS` presente y de más de 0 bytes); sin él responden `model_missing` (exit 4 en CLI).

## Gestión del ciclo de vida

**`start` (handler de inicio):** revalida el residual por PID vivo + probe (`classify_residual`, función de clasificación de residual): sano (probe + PID vivo) → `already_running` con salida 0; degradado (probe y PID discrepan: colgado, pista rancia o sin pista —incluido `Stopped` con residente vivo por 8766—) → reclama el árbol preciso (`reclaim_degraded_residual`, en Unix ante líder muerto además por grupo con verificación por 8766 cerrado + PID sin viveza; ante residente-solo reclama por PID registrado con verificación por puerto 8766, nunca imagen del daemon) y rearranca desde cero con salida 0 y payload `started` (nunca `already_running` ciego). Si no hay residual, `launch_daemon` lanza el hijo con `spawn_background` (función de spawn del daemon) con stdin nulo, stdout y stderr redirigidos al log del arranque (ver «Log por arranque») + `CREATE_NO_WINDOW|CREATE_NEW_PROCESS_GROUP` (Win; el daemon tiene una consola oculta propia que heredan los hijos de consola que lanza, así que no abren ventanas) / `setsid` (Unix); el no-heredar el `pipe` de `cargo test` lo garantiza el corte de herencia en la raíz (`SetHandleInformation` en `disinherit_standard_handles`), no una creation flag. Luego espera, hasta 10 s (`DAEMON_READY_DEADLINE`), el resultado explícito del arranque, vigilando a la vez el fichero ready (`--ready-file`) y al propio hijo (`try_wait`): **listo** (registro de éxito con la `addr` efímera, seguido de la confirmación por `/health` con el tiempo restante), **fallo tipado** (registro de fallo: `port_in_use` exit 6, `voice_not_found` exit 3, `daemon_error` exit 1) o **muerte** del hijo sin registro (exit 1 `daemon_error` con su estado de salida). Si el hijo sigue vivo sin publicar al vencer el deadline, o `/health` no responde tras publicar, sale con exit 5 `daemon_unreachable`. Ante cualquier fallo mata el árbol del hijo si sigue vivo, lo recolecta y borra `daemon.ready`: no quedan proceso, pidfile ni fichero ready. Con éxito, `write_daemon_pid` (`data_dir()/daemon.pid`, extendido con `resident_pid` y con la `addr` efímera resuelta, que el cliente lee luego vía `resolve_client_addr`).

**`stop` (handler de parada):** parada unificada `lifecycle::daemon_stop::stop` (función unificada de parada) con deadline global de 8 s (`STOP_DEADLINE_GLOBAL`): graceful (`POST /shutdown` 1,5 s + espera de `/health` down hasta 3 s) si responde, árbol preciso por PID (`taskkill /F /T /PID` en Windows, `kill -9` al grupo en Unix, con guarda anti-auto-muerte) cuando sigue vivo, más liquidación del residente por su PID registrado (`read_resident_pid`, `kill_tree_resident_by_pid`), y verificación a nivel de sistema (probe + `pid_alive` + 8766 cerrado). El pidfile solo se borra tras muerte verificada; si el árbol sigue vivo se conserva la pista y se falla con exit 5. Sin kill por imagen para el daemon ni para el residente; `netstat` y `pkill` quedan eliminados.

**`restart` (handler de reinicio):** parada unificada sobre el ayudante único (sin doble techo `timeout(5s, wait_health_down(5s))` ni kill por PID duplicado) → `launch_daemon` fresco, la misma secuencia que `start` con los mismos códigos de fallo y la misma limpieza, con el deadline acotado al restante del presupuesto de 12 s (nunca más de 10 s) → `write pid` con payload `restarted` (sin `/restart` dedicado).

**Log por arranque.** Cada `start` y `restart` crea `data/logs/daemon_<pid>_<ms>.log` (`<pid>` es el de la CLI lanzadora, `<ms>` la marca de creación en milisegundos) y le redirige stdout y stderr del daemon. Si el arranque falla (hijo caído, fallo publicado como `port_in_use` u otro, deadline vencido o `/health` sin respuesta), el mensaje de error de la CLI termina con `Log del daemon: <ruta>`, sin cambiar `reason` ni el exit code. Si el log no se puede crear, el error es `daemon_error` con «No se pudo crear el log del daemon». Se conservan los 10 logs `daemon_*` más recientes (por el `<ms>` del nombre); la poda se hace al crear uno nuevo y no toca otras familias ni ficheros ajenos. Nivel por defecto del daemon: `info` de los crates propios y `warn` de las dependencias, salvo que `RUST_LOG` esté definida (ver [`../../DAEMON-MODE.md`](../../DAEMON-MODE.md)).

**`status` (handler de estado):** `GET /health 500ms` + JSON 800ms → `status_body(true, engine, warm)` o `status_body(false)` (`stopped`) con `schema_version="3"` (fixture `tests/golden/cli_daemon_status.json`). Solo probe en display (contrato intacto); el `stopped` por probe incluye en `classify_residual` la búsqueda del residente por 8766 antes de declarar vía libre. Esto es la detección de residual *al arrancar*; es un mecanismo distinto de la salud observada *por petición* dentro de una sesión ya viva que verifica la salud del residente antes de reutilizarlo en la síntesis TTS — no debe leerse como que esa verificación ya estaba resuelta por esta vía.

## Códigos de error

Los fallos del arranque se traducen a código en un único punto, compartido por `start`, `restart` y `serve`, así que la misma causa da el mismo código en los tres. Con `--json` el objeto de error lleva `reason` y `error`.

| `reason` | Exit | Causa | Comandos |
|---|---|---|---|
| `model_missing` | 4 | No hay modelo TTS provisionado | `start`, `restart` |
| `port_in_use` | 6 | El puerto del daemon está en uso o reservado por otro proceso. Remedio: liberar el puerto o arrancar en otro con `AVI_DAEMON_PORT=<puerto>` (`0` = puerto efímero) | `start`, `restart`, `serve` |
| `voice_not_found` | 3 | La voz de `--warm-voice` no existe | `start`, `serve` |
| `daemon_error` | 1 | Fallo al lanzar el hijo, muerte del hijo durante el arranque (el mensaje incluye su estado de salida), fallo no clasificado previo a estar listo o fallo del servidor ya en marcha | `start`, `restart`, `serve` |
| `daemon_unreachable` | 5 | El hijo sigue vivo sin publicar su dirección al vencer el deadline, `/health` no responde tras publicarla, o el árbol sigue vivo tras la parada | `start`, `restart`, `stop` |

Tras un fallo de `start` o `restart` no quedan proceso hijo, `daemon.pid` ni `daemon.ready`.

## Foreground vs background

| Aspecto | `daemon start` | `daemon serve` |
|---|---|---|
| Proceso | Proceso hijo lanzado con `spawn_background` | Mismo proceso CLI (con Job `KILL_ON_JOB_CLOSE` en Windows: al morir el daemon el SO cierra el árbol, residente incluido) |
| PID | `data_dir()/daemon.pid` con `pid` y `resident_pid` (el handler además conserva el PID en memoria desde el spawn para la ventana sin pidfile) | No |
| `--json` | Sí (`started` tras arranque o reclamo / `already_running` solo si sano) | No |
| Warmup | background `spawn_blocking` | igual |
| Señales | Ctrl+C del CLI con limpieza acotada de 2 s (exit 130 preservado, con PID en memoria si aún no hay pidfile) | Ctrl+C/SIGTERM escuchados en `run_daemon_server` por la misma ruta que `POST /shutdown` (cobertura `serve` Unix sin pidfile ni auto-muerte) |

Supervisión configurable: `start`/`serve` con `--auto-restart` habilitan `run_supervised` (función de supervisión del daemon) con contador `retries`, backoff `500ms*2^retries` capado a 4s, hasta `max_retries` (default 3), y un **watchdog de supervisión** (`SUPERVISION_PROGRESS_MIN = 10s`, `SUPERVISION_STREAK_MAX = 3`): si el daemon sufre 3 caídas rápidas seguidas con vida menor a 10 s (sin progreso), aborta con error explícito evitando bucles de reinicio ciegos. Un fallo previo a que el daemon haya estado listo alguna vez (puerto ocupado, voz inexistente, fallo al cargar el estado) es de configuración: es terminal, se publica en el fichero ready y no se reintenta; la supervisión solo recupera caídas de un daemon que llegó a servir. Antes de cada reintento hay reclamo activo del árbol propio previo con deadline (5 s) y verificación (muerte + puerto libre en el log; el `Drop` previo ya mató el árbol preciso, sin kill global ni otra instancia); el reclamo activo matar-y-rearrancar ante otra instancia vive en `daemon start` (en Unix ante líder muerto por grupo con verificación por 8766), nunca en `serve`. Un apagado graceful vía `POST /shutdown` (`shutdown_notify`) no reintenta; solo los crashes reintentan. Sin `--auto-restart`, el daemon es `fail-stop`. No hay `--language` en `start`/`serve`: `language` es local a `translate`/`dub`, y el STT es la feature de compilación `native-stt`.
