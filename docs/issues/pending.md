# Pendientes de la purga de sandboxes de prueba huérfanos

Este documento recoge lo que quedó **pendiente de analizar o de corregir** en la
poda de sandboxes de prueba que `cargo xtask clean --prune` realiza.

**Estado actual: los cuatro pendientes están cerrados.** El texto conserva la
descripción de cada uno tal como se detectó —qué se vio, por qué importaba y
qué había que decidir—, y cada uno añade al final cómo se cerró. Se conserva
como registro: los documentos de este directorio no se borran, se anotan.

No es un registro de defectos: son trabajos que se detectaron al implementar
esa poda y que quedaron deliberadamente fuera de su alcance, cada uno con su
motivo y con lo que habría que decidir para cerrarlo. Se lee sin conocer la
sesión en que se detectaron: cada punto explica el estado del código, con los
símbolos y las rutas suficientes para revisar la afirmación.

## Qué hace hoy la poda

`cargo xtask clean --prune` borra, entre otras cosas, los **sandboxes de prueba
huérfanos**: los directorios que las pruebas crean con `tempfile` y que el
`temporal` del sistema conserva cuando la corrida se interrumpe (un `Ctrl-C`, un
pánico no capturado, un cierre de sesión cortan el proceso antes de que el `Drop`
corra y borre la carpeta).

La poda reconoce esos directorios por el **prefijo** de su nombre y los retira solo
cuando son más antiguos que un día. El criterio es la antigüedad porque el nombre
de un sandbox de `tempfile` es `<prefijo><etiqueta de la prueba>_<aleatorio>`: no
lleva PID, así que la antigüedad es lo único que distingue el residuo del sandbox
de una corrida que está en marcha.

**Hay una excepción desde el cierre de los pendientes**: la familia del sandbox
de ciclo de vida lleva el PID en su nombre, y por eso se decide por el proceso y
no por la antigüedad. El detalle está en el pendiente 2, ya cerrado.

El código que lo hace está repartido así:

- `crates/avi-shared/src/paths.rs` — la lista de prefijos reconocidos
  (`TEST_SANDBOX_PREFIXES` y las constantes individuales que la componen) y
  `system_temp_dir()`, que resuelve la raíz del temporal del sistema.
- `crates/xtask/src/clean.rs` — `prune_old_test_sandboxes` (descarta por
  antigüedad, y por PID en la familia del sandbox de ciclo de vida),
  `prune_dead_process_dirs` (descarta por PID muerto) y
  `prune_system_temp_sandboxes`, que las combina sobre la raíz del temporal.

**Las dos raíces no se tratan igual.** La poda de `target/` reconoce tres clases
de directorio —los que empiezan por `.tmp`, el que se llama `tmp` y los de la
lista de prefijos—, pero la poda del temporal del sistema reconoce **solo** la
lista de prefijos y la raíz `golden-avi-daemon`. La diferencia importa, y es el
pendiente 4.

## Pendiente 1 — Los prefijos del motor que quedan fuera de la purga

**Estado:** cerrado.
**Tipo:** cobertura incompleta de una capacidad ya existente.
**Gravedad del hueco:** baja en el estado actual; media si vuelve a haber corridas
que no pasen por Cargo.

La lista `TEST_SANDBOX_PREFIXES` reconocía tres prefijos:
`golden-sandbox_`, `cleanup_sandbox_` y `contract-sandbox_`. El motor crea
sandboxes con más prefijos, y la mayoría no los reconocía la poda.

### Prefijos que crean directorios

| Prefijo | Dónde se crea | ¿El `Drop` lo borra? |
|---|---|---|
| `sandbox-{tag}-{pid}-{nanos}` | `crates/avi-lifecycle/tests/support/mod.rs`, `Sandbox::unique_root` | Sí, con `remove_dir_all` — pero **no** es `tempfile` (ver el pendiente 2) |
| `avi_store_test_` | `crates/avi-store/src/lib.rs`, helper `temp_dir` | Sí, con `TempDir` |
| `avi_daemon_test_state_` | `crates/avi-daemon/src/lib.rs`, helper `test_state` | Sí, con `TempDir` |
| `avi-tts-double-` | `crates/avi-tts/src/lib.rs` | Sí, con `TempDir` |
| `avi_ct2_broken_` | `crates/avi-translation/src/lib.rs` | Sí, con `TempDir` |
| `avi_store_path_entry_` | `crates/avi-store/src/lib.rs` | Sí, con `TempDir` |
| `avi_store_hf_layout_` | `crates/avi-store/tests/hf_cache_layout.rs` | Sí, con `TempDir` |
| `avi_daemon_clone_operation_` | `crates/avi-daemon/tests/golden.rs` | Sí, con `TempDir` |
| `lifecycle-test-` | `crates/avi-lifecycle/src/lib.rs`, `scratch` | Sí, con `TempDir` |

