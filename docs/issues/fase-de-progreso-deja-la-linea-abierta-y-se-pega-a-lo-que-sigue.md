# La línea de la fase de progreso queda abierta y se pega a cualquier texto que salga después

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | alta |
| Tipo | funcional |
| Componente | `avi-lifecycle` (`PhaseProgress`, `cleanup::run`, `confirm::confirm`, `install::install`, `uninstall::run`), `src/main.rs` (todos los handlers con indicador), `avi-audio` (`capture_16k_mono_pcm_until_enter`), `avi-tts` (`resolve_binary`) |
| Versión detectada | 0.34.0 |
| Plataforma | Windows 11 / PowerShell 7 (reproducido); por diseño afecta a todas las plataformas porque el pintado `\r` no termina la línea en ninguna de ellas |
| Reproducibilidad | siempre |
| Detectado en | inspección de código y pruebas manuales del indicador de progreso, 2026-10-19 |

## Resumen

`PhaseProgress` pinta la línea de la fase sin terminarla (`\r\x1b[2K… {mensaje} ({t})` en TTY, `eprintln!` en no-TTY) y después **cede la consola a cualquier otro escritor de `stderr` o `stdout` sin cerrarla**. En TTY eso hace que el siguiente mensaje se dibuje a continuación de la misma línea física, de modo que la lectura se vuelve ininteligible: la fase de progreso, el aviso del producto, el menú interactivo, el plan de una operación o el propio `Error:` quedan pegados en una sola línea. Además, no hay un cierre explícito en muchos caminos de salida y `PhaseProgress` no es dueño de la línea, con lo que la fase nunca recibe su cierre y queda una línea huérfana que no informa de nada.

Afecta al flujo principal: `speech synthesize`, `speech say`, `speech transcribe`, `speech dub` y `voice clone` en su rama local, y también `cleanup`, `self update` y los comandos de ciclo de vida (`self install`/`self uninstall`). El resultado de la operación sigue siendo correcto; lo que se rompe es la salida de terminal y, con ella, el diagnóstico.

## Entorno

- Binario 0.34.0, build de desarrollo del repositorio (`target/debug/ai-voice-interconnector.exe`), Windows 11, PowerShell 7.
- Daemon en ejecución sobre `127.0.0.1:8765` (lo que hace posible la reproducción del doblaje con `--daemon`).
- Modelos provisionados: el TTS no está provisionado en el entorno de captura (se usa eso para forzar el camino de error de `speech say`); el de traducción y el STT no intervienen en las capturas.
- Sin variables `AVI_*` definidas: `AVI_NO_PROGRESS` sin definir, `AVI_ALLOW_CWD_VENDOR` sin definir.
- Las capturas de «Resultado observado» de las ramas 2-4 se hicieron redirigiendo `stdout` y `stderr` a ficheros separados (por eso no se ve el pegado entre canales, que solo se manifiesta en TTY).
- La captura de la rama 1 procede de quien detectó el defecto (2026-10-19): su propio entorno, con terminal interactiva, daemon arrancado y modelos provisionados. No es de las pruebas de este repositorio, que no tienen el TTS provisionado.

## Precondiciones

Ninguna en particular. Basta con ejecutar los comandos tal cual; los que usan micrófono requieren un dispositivo de entrada y una terminal interactiva, y los que usan el daemon requieren que responda en `127.0.0.1:8765`.

## Pasos para reproducir

**Rama 1 — captura del micrófono con la fase abierta (TTY):**

1. Con el daemon arrancado y un dispositivo de entrada disponible:

```powershell
ai-voice-interconnector speech dub --mic --source-language es --target-language en --voice default --daemon
```

2. Hablar y pulsar Enter para terminar la grabación.

**Rama 2 — error local sin cierre de la fase (no-TTY, sin modelos):**

3. `speech say` con el motor TTS sin provisionar:

```powershell
target\debug\ai-voice-interconnector.exe speech say --text hola --voice default --no-daemon
```

