# Comando `daemon` — ciclo de vida del daemon nativo

El daemon es un servidor `Axum` (`crates/avi-daemon`) que mantiene los modelos Qwen3-TTS, Parakeet TDT v3 y CT2 `es↔en` en memoria, evitando la carga en cada invocación (~15–30 s). El CLI actúa como cliente HTTP sobre `127.0.0.1:8765` (`src/main.rs:30` `DAEMON_ADDR`).

## Definición CLI

`src/main.rs:300` `enum DaemonCommands`:

| Subcomando | Parámetros | Descripción |
|---|---|---|
| `daemon start` | `--json` `--auto-restart` `--max-retries` (default 3) | Revalida el residual (PID vivo + probe): sano → `already_running`, degradado → reclama el árbol y rearranca con `started`; si no, lanza en background (`spawn_background`, `await_daemon_ready`); con `--auto-restart` los reintentos parten de reclamo activo del árbol propio previo con deadline y verificación |
| `daemon stop` | `--json` | Parada unificada con deadline global de 8 s (`stop_daemon_and_resident`: graceful + árbol preciso + verificación; borra `daemon.pid` solo tras muerte verificada, exit 5 sin borrar pista si sigue vivo) |
| `daemon restart` | `--json` | Ayudante único de parada (sin doble techo ni kill duplicado) → `start` fresco con presupuesto de 12 s (sin flags de supervisión heredados) |
| `daemon status` | `--json` | `GET /health` → `running`/`stopped` + `warm` |
| `daemon serve` | `--auto-restart` `--max-retries` (default 3) | Ejecuta servidor en foreground (`run_supervised` con escucha de Ctrl+C/SIGTERM por la misma ruta que `POST /shutdown`) |

`start`/`serve` aceptan `--auto-restart`/`--max-retries`; `start`/`stop`/`restart`/`status` aceptan `--json` ( `serve` sin `--json`). `start`/`restart` exigen modelo provisionado (`require_model_provisioned`), `stop`/`status` no.

## Despacho del handler

`src/main.rs:1409` `handle_daemon(json_mode, action)`:

```
handle_daemon
 ├── Serve  → Job KILL_ON_JOB_CLOSE (Windows) → run_supervised(127.0.0.1:8765) (foreground, warmup background, Ctrl+C/SIGTERM por la misma ruta que POST /shutdown)
 ├── Start  → clasificar_residual (PID vivo + probe) → Sano: already_running | Degradado: reclamar árbol + rearrancar → started : spawn_background → await_daemon_ready → write pid
 ├── Stop   → stop_daemon_and_resident (deadline 8 s) → muerte verificada? → remove pid + shutdown_sent : exit 5 sin borrar pista
 ├── Restart→ stop_daemon_and_resident → spawn_background → await ready (presupuesto 12 s)
 └── Status → GET /health (500ms timeout) → running/stopped + warm/engine
```

`serve` no usa subproceso; los otros 4 usan `avi_daemon::spawn::spawn_background`. El handler Ctrl+C está instalado en `main` para todos los modos (`src/main.rs:355`): limpieza acotada de 2 s sobre el árbol del pidfile —o del PID en memoria en la ventana spawn→write sin pidfile (D-02)— y salida 130 preservada.

## Arquitectura del daemon

```
CLI (ai-voice-interconnector)
  ├── handle_daemon ──► spawn_background ──► daemon (Axum)
  │                       │  Stdio::null + CREATE_NO_HANDLE_INHERIT (Win) / setsid (Unix)
  │                       ▼
  │                  DaemonState ──► Qwen3TtsEngine (resident qwen_tts)
  │                       ├── warm: RwLock<WarmState> (Warming/Warm/Failed)
  │                       ├── synthesis_lock: Mutex<()>
  │                       ├── ct2_engine: Option<HashMap<String,Ct2TranslationEngine>> (native-translation)
  │                       └── shutdown_notify: Arc<Notify>
  └── DaemonIPCClient (reqwest) ◄──► Axum Router
```