### Prefijos que crean ficheros

La poda recorre directorios, así que estos no la alcanzan:

| Prefijo | Dónde se crea | Qué crea |
|---|---|---|
| `avi_audio_test_` | `crates/avi-audio/src/lib.rs`, helper `temp_wav_path` | un `.wav` suelto, con `TempPath` |
| `xtask-ort-` | `crates/xtask/src/package.rs` | el ONNX Runtime descargado, con `tempfile()` |

### Prefijos que crean directorios, de las pruebas de `xtask`

Las pruebas del propio `xtask` usan temporales con dueño y también dejan residuo
si la corrida se interrumpe. Sus prefijos:

| Prefijo | Dónde se crea |
|---|---|
| `xtask_clean_`, `xtask_clean_sandbox_`, `xtask_clean_repo_`, `xtask_clean_prune_`, `xtask_clean_pid_`, `xtask_clean_pacman_`, `xtask_clean_log_`, `xtask_clean_global_` | `crates/xtask/src/clean.rs` |
| `xtask-comments-` y sus variantes `xtask-comments-bin-`, `xtask-comments-empty-`, `xtask-comments-allow-` | `crates/xtask/src/comments.rs` |
| `xtask-language-` y su variante `xtask-language-empty-` | `crates/xtask/src/language.rs` |
| `xtask_doctor_` y su variante `xtask_doctor_onnx_` | `crates/xtask/src/doctor.rs` |
| `xtask_install_` y su variante `xtask_install_rm_` | `crates/xtask/src/install.rs` |
| `xtask_package_zip_`, `xtask_package_symlink_`, `xtask_package_ort_`, `xtask_package_layout_`, `xtask-package-` | `crates/xtask/src/package.rs` |
| `xtask_pins_` | `crates/xtask/src/pins.rs` |

Los que se diferencian solo por un sufijo son **variantes de un mismo prefijo**: el
prefijo más corto ya los cubre, porque la poda compara por `starts_with`.

**Confirmado** en el código: los directorios de esta tabla usan `tempfile` (o un
`Drop` propio) y ninguno sobrevive a una corrida que termina bien.

**Por qué no se incluyeron en el encargo original:** el encargo nominaba tres
prefijos concretos. El razonamiento que se aplicó a ellos —la lista en
`avi-shared` como fuente única, el prefijo completo con el nombre del motor
dentro, la antigüedad como discriminante— es idéntico para el resto, así que
incluirlos es mecánico y no cambia el diseño ni el discriminante: la función
consume la lista con `any(|prefix| name.starts_with(prefix))` y no requiere más
cambios.

**Lo que había que decidir:** nada, en principio. Ampliar la lista es un cambio de
datos, no de lógica. Las dos excepciones son `sandbox-{tag}-{pid}-{nanos}`, que
tiene su propio pendiente (el 2) porque admite un discriminador mejor, y los
prefijos de fichero, que quedan fuera por no ser directorios.

### Cómo se cerró

La lista `TEST_SANDBOX_PREFIXES` de `crates/avi-shared/src/paths.rs` incluye hoy
los diecisiete prefijos de directorio de las tablas de arriba, más el prefijo
propio del sandbox de ciclo de vida (`ai-voice-interconnector-sandbox-`) y el del
temporal del test del menú (`ai-voice-interconnector-menu-`): **veintiuna
entradas** en total.

Los prefijos de fichero (`avi_audio_test_` y `xtask-ort-`) siguen fuera, porque
la poda recorre directorios, y `avi-paths-` sigue fuera, por lo que dice el
pendiente 3.

La prueba `prune_old_test_sandboxes_selects_by_age` de `crates/xtask/src/clean.rs`
recorre ahora la lista completa, salvo la familia del sandbox de ciclo de vida,
que tiene su propia prueba porque su criterio es distinto.

## Pendiente 2 — El sandbox de `avi-lifecycle` admite un barrido por PID

**Estado:** cerrado.
**Tipo:** decisión de diseño sobre el criterio de borrado.

El sandbox de `avi-lifecycle` no se nombra con `tempfile`, sino con una ruta
construida a mano en `Sandbox::unique_root`
(`crates/avi-lifecycle/tests/support/mod.rs`):

```rust
std::env::temp_dir().join(format!("sandbox-{tag}-{}-{n}", std::process::id()))
```

