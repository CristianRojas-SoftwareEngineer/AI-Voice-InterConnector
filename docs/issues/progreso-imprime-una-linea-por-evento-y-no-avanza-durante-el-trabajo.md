# El indicador de progreso imprime una línea por evento y, en cambio, no avanza durante el trabajo real

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | funcional |
| Componente | `avi-lifecycle` (`PhaseProgress`, `format_duration`), `src/main.rs` (`emit_ndjson_progress`, `consume_ndjson_stream`, `daemon_synthesize_wav`, `launch_daemon`, `await_daemon_ready`), `avi-daemon` (`with_heartbeats`) |
| Versión detectada | 0.34.0 |
| Plataforma | Windows 11 / PowerShell 7 (reproducido); la parte no-TTY es común a todas las plataformas por el formato del render |
| Reproducibilidad | siempre |
| Detectado en | inspección de código y pruebas manuales del indicador de progreso, 2026-10-19 |

## Resumen

El indicador no tiene un protocolo único de actualización. Por un lado, en no-TTY cada `tick()` genera una línea nueva (`PhaseProgress::render_ref` con `eprintln!`), de modo que una espera con sondeo de 250 ms produce decenas de líneas idénticas, y los eventos del daemon generan una línea nueva **en cualquier modo**, porque `emit_ndjson_progress` escribe con `eprintln!` sin distinguir TTY. Por otro lado, en TTY las fases de las ramas locales se pintan una sola vez y no se vuelven a repintar durante el trabajo bloqueante, así que el tiempo se queda congelado en `(0 s)` mientras la operación dura treinta segundos. Además, `phase()` seguido de un `tick()` inmediato duplica la línea en no-TTY, y los eventos del daemon se imprimen con un reloj propio, desfasado del de la fase.

El síntoma visible es que el indicador miente sobre el tiempo que lleva la operación: o repite la misma cifra en muchas líneas, o no la mueve en absoluto.

## Entorno

- Binario 0.34.0, build de desarrollo del repositorio (`target/debug/ai-voice-interconnector.exe`), Windows 11, PowerShell 7.
- Daemon arrancado en `127.0.0.1:8765` durante la exploración (por eso el plan de `cleanup --dry-run` de la rama A incluye el estado del daemon), y detenido al terminar. La salida literal de la rama D (TTY, doblaje con `--daemon`) no es de este entorno: es la que reportó quien detectó el defecto, con modelos provisionados.
- Sin `AVI_NO_PROGRESS` definida. Sin modelos TTS provisionados en el entorno de captura.
- Las capturas de no-TTY separan `stdout` y `stderr`; las de TTY son la salida completa del terminal.

## Precondiciones

- Para las ramas no-TTY: cualquier comando con indicador ejecutado con `stderr` redirigido a fichero o canal (PowerShell `2>`, tuberías, CI).
- Para las ramas TTY: una terminal interactiva. Para la rama del doblaje: daemon arrancado y dispositivo de entrada.

## Pasos para reproducir

**Rama A — duplicado por `phase()` + `tick()` (no-TTY):**

1. Ejecutar un comando que pinta una fase y hace un `tick()` inmediato:

```powershell
target\debug\ai-voice-interconnector.exe speech say --text hola --voice default --no-daemon
```

2. Ejecutar `cleanup`, que hace lo mismo:

```powershell
target\debug\ai-voice-interconnector.exe cleanup --dry-run --all
```

**Rama B — arranque del daemon con sondeo de 250 ms (no-TTY):**

3. Con el daemon detenido, arrancarlo con `stderr` redirigido:

```powershell
target\debug\ai-voice-interconnector.exe daemon start 2> salida.txt
```

4. Inspeccionar `salida.txt`: debe contener una línea `… Calentando motor de síntesis… (N s)` por cada sondeo de `await_daemon_ready` (hasta 20 líneas con el techo de 5 s de `DAEMON_HEALTH_DEADLINE`), además de las del sondeo previo de publicación de dirección.

**Rama C — tiempo congelado durante el trabajo (no-TTY y TTY):**

5. `self update` con red: la descarga entera ocurre con una sola fase `Descargando versión X…` y un solo `tick()` previo:

```powershell
target\debug\ai-voice-interconnector.exe self update --force
```

6. `speech say`/`speech synthesize` locales con el motor provisionado: la síntesis es una llamada síncrona que bloquea su hilo (`synthesize_voice_directly`), y solo se hace un `tick()` antes de entrar.