4. `voice clone` con una referencia inexistente:

```powershell
target\debug\ai-voice-interconnector.exe voice clone --name mi-voz-probe --speech-reference C:\no\existe\ref.wav --no-daemon
```

5. `speech synthesize` con una guarda de uso que se comprueba después de pintar la fase:

```powershell
target\debug\ai-voice-interconnector.exe speech synthesize --text hola --label tmp-progress-probe --voice default --play --no-daemon
```

6. La misma guarda en `--json` (el error sale por `stdout` como envelope):

```powershell
target\debug\ai-voice-interconnector.exe speech synthesize --text hola --label tmp-progress-probe --voice default --play --json --no-daemon
```

**Rama 3 — operación de ciclo de vida con la fase antes de la confirmación:**

7. Simulación de limpieza:

```powershell
target\debug\ai-voice-interconnector.exe cleanup --dry-run --all
```

8. Comprobación de actualización:

```powershell
target\debug\ai-voice-interconnector.exe self update --check
```

**Rama 4 — menú interactivo con la fase abierta (TTY):**

9. `speech synthesize --play` en un entorno con el motor TTS provisionado y una voz existente:

```powershell
ai-voice-interconnector speech synthesize --text hola --label tmp --voice default --play
```

## Resultado observado

**Rama 1 (TTY, salida completa en orden, tal cual la reportó quien detectó el defecto):**

```text
… Doblando audio (daemon)… (0 s)Grabando… pulsa Enter para detener.

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

La primera línea es la prueba del defecto: el aviso de la captura se dibuja **a continuación** de la fase abierta, en la misma línea física. La línea en blanco que sigue al aviso se explica por el Enter que el terminal devuelve como eco al cerrar el push-to-talk (probable, no verificada sin terminal). Las dos repeticiones por segundo de `Sintetizando voz…` son los latidos `heartbeat` del daemon (~500 ms), y el `Procesando…` inicial es un evento `progress` del daemon, que el CLI traduce siempre a «Procesando…» sin mirar su `stage` (`emit_ndjson_progress`, en `src/main.rs`).

**Rama 2 (no-TTY, `stderr` y `stdout` por separado):**

`speech say` (`stderr`, salida 1):

```text
… Validando texto y voz… (0 s)
Aviso: se ignoró 'vendor/qwen3-tts/qwen_tts.exe' del directorio de trabajo: define AVI_ALLOW_CWD_VENDOR=1 para habilitarlo.
… Sintetizando voz… (0 s)
… Sintetizando voz… (0 s)
Aviso: se ignoró 'vendor/qwen3-tts/qwen_tts.exe' del directorio de trabajo: define AVI_ALLOW_CWD_VENDOR=1 para habilitarlo.
Error: El binario Qwen3-TTS no está provisionado.
```

`voice clone` (`stderr`, salida 3):

```text
… Validando referencia y nombre… (0 s)
Error: El audio de referencia 'C:\no\existe\ref.wav' no existe.
```

`speech synthesize --play` (`stderr`, salida 2):

```text
… Validando texto y voz… (0 s)
Error: --play requiere una terminal interactiva (TTY).
```

`speech synthesize --play --json` (`stderr`, salida 2) y (`stdout`):

```text
# stderr
… Validando texto y voz… (0 s)
```

```json
{
  "error": "--play es incompatible con --json.",
  "reason": "usage_error",
  "schema_version": "5"
}
```

En ninguno de estos casos aparece una línea `[ok] …` o `[error] …`: la fase no se cierra. En TTY, además, la línea `Error:` o el envelope se escribirían pegadas a `… Validando texto y voz… (0 s)`.

**Rama 3 (no-TTY):**

`cleanup --dry-run --all` (`stderr`, salida 0):

```text
… Limpiando datos… (0 s)
… Limpiando datos… (0 s)
Se borrarán 6 ruta(s), 5,9 GB en total:
  modelos: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\cache\models (5,9 GB)
  locuciones: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\data\speech (946,0 KB)
  registros: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\data\logs (0 B)
  estado del daemon: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\data\daemon.pid (329 B)
  estado del daemon: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\data\daemon.ready (99 B)
  temporales: C:\Users\Cristian\AppData\Local\ai-voice-interconnector\cache\tmp (0 B)
  no se tocará C:\Users\Cristian\AppData\Local\Programs\ai-voice-interconnector: el programa y la integración de PATH son de `self uninstall`
