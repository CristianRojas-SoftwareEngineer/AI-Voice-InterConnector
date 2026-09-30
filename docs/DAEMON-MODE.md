# Modo daemon

El daemon nativo (Rust, Axum) mantiene los motores calientes entre invocaciones del CLI: sirve en cuanto enlaza el puerto y el peso de la voz `default` se precarga en segundo plano, sin bloquear el arranque; las peticiones que lleguen antes de que el motor esté caliente pagan carga fría.

## Tabla de contenidos

- [Arquitectura](#arquitectura)
- [Contrato HTTP](#contrato-http)
- [Comandos del daemon](#comandos-del-daemon)
- [Streaming NDJSON](#streaming-ndjson)
- [Resolución de binario y modelo](#resolución-de-binario-y-modelo)
- [Decisiones de diseño](#decisiones-de-diseño)

## Arquitectura

```
CLI (--json / texto)                    ai-voice-interconnector daemon serve
┌────────────────────┐   HTTP 127.0.0.1:8765 (defecto)   ┌──────────────────────────────┐
│ src/main.rs        │ ───────────────────────▶ │ crates/avi-daemon (Axum)     │
│ cliente reqwest    │ ◀─────────────────────── │ Qwen3TtsEngine residente     │
│ 3 modos: auto/     │   JSON / NDJSON          │ ParakeetEngine (sin VAD)     │
│ forzado/directo    │                          │ synthesis_lock (serializado) │
└────────────────────┘                          └──────────────────────────────┘
```

- **Servidor**: Axum sobre `127.0.0.1:8765` por defecto en loopback (`DAEMON_ADDR`, `src/main.rs`), con override por instancia vía `AVI_DAEMON_PORT` (`0` = efímero, el servidor publica `local_addr()`).
- **Warmup**: precarga de la voz elegida por `--warm-voice` (default `default`) en segundo plano (`spawn_blocking(warm_voice_engine)`), tras enlazar el puerto, cargar el estado y publicar el registro de éxito; no bloquea el arranque. El padre (`daemon start`/`restart`) consume el resultado del arranque en el fichero `--ready-file`, que el hijo escribe de forma atómica con el registro de éxito (`addr`/`warm`/`pid`) o con el de fallo (`error` con su causa, sin `addr`), y lo confirma por `/health`. Tras el warmup el servidor emite además `avi-daemon-ready warm=<warm|warm_failed> addr=<real>` en stderr como diagnóstico redundante. El warmup queda acotado por el arranque del residente más el presupuesto de síntesis del testigo; un vencimiento cancela el testigo y deja `warm_failed` sin matar el motor. Un warmup fallido no derriba el daemon: sigue sirviendo (una `--warm-voice` inexistente sí aborta el arranque antes de enlazar el puerto y de cargar los modelos, con exit 3 `voice_not_found`). El residente TTS es de una sola voz: clonar por daemon recalienta la voz nueva (warm-on-clone).
- **Serialización**: `synthesis_lock` — una síntesis a la vez; el resto espera.
- **STT**: `ParakeetEngine` (Parakeet TDT 0.6B v3 int8) transcribe en una sola pasada, sin segmentación VAD (RTF lineal ~0.11).

## Contrato HTTP

| Ruta | Método | Función |
|---|---|---|
| `/health` | GET | `status:"ready"` + `schema_version` (informativa, el cliente no la comprueba) + estado `warm` (`warming`/`warm`/`warm_failed`, con `warm_error` cuando falla), que refleja el warmup en curso o la última síntesis completada (no certifica síntesis futura, ver salud observada por petición abajo) |
| `/synthesize` | POST | Síntesis con progreso streaming NDJSON, evento final `result` (`audio_b64`, WAV 24 kHz) |
| `/transcribe` | POST | Transcripción PCM int16 base64 (`audio_b64`), una sola pasada sin VAD (feature `native-stt`); los errores responden 400 (`usage_error`, `invalid_audio`) o 500 (`transcription_failed`) |
| `/translate` | POST | Traducción CT2 residente (feature `native-translation`) |
| `/voices/clone` | POST | Clonado con streaming NDJSON y warm-on-clone (`{name, speech, precomputed:true}` = precarga en caliente iniciada; sin endpoint `precompute` separado) |
| `/dub` | POST | Pipeline transcribe→translate→synthesize con streaming NDJSON y latidos |
| `/shutdown` | POST | Apagado limpio (misma ruta que Ctrl+C/SIGTERM: kill preciso del residente por PID registrado + `notify_one()`) |

Son 7 rutas públicas (podados `GET /voices` y `POST /voices/precompute`; sin legado, ver `docs/CLI/commands/DAEMON.md`).

No hay handshake de versión: el cliente no comprueba la `schema_version` del daemon (hoy `"4"`) antes de usarlo, porque el daemon es el mismo ejecutable relanzado como `daemon serve`.

Readiness (`status:"ready"`) y warm son estados distintos: readiness es inmediato en cuanto el puerto está enlazado y el motor construido; warm indica si el precalentamiento en segundo plano ya terminó. `warm` lo fijan los warmups (de arranque y de clonado) y cada síntesis completada, que lo devuelve a `warm`; no refleja una degradación del residente posterior a la última síntesis y no certifica que una síntesis futura vaya a completarse. La salud efectiva de síntesis se observa por petición — antes de reutilizar el residente, el daemon ejecuta un healthcheck real (`synthesize_via_resident`, `crates/avi-tts/src/lib.rs`) y rearranca uno fresco si está degradado.

## Comandos del daemon

```bash
ai-voice-interconnector daemon start     # revalida el residual (sano → already_running; degradado → reclama el árbol y rearranca con started —incluido Stopped con residente vivo por resident_pid—; con --auto-restart los reintentos parten de reclamo activo del árbol propio previo con deadline y verificación —crash vivo con log pendiente de CI/entorno rápido, runtime diferido—; un puerto ocupado sale al instante con exit 6 port_in_use y --auto-restart no reintenta fallos previos a estar listo)
ai-voice-interconnector daemon serve     # primer plano (escucha Ctrl+C/SIGTERM por la misma ruta que POST /shutdown)
ai-voice-interconnector daemon status    # GET /health → running/stopped, además del estado `warm` para diagnóstico (el stopped por probe incluye en el arranque la detección del residente por resident_pid vivo)
ai-voice-interconnector daemon stop      # parada unificada con deadline global de 8 s (graceful + árbol preciso + verificación por resident_pid muerto, con barrido por imagen qwen_tts como último recurso sin PID; borra daemon.pid solo tras muerte verificada)
ai-voice-interconnector daemon restart   # parada unificada + arranque fresco (presupuesto 12 s)
```

Cierre: Ctrl+C ejecuta limpieza acotada de 2 s y sale con 130 preservado (con reclamo sin pidfile vía PID en memoria en la ventana spawn→write, tanto en `start` como en `restart`); `serve` en Windows corre bajo Job `KILL_ON_JOB_CLOSE` (al morir el daemon el SO cierra el árbol, residente incluido) y en Unix cierra por la misma ruta que `POST /shutdown` sin pidfile ni auto-muerte; en Unix el reclamo ante líder muerto mata además por grupo con verificación por resident_pid sin viveza (y barrido por imagen qwen_tts como último recurso cuando no hay PID registrado) (runtime diferido a CI, verificación pendiente en CI); `stop`/`restart` comparten el ayudante único `lifecycle::daemon_stop::stop` y `stop` falla con exit 5 sin borrar la pista si el árbol sigue vivo.

Despacho desde el CLI: `--daemon` fuerza IPC (exit 5 si no responde), `--no-daemon` fuerza proceso local, sin flags autodetecta.

## Streaming NDJSON

Las operaciones de síntesis (`POST /synthesize`), clonado (`POST /voices/clone`)
y doblaje (`POST /dub`) responden con `Content-Type: application/x-ndjson`:
1. Evento `started` inmediato tras validar parámetros y antes del trabajo pesado.
2. Latidos periódicos `{"event":"heartbeat"}` cada 500 ms emitidos mientras la inferencia en segundo plano continúa (y eventos de `progress`).
3. Evento final `{"event":"result", ...}` con la carga útil en su formato contractual (`audio_b64` para síntesis y dub; `name`, `speech`, `precomputed:true` para clonado) o `{"event":"error", ...}` ante fallos.

El cliente del CLI consume el stream con un timeout de inactividad entre latidos de 1500 ms y un deadline failsafe de 240 s. Si el cliente se desconecta, el servidor deja de emitir y descarta el resultado; en la síntesis cancela además el trabajo del motor TTS, que queda libre para la siguiente petición. La transcripción, la traducción y el clonado terminan en segundo plano y su resultado se descarta.

## Resolución de binario y modelo

El motor `Qwen3-TTS` resuelve su binario y pesos en este orden:

1. `QWEN3_TTS_BIN` / `QWEN3_TTS_MODEL_DIR` / `QWEN3_TTS_BASE_MODEL_DIR` (override absoluto)
2. `<exe_dir>/vendor/qwen3-tts/qwen_tts(.exe)` y `<exe_dir>/vendor/qwen3-tts/qwen3-tts-0.6b{,-base}` (junto al `current_exe` instalado)
3. `<cwd>/vendor/qwen3-tts/...` (desarrollo desde la raíz del repo)
4. `PATH` (`qwen_tts`) y snapshot HF (`ModelStore::model_snapshot_path`) como último fallback

Este orden garantiza que `daemon start` calienta desde cualquier `CWD` sin necesidad de `QWEN3_TTS_BIN` cuando se usa el binario instalado.

## Decisiones de diseño

- **Transporte HTTP (no stdio)**: mismo contrato que el canal Python previo; clientes externos no notan el cambio.
- **Captura siempre de cliente**: el daemon recibe PCM base64, nunca rutas ni dispositivos.
- **Sin multi-instancia soportada por el cliente**: el puerto por defecto es fijo y el cliente CLI aún apunta a él; correr dos daemons con puertos efímeros exige el descubrimiento por el cliente, cuya migración a instancia aislada queda diferida.
- **Motores residentes**: el TTS habla además con su propio servidor Qwen3-TTS (`127.0.0.1:8766`) gestionado por `avi-tts`. Ese puerto es el canal de servicio real (`DEFAULT_PORT`/`QWEN3_TTS_PORT`), no la identidad del proceso. Su PID se persiste en `daemon.pid` (`resident_pid` plano), y la parada, el reclamo y el reaper liquidan su proceso por su **identidad estable**: el `resident_pid` registrado (kill y verificación por viveza de PID) y, como faro independiente del pidfile cuando este se ha perdido, un barrido por imagen `qwen_tts` (`sweep_resident_by_image`, seguro porque el residente tiene imagen propia). No se descubre ni se verifica el cierre sondeando el puerto 8766.

Ver también [docs/DESIGN.md](DESIGN.md) y el contrato normativo [docs/CLI/CONTRACT.md](CLI/CONTRACT.md).