Su nombre lleva, por tanto, **el PID del proceso que lo creó**, además de una
marca de tiempo en nanosegundos. Esto es lo mismo que hace la raíz
`golden-avi-daemon/<pid>` de las pruebas doradas del daemon, que el propio harness
barre por PID muerto (`sweep_dead_process_dirs` en
`crates/avi-daemon/tests/golden.rs`).

**Por qué importa:** si este sandbox entra en la lista de prefijos y se purga por
antigüedad, se incurre en el defecto que el barrido por PID evita. Un proceso de
pruebas lento pero **vivo** —una prueba que instale modelos, por ejemplo— puede
tener un sandbox de más de un día, y la regla de antigüedad lo borraría de debajo
de las patas, rompiendo la corrida.

**Lo que había que decidir:** si este sandbox se purga por antigüedad (junto a los
demás) o por PID. La opción fina es la segunda: se borra el sandbox de un proceso
**muerto** aunque sea reciente, y se respeta el de un proceso **vivo** aunque sea
**viejo**. El requisito es parsear el PID del nombre —el PID es el penúltimo
componente, separado por guiones, y la etiqueta puede contener guiones, así que
conviene parsear desde la derecha— y comprobar la vivencia con
`avi_process::pid_alive_exact`, que ya está disponible y es lo que usa
`prune_dead_process_dirs`.

**Estado en el momento de detectarlo:** la lista de prefijos de `avi-shared` no
reconocía el sandbox de `avi-lifecycle` **ni por antigüedad ni por PID**: no estaba
en `TEST_SANDBOX_PREFIXES` y `prune_dead_process_dirs` barre la raíz
`golden-avi-daemon`, no la raíz de este sandbox. Quedaba, por tanto, sin purgar.

**Cuidado que había que tener al implementarlo:** `crates/avi-lifecycle/tests/recovery.rs`
afirma, en la prueba `sweep_never_touches_a_foreign_sandbox`, que un directorio
con forma de sandbox ajeno al scratch **no se toca** al barrer. La poda no es ese
barrido —vive en otro crate y barre otra raíz—, pero cualquier cambio en los
nombres de los sandboxes de `avi-lifecycle` obliga a revisar esa aserción, porque
la prueba usaba literalmente el nombre `golden-sandbox_invariant_12345_0` como
ejemplo de «sandbox ajeno».

**Cuidado adicional:** el prefijo `sandbox-` es **genérico**. A diferencia del
resto, no lleva el nombre del motor dentro, así que podría coincidir con un
directorio de otro proyecto que use el mismo prefijo en el temporal del sistema.
Antes de incluirlo había que decidir cómo se acota esa coincidencia.

### Cómo se cerró

**Criterio: por PID.** Un PID muerto permite retirarlo aunque sea reciente; uno
vivo lo protege aunque sea viejo. Las demás familias conservan el umbral de un
día, porque su nombre no lleva PID.

**Nombre: con el nombre del motor.** El sandbox se llama ahora
`ai-voice-interconnector-sandbox-{tag}-{pid}-{nanos}`. El nombre completo lleva
dentro al motor, de modo que no puede coincidir con el temporal de otro
proyecto, y no colisiona con los demás prefijos: `cleanup_sandbox_` usa guion
bajo donde este usa guion.

**Forma exigida.** Solo se admite el nombre completo: etiqueta no vacía formada
por componentes válidos, PID decimal que cabe en `u32` y nanosegundos decimales
no vacíos. Un nombre incompleto o inválido se deja intacto, porque la poda solo
borra lo que reconoce por nombre del motor. El parseo vive en
`lifecycle_sandbox_pid` de `crates/xtask/src/clean.rs` y toma el PID desde la
derecha, porque la etiqueta admite guiones.

**La aserción de sandbox ajeno se conservó.** La prueba
`sweep_never_touches_a_foreign_sandbox` sigue verificando que un directorio con
forma de sandbox ajeno al scratch no se toca; su ejemplo pasó a ser
`other-project-sandbox-invariant-12345-0`, que no lleva el nombre del motor y por
eso es ajeno a propósito.

**Las pruebas que lo cubren** son
`prune_lifecycle_sandboxes_by_pid_and_complete_name` en
`crates/xtask/src/clean.rs` —que afirma que un PID muerto se retira aunque sea
reciente, que uno vivo se respeta aunque sea viejo y que los nombres incompletos
no se tocan— y `sweep_never_touches_a_foreign_sandbox` en
`crates/avi-lifecycle/tests/recovery.rs`.

## Pendiente 3 — `avi-paths-`: una base que las pruebas nombran y no crean

