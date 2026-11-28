# `cleanup`

Borrado del estado por categorías, sin tocar el programa. Es la operación que hace posible quedarse con el programa y perder los datos (o al revés): borra modelos, voces, audios sintetizados, configuración, logs y estado del daemon, y **nunca** el directorio de programa ni su integración de `PATH` — eso es `self uninstall`.

**Implementación:** el motor es el módulo `cleanup` de `avi-lifecycle` (`crates/avi-lifecycle/src/cleanup.rs`), que resuelve la lista de destinos, la confirmación, la parada del daemon y el barrido. En `src/main.rs` solo queda `handle_cleanup`, que convierte tipos y compone el envelope `--json`. `uninstall` **no** es su reverso ni su supera: `self uninstall` reutiliza el mismo planificador para el estado y añade el directorio de programa, y es el único que borra el programa.

---

## Definición CLI (parser)

`Commands::Cleanup` declara:

| Argumento | Tipo | Categoría | Descripción |
|---|---|---|---|
| `--voices` | `bool` | datos | Voces no-fábrica y, con ellas, el arrastre `speech/<voz>` excepto `default` |
| `--synthetic-speech` | `bool` | datos | La raíz `speech/` entera, `default` incluida |
| `--model` | `bool` | datos | La hoja de modelos exclusiva, entera; y, si rige una caché HF compartida, solo lo atribuible de ella |
| `--all` | `bool` | datos | Las tres categorías **más** configuración, logs (`data/logs/`, con las familias `daemon_*` y `qwen3-tts_*`), estado del daemon y la hoja de temporales (categoría `scratch`, el directorio `tmp` del contenedor de caché entero). La retención de 10 logs por familia no es una opción de `cleanup`: se aplica sola al crear cada log. **Sin recibo de instalación** (descarga manual, binario de `target/` o Cask de Homebrew, que no deja recibo) deja el estado en cero, como `self uninstall`: la raíz de datos entera (la voz de fábrica incluida, que se regenera en el primer uso), los modelos, los temporales, los contenedores vacíos y los ficheros de bloqueo |
| `--dry-run` | `bool` | — | Lista el plan sin borrar nada y **sin tomar el bloqueo** |
| `--yes`, `-y` | `bool` | — | Omite la confirmación interactiva |
| `--json` | `bool` | — | Global (`Cli::json`); con `cleanup` emite `status`, `reason`, `removed`, `planned`, `preserved`, `unreadable`, `failed`, `kept` y las claves comunes del envelope (`context`, `total_ms`, `stages`, `warnings`) |

**Gate `sin categoría → 2`.** Sin ninguna de las cuatro categorías, `cleanup` sale con `usage_error` (**2**) y **no borra nada**: la comprobación es la primera de `cleanup::run`, antes incluso de mirar privilegios o el bloqueo, de modo que una invocación mal formada no deja ni el archivo de bloqueo detrás.

---

## El planificador: una sola fuente de la lista

