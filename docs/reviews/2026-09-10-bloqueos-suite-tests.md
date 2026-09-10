# Revisión: bloqueos de la suite de tests — causas y corrección estructural

- **Fecha**: 2026-09-10
- **Estado**: Abierto — diagnóstico con evidencia, pendiente de ejecución
- **Alcance**: `tests/cli_golden.rs` (36 tests), ciclo de vida del daemon (`src/main.rs`, `crates/avi-daemon/src/spawn.rs`, `crates/avi-daemon/src/lib.rs`) y régimen de ejecución de inferencia real. Solo se documentan soluciones que eliminan trabajo o defectos de raíz; no se documentan mitigaciones.
- **Síntoma que originó la revisión**: la corrida completa no terminó en 300s y dejó dos procesos huérfanos (`ai-voice-interconnector` + `qwen_tts`).

## 1. Resumen ejecutivo

No hay interbloqueo ni espera infinita en los caminos que la suite ejercita. La raíz es triple y verificable: (1) cada test pesado repite íntegro el costo más caro —carga de modelo de ~2,5 GB + síntesis de calentamiento— porque cada uno levanta y mata su propio daemon; (2) cada síntesis en modo directo recarga el modelo por invocación en vez de reutilizar el daemon caliente; (3) varias pruebas pagan reproducción de audio en tiempo real para verificar código que no produce sonido. El ciclo de vida del daemon, además, es frágil por diseño (archivo PID de una sola ranura, adhesión ciega a daemons previos, apagados best-effort, sleeps fijos), de modo que cualquier interrupción deja un entorno envenenado para la siguiente corrida. Las tres soluciones de este documento eliminan trabajo real o clases de defecto enteras; ninguna mueve el problema de carpeta.

## 2. Problemas identificados, con evidencia

### P1 — Cada test pesado paga un warmup completo

Los tests con daemon repiten el ciclo `stop → start → uso → stop` de forma individual: `daemon_start_exito`, `daemon_restart_rearma`, `daemon_status_running`, `daemon_start_con_auto_restart`, `translate_con_daemon_delega` y `clone_con_daemon_delega` (`tests/cli_golden.rs:1081-1364`), más los de `dub` vía daemon. Cada `start` carga el modelo (~2,5 GB) y ejecuta la síntesis de calentamiento (`warmup_tts`, `crates/avi-daemon/src/lib.rs:1092`). El propio harness documenta el costo: cada corrida consume ~2,7 GB de RAM (`tests/cli_golden.rs:551-553`). Con seis o más arranques serializados a minutos cada uno en CPU, el tiempo total excede cualquier presupuesto de loop diario. El factor N es evitable: el warmup solo necesita ocurrir una vez por corrida.

### P2 — La vía directa recarga el modelo por invocación

Cada síntesis en modo directo construye el motor y resuelve el modelo desde cero por invocación (`src/main.rs`, manejadores de `Synthesize`/`Say`/`Dub` + `Qwen3TtsEngine::new`). Los tests pesados que ejercitan la vía directa (`synthesize_exito_con_label`, `voice_clone_exito`) pagan la carga completa una vez por test, cuando el daemon caliente ya resuelve exactamente la misma necesidad una sola vez por corrida. Es el mismo defecto que P1 visto desde el otro modo de despacho: pagar N veces un costo fijo.

### P3 — Reproducción en tiempo real como verificación

Las pruebas de `say` y `dub` reproducen audio por los altavoces (`hay_dispositivo_audio`, `tests/cli_golden.rs:649-654`) y bloquean en tiempo real hasta que termina el audio. Reproducir verifica el mezclador del sistema operativo, no el código del proyecto: la señal que estos tests necesitan (¿el WAV es válido? ¿dice lo pedido?) ya la cubren los helpers existentes de aserción de archivo (`wav_valido_24k`, `tests/cli_golden.rs:572-580`) y WER (`wer_vs_texto`, `tests/cli_golden.rs:585-603`). Cada minuto de audio reproducido es pared pura sin señal adicional.

