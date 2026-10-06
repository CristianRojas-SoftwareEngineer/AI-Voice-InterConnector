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
  - [H2 — Dos pruebas sin puerta require](#h2--dos-pruebas-sin-puerta-require)
  - [H6 — Avisos por stop](#h6--avisos-por-stop)
  - [H9 — Caso rancio sin documentar](#h9--caso-rancio-sin-documentar)
  - [H12 — Arista avi-shared a avi-process](#h12--arista-avi-shared-a-avi-process)
- [3. Hallazgos parciales y pendientes de decisión](#3-hallazgos-parciales-y-pendientes-de-decisión)
  - [H5 — Degradación de la garantía de T7](#h5--degradación-de-la-garantía-de-t7)
  - [H8 — let _ de disco fuera de T9](#h8--let-_-de-disco-fuera-de-t9)
  - [H13 — Puerto literal frente a entorno](#h13--puerto-literal-frente-a-entorno)
  - [H3a — kill bloqueante en stop async (diseño pendiente)](#h3a--kill-bloqueante-en-stop-async-diseño-pendiente)
  - [H10 — Tests por substring (garantía sin reemplazo)](#h10--tests-por-substring-garantía-sin-reemplazo)
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

### H2 — Dos pruebas sin puerta require

`synthesize_result_delivers_the_file_the_engine_wrote`
(`crates/avi-daemon/tests/golden.rs:1128`) y
`synthesize_disconnect_removes_temporary_when_blocking_work_ends` (`:1177`)
llegan a `with_stores().expect(...)` (`:85-89`) sin `#[ignore]` ni
`require_parakeet()` (barrido completo de puertas: `:74/:117/:289/:416/:471/:539/:852/:1200`;
ninguna en estos dos tests). Bajo `native-stt` sin modelo,
`ParakeetEngine::new(...)?` (`crates/avi-daemon/src/lib.rs:120-122`) hace
fallar el `expect` con el error de carga del modelo, no con el `requiere
Parakeet: …` y la instrucción de provisionado que la cabecera promete
(`golden.rs:10-24`, que solo nombra a `test_state` como ayudante con puerta). **Categoría 1. VERIFICADO** (el texto exacto del pánico
en máquina sin provisionar: HIPÓTESIS). Objetivo incumplido: T13-objetivos
(plan:303-304, "falla con `requiere`").

**Solución.** Una línea `#[cfg(feature = "native-stt")] require_parakeet();`
dentro de `contract_state` (`golden.rs:83`) cubre sus tres llamadores
(`test_state:76`, `state_without_base_model:99`, `state_with_held_tts:1030`) y,
con ellos, los dos tests. En featureless compila a nada. Punto único: imposible
olvidar la puerta. `clone_state` llama a `with_stores` directo (`:120`) y conserva
su puerta propia (`:117`), de modo que todos los caminos al constructor quedan
cubiertos. Las puertas por test (`:74/:289/:416/:471/:539/:852/:1200`) quedan
redundantes o inertes y no se tocan (borrarlas no es parte del cambio). La
cabecera (`:16-23`) se reescribe para decir que la puerta vive en `contract_state`
y la heredan sus tres derivados, no solo `test_state`.

### H6 — Avisos por stop

Tres `eprintln!` en `read_pid_file`
(`crates/avi-lifecycle/src/daemon_stop.rs:150/161/170`; los tres avisos no
coexisten: cada uno precede a un `return None`), y cada lectura de un pidfile
descartado emite uno. Un `stop` con el pidfile descartado lo lee 6 veces fuera
del lazo (`resolve_addr` en `:343` vía `read_addr`, `:344`, `:366`, `:379`,
`:400`, `:406`) y 2 por vuelta del lazo de verificación (`:390-397`:
`daemon_registered` en `:391` y `resident_alive_by_pid` en `:392`; ≈ 80 avisos
en ~40 vueltas de 200 ms en 8 s). El lazo sale en la primera vuelta si nada
está vivo (`:393`), así que el spam exige daemon vivo: el escenario de upgrade
con un `daemon.pid` anterior sin identidad. Que antes de T1 esa función no
avisara es origen histórico no verificable por lectura (sin diff disponible; no
afecta a la solución). **Categoría 1. VERIFICADO.**

**Solución.** Separar observación de reporte. Extraer de `read_pid_file` una
lectura sin efectos que devuelva el desenlace (ausente, descartado con su
motivo, presente); `read_pid_file` queda como esa lectura más el `eprintln!` del
motivo, con el mismo texto y la misma firma, de modo que `main.rs:2649/:2684`
(decisiones de la CLI) y los 9 sitios de test que la llaman directo no cambian.
Los 5 helpers internos (`read_pid`, `read_resident_pid`, `read_addr`,
`daemon_registered`, `resident_registered`) pasan a la lectura sin efectos, que
los vuelve silenciosos. `stop` avisa una sola vez, al entrar y antes de
`resolve_addr` (`:343`): observa el desenlace y reporta si fue descartado; el
resto de sus lecturas (`:344`, `:366`, `:379`, `:400`, `:406` y el lazo) quedan
silenciosas. Avisar también en `:344` y `:400`, como proponía una versión
anterior de este análisis, dejaría 3 avisos idénticos por parada. `read_pid`,
`read_resident_pid` y `read_addr` no tienen llamadores fuera de `daemon_stop.rs`
(barrido de `crates`, `src`, `tests`). Ningún test afirma el texto del aviso
(barrido de `descartado`: solo los 3 `eprintln!` y usos ajenos en `main.rs`,
`avi-daemon` y `avi-tts`).

### H9 — Caso rancio sin documentar

Sin `start=`/`image=`, `recorded = None` (`daemon_stop.rs:531-534`) y, sin la
rama `None → Ours` que T1 retiró, el fichero se clasifica `Absent` y **se
borra** (`:435-437`); antes se conservaba. Ninguna enumeración nombra el quinto
caso, PID vivo sin identidad: ni el comentario de `stop` (`:432-434`, enumera 3:
ilegible, sin PID, PID propio), ni el doc de `ready_file_owner_alive`
(`:516-518`, enumera 4: ausente, ilegible, sin línea `pid=`, PID propio), ni la
guía (`docs/CLI/commands/DAEMON.md:82`), cuya frase «un fichero sin identidad
registrada es ilegible: se descarta con aviso» describe al pidfile y, leída en
contexto, parece aplicar también al ready, que se borra por rancio y sin aviso.
El texto ya existe en `docs/CLI/CONTRACT.md:656` («un ready sin las dos no
declara a su PID como propio (cuenta como muerto, no bloquea la parada y se
borra por rancio)») y en `CHANGELOG.md:114`. Los dos comentarios del código
omiten además el PID reasignado a otro programa (identidad que no coincide,
ya cubierto por la guía y por una prueba, `:1452`). **Categoría 1. VERIFICADO.**

**Solución.** Propagar la enumeración a los tres sitios: copiar el texto de
`CONTRACT.md:656`, sin redactar nada nuevo. En los dos comentarios del código,
añadir los dos casos omitidos; en la guía, solo el de identidad ausente. El
comportamiento es correcto y principista (sin identidad no hay propiedad
demostrable); terminar el cambio no es parchear.

### H12 — Arista avi-shared a avi-process

`avi-shared/Cargo.toml:12` consume `pid_alive_exact` (vía el wrapper privado
`pid_alive`, `logs.rs:154-156`, que invoca el closure de `create_log_at`),
símbolo de definición T2 (plan:96), y arrastra `avi-process` a todo
consumidor de `avi-store` y `avi-lifecycle` (re-exportan `create_log`:
`avi-store/src/lib.rs:8`, `avi-lifecycle/src/lib.rs:42`; consumidores en
`avi-tts/src/lib.rs:952/1080` y `src/main.rs:3523/3564/3688`). El plan T7
ofrecía "trait o parámetro inyectado". **Categoría 1. VERIFICADO.**

**Solución.** La costura ya existe: `create_log_at_with` y `prune` reciben el
sondeo; solo `create_log_at` (`logs.rs:46-55`, closure en `:52-54`) lo
hardcodea. Los 5 llamadores no llaman a `create_log_at` (privada) sino a
`create_log` (`:24`, la API pública), así que el cambio es de firma: `create_log`
recibe el sondeo (`&dyn Fn(u32) -> bool`) y llama a `create_log_at_with`. Se
borran `create_log_at`, el wrapper `pid_alive` y la dependencia
(`avi-shared/Cargo.toml:9-12`). La exclusión del PID propio (hoy dentro del
closure; `prune` documenta que «el PID propio no se consulta») es invariante de
la poda y no puede replicarse en cada llamador: se mueve dentro de
`create_log_at_with`/`prune`, de modo que los llamadores pasan el sondeo puro,
`&avi_process::pid_alive_exact` (`fn(u32) -> bool`, una por plataforma,
`avi-process/src/lib.rs:137/:166`). Llamadores (5; barrido en `crates`, `src`,
`tests`, `xtask` y `benches`, sin más alias tras los re-exports
`avi-store/src/lib.rs:8` y `avi-lifecycle/src/lib.rs:42`): 3 en
`main.rs:3523/3564/3688` —los dos primeros bajo `cfg(windows)`— y 2 en
`avi-tts:952/1080`. De paso, borrar `create_log_for_pid` (`logs.rs:32-42`): sin
llamador en producción (solo su propio test, `:261`) y su propósito documentado
lo sirve `rename_log_to_pid`, que sí se usa (`main.rs:3720`); al borrarla, la
mención de `create_log_for_pid` en §5 («Allows y matices de código muerto») queda
obsoleta y se retira. Sin altas en `Cargo.toml`: la raíz (`:25`) y `avi-tts`
(`:14`) ya dependen de `avi-process`. Las 7 llamadas de test a `create_log_at`
(`:194/:216/:219/:238/:239/:248/:329`) pasan a `create_log_at_with` con un
sondeo fijo; se borra el test `:261`.

## 3. Hallazgos parciales y pendientes de decisión

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

### H3a — kill bloqueante en stop async (diseño pendiente)

`spawn.rs:112-114` documenta la partición: lo bloqueante vive fuera del
runtime. `kill_tree_by_pid_exact` es `Command::new("taskkill")...status()`
bloqueante sin plazo (`avi-process/src/lib.rs:198-205`) y `stop()` es async
(usa `.await` en `daemon_stop.rs:348`; el kill va en `:370`). **Categoría 1.
VERIFICADO** (que `taskkill` llegue a colgarse: HIPÓTESIS).

**Por qué no es trivial (degradado desde §2 en verificación).** "Envolver en
`spawn_blocking`" no compila tal cual: `stop()` recibe `control: &dyn
ProcessControl` prestado (no-`'static`, `daemon_stop.rs:337-341`), el trait no
exige `Send`/`Sync` (`:256-275`), `spawn_blocking` exige `'static` y
`block_in_place` exige `Send` más runtime multi-hilo (los `#[tokio::test]` usan
`current_thread` por defecto: pánico). Diseño pendiente: `Arc<dyn
ProcessControl + Send + Sync>` (todos los impls ya son `Send+Sync`: solo
Atomics/`Mutex`/`Vec`/`u32`/unitaria) + cambio de firma de `stop()` + ~10
call sites. Redefinible con precisión, pero inejecutable como estaba enunciado.
**Decisión abierta.**

### H10 — Tests por substring (garantía sin reemplazo)

`daemon_stop.rs:1478-1480` lee `src/main.rs` vía
`CARGO_MANIFEST_DIR/../../src/main.rs` y afirma por substring, con `concat()`
para no autodetectarse (`:1502-1503`, `:1530-1532`). Se rompen con un
`use ... as` inocuo; pasan si se redeclara con otro nombre. **Categoría 1.
VERIFICADO.**

**Por qué no es trivial (degradado desde §2 en auditoría adversarial).**
Borrar los dos bloques (`:1501-1519` y `:1533-1550`, más el helper
`:1478-1481`) deja ambos tests con aserciones (valores en `:1489`, `:1497` y
`:1554-1558`), pero esas solo afirman valores (8 s, 8765, convergencia de
resolvers): la garantía de fuente única —el lanzador no redeclara plazos ni
dirección y los toma de `daemon_stop`— desaparece sin reemplazo. Lo que queda
NO protege lo mismo. Decisión abierta: aceptar la pérdida o reubicar el check
sin leer el workspace (p. ej. vía API o en build-time).

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