`cleanup::plan(roots, options)` construye la lista de destinos con sus tamaños aplicando **R1 y R3**, y la usan **las tres cosas que la necesitan**: el resumen de la confirmación, la salida de `--dry-run` y la ejecución. No puede haber divergencia entre lo que se anuncia y lo que ocurre porque no hay dos implementaciones: el plan de `--dry-run` y la lista de borrado salen de la misma llamada, como comprueba `shared_hf_cache_keeps_foreign_entries`.

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
3. **`--dry-run`**: escribe la cabecera `Limpieza · <categorías> · simulación` por stderr y el plan por **stdout**, incluido el barrido que *se haría*; después, `✓ Simulación completada · no se ha borrado nada`, y termina con exit 0 **sin abrir ninguna etapa de borrado, sin tomar el bloqueo, sin barrer y sin borrar**. Con `--json`, el envelope tiene `status: "planned"`, el plan en `planned`, `removed: []` y `context`, sin etapas. Si no hay nada que borrar, escribe solo `✓ No había nada que limpiar.`. El barrido no se ejecuta en la simulación porque lo que no puede coexistir con una simulación es modificar el disco; lo que sí hace es **anunciarlo con la misma decisión** del barrido real, de modo que lo que dice el `--dry-run` es lo que ocurriría.
4. **Bloqueo y recuperación**: bloqueo exclusivo de SO (el mismo, sobre el directorio de programa registrado, que toman `self update` y `self uninstall`; excluye a los comandos de uso, que lo toman compartido) y barrido transversal por identidad exacta: cada residuo (scratch, temporales con token, stagings de instalación, aparcados `.old-<token>-<nanos>` y registros `*.removal-failed.log` cuyo destino ya no existe) lleva el token `<pid>-<inicio>` de su dueño, y se barre si el dueño está muerto (inexistente o con otro inicio, como un PID reutilizado), se conserva sin más si vive y se conserva e informa si no se puede observar. Un registro `*.removal-failed.log` cuyo destino sigue existiendo se informa como borrado diferido sin completar. El barrido va **antes** del plan, para que el plan que el usuario ve sea el que queda después y no una lista que el barrido va a invalidar.
5. **Cabecera, plan** y **confirmación destructiva** `[s/N]`. La cabecera es `Limpieza · <categorías | todo>`, con ` · simulación` en `--dry-run`, y sale antes que el plan y la pregunta. Si no hay nada que borrar, no se pide confirmación: escribe `✓ No había nada que limpiar.` y termina. El plan sale de la misma composición que el de la simulación (`cleanup::plan_lines`): `Se borrarán N rutas, X,Y GB en total:` (los plurales concuerdan: `1 ruta`, `2 rutas`) y una línea por destino, cada uno una sola vez, con su categoría en español y su tamaño (`  modelos: <ruta> (3,4 GB)`; las categorías son `modelos`, `voces`, `audios sintetizados`, `configuración`, `registros`, `estado del daemon`, `temporales` y, solo sin recibo de instalación, `datos de usuario`), seguido de una línea `  bloqueo: <ruta>` por cada fichero de bloqueo que se borrará (solo sin recibo de instalación) y de lo que no se tocará. La pregunta es `¿Borrar lo indicado? [s/N]`.
6. **Parar el daemon** antes de borrar nada que use, como etapa `Parada del daemon`, que solo existe si había un daemon en marcha y deja un `⚠` que avisa de que hay que volver a iniciarlo. Si no se detiene → `daemon_stop_failed` (**16**) y **nada del plan se borra**.
7. **Borrado** de cada destino del plan, en el orden del plan, y después la **poda de contenedores**: el ejecutor del árbol de propiedad recorre de abajo arriba los contenedores ancestros de lo borrado y aplica `remove_dir` (nunca recursivo) a los que quedaron vacíos. Un contenedor con cualquier entrada ajena se conserva y se informa en `preserved` (una vez), y la poda de esa rama se detiene ahí. Una raíz de modelos compartida (`External`) nunca se poda. Los contenedores podados entran en `planned` tras los destinos, y en `removed` si la ejecución es real. Con recibo, `cleanup --all` no borra el contenedor raíz de la aplicación en Windows (`%LOCALAPPDATA%\ai-voice-interconnector`), porque la raíz de datos sigue en pie con los datos de fábrica; el contenedor de caché sí se poda si queda vacío. Sin recibo, `--all` borra también la raíz de datos entera y el contenedor raíz se poda si queda vacío. El directorio de programa solo entra en `preserved` si existe.
8. **Ficheros de bloqueo** (solo `--all` sin recibo de instalación): como último paso con efectos, se borran el fichero de bloqueo y la compuerta (`.ai-voice-interconnector.lock` y `.gate`, junto al directorio de programa) con el bloqueo aún tomado. Entran en `planned` en la simulación y en la ejecución, y en `removed` solo en la ejecución real, pero no cuentan en «Se borrarán N rutas» ni en el cierre «N rutas borradas». Si no hay nada más que borrar, se retiran igualmente antes de responder, sin confirmación.

**`NotFound` es éxito.** El plan dice lo que tiene que dejar de existir, y si ya no existe el objetivo está cumplido. No es un caso teórico: la parada del daemon borra el pidfile cuando no había daemon vivo, y el pidfile es un destino de `--all`.