**Rama D — eventos del daemon con reloj propio (TTY):**

7. Con el daemon arrancado:

```powershell
ai-voice-interconnector speech dub --mic --source-language es --target-language en --voice default --daemon
```

8. Comparar el `(N s)` de cada línea de evento con el de la línea de la fase.

**Rama E — `speech synthesize --daemon` sin progreso incremental (TTY):**

9. Con el daemon arrancado:

```powershell
ai-voice-interconnector speech synthesize --text <texto largo> --label tmp --voice default --daemon
```

## Resultado observado

**Rama A (`stderr`, no-TTY):** la fase sale dos veces, idéntica, sin tiempo transcurrido entre medias.

`speech say` (salida 1):

```text
… Validando texto y voz… (0 s)
Aviso: se ignoró 'vendor/qwen3-tts/qwen_tts.exe' del directorio de trabajo: define AVI_ALLOW_CWD_VENDOR=1 para habilitarlo.
… Sintetizando voz… (0 s)
… Sintetizando voz… (0 s)
Aviso: se ignoró 'vendor/qwen3-tts/qwen_tts.exe' del directorio de trabajo: define AVI_ALLOW_CWD_VENDOR=1 para habilitarlo.
Error: El binario Qwen3-TTS no está provisionado.
```

`cleanup --dry-run --all` (`stderr`, salida 0), primeras líneas:

```text
… Limpiando datos… (0 s)
… Limpiando datos… (0 s)
Se borrarán 6 ruta(s), 5,9 GB en total:
```

**Rama B (no-TTY, `daemon start`):** por inspección de `launch_daemon` y `await_daemon_ready` (la captura literal de esta rama no se conserva), el sondeo de 250 ms emite una línea `… Calentando motor de síntesis… (N s)` por iteración: hasta veinte con el techo de `DAEMON_HEALTH_DEADLINE` (5 s) de `await_daemon_ready`, más las del sondeo previo de publicación de dirección con techo de `DAEMON_READY_DEADLINE` (10 s). La salida acumulada supera con facilidad las veinte líneas idénticas.

**Rama C:** el tiempo no avanza. En `self update`, la fase `… Descargando versión X… (0 s)` permanece congelada durante toda la descarga; en `speech say`, la fase `… Sintetizando voz… (0 s)` permanece congelada mientras dura la síntesis, y el cierre muestra el total real (por ejemplo `[ok] Reproducción completada. (12 s)`). La síntesis local es una llamada síncrona que bloquea su hilo (`synthesize_voice_directly`), no un `await` bloqueante: el indicador no puede repintar mientras dura, y el tiempo total lo aporta el mensaje final.

**Rama D (TTY, `speech dub --mic --daemon`; salida literal reportada por quien detectó el defecto):** cada segundo aparece dos veces la misma cifra, con un latido `heartbeat` cada ~500 ms y sin que la fase se repinte:

```text
Iniciando operación… (0 s)
Procesando… (0 s)
Sintetizando voz… (1 s)
Sintetizando voz… (1 s)
Sintetizando voz… (2 s)
Sintetizando voz… (2 s)
Sintetizando voz… (3 s)
Sintetizando voz… (3 s)
Sintetizando voz… (4 s)
Sintetizando voz… (4 s)
Sintetizando voz… (5 s)
Sintetizando voz… (5 s)
Sintetizando voz… (6 s)
Sintetizando voz… (6 s)
Sintetizando voz… (7 s)
Sintetizando voz… (7 s)
Sintetizando voz… (8 s)
Doblaje reproducido.
[ok] Doblaje completado. (23 s)
```

Además, las líneas de evento arrancan en `(0 s)` aunque la fase `… Doblando audio (daemon)… (0 s)` ya lleve rato abierta, porque cada `emit_ndjson_progress` usa su propio `Instant` (creado en `consume_ndjson_stream`, después de las cabeceras): los eventos cubren 0-8 s mientras el cierre revela un total de 23 s. En no-TTY, los mismos eventos producen una línea por latido (dos por segundo) en cualquier comando con `--daemon`.

**Rama E:** el cuerpo NDJSON de `/synthesize` se lee entero antes de iterarlo, de modo que todas las líneas de evento aparecen de golpe al terminar la síntesis, en lugar de ir llegando.

## Resultado esperado

