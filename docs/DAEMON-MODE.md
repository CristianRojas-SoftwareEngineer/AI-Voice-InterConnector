# Daemon Mode

El daemon mode mantiene el modelo de Chatterbox en memoria entre invocaciones del CLI, eliminando el overhead de carga (~15-30s) en cada llamada.

## Tabla de contenidos

- [Problema](#problema)
- [Solución](#solución)
- [Arquitectura](#arquitectura)
- [Comandos del Daemon](#comandos-del-daemon)
- [Cancelación cooperativa del cliente](#cancelación-cooperativa-del-cliente)
- [Parámetros Optimizados](#parámetros-optimizados)
- [Métricas de Rendimiento](#métricas-de-rendimiento)
- [Decisiones de Diseño](#decisiones-de-diseño)
- [Compatibilidad](#compatibilidad)

## Problema

Sin daemon, cada ejecución del CLI funciona así:

```
$ tts-sidecar speech synthesize --text "Hola" --label demo
→ Nuevo proceso Python
→ Importa engine.py
→ ChatterboxEngine.__init__() carga modelo (~5-8s)
→ Genera audio (~45s)
→ Persiste el WAV en data_root()/synthetic-speech/demo/<etiqueta>.wav
→ Proceso termina
→ Modelo en RAM se libera
```

**Problemas:**
1. El modelo se carga desde disco en cada invocación
2. `torch.compile` no persiste entre llamadas (overhead de ~30-60s)
3. El caché de clase en `ChatterboxEngine._cache` no se comparte entre procesos

## Solución

El daemon es un servidor HTTP persistente que mantiene el modelo cargado:

```
┌─────────────────────────────────────────────────────────────────────┐
│                          Cliente CLI                                 │
│          speech synthesize | speech say (3-mode dispatch)          │
└──────────────────────────────┬──────────────────────────────────────┘
                               │
                               │ ¿Daemon corriendo?
                               ▼
                    ┌──────────────────────┐
                    │  ¿Daemon corriendo?   │
                    └──────────┬───────────┘
                               │
              ┌────────────────┴────────────────┐
              │ NO                                 │ SÍ
              ▼                                    ▼
    ┌─────────────────┐                ┌─────────────────────────────┐
    │ Modo fallback   │                │  IPC (HTTP)                │
    │ (carga directa) │                │  127.0.0.1:8765           │
    └─────────────────┘                └──────────┬──────────────────┘
                                                  │
                                                  ▼
                                ┌───────────────────────────────────┐
                                │     tts-sidecar-daemon            │
                                │                                   │
                                │  - ChatterboxEngine (cacheado)    │
                                │  - torch.compile (aplicado)      │
                                │  - Puerto fijo 8765 (TCP)      │
                                └───────────────────────────────────┘
```

## Arquitectura

### Estructura de Archivos

```
src/tts_sidecar/
├── cli.py              # CLI con fallback a daemon
├── engine.py           # ChatterboxEngine
├── audio.py            # AudioPlayer
├── timing.py           # Instrumentation
└── daemon/
    ├── __init__.py
    ├── server.py       # Servidor FastAPI
    ├── daemon.py       # Gestor del ciclo de vida (start/stop/restart)
    ├── ipc.py          # Cliente HTTP para CLI → daemon
    ├── protocol.py     # Modelos Pydantic de request/response
    └── run.py          # Entry point: python -m tts_sidecar.daemon.run
```

### Protocolo de Comunicación

**Request** (CLI → Daemon):
```json
POST /synthesize
{
  "text": "Hola mundo",
  "voice": "nombre-de-voz-registrada",
  "language": "es-latam",
  "exaggeration": null,
  "cfg_weight": null,
  "temperature": null
}
```

El daemon sirve **varios modelos a la vez, uno por idioma** (`es-latam`/`en`,
rediseño cross-lingual): `language` (default `"es-latam"`) elige cuál atiende la
petición, reutilizando el timbre de una voz clonada en el idioma que sea. Los
tres overrides de síntesis son opcionales (`null`/ausentes = default de
`ChatterboxEngine.SYNTHESIS_DEFAULTS` para ese idioma). El protocolo no lleva
`model` ni `compute_backend`: el backend de cómputo se resuelve una sola vez al
arrancar el daemon (auto-detect o override vía variable de entorno) y aplica
por igual a todos los modelos servidos. `text` está acotado a 5000 caracteres.

**Response** (Daemon → CLI):

La respuesta es un **stream NDJSON** (`Content-Type: application/x-ndjson`): una
línea JSON por evento. El daemon emite N líneas `progress` con el avance real de
la síntesis (etapa actual y conteo de tokens del T3 en vivo) y cierra con una
línea `result` que lleva el WAV completo codificado en base64 y los tiempos por
sub-etapa. Si la síntesis falla en el hilo worker, se emite una línea `error` en
lugar de `result` (el cliente la convierte en un fallo con código de salida 5).

```
HTTP/1.1 200 OK
Content-Type: application/x-ndjson

{"event":"progress","stage":"conditionals","tokens":null,"elapsed":null}
{"event":"progress","stage":"t3","tokens":10,"elapsed":null}
{"event":"progress","stage":"t3","tokens":210,"elapsed":null}
{"event":"progress","stage":"s3gen","tokens":null,"elapsed":null}
{"event":"result","audio_b64":"<WAV en base64>","t3_time":9.7,"s3gen_time":7.0}
```

El orden garantizado es N×`progress` → 1×`result`, o bien 1×`error`. El esquema de
cada línea lo define `daemon/protocol.py` (`ProgressEvent` / `ResultEvent` /
`ErrorEvent`), fuente única de verdad **validada por ambos extremos**: `server.py`
(productor) emite exclusivamente vía `model_dump_json()`, e `ipc.py` (consumidor)
valida cada línea con `model_validate` contra esos mismos modelos y aborta con
`DaemonIPCError` ante cualquier frame no conforme (línea no-JSON, `event`
desconocido, esquema inválido o `audio_b64` no decodificable) — sin tolerancia a
frames sucios. El cliente reenvía cada `progress` validado al spinner de
`speech synthesize` (o `speech say`) para mostrar progreso real (p. ej. «Generando
voz · 210 tokens»); ver más abajo.

> **Errores de validación**: el rechazo por modelo no cargado sigue siendo una
> respuesta HTTP de error inmediata (`503` con cuerpo JSON `{"detail": ...}`),
> **no** un frame del stream: se valida antes de arrancar la síntesis.

### Versionado del protocolo

Los 5 modelos de `daemon/protocol.py` (`ProgressEvent`, `ResultEvent`,
`ErrorEvent`, `HealthResponse`, `VoicesResponse`) heredan de una clase base
común, `ProtocolModel`, que fija dos garantías en un solo lugar:

- **`schema_version`** (string, `"3"` actualmente): presente en cada línea del
  stream y en `/health`. Igual que el `schema_version` del CLI (`cli.SCHEMA_VERSION`),
  es un campo aditivo — añadir claves nuevas con default no lo incrementa; solo
  lo haría un cambio incompatible de una clave existente. La versión subió a
  `"3"` justo por eso: `model_loaded` de `HealthResponse` dejó de ser un
  booleano y pasó a ser un `dict[str, bool]` por idioma (rediseño
  cross-lingual), un cambio incompatible de un campo ya existente.
- **`extra="ignore"`**: un campo desconocido en el payload se descarta al
  parsear en vez de romper la validación. Esto es lo que hace tolerable el
  **rolling skew**: un daemon que sigue corriendo con la versión anterior
  mientras el CLI ya se actualizó (o viceversa) no revienta la comunicación —
  el extremo más nuevo simplemente ignora los campos que el más viejo no
  conoce, y los campos nuevos siempre traen un default para el extremo viejo
  que aún no los envía.

`HealthResponse` expone además **`version`** (string, vacío por defecto): la
versión del paquete `tts-sidecar` que sirve ese daemon (`__version__`), poblada
por el endpoint `/health`. Sirve para diagnosticar el skew real entre el CLI y
un daemon residente: si `tts-sidecar version` y el `version` de
`tts-sidecar daemon status --json` (o `/health` directamente) difieren tras una
actualización, `tts-sidecar daemon restart` relanza el daemon con el binario
nuevo.

Estas garantías son deliberadamente aditivas: mientras los cambios al protocolo
sean solo campos nuevos con default, `schema_version` no se incrementa; solo un
cambio incompatible de un campo existente lo amerita (como el de `model_loaded`
arriba).

## Comandos del Daemon

```bash
# Iniciar daemon (background)
tts-sidecar daemon start

# Iniciar daemon con auto-restart
tts-sidecar daemon start --autorestart --max-retries 3

# Precargar solo un idioma (default "all" precarga es-latam y en)
tts-sidecar daemon start --language es-latam

# Detener daemon
tts-sidecar daemon stop

# Reiniciar daemon
tts-sidecar daemon restart

# Ver estado del daemon
tts-sidecar daemon status

# Sin flags: synthesis sondea el daemon y lo usa si responde (autodetect)
tts-sidecar speech synthesize --text "Hola" --label demo

# Forzar daemon (sin sondear previo; falla si el daemon no responde)
tts-sidecar speech synthesize --text "Hola" --label demo --daemon

# Forzar modo directo (sin sondear el daemon)
tts-sidecar speech synthesize --text "Hola" --label demo --no-daemon
```

> **Código de salida para integradores**: `speech synthesize --daemon` (y
> `speech say --daemon`) terminan con código **5** (daemon inalcanzable) si el
> daemon no responde, en lugar del código de error genérico. Los comandos
> `daemon start/stop/restart` también devuelven `5` cuando la operación de
> ciclo de vida falla. Ver la tabla completa de códigos en `USAGE.md`
> (sección «Experiencia unificada entre sistemas operativos»).

> **Ventana de arranque (30-90 s)**: el puerto 8765 no abre hasta que el modelo
> termina de cargarse en memoria, lo que puede tardar entre 30 y 90 segundos
> según el hardware. `daemon start` bloquea internamente hasta confirmar
> «Daemon listo» (o el timeout de 120 s) antes de devolver el control, así que
> un script que lo invoca y espera esa confirmación no necesita hacer nada
> especial. Durante esa ventana, `daemon stop` **detecta el arranque en curso**:
> avisa por stderr que «el daemon está arrancando y aún no acepta conexiones»,
> **no mata el proceso** y termina con exit **5**, para que un orquestador
> distinga «arrancando» de «detenido» sin parsear texto — reintenta `daemon stop`
> cuando la carga termine. `daemon status`, en cambio, sigue reportando «no está
> corriendo» durante la ventana (su fuente es el health check): un orquestador
> que lance `daemon start` en background debe esperar su confirmación (o sondear
> `/health`) antes de asumir que el daemon está listo.

> **PID/lock file del daemon (`<user-data-dir>/daemon.pid`)**: `daemon start`
> crea este archivo de forma **atómica** (`os.open` con `O_CREAT|O_EXCL`) antes
> de lanzar el subproceso, de modo que dos `daemon start` concurrentes no pueden
> arrancar dos daemons —el segundo ve el lock vigente y no lanza nada— y persiste
> el PID del daemon una vez lanzado. Ese PID es la **fuente autoritativa** para
> `daemon stop` en la ventana de arranque: si registra un proceso vivo del
> daemon, es un arranque en curso (aviso + exit 5, como arriba); si el PID ya
> está muerto (un zombie que dejó el archivo tras un cierre abrupto), `daemon
> stop` **limpia el pidfile** y reporta «no está corriendo» en vez de quedar
> atascado en un exit 5 perpetuo. El daemon borra su propio pidfile al cerrar
> (graceful o por señal); un lock obsoleto que sobreviva a un `SIGKILL` se
> **reclama** en el siguiente `daemon start` al validar con psutil que su PID ya
> no corresponde a un daemon vivo. Sin pidfile, `daemon stop` cae al escaneo de
> procesos por cmdline (comportamiento previo, conservado como respaldo).
>
> La ruta depende del SO (es `data_root()` + `daemon.pid`, **no** del
> directorio de instalación, así que es escribible aunque el binario esté en
> `Program Files`, `Applications` o `site-packages`). El padre (`start`) y el
> hijo (`serve`) resuelven la misma ruta porque el hijo hereda las variables de
> entorno del padre:
>
> | Target de build | SO        | Ruta de `daemon.pid` |
> | --------------- | --------- | -------------------- |
> | `build-windows-x64` | Windows     | `%LOCALAPPDATA%\tts-sidecar\daemon.pid` (p. ej. `C:\Users\<user>\AppData\Local\tts-sidecar\daemon.pid`) |
> | `build-linux-x64`   | Linux x64   | `$XDG_DATA_HOME/tts-sidecar/daemon.pid` o `~/.local/share/tts-sidecar/daemon.pid` |
> | `build-linux-arm64` | Linux arm64 | `$XDG_DATA_HOME/tts-sidecar/daemon.pid` o `~/.local/share/tts-sidecar/daemon.pid` |
> | `build-darwin-arm64`| macOS arm64 | `~/Library/Application Support/tts-sidecar/daemon.pid` |
>
> La arquitectura no cambia la plantilla de ruta (los dos targets Linux la
> comparten), y los tres modos de ejecución (fuente, pip-install, congelado)
> resuelven la misma ruta porque `data_root()` no depende de `__file__`.

> **Indicador de progreso durante `speech synthesize` y `speech say`**:
> aunque la síntesis ocurre en el proceso del daemon, su progreso **real** viaja
> al cliente por el stream NDJSON de `/synthesize` (etapa actual + conteo de
> tokens del T3 en vivo). El CLI alimenta con esos eventos un **spinner** sobre
> **stderr** que muestra la etapa y el avance (p. ej. «Generando voz · 210
> tokens», subiendo) — tanto en modo daemon (eventos del stream) como en modo
> directo (mismo `progress_callback` del motor, sin HTTP). Es un indicador de
> etapa y avance de tokens, **no un porcentaje** del total. Solo aparece en
> terminales interactivas (TTY): si la salida está redirigida a un archivo o
> pipe, o corre en CI, el spinner se desactiva por completo y stdout queda
> intacto (contrato del CLI: stdout = datos, stderr = progreso).

> **Timeout de síntesis del cliente**: el cliente IPC espera la respuesta de
> `/synthesize` hasta **300 s** por defecto (audio largo en CPU lenta). Un
> consumidor programático que prefiera fallar antes puede reducirlo con la
> variable de entorno **`TTS_SIDECAR_REQUEST_TIMEOUT`** (segundos, admite
> decimales; un valor inválido o no positivo se ignora y se conserva el
> default). Al expirar, `speech synthesize --daemon` (o `speech say --daemon`)
> falla con el error IPC estándar; no hay reintento automático.

> **Control de admisión (tope de concurrencia)**: `/synthesize` admite como
> máximo **4** síntesis concurrentes (1 activa + hasta 3 en espera sobre el
> lock interno de síntesis). Una petición que exceda ese cupo recibe
> `HTTP 503` de inmediato, sin llegar a lanzar un hilo worker — el cliente IPC
> ya convierte cualquier no-200 en `DaemonIPCError`, por lo que `speech synthesize
> --daemon` (o `speech say --daemon`) falla con el mismo código de salida **5**
> que un daemon inalcanzable. El tope
> es fijo (`MAX_INFLIGHT_SYNTHESIS` en `server.py`), no configurable, y protege
> al proceso de acumular un thread sin límite por ráfaga de invocaciones
> concurrentes.

## Cancelación cooperativa del cliente

Cuando un cliente IPC cierra la conexión a mitad de una síntesis (`/synthesize`), el
daemon **detecta la desconexión y aborta la síntesis en curso** en vez de malgastar
GPU/CPU hasta completarla. Para un integrador que consume el stream NDJSON, «qué
pasa si cierro la conexión a mitad de síntesis» es parte del contrato, no un detalle
interno: sin documentarlo, el comportamiento correcto parece un bug (stream sin
evento terminal).

El mecanismo es **cooperativo** (no preemptivo): el generador del stream setea un
`threading.Event` al detectar la desconexión (vía `GeneratorExit`/`OSError`), y el
callback de progreso del worker (`push`) consulta ese evento en cada punto
cooperativo y eleva `SynthesisCancelled` (excepción compartida en `exceptions.py`).
El engine la re-lanza selectivamente desde `_emit_progress`/`_token_counting_iter` sin
romper el contrato best-effort para otras excepciones del callback (ver
`server.py:201-283`).

**Contrato observable** al desconectar el cliente:

1. La síntesis se aborta *best-effort* durante la fase **T3** (el autoregresivo). No
   se emite ningún frame terminal: ni `result` ni `error`, porque la conexión ya no
   existe para recibirlos.
2. El `finally` del worker **siempre** libera el semáforo de admisión y la memoria del
   modelo (igual que en éxito o error), así que el slot de concurrencia vuelve a estar
   disponible de inmediato.
3. Un stream que se corta de forma abrupta (sin `GeneratorExit` limpio) se trata
   igual: se señaliza la cancelación y el worker la honra en el próximo punto
   cooperativo.

> **Límite deliberado (no es un bug):** la etapa del vocoder **S3Gen no está
> instrumentada** para cancelación cooperativa. Si la desconexión ocurre *durante* el
> S3Gen, la cancelación solo se aplica tras completar esa etapa (unos segundos de
> consumo residual). No reportes como bug «cancelé y el daemon siguió consumiendo
> unos segundos»: es el comportamiento documentado para esa ventana.

## Parámetros Optimizados

Los parámetros optimizados son configuración propia del engine
(`ChatterboxEngine._apply_synthesis_optimizations`), no monkey-patches del
daemon: aplican por igual en modo directo y en el daemon, junto con el bypass
del watermark PerthNet y el timing por sub-etapa:

| Parámetro | Valor | Descripción |
|-----------|-------|-------------|
| `max_new_tokens` | 500 | Limita output del T3 (default: 1000), fijo para ambos idiomas |
| `n_cfm_timesteps` | 4 | Pasos de flow matching (default: 10), fijo para ambos idiomas |
| `exaggeration` | 0.75 (`es-mx-latam`) / 0.65 (`en`) | Expresividad emocional (default de fábrica: 0.5), overrideable con `--exaggeration` |

`cfg_weight` y `temperature` también tienen un default propio por idioma
(`ChatterboxEngine.SYNTHESIS_DEFAULTS`) y son overrideables con `--cfg-weight`
y `--temperature`; ver [USAGE.md](../USAGE.md) para el detalle completo de
flags de síntesis.

## Métricas de Rendimiento

| Métrica | Sin Daemon | Con Daemon |
|---------|------------|------------|
| Tiempo síntesis | ~50s | ~15-20s |
| Carga de modelo | 5-8s por llamada | 5-8s solo al iniciar |
| Overhead compilación | ~30s por llamada | ~1.6s solo al inicio |

## Decisiones de Diseño

| Aspecto | Decisión | Alternativa Considerada |
|---------|----------|------------------------|
| **IPC** | HTTP (FastAPI) | Named pipes, gRPC |
| **Puerto** | Fijo 8765 en loopback (sin flag `--port`) | Puerto configurable |
| **Fallback** | Automático a modo directo | Error si daemon no disponible |
| **Lifecycle** | start/stop/restart/status | Solo auto-start |
| **Resiliencia** | Retry + auto-restart flag | Ninguna |
| **torch.compile** | Compartido via proceso daemon | Memory-mapped files |
| **Gestión de memoria** | Limpieza de caché CUDA + GC tras cada síntesis | Sin liberación (fragmentación bajo uso prolongado en CUDA) |
| **Control de admisión** | Semáforo acotado (tope fijo 4), rechazo `503` inmediato | Encolado con espera indefinida |

## Compatibilidad

- El **contrato del CLI** (comandos, flags, códigos de salida, stdout = datos /
  stderr = progreso) es estable: los comandos del grupo `speech` (`speech synthesize`,
  `speech say`) aceptan `--daemon`, `--no-daemon` y las demás flags de manera
  idéntica.
- El **protocolo interno daemon→cliente** de `/synthesize` es un stream NDJSON:
  N líneas `progress` (etapa y tokens en vivo), seguidas de una línea `result`
  con el audio en base64 y los tiempos por sub-etapa, o una línea `error` si la
  síntesis falla. Daemon y cliente viajan siempre en la misma versión (no hay
  usuarios externos desplegados), así que no se negocia capacidad; si
  actualizas el binario, actualiza ambos lados a la vez.
- Si el daemon no está corriendo, el CLI degrada a modo directo; `--no-daemon`
  fuerza ese modo directo explícitamente.