**Una ruta que no se pudo borrar es un fallo de la operación.** El borrado sigue con el resto del plan, pero cada destino que falla marca la etapa `Limpieza de datos` con `✗` y deja bajo ella un `⚠ No se pudo borrar <ruta>: <motivo>.`; después vienen el separador y `Error: No se pudo borrar N rutas.` con `reason: cleanup_incomplete` y salida **23**. Ese destino no entra en `removed` pero sí sigue en `planned` y en la clave `failed`; lo que queda a la espera del borrado diferido (por ejemplo, un archivo en uso) va en `kept`. El `status` sigue siendo `cleanup_complete`: el `reason` es lo que indica que la limpieza quedó a medias.

**Las etapas.** Tras la cabecera, el plan y la confirmación, el comando numera sus etapas: `Parada del daemon` (solo si había un daemon en marcha) y `Limpieza de datos`. El tiempo de la pregunta no cuenta en el reloj de la etapa. El cierre —`✓ Limpieza completada en <total> · N rutas borradas`— lo escribe el motor por stderr y cuenta los destinos del plan que se borraron, la misma cifra que muestra el plan. Sin daemon en marcha solo queda `Limpieza de datos`, que no escribe línea propia: tras el plan solo sale el cierre.

**`--json` solo cambia stdout.** Cabecera, plan, etapas, avisos, separador y cierre salen por stderr igual con y sin `--json`; con `--json`, stdout lleva solo el envelope.

**Silenciar el progreso.** `AVI_NO_PROGRESS=1` silencia solo las etapas por stderr, y nada más: la cabecera, el plan, los avisos de ruta, el cierre, el envelope `--json` y los códigos de salida no cambian.

---

## Confirmación: la tabla de las reglas transversales, y solo su parte destructiva

