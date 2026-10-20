# La rama local y la del daemon muestran fases de progreso distintas para la misma operación

| Campo | Valor |
|---|---|
| Estado | **resuelto** el 2026-10-20 |
| Severidad | media |
| Tipo | funcional |
| Componente | `src/main.rs` (handlers `SpeechCommands::*`, `VoiceCommands::Clone`, `emit_ndjson_progress`), `avi-daemon` (`synthesize_handler`, `transcribe_handler`, `translate_handler`, `voices_clone_handler`, `dub_handler`) |
| Versión detectada | 0.34.0 |
| Plataforma | todas (la diferencia está en el código, no en la terminal) |
| Reproducibilidad | siempre |
| Detectado en | inspección de código y comparación de salidas, 2026-10-19 |
| Prueba de regresión | `src/main.rs`: `command_stream_phase_uses_local_labels_for_real_events`, `ndjson_progress_phase_maps_stages_and_rate_limits_by_stage`; `crates/avi-daemon/tests/golden.rs`: `synthesize_emits_contract_ndjson_stream`, `dub_emits_contract_ndjson_stream_naming_the_operation` |
| Documento canónico | [`docs/CLI/CONTRACT.md`](../CLI/CONTRACT.md#el-contrato-de-salida-por-comando) y [`docs/CLI/commands/DAEMON.md`](../CLI/commands/DAEMON.md#protocolo) |

## Resumen

El mismo comando muestra recorridos de progreso distintos según despache en local o a través del daemon. La rama local pinta una fase por etapa (capturar, leer, transcribir, traducir, sintetizar, guardar, reproducir); la rama daemon pinta una o dos fases gruesas (`validating` y la fase con sufijo `(daemon)`, o solo `dubbing` en el doblaje) y después confía en los eventos del stream NDJSON. El problema es doble: hay etapas que la rama daemon no nombra en ningún momento, y hay endpoints del daemon que no emiten **ningún** evento, con lo que la CLI se queda con una sola línea congelada durante toda la operación remota.

Para quien usa el producto, la misma operación se ve de forma distinta según el daemon esté o no en marcha, y en algunos casos el indicador desaparece por completo en la vía que debería ser la rápida.

## Entorno

- Binario 0.34.0, build de desarrollo del repositorio, Windows 11, PowerShell 7.
- Las observaciones de la rama daemon se hicieron con el daemon en ejecución en `127.0.0.1:8765`; las de la rama local, con `--no-daemon`.
- Sin `AVI_NO_PROGRESS`.

## Precondiciones

- Rama daemon: daemon arrancado (`daemon start`) y responde a `GET /health`.
- Rama local: `--no-daemon`, o bien daemon detenido con modo auto.
- Ambas ramas de `speech dub`/`speech transcribe` requieren un WAV de entrada o micrófono; ambas de `speech synthesize`/`say` requieren voz y modelo provisionados.

## Pasos para reproducir

**Comparación 1 — `speech dub`:**

1. Con daemon arrancado:

```powershell
ai-voice-interconnector speech dub --source-language es --target-language en --voice default --audio entrada.wav --daemon
```

2. Sin daemon:

```powershell
ai-voice-interconnector speech dub --source-language es --target-language en --voice default --audio entrada.wav --no-daemon
```

**Comparación 2 — `speech synthesize`:**

3. Con daemon: `--daemon`; sin daemon: `--no-daemon`, con la misma etiqueta y `--force`.

**Comparación 3 — `speech transcribe` sin eventos del daemon:**

4. Con daemon arrancado y `--audio`:

```powershell
ai-voice-interconnector speech transcribe --source-language es --audio entrada.wav --daemon
```

5. Sin daemon: mismo comando con `--no-daemon`.

**Comparación 4 — `translate` sin eventos del daemon:**

6. Con daemon: `ai-voice-interconnector translate --text <texto> --source-language en --target-language es --daemon`; sin daemon: `--no-daemon`.

## Resultado observado

**Comparación 1 (`speech dub --audio`):**

- Rama daemon (una sola fase y luego eventos; salida ajustada a la real reportada con `--mic`, cuyo cierre es este):

```text
… Doblando audio (daemon)… (0 s)
Iniciando operación… (0 s)
Procesando… (0 s)
Sintetizando voz… (1 s)
…
Doblaje reproducido.
[ok] Doblaje completado. (23 s)
```

- Rama local (una fase por etapa):

```text
… Leyendo archivo de audio… (0 s)
… Transcribiendo audio… (0 s)
… Traduciendo texto transcrito… (0 s)
… Sintetizando voz doblada… (0 s)
… Reproduciendo doblaje… (0 s)
[ok] Doblaje completado. (N s)
Doblaje reproducido.
```

Fíjese en el orden del cierre: en la rama daemon `Doblaje reproducido.` sale **antes** que el `[ok]` (el daemon lo emite por stream y la CLI lo imprime al recibirlo, mientras el `[ok]` se pinta al cerrar la fase en el handler), y en la rama local sale **después** (la fase se cierra antes de imprimir el mensaje). La misma operación cambia de orden según el camino.

**Comparación 2 (`speech synthesize`):** la rama local pinta `… Guardando locución…`; la rama daemon no pinta ninguna fase de guardado, aunque la CLI es quien persiste la locución etiquetada con `SpeechStore::save` tras recibir el WAV del daemon, que solo devuelve el audio.

**Comparación 3 (`speech transcribe --audio --daemon`):** la única línea de progreso que se ve es `… Transcribiendo audio (daemon)… (0 s)` y permanece idéntica hasta el cierre. No aparece ningún `Iniciando …`, ningún `Procesando…` ni ningún latido: el endpoint no emite eventos.

**Comparación 4 (`translate --daemon`):** ídem: `… Traduciendo texto (daemon)… (0 s)` congelada hasta el cierre.

## Resultado esperado

- `docs/DESIGN.md`, «Salida en terminal»: el progreso de fase es un mensaje del producto y debe informar de la etapa real, sea cual sea el camino de despacho. El contrato de `--daemon`/`--no-daemon` (`docs/CLI/CONTRACT.md`, secciones de `speech synthesize`, `speech say`, `speech transcribe`, `speech dub` y `translate`) declara que ambas vías hacen el mismo trabajo y producen el mismo envelope; el indicador de progreso no está excluido de esa paridad.
- `docs/CLI/CONTRACT.md`, «stderr, trazas y logs»: *«stderr solo lleva mensajes del producto y avisos»* y, del progreso de descarga, *«es un mensaje del producto, no una traza»*. La cita está redactada para `setup`/`self install`, pero el progreso de fase es también un mensaje del producto y debe salir por stderr en ambas vías, incluida la daemon.
- `src/main.rs`, `emit_ndjson_progress`: la función ya tiene traducción para `warming`, `queued`, `synthesis` y `clone`; la ausencia de `transcribe` y `translate` en ese mapa hace que esos latidos caigan en «Procesando…», que es la traducción por defecto.

## Impacto y workaround

- Quien usa el daemon (el modo recomendado para operaciones repetidas) ve menos información que quien trabaja en local: no sabe si la operación está transcribiendo, traduciendo o sintetizando.
- En `speech transcribe` y `translate` con daemon no hay ningún indicador de actividad mientras dura el trabajo remoto; en una operación de varios segundos la CLI parece colgada.
- Workaround: no hay ninguno que preserve el progreso. `AVI_NO_PROGRESS=1` solo silencia lo poco que hay. Se puede forzar `--no-daemon` para ver las fases, perdiendo el calentamiento de los motores.

## Evidencia

- Salidas literales de las comparaciones 1 (rama local), 3 y 4, capturadas el 2026-10-19 con el binario de desarrollo de 0.34.0. La rama daemon de la comparación 1 se ajusta a la salida real del doblaje con `--daemon` y `--mic` que reportó quien detectó el defecto (ver «Resultado observado»).
- `crates/avi-daemon/src/lib.rs`, `transcribe_handler` y `translate_handler`: devuelven un `Json(body)` plano; no tienen canal de eventos y no emiten `started` ni `heartbeat`.
- `crates/avi-daemon/src/lib.rs`, `synthesize_handler`, `voices_clone_handler` y `dub_handler`: sí emiten `started` y usan `with_heartbeats`, pero los `started` no incluyen la clave `operation`.
- `crates/avi-daemon/src/lib.rs`, `dub_handler`: los latidos de la transcripción y de la traducción usan los `stage` `transcribe` y `translate`, que `emit_ndjson_progress` no traduce.
- `src/main.rs`, handler de `SpeechCommands::Transcribe`: pinta la fase `capturing` solo si `mic`, de modo que con `--audio` la rama local no empieza con ninguna fase.
- `src/main.rs`, handler de `SpeechCommands::Transcribe` (rama local) y de `SpeechCommands::Dub` (rama local): en el error del motor STT, `dub` llama a `progress.finish(false, …)` y `transcribe` no.

## Análisis de causa

**Causa raíz (probable).** La rama daemon se construyó como «una fase + eventos del stream», y la rama local como «una fase por etapa», sin una tabla compartida de etapas. Cuando el endpoint del daemon no emite eventos, el diseño de la rama daemon se queda sin lastre y la CLI no tiene nada con que repintar.

**Fichas por síntoma:**

- **Ficha A — etapas que la rama daemon no nombra.** `speech dub`: la rama local pinta `capturing`/`reading`, `transcribing`, `translating`, `synthesizing` y `playing`; la rama daemon solo `dubbing`. `speech synthesize`: la rama local pinta `saving`; la daemon, no (la CLI guarda la locución etiquetada con `SpeechStore::save` tras recibir el WAV del daemon, que devuelve el audio). `speech say`: la local pinta `playing`; la daemon, no. `voice clone`: la local pinta `saving`. Confirmada por comparación de salidas.
- **Ficha B — `/transcribe` y `/translate` no emiten ningún evento.** Confirmada por inspección de `transcribe_handler` y `translate_handler`. Es el síntoma más grave de esta ficha: el indicador se congela.
- **Ficha C — los `stage` `transcribe` y `translate` caen en «Procesando…».** Confirmada: `emit_ndjson_progress` no tiene traducción para ellos (el mapa solo cubre `warming`, `queued`, `synthesis` y `clone`, y el `progress` siempre se imprime como «Procesando…» sin mirar su `stage`). Además, el contrato de los latidos (`docs/CLI/CONTRACT.md`, § de `speech dub`) solo documenta los `stage` `warming`, `queued` y `synthesis`: los `transcribe`/`translate` que el daemon emite no están reflejados en el contrato ni en el CLI.
- **Ficha D — la rama local de `speech transcribe` no pinta fase inicial con `--audio`.** Confirmada: el `phase("capturing", …)` está condicionado a `mic`.
- **Ficha E — asimetría de cierre en el error del motor STT.** Confirmada: `dub` cierra la fase y `transcribe` no, en el mismo error.
- **Ficha F — el orden del cierre final difiere entre vías.** Confirmada por inspección: en la rama daemon de `speech dub`, `dub_via_daemon` imprime «Doblaje reproducido.» al recibirlo del stream y el handler cierra la fase después (`[ok] Doblaje completado.`); en la rama local, `progress.finish` pinta el `[ok]` antes de que el handler imprima «Doblaje reproducido.».

**Lo que ya se descartó.** No es un desajuste de los envelopes JSON: las respuestas finales de ambas vías son equivalentes (está cubierto por los golden tests). Solo afecta al indicador de stderr.

## Diagnóstico sugerido

Dirección de la corrección (ya confirmada el síntoma; parcialmente la causa):

1. Definir la tabla de etapas una sola vez (etapa → texto en español) y usarla en los dos handlers de la CLI y en `emit_ndjson_progress`, de modo que cada `stage` del daemon tenga su traducción (falta `transcribe` y `translate`).
2. `/transcribe` y `/translate` devuelven JSON plano y no tienen canal de eventos: la CLI debe repintar con el reloj de la fase mientras espera la respuesta JSON, en lugar de depender de un stream que no existe.
3. Que la rama daemon pinte las fases de guardado y reproducción que la rama local pinta, o que la rama local deje de pintarlas; en cualquier caso, la misma operación debe mostrar el mismo recorrido en las dos vías.
4. Pintar una fase inicial en la rama local de `speech transcribe --audio` (por ejemplo, `reading`), alineada con `speech dub`.
5. Alinear el cierre en el error del motor STT: `transcribe` debe llamar a `finish(false, …)` como ya hace `dub`.

## Criterio de aceptación

- Para cada comando delegable (`speech synthesize`, `speech say`, `speech transcribe`, `speech dub`, `voice clone`, `translate`), el conjunto de fases nombradas en la rama local y el de la rama daemon es el mismo para el mismo trabajo. La paridad se exige para entradas equivalentes y con las fases condicionales aplicadas: `capturing` solo en la rama del micrófono, `reading` solo cuando hay fichero de entrada, traducción solo si los idiomas de origen y destino difieren, y `playing` solo cuando hay reproducción. No se exige una lista idéntica de fases en toda invocación.
- `speech transcribe --daemon` y `translate --daemon` muestran al menos una actualización de progreso mientras dura la operación remota (un repintado con el reloj de la fase). `/transcribe` y `/translate` devuelven JSON plano, sin NDJSON: lo que debe comprobarse es el repintado de la CLI durante la espera, no la existencia de eventos.
- Ningún `stage` de latido cae en el texto genérico «Procesando…».
- Pruebas de regresión que deben existir:
  - Un test de `emit_ndjson_progress` por cada `stage` emitido por el daemon, que verifique el texto esperado (incluidos `transcribe` y `translate`).
  - Un test de doble daemon que responda a `/transcribe` con JSON plano y verifique que la CLI repinta la fase mientras espera la respuesta, sin depender de eventos NDJSON.
  - Un test que compare, comando a comando, la lista de `phase()` de la rama local y de la rama daemon (por ejemplo, registrando las etiquetas de fase en un doble).

## Relacionados

- `progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md` (los eventos del daemon y su reloj).
- `los-cierres-y-los-relojes-del-progreso-se-duplican.md` (los cierres duplicados que se ven en ambas vías).
- `fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md` (la ficha E de este documento es también un caso de fase sin cerrar).
- `CHANGELOG.md`, 0.34.0: «progreso de fases por stderr, una sola línea por terminal».

## Resolución (2026-10-20)

**Resuelto, con una asimetría declarada.** Las dos vías de un comando nombran ahora las **mismas fases con los mismos literales** cuando la entrada es equivalente. La etapa técnica que el daemon anuncia se traduce a la etiqueta canónica de la vía local —`synthesis` → `synthesizing`, `clone` → `cloning`, `transcribe` → `transcribing`, `translate` → `translating`—, de modo que la etiqueta que el usuario ve es la que la prueba comparativa compara. La matriz completa, comando por comando y vía por vía, está en el Anexo B del plan.

Lo que queda es una asimetría **estructural y documentada**, no un defecto pendiente: la paridad está acotada por lo que el daemon puede anunciar.

| Asimetría que queda | Por qué |
|---|---|
| La captura (`capturing`) y la lectura (`reading`) solo existen en la vía local | Ocurren en la CLI **antes** del despacho: el daemon no puede nombrarlas |
| `saving` no se nombra en `voice clone` por daemon | El daemon persiste la referencia y su wire no expone un evento de guardado |
| `translating` no se nombra en `synthesize`/`say` por daemon | El daemon anuncia `stage: "synthesis"` antes de traducir y no emite `stage: "translate"` |

Ninguna de esas fases se **inventa**: anunciar una etapa que no ocurrió sería mentir sobre el trabajo en curso. La clave `operation`, que el daemon añadió a su evento `started`, amplía el wire y es donde esa frontera puede moverse.

**Pruebas de regresión** (las que cubren este defecto):

| Prueba | Qué fija |
|---|---|
| `src/main.rs` → `command_stream_phase_uses_local_labels_for_real_events` | La etapa técnica se traduce a la etiqueta canónica de la vía local |
| `src/main.rs` → `ndjson_progress_phase_maps_stages_and_rate_limits_by_stage` | Los siete `stage` del daemon tienen traducción propia y racionan por etapa |
| `crates/avi-daemon/tests/golden.rs` → `synthesize_emits_contract_ndjson_stream` | El `started` de `/synthesize` nombra la operación |
| `crates/avi-daemon/tests/golden.rs` → `dub_emits_contract_ndjson_stream_naming_the_operation` | El `started` de `/dub` nombra la operación |

**Documento canónico**: [`docs/CLI/CONTRACT.md`](../CLI/CONTRACT.md#el-contrato-de-salida-por-comando) —la tabla del contrato de salida por comando, con sus fases— y [`docs/CLI/commands/DAEMON.md`](../CLI/commands/DAEMON.md#protocolo) —el mapa de etapas del daemon—.
