# Grupo `self` — ciclo de vida de la instalación del usuario

`self` es el grupo con el que **el binario gestiona su propia instalación**. Reemplaza al comando de nivel superior `uninstall` —que ya no existe, sin alias ni flag deprecado— y cubre los dos lados del ciclo de vida: `self install` instala o repara, `self update` actualiza a la última estable o a una concreta, y `self uninstall` desinstala.

La Normativa del grupo está en `docs/specs/sdlc-lifecycle.md` (§5.4 superficie, §8.1 reglas transversales, §8.3 `self install`, §8.5 `self uninstall`); el contrato de la CLI —flags, `reason`, códigos de salida, sobre `--json`— está en [`../CONTRACT.md`](../CONTRACT.md). Este documento describe **dónde vive cada cosa y por qué**.

**Implementación:** el motor es el crate `avi-lifecycle` (`crates/avi-lifecycle/src/install.rs` y `uninstall.rs`), sin punto de entrada propio: el parseo de la CLI y el cableado se quedan en `src/main.rs` (`handle_self`), porque el motor no depende de `clap` ni de `avi-core` (§5.3 de la especificación). El binario aporta las dos primitivas que el motor no puede tener —el control de procesos (`ProductProcesses`, que vive en `avi-daemon`/`avi-tts`) y el borrado diferido de Windows (`ProgramRemoval`, que usa `avi_process::spawn_deferred_removal`)—. El motor decide; el binario ejecuta esas dos.

---

## Definición CLI (parser)

`SelfSub` (`src/main.rs`), el enum de subcomandos. Se llama `SelfSub` y no `SelfComandos` por dos razones que el propio código declara: `Self` es palabra reservada de Rust, y el nombre no debe colisionar por subcadena con el enum de comandos de nivel superior, cuya variante de desinstalación este ciclo retira —una comprobación de que esa variante ya no aparece en el árbol tiene que dar cero, y un enum cuyo nombre la contenga daría una coincidencia sin que el comando existiera—.

| Subcomando | Flags | Efecto |
|---|---|---|
| `self install` | `--no-setup` · `--no-modify-path` · `--force`/`-f` · `--yes` · `--json` · `--channel` (oculta) | Instala el bundle del que forma parte el ejecutable, o repara la instalación si se ejecuta desde ella |
| `self update` | `--check` · `--version X.Y.Z` · `--force`/`-f` · `--no-setup` · `--yes` · `--json` | Actualiza la instalación registrada a la última estable o a una concreta, con verificación y traspaso |
| `self uninstall` | `--keep-data` · `--dry-run` · `--yes` · `--json` | Borra el estado, revierte el `PATH` y borra el directorio de programa |

`--json` es global (`Cli::json`), no un flag por subcomando. `--channel` es la única opción oculta del grupo: la reserva `cargo xtask install` (§9.5) y **solo surte efecto cuando el recibo se crea por primera vez**, de modo que reparar una instalación no reescriba su canal (`install::resolve_channel`).

---

## Los tres modos de `self install`

El modo lo decide **la posición del ejecutable**, no un flag:

| Modo | Condición | Qué hace |
|---|---|---|
| `Install` | El ejecutable está **fuera** del directorio de programa (bundle en staging o extraído a mano) | Instala, o reemplaza si ya hay una versión |
| `Repair` | El ejecutable está **dentro** del directorio de programa | Reaplica integración de `PATH`, permisos, cuarentena y recibo **sin copiar archivos** |
| — | El ejecutable **no tiene bundle alrededor** (por ejemplo `target\debug`) | No es un modo: es el desenlace de validar el bundle, `bundle_invalid` (**15**) con la indicación de usar `cargo xtask install` |

La comparación es por **clave canónica** (`avi_store::canonical_path_entry_matches`), no de cadenas: las dos rutas llegan de resoluciones distintas y una barra final de más daría un modo equivocado sin dar ningún error (`install::detect_mode`).

**El caso del ejecutable sin bundle es el que más confunde y el que hay que saber de memoria:** `target\debug\ai-voice-interconnector.exe self install` responde `bundle_invalid` con salida 15 no porque la instalación esté rota, sino porque no hay nada alrededor del binario que instalar. El bundle se produce con `cargo xtask package`, que es el ciclo 3.

---

## Los doce pasos de `self install`