- `avi-lifecycle/src/progress.rs`, `PhaseProgress::tick`: *«Vuelve a pintar la línea de la fase actual con el tiempo transcurrido actualizado. Útil en bucles de espera»*. El repintado es la operación sobre la **misma** línea; en no-TTY no debe producir una línea nueva por sondeo.
- `docs/DESIGN.md`, «Salida en terminal» → «Tamaños y duraciones»: las duraciones salen de `progress::format_duration`; una duración que no se mueve durante una operación larga no informa de nada.
- `docs/DESIGN.md`, «Salida en terminal» → «Bloques»: dentro de un bloque no hay ninguna línea en blanco ni repetición inútil.
- El contrato de los eventos del daemon (`docs/CLI/CONTRACT.md`, § de `speech dub` y § de `voice clone`) describe los latidos `heartbeat` como actividad remota «cada ~500 ms» junto a los eventos `progress` por etapa; esa actividad debe verse **mientras** ocurre, no al final.

## Impacto y workaround

- En no-TTY (CI, capturas de log, pipelines) `daemon start` puede dejar más de veinte líneas idénticas en el log, y cualquier comando con `phase()` + `tick()` deja dos.
- En TTY el usuario ve un contador que no se mueve durante las operaciones largas (descarga, síntesis, clonado) y, en la rama del doblaje, un segundo que se repite.
- Workaround: `AVI_NO_PROGRESS=1` silencia todo el indicador. No hay forma de pedir solo «una línea por fase».

## Evidencia

- Salidas literales de la rama A (capturada el 2026-10-19 con el binario de desarrollo de 0.34.0) y de la rama D (la salida del doblaje con `--daemon` que reportó quien detectó el defecto); la rama B está confirmada por inspección de `launch_daemon`/`await_daemon_ready`.
- `crates/avi-lifecycle/src/progress.rs`, `PhaseProgress::render_ref`: en no-TTY hace `eprintln!("… {msg} ({elapsed})")`, es decir una línea por llamada; en TTY hace `eprint!("\r\x1b[2K… {msg} ({elapsed})")`, que repinta.
- `src/main.rs`, `launch_daemon` y `await_daemon_ready`: llaman a `progress.tick()` tras cada `tokio::time::sleep(DAEMON_POLL_INTERVAL)`.
- `src/main.rs`, `emit_ndjson_progress`: hace un `eprintln!` por evento (`started`, `progress`, `heartbeat`) sin distinguir TTY.
- `src/main.rs`, `daemon_synthesize_wav`: hace `resp.bytes().await` (cuerpo entero) antes de iterar las líneas, a diferencia de `consume_ndjson_stream`, que procesa por trozos.
- `crates/avi-daemon/src/lib.rs`, `with_heartbeats` y `STREAM_HEARTBEAT`: el intervalo de los latidos es de 500 ms; combinado con `format_duration` (segundos hacia abajo) produce exactamente dos repeticiones por segundo.

## Análisis de causa

**Causa raíz (confirmada).** No hay un protocolo único de actualización: `PhaseProgress` es un reloj que se imprime a sí mismo, y cada consumidor decide cuándo llamarlo. En no-TTY, cada llamada es una línea; en TTY, cada llamada repinta. Como el llamador no conoce esa diferencia, los bucles de sondeo generan spam y las fases bloqueantes se quedan sin repintar.

**Fichas por síntoma:**

- **Ficha A — `phase()` + `tick()` inmediato duplica la línea en no-TTY.** Confirmada por la captura de `speech say` y `cleanup`. Aparece en `handle_translate` local, `voice clone`, `speech synthesize`, `speech say`, `speech dub` (fase de transcripción y de síntesis), `handle_cleanup`, `self update` (fase de descarga) y `doctor --repair`.
- **Ficha B — los bucles de sondeo generan una línea por iteración en no-TTY.** Confirmada por inspección de `launch_daemon` y `await_daemon_ready`; el techo es `DAEMON_READY_DEADLINE` (10 s) y `DAEMON_HEALTH_DEADLINE` (5 s) con `DAEMON_POLL_INTERVAL` de 250 ms.
- **Ficha C — el tiempo se congela durante el trabajo bloqueante.** Confirmada: los únicos `tick()` son los de entrada de fase (`self update`, `cleanup`, `doctor --repair`, `translate`, `voice clone`, `speech synthesize`, `speech say`, `speech dub`), hechos en el instante de pintar; ninguna rama llama a `tick()` durante el trabajo bloqueante, y ninguna rama daemon llama a `tick()` jamás. En TTY el usuario ve `(0 s)` durante toda la operación.
- **Ficha D — los eventos del daemon usan un reloj distinto al de la fase.** Confirmada: `emit_ndjson_progress` recibe un `Instant` creado en `daemon_synthesize_wav` (antes del POST) o en `consume_ndjson_stream` (después de las cabeceras), mientras la fase mide desde la entrada del handler, antes del sondeo de vida del daemon y, en `--mic`, antes de la captura.
- **Ficha E — `speech synthesize --daemon` no muestra progreso incremental.** Confirmada por inspección de `daemon_synthesize_wav`: todo el cuerpo se descarga antes de emitir los eventos.
- **Ficha F — el evento `started` siempre se traduce como «Iniciando operación…».** Confirmada: `emit_ndjson_progress` lee la clave `operation` del evento y ningún emisor del daemon la incluye (`/synthesize`, `/voices/clone` y `/dub` emiten `started` con `voice`/`name`/`text_length`).