**Estado:** cerrado, con la decisión de **no** incluirlo en la purga.
**Tipo:** inconsistencia de una prueba.

Las pruebas de `crates/avi-shared/src/paths.rs` usan `EnvSandbox`, una guarda que
fija las variables de entorno del proceso y las restaura al soltarse. Su `Drop`
**restaura las variables y nada más**: no borra el directorio base. Y la base no
llega a crearse nunca, porque la guarda tampoco la crea —solo fija variables
como `LOCALAPPDATA`, `HOME` y `XDG_*` apuntando dentro de ella.

Ese directorio se llama `avi-paths-default`, `avi-paths-cache`, `avi-paths-data`
o `avi-paths-hf`, según la prueba, y cuelga de `std::env::temp_dir()`.

**Comprobado:** tras ejecutar la suite de `avi-shared` —39 pruebas, las que viven
dentro del módulo `paths`—, no queda ningún directorio `avi-paths-*` en `target/`
ni en el temporal del sistema, porque las pruebas no crean contenido bajo esa
base: la guarda solo fija variables de entorno y las aserciones comparan rutas,
sin escribir dentro.

**Por qué no debería entrar en la purga:** no hay nada que purgar. No es un
residuo de una corrida interrumpida, sino una ruta que las pruebas nombran y no
llegan a materializar. Incluirlo sería añadir una regla de borrado sin un caso
detrás, sobre un nombre que —a diferencia de los otros— no identifica un
directorio del motor sino una base vacía.

**Lo que sí convendría corregir, si se decide tocarlo:** dar a `EnvSandbox` una
garantía de limpieza, para que el directorio que sus aserciones nombran no
dependa de que ninguna prueba escriba dentro. Es un cambio local a `avi-shared` y
no afecta a la poda.

### Cómo se cerró

`avi-paths-` **queda fuera de la purga**: no hay residuo que retirar y añadir una
regla de borrado sin un caso detrás sería una condición que la poda no necesita.
La lista de prefijos no lo incluye.

La segunda parte —dar a `EnvSandbox` una garantía de limpieza de su base— **no se
hizo**: es un cambio local a `avi-shared` que no pertenece a la poda y que no
exigía ninguna de las decisiones abiertas. Sigue siendo una mejora opcional, sin
efecto sobre la limpieza de temporales.

## Pendiente 4 — El prefijo `.tmp` se purga en `target/` y no en el temporal del sistema

**Estado:** cerrado.
**Tipo:** cobertura incompleta de una capacidad ya existente.

Las dos raíces de la poda no aplican los mismos criterios, y hay una clase de
directorio que una cubre y la otra no.

**Lo que cubre cada raíz.** La poda de `target/` recorre sus entradas y
selecciona las que cumplen una de estas condiciones:

- el nombre empieza por `.tmp`;
- el nombre es `tmp`;
- el nombre empieza por uno de los prefijos de `TEST_SANDBOX_PREFIXES`.

La poda del temporal del sistema, en cambio, solo aplica las dos últimas: la
lista de prefijos y el barrido por PID sobre la raíz `golden-avi-daemon`. **El
criterio `.tmp` no está en ella.**

**Por qué importa.** `tempfile` usa `.tmp` como prefijo por defecto cuando el
constructor no recibe uno —comprobado: un `tempfile::tempdir()` se llama
`.tmpXXXXXX`—. En el repositorio hay al menos un caso de ese tipo: `src/main.rs`,
en el helper `menu_fixture`, crea el directorio temporal del test del menú con
`tempfile::tempdir()`, sin prefijo explícito. Ese directorio hoy:
- si cae en `target/`, lo retira la regla `starts_with(".tmp")` de la poda de
  `target/`;
- si cae en el temporal del sistema, **no lo retira nadie**: no está en
  `TEST_SANDBOX_PREFIXES` y la regla `.tmp` no se aplica a esa raíz.

**Lo que había que decidir:** si la poda del temporal del sistema debe reconocer
también el prefijo `.tmp` —y con qué criterio de antigüedad—, o si conviene
nombrar el directorio del test del menú con un prefijo explícito del motor, que
es lo que hacen los demás sandboxes y encaja con la lista.

**La segunda opción tiene una ventaja que conviene pesar:** un prefijo
explícito hace el directorio identificable y lo mete en la lista, de modo que
una sola regla lo cubre en las dos raíces. La primera deja la excepción en el
código de la poda y obliga a mantener sincronizadas dos listas de criterios que
hoy ya difieren.

**Nota sobre el alcance:** esto solo afecta a lo que caiga en el temporal del
sistema. Con el confine de `TMP` a `target/` de `.cargo/config.toml`, un
`cargo test` no deja nada ahí; el hueco se manifiesta con corridas que no pasan
por Cargo, que es el mismo escenario que motiva la purga entera.