El orden es el de §8.3 y está escrito en el propio código, con el número de paso en cada bloque:

1. **Recuperación y bloqueo** (`install.rs`). La recuperación va **con el bloqueo tomado**, no antes: si fuera al revés, el barrido de stagings huérfanos se llevaría por delante el staging que esta misma operación va a instalar. Por eso el bundle de donde se invoca se declara **en uso** (`recovery::Roots.in_use`). Este campo existe por un defecto real: sin él, `self install` borraba su propio bundle entre los pasos 1 y 2.
2. **Validar el bundle** contra la lista de archivos del target, fijada en compilación (`BUNDLE_MANIFEST`, embebido desde `packaging/bundle-manifest.json`). Es la misma lista que usa `cargo xtask package`, de modo que empaquetado e instalación no pueden divergir. Si falta un archivo → `bundle_invalid` (15), sin haber modificado nada.
3. **Detectar la instalación previa**: registrada (recibo) o ajena. Una ajena (un Cask en el `PATH`) **solo genera aviso** de coexistencia y precedencia: no bloquea.
4. **Resumen previo y confirmación.** Instalar una versión **menor** que la instalada es una degradación y se confirma como operación **destructiva** (`confirm::Kind::Destructive`), aunque instalar no borre nada por sí mismo. Cancelar aquí es salida 0 y ningún `reason`: §8.1 no cuenta la cancelación entre los `reason`.
5. **Parar el daemon**, incluido el proceso residente del motor. Si no se detiene → `daemon_stop_failed` (16), **sin modificar nada**.
6. **Reemplazo transaccional** (`transaction::replace`): aparcar, colocar, ajustar permisos, confirmar y borrar lo aparcado; revertir restaurando lo aparcado si falla → `rolled_back` (13). **En reparación no se copia nada.**
7. **macOS**: eliminar `com.apple.quarantine` de forma recursiva en **todo** el directorio de programa, no solo en el ejecutable, porque el motor y la librería de ONNX Runtime también se ejecutan o cargan. No es un fallo de la instalación: la cuarentena que no se quita degrada el arranque y el resumen lo informa.
8. **Integración de `PATH`** ([`../CONTRACT.md` §11](../CONTRACT.md) y §8.3.1 de la especificación). En Unix el `PATH` se modifica **por defecto** (decisión D2) y se anuncia en el resumen; `--no-modify-path` lo desactiva. Un archivo ajeno en la ruta del enlace → `path_conflict` (14), salvo `--force`.
9. **Windows**: si el `PATH` de máquina parece llevar una instalación per-machine antigua, se avisa y se muestra el comando exacto para quitarla desde una PowerShell de administrador. **HKLM nunca se modifica**, ni para escribir ni para leer.
10. **Escribir el recibo** de forma atómica y liberar el bloqueo.
11. **La misma provisión que `setup`**, salvo `--no-setup`: descarga de la selección guardada y conversión CT2 obligatoria según el estado real del almacén, sin repetir la confirmación ni escribir la selección.
12. **Resumen final**: versión, rutas, estado del `PATH` y estado de los modelos.

**Lo que el recibo registra es el estado, no el diff de esta pasada.** El recibo dice qué integración de `PATH` está en pie, no qué cambió en esta ejecución: si registrara solo el diff, una segunda instalación desde el mismo bundle escribiría un recibo sin entrada de `PATH` y `self uninstall` no podría revertir la que puso la primera —dejando residuo, que es el criterio 17—. El diff vive aparte, en `Outcome::path_rewritten`, y es lo único que decide si el resumen pide abrir una terminal nueva.

**El resumen final lleva una sola línea del `PATH`, y es la del estado.** Copiar debajo la línea del plan imprimía «se añadirá X» y «se añadió X» en el mismo bloque, que parece una contradicción y no es más que el mismo hecho contado dos veces.

---

## `self update`: los once pasos

El orden es el de §8.4 y está escrito en el propio código (`handle_self`, brazo `Update`):

