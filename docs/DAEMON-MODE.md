# Daemon Mode

El daemon nativo (Rust, Axum) mantiene los motores calientes entre invocaciones del CLI: el peso de la voz `default` se precarga al arranque y las síntesis posteriores evitan la carga fría del modelo.

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
- **Warmup**: precarga del preset `ryan` (voz `default`) al arrancar.
- **Serialización**: `synthesis_lock` — una síntesis a la vez; el resto espera.
- **STT**: audio largo (>15 s) se segmenta con VAD Silero antes de transcribir.

## Contrato HTTP

| Ruta | Método | Función |
|---|---|---|
| `/health` | GET | Estado + handshake de `schema_version` |
| `/synthesize` | POST | Síntesis con progreso streaming NDJSON, evento final `result` (`audio_b64`, WAV 24 kHz) |
| `/transcribe` | POST | Transcripción PCM int16 base64 (`audio_b64`), VAD para clips largos |
| `/voices` | GET | Voces registradas |
| `/voices/precompute` | POST | Clonado vía `clone_voice` |
| `/shutdown` | POST | Apagado limpio |

El handshake es estricto: un daemon de otra `schema_version` se trata como no utilizable.

## Comandos del Daemon

```bash
ai-voice-interconnector daemon start     # inicio en segundo plano (pendiente; usar serve)
ai-voice-interconnector daemon serve     # primer plano
ai-voice-interconnector daemon status    # GET /health → running/stopped
ai-voice-interconnector daemon stop      # POST /shutdown
ai-voice-interconnector daemon restart   # stop + aviso de rearme manual
```

Despacho desde el CLI: `--daemon` fuerza IPC (exit 5 si no responde), `--no-daemon` fuerza proceso local, sin flags autodetecta.

## Streaming NDJSON

`POST /synthesize` responde `Content-Type: application/x-ndjson`: eventos de
progreso línea a línea y un objeto final `{"type":"result","audio_b64":…}` con el
WAV en base64. El cliente del CLI reconstruye el WAV y lo persiste/reproduce según
los flags.

## Decisiones de Diseño

- **Transporte HTTP (no stdio)**: mismo contrato que el canal Python previo; clientes externos no notan el cambio.
- **Captura siempre de cliente**: el daemon recibe PCM base64, nunca rutas ni dispositivos.
- **Sin multi-instancia**: puerto fijo; correr dos daemons no está soportado.
- **Motores residentes**: el TTS habla además con su propio servidor Qwen3-TTS (`127.0.0.1:8766`) gestionado por `avi-tts`.

Ver también [docs/DESIGN.md](DESIGN.md) y el contrato normativo [docs/CLI/CONTRACT.md](CLI/CONTRACT.md).
