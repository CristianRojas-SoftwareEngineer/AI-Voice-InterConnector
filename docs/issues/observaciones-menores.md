# Observaciones menores del indicador de progreso

Defectos menores sueltos de la misma familia que los documentos de este directorio. Se recogen aquí hasta que crezcan lo bastante para tener documento propio (ver `README.md`, «Convenciones»).

| # | Síntoma | Severidad | Confianza |
|---|---|---|---|
| 1 | `AVI_NO_PROGRESS` está implementado pero no documentado | baja | confirmada |
| 2 | Varios comandos no emiten ninguna fase de progreso | baja | confirmada |
| 3 | `daemon restart` y `start` con daemon degradado tardan en mostrar progreso | baja | confirmada |
| 4 | `self update` muestra la etiqueta «Descargando…» durante fases que no son la descarga | baja | confirmada |
| 5 | `speech say` y `speech play` anuncian la reproducción cuando ya ha terminado | baja | confirmada |
| 6 | El aviso del binario del motor se emite dos veces por invocación | baja | confirmada |
| 7 | `PhaseProgress` no abre bloque con la línea en blanco que exige el diseño | baja | confirmada |
| 8 | `speech transcribe` local no cierra la fase si el motor STT no se puede inicializar | media | confirmada |
| 9 | `faults::trip` puede dejar una fase abierta (solo con el feature `faults`) | baja | confirmada |
| 10 | La línea en blanco tras el aviso del push-to-talk procede del eco del Enter | baja | probable |

---

## 1. `AVI_NO_PROGRESS` está implementado pero no documentado

`PhaseProgress::new` (en `avi-lifecycle/src/progress.rs`) y `emit_ndjson_progress` (en `src/main.rs`) silencian toda la salida de progreso cuando la variable `AVI_NO_PROGRESS` tiene cualquier valor no vacío. La regla está documentada solo en el comentario del código: no aparece en `USAGE.md`, en `docs/CLI/CONTRACT.md` (sección «stderr, trazas y logs») ni en las páginas de comando de `docs/CLI/commands/`. Quien necesite silenciar el indicador (por ejemplo, en un pipeline) no tiene forma de descubrirlo. Es también el workaround natural de los otros defectos de este directorio.

**Qué haría falta:** documentar la variable junto a las demás `AVI_*` (están documentadas `AVI_PUSH_TO_TALK_MAX_SECS` y `AVI_ALLOW_CWD_VENDOR`), indicando que afecta solo a stderr, que no cambia el envelope `--json` ni los códigos de salida.

## 2. Varios comandos no emiten ninguna fase de progreso

`doctor` sin `--repair`, `daemon status`, `voice list`, `speech list`, `devices` y `version` no crean `PhaseProgress`. Para los que son instantáneos no hace falta. `daemon status` sí puede tardar: hace un único `GET /health` con presupuesto de 500 ms para la conexión y hasta 800 ms para el cuerpo JSON (del orden de 1,3 s en el peor caso) sin emitir nada mientras tanto. `setup` solo emite progreso cuando hay modelos que descargar (`ProvisionView`), no para las comprobaciones, la purga ni la confirmación, que son pasos que también pueden tardar.

**Qué haría falta:** decidir si algún comando de esta lista necesita fase propia; si no, dejarlo así y no documentarlo como defecto.

## 3. `daemon restart` y `start` con daemon degradado tardan en mostrar progreso

En `DaemonCommands::Restart`, y en `Start` cuando el daemon responde degradado, la parada del daemon (`daemon_stop::stop`) se ejecuta **antes** de crear el indicador, con lo que hay varios segundos de silencio antes de que aparezca «… Deteniendo daemon…» o «… Iniciando daemon…». Contrasta con `DaemonCommands::Stop`, que sí crea el indicador antes de parar.

**Qué haría falta:** crear el indicador antes de la parada en los dos caminos, reutilizando la fase `stopping`.

## 4. `self update` muestra la etiqueta «Descargando…» durante fases que no son la descarga

El comando `SelfCommands::Update` pinta `… Descargando versión X…` y después ejecuta la descarga, la verificación del binario extraído, la parada del daemon y el traspaso al `self install` interno. Solo la primera de esas etapas es la descarga; el usuario ve «Descargando» mientras el daemon se detiene.

**Qué haría falta:** pintar una fase por paso (`downloading`, `verifying`, `stopping`, `replacing`) como ya hace `self uninstall`.