1. **Recuperación y bloqueo.** La recuperación corre con el bloqueo tomado sobre la instalación registrada; barre aparcados, stagings huérfanos y temporales propios sin proceso vivo.
2. **Leer el recibo y el canal** (§7.2). `homebrew` o `dev` → `externally_managed` (12) con el comando correcto; sin instalación → `not_installed` con el one-liner y salida 3.
3. **Resolver la versión objetivo**: `--version` explícito sin tocar la red, o la última estable siguiendo la redirección de `releases/latest` (con `AVI_DOWNLOAD_BASE_URL` si está definida); la API REST solo es respaldo.
4. **Comparar las versiones** con el comparador numérico vigente: iguales → `already_up_to_date` sin descargar, éxito con 0 (`--force` reinstala); objetivo menor → solo con `--version` explícito y marca destructiva.
5. **`--check`**: informa la transición (`anterior → nueva`, o que ya se está en la última) y termina sin cambios de actualización. En `--json`: `current`, `latest`, `update_available` y `channel`.
6. **Resumen y confirmación.** Normal `[S/n]`; degradación `[s/N]` con `confirmation_required` sin terminal.
7. **Preparar el bundle nuevo**: descarga del archivo y de `SHA256SUMS.txt` en un staging hermano con HTTPS y reintentos acotados, verificación de coincidencia exacta en `SHA256SUMS.txt` (discrepancia o línea ausente → `checksum_mismatch`, 21, con staging borrado), extracción y comprobación de arranque con coincidencia de `--version` (si no → `binary_incompatible`, 19) y validación contra el manifiesto.
8. **Parar el daemon con el binario actual**, que conoce su protocolo y su `daemon.pid`, anotando si estaba en ejecución. Si no se detiene → `daemon_stop_failed` (16) sin tocar nada y con el staging retirado.
9. **Traspaso**: ejecuta `<staging>/ai-voice-interconnector self install --yes` heredando la consola, con las preferencias del recibo (`--no-modify-path` si la instalación no tocaba el `PATH`), `--no-setup` si se pidió, y **`--force` propagado cuando el `update` lo recibió**. Se espera y se propaga el resultado: éxito, `setup_failed` parcial con `models_cause`, o `rolled_back`.
10. **Limpiar**: borra siempre el staging; lo aparcado que siga en uso (en Windows, el ejecutable del proceso que actualiza) queda a borrado diferido con un auxiliar desacoplado que espera la muerte del proceso y reintenta de forma acotada, o a la recuperación de la siguiente operación. En Unix no hay diferido.
11. **Resultado**: `anterior → nueva`. Sin reinicio automático del daemon: si estaba activo, el resumen indica cómo relanzarlo con el comando habitual.

**Garantías.** Un fallo antes del traspaso deja todo intacto (staging borrado, instalación intacta). Un fallo durante el traspaso revierte la transacción nueva del `self install` invocado. Una interrupción en cualquier punto queda recuperable en la siguiente operación de ciclo de vida.

---

## `setup_failed`: el único desenlace que no es ni éxito ni error

Si el `setup` del paso 11 falla, **la instalación no falla**. §8.1 lo declara éxito parcial, y el código lo modela así:

| | Valor |
|---|---|
| `status` | `installed` (o `repaired`) |
| `reason` | `setup_failed` |
| Código de salida | **11** (`ExitCode::SetupFailed`) |
| Forma de salida | **Por veredicto**: el sobre y el resumen se emiten igual, y **no** se adjunta el objeto `error` |
| Qué hacer | Reintentar con `setup`: el programa está instalado |

El motivo del fallo de provisión **no se pierde**: viaja anidado en `models_cause`, con su propio `reason` —`network_error` para un fallo de descarga, `ct2_conversion_failed` para uno de conversión— y su mensaje. Los dos `reason` dicen cosas distintas y por eso no se funden: uno dice **qué** falló y el otro **qué dejó de completarse**.

```json
{
  "schema_version": "4",
  "status": "installed",
  "reason": "setup_failed",
  "install_dir": "…/ai-voice-interconnector",
  "version": "0.23.1",
  "channel": "script",
  "path_integrated": true,
  "models": "failed",
  "models_cause": { "reason": "network_error", "message": "…" }
}
```

`models_cause` **solo existe si hubo fallo**: un sobre estable es más fácil de leer que uno con nulos. Los cuatro valores de `models` son `skipped` (`--no-setup`), `already_provisioned`, `provisioned` y `failed`.

**`ct2_conversion_failed` no es nunca el código de salida del proceso.** Es un `reason` anidado cuyo valor declarado es **1**, el del error genérico, y el proceso sale con el de la operación (`setup_failed`, 11). Anidarlo con un 11 haría que un consumidor leyera un 11 donde la tabla de §8.1 no lo promete.

