# Propuesta: rediseño del contrato de `speak` → `speech synthesize`

**Estado**: propuesta, sin implementar. Ninguna decisión de este documento está en el código.
**Alcance**: contrato público de la CLI (comandos, flags, códigos de salida, payloads `--json`) y el almacén de audio generado.
**Base**: commit `26735cd`, working tree limpio, 580 tests en verde.
**Estado de las secciones**: las secciones 1 a 3 son el diseño vigente y ya incorporan las conclusiones de una primera revisión crítica. La **sección 4 es una segunda revisión, hecha sobre las secciones 2 y 3 consolidadas**. **§4.2 está propagado y resuelto**: las subsecciones 4.2.1–4.2.5 han sido aplicadas a §2/§3. El resto de §4 (§4.1, §4.3, §4.4) sigue pendiente de decisión. La **sección 5 no es una revisión**: es una especificación que sustituye a §2.5 y rediseña el grupo `speech` por responsabilidades; está decidida y pendiente de propagar.

---

## Tabla de contenidos

- [1. Punto A — contrato implementado hoy](#1-punto-a--contrato-implementado-hoy)
  - [1.1. Superficie de comandos](#11-superficie-de-comandos)
  - [1.2. `speak` en detalle](#12-speak-en-detalle)
  - [1.3. Contratos transversales](#13-contratos-transversales)
- [2. Punto B — contrato tras el rediseño](#2-punto-b--contrato-tras-el-rediseño)
  - [2.1. Delta de superficie](#21-delta-de-superficie)
  - [2.2. `speech synthesize` en detalle](#22-speech-synthesize-en-detalle)
  - [2.3. Reglas de validación](#23-reglas-de-validación)
  - [2.4. Matrices de comportamiento](#24-matrices-de-comportamiento)
  - [2.5. El grupo `speech`](#25-el-grupo-speech)
  - [2.6. El almacén de audio generado](#26-el-almacén-de-audio-generado)
  - [2.7. Cambios en otros comandos](#27-cambios-en-otros-comandos)
  - [2.8. Contratos transversales que cambian](#28-contratos-transversales-que-cambian)
- [3. Camino de A a B](#3-camino-de-a-a-b)
  - [3.1. Principio que ordena el trabajo](#31-principio-que-ordena-el-trabajo)
  - [3.2. Movimiento 1 — endurecimiento](#32-movimiento-1--endurecimiento)
  - [3.3. Movimiento 2 — feature](#33-movimiento-2--feature)
  - [3.4. La ventana entre movimientos](#34-la-ventana-entre-movimientos)
  - [3.5. Puntos sin cerrar](#35-puntos-sin-cerrar)
- [4. Segunda revisión crítica](#4-segunda-revisión-crítica)
  - [4.1. Contradicciones internas](#41-contradicciones-internas)
  - [4.2. Vocabulario y contrato](#42-vocabulario-y-contrato)
  - [4.3. Huecos de especificación](#43-huecos-de-especificación)
  - [4.4. El plan de trabajo](#44-el-plan-de-trabajo)
  - [4.5. Impacto resumido](#45-impacto-resumido)
- [5. Rediseño del grupo `speech`](#5-rediseño-del-grupo-speech)
  - [5.1. El defecto de fondo](#51-el-defecto-de-fondo)
  - [5.2. La superficie: cinco sub-acciones](#52-la-superficie-cinco-sub-acciones)
  - [5.3. Parámetros de cada sub-acción](#53-parámetros-de-cada-sub-acción)
  - [5.4. El bucle de `synthesize --play`](#54-el-bucle-de-synthesize---play)
  - [5.5. El despacho al daemon](#55-el-despacho-al-daemon)
  - [5.6. Reglas de validación](#56-reglas-de-validación)
  - [5.7. Matrices de comportamiento](#57-matrices-de-comportamiento)
  - [5.8. Payloads `--json`](#58-payloads---json)
  - [5.9. Efecto sobre §4](#59-efecto-sobre-4)
  - [5.10. Qué toca, y qué queda abierto](#510-qué-toca-y-qué-queda-abierto)

---

## 1. Punto A — contrato implementado hoy

Todo lo de esta sección está verificado contra `src/tts_sidecar/cli.py` en `26735cd`.

### 1.1. Superficie de comandos

| Comando | Sub-acciones | Propósito |
|---|---|---|
| `speak` | — | Sintetiza voz; la reproduce, o la guarda con `--output` |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
| `devices` | — | Lista dispositivos de audio |
| `doctor` | — | Diagnósticos |
| `setup` | — | Provisión del runtime |
| `cleanup` | — | Borrado de modelo y/o voces |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon |
| `version` | — | Versión |

Todos los subcomandos salvo `daemon serve` declaran `--json`.

### 1.2. `speak` en detalle

**Flags** (`cli.py:1735-1760`):

| Flag | Corto | Requerido | Default | Descripción |
|---|---|---|---|---|
| `--text` | `-t` | **sí** | — | Texto a sintetizar |
| `--voice` | `-v` | no | `default` (resuelto aguas abajo) | Nombre de voz del registro |
| `--output` | `-o` | no | — | Ruta del WAV de salida; **si se omite, se reproduce** |
| `--compute-backend` | `-cb` | no | `auto` | `auto` / `cpu` / `cuda` / `mps` |
| `--voice-audio` | — | no | — | WAV ad-hoc para el Voice Encoder (timbre) |
| `--speech-audio` | — | no | — | WAV ad-hoc para conditioning T3 + decoder S3Gen |
| `--daemon` | — | no | `false` | Usar el daemon sin sondeo previo; falla si no responde |
| `--no-daemon` | — | no | `false` | Forzar modo directo |
| `--json` | — | no | `false` | Payload de metadatos a stdout |

**Reglas de validación vigentes**:

1. `--daemon` y `--no-daemon` son mutuamente excluyentes → exit 4 (`cli.py:267-269`). Validación manual, no `add_mutually_exclusive_group`, porque el exit 2 nativo de argparse colisionaría con `EXIT_MODEL_MISSING`.
2. `--json` **requiere** `--output` → exit 4 (`cli.py:275-281`). Razón declarada: el archivo es el canal de datos, stdout el de control.
3. `--text` no puede estar vacío ni ser solo espacios → exit 4 (`cli.py:283-285`).
4. `--text` no puede exceder `protocol.MAX_TEXT_LENGTH` (5000) → exit 4 (`cli.py:287-299`). Validado **en el cliente** antes de cualquier despacho, con el mismo exit code por ambas vías; el tope del daemon queda como defensa en profundidad, no como fuente única de la validación.

> **Nota sobre la ausencia de `--text`**: al ser `required=True` en argparse (`cli.py:1736`), omitirlo produce el **exit 2 nativo de argparse**, que colisiona con `EXIT_MODEL_MISSING` — exactamente la colisión que la regla 1 se toma el trabajo de evitar. Es un defecto del contrato actual, no del rediseño; §4.1.6 propone corregirlo.

**Matriz de comportamiento actual** (dos destinos, mutuamente excluyentes):

| Invocación | Sintetiza | Reproduce | Persiste |
|---|---|---|---|
| `speak --text T` | sí | sí | no |
| `speak --text T --output P` | sí | no | sí, en `P` (ruta elegida por el llamador) |
| `speak --text T --json` | — | — | — → **exit 4** |
| `speak --text T --output P --json` | sí | no | sí, en `P` |

**Consecuencia estructural de la regla 2**: como `--json` exige `--output` y `--output` suprime la reproducción, **ninguna invocación programática del sidecar produce sonido jamás**.

**Payload `--json`** (`cli.py:243-256`), idéntico por ambas vías:

```json
{
  "schema_version": "1",
  "output": "<ruta absoluta resuelta>",
  "voice": "<nombre>",
  "t3_time": 0.0,
  "s3gen_time": 0.0,
  "daemon": true
}
```

### 1.3. Contratos transversales

**Códigos de salida — congelados** (`cli.py:43-49`):

| Código | Constante | Significado |
|---|---|---|
| 0 | `EXIT_OK` | éxito |
| 1 | `EXIT_ERROR` | error genérico |
| 2 | `EXIT_MODEL_MISSING` | modelo no provisionado |
| 3 | `EXIT_NOT_FOUND` | voz o archivo de audio no encontrado |
| 4 | `EXIT_INVALID_INPUT` | entrada inválida (texto vacío, nombre ilegal, colisión) |
| 5 | `EXIT_DAEMON_UNREACHABLE` | daemon inalcanzable |
| 130 | `EXIT_INTERRUPTED` | interrupción del usuario |

**Dos versiones de esquema independientes**, hoy ambas en `"1"`:

- `cli.SCHEMA_VERSION` (`cli.py:55`) — forma de los payloads `--json` de la CLI. Su comentario declara la política: añadir claves no la incrementa, *«solo lo haría un cambio incompatible de las existentes»*.
- `protocol.SCHEMA_VERSION` (`protocol.py:42-45`) — forma de los mensajes IPC del daemon, con política idéntica.

**Contrato del integrador de narración** (`docs/NARRATION-INTEGRATION.md:42`): `speak --text "<msg>" --daemon` → síntesis y reproducción. La línea 49 declara esa tabla «el contrato a preservar». `--output` no aparece en ese documento.

**Sandbox del daemon**: `/synthesize` acepta `voice_audio`/`speech_audio` como rutas, validadas contra `voices.allowed_audio_dirs()` = registro de usuario + registro de fábrica + `<tempdir>/tts-sidecar/`.

---

## 2. Punto B — contrato tras el rediseño

### 2.1. Delta de superficie

| Elemento | Hoy | Propuesto |
|---|---|---|
| Nombre del comando de síntesis | `speak` | **`speech synthesize`**, sin alias de compatibilidad |
| `--output` | existe | **eliminado** |
| `--voice-audio` / `--speech-audio` | existen | **eliminados** |
| `--label` / `-l` | — | **nuevo** |
| `--no-play` / `-n` | — | **nuevo** |
| `--yes` / `-y` | — | **nuevo** (existe ya en `setup` y `cleanup`) |
| `--force` / `-f` | — | **nuevo** (existe ya en `voice clone`) |
| `--text` / `-t` | requerido | **requerido** (sin cambios) |
| `--voice`, `--compute-backend`, `--daemon`, `--no-daemon`, `--json` | existen | sin cambios |
| Sub-acciones de `speech` (`synthesize`, `list`, `remove`, `play`) | — | **nuevo**, absorbidos en el namespace `speech` (ver 2.5) |
| `speech synthesize --list` | — | **no se crea**: el listado viva en `speech list` |
| `cleanup --synthetic-speech` | — | **nuevo** (ver 2.7) |
| `--reference/-r` en `voice clone` | existe | **renombrado a `--timbre-reference/-t`** |
| `--speech/-s` en `voice clone` | existe | **renombrado a `--speech-reference/-s`** |
| Archivos de voz `reference.wav` / `speech.wav` | existen | **renombrados a `timbre-reference.wav` / `speech-reference.wav`** |
| `voice_audio` / `reference_audio` (interno) | existen | **unificados a `timbre`** |

Superficie resultante: ocho comandos de nivel superior (`speech`, `voice`, `devices`, `doctor`, `setup`, `cleanup`, `daemon`, `version`), tres de ellos grupos nominales de gestión.

### 2.2. `speech synthesize` en detalle

**Modelo mental.** `speech synthesize` **siempre genera**: no hay ningún camino en el que el subcomando no sintetice. El nombre es literalmente cierto y no necesita ningún encuadre que lo justifique. `--label` es la clave de persistencia dentro del namespace de la voz; su ausencia significa efímero. La reproducción es el efecto por defecto y `--no-play` es su opt-out.

> **Nota histórica.** La primera versión definía el comando como *«asegura que existe el habla para esta clave»* y admitía `--text` opcional, de modo que invocarlo con una etiqueta existente era un *cache hit*. Ese encuadre existía solo para justificar que el comando a veces no generase. Con la reproducción de locuciones guardadas movida a `speech play` (§2.5), el encuadre es innecesario y desapareció; con él, la contradicción de prometer un *get-or-create* que la matriz prohibía. La absorción en el namespace `speech` significa que `speech synthesize` y `speech play` comparten el mismo grupo, pero no el mismo modelo mental: uno genera, el otro reproduce.

**Flags nuevos y su rol**:

| Flag | Corto | Eje | Rol |
|---|---|---|---|
| `--label` | `-l` | identidad | Clave dentro del namespace de la voz **e interruptor de persistencia**: su ausencia significa efímero |
| `--voice` | `-v` | identidad | Namespace de la clave (rol adicional; ya seleccionaba la voz) |
| `--text` | `-t` | contenido | Fuente de síntesis; **requerido** |
| `--no-play` | `-n` | efecto | Silencia la reproducción |
| `--yes` | `-y` | interacción | Suprime el bucle interactivo sin silenciar |
| `--force` | `-f` | colisión | Permite sobrescribir una etiqueta existente |

**Los tres estados del eje interacción**, con dos flags:

| Estado | Cómo se pide |
|---|---|
| Reproduce y pregunta | default, con `--label` |
| Reproduce y no pregunta | `--yes` |
| Ni reproduce ni pregunta | `--no-play` |

El cuarto estado —«no reproduce pero pregunta»— es imposible: el bucle de tres opciones exige haber oído el audio para elegir entre repetir, aceptar y regenerar. De ahí que **`--no-play` implique `--yes`**; no es una decisión de diseño sino una consecuencia.

**El bucle interactivo** (repetir / aceptar y persistir / regenerar) se activa con `--label`, sin `--yes` ni `--no-play`. **No comprueba si hay TTY**: arranca siempre, y si `input()` levanta `EOFError` —stdin cerrado, invocación vía subprocess— la invocación se trata como **cancelación**: no persiste, mensaje informativo a stderr, **exit 0**. El molde es `cmd_cleanup` (`cli.py:1591-1600`), que resuelve el mismo problema sin depender del entorno.

Repetir cuesta cero síntesis (los bytes están en memoria); regenerar paga T3+S3Gen pero **nada** de la Etapa 1, porque los conditionals de una voz del registro están precomputados desde `26735cd`.

**Dos divergencias de vocabulario, asumidas a conciencia**:

- `--yes` aquí significa «no me muestres el bucle», no «confirmo una acción destructiva» como en `setup` y `cleanup`. La diferencia se **declara explícitamente en el texto del help** en vez de introducir un flag nuevo: la alternativa evaluada, `--accept` / `--accept-first`, nombraría mejor lo que hace pero añade vocabulario sin precedente en el repo.
- `-n` es `--no-play` aquí y `--name` en `voice clone` / `voice remove`. Se acepta porque el corto sigue a su flag largo dentro de cada subcomando y `speech synthesize` no tiene ningún `--name`; el precio es que `-n` no tiene un significado único en toda la CLI.
- `-t` es `--text` en `speech synthesize` y `--timbre-reference` en `voice clone`. No es una divergencia: es el mismo patrón del ítem anterior, aplicado a un par de flags que, al renombrarse en el movimiento 1, comparten carácter sin solaparse —cada corto vive en su subcomando y `voice clone` no declara `--text`.

### 2.3. Reglas de validación

Las cuatro vigentes se conservan, con una modificación:

1. `--daemon` XOR `--no-daemon` → exit 4. **Sin cambios.**
2. `--json` con `--label` requiere `--yes` **o** `--no-play` → exit 4. *(Sustituye a «`--json` requiere `--output`».)* Razón: el bucle interactivo contaminaría stdout. Precedente literal: `cmd_cleanup` (`cli.py:1533-1539`).
3. `--text` requerido y no vacío ni solo espacios → exit 4. **Sin cambios.**
4. `--text` no excede `MAX_TEXT_LENGTH` → exit 4. **Sin cambios** (heredada de §1.2, regla 4).

Y se añaden **dos dependencias entre flags**, ambas de la misma clase: un flag cuyo objeto es la etiqueta no puede usarse sin etiqueta.

5. **`--no-play` requiere `--label`** → exit 4 en caso contrario. Sin etiqueta no hay nada que persistir, así que silenciar la reproducción dejaría la invocación con cero efecto observable: la regla exige que haya **algo generado que persistir**.
6. **`--force` requiere `--label`** → exit 4 en caso contrario. Sin etiqueta no hay colisión posible que forzar.

> La regla 5 era, en la primera versión, «`--no-play` es válido si y solo si están presentes `--text` *y* `--label`». Con `--text` requerido de nuevo, la mitad de la condición es redundante y la regla se simplifica a la mitad restante. La regla 6 se añadió por simetría con la 5: la alternativa era ignorar `--force` en silencio, y un flag sin efecto que nadie avisa se reporta como bug.

La regla informal «toda invocación debe producir sonido o persistencia» no necesita codificarse: con la reproducción por defecto y `--text` requerido, los defaults la garantizan estructuralmente y solo quedan estas dos dependencias explícitas.

> **Herencia del exit 2 de argparse**: tal como está especificado aquí, omitir `--text` sigue produciendo el exit 2 nativo de argparse en vez de un 4 (ver la nota de §1.2). No es solo herencia: el rediseño **añade instancias nuevas** del mismo defecto, porque `speech play` y `speech remove` declaran `--label` requerido (§2.5). §4.1.6 propone cerrarlo.

### 2.4. Matrices de comportamiento

**`speech synthesize`**:

| Invocación | Genera | Reproduce | Persiste | Exit |
|---|---|---|---|---|
| `-t T` | sí | sí | no | 0 |
| `-t T -l L` *(L libre)* | sí | sí, en el bucle | al aceptar | 0 |
| `-t T -l L` *(L libre, stdin cerrado)* | sí | sí | no: cancelación | 0 |
| `-t T -l L -y` | sí | sí, una vez | sí | 0 |
| `-t T -l L -n` | sí | no | sí | 0 |
| `-t T -l L -f` *(L existe)* | sí | sí, en el bucle | al aceptar, sobrescribe | 0 |
| `-t T -l L` *(L existe, sin `-f`)* | — | — | — | **4** |
| `-t T -n` *(sin `-l`)* | — | — | — | **4** |
| `-t T -f` *(sin `-l`)* | — | — | — | **4** |

Fila 1 = el contrato del integrador de narración, intacto salvo el nombre.
Fila 3 = el camino de CI sin `--no-play`: se genera y suena, pero no se persiste nada, y el exit 0 lo distingue de un fallo.
Fila 5 = generación headless de primera clase.
Fila 7 = misma semántica que `voice clone` ante una voz existente.

**Grupo `speech`** (ver 2.5):

| Invocación | Genera | Reproduce | Exit |
|---|---|---|---|
| `speech list` *(todas las voces)* | no | no | 0 |
| `speech list -v V` *(filtrado)* | no | no | 0 |
| `speech play -l L` *(L existe)* | no | sí | 0 |
| `speech remove -l L` *(L existe)* | no | no | 0 |
| `speech play -l L` / `speech remove -l L` *(L no existe)* | — | — | **3** |
| cualquier sub-acción con etiqueta ilegal | — | — | **4** |

Ninguna combinación de flags queda ignorada en silencio en ninguna de las dos matrices: o tiene efecto declarado, o es exit 3 o 4.

### 2.5. El grupo `speech`

El almacén etiquetado es un recurso, y el repo ya tiene una gramática para gestionar recursos: un grupo nominal con sub-acciones, como `voice`. La absorción de `speak` en el namespace `speech` crea su homólogo directo: el grupo `speech` con sub-acciones `synthesize`, `list`, `play` y `remove`.

| Registro de voces | Almacén de habla sintética |
|---|---|
| `voice list` | `speech list` |
| `voice clone` | `speech synthesize` |
| `voice remove` | `speech remove` |
| — | `speech play` |

**Sub-acciones y flags**:

| Sub-acción | Flags | Efecto |
|---|---|---|
| `speech synthesize` | `--text/-t` (requerido), `--label/-l`, `--voice/-v`, `--no-play/-n`, `--yes/-y`, `--force/-f`, `--json` | Genera y reproduce (o persiste con `--label`) |
| `speech list` | `--voice/-v` (opcional, filtro), `--json` | Lista las etiquetas con su texto y fecha |
| `speech play` | `--label/-l` (requerido), `--voice/-v`, `--json` | Reproduce una locución guardada |
| `speech remove` | `--label/-l` (requerido), `--voice/-v`, `--json` | Borra una locución y su sidecar |

**Decisiones que arrastra el grupo**:

- **El identificador es `--label/-l`, no `--name/-n`.** Por homología con `voice` correspondería `--name`, pero en `speech synthesize` `--name` sería ambiguo frente a `--voice` («¿nombre de qué?»). Se acepta la divergencia con `voice --name` a cambio de que el mismo concepto no tenga dos nombres en dos comandos.
- **El namespace es obligatorio en la gestión.** Las etiquetas viven bajo una voz, así que `play` y `remove` toman `--voice` con el mismo default `default` que `speech synthesize`; `list` lo admite como filtro y sin él recorre todas las voces. Es un segmento más que en `voice remove --name X`, inevitable dado el layout de §2.6.
- **`speech play` no necesita modelo ni daemon**: lee el WAV del almacén y lo reproduce con `AudioPlayer.play_file` (`audio.py:77-81`), que hoy existe y no tiene ningún llamador en producción. La feature consume código muerto en vez de escribir nuevo.
- **Etiqueta inexistente → exit 3**, no 4: la invocación está bien formada y el recurso no está, que es exactamente `EXIT_NOT_FOUND` = *«voz o archivo de audio no encontrado»* (`cli.py:46`). Mapearla a 4 rompería la distinción que el contrato congelado existe para preservar.
- **Las sub-acciones declaran `--json`, y es obligatorio.** `tests/test_cli.py::TestJSONContractStructure` recorre el parser real para descubrir qué subcomandos lo declaran (ver el docstring de `top_level_subparsers`, `cli.py:1704-1716`); una sub-acción nueva sin `--json` **hará fallar ese test**. No es un problema: es el contrato funcionando.
- **Reparto con `cleanup`**: `speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que ya existe entre `voice remove` y `cleanup --voices`. No hay redundancia nueva.

### 2.6. El almacén de audio generado

**Ubicación**: `data_root()/synthetic-speech/<voz>/<etiqueta>.wav`, **raíz hermana** de `voices/`.

**Un solo nombre para un solo concepto.** El directorio se llama `synthetic-speech/`, el qualifier del namespace `speech` es `synthetic-speech` y el flag de borrado masivo es `cleanup --synthetic-speech`. La primera versión usaba `generated-speech/` para el directorio y `--clips` para el flag, dando tres nombres al mismo recurso; la segunda unificó los tres en `speech`, que resultó ser homónimo de la entrada de referencia de una voz (§4.2.1). `synthetic` es el qualifier definitivo: un identificador único para las tres capas, y marca la dirección del flujo de datos —lo que el sistema produce, frente a lo que el usuario aporta—, que es exactamente lo que la homonimia destruía. En prosa española la unidad se llama **locución**, el término que ya usa `docs/NARRATION-INTEGRATION.md:42`, y nunca aparece como identificador.

**Por qué no anidado en `voices/<voz>/synthetic-speech/`** (que sería la opción intuitiva y ahorraría código de borrado): `default` es una voz de **fábrica**, en un directorio empaquetado de solo lectura. Sus locuciones tendrían que ir a un espejo en el registro de usuario: un directorio con `synthetic-speech/` pero sin `timbre-reference.wav` ni `speech-reference.wav`. Ese directorio sería invisible para `list_voices` (`voices.py:156`) e indeleble por `voice remove`, porque `_is_valid_voice_dir` es el guard que protege el `rmtree` y exige ambos WAV (`voices.py:170`). Coste aceptado de la raíz separada: el arrastre de las locuciones al borrar una voz deja de ser gratis y exige código explícito.

**Sidecar de metadatos**: junto a cada `<etiqueta>.wav` se escribe `<etiqueta>.json` con tres campos —`text`, `voice`, `created_at`—. Sin él las etiquetas son opacas: pasadas unas semanas, `saludo2` no le dice nada a nadie. `speech list` muestra el texto **truncado** en la salida humana y **completo** en el payload `--json`.

**Orden de escritura y atomicidad**: cada archivo se escribe a un temporal en el mismo directorio y se publica con `os.replace`, de modo que una interrupción no deja un WAV truncado que `speech list` mostraría como válido y `speech play` intentaría reproducir. El sidecar se publica **antes** del WAV, así que la aparencia del `.wav` implica que sus metadatos ya están completos. `speech list` enumera los `.wav` y tolera un sidecar ausente mostrando la locución sin metadatos, en vez de fallar.

**Validación de etiquetas**: misma clase de problema que los nombres de voz. Conviene generalizar `_validate_voice_name` (`voices.py:28-44`) a validador de segmento de ruta parametrizado (`_validate_path_segment(value, kind="voz" | "etiqueta")`) y que voz y etiqueta la invoquen, en vez de duplicar la regex. El parámetro `kind` determina el sustantivo en el mensaje de error —«Nombre de **voz** inválido» vs. «Nombre de **etiqueta** inválido»—, de modo que `speech synthesize --label "mi saludo"` no culpe a `--voice`. La defensa anti-escape por `realpath` de `voice_dir` (`voices.py:89-98`) debe correr sobre **ambos** segmentos.

> **Las etiquetas se normalizan a minúsculas**, porque `_validate_voice_name` lo hace deliberadamente para evitar colisiones en filesystems case-insensitive (`voices.py:44`). Es decir: `--label Saludo` y `--label saludo` son la misma etiqueta, y el archivo se llama `saludo.wav`. Hay que declararlo en el help de `--label` y en `USAGE.md`, o un usuario que escriba `--label Bienvenida` y no la encuentre listada así lo reportará como bug.

**El almacén NO se añade a `allowed_audio_dirs()`**: es salida de síntesis, escrita y leída solo por el cliente. El daemon jamás lo toca. (La función además desaparece entera en el paso 1.6.)

### 2.7. Cambios en otros comandos

**`cleanup`** gana un flag y dos comportamientos declarados:

- `--synthetic-speech`: `rmtree` de la raíz `synthetic-speech/`.
- `--voices`: además de borrar voces, **arrastra sus locuciones**. Con la raíz separada esto requiere código explícito.
- `--all` = modelo + voces + **habla sintética**. Declararlo es necesario: si `--all` no la incluyera dejaría residuo tras una desinstalación completa, que es justo lo que ese flag existe para evitar. Requiere un test propio.
- `--dry-run` cubre las locuciones, en los tres modos anteriores.

**`setup`** conserva la degradación FAIL → WARN del chequeo de audio, pero con **la premisa reescrita**. Hoy el comentario dice: *«la síntesis a archivo con `speak --output` funciona sin subsistema de sonido, p. ej. en hosts headless/SSH»* (`cli.py:1331-1335`). Esa frase muere con `--output`, pero el sumidero reaparece: `speech synthesize --text T --label L --no-play` persiste en ruta computada por el sistema. `setup` sigue siendo provisión, no diagnóstico, y el sidecar sigue instalable en headless/SSH/CI. Hay que reescribir el comentario, el mensaje al usuario (`cli.py:1363-1364`) y el aserto de `tests/test_cli.py:925`.

**`voice`**: el único cambio es el renombrado de los flags y archivos de `voice clone` para resolver la homonimia de `speech` (§4.2.1). Los flags `--reference/-r` y `--speech/-s` pasan a llamarse `--timbre-reference/-t` y `--speech-reference/-s`; los archivos `reference.wav` y `speech.wav` pasan a llamarse `timbre-reference.wav` y `speech-reference.wav`. Internamente, `voice_audio`/`reference_audio` se unifican bajo el nombre `timbre`. El rename se paga una sola vez en el movimiento 1, sin coste de migración de datos —sólo reempaquetado de voces de fábrica y actualización de `_is_valid_voice_dir` (`voices.py:113-126`) para que chequee los nuevos nombres.

**`devices`, `doctor`, `daemon` y `version`**: sin cambios especificados hasta ahora. No es una garantía de que no deban cambiar, sino el estado del rediseño en este punto: nada de lo analizado hasta aquí exige tocarlos.

### 2.8. Contratos transversales que cambian

**Los siete códigos de salida siguen congelados.** El rediseño no añade ninguno:

| Situación | Código |
|---|---|
| Aceptar en el bucle; rechazar; cancelar; `EOFError` por stdin cerrado | 0 |
| Violación de cualquiera de las reglas 1-6 de §2.3 | 4 |
| Etiqueta con caracteres ilegales | 4 (misma clase que «nombre ilegal») |
| Colisión de etiqueta sin `--force` | 4 (misma clase que «colisión») |
| Etiqueta inexistente en `speech play` / `speech remove` | **3** |
| Ctrl-C durante el bucle | 130 |
| Fallo del daemon al generar o regenerar | 5 |

**Las dos versiones de esquema suben a `"2"`, por razones independientes**:

- `protocol.SCHEMA_VERSION`: el refactor de `/synthesize` a `voice: str` **quita** los campos `voice_audio`/`speech_audio` de `SynthesizeRequest`. Quitar campos existentes no es aditivo (`protocol.py:42-45`).
- `cli.SCHEMA_VERSION`: el payload de `speak` pierde la clave `"output"`. Por la política declarada en `cli.py:51-54`, eso es un cambio incompatible de una clave existente.

Son dos bumps, no uno; conviene no confundirlos al implementar. Los payloads **nuevos** del grupo `speech` no influyen: añadir subcomandos es aditivo.

**Payloads `--json` propuestos**:

- `speech synthesize`: métricas puras (`voice`, `t3_time`, `s3gen_time`, `daemon`), más `label` cuando se pasó `--label`. No hace falta un campo de persistencia ni de cancelación: bajo `--json` la regla 2 exige `--yes` o `--no-play`, así que el bucle es inalcanzable y la persistencia es cierta — misma propiedad que hace inocuo el `print()` a stdout de `cmd_cleanup`.
- `speech list`: `{"synthetic_speech": [{"voice", "label", "text", "created_at"}]}`, con el texto completo. La clave es el nombre del recurso en snake_case, siguiendo el precedente de `voice list --json`, que emite `{"voices": [...]}` (`cli.py:565`).
- `speech play`: `{"voice", "label"}`.
- `speech remove`: `{"voice", "label"}`. El campo `removed` se elimina: el exit code ya transporta la información (0 = se borró, 3 = no existía).

Si conviene o no exponer también la ruta computada de la locución es un punto abierto (ver 3.5).

**Contrato del integrador**: `docs/NARRATION-INTEGRATION.md:42` pasa de `speak --text "<msg>" --daemon` a `speech synthesize --text "<msg>" --daemon`. Cambia el nombre y gana un nivel de anidamiento; la reproducción sigue siendo el comportamiento por defecto. La ruptura es deliberada y hay que actualizar el documento en el mismo movimiento que la absorción. Se descartó un alias de compatibilidad.

**Sandbox del daemon**: `allowed_audio_dirs()` desaparece por completo, porque `/synthesize` deja de aceptar rutas. La superficie de ataque «leer un `.wav` de una ruta elegida por el llamador» se cierra en el protocolo, no en la validación.

---

## 3. Camino de A a B

### 3.1. Principio que ordena el trabajo

El invariante que sostiene todas las eliminaciones: **el sistema no debe poder leer ni escribir `.wav` en rutas elegidas por el llamador.** `--output` lo viola en escritura; `--voice-audio`/`--speech-audio`, en lectura; `daemon_session_dir` abriría una tercera puerta; y la rama de degradación silenciosa convierte la restricción del daemon en algo que se elude por defecto, sin ningún flag, simplemente pasando una ruta que la sandbox rechaza.

El almacén etiquetado **no** viola el invariante: la ruta la computa el sistema a partir de `(voz, etiqueta)`, no la elige el llamador.

**El trabajo son dos movimientos secuenciales, y el orden no es invertible.** La limpieza es precondición de la feature, no una tarea ortogonal: `--label` no puede diseñarse mientras exista `--output` (dos mecanismos de persistencia compitiendo, y `--label --output` sin semántica definida), y el bucle es barato solo con voces del registro — con rutas ad-hoc, `synthesis.py:62-65` busca `conditionals.pt` en `dirname(speech_audio)`, donde nunca existe, y la caché falla estructuralmente en cada iteración.

### 3.2. Movimiento 1 — endurecimiento

Solo endurecimiento: eliminaciones, refactors y renombres. Sin feature nueva.

**Paso 1.1 — eliminar `--output` y su cascada.** Es el de mayor radio. Arrastra, dentro de `cli.py`: la rama de archivo de `_emit_audio` (`113-120`), el gate `--json` sin `--output` (`275-281`), la clave `"output"` del payload (`251`), la rama `if args.output:` del modo directo (`394-398`) y `output_path=args.output` (`388`). Y fuera de `cli.py`, porque `cli.py:388` es el **único productor de `output_path` en todo el proyecto**: el parámetro de `engine.speak` (`engine.py:449`), la Etapa 4 de `_synthesize_impl` (`synthesis.py:101-105`), la forma de tres argumentos de `AudioWriter.write` (`audio_writer.py:20-26`) y, con ella, `paths.ensure_parent_dir` (`paths.py:34-43`).

> **Verificación**: `grep -rn "output_path\|ensure_parent_dir" src/` sin coincidencias.

**Paso 1.2 — eliminar `--voice-audio` / `--speech-audio` y la maquinaria que solo ellos justifican.** `_resolve_voice_paths` (`cli.py:86-108`) colapsa a `voices.voice_paths(args.voice or "default")`. Caen `_check_audio_paths_present` (`155-179`), `_paths_allowed_by_daemon` (`129-152`, que sin rutas ad-hoc es una tautología) y el mensaje de tres alternativas (`344-353`).

**Paso 1.3 — eliminar la rama de degradación silenciosa** (`cli.py:367-371`). Es la eliminación de seguridad del movimiento. `--no-daemon` **sobrevive**, pero por otro motivo: es la vía documentada para forzar un compute backend distinto al que el daemon fijó al arrancar (`USAGE.md:490-494`). Un opt-out explícito del usuario es categóricamente distinto de una degradación automática.

**Paso 1.4 — eliminar `daemon_session_dir` / `ensure_daemon_session_dir`** (`voices.py:57-73`). `ensure_*` tiene cero consumidores en producción; `daemon_session_dir` tiene uno solo, `allowed_audio_dirs()`. La mitad servidor de esa frontera de confianza está construida y la mitad cliente que la poblaría nunca se escribió. Terminarla implementaría exactamente el anti-requisito: una segunda puerta para sintetizar desde audio fuera del registro.

**Paso 1.5 — eliminar las fachadas `engine.remove_voice` y `engine.resolve_voice`.** Residuo; los llamadores reales usan `voices.py` directo (`cli.py:443,517,522,562`).

**Paso 1.6 — refactorizar `/synthesize` a `voice: str` y subir `protocol.SCHEMA_VERSION` a `"2"`.** El patrón **ya existe en el mismo archivo**: `PrecomputeVoiceRequest` (`protocol.py:129-136`) lleva solo `name: str` y su docstring enuncia el razonamiento. Esto no es un diseño nuevo, es alinear `/synthesize` con lo que `/voices/precompute` estableció. Borra los campos `voice_audio`/`speech_audio`, `MAX_AUDIO_PATH_LENGTH` (`protocol.py:28`), `_validate_audio_path` completo (`server.py:111-146`, 36 líneas), el bloque `allowed_dirs`/`real_paths` (`server.py:179-189`) y `voices.allowed_audio_dirs()` entero.

> **Riesgo conocido**: `data_root()` depende de `LOCALAPPDATA` / `XDG_DATA_HOME`. Si daemon y cliente arrancaron con entornos distintos, hoy falla con un 400 visible; después fallará con un 404 «Voz no encontrada» para una voz que el cliente sí lista. Es una regresión de diagnosticabilidad, atenuada porque `/voices/precompute` ya carga hoy ese mismo riesgo y `/voices` (`ipc.py:212-235`) permite inspeccionar la vista del daemon.

**Paso 1.7 — absorber `speak` en el namespace `speech` como `speech synthesize`, sin alias**, y subir `cli.SCHEMA_VERSION` a `"2"`.

**Paso 1.8 — reescribir la premisa del WARN de `setup`** (`cli.py:1331-1335`, `1363-1364`). Ver la dependencia cruzada de §3.4 antes de ejecutarlo.

**Paso 1.9 — estrechar `__all__`** (`__init__.py:30`) y eliminar el parámetro `verbose`, que hoy existe en la firma y no se lee en ninguna parte. La API Python no es superficie soportada; el contrato público es solo la CLI.

**Paso 1.10 — renombrar flags y archivos de `voice clone`, y unificar internamente.** Los flags `--reference/-r` y `--speech/-s` pasan a `--timbre-reference/-t` y `--speech-reference/-s`; los archivos `reference.wav` y `speech.wav` pasan a `timbre-reference.wav` y `speech-reference.wav`. Internamente, `voice_audio`/`reference_audio` se unifican bajo el nombre `timbre`. El rename resuelve la homonimia de `speech` en las tres capas (CLI, filesystem, interno) de una sola vez (§4.2.1). No hay migración de datos: las voces de fábrica se reempaquetan y `_is_valid_voice_dir` (`voices.py:113-126`) se actualiza para chequear los nuevos nombres. El rename es puro movimiento 1: no hay feature nueva, y su verificación es un test que liste una voz de fábrica y confirme que ambos WAV llevan los nuevos nombres.

**Tests que caen**: `test_allowed_audio_dirs_returns_three_dirs` (`test_daemon_sandbox.py:48-55`), `test_daemon_session_dir_is_namespaced` (65-72), `test_ensure_daemon_session_dir_matches` (74-79), `test_accepts_wav_in_namespaced_session_dir` (`test_daemon.py:347-373`), `TestValidateAudioPath` completa (`test_daemon_sandbox.py:82-194`, seis tests), y en `test_protocol.py` los asertos de campo (26-33, 51, 56-66), el bloque de `MAX_AUDIO_PATH_LENGTH` (169-179) y la clase de independencia de campos (184-219).

**Tests que sobreviven y se refuerzan**: `test_allowed_audio_dirs_excludes_general_tempdir` (`test_daemon_sandbox.py:57-63`) y `test_rejects_wav_in_general_tempdir` (`test_daemon.py:323-345`).

**Sobre las 18 ocurrencias de `--output` en `test_cli.py`**: solo tres son sustantivas (`TestSpeakJSON`, `test_cmd_speak_saves_with_output`, `TestEmitAudioCreatesParentDirs`). El resto son defaults de `_args` que pasan `output=` únicamente para que `speak` no reproduzca durante el test; **se simplifican**, no se complican, al pasar a mockear `AudioPlayer`.

**Documentación del movimiento 1**: `USAGE.md:463-532`, `docs/DESIGN.md:178-179`, `docs/GOAL.md:126`, `docs/DISTRIBUTION.md:93`, `docs/DAEMON-MODE.md:316-335` (se reescribe entera) y `docs/NARRATION-INTEGRATION.md:42,49`. `SECURITY.md:61-67` **sobrevive verbatim**: no nombra el directorio de sesión.

> **Verificación del movimiento**: suite completa en verde, y `grep -rn "voice_audio\|speech_audio\|allowed_audio_dirs\|daemon_session_dir" src/` sin coincidencias.

### 3.3. Movimiento 2 — feature

La mayor parte de la feature ya está cubierta por código que el movimiento 1 conserva. Lo genuinamente nuevo son tres bloques: el módulo del almacén (~100 líneas), el cuerpo del bucle (~30) y el grupo `speech` (~60, tres sub-acciones que reutilizan el módulo).

**Paso 2.1 — módulo del almacén**, calcado de `voices.py`: validación de etiqueta reutilizando el validador de segmento generalizado con `kind="etiqueta"`, composición de ruta, defensa anti-escape sobre los dos segmentos, escritura atómica del WAV y del sidecar (temporal + `os.replace`, sidecar primero), lectura de metadatos tolerante a sidecar ausente, listado y borrado.

**Paso 2.2 — desacoplar la síntesis de la emisión.** Único obstáculo técnico del bucle, y es de una línea: `_synthesize_via_daemon` invoca `_emit_audio(...)` **dentro de sí misma** (`cli.py:227`) y luego devuelve el resultado. El bucle necesita lo contrario: sintetizar, devolver, y que el llamador decida cuándo reproducir. Esa línea ya desaparece en el paso 1.1 al quedarse sin segundo argumento.

**Paso 2.3 — parser de `speech synthesize`**: `--label/-l`, `--no-play/-n`, `--yes/-y`, `--force/-f`; `--text` **sigue siendo requerido**; las seis reglas de validación de §2.3; la nota sobre el significado de `--yes` en el texto del help y la de la normalización a minúsculas en el de `--label`.

**Paso 2.4 — cuerpo del bucle** en `cmd_generate_speech`, copiando el molde de `cmd_cleanup`: separación de canales `info_out = sys.stderr if json_mode else sys.stdout` (`cli.py:1531`), `input()` envuelto en `try/except EOFError` (`1591-1600`) tratado como cancelación, y la cancelación modelada como campo del resultado con exit 0 (`1499-1511`). **Sin ninguna comprobación de TTY.**

> **Matiz a no copiar literalmente**: la rama de cancelación de `cleanup` imprime con `print()` a stdout (`cli.py:1596,1599`). Ahí es inocuo porque el gate lo hace inalcanzable bajo `--json`; el bucle debe replicar la *estructura*, no esa línea.

**Paso 2.5 — grupo `speech`**: parser del grupo y las sub-acciones `synthesize` / `list` / `play` / `remove`, todas con `--json` (o `TestJSONContractStructure` falla), sobre el módulo del paso 2.1. `play` usa `AudioPlayer.play_file` (`audio.py:77-81`). Etiqueta inexistente → `EXIT_NOT_FOUND`.

**Paso 2.6 — `cleanup --synthetic-speech`**, el arrastre explícito de las locuciones en `cleanup --voices`, su inclusión en `--all` y la cobertura de `--dry-run`, cada cosa con su test.

**Paso 2.7 — documentación**: `USAGE.md` (`speech synthesize` completo con su matriz, el grupo `speech`, la normalización a minúsculas y los flags nuevos de `cleanup`) y `docs/NARRATION-INTEGRATION.md` si conviene mencionar el modo etiquetado.

> **Verificación del movimiento**: cada fila de las dos matrices de §2.4 tiene un test, incluidas las tres filas de exit 4 de `speech synthesize` y las de exit 3 y 4 del grupo `speech`.

### 3.4. La ventana entre movimientos

Hay una dependencia cruzada que conviene resolver explícitamente en el plan de implementación, no descubrirla al ejecutar.

El WARN de `setup` (paso 1.8) sobrevive apoyado en un sumidero —`--label --no-play`— que **solo existe tras el movimiento 2**. Entre ambos movimientos hay una ventana en la que la premisa reescrita es literalmente falsa: `--output` ya no está y `--label` todavía no.

Tres salidas posibles, a decidir al planificar:

1. Escribir el comentario en el movimiento 1 anticipando el 2, aceptando que es cierto solo al final.
2. Mover el paso 1.8 al movimiento 2.
3. Dejar el WARN sin tocar durante el movimiento 1 y corregirlo en el 2, asumiendo una premisa obsoleta transitoria.

La opción 2 parece la más honesta: el comentario describe una capacidad, y la capacidad llega en el movimiento 2.

### 3.5. Puntos sin cerrar

1. **¿El payload `--json` expone la ruta computada de la locución?** Emitirla es útil para un orquestador y no viola el invariante (la ruta la computa el sistema). Pero acopla al integrador a una ruta del filesystem, que es justo el acoplamiento que el rediseño quiere disolver. Alternativa, y la que asumen los payloads de §2.8: emitir solo `label` y `voice`.
2. **Resolución de la ventana de §3.4** (opciones 1, 2 o 3).

> El punto que ocupaba el primer lugar en la primera versión —el nombre del flag de silenciado— quedó cerrado: es `--no-play/-n`.
>
> La revisión de §4 añade decisiones pendientes que no están en esta lista porque son propuestas, no puntos que el diseño dejara abiertos a conciencia. Ver §4.5.

---

## 4. Segunda revisión crítica

Esta sección es una revisión de las secciones 2 y 3 **ya consolidadas**, hecha después de incorporarles las conclusiones de una primera revisión. Cada apartado declara el problema, la evidencia, el cambio propuesto y qué habría que tocar.

**§4.2 está propagado y resuelto**: las subsecciones 4.2.1–4.2.5 han sido aplicadas a §2/§3. El resto de §4 (§4.1, §4.3, §4.4) sigue pendiente: son decisiones pendientes sobre §2/§3, que definen el diseño vigente. §4 no propaga; registra lo que habría que cambiar y por qué.

Todas las referencias a código están verificadas contra `26735cd`.

> **Numeración de códigos de salida en esta sección.** Con §4.1.6 aprobada, los códigos que cita §4 son los de su tabla: **2** uso incorrecto, **3** no encontrado, **4** modelo no provisionado, **6** conflicto de estado, **7** operación no aplicable, **8** precondición de entorno incumplida. §2, §3 y el código siguen llevando los anteriores —2 y 4 intercambiados, sin 6/7/8— hasta que §4.1.6 se propague. La divergencia es deliberada y temporal: donde §4 diga «2» y §2.4 diga «4», manda §4.

El diagnóstico general: la consolidación resolvió las contradicciones de la primera versión y creó una clase nueva de problemas, más pequeños pero del mismo tipo. Dos patrones se repiten:

1. **Afirmaciones absolutas que el propio diseño no cumple.** §2.4 declara que ninguna combinación de flags queda ignorada en silencio; §2.8 declara que las violaciones de las reglas 1-6 salen con 4. Ninguna de las dos es cierta tal como está escrito (§4.1.2, §4.1.3).
2. **Decisiones de vocabulario tomadas sin inventariar el vocabulario existente.** El caso grave es `speech`, que ya nombra otra cosa en este proyecto (§4.2.1). La homonimia cubre tres capas: CLI, filesystem e interno —las tres resueltas en una sola decisión de §4.2.1—.
3. **Defectos declarados en vez de resueltos.** El documento detectaba anomalías y las anotaba con una frase que las excluía —«este documento no la corrige», «nada más cambia»—, cuando su propósito es definir todas las modificaciones necesarias. La colisión del exit 2 de argparse es el caso claro: advertida dos veces, cerrada ninguna, y el rediseño la propaga a dos comandos nuevos (§4.1.6).

Ninguno de los hallazgos invalida el rediseño. El de §4.2.1 sí obliga a rehacer una decisión de nombres antes de escribir una línea de código, porque después es caro.

### 4.1. Contradicciones internas

#### 4.1.1. El bucle interactivo no tiene salida sin persistir, y la fila de exit 0 de §2.8 nombra cuatro estados donde hay dos *(✅ estrategia aprobada)*

**Problema.** §2.2 define el bucle con tres opciones: **repetir / aceptar y persistir / regenerar**. La tabla de códigos de salida de §2.8 lista, en cambio, cuatro estados terminales para el exit 0: *«Aceptar en el bucle; rechazar; cancelar; `EOFError` por stdin cerrado»*. Dos de los cuatro no se sostienen:

- **«rechazar»** no corresponde a ninguna opción del bucle;
- **«cancelar»** y **«`EOFError` por stdin cerrado»** son el mismo estado, no dos: §2.2 define literalmente el EOF *como* cancelación, así que la fila lo nombra por la causa y por el efecto y los cuenta como si fueran independientes.

Lo de «rechazar» no es solo una palabra de más: el usuario que escucha una toma mala, regenera, escucha otra igual de mala y decide abandonar **no tiene ninguna salida limpia**. Sus opciones reales son seguir regenerando indefinidamente o Ctrl-C, que sale con **130** — un código que un orquestador lee como «el usuario interrumpió», no como «el usuario decidió no guardar nada».

Es la misma asimetría que el diseño evitó en `cleanup`, donde responder «n» es una salida de primera clase con exit 0 (`cli.py:1598-1600`).

**Propuesta.** Añadir una cuarta opción al bucle —**descartar y salir**— con exit 0 y sin persistencia, y sanear con ella la fila entera: la etiqueta huérfana pasa a nombrarla, y «cancelar» desaparece por redundante con el `EOFError` que la produce. El molde del modelado ya está escrito: es la rama `if response not in ("s", "si", "sí", "y", "yes")` de `cmd_cleanup`, que trata el rechazo como campo del resultado y no como error. Lo que no se copia de ahí es la forma de la elección: allí es binaria, y aquí la opción nueva convive con otras tres.

**Por qué «descartar» y no «rechazar».** En el bucle, regenerar **también** rechaza la toma. La palabra del contrato no distinguiría entre las dos opciones que descartan el audio actual, y solo una de ellas termina la invocación: «descartar y salir» nombra a esa. El precedente de `cleanup` no arbitra la elección, porque allí «no» solo puede significar una cosa.

**Efecto sobre §4.3.2, ya aplicado.** Su opción 1 se justificaba con un «exit 0 **siempre** significa "el artefacto existe"» que la cuarta opción falsifica: descartar sale 0 sin persistir nada. La propiedad sobrevive acotada al consumidor programático, que no alcanza el bucle —bajo `--json` lo prohíbe la regla 2, y sin `--json` con stdin cerrado entra por el `EOFError`—, así que la única salida 0 sin artefacto exige una respuesta interactiva. El enunciado de §4.3.2 ya está acotado a esa clase de consumidor; su recomendación no cambia.

**Efecto sobre §2.2:183, comprobado y nulo.** Esa frase declara imposible el estado «no reproduce pero pregunta» y deriva de ahí que `--no-play` implique `--yes`. La cuarta opción le cambia un número —«el bucle de tres opciones» pasa a cuatro— y nada más: la premisa nunca fue que las opciones sean mecánicamente inelegibles a ciegas —«aceptar» ya lo era con tres— sino que ninguna tiene sentido sin haber oído el audio, y descartar a ciegas deja la invocación en el efecto cero que la regla 5 de §2.3 condena. La conclusión, su estatus de consecuencia y la tabla de tres estados quedan intactas.

**Toca**: §2.2 (el bucle pasa de tres a cuatro opciones), §2.4 (una fila nueva de exit 0), §2.8 (la fila del exit 0 pierde «cancelar» y cambia «rechazar» por «descartar»; qué queda del `EOFError` lo decide §4.3.2, que escribe la misma fila), §3.3 paso 2.4.

#### 4.1.2. `--yes` sin `--label` es un no-op silencioso, y §2.4 declara que no hay ninguno

**Problema.** §2.4 cierra con: *«Ninguna combinación de flags queda ignorada en silencio en ninguna de las dos matrices: o tiene efecto declarado, o es exit 3 o 4.»* Las reglas 5 y 6 de §2.3 existen precisamente para sostener esa frase: `--no-play` y `--force` sin `--label` son un error declarado en vez de no-ops.

Pero `--yes` sin `--label` **no está cubierto por ninguna regla**. Sin etiqueta no hay bucle, así que `speech synthesize -t T -y` acepta el flag, no hace nada con él y sale 0. Es exactamente el patrón que las reglas 5 y 6 prohíben, en el tercer flag del mismo grupo.

Hay además un segundo no-op que la frase tampoco admite: `--force` con una etiqueta **libre**. Ahí sí hay precedente —`voice clone --force` sobre un nombre libre también es un no-op— así que no conviene prohibirlo; lo que hay que corregir es la afirmación.

**Propuesta.** Dos cambios que van juntos:

1. Añadir la **regla 7**: `--yes` requiere `--label` → exit 2. Completa el patrón de las reglas 5 y 6 y hace que los tres flags del bucle se comporten igual.
2. Reescribir la afirmación de §2.4 como *«ningún flag queda sin efecto sin que la CLI lo diga, salvo `--force` sobre una etiqueta libre, que es un no-op con precedente en `voice clone`»*.

**Toca**: §2.3 (regla nueva), §2.4 (una fila de exit 2 y el párrafo de cierre), §3.3 paso 2.3.

#### 4.1.3. «Violación de cualquiera de las reglas 1-6 → 4» es falso para media regla 3 *(⛔ suprimido por §4.1.6)*

> El diagnóstico de abajo describe el documento antes de §4.1.6 y se conserva como registro. Su propuesta ya no se aplica: el remapeo lleva ambas mitades de la regla 3 a exit 2 y no queda nada que partir. Ver §4.1.6, «Relación con §4.1.3».

**Problema.** La tabla de §2.8 dice: *«Violación de cualquiera de las reglas 1-6 de §2.3 → 4»*. La regla 3 es *«`--text` requerido y no vacío»*, y sus dos mitades salen con códigos distintos:

- vacío o solo espacios → **4**, validado a mano (`cli.py:283-285`);
- **ausente → 2**, porque `--text` es `required=True` en argparse (`cli.py:1736`).

El documento ya sabe esto: §1.2 y §2.3 lo advierten en sendas notas. Pero la tabla de códigos de salida —que es la parte que un integrador va a leer como contrato— afirma lo contrario en una línea.

**Propuesta.** Partir la regla 3 en dos numeradas, `3a` (presencia, exit 2, heredado de argparse) y `3b` (no vacía, exit 4), y ajustar la fila de §2.8 a «reglas 1, 2, 3b, 4, 5 y 6 → 4». Alternativa más barata y menos clara: dejar la regla como está y anotar la excepción en la propia fila de la tabla.

**Toca**: §2.3, §2.8 (una fila).

#### 4.1.4. El paso 2.2 se apoya en una afirmación falsa sobre el paso 1.1 *(✅ estrategia aprobada)*

**Problema.** §3.3 paso 2.2 dice que desacoplar la síntesis de la emisión es *«el único obstáculo técnico del bucle»* y cierra con: *«Esa línea ya desaparece en el paso 1.1 al quedarse sin segundo argumento.»*

No desaparece. La línea es `_emit_audio(result.audio_bytes, args.output)` (`cli.py:227`), **dentro** de `_synthesize_via_daemon`. El paso 1.1 le quita el segundo argumento y la deja en `_emit_audio(result.audio_bytes)`, seguida de `return result`: la invocación sigue viviendo dentro de la función de síntesis, que es exactamente el acoplamiento que el bucle no tolera. Lo que desaparece en el paso 1.1 es el argumento, no la llamada.

El efecto práctico es que el paso 2.2 queda descrito como ya resuelto cuando es trabajo real, y quien implemente el movimiento 2 lo dará por hecho. No es una imprecisión de redacción: **es un hueco de propiedad**. El trabajo existe y hoy no pertenece a ningún paso de ningún movimiento.

El paso 2.2 también describe el trabajo como «de una línea», y no lo es. El despacho de `cmd_speak` tiene **tres** ramas (`cli.py:337-401`): `--daemon` explícito (341-357), autodetección (358-373) y modo directo (375-401). Las dos primeras llaman a `_synthesize_via_daemon` y **retornan ahí mismo** (357, 366). La tercera ya emite desde el llamador (`394-398`), o sea que la rama directa **ya tiene la forma correcta** y la asimetría es solo entre ella y las dos daemon. Sacar la llamada de la función de síntesis no es moverla a un sitio: es decidir a cuántos.

**Propuesta.** Corregir la frase y dar al desacople **un paso propio del movimiento 1**. Cuatro decisiones:

*1. Un paso propio (1.13), no una cláusula del paso 1.1.* El defecto denunciado es un hueco de propiedad, y lo que cura un hueco de propiedad es propiedad explícita. El paso 1.1 es el de mayor radio del plan —cinco sitios en `cli.py` más cuatro módulos— y §4.3.6 y §4.4.1 ya quieren meterle trabajo; una cláusula más dentro de una enumeración larga es exactamente cómo este trabajo se perdió la primera vez. Por naturaleza pertenece al movimiento 1 (refactor puro, sin feature nueva); su motivación viene del movimiento 2 y se declara en el paso, en vez de dejarla implícita.

Va **al final de §3.2, sin renumerar los pasos existentes**. La restricción real es solo que corra después del 1.1, porque la convergencia depende de que ya no existan `--output` ni la rama `if args.output:` (`394-398`); insertarlo antes del 1.7 cumpliría eso igual, pero correría la numeración de 1.7-1.10 y rompería cinco citas: §4.4.1 cita el paso 1.7 tres veces y §3.4 el paso 1.8 dos, y ninguna quedaría apuntando a un error evidente sino a un paso *plausible*. Es el mismo argumento de la decisión 3 aplicado al movimiento 1; usarlo allí y no aquí sería incoherente. Consecuencia de ir después del 1.7: la función ya no se llama `cmd_speak`, así que el paso se redacta contra `speech synthesize` y declara la equivalencia —*«`cmd_speak` hasta el paso 1.7; las referencias de línea son al código previo al movimiento»*— para seguir siendo legible contra el repo actual, que es de donde salen todas sus citas.

*2. Alcance: converger el despacho a una cola única.* Las tres ramas pasan a calcular solo `result` y si la síntesis fue vía daemon; una única cola al final emite el audio y el payload. La alternativa mínima —emitir antes de cada `return`— satisface el enunciado pero deja **tres** puntos de emisión, así que la decisión de reproducir sigue triplicada y el bucle no gana un lugar único donde intervenir: el paso 2.4 tendría que volver a tocar estas mismas líneas. La convergencia unifica de paso las tres llamadas duplicadas a `_emit_speak_json` (356, 365, 400-401), y eso no es mejora colateral: es la misma triplicación, y dejarla produciría una cola única para el audio y triple para el JSON. Lo que **no** se hace es extraer el despacho a un helper invocable en bucle: eso es construir el esqueleto del movimiento 2 por adelantado.

*3. El paso 2.2 queda como puntero de una línea, sin renumerar.* Seis hallazgos citan pasos del movimiento 2 por número: §4.3.1 y §4.3.6 el 2.1, §4.1.2 el 2.3, §4.1.1 y §4.3.2 el 2.4, §4.3.4 el 2.6. Borrar el 2.2 y correr la numeración los invalida a los seis de una vez —la misma clase de rotura entre secciones que ya costó dos residuos en §4.2— y obliga a un barrido de citas donde cada una quedaría apuntando a un paso *plausible*, que es el modo de fallo más difícil de detectar. Un puntero cuesta una línea y además deja rastro de por qué ese trabajo no está en el movimiento 2.

*4. `_emit_audio` pasa a `_play_audio`.* El paso 1.1 le quita la rama de archivo (`cli.py:113-120`) y la función queda siendo solo «reproducir estos bytes», con un nombre que describe una capacidad que acaba de perder. Importa porque el bucle del movimiento 2 la llamará para reproducir una toma aceptada mientras persiste el WAV por otra vía, y ahí «emitir» se lee como ambiguo entre reproducir y persistir. El rename va en este paso porque ya está moviendo los sitios de llamada; y como el 1.13 corre después del 1.1, el texto del paso 1.1 sigue nombrando `_emit_audio` sin necesidad de ajuste.

**Toca**: §3.2 (paso 1.13 nuevo, al final), §3.3 paso 2.2 (la frase falsa desaparece; el paso sobrevive como puntero al 1.13).

*Dependencias dentro de §4.* **Cuatro hallazgos crean un paso al final de §3.2** —§4.1.6 el remapeo del contrato de salida, §4.1.7 el canal de error, este la convergencia del despacho y §4.3.6 el refactor del validador—, así que el reparto se fija aquí en vez de dejarlo al orden de propagación: **§4.1.6 toma el 1.11, §4.1.7 el 1.12, este el 1.13 y §4.3.6 el 1.14**.

Lo decide un criterio, no la conveniencia. Entre este hallazgo y §4.3.6 no hay dependencia técnica, así que ordena la seguridad ante descarte: §4.3.6 es de prioridad media y este de alta, luego el más probable de aplazarse lleva el número mayor y su caída deja la lista sin huecos. Frente a §4.1.6 y §4.1.7, los tres de prioridad alta, decide el **solape de región**: el `sys.exit()` de `cli.py:353` cae dentro del bloque `337-401` que este paso converge, y tras §4.1.7 es un `raise CliError` que abandona el despacho por excepción. Ir después le simplifica la cola a este paso en vez de obligar a §4.1.7 a rehacerla. No es conflicto de contenido —aquellos cambian cómo se sale con error, este cómo se emite en éxito—, es orden de ejecución. Comprobada y nula: **§4.4.1** mueve el bump de esquema al paso 1.1, que este hallazgo ya no toca. Y los seis hallazgos que citan pasos del movimiento 2 por número no quedan afectados **porque la decisión 3 los protege**: si el paso 2.2 desapareciera, los seis pasarían a ser aristas vivas. Simétricamente, §4.4.1 (paso 1.7) y §3.4 (paso 1.8) no quedan afectados porque la decisión 1 sitúa el paso nuevo al final de §3.2 en vez de antes del 1.7.

*Verificación.* Dos comprobaciones, una por decisión con entregable propio:

- **La convergencia (decisión 2)**: `grep -n "_play_audio(" src/tts_sidecar/cli.py` y `grep -n "_emit_speak_json(" src/tts_sidecar/cli.py` devuelven **dos líneas cada uno** —la definición y un único sitio de llamada—, que es lo que distingue la cola única de la alternativa mínima descartada. Sin la segunda, emitir antes de cada `return` pasaría la verificación igual.
- **El rename (decisión 4)**: `grep -n "_emit_audio" src/` sin coincidencias.

#### 4.1.5. §2.8 especifica los cuatro payloads y se desdice en la línea siguiente *(✅ estrategia aprobada)*

**Problema.** §3.5 presenta como abierto si el payload `--json` expone la ruta computada de la locución, y añade *«Alternativa, y la que asumen los payloads de §2.8: emitir solo `label` y `voice`»*.

El verbo «asumen» sugiere que §2.8 tomó una posición tácita. No es eso lo que pasa. §2.8 **declara** los cuatro payloads sin ruta (líneas 334-337) y a continuación, en la 339, los reabre: *«Si conviene o no exponer también la ruta computada de la locución es un punto abierto (ver 3.5)»*.

O sea que el punto no está «cerrado en el contrato y abierto en la lista de pendientes»: está abierto en los dos sitios, y el contrato se desdice de sí mismo a una línea de distancia. Es peor que cualquiera de las dos decisiones posibles. Un integrador que lea §2.8 —la sección titulada «contratos transversales», que es donde va a buscar qué puede consumir— obtiene cuatro payloads y, acto seguido, el aviso de que pueden crecer una clave. No hay nada sobre lo que programar.

**Propuesta.** **No se emite la ruta**, y el punto se cierra en las dos secciones.

*El criterio, que no es un decreto.* Un payload transporta una ruta del filesystem **solo cuando el recurso no tiene otro nombre en el contrato**. No hay que inventarlo: es lo que el repo ya hace, y los tres payloads existentes lo confirman sin excepción.

| Payload | Emite ruta | Por qué |
|---|---|---|
| `voice list --json` (`cli.py:562-565`) | No: `{"voices": [nombres]}` | La voz tiene handle propio —su nombre—, así que el directorio nunca sale |
| `cleanup --json` (`cli.py:1541-1546`) | Sí: `removed` como lista de rutas | Los directorios de caché del modelo y de voces **no tienen ningún handle en la CLI**; la ruta es su único nombre |
| `speak --json` (`cli.py:243-256`) | Sí: `output` | El `.wav` **no tiene ningún otro nombre en el contrato**: `speak` no registra la locución ni le da etiqueta, así que la ruta es su único nombre. Cae del mismo lado que `cleanup`, por la misma razón. La clave desaparece con `--output` en el paso 1.1 |

La locución tiene `(voz, etiqueta)`, y las cuatro sub-acciones del grupo operan exactamente sobre ese par. Cae del lado de `voice list`. Emitir además la ruta le daría al integrador un **segundo handle, no gobernado**, sobre un recurso que ya tiene el suyo — y nada le impediría usarlo, momento en el cual el invariante de §3.1 pasa a ser decorativo: no lo violaría el sistema, lo violaría el consumidor con lo que el sistema le entregó.

*Y el argumento que decide, que es de reversibilidad.* Las dos opciones no cuestan lo mismo si resultan equivocadas:

- Decidir **no** y tener que añadirla después es **aditivo**: añadir una clave está cubierto por la política de compatibilidad de `cli.py:51-54`, sin bump de esquema.
- Decidir **sí** y tener que quitarla después es **incompatible**, y este documento tiene la prueba escrita: §2.8 línea 328 justifica el bump de `cli.SCHEMA_VERSION` a `"2"` precisamente porque el payload de `speak` **pierde la clave `output`** — una ruta.

Es decir: el error barato y el error caro están identificados, y son el mismo caso que el rediseño ya está pagando. Con esa asimetría, mantener el punto abierto no compra opcionalidad; solo aplaza una decisión cuyo lado seguro ya se conoce.

*El coste, declarado.* Hoy nada permite sacar los bytes de una locución fuera de la CLI: `speech play` la reproduce y no hay ningún `export`. Sin la ruta, un orquestador que quiera el WAV no lo tiene. Eso es un hueco real **de la superficie de comandos**, no un argumento para filtrar la ruta por el payload. Si la necesidad aparece, la respuesta es un comando explícito con su propia decisión —tendría que aceptar una ruta de escritura del llamador, que es justo lo que §3.1 prohíbe—, y esa discusión no se gana metiendo una clave en un listado. Se declara aquí y queda fuera de alcance.

**Toca**: §2.8 (la línea 339 desaparece; los cuatro payloads pasan a ser la especificación, sin disclaimer) **y gana el criterio como regla explícita**, §3.5 (un punto menos; queda solo la ventana de §3.4).

*Dónde se deposita el criterio.* La decisión se registra aquí, pero **el criterio viaja con ella al propagar**: no basta con que §2.8 pierda la línea 339 y quede con cuatro payloads correctos y sin la regla que los generó. Un criterio que solo vive en un hallazgo de §4 —sección que se archiva al propagar— no es una regla del contrato: es la explicación de por qué cuatro casos salieron así, y el quinto payload tendría que deducirla por inducción, con la lectura equivocada («`cleanup` emite rutas porque es de limpieza») como la más disponible. El vecindario correcto en §2.8 son las líneas 325-330, donde ya vive la política de versiones de esquema apoyada en `cli.py:51-54`: ambas reglas gobiernan lo mismo, qué claves pueden aparecer y desaparecer de un payload.

*Limitación conocida del criterio.* Se extrajo de los tres payloads existentes y se validó contra esos mismos tres: el conjunto de extracción y el de validación coinciden, así que ninguna de las tres filas es una prueba independiente. Las explica sin excepciones y en los dos sentidos —una sin ruta, dos con ruta, por la misma razón—, que es lo máximo que puede ofrecer una muestra que él mismo moldeó. Someterlo a un caso que no haya visto solo es posible después de propagarlo, y de ahí que necesite quedar escrito en §2.8 y no aquí.

*Dependencias dentro de §4.* Una sola, y ya ajustada: **§4.2.3** sugería `path` como campo alternativo del payload de borrado, que este criterio excluye. Comprobadas y nulas: §4.2.2 y §4.3.5 (definen claves del listado y del sidecar, ninguna es una ruta), §4.3.1 (decide qué archivo registra la existencia, no qué se emite) y §4.4.1 (no hay clave nueva, así que no hay bump). Y una segunda, ajustada al aprobar: **§4.4.2** gana un cuarto punto para las claves exactas del payload. Se creía absorbida por su criterio reformulado y no lo estaba —ese criterio cubre combinaciones de flags y validación de entrada, no la forma del payload—, exactamente el mismo hueco que §4.1.7 tuvo que cerrar con su punto 3.

*El sitio de aterrizaje está libre.* El criterio entra en §2.8 junto al bloque de versiones de esquema (325-330), y ningún otro hallazgo reclama ese bloque: §4.1.6 y §4.3.2 escriben en la tabla de códigos de salida, §4.1.7 añade un apartado nuevo para el payload de error, §4.2.2 y §4.2.3 escriben en las viñetas de payloads —y ya están propagados—, y §4.4.1 mueve el paso que ejecuta el bump en §3.2 sin tocar su justificación en §2.8. No hay conflicto de escritura.

*Verificación.* Un test que fije las **claves exactas** de los cuatro payloads del grupo, no su contenido, de modo que añadir la ruta por descuido rompa la suite en vez de colarse. Queda escrito como **punto 4 de la propuesta de §4.4.2**: su criterio reformulado —matrices de §2.4 más reglas de §2.3— no alcanzaba a la forma del payload, así que no lo absorbía por sí solo.

#### 4.1.6. El documento declara la colisión del exit 2 y la deja sin cerrar, mientras el rediseño la propaga *(✅ estrategia aprobada)*

**Problema.** `--text` es `required=True` en argparse (`cli.py:1736`), así que omitirlo produce el **exit 2 nativo de argparse**, que en este proyecto significa `EXIT_MODEL_MISSING` (`cli.py:45`). §1.2 y §2.3 lo advierten en sendas notas y ahí se detienen.

Dejarlo sin cerrar tiene dos problemas.

El primero es de contrato: un orquestador que mapee exit 2 a «el modelo no está provisionado, ejecuta `setup`» **ejecutará una provisión completa del runtime ante una invocación mal formada**. No es un mensaje impreciso, es una remediación automática equivocada, y es la única de las siete situaciones de la tabla de §2.8 en la que el código de salida miente sobre la causa.

El segundo es que **el rediseño no hereda el defecto: lo propaga**. §2.5 declara `--label/-l` requerido en `speech play` y `speech remove`, así que la superficie nueva añade dos comandos más en los que olvidar un flag se reporta como «falta el modelo». Con `voice clone` —que ya tiene tres flags requeridos— serían seis puntos de la CLI con el mismo fallo.

**La causa no es que argparse invada un código ajeno: es que el contrato se apropió del código que la convención reserva para «uso incorrecto».** El 2 es, en Unix y en argparse, el código del error de invocación. El proyecto ya decidió una vez que argparse no debe decidir códigos de salida —la exclusión mutua `--daemon`/`--no-daemon` se valida a mano *«porque el exit 2 nativo de argparse colisionaría con `EXIT_MODEL_MISSING`»* (`cli.py:267-269`)— y aplicó esa solución en un caso y no en el otro. Pero construir maquinaria para impedir que argparse emita el 2, en vez de devolverle el código que le corresponde, es tratar el síntoma por el lado equivocado.

Hay **tres** pruebas internas de que la convención es la correcta y el contrato es el que diverge:

1. **La tabla ya honra la convención en otro punto.** `EXIT_INTERRUPTED = 130` es exactamente `128 + SIGINT` (`cli.py:49`). Respetar 128+n y no respetar 2 es incoherente dentro de la misma tabla.
2. **El proyecto hermano aplica la convención que este contradice.** `tts-sidecar-narrator` —el plugin de Claude Code que integra este motor— usa en su propio CLI **2 = uso incorrecto** en los tres casos que tiene: valor fuera de dominio (`narrate-ctl.ts:57`), argumento vacío (`:71`) y comando desconocido (`:77`), con **1 = error genérico** (`:32`). Es decir: dentro de la misma familia de proyectos, el 2 ya significa dos cosas distintas, y el que se salió de la norma es TTS-Sidecar.
3. **El bloque «congelado» ya no contiene la tabla completa.** `daemon/run.py:33` declara `EXIT_DAEMON_PORT_IN_USE = 6`, fuera del bloque de `cli.py:43-49`, ausente de la tabla pública de `USAGE.md:900-908`, y con una justificación escrita —*«vive en el paquete daemon (no en cli) para evitar un ciclo de import»*— que la **línea 24 del mismo archivo refuta**, porque ya hace `from ..cli import EXIT_ERROR`. Un contrato que se declara congelado y aun así crece por fuera de sí mismo no está congelado: está **sin dueño**.

**El defecto es más ancho que la colisión del 2.** Una auditoría de los cuarenta y tres `sys.exit()` del paquete encuentra cinco divergencias, no una:

| # | Divergencia | Evidencia |
|---|---|---|
| **D1** | El 2 está asignado a «modelo no provisionado» en vez de a «uso incorrecto» | `cli.py:45` frente a la convención y a `narrate-ctl.ts` |
| **D2** | Existe un séptimo código declarado fuera del bloque y no documentado | `daemon/run.py:33` (`= 6`), ausente de `USAGE.md:900-908` |
| **D3** | `EXIT_INVALID_INPUT` es un cajón de sastre: **7 de sus 15 llamadores** no reportan entrada inválida | ver la auditoría más abajo |
| **D4** | `EXIT_ERROR` recoge **2 conflictos de estado** que el código ya distingue por mensaje pero no por valor | `cli.py:547` y `cli.py:920` |
| **D5** | `EXIT_ERROR` recoge además **6 precondiciones de entorno** que el código diagnostica una por una, y que solo puede expresar en castellano | `cli.py:1367`, `1453`, y las cuatro ramas de `_describe_provision_failure()` (`1283`, `1292`, `1301`, `1307`) |

D3 no es un problema mientras el código valga 4 bajo una etiqueta laxa («entrada inválida»), pero **se vuelve una mentira en cuanto valga 2**, que tiene un significado convencional estrecho. Corregir D1 sin corregir D3 traslada el defecto en vez de eliminarlo.

D4 y D5 son el mismo patrón en dos grados: **una distinción que el código sabe hacer y solo puede expresar en castellano, porque no hay código donde ponerla**. En D4 es artesanal —`cli.py:533-547` existe porque alguien detectó que «los archivos de la voz están abiertos por otro proceso» no es un fallo cualquiera, y su comentario lo dice: *«sin esta rama, el except genérico de abajo reportaba el mismo mensaje que un nombre de voz inválido»*—. En D5 está institucionalizado: `_describe_provision_failure()` (`cli.py:1260-1313`) es una **función clasificadora** cuyo único trabajo es decidir si la provisión falló por credenciales, red, permisos o disco lleno, devuelve `str`, y sus cuatro ramas desembocan en el mismo `sys.exit(EXIT_ERROR)` (`cli.py:1493`). Con el pre-chequeo de disco (`1453`) y el FAIL de entorno (`1367`) son seis sitios cuya reacción no es la de ningún código existente.

**Propuesta.** Tres piezas, y las dos últimas se siguen de la primera: **un criterio que genera la tabla**, **un remapeo con tres códigos nuevos** —uno por divergencia sin cubrir— y **un dueño** para el bloque de constantes. La cuarta pieza que este defecto exige —un segundo canal para la causa fina que el entero no puede llevar— se especifica en **§4.1.7**.

| Código | Constante | Significado | Cambio |
|---|---|---|---|
| `0` | `EXIT_OK` | Éxito | — |
| `1` | `EXIT_ERROR` | Error genérico | — |
| `2` | `EXIT_INVALID_INPUT` | Uso incorrecto: la invocación está mal formada | **antes 4** (D1) |
| `3` | `EXIT_NOT_FOUND` | El recurso nombrado no existe | — |
| `4` | `EXIT_MODEL_MISSING` | Modelo no provisionado | **antes 2** (D1) |
| `5` | `EXIT_DAEMON_UNREACHABLE` | Daemon inalcanzable | — |
| `6` | `EXIT_STATE_CONFLICT` | El recurso existe o está ocupado; la operación no procede sin liberarlo o forzarla | **generaliza** `EXIT_DAEMON_PORT_IN_USE` (D2) |
| `7` | `EXIT_NOT_APPLICABLE` | La operación no aplica a este objetivo o entorno, y no aplicará reintentando | **nuevo** (D3) |
| `8` | `EXIT_PRECONDITION_FAILED` | Una precondición del entorno no se cumple; el remedio está fuera del programa y la operación es reintentable una vez corregida | **nuevo** (D5) |
| `130` | `EXIT_INTERRUPTED` | Interrupción del usuario | — |

**El criterio que genera la tabla son dos preguntas encadenadas, no una.** La primera forma las clases; la segunda decide cuáles merecen un entero propio. Separarlas es lo que vuelve la tabla derivable: un eje único mezcla dos trabajos distintos —clasificar y repartir— y toda formulación que los funde acierta en una mitad y falla en la otra.

**Primera pregunta — clasificación: ¿qué tipo de hecho impidió la operación?** Da seis clases, y son las únicas que la auditoría encuentra: invocación mal formada, recurso ausente, recurso ocupado, precondición de entorno incumplida, imposibilidad permanente e imprevisto.

**Segunda pregunta — admisión: ¿un consumidor programado cambiaría su siguiente llamada al distinguir esta clase de las demás?** Si sí, la clase gana entero propio; si no, comparte entero y la distinción baja al `reason` de §4.1.7. Es la pregunta que justifica el reparto, y se responde mirando qué se invocaría a continuación — sin apelar a la intuición de quien redacta.

| Código | Clase de causa | Siguiente llamada del consumidor |
|---|---|---|
| **1** | Imprevisto | Reintentar a ciegas, registrar o escalar |
| **2** | Invocación mal formada | Corregir el comando y reintentar |
| **3** | Recurso ausente | Crearlo, o nombrar otro |
| **4** | Precondición de entorno: el modelo | `tts-sidecar setup`, luego el mismo comando |
| **5** | Precondición de entorno: el daemon | `tts-sidecar daemon start`, luego el mismo comando |
| **6** | Recurso ocupado | `--force`, otro nombre, `daemon stop`, o esperar a que se libere |
| **7** | Imposibilidad permanente | **Ninguna** — no reintentar nunca |
| **8** | Precondición de entorno: el resto | Ninguna propia: delegar y reintentar el mismo comando |

**Los dos casos límite son inversos, y esa simetría es lo que valida el criterio.** El 4, el 5 y el 8 son **una** clase por causa —modelo ausente, daemon caído, disco lleno y token vencido son el mismo tipo de hecho— repartida en **tres** enteros, porque lo único que un consumidor puede convertir en una llamada distinta es un comando de esta CLI: `setup` y `daemon start` se separan y el resto colapsa en el 8. El 6 es lo contrario: **tres** remedios de naturaleza distinta (`--force`, `daemon stop`, cerrar un proceso externo) plegados en **un** entero, porque ninguno cambia lo que el consumidor distingue —«ocupado» frente a «ausente» y «mal escrito»—. La resolución del entero es la de lo que este programa puede nombrar como paso ejecutable, no una preferencia de diseño, y ninguna de las dos preguntas basta sola.

**El 6 no es un código nuevo: es el que ya existía, con dueño.** «Puerto ya en uso» (`daemon/run.py:152`) y «la voz ya existe» (`voices.py:204`) son el mismo hecho —el recurso está ocupado y hay que liberarlo o forzar— y merecen el mismo código; mantener `EXIT_DAEMON_PORT_IN_USE = 6` aparte mientras se añade otra constante al lado para «ya existe» reproduciría D2. La constante desaparece de `daemon/run.py` y `serve()` importa `EXIT_STATE_CONFLICT` por la misma vía por la que ya importa `EXIT_ERROR` (`daemon/run.py:24`).

**Auditoría de los quince llamadores de `EXIT_INVALID_INPUT`.** Siete se quedan, uno desaparece y siete se reclasifican:

| Sitio | Situación | Nuevo |
|---|---|---|
| `cli.py:270` | `--daemon` y `--no-daemon` a la vez | **desaparece**: pasa a un grupo mutuamente excluyente del parser, que ya sale con 2 |
| `cli.py:281` | `--json` sin `--output` | 2 ✔ |
| `cli.py:285` | `--text` vacío | 2 ✔ |
| `cli.py:299` | `--text` excede `MAX_TEXT_LENGTH` | 2 ✔ |
| `cli.py:353` | `--daemon` con una ruta fuera del sandbox | 2 ✔ (los dos argumentos son incompatibles entre sí) |
| `cli.py:551` | Nombre de voz ilegal (escapes de ruta) | 2 ✔ |
| `cli.py:1013` | `setup --uninstall --json` sin `--yes` | 2 ✔ |
| `cli.py:1539` | `cleanup --json` sin `--yes` | 2 ✔ |
| `cli.py:475` | La voz ya existe y no hay `--force` | **6** |
| `cli.py:1140` | Caskroom presente: abortar para no dejar un estado híbrido | **6** |
| `cli.py:528` | La voz es de fábrica y es de solo lectura | **7** |
| `cli.py:1003` | Instalación pip/uv: `--uninstall` solo aplica al canal nativo | **7** |
| `cli.py:1026` | `setup --uninstall` no soporta esta plataforma | **7** |
| `cli.py:1126` | `--uninstall` solo aplica a la instalación nativa de macOS | **7** |
| `cli.py:1229` | La desinstalación de Windows la gestiona el instalador | **7** |

Las siete reclasificadas comparten un rasgo: **la invocación es correcta**. Llamarlas «uso incorrecto» le dice al consumidor que arregle un comando que no tiene nada que arreglar — el mismo tipo de mentira que D1, con la misma consecuencia de remediación automática equivocada. Las cinco del 7 comparten otro: **son estáticas**, no dependen del estado; un consumidor que reintenta tras un 2 o un 6 puede tener éxito, uno que reintenta tras un 7 gira en vacío.

**Auditoría de los trece llamadores de `EXIT_ERROR` (D4 y D5).** Ocho se quedan: cinco `except Exception` genéricos (`cli.py:420`, `478`, `554`, `588`, `599`), los dos fallos de chequeo de `doctor` (`829`, `842`), que la propia constante documenta como suyos, y el único del paquete que vive fuera de `cli.py`: el `OSError` de bind que **no** es «puerto ya en uso» (`daemon/run.py:154`), un catch-all genuino —captura `OSError` sin tipar y su mensaje no promete reintento— que se queda en 1. Dos se reclasifican al **6**, y en ambos el estado del sistema es lo que bloquea:

| Sitio | Situación | Nuevo |
|---|---|---|
| `cli.py:547` | Los archivos de la voz están abiertos por otro proceso (daemon, reproductor, antivirus) y no se pueden borrar | **6** |
| `cli.py:920` | La ruta del symlink de PATH existe pero no es un symlink, así que no se toca | **6** |

Los dos cumplen la definición del 6 al pie de la letra —recurso presente u ocupado, con remedio: cerrar el proceso, mover el archivo— y sus mensajes ya le dicen al usuario exactamente eso. Lo único que falta es decírselo también al consumidor programado.

**Los tres restantes son D5, y van al 8 — seis sitios en total.** Los cuatro últimos comparten la línea de salida (`cli.py:1493`): la distinción entre ellos la hace `_describe_provision_failure()` y hoy muere en la cadena que devuelve.

| Sitio | Causa | Lo que su propio mensaje ya pide | Nuevo |
|---|---|---|---|
| `cli.py:1367` | Falta una dependencia del runtime (p. ej. `chatterbox-tts`) | «NO INSTALADO (pip install chatterbox-tts)» | **8** |
| `cli.py:1453` | Menos de `MIN_FREE_DISK_BYTES` libres antes de descargar | «Libera espacio y reintenta» | **8** |
| `cli.py:1283` | `HF_TOKEN` ausente, expirado o sin acceso al repo | «Revísalo y reintenta» | **8** |
| `cli.py:1292` | Fallo de red durante la descarga | «Verifica tu conexión (o el proxy/firewall) y reintenta» | **8** |
| `cli.py:1301` | Sin permisos de escritura en la caché del modelo | «Corrige los permisos y reintenta» | **8** |
| `cli.py:1307` | Disco lleno *durante* la descarga (`ENOSPC`) | «Libera espacio y reintenta» | **8** |

Los seis terminan con la misma frase —*«y reintenta `tts-sidecar setup`»*— porque los seis son reintentables; lo que falta antes del reintento es una acción humana que el programa nombra pero no puede ejecutar. Esa es la reacción que el 8 codifica y que ningún código existente expresa: el 1 es lo imprevisto y estos están previstos, nombrados y diagnosticados uno a uno; el 6 exige un remedio al alcance del consumidor, y aquí no lo está.

**El caso de red es el único ambiguo de los seis.** `requests.exceptions.RequestException` (`cli.py:1292`) cubre a la vez el corte transitorio —que solo admite espera, y por el eje encajaría en el 1— y el bloqueo estructural de un proxy o un firewall, que sí tiene remedio fuera de esta CLI y encaja en el 8. El código no puede distinguirlos, así que se elige **por el fallo seguro**: clasificar un corte transitorio como 8 cuesta un turno, mientras que un consumidor que reintenta a ciegas contra un firewall gira sin cota.

**Lo que no va al 8.** `doctor` (`cli.py:829`, `842`) sale con 1 aunque varios de sus chequeos sean precondiciones de entorno: su trabajo *es* diagnosticar, así que su código no-cero no reporta un fallo del comando sino el resultado del diagnóstico. La propia constante lo documenta como suyo (`cli.py:44`) y se queda en 1 — ver *Límites declarados*.

**Con esto la colisión no se mitiga: deja de existir.** El exit 2 nativo de argparse pasa a significar exactamente lo que argparse quiere decir con él, así que **las siete rutas de fallo de parseo quedan correctas sin escribir una línea de código**: flag requerido ausente —los cinco de hoy (`cli.py:1736`, `1771`, `1772`, `1774`, `1782`) y los dos que añade §2.5 (`speech play` y `speech remove --label`)—, valor fuera de `choices` (`cli.py:1740`, `--compute-backend`), grupo mutuamente excluyente violado (`setup` en `cli.py:1801`, y `speak` tras la unificación de más abajo), subcomando inválido en los tres niveles (`cli.py:1732`, `1764`, `1838`), flag desconocido en cualquier comando y el parser del daemon (`daemon/run.py:193`).

**La octava ruta no es una excepción: es la distinción correcta.** `tts-sidecar` a secas y `tts-sidecar voice` a secas no llegan a `error()`: `add_subparsers` no es `required`, así que `main()` los intercepta, imprime la ayuda y sale con **`EXIT_OK`** (`cli.py:1881-1888`). Es deliberado y debe quedarse: una invocación sin subcomando es exploratoria, igual que `--help`, y ambas salen con 0 por la misma razón. La regla no es «ausente o inválido → 2», sino **ausente = exploración (0), inválido = error (2)**. El remapeo no toca `--help` —argparse lo despacha por `exit(0)`, nunca por `error()`—, pero el render único de §4.1.7 sí lo pone en su camino; la salvaguarda se especifica allí.

**Qué deja de ser necesario.**

- La validación manual de la regla 1 (`cli.py:266-270`): su única justificación escrita es la colisión, que desaparece. Se sustituye por un grupo mutuamente excluyente, y `add_mutually_exclusive_group` de `setup` (`cli.py:1801`) queda correcto de forma retroactiva — ver el bloque siguiente.
- El comentario de `daemon/run.py:30-32`, cuya premisa (el ciclo de import) es falsa.

**Un solo mecanismo para la exclusión mutua, y es el declarativo.** El proyecto tiene hoy los dos: el grupo de argparse en `setup` (`cli.py:1801`, tres modos) y el `if` a mano en `speak` (`cli.py:266-270`, dos flags). Retirada la colisión, el `if` queda sin justificación escrita y la divergencia sin árbitro, así que hay que elegir uno y aplicarlo a ambos. Se elige el grupo, por el mismo criterio con el que §4.1.7 rechaza el `if` por sitio: *«la garantía queda en un solo lugar, no repetida por convención en cada comando»* (`cli.py:59-66`). Una comprobación manual de exclusión mutua **es** esa convención repetida, y no escala: el grupo de `setup` es de tres modos, y a mano un cuarto no rompería nada —el conteo sigue pasando y deja de cubrir una combinación en silencio—. El grupo declara la regla junto a los flags que restringe; el `if` vive a unas mil cuatrocientas líneas de ellos, donde nadie que añada uno la va a leer.

Cuesta el mensaje en castellano de `speak --daemon --no-daemon`, y ese coste es menor de lo que parece: no es una regresión de la CLI sino la retirada de su única excepción, porque las siete rutas de parseo que este hallazgo declara correctas sin tocar código ya responden con el texto inglés de argparse. Lo que se pierde del payload lo recupera §4.1.7 con el `error()` sobrescrito. `tests/test_cli.py:1158`, que hoy afirma el mensaje llamando a `cmd_speak` con un `args` falso, migra a un test de parser.

**El bloque de constantes necesita un dueño, no una advertencia mejor.** D2 no ocurrió pese a que el contrato estuviera declarado congelado en `cli.py:40`: ocurrió **porque** lo estaba. Un contrato cerrado sin un lugar legítimo donde crecer no impide el crecimiento: lo empuja fuera del campo de visión. Reescribir ese comentario con una regla mejor es repetir la apuesta que ya se perdió una vez.

**Las constantes se mudan a un módulo hoja, `exit_codes.py`, sin imports del paquete.** Ataca la causa en vez del síntoma: un módulo sin dependencias internas **no puede** cerrar un ciclo, así que la justificación que produjo D2 deja de estar disponible ni siquiera como pretexto. `cli.py` reexporta las constantes, de modo que los 139 usos por nombre y los tests que las alcanzan como `cli.EXIT_*` siguen valiendo sin tocarse; `daemon/run.py` importa del módulo hoja en vez de arrastrar `cli` entero, que es lo que se quería evitar en primer lugar.

**Dos invariantes de test cierran las dos mitades de D2**, que son distintas y de las que solo una se estaba vigilando:

1. **Ningún `EXIT_*` puede definirse fuera de `exit_codes.py`.** Recorre los módulos del paquete y falla ante una asignación con ese prefijo en cualquier otro archivo. Es el test que habría detectado `daemon/run.py:33` el mismo día que se escribió.
2. **La tabla de `USAGE.md` y el módulo dicen lo mismo.** Compara los pares valor/constante con las filas de la tabla pública. Es la otra mitad de por qué D2 permaneció invisible: no solo se declaró por fuera, además quedó sin documentar, y nada acusó la diferencia.

**Coste y alcance del cambio**, medidos sobre el repositorio:

- **`src/`**: cero literales numéricos de salida fuera de los dos bloques de constantes; los 139 usos de los `EXIT_*` son por nombre. Los quince sitios reclasificados son doce `sys.exit()` de una línea, salvo `1493`, que abre las cuatro ramas de `_describe_provision_failure()` y cambia de firma. Se suma la mudanza a `exit_codes.py` y la reexportación desde `cli.py`, sin tocar ningún llamador.
- **`docs/ROADMAP.md`**: nueve menciones, todas por nombre. Cero cambios.
- **Tests**: `tests/test_cli.py:1431` (`exc.value.code == 2` para `setup --uninstall --remove-path`) **pasa a ser correcto sin tocarlo**; hay que corregir su comentario de la línea 1425. `tests/test_daemon.py:1157`, `:1166` y `:1180` afirman el literal `6` y siguen pasando; `tests/test_daemon_run.py:117` nombra `daemon_run.EXIT_DAEMON_PORT_IN_USE` y hay que reapuntarlo a `EXIT_STATE_CONFLICT`, igual que el docstring de `test_daemon.py:1125`. `tests/test_cli.py:2425` afirma `cli.EXIT_ERROR` para el pre-chequeo de disco y pasa a `EXIT_PRECONDITION_FAILED`. `tests/test_cli.py:1158` afirma el mensaje castellano de la regla 1 invocando `cmd_speak` con un `args` falso, así que la validación ya no lo alcanza: migra a un test de parser. Los quince sitios reclasificados necesitan que sus tests, donde existan, cambien de código esperado.
- **`tts-sidecar-narrator`**: cero cambios **hoy**. No ramifica por código de salida en ningún punto —`narrate-worker.ts:105-106` solo registra `code !== 0`, y `health-check.ts` y `daemon.ts` deciden por el payload JSON—, y su superficie de invocación se limita a seis comandos. Eso mide el coste de migración, **no la vigencia del contrato**: ver más abajo.
- **Documentación**: `USAGE.md:900-908` (dos filas intercambiadas, tres nuevas, y ejemplos que hoy faltan), §2.8 de este documento y §1.3 —esta última solo un puntero, no una renumeración—, y una entrada de `CHANGELOG.md` declarando el cambio incompatible.

**Este cambio solo es viable ahora, y la ventana se cierra antes de lo que parece.** El intercambio de 2 y 4 es **indetectable para un consumidor**: mismo binario, mismos valores, semántica invertida en los dos sentidos a la vez, y el `schema_version` versiona el payload JSON (`cli.py:51-53`), no el código de salida, así que no hay señal posible. Hoy el riesgo es cero **por la superficie que el plugin todavía no invoca**, no por la versión ni por la fecha de publicación: `tts-sidecar-narrator` va a crecer hasta cubrir la CLI entera —están previstas las skills de clonado, síntesis y gestión del almacén—, y ese consumidor **no debe recurrir a leer castellano** para decidir su siguiente paso. La ventana no se cierra en la 1.0: se cierra cuando la primera skill ramifique por uno de estos códigos.

De ahí el corolario que gobierna el resto de este hallazgo: **la ausencia de consumidor actual no valida ninguna clasificación**. Un código que hoy nadie lee y que miente seguirá mintiendo cuando lo lean, y para entonces corregirlo será una ruptura en vez de un refinamiento. El comentario de `cli.py:41-42` se reescribe en consecuencia: fechar el congelamiento **en la 1.0** en vez de darlo por vigente hoy, y advertir que la tabla se define por el tipo de causa y por la siguiente llamada del consumidor, no por quién consume el código ni por si alguien lo consume.

**Dos consecuencias de implementación, que no son enteros.**

La primera: `cli.py:472-475` captura `ValueError` de `clone_voice_files` y mapea **todas** sus causas al mismo código —audio ilegible, nombre ilegal y colisión son hoy indistinguibles para el llamador—. Para emitir 6 solo en la colisión hay que distinguirlas en origen: declarar en `voices.py` una excepción estrecha —`VoiceExistsError(ValueError)`, levantada en `voices.py:203-204`— y capturarla **antes** del `except ValueError` genérico, que se queda con el 2. Hereda de `ValueError` por precaución y no por necesidad: `clone_voice_files` tiene un solo llamador (`cli.py:443`) y un solo `except ValueError` en esa ruta.

La segunda: la firma de `_describe_provision_failure()`. Hoy devuelve `str` y su llamador (`cli.py:1491-1493`) imprime esa cadena y sale con un código fijo, así que las cuatro ramas no pueden emitir códigos distintos. Pasa a devolver el par `(code, message)`, con 8 en las cuatro ramas diagnosticadas y 1 en la genérica. §4.1.7 la extiende a la terna `(code, reason, message)`; sin §4.1.7, el par basta para el remapeo.

Alcance del 6 en la superficie nueva: `voice clone` sobre un nombre tomado (hoy 4) y `speech synthesize --label` sobre una etiqueta tomada (§2.4 fila 7, hoy especificada como 4). El rediseño cambia el peso del caso: hoy la colisión solo ocurre en `voice clone`, que es esporádico; con `speech synthesize --label` ocurre cada vez que se regenera una locución ya existente, que es el flujo normal de trabajo y no la excepción.

**Alternativa considerada para el 7, y por qué se descarta.** Los cinco casos de «no aplicable» podrían plegarse sobre `EXIT_ERROR = 1` en vez de tener código propio. Sería más barato y no introduciría ninguna mentira, porque «error genérico» no afirma una causa concreta, y el mensaje de stderr —detallado y accionable en los cinco— seguiría estando.

Se descarta porque el 1 y el 7 no son vecinos: en el 1 **no se conoce** remedio, en el 7 **se sabe que no lo hay**, y esa diferencia no es epistemológica sino operativa. Colapsar causas distintas sobre un genérico es incorrecto precisamente cuando la acción que sigue al error difiere entre ellas, y aquí difiere de la forma más fuerte posible: 1 es el código de lo imprevisto —potencialmente transitorio y por tanto razonable de reintentar—, y estos cinco son previstos, deterministas y con remedio nulo. Fundirlos borra la única señal que importa, que es *no reintentar*. Que hoy ninguno de los cinco sitios tenga consumidor programático no cambia nada, por el corolario de arriba. La decisión es reversible y afecta a cinco líneas.

**Relación con §4.1.3.** Lo sustituye. §4.1.3 propone partir la regla 3 en `3a` (presencia, exit 2) y `3b` (no vacía, exit 4) para que la tabla de §2.8 deje de mentir. Tras el remapeo **ambas mitades salen con 2** —la ausencia por argparse, el texto vacío por la validación de `cli.py:283`— y la regla 3 no necesita partirse. Son dos soluciones al mismo síntoma: 4.1.3 documenta la anomalía, 4.1.6 elimina su causa.

**Relación con §4.1.7.** Lo habilita y lo necesita. Este hallazgo corrige las instancias que la auditoría encontró; §4.1.7 retira la presión que las produjo, que es la que convirtió D2, D3, D4 y D5 en el mismo defecto repetido. Sin él, la próxima distinción real se topará con las mismas tres salidas —gastar un entero, declararlo por fuera, o perderlo en prosa— y esta sección habrá que escribirla otra vez. El orden es este primero: §4.1.7 convierte los `sys.exit()` que este hallazgo acaba de reclasificar.

**Límites declarados.**

- **El eje no genera el 0, el 130 ni el 1 de `doctor`, y no debe intentarlo.** El 0 no es un fallo, así que no tiene remedio del que hablar; el 130 es convención de señales (`128 + SIGINT`), ajena al eje y correcta por otra razón; y el 1 de `doctor` (`cli.py:829`, `842`) usa el entero como canal de **veredicto** y no de fallo. Son las tres únicas excepciones, y las tres son declaradas. Anotarlas importa: sin la tercera, un lector futuro aplica el eje a `doctor` y lo «corrige».
- **La reexportación desde `cli.py` crea dos sitios donde *parecen* vivir las constantes.** El invariante 1 lo desactiva: cualquier definición fuera de `exit_codes.py` falla, así que la reexportación es un alias y no una segunda declaración. La distinción hay que dejarla escrita en el módulo, porque no es evidente al leer `cli.py`.
- **De las dos reglas transversales que este hallazgo deja, solo una es un test.** «Ningún `sys.exit(EXIT_ERROR)` puede alcanzarse por una causa prevista con remedio declarado en su propio mensaje» **sí es mecanizable** —tras D5, un `EXIT_ERROR` cuyo mensaje contenga «reintenta» es por construcción un olvido—. «Ningún `sys.exit(EXIT_INVALID_INPUT)` puede alcanzarse con una invocación bien formada», en cambio, **no lo es**: «bien formada» no tiene definición ejecutable, y escribirla como test produciría una aserción que no afirma nada. Es un criterio de revisión, y su lugar es el comentario del módulo junto al criterio generador, no la suite.

**Toca.**

*Código.*

| Sitio | Cambio |
|---|---|
| **`src/tts_sidecar/exit_codes.py`** (módulo nuevo) | Las diez constantes y el comentario del criterio, sin imports del paquete |
| `cli.py:43-49` | Convertido en reexportación del módulo hoja |
| `cli.py:41-42` | Comentario reescrito y trasladado a `exit_codes.py`: fechar el congelamiento en la 1.0, advertir que el intercambio es indetectable, enunciar el criterio generador en sus dos tiempos —clase de causa y admisión por la siguiente llamada del consumidor— y recoger el criterio de revisión que no puede ser test |
| `cli.py:266-270` | Sustituir la validación manual de `--daemon`/`--no-daemon` por un `add_mutually_exclusive_group` en el parser de `speak`, junto a los dos flags. Cae con ella el comentario de `266-267`, cuya premisa —la colisión— ya no existe; el de `264-265` justifica **que** se valide y sobrevive, trasladado al grupo |
| `cli.py:475`, `528`, `1003`, `1026`, `1126`, `1140`, `1229` | Los siete reclasificados desde `EXIT_INVALID_INPUT` al 6 o al 7 |
| `cli.py:547`, `920` | De `EXIT_ERROR` al 6 |
| `cli.py:1367`, `1453` | De `EXIT_ERROR` al 8 |
| `cli.py:1491-1493` + las cuatro ramas de `_describe_provision_failure()` (`1283`, `1292`, `1301`, `1307`) | Cambia de firma a `(code, message)`: 8 en las cuatro ramas diagnosticadas, 1 en la genérica |
| `voices.py:203-204`, `cli.py:472-475` | `VoiceExistsError` y su captura previa al `except ValueError` genérico |
| `daemon/run.py:30-33`, `:24`, `:152` | Retirar `EXIT_DAEMON_PORT_IN_USE` y su comentario, reapuntar el import al módulo hoja y emitir `EXIT_STATE_CONFLICT`. `:154` no cambia: se queda en 1 |

*Tests.* `tests/test_cli.py:1425` (el comentario, no la aserción); `tests/test_cli.py:1158` (migra de `cmd_speak` con `args` falso a un test de parser); `tests/test_daemon_run.py:117` y el docstring de `tests/test_daemon.py:1125` (renombre de la constante; los literales `6` de `:1157`, `:1166` y `:1180` siguen siendo correctos); `tests/test_cli.py:2425` (pasa de `EXIT_ERROR` a `EXIT_PRECONDITION_FAILED`); los tests existentes de los quince sitios reclasificados. **Invariantes de gobernanza**: uno que falle si algún módulo del paquete define un `EXIT_*` fuera de `exit_codes.py`, y otro que compare el módulo con la tabla de `USAGE.md`.

*Documento.* La nota de §2.3 (línea 213) desaparece: el rediseño deja de heredar el defecto. Pasan de «4» a «2» **once menciones**: las seis reglas de §2.3, tres de §2.4 (la fila de etiqueta ilegal, la frase de cierre de la línea 247 y el «no 4» de la línea 274) y dos de §2.8 (la fila de las reglas 1-6 y la de etiqueta ilegal). Pasan a **6**, no a 2, la fila 7 de §2.4 y la fila de colisión de §2.8: son contiguas a las anteriores y parten del mismo valor, así que un reemplazo global las estropea sin dejar rastro —las tres quedan con un número plausible—. Y la **tesis de apertura de §2.8** —*«Los siete códigos de salida siguen congelados. El rediseño no añade ninguno»*— se reescribe aparte de su tabla: no es una fila, así que un barrido guiado por tablas la deja intacta y falsa encabezando la tabla ya corregida. §3.2 gana el **paso 1.11**, al final y sin renumerar los existentes, y la nota de verificación de la línea 415 se ajusta. Su enunciado, para que no lo redacte el implementador: *«Paso 1.11 — remapear el contrato de salida. Extraer las diez constantes a `exit_codes.py`, intercambiar el 2 y el 4, añadir el 6, el 7 y el 8, reclasificar los quince llamadores impropios, sustituir la validación manual de `--daemon`/`--no-daemon` por un grupo mutuamente excluyente, declarar `VoiceExistsError` y cambiar la firma de `_describe_provision_failure()` a `(code, message)`. Refactor puro: no añade superficie de comandos.»*

**§1.2 y §1.3 no se renumeran**: son el retrato del estado de partida y sus cifras están ancladas a la cita de `cli.py:43-49`, así que cambiarlas dejaría a §1.2 describiendo una CLI que no existe ni antes ni después. Cada una gana un puntero a la tabla nueva. La nota de §1.2 (línea 82) se conserva —el defecto que describe es real en el contrato actual— y solo cambia «§4.1.6 propone corregirlo» por la constatación de que ya está corregido.

*Documentación pública.* `USAGE.md:900-908` (tabla completa: filas del 2 y el 4 intercambiadas, filas del 6, el 7 y el 8 nuevas, las dos columnas del criterio —clase de causa y siguiente llamada del consumidor— y ejemplos de «flag requerido ausente», «flag desconocido» y «subcomando inválido» en la fila del 2); `CHANGELOG.md` (entrada de cambio incompatible).

*Verificación del paso 1.11.* Un test por ruta de fallo de parseo comprobando 2; uno que fije en 0 la invocación sin subcomando y sin sub-acción (`cli.py:1881-1888`), que es la única salida 0 que depende de código propio; uno de conflicto por cada superficie —`voice clone`, `speech synthesize --label`, el bind del daemon y el borrado de una voz con archivos abiertos— comprobando 6; y uno por cada llamador del 7. Para el 8, un test por sitio: el FAIL de entorno, el pre-chequeo de disco (que ya existe en `tests/test_cli.py:2411` y solo cambia de código esperado) y las cuatro ramas de `_describe_provision_failure()`, inyectando la excepción correspondiente.

#### 4.1.7. El fallo no tiene forma legible por máquina, y tres comandos ya improvisaron la suya *(✅ estrategia aprobada)*

**Problema.** §4.1.6 reparte las causas entre diez enteros, pero el entero no puede llevar la causa fina y no debe intentarlo. Las seis causas del 8 inducen una sola reacción en el consumidor —delegar y esperar—, y eso es lo que las hace *un* código; pero inducen **cinco acciones distintas en el destinatario**: liberar disco, corregir permisos, renovar el token, desbloquear la red, instalar una dependencia. Esa distinción existe hoy, la calcula `_describe_provision_failure()`, y muere en la cadena que devuelve. Darle un entero a cada una repetiría el error que §4.1.6 denuncia; dejarla en castellano repite D5 un nivel más arriba, con el agravante de que el consumidor previsto es un agente que en algunos de esos casos **podría** ejecutar el remedio.

**El proyecto tiene dos canales legibles por máquina y solo ha usado uno para los fallos.** El payload JSON se declara **aditivo** por contrato (`cli.py:51-53`: *«añadir claves nuevas no incrementa la versión»*) y tiene un punto único de emisión (`emit_json()`, `cli.py:56-69`). El entero es un espacio de diez valores declarado congelado. Usar el canal cerrado para cada distinción nueva mientras el abierto queda sin tocar es la causa raíz común de D2, D3, D4 y D5: **cuando la única forma de ser legible por máquina es gastar un entero del contrato, toda distinción nueva o gasta uno, o se declara por fuera, o se pierde en prosa**. Las tres cosas pasaron.

**Y hoy el canal se corta casi exactamente donde más falta hace.** De los diecisiete llamadores de `emit_json()`, **catorce están en rutas de éxito o de diagnóstico**: `speak --json` que falla deja stdout vacío (`cli.py:400-420`) — el consumidor pidió una respuesta legible por máquina y, justo cuando algo va mal, recibe el entero y castellano en stderr.

**Los tres restantes son la evidencia decisiva.** `daemon start`, `stop` y `restart` (`cli.py:1651`, `1663`, `1680`) **sí** emiten payload al fallar, y lo hacen con una clave booleana `ok` inventada en el sitio, seguida de `sys.exit(EXIT_DAEMON_UNREACHABLE)`. Donde alguien necesitó señalar un fallo por el canal legible, **improvisó una convención propia**, porque no había ninguna. Es la misma enfermedad que D2 —una necesidad legítima resuelta fuera del contrato, por falta de un lugar dentro— manifestada en el payload y no en el entero. No hay que inventar el canal: hay que **canonizar y extender el que ya se abrió paso solo**.

**Propuesta.** Una forma común de payload de error, una política que la mantiene ampliable, y un mecanismo que la sostiene sin depender de que nadie olvide un sitio.

**Forma del payload de error.** Una clave de primer nivel que hoy no existe en ningún payload —verificado: cero apariciones de `error` como clave en `cli.py`—, emitida solo bajo `--json` y dejando intacto el stderr en castellano para el uso humano:

```json
{"schema_version": "1", "error": {"code": 8, "reason": "disk_full", "message": "…"}}
```

**Política de compatibilidad, que es lo que impide reabrir la misma brecha un nivel más allá.** Tres reglas:

1. **El entero siempre basta por sí solo.** `reason` refina; nunca contradice ni condiciona. Un consumidor que ignore la clave se comporta correctamente, solo que con menos resolución. Esta regla es la que hace el canal ampliable sin ruptura, y sin ella el segundo canal sería una segunda tabla congelada.
2. **Añadir un `reason` nuevo no incrementa `schema_version`**, igual que añadir una clave. Un `reason` desconocido se trata como ausente, es decir, se degrada al entero.
3. **Regla de promoción**, que resuelve de una vez la pregunta «¿código nuevo o razón nueva?»: un código de salida nuevo solo se justifica cuando cambia **la siguiente llamada del consumidor** —la segunda de las dos preguntas que generan la tabla de §4.1.6—; cuando la llamada siguiente es la misma y lo que cambia es la acción concreta que alguien ejecuta antes de repetirla, es un `reason`. Su árbitro es comprobable sin discutir definiciones: se responde diciendo qué se invocaría a continuación. Bajo esta regla el 6, el 7 y el 8 son códigos legítimos, y las cinco causas del 8 no lo son.

**Invariante del canal.** Bajo `--json`, **toda salida no-cero emite el payload de error**; `code` y `message` son obligatorios, y `reason` es opcional en cualquier código: se define donde la distinción **ya existe calculada** en el código, que hoy es solo el 8 (`_describe_provision_failure()`). El 6 y el 7 agrupan subcausas todavía sin nombrar, y añadírselas más adelante es aditivo por la regla 2 — la restricción de hoy es de secuencia, no de diseño.

**La invariante no se sostiene con un `if` por sitio.** Hacerlo así la dejaría en manos de que nadie olvide uno, y exigiría un test que vigilara el olvido — que es exactamente la forma que este proyecto ya descartó para la ruta de éxito: `emit_json()` existe porque *«la garantía queda en un solo lugar, no repetida por convención en cada comando»* (`cli.py:59-66`). La ruta de fallo merece la misma solución. Los sitios dejan de llamar a `sys.exit(CODE)` tras imprimir y pasan a levantar un tipo propio, `CliError(code, reason, message)`, y **`main()` es el único punto que lo traduce**: mensaje humano a stderr, payload a stdout si se pidió `--json`, y salida con el código. Así la invariante deja de necesitar vigilancia, porque no queda otro camino hasta la salida — y el test que la protege pasa de «comprobar que cada sitio emite» a «ningún `sys.exit()` no-cero fuera de `main()`», que es la misma forma mecanizable de los dos invariantes de gobernanza de §4.1.6.

**`CliError` hereda de `BaseException` y no de `Exception`, y esa elección no es estilística.** Es lo que hace la conversión preservadora de comportamiento. Hoy `sys.exit()` levanta `SystemExit` —también `BaseException`— y por eso atraviesa los `except Exception` que envuelven a cada comando. Con `Exception` como base, **siete de las salidas que la conversión alcanza quedarían capturadas por el manejador genérico de su propio comando y saldrían con 1** —seis de ellas auditadas en las tablas de §4.1.6, más el `EXIT_NOT_FOUND` de `cli.py:531`, que la conversión toca sin reclasificar—: los cuatro de `cmd_speak` (`281`, `285`, `299`, `353`) contra el `except` de `412`; los dos de `cmd_voice_remove` (`528`, `531`) contra el de `552`; y el pre-chequeo de disco (`1453`) contra el de `1491`, que además lo pasaría por `_describe_provision_failure()` —la función clasificadora que es el objeto de D5— y lo diagnosticaría como imprevisto. Una señal de control de flujo no debe ser capturable por un manejador de errores de dominio — es la razón por la que `SystemExit` tampoco lo es. Con la base correcta, los treinta y siete sitios se convierten mecánicamente sin auditar el `try` de cada uno.

**Y absorbe el caso de argparse, que un `if` por sitio no podría alcanzar.** `parse_args()` levanta `SystemExit(2)` sin pasar por código propio, así que una invocación mal formada bajo `--json` dejaría stdout vacío — y tras el remapeo de §4.1.6 el 2 es el fallo más frecuente que verá un consumidor programado. Envolver `parse_args()` en el mismo handler cierra la mitad: da el código, no el `message` que la invariante declara obligatorio, porque argparse lo formatea y lo imprime a stderr sin devolverlo. La otra mitad la cierra **sobrescribir `error()` en una subclase de `ArgumentParser`** para que levante `CliError(EXIT_INVALID_INPUT, "usage_error", message)` en vez de imprimir y salir: cuatro líneas en un único sitio, y con ellas el texto que argparse ya calcula entra al payload en vez de perderse. Es también lo que sostiene la unificación de §4.1.6 —el grupo mutuamente excluyente de `speak` sale por esta ruta, no por código propio—, y no alcanza al parser de `daemon/run.py:193`, que queda fuera por la razón de más abajo. Queda un residuo honesto: al fallar el parseo no existe `args`, así que hay que mirar `sys.argv` para saber si se pidió `--json`. Es feo, pero solo decide *si* emitir —no qué— y vive en un único sitio.

**El render debe dejar pasar intacto cualquier `SystemExit` de código 0.** Es la contrapartida de meter `parse_args()` en el handler: `--help` sale por `exit(0)` sin pasar nunca por `error()`, así que un handler que no discrimine por código emitiría payload de error y mensaje a stderr en la invocación más común de toda la CLI. Es el único caso —verificado: no hay `action="version"` ni ningún otro `parse_args()` en `cli.py`—, y por eso justifica un test de regresión que antes del canal no hacía falta. Con `error()` sobrescrito la salvaguarda se afila en vez de complicarse: el único `SystemExit` que el parser levanta ya es el del código 0, así que dejarlo pasar no compite con ninguna otra salida del mismo tipo.

`daemon/run.py` queda fuera del mecanismo, pero no por la razón que parece: `tts-sidecar daemon serve` **sí** pasa por `main()` —`cmd_daemon` importa `serve()` y lo ejecuta en el mismo proceso (`cli.py:1616-1628`)—, así que sus dos `sys.exit()` no-cero (`run.py:152`, `154`) corren dentro de su árbol. Queda fuera porque **no acepta `--json`** (`cli.py:1858-1860`): no hay payload que emitir, y la invariante del canal no tiene alcance ahí. Conserva sus `sys.exit()` directos, y esa es la condición que lo autoriza y ninguna otra — darle `--json` reabriría el hueco sin que ningún invariante lo acuse, porque el que vigila las salidas está acotado a `cli.py`.

**Consecuencia de implementación: la terna de `_describe_provision_failure()`.** El par `(code, message)` que deja §4.1.6 pasa aquí a `(code, reason, message)`, que es exactamente la forma de `CliError`, así que el llamador (`cli.py:1491-1493`) se reduce a levantarla con lo que recibe. `reason` es el nombre estable de la distinción que la función ya calcula, y ponerle nombre es el objeto entero de D5.

**Coste, medido sobre el repositorio.** Es la partida más cara de las dos mitades, y hay que decirlo con su número. La forma común no existe —solo tres sitios improvisados con una clave `ok` propia—, así que se define entera y esos tres se normalizan. El coste real no es el payload sino el cambio de forma de las salidas: los `sys.exit()` no-cero de `cli.py` pasan a `raise CliError(...)` —**36 sitios**: los cuarenta `sys.exit()` del archivo menos el de la regla 1, que el paso 1.11 pasa al parser, y menos los dos `EXIT_OK` y el `EXIT_INTERRUPTED` de `main()`, que se quedan donde están; 26 de ellos entre `EXIT_INVALID_INPUT` y `EXIT_ERROR`, porque el decimotercer `EXIT_ERROR` vive en `daemon/run.py:154` y queda fuera del mecanismo— y `main()` gana el bloque de render. A cambio desaparece la propagación del modo `--json` a los helpers que no reciben `args` —`_remove_linux_path()` (`cli.py:904`) sale en `920` y es alcanzable desde `setup --remove-path`—, que con `CliError` simplemente levantan. Es un diff mayor y más mecánico, no un diseño nuevo por comando. En `tts-sidecar-narrator`, cero cambios: el único payload que consume del grupo `daemon` es `status.running` (`daemon.ts:22`), que la normalización de la clave `ok` no toca.

**Relación con §4.1.6.** Depende de él y va después. §4.1.6 fija qué significa cada entero y reclasifica los quince sitios impropios; este hallazgo cambia la forma en que esos enteros salen del proceso. Al revés no funciona: convertir a `CliError` antes del remapeo obligaría a tocar los mismos sitios dos veces. §4.1.6 se sostiene sin este —los códigos quedan correctos aunque la causa fina siga muriendo en stderr—, pero entonces la presión que produjo D2, D3, D4 y D5 sigue intacta.

**Relación con §4.4.2.** La refuerza y le añade alcance. §4.4.2 pide que la matriz de §2.4 cubra las reglas y no solo las filas, y que se añadan filas de `--json`. El payload de error hace que esas filas dejen de ser un detalle de formato: bajo `--json`, el fallo pasa a tener una forma observable y por tanto verificable en la matriz, con una fila por código emitido. Lo que §4.4.2 pedía por cobertura, este hallazgo lo vuelve obligatorio por contrato.

**Límites declarados.**

- **`reason` puede osificarse si alguien ramifica por él como si fuera un enum cerrado.** Lo acotan las reglas 1 y 2, pero solo si se documentan como **contrato de consumo** y no solo de emisión: `USAGE.md` debe decir explícitamente que un `reason` desconocido se trata como ausente. Sin esa frase, el segundo canal reproduce a los diez años el problema del primero.
- **Heredar de `BaseException` tiene un residuo, y es futuro.** Hoy no lo intercepta nada: cero `except BaseException` en `src/`, verificado, y los `finally` y los `with` de limpieza siguen corriendo igual que con `SystemExit`. Lo que queda abierto es que alguien envuelva un comando en `except BaseException` para «no dejar escapar nada» y rompa el canal en silencio — **ninguno de los invariantes lo detecta**: el de tipo mira la declaración y el de `sys.exit()` mira la salida, no quién la captura. Un tercer invariante que prohíba `except BaseException` en `cli.py` sería mecanizable si algún día hace falta, pero hoy vigilaría un caso que no existe.
- **La regla de promoción puede usarse para negar un código legítimo.** Lo acota que su árbitro sea único y comprobable (regla 3): una discusión futura se resuelve diciendo qué se invocaría a continuación, no sopesando importancia.
- **El payload de error no dice nada del canal de fallo del daemon**, que devuelve JSON por HTTP y no por stdout. Es superficie distinta y le corresponde a §4.3.x si algún día ramifica por causa; anotarlo aquí solo sirve para que no se confunda con un olvido.

**Toca.**

*Código.* El tipo `CliError(BaseException)` con la terna `(code, reason, message)` en `exit_codes.py` (el módulo que crea §4.1.6); la subclase de `ArgumentParser` que sobrescribe `error()` para levantar `CliError(EXIT_INVALID_INPUT, "usage_error", message)`, usada en el parser de `cli.py:1727` y en sus subparsers; el bloque de render en `main()` (`cli.py:1878-1898`), con `parse_args()` dentro de su handler y el paso intacto de cualquier `SystemExit` de código 0; la conversión de los 36 `sys.exit()` no-cero de `cli.py` a `raise CliError(...)`; el helper de serialización junto a `emit_json()` (`cli.py:56-69`); `cli.py:1260-1313` (la firma de `_describe_provision_failure()` pasa de `(code, message)` a la terna); y la normalización de los tres payloads con clave `ok` de `daemon start`/`stop`/`restart` (`cli.py:1651`, `1663`, `1680`), que hoy son la convención improvisada que este canal sustituye.

*Tests.* Un invariante de gobernanza que falle ante cualquier `sys.exit()` no-cero fuera de `main()` en `cli.py` —es lo que sostiene la invariante del canal sin depender de que nadie olvide un sitio—; uno que afirme `not issubclass(CliError, Exception)`, porque el anterior comprueba **la forma** de la salida y no su destino: pasaría en verde con las ocho salidas absorbidas por los `except Exception` de sus propios comandos; uno que compruebe que toda salida no-cero bajo `--json` emite el payload, **incluido el fallo de parseo de argparse**; y uno de degradación que verifique que un `reason` desconocido no rompe a un consumidor que solo lee `code`.

*Documento.* §3.2 gana el **paso 1.12**, inmediatamente después del 1.11 y sin renumerar los existentes. Su enunciado, para que no lo redacte el implementador: *«Paso 1.12 — abrir el canal de error. Declarar `CliError(BaseException)` en `exit_codes.py`, sustituir los 36 `sys.exit()` no-cero de `cli.py` por `raise CliError(code, reason, message)`, sobrescribir `error()` en una subclase de `ArgumentParser` para que el fallo de parseo entre por el mismo canal, añadir el render único en `main()` con `parse_args()` dentro de su handler, y normalizar los tres payloads con clave `ok` de `daemon start`/`stop`/`restart`. Refactor puro más una clave de payload nueva: no añade superficie de comandos.»* §2.8 gana la especificación del payload de error.

*Documentación pública.* Un apartado nuevo en `USAGE.md` para el **payload de error** con su forma, la enumeración de `reason` del 8 y las tres reglas de compatibilidad, incluida la de consumo: un `reason` desconocido se trata como ausente. La entrada de `CHANGELOG.md` de §4.1.6 recoge también la clave nueva.

*Verificación del paso 1.12.* Un test que compruebe el payload en toda salida no-cero bajo `--json`, con un caso explícito de flag desconocido y otro de grupo mutuamente excluyente violado —las dos rutas de argparse— comprobando que el `message` es el que argparse formatea y no un genérico; uno que fije en 0 `--help` en los tres niveles de parser y compruebe que **no** emite payload de error; uno de degradación ante un `reason` desconocido; el invariante que prohíbe salidas no-cero fuera de `main()`; y el que afirma que `CliError` no desciende de `Exception`.

### 4.2. Vocabulario y contrato

> **Resuelto y propagado a §2/§3.** Todas las subsecciones de §4.2 han sido aplicadas a las secciones 2 y 3 del documento.

#### 4.2.1. `speech` ya nombra otra cosa en este proyecto *(✅ propagado a §2/§3)*

**Problema, y es el hallazgo importante de esta revisión.** La consolidación eligió `speech` como término único del habla generada: directorio `data_root()/speech/`, grupo `speech list/play/remove`, flag `cleanup --speech`. La decisión se tomó para eliminar tres nombres de un mismo concepto, y en eso acierta. Lo que no se inventarió es que **`speech` ya significa otra cosa, incompatible, en el código y en la CLI de hoy**:

| Uso existente | Significado | Evidencia |
|---|---|---|
| `voice clone --speech/-s <archivo>` | Audio de **referencia** para el conditioning del T3 | `cli.py:1775-1777` |
| `voices/<voz>/speech.wav` | El mismo audio, ya en el registro | `voices.py:113-126`, `214`, `233-237` |
| `speech_audio` en el motor y el protocolo | El mismo audio, como parámetro | `synthesis.py`, `protocol.py` |

Es decir: hoy «speech» es la **entrada** de referencia de una voz, y el rediseño lo convierte además en la **salida** generada. Son conceptos opuestos en el flujo de datos, con el mismo nombre, dentro del mismo comando en un caso (`voice clone --speech` frente a `speech list`). Y quedan uno al lado del otro en el filesystem, bajo la misma raíz:

```
data_root()/
  voices/<voz>/speech.wav          ← entrada de referencia
  speech/<voz>/<etiqueta>.wav      ← salida generada
```

Un usuario que lea `USAGE.md` de arriba abajo encuentra `--speech` como «el audio que aportas» y `speech list` como «el audio que el sistema produjo». La homonimia no es cosmética: invierte el sentido.

**La homonimia vive en tres capas**, y el término adoptado las cierra a las tres:

| Capa | Colisión original | Resolución |
|---|---|---|
| Flags de la CLI | `voice clone --speech` (entrada) vs. grupo `speech` (gestión) | Rename a `--timbre-reference` / `--speech-reference`; los archivos también se renombran a `timbre-reference.wav` y `speech-reference.wav` |
| Filesystem | `voices/<voz>/speech.wav` (entrada) vs. `speech/<voz>/<etiqueta>.wav` (salida) | Directorio → `synthetic-speech/` |
| Interno | `speech_audio` como parámetro de entrada del motor | Unificación a `timbre`; `voice_audio`/`reference_audio` desaparecen |

**Propuesta:**

El qualifier `synthetic` nombra la propiedad que distingue la salida de la entrada —una la produce el sistema, la otra la aporta el usuario—. El par de flags de entrada pasa a `--timbre-reference` y `--speech-reference`, con los archivos renombrados a `timbre-reference.wav` y `speech-reference.wav` respectivamente, e interno unificado bajo `timbre`. La resolución es coherente en todas las capas: el género compartido es `speech` y la diferencia explícita en el qualifier marca la dirección del flujo. El orden de palabras respeta la convención del repo, con el núcleo al final (`--compute-backend`, `--voice-audio`, `--timbre-reference`). El término no es del todo nuevo: la primera versión del rediseño ya llamaba `generated-speech/` al directorio (§2.6), así que esto recupera esa intuición y la aplica a las tres capas.

La tabla siguiente resume la resolución completa del hallazgo:

| Elemento | Consolidado antes | Propuesto |
|---|---|---|
| Grupo | `speech list/play/remove` | `speech synthesize/list/play/remove` (dentro del namespace `speech`, absorbiendo `speak`) |
| Almacén | `data_root()/speech/` | `data_root()/synthetic-speech/` |
| Flag de `cleanup` | `--speech` | `--synthetic-speech` |
| Clave del payload de listado | `clips` | `synthetic_speech` |
| Flag de entrada `voice clone` | `--reference/-r` | `--timbre-reference` (`-t`) |
| Flag de entrada `voice clone` | `--speech/-s` | `--speech-reference` (`-s`) |
| Archivo de voz en disco | `reference.wav` | `timbre-reference.wav` |
| Archivo de habla en disco | `speech.wav` | `speech-reference.wav` |
| Parametric interno del timbre | `voice_audio` / `reference_audio` | `timbre` |

**Coste**: con la absorción, el grupo `speech` no paga el qualifier —`speech list` son 11 caracteres, igual que `voice list`. El qualifier `synthetic` vive en el directorio (`synthetic-speech/`) y en el flag de `cleanup` (`--synthetic-speech`), que es el flag más largo del repo. Se acepta porque ambos son operaciones de gestión —no la ruta caliente, que sigue siendo `speech synthesize`— y porque la alternativa es una homonimia permanente en la superficie más pública del proyecto. El rename de files y flags de `voice clone` se paga una sola vez en el movimiento 1, sin coste de migración de datos —sólo reempaquetado de voces de fábrica y actualización de `_is_valid_voice_dir`.

**Tres alternativas descartadas.**

**(A) `clip`.** Era la recomendación de esta revisión antes de la consolidación, y falla por una razón que solo se ve al mirar la fila 1 de la matriz de §2.4: **sin `--label` no se persiste nada**. `clip` nombra el artefacto persistido —directorio `clips/`, flag `--clips`, clave `clips`—, así que un grupo nominal llamado `clip` prometería un recurso que la invocación por defecto, la del integrador, no crea. `synthetic-speech` nombra el dominio y no el artefacto, así que es cierto en las dos ramas de la matriz. Coste adicional: el verbo y el recurso no compartirían palabra, porque `speech synthesize` produciría «clips».

**(B) Conservar `speech` y declarar la homonimia** en `USAGE.md` y en el help, igual que §2.2 declara la divergencia de `-n`. Coste de implementación cero; coste de lectura permanente. Su punto débil decisivo es que hace depender el significado de la **ausencia** de un qualifier —`speech/` sin marca significaría «generado» y `speech-reference` «aportado»—, mientras que con `synthetic` cada término es inequívoco por sí solo.

**(C) `generated-speech` como nombre del grupo independiente.** La primera versión del rediseño ya usaba este nombre para el directorio. Comparte morfema con `generate-speech`, evitando la divergencia de (A). Pero introduce una trampa de tipeo —`generated` vs `generate` difieren en una letra— y en tab-completion `generated-speech` y `generate-speech` se confunden por prefijo común. Su desventaja real es una trampa de escritura, no un argumento de homonimia.

**Por qué decidirlo ahora sale barato.** Nada está implementado, así que el renombre costó una búsqueda y reemplazo en este documento. Después habría costado el directorio de datos de todos los usuarios, el nombre de un grupo de comandos y una clave de payload bajo `cli.SCHEMA_VERSION` — que además ya sube a `"2"` en este mismo rediseño, así que el cambio tampoco cuesta en versionado.

**Aplicado en**: §2.1, §2.2 (lista de divergencias gana `-t`), §2.4 (segunda matriz), §2.5 (entera), §2.6, §2.7, §2.8 (payloads), §3.3 pasos 2.1, 2.5 y 2.6. La absorción del namespace está decidida, así que el grupo es `speech synthesize/list/play/remove` y el qualifier `synthetic` marca la salida en las tres capas. El frente CLI de la homonimia se cierra con el rename de los flags y archivos de `voice clone`.

#### 4.2.2. La clave `clips` del payload viola la regla que §2.6 se impuso *(✅ propagado a §2/§3)*

**Problema.** §2.6 declaraba que «clip» sobrevive *«solo como palabra en prosa, nunca como identificador»*, y §2.8 definía el payload de listado como `{"clips": [...]}`. Una clave JSON bajo `cli.SCHEMA_VERSION` es un identificador, y de los más difíciles de cambiar: forma parte del contrato legible por máquina.

**Propuesta.** La clave pasa a `synthetic_speech`: snake_case como el resto del contrato (`t3_time`, `created_at`, `schema_version`) y con el precedente de `voice list --json`, que emite `{"voices": [...]}` (`cli.py:565`) — la clave del listado es el nombre del recurso. «clip» desaparece del documento como identificador y como palabra en prosa, donde lo sustituye «locución» (§2.6). No depende de la absorción: `synthetic_speech` es el snake_case de `synthetic-speech` sin importar si el grupo vive en el namespace `speech` o como comando independiente.

**Aplicado en**: §2.6, §2.8 (payload de `speech list`).

#### 4.2.3. `removed` tendría dos tipos distintos en dos payloads *(✅ propagado a §2/§3)*

**Problema.** §2.8 define `speech remove --json` como `{"voice", "label", "removed"}`, donde `removed` es booleano. Pero `cleanup --json` ya emite `removed` como **lista de rutas** (`cli.py:1543-1546`). La misma clave con dos tipos en el mismo contrato de esquema es justo lo que un consumidor tipado no puede manejar, y el proyecto tiene una sola `cli.SCHEMA_VERSION` para ambos.

**Propuesta.** Eliminar el campo. El exit code ya transporta la información: 0 = se borró, 3 = no existía. El payload queda `{"voice", "label"}`, simétrico con `speech play`. Si se quiere un campo, que sea `deleted` — **no `path`**: §4.1.5 excluye la ruta de los payloads del grupo por criterio, y la locución no es excepción porque tiene `(voz, etiqueta)`. A diferencia del residuo de §4.2.4, este no llegó a §2/§3: el campo se eliminó, así que la mención a `path` solo vivía aquí.

**Toca**: §2.8 (una línea).

#### 4.2.4. El validador generalizado dirá «Nombre de voz inválido» para una etiqueta *(✅ propagado a §2/§3)*

**Problema.** §2.6 propone generalizar `_validate_voice_name` a validador de segmento de ruta y que voz y etiqueta lo invoquen. La función levanta `ValueError(f"Nombre de voz inválido: {name!r}. Usa solo letras, números, punto, guion y guion bajo…")` (`voices.py:38-42`). Generalizada tal cual, `speech synthesize --label "mi saludo"` responde **«Nombre de voz inválido: 'mi saludo'»**, culpando al flag equivocado.

**Propuesta.** Parametrizar el sustantivo en la firma (`_validate_path_segment(value, kind="voz" | "etiqueta")`) y usarlo en el mensaje. El exit code sigue siendo el mismo en ambos casos: **2**, uso incorrecto. Es una línea, pero sin ella el mensaje de error más frecuente del flag más usado del rediseño apunta a otra cosa.

> **Residuo de §4.1.6 en una subsección ya sellada.** Este hallazgo se propagó a §2/§3 cuando el código era 4, así que §2/§3 arrastran ese 4 pese al sello. La propagación de §4.1.6 tiene que barrerlos también: el sello de §4.2 no garantiza la numeración, solo el vocabulario.

**Toca**: §2.6, §3.3 paso 2.1.

#### 4.2.5. El recuento de grupos de §2.1 está mal *(✅ propagado a §2/§3)*

**Problema.** §2.1 cierra con: *«nueve comandos de nivel superior (…), dos de ellos grupos nominales de gestión»*. Son **tres**: `voice`, `speech` y `daemon`, los tres con sub-acciones y sin acción propia. El propio documento lo dice en otra parte al citar el patrón mixto del repo.

**Propuesta.** Corregir a tres.

**Toca**: §2.1 (una palabra).

### 4.3. Huecos de especificación

#### 4.3.1. No se declara qué archivo determina que una etiqueta existe

**Problema.** Desde que cada locución son **dos** archivos (`<etiqueta>.wav` y `<etiqueta>.json`), «la etiqueta existe» dejó de ser una pregunta trivial, y de ella dependen dos comportamientos del contrato: la colisión sin `--force` (§2.4 fila 7) y el exit 3 de `speech play` / `speech remove` (§2.4, segunda matriz). Ninguna sección dice cuál de los dos archivos manda.

No es teórico: §2.6 escribe el sidecar **antes** del WAV, así que una interrupción entre los dos `os.replace` deja un sidecar huérfano. Si la existencia se decide por el sidecar, esa etiqueta queda «ocupada» sin audio y `--force` se vuelve obligatorio para una etiqueta que no tiene locución; si se decide por el WAV, el huérfano es basura inocua pero nadie lo borra, porque `speech list` enumera los `.wav`.

**Propuesta.** Declarar que **el `.wav` es el recurso de registro** y el sidecar metadatos derivados: la existencia, la colisión y el exit 3 se deciden por el WAV. Añadir que `speech remove` borra ambos archivos si están, de modo que un huérfano sea removible por su etiqueta aunque `speech list` no lo muestre.

**Toca**: §2.6 (dos frases), §3.3 paso 2.1.

#### 4.3.2. La cancelación por `EOFError` invierte la polaridad de su precedente

**Problema.** §2.2 y §2.4 fila 3 establecen que, con stdin cerrado, el bucle trata el `EOFError` como cancelación: exit **0**, sin persistir. El molde es `cmd_cleanup`, y ahí la decisión es correcta porque cancelar significa **no se destruyó nada**: el estado seguro es no actuar.

En `speech synthesize` la polaridad es la opuesta. El usuario pidió `--label`, es decir pidió un artefacto; cancelar significa **el artefacto que pediste no se creó**, y sale 0. Un script de CI que olvide `--yes` obtiene éxito y ninguna locución, y no tiene forma programática de notarlo: bajo `--json` el caso es inalcanzable (regla 2), así que la única señal es texto en stderr.

La decisión de no detectar TTY es correcta y no está en cuestión. Lo que está mal elegido es qué hacer cuando la interacción resulta imposible.

**Propuesta**, en orden de preferencia:

1. **Tratar el `EOFError` como «acepta la primera toma»**, es decir como `--yes` implícito. Justificación: `--label` *es* la petición de persistir, y el bucle es solo la revisión; si la revisión es imposible, se honra la petición. Con esto **exit 0 significa «el artefacto existe»** para todo consumidor programático, que es la propiedad que un orquestador necesita: la única salida 0 sin artefacto que queda es la cuarta opción del bucle de §4.1.1, y llegar a ella exige una respuesta interactiva. Y sigue sin haber ninguna detección de entorno: se reacciona a un EOF real, no se sondea un TTY.
2. **Exit 2 remitiendo a `--yes`.** Explícito y accionable: el entero es señal programática con `--json` o sin él, que es justo lo que le falta al exit 0 actual. Pero sigue haciendo que el código de salida dependa del entorno, que es la mitad del argumento por el que se rechazó el gate por TTY, y eso es lo que decide entre las dos. El canal de error de §4.1.7 no la alcanza: se emite solo bajo `--json`, donde el bucle —y con él el `EOFError`— es inalcanzable por la regla 2.
3. Conservar el exit 0 y documentar que `--label` en automatización exige siempre `--yes` o `--no-play`.

Recomiendo la 1.

**Toca**: §2.2, §2.4 (fila 3), §2.8 (tabla de exit codes), §3.3 paso 2.4.

#### 4.3.3. La voz inexistente no está mapeada en el grupo `speech`

**Problema.** La segunda matriz de §2.4 cubre etiqueta inexistente (3) y etiqueta ilegal (2), pero no dice qué pasa cuando la **voz** no existe. `speech list --voice noexiste` es el caso ambiguo: ¿exit 0 con lista vacía, o exit 3? Sin declararlo, un usuario que se equivoca al escribir el nombre de la voz recibe «no hay locuciones» y concluye que se perdieron.

Nota menor de la misma tabla: la fila «cualquier sub-acción con etiqueta ilegal» incluye `speech list`, que no toma `--label`.

**Propuesta.** Declarar que las tres sub-acciones validan la voz contra `voices.list_voices()` y salen **3** si no está, de modo que «voz mal escrita» nunca se disfrace de «sin resultados». Y acotar la fila de etiqueta ilegal a `play` y `remove`.

**Toca**: §2.4 (segunda matriz), §2.5.

#### 4.3.4. `cleanup --voices` no dice qué pasa con las locuciones de la voz de fábrica

**Problema.** §2.7 dice que `--voices` *«además de borrar voces, arrastra sus locuciones»*. Pero `remove_voice` solo opera sobre voces de **usuario** —las de fábrica son de solo lectura (`voices.py:162-174`)—, así que `default` sobrevive a `cleanup --voices`. ¿Sus locuciones también? El texto no lo dice, y la respuesta importa: `synthetic-speech/default/` es probablemente el namespace más poblado, porque `default` es la voz por defecto de `speech synthesize`.

**Propuesta.** Declarar que `--voices` arrastra **solo los namespaces de las voces que efectivamente borra**, y que `synthetic-speech/default/` cae únicamente con `--synthetic-speech` o `--all`. Es coherente con el criterio del propio flag —las locuciones se van con su voz— y con que la voz de fábrica no se va nunca.

**Toca**: §2.7 (una frase), §3.3 paso 2.6 (un test).

#### 4.3.5. El sidecar es un segundo formato en disco sin política de compatibilidad

**Problema.** §2.6 introduce `<etiqueta>.json` con `text`, `voice` y `created_at`, y §2.8 re-emite esos tres campos en el payload de listado, que sí está bajo `cli.SCHEMA_VERSION`. Quedan por declarar dos cosas:

- **El formato de `created_at`.** No se especifica (¿epoch, ISO 8601, con o sin zona?). Es un campo de un contrato legible por máquina.
- **Quién gobierna la forma del sidecar.** Si un día se le añade un campo, cambia también la forma del payload; si el sidecar tuviera su propia versión, el proyecto pasaría a tener **tres** versiones de esquema, y §1.3 y §2.8 se toman el trabajo de mantener claras las dos que ya hay.

**Propuesta.** Declarar `created_at` en **ISO 8601 UTC** y que el sidecar es **formato interno sin versión propia**: su única superficie estable es el payload de `--json`, gobernado por `cli.SCHEMA_VERSION`. Un lector que encuentre un campo desconocido lo ignora, igual que hace `ProtocolModel` con `extra="ignore"` (`protocol.py:37-45`).

**Toca**: §2.6, §2.8.

#### 4.3.6. Ningún paso del plan es dueño del refactor de `_validate_voice_name`

**Problema.** §2.6 dice que *«conviene generalizar `_validate_voice_name` a validador de segmento de ruta»*, y §3.3 paso 2.1 lo da por hecho al hablar de *«el validador de segmento generalizado»*. Ningún paso lo ejecuta. No es un detalle de redacción: el refactor **modifica una función que las voces ya usan** en `voice clone`, `voice remove` y `voice_dir`, así que es el único cambio del movimiento 2 que puede romper comportamiento existente, y está descrito como si fuera preexistente.

**Propuesta.** Darle un paso propio **en el movimiento 1**, donde encaja por naturaleza (limpieza sin feature nueva), con dos criterios de verificación: que los tests de nombres de voz pasen **sin más cambio que el código de salida** —§4.1.6 mueve el nombre ilegal de 4 a 2, así que exigir tests intactos volvería el criterio imposible de cumplir y haría creer al implementador que rompió algo—, y que el mensaje de error quede parametrizado (§4.2.4). El movimiento 2 lo consume ya hecho, que es lo que el paso 2.1 asume.

**Toca**: §3.2 (**paso 1.14** nuevo, al final), §3.3 paso 2.1, §2.6.

> **Numeración acordada con §4.1.6, §4.1.7 y §4.1.4.** Los cuatro hallazgos crean un paso al final de §3.2 —el remapeo del contrato de salida, el canal de error, el desacople de la emisión y este refactor—. Reparto fijado: **§4.1.6 el 1.11, §4.1.7 el 1.12, §4.1.4 el 1.13 y este el 1.14**. Este va último por seguridad ante descarte: es el único de prioridad media, luego es el más probable de aplazarse y su caída deja la lista sin huecos. No hay conflicto de contenido con ninguno de los otros tres; conviene propagarlos en el mismo movimiento de todos modos.

### 4.4. El plan de trabajo

#### 4.4.1. El bump de `cli.SCHEMA_VERSION` está un paso después de su causa

**Problema.** §3.2 pone el bump de `cli.SCHEMA_VERSION` a `"2"` en el **paso 1.7**, junto al renombre. Pero §2.8 justifica ese bump por la pérdida de la clave `"output"` del payload, y esa pérdida ocurre en el **paso 1.1**. Entre ambos pasos el payload ya cambió de forma y sigue anunciando `"1"`.

Es la misma clase de problema que §3.4 se ocupa de declarar para el WARN de `setup`, y aquí es más grave porque afecta a un campo cuyo único propósito es que un consumidor detecte cambios de forma.

**Propuesta.** Mover el bump al paso 1.1, que es donde el payload cambia, y dejar el 1.7 como renombre puro. Coste cero, y el paso 1.1 ya toca el bloque del payload.

**Toca**: §3.2 pasos 1.1 y 1.7.

#### 4.4.2. El criterio de verificación del movimiento 2 deja fuera cuatro de las siete reglas

**Problema.** §3.3 cierra con: *«cada fila de las dos matrices de §2.4 tiene un test»*. Como criterio de cobertura es engañoso, porque las matrices no contienen las reglas 1-4 de §2.3 ni el gate `--json` de la regla 2 — la matriz de `speech synthesize` no tiene ninguna fila con `--json`, a diferencia de la del Punto A en §1.2, que sí las tiene. Con el criterio tal como está escrito, la regla que sustituye a «`--json` requiere `--output`» se implementaría sin test.

**Propuesta.** Tres cambios:

1. Reformular el criterio: *«cada fila de las matrices de §2.4 **y cada regla de §2.3**»*.
2. Añadir a la matriz de §2.4 las filas de `--json`, por paridad con §1.2 y para que las dos matrices del documento sean comparables.
3. Extender la cobertura al canal de error de §4.1.7: cada salida no-cero de los comandos nuevos necesita un test que fije el **payload** —`code` y `message`—, no solo el entero. Antes de §4.1.7 un fallo solo tenía código y el criterio por filas lo agotaba; ahora tiene forma, y una forma sin test es contrato sin verificar. El par `(exit code, reason)` es cláusula de futuro: por la invariante de §4.1.7 el `reason` hoy solo existe en el 8, que ninguno de estos comandos emite, pero en cuanto el 6 o el 7 nombren subcausas la matriz debe fijarlas.
4. Extender la cobertura a las **claves exactas** de los cuatro payloads `--json` del grupo, por §4.1.5. Ni las matrices de §2.4 ni las reglas de §2.3 alcanzan a la forma del payload: las primeras cubren combinaciones de flags, las segundas validación de entrada. Sin esta pieza, el criterio reformulado del punto 1 **sigue dejando fuera** lo que §4.1.5 decidió, y añadir la ruta por descuido se colaría sin romper nada. El test fija las claves, no su contenido.

**Toca**: §2.4, §3.3 (criterio de verificación).

### 4.5. Impacto resumido

| # | Hallazgo | Prioridad | Toca |
|---|---|---|---|
| 4.2.1 | `speech` ya nombra la entrada de referencia; adoptar `synthetic` como qualifier de la salida | **Alta** | ✅ Propagado a §2/§3 |
| 4.2.2 | Clave del payload de listado → `synthetic_speech` | **Baja** | ✅ Propagado a §2/§3 |
| 4.1.2 | Regla 7 (`--yes` requiere `--label`) y matizar la afirmación de §2.4 | **Alta** | §2.3, §2.4, §3.3 paso 2.3 |
| 4.1.4 | El paso 2.2 no está resuelto por el 1.1: es un hueco de propiedad, y el trabajo no es de una línea (el despacho tiene tres ramas y solo la directa emite ya desde el llamador). Paso propio en el movimiento 1 que converge las tres ramas a una cola única de emisión, renombra `_emit_audio` a `_play_audio` y deja el paso 2.2 como puntero **sin renumerar**, porque seis hallazgos citan pasos del movimiento 2 por número. El paso nuevo va al final de §3.2 por el mismo motivo: insertarlo antes del 1.7 rompería cinco citas de §4.4.1 y §3.4. Es el 1.13, dentro del reparto 1.11 (§4.1.6) / 1.12 (§4.1.7) / 1.13 / 1.14 (§4.3.6) | **Alta** | §3.2 (paso 1.13 nuevo, al final), §3.3 paso 2.2 |
| 4.3.2 | `EOFError` → aceptar la primera toma, para que exit 0 signifique «existe» | **Alta** | §2.2, §2.4, §2.8, §3.3 paso 2.4 |
| 4.1.6 | Sanear el contrato de salida completo bajo un criterio generador de dos tiempos (clase de causa + admisión por la siguiente llamada del consumidor): 2 = uso incorrecto, 4 = modelo no provisionado, 6 = conflicto de estado (absorbe el código huérfano del daemon), 7 = operación no aplicable, 8 = precondición de entorno incumplida; la colisión con argparse desaparece, los quince sitios impropios de `EXIT_INVALID_INPUT` y `EXIT_ERROR` se reclasifican, la exclusión mutua se unifica en un solo mecanismo —el grupo declarativo de argparse, en los dos sitios que hoy divergen— y las constantes ganan un módulo propio con invariantes que impiden reincidir en D2 | **Alta** | `exit_codes.py` (nuevo), `cli.py:41-49` (reexporta) y quince sitios, `cli.py:266-270` (grupo mutuamente excluyente), `voices.py:203-204`, `cli.py:1260-1313` (firma de `_describe_provision_failure`), `daemon/run.py:24,30-33,152`, `tests/test_cli.py:1425,2425`, `tests/test_daemon_run.py:117`, invariantes de gobernanza, §1.2, §1.3, §2.3, §2.4, §2.8, §3.2 (paso 1.11 nuevo, al final), `USAGE.md`, `CHANGELOG.md` |
| 4.1.7 | El fallo no tiene forma legible por máquina y tres comandos ya improvisaron la suya. La causa fina que el entero no puede llevar pasa a un payload de error aditivo bajo `--json`, sostenido por un tipo `CliError(BaseException)` con render único en `main()` y un `error()` sobrescrito que mete también el fallo de parseo de argparse por el canal, con su mensaje; tres reglas de compatibilidad mantienen el canal ampliable sin ruptura | **Alta** | `CliError` en `exit_codes.py`, subclase de `ArgumentParser` (`cli.py:1727`), render en `main()` (`cli.py:1878-1898`), 36 `sys.exit()` de `cli.py`, helper junto a `emit_json` (`cli.py:56-69`), `cli.py:1260-1313` (terna), `cli.py:1651,1663,1680` (clave `ok`), invariantes de canal, §2.8, §3.2 (paso 1.12 nuevo, al final), `USAGE.md`, `CHANGELOG.md` |
| 4.1.1 | Cuarta opción del bucle —descartar y salir con 0— y saneo de la fila de exit 0 de §2.8, que nombra cuatro estados donde hay dos: «rechazar» pasa a «descartar» y nombra la opción nueva, y «cancelar» cae por redundante con el `EOFError` que la produce. Acota el «siempre» de §4.3.2, con la que comparte tres destinos de escritura | Media | §2.2, §2.4, §2.8, §3.3 paso 2.4 |
| 4.1.3 | Partir la regla 3 en 3a (exit 2) y 3b (exit 4) — **superado por §4.1.6**: tras el remapeo las dos mitades salen con 2 y no hay nada que partir | — | Nada |
| 4.3.1 | El `.wav` es el recurso de registro; el sidecar es derivado | Media | §2.6, §3.3 paso 2.1 |
| 4.3.6 | Dar un paso propio al refactor de `_validate_voice_name`, en el movimiento 1 | Media | §3.2 (paso 1.14 nuevo, al final), §3.3 paso 2.1, §2.6 |
| 4.4.1 | Mover el bump de `cli.SCHEMA_VERSION` al paso 1.1 | Media | §3.2 |
| 4.4.2 | Cobertura por reglas, no solo por filas; añadir filas de `--json`; más el payload de error de §4.1.7 —`code` y `message`, con el `reason` como cláusula de futuro— y las claves exactas del payload de §4.1.5 | Media | §2.4, §3.3 |
| 4.2.4 | Parametrizar el sustantivo del mensaje del validador | Media | ✅ Propagado a §2/§3 |
| 4.3.3 | Voz inexistente → exit 3 en las tres sub-acciones del grupo | Media | §2.4, §2.5 |
| 4.3.4 | `--voices` arrastra solo lo que borra; el namespace de fábrica cae con `--all` | Baja | §2.7, §3.3 paso 2.6 |
| 4.3.5 | `created_at` en ISO 8601 UTC; el sidecar no lleva versión propia | Baja | §2.6, §2.8 |
| 4.2.3 | Eliminar `removed` del payload de borrado (choca con `cleanup --json`) | Baja | ✅ Propagado a §2/§3 |
| 4.1.5 | La ruta **no** se emite: un payload transporta una ruta solo cuando el recurso no tiene otro nombre en el contrato, y la locución tiene `(voz, etiqueta)`. Decide la asimetría de reversibilidad: añadir la clave después es aditivo, quitarla es incompatible | Baja | §2.8 (desaparece la frase que reabre el punto y **entra el criterio como regla explícita**), §3.5 (un punto menos) |
| 4.2.5 | Son tres grupos nominales, no dos | Baja | ✅ Propagado a §2/§3 |

**Orden recomendado.** Las cinco subsecciones de §4.2 están resueltas y propagadas a §2/§3; **§4.1.6**, **§4.1.7**, **§4.1.5**, **§4.1.4** y **§4.1.1** tienen estrategia aprobada y están pendientes de propagar. Ahora corresponde resolver las restantes en orden de prioridad:

1. **§4.1.2** (alta) — la regla 7 de `--yes`/`--label`, que elimina un no-op silencioso. Va detrás de §4.1.6 no por prioridad sino por dependencia: aquella fija el vocabulario de códigos que otros siete hallazgos usan en su propia especificación (ver abajo), y ambas compiten por la frase de cierre de §2.4.
2. **§4.3.2** (alta) — el tratamiento del `EOFError`, que afecta la corrección semántica del rediseño.
3. **§4.3.1** y **§4.3.6** (medias) — el `.wav` como recurso de registro y el refactor del validador. §4.3.6 necesita un paso propio en el movimiento 1, ya numerado como 1.14 dentro del reparto 1.11 (§4.1.6) / 1.12 (§4.1.7) / 1.13 (§4.1.4) / 1.14; conviene propagar los cuatro juntos.
4. **§4.4.1 y §4.4.2** (restantes) — correcciones de orden y cobertura, pendientes de decisión.

**Dependencias de §4.1.6 y §4.1.7.** Ocho hallazgos cambian de contenido al adoptarlas, repartidos según de cuál dependan: el remapeo del entero, o el canal de error. Todos están ya ajustados en §4 bajo la numeración nueva declarada en la cabecera de esta sección; lo que queda es propagarlos a §2/§3 junto con las dos.

| Hallazgo | De | Qué le hace |
|---|---|---|
| 4.1.3 | 4.1.6 | Lo suprime: el motivo del hallazgo desaparece con el remapeo |
| 4.1.2 | 4.1.6 | Renumera su regla 7 (4 → 2) **y** compite por la misma frase de cierre de §2.4; §4.1.6 escribe primero |
| 4.2.4 | 4.1.6 | Renumera el error del validador (4 → 2) en una subsección ya sellada como propagada: §2/§3 arrastran el 4 |
| 4.3.3 | 4.1.6 | Renumera la etiqueta ilegal (4 → 2); su exit 3 no cambia |
| 4.3.6 | 4.1.6 | Invalida su criterio de verificación: los tests de nombres de voz sí cambian |
| 4.3.2 | 4.1.6 | Renumera su opción 2 (4 → 2). §4.1.7 comprobada y nula: su canal solo emite bajo `--json`, donde el bucle es inalcanzable. No cambia la recomendación |
| 4.4.2 | 4.1.7 | Amplía su cobertura al canal de error: el payload —`code` y `message`—, no solo el entero |
| 4.1.4 | 4.1.7 | Le fija el orden y el número: §4.1.6 toma el paso 1.11, §4.1.7 el 1.12 y aquel el 1.13. No hay conflicto de contenido —aquellas cambian cómo se sale con error, este cómo se emite en éxito— pero sí solape de región: el `sys.exit()` de `cli.py:353` está dentro del bloque `337-401` que §4.1.4 converge, y tras §4.1.7 es un `raise CliError` que abandona el despacho por excepción. Ir primero le simplifica la cola a §4.1.4 en vez de obligarla a rehacerla |

Dos aristas comprobadas y nulas: **§4.3.1** cita la fila 7 de §2.4 por referencia, no por número, así que no requiere edición; y **§4.4.1** no necesita un segundo bump de `cli.SCHEMA_VERSION`, porque añadir la clave `error` de §4.1.7 está cubierto por la regla de compatibilidad 2 de §1.3.

**Si solo se aplican cinco**, que sean las de prioridad alta: dos corrigen el contrato de salida (§4.1.6 sanea el mapa de códigos y con él la colisión del exit 2 de argparse; §4.1.7 le da forma legible por máquina al fallo), una elimina un no-op silencioso (§4.1.2) y dos arreglan contradicciones entre lo declarado y lo especificado (§4.1.4 mueve el desacople al movimiento 1; §4.3.2 cambia la polaridad del `EOFError`).

**Estado de los hallazgos**: 20 en total —§4.1.7 nace de partir §4.1.6, que acumulaba el remapeo del entero y el canal de error en un solo bloque— 5 resueltos y propagados a §2/§3 (§4.2.1–4.2.5), 5 con estrategia aprobada y pendientes de propagar (§4.1.6, §4.1.7, §4.1.5, §4.1.4, §4.1.1), 1 suprimido por §4.1.6 (§4.1.3) y 9 pendientes de decisión. Ninguno queda aparcado; el propósito de este documento es definir **todas** las modificaciones necesarias, así que un hallazgo abierto es una decisión pendiente, no algo excluido del alcance. Las notas de §1.2, §2.3 y §2.7 que antes declaraban defectos como fuera de alcance ahora remiten al hallazgo que los cierra.

**Lo que esta revisión no encontró.** El invariante de §3.1 se sostiene en todo el diseño: ninguna superficie nueva acepta rutas del llamador. El orden de los dos movimientos sigue siendo correcto y no invertible por las razones que §3.1 da. Y las eliminaciones del movimiento 1 están trazadas contra código real, con verificaciones ejecutables: es la parte más sólida del documento.

---

## 5. Rediseño del grupo `speech`

*(✅ decidido, pendiente de propagar a §2/§3)*

Esta sección **no es un hallazgo de §4**. Los de §4 critican a §2/§3 y proponen correcciones acotadas; esta sustituye §2.5 entera, añade una sub-acción a la CLI y retira tres flags. Se escribe aparte porque meterla en §4 la obligaría a disfrazarse de crítica y descuadraría los recuentos de §4.5.

Lo que decide §5 gobierna sobre lo que dicen §2 y §3, igual que §4. Donde §2 o §3 contradigan esta sección, manda esta sección.

> **Códigos de salida.** Se usan los de §4.1.6, como en el resto de §4: **2** uso incorrecto, **3** no encontrado, **5** daemon inalcanzable, **6** conflicto de estado.

### 5.1. El defecto de fondo

`speak` —y su heredero `speech synthesize` tal como lo especifica §2— es un solo comando cuyo comportamiento lo deciden los flags: con `--label` persiste, sin él es efímero; con `--no-play` calla, sin él suena; con `--yes` no pregunta. Esos tres flags no son opciones de una misma acción: **seleccionan acciones distintas**.

De ahí sale casi todo lo que §4 encontró en el eje de interacción, y no como defectos independientes sino como consecuencias del mismo reparto:

- la regla 5 de §2.3, que exige `--label` junto a `--no-play` para que la invocación no quede con efecto cero;
- la derivación de §2.2:183, que hace que `--no-play` implique `--yes`;
- el cuarto estado del eje, que hay que declarar imposible y justificar;
- el no-op silencioso de `--yes` sin `--label` (§4.1.2);
- y la pérdida de trabajo del `EOFError` con stdin cerrado (§4.3.2).

Los cinco existen porque un comando carga con dos responsabilidades —**producir un artefacto** y **emitir sonido**— y las reparte con flags. La corrección no es añadir reglas que tapen las combinaciones malas: es **partir el comando por la responsabilidad**, y que las combinaciones malas dejen de ser expresables.

### 5.2. La superficie: cinco sub-acciones

| Sub-acción | Responsabilidad | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda | **sí** | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech play` | Reproduce una locución guardada | no | **no** |
| `speech list` | Lista las locuciones guardadas | no | no |
| `speech remove` | Borra una locución guardada | no | no |

`synthesize` y `say` son gemelos: misma síntesis, distinto destino —disco o parlantes—. `play`, `list` y `remove` son la gestión del almacén.

**El reparto es legible porque el nombre de cada sub-acción declara su costo.** Sintetizar paga GPU y puede exigir provisión del modelo; reproducir paga una lectura de archivo. Hoy `speak` cobra una cosa u otra según los flags, y desde fuera no hay forma de saber cuál pasó. La homología con `voice` de §2.5 se extiende con la fila nueva:

| Registro de voces | Almacén de habla sintética |
|---|---|
| `voice list` | `speech list` |
| `voice clone` | `speech synthesize` |
| `voice remove` | `speech remove` |
| — | `speech play` |
| — | `speech say` |

**`say` no es superficie nueva: es el hueco que deja `--label` requerido.** Hoy `speak --text T` sin etiqueta sintetiza y reproduce sin guardar, y ese camino tiene usuarios —es la fila 1 de la matriz de §2.4, declarada allí como «el contrato del integrador de narración»—. Con `--label` obligatorio en `synthesize`, ese caso necesita un comando propio en vez de desaparecer.

### 5.3. Parámetros de cada sub-acción

| Sub-acción | Parámetros |
|---|---|
| `speech synthesize` | `--text/-t` **requerido** · `--label/-l` **requerido** · `--voice/-v` · `--play/-p` · `--force/-f` · `--json` · `--daemon`/`--no-daemon` |
| `speech say` | `--text/-t` **requerido** · `--voice/-v` · `--json` · `--daemon`/`--no-daemon` |
| `speech play` | `--label/-l` **requerido** · `--voice/-v` · `--json` |
| `speech list` | `--voice/-v` (filtro) · `--json` |
| `speech remove` | `--label/-l` **requerido** · `--voice/-v` · `--json` |

**`--voice/-v` es opcional en las cinco** y, si falta, usa la voz de fábrica `default`. Se conserva el nombre `--voice` en vez de `--voice-profile` por el mismo criterio que resolvió §4.2.1: el concepto ya se llama «voice» en `voice list`, `voice clone` y `voice remove`, y darle un segundo nombre en otro comando es la homonimia al revés —dos palabras para una cosa— con el mismo costo.

**Tres flags desaparecen**, y ninguno necesita sustituto:

| Flag | Por qué muere |
|---|---|
| `--no-play` | La reproducción deja de ser el comportamiento por defecto, así que no hay nada que silenciar |
| `--yes` | Sin bucle por defecto no hay pregunta que suprimir |
| El `--label` opcional | Pasa a requerido: es lo que `synthesize` existe para producir |

**`--label` requerido es el cambio de mayor efecto de esta sección.** Elimina de raíz la invocación con efecto cero, sin escribir ninguna regla —la rechaza argparse—, y hace vacías dos de las reglas de §2.3 (ver §5.6). También elimina la trampa de «previsualizo con un comando y guardo con otro»: como `synthesize` siempre persiste, nadie pierde la toma que acaba de oír.

**Dos notas de vocabulario**:

- **`--play` y la sub-acción `play` comparten palabra a propósito.** No es el caso de §4.2.1: allí una palabra nombraba dos cosas distintas; aquí nombra una sola —emitir audio por los parlantes— en los dos sitios donde ocurre.
- **La tercera divergencia de §2.2 se disuelve.** Allí se aceptaba que `-n` fuera `--no-play` en `speech synthesize` y `--name` en `voice clone` / `voice remove`, al precio de que el corto no tuviera significado único en la CLI. Retirado `--no-play`, `-n` queda libre y el precio no hay que pagarlo.

### 5.4. El bucle de `synthesize --play`

Sin `--play`, `synthesize` sintetiza, guarda y termina.

Con `--play`, reproduce la toma y pregunta. **Cuatro opciones**, que son las de §4.1.1:

| Opción | Efecto | Costo |
|---|---|---|
| Reproducir otra vez | Vuelve a sonar la misma toma | **Cero síntesis**: los bytes están en memoria |
| Aceptar y guardar | Persiste la toma que acabas de oír, y termina con 0 | Cero |
| Rechazar y regenerar | Sintetiza otra toma y vuelve a preguntar | T3+S3Gen, **nada** de la Etapa 1: los conditionals de una voz del registro están precomputados desde `26735cd` |
| Rechazar y descartar | Termina con 0 **sin guardar nada** | Cero |

**Cuándo persiste.** Sin `--play`, inmediatamente después de sintetizar. Con `--play`, solo al aceptar. Así «descartar» nunca es un borrado: es no haber escrito.

**Ctrl-D es el atajo de «descartar y salir».** Con terminal presente, cerrar la entrada en la pregunta es una forma legítima de abandonar, y mapea exactamente sobre la cuarta opción: exit 0, sin persistir. Es el único `EOFError` que queda alcanzable, y tiene significado propio en vez de ser un accidente (ver §5.6, regla 5).

**La colisión de etiqueta se comprueba antes de sintetizar.** Si la etiqueta está tomada y no hay `--force`, el comando sale con **6** sin gastar GPU. Comprobarla después obligaría a pagar la síntesis entera para descubrir que no se puede guardar, y con `--play` además a recorrer el bucle hasta «aceptar» para fallar ahí.

### 5.5. El despacho al daemon

Aplica a las dos sub-acciones que sintetizan, `synthesize` y `say`:

| Invocación | Qué hace |
|---|---|
| Sin flags | **Comprueba el daemon.** Si está activo, sintetiza por él; si no, carga el modelo al vuelo |
| `--no-daemon` | Fuerza la síntesis directa aunque el daemon esté activo |
| `--daemon` | **Exige** el daemon: si no está activo, sale con **5** en vez de degradar |

**No es superficie nueva: es la rama que ya existe, ascendida a comportamiento especificado.** El despacho de `cmd_speak` tiene hoy tres ramas (`cli.py:337-401`) y la autodetección es una de ellas (`358-373`), pero solo se alcanza cuando el llamador no dice nada, y ningún documento la declara. Con este cambio deja de ser un tercer camino no documentado y pasa a ser el único camino por defecto.

**`--daemon` cambia de significado, de selección a exigencia.** Con la autodetección por defecto, «usa el daemon» deja de ser algo que haya que pedir. Retirar el flag sin más dejaría al llamador sin forma de exigir la ruta rápida y **al código 5 sin ningún productor en la síntesis**: si la ausencia del daemon siempre degrada, nunca hay «daemon inalcanzable», solo una invocación más lenta. Un consumidor con presupuesto de latencia —el narrator es el caso previsto— necesita poder decir «prefiero fallar a esperar a que cargue el modelo». Que hoy no lo pida no valida retirarlo, por el corolario de §4.1.6: *la ausencia de consumidor actual no valida ninguna clasificación*.

Con los dos flags conservados, la exclusión mutua entre ellos sigue teniendo sentido —«exige daemon» y «prohíbe daemon» se contradicen— y la regla 1 se mantiene, resuelta por el grupo declarativo de argparse que decide §4.1.6.

### 5.6. Reglas de validación

Quedan **cinco**, todas con exit 2:

1. **`--daemon` y `--no-daemon` son excluyentes.** Resuelta por el grupo mutuamente excluyente del parser (§4.1.6), no por una comprobación a mano.
2. **`--json` es incompatible con `--play`.** El bucle escribe la pregunta y lee la respuesta por los canales estándar, y contaminaría el payload.
3. **`--text` no vacío ni solo espacios.**
4. **`--text` no excede `MAX_TEXT_LENGTH`.**
5. **`--play` exige terminal en la entrada estándar.** Si no la hay, se rechaza **antes de sintetizar**.

**Mueren tres reglas, y ninguna necesita sustituto**:

| Regla | Por qué muere |
|---|---|
| §2.3 regla 5 — `--no-play` requiere `--label` | El flag no existe |
| §2.3 regla 6 — `--force` requiere `--label` | `--label` es requerido, así que la condición no puede incumplirse |
| §4.1.2 regla 7 — `--yes` requiere `--label` | El flag no existe; el hallazgo se suprime |

Y la regla 2 se simplifica: hoy es condicional —«`--json` con `--label` exige `--yes` o `--no-play`»— y pasa a ser una incompatibilidad directa entre dos flags, sin antecedente.

**La regla 5 es de otra clase que las cuatro anteriores**, y conviene declararlo. Las cuatro primeras miran los flags; la quinta mira el entorno. §2.2 declara que el bucle *«no comprueba si hay TTY: arranca siempre»* y §4.3.2 afirma que *«la decisión de no detectar TTY es correcta y no está en cuestión»*: **las dos frases quedan derogadas por esta regla**, y hay que reescribirlas al propagar.

La derogación no contradice el motivo original. Aquella decisión protegía el comportamiento **por defecto** de depender del entorno: con el bucle activándose solo por `--label`, una comprobación de TTY habría hecho que la misma línea de comandos significara cosas distintas según dónde corriera. Con `--play` explícito eso ya no puede ocurrir: la comprobación no altera ningún default, solo rechaza antes una invocación que iba a fallar igual. Lo único que se pierde es alimentar las respuestas del bucle por una tubería —`echo "aceptar" | tts-sidecar speech synthesize … --play`—, un caso marginal cuyo precio, de conservarlo, sería pagar una síntesis y una reproducción completas antes de fallar.

### 5.7. Matrices de comportamiento

**`speech synthesize`**:

| Invocación | Genera | Reproduce | Guarda | Exit |
|---|---|---|---|---|
| `-t T -l L` *(L libre)* | sí | no | sí | 0 |
| `-t T -l L -p` *(L libre, con terminal)* | sí | sí, en el bucle | al aceptar | 0 |
| `-t T -l L -f` *(L existe)* | sí | no | sí, sobrescribe | 0 |
| `-t T -l L -p -f` *(L existe)* | sí | sí, en el bucle | al aceptar, sobrescribe | 0 |
| `-t T -l L` *(L existe, sin `-f`)* | — | — | — | **6** |
| `-t T -l L -p` *(sin terminal)* | — | — | — | **2** |
| `-t T -l L -p --json` | — | — | — | **2** |
| `-t T` *(sin `-l`)* | — | — | — | **2** |
| `-t T -l L --daemon` *(daemon caído)* | — | — | — | **5** |

Fila 1 es el camino de automatización, y ya no necesita ningún flag: sintetizar y guardar **es** lo que el comando hace. Era la fila 5 de §2.4, que aquella matriz llamaba «generación headless de primera clase» y que exigía `--no-play` para conseguirse.

**Resto del grupo**:

| Invocación | Genera | Reproduce | Exit |
|---|---|---|---|
| `speech say -t T` | sí | sí | 0 |
| `speech say -t T --daemon` *(daemon caído)* | — | — | **5** |
| `speech list` *(todas las voces)* | no | no | 0 |
| `speech list -v V` *(filtrado)* | no | no | 0 |
| `speech play -l L` *(L existe)* | no | sí | 0 |
| `speech remove -l L` *(L existe)* | no | no | 0 |
| `speech play -l L` / `speech remove -l L` *(L no existe)* | — | — | **3** |
| `play`, `remove` o `synthesize` con etiqueta ilegal | — | — | **2** |

`speech say` es la única fila del grupo que genera, y la única que puede exigir provisión del modelo. Es la contrapartida de que `play`, `list` y `remove` no lo necesiten.

### 5.8. Payloads `--json`

**Los cuatro payloads existentes no cambian**: quedan como los especifica §2.8 con las correcciones ya propagadas de §4.2.2 (la clave del listado es `synthetic_speech`) y §4.2.3 (el borrado pierde `removed`), y sin ruta, por el criterio de §4.1.5.

**`speech say --json` emite solo `{"voice": "…"}`**, además de los campos transversales del sobre.

No lleva `label` porque no produce artefacto, y **no repite el `text`**: el llamador acaba de mandarlo, y devolver la entrada no es información. Lo único que el llamador puede no saber es qué voz se usó, porque si no pasó `--voice` la eligió el sistema.

Quedarse en un solo campo es deliberado, y lo decide la asimetría de reversibilidad que §4.1.5 ya estableció: **añadir una clave después es aditivo** y está cubierto por la política de compatibilidad de `cli.py:51-54`, mientras que **retirarla es incompatible** y obliga a subir `cli.SCHEMA_VERSION`. Si el narrator necesita más adelante la duración del audio o el tiempo de síntesis, se añaden sin coste; al revés no.

### 5.9. Efecto sobre §4

De los veinte hallazgos, **tres cambian de estado y dos cambian un dato**. Los quince restantes no tocan reproducción ni la superficie del grupo y quedan intactos.

| Hallazgo | Qué le pasa |
|---|---|
| **§4.1.2** | **Se suprime.** Su regla 7 prohibía que `--yes` fuera un no-op; el flag desaparece y no queda nada que prohibir. Misma clase de supresión que §4.1.3 por §4.1.6: el hallazgo no se resuelve, se le quita la causa |
| **§4.3.2** | **Se disuelve.** Sus dos caminos quedan cerrados por §5.6 regla 5 y §5.4: el script que pide `--play` sin terminal se rechaza con 2 antes de sintetizar, y el Ctrl-D interactivo es «descartar y salir» con 0. No queda ningún estado ambiguo, y su recomendación —el `EOFError` como `--yes` implícito— ya no tiene objeto |
| **§4.1.1** | **El núcleo sobrevive**: las cuatro opciones del bucle y el saneo de la fila de exit 0 de §2.8 son exactamente lo que especifica §5.4. Se borran sus dos anexos —«Efecto sobre §4.3.2» y «Efecto sobre §2.2:183»—, que razonan sobre la regla 2, la regla 5 y la derivación `--no-play` ⇒ `--yes`, las tres retiradas aquí |
| **§4.1.6** | Solo un dato de su bloque de propagación: cuenta «las seis reglas de §2.3» entre las once menciones que pasan de 4 a 2, y las reglas pasan a ser cinco con otra numeración |
| **§4.4.2** | Su criterio —«cada fila de las matrices de §2.4 **y cada regla de §2.3**»— **no cambia de texto**; cambian las matrices y las reglas que cubre, que son las de §5.6 y §5.7 |

**§4.5 se ajusta en consecuencia**: §4.1.2 y §4.3.2 salen de la lista de prioridad alta y del cierre «si solo se aplican cinco», y el recuento de estado pasa a **2 suprimidos** (§4.1.3 por §4.1.6, §4.1.2 por §5) más uno disuelto (§4.3.2).

### 5.10. Qué toca, y qué queda abierto

**En §2**:

| Sección | Cambio |
|---|---|
| §2.2 | Reescritura completa: la tabla de flags, los tres estados del eje de interacción y su cuarto estado imposible, la derivación `--no-play` ⇒ `--yes`, la frase del TTY y las tres divergencias de vocabulario |
| §2.3 | Las reglas pasan de seis a cinco, renumeradas (§5.6) |
| §2.4 | Las dos matrices, sustituidas por las de §5.7 |
| §2.5 | La tabla de sub-acciones gana `speech say`; la de flags se sustituye por §5.3 |
| §2.7 | El comentario del chequeo de audio de `setup` ejemplifica el sumidero headless con `speech synthesize --text T --label L --no-play`; ese ejemplo pasa a ser `speech synthesize --text T --label L` |
| §2.8 | La fila del exit 0 (compartida con §4.1.1) y el payload nuevo de `speech say` |

**En §3**: el paso 2.4 del movimiento 2 pasa a especificar el bucle bajo `--play`, y el paso 2.3 pierde la regla que §4.1.2 le añadía. **§3.3 necesita además un paso que cree `speech say`**, o que el paso 2.1 lo absorba declarándolo; hoy ningún paso es dueño de esa sub-acción, que es el mismo tipo de hueco de propiedad que §4.1.4 denuncia para el desacople de la emisión.

**Tres puntos abiertos**, ninguno bloqueante:

1. **La voz inexistente en `synthesize` y `say`.** §4.3.3 declara exit 3 para las tres sub-acciones de gestión, pero se escribió antes de que `say` existiera y no cubre las dos que sintetizan. Con `--voice` opcional en las cinco, la pregunta es la misma en todas y conviene que la respuesta también.
2. **`voice clone` y el despacho al daemon.** Desde `26735cd` precomputa los conditionals, así que necesita el modelo cargado igual que `synthesize` y `say`. O recibe el mismo tratamiento de §5.5, o hay que declarar por qué no.
3. **Quién es dueño de `speech say` en el plan de trabajo**, según lo anotado arriba.
