# Comando `daemon` — ciclo de vida del daemon nativo

El daemon es un servidor `Axum` (`crates/avi-daemon`) que mantiene los modelos Qwen3-TTS, Parakeet TDT v3 y CT2 `es↔en` en memoria, evitando la carga en cada invocación (~15–30 s). El CLI actúa como cliente HTTP sobre `127.0.0.1:8765` (`src/main.rs:30` `DAEMON_ADDR`).

## Definición CLI

`src/main.rs:257` `enum DaemonCommands`:

| Subcomando | Parámetros | Descripción |
|---|---|---|
| `daemon start` | `--json` `--auto-restart` `--max-retries` (default 3) | Lanza daemon en background (`spawn_background` con supervisión opcional, `await_daemon_ready`) |
| `daemon stop` | `--json` | `POST /shutdown` + borra `daemon.pid` |
| `daemon restart` | `--json` | `stop` → `wait_health_down 5s` → `start` (sin flags de supervisión) |
| `daemon status` | `--json` | `GET /health` → `running`/`stopped` + `warm` |
| `daemon serve` | `--auto-restart` `--max-retries` (default 3) | Ejecuta servidor en foreground (`run_supervised`) |

`start`/`serve` aceptan `--auto-restart`/`--max-retries`; `start`/`stop`/`restart`/`status` aceptan `--json` ( `serve` sin `--json`). `start`/`restart` exigen modelo provisionado (`require_model_provisioned`), `stop`/`status` no.

## Despacho del handler

`src/main.rs:1138` `handle_daemon(json_mode, action)`:

```
handle_daemon
 ├── Serve  → run_daemon_server(127.0.0.1:8765) (foreground, warmup background)
 ├── Start  → daemon_activo? → already_running : spawn_background → await_daemon_ready → write pid
 ├── Stop   → POST /shutdown → remove pid (idempotente)
 ├── Restart→ stop (shutdown+wait) → spawn_background → await ready
 └── Status → GET /health (500ms timeout) → running/stopped + warm/engine
```

`serve` no usa subproceso; los otros 4 usan `avi_daemon::spawn::spawn_background`.

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

`crates/avi-daemon/src/lib.rs:68` `DaemonState { synthesis_lock, voice_store, speech_store, tts_engine, stt_engine, ct2_engine, warm, shutdown_notify }`.
`crates/avi-daemon/src/lib.rs:614` `run_daemon_server` bindea `TcpListener`, `spawn_blocking(warmup_tts)` (`crates/avi-daemon/src/lib.rs:589`), `with_graceful_shutdown(notify)`.

## Endpoints

| Endpoint | Método | Request | Response | Descripción |
|---|---|---|---|---|
| `/health` | GET | — | `{status:"ready", warm, engine, warm_error?}` + `schema_version="3"` | Readiness + warmup |
| `/synthesize` | POST | `{text, voice}` | NDJSON `start → progress → result{audio_b64}` o `error` | Síntesis streaming 24 kHz |
| `/transcribe` | POST | `{audio_b64, source_language}` | `{text}` o `error` | Transcripción Parakeet (feature `native-stt`) |
| `/translate` | POST | `{text, from, to}` | `{translated, source, target}` o `error` | Traducción CT2 residente (feature `native-translation`) |
| `/voices` | GET | — | `{voices: [{name,is_factory}]}` | Listar voces |
| `/voices/precompute` | POST | `{name}` | `{precomputed, message}` | Precomputar `.qvoice` |
| `/voices/clone` | POST | `{name, audio_b64, timbre_b64?, force?}` | `{name, speech, precomputed:false}` o `error` | Clonar voz (audio base64) |
| `/dub` | POST | `{audio_b64, from, to, voice}` | `{status:"dubbed", text, translated, audio_b64}` o `error` | Pipeline transcribe→translate→synthesize |
| `/shutdown` | POST | — | `{status:"shutting_down"}` | `shutdown_notify` + `tts_engine.shutdown()` |

Prefijo `x-schema-version: 3` (`crates/avi-core/src/json_emitter.rs:5`).

## Protocolo

- `synthesize`: NDJSON `application/x-ndjson` con `schema_version`.
- `transcribe`: PCM `i16le 16kHz mono` base64 en `audio_b64`.
- `health_body` (`lib.rs:158`): `Warming → Warm → Failed(causa)`; `warm_error` solo si `Failed`.

## Gestión del ciclo de vida

**`start` (`src/main.rs:1151`):** idempotente si `daemon_activo` (GET /health ok) → `already_running`. Si no, `spawn_background` (`src/main.rs:1163` / `crates/avi-daemon/src/spawn.rs:21`) con `Stdio::null` + `CREATE_NO_WINDOW|CREATE_NEW_PROCESS_GROUP` (Win) / `setsid` (Unix) + `CREATE_NO_HANDLE_INHERIT` (`0x02000000`) para no heredar `pipe` de `cargo test`. Luego `await_daemon_ready` (`10s deadline, 250ms poll`) y `write_daemon_pid` (`data_dir()/daemon.pid`).

**`stop` (`src/main.rs:1181`):** `POST /shutdown` → `tts_engine.shutdown()` (mata `qwen_tts` por PID sin `Mutex`) + `notify_one()` para cierre graceful de `Axum` sin `process::exit`.

**`restart` (`src/main.rs:1213`):** `shutdown` → `wait_health_down 5s` → `start` (sin `/restart` dedicado).

**`status` (`src/main.rs:1246`):** `GET /health 500ms` → `status_body(true, engine, warm)` o `status_body(false)` (`stopped`) con `schema_version="3"` (fixture `tests/golden/cli_daemon_status.json`).

## Foreground vs background

| Aspecto | `daemon start` | `daemon serve` |
|---|---|---|
| Proceso | `Popen` separado | Mismo proceso CLI |
| PID | `data_dir()/daemon.pid` | No |
| `--json` | Sí (`started`/`already_running`) | No |
| Warmup | background `spawn_blocking` | igual |

Supervisión configurable: `start`/`serve` con `--auto-restart` habilitan `run_supervised` (`crates/avi-daemon/src/lib.rs:614`) con contador `retries` y backoff `500ms*2^retries` capado a 4s, hasta `max_retries` (default 3). Un apagado graceful vía `POST /shutdown` (`shutdown_notify`) no reintenta; solo los crashes reintentan. Sin `--auto-restart`, el daemon es `fail-stop`. No hay `--language/--with-stt` en `start`/`serve` — `language` es local a `translate`/`dub` y `with-stt` es feature de compilación `native-stt`.
