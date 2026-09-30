# `cleanup`

Borrado del estado por categorías, sin tocar el programa. Es la operación que hace posible quedarse con el programa y perder los datos (o al revés): borra modelos, voces, habla sintética, configuración, logs y estado del daemon, y **nunca** el directorio de programa ni su integración de `PATH` — eso es `self uninstall`.

**Implementación:** el motor es el módulo `cleanup` de `avi-lifecycle` (`crates/avi-lifecycle/src/cleanup.rs`), que resuelve la lista de destinos, la confirmación, la parada del daemon y el barrido. En `src/main.rs` solo queda `handle_cleanup`, que convierte tipos y compone el sobre `--json`. `uninstall` **no** es su reverso ni su supera: `self uninstall` reutiliza el mismo planificador para el estado y añade el directorio de programa, y es el único que borra el programa.

---

## Definición CLI (parser)

`Commands::Cleanup` declara:

| Argumento | Tipo | Categoría | Descripción |
|---|---|---|---|
| `--voices` | `bool` | datos | Voces no-fábrica y, con ellas, el arrastre `speech/<voz>` excepto `default` |
| `--synthetic-speech` | `bool` | datos | La raíz `speech/` entera, `default` incluida |
| `--model` | `bool` | datos | La raíz de modelos: entera si es exclusiva, o solo lo atribuible si es compartida |
| `--all` | `bool` | datos | Las tres categorías **más** configuración, logs y estado del daemon |
| `--dry-run` | `bool` | — | Lista el plan sin borrar nada y **sin tomar el bloqueo** |
| `--yes`, `-y` | `bool` | — | Omite la confirmación interactiva |
| `--json` | `bool` | — | Global (`Cli::json`); con `cleanup` emite `status` + `reason` + `removed` + `dry_run` |

**Gate `sin categoría → 2`.** Sin ninguna de las cuatro categorías, `cleanup` sale con `usage_error` (**2**) y **no borra nada**: la comprobación es la primera de `cleanup::run`, antes incluso de mirar privilegios o el bloqueo, de modo que una invocación mal formada no deja ni el archivo de bloqueo detrás (criterio 22).

---

## El planificador: una sola fuente de la lista

`cleanup::plan(roots, options)` construye la lista de destinos con sus tamaños aplicando **R1 a R3**, y la usan **las tres cosas que la necesitan**: el resumen de la confirmación, la salida de `--dry-run` y la ejecución. No puede haber divergencia entre lo que se anuncia y lo que ocurre porque no hay dos implementaciones.

**Lo que sustituye.** Antes de este ciclo, `handle_cleanup` en `src/main.rs` calculaba la lista de candidatas y la lista de borrado **por separado**: el plan de `--dry-run` anunciaba `xet` y `.locks` sin mirar si la raíz de modelos era compartida, mientras el ejecutor devolvía `Ok(false)` bajo R3. El resultado era que `cleanup --model --dry-run` anunciaba un borrado que no ocurría, y que ninguna prueba lo detectaba porque nadie comparaba las dos listas. Hoy la hay (`plan_and_execution_agree_under_shared_root`).

### R1 a R3, en una frase cada una

- **R1**: solo se borra dentro de raíces de propiedad exclusiva. Una ruta fuera de ellas es un error interno y nunca se borra.
- **El directorio de programa no lo toca `cleanup`** —eso es `self uninstall`— y aparece en el plan como recurso **conservado**, con su motivo, porque un plan que no dice qué se queda no permite saber que los modelos de otra herramienta siguen ahí.
- **R3**: en la raíz de modelos **exclusiva** el borrado es de **directorio entero** (snapshots, derivado CT2, locks y `xet` cuelgan de ella). En la raíz **compartida** que el usuario eligió con `HF_HUB_CACHE`/`HF_HOME` solo se borran los repos fijados (`MODEL_REVISIONS`), sus locks y el derivado `ct2`: nunca `xet` ni el `.locks` completo, ni un repo de otra herramienta (criterio 23).

**La exclusividad se decide por identidad de ruta, no por el valor de la variable**: la raíz registrada en el recibo era exclusiva en el momento de instalar, y que el usuario después apunte `HF_HUB_CACHE` a otro sitio no convierte la raíz registrada en compartida. Es lo que evita que un recibo viejo autorice borrar lo ajeno o, al revés, que una variable obsoleta impida borrar lo propio.

**Las raíces llegan como dato, no se releen del entorno dentro del planificador**, por dos razones: leer `HF_HUB_CACHE` desde el planificador hace que dos pruebas que corren en paralelo se contaminen (el entorno del proceso es global), y el plan tiene que ser una función de sus entradas —si dependiera del entorno, `--dry-run` podría anunciar un borrado distinto del que la ejecución hace un instante después—.

---

## Ejecución, paso a paso