## 5. `speech say` y `speech play` anuncian la reproducción cuando ya ha terminado

Tras cerrar el indicador con «[ok] Reproducción completada. (N s)», el handler escribe por stdout «Reproduciendo.» (`say`) o «Reproduciendo locución '<etiqueta>' de la voz '<voz>'.` (`play`). El texto está en presente y describe una acción que ya ha terminado, además de repetir el estado que acaba de informar el cierre.

**Qué haría falta:** redactar el mensaje de resultado en pasado («Locución reproducida.») o eliminarlo, reservando el estado al cierre del indicador.

## 6. El aviso del binario del motor se emite dos veces por invocación

`avi-tts`, `resolve_binary`, avisa cuando encuentra `vendor/qwen3-tts` en el directorio de trabajo sin `AVI_ALLOW_CWD_VENDOR=1`. En un comando local de síntesis se llama dos veces: una desde `Qwen3TtsEngine::new` (creación del motor) y otra desde `Qwen3TtsResident::spawn` (arranque del residente), de modo que el mismo aviso sale dos veces en la misma ejecución. Capturado con `speech say --text hola --voice default --no-daemon`.

**Qué haría falta:** memorizar la decisión por proceso (una única vez por ejecución) o emitir el aviso solo desde la capa que decide no usar el binario.

## 7. `PhaseProgress` no abre bloque con la línea en blanco que exige el diseño

`DESIGN.md`, «Salida en terminal» → «Bloques»: *«Cada bloque que no es el primero de la salida va precedido de una línea en blanco»*. `ProvisionView::start` lo hace (`eprintln!()` antes del encabezado); `PhaseProgress` no, de modo que la fase se pega visualmente al bloque anterior cuando el anterior terminó sin línea en blanco propia. En `cleanup` y `self update` esto se junta con el hecho de que la fase se pinta antes de la confirmación.

**Qué haría falta:** que el primer `phase()` de un indicador abra con una línea en blanco, como ya hace la vista de provisión.

## 8. `speech transcribe` local no cierra la fase si el motor STT no se puede inicializar

En la rama local de `SpeechCommands::Transcribe`, el error de `STT_HOLDER.get_or_try_init` se devuelve con `?` sin llamar a `progress.finish(false, …)`, con lo que la fase «… Transcribiendo audio… (N s)» queda abierta. La rama local de `SpeechCommands::Dub` sí cierra la fase en el mismo error. Es también un caso de la ficha B del documento principal de fases abiertas, pero conviene señalar la asimetría entre los dos handlers.

**Qué haría falta:** alinear el cierre con el del resto de comandos, con cierre explícito y mensaje único del handler.

## 9. `faults::trip` puede dejar una fase abierta (solo con el feature `faults`)

Los puntos de fallo inyectado (`avi-lifecycle/src/faults.rs`, `trip`) devuelven error con `?` en rutas que tienen el indicador abierto (`install`, entre otras). Sin el feature `faults`, `is_armed` devuelve siempre `false`, así que el binario distribuido no puede provocarlo (garantizado por el comentario del propio módulo). Solo importa para los builds de prueba.

**Qué haría falta:** nada mientras el cierre de `PhaseProgress` sea explícito en cada camino de salida.

## 10. La línea en blanco tras el aviso del push-to-talk procede del eco del Enter

En la salida observada de `speech dub --mic --daemon` hay una línea en blanco entre el aviso «Grabando… pulsa Enter para detener.» y el resto del progreso. La explicación más simple es que el terminal devuelve el Enter como eco cuando el usuario lo pulsa para terminar la grabación, y en ese momento el cursor ya estaba al principio de una línea nueva (el aviso termina con `\n`). Es una observación de terminal, no de código: no se ha verificado reproduciéndola con un doble.

**Qué haría falta:** reproducirla en una terminal de verdad antes de decidir si es un defecto; si lo fuera, el arreglo es el mismo que el del documento de fases abiertas (cerrar la línea antes de ceder la consola).

---

Las soluciones de esta tabla se aplican con cierre explícito; las alternativas con `Drop` quedan descartadas.

Relacionados: `fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md`, `progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md`, `la-rama-local-y-la-del-daemon-muestran-fases-y-cierres-distintos.md`, `los-cierres-y-los-relojes-del-progreso-se-duplican.md`.
