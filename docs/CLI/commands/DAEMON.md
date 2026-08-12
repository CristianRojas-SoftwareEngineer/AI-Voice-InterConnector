## Recorrido

La investigación examinó la implementación completa del comando `daemon` explorando siete fuentes principales: el parser CLI (`cli.py:2708-2742`), el handler de despacho `cmd_daemon` (`cli.py:2296-2413`), el servidor FastAPI (`daemon/server.py`), los modelos del protocolo IPC (`daemon/protocol.py`), el gestor del ciclo de vida (`daemon/daemon.py`), el cliente IPC (`daemon/ipc.py`), y el entry point del daemon (`daemon/run.py`). También se consultaron los códigos de salida (`exit_codes.py`) y el `__init__.py` del paquete daemon. No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de la gestión del daemon:** El sistema sigue un patrón cliente-servidor sobre HTTP/localhost (127.0.0.1:8765). El CLI actúa como cliente IPC que gestiona el ciclo de vida (start/stop/restart/status) y como consumidor de los endpoints del daemon (synthesize/transcribe/voices). El daemon es un proceso FastAPI/uvicorn que mantiene los modelos de TTS en memoria entre invocaciones, eliminando la sobrecarga de carga en cada llamada.

**Implementación de cada subcomando:** Los 5 subcomandos (`start`, `stop`, `restart`, `status`, `serve`) comparten un único handler `cmd_daemon` que despacha por `args.action`. Los primeros 4 delegan en `DaemonManager` (que ejecuta subprocessos y usa IPC); `serve` es la excepción: ejecuta el servidor en primer plano sin subproceso, reutilizando la misma función `serve()` de `daemon/run.py`.

**Arquitectura del daemon:** El daemon es una aplicación FastAPI con estado inyectado (`DaemonState` vía `app.state`), 6 endpoints REST, y un protocolo NDJSON para streaming de progreso de síntesis. Los modelos se cargan perezosamente por idioma y se cachean en el registro `DaemonState.engines`. La traducción y transcripción también se cargan bajo demanda (lazy imports).

---

## Hallazgos por tema

### Definición CLI

El parser define `daemon` como subcomando de nivel superior con 5 subacciones (`cli.py:2708-2742`):

| Subcomando | Parámetros | Descripción |
|---|---|---|
| `daemon start` | `--autorestart`, `--max-retries`, `--language {es-latam,en,all}`, `--with-stt`, `--json` | Inicia daemon en background |
| `daemon stop` | `--json` | Detiene el daemon |
| `daemon restart` | `--json` | Reinicia el daemon |
| `daemon status` | `--json` | Muestra estado del daemon |
| `daemon serve` | `--auto-restart`, `--max-retries`, `--language {es-latam,en,all}`, `--with-stt` | Ejecuta servidor en foreground |

Nota: `start` usa `--autorestart` (sin guion) mientras `serve` usa `--auto-restart` (con guion). Todos los subcomandos excepto `serve` soportan `--json` para salida machine-readable.

### Despacho del handler

`cmd_daemon` (`cli.py:2296-2413`) despacha por `args.action`:

```
cmd_daemon(args)
    ├── action == "serve"   → _require_models_cached_for_daemon() → daemon.run.serve()
    ├── action == "start"   → _require_models_cached_for_daemon() → DaemonManager.start()
    ├── action == "stop"    → DaemonManager.stop()
    ├── action == "restart" → DaemonManager.restart()
    └── action == "status"  → DaemonManager.status()
```

Los 4 primeros modos comparten `DaemonManager()` (`cli.py:2316-2318`). `serve` se bifurca temprano (`cli.py:2298-2314`) porque ejecuta el servidor en el proceso actual sin subproceso.

### Arquitectura del daemon

```
┌─────────────────────────────────────────────────────────────────┐
│  CLI (ai-voice-interconnector)                                              │
│                                                                 │
│  cmd_daemon ──► DaemonManager ──► subprocess.Popen ──► daemon   │
│                   │  (IPC HTTP)                    │            │
│                   ▼                                ▼            │
│              DaemonIPCClient ◄────────────► FastAPI app         │
│              (requests HTTP)                (uvicorn)           │
│                                                                 │
│  ┌──────────────────┐    ┌──────────────────────────────────┐   │
│  │ daemon/daemon.py │    │ daemon/server.py                  │   │
│  │ - start (spawn)  │    │ - DaemonState (engines registry)  │   │
│  │ - stop  (HTTP    │    │ - /health                         │   │
│  │   /shutdown +    │    │ - /synthesize (NDJSON stream)     │   │
│  │   kill fallback) │    │ - /transcribe                     │   │
│  │ - restart        │    │ - /voices                         │   │
│  │ - status         │    │ - /voices/precompute              │   │
│  │ - PID/lock mgmt  │    │ - /shutdown                       │   │
│  └──────────────────┘    └──────────────────────────────────┘   │
│                                                                 │
│  daemon/protocol.py ←── modelos Pydantic compartidos            │
│  daemon/run.py      ←── composition root + serve loop           │
│  daemon/ipc.py      ←── DaemonIPCClient (consumidor HTTP)       │
└─────────────────────────────────────────────────────────────────┘
```