Simulación: no se ha modificado nada.
```

Sin cierre de progreso, y en TTY la primera línea del plan (`Se borrarán …`) se pegaría a `… Limpiando datos… (0 s)`.

`self update --check` (`stderr`, salida 0) y (`stdout`):

```text
# stderr
… Buscando actualizaciones… (0 s)
```

```text
# stdout
Ya está actualizado (0.34.0).
```

Sin cierre de progreso; en TTY el mensaje de `stdout` se pegaría a la fase abierta.

**Rama 4 (TTY):** el menú y el prompt de `synthesize_play_loop` se escriben con `eprintln!`/`eprint!` mientras la fase `… Sintetizando voz… (0 s)` sigue abierta, con lo que la primera línea del menú queda pegada a la fase y el prompt `Opción [1-4]: ` deja el cursor a mitad de línea.

## Resultado esperado

- `DESIGN.md`, «Salida en terminal» → «Bloques»: *«Cada bloque que no es el primero de la salida va precedido de una línea en blanco, y dentro de un bloque no hay ninguna»*. Una línea de progreso y el mensaje de producto que la sigue son bloques distintos y no deben compartir línea física.
- `DESIGN.md`, «Salida en terminal» → «Prompt»: *«Termina con un espacio, para que la respuesta no quede pegada: `¿Continuar? [S/n] ` … Lo escribe solo `confirm::confirm`»*. El principio es el mismo: nadie debe quedar pegado a lo que otro ha escrito.
- `avi-lifecycle/src/progress.rs`, `PhaseProgress::finish`: *«Cierra el indicador con una línea final de estado (éxito o fallo)»*. La fase termina con cierre explícito, sin mensaje de estado; el mensaje final del handler lleva la duración, sea cual sea el camino de salida.
- `avi-lifecycle/src/uninstall.rs`, `uninstall::run`, comentario sobre la creación del indicador: *«El indicador nace tras la confirmación destructiva: ni `--dry-run` (paso 3) ni la cancelación emiten progreso»*. Es el patrón correcto y el que ya sigue un comando del producto; `cleanup` y `self update` no lo siguen.
- `docs/CLI/CONTRACT.md`, «stderr, trazas y logs»: *«stderr solo lleva mensajes del producto y avisos, nunca trazas internas»* y, del progreso de descarga de modelos, *«es un mensaje del producto, no una traza: sale siempre por stderr»*. La cita de «mensaje del producto» está redactada para `setup`/`self install`, pero el precepto es el mismo para el progreso de fase: es un mensaje del producto que tiene que convivir en `stderr` con los avisos sin ensuciarse.

## Impacto y workaround

- Afecta a cualquier persona que use la CLI en una terminal: la lectura del progreso se vuelve confusa y los mensajes de error quedan pegados a la fase, lo que retrasa el diagnóstico. En `speech synthesize`/`speech say`/`voice clone` locales es sistemático en todo error.
- Afecta también a las capturas de pantalla y al copiado/pega de la salida: la línea pegada se copia como una sola línea larga.
- Workaround: `AVI_NO_PROGRESS=1` silencia toda la salida del indicador (implementado en `PhaseProgress::new` y en `emit_ndjson_progress`) y deja la salida limpia, pero elimina también la información de progreso. La variable no está documentada en `USAGE.md` ni en `docs/`.

## Evidencia

- Salidas literales de las cuatro ramas. La rama 2-4 se capturaron el 2026-10-19 con el binario de desarrollo de 0.34.0 (ver «Resultado observado»); la rama 1 es la salida literal del doblaje con `--daemon` que reportó la persona que detectó el defecto.
- `crates/avi-audio/src/lib.rs`, `capture_16k_mono_pcm_until_enter`: escribe el aviso de grabación y el aviso del techo `AVI_PUSH_TO_TALK_MAX_SECS` con `eprintln!` mientras el caller mantiene la fase abierta.
- `crates/avi-lifecycle/src/cleanup.rs`, `run`: imprime el plan, el prompt de confirmación y el aviso de simulación con `eprintln!` mientras `handle_cleanup` mantiene la fase `Limpiando datos…` abierta.
- `src/main.rs`, `synthesize_play_loop`: imprime las opciones y `eprint!("Opción [1-4]: ")` mientras el handler `SpeechCommands::Synthesize` mantiene la fase `Sintetizando voz…` abierta.
- `crates/avi-tts/src/lib.rs`, `resolve_binary`: avisa dos veces por invocación (una desde `Qwen3TtsEngine::new` y otra desde `Qwen3TtsResident::spawn`), siempre con una fase abierta.

## Análisis de causa

**Causa raíz (confirmada).** `PhaseProgress` pinta una línea **sin terminarla** (`\r\x1b[2K… {mensaje} ({t})` en `PhaseProgress::render_ref`) y no reclama la consola de forma exclusiva: el indicador no es dueño de la línea y ningún protocolo obliga a cerrar la línea antes de ceder la escritura a otro sujeto. Además, muchos caminos de salida no cierran la fase de forma explícita. El resultado es que cualquier `eprintln!`/`println!` posterior se escribe a continuación de la misma línea física.

**Fichas por síntoma** (una causa común, varias manifestaciones):

- **Ficha A — la captura del micrófono se pega a la fase abierta.** `avi-audio`, `capture_16k_mono_pcm_until_enter`, aviso «Grabando… pulsa Enter para detener.» y aviso del techo de grabación. Confirmada por la salida de la rama 1.
- **Ficha B — casi ningún error de la rama local cierra la fase.** `src/main.rs`: los handlers de `speech synthesize`, `speech say`, `speech transcribe`, `speech dub` y `voice clone` devuelven errores con `?` (validación de texto, `voice_not_found`, `audio_not_found`, `model_missing`, `require_model_provisioned`, `open_voice_store`, `route_to_daemon`, `synthesize_voice_directly`, `acquire_pcm`, `ensure_pcm_within_limit`, `direct_tts_engine`) sin llamar a `finish()`. Confirmada por las capturas de las ramas 2 y 3. Contrasta con las ramas daemon de los mismos comandos, que sí cierran en un `match` sobre el resultado.
- **Ficha C — el menú interactivo se pega a la fase abierta.** `src/main.rs`, `synthesize_play_loop`: `eprintln!` de las opciones y `eprint!` del prompt. Confirmada por inspección; visible en `speech synthesize --play`.
- **Ficha D — `cleanup` y `self update` pintan la fase antes de la confirmación.** `src/main.rs`, `handle_cleanup` y el comando `SelfCommands::Update` crean el indicador antes de llamar a `confirm::confirm`, cuyo plan, prompt y aviso de simulación se escriben a `stderr`. `uninstall::run` ya sigue el patrón inverso. Confirmada por las capturas de la rama 3.
- **Ficha E — los avisos internos de los motores se pegan a la fase abierta.** `avi-tts`, `resolve_binary` (aviso de `vendor/` del directorio de trabajo); `avi-lifecycle`, `privileges::ensure_per_user` (aviso por stderr) y `install::install` (fallos de la cuarentena); `avi-process` y `avi-shared` (avisos de borrado diferido). Confirmada por la salida de `speech say`.
- **Ficha F — el cierre por `stdout` se pega a la fase abierta.** `self update --check` y `self update` ya actualizado escriben el resultado por `stdout` sin cerrar antes la fase de `stderr`. Confirmada por la captura de la rama 3; es la única ficha que cruza canales, y por eso solo es visible en TTY.
- **Ficha G — `self install`/`self uninstall` dejan fases abiertas en algunos caminos.** `install::install` (fallos de la cuarentena, avisos de privilegios) y `uninstall::run` (los `eprintln!` de «no se pudo borrar»/«no se pudo revertir el PATH» ocurren **entre** dos `phase()`, con la fase anterior todavía abierta). Probable: no se reprodujo en el entorno de captura por ser destructivo.

**Lo que ya se descartó.** No es un problema de búfer de `stdout`: en TTY ambas salidas llegan sin búfer y el síntoma es de escritura a la misma línea, no de orden de vaciado. No es un problema del eco del terminal: el aviso de la rama 2 y el de `resolve_binary` se pegan a la fase incluso cuando el texto no lleva Enter asociado.

## Diagnóstico sugerido

Dirección de la corrección (ya confirmada la causa):

1. Que `PhaseProgress` sea propietario exclusivo de la línea: cerrarla (`\n`) siempre que se ceda la escritura, o bien repintar la fase después de cada aviso ajeno. El cierre de las ramas de error se hará de forma explícita, con el mismo cierre que el resto de comandos; queda como **alternativa descartada** un `impl Drop for PhaseProgress` que pinte el cierre si todavía hay una fase abierta: el cierre será explícito, sin `Drop`.
2. En `cleanup` y `self update`, mover la creación del indicador a **después** de la confirmación, igual que ya hace `uninstall::run`.
3. En `synthesize_play_loop`, cerrar la fase (o no abrirla hasta elegir «guardar») antes de imprimir el menú.
4. Centralizar los avisos de `avi-tts` y de los motores para que salgan a través del indicador o al menos precedidos de un cierre de línea.

Alternativa descartada por ahora: imprimir la fase con `\n` siempre (perdería el repintado en TTY, que es justo lo que hace legible el indicador).

## Criterio de aceptación

- En TTY, ningún mensaje de producto, menú, prompt u `Error:` aparece a continuación de la línea de una fase en la misma línea física: entre ambos hay siempre un fin de línea.
- Todo camino de salida de un comando con indicador termina con un cierre explícito, incluidos los errores devueltos con `?` y los retornos anticipados (`--dry-run`, `--check`, cancelado). El cierre no imprime estado; el mensaje final del handler lleva la duración.
- Pruebas de regresión que deben existir:
  - Un test de salida que ejecute `speech synthesize --play --json`, `voice clone` con referencia inexistente, `cleanup --dry-run --all` y `self update --check` con `stdout`/`stderr` redirigidos y verifique (a) que ninguna línea empieza por `… ` sin cerrarse en otra línea, (b) que toda fase abierta recibe un cierre, (c) que ningún mensaje de producto comparte línea con una fase. Esta comprobación de contenido no-TTY **no verifica secuencias de terminal**: no demuestra que no haya pegado en TTY, porque al redirigir a fichero no se emiten ni se ven los `\r` y `\x1b[2K`. La prueba de no-pegado en TTY va aparte y requiere un PTY o un doble TTY.
  - Un test de `PhaseProgress` que, con un doble TTY, verifique que el cierre termina la línea aunque haya habido escrituras intermedias de terceros (por ejemplo, un `eprintln!` entre `phase()` y el cierre).
  - El test manual de `MANUAL-VALIDATION.md` §9 debe pasar sin pegado en el push-to-talk de `speech dub --mic`.

## Relacionados

- `progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md` (misma familia de indicadores; distinto síntoma y distinta corrección).
- `los-cierres-y-los-relojes-del-progreso-se-duplican.md` (los cierres duplicados que se ven en la rama 1).
- `CHANGELOG.md`, 0.34.0: «progreso de fases por stderr, una sola línea por terminal» — la corrección de este defecto debe convivir con esa entrada.
- `docs/MANUAL-VALIDATION.md` §9 «Rutas interactivas de audio, push-to-talk y `--play`».
