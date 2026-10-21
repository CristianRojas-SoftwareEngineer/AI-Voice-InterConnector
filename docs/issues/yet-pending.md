# `daemon start` tarda unos 8 segundos en el ejecutor Windows de CI y rompe su propio plazo de 5 s

| Campo | Valor |
|---|---|
| Estado | en diagnóstico |
| Severidad | media |
| Tipo | rendimiento |
| Componente | `src/main.rs` (lanzador de `daemon start`), `crates/avi-daemon` (hijo `daemon serve`) |
| Versión detectada | 0.36.0 |
| Plataforma | Windows Server 2022, ejecutor CircleCI `win/server-2022`. En el pipeline donde se observó el fallo, los jobs Linux y macOS pasaron esas mismas pruebas en 3.547 s y 0.271 s (caso de puerto y caso de voz) |
| Reproducibilidad | no caracterizada: el fallo se observó una sola vez, en 1 de 5 ejecuciones completas de la suite en el ejecutor Windows, y esa ejecución no llevaba instrumentación. Con instrumentación, 0 de 4 |
| Detectado en | pipeline de release de la 0.36.0, 2026-10-21 |

## Resumen

En el ejecutor Windows de CircleCI, dos pruebas de contrato de `daemon start` tardaron
8.774 s y 8.912 s y incumplieron su propio plazo de 5 s, con lo que el pipeline de
release de la 0.36.0 quedó en rojo y la release no se publicó. El error funcional que
devuelven las pruebas es correcto (`port_in_use` con salida 6 y `voice_not_found` con
salida 3): el defecto es que el fallo tarda en llegar, no que el fallo sea equivocado.

La instrumentación de fases no ha reproducido el defecto: en cuatro corridas
completas de la suite, en dos pipelines de diagnóstico, las mismas pruebas terminaron
siempre por debajo de su plazo (4.686 s, 4.687 s, 4.713 s y 4.909 s para el caso de
puerto; 0.839 s a 0.886 s para el de voz). En esas corridas la traza muestra que todo el
trabajo del hijo ocurre en los primeros 25 ms y que el resto es espera del lanzador.

**La causa raíz está sin determinar.** Lo que falta es una ejecución que reproduzca el
defecto **con la traza por fases activada**: la única ejecución que falló no la
tenía, de modo que no se sabe en qué fase se fue el tiempo. El solapamiento temporal
con otro job del mismo pipeline es un indicio, no una causa: los jobs de CircleCI se
ejecutan en ejecutores aislados y no hay evidencia de que compartieran máquina ni
recursos.

## Entorno

- CircleCI, executor `win/server-2022`, `resource_class: medium`.
- Suite completa: `cargo nextest run --all` (nextest 0.9.148 pineado).
- Pipeline 459 (release de la 0.36.0, tag `v0.36.0`): el job `test-windows` #2341
  corrió de 15:02:03 a 15:12:41. En ese pipeline corrieron además
  `test-bootstrap-windows` #2342 (de 15:02:37 a 15:07:53) y `test-macos` #2344.
  Los cuatro `build-*` y los dos `publish-*` quedaron en `not_run`: no se publicó
  nada.