Cuando `setup` se invoca **directamente** (no desde `self install` ni desde el traspaso de `self update`), un fallo de conversión sale con `reason` `setup_failed` y 11, y un fallo de descarga sale con `network_error` y **20**. El `self update` propaga el mismo parcial: si el `setup` del binario nuevo falla, el resultado es `updated` con `reason` `setup_failed`, salida 11 y causa anidada en `models_cause`.

---

## `self uninstall`: qué se borra y por qué

Los nueve pasos de §8.5, en orden, sobre la **instalación registrada** y no sobre la posición del ejecutable.

1. **El canal decide si se puede actuar**: `homebrew` → `externally_managed` (12) con el comando de Homebrew en el mensaje. Sin instalación, sin estado y sin directorio de programa → **`not_installed` y salida 0** (criterio 21).
2. **`--dry-run`**: imprime el plan y termina **sin tomar el bloqueo**, porque tomar el bloqueo crea el archivo y una simulación que deja un archivo detrás no es una simulación (criterio 20).
3. **Plan** con rutas y tamaños, más la lista de lo que **no** se tocará y por qué.
4. **Confirmación destructiva** `[s/N]`. Sin terminal y sin `--yes` → `confirmation_required` (2) y nada borrado.
5. **Parar el daemon**: va **después** de la confirmación, para que un `daemon_stop_failed` no deje nada a medias.
6. **Borrar el estado.**
7. **Revertir el `PATH`** exactamente según el recibo: el enlace solo si apunta al directorio de programa, los bloques delimitados de los perfiles, y en Windows la entrada del registro con comparación canónica conservando el tipo del valor y difundiendo `WM_SETTINGCHANGE`.
8. **Borrar el directorio de programa** aplicando R2.
9. **Borrar el archivo de bloqueo**, que vive en el hermano del directorio de programa: se borra también cuando el directorio quedó programado para después.

### El paso 6: la raíz de datos entera, no el plan de `cleanup --all`

**Sin `--keep-data`, el destino del estado es la raíz de datos completa.** No es el plan de `cleanup --all`, y la diferencia es deliberada:

- `cleanup --all` **protege las voces de fábrica** (`default`, `ryan`, `vivian`) porque van embebidas en el binario y el programa sigue instalado: borrarlas sería tirar un trabajo que `setup` vuelve a materializar.
- Al desinstalar **el programa desaparece**, y con él las voces de fábrica. Dejarlas sería **residuo dentro de una raíz de propiedad exclusiva**, que es exactamente lo que prohíbe el criterio 17.

Por eso la composición del plan vive en `uninstall::compose_plan` y tiene dos ramas, no dos implementaciones del mismo alcance:

| Invocación | Destinos del estado |
|---|---|
| Sin `--keep-data` | Los del plan de modelos (raíz de modelos según R1–R3) **más la raíz de datos entera** |
| Con `--keep-data` | El plan de `cleanup --all` **filtrado**: fuera modelos, voces y habla; dentro configuración, logs y estado del daemon |

Con `--keep-data` el directorio de programa **se borra igual**: la bandera conserva el estado, no el programa.

**R2 gobierna el paso 8 y por eso tiene su propia función pública** (`program_dir_is_removable`). Exige las dos mitades de la regla: la positiva —el directorio contiene el recibo o el ejecutable, que es lo que lo convierte *en* el directorio de programa— y la negativa —nunca es la raíz de una unidad, `$HOME`, un ancestro de `$HOME` ni coincide con otra raíz del producto—. Ni una variable de reubicación ni un recibo manipulado pueden ampliar el alcance.

En Windows, si el ejecutable en uso está dentro del directorio de programa, el borrado se **programa** para cuando termine el proceso (`removal_scheduled`, que es éxito) en vez de hacerse de forma síncrona. El auxiliar es un `powershell.exe` con consola oculta cuyo script escribe una marca `.ready` como primera instrucción (con `Set-Content`); el comando solo da el borrado por programado cuando la marca aparece o el auxiliar sigue vivo tras el plazo, y borra el script y la marca si el auxiliar muere sin arrancar. Si el borrado no se puede programar (o, fuera de Windows, el directorio no se puede borrar), el resto de la desinstalación se completa y el comando termina con `status` `uninstalled`, `reason` `program_dir_kept` y salida 22, sin borrado parcial del directorio.