La tabla vive en las [reglas transversales](../../specs/LIFECYCLE.md#81-reglas-transversales) del ciclo de vida.

`cleanup` es **destructiva**, y su celda es la que obliga:

| | Con terminal (TTY) | Sin terminal |
|---|---|---|
| `cleanup` | Cabecera y plan con categorías y tamaños, luego `¿Borrar lo indicado? [s/N] ` (con espacio final); si se rechaza, separador y `Cancelado; no se ha borrado nada.` | **Exige `--yes`**: sin él termina con `confirmation_required` (2) y **no borra nada** |

La aceptación es `s`/`si`/`sí`/`y`/`yes`/`dale`/`ok` (sin distinguir mayúsculas). **Cualquier otra respuesta es un no**, incluido el salto de línea, y también en el prompt no destructivo: una errata nunca debe autorizar un borrado. Responder «no» **no es un error**: es `status` `cancelled` con salida 0, porque las reglas transversales no cuentan la cancelación entre los `reason`.

El prompt va a **stderr** y la respuesta se lee de **stdin**, para que `--json` no se contamine. La celda no destructiva de la tabla —lo que no borra nada procede sin preguntar— es la de `self install` y `setup`, **no** la de `cleanup`.

Las tres operaciones destructivas del producto (`cleanup`, `self uninstall` y la purga de `setup --force-update`) usan **el mismo módulo `confirm`**, para que esa tabla tenga una sola implementación.

---

## Contrato JSON (`--json`)

Ejecución real (`--json cleanup --model --yes`):

```json
{
  "context": {
    "categories": "model",
    "dry_run": false
  },
  "failed": [],
  "kept": [],
  "planned": [
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models"
  ],
  "preserved": [],
  "reason": null,
  "removed": [
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models"
  ],
  "schema_version": "7",
  "stages": [
    {
      "end_ms": 600,
      "phase": "cleanup",
      "stage": "cleaning",
      "start_ms": 0
    }
  ],
  "status": "cleanup_complete",
  "total_ms": 600,
  "unreadable": [],
  "warnings": []
}
```

Simulación (`--json cleanup --all --dry-run`): `status` es `planned`, `removed` va vacío, el plan está solo en `planned` y no hay `stages`.

```json
{
  "context": {
    "categories": "all",
    "dry_run": true
  },
  "failed": [],
  "kept": [],
  "planned": [
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\models",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache\\tmp",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\data",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\cache",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\.ai-voice-interconnector.lock",
    "%LOCALAPPDATA%\\ai-voice-interconnector\\.ai-voice-interconnector.gate"
  ],
  "preserved": [],
  "reason": null,
  "removed": [],
  "schema_version": "7",
  "status": "planned",
  "unreadable": []
}
```

Los instantes (`total_ms`, `start_ms` y `end_ms`) son orientativos: una ejecución real lleva los milisegundos medidos.

`unreadable` lista los directorios del barrido transversal que existen y no se pudieron listar (lo que contienen queda sin recoger); en texto, la ejecución real y `--dry-run` los avisan por stderr, una línea por ruta.

`preserved` está **siempre presente** y lista, como objetos `{"path", "reason"}`, lo que el plan deja intacto a propósito: el directorio de programa (que es de `self uninstall`), los contenedores con entradas ajenas y, si rige una caché HF compartida, su raíz y su `.locks`/`xet`. Un contenedor preservado puede contener descendientes que el propio plan sí borra —en la caché compartida la raíz queda y solo se retiran los repos fijados y sus locks—, así que la lista nombra rutas que quedan, no subárboles completos. Es una lista **por operación**: lo que `cleanup` conserva puede borrarlo `self uninstall` después, y `preserved` de este envelope no dice nada de la otra operación. En texto, cada entrada se narra cuando el plan se imprime (`no se tocará <ruta>: <motivo>`); en `--dry-run` la lista es la del plan, y en la ejecución real la misma, como el resto del recibo.

`status` toma `cleanup_complete`, `planned` o `cancelled`. `planned` es el recibo de `plan()`: lo que la operación iba a borrar, y en `--dry-run` lo único que lleva rutas, porque no borra nada. `removed` son las rutas del plan que la operación borró de verdad y va vacío en `--dry-run`; el plan de la simulación y el de la ejecución real salen de la misma llamada al planificador, así que se pueden comparar y no pueden divergir. `failed` lista los destinos que no se pudieron borrar y `kept` lo que queda a la espera del borrado diferido; con `failed` no vacío la salida es 23 (`cleanup_incomplete`). Exactamente un objeto JSON por invocación, incluso cuando falla: sin categoría con `--json` sale el objeto de error de §10 del contrato (`error` + `reason`), y nada más.

Sobre `schema_version`, ver [las dos versiones de esquema](../CONTRACT.md#las-dos-versiones-de-esquema). El simulacro informa `status` `planned` sin clave `dry_run`: con `planned` la clave sobra.

---

## Reparto con el resto de la superficie

| Necesidad | Comando |
|---|---|
| Borrar un audio sintetizado | `speech remove --label` |
| Borrar muchos audios sintetizados | `cleanup --synthetic-speech` |
| Borrar una voz clonada | `voice remove --name X` |
| Borrar las voces del usuario | `cleanup --voices` |
| Liberar el espacio de los modelos y volver a descargarlos con `setup` | `cleanup --model` |
| Desinstalar el programa y su `PATH` | `self uninstall` |
| Desinstalar conservando modelos, voces y audios sintetizados | `self uninstall --keep-data` |

**`--voices` nunca borra la voz de fábrica** (`default`): va embebida en el binario y el programa sigue instalado, así que `setup` la vuelve a materializar. `voice remove` la protege con exit 2 por la misma razón. Y **`self uninstall` sí la borra**, porque al desinstalar desaparece el programa: es la diferencia de alcance entre `cleanup --all` con recibo de instalación y `self uninstall` sin `--keep-data`, y el motivo está en [`SELF.md`](SELF.md). Sin recibo de instalación, `cleanup --all` también la borra, porque ningún `self uninstall` retirará ese estado.

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
| Destino que no se pudo borrar | 23 | `cleanup_incomplete` (borró lo que pudo; el destino está en `planned` y `failed` y no en `removed`) |

---

## Ejemplos

```bash
ai-voice-interconnector cleanup --model               # libera los modelos; reintentable con setup
ai-voice-interconnector cleanup --all --dry-run       # lista el plan, no borra y no deja el bloqueo
ai-voice-interconnector cleanup --voices --yes        # omite la confirmación ( -y alias )
ai-voice-interconnector --json cleanup --model --dry-run
```

---

## Muestras de salida

Salidas reales del binario. Las duraciones y tamaños son orientativos.

En la ejecución real, la cabecera, el plan y la pregunta van por stderr; las etapas empiezan **tras la confirmación**; con un daemon en marcha son dos (`Parada del daemon` y `Limpieza de datos`) y sin él `Limpieza de datos` no deja línea propia, así que queda solo el cierre. En `--dry-run`, el plan va por **stdout** y el resto por stderr; no hay etapas.

**Con un daemon en marcha** (`cleanup --voices --yes`, en la terminal):

```
Limpieza · voces
Se borrarán 3 rutas, 12,3 KB en total:
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-daemon (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-limpia (4,1 KB)
  [1/2] ✓ Parada del daemon  0,5 s
        ⚠ El daemon estaba en marcha y se detuvo; vuelve a iniciarlo con `ai-voice-interconnector daemon start`.
  [2/2] ✓ Limpieza de datos  0,0 s
──────────────────────────────────────────────────
✓ Limpieza completada en 0,5 s · 3 rutas borradas
```

**Sin daemon** (`cleanup --voices --yes`):

```
Limpieza · voces
Se borrará 1 ruta, 4,1 KB en total:
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra (4,1 KB)
✓ Limpieza completada en 0,0 s · 1 ruta borrada
```

**Modelos** (`cleanup --model --yes`; con `--json` el envelope es el del «Contrato JSON»):

```
Limpieza · modelos
Se borrará 1 ruta, 6,7 GB en total:
  modelos: %LOCALAPPDATA%\ai-voice-interconnector\cache\models (6,7 GB)
✓ Limpieza completada en 0,6 s · 1 ruta borrada
```

**Simulación** (`cleanup --all --dry-run`; el plan sale por stdout y la cabecera y el cierre por stderr):

```
Limpieza · todo · simulación
Se borrarán 3 rutas, 6,7 GB en total:
  modelos: %LOCALAPPDATA%\ai-voice-interconnector\cache\models (6,7 GB)
  temporales: %LOCALAPPDATA%\ai-voice-interconnector\cache\tmp (0 B)
  datos de usuario: %LOCALAPPDATA%\ai-voice-interconnector\data (106,6 KB)
  bloqueo: %LOCALAPPDATA%\ai-voice-interconnector\.ai-voice-interconnector.lock
  bloqueo: %LOCALAPPDATA%\ai-voice-interconnector\.ai-voice-interconnector.gate
✓ Simulación completada · no se ha borrado nada
```

Con `--json` el envelope de la simulación es el del «Contrato JSON», con `removed: []` y el plan en `planned`.

**Cancelación en la terminal** (`cleanup --voices`, contestando `n`):

```
Limpieza · voces
Se borrarán 3 rutas, 12,3 KB en total:
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-daemon (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-limpia (4,1 KB)
¿Borrar lo indicado? [s/N] n
──────────────────────────────────────────────────
Cancelado; no se ha borrado nada.
```

**Sin terminal y sin `--yes`** (`--json cleanup --voices`, con stdin redirigido; sale con 2). stdout lleva solo el envelope de error:

```json
{
  "context": {
    "categories": "voices",
    "dry_run": false
  },
  "error": "Esta orden necesita tu confirmación, pero no hay una terminal para preguntarte.",
  "reason": "confirmation_required",
  "schema_version": "7"
}
```

y stderr lleva la cabecera, el plan, el error y su solución:

```
Limpieza · voces
Se borrarán 3 rutas, 12,3 KB en total:
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-daemon (4,1 KB)
  voces: %LOCALAPPDATA%\ai-voice-interconnector\data\voices\muestra-limpia (4,1 KB)
Error: Esta orden necesita tu confirmación, pero no hay una terminal para preguntarte.
Solución: Repite la orden con `--yes` para confirmarla: `ai-voice-interconnector --json cleanup --voices --yes`.
```

Cancelar la confirmación en la terminal escribe el separador y `Cancelado; no se ha borrado nada.` **por stderr y una sola vez**, y sale con 0; con `--json`, stdout lleva el envelope `cancelled`. Con `--json`, stderr es el mismo que sin él y stdout lleva **solo** el envelope.