### Cómo se cerró

**Se nombró el temporal con un prefijo explícito del motor.** `menu_fixture`, en
`src/main.rs`, crea ahora su directorio con
`tempfile::Builder::new().prefix(avi_shared::paths::MENU_SANDBOX_PREFIX)`, y el
prefijo es `ai-voice-interconnector-menu-`: entra en la lista y una sola regla lo
cubre en las dos raíces.

**La poda no se amplió al prefijo `.tmp` genérico.** `.tmp` es el prefijo por
defecto de `tempfile`, no del motor: borrarlo sería borrar temporales de otros
proyectos que comparten el temporal del sistema. El criterio del motor es
"la poda solo borra lo que reconoce por nombre del motor", y ampliarse a `.tmp`
lo rompería.

## Comprobación del estado actual

En el momento de redactar este documento, el residuo de **todos** los prefijos
listados —los tres cubiertos y los pendientes— es **cero**, tanto en `target/`
como en el temporal del sistema.

La causa es que `.cargo/config.toml` fija `TMP`, `TEMP` y `TMPDIR` a `target/`
con `force = true`: todo lo que lanza Cargo —las pruebas, `cargo run`,
`cargo xtask`— escribe sus temporales dentro de `target/`, que `cargo clean` y
`cargo xtask clean` ya vacían.

Los sandboxes que se acumulaban fuera de `target/` eran residuos anteriores a ese
confinamiento, o de corridas que no pasaron por Cargo. Por eso la purga del
temporal del sistema **sí** hace falta —ese es el hueco que motiva el trabajo—, y
por eso los prefijos pendientes **no** acumulan hoy: sus residuos caen dentro de
`target/`.

**Consecuencia que conviene tener presente:** ampliar la lista de prefijos
cambia el comportamiento de `target/` también, no solo el del temporal del
sistema. La poda de `target/` reconoce hoy los sandboxes por dos vías: el
criterio general `starts_with(".tmp")` o `name == "tmp"`, y la lista de prefijos
del motor que se aplica a `target/` con el mismo criterio de antigüedad. Los
prefijos que quedan fuera de esa lista no los reconoce ninguna de las dos reglas,
de modo que sus residuos en `target/` no los retira nadie. Ampliar la lista es, por
tanto, lo que cierra ese hueco también.

Lo mismo vale para el prefijo `.tmp` del pendiente 4, con la diferencia de que
ese lo cubre la regla general de `target/` y no la del temporal del sistema: hay
que resolverlo en las dos raíces o cambiar el nombre del directorio que lo origina.

## Cómo se cerró cada pendiente

- **Pendiente 1:** la lista `TEST_SANDBOX_PREFIXES` de
  `crates/avi-shared/src/paths.rs` incluye los diecisiete prefijos de directorio
  pendientes, más los dos nombres propios que el cierre de los pendientes 2 y 4
  añaden. La prueba `prune_old_test_sandboxes_selects_by_age` de
  `crates/xtask/src/clean.rs` recorre ahora la lista completa. Quedan fuera de la
  lista los prefijos de fichero (`avi_audio_test_` y `xtask-ort-`), que no son
  directorios, y `avi-paths-`, que corresponde al pendiente 3.
- **Pendiente 2:** el sandbox de ciclo de vida se llama
  `ai-voice-interconnector-sandbox-{tag}-{pid}-{nanos}` y se purga por PID
  (`lifecycle_sandbox_pid` y `avi_process::pid_alive_exact` en
  `crates/xtask/src/clean.rs`). La colisión con otros proyectos se acota con el
  nombre del motor y con la exigencia de la forma completa. La aserción de
  `crates/avi-lifecycle/tests/recovery.rs` que usa un nombre de sandbox como
  ejemplo de «ajeno» se conserva, con un ejemplo ajeno de verdad.
- **Pendiente 3:** `avi-paths-` queda fuera de la purga. La garantía de limpieza
  de la base de `EnvSandbox` no se hizo, por no pertenecer a la poda.
- **Pendiente 4:** el temporal del test del menú lleva el prefijo explícito
  `ai-voice-interconnector-menu-`, y la poda del temporal del sistema no reconoce
  el prefijo `.tmp` genérico.

## Referencia canónica

El comportamiento vigente de la poda está en
[`docs/specs/LIFECYCLE.md`](../specs/LIFECYCLE.md), en la sección de `cargo
xtask clean`, y su alcance en el `CHANGELOG.md`, en la sección «No publicado».
