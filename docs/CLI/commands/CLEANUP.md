# `cleanup`

Borrado del estado por categorías, sin tocar el programa. Es la operación que hace posible quedarse con el programa y perder los datos (o al revés): borra modelos, voces, habla sintética, configuración, logs y estado del daemon, y **nunca** el directorio de programa ni su integración de `PATH` — eso es `self uninstall`.

**Implementación:** el motor es el módulo `cleanup` de `avi-lifecycle` (`crates/avi-lifecycle/src/cleanup.rs`), que resuelve la lista de destinos, la confirmación, la parada del daemon y el barrido. En `src/main.rs` solo queda `handle_cleanup`, que convierte tipos y compone el envelope `--json`. `uninstall` **no** es su reverso ni su supera: `self uninstall` reutiliza el mismo planificador para el estado y añade el directorio de programa, y es el único que borra el programa.

---

## Definición CLI (parser)

`Commands::Cleanup` declara:

| Argumento | Tipo | Categoría | Descripción |
|---|---|---|---|
| `--voices` | `bool` | datos | Voces no-fábrica y, con ellas, el arrastre `speech/<voz>` excepto `default` |
| `--synthetic-speech` | `bool` | datos | La raíz `speech/` entera, `default` incluida |
| `--model` | `bool` | datos | La hoja de modelos exclusiva, entera; y, si rige una caché HF compartida, solo lo atribuible de ella |
| `--all` | `bool` | datos | Las tres categorías **más** configuración, logs (`data/logs/`, con las familias `daemon_*` y `qwen3-tts_*`), estado del daemon y la hoja de temporales (categoría `scratch`, el directorio `tmp` del contenedor de caché entero). La retención de 10 logs por familia no es una opción de `cleanup`: se aplica sola al crear cada log |
| `--dry-run` | `bool` | — | Lista el plan sin borrar nada y **sin tomar el bloqueo** |
| `--yes`, `-y` | `bool` | — | Omite la confirmación interactiva |
| `--json` | `bool` | — | Global (`Cli::json`); con `cleanup` emite `status` + `reason` + `removed` + `planned` + `unreadable` (el plan como recibo, en ejecución y en `--dry-run`) |

**Gate `sin categoría → 2`.** Sin ninguna de las cuatro categorías, `cleanup` sale con `usage_error` (**2**) y **no borra nada**: la comprobación es la primera de `cleanup::run`, antes incluso de mirar privilegios o el bloqueo, de modo que una invocación mal formada no deja ni el archivo de bloqueo detrás.

---

## El planificador: una sola fuente de la lista

`cleanup::plan(roots, options)` construye la lista de destinos con sus tamaños aplicando **R1 y R3**, y la usan **las tres cosas que la necesitan**: el resumen de la confirmación, la salida de `--dry-run` y la ejecución. No puede haber divergencia entre lo que se anuncia y lo que ocurre porque no hay dos implementaciones: el plan de `--dry-run` y la lista de borrado salen de la misma llamada, como comprueba `plan_and_execution_agree_under_shared_root`.

### R1 y R3, en una frase cada una

- **R1**: solo se borra dentro de raíces de propiedad exclusiva. Una ruta fuera de ellas es un error interno y nunca se borra.
- **R3**: la hoja de modelos **exclusiva** (`<contenedor de caché registrado>/models`) se borra siempre de **directorio entero** (snapshots, locks y `xet` cuelgan de ella), haya o no una caché compartida vigente. Si además rige una raíz **compartida** que el usuario eligió con `HF_HUB_CACHE`/`HF_HOME`, de ella solo se borran los repos fijados (`MODEL_REVISIONS`), y sus locks: nunca `xet` ni el `.locks` completo, ni un repo de otra herramienta.

**R2 (el directorio de programa) no la aplica `cleanup`**: corresponde a `self uninstall` (ver [la especificación del ciclo de vida](../../specs/LIFECYCLE.md)). En el plan de `cleanup` el directorio de programa aparece como recurso **conservado**, con su motivo, porque un plan que no dice qué se queda no permite saber que los modelos de otra herramienta siguen ahí.

**La exclusividad se decide por identidad de ruta, no por el valor de la variable**: la hoja `models` del contenedor registrado en el recibo es siempre exclusiva, y que el usuario después apunte `HF_HUB_CACHE` a otro sitio no la convierte en compartida; esa caché es una raíz externa adicional. Es lo que evita que un recibo viejo autorice borrar lo ajeno o, al revés, que una variable obsoleta impida borrar lo propio.

