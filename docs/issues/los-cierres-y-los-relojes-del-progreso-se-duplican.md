# Los cierres del progreso y los relojes del tiempo transcurrido se duplican y no coinciden entre sí

| Campo | Valor |
|---|---|
| Estado | **resuelto** el 2026-10-20 |
| Severidad | baja |
| Tipo | funcional |
| Componente | `src/main.rs` (todos los handlers con indicador y sus mensajes de resultado), `avi-lifecycle` (`progress.rs`, `confirm.rs`) |
| Versión detectada | 0.34.0 |
| Plataforma | todas (los relojes y los mensajes no dependen de la terminal) |
| Reproducibilidad | siempre |
| Detectado en | inspección de código y pruebas manuales del indicador de progreso, 2026-10-19 |
| Prueba de regresión | `crates/avi-lifecycle/src/progress.rs`: `close_does_not_paint_a_closing_line`; `tests/cli_golden.rs`: `a_path_that_fails_with_question_mark_leaves_no_open_phase`, `the_final_message_is_said_once_by_stdout_with_its_duration`; `src/main.rs`: `started_names_the_operation_the_daemon_reports` |
| Documento canónico | [`docs/CLI/CONTRACT.md`](../CLI/CONTRACT.md#el-contrato-de-salida-por-comando) y [`docs/DESIGN.md`](../DESIGN.md#salida-en-terminal) |

## Resumen

Casi todos los comandos con indicador terminan contando dos veces lo mismo: una vez por `PhaseProgress::finish` (por stderr, con formato `[ok] … (N s)`) y otra por el mensaje de resultado del handler (por stdout). Y cuando la operación pasa por el daemon, el tiempo que muestran las líneas de actividad no es el tiempo de la fase sino el de un reloj propio que empieza más tarde, con lo que las cifras nunca cuadran. El resultado es una salida redundante y un tiempo transcurrido que no se puede leer.

## Entorno

- Binario 0.34.0, build de desarrollo del repositorio, Windows 11, PowerShell 7.
- Daemon arrancado en `127.0.0.1:8765` para la rama del doblaje; detenido para la captura de `daemon stop`.
- Sin `AVI_NO_PROGRESS`.

## Precondiciones

- Para el cierre duplicado: cualquier comando con indicador que termine bien (o que cancele).
- Para los relojes desfasados: daemon arrancado y una operación con stream NDJSON (`speech dub`, `voice clone`, `speech synthesize`).

## Pasos para reproducir

**Rama A — cierre doble en la rama daemon:**

1. Con el daemon arrancado y un dispositivo de entrada:

```powershell
ai-voice-interconnector speech dub --mic --source-language es --target-language en --voice default --daemon
```

**Rama B — cierre doble con daemon detenido:**

2. Con el daemon detenido:

```powershell
target\debug\ai-voice-interconnector.exe daemon stop
```

**Rama C — «Cancelado.» dos veces:**

3. En TTY, sin `--yes`, con una voz existente:

```powershell
ai-voice-interconnector voice remove <voz>
```

4. Responder `n` a la confirmación.

**Rama D — relojes distintos:**

5. Con el daemon arrancado, sintetizar un texto largo por `--daemon` y comparar el `(N s)` de las líneas de actividad con el de la fase y con el del cierre.

## Resultado observado

**Rama A (TTY):**

```text
Sintetizando voz… (8 s)
Doblaje reproducido.
[ok] Doblaje completado. (23 s)
```

Dos mensajes para el mismo hecho (`stdout` y `stderr`), y el `(8 s)` de la última línea de actividad no guarda relación con el `(23 s)` del cierre.

**Rama B (no-TTY):**

```text
# stderr
… Deteniendo daemon… (0 s)
[ok] Daemon detenido. (1 s)
```

```text
# stdout
Señal de apagado enviada al daemon en 127.0.0.1:8765.
```

Con el daemon **no** arrancado, la salida es la misma salvo por el mensaje de `stdout` («El daemon no estaba en ejecución.»): el cierre dice «Daemon detenido.» también en ese caso, y el `(1 s)` es mayor que el `(0 s)` de la fase aunque la operación haya tardado menos de un segundo.

**Rama C (TTY):** tras responder `n`, la cadena `Cancelado.` aparece dos veces, una por stderr (escrita por el motor de confirmación) y otra por stdout (escrita por el handler).

**Rama D (TTY):** las líneas de actividad del daemon arrancan siempre en `(0 s)` aunque la fase lleve segundos, porque cada una usa un `Instant` propio.

**Otros cierres dobles observados en el mismo patrón:** `Voz 'x' clonada.` + `[ok] Voz clonada.`; `Síntesis completada: <ruta>` + `[ok] Síntesis completada.`; `Limpieza completada: N ruta(s), M barrido(s).` + `[ok] Limpieza completada.`; `Reproduciendo.` + `[ok] Reproducción completada.`; `Voz 'x' eliminada.` + `[ok] Voz eliminada.`; `Daemon iniciado correctamente (pid N).` + `[ok] Daemon iniciado.`

## Resultado esperado

- `docs/DESIGN.md`, «Salida en terminal» → «Bloques»: cada bloque no debe repetir lo que ya ha dicho el anterior. Un cierre de progreso y un mensaje de resultado que dicen lo mismo son redundancia dentro de la misma salida.
- `docs/DESIGN.md`, «Salida en terminal» → «Tamaños y duraciones»: las duraciones salen de `progress::format_duration`; una duración debe medir la operación que el usuario acaba de esperar, no un tramo de ella.
- `avi-lifecycle/src/confirm.rs`, `confirm`: *«El usuario dijo que no. No es un error … El producto responde `Cancelado.` y sale con éxito»*. Un único «Cancelado.» es el contrato.
- `avi-lifecycle/src/progress.rs`, `PhaseProgress::finish`: el cierre lleva el tiempo transcurrido del indicador, que debe ser el mismo que el que ha estado mostrando la fase. Con el cierre explícito sin mensaje de estado, esa duración la muestra el mensaje final del handler, tomada del mismo reloj.

## Impacto y workaround

- Severidad baja: la información es correcta, pero la salida es redundante y el tiempo transcurrido no se puede interpretar (dos cifras distintas para la misma operación).
- Afecta a la lectura de logs y a cualquier script que haga parseo de la salida por palabras, que verá el mismo estado dos veces con dos redacciones distintas.
- Workaround: ninguno necesario; `AVI_NO_PROGRESS=1` elimina el cierre del indicador y deja solo el mensaje de resultado.

## Evidencia

- Salidas literales de las ramas A y B. La rama B se capturó el 2026-10-19 con el binario de desarrollo de 0.34.0; la rama A es el cierre del doblaje con `--daemon` y `--mic` que reportó quien detectó el defecto.
- `src/main.rs`: los pares de cierre están en los handlers de `voice clone`, `voice remove`, `speech synthesize`, `speech say`, `speech transcribe`, `speech dub`, `speech play`, `cleanup`, `daemon start`, `daemon restart`, `daemon stop` y `self update`.
- `src/main.rs`, `handle_cleanup` y el comando `SelfCommands::Update`: escriben `println!("Cancelado.")` por stdout en un camino en el que `confirm::confirm` ya ha escrito `Cancelado.` por stderr. Lo mismo en `VoiceCommands::Remove` y en `SelfCommands::Uninstall`.
- `src/main.rs`, `daemon_synthesize_wav` y `consume_ndjson_stream`: cada una crea su propio `Instant` y se lo pasa a `emit_ndjson_progress`; `PhaseProgress` usa otro distinto, creado en la entrada del handler.
- `src/main.rs`, `emit_ndjson_progress`: `started`, `progress` y `heartbeat` calculan el tiempo desde ese `Instant` propio, no desde el de la fase.

## Análisis de causa

**Causa raíz (confirmada).** Hay dos dueños para el mensaje de cierre (el indicador y el handler) y tres relojes en juego (`PhaseProgress.start`, el `Instant` de `daemon_synthesize_wav` y el de `consume_ndjson_stream`). No hay una decisión única de quién informa del resultado ni de qué reloj mide la operación completa.

**Fichas por síntoma:**

- **Ficha A — cierre doble en el éxito.** Confirmada en `voice clone`, `voice remove`, `speech synthesize`, `speech say`, `speech play`, `speech dub`, `cleanup`, `daemon start`/`restart`. El mensaje del handler suele ser más rico (incluye ruta, PID o conteos) y el del indicador más escueto, pero dicen lo mismo.
- **Ficha B — «Cancelado.» dos veces.** Confirmada en `cleanup`, `voice remove`, `self update` y `self uninstall`: `confirm::confirm` lo escribe por stderr y el handler lo repite por stdout.
- **Ficha C — relojes desfasados.** Confirmada: la fase mide desde la entrada del handler (incluye el sondeo de vida del daemon y, en `--mic`, la captura); las líneas de actividad miden desde justo antes del POST o desde la recepción de las cabeceras. Por eso el `(8 s)` de una línea de actividad puede convivir con un `(23 s)` de cierre.
- **Ficha D — el cierre de `daemon stop` dice «detenido» aunque no hubiera daemon.** Confirmada por la captura de la rama B: el texto del cierre no refleja `was_running`.
- **Ficha E — `finish` borra la línea en la que esté el cursor.** Por verificar: en TTY el cierre hace `\r\x1b[2K` sobre la línea actual; si el último escritor dejó una línea sin terminar (por ejemplo el prompt del bucle `--play`), la borra. No se reprodujo porque el eco del Enter coloca el cursor en una línea nueva.

**Lo que ya se descartó.** No es un desajuste de búferes: los mensajes llegan por canales distintos y el orden observado es el esperado. No es un redondeo: `format_duration` es determinista y está probada.

## Diagnóstico sugerido

Dirección de la corrección (ya confirmada la causa):

1. Decidir quién informa del resultado: el cierre no imprime estado y el mensaje del handler lleva la duración, con un único mensaje de resultado. Queda como **alternativa descartada** conservar el `[ok]` del indicador como cierre: el cierre no imprime estado y el mensaje del handler lleva la misma duración que la fase mostró.
2. Eliminar el `println!("Cancelado.")` de los handlers: `confirm::confirm` ya lo ha escrito por stderr, que es donde vive la confirmación.
3. Unificar el reloj: el reloj único es el del indicador, compartido por la fase y por el mensaje final, de modo que todas las cifras salgan del mismo origen. La actividad del daemon debe usar ese mismo reloj, no un `Instant` propio.
4. En `daemon stop`, atar el texto del cierre a `was_running` (por ejemplo, «No había daemon que detener.» cuando no corría).
5. Revisar el cierre de `finish` para que no borre una línea con contenido de otro escritor.

## Criterio de aceptación

- En la salida de cada comando con indicador, el estado (`ok`/`cancelado`/`fallido`) aparece una sola vez. El cierre no imprime estado: el mensaje final del handler lleva la duración.
- Toda cifra de tiempo transcurrido de una misma operación sale del mismo reloj: el del indicador, compartido por la fase y por el mensaje final. La actividad del daemon usa ese mismo reloj; la fase, las líneas de actividad y el mensaje final muestran valores monótonos y coherentes entre sí.
- `daemon stop` sin daemon en marcha no dice «Daemon detenido.».
- Pruebas de regresión que deben existir:
  - Un test de salida (no-TTY) por comando que verifique que el literal «Cancelado.» aparece exactamente una vez y que el estado del éxito aparece exactamente una vez.
  - Un test de `emit_ndjson_progress` que, con una fase cuyo reloj avanza, verifique que el tiempo impreso es el de la fase y no uno propio.
  - Un test de `daemon stop` sin daemon que verifique el texto del cierre.

## Relacionados

- `fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md` (los mismos caminos de salida).
- `progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md` (el reloj de los eventos del daemon).
- `la-rama-local-y-la-del-daemon-muestran-fases-y-cierres-distintos.md` (el recorrido distinto entre vías).
- `CHANGELOG.md`, 0.34.0: «progreso de fases por stderr, una sola línea por terminal».

## Resolución (2026-10-20)

**Resuelto.** Hay **un solo dueño del mensaje de resultado** y **un solo reloj**.

- El indicador **no imprime estado**: el cierre `[ok]`/`[error]` desapareció. El mensaje final lo emite el handler —o el flujo de la librería en `self install` y `self uninstall`—, por stdout, **una sola vez** y con la duración de la operación. Se dice el estado una vez, y lo dice quien cierra.
- El reloj es **único**: el del indicador. La fase, la actividad del daemon y el mensaje final salen del mismo origen, de modo que sus cifras son monótonas y coherentes entre sí. La traducción de los eventos ya no tiene reloj propio.
- `Cancelado.` lo dice **solo** el motor de confirmación, por stderr. Los handlers ya no lo repiten.
- `daemon stop` ya no dice «Daemon detenido.» cuando no había ningún daemon en marcha: dice «No había daemon que detener (N s).».

**Pruebas de regresión** (las que cubren este defecto):

| Prueba | Qué fija |
|---|---|
| `crates/avi-lifecycle/src/progress.rs` → `close_does_not_paint_a_closing_line` | El cierre no imprime estado: el indicador no dice cómo terminó |
| `tests/cli_golden.rs` → `a_path_that_fails_with_question_mark_leaves_no_open_phase` | El error se dice **una sola vez** y no queda cierre en stderr |
| `tests/cli_golden.rs` → `the_final_message_is_said_once_by_stdout_with_its_duration` | El mensaje final lo dice **una sola vez**, por stdout, y con la duración de la operación |
| `src/main.rs` → `started_names_the_operation_the_daemon_reports` | El mensaje nombra la operación concreta, no una genérica |

**Documento canónico**: [`docs/CLI/CONTRACT.md`](../CLI/CONTRACT.md#el-contrato-de-salida-por-comando) —la tabla del contrato de salida por comando, con su columna de mensaje final— y [`docs/DESIGN.md`](../DESIGN.md#salida-en-terminal) —«Salida en terminal», que fija el dueño único del mensaje final y el reloj común—.

**Nota sobre el alcance**: el cambio es **visible para quien lea logs**: el cierre `[ok] … (N s)` desaparece de stderr y el mensaje final lleva ahora la duración. El envelope `--json`, los códigos de salida y los `reason` no cambian.