### Endpoints del daemon

| Endpoint | Método | Modelo request | Modelo response | Descripción |
|---|---|---|---|---|
| `/health` | GET | — | `HealthResponse` | Estado, modelos cargados, uptime, versión |
| `/synthesize` | POST | `SynthesizeRequest` | NDJSON stream | Síntesis con progreso en tiempo real |
| `/transcribe` | POST | `TranscribeRequest` | `TranscribeResponse` | Transcripción vía faster-whisper |
| `/voices` | GET | — | `VoicesResponse` | Lista de voces registradas |
| `/voices/precompute` | POST | `PrecomputeVoiceRequest` | `PrecomputeVoiceResponse` | Precomputa conditionals de una voz |
| `/shutdown` | POST | — | `{"status": "shutting_down"}` | Apagado graceful del daemon |

(`/synthesize` es síncrono — `def` en `server.py:203` — FastAPI lo despacha a su threadpool, evitando bloquear el event loop.)

### Protocolo IPC

Los modelos Pydantic (`daemon/protocol.py`) definen el contrato:

- **`SynthesizeRequest`** (`protocol.py:62-92`): `text`, `voice`, `language`, `source_language`, `exaggeration`, `cfg_weight`, `temperature`. El `model_validator` resuelve `source_language=None` → `language` (sin traducción).
- **`TranscribeRequest`** (`protocol.py:95-109`): `audio_b64` (PCM int16, max 12.8 MB), `source_language`.
- **`HealthResponse`** (`protocol.py:151-163`): `status`, `model_loaded` (dict por idioma), `uptime_seconds`, `version`.
- **Stream de `/synthesize`**: N × `ProgressEvent` → 1 × `ResultEvent` (WAV base64 + timings) o 1 × `ErrorEvent`.
- **`ProtocolModel`** (`protocol.py:36-59`): base con `extra="ignore"` y `schema_version="3"` para compatibilidad hacia atrás.

Límites: `MAX_TEXT_LENGTH = 5000` (`protocol.py:22`), `MAX_AUDIO_BYTES = 12_800_000` (`protocol.py:28`), `MAX_VOICE_NAME_LENGTH = 255` (`protocol.py:33`).

### Gestión del ciclo de vida

#### `start` (`daemon.py:39-153`)

1. **Idempotencia**: si `is_running()` es True, retorna True sin hacer nada (`daemon.py:67-69`).
2. **Lock atómico**: `_acquire_start_lock()` crea el pidfile con `os.open(O_CREAT|O_EXCL)` para serializar `start` concurrentes (`daemon.py:93-95`).
3. **Construcción del comando**:
   - Modo congelado: `[sys.executable, "daemon", "serve"]` (`daemon.py:73-74`)
   - Modo fuente: `[sys.executable, "-m", "ai_voice_interconnector.daemon.run"]` (`daemon.py:76-79`)
4. **Lanzamiento del subproceso**:
   - Windows: `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP` (`daemon.py:119-128`)
   - Unix: `start_new_session=True` (`daemon.py:130-136`)
5. **Persistencia del PID**: `_write_pid(proc.pid)` en el pidfile (`daemon.py:143`).
6. **Espera de readiness**: `_wait_for_ready()` sondea `is_running()` cada 1s con timeout de 120s (`daemon.py:265-278`).

#### `stop` (`daemon.py:155-226`)

1. Si no está corriendo, verifica el puerto (`_get_pid_from_port`) y el pidfile.
2. **Cierre graceful**: `POST /shutdown` → uvicorn señala `should_exit` → cierre ordenado (`daemon.py:203-215`).
3. **Kill de seguridad**: si sigue corriendo tras 0.5s, `_kill_pid()` con `psutil` → `terminate()` → `kill()` (`daemon.py:221-224`).
4. **Validación de ownership**: `_is_own_daemon_process()` verifica el cmdline antes de matar para no terminar procesos ajenos (`daemon.py:332-346`).

#### `restart` (`daemon.py:228-234`)

Secuencial: `stop()` → `time.sleep(1)` → `start()`. El sleep de 1s permite que el puerto se libere.

#### `status` (`daemon.py:236-254`)

Consulta `GET /health` y devuelve dict con `running`, `status`, `model_loaded`, `uptime_seconds`.

### Modo foreground vs background

| Aspecto | `daemon start` | `daemon serve` |
|---|---|---|
| Proceso | Subproceso separado (Popen) | Mismo proceso del CLI |
| Persistencia | Sigue al cerrar la terminal | Muere al cerrar la terminal |
| PID management | Sí (pidfile + lock) | No |
| Modo congelado | `sys.executable daemon serve` | Directo |
| `--json` output | Sí | No |
| `--autorestart`/`--auto-restart` | `--autorestart` (CLI) | `--auto-restart` (directo) |

