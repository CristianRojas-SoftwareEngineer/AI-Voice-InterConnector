# Deuda técnica vigente tras C10

## 1. Introducción y contexto

El ciclo C10 ("cierre integral de deuda técnica pre-release", rama
`fix/deuda-pre-release`) cerró con G-Resultado aprobado: batería §5.3 en verde,
sin intermitencias y revisión independiente sin bloqueantes. Este documento
recoge la deuda que **sigue vigente** después de C10, clasificada en tres
categorías excluyentes:

- **Categoría 1 — Introducida por C10.** El artefacto solo puede existir tras
  C10 (símbolos creados por T1–T16, o decisiones tomadas al ejecutarlas).
- **Categoría 2 — Descubierta durante C10 (preexistente).** Código que ninguna
  tarea de C10 lista en sus acciones y el ledger no registra, sacado a la luz
  por el trabajo del ciclo.
- **Categoría 3 — Pendiente desde antes, sin cambios.** Descrita como estado
  previo en el plan o en decisiones anteriores; C10 no la tocó.

**Método.** Investigación solo de lectura. Cada hallazgo cita evidencia
`ruta:línea` y lleva marca **VERIFICADO** (hecho de código) o **HIPÓTESIS**
(requiere ejecución para confirmar). Sin diff contra `main` disponible, el
origen de los hallazgos 2/3 se apoya en el alcance declarado de cada tarea
del plan (`C:\Users\Cristian\.claude\plans\cosmic-gathering-reddy.md`) y en
el ledger (`docs/issues/iterative-workflow-design.progreso.json`).

**Qué cuenta como "solución estructural trivial".** La que elimina la causa
(no compensa el síntoma) y o bien borra código, o bien unifica un mecanismo,
o bien el compilador guía todos los puntos de cambio. Quedan prohibidos:
parches ad-hoc, parches superficiales, parches de síntomas, parches de
retrocompatibilidad y fallbacks.

## Tabla de contenidos