### Idempotencia y residuo

Sin instalación ni estado, `self uninstall` termina con éxito y `status` `not_installed`. Con operación completa, **cero residuo dentro de las raíces de propiedad exclusiva**, y lo compartido que no se borra se informa explícitamente con su motivo: es la lista `preserved`, que forma parte del plan y se imprime con él.

---

## Bloqueo, recuperación y el reparto con el binario

- **El bloqueo es exclusivo de SO** (`flock` en Unix, `LockFileEx` en Windows) sobre el archivo de §6, y el SO lo libera aunque el proceso muera. Mientras está tomado, una segunda operación de ciclo de vida sale con `lifecycle_locked` (**17**). La propiedad de la que depende: dos instalaciones concurrentes no se pisan.
- **La recuperación corre antes de componer el plan**, para que el plan que el usuario ve y confirma sea el que queda después del barrido de aparcados, stagings huérfanos y temporales propios sin proceso vivo.
- **El motor no puede matar un árbol de procesos ni agendar un borrado en Windows**, así que ambos entran por rasgos (`ProcessControl`, `ProgramDirRemover`). La frontera es deliberada: arrastrar `avi-daemon` y `avi-tts` al crate del motor crearía un ciclo de dependencias, y §5.3 lo prohíbe.

---

## Contrato `--json`

| Subcomando | Claves |
|---|---|
| `self install` | `status` · `reason` · `install_dir` · `version` · `channel` · `path_integrated` · `models` · `models_cause` (solo si `models` es `failed`) |
| `self update` | `status` (`updated` o `already_up_to_date` o `check`) · `reason` (`setup_failed` en el parcial) · `previous_version` · `version`/`latest` · `channel` · `current`/`update_available` (en `--check` y `already_up_to_date`) · `models_cause` (solo en el parcial) |
| `self uninstall` | `status` · `reason` (`null`) · `removed` · `path_reverted` · `dry_run` |

`status` toma los valores `installed` / `repaired` en `self install`, y `uninstalled` / `removal_scheduled` / `not_installed` / `cancelled` en `self uninstall`. `schema_version` lo inyecta `emit_raw_json` y vale **`"4"`**; el protocolo del daemon sigue en `"3"` porque es otro contrato.

`--json` no cambia ninguna fila de éxito: el comando hace lo mismo y además emite su payload. La excepción es `setup_failed`, que sí cambia la salida, y por eso está declarado como salida por veredicto y no como error.

---

## Errores

| `reason` | Código | Cuándo |
|---|---|---|
| `bundle_invalid` | 15 | Falta un archivo obligatorio del bundle; nada modificado |
| `lifecycle_locked` | 17 | Otra operación de ciclo de vida tiene el bloqueo |
| `path_conflict` | 14 | Hay un archivo ajeno en la ruta del enlace; nada aplicado, salvo `--force` |
| `daemon_stop_failed` | 16 | No se pudo detener el daemon; nada del plan se aplicó |
| `rolled_back` | 13 | Fallo en el reemplazo; la versión anterior quedó restaurada |
| `externally_managed` | 12 | `homebrew`: la copia la gestiona Homebrew (`self uninstall`); también el canal `dev` en `self update` |
| `confirmation_required` | 2 | Destructiva sin terminal y sin `--yes` |
| `setup_failed` | 11 | `self install` terminó con el programa instalado y la provisión sin completar; `self update` propaga el mismo parcial del binario nuevo |
| `already_up_to_date` | 0 | `self update` sin descarga: ya se está en la versión objetivo (éxito) |
| `not_installed` | 3 | `self update` sin instalación registrada, con el one-liner |
| `unsupported_platform` | 18 | Target no soportado, antes de tocar la red |
| `binary_incompatible` | 19 | El binario descargado no arranca o no informa la versión objetivo, con diagnóstico |
| `network_error` | 20 | Fallo de descarga tras reintentos acotados |
| `checksum_mismatch` | 21 | El hash no coincide o falta en `SHA256SUMS.txt`; staging borrado y nada más modificado |
| `program_dir_kept` | 22 | `self uninstall`: el resto se completó, pero el directorio de programa no se pudo borrar ni programar su borrado |

Los `reason` del ciclo 3 salen con el **1** genérico: los declara el ciclo que también fija su entero.