- En los pipelines de diagnóstico 463 y 464 corrió únicamente `test-windows`
  (#2359 y #2360), ambos en verde.
- Modelos: la suite siembra un modelo TTS falso en la raíz de caché de cada sandbox
  (`seed_pinned_models`), de modo que el arranque falla antes de cargar motores.

## Precondiciones

- Ningún daemon registrado en la instancia de la prueba: el pidfile señuelo contiene
  un PID muerto y deja el estado en `Stopped`, no `Degraded`.
- El puerto de la instancia ocupado por un listener propio de la prueba (caso
  `port_in_use`).

## Pasos para reproducir

La condición que dispara el defecto **no se ha establecido**, así que el paso 3 es
iterativo por definición. No hay aquí ningún paso que requiera publicar un tag: el
diagnóstico no necesita la cadena de release.

1. Mantener la instrumentación por fases en las pruebas de `daemon start` de la suite
   Windows ordinaria y ejecutar `cargo nextest run --all` como parte del job
   `test-windows`, sin provocar carga adicional ni repetir el pipeline para buscar
   este fallo.
2. Cuando vuelva a aparecer una ejecución lenta, conservar la traza asociada a la
   prueba fallida y el volcado del job (`when: always`), incluso si el sandbox de la
   prueba ya se ha eliminado.
3. Localizar el tramo de tiempo grande entre dos marcas consecutivas y contrastarlo
   con la tabla de discriminantes antes de decidir una corrección.

## Resultado observado

En el job `test-windows` #2341 del pipeline 459, con salida literal del log de
nextest:

```
    FAIL [   8.934s] ( 81/919) ai-voice-interconnector::cli_golden tts::daemon_start_port_in_use_fails_fast_with_state_conflict
    FAIL [   9.164s] ( 82/919) ai-voice-interconnector::cli_golden tts::daemon_start_missing_warm_voice_exits_not_found
    PASS [  10.193s] ( 83/919) ai-voice-interconnector::cli_golden tts::daemon_start_failure_is_written_to_daemon_log
    PASS [  10.823s] ( 84/919) ai-voice-interconnector::cli_golden tts::daemon_start_reclaims_unresponsive_registered_daemon
    PASS [   0.300s] ( 79/919) ai-voice-interconnector::cli_golden tts::daemon_serve_port_in_use_exits_state_conflict
```

Y el detalle de la aserción incumplida, tal cual aparece en el log:

```
    thread 'tts::daemon_start_port_in_use_fails_fast_with_state_conflict' panicked at tests\cli_golden.rs:5211:9:
    el fallo debe ser inmediato, no esperar el deadline: 8.7743121s
```

```
    thread 'tts::daemon_start_missing_warm_voice_exits_not_found' panicked at tests\cli_golden.rs:5395:9:
    el fallo debe ser inmediato: 8.9119569s
```

El `reason` del envelope JSON fue correcto en ambos casos (`port_in_use` y
`voice_not_found`), y los códigos de salida fueron 6 y 3, como establece el contrato.

El valor `8.7743121s` que reporta la aserción es el de **una sola invocación** del
binario (`elapsed` medido alrededor de la llamada a la CLI). El `8.934s` que nextest
atribuye a la prueba es el total de la prueba, que en el caso que fallóaborta en la
primera aserción, antes de la segunda invocación y de la ventana de huérfanos.

## Resultado esperado

Las dos pruebas afirman que el fallo por puerto ocupado y el fallo por voz inexistente
son inmediatos, con un plazo de 5 s por invocación. La fuente es el propio enunciado
de las pruebas: `assert_start_port_in_use` afirma `elapsed < Duration::from_secs(5)`
porque `daemon start` debe fallar antes de agotar el plazo de publicación de la
dirección (`DAEMON_READY_DEADLINE`, 10 s, en `src/main.rs`), y el comentario de la
prueba explica que una espera de 10 s sería la regresión que se quiere evitar.

## Impacto y workaround

El impacto es sobre la **puerta de release**, no sobre el usuario: el pipeline de la
0.36.0 quedó en rojo y la release no se publicó, aunque el producto funcione
correctamente. Un usuario que ejecute `daemon start` con el puerto ocupado o con una
voz inexistente puede esperar varios segundos de más antes de recibir el error, pero
el error y su código son correctos.

Workaround: reintentar el pipeline. El defecto no se ha reproducido de forma
determinista.

## Evidencia

### Comparación de los jobs Windows

Los cuatro tests de `daemon start` que se degradaron, con su duración en el job verde
anterior (#2320), en el que falló (#2341) y en los dos pipelines de diagnóstico
(#2359 y #2360, dos corridas completas cada uno, todas verdes):

| Prueba | #2320 verde | #2341 fallo | #2359 verde | #2360 verde |
|---|---|---|---|---|
| `daemon_start_port_in_use_fails_fast_with_state_conflict` | 4.700 s | **8.934 s** | 4.686 / 4.909 s | 4.713 / 4.687 s |
| `daemon_start_missing_warm_voice_exits_not_found` | 0.858 s | **9.164 s** | 0.849 / 0.886 s | 0.859 / 0.839 s |
| `daemon_start_failure_is_written_to_daemon_log` | 0.861 s | **10.193 s** | 0.842 / 0.881 s | 0.864 / 0.848 s |
| `daemon_start_reclaims_unresponsive_registered_daemon` | 1.976 s | **10.823 s** | 1.935 / 2.057 s | 1.973 / 1.932 s |

Los cuatro son los **consecutivos** de la suite (índices 81 a 84) y los cuatro se
degradaron a la vez, del orden de 0.9 s a unos 10 s.

**El 4.7 s del caso de puerto no es el tiempo de una invocación.** Esa prueba hace
dos invocaciones de `daemon start` (con y sin `--auto-restart`) y después una
ventana de 3 s en la que comprueba que no aparece un daemon huérfano
(`assert_no_orphan_launch`). Medido con la traza, el reparto de los 4.7 s es:

```
  invocación #1                                772 ms
  invocación #2                                781 ms
  ventana de comprobación de huérfanos       3000 ms
  -----------------------------------------------
  suma                                       4553 ms
  resto (montaje de la instancia)             147 ms
```

En el caso que falló, en cambio, la aserción saltó en la primera invocación: 8.774 s
es el tiempo de **una** invocación, 11.4 veces la de una invocación sana (772 ms). La
diferencia no está en el reparto de los 4.7 s, sino en que una sola invocación pasó
de 0.8 s a 8.8 s.

### Cronología por fases, medida con instrumentación temporal

La instrumentación (marcas monotónicas y de pared con PID) se aplicó al lanzador
(`start.enter`, `start.model_provisioned`, `start.health_probe_done`,
`start.observe_begin/done`, `launch_daemon.enter`, `.ready_path_prepared`,
`.log_created`, `.child_spawned`, `.log_renamed`, `.failed_signal_read`,
`abort_launch.enter/done`) y al hijo (`child.serve_enter`,
`child.prepare_server.enter`, `.voices_initialized`, `.warm_voice_validated`,
`.warm_voice_missing`, `.bind_failed_port_in_use`, `.failure_published`).

En el pipeline de diagnóstico, las dos invocaciones del caso de puerto de la primera
corrida, con `epoch_ms` de pared y `mono_ms` relativo al inicio de cada proceso:

```
epoch_ms=1792605512092 mono_ms=0    pid=6468  start.enter
epoch_ms=1792605512092 mono_ms=0    pid=6468  start.model_provisioned
epoch_ms=1792605512607 mono_ms=514  pid=6468  start.health_probe_done
epoch_ms=1792605512607 mono_ms=515  pid=6468  start.observe_begin
epoch_ms=1792605512607 mono_ms=515  pid=6468  start.observe_done
epoch_ms=1792605512608 mono_ms=515  pid=6468  launch_daemon.enter
epoch_ms=1792605512608 mono_ms=515  pid=6468  launch_daemon.ready_path_prepared
epoch_ms=1792605512609 mono_ms=517  pid=6468  launch_daemon.log_created
epoch_ms=1792605512611 mono_ms=519  pid=6468  launch_daemon.child_spawned
epoch_ms=1792605512612 mono_ms=520  pid=6468  launch_daemon.log_renamed
epoch_ms=1792605512638 mono_ms=0    pid=4812  child.serve_enter
epoch_ms=1792605512638 mono_ms=0    pid=4812  child.prepare_server.enter
epoch_ms=1792605512647 mono_ms=9    pid=4812  child.voices_initialized
epoch_ms=1792605512648 mono_ms=10   pid=4812  child.warm_voice_validated
epoch_ms=1792605512649 mono_ms=11   pid=4812  child.bind_failed_port_in_use
epoch_ms=1792605512650 mono_ms=12   pid=4812  child.failure_published
epoch_ms=1792605512864 mono_ms=771  pid=6468  launch_daemon.failed_signal_read
epoch_ms=1792605512864 mono_ms=772  pid=6468  abort_launch.enter
epoch_ms=1792605512864 mono_ms=772  pid=6468  abort_launch.done
```

Lectura de la cronología, **en las ejecuciones que pasaron**:

- Los intervalos consecutivos de esta invocación son: 0 ms entre `start.enter` y
  `start.model_provisioned`; **515 ms** entre `start.model_provisioned` y
  `start.health_probe_done`; 0 ms en los dos tramos de `observe`; 1 ms hasta
  `launch_daemon.enter`; 4 ms en total desde ahí hasta
  `launch_daemon.log_renamed`; **26 ms** hasta la primera marca del hijo
  (`child.serve_enter`); 12 ms de trabajo del hijo
  (`child.serve_enter` → `child.failure_published`); **214 ms** entre
  `child.failure_published` y `launch_daemon.failed_signal_read`; 0 ms en la
  limpieza.
- El tramo de 515 ms es el sondeo de `/health` al puerto que responde el pidfile
  señuelo, con un tiempo de espera de 500 ms (`probe_health` en `src/main.rs`). Es
  un coste fijo, presente también en los jobs verdes, y no explica los 8 s.
- El total de la invocación, de `start.enter` a `abort_launch.done`, es de **772 ms**.

Ninguna de las seis invocaciones medidas muestra un tramo de 8 s: la más lenta es de
813 ms. **La fase lenta no llegó a materializarse en ninguna ejecución instrumentada**,
porque el defecto no se reprodujo. Lo único que estas mediciones acreditan es el
reparto del tiempo **cuando el caso pasa**; no dicen nada sobre lo que ocurrió en la
única ejecución que falló, porque en ella no había traza.

### Observaciones del job que falló, sin traza

- `test-bootstrap-windows` #2342 (otro job Windows del mismo pipeline) corrió de
  15:02:37 a 15:07:53, es decir **316 s de solapamiento temporal** con `test-windows`
  #2341. En los pipelines de diagnóstico 463 y 464 no había otro job Windows y ambos
  pasaron. Esto es una **correlación temporal**, no una causa: los jobs de CircleCI se
  ejecutan en **ejecutores aislados**, y nada en la evidencia disponible indica que
  #2341 y #2342 compartieran máquina, CPU, disco ni red. Ejecutar dos jobs Windows en
  paralelo no garantiza por sí solo que estén en el mismo host, de modo que este
  solapamiento **no puede usarse como condición de reproducción**.
- El job que falló compila desde cero: 264 líneas `Compiling` frente a 12 en el job
  verde #2320, es decir, la caché de `target/` no se restauró. En los pipelines de
  diagnóstico también hubo 264 líneas `Compiling`, así que la compilación desde cero
  no es por sí sola el factor: está presente en los jobs verdes y en el que falló.
- `daemon_serve_port_in_use` tardó 0.300 s en el job que falló, frente a 0.093 s y
  0.125 s en los verdes. Es una desviación pequeña y en la dirección esperada por el
  ruido del ejecutor; **no** acredita que el ejecutor no estuviera saturado, porque
  ese caso ejecuta el hijo en primer plano sin el lanzador y sin el intercambio por
  fichero `daemon.ready`, y su trabajo es incomparable con el de los cuatro tests
  degradados.
- Los casos `daemon_stop` y `daemon_restart`, cuyo plazo propio es de 8 s, no
  cambiaron: 8.220 s y 8.211 s frente a 8.279 s y 8.273 s en el job verde. Son
  pruebas de parada con su propio plazo, no de arranque, y no se les puede atribuir
  el mismo mecanismo.

### Ratios de duración entre jobs

Sobre los 79 tests comunes a #2320 y #2341 con duración base de al menos 0.05 s, la
mediana del ratio es 1.03 y el percentil 90 es 3.05, con máximo 11.84 (uno de los
cuatro tests de `daemon start`). En el pipeline de diagnóstico, sobre 317 tests
comunes, la mediana es 1.12 y el percentil 90 es 2.63. La degradación del job que
falló **no fue general**: se concentró en los tests de `daemon start`. Esto describe
la forma del defecto, no su causa.

## Análisis de causa

### Hechos medidos

- **Los errores funcionales devueltos fueron los correctos.** `port_in_use` (salida
  6) y `voice_not_found` (salida 3) son los que establece el contrato, y el mensaje
  citado los contiene. El desenlace funcional de esa ejecución no fue incorrecto.
- **El motivo del fallo fue el esperado.** Los tests leyeron el registro de fallo
  del hijo: si hubieran agotado `DAEMON_READY_DEADLINE` (10 s), el motivo habría
  sido `daemon_unreachable` con salida 5, no `port_in_use` ni `voice_not_found`.
- **El bloqueo de ciclo de vida no puede explicar una espera.** Se toma en modo
  compartido, no espera, y un conflicto se traduce en `lifecycle_locked` con salida
  17, no en una demora.
- **La degradación fue selectiva y simultánea.** Solo los cuatro tests consecutivos
  de `daemon start` se degradaron; el resto de la suite mantuvo su duración
  (mediana de ratio 1.03).

Lo que **no** puede afirmarse a partir de la evidencia disponible:

- **Que el hijo no falló en la ejecución que se degradó.** En las seis invocaciones
  instrumentadas el hijo fue rápido, pero todas ellas pertenecen a corridas que
  pasaron. La única ejecución degradada no tenía traza, así que se desconoce si el
  hijo llegó a publicar su registro a tiempo o si el retraso estuvo en él, en el
  lanzador o en el intercambio entre ambos.
- **Que no hubiera saturación general del ejecutor.** El caso `daemon serve` ejecuta
  el hijo en primer plano, sin el lanzador y sin el intercambio por fichero
  `daemon.ready`, de modo que su trabajo no es comparable con el de los tests
  degradados; su duración de 0.300 s no acredita nada sobre la saturación. Los ratios
  de la suite solo acotan el ruido general.
- **Que la causa no sea una regresión de código.** Que los cuatro tests fallen antes
  de cargar modelos demuestra que el camino ejercitado no carga motores, y que la
  clasificación `model_missing` añadida entre la 0.35.0 y la 0.36.0 no participa en
  este fallo concreto. No demuestra que no haya ningún otro cambio de la 0.36.0 que
  afecte a `daemon start` en general, ni que el conjunto de la 0.36.0 esté libre de
  regresiones en ese camino.

### Hipótesis, por confianza

- **Por verificar: el retraso está en el intercambio por fichero entre el lanzador y
  su hijo.** Los dos procesos se comunican con el fichero `daemon.ready`, y el
  lanzador lo sondea cada 250 ms. Un retraso en la escritura del hijo o en la
  visibilidad de esa escritura, o un retraso en el planificador que retrase al
  lanzador, ampliaría el tiempo hasta el error. Encaja con que la degradación fuera
  selectiva y afectara justo a los tests que dependen de ese intercambio.
- **Por verificar: el arranque del hijo se retrasó.** Si el hijo tardara segundos
  en llegar a `prepare_server` —por ejemplo, por la carga de la imagen ejecutable o
  por la resolución de rutas—, el lanzador no lo vería hasta el siguiente ciclo de
  sondeo. **La instrumentación no mide ese intervalo**: el origen monotónico de cada
  proceso es el suyo propio, así que hace falta comparar el `epoch_ms` del spawn con
  la primera marca del hijo. Ese contraste solo es concluyente en una ejecución que
  reproduzca el defecto, que es justo la que falta.
- **Por verificar: el ejecutor estaba bajo una carga externa.** El solapamiento
  temporal con otro job del pipeline es compatible con esta hipótesis, pero no la
  apoya: los jobs de CircleCI se ejecutan en ejecutores aislados y no hay evidencia
  de que compartieran máquina. Habría que medir la carga del propio ejecutor, o
  reproducir el fallo dentro de un mismo ejecutor bajo carga controlada.
- **Por verificar: una regresión de código en el camino de arranque.** Los cuatro
  tests que se degradaron son los que ejercitan el lanzador completo. Una regresión
  que afectara solo a ese camino y que apareciera en la 0.36.0 explicaría un fallo
  determinista, pero el defecto no se reproduce en cuatro corridas completas con el
  código de la 0.36.0 más la instrumentación, lo que la hace improbable sin más
  evidencia.

### Qué se puede discriminar en CI con la observación de la fase lenta

La instrumentación por fases permite separar las hipótesis anteriores sin depender
de conjeturas sobre la infraestructura. Basta con leer, en la invocación que se
degrade, el intervalo más largo entre dos marcas consecutivas. La columna de la
derecha dice **qué habría que medir dentro de ese tramo** para distinguir las
explicaciones posibles; no presume ninguna de ellas.

| Tramo donde se concentra el retraso | Qué queda identificado como lento | Qué falta medir dentro del tramo |
|---|---|---|
| `start.enter` → `start.model_provisioned` | el arranque del propio proceso de la CLI, antes de cualquier operación | el tiempo de carga de la imagen ejecutable y de la inicialización del runtime |
| `start.model_provisioned` → `start.health_probe_done` | el sondeo de `/health` (su tiempo de espera es de 500 ms) | si la espera agotó el tiempo o la respuesta llegó tarde, y cuánto tardó la red local |
| `launch_daemon.log_renamed` → `child.serve_enter` | el arranque del proceso hijo: aún no ha ejecutado su primera marca | el coste de crear el proceso y de cargar su imagen ejecutable |
| `child.serve_enter` → `child.failure_published` | el trabajo del hijo (inicializar voces, validar la voz, enlazar) | qué parte concreta de ese trabajo ocupa el tiempo |
| `child.failure_published` → `launch_daemon.failed_signal_read` | la entrega de la señal: el hijo ya publicó y el lanzador aún no lo ha visto | si el retraso está en el planificador del lanzador, en la visibilidad del fichero o en la cadencia del sondeo |
| `launch_daemon.failed_signal_read` → `abort_launch.done` | la limpieza: matar y recolectar el árbol del hijo | el coste de `taskkill` y de la recolección |

Si el tramo lento cae dentro del hijo, la corrección sería del arranque del hijo; si
cae entre la publicación del fallo y su lectura, sería del mecanismo de señal o del
planificador, no del plazo de la prueba. En ningún caso la lectura de la traza por sí
 sola distingue entre esas dos familias: hay que medir dentro del tramo.

### Factor discriminante pendiente

La evidencia que falta es **una ejecución que reproduzca el defecto con la traza por
fases activada**, para situar el tramo lento con la tabla anterior. La única
ejecución que falló no llevaba instrumentación.

Lo que no se ha establecido es la condición que dispara el fallo. El solapamiento
temporal con otro job Windows del pipeline es un indicio, no una condición
reproducible: los jobs de CircleCI se ejecutan en ejecutores aislados, así que
ejecutar dos jobs Windows en paralelo no garantiza ni siquiera que estén en la misma
máquina. La instrumentación permanece en la suite ordinaria para que, si reaparece
la latencia, la ejecución lenta aporte la evidencia que faltó en el primer fallo.
No se ejecutarán corridas adicionales ni se generará carga para forzarla.

## Diagnóstico sugerido

1. Conservar las marcas de fases en los tests afectados y recoger las trazas en el
   job ordinario `test-windows`, también cuando falle la suite.
2. Esperar una nueva ejecución lenta sin provocar ni repetir el fallo deliberadamente.
3. Localizar el tramo lento con la tabla de discriminantes de la sección anterior.
4. Si el tramo cae en el intercambio por fichero, investigar el mecanismo de señal
   y el planificador antes de elegir una corrección; no relajar el plazo de la prueba.

## Criterio de aceptación

- La causa queda documentada con la fase exacta, a partir de la traza por fases de
  una ejecución que reproduce el defecto.
- La suite completa de `test-windows` pasa de forma repetida en el ejecutor Windows
  de CircleCI, con los cuatro tests de `daemon start` por debajo de su plazo de 5 s
  **por invocación**.
- Si la causa resulta ser el intercambio por fichero entre el lanzador y el hijo,
  existe una prueba de regresión que falla cuando el retraso de ese intercambio
  supera el plazo esperado.

## Relacionados

- El contrato de `daemon start` y sus plazos viven en `src/main.rs`
  (`DAEMON_READY_DEADLINE`, `DAEMON_HEALTH_DEADLINE`, `DAEMON_POLL_INTERVAL`) y en
  `crates/avi-daemon` (`run_daemon_server`, `prepare_server`).
- El intercambio entre lanzador e hijo por fichero `daemon.ready` está en
  `crates/avi-daemon` (`publish_ready_record`, `read_ready_signal`).