- [1. Introducción y contexto](#1-introducción-y-contexto)
- [Tabla de contenidos](#tabla-de-contenidos)
- [2. Hallazgos con solución estructural trivial](#2-hallazgos-con-solución-estructural-trivial)
  - [H1 — stderr fusionado en handover](#h1--stderr-fusionado-en-handover)
  - [H2 — Dos pruebas sin puerta require](#h2--dos-pruebas-sin-puerta-require)
  - [H3a — kill bloqueante en stop async](#h3a--kill-bloqueante-en-stop-async)
  - [H6 — Avisos por stop](#h6--avisos-por-stop)
  - [H7 y H11 — Control positivo común](#h7-y-h11--control-positivo-común)
  - [H9 — Caso rancio sin documentar](#h9--caso-rancio-sin-documentar)
  - [H10 — Tests por substring](#h10--tests-por-substring)
  - [H12 — Arista avi-shared a avi-process](#h12--arista-avi-shared-a-avi-process)
- [3. Hallazgos parciales o con decisiones abiertas](#3-hallazgos-parciales-o-con-decisiones-abiertas)
  - [H5 — Degradación de la garantía de T7](#h5--degradación-de-la-garantía-de-t7)
  - [H8 — let _ de disco fuera de T9](#h8--let-_-de-disco-fuera-de-t9)
  - [H13 — Puerto literal frente a entorno](#h13--puerto-literal-frente-a-entorno)
- [4. Hallazgos no triviales: análisis y diseño](#4-hallazgos-no-triviales-análisis-y-diseño)
  - [H3b — Muerte sin binario externo](#h3b--muerte-sin-binario-externo)
  - [H4 — Código solo-Unix sin ejecutar](#h4--código-solo-unix-sin-ejecutar)
  - [H14 — Pata full solo en máquina provisionada](#h14--pata-full-solo-en-máquina-provisionada)
  - [H15 — Locks en avi-stt](#h15--locks-en-avi-stt)
  - [H16 — Residuos de suite](#h16--residuos-de-suite)
  - [H17 — Doble probe](#h17--doble-probe)
- [5. Hipótesis refutadas](#5-hipótesis-refutadas)
  - [Hueco en stop paso 2b](#hueco-en-stop-paso-2b)
  - [Sleeps restantes en cli_golden](#sleeps-restantes-en-cli_golden)
  - [Allows y matices de código muerto](#allows-y-matices-de-código-muerto)

## 2. Hallazgos con solución estructural trivial

### H1 — stderr fusionado en handover

El plan exige "los tres extremos, por extremo" (plan:166); el código ata
ambos al mismo fichero en Windows (`crates/avi-process/src/restricted_spawn.rs:334-335`)
y en Unix (`:284-286`), y lo confiesa en `crates/avi-lifecycle/src/update.rs:158-166`
("equivalente a `2>&1`"). `RestrictedSpawnRequest` (`:36-51`) no puede expresar
salidas separadas. Cambio funcional silencioso en `self update` (antes heredaba
los tres por separado por vía clásica, plan:162). **Categoría 1.
VERIFICADO.** Objetivo incumplido: T6-objetivos (plan:166).

**Solución.** Añadir el segundo campo al request; el threading ya existe
(`launch_process_with_list` recibe los tres handles por separado, `:344-351`);
solo el *tipo* los fusiona en un `log_file`. Actualizar los 11 literales
(3 en `spawn.rs`, 4 en `avi-tts`, 2 en tests, `update.rs` y `update_fetch`;
todos en el repo, todos guiados por el compilador). `verify_boot` pasa el
mismo fichero dos veces a propósito y `handover` duplica también el stderr
del padre: la fusión pasa de restricción oculta a decisión explícita por call
site. Sin shims.

### H2 — Dos pruebas sin puerta require

`synthesize_result_delivers_the_file_the_engine_wrote`
(`crates/avi-daemon/tests/golden.rs:1128`) y
`synthesize_disconnect_removes_temporary_when_blocking_work_ends` (`:1177`)
llegan a `with_stores().expect(...)` (`:85-89`) sin `#[ignore]` ni
`require_parakeet()` (barrido completo de puertas: `:74/:117/:289/:416/:471/:539/:852/:1200`;
ninguna en estos dos tests). Bajo `native-stt` sin modelo,
`ParakeetEngine::new(...)?` (`crates/avi-daemon/src/lib.rs:120-122`) hace
fallar el `expect` con mensaje opaco, no con el "requiere Parakeet" que la
cabecera promete (`golden.rs:10-24`, que solo nombra a `test_state` como
ayudante con puerta). **Categoría 1. VERIFICADO** (el texto exacto del pánico
en máquina sin provisionar: HIPÓTESIS). Objetivo incumplido: T13-objetivos
(plan:303-304, "falla con `requiere`").

**Solución.** Una línea `#[cfg(feature = "native-stt")] require_parakeet();`
dentro de `contract_state` (`golden.rs:83`) cubre sus tres llamadores
(`test_state:76`, `state_without_base_model:99`, `state_with_held_tts:1030`).
En featureless compila a nada. Punto único: imposible olvidar la puerta.

### H3a — kill bloqueante en stop async

`spawn.rs:112-114` documenta la partición: lo bloqueante vive fuera del
runtime (`wait_for_pid_death` es sync solo por diseño). `kill_tree_by_pid_exact`
es `Command::new("taskkill")...status()` bloqueante sin plazo
(`avi-process/src/lib.rs:198-205`) y `stop()` es async (usa `.await` en
`daemon_stop.rs:348`; el kill va en `:370`). **Categoría 1. VERIFICADO** (que
`taskkill` llegue a colgarse: HIPÓTESIS).

**Solución.** Envolver la llamada en `spawn_blocking` con el deadline
existente, como ya hace `handover` (`update.rs:116`). Restaura la partición
documentada; no toca la primitiva.

### H6 — Avisos por stop

Tres `eprintln!` en `read_pid_file`
(`crates/avi-lifecycle/src/daemon_stop.rs:150/161/170`) × ~80 lecturas por
`stop` (una línea por lectura en el caso realista: los tres avisos no
coexisten) cuando el daemon está vivo y el pidfile se descarta. El lazo
(`:390-397`) sale en la primera vuelta si nada está vivo (`:393`), así que el
spam exige daemon vivo: el escenario de upgrade. Antes de T1 esa función no
avisaba. **Categoría 1. VERIFICADO.**

**Solución.** Separar observación silenciosa de reporte: el lazo usa la
variante silenciosa (o recibe el motivo de descarte) y el veredicto avisa una
sola vez. Guiado por el compilador en todos los llamadores.

### H7 y H11 — Control positivo común

El doble PowerShell escribe `'closed'` ante **cualquier** excepción
(`crates/avi-process/tests/restricted_spawn.rs:130-132`): si `Add-Type` está
bloqueado o el script tiene una errata, pasa aunque la allowlist esté rota
(la dependencia de máquina es HIPÓTESIS; el `catch` total, VERIFICADO). Y el
bucle de `extra_allowed` (`restricted_spawn.rs:337`) no itera nunca: 11
constructores, todos `Vec::new()` (3 en `spawn.rs`, 4 en `avi-tts`, 2 en tests,
`update.rs` y `update_fetch`); su doc cita "el HANDLE de espera del
limpiador" (`:48-50`) pero `deferred.rs:95-99` construye su allowlist en línea.
**Categoría 1 (H7) y 2 (H11, origen C9; T11 no lo retiró). VERIFICADO.**

**Solución (común).** Añadir el caso positivo: el handle testigo **listado vía
`extra_allowed`** debe llegar al hijo (`eof`/`open`). La misma prueba demuestra
que la sonda funciona (si se rompe, el positivo enrojece) y ejecuta la ruta
`:337` por primera vez; más corregir el ejemplo del doc. Solo test + una línea
de doc. (El CHANGELOG:115 bendice `extra_allowed` como vía de migración, así
que se conserva y se estrena, no se borra.)

### H9 — Caso rancio sin documentar

Sin `start=`/`image=`, `recorded = None` (`daemon_stop.rs:531-534`) y, sin la
rama `None → Ours` que T1 retiró, el fichero se clasifica `Absent` y **se
borra** (`:435-437`); antes se conservaba. Ni el doc (`:516-518`, enumera 4
casos) ni la guía (`docs/CLI/commands/DAEMON.md:82`) nombran el quinto. (El
CHANGELOG:114 sí lo documenta.) **Categoría 1. VERIFICADO.**

**Solución.** Propagar la enumeración a los dos sitios. El comportamiento es
correcto y principista (sin identidad no hay propiedad demostrable); terminar
el cambio no es parchear.

### H10 — Tests por substring

`daemon_stop.rs:1478-1480` lee `src/main.rs` vía
`CARGO_MANIFEST_DIR/../../src/main.rs` y afirma por substring, con `concat()`
para no autodetectarse (`:1502-1503`, `:1530-1532`). Se rompen con un
`use ... as` inocuo; pasan si se redeclara con otro nombre. **Categoría 1.
VERIFICADO.**

**Solución.** Borrar los dos bloques textuales (`:1501-1519` y `:1533-1550`,
~40 líneas con el helper `:1478-1481`); los valores ya se afirman
conductualmente (`:1489`, `:1497`: 8 s, 8765, más `:1554-1558`). Lo que queda
protege lo mismo y desaparece el acople al layout del workspace.

### H12 — Arista avi-shared a avi-process

`avi-shared/Cargo.toml:12` consume `pid_alive_exact` (`logs.rs:154-156`),
símbolo de definición T2 (plan:96), y arrastra `avi-process` a todo
consumidor de `avi-store` y `avi-lifecycle` (re-exportan `create_log`:
`avi-store/src/lib.rs:8`, `avi-lifecycle/src/lib.rs:42`; consumidores en
`avi-tts/src/lib.rs:952/1080` y `src/main.rs:3523/3564/3688`). El plan T7
ofrecía "trait o parámetro inyectado". **Categoría 1. VERIFICADO.**

**Solución.** La costura ya existe: `create_log_at_with` recibe el sondeo;
solo `create_log_at` (`logs.rs:52-54`) lo hardcodea. Pasar la sonda desde los
5 llamadores y borrar la arista. De paso, borrar `create_log_for_pid`
(`logs.rs:32-36`): sin llamador en producción (solo su propio test, `:261`) y
su propósito documentado lo sirve `rename_log_to_pid`, que sí se usa
(`main.rs:3720`).

## 3. Hallazgos parciales o con decisiones abiertas

### H5 — Degradación de la garantía de T7

Lo trivial: borrar la frase falsa (`logs.rs:85-87`: el nombre con el PID del
lanzador **no** "sigue siendo válido para la poda" cuando el lanzador ya salió,
por la guarda `logs.rs:52-54`) y renombrar con comprobación de existencia
(`fs::rename` en `logs.rs:97` **pisa en Unix y falla en Windows**; el doc solo
describe el fallo). Lo no trivial: tras un fallo de renombrado el log queda
desprotegido, y solo un registro del log vivo (p. ej. en el ready) lo cierra.
Eso es diseño, no arreglo. **Categoría 1. VERIFICADO** (el escenario de
colisión misma-familia+PID+ms: HIPÓTESIS).

### H8 — let _ de disco fuera de T9

`cleanup.rs:783`, `avi-lifecycle/src/lib.rs:198`, `path_unix.rs:304/319/325`,
`receipt.rs:208`, `setup.rs:136`, `transaction.rs:210/322/381/420`,
`uninstall.rs:361/1011/1023/1035/1049/1062` y `daemon_stop.rs:131/436`: 19
sitios de mutación de disco en producción (separados de los `mod tests` por
sus líneas de declaración; verificado uno por uno), más
`daemon_stop.rs:353/358`, descartes de timeout de apagado gracioso —flujo de
control deliberado, fuera del criterio—. El objetivo T9 dice "cero" sin
limitarse a 3 ficheros (plan:224), pero las acciones sí. Doble estándar sin
regla escrita. **Categoría 2. VERIFICADO.**

**Vía de cierre.** Un helper único `best_effort_cleanup(result, contexto)` con
traza sustituye los 19 silencios ad hoc. El mecanismo es trivial; la migración
es mecánica pero exige leer cada sitio para su contexto. (Algunos silencios
son genuinamente triviales, p. ej. el temporal propio en `daemon_stop.rs:131`;
la regla debe decir cuáles.)

### H13 — Puerto literal frente a entorno

Literal `DEFAULT_ADDR` en `main.rs:2298` (stop) y `:2365` (restart), con
justificación en `:2293-2295` que solo vale si nadie desvió el puerto, frente
a `default_addr()` sensible al entorno en `:3032`. El plan lo describe como
estado previo (plan:239: "tres resolvers con semánticas distintas").
**Categoría 3. VERIFICADO.**

**Vía de cierre.** Mecánicamente son 2 sitios (pasar el valor resuelto en vez
del literal; `stop()` ya lo recibe como parámetro a propósito,
`daemon_stop.rs:334-336`). Pero cambia qué puerto se sondea con env fijado:
**decisión de producto**, diferida a G-Resultado por T10. No se toca sin
veredicto.

## 4. Hallazgos no triviales: análisis y diseño

### H3b — Muerte sin binario externo

`kill_tree_by_pid_exact` (Windows) y `sweep_resident_by_image`
(`avi-tts/src/lib.rs:1243-1265`, `taskkill`/`pkill`) dependen de binarios
externos en la ruta central de parada, tras centralizar en ellos T2 y T11.
Eliminarlos exige enumeración nativa del árbol (Toolhelp en Windows):
esfuerzo medio, con sus propios riesgos de `unsafe`. **Categoría 1
(centralización) sobre base 2/3 (los binarios ya se usaban). VERIFICADO.**

### H4 — Código solo-Unix sin ejecutar

`pre_exec(setsid)` (`restricted_spawn.rs:291-298`), las tres primitivas
`cfg(unix)` con `fits_pid_t` (`avi-process/src/lib.rs:88-111/165-184`), el
parser de ancho fijo de 24 caracteres sobre un campo `ps` dependiente del
locale (`:113-127`) y 2 tests `cfg(unix)`: verificados solo por compilación
cruzada. Ningún cambio de código lo cierra; solo D-11
(pase manual en Linux x64 y macOS arm64, abierto:
`iterative-workflow-design.md:1556`, `:1581`). **Categoría 3. VERIFICADO.**

### H14 — Pata full solo en máquina provisionada

Los 21 contratos construyen `DaemonState` y `with_stores` carga Parakeet bajo
`native-stt` (`avi-daemon/src/lib.rs:113-122`); la evidencia de "sin
intermitencias" viene de una máquina con modelos. Con D17 vigente (sin rama
`None` en producción) un contrato "sin modelo" es imposible; la alternativa es
CI provisionada o puerta documentada. Requiere decisión, no código.
**Categoría 2/3 según se mire (diseño heredado + verificación de C10).
VERIFICADO.** (Antes N3; se renumera para la tabla.)

### H15 — Locks en avi-stt

3 `lock().unwrap()` en `crates/avi-stt/src/parakeet.rs:168/188` (+:267):
el fichero no tiene `mod tests` (cero marcas `#[test]`/`cfg(test)` en él;
los tests del crate viven en `lib.rs`) y no figura en ninguna acción de
C10. Requiere decidir si se aplica el patrón `into_inner` de T8 o se
documenta por qué las sesiones ORT no se envenenan. **Categoría 2.
VERIFICADO.**

### H16 — Residuos de suite

Barrido de temporales no invocado por la suite y escrituras de los tests de
TTS en `data/speech` real (del informe T17; no re-verificados en esta pasada).
**Categoría 2.**

### H17 — Doble probe

Doble `probe` en `stop()` (`daemon_stop.rs:393` y `:407`): el lazo rompe con
un sondeo y el veredicto re-sondea, contra el comentario de "los tres hechos
se observan una vez" (`:402-405`). Ninguna tarea de C10 lo toca (origen previo
inferido, sin diff disponible). **Categoría 2. VERIFICADO.**

## 5. Hipótesis refutadas

No son deuda; se registran para no reabrirlas.

### Hueco en stop paso 2b

Refutado: `read_resident_pid` (`daemon_stop.rs:191`) colapsa a `0` al
descartarse el fichero y el caso cae en el barrido por imagen (`:385` →
`avi-tts/src/lib.rs:1243`, implementación real con `taskkill`/`pkill`).

### Sleeps restantes en cli_golden

Refutado para ese fichero: los 10 verificados (`:286/:415/:557/:573/:619/:1016/:4742/:4796`
cadencias de sondeo acotadas, `:5608` estímulo deliberado de un
`#[should_panic]`, `:5834` backoff acotado) siguen el patrón prescrito; el
objetivo T12 se cumple aquí.

### Allows y matices de código muerto

Legítimos: `unix_claim_verified` se llama en `src/main.rs:2830` bajo
`cfg(unix)`; `stdin_holder` se puebla en `restricted_spawn.rs:317` y se mueve
al struct en `:366` (retenido solo por `Drop`, lo declarado). Y precisión:
`extra_allowed` no es código muerto sino API viva nunca poblada (H11), y
`create_log_for_pid` es una entrada sin llamador, no un subárbol muerto (H12).