### Lógica de autorestart y reintentos

Implementada en `daemon/run.py:94-233`:

```
while True:
    # Composition root: DaemonState fresco por iteración
    app.state.daemon = DaemonState(start_time=time.time())
    # Cargar modelos → iniciar uvicorn.Server → server.run()
    ...
    if auto_restart and max_retries > 0 and retries >= max_retries:
        break  # Límite alcanzado
    except:
        if not auto_restart:
            break  # Sin autorestart, salir
    retries += 1
    # Invalidar caché de engines para recarga real
    ChatterboxEngine._cache.pop(cache_key, None)
    time.sleep(1)
```

- **`max_retries=0`** (default): reintentos infinitos (`daemon/run.py:178`).
- **Cache invalidation**: antes de cada reinicio, se eliminan las instancias cacheadas de `ChatterboxEngine` para forzar recarga real (`daemon/run.py:224-231`).
- **Detección de puerto en uso**: `OSError` con `errno.EADDRINUSE` (POSIX) o `10048` (Windows) → `EXIT_STATE_CONFLICT` (`daemon/run.py:195-204`).

### Precarga de idiomas

El parámetro `--language` (`es-latam`, `en` o `all`) controla qué modelos se precargan en caliente:

- **`all`** (default): ambos motores `es-latam` y `en` se cargan al arrancar (`run.py:72,122-126`).
- **`es-latam`** o **`en`**: solo el motor del idioma especificado.
- El resto se carga perezosamente en el primer `POST /synthesize` que lo solicite (`server.py:229-241`).
- **Traducción**: si `language` es `en` o `all`, el par opus-mt es↔en se precarga (`run.py:136-153`).
- **Transcripción**: `--with-stt` precarga faster-whisper-small (`run.py:161-166`); requiere que `setup --with-stt` ya haya provisionado el modelo en disco.

### Control de concurrencia en síntesis

`server.py:192-199`:

- **`_synthesis_lock`** (`threading.Lock`): serializa la síntesis porque `engine.synthesize()` muta estado global del modelo (`tts.conds`).
- **`_admission_semaphore`** (`BoundedSemaphore(4)`): máximo 4 peticiones en vuelo (1 activa + 3 en espera). La N+1 se rechaza con HTTP 503 antes de crear thread.

### Cancelación cooperativa

`server.py:258-364`: el stream NDJSON usa un patrón productor/consumidor con `threading.Event` como señal de cancelación. Si el cliente cierra la conexión (`GeneratorExit`), se setea `cancel_event` y el worker consulta antes de cada operación costosa. La limpieza de memoria (`_clear_model_memory`) corre siempre en el `finally`.

### Manejo de errores

| Escenario | Mecanismo | Código exit |
|---|---|---|
| Daemon ya corriendo (`start`) | Idempotente, retorna True | 0 |
| Daemon en arranque (`stop`) | Aviso + retorna False | 5 |
| Puerto en uso | `OSError(EADDRINUSE)` → `EXIT_STATE_CONFLICT` | 6 |
| Modelo no provisionado | `_require_models_cached_for_daemon()` | 4 |
| Modelo transcripción faltante | `TranscriptionModelMissingError` → 503 | 4 |
| Daemon inalcanzable | `DaemonIPCError` → `CliError(EXIT_DAEMON_UNREACHABLE)` | 5 |
| Síntesis fallida | `ErrorEvent` en stream NDJSON | 5 (IPC) |
| Audio base64 inválido | HTTP 400 | — |
| Demasiadas síntesis concurrentes | HTTP 503 (semáforo) | — |
| Puerto occupied al reiniciar | `EXIT_STATE_CONFLICT` | 6 |

### Seguridad del endpoint `/shutdown`

`server.py:478-482`: no lleva token ni confirmación explícita. El daemon bindea exclusivamente a `127.0.0.1`, por lo que solo un proceso con acceso local puede invocarlo. Se acepta ese riesgo residual en vez de añadir un secreto que el propio cliente IPC tendría que gestionar.

---

## Conclusiones

El comando `daemon` es el subsistema más complejo del CLI después de `speech`, con 5 subcomandos que orquestan un proceso FastAPI persistente. Su diseño se caracteriza por: (1) la separación limpia entre gestión del ciclo de vida (`daemon.py` — subprocessos, PID, locks) y operaciones del servidor (`server.py` — endpoints, estado, streaming); (2) el protocolo IPC tipado con Pydantic y streaming NDJSON que permite progreso en tiempo real; (3) la robustez en el manejo de carreras de arranque (lock atómico `O_CREAT|O_EXCL`), procesos huérfanos (validación por cmdline con psutil), y shutdown gracefully (señal `should_exit` + kill de seguridad); y (4) la flexibilidad de precarga de idiomas y modelos con lazy loading como fallback. El daemon sirve como base para todos los comandos de síntesis/transcripción del CLI, manteniendo los modelos en memoria y eliminando la sobrecarga de carga (~30-90s) en cada invocación.
