# Pendientes de la purga de sandboxes de prueba huérfanos

Este documento recoge lo que queda **pendiente de analizar o de corregir** en la
poda de sandboxes de prueba que `cargo xtask clean --prune` realiza.

No es un registro de defectos: son trabajos que se detectaron al implementar
esa poda y que quedaron deliberadamente fuera de su alcance, cada uno con su
motivo y con lo que habría que decidir para cerrarlo. Se lee sin conocer la
sesión en que se detectaron: cada punto explica el estado actual del código, con
los símbolos y las rutas suficientes para revisar la afirmación.

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

El código que lo hace está repartido así:

- `crates/avi-shared/src/paths.rs` — la lista de prefijos reconocidos
  (`TEST_SANDBOX_PREFIXES` y las constantes individuales que la componen) y
  `system_temp_dir()`, que resuelve la raíz del temporal del sistema.
- `crates/xtask/src/clean.rs` — `prune_old_test_sandboxes` (descarta por
  antigüedad), `prune_dead_process_dirs` (descarta por PID muerto) y
  `prune_system_temp_sandboxes`, que las combina sobre la raíz del temporal.

**Las dos raíces no se tratan igual.** La poda de `target/` reconoce tres clases
de directorio —los que empiezan por `.tmp`, el que se llama `tmp` y los de la
lista de prefijos—, pero la poda del temporal del sistema reconoce **solo** la
lista de prefijos y la raíz `golden-avi-daemon`. La diferencia importa, y es el
pendiente 4.

## Pendiente 1 — Los prefijos del motor que quedan fuera de la purga

**Estado:** abierto.
**Tipo:** cobertura incompleta de una capacidad ya existente.
**Gravedad del hueco:** baja en el estado actual; media si vuelve a haber corridas
que no pasen por Cargo.

La lista `TEST_SANDBOX_PREFIXES` reconoce hoy tres prefijos:
`golden-sandbox_`, `cleanup_sandbox_` y `contract-sandbox_`. El motor crea
sandboxes con más prefijos, y la mayoría no los reconoce la poda.

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
| `xtask_clean_size_`, `xtask_clean_sandbox_`, `xtask_clean_repo_`, `xtask_clean_prune_`, `xtask_clean_pid_`, `xtask_clean_pacman_`, `xtask_clean_log_`, `xtask_clean_global_` | `crates/xtask/src/clean.rs` |
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

**Lo que hay que decidir:** nada, en principio. Ampliar la lista es un cambio de
datos, no de lógica. Las dos excepciones son `sandbox-{tag}-{pid}-{nanos}`, que
tiene su propio pendiente (el 2) porque admite un discriminador mejor, y los
prefijos de fichero, que quedan fuera por no ser directorios.

## Pendiente 2 — El sandbox de `avi-lifecycle` admite un barrido por PID

**Estado:** abierto.
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

**Lo que hay que decidir:** si este sandbox se purga por antigüedad (junto a los
demás) o por PID. La opción fina es la segunda: se borra el sandbox de un proceso
**muerto** aunque sea reciente, y se respeta el de un proceso **vivo** aunque sea
viejo. El requisito es parsear el PID del nombre —el PID es el penúltimo
componente, separado por guiones, y la etiqueta puede contener guiones, así que
conviene parsear desde la derecha— y comprobar la vivencia con
`avi_process::pid_alive_exact`, que ya está disponible y es lo que usa
`prune_dead_process_dirs`.

**Estado actual:** la lista de prefijos de `avi-shared` reconoce el sandbox de
`avi-lifecycle` **ni por antigüedad ni por PID**: no está en
`TEST_SANDBOX_PREFIXES` y `prune_dead_process_dirs` barre la raíz
`golden-avi-daemon`, no la raíz de este sandbox. Queda, por tanto, sin purgar.

**Cuidado que hay que tener al implementarlo:** `crates/avi-lifecycle/tests/recovery.rs`
afirma, en la prueba `sweep_never_touches_a_foreign_sandbox`, que un directorio
con forma de sandbox ajeno al scratch **no se toca** al barrer. La poda no es ese
barrido —vive en otro crate y barre otra raíz—, pero cualquier cambio en los
nombres de los sandboxes de `avi-lifecycle` obliga a revisar esa aserción, porque
la prueba usa literalmente el nombre `golden-sandbox_invariant_12345_0` como
ejemplo de «sandbox ajeno».

**Cuidado adicional:** el prefijo `sandbox-` es **genérico**. A diferencia del
resto, no lleva el nombre del motor dentro, así que podría coincidir con un
directorio de otro proyecto que use el mismo prefijo en el temporal del sistema.
Antes de incluirlo conviene decidir cómo se acota esa coincidencia.

## Pendiente 3 — `avi-paths-`: una base que las pruebas nombran y no crean

**Estado:** abierto, con recomendación de **no** incluirlo en la purga.
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

## Pendiente 4 — El prefijo `.tmp` se purga en `target/` y no en el temporal del sistema

**Estado:** abierto.
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

**Lo que hay que decidir:** si la poda del temporal del sistema debe reconocer
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

## Cómo cerrar cada pendiente

- **Pendiente 1:** ampliar `TEST_SANDBOX_PREFIXES` en
  `crates/avi-shared/src/paths.rs` con los prefijos de directorio pendientes y
  ajustar la prueba `prune_old_test_sandboxes_selects_by_age` de
  `crates/xtask/src/clean.rs` para que recorra la lista completa. Sin cambios de
  discriminante ni de lógica. Quedan fuera de la lista los prefijos de fichero
  (`avi_audio_test_` y `xtask-ort-`), que no son directorios, y `avi-paths-`,
  que corresponde al pendiente 3.
- **Pendiente 2:** añadir un barrido por PID para el patrón
  `sandbox-{tag}-{pid}-{nanos}`, decidir cómo se acota la coincidencia del
  prefijo genérico `sandbox-` con otros proyectos, y revisar la aserción de
  `crates/avi-lifecycle/tests/recovery.rs` que usa un nombre de sandbox como
  ejemplo de «ajeno».
- **Pendiente 3:** dejar `avi-paths-` fuera de la purga y, si se desea, hacer que
  `EnvSandbox` borre su base al soltarse.
- **Pendiente 4:** decidir si la poda del temporal del sistema reconoce el
  prefijo `.tmp` —con qué criterio de antigüedad—, o si el directorio del test
  del menú nombra su temporal con un prefijo explícito del motor, que es lo que
  hacen los demás sandboxes.