`crates/avi-daemon/src/lib.rs:69` `DaemonState { synthesis_lock, voice_store, speech_store, tts_engine, stt_engine, ct2_engine, warm, shutdown_notify }`.
`ct2_engine: None` significa motor ausente o roto: también es `None` con `model.bin` huérfano (sin tokenizador) — el arranque lo registra con los ficheros faltantes (`ct2_archivos_faltantes`) sin derribar el servidor (`DaemonState::new`, `crates/avi-daemon/src/lib.rs:116-128`, gate `is_ct2_provisioned` == loader).
`crates/avi-daemon/src/lib.rs:1252` `run_daemon_server` bindea `TcpListener`, `spawn_blocking(warmup_tts)`, `with_graceful_shutdown` por la misma ruta desde `POST /shutdown` y desde Ctrl+C/SIGTERM (`tts_engine.shutdown()` preciso-primero + `notify_one()`). Cierre garantizado: Job `KILL_ON_JOB_CLOSE` en `Serve` (Windows), kill preciso del árbol por PID con verificación (`crates/avi-daemon/src/spawn.rs:84-168`) y reclamo matar-y-rearrancar en `start` (`src/main.rs:2110-2164`, en Unix ante líder muerto además por grupo con verificación por 8766 cerrado; runtime Unix diferido a CI, ver H-15).

## Endpoints

| Endpoint | Método | Request | Response | Descripción |
|---|---|---|---|---|
| `/health` | GET | — | `{status:"ready", warm, engine, warm_error?, ct2?, stt?}` + `schema_version="3"` | Readiness + warmup (7 rutas) |
| `/synthesize` | POST | `{text, voice}` | NDJSON `start → progress → result{audio_b64}` o `error` | Síntesis streaming 24 kHz |
| `/transcribe` | POST | `{audio_b64, source_language}` | `{text}` o `error` | Transcripción Parakeet (feature `native-stt`) |
| `/translate` | POST | `{text, from, to}` | `{translated, source, target}` o `error` | Traducción CT2 residente (feature `native-translation`) |
| `/voices/clone` | POST | `{name, audio_b64, timbre_b64?, force?}` | `{name, speech, precomputed:false}` o `error` | Clonar voz (audio base64) |
| `/dub` | POST | `{audio_b64, from, to, voice}` | `{status:"dubbed", text, translated, audio_b64}` o `error` | Pipeline transcribe→translate→synthesize |
| `/shutdown` | POST | — | `{status:"shutting_down"}` | `shutdown_handler`: mata el árbol preciso del residente por PID (kill por imagen solo como último recurso documentado) + `notify_one()` para cierre graceful sin `process::exit` |

7 rutas públicas (podadas `GET /voices` y `POST /voices/precompute`; sin legado). Prefijo `x-schema-version: 3` (`crates/avi-core/src/json_emitter.rs:5`).

## Protocolo

- `synthesize`: NDJSON `application/x-ndjson` con `schema_version`.
- `transcribe`: PCM `i16le 16kHz mono` base64 en `audio_b64`.
- `health_body` (`lib.rs:183`): `Warming → Warm → Failed(causa)`; `warm_error` solo si `Failed`. `GET /health` puede incluir `ct2`/`stt` aditivas `warm/warming/warm_failed` cuando residentes, sin bump `schema_version`.
- `translate`/`dub` (etapa de traducción) exigen el derivado sano vía `is_ct2_provisioned` (`model.bin` más `tokenizer.json` o `source.spm`+`target.spm`); sin él responden `model_missing` (exit 4 en CLI) con los ficheros faltantes.

## Gestión del ciclo de vida

**`start` (`src/main.rs:1431`):** revalida el residual por PID vivo + probe (`clasificar_residual`, `src/main.rs:2110`): sano (probe + PID vivo) → `already_running` con salida 0; degradado (probe y PID discrepan: colgado, pista rancia o sin pista —incluido `Parado` con residente vivo por 8766, D-04—) → reclama el árbol preciso (`reclamar_residual_degradado`, `src/main.rs:2135`, en Unix ante líder muerto además por grupo con verificación por 8766 cerrado + PID sin viveza —runtime Unix diferido a CI, ver H-15—; ante residente-solo con preciso-primero e imagen del residente solo como último recurso verificado, nunca imagen del daemon) y rearranca desde cero con salida 0 y payload `started` (nunca `already_running` ciego). Si no hay residual, `spawn_background` (`crates/avi-daemon/src/spawn.rs:21`) con `Stdio::null` + `CREATE_NO_WINDOW|CREATE_NEW_PROCESS_GROUP` (Win) / `setsid` (Unix) + `CREATE_NO_HANDLE_INHERIT` (`0x02000000`) para no heredar `pipe` de `cargo test`. Luego `await_daemon_ready` (`10s deadline, 250ms poll`) y `write_daemon_pid` (`data_dir()/daemon.pid`).

