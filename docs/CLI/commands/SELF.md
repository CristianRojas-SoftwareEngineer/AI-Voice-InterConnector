# Grupo `self` — ciclo de vida de la instalación del usuario

`self` es el grupo con el que **el binario gestiona su propia instalación** y cubre los dos lados del ciclo de vida: `self install` instala o repara, `self update` actualiza a la última estable o a una concreta, y `self uninstall` desinstala.

La Normativa del grupo está en [`LIFECYCLE.md`](../../specs/LIFECYCLE.md) (la [superficie de comandos](../../specs/LIFECYCLE.md#54-superficie-de-comandos), las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales), [`self install`](../../specs/LIFECYCLE.md#83-self-install) y [`self uninstall`](../../specs/LIFECYCLE.md#85-self-uninstall)); el contrato de la CLI —flags, `reason`, códigos de salida, envelope `--json`— está en [`../CONTRACT.md`](../CONTRACT.md). Este documento describe **dónde vive cada cosa y por qué**.

**Implementación:** el motor es el crate `avi-lifecycle` (`crates/avi-lifecycle/src/install.rs` y `uninstall.rs`), sin punto de entrada propio: el parseo de la CLI y el cableado se quedan en `src/main.rs` (`handle_self`), porque el motor no depende de `clap` ni de `avi-core` (ver la [organización del repositorio](../../specs/LIFECYCLE.md#53-organización-del-repositorio)). El binario aporta las dos primitivas que el motor no puede tener —el control de procesos (`ProductProcesses`, que vive en `avi-daemon`/`avi-tts`) y el borrado diferido de Windows (`ProgramRemoval`, que usa `avi_process::schedule_removal`)—. El motor decide; el binario ejecuta esas dos.

---

## Definición CLI (parser)

`SelfSub` (`src/main.rs`), el enum de subcomandos. Se llama `SelfSub` y no `SelfComandos` por dos razones: `Self` es palabra reservada de Rust, y el nombre no debe colisionar por subcadena con el enum de comandos de nivel superior.

| Subcomando | Flags | Efecto |
|---|---|---|
| `self install` | `--no-setup` · `--no-modify-path` · `--force`/`-f` · `--yes` · `--json` | Instala el bundle del que forma parte el ejecutable, o repara la instalación si se ejecuta desde ella |
| `self update` | `--check` · `--version X.Y.Z` · `--force`/`-f` · `--no-setup` · `--yes` · `--json` | Actualiza la instalación registrada a la última estable o a una concreta, con verificación y traspaso |
| `self uninstall` | `--keep-data` · `--dry-run` · `--yes` · `--json` | Borra el estado, revierte el `PATH` y borra el directorio de programa |

`--json` es global (`Cli::json`), no un flag por subcomando. `self install` no tiene opción de canal: el recibo nuevo lleva el canal `script`, y reparar o reemplazar una instalación conserva el del recibo anterior. Los canales son `script`, `homebrew` y `unmanaged`; `cargo xtask install` delega en `self install` y deja una instalación `script`.

**Un recibo ilegible falla, no se ignora.** `Ok(None)` del recibo significa solo que el fichero no existe. Si `install-receipt.json` existe pero no se puede leer, no es JSON, tiene un campo desconocido o ausente, o declara un `schema_version` distinto del vigente, `self install`, `self update`, `self uninstall` y `doctor` fallan con su propio `reason` (`self_install_failed`, `self_update_failed`, `self_uninstall_failed`, `doctor_failed`; el resto de comandos, `lifecycle_failed`) y un mensaje que nombra la ruta del fichero.

---

## Los tres modos de `self install`

El modo lo decide **la posición del ejecutable**, no un flag:

| Modo | Condición | Qué hace |
|---|---|---|
| `Install` | El ejecutable está **fuera** del directorio de programa (bundle en staging o extraído a mano) | Instala, o reemplaza si ya hay una versión |
| `Repair` | El ejecutable está **dentro** del directorio de programa | Reaplica integración de `PATH`, permisos, cuarentena y recibo **sin copiar archivos** |
| — | El ejecutable **no tiene bundle alrededor** (por ejemplo `target\debug`) | No es un modo: es el desenlace de validar el bundle, `bundle_invalid` (**15**) con la indicación de usar `cargo xtask install` |

La comparación es por **clave canónica** (`avi_store::canonical_path_entry_matches`), no de cadenas: las dos rutas llegan de resoluciones distintas y una barra final de más daría un modo equivocado sin dar ningún error (`install::detect_mode`).

**El caso del ejecutable sin bundle es el que más confunde y el que hay que saber de memoria:** `target\debug\ai-voice-interconnector.exe self install` responde `bundle_invalid` con salida 15 no porque la instalación esté rota, sino porque no hay nada alrededor del binario que instalar. El bundle se produce con `cargo xtask package`.

---

## Los pasos de `self install`

El orden es el de [`self install`](../../specs/LIFECYCLE.md#83-self-install) y está escrito en el propio código, con el número de paso en cada bloque:

1. **Recuperación y bloqueo** (`install.rs`). La recuperación va **con el bloqueo tomado**, no antes: si fuera al revés, el barrido podría llevarse por delante un staging que otra operación está usando. El staging de la propia operación está a salvo sin ninguna excepción: lleva el token de este proceso vivo, y el barrido solo retira stagings de dueño muerto.
2. **Validar el bundle** contra la lista de archivos del target, fijada en compilación (`BUNDLE_MANIFEST`, embebido desde `packaging/bundle-manifest.json`). Es la misma lista que usa `cargo xtask package`, de modo que empaquetado e instalación no pueden divergir. Si falta un archivo → `bundle_invalid` (15), sin haber modificado nada.
3. **Detectar la instalación previa**: registrada (recibo) o ajena. Una ajena (un Cask en el `PATH`) **solo genera aviso** de coexistencia y precedencia: no bloquea.
4. **Resumen previo y confirmación.** Instalar una versión **menor** que la instalada es una degradación y se confirma como operación **destructiva** (`confirm::Kind::Destructive`), aunque instalar no borre nada por sí mismo. Cancelar aquí es salida 0 y ningún `reason`: las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) no cuentan la cancelación entre los `reason`. El resumen anuncia solo los cambios pendientes: el plan del `PATH` se calcula con el estado real (el valor `Path` del registro de usuario en Windows, el enlace del comando en Unix), y si no hay nada que cambiar la línea dice `<dir> ya está en el PATH; no se modifica` (`… en el PATH del usuario` en Windows). Un registro ilegible al planificar sale con `path_conflict` (14), igual que al aplicar.
5. **Parar el daemon**, incluido el proceso residente del motor. Si no se detiene → `daemon_stop_failed` (16), **sin modificar nada**.
6. **Reemplazo transaccional** (`transaction::replace`): aparcar, colocar, ajustar permisos, confirmar y borrar lo aparcado; revertir restaurando lo aparcado si falla → `rolled_back` (13). **En reparación no se copia nada.** El reemplazo parte siempre de un staging propio, hermano del directorio de programa y con el token del proceso en el nombre: si el bundle ya está en un staging con el token de este mismo proceso, se usa tal cual; en cualquier otro caso (la descarga del bootstrap, el staging de `self update`, que lleva el token del proceso que actualiza, o una extracción manual) `self install` lo copia, con el bloqueo ya tomado, a `.ai-voice-interconnector-staging-<token>-self-install` y lo borra al terminar, con aviso si no puede.
7. **macOS**: eliminar `com.apple.quarantine` de forma recursiva en **todo** el directorio de programa, no solo en el ejecutable, porque el motor y la librería de ONNX Runtime también se ejecutan o cargan. No es un fallo de la instalación: la cuarentena que no se quita degrada el arranque y el resumen lo informa.
8. **Integración de `PATH`** ([contrato de `self`](../CONTRACT.md#self-install-self-update-y-self-uninstall) e [integración de PATH](../../specs/LIFECYCLE.md#831-integración-de-path)). En Unix el `PATH` se modifica **por defecto** y se anuncia en el resumen; `--no-modify-path` lo desactiva. Un archivo ajeno en la ruta del enlace → `path_conflict` (14), salvo `--force`.
9. **Windows**: si el `PATH` de máquina parece llevar una instalación per-machine antigua, se avisa y se muestra el comando exacto para quitarla desde una PowerShell de administrador. **HKLM nunca se modifica**, ni para escribir ni para leer.
10. **Escribir el recibo** de forma atómica.
11. **La misma provisión que `setup`**, salvo `--no-setup`: descarga de la selección (todos los modelos, el Base de clonado incluido) según el estado real del almacén, sin repetir la confirmación. El bloqueo se retiene hasta aquí y se libera al terminar la provisión.
12. **Resumen final**: versión, rutas, estado del `PATH` y estado de los modelos.

**Lo que el recibo registra es el estado, no el diff de esta pasada.** El recibo dice qué integración de `PATH` está en pie, no qué cambió en esta ejecución: si registrara solo el diff, una segunda instalación desde el mismo bundle escribiría un recibo sin entrada de `PATH` y `self uninstall` no podría revertir la que puso la primera, dejando residuo dentro de una raíz de propiedad exclusiva. El diff vive aparte, en `Outcome::path_rewritten`, y es lo único que decide si el resumen pide abrir una terminal nueva.

**El resumen final lleva una sola línea del `PATH`, y es la del estado.** Copiar debajo la línea del plan imprimía «se añadirá X» y «se añadió X» en el mismo bloque, que parece una contradicción y no es más que el mismo hecho contado dos veces.

---

## `self update`: los pasos

El orden es el de [`self update`](../../specs/LIFECYCLE.md#84-self-update) y está escrito en el propio código (`handle_self`, brazo `Update`):

1. **Recuperación y bloqueo.** La recuperación corre con el bloqueo tomado sobre la instalación registrada; barre por identidad exacta los residuos de dueño muerto (scratch, temporales con token, stagings de instalación, aparcados y registros de borrado resueltos) y conserva e informa los de dueño no observable.
2. **Leer el recibo y el [canal](../../specs/LIFECYCLE.md#72-canales)**. `homebrew` → `externally_managed` (12) con el comando correcto; sin instalación → `not_installed` con el one-liner y salida 3.
3. **Resolver la versión objetivo**: `--version` explícito sin tocar la red, o la última estable siguiendo la redirección de `releases/latest` (con `AVI_DOWNLOAD_BASE_URL` si está definida); la API REST solo es respaldo.
4. **Comparar las versiones** con el comparador numérico vigente: iguales → `already_up_to_date` sin descargar, éxito con 0 (`--force` reinstala); objetivo menor → solo con `--version` explícito y marca destructiva.
5. **`--check`**: informa la transición (`anterior → nueva`, o que ya se está en la última) y termina sin cambios de actualización. En `--json`: `current`, `latest`, `update_available` y `channel`.
6. **Resumen y confirmación.** Normal `[S/n]`; degradación `[s/N]` con `confirmation_required` sin terminal.
7. **Preparar el bundle nuevo**: descarga del archivo y de `SHA256SUMS.txt` en un staging hermano con HTTPS y reintentos acotados, verificación de coincidencia exacta en `SHA256SUMS.txt` (discrepancia o línea ausente → `checksum_mismatch`, 21, con staging borrado), extracción y comprobación de arranque con coincidencia de `--version` (si no → `binary_incompatible`, 19) y validación contra el manifiesto.
8. **Parar el daemon con el binario actual**, que conoce su protocolo y su `daemon.pid`, anotando si estaba en ejecución. Si no se detiene → `daemon_stop_failed` (16) sin tocar nada y con el staging retirado.
9. **Traspaso**: ejecuta `<staging>/ai-voice-interconnector self install --yes` heredando la consola, con las preferencias del recibo (`--no-modify-path` si la instalación no tocaba el `PATH`), `--no-setup` si se pidió, y **`--force` propagado cuando el `update` lo recibió**. Se espera y se propaga el resultado: éxito, `setup_failed` parcial con `models_cause`, o `rolled_back`.
10. **Limpiar**: borra siempre el staging; los restos del aparcado que el reemplazo no pudo borrar por estar en uso (en Windows, los retiene el ejecutable del proceso que actualiza) se programan con el borrado diferido en vez de descartarse, o caen a la recuperación de la siguiente operación. En Unix no hay diferido.
11. **Resultado**: `anterior → nueva`. Sin reinicio automático del daemon: si estaba activo, el resumen indica cómo relanzarlo con el comando habitual.

**Garantías.** Un fallo antes del traspaso deja todo intacto (staging borrado, instalación intacta). Un fallo durante el traspaso revierte la transacción nueva del `self install` invocado. Una interrupción en cualquier punto queda recuperable en la siguiente operación de ciclo de vida.

---

## `setup_failed`: el único desenlace que no es ni éxito ni error

Si la provisión de `self install` (la misma de `setup`) falla, **la instalación no falla**. Las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) lo declaran éxito parcial, y el código lo modela así:

| | Valor |
|---|---|
| `status` | `installed` (o `repaired`) |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Forma de salida | **Por veredicto**: el envelope y el resumen se emiten igual, y **no** se adjunta el objeto `error` |
| Qué hacer | Reintentar con `setup`: el programa está instalado |

El motivo del fallo de provisión **no se pierde**: viaja anidado en `models_cause`, con su propio `reason` —`network_error` para un fallo de descarga— y su mensaje. Ese `reason` y `setup_failed` dicen cosas distintas y por eso no se funden: uno dice **qué** falló y el otro **qué dejó de completarse**.

```json
{
  "schema_version": "5",
  "status": "installed",
  "reason": "setup_failed",
  "install_dir": "…/ai-voice-interconnector",
  "version": "X.Y.Z",
  "channel": "script",
  "path_integrated": true,
  "models": "failed",
  "models_cause": { "reason": "network_error", "message": "…" }
}
```

`models_cause` **solo existe si hubo fallo**: un envelope estable es más fácil de leer que uno con nulos. Los cuatro valores de `models` son `skipped` (`--no-setup`), `already_provisioned`, `provisioned` y `failed`.

Cuando `setup` se invoca **directamente** (no desde `self install` ni desde el traspaso de `self update`), un fallo de descarga sale con `network_error` y **20**. El `self update` propaga el mismo parcial: si el `setup` del binario nuevo falla, el resultado es `updated` con `reason` `setup_failed`, salida 11 y causa anidada en `models_cause`.

---

## `self uninstall`: qué se borra y por qué

Los pasos de [`self uninstall`](../../specs/LIFECYCLE.md#85-self-uninstall), en orden, sobre la **instalación registrada** y no sobre la posición del ejecutable.

1. **El canal decide si se puede actuar**: `homebrew` → `externally_managed` (12) con el comando de Homebrew en el mensaje. Sin instalación, sin estado y sin directorio de programa → **`not_installed` y salida 0**.
2. **`--dry-run`**: imprime el plan y termina **sin tomar el bloqueo**: tomar el bloqueo crea el archivo, y una simulación que deja un archivo detrás no es una simulación.
3. **Plan** con rutas y tamaños, más la lista de lo que **no** se tocará y por qué.
4. **Confirmación destructiva** `[s/N]`. Sin terminal y sin `--yes` → `confirmation_required` (2) y nada borrado.
5. **Parar el daemon**: va **después** de la confirmación, para que un `daemon_stop_failed` no deje nada a medias.
6. **Borrar el estado.**
7. **Revertir el `PATH`** exactamente según el recibo: el enlace solo si apunta al directorio de programa, los bloques delimitados de los perfiles, y en Windows la entrada del registro con comparación canónica conservando el tipo del valor y difundiendo `WM_SETTINGCHANGE`. Un resto que no se pueda revertir se informa en `failed`.
8. **Borrar el directorio de programa** aplicando la regla del directorio de programa.
9. **Borrar el archivo de bloqueo y la compuerta**, que viven en el hermano del directorio de programa: se borran también cuando el directorio quedó programado para después, y es el último paso con efectos. En Unix se borran con el bloqueo aún tomado; en Windows se sueltan primero los descriptores propios, y si otro proceso los tiene abiertos en ese instante el fichero se queda (sin consecuencias) y se informa en `failed`.

### El paso 6: la raíz de datos entera, no el plan de `cleanup --all`

**Sin `--keep-data`, el destino del estado es la raíz de datos completa.** No es el plan de `cleanup --all`, y la diferencia es deliberada:

- `cleanup --all` **protege las voces de fábrica** (`default`, `ryan`, `vivian`) porque van embebidas en el binario y el programa sigue instalado: borrarlas sería tirar un trabajo que `setup` vuelve a materializar.
- Al desinstalar **el programa desaparece**, y con él las voces de fábrica. Dejarlas sería **residuo dentro de una raíz de propiedad exclusiva**.

Por eso la composición del plan vive en `uninstall::compose_plan` y tiene dos ramas, no dos implementaciones del mismo alcance:

| Invocación | Destinos del estado |
|---|---|
| Sin `--keep-data` | Los del plan de modelos (raíz de modelos según R1–R3) **más la raíz de datos entera** |
| Con `--keep-data` | El plan de `cleanup --all` **filtrado**: fuera modelos, voces y habla; dentro configuración, logs y estado del daemon |

Con `--keep-data` el directorio de programa **se borra igual**: la bandera conserva el estado, no el programa.

**La regla del directorio de programa gobierna el paso 8 y por eso tiene su propia función pública** (`program_dir_is_removable`). Exige las dos mitades de la regla: la positiva —el directorio contiene el recibo o el ejecutable, que es lo que lo convierte *en* el directorio de programa— y la negativa —nunca es la raíz de una unidad, `$HOME`, un ancestro de `$HOME` ni coincide con otra raíz del producto—. Ni una variable de reubicación ni un recibo manipulado pueden ampliar el alcance.

**La consulta ocurre en el planificador, no en el paso 8.** `compose_plan` es la fuente única que usan el simulacro y la ejecución, así que es él quien decide si el directorio de programa entra como destino. Cuando esa regla lo prohíbe no entra, y ahí hay dos casos que el plan distingue porque dicen cosas distintas:

- **Ningún destino del plan lo contiene** → se conserva de verdad, y pasa a la lista `preserved` con el motivo de la regla del directorio de programa. Es el caso de un directorio de programa que es `$HOME` o un ancestro suyo.
- **Un destino del plan lo contiene** —típico con `AVI_INSTALL_DIR` dentro de la raíz de datos— → **no se conserva**: el paso 6 borra la raíz entera y se lo lleva. El plan no lo anuncia ni como destino propio ni como conservado, porque la raíz que lo borra ya está en la lista.

R2 dice «no lo borro yo», no «no se borra»: el plan distingue los dos casos porque dicen cosas distintas.

En Windows, si el ejecutable en uso está dentro del directorio de programa, el borrado se **programa** para cuando termine el proceso (`removal_scheduled`, que es éxito) en vez de hacerse de forma síncrona. Quien espera y borra es el Windows PowerShell 5.1 del sistema, lanzado desacoplado y oculto, sin copiar ningún ejecutable: el destino y la identidad esperada (`<pid>` e inicio del proceso) viajan por variables de entorno, el script espera por identidad (un PID reutilizado por otro proceso cuenta como muerto) y borra con 10 reintentos. El destino es el contenedor más externo del árbol de propiedad que, retirados el programa y sus archivos de bloqueo, queda vacío (o el propio directorio de programa si no hay ninguno). Un borrado correcto no deja ningún registro; si fallan todos los reintentos escribe `<destino>.removal-failed.log` junto al destino, que la recuperación retira en cuanto el destino ya no existe, también cuando el destino era un contenedor del árbol de propiedad, y que `doctor` informa mientras exista. Si el borrado no se puede programar (o, fuera de Windows, el directorio no se puede borrar), el resto de la desinstalación se completa y el comando termina con `status` `uninstalled`, `reason` `program_dir_kept` y salida 22, sin borrado parcial del directorio.

### Idempotencia y residuo

Sin instalación ni estado, `self uninstall` termina con éxito y `status` `not_installed`. Con operación completa, **cero residuo dentro de las raíces de propiedad exclusiva**, y lo compartido que no se borra se informa explícitamente con su motivo: es la lista `preserved`, que forma parte del plan y se imprime una sola vez con él. `preserved` solo lista rutas que existen: un directorio de programa inexistente no se anuncia como conservado. Como parte del residuo cero, el ejecutor del árbol de propiedad borra las hojas del estado y poda de abajo arriba los contenedores que quedan vacíos (el contenedor de caché y, en Windows, el contenedor raíz de la aplicación), con `remove_dir` no recursivo; un contenedor con entradas ajenas se conserva y se informa, y la raíz HF compartida (`External`) nunca se poda. Los contenedores que solo retenían el programa y sus archivos de bloqueo se podan después de retirarlos (en el borrado diferido, el destino programado ya es el contenedor más externo).

---

## Bloqueo, recuperación y el reparto con el binario

- **El bloqueo es de SO** (`flock` en Unix, `LockFileEx` en Windows) sobre el archivo de bloqueo del [modelo de rutas](../../specs/LIFECYCLE.md#6-modelo-de-rutas-y-propiedad) y su compuerta de traspaso, y el SO lo libera aunque el proceso muera. Las operaciones lo toman en modo exclusivo y los comandos de uso en modo compartido; con una operación en curso, otra operación o un comando de uso salen con `lifecycle_locked` (**17**). La propiedad de la que depende: dos instalaciones concurrentes no se pisan. `self update` suelta solo el bloqueo antes del traspaso y retiene la compuerta hasta que vuelve; el `self install` hijo, reconocido por `AVI_HANDOVER_PARENT_PID`, toma solo el bloqueo.
- **La recuperación corre antes de componer el plan**, para que el plan que el usuario ve y confirma sea el que queda después del barrido por identidad de los residuos de dueño muerto (aparcados, stagings de instalación, scratch, temporales con token y registros de borrado resueltos). Los residuos de dueño no observable se conservan y se informan, nunca se dan por huérfanos.
- **El motor no puede matar un árbol de procesos ni agendar un borrado en Windows**, así que ambos entran por rasgos (`ProcessControl`, `ProgramDirRemover`). La frontera es deliberada: arrastrar `avi-daemon` y `avi-tts` al crate del motor crearía un ciclo de dependencias, y la [organización del repositorio](../../specs/LIFECYCLE.md#53-organización-del-repositorio) lo prohíbe.

---

## Contrato `--json`

| Subcomando | Claves |
|---|---|
| `self install` | `status` · `reason` · `install_dir` · `version` · `channel` · `path_integrated` · `models` · `models_cause` (solo si `models` es `failed`) |
| `self update` | `status` (`updated` o `already_up_to_date` o `check` o `cancelled`) · `reason` (`setup_failed` en el parcial) · `previous_version` · `version`/`latest` · `channel` · `current`/`update_available` (en `--check`, `already_up_to_date` y `cancelled`; `cancelled` lleva `status`, `reason` nulo, `current`, `latest` y `channel`) · `models_cause` (solo en el parcial) |
| `self uninstall` | `status` · `reason` (`null`) · `removed` · `planned` · `path_reverted` |

`status` toma los valores `installed` / `repaired` en `self install`, y `uninstalled` / `removal_scheduled` / `not_installed` / `cancelled` / `planned` en `self uninstall`. `schema_version` lo inyecta `emit_raw_json` y vale **`"5"`**; el protocolo del daemon es otro contrato con su propia versión ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)).

`planned` es el recibo de `compose_plan`, el mismo que usa la ejecución real, así que `--dry-run` anuncia exactamente lo que ocurriría. Con `--dry-run` el `status` es `planned`, `removed` copia `planned` y `path_reverted` es el valor previsto; el envelope no lleva clave `dry_run`: `planned` ya dice que nada se modificó.

**`path_reverted` previsto y real salen del mismo predicado.** No basta con leer la marca `modify_path` del recibo: puede decir que se integró el `PATH` y que hoy no queda nada que revertir —el enlace se borró a mano, el bloque ya no está en el perfil—. El valor publicado lo decide `path_would_be_reverted`, que además de la marca comprueba la existencia real de cada artefacto (el enlace solo si apunta al directorio de programa, el bloque delimitado del perfil, la entrada de registro en Windows), y lo usan tanto el simulacro como la ejecución. Por eso `--dry-run` no puede afirmar una reversión que la operación real no haría.

`--json` no cambia ninguna fila de éxito: el comando hace lo mismo y además emite su payload. La excepción es `setup_failed`, que sí cambia la salida, y por eso está declarado como salida por veredicto y no como error.

---

## Errores

| `reason` | Código | Cuándo |
|---|---|---|
| `bundle_invalid` | 15 | Falta un archivo obligatorio del bundle; nada modificado |
| `lifecycle_locked` | 17 | Otra operación de ciclo de vida tiene el bloqueo |
| `sudo_not_supported` | 1 | Unix: `self install` o `self uninstall` lanzado con `sudo`; nada modificado |
| `path_conflict` | 14 | Hay un archivo ajeno en la ruta del enlace; nada aplicado, salvo `--force` |
| `daemon_stop_failed` | 16 | No se pudo detener el daemon; nada del plan se aplicó |
| `rolled_back` | 13 | Fallo en el reemplazo; la versión anterior quedó restaurada |
| `externally_managed` | 12 | `homebrew`: la copia la gestiona Homebrew (`self uninstall` y `self update`) |
| `confirmation_required` | 2 | Destructiva sin terminal y sin `--yes` |
| `setup_failed` | 11 | `self install` terminó con el programa instalado y la provisión sin completar; `self update` propaga el mismo parcial del binario nuevo |
| `already_up_to_date` | 0 | `self update` sin descarga: ya se está en la versión objetivo (éxito) |
| `not_installed` | 3 | `self update` sin instalación registrada, con el one-liner |
| `unsupported_platform` | 18 | Target no soportado, antes de tocar la red |
| `binary_incompatible` | 19 | El binario descargado no arranca o no informa la versión objetivo, con diagnóstico |
| `network_error` | 20 | Fallo de descarga tras reintentos acotados |
| `checksum_mismatch` | 21 | El hash no coincide o falta en `SHA256SUMS.txt`; staging borrado y nada más modificado |
| `program_dir_kept` | 22 | `self uninstall`: el resto se completó, pero el directorio de programa no se pudo borrar ni programar su borrado |

Los `reason` del empaquetado salen con el **1** genérico: ningún `reason` nuevo de esa superficie tiene entero propio.
