# Grupo `self` — ciclo de vida de la instalación del usuario

`self` es el grupo con el que **el binario gestiona su propia instalación** y cubre los dos lados del ciclo de vida: `self install` instala o repara, `self update` actualiza a la última estable o a una concreta, y `self uninstall` desinstala.

La Normativa del grupo está en [`LIFECYCLE.md`](../../specs/LIFECYCLE.md) (la [superficie de comandos](../../specs/LIFECYCLE.md#54-superficie-de-comandos), las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales), [`self install`](../../specs/LIFECYCLE.md#83-self-install) y [`self uninstall`](../../specs/LIFECYCLE.md#85-self-uninstall)); el contrato de la CLI —flags, `reason`, códigos de salida, envelope `--json`— está en [`../CONTRACT.md`](../CONTRACT.md). Este documento describe **dónde vive cada cosa y por qué**.

**Implementación:** el motor es el crate `avi-lifecycle` (`crates/avi-lifecycle/src/install.rs` y `uninstall.rs`), sin punto de entrada propio: el parseo de la CLI y el cableado se quedan en `src/main.rs` (`handle_self`), porque el motor no depende de `clap` ni de `avi-core` (ver la [organización del repositorio](../../specs/LIFECYCLE.md#53-organización-del-repositorio)). El binario aporta las dos primitivas que el motor no puede tener —el control de procesos (`ProductProcesses`, que vive en `avi-daemon`/`avi-tts`) y el mecanismo de borrado de plataforma (`PlatformRemoval`, que en Windows usa `avi_process::schedule_removal` para el borrado diferido)—. El motor decide con dos políticas con nombre, `retire` para el staging y el aparcado y `remove_program_dir` para el programa en uso; el binario ejecuta esas dos primitivas.

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
4. **Cabecera, resumen previo y confirmación.** La cabecera es `Instalación de ai-voice-interconnector <versión>` (o `Reparación de …`), con ` · sin modificar el PATH` si se pasa `--no-modify-path`, y sale antes que el resumen y la pregunta; el tiempo de la pregunta no cuenta en el reloj. Instalar una versión **menor** que la instalada es una degradación y se confirma como operación **destructiva** (`confirm::Kind::Destructive`), aunque instalar no borre nada por sí mismo. Cancelar aquí es salida 0 y ningún `reason`: las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) no cuentan la cancelación entre los `reason`. El resumen anuncia solo los cambios pendientes: el plan del `PATH` se calcula con el estado real (el valor `Path` del registro de usuario en Windows, el enlace del comando en Unix), y si no hay nada que cambiar la línea dice `<dir> ya está en el PATH; no se modifica` (`… en el PATH del usuario` en Windows). Un registro ilegible al planificar sale con `path_conflict` (14), igual que al aplicar.
5. **Parar el daemon** (etapa `Parada del daemon`, solo si había uno en marcha), incluido el proceso residente del motor. Si no se detiene → `daemon_stop_failed` (16), **sin modificar nada**.
6. **Copia o reemplazo transaccional** (etapa `Copia` en una instalación nueva y `Reemplazo` si ya había una versión; en la reparación no hay ninguna de las dos). Es `transaction::replace`: aparcar, colocar, ajustar permisos, confirmar y borrar lo aparcado; revertir restaurando lo aparcado si falla → `rolled_back` (13). **En reparación no se copia nada.** El reemplazo parte siempre de un staging propio, hermano del directorio de programa y con el token del proceso en el nombre: si el bundle ya está en un staging con el token de este mismo proceso, se usa tal cual; en cualquier otro caso (la descarga del bootstrap, el staging de `self update`, que lleva el token del proceso que actualiza, o una extracción manual) `self install` copia, con el bloqueo ya tomado, solo los archivos que lista el manifiesto a `.ai-voice-interconnector-staging-<token>-self-install` y lo borra al terminar, con aviso si no puede. Lo que haya junto al bundle (el archivo descargado, una carpeta con otros archivos) no se copia. `self update` entrega un staging propio, que se coloca tal cual, y por eso retira antes el archivo descargado; los bootstraps extraen fuera del programa y dependen de que la copia filtre.
7. **macOS**: eliminar `com.apple.quarantine` de forma recursiva en **todo** el directorio de programa, no solo en el ejecutable, porque el motor y la librería de ONNX Runtime también se ejecutan o cargan. No es un fallo de la instalación: la cuarentena que no se quita degrada el arranque y el resumen lo informa.
8. **Integración de `PATH`** (etapa `Integración en el PATH`, que no existe con `--no-modify-path`; su nota dice `se añadió <bin>` y bajo ella un `⚠` avisa de que las terminales que ya estaban abiertas no verán el cambio hasta reiniciarse) ([contrato de `self`](../CONTRACT.md#self-install-self-update-y-self-uninstall) e [integración de PATH](../../specs/LIFECYCLE.md#831-integración-de-path)). En Unix el `PATH` se modifica **por defecto** y se anuncia en el resumen; `--no-modify-path` lo desactiva. Un archivo ajeno en la ruta del enlace → `path_conflict` (14), salvo `--force`.
9. **Escribir el recibo** de forma atómica (etapa `Recibo`).
10. **La misma provisión que `setup`**, salvo `--no-setup`: descarga de la selección (todos los modelos, el Base de clonado incluido) según el estado real del almacén, sin repetir la confirmación, con una etapa `Descarga` por modelo pendiente (ver [el progreso de `setup`](SETUP.md#modelos-provisionados-model_revisions)). El bloqueo se retiene hasta aquí y se libera al terminar la provisión.
11. **Cierre**, por stderr: tras el separador, `✓ ai-voice-interconnector <versión> instalado en <total>` (o `reparado en <total>`), y **nada más**: ni resumen ni consejo bajo el cierre. Las notas de cada etapa llevan lo que antes se resumía al final: la ruta del programa en `Copia`, `· reemplaza <versión>` en `Reemplazo` y `se añadió <bin>` en la del `PATH`. El plan previo a la confirmación pregunta `¿Instalar el programa?` (o `¿Reparar la instalación?`), y con el `PATH` ya integrado dice `PATH: ya está en el PATH de tu usuario; no se modifica.`

La instalación con la integración en el `PATH` y la que reemplaza una versión anterior no tienen muestra en este documento, porque dependen de modificar el `PATH` del usuario y de tener una versión previa instalada; se describen aquí en prosa.

**Lo que el recibo registra es el estado, no el diff de esta pasada.** El recibo dice qué integración de `PATH` está en pie, no qué cambió en esta ejecución: si registrara solo el diff, una segunda instalación desde el mismo bundle escribiría un recibo sin entrada de `PATH` y `self uninstall` no podría revertir la que puso la primera, dejando residuo dentro de una raíz de propiedad exclusiva. El diff vive aparte, en `Outcome::path_rewritten`, y es lo único que decide si la etapa del `PATH` avisa de que las terminales ya abiertas no ven el `PATH` nuevo.

**La salida cuenta el `PATH` una sola vez por momento.** El plan previo dice qué se hará (`se añadirá …`) y la nota de la etapa dice qué se hizo (`se añadió …`), cada uno en su sitio y sin repetirse bajo el cierre.

---

## `self update`: los pasos

El orden es el de [`self update`](../../specs/LIFECYCLE.md#84-self-update) y está escrito en el propio código (`handle_self`, brazo `Update`):

1. **Recuperación y bloqueo.** La recuperación corre con el bloqueo tomado sobre la instalación registrada; barre por identidad exacta los residuos de dueño muerto (scratch, temporales con token, stagings de instalación, aparcados y registros de borrado resueltos) y conserva e informa los de dueño no observable.
2. **Leer el recibo y el [canal](../../specs/LIFECYCLE.md#72-canales)**. `homebrew` → `externally_managed` (12) con el comando correcto; sin instalación → `not_installed` con el one-liner y salida 3.
3. **Resolver la versión objetivo**: `--version` explícito sin tocar la red, o la última estable siguiendo la redirección de `releases/latest` (con `AVI_DOWNLOAD_BASE_URL` si está definida); la API REST solo es respaldo.
4. **Comparar las versiones** con el comparador numérico vigente: iguales → `already_up_to_date` sin descargar, éxito con 0 (`--force` reinstala); objetivo menor → solo con `--version` explícito y marca destructiva.
5. **`--check`**: es una consulta, no una operación: no escribe cabecera, etapas ni cierre, y por stdout dice `Versión instalada: X` y `Versión disponible: Y`. En `--json` emite `current`, `latest`, `update_available`, `channel` y `reason` (nulo), sin `status` ni claves comunes.
6. **Cabecera, resumen y confirmación.** La cabecera `Actualización desde la versión <X>` sale antes de la pregunta, y el tiempo de la pregunta no cuenta en el reloj. Normal `[S/n]`; degradación `[s/N]` con `confirmation_required` sin terminal. Con la versión ya al día, tras la cabecera solo sale `✓ Ya tienes la última versión (X)`, sin etapas.
7. **Preparar el bundle nuevo** (etapas `Descarga` y `Verificación`; la búsqueda de la versión es la etapa transitoria `Buscando actualizaciones…`, que no deja línea): descarga del archivo y de `SHA256SUMS.txt` en un staging hermano con HTTPS y reintentos acotados, verificación de coincidencia exacta en `SHA256SUMS.txt` (discrepancia o línea ausente → `checksum_mismatch`, 21, con staging borrado), extracción y comprobación de arranque con coincidencia de `--version` (si no → `binary_incompatible`, 19) y validación contra el manifiesto.
8. **Parar el daemon con el binario actual** (etapa `Parada del daemon`, solo si había uno), que conoce su protocolo y su `daemon.pid`, anotando si estaba en ejecución. Si no se detiene → `daemon_stop_failed` (16) sin tocar nada y con el staging retirado.
9. **Traspaso** (etapa `Reemplazo`): ejecuta `<staging>/ai-voice-interconnector --json self install --yes`. El binario nuevo pinta sus propias etapas por stderr, sin numerar y sangradas dentro de `Reemplazo`, y no escribe cabecera ni cierre; el padre lo vigila con un fichero de latido (`AVI_HANDOVER_HEARTBEAT_FILE`) y solo lo corta si deja de emitir latidos durante 600 s. Captura stdout estructurado; propaga las preferencias del recibo (`--no-modify-path` si la instalación no tocaba el `PATH`), `--no-setup` si se pidió y **`--force` cuando el `update` lo recibió**. Valida la causa anidada en el parcial; stdout inválido o sin `models_cause` falla explícitamente.
10. **Limpiar**: borra siempre el staging. El aparcado `.old-*` que el reemplazo no pudo borrar por tener algo en uso (en Windows, lo retiene el ejecutable del `self update` que lanzó el traspaso) lo retira el propio `self install` **como unidad**: un único borrado diferido de la carpeta entera, que espera a que termine ese proceso, o, si no puede programarse, la recuperación de la siguiente operación. Así no queda ninguna carpeta `.old-*` vacía ni se lanza un limpiador por archivo. En Unix no hay diferido.
11. **Resultado**: tras el separador, `✓ Actualizado de <anterior> a <nueva> en <total>`, por stderr. Sin reinicio automático del daemon: si estaba activo, un `⚠` indica cómo relanzarlo con el comando habitual.

La actualización con reemplazo real no tiene muestra en este documento, porque exige publicar una versión posterior a la instalada; se describe aquí en prosa.

**Garantías.** Un fallo antes del traspaso deja todo intacto (staging borrado, instalación intacta). Un fallo durante el traspaso revierte la transacción nueva del `self install` invocado. Una interrupción en cualquier punto queda recuperable en la siguiente operación de ciclo de vida.

---

## El progreso por stderr

Los tres comandos de este grupo escriben por stderr una cabecera, sus etapas numeradas, un separador y un cierre: `self install` con `Parada del daemon`, `Copia` o `Reemplazo`, `Integración en el PATH`, `Recibo` y una `Descarga` por modelo; `self update` con `Descarga`, `Verificación`, `Parada del daemon` y `Reemplazo`; y `self uninstall` con `Parada del daemon`, `Estado`, `Reversión del PATH`, `Programa` y `Bloqueos`. La cabecera (`Instalación de ai-voice-interconnector <versión>`, `Actualización desde la versión <X>` o `Desinstalación de ai-voice-interconnector`) sale **antes de la pregunta**, y el tiempo que el usuario tarda en contestarla no cuenta en el reloj. El cierre —`✓ <acción> en <total>`— lo dice **una sola vez** el flujo de la biblioteca (o el handler en `self update`).

`--json` solo cambia stdout: stderr es el mismo con y sin él, y stdout lleva solo el envelope. `AVI_NO_PROGRESS=1` silencia solo las etapas por stderr: la cabecera, el cierre, el envelope `--json` y los códigos de salida no cambian.

## `setup_failed`: el único desenlace que no es ni éxito ni error

Si la provisión de `self install` (la misma de `setup`) falla, **la instalación no falla**. Las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) lo declaran éxito parcial, y el código lo modela así:

| | Valor |
|---|---|
| `status` | `installed` (o `repaired`) |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Forma de salida | **Por veredicto**: el envelope se emite igual y **no** se adjunta el objeto `error`; por stderr, en los dos modos, la etapa `Descarga` queda en `✗` con un `⚠` por causa, y tras el separador salen `Error:` y `Solución:` |
| Qué hacer | El programa está instalado. Con causa `network_error`, reintentar con `setup`; con `model_unavailable`, reintentar no lo resuelve y hace falta una versión que vuelva a fijar el modelo |

El motivo del fallo de provisión **no se pierde**: viaja anidado en `models_cause`, con su propio `reason` —`network_error` para un fallo de transporte o del servidor, `model_unavailable` cuando Hugging Face rechaza el modelo fijado (401, 403 o 404)— y su mensaje. La `Solución:` del texto se elige según esa causa; no hay resumen ni consejo aparte bajo el cierre. Ese `reason` y `setup_failed` dicen cosas distintas y por eso no se funden: uno dice **qué** falló y el otro **qué dejó de completarse**.

El envelope completo de este desenlace, con la salida de texto, está en las muestras al final.

`models_cause` **solo existe si hubo fallo**: un envelope estable es más fácil de leer que uno con nulos. Los cuatro valores de `models` son `skipped` (`--no-setup`), `already_provisioned`, `provisioned` y `failed`. Son valores del envelope; el texto nunca los imprime.

Cuando `setup` se invoca **directamente** (no desde `self install` ni desde el traspaso de `self update`), un fallo de transporte sale con `network_error` y **20**, y un rechazo del modelo fijado con `model_unavailable` y **1**. El `self update` propaga el parcial del binario nuevo: el resultado es `updated` con `reason` `setup_failed`, salida 11 y la causa real de provisión en `models_cause.reason`/`models_cause.message`; `reason` describe el resultado parcial, no sustituye la causa.

---

## `self uninstall`: qué se borra y por qué

Los pasos de [`self uninstall`](../../specs/LIFECYCLE.md#85-self-uninstall), en orden, sobre la **instalación registrada** y no sobre la posición del ejecutable.

1. **El canal decide si se puede actuar**: `homebrew` → `externally_managed` (12) con el comando de Homebrew en el mensaje. Sin recibo de instalación, el comando no adopta las raíces convencionales aunque contengan datos y devuelve **`not_installed` y salida 0** sin borrarlas.
2. **Plan**, el mismo en la simulación y en la ejecución (`uninstall::plan_lines`): cada ruta una sola vez, con su categoría en español cuando la tiene (`datos de usuario` para la raíz de datos entera) y su tamaño; las reversiones del `PATH` que el recibo registra, una línea por cada una con lo que se retira (`PATH: se quitará la entrada <ruta> del PATH de tu usuario.`), que son acciones del plan y no recursos conservados; y la lista de lo que **no** se tocará y por qué. Con recibo, la última línea del plan avisa de los datos que el sistema operativo guarda por su cuenta y enlaza [`Datos del sistema operativo`](#datos-del-sistema-operativo) en la versión instalada (`no se tocará: los datos que el sistema operativo guarda por su cuenta (permisos, reglas del firewall, preferencias de audio); cómo borrarlos: https://github.com/<repositorio>/blob/v<versión>/docs/CLI/commands/SELF.md#datos-del-sistema-operativo`); no es una ruta, así que no cuenta en la cabecera `N rutas a tocar y M rutas conservadas` ni aparece en el JSON.
3. **`--dry-run`**: escribe la cabecera (`Desinstalación de ai-voice-interconnector · simulación`) por stderr y el plan por stdout —que abre con `Se desinstalará ai-voice-interconnector…`—, después `✓ Simulación completada · no se ha borrado nada`, sin etapas, y termina **sin tomar el bloqueo**: tomar el bloqueo crea el archivo, y una simulación que deja un archivo detrás no es una simulación. Con `--json`, el envelope `planned` lleva el plan en `planned`, `removed: []` y `context`.
4. **Confirmación destructiva** `¿Desinstalar el programa y borrar lo indicado? [s/N] ` (con `--keep-data`, `¿Desinstalar el programa y borrar lo indicado, conservando tus datos? [s/N] `). La cabecera es `Desinstalación de ai-voice-interconnector`, con ` · conservando los datos` si se pasa `--keep-data` y ` · simulación` en `--dry-run`, y sale antes que el plan y la pregunta. Sin terminal y sin `--yes` → `confirmation_required` (2) y nada borrado. Al terminar, el cierre por stderr dice `✓ Desinstalación completada en <total>`; el `status` es del envelope `--json`. Si quedan rutas sin borrar, el desenlace es de éxito parcial y sale igual en los dos modos, con `failed` en el envelope: `program_dir_kept` (22) si entre ellas está el directorio de programa, y `uninstall_incomplete` (24) en cualquier otro caso; en texto, la etapa afectada queda en `✗` con un `⚠` por ruta, y tras el separador salen el `Error:` y su `Solución:`. Sin recibo de instalación, el desenlace es `not_installed` y el texto dice `No había nada que desinstalar.`, sin duración: el comando no adopta ni inspecciona raíces convencionales del perfil.
5. **Parar el daemon** (etapa `Parada del daemon`, solo si había uno en marcha): va **después** de la confirmación, para que un `daemon_stop_failed` no deje nada a medias. Las etapas siguientes son `Estado`, `Reversión del PATH`, `Programa` y `Bloqueos`, una por paso.
6. **Borrar el estado.**
7. **Revertir el `PATH`** exactamente según el recibo: el enlace solo si apunta al directorio de programa, los bloques delimitados de los perfiles, y en Windows la entrada del registro con comparación canónica conservando el tipo del valor y difundiendo `WM_SETTINGCHANGE`. Un resto que no se pueda revertir se informa en `failed` y el desenlace es `uninstall_incomplete`.
8. **Borrar el directorio de programa** aplicando la regla del directorio de programa.
9. **Borrar el archivo de bloqueo y la compuerta**, que viven en el hermano del directorio de programa: se borran también cuando el directorio quedó programado para después, y es el último paso con efectos. En Unix se borran con el bloqueo aún tomado; en Windows se sueltan primero los descriptores propios, y si otro proceso los tiene abiertos en ese instante el fichero se queda (sin consecuencias) y se informa en `failed`.

### El paso 6: la raíz de datos entera, no el plan de `cleanup --all`

**Sin `--keep-data`, el destino del estado es la raíz de datos completa.** No es el plan de `cleanup --all`, y la diferencia es deliberada:

- `cleanup --all` con recibo de instalación **protege la voz de fábrica** (`default`) porque va embebida en el binario y el programa sigue instalado: borrarla sería tirar un trabajo que `setup` vuelve a materializar. Sin recibo (descarga manual, binario de `target/` o Cask de Homebrew), `cleanup --all` deja el estado en cero como `self uninstall`, porque ningún `self uninstall` lo retirará.
- Al desinstalar **el programa desaparece**, y con él las voces de fábrica. Dejarlas sería **residuo dentro de una raíz de propiedad exclusiva**.

Por eso la composición del plan vive en `uninstall::compose_plan` y tiene dos ramas, no dos implementaciones del mismo alcance:

| Invocación | Destinos del estado |
|---|---|
| Sin `--keep-data` | Los del plan de modelos (raíz de modelos según R1–R3) **más la raíz de datos entera** |
| Con `--keep-data` | El plan de `cleanup --all` **filtrado**: fuera modelos, voces y audios sintetizados; dentro configuración, logs y estado del daemon |

Con `--keep-data` el directorio de programa **se borra igual**: la bandera conserva el estado, no el programa.

**La regla del directorio de programa gobierna el paso 8 y por eso tiene su propia función pública** (`program_dir_is_removable`). Exige las dos mitades de la regla: la positiva —el directorio contiene el recibo o el ejecutable, que es lo que lo convierte *en* el directorio de programa— y la negativa —nunca es la raíz de una unidad, `$HOME`, un ancestro de `$HOME` ni coincide con otra raíz del producto—. Ni una variable de reubicación ni un recibo manipulado pueden ampliar el alcance.

**La consulta ocurre en el planificador, no en el paso 8.** `compose_plan` es la fuente única que usan el simulacro y la ejecución, así que es él quien decide si el directorio de programa entra como destino. Cuando esa regla lo prohíbe no entra, y ahí hay dos casos que el plan distingue porque dicen cosas distintas:

- **Ningún destino del plan lo contiene** → se conserva de verdad, y pasa a la lista `preserved` con el motivo de la regla del directorio de programa. Es el caso de un directorio de programa que es `$HOME` o un ancestro suyo.
- **Un destino del plan lo contiene** —típico con `AVI_INSTALL_DIR` dentro de la raíz de datos— → **no se conserva**: el paso 6 borra la raíz entera y se lo lleva. El plan no lo anuncia ni como destino propio ni como conservado, porque la raíz que lo borra ya está en la lista.

R2 dice «no lo borro yo», no «no se borra»: el plan distingue los dos casos porque dicen cosas distintas.

En Windows, si el ejecutable en uso está dentro del directorio de programa, el borrado se **programa** para cuando termine el proceso (`removal_scheduled`, que es éxito) en vez de hacerse de forma síncrona. Quien espera y borra es el Windows PowerShell 5.1 del sistema, lanzado oculto y, cuando el proceso anfitrión lo permite, desacoplado de él, sin copiar ningún ejecutable: el destino y la identidad esperada (`<pid>` e inicio del proceso) viajan por variables de entorno, el script espera por identidad (un PID reutilizado por otro proceso cuenta como muerto) y borra con 10 reintentos. El destino es el contenedor más externo del árbol de propiedad que, retirados el programa y sus archivos de bloqueo, queda vacío (o el propio directorio de programa si no hay ninguno). Un borrado correcto no deja ningún registro; si fallan todos los reintentos escribe `<destino>.removal-failed.log` junto al destino, que la recuperación retira en cuanto el destino ya no existe, también cuando el destino era un contenedor del árbol de propiedad, y que `doctor` informa mientras exista. Si el borrado no se puede programar (o, fuera de Windows, el directorio no se puede borrar), el resto de la desinstalación se completa y el comando termina con `status` `uninstalled`, `reason` `program_dir_kept` y salida 22, sin borrado parcial del directorio. Si el anfitrión no permite desacoplar el limpiador, `self uninstall` lo dice y nombra la ruta que habría que borrar a mano si el proceso que ejecuta el comando se cierra antes de terminar.

### Idempotencia y residuo

Sin instalación ni estado, `self uninstall` termina con éxito y `status` `not_installed`. Con operación completa, **cero residuo dentro de las raíces de propiedad exclusiva**, y lo compartido que no se borra se informa explícitamente con su motivo: es la lista `preserved`, que forma parte del plan y se imprime una sola vez con él. `preserved` solo lista rutas que existen: un directorio de programa inexistente no se anuncia como conservado. Como parte del residuo cero, el ejecutor del árbol de propiedad borra las hojas del estado y poda de abajo arriba los contenedores que quedan vacíos (el contenedor de caché y, en Windows, el contenedor raíz de la aplicación), con `remove_dir` no recursivo; un contenedor con entradas ajenas se conserva y se informa, y la raíz HF compartida (`External`) nunca se poda. Los contenedores que solo retenían el programa y sus archivos de bloqueo se podan después de retirarlos (en el borrado diferido, el destino programado ya es el contenedor más externo).

### Datos del sistema operativo

Son los datos que el sistema operativo guarda por su cuenta cuando se usa AVI: permisos, reglas del firewall y preferencias de audio por aplicación. AVI no los borra en ningún sistema operativo, porque pertenecen al sistema y al usuario, se guardan en formatos internos del sistema y algunos solo se borran con privilegios de administrador, que AVI nunca pide. Si quieres retirarlos tras desinstalar, se hace a mano, según tu sistema.

**Windows**

- **Permiso de micrófono.** AVI aparece en Configuración → Privacidad y seguridad → Micrófono. La clave que lo guarda se borra con este comando, que lleva la ruta del ejecutable con `#` en lugar de `\` (ajusta el usuario y la ruta si instalaste en otra parte):

  ```text
  reg delete "HKCU\Software\Microsoft\Windows\CurrentVersion\CapabilityAccessManager\ConsentStore\microphone\NonPackaged\C:#Users#<usuario>#AppData#Local#Programs#ai-voice-interconnector#ai-voice-interconnector.exe" /f
  ```

- **Reglas del firewall.** Solo las crearon versiones anteriores a la 0.26.0, cuyo motor escuchaba fuera de `127.0.0.1`; desde entonces el motor residente escucha solo en `127.0.0.1` y no se crean reglas nuevas. En un **Símbolo del sistema** abierto como administrador:

  ```text
  netsh advfirewall firewall delete rule name=all program="%LOCALAPPDATA%\Programs\ai-voice-interconnector\vendor\qwen3-tts\qwen_tts.exe"
  ```

- **Volumen y dispositivo por aplicación.** En Windows 11: Configuración → Sistema → Sonido → Mezclador de volumen → Restablecer. En Windows 10: Configuración → Sistema → Sonido → Opciones avanzadas de sonido → Preferencias de volumen y dispositivo de la aplicación → Restablecer. En los dos casos se devuelven **todas** las aplicaciones a los valores recomendados ([Microsoft Q&A](https://learn.microsoft.com/en-us/answers/questions/1337541/ap-volume-and-device-properties)).

**macOS**

- **Firewall de aplicaciones**, si está activado: Ajustes del Sistema → Red → Firewall → Opciones, selecciona la entrada y pulsa Eliminar ([guía de Apple](https://support.apple.com/guide/mac-help/change-firewall-settings-on-mac-mh11783/mac)).
- **Micrófono.** El permiso lo concede la aplicación desde la que se ejecuta AVI, normalmente la terminal, y se revoca en Ajustes del Sistema → Privacidad y seguridad → Micrófono.

**Linux**

No hay reglas de firewall por aplicación ni registro de permisos de micrófono para aplicaciones sin *sandbox*. Con PipeWire y WirePlumber, los volúmenes recordados por aplicación están en el archivo `stream-props` de `$XDG_STATE_HOME/wireplumber` (normalmente `~/.local/state/wireplumber`): con WirePlumber detenido se borra ese archivo, lo que olvida los volúmenes de **todas** las aplicaciones; `wpctl reset` restablece todo su estado ([documentación de WirePlumber](https://pipewire.pages.freedesktop.org/wireplumber/daemon/locations.html)).

---

## Bloqueo, recuperación y el reparto con el binario

- **El bloqueo es de SO** (`flock` en Unix, `LockFileEx` en Windows) sobre el archivo de bloqueo del [modelo de rutas](../../specs/LIFECYCLE.md#6-modelo-de-rutas-y-propiedad) y su compuerta de traspaso, y el SO lo libera aunque el proceso muera. Las operaciones lo toman en modo exclusivo y los comandos de uso en modo compartido; con una operación en curso, otra operación o un comando de uso salen con `lifecycle_locked` (**17**). La propiedad de la que depende: dos instalaciones concurrentes no se pisan. `self update` suelta solo el bloqueo antes del traspaso y retiene la compuerta hasta que vuelve; el `self install` hijo, reconocido por `AVI_HANDOVER_PARENT_PID`, toma solo el bloqueo.
- **La recuperación corre antes de componer el plan**, para que el plan que el usuario ve y confirma sea el que queda después del barrido por identidad de los residuos de dueño muerto (aparcados, stagings de instalación, scratch, temporales con token y registros de borrado resueltos). Los residuos de dueño no observable se conservan y se informan, nunca se dan por huérfanos.
- **El motor no puede matar un árbol de procesos ni agendar un borrado en Windows**, así que ambos entran por rasgos (`ProcessControl`, `ProgramDirRemover`). La frontera es deliberada: arrastrar `avi-daemon` y `avi-tts` al crate del motor crearía un ciclo de dependencias, y la [organización del repositorio](../../specs/LIFECYCLE.md#53-organización-del-repositorio) lo prohíbe.

---

## Contrato `--json`

| Subcomando | Claves |
|---|---|
| `self install` | `status` · `reason` · `install_dir` · `version` · `channel` · `path_integrated` · `models` · `models_cause` (solo si `models` es `failed`) · `context` (`mode`, `modify_path` y `version`) · `total_ms` · `stages` · `warnings` |
| `self update` | `status` (`updated`, `already_up_to_date` o `cancelled`) · `reason` (`setup_failed` en el parcial) · `previous_version` · `version`/`latest` · `channel` · `current` (en `already_up_to_date` y `cancelled`) · `models_cause` (solo en el parcial) · las claves comunes cuando hay etapas. `--check` es una consulta: lleva `current`, `latest`, `update_available`, `channel` y `reason`, sin `status` ni claves comunes |
| `self uninstall` | `status` · `reason` (`null` normalmente; `program_dir_kept` si el directorio de programa no se pudo borrar ni programar su borrado; `uninstall_incomplete` si quedaron otras rutas) · `removed` · `planned` · `preserved` · `failed` · `path_reverted` · `context` (`dry_run` y `keep_data`) · `total_ms`, `stages` y `warnings` cuando hay etapas |

`status` toma los valores `installed` / `repaired` en `self install`, `updated` / `already_up_to_date` / `cancelled` en `self update` (`--check` no lleva `status`), y `uninstalled` / `removal_scheduled` / `not_installed` / `cancelled` / `planned` en `self uninstall`. Las claves comunes (`context`, `total_ms`, `stages` y `warnings`) se definen en [`CONTRACT.md`](../CONTRACT.md): `already_up_to_date` no las lleva (no hay etapas) y la simulación de `self uninstall` lleva `context` y no `stages`. `already_up_to_date` no lleva `update_available`. En el parcial de `self update`, el padre valida el `models_cause` del hijo y conserva su causa concreta. `schema_version` lo inyecta `emit_raw_json` y vale **`"7"`**; el protocolo del daemon es otro contrato con su propia versión ([las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema)).

`planned` es el recibo de `compose_plan`, el mismo que usa la ejecución real, así que `--dry-run` anuncia exactamente lo que ocurriría. Con `--dry-run` el `status` es `planned`, el plan va solo en `planned`, `removed` va vacío y `path_reverted` es el valor previsto; el envelope no lleva clave `dry_run` de primer nivel: `planned` ya dice que nada se modificó.

`preserved` está **siempre presente** como array de objetos `{"path", "reason"}` con lo que la operación conserva a propósito: los recursos compartidos (caché HF ajena, voces/modelos/audios sintetizados con `--keep-data`) y el directorio de programa cuando la regla de borrado lo impide y ningún destino del plan lo recorre. Sin instalación registrada (`not_installed`) vale `[]`. Es una lista **por operación**, igual que en `cleanup`: lo que `cleanup` preserva `self uninstall` puede borrarlo, y preservar un contenedor no conserva a sus descendientes que el plan sí lista. Con `--keep-data` la lista anuncia lo que cleanup sin `--keep-data` borraría —modelos, voces y audios sintetizados— para que la decisión de conservar el estado sea visible. En texto, cada entrada se narra junto al plan (`no se tocará <ruta>: <motivo>`).

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
| `setup_failed` | 11 | `self install` terminó con el programa instalado y la provisión sin completar; `self update` propaga el mismo parcial del binario nuevo. La causa (`network_error` o `model_unavailable`) viaja en `models_cause` |
| `already_up_to_date` | 0 | — (es el `status` del envelope, no un `reason`): `self update` sin descarga, ya se está en la versión objetivo (éxito) |
| `not_installed` | 3 | `self update` sin instalación registrada, con el one-liner |
| `unsupported_platform` | 18 | Target no soportado, antes de tocar la red |
| `binary_incompatible` | 19 | El binario descargado no arranca o no informa la versión objetivo, con diagnóstico |
| `network_error` | 20 | Fallo de transporte o del servidor en la descarga tras reintentos acotados (sin red, 5xx, o el 403 del límite de peticiones de GitHub); reintentar puede resolverlo |
| `release_unavailable` | 1 | `self update`: GitHub responde 404 a la versión o al asset pedido (no está publicado para esta plataforma); reintentar no lo resuelve |
| `io_error` | 1 | `self update`: falla el disco local al crear el staging, escribir o leer el archivo descargado; no es un fallo de red |
| `checksum_mismatch` | 21 | El hash no coincide o falta en `SHA256SUMS.txt`; staging borrado y nada más modificado |
| `program_dir_kept` | 22 | `self uninstall`: el resto se completó, pero el directorio de programa no se pudo borrar ni programar su borrado |
| `uninstall_incomplete` | 24 | `self uninstall`: desinstaló lo que pudo y quedaron otras rutas sin borrar (listadas en `failed`); sale igual en texto y con `--json` |

Los `reason` del empaquetado salen con el **1** genérico: ningún `reason` nuevo de esa superficie tiene entero propio.

---

## Muestras de salida

Salidas reales del binario. Las duraciones y los tamaños son orientativos. La cabecera, las etapas, la pregunta y el cierre van por stderr; con `--dry-run`, el plan va por stdout; con `--json`, stdout lleva solo el envelope y stderr es el mismo que sin él. La actualización con reemplazo y la etapa `Integración en el PATH` no tienen muestra y se describen en prosa en sus secciones.

**`self install --yes --no-modify-path`** (instalación nueva, en la terminal):

```
Instalación de ai-voice-interconnector 0.47.0 · sin modificar el PATH
Se instalará ai-voice-interconnector 0.47.0 (x86_64-pc-windows-msvc)
  Programa:  %LOCALAPPDATA%\Programs\ai-voice-interconnector
  Comando:   %LOCALAPPDATA%\Programs\ai-voice-interconnector\ai-voice-interconnector.exe
  PATH:      no se modifica (--no-modify-path); invoca el comando por su ruta completa.
  Modelos:   ya están provisionados en %LOCALAPPDATA%\ai-voice-interconnector\cache\models
  [1/2] ✓ Copia   0,1 s · %LOCALAPPDATA%\Programs\ai-voice-interconnector
  [2/2] ✓ Recibo  0,0 s
──────────────────────────────────────────────────
✓ ai-voice-interconnector 0.47.0 instalado en 0,1 s
```

**Con `--json`** (stdout):

```json
{
  "channel": "script",
  "context": {
    "mode": "install",
    "modify_path": false,
    "version": "0.47.0"
  },
  "install_dir": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
  "models": "already_provisioned",
  "path_integrated": false,
  "reason": null,
  "schema_version": "7",
  "stages": [
    {
      "end_ms": 100,
      "phase": "install",
      "stage": "copying",
      "start_ms": 0
    },
    {
      "end_ms": 100,
      "phase": "install",
      "stage": "receipt",
      "start_ms": 100
    }
  ],
  "status": "installed",
  "total_ms": 100,
  "version": "0.47.0",
  "warnings": []
}
```

**Reparación** (`self install --yes --no-modify-path` desde el ejecutable instalado). No copia archivos y su única etapa no escribe línea propia:

```
Reparación de ai-voice-interconnector 0.47.0 · sin modificar el PATH
Se reparará ai-voice-interconnector 0.47.0 (x86_64-pc-windows-msvc)
  Programa:  %LOCALAPPDATA%\Programs\ai-voice-interconnector   (reparación: no se copian archivos)
  Comando:   %LOCALAPPDATA%\Programs\ai-voice-interconnector\ai-voice-interconnector.exe
  PATH:      no se modifica (--no-modify-path); invoca el comando por su ruta completa.
  Modelos:   ya están provisionados en %LOCALAPPDATA%\ai-voice-interconnector\cache\models
✓ ai-voice-interconnector 0.47.0 reparado en 0,0 s
```

**Instalación con la provisión fallida** (`--json self install --yes --no-modify-path`, sin red; sale con 11). El programa queda instalado y el envelope sale por veredicto, sin objeto `error`:

```json
{
  "channel": "script",
  "context": {
    "mode": "install",
    "modify_path": false,
    "version": "0.47.0"
  },
  "install_dir": "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector",
  "models": "failed",
  "models_cause": {
    "message": "dpdfnet8: HTTP request error: error sending request for url (https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964?recursive=true) (https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964?recursive=true)",
    "reason": "network_error"
  },
  "path_integrated": false,
  "reason": "setup_failed",
  "schema_version": "7",
  "stages": [
    {
      "end_ms": 100,
      "phase": "install",
      "stage": "copying",
      "start_ms": 0
    },
    {
      "end_ms": 100,
      "phase": "install",
      "stage": "receipt",
      "start_ms": 100
    },
    {
      "end_ms": 17100,
      "failed": true,
      "phase": "install",
      "stage": "downloading",
      "start_ms": 100
    }
  ],
  "status": "installed",
  "total_ms": 17100,
  "version": "0.47.0",
  "warnings": []
}
```

y stderr, igual con y sin `--json`:

```
Instalación de ai-voice-interconnector 0.47.0 · sin modificar el PATH
Se instalará ai-voice-interconnector 0.47.0 (x86_64-pc-windows-msvc)
  Programa:  %LOCALAPPDATA%\Programs\ai-voice-interconnector
  Comando:   %LOCALAPPDATA%\Programs\ai-voice-interconnector\ai-voice-interconnector.exe
  PATH:      no se modifica (--no-modify-path); invoca el comando por su ruta completa.
  Modelos:   se descargarán unos 14,9 MB en %LOCALAPPDATA%\ai-voice-interconnector\cache\models
  [1/3] ● Copiando el programa…
  [1/3] ✓ Copia     0,1 s · %LOCALAPPDATA%\Programs\ai-voice-interconnector
  [2/3] ● Escribiendo el recibo…
  [2/3] ✓ Recibo    0,0 s
  [3/3] ● Descargando dpdfnet8…
  [3/3] ● dpdfnet8 · 0 B · 0 B/s 10,0 s
2026-05-12T10:30:00.000000Z ERROR hf_hub::retry: retry exhausted url=https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964?recursive=true max_attempts=5
  [3/3] ✗ dpdfnet8  17,0 s
──────────────────────────────────────────────────
Error: La provisión de modelos no se completó (network_error: dpdfnet8: HTTP request error: error sending request for url (https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964?recursive=true) (https://huggingface.co/api/models/Ceva-IP/DPDFNet/tree/dd6818d00f50c836fed43a6243ebe49116de5964?recursive=true)). El programa queda instalado en %LOCALAPPDATA%\Programs\ai-voice-interconnector y puedes reintentarlo con `ai-voice-interconnector setup`.
Solución: Repite la descarga con `ai-voice-interconnector setup`.
```

**`self update --check`** (consulta: sin cabecera ni cierre):

```
Versión instalada: 0.47.0
Versión disponible: 0.47.0
```

Con `--json`:

```json
{
  "channel": "script",
  "current": "0.47.0",
  "latest": "0.47.0",
  "reason": null,
  "schema_version": "7",
  "update_available": false
}
```

**`self update --yes` con la versión ya al día**:

```
Actualización desde la versión 0.47.0
✓ Ya tienes la última versión (0.47.0)
```

**Simulación de `self uninstall`** (`self uninstall --dry-run`; el plan va por stdout):

```
Desinstalación de ai-voice-interconnector · simulación
Se desinstalará ai-voice-interconnector, incluido el estado de usuario.
4 rutas a tocar y 0 rutas conservadas:
  modelos: %LOCALAPPDATA%\ai-voice-interconnector\cache\models (6,7 GB)
  datos de usuario: %LOCALAPPDATA%\ai-voice-interconnector\data (0 B)
  carpeta vacía: %LOCALAPPDATA%\ai-voice-interconnector\cache
  programa: %LOCALAPPDATA%\Programs\ai-voice-interconnector (257,0 MB)
  no se tocará: los datos que el sistema operativo guarda por su cuenta (permisos, reglas del firewall, preferencias de audio); cómo borrarlos: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/blob/v0.47.0/docs/CLI/commands/SELF.md#datos-del-sistema-operativo
✓ Simulación completada · no se ha borrado nada
```

Con `--json`, `removed` va vacío y el plan está solo en `planned`:

```json
{
  "context": {
    "dry_run": true,
    "keep_data": false
  },
  "failed": [],
  "path_reverted": false,
  "planned": [
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\data",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache",
    "%LOCALAPPDATA%\\Programs\\ai-voice-interconnector"
  ],
  "preserved": [],
  "reason": null,
  "removed": [],
  "schema_version": "7",
  "status": "planned"
}
```

**`self uninstall --yes`** desde el ejecutable instalado: el directorio de programa queda programado para borrarse al terminar el proceso.

```
Desinstalación de ai-voice-interconnector
Se desinstalará ai-voice-interconnector, incluido el estado de usuario.
4 rutas a tocar y 0 rutas conservadas:
  modelos: %LOCALAPPDATA%\ai-voice-interconnector\cache\models (6,7 GB)
  datos de usuario: %LOCALAPPDATA%\ai-voice-interconnector\data (0 B)
  carpeta vacía: %LOCALAPPDATA%\ai-voice-interconnector\cache
  programa: %LOCALAPPDATA%\Programs\ai-voice-interconnector (257,0 MB)
  no se tocará: los datos que el sistema operativo guarda por su cuenta (permisos, reglas del firewall, preferencias de audio); cómo borrarlos: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/blob/v0.47.0/docs/CLI/commands/SELF.md#datos-del-sistema-operativo
  [1/4] ✓ Estado              0,6 s
  [2/4] ✓ Reversión del PATH  0,0 s
  [3/4] ✓ Programa            2,0 s
        ⚠ El directorio se borrará al terminar este proceso: %LOCALAPPDATA%\Programs\ai-voice-interconnector.
  [4/4] ✓ Bloqueos            0,0 s
──────────────────────────────────────────────────
✓ Desinstalación completada en 2,6 s
```

Cancelar la confirmación escribe el separador y `Cancelado; no se ha borrado nada.` (o `Cancelado.` si no es destructiva) **por stderr y una sola vez**, y sale con 0. Con `--json`, stdout lleva **solo** el envelope y stderr es el mismo.