1. **Gate de categoría**: sin categoría → `usage_error` (2), sin borrar nada.
2. **Privilegios**: ninguna operación pide elevación. En Unix, si se detecta ejecución vía `sudo` (uid 0 con `SUDO_USER`), se aborta; root sin `sudo` (contenedores) sí se permite. En Windows, si el proceso está elevado, se avisa por stderr.
3. **`--dry-run`**: imprime el plan, incluido el barrido que *se haría*, y termina con exit 0 **sin tomar el bloqueo, sin barrer y sin borrar**. El barrido no se ejecuta en la simulación porque lo que no puede coexistir con una simulación es modificar el disco (criterio 20); lo que sí hace es **anunciarlo con la misma decisión** del barrido real, de modo que lo que dice el `--dry-run` es lo que ocurriría.
4. **Bloqueo y recuperación**: bloqueo exclusivo de SO y barrido transversal (aparcados `.old-*`, stagings huérfanos, temporales propios sin proceso vivo). El barrido va **antes** del plan, para que el plan que el usuario ve sea el que queda después y no una lista que el barrido va a invalidar.
5. **Plan** y **confirmación destructiva** `[s/N]`.
6. **Parar el daemon** antes de borrar nada que use. Si no se detiene → `daemon_stop_failed` (**16**) y **nada del plan se borra**.
7. **Borrado** de cada destino del plan, en el orden del plan.

**`NotFound` es éxito.** El plan dice lo que tiene que dejar de existir, y si ya no existe el objetivo está cumplido. No es un caso teórico: la parada del daemon borra el pidfile cuando no había daemon vivo, y el pidfile es un destino de `--all`.

**Lo que no se pudo borrar no es un fallo de la operación**: §8.1 lo considera un archivo en uso que recoge el borrado diferido, y va en `kept`/`failed` con su motivo en lugar de tumbar la operación entera.

---

## Confirmación: la tabla de §8.1, y solo su parte destructiva

`cleanup` es **destructiva**, y su celda es la que obliga:

| | Con terminal (TTY) | Sin terminal |
|---|---|---|
| `cleanup` | Lista de rutas con tamaños, luego `Esto eliminará lo indicado. ¿Continuar? [s/N]` | **Exige `--yes`**: sin él termina con `confirmation_required` (2) y **no borra nada** |

La aceptación es `s`/`si`/`sí`/`y`/`yes`/`dale`/`ok` (sin distinguir mayúsculas). **Cualquier otra respuesta es un no**, incluido el salto de línea, y también en el prompt no destructivo: una errata nunca debe autorizar un borrado. Responder «no» **no es un error**: es `status` `cancelled` con salida 0, porque §8.1 no cuenta la cancelación entre los `reason`.

El prompt va a **stderr** y la respuesta se lee de **stdin**, para que `--json` no se contamine. La celda no destructiva de la tabla —lo que no borra nada procede sin preguntar— es la de `self install` y `setup`, **no** la de `cleanup`.

Las tres operaciones destructivas del producto (`cleanup`, `self uninstall` y la purga de `setup --force-update`) usan **el mismo módulo `confirm`**, para que la tabla de §8.1 tenga una sola implementación.

---

## Contrato JSON (`--json`)

```json
{
  "schema_version": "4",
  "status": "cleanup_complete",
  "reason": null,
  "removed": ["…/data/voices/mi_voz", "…/speech/mi_voz"],
  "dry_run": false
}
```

`status` toma `cleanup_complete` o `cancelled`. `removed` son las rutas del plan con lo que la operación borró, o —con `--dry-run`— lo que habría borrado. Exactamente un objeto JSON por invocación, incluso cuando falla: sin categoría con `--json` sale el objeto de error de §10 del contrato (`error` + `reason`), y nada más.

`schema_version` vale **`"4"`** (el sobre de la CLI); el protocolo del daemon sigue en `"3"` porque es otro contrato.

---

## Reparto con el resto de la superficie

| Necesidad | Comando |
|---|---|
| Borrar una locución | `speech remove --label` |
| Borrar muchas locuciones | `cleanup --synthetic-speech` |
| Borrar una voz clonada | `voice remove --name X` |
| Borrar las voces del usuario | `cleanup --voices` |
| Liberar los 4,4 GiB de los modelos base (6,8 GiB con el de clonado) y volver a descargarlos con `setup` | `cleanup --model` |
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
| Cancelación del usuario | 0 | — (`status` `cancelled`, no es un error) |
| Destino que no se pudo borrar | 0 | — (va en `failed`, no tumba la operación) |

---

## Ejemplos

```bash
ai-voice-interconnector cleanup --model               # libera los modelos; reintentable con setup
ai-voice-interconnector cleanup --all --dry-run       # lista el plan, no borra y no deja el bloqueo
ai-voice-interconnector cleanup --voices --yes        # omite la confirmación ( -y alias )
ai-voice-interconnector --json cleanup --model --dry-run
```