**Las raíces llegan como dato, no se releen del entorno dentro del planificador**, por dos razones: leer `HF_HUB_CACHE` desde el planificador hace que dos pruebas que corren en paralelo se contaminen (el entorno del proceso es global), y el plan tiene que ser una función de sus entradas —si dependiera del entorno, `--dry-run` podría anunciar un borrado distinto del que la ejecución hace un instante después—.

---

## Ejecución, paso a paso

1. **Gate de categoría**: sin categoría → `usage_error` (2), sin borrar nada.
2. **Privilegios**: ninguna operación pide elevación. En Unix, si se detecta ejecución vía `sudo` (uid 0 con `SUDO_USER`), se aborta con `sudo_not_supported` (1); root sin `sudo` (contenedores) sí se permite. En Windows, si el proceso está elevado, se avisa por stderr.
3. **`--dry-run`**: imprime el plan, incluido el barrido que *se haría*, después una sola vez `Simulación: no se ha modificado nada.`, y termina con exit 0 **sin tomar el bloqueo, sin barrer y sin borrar**. El barrido no se ejecuta en la simulación porque lo que no puede coexistir con una simulación es modificar el disco; lo que sí hace es **anunciarlo con la misma decisión** del barrido real, de modo que lo que dice el `--dry-run` es lo que ocurriría.
4. **Bloqueo y recuperación**: bloqueo exclusivo de SO (el mismo, sobre el directorio de programa registrado, que toman `self update` y `self uninstall`; excluye a los comandos de uso, que lo toman compartido) y barrido transversal por identidad exacta: cada residuo (scratch, temporales con token, stagings de instalación, aparcados `.old-<token>-<nanos>` y registros `*.removal-failed.log` cuyo destino ya no existe) lleva el token `<pid>-<inicio>` de su dueño, y se barre si el dueño está muerto (inexistente o con otro inicio, como un PID reutilizado), se conserva sin más si vive y se conserva e informa si no se puede observar. Un registro `*.removal-failed.log` cuyo destino sigue existiendo se informa como borrado diferido sin completar. El barrido va **antes** del plan, para que el plan que el usuario ve sea el que queda después y no una lista que el barrido va a invalidar.
5. **Plan** y **confirmación destructiva** `[s/N]`. El plan sale de la misma composición que el de la simulación (`cleanup::plan_lines`): `Se borrarán N ruta(s), X,Y GB en total:` y una línea por destino, cada uno una sola vez, con su categoría en español y su tamaño (`  modelos: <ruta> (5,9 GB)`; las categorías son `modelos`, `voces`, `locuciones`, `configuración`, `registros`, `estado del daemon` y `temporales`), seguido de lo que no se tocará.
6. **Parar el daemon** antes de borrar nada que use. Si no se detiene → `daemon_stop_failed` (**16**) y **nada del plan se borra**.
7. **Borrado** de cada destino del plan, en el orden del plan, y después la **poda de contenedores**: el ejecutor del árbol de propiedad recorre de abajo arriba los contenedores ancestros de lo borrado y aplica `remove_dir` (nunca recursivo) a los que quedaron vacíos. Un contenedor con cualquier entrada ajena se conserva y se informa en `preserved` (una vez), y la poda de esa rama se detiene ahí. Una raíz de modelos compartida (`External`) nunca se poda. Los contenedores podados entran en `planned` y `removed` tras los destinos. `cleanup --all` no borra el contenedor raíz de la aplicación en Windows (`%LOCALAPPDATA%\ai-voice-interconnector`), porque la raíz de datos sigue en pie con los datos de fábrica; el contenedor de caché sí se poda si queda vacío. El directorio de programa solo entra en `preserved` si existe.

**`NotFound` es éxito.** El plan dice lo que tiene que dejar de existir, y si ya no existe el objetivo está cumplido. No es un caso teórico: la parada del daemon borra el pidfile cuando no había daemon vivo, y el pidfile es un destino de `--all`.

**Lo que no se pudo borrar no es un fallo de la operación**: cada destino que falla se avisa por stderr (`no se pudo borrar <ruta>: <motivo>`) y el borrado sigue con el resto del plan, sin tumbar la operación. Ese destino no entra en `removed` pero sí sigue en `planned`, así que con `--json` se detecta comparando las dos listas; el envelope no tiene una clave propia para él.

---

## Confirmación: la tabla de las reglas transversales, y solo su parte destructiva

La tabla vive en las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) del ciclo de vida.

`cleanup` es **destructiva**, y su celda es la que obliga:

| | Con terminal (TTY) | Sin terminal |
|---|---|---|
| `cleanup` | Plan con categorías y tamaños, luego `Esto eliminará lo indicado. ¿Continuar? [s/N] ` (con espacio final) | **Exige `--yes`**: sin él termina con `confirmation_required` (2) y **no borra nada** |

La aceptación es `s`/`si`/`sí`/`y`/`yes`/`dale`/`ok` (sin distinguir mayúsculas). **Cualquier otra respuesta es un no**, incluido el salto de línea, y también en el prompt no destructivo: una errata nunca debe autorizar un borrado. Responder «no» **no es un error**: es `status` `cancelled` con salida 0, porque las reglas transversales no cuentan la cancelación entre los `reason`.

El prompt va a **stderr** y la respuesta se lee de **stdin**, para que `--json` no se contamine. La celda no destructiva de la tabla —lo que no borra nada procede sin preguntar— es la de `self install` y `setup`, **no** la de `cleanup`.

Las tres operaciones destructivas del producto (`cleanup`, `self uninstall` y la purga de `setup --force-update`) usan **el mismo módulo `confirm`**, para que esa tabla tenga una sola implementación.

---

## Contrato JSON (`--json`)

```json
{
  "schema_version": "5",
  "status": "cleanup_complete",
  "reason": null,
  "removed": ["…/data/voices/mi_voz", "…/speech/mi_voz"],
  "planned": ["…/data/voices/mi_voz", "…/speech/mi_voz"],
  "unreadable": []
}
```

`unreadable` lista los directorios del barrido transversal que existen y no se pudieron listar (lo que contienen queda sin recoger); en texto, la ejecución real y `--dry-run` los avisan por stderr, una línea por ruta.

`status` toma `cleanup_complete`, `planned` o `cancelled`. `planned` es el recibo de `plan()`: lo que la operación iba a borrar, y en `--dry-run` lo único que dice, porque no borra nada. `removed` son las rutas del plan con lo que la operación borró de verdad, y con `--dry-run` copia `planned`, que es lo que permite comparar el simulacro con la ejecución real. **Los dos salen de la misma llamada al planificador**, así que no pueden divergir. Exactamente un objeto JSON por invocación, incluso cuando falla: sin categoría con `--json` sale el objeto de error de §10 del contrato (`error` + `reason`), y nada más.

Sobre `schema_version`, ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema). El simulacro informa `status` `planned` sin clave `dry_run`: con `planned` la clave sobra.

---

## Reparto con el resto de la superficie

| Necesidad | Comando |
|---|---|
| Borrar una locución | `speech remove --label` |
| Borrar muchas locuciones | `cleanup --synthetic-speech` |
| Borrar una voz clonada | `voice remove --name X` |
| Borrar las voces del usuario | `cleanup --voices` |
| Liberar el espacio de los modelos y volver a descargarlos con `setup` | `cleanup --model` |
| Desinstalar el programa y su `PATH` | `self uninstall` |
| Desinstalar conservando modelos, voces y habla | `self uninstall --keep-data` |

**`--voices` nunca borra las voces de fábrica** (`default`, `ryan`, `vivian`): van embebidas en el binario y el programa sigue instalado, así que `setup` las vuelve a materializar. `voice remove` las protege con exit 2 por la misma razón. Y **`self uninstall` sí las borra**, porque al desinstalar desaparece el programa: es la diferencia de alcance entre `cleanup --all` y `self uninstall` sin `--keep-data`, y el motivo está en [`SELF.md`](SELF.md).

**Tras `--model` la aplicación queda reintentable**: `setup` vuelve a descargar lo que falte, porque la provisión no depende de nada que sobreviva al borrado.

---

## Errores

| Condición | Código | `reason` |
|---|---|---|
| Sin categoría | 2 | `usage_error` |
| Sin terminal y sin `--yes` | 2 | `confirmation_required` |
| No se pudo detener el daemon | 16 | `daemon_stop_failed` |
| Otra operación de ciclo de vida tiene el bloqueo | 17 | `lifecycle_locked` |
| Unix: ejecución lanzada con `sudo` | 1 | `sudo_not_supported` |
| Cancelación del usuario | 0 | — (`status` `cancelled`, no es un error) |
| Destino que no se pudo borrar | 0 | — (aviso por stderr; está en `planned` y no en `removed`, no tumba la operación) |

---

## Ejemplos

```bash
ai-voice-interconnector cleanup --model               # libera los modelos; reintentable con setup
ai-voice-interconnector cleanup --all --dry-run       # lista el plan, no borra y no deja el bloqueo
ai-voice-interconnector cleanup --voices --yes        # omite la confirmación ( -y alias )
ai-voice-interconnector --json cleanup --model --dry-run
```