**`stop` (`src/main.rs:1496`):** parada unificada `stop_daemon_and_resident` (`src/main.rs:2187`) con deadline global de 8 s (`STOP_DEADLINE_GLOBAL`): graceful (`POST /shutdown` 1,5 s + espera de `/health` down hasta 3 s) si responde, árbol preciso por PID (`taskkill /F /T /PID` en Windows, `kill -9` al grupo en Unix —ante líder muerto el reclamo de `start` reutiliza esa vía de grupo con verificación por 8766, runtime Unix diferido a CI, ver H-15—, con guarda anti-auto-muerte) cuando sigue vivo, más reclamo del residente-solo sin PID del daemon con preciso-primero e imagen del residente solo como último recurso verificado (D-04, nunca imagen del daemon), y verificación a nivel de sistema (probe + `pid_vivo` + 8766 cerrado). El pidfile solo se borra tras muerte verificada; si el árbol sigue vivo se conserva la pista y se falla con exit 5. Sin kill por imagen para el daemon (comparte imagen con el CLI); el residente se reclama en `avi-tts` con preciso primero e imagen solo como último recurso documentado.

**`restart` (`src/main.rs:1525`):** parada unificada sobre el ayudante único (sin doble techo `timeout(5s, wait_health_down(5s))` ni kill por PID duplicado) → `spawn_background` fresco → `await ready` acotado al restante del presupuesto de 12 s (nunca más de 10 s) → `write pid` con payload `restarted` (sin `/restart` dedicado).

**`status` (`src/main.rs:1573`):** `GET /health 500ms` + JSON 800ms → `status_body(true, engine, warm)` o `status_body(false)` (`stopped`) con `schema_version="3"` (fixture `tests/golden/cli_daemon_status.json`). Solo probe en display (contrato intacto); D-04: el `stopped` por probe incluye en `clasificar_residual` la búsqueda del residente por 8766 antes de declarar vía libre (ver H-05).

## Foreground vs background

| Aspecto | `daemon start` | `daemon serve` |
|---|---|---|
| Proceso | `Popen` separado | Mismo proceso CLI (con Job `KILL_ON_JOB_CLOSE` en Windows: al morir el daemon el SO cierra el árbol, residente incluido) |
| PID | `data_dir()/daemon.pid` (el handler además conserva el PID en memoria desde el spawn para la ventana sin pidfile, D-02) | No |
| `--json` | Sí (`started` tras arranque o reclamo / `already_running` solo si sano) | No |
| Warmup | background `spawn_blocking` | igual |
| Señales | Ctrl+C del CLI con limpieza acotada de 2 s (exit 130 preservado, con PID en memoria si aún no hay pidfile) | Ctrl+C/SIGTERM escuchados en `run_daemon_server` por la misma ruta que `POST /shutdown` (cobertura `serve` Unix sin pidfile ni auto-muerte, D-02) |

Supervisión configurable: `start`/`serve` con `--auto-restart` habilitan `run_supervised` (`crates/avi-daemon/src/lib.rs:1342`) con contador `retries` y backoff `500ms*2^retries` capado a 4s, hasta `max_retries` (default 3). Antes de cada reintento hay reclamo activo del árbol propio previo con deadline (5 s) y verificación (muerte + puerto libre en el log; el `Drop` previo ya mató el árbol preciso, sin kill global ni otra instancia); el reclamo activo matar-y-rearrancar ante otra instancia vive en `daemon start` (en Unix ante líder muerto por grupo con verificación por 8766, runtime diferido a CI; prueba de crash vivo con log pendiente de CI/entorno rápido, ver H-15), nunca en `serve`. Un apagado graceful vía `POST /shutdown` (`shutdown_notify`) no reintenta; solo los crashes reintentan. Sin `--auto-restart`, el daemon es `fail-stop`. No hay `--language/--with-stt` en `start`/`serve` — `language` es local a `translate`/`dub` y `with-stt` es feature de compilación `native-stt`.