### P4 — Ciclo de vida del daemon frágil por diseño

Cuatro defectos encadenados, todos verificados en código: (a) el ciclo de vida cabe en un archivo de una sola ranura (`daemon.pid`, `src/main.rs:2339-2350`); (b) `start` ante un daemon que responde devuelve `already_running` y se adhiere a él (`src/main.rs:1359-1369`) sin distinguir un daemon sano de un rancio de otra corrida; (c) el `stop` final de cada test solo se ejecuta si el test llega hasta allí —un panic, un retorno anticipado o un timeout externo lo salta y el daemon queda huérfano (observado: dos procesos huérfanos tras el corte de la corrida); (d) varios tests ignoran el resultado del `start` (`let _`, p. ej. `tests/cli_golden.rs:1149`) y uno convierte la aserción en condicional (`tests/cli_golden.rs:1154`: si el daemon no está corriendo, casi nada se verifica y pasa igual). Cualquier interrupción deja un puerto ocupado y un estado tibio que envenena la siguiente corrida.

### P5 — Sleeps fijos en vez de observación de estado

Tras cada `start`/`stop` los tests duermen 300–1500ms fijos (`tests/cli_golden.rs:1144,1150,1162` y gemelas) asumiendo el warmup, cuando el producto ya define readiness con deadline (`DAEMON_READY_DEADLINE`, `src/main.rs:35-37`). Bajo carga el daemon no está listo en 300ms: el test avanza contra un daemon tibio y caen esperas hasta el deadline en cascada. El tiempo supuesto sustituye al estado observado.

### Descartado con evidencia (no son la causa)

- **Interbloqueo de locks**: los dos mutex (`STATE_LOCK`, `tests/cli_golden.rs:27`; `TTS_LOCK`/`lock_tts`, `tests/cli_golden.rs:554-558`) se toman siempre en el mismo orden y nunca dos veces. Imposible por construcción.
- **Cuelgue por pipes heredados**: ya corregido de raíz en el producto (`Stdio::null` + `bInheritHandles=FALSE`, `crates/avi-daemon/src/spawn.rs:21-66`, con la clase de defecto documentada en `spawn.rs:3-20`).
- **Servidor en foreground**: ningún test invoca `daemon serve` salvo `--help` (`tests/cli_golden.rs:1186`); todas las esperas del producto tienen deadline.
- **Paralelismo total como salida**: inviable por capacidad, no por código —los hilos se dimensionan a núcleos físicos a propósito (`crates/avi-core/src/engine.rs:185-187`) y cada corrida reserva ~2,7 GB. N inferencias en paralelo compiten por los mismos núcleos y la misma RAM: más lento en pared que en serie. La serialización actual es lo correcto; hay que conservarla.

## 3. Opciones estructurales

### O1 — Fixture de daemon por sesión: un solo dueño del ciclo de vida

**Mecanismo.** Un arranque por corrida pesada, reutilizado por todos los tests que hoy hacen su propio ciclo; un solo apagado al final. La fixture es la única dueña del ciclo de vida: arranca con poll-hasta-ready contra el deadline ya definido en el producto (nada de sleeps), detecta PID rancio al arrancar (proceso muerto o daemon ajeno → parte de cero en vez de adherirse) y apaga al cerrar la sesión. Solo los 3–4 tests que verifican la semántica del ciclo de vida (`start`, `restart`, `status`) conservan arranques propios; todo lo demás usa el daemon de la sesión.

**Por qué ataca la raíz.** Elimina N−1 warmups y N−1 cargas de ~2,5 GB: es trabajo que deja de existir, medible en minutos por cada ciclo suprimido. Y sustituye N dueños frágiles del ciclo de vida por uno solo determinista, lo que elimina de una vez la clase entera de P4 (huérfanos por interrupción dentro de la sesión, adhesión a daemons rancios) y P5 (los sleeps desaparecen porque la espera es por estado observado).

