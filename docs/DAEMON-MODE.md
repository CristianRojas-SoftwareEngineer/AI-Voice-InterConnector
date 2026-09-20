# Daemon Mode

El daemon nativo (Rust, Axum) mantiene los motores calientes entre invocaciones del CLI: sirve en cuanto enlaza el puerto y el peso de la voz `default` se precarga en segundo plano, sin bloquear el arranque; las peticiones que lleguen antes de que el motor esté caliente pagan carga fría.

## Tabla de contenidos

- [Arquitectura](#arquitectura)
- [Contrato HTTP](#contrato-http)
- [Comandos del Daemon](#comandos-del-daemon)
- [Streaming NDJSON](#streaming-ndjson)
- [Decisiones de Diseño](#decisiones-de-diseño)

## Arquitectura

```
CLI (--json / texto)                    ai-voice-interconnector daemon serve
┌────────────────────┐   HTTP 127.0.0.1:8765   ┌──────────────────────────────┐
│ src/main.rs        │ ───────────────────────▶ │ crates/avi-daemon (Axum)     │
│ cliente reqwest    │ ◀─────────────────────── │ Qwen3TtsEngine residente     │
│ 3 modos: auto/     │   JSON / NDJSON          │ Ct2SttEngine + VAD Silero    │
│ forzado/directo    │                          │ synthesis_lock (serializado) │
└────────────────────┘                          └──────────────────────────────┘
```

- **Servidor**: Axum sobre `127.0.0.1:8765` (loopback, puerto fijo por diseño).
- **Warmup**: precarga de la voz elegida por `--warm-voice` (default `default`) en segundo plano (`spawn_blocking(precalentar_voz)`), tras el `bind` del puerto; no bloquea el arranque y el readiness es inmediato al enlazar. Un warmup fallido no derriba el daemon: sigue sirviendo (una `--warm-voice` inexistente sí aborta el arranque, fail-fast antes del bind). El residente TTS es de una sola voz: clonar por daemon recalienta la voz nueva (warm-on-clone).
- **Serialización**: `synthesis_lock` — una síntesis a la vez; el resto espera.
- **STT**: audio largo (>15 s) se segmenta con VAD Silero antes de transcribir.

## Contrato HTTP

| Ruta | Método | Función |
|---|---|---|
| `/health` | GET | `status:"ready"` + handshake de `schema_version` + estado de warmup `warm` (`warming`/`warm`/`warm_failed`, con `warm_error` cuando falla; no certifica síntesis futura, ver salud observada por petición abajo) |
| `/synthesize` | POST | Síntesis con progreso streaming NDJSON, evento final `result` (`audio_b64`, WAV 24 kHz) |
| `/transcribe` | POST | Transcripción PCM int16 base64 (`audio_b64`), VAD para clips largos (feature `native-stt`) |
| `/translate` | POST | Traducción CT2 residente (feature `native-translation`) |
| `/voices/clone` | POST | Clonado con warm-on-clone (`{name, speech, precomputed:true}` = precarga en caliente iniciada; sin endpoint `precompute` separado) |
| `/dub` | POST | Pipeline transcribe→translate→synthesize |
| `/shutdown` | POST | Apagado limpio (misma ruta que Ctrl+C/SIGTERM: kill preciso del residente + `notify_one()`) |

Son 7 rutas públicas (podados `GET /voices` y `POST /voices/precompute`; sin legado, ver `docs/CLI/commands/DAEMON.md`).

El handshake es estricto: un daemon de otra `schema_version` se trata como no utilizable.

Readiness (`status:"ready"`) y warm son estados distintos: readiness es inmediato en cuanto el puerto está enlazado y el motor construido; warm indica si el precalentamiento en segundo plano ya terminó. `warm` es append-only (se fija una sola vez en el warmup de arranque y no refleja ninguna degradación posterior del residente): no certifica que una síntesis futura vaya a completarse. La salud efectiva de síntesis se observa por petición — antes de reutilizar el residente, el daemon ejecuta un healthcheck real (`synthesize_via_residente`, `crates/avi-tts/src/lib.rs`) y rearranca uno fresco si está degradado.

## Comandos del Daemon

```bash
ai-voice-interconnector daemon start     # revalida el residual (sano → already_running; degradado → reclama el árbol y rearranca con started —incluido Parado con residente vivo por 8766, D-04—; con --auto-restart los reintentos parten de reclamo activo del árbol propio previo con deadline y verificación —crash vivo con log pendiente de CI/entorno rápido, ver H-15—)
ai-voice-interconnector daemon serve     # primer plano (escucha Ctrl+C/SIGTERM por la misma ruta que POST /shutdown)
ai-voice-interconnector daemon status    # GET /health → running/stopped, además del estado `warm` para diagnóstico (el stopped por probe incluye en el arranque la búsqueda del residente por 8766, D-04)
ai-voice-interconnector daemon stop      # parada unificada con deadline global de 8 s (graceful + árbol preciso + verificación con 8766; borra daemon.pid solo tras muerte verificada)
ai-voice-interconnector daemon restart   # parada unificada + arranque fresco (presupuesto 12 s)
```

Cierre H-01 + D-01 + D-02: Ctrl+C ejecuta limpieza acotada de 2 s y sale con 130 preservado (con reclamo sin pidfile vía PID en memoria en la ventana spawn→write); `serve` en Windows corre bajo Job `KILL_ON_JOB_CLOSE` (al morir el daemon el SO cierra el árbol, residente incluido) y en Unix cierra por la misma ruta que `POST /shutdown` sin pidfile ni auto-muerte; en Unix el reclamo ante líder muerto mata además por grupo con verificación por 8766 cerrado + PID sin viveza (runtime diferido a CI, ver H-15); `stop`/`restart` comparten el ayudante único `stop_daemon_and_resident` y `stop` falla con exit 5 sin borrar la pista si el árbol sigue vivo.

Despacho desde el CLI: `--daemon` fuerza IPC (exit 5 si no responde), `--no-daemon` fuerza proceso local, sin flags autodetecta.

## Streaming NDJSON

`POST /synthesize` responde `Content-Type: application/x-ndjson`: eventos de
progreso línea a línea y un objeto final `{"type":"result","audio_b64":…}` con el
WAV en base64. El cliente del CLI reconstruye el WAV y lo persiste/reproduce según
los flags.

## Resolución de binario y modelo

El motor `Qwen3-TTS` resuelve su binario y pesos en este orden (decisión e1):

1. `QWEN3_TTS_BIN` / `QWEN3_TTS_MODEL_DIR` / `QWEN3_TTS_BASE_MODEL_DIR` (override absoluto)
2. `<exe_dir>/vendor/qwen3-tts/qwen_tts(.exe)` y `<exe_dir>/vendor/qwen3-tts/qwen3-tts-0.6b{,-base}` (junto al `current_exe` instalado)
3. `<cwd>/vendor/qwen3-tts/...` (desarrollo desde la raíz del repo)
4. `PATH` (`qwen_tts`) y snapshot HF (`ModelStore::model_snapshot_path`) como último fallback

Este orden garantiza que `daemon start` calienta desde cualquier `CWD` sin necesidad de `QWEN3_TTS_BIN` cuando se usa el binario instalado.

## Decisiones de Diseño

- **Transporte HTTP (no stdio)**: mismo contrato que el canal Python previo; clientes externos no notan el cambio.
- **Captura siempre de cliente**: el daemon recibe PCM base64, nunca rutas ni dispositivos.
- **Sin multi-instancia**: puerto fijo; correr dos daemons no está soportado.
- **Motores residentes**: el TTS habla además con su propio servidor Qwen3-TTS (`127.0.0.1:8766`) gestionado por `avi-tts` (D-04: ante `Parado` con 8766 abierto se reclama su árbol con preciso-primero e imagen solo como último recurso verificado).

Ver también [docs/DESIGN.md](DESIGN.md) y el contrato normativo [docs/CLI/CONTRACT.md](CLI/CONTRACT.md).