**Lo que ya se descartó.** No es un fallo de `format_duration`: la función es determinista y está probada (`format_duration_uses_seconds_minutes_and_hours`). No es un fallo del búfer de stderr: los duplicados se ven igual con y sin redirección.

## Diagnóstico sugerido

Dirección de la corrección (ya confirmada la causa):

1. Separar el estado del render: `PhaseProgress` debe guardar la fase y el reloj, y exponer un único `render()` que en no-TTY **no** imprima por cada `tick()` (por ejemplo, imprimir solo cuando el segundo mostrado cambia, o imprimir solo al cambiar de fase y cerrar). Así los bucles de sondeo dejan de generar spam.
2. Pintar la fase al empezar cada bloque de trabajo real y repintarla en cada punto de espera (descarga, sondeo, latido), en lugar de un único `tick()` previo.
3. Hacer que las ramas daemon compartan el reloj de la fase: `emit_ndjson_progress` debe recibir la fase en lugar de un `Instant` propio, o bien debe repintar la línea de la fase en TTY en lugar de escribir una línea nueva.
4. En `daemon_synthesize_wav`, consumir el stream por trozos como ya hace `consume_ndjson_stream`.
5. Que los emisores del daemon incluyan `operation` en `started` (o que `emit_ndjson_progress` traduzca el `stage` de `started`).

Alternativa descartada: imprimir siempre en modo no-TTY con `\r` — violaría el contrato de que sin terminal no hay secuencias ANSI.

## Criterio de aceptación

- En no-TTY, `daemon start` no produce más de dos líneas de progreso por fase (una al entrar y el cierre), sin importar cuántos sondeos se hagan.
- En no-TTY, ningún comando produce dos líneas consecutivas idénticas de progreso.
- En TTY, el tiempo mostrado por la fase avanza al menos un segundo por segundo durante una operación de más de 5 s, en las esperas asíncronas: la descarga de `self update` y la espera de las respuestas de `/transcribe` y `/translate`. En esas esperas sí debe haber repintado visible.
- El trabajo síncrono queda como excepción declarada al repintado: la síntesis y el clonado local (`synthesize_voice_directly`, `avi_tts::clone_voice`) y la captura del micrófono son llamadas síncronas que bloquean su hilo, así que el indicador no puede avanzar mientras duran. En ellas no se promete avance visible; el tiempo total lo aporta el mensaje final.
- Ninguna línea de evento del daemon muestra un tiempo menor que el de la fase que la precede.
- `speech synthesize --daemon` muestra actividad mientras ocurre la síntesis, no al final.
- Pruebas de regresión que deben existir:
  - Un test de `PhaseProgress` que, en no-TTY y con un doble de `phase()` + 40 `tick()`, verifique el número exacto de líneas emitidas.
  - Un test de `emit_ndjson_progress` que verifique que `started` sin `operation` no produce «Iniciando operación…» (o que el daemon sí lo envía).
  - Un test de `daemon_synthesize_wav` con un doble que emita varios eventos y verifique que se procesan por trozos (eventos visibles antes del `result`).

## Relacionados

- `fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md` (misma familia de indicadores; distinto síntoma).
- `la-rama-local-y-la-del-daemon-muestran-fases-y-cierres-distintos.md` (la asimetría de eventos entre endpoints del daemon).
- `los-cierres-y-los-relojes-del-progreso-se-duplican.md` (el reloj desfasado entre fase y eventos).
- `CHANGELOG.md`, 0.34.0: «progreso de fases por stderr, una sola línea por terminal».