**Riesgos y costo.** Toca solo el harness, nunca el producto: no puede introducir defectos de usuario. El riesgo real es acoplamiento entre tests vía estado compartido del daemon (voces, habla sintética): se contiene con namespaces únicos por test, convención que el harness ya usa (`etiqueta_unica`, `tests/cli_golden.rs:561-567`). Esfuerzo medio-bajo.

### O2 — Pesados por vía daemon, un testigo en directo por comando

**Mecanismo.** Reenrutar los tests pesados de síntesis, clonado y doblaje a la vía daemon caliente de la fixture (principio: aprovechar el daemon, evitar inferencia en frío) y conservar exactamente un test en modo directo por comando como testigo de esa ruta.

**Por qué ataca la raíz.** Elimina las recargas de modelo por invocación (P2): cada test directo actual paga una carga completa que el daemon caliente ya pagó una vez. La cobertura de la vía directa no se pierde —queda fijada por el testigo— y todo lo demás verifica el mismo comportamiento contra motores calientes en una fracción del tiempo.

**Riesgos y costo.** Solo harness. Riesgo: un defecto exclusivo de la vía directa fuera del testigo pasaría inadvertido; se acepta porque el testigo cubre la ruta y el resto del comportamiento (flags, validación, payloads) es común a ambas vías antes del despacho. Esfuerzo bajo una vez existe O1, ya que requiere el daemon de sesión para no reintroducir arranques por test.

### O3 — Verificación por archivo en vez de reproducción

**Mecanismo.** Sustituir la reproducción en tiempo real en los tests por aserción sobre el WAV producido con los helpers ya existentes (`wav_valido_24k`, `wer_vs_texto`), más una única prueba de humo de audio que conserve la cobertura del path de reproducción.

**Por qué ataca la raíz.** Elimina esperas de tiempo real proporcionales a la duración del audio (P3): cada minuto reproducido es pared sin señal, porque reproducir verifica el mezclador del sistema, no el código. La señal real —validez del WAV y fidelidad del contenido— ya está implementada y es más estricta que "sonó sin error".

**Riesgos y costo.** Solo harness. Riesgo mínimo: la única cobertura que se adelgaza (fallo del dispositivo de salida) queda retenida por la prueba de humo. Esfuerzo bajo e independiente de O1/O2.

## 4. Orden de ejecución y criterio de cierre

Orden: **O1 primero** (es la base que abarata todo lo demás y la única que elimina la fragilidad del ciclo de vida), **O2 y O3 en paralelo después** (independientes entre sí una vez existe el daemon de sesión). Régimen permanente: contractuales en paralelo, inferencia en serie por diseño (ver sección 2, descartados) — la serie se conserva, lo que se reduce es el trabajo dentro de ella.

Criterio de cierre, medido y no estimado: corrida pesada en serie en verde con el tiempo de pared registrado antes y después —el delta debe corresponder a los warmups, cargas y reproducciones suprimidos— más la suite rápida en verde en segundos. Sin esa medición no hay veredicto.

## 5. Método y fuentes

Inspección directa sin mutaciones: `crates/avi-daemon/src/spawn.rs` completo (herencia de handles), `src/main.rs:1355-1504` (start/stop/restart), `src/main.rs:2339-2350` (pidfile), `crates/avi-daemon/src/lib.rs:1092` (warmup), `crates/avi-core/src/engine.rs:185-187` (dimensionado de hilos), `tests/cli_golden.rs:27,551-654` (locks, costos, helpers) y `tests/cli_golden.rs:1081-1411` (ciclos de vida y aserciones condicionales), más observación de la corrida interrumpida (procesos huérfanos con timestamp de la corrida, suite rápida 17/17 en 0,59s, modelos provisionados sin skips).
