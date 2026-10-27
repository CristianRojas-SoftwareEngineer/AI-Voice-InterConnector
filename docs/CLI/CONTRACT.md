# Contrato de la CLI: el grupo `speech` y el contrato de salida

Este documento es la descripción normativa del contrato público de la CLI —comandos, flags, códigos de salida, payloads `--json`— y del almacén de habla sintética. Recoge también el porqué de cada decisión de diseño: los criterios generadores, las asimetrías de reversibilidad y el razonamiento detrás de cada código de salida.

---

## Tabla de contenidos

- [1. Invariantes y criterios generadores](#1-invariantes-y-criterios-generadores)
  - [El daemon no recibe rutas de archivos elegidas por el llamador](#el-daemon-no-recibe-rutas-de-archivos-elegidas-por-el-llamador)
  - [Una responsabilidad por sub-acción](#una-responsabilidad-por-sub-acción)
  - [El eje de dos preguntas que genera la tabla de códigos de salida](#el-eje-de-dos-preguntas-que-genera-la-tabla-de-códigos-de-salida)
  - [Cuándo un payload transporta una ruta del filesystem](#cuándo-un-payload-transporta-una-ruta-del-filesystem)
  - [El canal de la causa fina, y la regla que decide entre código y razón](#el-canal-de-la-causa-fina-y-la-regla-que-decide-entre-código-y-razón)
- [2. La superficie y el vocabulario](#2-la-superficie-y-el-vocabulario)
  - [Los comandos de nivel superior](#los-comandos-de-nivel-superior)
  - [Sin comando `uninstall` de nivel superior](#sin-comando-uninstall-de-nivel-superior)
  - [La superficie del ciclo de vida, flag a flag](#la-superficie-del-ciclo-de-vida-flag-a-flag)
  - [El qualifier `synthetic` y la resolución del vocabulario](#el-qualifier-synthetic-y-la-resolución-del-vocabulario)
  - [Las decisiones de vocabulario de la superficie](#las-decisiones-de-vocabulario-de-la-superficie)
- [3. El grupo `speech`](#3-el-grupo-speech)
  - [Reparto de responsabilidades](#reparto-de-responsabilidades)
  - [Parámetros](#parámetros)
  - [`speech denoise`: limpieza local de archivos](#speech-denoise-limpieza-local-de-archivos)
- [4. Síntesis y el bucle de `--play`](#4-síntesis-y-el-bucle-de---play)
  - [Qué hace cada gemelo](#qué-hace-cada-gemelo)
  - [El bucle de `--play`: cuatro opciones](#el-bucle-de---play-cuatro-opciones)
  - [Cuándo persiste, y qué protege la colisión](#cuándo-persiste-y-qué-protege-la-colisión)
- [5. El despacho al daemon](#5-el-despacho-al-daemon)
  - [Tres modos](#tres-modos)
  - [Qué superficies lo reciben](#qué-superficies-lo-reciben)
  - [Transporte streaming NDJSON y presupuesto de inactividad](#transporte-streaming-ndjson-y-presupuesto-de-inactividad)
  - [Por qué `--daemon` significa exigir y no seleccionar](#por-qué---daemon-significa-exigir-y-no-seleccionar)
  - [Despacho y modo directo](#despacho-y-modo-directo)
- [6. Reglas de validación](#6-reglas-de-validación)
  - [Las cinco reglas, todas con exit 2](#las-cinco-reglas-todas-con-exit-2)
  - [Un solo mecanismo para la exclusión mutua, y es el declarativo](#un-solo-mecanismo-para-la-exclusión-mutua-y-es-el-declarativo)
  - [Validación de identificadores y de existencia](#validación-de-identificadores-y-de-existencia)
  - [Ningún flag queda sin efecto sin que la CLI lo diga](#ningún-flag-queda-sin-efecto-sin-que-la-cli-lo-diga)
- [7. Matrices de comportamiento](#7-matrices-de-comportamiento)
  - [`speech synthesize`](#speech-synthesize)
  - [El resto del grupo](#el-resto-del-grupo)
  - [Qué añade `--json` a las matrices](#qué-añade---json-a-las-matrices)
- [8. El almacén de habla sintética](#8-el-almacén-de-habla-sintética)
  - [Ubicación y layout](#ubicación-y-layout)
  - [El `.wav` es el recurso de registro](#el-wav-es-el-recurso-de-registro)
  - [El sidecar de metadatos](#el-sidecar-de-metadatos)
  - [Atomicidad de la escritura](#atomicidad-de-la-escritura)
  - [Validación de identificadores](#validación-de-identificadores)
- [9. Los códigos de salida](#9-los-códigos-de-salida)
  - [La tabla](#la-tabla)
  - [Cómo se reparten los enteros](#cómo-se-reparten-los-enteros)
  - [El 2 significa lo que `clap` quiere decir con él](#el-2-significa-lo-que-clap-quiere-decir-con-él)
  - [Dónde viven las constantes, y por qué eso es parte del contrato](#dónde-viven-las-constantes-y-por-qué-eso-es-parte-del-contrato)
- [10. El canal de error y los payloads](#10-el-canal-de-error-y-los-payloads)
  - [La invariante del canal](#la-invariante-del-canal)
  - [El mecanismo: un solo punto de traducción](#el-mecanismo-un-solo-punto-de-traducción)
  - [stderr, trazas y logs](#stderr-trazas-y-logs)
  - [El contrato de salida por comando](#el-contrato-de-salida-por-comando)
  - [Los payloads del grupo `speech`](#los-payloads-del-grupo-speech)
  - [Las dos versiones de esquema](#las-dos-versiones-de-esquema)
- [11. `self`, `setup`, `cleanup` y `voice`](#11-self-setup-cleanup-y-voice)
  - [`self install`, `self update` y `self uninstall`](#self-install-self-update-y-self-uninstall)
  - [`cleanup`](#cleanup)
  - [`setup`](#setup)
  - [`voice`](#voice)
- [12. Contratos externos](#12-contratos-externos)
  - [El integrador de narración](#el-integrador-de-narración)
  - [La frontera del daemon](#la-frontera-del-daemon)
- [13. `translate`, `speech transcribe`, `speech dub` y la síntesis cross-lingual](#13-translate-speech-transcribe-speech-dub-y-la-síntesis-cross-lingual)
  - [`translate`: texto→texto, aislado de la síntesis](#translate-textotexto-aislado-de-la-síntesis)
  - [Traducción opt-in en `speech say`/`speech synthesize`](#traducción-opt-in-en-speech-sayspeech-synthesize)
  - [El flag `--target-language`](#el-flag---target-language)
  - [Provisión y daemon](#provisión-y-daemon)
  - [`speech transcribe`: audio→texto, verificable sin traducir ni sintetizar](#speech-transcribe-audiotexto-verificable-sin-traducir-ni-sintetizar)
  - [`speech dub`: el bucle voz→voz en un comando dedicado](#speech-dub-el-bucle-vozvoz-en-un-comando-dedicado)
- [14. Documentación complementaria por comando](#14-documentación-complementaria-por-comando)

---

## 1. Invariantes y criterios generadores

Cinco criterios gobiernan el resto del diseño. No son conclusiones: son las reglas con las que se resuelven las preguntas que el diseño todavía no ha visto.

### El daemon no recibe rutas de archivos elegidas por el llamador

**El daemon no recibe rutas de archivos elegidas por el llamador.** En las peticiones, el audio viaja como bytes codificados en base64 (`audio_b64`) y las voces se seleccionan por identificador (`voice`); las locuciones persistidas se nombran por `(voz, etiqueta)` en la CLI. La CLI lee los ficheros de entrada que el usuario nombra con sus flags (`speech transcribe --audio`, `speech dub --audio`, `voice clone --speech-reference`) en su propio proceso y con los permisos de quien invoca; el daemon recibe los bytes, no esas rutas. La salida de `speech synthesize` no tiene destino elegible por quien invoca: vive en el almacén de habla sintética, cuya ruta se calcula a partir de `(voz, etiqueta)` y la CLI expone (salida humana y clave `audio_path` de su `--json`).

La consecuencia sobre el daemon es estructural: `/synthesize` recibe `voice` como identificador y devuelve audio, sin pedirle un destino del filesystem. En `/voices/clone`, la petición lleva el nombre de la voz y el audio en base64, no la ruta de la referencia elegida por el usuario; su evento `result` sí devuelve `speech`, la ubicación efectiva de `reference.qvoice` calculada por el daemon al guardar la voz. La CLI la propaga en `voice clone --json` sin abrirla ni escribirla. Esta ruta identifica un fichero del filesystem del daemon: no autoriza al llamador a elegir qué fichero lee o escribe el daemon ni garantiza que cualquier cliente remoto pueda acceder a ella.

**Distinción clave: petición vs respuesta.** La restricción de rutas afecta a las *peticiones* que el daemon recibe (no hay `path` en el request). La *respuesta* de `/voices/clone` sí incluye `speech` como metadato de salida calculado por el almacén que ejecutó el clonado; la CLI lo propaga tal cual. No es una ruta de entrada ni un destino elegido por el llamador, es la ubicación efectiva donde quedó la huella de timbre en el filesystem que corrió el clonado.

### Una responsabilidad por sub-acción

Un comando cuyo comportamiento lo deciden los flags no tiene una responsabilidad con opciones: tiene varias acciones disfrazadas de una. **Producir un artefacto** y **emitir sonido** son responsabilidades distintas, y cada una tiene su propia sub-acción.

De ahí sale la forma del grupo `speech`, y de ahí sale que no haya reglas que tapen combinaciones malas: **las combinaciones malas no son expresables**. Cuando una regla de validación existe solo para impedir que un flag quede sin objeto, el defecto está en el reparto de responsabilidades y no en la falta de la regla.

Corolario de legibilidad: **el nombre de cada sub-acción declara su costo.** Sintetizar paga una inferencia pesada (residente TTS + RAM) y puede exigir provisión del modelo; reproducir paga una lectura de archivo. Desde fuera se sabe cuál se pagó sin leer los flags.

### El eje de dos preguntas que genera la tabla de códigos de salida

Son **dos preguntas encadenadas, no una**. La primera forma las clases; la segunda decide cuáles merecen un entero propio. Separarlas es lo que vuelve la tabla derivable: un eje único mezcla dos trabajos distintos —clasificar y repartir— y toda formulación que los funde acierta en una mitad y falla en la otra.

1. **Clasificación: ¿qué tipo de hecho impidió la operación?** Da cinco clases: invocación mal formada, recurso ausente, recurso ocupado, precondición de entorno incumplida e imprevisto.
2. **Admisión: ¿un consumidor programado cambiaría su siguiente llamada al distinguir esta clase de las demás?** Si sí, la clase gana entero propio; si no, comparte entero y la distinción baja al `reason` del payload de error. Se responde diciendo qué se invocaría a continuación, sin apelar a la intuición de quien redacta.

**El dominio del eje son los códigos de fallo.** Quedan fuera, y es deliberado: el `0`, que no es un fallo y por tanto no tiene remedio del que hablar; el `130`, que es convención de señales (`128 + SIGINT`) y es correcto por otra razón —ante Ctrl+C el CLI ejecuta limpieza acotada del árbol del pidfile (o del PID que conserva en memoria desde el spawn, si todavía no existe pidfile) (2 s) y sale igualmente con 130, sin colgarse—; y el `1` de `doctor`, que usa el entero como canal de **veredicto** y no de fallo, porque el trabajo de ese comando *es* diagnosticar.

**Corolario que gobierna toda clasificación: la ausencia de consumidor no valida ninguna clasificación.** Un código que nadie lee y que miente seguirá mintiendo cuando lo lean, y para entonces corregirlo será una ruptura en vez de un refinamiento. La tabla se define por el tipo de causa y por la siguiente llamada del consumidor, no por quién consume el código ni por si alguien lo consume.

### Cuándo un payload transporta una ruta del filesystem

**Un payload puede emitir una ruta calculada por el sistema para localizar el artefacto creado o identificar los directorios afectados; no acepta por ello una ruta de salida elegida por quien invoca.**

| Payload | Emite ruta | Por qué |
|---|---|---|
| `voice list --json` | No: `{"voices": [nombres]}` | La voz tiene handle propio —su nombre—, así que el directorio nunca sale |
| `speech list`, `play`, `remove`, `say`, `dub` | No | Operan sobre `(voz, etiqueta)` o no persisten nada: el identificador basta y no hay fichero que entregar |
| `speech synthesize --json` | Sí: `audio_path`, la ruta del WAV en `<data_dir>/speech/<voz>/<etiqueta>.wav` | La operación produce el fichero y no existe ningún comando de exportación: la ruta es la única forma de que el integrador obtenga los bytes |
| `voice clone --json` | Sí: `speech`, la ruta de `reference.qvoice` en `<data_dir>/voices/<nombre>/` del almacén que ejecutó el clonado | En la vía directa la calcula la CLI; en la vía daemon la calcula el daemon (en su filesystem) y la CLI la propaga sin abrirla ni escribirla. El llamador no elige la ruta y esta no garantiza acceso desde otro filesystem; la voz se sigue manejando por su nombre |
| `cleanup --json` | Sí: `removed` como lista de rutas borradas | Los directorios de caché del modelo y de voces no tienen ningún handle en la CLI; la ruta es su único nombre |

La ruta se expone para que el consumidor lea el fichero, no como destino de escritura de la CLI: el almacén pertenece a la CLI, que lo reemplaza con `--force` y lo borra con `speech remove`. Quien quiera conservar un WAV más allá de esas operaciones lo copia por su cuenta desde `audio_path`.

**Coste declarado.** La CLI no tiene comando de exportación ni flag de destino: los bytes de una locución se obtienen leyendo el fichero de `audio_path` (o de la ruta que imprime `speech synthesize`) con los permisos de quien invoca. Eso acopla al integrador al layout del almacén durante esa lectura; si la necesidad de un destino elegible aparece, la respuesta es un comando explícito con su propia decisión, no una clave en un listado.

**La asimetría de reversibilidad que respalda el criterio.** Las dos opciones no cuestan lo mismo si resultan equivocadas: **añadir una clave después es aditivo** y está cubierto por la política de compatibilidad del esquema `--json`; **retirarla es incompatible** y obliga a subir `schema_version` (`CLI_SCHEMA_VERSION` en [`json_emitter.rs`](../../crates/avi-core/src/json_emitter.rs)). Con esa asimetría, el lado seguro se conoce de antemano y no hay opcionalidad que comprar aplazando la decisión.

### El canal de la causa fina, y la regla que decide entre código y razón

El entero no puede llevar la causa fina y no debe intentarlo. Una misma reacción del consumidor puede corresponder a varias acciones distintas del destinatario humano: liberar disco, corregir permisos, renovar un token, desbloquear la red o instalar una dependencia inducen todas la misma siguiente llamada, y son cinco cosas distintas que hacer antes de repetirla.

El proyecto tiene dos canales legibles por máquina y usa los dos: el entero, que es un espacio cerrado, y el payload JSON, que es **aditivo por contrato** y tiene un punto único de emisión, `emit_raw_json()`. La distinción fina va por el canal abierto.

**Tres reglas de compatibilidad**, que son lo que impide reabrir la misma brecha un nivel más allá:

1. **El entero siempre basta por sí solo.** `reason` refina; nunca contradice ni condiciona. Un consumidor que ignore la clave se comporta correctamente, solo que con menos resolución. Sin esta regla el segundo canal sería una segunda tabla congelada.
2. **Añadir un `reason` nuevo no incrementa `schema_version`**, igual que añadir una clave. Es contrato de emisión **y de consumo**: un `reason` desconocido se trata como ausente, es decir, se degrada al entero.
3. **Regla de promoción.** Un código de salida nuevo solo se justifica cuando cambia **la siguiente llamada del consumidor** —la segunda pregunta del eje—; cuando la llamada siguiente es la misma y lo que cambia es la acción concreta que alguien ejecuta antes de repetirla, es un `reason`. Su árbitro es único y comprobable: se responde diciendo qué se invocaría a continuación, no sopesando importancia.

## 2. La superficie y el vocabulario

### Los comandos de nivel superior

La lista de comandos y subcomandos vigente, con su propósito, está en [`README.md`](README.md#resumen-de-comandos); esta tabla fija solo el reparto de responsabilidades que el contrato necesita.

| Comando | Sub-acciones | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `play`, `list`, `remove`, `transcribe`, `dub`, `denoise` | Síntesis de habla, gestión del almacén, transcripción, composición voz→voz y limpieza local de archivos de audio/vídeo |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
| `translate` | — | Traducción de texto `es↔en`, aislada de la síntesis |
| `devices` | — | Lista dispositivos de audio |
| `doctor` | — | Diagnósticos |
| `setup` | — | Provisión del runtime |
| `cleanup` | — | Borrado del estado por categorías, sin tocar el programa |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon |
| `version` | — | Versión |
| `self` | `install`, `update`, `uninstall` | Operaciones del ciclo de vida sobre la instalación del usuario |

**`speech`, `voice`, `daemon` y `self` son grupos nominales de gestión**: tienen sub-acciones y ninguna acción propia.

Todos los subcomandos salvo `daemon serve` declaran `--json`, y la garantía es mecánica: un test recorre el parser real para descubrir cuáles lo declaran, de modo que una sub-acción nueva sin `--json` lo hace fallar.

### Sin comando `uninstall` de nivel superior

**El comando de nivel superior `uninstall` no existe**, y tampoco su `--force`. El grupo `self` cubre ese papel, y `--force` existe **con otro significado** en `self install` y `self update`: resolver un `path_conflict` en la ruta del enlace ([§11](#11-self-setup-cleanup-y-voice)).

### La superficie del ciclo de vida, flag a flag

| Sub-acción | Flags | Estado |
|---|---|---|
| `self install` | `--no-setup` · `--no-modify-path` · `--force`/`-f` · `--yes` · `--json` | Vigente |
| `self update` | `--check` · `--version X.Y.Z` · `--force`/`-f` · `--no-setup` · `--yes` · `--json` | Vigente |
| `self uninstall` | `--keep-data` · `--dry-run` · `--yes` · `--json` | Vigente |
| `setup` | `--force-update` · `--yes`/`-y` · `--json` | Vigente |
| `cleanup` | `--model` · `--voices` · `--synthetic-speech` · `--all` · `--dry-run` · `--yes`/`-y` · `--json` | Vigente |
| `doctor` | `--repair` · `--json` | Vigente |

`--json` es **global** (`Cli::json`, `src/main.rs`): lo declara la raíz, no cada sub-acción, y por eso aparece en todas las filas. Es el único flag global. `--daemon`/`--no-daemon` no son globales: solo los declaran los subcomandos con dos vías (`translate`, `speech synthesize`, `speech say`, `speech transcribe`, `speech dub` y `voice clone`, [§5](#5-el-despacho-al-daemon)). `self install` no tiene opciones ocultas ni de canal.

**`self install` tiene además un modo que no es un flag.** El modo lo decide la posición del ejecutable, no una opción: dentro del directorio de programa **repara** en vez de instalar, y un ejecutable sin bundle alrededor (por ejemplo `target\debug`) sale con `bundle_invalid` (15) indicando `cargo xtask install` ([`commands/SELF.md`](commands/SELF.md)).

### El qualifier `synthetic` y la resolución del vocabulario

`speech` nombra el **género**: habla. El qualifier `synthetic` marca la dirección del flujo de datos —lo que el sistema produce frente a lo que el usuario aporta— y mantiene separadas las tres capas donde el término aparece.

| Capa | Elemento | Nombre |
|---|---|---|
| CLI | Grupo de síntesis y gestión de la salida | `speech synthesize/say/play/list/remove` |
| CLI | Entrada de referencia de habla de `voice clone` (obligatoria; ~10 s recomendados, máximo 30 s) | `--speech-reference` (`-s`) |
| CLI | Borrado masivo de la salida | `cleanup --synthetic-speech` |
| Filesystem | Almacén de la salida generada | `<data_dir>/speech/<voz>/<etiqueta>.wav` (`crates/avi-store/src/lib.rs` `SpeechStore`) |
| Filesystem | Huella de timbre persistida de una voz clonada | `reference.qvoice` (`crates/avi-store/src/lib.rs` `VoiceStore`); el WAV de entrada (`--speech-reference`) se consume y no se copia al almacén |
| Payload | Clave del listado | `synthetic_speech` |

El orden de palabras respeta la convención del repo, con el núcleo al final (`--temperature`, `--speech-reference`). El qualifier vive solo en el directorio y en el flag de `cleanup` —las dos operaciones de gestión—, no en la ruta caliente, que es `speech synthesize`.

En disco los dos sentidos quedan separados por nombre y no por posición:

```
<data_dir>/
  voices/<voz>/reference.qvoice         ← huella de timbre persistida (embedding de locutor), derivada de la referencia de habla aportada (`VoiceStore`)
  speech/<voz>/<etiqueta>.wav           ← salida generada (`SpeechStore`)
```

**En prosa española la unidad se llama «locución».** Nunca aparece como identificador.

### Las decisiones de vocabulario de la superficie

- **El identificador de una locución es `--label/-l`, no `--name/-n`.** Por homología con `voice` correspondería `--name`, pero dentro del grupo `speech` sería ambiguo frente a `--voice` («¿nombre de qué?»). Se acepta la divergencia con `voice --name` a cambio de que el mismo concepto no tenga dos nombres en dos comandos.
- **La voz se selecciona con `--voice/-v` en las cinco sub-acciones**, no con `--voice-profile`: el concepto ya se llama «voice» en `voice list`, `voice clone` y `voice remove`, y darle un segundo nombre en otro comando es la homonimia al revés —dos palabras para una cosa— con el mismo costo.
- **`--play` y la sub-acción `play` comparten palabra a propósito.** Nombran una sola cosa —emitir audio por los parlantes— en los dos sitios donde ocurre.
- **`-t` es `--text` en `speech` y no existe en `voice clone`.** `voice clone` no declara `--text` y `speech` no declara referencias, así que ningún corto se solapa.
- **`-n` no está tomado en el grupo `speech`**, así que tiene un significado único en toda la CLI: `--name` en `voice clone` y `voice remove`.

## 3. El grupo `speech`

### Reparto de responsabilidades

| Sub-acción | Responsabilidad | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda | **sí** | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech play` | Reproduce una locución guardada | no | **no** |
| `speech list` | Lista las locuciones guardadas | no | no |
| `speech remove` | Borra una locución guardada | no | no |
| `speech transcribe` | Transcribe audio a texto | no | sí |
| `speech dub` | Compone voz→voz | no | sí |
| `speech denoise` | Limpia archivos de audio o la pista de audio de vídeo | no | sí, `native-denoise` |

`speech denoise` es local-only: procesa archivos directamente y no envía rutas al daemon. Acepta entradas de audio y vídeo; para vídeo conserva la imagen y limpia únicamente la pista de audio. Su perfil de archivos predeterminado es `studio`; `--preset default` selecciona el perfil original sin normalización. Requiere `native-denoise`, incluido en `full`; DPDFNet debe estar provisionado con `setup` y FFmpeg se resuelve desde el paquete de AVI, sin fallback al `PATH`; el binario integrado debe declarar versión 6 o superior, con o sin el prefijo de `git describe` que anteponen los builds de Linux, y una versión anterior o indescifrable se rechaza nombrando lo encontrado. La limpieza de archivos no se expone como endpoint del daemon.

`synthesize` y `say` son gemelos: misma síntesis, distinto destino —disco o parlantes—. `play`, `list` y `remove` son la gestión del almacén. **`say` es la única sub-acción que genera sin persistir, y junto con `synthesize` la única que puede exigir provisión del modelo**; esa es la contrapartida de que `play`, `list` y `remove` no lo necesiten.

El almacén etiquetado es un recurso, y el repo tiene gramática para gestionar recursos: un grupo nominal con sub-acciones. La homología con `voice` es directa:

| Registro de voces | Almacén de habla sintética |
|---|---|
| `voice list` | `speech list` |
| `voice clone` | `speech synthesize` |
| `voice remove` | `speech remove` |
| — | `speech play` |
| — | `speech say` |

### Parámetros

| Sub-acción | Parámetros |
|---|---|
| `speech synthesize` | `--text/-t` **requerido** · `--label/-l` **requerido** · `--voice/-v` · `--play` · `--force/-f` · `--source-language` · `--target-language` · `--temperature` · `--json` · `--daemon`/`--no-daemon` |
| `speech say` | `--text/-t` **requerido** · `--voice/-v` · `--source-language` · `--target-language` · `--temperature` · `--json` · `--daemon`/`--no-daemon` |
| `speech play` | `--label/-l` **requerido** · `--voice/-v` · `--json` |
| `speech list` | `--voice/-v` · `--json` |
| `speech remove` | `--label/-l` **requerido** · `--voice/-v` · `--json` |
| `speech denoise` | `<entrada>...` · `--output-dir` · `--output-name` · `--prefix` · `--suffix` · `--recursive` · `--overwrite` · `--skip-existing` · `--dry-run` · `--json` global (JSONL) · `--verbose` · `--preset studio|default` · `--audio-bitrate` · `--highpass` · `--normalize` · `--target-lufs` · `--pause-gate-db` · `--dry-blend` · `--stereo` · `--help` |

### `speech denoise`: limpieza local de archivos

**`speech denoise` es una operación local de archivos**: el parser único del subcomando reutiliza el pipeline migrado. Las rutas `--model-dir` y `--ffmpeg-path` no forman parte de la CLI AVI y deben rechazarse como flags desconocidos; el modelo procede de `setup`/`doctor` y FFmpeg del paquete, sujeto a la misma comprobación de versión. `--json` global emite líneas JSONL de progreso y resultados sin envolverlas en otro objeto. Los flags opt-in `--denoise` de transcripción, doblaje y clonado son rutas independientes; la evaluación humana de preservación de identidad vocal en clonado no se considera validada. Para los contratos completos de opciones, entradas múltiples, nombres, colisiones, dry-run y resultados, véase [`commands/SPEECH.md`](commands/SPEECH.md#speech-denoise).

`studio` es el perfil por omisión de este subcomando de archivos, con normalización de una pasada hacia −16 LUFS (objetivo, no garantía exacta), highpass, gate −25 dB, mezcla seca 0 y mono salvo `--stereo`. Los bitrates predeterminados son MP3/AAC 320, Vorbis 192 y Opus 256 kbps; WAV/FLAC no usan bitrate. El bitrate explícito incompatible con el códec se rechaza, sin ajuste automático. `--preset default` elige el perfil original (192 kbps, highpass desactivado, sin normalización, gate −25 dB, mezcla seca 0 y mono). Se conserva la precedencia defaults → preset → flags individuales; los flags positivos no desactivan booleanos activados por el preset.

**`--voice/-v` es opcional en las cinco que lo aceptan** (`synthesize`, `say`, `play`, `list`, `remove`). En `synthesize`, `say`, `play` y `remove`, si falta, usa la voz de fábrica `default`; en `list`, sin `--voice` recorre todas las voces. La voz `default` es una voz clonada de fábrica con solo la huella de timbre ( `crates/avi-store/assets/default/reference.qvoice`, `crates/avi-store/src/lib.rs` `FACTORY_VOICES`). El catálogo de voces de fábrica es `default` (clonada), `ryan` y `vivian` (presets del motor `qwen_tts.c:spk_table`): todas entradas del registro `VoiceStore`, y el almacén garantiza las tres en cada consulta, incluida una raíz de datos recién creada. La distinción interna preset/clonada (`avi-tts/src/lib.rs` `resolve_voice_engine`, presencia de `reference.qvoice`) es detalle no-normativo; para el contrato toda voz es una entrada del registro. Las tres voces de fábrica sintetizan texto corto (2-4 palabras, p. ej. `Hola mundo`) con una tasa de error de palabras ≤ 0.25.

**Matriz voces × idioma.** Las tres voces de fábrica hablan `es` y `en`: el motor traduce el token del producto a su nombre de idioma (`es` → `Spanish`, `en` → `English`) en la síntesis y en la clonación, y los nueve locutores del motor son multilingües. El idioma nativo de un preset es solo informativo (`ryan`: inglés; `vivian`: chino) y no restringe el idioma de destino. `default` es una clonación generada con la ruta de clonación del producto a partir de `assets/audios/Speech Sampler.wav` (~10 s): guarda solo la huella de timbre. Toda voz clonada (`default` incluida) se sintetiza con el modelo Base y los presets con CustomVoice; sin el modelo Base provisionado, sintetizar con una voz clonada sale con `model_missing` (exit 4), sin caer a CustomVoice.

**El namespace es obligatorio en la gestión.** Las etiquetas viven bajo una voz, así que `play`, `remove` y `list` toman `--voice` con el mismo default que `synthesize` y `say`. `speech list` filtra por voz cuando se pasa `--voice/-v` (`SpeechCommands::List { voice: Option<String> }`, del binario principal; brazo `List` en el binario principal, lectura acotada vía `SpeechStore::list_by_voice` en el almacén de voces); sin `--voice` recorre todas las voces. Es un segmento más que en `voice remove --name X`, inevitable dado el layout del almacén.

**`--label` requerido en `synthesize` es lo que sostiene el reparto.** Elimina de raíz la invocación con efecto cero sin escribir ninguna regla —la rechaza el parser— y elimina la trampa de «previsualizo con un comando y guardo con otro»: como `synthesize` siempre persiste, nadie pierde la toma que acaba de oír.

**`speech play` no necesita modelo ni daemon**: lee el WAV del almacén y lo reproduce.

**El listado no vive dentro de `synthesize`.** No hay `speech synthesize --list`: el listado es `speech list`.

**Reparto con `cleanup`**: `speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que existe entre `voice remove` y `cleanup --voices`. Las dos operaciones de voz arrastran además las locuciones de las voces que borran.

## 4. Síntesis y el bucle de `--play`

### Qué hace cada gemelo

Sin `--play`, `synthesize` sintetiza, guarda y termina. Con `--play`, reproduce la toma y pregunta antes de guardar.

`speech say` sintetiza y reproduce, y no escribe nada en el almacén. Es el destino de la invocación que solo quiere oír el resultado: la que no nombra un artefacto porque no lo quiere.

**Son dos usos que no se cruzan, y el diseño no supone que la síntesis sea determinista.** `say` es locución continua, generada al vuelo: cada mensaje es distinto del anterior y se descarta al sonar, así que persistir no tendría sentido. `synthesize` es para grabar un mensaje reutilizable —el caso de los mensajes por defecto— y reproducirlo después sin volver a sintetizarlo. No existe un recorrido que salte de `say` a `synthesize` para «quedarse» con una toma ya oída: quien quiere conservar usa `synthesize` desde el principio. Por eso la reproducción sin re-síntesis la garantiza **el almacén** —se guarda un WAV y se reproduce ese WAV—, y no una supuesta reproducibilidad del motor entre dos llamadas. Dentro de `synthesize`, la variación entre tomas es esperada y es justo lo que «rechazar y regenerar» aprovecha; «aceptar y guardar» persiste la toma que sonó, nunca una nueva.

### El bucle de `--play`: cuatro opciones

| Opción | Efecto | Costo |
|---|---|---|
| Reproducir otra vez | Vuelve a sonar la misma toma | **Cero síntesis**: los bytes están en memoria |
| Aceptar y guardar | Persiste la toma que acabas de oír, y termina con 0 | Cero |
| Rechazar y regenerar | Sintetiza otra toma y vuelve a preguntar | Una síntesis completa por toma |
| Rechazar y descartar | Termina con 0 **sin guardar nada** | Cero |

**«Descartar y salir» es una salida de primera clase**, con exit 0 y sin persistencia: el rechazo es un campo del resultado, no un error. Es el mismo modelado que `cleanup`, donde responder «n» a la confirmación termina con 0. Lo que el bucle no comparte con ese comando es la forma de la elección —allí es binaria— ni el destino de su prosa: la pregunta y sus avisos respetan la separación de canales (con `--json` la información humana va a stderr y stdout queda para el payload) y la cancelación viaja como campo del resultado.

**«Descartar» y no «rechazar».** En el bucle, regenerar también rechaza la toma; la palabra del contrato no distinguiría entre las dos opciones que descartan el audio actual, y solo una de ellas termina la invocación.

**Ctrl-D es el atajo de «descartar y salir».** Con terminal presente, cerrar la entrada en la pregunta es una forma legítima de abandonar y mapea exactamente sobre la cuarta opción: exit 0, sin persistir. Es el único fin de entrada alcanzable en el bucle, y tiene significado propio.

### Cuándo persiste, y qué protege la colisión

**Cuándo persiste.** Sin `--play`, inmediatamente después de sintetizar. Con `--play`, solo al aceptar. Así «descartar» nunca es un borrado: es no haber escrito.

**La colisión de etiqueta se comprueba dos veces, y cada comprobación tiene un papel distinto.**

- **Antes de sintetizar**, como *fast-fail*: si la etiqueta está tomada y no hay `--force`, el comando sale con **6** sin pagar la síntesis. Comprobarla solo después obligaría a pagar la síntesis entera para descubrir que no se puede guardar, y con `--play` además a recorrer el bucle hasta «aceptar» para fallar ahí.
- **Al escribir**, y **esta es la que gobierna el contrato**: entre la comprobación previa y la escritura hay una ventana —el bucle puede durar minutos— y la etiqueta puede quedar tomada en ese intervalo. Si al escribir está tomada y no hay `--force`, la salida es **6**.

## 5. El despacho al daemon

### Tres modos

| Invocación | Qué hace |
|---|---|
| Sin flags | **Comprueba el daemon.** Si está activo, sintetiza por él; si no, carga el modelo al vuelo |
| `--no-daemon` | Fuerza la síntesis directa aunque el daemon esté activo |
| `--daemon` | **Exige** el daemon: si no está activo, sale con **5** en vez de degradar |

La autodetección es el único camino por defecto: un comportamiento especificado, no una rama a la que se cae cuando el llamador no dice nada.

**No hay degradación silenciosa.** `--no-daemon` es un opt-out explícito del usuario, categóricamente distinto de una degradación automática que elude una restricción sin que nadie la pida.

**Los dos flags son propios de los subcomandos con dos vías** (`translate`, `speech synthesize`, `speech say`, `speech transcribe`, `speech dub` y `voice clone`), que los declaran tras su nombre (`speech say --daemon …`); no son flags globales, así que no se aceptan antes del subcomando. Cualquier otro comando los rechaza como argumento desconocido: sale con **2** y, con `--json`, `reason` `usage_error`.

### Qué superficies lo reciben

| Comando | Delegable | Endpoint | Razón local-only |
|---|---|---|---|
| `speech synthesize` | Sí | `POST /synthesize` | — |
| `speech say` | Sí | `POST /synthesize` | — |
| `speech transcribe` | Sí | `POST /transcribe` | — |
| `speech dub` | Sí | `POST /dub` | — |
| `voice clone` | Sí | `POST /voices/clone` | — |
| `translate` | Sí | `POST /translate` | — |
| `speech play` | No | — | sin modelo (`play_wav` sobre `SpeechStore`) |
| `speech list` | No | — | sin modelo |
| `speech remove` | No | — | sin modelo |
| `voice list` | No | — | sin modelo (`VoiceStore`) |
| `voice remove` | No | — | sin modelo |

Delegables: `synthesize`, `say`, `transcribe`, `dub`, `clone` y `translate`. Local-only: `play`, `remove`, `list` y `voice list`/`remove`. `setup`/`cleanup`/`doctor`/`daemon`/`devices`/`version` fuera de alcance (provisión/diagnóstico/gestión, no usan modelo residente).

### Transporte streaming NDJSON y presupuesto de inactividad

Las operaciones de síntesis (`POST /synthesize`), clonado (`POST /voices/clone`) y doblaje (`POST /dub`) utilizan transporte **streaming NDJSON** (`application/x-ndjson`). La disciplina de eventos es uniforme:

1. **Evento `started` inmediato**: se emite tras superar las validaciones baratas (parámetros, nombres, formatos) y justo antes de iniciar el trabajo pesado o la toma de locks. Los tres endpoints lo emiten con el mismo nombre y **con la clave `operation`**, que nombra el trabajo: «síntesis» en `/synthesize`, «clonación» en `/voices/clone` y «doblaje» en `/dub`. La clave es aditiva: las demás claves del evento no cambian (`voice` y `text_length` en `/synthesize`, `name` en `/voices/clone`, `voice` en `/dub`) y una CLI anterior la ignora.
2. **Latidos periódicos (`heartbeat`)**: el daemon emite latidos `{"event":"heartbeat","stage":"<etapa>"}` cada ~500 ms (y eventos de `progress` según la etapa) mientras el trabajo en segundo plano continúa. Mientras una petición de síntesis espera el lock de síntesis emite `stage: "warming"` si el daemon aún calienta el motor, o `stage: "queued"` si otra síntesis retiene el lock; una vez sintetizando, `stage: "synthesis"`. Así una petición en espera nunca queda muda ni agota el timeout de inactividad.
3. **Evento final (`result` o `error`)**: transporta el resultado exitoso en su forma contractual (`{name, speech, precomputed: true}` en clonado; `{status: "dubbed", text, translated, audio_b64, voice}` en dub) o el objeto de error con su `reason` correspondiente.

**Las etapas que el daemon nombra.** El daemon emite siete `stage` distintos, y la CLI los traduce a su texto propio: `warming` (calentando motor de síntesis), `queued` (en cola de síntesis), `synthesis` (sintetizando voz), `warmup` (precarga en caliente del clonado), `clone` (clonando voz), `transcribe` (transcribiendo audio) y `translate` (traduciendo texto). Un `stage` que no está en esa lista **se ignora**: no se anuncia ni un texto genérico que confunda una etapa con otra. La etapa se anuncia **una vez** en no-TTY —la actividad repetida dentro de la misma etapa no abre línea nueva— y la fase se repinta sobre la misma línea en TTY.

**Semántica del timeout en el cliente:** el timeout de inactividad entre latidos (`STREAM_INACTIVITY_TIMEOUT` en [`src/main.rs`](../../src/main.rs)) solo salta si el bucle del daemon se atasca sin emitir señal alguna; se complementa con un plazo total de seguridad failsafe (`STREAM_TOTAL_DEADLINE`, el mismo techo `REQUEST_FAILSAFE` del cliente HTTP, definido en [`limits.rs`](../../crates/avi-core/src/limits.rs): el presupuesto del texto máximo más una holgura para el calentamiento, la transcripción y la traducción). Ante una desconexión prematura del cliente, el servidor deja de emitir y descarta el resultado; en la síntesis cancela además el trabajo del motor TTS, que queda libre para la siguiente petición. La transcripción, la traducción y el clonado terminan en segundo plano y su resultado se descarta.

**Presupuesto de síntesis.** La síntesis está acotada por `synthesis_budget` ([`limits.rs`](../../crates/avi-core/src/limits.rs)): una parte fija más una parte proporcional a los caracteres del texto que realmente se sintetiza (el ya traducido, en `dub`). La parte fija cubre la primera inferencia de un motor recién lanzado, bastante más lenta que las siguientes. El presupuesto es el plazo total de la petición al motor, no un plazo por lectura. Al vencer, el daemon emite el evento de error `synthesis_timeout` (exit 1) y cancela el trabajo en el motor TTS residente, que lo aborta y queda caliente para la siguiente petición; si el motor no responde, se reemplaza antes de la siguiente síntesis. La vía directa devuelve el mismo `synthesis_timeout` con exit 1.

### Por qué `--daemon` significa exigir y no seleccionar

Con la autodetección por defecto, «usa el daemon» deja de ser algo que haya que pedir. Sin el flag, el llamador no tendría forma de exigir la ruta rápida y el código 5 **se quedaría sin ningún productor en la síntesis**: si la ausencia del daemon siempre degrada, nunca hay «daemon inalcanzable», solo una invocación más lenta. Un consumidor con presupuesto de latencia —el narrator es el caso previsto— necesita poder decir «prefiero fallar a esperar a que cargue el modelo».

Con los dos flags declarados, la exclusión mutua entre ellos tiene sentido pleno: «exige daemon» y «prohíbe daemon» se contradicen.

### Despacho y modo directo

Con el daemon activo, `speech synthesize`/`say`/`transcribe`/`dub` y `voice clone` pueden usar modelo caliente; `--no-daemon` fuerza ruta directa sin sondeo.

## 6. Reglas de validación

### Las cinco reglas, todas con exit 2

1. **`--daemon` y `--no-daemon` son excluyentes.** La resuelve el parser (`conflicts_with` en el par que comparten los subcomandos con dos vías, [§5](#5-el-despacho-al-daemon)), no una comprobación a mano. Los demás comandos no declaran ninguno de los dos y los rechazan como argumento desconocido, también por el parser.
2. **`--json` es incompatible con `--play`.** El bucle escribe la pregunta y lee la respuesta por los canales estándar, y contaminaría el payload. Aplica a `speech synthesize`.
3. **`--text` no vacío ni solo espacios.** Aplica a `speech synthesize` y `speech say`.
4. **El texto a sintetizar no excede `MAX_TEXT_LENGTH`** (500 caracteres Unicode, no bytes; `reason` `text_too_long`). Se valida **en el cliente** antes de cualquier despacho, con el mismo código por ambas vías; el tope del daemon es defensa en profundidad (responde HTTP 400 con `{schema_version, error, reason, message}` en `/synthesize`, y un evento `error` `text_too_long` en `/dub`) y no la fuente de la validación. Aplica a `speech synthesize` y `speech say` (el `--text`) y a `speech dub` (el texto ya traducido). En `speech dub`, además, **el audio de entrada no dura más de `MAX_DUB_AUDIO_SECS`** (40 s; `reason` `audio_too_long`), sea `--audio`, `--mic` o `--duration`: un `--duration` superior sale con 2 y el push-to-talk se corta a 40 s. Los otros dos puntos de entrada de audio tienen su propio tope, igual en la vía local y en la del daemon: **el audio de `speech transcribe` no dura más de `MAX_TRANSCRIBE_AUDIO_SECS`** (300 s) y **la referencia de `voice clone` no dura más de `MAX_CLONE_REFERENCE_SECS`** (30 s). Un exceso sale con 2 y `reason` `audio_too_long`, con un mensaje que nombra la operación. En la CLI la comprobación precede a la carga de modelos (`model_missing`), a `stt_unsupported` y al contacto con el daemon; la duración se mide primero por la cabecera del WAV y después por el PCM decodificado, y solo se admiten WAV. En `speech transcribe --mic`, un `--duration` superior a 300 y un `AVI_PUSH_TO_TALK_MAX_SECS` superior a 300 (push-to-talk) salen con 2 `audio_too_long` antes de grabar y, en el segundo caso, antes de exigir terminal. El daemon repite el tope como defensa en profundidad: `/transcribe` rechaza con HTTP 400 `audio_too_long` más de 300 s de muestras y `/voices/clone` rechaza con 400 `audio_too_long` una referencia de más de 30 s, antes de comprobar el modelo; además, cada ruta limita el cuerpo de la petición (`body_limit_for` en [`limits.rs`](../../crates/avi-core/src/limits.rs) dimensiona el tope de `/transcribe` y el de `/voices/clone` a partir de su duración máxima y de una tasa de bytes por segundo) y responde 413 si se excede, aunque la duración sea menor que el tope.
5. **`--play` exige terminal en la entrada estándar.** Si no la hay, se rechaza **antes de sintetizar**. Aplica a `speech synthesize`.

**La regla 5 es de otra clase que las cuatro anteriores**: las cuatro primeras miran los flags, la quinta mira el entorno. La comprobación no altera ningún default —`--play` es explícito, así que la misma línea de comandos no puede significar cosas distintas según dónde corra—; solo rechaza antes una invocación que iba a fallar igual. Lo único que queda fuera de alcance es alimentar las respuestas del bucle por una tubería, un caso marginal cuyo precio, de conservarlo, sería pagar una síntesis y una reproducción completas antes de fallar.

### Un solo mecanismo para la exclusión mutua, y es el declarativo

La exclusión mutua se declara con `clap` (`conflicts_with` o un `ArgGroup`) en `src/main.rs` (`Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`), junto a los flags que restringe, en todos los sitios donde exista —el grupo de tres modos de `setup` incluido. **La garantía queda en un solo lugar, no repetida por convención en cada comando.** Una comprobación manual es esa convención repetida, y no escala: en un grupo de tres modos, un cuarto añadido a mano no rompe nada y deja de cubrir una combinación en silencio; el `if` vive lejos de los flags que restringe, donde nadie que añada uno lo va a leer.

El coste es que el mensaje lo formatea `clap` en inglés, igual que el de todas las demás rutas de parseo, y ese mensaje entra íntegro en el payload de error.

### Validación de identificadores y de existencia

| Situación | Superficies | Código |
|---|---|---|
| Etiqueta con caracteres ilegales | `synthesize`, `play`, `remove` | **2** |
| Nombre de voz con caracteres ilegales | Todas las que toman `--voice` | **2** |
| Voz inexistente | **Las cinco que toman `--voice`**: `synthesize`, `say`, `play`, `list`, `remove` | **3** |
| Etiqueta inexistente | `play`, `remove` | **3** |
| Colisión de etiqueta sin `--force` | `synthesize` | **6** |
| Colisión de nombre de voz sin `--force` | `voice clone` | **6** |

**La voz se valida en las cinco sub-acciones que toman `--voice` y sale 3 si no está** —el catálogo es el registro unificado `VoiceStore` (`crates/avi-store/src/lib.rs` `FACTORY_VOICES`): voces de fábrica (`default` —clonada con `reference.qvoice`—, `ryan`, `vivian`) más clonadas del usuario—, de modo que «voz mal escrita» nunca se disfrace de «sin resultados»: sin esa regla, `speech play --voice noexiste` devolvería un error de etiqueta inexistente y un usuario que se equivoca al escribir concluiría que sus locuciones se perdieron en vez de que escribió mal el nombre. Con `--voice` opcional en las cinco, la pregunta es la misma en todas y la respuesta también. (`list`, `play` y `remove` validan igual y antes de buscar nada: identificador ilegal → 2 `invalid_identifier` vía `is_valid_identifier` del binario principal, voz inexistente → 3 `voice_not_found` vía `VoiceStore::exists` del almacén de voces; `speech list` sin `--voice` lista todas las voces. La regla es sostenible porque toda locución guardada pertenece a una voz existente: `voice remove` y `cleanup --voices` se llevan las locuciones con su voz.)

La etiqueta inexistente sale **3** y no 2: la invocación está bien formada y el recurso no está, que es exactamente lo que el 3 significa.

**La colisión de etiqueta y la de nombre de voz son el mismo hecho** —el recurso está ocupado y hay que liberarlo o forzar— y comparten código. Con el almacén etiquetado, la colisión no es un caso esporádico: ocurre cada vez que se regenera una locución ya existente, que es flujo normal de trabajo.

### Ningún flag queda sin efecto sin que la CLI lo diga

La afirmación vale con una excepción declarada: **`--force` sobre una etiqueta libre es un no-op**, igual que `voice clone --force` sobre un nombre libre. Fuera de ese caso, toda combinación de flags tiene efecto declarado o sale con 2, 3 o 6.

## 7. Matrices de comportamiento

### `speech synthesize`

| Invocación | Genera | Reproduce | Guarda | Exit |
|---|---|---|---|---|
| `-t T -l L` *(L libre)* | sí | no | sí | 0 |
| `-t T -l L --json` *(L libre)* | sí | no | sí | 0 |
| `-t T -l L -p` *(L libre, con terminal)* | sí | sí, en el bucle | al aceptar | 0 |
| `-t T -l L -p` *(L libre, se descarta en el bucle)* | sí | sí, en el bucle | no | 0 |
| `-t T -l L -f` *(L existe)* | sí | no | sí, sobrescribe | 0 |
| `-t T -l L -p -f` *(L existe)* | sí | sí, en el bucle | al aceptar, sobrescribe | 0 |
| `-t T -l L` *(L existe, sin `-f`)* | — | — | — | **6** |
| `-t T -l L -p` *(L libre al empezar, tomada al aceptar, sin `-f`)* | sí | sí, en el bucle | no | **6** |
| `-t T -l L -p` *(sin terminal)* | — | — | — | **2** |
| `-t T -l L -p --json` | — | — | — | **2** |
| `-t T` *(sin `-l`)* | — | — | — | **2** |
| `-t T -l L` *(etiqueta ilegal)* | — | — | — | **2** |
| `-t T -l L -v V` *(V no existe)* | — | — | — | **3** |
| `-t T -l L --daemon` *(daemon caído)* | — | — | — | **5** |
| `-t T -l L` *(modelo no provisionado)* | — | — | — | **4** |

La primera fila es el camino de automatización, y no necesita ningún flag: sintetizar y guardar **es** lo que el comando hace.

### El resto del grupo

| Invocación | Genera | Reproduce | Exit |
|---|---|---|---|
| `speech say -t T` | sí | sí | 0 |
| `speech say -t T --json` | sí | sí | 0 |
| `speech say -t T --daemon` *(daemon caído)* | — | — | **5** |
| `speech say -t T` *(modelo no provisionado)* | — | — | **4** |
| `speech list` *(todas las voces)* | no | no | 0 |
| `speech list -v V` *(V existe)* | no | no | 0 |
| `speech play -l L` *(L existe)* | no | sí | 0 |
| `speech remove -l L` *(L existe)* | no | no | 0 |
| `speech play -l L` / `speech remove -l L` *(L no existe)* | — | — | **3** |
| `speech say`, `list`, `play` o `remove` con `-v V` *(V no existe)* | — | — | **3** |
| `speech play`, `remove` o `synthesize` con etiqueta ilegal | — | — | **2** |

`speech list` no toma `--label`, así que la fila de etiqueta ilegal no la alcanza.

### Qué añade `--json` a las matrices

`--json` no cambia ninguna fila de éxito: el comando hace lo mismo y además emite su payload por stdout. **Bajo `--json`, toda salida no-cero de las tablas anteriores emite además el payload de error** con su `code` y su `message`. El fallo tiene forma observable, y por tanto verificable, en cada fila.

La única interacción entre `--json` y el comportamiento es la regla 2: `--json` con `--play` es exit 2, así que bajo `--json` el bucle es inalcanzable y **la persistencia de `synthesize` es cierta** siempre que la salida sea 0.

## 8. El almacén de habla sintética

### Ubicación y layout

`<data_dir>/speech/<voz>/<etiqueta>.wav` (`crates/avi-store/src/lib.rs` `SpeechStore`), **raíz hermana de `voices/`** (`VoiceStore`: `<data_dir>/voices/<nombre>/`; raíz de modelos: `models_cache_dir()`).

**Por qué no anidado en `voices/<voz>/speech/`**, que sería la opción intuitiva y ahorraría código de borrado: las voces de fábrica (`default` —clonada con `reference.qvoice`—, `ryan` y `vivian` —presets puros sin referencia—; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) son entradas del registro `VoiceStore` —la resolución preset/clonada (`avi-tts/src/lib.rs` `resolve_voice_engine`, presencia de `reference.qvoice`) es detalle interno no-normativo— y el almacén separa la salida generada (`speech/`) del registro (`voices/`).

Coste aceptado de la raíz separada: el arrastre de las locuciones al borrar una voz no es gratis y exige código explícito. Lo hacen los dos caminos que borran voces del usuario con el mismo criterio de propiedad —las locuciones se van con su voz—: `voice remove` (`avi_store::remove_voice`, que borra el namespace de habla antes que el registro) y `cleanup --voices` (que ubica `speech/<voz>` con el mismo `SpeechStore`). Así vale el invariante «locución existente ⇒ voz existente». Como el arrastre borra datos que el usuario no nombró, `voice remove` de una voz con locuciones es destructivo y exige confirmación (ver la sección `voice remove` de `commands/VOICE.md`).

El almacén lo escribe y lo lee **solo el cliente**: es salida de síntesis y el daemon jamás lo toca.

### El `.wav` es el recurso de registro

Cada locución son dos archivos, y **el `.wav` manda**. El `.json` son metadatos derivados.

| Pregunta | La decide |
|---|---|
| ¿La etiqueta existe? | El WAV |
| ¿Hay colisión (exit 6)? | El WAV |
| ¿`speech play` / `speech remove` salen 3? | El WAV |
| ¿Qué enumera `speech list`? | Los WAV |

**`speech remove` borra ambos archivos si están**, de modo que un sidecar huérfano sea removible por su etiqueta aunque `speech list` no lo muestre.

### El sidecar de metadatos

Junto a cada `<etiqueta>.wav` se escribe `<etiqueta>.json` con tres campos: `text`, `voice` y `created_at`. Sin él las etiquetas son opacas: pasadas unas semanas, `saludo2` no le dice nada a nadie.

- **`created_at` en ISO 8601 UTC.**
- **El sidecar es formato interno y no lleva versión de esquema propia.** Su única superficie estable es el payload `--json`, gobernado por `schema_version` (`CLI_SCHEMA_VERSION` en [`json_emitter.rs`](../../crates/avi-core/src/json_emitter.rs)). Darle versión propia daría al proyecto tres versiones de esquema donde hay dos.
- **Un lector que encuentre un campo desconocido lo ignora**, igual que el protocolo IPC del daemon, cuya deserialización ignora las claves desconocidas.
- **`speech list` tolera un sidecar ausente** mostrando la locución sin metadatos (`text`, `created_at` y `duration_secs` a `null` en `--json`), en vez de fallar. Muestra el texto **truncado** en la salida humana y **completo** en el payload `--json`.
- **Un sidecar ilegible o corrupto no se tolera**: `speech list` y `speech play` fallan con exit 1 `io_error`, nombrando el fichero y remitiendo a `speech remove`, que borra el WAV y el sidecar sin interpretarlo.

### Atomicidad de la escritura

Cada archivo se escribe a un temporal en el mismo directorio y se publica con `rename` atómico, de modo que una interrupción no deje un WAV truncado que `speech list` mostraría como válido y `speech play` intentaría reproducir.

**El sidecar se publica antes del WAV**, así que la aparición del `.wav` implica que sus metadatos ya están completos. Combinado con que el WAV es el recurso de registro, una interrupción entre ambos `rename` deja basura inocua: el sidecar huérfano no ocupa la etiqueta, y `speech remove` lo alcanza.

### Validación de identificadores

La etiqueta y el nombre de voz son la misma clase de identificador: un segmento de ruta. Los valida **`crates/avi-store/src/lib.rs` `VoiceStore::validate_name`** (validador único parametrizado por `kind="voz" | "etiqueta"`), que `VoiceStore` y `SpeechStore` invocan en vez de duplicar la regla.

- **El parámetro `kind` determina el sustantivo del mensaje** —«Nombre de voz inválido» frente a «Nombre de etiqueta inválido»—, de modo que `speech synthesize --label "mi saludo"` no culpe a `--voice`. Sin eso, el mensaje de error más frecuente del flag más usado apuntaría a otra cosa.
- **Las etiquetas se normalizan a minúsculas**, porque el validador lo hace deliberadamente para evitar colisiones en filesystems case-insensitive. `--label Saludo` y `--label saludo` son la misma etiqueta, y el archivo se llama `saludo.wav`. Se declara en el help de `--label` y en `USAGE.md`.
- **La defensa anti-escape por `realpath`** corre sobre **ambos** segmentos.
- Un identificador ilegal sale con **2**, sea voz o etiqueta.

## 9. Los códigos de salida

### La tabla

Los códigos de salida con su constante `ExitCode` y su significado están en la tabla de [`README.md`](README.md#códigos-de-salida); este documento fija las reglas que los reparten. Dos significados que esa tabla resume y que aquí son normativos: el `2` incluye `--daemon` o `--no-daemon` en un comando que no los declara (argumento desconocido, `usage_error`), y el `10` no cubre un audio ausente (3), ilegible (2) o con error de E/S (1).

**Los enteros del 11 al 22 son de la tabla cerrada del ciclo de vida, uno por `reason`**, y no se reparten por el eje de dos preguntas de §1 como los anteriores: cada uno corresponde a un `reason` que la [tabla de reglas transversales de la especificación](../specs/LIFECYCLE.md#81-reglas-transversales) declara, y la correspondencia es 1:1 con la variante de `ExitCode` (`crates/avi-core/src/exit_codes.rs`). El `17` lo reciben también los comandos de uso (`translate`, `voice`, `speech`, `setup`, `daemon start`, `daemon restart`, `daemon serve`) mientras dura una operación de ciclo de vida, sin ejecutarse. Los dos casos que rompen el patrón son deliberados y son los que hay que recordar al leer la tabla:

- **El 11 no es un error.** `setup_failed` es un **éxito parcial**: el programa está instalado, el resumen y el envelope se emiten igual, y lo único que cambia es el `reason` del envelope y el código de salida. Por eso el envelope de `self install` sale por *veredicto* y no por el objeto `error` de §10.
- **El 15 y el 17 son los que el ejecutable sin bundle alrededor y el bloqueo ya tomado producen**, y son los dos que un usuario se encuentra sin haber hecho nada mal: `target\debug\ai-voice-interconnector.exe self install` responde `bundle_invalid` (15) porque no hay bundle alrededor, no porque la instalación esté rota.

`unsupported_platform` (18), `binary_incompatible` (19), `network_error` (20), `checksum_mismatch` (21) y `program_dir_kept` (22) **tienen variante propia**: los emiten `self update`, la descarga de `setup`, la comprobación de arranque del bundle y el paso 8 de `self uninstall`, con la traducción en `ExitCode::from_reason` y las variantes en la cabecera de `crates/avi-core/src/exit_codes.rs`.

### Cómo se reparten los enteros

La tabla se deriva del eje de dos preguntas. La segunda es la que reparte los enteros:

| Código | Clase de causa | Siguiente llamada del consumidor |
|---|---|---|
| **1** | Imprevisto | Reintentar a ciegas, registrar o escalar |
| **2** | Invocación mal formada | Corregir el comando y reintentar |
| **3** | Recurso ausente | Crearlo, o nombrar otro |
| **4** | Precondición de entorno: el modelo | `ai-voice-interconnector setup`, luego el mismo comando |
| **5** | Precondición de entorno: el daemon | `ai-voice-interconnector daemon start`, luego el mismo comando |
| **6** | Recurso ocupado | `--force`, otro nombre, `daemon stop`, o esperar a que se libere |
| **9** | Imprevisto, pero en la etapa de traducción | Distinguirlo del fallo de síntesis (**1**) es lo que cambia la siguiente llamada: el modelo TTS puede seguir intentándose sin traducir |

**Los dos casos límite son inversos, y esa simetría es lo que valida el criterio.** El 4 y el 5 son **una** clase por causa —modelo ausente y daemon caído son el mismo tipo de hecho: una precondición de entorno— repartida en **dos** enteros, porque lo único que un consumidor puede convertir en una llamada distinta es un comando de esta CLI: `setup` y `daemon start` se separan. Una precondición de entorno cuyo remedio no es un comando de esta CLI no gana entero: sale con el 1 y su `reason` la distingue. El 6 es lo contrario: **tres** remedios de naturaleza distinta (`--force`, `daemon stop`, cerrar un proceso externo) plegados en **un** entero, porque ninguno cambia lo que el consumidor distingue —«ocupado» frente a «ausente» y «mal escrito»—. La resolución del entero es la de lo que este programa puede nombrar como paso ejecutable.

**Los `reason` declarados.** Todo `reason` que emite la CLI, el daemon o el ciclo de vida está en una tabla única (`REASON_CODES` en `crates/avi-core/src/exit_codes.rs`) con su código, y una prueba falla ante un `reason` emitido sin declarar o declarado sin emitir:

| Código | `reason` |
|---|---|
| `1` | `io_error`, `voice_clone_failed`, `synthesis_failed`, `synthesis_timeout`, `stt_unsupported`, `translation_unsupported`, `denoise_unsupported`, `denoise_failed`, `playback_failed`, `audio_enumeration_failed`, `speech_list_failed`, `voice_list_failed`, `voice_store_init_failed`, `daemon_error`, `startup_failed`, `sudo_not_supported`, `lifecycle_failed`, `doctor_failed`, `self_install_failed`, `self_uninstall_failed`, `self_update_failed` |
| `2` | `confirmation_required`, `usage_error`, `empty_text`, `text_too_long`, `invalid_voice_name`, `invalid_identifier`, `cannot_remove_default`, `invalid_audio`, `empty_audio`, `audio_too_long` |
| `3` | `voice_not_found`, `warm_voice_missing`, `audio_not_found`, `speech_not_found`, `not_installed` |
| `4` | `model_missing` |
| `5` | `daemon_unreachable` |
| `6` | `voice_exists`, `label_exists`, `port_in_use`, `denoise_busy` |
| `9` | `translation_failed` |
| `10` | `transcription_failed` |
| `11` a `22` | `setup_failed`, `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`, `daemon_stop_failed`, `lifecycle_locked`, `unsupported_platform`, `binary_incompatible`, `network_error`, `checksum_mismatch`, `program_dir_kept`, en ese orden |

`warm_voice_missing`, `model_missing` y `startup_failed` viajan en la línea `error=<reason>` del fichero ready del daemon; la CLI traduce el primero a `voice_not_found` (3), conserva `model_missing` (4) y traduce el tercero a `daemon_error` (1).

**Los tres `reason` de la limpieza de ruido** distinguen tres hechos que el consumidor trata distinto: `denoise_unsupported` (1) significa que el binario se compiló sin `native-denoise` y la limpieza no existe en esa instalación; `denoise_busy` (6) significa que hay una inferencia DPDFNet activa y otra solicitud esperando, así que la limpieza no se inició y conviene reintentar cuando termine la solicitud en curso; `denoise_failed` (1) significa que el modelo estaba provisionado y la inferencia o el DSP fallaron. En los tres casos la solicitud **no** devuelve audio sin limpiar: `model_missing` (4) es la precondición de modelo y se comprueba antes de reservar buffers o crear temporales.

**El 6 tiene un solo dueño.** «Puerto ya en uso» y «la voz ya existe» son el mismo hecho y llevan el mismo código; no hay una constante aparte para el conflicto del daemon.

### El 2 significa lo que `clap` quiere decir con él

El exit 2 es, en Unix y en `clap`, el código del error de invocación, y aquí significa exactamente eso. Como consecuencia, **todas las rutas de fallo de parseo son correctas sin escribir una línea de validación**: flag requerido ausente, valor fuera de `choices`, grupo mutuamente excluyente violado (`conflicts_with`), subcomando inválido en los tres niveles, y flag desconocido en cualquier comando.

**La raíz sin subcomando explora (0); un grupo sin subcomando es un error de uso (2).** `ai-voice-interconnector` a secas muestra la ayuda y sale con `ExitCode::Ok`, igual que `--help` (con `--json`, el payload `help`), porque quien lo invoca así, persona o agente, necesita saber cómo corregir la invocación. `ai-voice-interconnector speech` a secas (y `voice`, `daemon`, `self`) sale con 2 y trae la ayuda del grupo, porque un script que olvida el subcomando de `self` o `daemon` no debe terminar con éxito sin hacer nada.

Dos pruebas de que la convención es la correcta:

1. **La tabla la honra en otro punto**: `ExitCode::Interrupted = 130` es exactamente `128 + SIGINT`. Respetar 128+n y no respetar 2 sería incoherente dentro de la misma tabla.
2. **El proyecto hermano aplica la misma convención**: `agent-voice-interconnector` usa **2 = uso incorrecto** en sus tres casos —valor fuera de dominio, argumento vacío y comando desconocido— con **1 = error genérico**.

### Dónde viven las constantes, y por qué eso es parte del contrato

**Las constantes viven en `crates/avi-core/src/exit_codes.rs` (`ExitCode`), sin dependencias circulares.** Un crate hoja sin imports del binario **no puede** cerrar un ciclo, así que la justificación que empujaría una constante a declararse fuera del módulo no está disponible ni siquiera como pretexto. `crates/avi-core/src/json_emitter.rs` (`emit_raw_json`) y `src/main.rs` (`Cli::parse`, `handle_*`) reexportan el contrato, de modo que `ExitCode::InvalidInput` es el nombre canónico.

**Un contrato cerrado sin un lugar legítimo donde crecer no impide el crecimiento: lo empuja fuera del campo de visión.** El dueño es el crate `avi-core`, no una advertencia.

**Dos invariantes de gobernanza lo sostienen**, y son distintos:

1. **Ningún `ExitCode` puede definirse fuera de `crates/avi-core/src/exit_codes.rs`.** Un test recorre los crates y falla ante una definición con ese prefijo en cualquier otro archivo.
2. **La tabla de [`docs/CLI/README.md`](README.md#códigos-de-salida) y el módulo dicen lo mismo.** Un test exige que esa tabla pública tenga una fila por cada variante de `ExitCode`. Un código declarado por fuera y además sin documentar es invisible dos veces.

La reexportación desde `src/main.rs` crea dos sitios donde *parecen* vivir las constantes; el primer invariante lo desactiva —cualquier definición fuera del crate hoja falla—, así que la reexportación es un alias y no una segunda declaración. La distinción queda escrita en el crate.

**El comentario del crate** enuncia el criterio generador en sus dos tiempos —clase de causa y admisión por la siguiente llamada del consumidor—, fecha el congelamiento de la tabla **en la 1.0**, advierte que un intercambio de valores es indetectable para un consumidor, y recoge el criterio de revisión que no puede ser test. La versión del esquema es `schema_version` (`CLI_SCHEMA_VERSION` en [`json_emitter.rs`](../../crates/avi-core/src/json_emitter.rs)).

**Dos reglas transversales, y solo una es mecanizable.**

- **Test**: ningún `ExitCode::Error` puede alcanzarse por una causa prevista con remedio declarado en su propio mensaje. Un `ExitCode::Error` cuyo mensaje contenga «reintenta» es por construcción un olvido.
- **Criterio de revisión, no test**: ningún `ExitCode::InvalidInput` puede alcanzarse con una invocación bien formada. «Bien formada» no tiene definición ejecutable, y escribirla como test produciría una aserción que no afirma nada. Su lugar es el comentario del módulo, junto al criterio generador.

## 10. El canal de error y los payloads

### La invariante del canal

**Bajo `--json`, toda salida no-cero emite el payload de error, salvo la salida por veredicto y los resultados JSONL propios de `speech denoise`.** `error` (el mensaje) y `reason` son obligatorios en el payload; el código **no viaja en el envelope**, lo transporta el estado de salida del proceso, y por eso un consumidor tiene que leer las dos cosas.

Los demás comandos tienen **tres formatos** y emiten **exactamente un objeto JSON** por invocación. `speech denoise` conserva su contrato JSONL: emite una línea JSON por archivo y un resumen final, sin segundo envelope al devolver un veredicto:

1. **Éxito**: el payload propio del comando, vía `emit_raw_json()`, con salida 0. La ayuda (`--help`, `-h` y la raíz sin subcomando) emite `{"help"}` y `--version`/`-V` el payload de `version`, de modo que bajo `--json` no queda ninguna salida en texto.
2. **Error**: el objeto `{"error": …, "reason": …}`, vía `CliError` traducido por `main()`, con salida ≠ 0.
3. **Veredicto**: código ≠ 0 con el payload **propio** del comando ya emitido y **sin** objeto `error`. Es un dictamen, no un fallo: el comando corrió sin error pero su resultado es negativo. Hay **tres** casos: **`doctor`**, cuyo exit 1 con FAIL (§9) emite solo el reporte (`checks`, `failed`); **`self install` con `setup_failed`**, que emite su envelope completo con `status` `installed` y sale con **11** sin adjuntar objeto `error` (§11); y **`daemon stop` cuando la parada no concluye**, que emite su envelope con `status` `still_running` y el veredicto completo del motor y sale con **5** sin adjuntar objeto `error` (§10).

El payload de error usa **dos claves de primer nivel** —`error` con el mensaje y `reason` con el literal de máquina—, ambas planas y no anidadas, y deja intacto el stderr en castellano para el uso humano. Los ejemplos de payload están en la [referencia de esquemas `--json`](../../USAGE.md#referencia-de-esquemas---json).

`reason` es la clave de la causa fina y es **aditiva**: los `reason` del ciclo de vida (`lifecycle_locked`, `bundle_invalid`, `daemon_stop_failed`, `path_conflict`, `rolled_back`, `externally_managed`, `confirmation_required`, `usage_error`, `unsupported_platform`, `binary_incompatible`, `network_error`, `checksum_mismatch`, `program_dir_kept`, `sudo_not_supported`) viajan por ella; los del ciclo de vida tienen además entero propio del 11 al 22, salvo `confirmation_required` y `usage_error` (2) y `sudo_not_supported` (1), que rechaza en Unix `self install`, `self uninstall` y `cleanup` lanzados con `sudo`, y un `reason` desconocido se trata como ausente. El fallo de parseo lleva `reason: "usage_error"`.

Las tres reglas de compatibilidad y la regla de promoción son contrato **de consumo** además de emisión: [`USAGE.md`](../../USAGE.md#comandos) declara explícitamente que un `reason` desconocido se trata como ausente.

### El mecanismo: un solo punto de traducción

**La invariante no se sostiene con un `if` por sitio**, porque eso la deja en manos de que nadie olvide uno. Es la misma solución que la ruta de éxito ya tiene con `emit_raw_json` (`crates/avi-core/src/json_emitter.rs`), cuyo doc enuncia el motivo: *«la garantía queda en un solo lugar, no repetida por convención en cada comando»*. La ruta de fallo tiene la misma forma:

- Los sitios de fallo retornan **`ExitCode` + `reason` + `message`** (tipo `CliError` en `crates/avi-core/src/exit_codes.rs` / `crates/avi-core/src/json_emitter.rs`) en vez de imprimir y salir.
- **`src/main.rs` (`main` / `handle_*` / `emit_raw_json`) es el único punto que lo traduce**: mensaje humano a stderr, payload a stdout si se pidió `--json`, y salida con el código. No queda otro camino hasta la salida, así que la invariante no necesita vigilancia.
- El invariante que la protege es mecanizable: **ninguna salida no-cero fuera de `src/main.rs`**.
- **La traducción de `reason` a código vive en una sola tabla**, `ExitCode::from_reason` (`crates/avi-core/src/exit_codes.rs`). La usan todos los sitios que reciben un `reason` ya formado: la vía daemon (respuestas HTTP no 2xx y eventos `error` del stream NDJSON), el ciclo de vida y sus pruebas. Así la misma causa sale con el mismo código por cualquier vía, y añadir un `reason` obliga a tocar un solo sitio. Un `reason` que la tabla no conoce sale con 1.

**La salida por veredicto entra por el mismo punto único.** Un comando que ya emitió su payload propio y quiere salir con código ≠ 0 sin adjuntar objeto `error` **devuelve el entero** del código; `src/main.rs` honra un retorno `ExitCode` ≠ 0 y sale con ese código. No hay tipo de error nuevo disperso: la salida sigue pasando por `main`, así que **ninguna salida no-cero fuera de `src/main.rs`** se mantiene. `doctor` es el caso que lo usa: emite su reporte y retorna `ExitCode::Error` cuando hay FAIL.

**`CliError` es señal de control de flujo, no error de dominio.** Una señal de flujo no debe ser capturable por un manejador genérico; en Rust se modela como tipo propio en `avi-core` que `src/main.rs` traduce, sin propagación silenciosa por handlers genéricos. Un test afirma la separación entre error de dominio y señal de salida.

**El fallo de parseo entra por el mismo canal.** `src/main.rs` (`Cli::parse` con `clap`) traduce el error de parseo a `ExitCode::InvalidInput` (`"usage_error"`) en vez de imprimir y salir: así el texto que `clap` ya calcula entra al payload en vez de perderse, y el 2 —el fallo más frecuente que verá un consumidor programado— deja stdout tan poblado como cualquier otro. `Cli::parse` corre dentro del mismo handler. Queda un residuo honesto: al fallar el parseo no existe `Cli`, así que hay que inspeccionar los args crudos para saber si se pidió `--json`; decide *si* emitir, no qué, y vive en un único sitio.

**Ayuda, versión y grupo sin subcomando se interceptan en el mismo punto.** `clap` los reporta como «errores» de tipo no mapeado (`DisplayHelp`, `DisplayVersion`, `DisplayHelpOnMissingArgumentOrSubcommand`) y su render original imprime texto aunque se haya pedido `--json`. Bajo `--json`, `render_clap_error` emite la ayuda como `{"help"}` con salida 0, la versión como el payload de `version` con salida 0, y el grupo sin subcomando como `usage_error` cuyo `error` es «Error: falta un subcomando» seguido de la ayuda del grupo, con salida 2. Sin `--json` conservan el render original. Los protegen `no_subcommand_shows_root_help`, `json_root_help_is_payload`, `json_nested_help_is_payload_of_that_level`, `json_version_flags_match_version_command` y `group_without_subcommand_is_usage_error` en `tests/cli_golden.rs`.

**`daemon serve` queda fuera del mecanismo, y por una razón concreta: no acepta `--json`.** No hay payload que emitir, así que la invariante del canal no tiene alcance ahí y ese comando sale directamente. Esa es la condición que lo autoriza y ninguna otra: darle `--json` reabriría el hueco.

### stderr, trazas y logs

**stderr solo lleva mensajes del producto y avisos**, nunca trazas internas por defecto. La verbosidad la gobierna `RUST_LOG`, con sintaxis de `tracing` y tolerante a directivas inválidas (se ignoran en vez de abortar):

- **Con `RUST_LOG` definida, manda**: aplica tanto a la CLI como a `daemon serve`.
- **Sin ella** (o vacía), la CLI emite `warn` y superiores; `daemon serve` emite `info` de los crates propios y `warn` de las dependencias.
- El progreso de la descarga de modelos (`setup` y `self install`) es un mensaje del producto, no una traza: sale siempre por stderr. Las barras solo se dibujan con terminal; el encabezado, la línea de cada modelo terminado y la de cierre salen también sin ella.
- stderr no lleva color ANSI cuando no es una terminal.
- **`AVI_NO_PROGRESS` silencia el progreso de fase.** Con cualquier valor no vacío, el indicador no escribe nada: ni la fase, ni la actividad, ni la cesión de línea. El alcance es **solo stderr**: el envelope `--json`, el texto del resultado y los códigos de salida no cambian, y el mensaje final del handler sigue saliendo por stdout. Es el interruptor para un pipeline o un log donde el progreso estorba.

**Los logs viven en `data/logs/`** y hay dos familias, ambas con nombre `<familia>_<token>_<ms>.log`, donde `<token>` es el token de propietario `<pid>-<inicio>` del proceso que escribe el log y `<ms>` la marca de tiempo de creación en milisegundos:

| Familia | Fichero | Contenido |
|---|---|---|
| Daemon | `daemon_<token>_<ms>.log` | stdout y stderr del daemon; uno por cada `daemon start`/`restart` (`<token>` es el del daemon hijo) |
| Motor Qwen3-TTS | `qwen3-tts_<token>_<ms>.log` | salida del motor residente y del motor de `voice clone` (`<token>` es el del proceso que lo crea: el daemon, o la CLI en la vía directa) |

El borrado diferido de Windows no escribe ningún log: un borrado correcto no deja registro, y si falla deja `<destino>.removal-failed.log` junto al destino.

**Retención: los 10 logs más recientes por familia**, ordenados por el `<ms>` del nombre. La poda ocurre al crear un log nuevo; no toca otras familias ni ficheros ajenos, y nunca borra el log de un propietario que sigue en marcha ni el de uno cuya identidad no se puede observar: solo compiten por la retención los de dueño muerto (inexistente o con otro inicio) y los del propio proceso. Un log que no se pueda borrar se ignora.

Cuando un arranque del daemon falla, el mensaje de error de la CLI termina con `Log del daemon: <ruta>` sin cambiar `reason` ni el exit code; si el log no se puede crear, el error es `daemon_error` («No se pudo crear el log del daemon»). El error `voice_clone_failed` termina con `Log del motor: <ruta>`.

### El contrato de salida por comando

Esta sección es el **contrato de salida** de cada comando: qué escribe por stdout, qué escribe por stderr, con qué código de salida y con qué `reason`. Los destinos, los códigos y los `reason` no cambian con el protocolo del indicador; lo que el protocolo fija es **quién dice el estado y cuántas veces**.

**Las tres reglas que la gobiernan.**

1. **stdout es el contrato y stderr es la explicación.** Bajo `--json`, stdout lleva un objeto JSON (§10), salvo los registros JSONL de `speech denoise`; stderr lleva el progreso, los avisos y el mensaje de error. Sin `--json`, stdout lleva el texto del resultado y stderr lleva el progreso y los avisos. En ningún caso el progreso —fase o actividad— llega a stdout: por eso el indicador escribe siempre por stderr.
2. **La fase es un mensaje de stderr, y su forma es `… <mensaje> (<tiempo>)`.** El indicador **no imprime estado**: no hay cierre `[ok]` ni `[error]`. El estado lo dice **quien cierra** —el handler de la CLI, o el flujo de la librería en `install` y `uninstall`—, por stdout y **una sola vez**, con la duración de la operación. En no-TTY (la forma en que un script lee la salida) cada fase es una línea propia y la actividad repetida dentro de la misma fase no abre línea nueva; en TTY la fase se repinta sobre la misma línea.
3. **El `reason` solo viaja en la salida no-cero**, y solo en el payload de error (§10). En el éxito es `null` o está ausente; en el veredicto (§10) el `reason` propio del comando viaja dentro del payload del comando, no en un objeto `error` aparte.

**Excepción de salida para `speech denoise`.** El flag global `--json` de este comando selecciona JSONL: stdout contiene cero o más objetos JSON independientes, uno por evento de progreso/resultado, no un único envelope `schema_version`; cada registro mantiene su forma definida por el reporte del pipeline. Los errores de ejecución se expresan como registros JSONL y por el código de salida; el texto de ayuda/parseo sigue el canal general de errores de CLI. Sin `--json`, mensajes y progreso son texto humano según el comando. Esta excepción se limita a la interfaz del pipeline de archivos y no cambia el envelope `--json` ni el protocolo NDJSON del daemon para otros comandos.

**La tabla.** «Fase» son los literales de fase que el comando nombra; «Mensaje final» es el estado que dice el dueño, por stdout, con su duración. Un guion significa que el comando no emite esa clase de salida.

| Comando | stdout (texto) | stdout (`--json`) | stderr | Fase(s) | Mensaje final | Código y `reason` |
|---|---|---|---|---|---|---|
| `translate` (origen = destino) | el texto original | `{ "translated", "source_language", "target_language" }` | — | — | — | 0 |
| `translate` (vía daemon) | el texto traducido | `{ "translated", "source_language", "target_language" }` | la fase | `translating` | el texto traducido, tal cual | 0; 9 `translation_failed`; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable` |
| `translate` (local) | el texto traducido | `{ "translated", "source_language", "target_language" }` | la fase | `translating` | el texto traducido, tal cual | 0; 9 `translation_failed`; 2 `empty_text`, `translation_unsupported`; 4 `model_missing` |
| `voice list` | «Voces registradas:» y la lista | `{ "voices": [...] }` | — | — | — | 0; 1 `voice_list_failed` |
| `voice clone` (vía daemon) | «Voz '\<nombre\>' clonada (N s).» | `{ "name", "speech", "precomputed" }` | las fases | `validating`, `cloning` | «Voz '\<nombre\>' clonada (N s).» | 0; 1 `voice_clone_failed`, `io_error`; 2 `invalid_voice_name`; 3 `audio_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `voice_exists` |
| `voice clone` (local) | «Voz '\<nombre\>' clonada (N s).» | `{ "name", "speech", "precomputed": false }` | las fases | `validating`, `cloning`, `saving` | «Voz '\<nombre\>' clonada (N s).» | 0; 1 `voice_clone_failed`, `io_error`; 2 `invalid_voice_name`, `invalid_audio`; 3 `audio_not_found`; 4 `model_missing`; 6 `voice_exists` |
| `voice remove` | «Voz '\<nombre\>' eliminada (N s).» o «Cancelado.» | `{ "status": "removed"\|"cancelled", "voice", "removed_speech" }` | el plan de borrado y la confirmación | `removing` | «Voz '\<nombre\>' eliminada (N s).» | 0; 1 `io_error`; 2 `cannot_remove_default`, `invalid_voice_name`; 3 `voice_not_found` |
| `speech list` | «Habla sintética albergada:» y la lista | `{ "speech": [...] }` | — | — | — | 0; 1 `io_error`, `speech_list_failed`; 3 `voice_not_found` |
| `speech transcribe` (vía daemon) | el texto transcrito | `{ "text", "source_language" }` | las fases | `capturing` (solo `--mic`), `reading` (solo `--audio`), `transcribing` | el texto transcrito, tal cual | 0; 1 `daemon_error`; 2 `usage_error`, `invalid_audio`, `audio_too_long`; 3 `audio_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 10 `transcription_failed` |
| `speech transcribe` (local) | el texto transcrito | `{ "text", "source_language" }` | las fases | `capturing` (solo `--mic`), `reading` (solo `--audio`), `transcribing` | el texto transcrito, tal cual | 0; 1 `io_error`; 2 `usage_error`, `invalid_audio`, `audio_too_long`; 3 `audio_not_found`; 4 `model_missing`; 10 `transcription_failed`, `stt_unsupported` |
| `speech synthesize` (vía daemon) | «Síntesis completada: \<ruta\> (N s).» | `{ "status": "success", "audio_path", "voice" }` | las fases | `validating`, `synthesizing`, `saving` | «Síntesis completada: \<ruta\> (N s).» | 0; 1 `synthesis_failed`, `daemon_error`; 2 `usage_error`, `text_too_long`; 3 `voice_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `label_exists`; 9 `translation_failed` |
| `speech synthesize` (local) | «Síntesis completada: \<ruta\> (N s).» o «Descartado.» | `{ "status": "success", "audio_path", "voice" }` o `{ "status": "discarded", "voice" }` | las fases | `validating`, `translating` (si difieren los idiomas), `synthesizing`, `saving` | «Síntesis completada: \<ruta\> (N s).» | 0; 1 `synthesis_failed`, `io_error`; 2 `usage_error`, `text_too_long`; 3 `voice_not_found`; 4 `model_missing`; 6 `label_exists`; 9 `translation_failed` |
| `speech say` (vía daemon) | «Reproduciendo (N s).» | `{ "status": "reproduced", "voice" }` | las fases | `validating`, `synthesizing`, `playing` | «Reproduciendo (N s).» | 0; 1 `synthesis_failed`, `daemon_error`; 2 `usage_error`, `text_too_long`; 3 `voice_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 9 `translation_failed` |
| `speech say` (local) | «Reproduciendo (N s).» | `{ "status": "reproduced", "voice" }` | las fases | `validating`, `translating` (si difieren), `synthesizing`, `playing` | «Reproduciendo (N s).» | 0; 1 `synthesis_failed`, `playback_failed`, `io_error`; 2 `usage_error`, `text_too_long`; 3 `voice_not_found`; 4 `model_missing`; 9 `translation_failed` |
| `speech dub` (vía daemon) | «Doblaje reproducido (N s).» | `{ "status": "dubbed", "text" }` | las fases y el stream NDJSON del daemon | `capturing` (solo `--mic`) o `reading`, `dubbing`, `transcribing`, `translating`, `synthesizing`, `playing` | «Doblaje reproducido (N s).» | 0; 1 `daemon_error`; 2 `usage_error`, `invalid_audio`, `audio_too_long`; 3 `audio_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 9 `translation_failed`; 10 `transcription_failed` |
| `speech dub` (local) | «Doblaje reproducido (N s).» | `{ "status": "dubbed", "text" }` | las fases | `capturing` (solo `--mic`) o `reading`, `transcribing`, `translating` (si difieren), `synthesizing`, `playing` | «Doblaje reproducido (N s).» | 0; 1 `playback_failed`, `io_error`; 2 `usage_error`, `empty_text`, `invalid_audio`, `audio_too_long`; 3 `audio_not_found`; 4 `model_missing`; 9 `translation_failed`; 10 `transcription_failed`, `stt_unsupported` |
| `speech play` | «Reproduciendo locución '\<etiqueta\>' de la voz '\<voz\>' (N s).» | `{ "status": "played", "label", "voice" }` | las fases | `playing` | «Reproduciendo locución '\<etiqueta\>' de la voz '\<voz\>' (N s).» | 0; 1 `playback_failed`; 2 `invalid_identifier`; 3 `speech_not_found`, `voice_not_found` |
| `speech remove` | «Locución '\<etiqueta\>' de la voz '\<voz\>' eliminada.» | `{ "status": "removed", "label", "voice" }` | — | — | — | 0; 1 `io_error`; 2 `invalid_identifier`; 3 `speech_not_found`, `voice_not_found` |
| `daemon start` | «Daemon iniciado correctamente (pid N, N s).» o «Daemon ya en ejecución (pid N).» | `{ "status": "started"\|"already_running", "daemon": "running", "pid" }` | las fases; el aviso de daemon residual degradado | `stopping` (si hay residual), `starting`, `warming` | «Daemon iniciado correctamente (pid N, N s).» | 0; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `port_in_use`; 16 `daemon_stop_failed` |
| `daemon stop` | «Señal de apagado enviada al daemon en \<addr\> (N s).» o «No había daemon que detener (N s).» | `{ "status", "daemon": "stopped", "was_running", "daemon_fully_stopped" }` | las fases | `stopping` | «Señal de apagado enviada al daemon en \<addr\> (N s).» | 0; 5 (veredicto `still_running`, sin objeto `error`) |
| `daemon restart` | «Daemon reiniciado (pid N, N s).» | `{ "status": "restarted", "daemon": "running", "pid" }` | las fases | `stopping`, `starting`, `warming` | «Daemon reiniciado (pid N, N s).» | 0; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `port_in_use`; 16 `daemon_stop_failed` |
| `daemon status` | una línea de estado | el reporte de estado | — | — | — | 0; 1 `daemon_error`; 5 `daemon_unreachable` |
| `daemon serve` | — (no acepta `--json`) | — | el log del daemon y sus avisos | — | — | 0; 1 `daemon_error`, `startup_failed` |
| `setup` | «Setup completado: N modelo(s) disponibles.» | `{ "status": "completed", "models_provisioned" }` | el bloque de descarga de modelos (§ «stderr, trazas y logs») | — | el cierre del bloque de provisión | 0; 1 `voice_store_init_failed`, `io_error`; 11 `setup_failed`; 20 `network_error` |
| `cleanup` | «Limpieza completada: N ruta(s), M barrido(s) (N s).» o «Cancelado.» | `{ "status", "reason", "removed", "planned", "unreadable" }` | la fase, el plan de borrado y los avisos de ruta | `cleaning` | «Limpieza completada: N ruta(s), M barrido(s) (N s).» | 0; 1 `io_error`; 2 `usage_error`, `confirmation_required`; 16 `daemon_stop_failed` |
| `self install` | «ai-voice-interconnector instalado: \<versión\> (N s).» o «ai-voice-interconnector reparado: \<versión\> (N s).», y el resumen con el programa, el comando, el `PATH` y los modelos | `{ "status", "reason", "install_dir", "version", "channel", "path_integrated", "models" }` | las fases del motor y el resumen por stderr | `stopping`, `replacing`, `path`, `receipt` | «ai-voice-interconnector instalado: \<versión\> (N s).» o «ai-voice-interconnector reparado: \<versión\> (N s).» | 0; 1 `self_install_failed`, `io_error`; 2 `usage_error`, `confirmation_required`; 11 `setup_failed`; 13 `rolled_back`; 14 `path_conflict`; 15 `bundle_invalid`; 16 `daemon_stop_failed`; 17 `lifecycle_locked`; 18 `unsupported_platform`; 19 `binary_incompatible`; 20 `network_error`; 21 `checksum_mismatch` |
| `self update` | «ai-voice-interconnector actualizado: A → B (N s).» | `{ "status", "reason", "previous_version", "version", "channel" }` | las fases del indicador y los avisos de la recuperación | `checking`, `downloading`, `verifying`, `stopping`, `replacing` | «ai-voice-interconnector actualizado: A → B (N s).» | 0; 1 `self_update_failed`; 2 `usage_error`, `confirmation_required`; 3 `not_installed`; 11 `setup_failed`; 12 `externally_managed`; 13 `rolled_back`; 16 `daemon_stop_failed`; 17 `lifecycle_locked`; 18 `unsupported_platform`; 19 `binary_incompatible`; 20 `network_error`; 21 `checksum_mismatch` |
| `self uninstall` | «Desinstalación completada (N s).» o «Quedaron restos sin borrar (N s).» o «No había nada que desinstalar.» o «Cancelado.» | `{ "status", "reason", "removed", "planned", "path_reverted" }` | las fases del motor y el plan de borrado | `stopping`, `state`, `path`, `program`, `locks` | «Desinstalación completada (N s).» o «Quedaron restos sin borrar (N s).» o «No había nada que desinstalar.» | 0; 1 `self_uninstall_failed`, `io_error`; 2 `usage_error`, `confirmation_required`; 17 `lifecycle_locked`; 22 `program_dir_kept` (veredicto, sin objeto `error`) |
| `doctor` | «Diagnóstico: todo correcto.» y la lista de chequeos con `✓` | el reporte (`checks`, `failed`, …) | los chequeos fallidos con `✗`, la línea de veredicto «Diagnóstico: N comprobación(es) fallan.» y la pista de reparación | `repairing` (solo con `--repair`) | «Reparación completada (N s).» | 0; 1 `doctor_failed`; 1 (veredicto, sin objeto `error`) cuando hay comprobaciones fallidas |
| `devices` | «Dispositivos de salida de audio:» y la lista | `{ "devices": [...] }` | — | — | — | 0; 1 `audio_enumeration_failed` |
| `--version` | «ai-voice-interconnector X.Y.Z» | `{ "name", "version" }` | — | — | — | 0 |
| `--help` (o raíz sin subcomando) | la ayuda | `{ "help" }` | — | — | — | 0; 2 `usage_error` (grupo sin subcomando) |

**Lo que la tabla no cubre, y por qué.** La tabla fija el contrato de **éxito y de error por `reason`**, que es lo que un consumidor programado lee. No fija el texto literal de los mensajes de error —son mensajes humanos y cambian sin romper el contrato—, ni el orden de las líneas de stderr, que es propio de cada comando. La enumeración completa de los `reason` y su código está en §9.

**El indicador y el envelope.** Un comando con indicador escribe por stderr **una clase** de línea: la fase (`… <mensaje> (<tiempo>)`). La actividad —la fase repetida o un latido del daemon— no abre línea nueva en no-TTY, porque se raciona por fase, y en TTY repinta sobre la misma línea, con la misma forma. El indicador **no dice cómo terminó**: no hay cierre `[ok]` ni `[error]`. El resultado del comando —el texto o el envelope— va por stdout, y su dueño es el handler (o el flujo de la librería en `install` y `uninstall`), que lo dice **una sola vez** y le añade la duración de la operación. El estado se dice una vez: lo dice quien cierra.

**La paridad de fases.** Las dos vías de un comando nombran las mismas fases cuando la entrada es equivalente. La paridad está acotada por lo que el daemon puede anunciar: su wire expone un conjunto de etapas, y las que la vía local nombra y el daemon no anuncia quedan como asimetría declarada —la traducción de `synthesize` y `say`, que el daemon hace de forma síncrona sin emitir su propia etapa, y el guardado de la voz clonada, que el daemon persiste sin evento de guardado—. Las fases que la CLI nombra en las dos vías —la captura y la lectura del audio, que ocurren antes del despacho; el guardado de la locución y la reproducción, que hace la CLI— se nombran igual en ambas. El mapa de etapas está en [`commands/DAEMON.md`](commands/DAEMON.md).
### Los payloads del grupo `speech`

La forma de cada payload está en la [referencia de esquemas `--json`](../../USAGE.md#referencia-de-esquemas---json) y en [`commands/SPEECH.md`](commands/SPEECH.md). Las reglas que fija el contrato:

- Solo `synthesize` emite ruta (`audio_path`), por el criterio de la ruta en los payloads: produce el WAV y no hay comando de exportación. Es el único cuyo WAV sobrevive a la invocación; `label` va implícito en `audio_path`. Bajo `--json` el bucle es inalcanzable y la persistencia es cierta cuando la salida es 0.
- **`say`** no persiste y no emite ruta: sintetiza a un temporal autogestionado dentro del scratch del proceso (`avi_say_…`, con dueño `TempWav` en `avi-shared`, que lo borra al salir de ámbito también si falla la reproducción); no repite `text` ni informa `audio_path`. El texto humano («Reproduciendo.») tampoco muestra ruta. `say` es humo de punta a punta: afirma que sintetizó y reprodujo, sin medir la calidad del audio; la paridad de inteligibilidad la cubre `synthesize`, que persiste y verifica.
- **`dub`** tampoco persiste ni emite ruta: temporal autogestionado igual (`avi_dub_…`), sin `audio_path`. Quien quiera conservar el audio usa `speech synthesize`. Como `say`, es humo sin puerta de calidad sobre lo que sonó.
- **`synthesize` y `say` comparten `voice`** porque si no se pasó `--voice` lo eligió el sistema (`default`); `dub` no lo emite, y su `text` es lo que necesita el consumidor.
- **`list`** emite el texto completo. La clave es el nombre del recurso en snake_case, siguiendo el precedente de `voice list --json` (una lista bajo el nombre del recurso) y evitando que un identificador del contrato legible por máquina contradiga el vocabulario de la superficie.
- **`remove`** no lleva campo de resultado: el código de salida ya transporta la información (0 = se borró, 3 = no existía). Un campo `removed` chocaría además con `cleanup --json`, que emite `removed` como lista de rutas, y la misma clave con dos tipos bajo una sola versión de esquema es justo lo que un consumidor tipado no puede manejar.

Los payloads de `daemon start` y `restart` no llevan clave booleana propia: el fallo se reporta por el payload de error como en el resto de la CLI. `stop` sí bindea booleanos, porque son el veredicto del motor y no un eco de la entrada. `start` distingue sano de degradado sin bump: sano responde `already_running` y degradado reclama el árbol y rearranca con `started`, ambos con salida 0. `stop` bindea el veredicto del motor (`StopOutcome`, calculado antes de tocar nada): con la muerte verificada responde `status` `not_running` (sin daemon previo) o `shutdown_sent`, con `daemon: "stopped"`, `was_running` y `daemon_fully_stopped` y exit 0; si el árbol o el residente siguen vivos sale por veredicto con `status` `still_running` y exit 5, sin objeto `error` y sin borrar la pista. Un arranque fallido de `start` o `restart` no deja proceso, pidfile ni fichero ready. El readiness lo transporta el fichero de `--ready-file` (escritura atómica), con registro de éxito (`addr`) o de fallo (nunca `addr`). El envelope de `stop`, la búsqueda del daemon, el reclamo, el arranque, la dirección y los `reason` de fallo con su exit están en [`commands/DAEMON.md`](commands/DAEMON.md#gestión-del-ciclo-de-vida) y [`commands/DAEMON.md`](commands/DAEMON.md#códigos-de-error).

**`daemon.pid` y contabilidad del residente:** `data_dir()/daemon.pid` lo escribe solo el daemon, completo y de forma atómica, tras enlazar el puerto y antes de publicar `daemon.ready`; la CLI solo lo lee y lo borra tras una parada verificada. Un PID registrado cuenta como vivo y propio solo si la identidad observada del proceso coincide con la registrada; si no coincide o no se puede observar, es un proceso ajeno: se trata como muerto y nunca se mata. Un fichero sin las claves de identidad es ilegible: se descarta con aviso y se propaga como ausencia, sin degradar a «PID solo». El residente TTS se registra al lanzarlo y se liquida por su PID registrado e identidad, nunca por imagen ni sondeando puertos; la vía directa (`--no-daemon`) no registra nada ni toca `daemon.pid`. Con `AVI_DATA_DIR` el pidfile es por instancia. Las claves del esquema y el resto del registro están en [`commands/DAEMON.md`](commands/DAEMON.md#esquema-de-daemonpid). Las pruebas del harness operan con aislamiento total por instancia (`IsolatedInstance`) y semáforo de capacidad para inferencia (file-lock exclusivo del SO sobre `target/tts-serial.lock`, tomado en `lock_tts()`).

### Las dos versiones de esquema

Son **dos, independientes**: el envelope de la CLI va por `"5"` y el protocolo del daemon también va por `"5"`, y cada uno cubre su propia superficie.

- **`crates/avi-daemon/src/lib.rs` (`DaemonState`, `run_daemon_server`) — protocolo IPC del daemon, `DAEMON_SCHEMA_VERSION = "5"` (`crates/avi-core/src/json_emitter.rs`).** `/synthesize` identifica la voz por su nombre y devuelve audio sin ruta de destino; `/voices/clone` recibe bytes e identificador y devuelve `speech`, la ruta calculada por el daemon en su filesystem; `/health` informa `warm` (`warming`/`warm`/`warm_failed`, con `warm_error` si falló) y, cuando están residentes, `stt` y `ct2`; `/transcribe` responde 400 (`usage_error`, `invalid_audio`) y 500 ante errores. **El ciclo de vida no lo toca**: ni `self install`, ni `self uninstall`, ni `cleanup`, ni la sección de ciclo de vida de `doctor` cambian una clave del protocolo del daemon, así que su versión no se mueve.
- **`src/main.rs` / `crates/avi-core/src/json_emitter.rs` (`CLI_SCHEMA_VERSION = "5"`) — payloads `--json` de la CLI.** El payload de síntesis lleva `audio_path`, la ruta del WAV en el almacén, y no existe ninguna ruta de salida elegida por quien invoca; `doctor --json` expone la sección de ciclo de vida (`version`, `target`, `channel`, `install`, `path`, `pending`, `models`), con `checks` y `failed` como veredicto; los simulacros de `self uninstall` y `cleanup` informan `status` `planned` con su recibo `planned` sin `dry_run`; `daemon stop` bindea `was_running` y `daemon_fully_stopped` y su fallo sale por veredicto con `status` `still_running`; los fallos de parseo con `--json` salen como envelope con `reason` `usage_error` por stdout; y `say`/`dub` no emiten `audio_path`.

**La política de compatibilidad es la misma en ambas**: añadir claves no incrementa la versión; solo lo hace un cambio incompatible de las existentes. Y el **número de versión no es comparable entre las dos**: que hoy ambas valgan `"5"` es una coincidencia, no un acoplamiento: son dos contratos que suben por separado, y un cambio incompatible en uno no mueve la versión del otro.

## 11. `self`, `setup`, `cleanup` y `voice`

### `self install`, `self update` y `self uninstall`

`self` es el grupo que hace que **el ciclo de vida viva en el binario que se gestiona a sí mismo**. Los tres subcomandos comparten cuatro propiedades, y las cuatro importan más que sus flags:

- **Actúan sobre la instalación registrada, no sobre la copia que se invoca.** Las raíces efectivas salen del recibo (`cleanup::Roots::from_receipt`), de modo que la operación acierta aunque `AVI_DATA_DIR` o `AVI_CACHE_DIR` ya no estén definidas ([recibo de instalación](../specs/LIFECYCLE.md#71-recibo)). `AVI_CACHE_DIR` designa el **contenedor** de caché, no la raíz de modelos: los modelos son `$AVI_CACHE_DIR/models` y los temporales del producto `$AVI_CACHE_DIR/tmp`; el recibo registra ese contenedor en `roots.cache_dir`.
- **Toman el bloqueo de SO en modo exclusivo**, sobre el archivo de bloqueo y su compuerta de traspaso (hermanos del directorio de programa registrado, el mismo para todas las operaciones), y por eso un segundo proceso concurrente sale con `lifecycle_locked` (17) en vez de esperar. Los comandos de uso (`translate`, `voice`, `speech`, `setup`, `daemon start`, `daemon restart` y `daemon serve`) lo toman en modo compartido y lo retienen toda su vida, salvo `daemon serve`, que lo suelta al quedar registrado en `daemon.pid`: coexisten entre sí y, mientras hay una operación en curso, terminan con `lifecycle_locked` (17) sin ejecutarse. No lo toman `daemon status`, `daemon stop`, `doctor` sin `--repair` ni las simulaciones `--dry-run`. `self update` retiene la compuerta durante todo el traspaso al binario nuevo, y `self install` retiene el bloqueo hasta terminar su provisión de modelos.
- **Ejecutan la recuperación antes de componer el plan**, para que el plan que el usuario ve y confirma sea el que queda después del barrido por identidad de los residuos de dueño muerto (aparcados, stagings de instalación, scratch, temporales con token y registros de borrado resueltos); los de dueño no observable se conservan y se informan.
- **El binario aporta lo que el motor no puede tener**: el control de procesos (parada del daemon) y el borrado diferido de Windows. El motor decide; el binario ejecuta esas dos primitivas de plataforma (`ProductProcesses`, `ProgramRemoval`, esta última sobre `avi_process::schedule_removal`).

| Sub-acción | Qué hace | `reason` de éxito parcial o de fallo |
|---|---|---|
| `self install` | Instala el bundle del que forma parte el ejecutable, o **repara** la instalación si se ejecuta desde ella; aplica al final la misma provisión que `setup` salvo `--no-setup` | `setup_failed` (11) con el programa instalado |
| `self update` | Actualiza la instalación registrada a la última estable o a `--version X.Y.Z`, con verificación y traspaso al binario nuevo; `--check` solo informa | `already_up_to_date` (0) sin descargar; `not_installed` (3) sin instalación; `externally_managed` (12) en `homebrew`; `setup_failed` (11) parcial; `unsupported_platform` (18), `binary_incompatible` (19), `network_error` (20), `checksum_mismatch` (21) |
| `self uninstall` | Borra el estado, revierte el `PATH` según el recibo y borra el directorio de programa aplicando R2 | `program_dir_kept` (22) si el directorio de programa no se pudo borrar ni programar su borrado |

**`self uninstall` sin `--keep-data` borra la raíz de datos entera, no el plan de `cleanup --all`.** La diferencia es deliberada y está en `uninstall::compose_plan`: `cleanup --all` protege las voces de fábrica (`default`, `ryan`, `vivian`) porque van embebidas en el binario y el programa sigue instalado, así que `setup` las vuelve a materializar. Al desinstalar **el programa desaparece**, y con él las voces de fábrica: dejarlas sería residuo dentro de una raíz de propiedad exclusiva. Con `--keep-data` sí se aplica el plan de `cleanup --all` **filtrado** —modelos, voces y habla quedan fuera— y el directorio de programa se borra igualmente.

**Idempotencia**: sin instalación ni estado, `self uninstall` termina con éxito y `status` `not_installed`. En Windows, si el ejecutable en uso está dentro del directorio de programa, el borrado se programa para cuando el proceso termine y el `status` es `removal_scheduled`, que también es éxito porque el borrado diferido quedó programado (sin registro si va bien; si falla deja `<destino>.removal-failed.log` junto al destino). Si no se puede programar, el `status` es `uninstalled` con `reason` `program_dir_kept` y salida 22.

El detalle completo —los pasos de `self install` y de `self update` con traspaso y borrado diferido, el bundle y su manifiesto, el recibo y la integración de `PATH`— está en [`commands/SELF.md`](commands/SELF.md) y, como especificación, en [`LIFECYCLE.md`](../specs/LIFECYCLE.md#83-self-install).

### `cleanup`

| Flag | Qué borra / efecto |
|---|---|
| `--voices` | Las voces que puede borrar y, **con ellas, solo los namespaces de habla sintética de esas voces** (arrastre `speech/<voz>` excepto `default`) |
| `--synthetic-speech` | La raíz `speech/` entera, `default` incluida |
| `--model` | En la raíz de modelos **exclusiva** de la aplicación, la **raíz entera** (snapshots, locks y `xet` cuelgan de ella). En la raíz **compartida** que el usuario eligió con `HF_HUB_CACHE`/`HF_HOME`, solo los repos fijados (`MODEL_REVISIONS`) y sus locks; **`xet` y el `.locks` completo nunca se borran** (regla R3) |
| `--all` | Unión de las tres categorías **más** configuración (`config.json`), logs, el estado de ejecución del daemon (`daemon.pid` y `daemon.ready`) y la hoja de temporales (`scratch`), con la poda de los contenedores que queden vacíos (**sin programa ni PATH**) |
| `--dry-run` | Plan puro (exit 0): lista sin borrar, sin tomar el bloqueo y sin pedir confirmación; con `--json` emite `status: "planned"` con `removed` y su recibo `planned`, sin `dry_run` |
| `--yes` / `-y` | Omite la confirmación interactiva (`s/si/sí/y/yes/dale/ok`); con `--dry-run` es no-op |

**Gate `sin flags → exit 2`:** `cleanup` sin ningún flag de categoría (`--voices`, `--synthetic-speech`, `--model`, `--all`) sale con `2` `usage_error` **sin borrar nada y sin tomar el bloqueo** (`cleanup::run`, primera comprobación). `--all` equivale a `--voices --synthetic-speech --model` más configuración, logs y estado del daemon.

**Sin terminal, `cleanup` no procede por su cuenta.** Es una operación destructiva: sin TTY y sin `--yes` termina con `confirmation_required` (2) y no borra nada. La celda no destructiva de la [tabla de reglas transversales](../specs/LIFECYCLE.md#81-reglas-transversales) —lo no destructivo procede sin preguntar— es la de `self install` y `setup`, no la de `cleanup`.

**El plan es una función pura de sus entradas** (`cleanup::plan`): la usan la confirmación, el `--dry-run` y la ejecución, de modo que no puede haber divergencia entre lo que se anuncia y lo que ocurre. Antes de borrar recursos que el daemon usa, `cleanup` lo detiene; si no se detiene, `daemon_stop_failed` (16) y **nada del plan se borra**.

**`speech/default/` (y `ryan`/`vivian`) sobrevive a `--voices` y cae únicamente con `--synthetic-speech` o `--all`.** El criterio es el del propio flag —las locuciones se van con su voz— y las voces de fábrica (`default`, `ryan`, `vivian`; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) no se van nunca: `voice remove` las protege (exit 2) y `--voices` no las borra. Importa declararlo porque `default` es la voz por defecto de `speech synthesize` y su namespace es probablemente el más poblado. Reparto con `speech remove`: el borrado individual es `speech remove --label`, el masivo es `cleanup --synthetic-speech` (ver «Reparto con `cleanup`» en [§3](#3-el-grupo-speech)).

`--all` es unión de limpiezas de datos; **no toca programa ni PATH** — eso es `self uninstall`, y es el único comando que borra el directorio de programa. Con la raíz separada del registro de voces, el arrastre de `--voices` es código explícito y no un efecto del `rmtree`.

**El plan nunca dice qué se conserva por callado**: la lista de `preserved` (raíz compartida con su motivo, directorio de programa) es parte del plan y se imprime junto a él, porque un usuario que ve `cleanup --all` sin lista de lo que se conserva no puede saber que sus modelos de otra herramienta siguen ahí.

### `setup`

El chequeo de audio degrada a WARN en vez de FAIL, **con la premisa que lo sostiene**: el sidecar es instalable en hosts headless, SSH y CI porque existe un sumidero que no necesita subsistema de sonido —`speech synthesize --text T --label L` sintetiza y persiste sin reproducir nada—. `setup` es provisión, no diagnóstico.

`parakeet-tdt-v3` (int8, runtime `ort` load-dynamic vía `ParakeetEngine`) se
provisiona siempre en `setup` base. No existe `--language` en `setup`: el
conjunto provisionado es fijo (es+en offline completo desde el primer uso).
Pasar `--with-stt` falla como argumento desconocido con exit 2
(error de uso de `clap`, mensaje de texto por stderr; con `--json`, además el
envelope `usage_error` por stdout, ver
[el mecanismo](#el-mecanismo-un-solo-punto-de-traducción)).

**Su flujo está en el motor**: selección de modelos, idempotencia por presencia del snapshot, purga de `--force-update` **sobre la misma selección** y validación de cada modelo por presencia de sus ficheros.

**Poda**: la selección es siempre la tabla de pines completa (el modelo Base de clonado incluido) y no se persiste; tras el reemplazo el `setup` provisiona el pin nuevo y poda las revisiones propias obsoletas, con R3 en raíz compartida (`crates/avi-lifecycle/src/setup.rs`).

**`setup` invocado al final de una instalación no es un error de la instalación.** `self install` aplica en el mismo proceso la misma provisión que `setup` (descarga de la selección, y `doctor` exige después los modelos de la selección); si falla, el programa **queda instalado** y el desenlace es `setup_failed` (11), con el motivo del fallo de provisión anidado en `models_cause`. Ver `self install` en [`commands/SELF.md`](commands/SELF.md) y §10.


### `voice`

- **`voice clone` toma un único audio de referencia, `--speech-reference/-s`** (obligatorio; se recomiendan unos 10 s y se rechaza por encima de `MAX_CLONE_REFERENCE_SECS`, 30 s); el WAV se consume para producir `reference.qvoice` y no se copia al almacén. Cualquier otro flag de referencia sale con exit 2 `usage_error` (argumento desconocido).
- **`voice clone` guarda solo la huella de timbre (x-vector) de la referencia**: no recibe transcripción ni idioma, no depende del STT y funciona igual con o sin la feature `native-stt`. Las voces clonadas se sintetizan con el modelo Base (`qwen3-tts-0.6b-base`, que `setup` provisiona siempre); en una instalación incompleta, su ausencia sale con `model_missing` (exit 4) tanto al clonar como al sintetizar.
- **`voice clone` recibe el despacho al daemon en sus tres modos**, porque el clonado ejecuta el modelo Base (y, con el daemon, precalienta la voz nueva) y necesita un modelo cargado igual que las dos sub-acciones que sintetizan.
- **`voice clone` sobre un nombre tomado sin `--force` sale con 6**, y sobre un nombre libre `--force` es un no-op declarado.
- `VoiceStore` (`crates/avi-store/src/lib.rs`) reconoce una voz clonada únicamente por la presencia de `reference.qvoice`, sin fallback a WAV. Las voces de fábrica son `default` (clonada, con `reference.qvoice`) y `ryan`/`vivian` (presets puros sin referencia); `voice remove` las protege a las tres (exit 2; `FACTORY_VOICES` del almacén de voces, `avi_store::is_factory_name` definida en el almacén de voces, invocada en el binario principal).
- `voice list` muestra las tres de fábrica (`is_factory=true`) más las clonadas del usuario; `voice remove` rechaza las de fábrica con exit 2. `voice remove` de una voz del usuario con locuciones las borra con ella: muestra el plan y exige confirmación como `cleanup` (con terminal pregunta `[s/N]`; sin terminal exige `--yes` y, sin él, sale con exit 2 `confirmation_required` sin borrar nada; cancelar sale con 0 y `status: "cancelled"` en `--json`); el envelope de éxito lleva `removed_speech` con las etiquetas arrastradas.
- En `voice list` y `voice remove`, `-n` es `--name`; `voice remove` añade `--yes/-y` para omitir la confirmación del arrastre.

## 12. Contratos externos

### El integrador de narración

**`speech say --text "<msg>" --daemon`** es la invocación que sintetiza y reproduce, y es el contrato del integrador de narración: exige el daemon porque su presupuesto de latencia no admite cargar el modelo al vuelo. No hay alias de compatibilidad; esa es la única forma de la invocación.

El integrador que quiera además conservar el audio usa `speech synthesize --text "<msg>" --label L`, que no reproduce.

El integrador es **Agent Voice-InterConnector**, un plugin de [Claude Code](https://code.claude.com) que narra por voz la actividad de la sesión: al final de cada turno y en avisos relevantes el usuario escucha una locución corta en español. Se dispara por hooks (`Stop`, `Notification`; `SessionStart` verifica el entorno y deja el daemon caliente), nunca bloquea ni retrasa el turno y falla en silencio si el motor no está disponible. El resto de sus propiedades de diseño vive en su repositorio, que es su fuente de verdad; su contraparte de este contrato está en su [documento de integración](https://github.com/CristianRojas-SoftwareEngineer/Agent-Voice-InterConnector/blob/main/docs/INTEGRATION.md).

**Dependencia unidireccional.** El plugin consume a AI Voice-InterConnector; este repositorio no conoce, importa ni depende del plugin, y ningún código, test ni build de aquí sabe de su existencia. A efectos del motor, el plugin es un consumidor externo más de la CLI pública (`ai-voice-interconnector` en `PATH`), como un script de usuario.

**Superficies que consume y su compromiso de estabilidad.** El único acoplamiento es la CLI pública:

| Superficie | Qué consume el plugin | Compromiso de estabilidad |
|------------|-----------------------|----------------------------|
| `speech say --text "<msg>" --daemon` | Síntesis dinámica y reproducción de cada locución. Usa el daemon y falla si no está levantado (no lo arranca solo). | Mantener el flag `--daemon` y su semántica (usar el daemon, no auto-arrancarlo). |
| `speech synthesize --text "<aviso>" --label <label>` | Pre-síntesis de avisos; se guarda en el almacén de habla sintética bajo un `label`. | Mantener el flag `--label` y la colisión de label (exit `6`, ver abajo). |
| `speech play --label <label>` | Reproducción de una locución pre-cacheada por `label`. | Mantener el subcomando `play` y el flag `--label`; exit `3`/`NotFound` si el label no existe. |
| `doctor --json` | Verifica el entorno; lee `checks` (una entrada `{name, ok, detail}` por chequeo) y `failed` (los `name` que fallan: el entorno está sano si `failed` está vacío). Ver [`doctor`](commands/DOCTOR.md). | Mantener las claves `checks` y `failed` con ese significado. |
| `daemon status --json` | Lee `daemon` (`"running"`/`"stopped"`) para saber si el daemon corre: `running` solo con un daemon propio en servicio, el mismo criterio con el que `daemon start` decide si arranca. `pid` (en servicio) y `degraded` (restos) son aditivos. Ver [`daemon`](commands/DAEMON.md). | Mantener la clave `daemon` con esos dos valores. |
| `daemon start` | Levanta el daemon para dejar los modelos en memoria. | Mantener el subcomando y su arranque desanclable. |

Además, el plugin interpreta dos exit codes semánticos (definidos en `crates/avi-core/src/exit_codes.rs`) para decidir sin parsear stderr:

- exit `5` = `DaemonUnreachable`: el daemon está caído o inalcanzable; el plugin lo usa para saber que no responde.
- exit `6` = `StateConflict`, emitido con reason `"label_exists"` cuando un `--label` ya existe en `speech synthesize`; el plugin lo usa para no re-sintetizar un aviso ya cacheado.

El plugin decide por el código de salida y trata como ausente un `reason` que no conoce: añadir un `reason` no sube `schema_version`. La regla está en [USAGE](../../USAGE.md#comandos).

Cambiar cualquiera de estos nombres, flags, campos o exit codes rompe la narración sin que este repositorio tenga tests que lo detecten, porque el plugin vive fuera. Por eso esta tabla es el contrato a preservar: al tocar `src/main.rs` en `speech say`, `speech synthesize`, `speech play`, `doctor` o `daemon`, o `crates/avi-core/src/exit_codes.rs`, hay que revisarla. `speech transcribe` y `speech dub` existen en la CLI pero el plugin no los consume; no forman parte de este contrato.

**Qué no comparten los dos proyectos.** Todo lo que no es el contrato público de la CLI es disjunto, y por eso el plugin vive en su propio repositorio (el modelo de distribución de plugins por marketplace también asume un repo git propio):

- **Código**: el plugin es TypeScript sobre el Node.js que trae Claude Code; no depende del árbol Rust.
- **Versionado**: este repositorio versiona el motor (binarios por SO); el plugin versiona con el campo `version` de `plugin.json`, al ritmo de Claude Code. Un fix en uno no obliga a un release del otro.
- **CI e infraestructura**: aquí `cargo test`/build y publicaciones por tag; allá toolchain TypeScript y `claude plugin validate`.

**Instalación.** Desde el lado del motor no hay nada que instalar para el plugin: basta con que `ai-voice-interconnector` esté en el `PATH` y los modelos estén en caché (`ai-voice-interconnector setup`). El repositorio del plugin dobla como su propio marketplace:

```
/plugin marketplace add CristianRojas-SoftwareEngineer/Agent-Voice-InterConnector
/plugin install agent-voice-interconnector@Agent-Voice-InterConnector
/agent-voice-interconnector:install
```

`/agent-voice-interconnector:install` guía la instalación del binario, la descarga de los modelos y la activación de la narración.

### La frontera del daemon

`/synthesize` recibe `voice: str` y devuelve el audio sintetizado sin ruta de destino en la petición. No hay lista de directorios de audio permitidos, ni validación de destinos de audio, ni directorio de sesión del daemon, porque la petición no proporciona un destino de archivo. `/voices/clone` tampoco recibe la ruta del WAV de entrada; su evento `result` sí emite `speech`, la ruta de `reference.qvoice` calculada al guardar la voz en el filesystem del daemon. La CLI propaga ese dato sin abrir ni escribir el fichero; la ruta solo es utilizable para leerlo si el cliente tiene acceso a ese filesystem.

**Riesgo conocido y declarado**: `data_dir()` / `models_cache_dir()` (`crates/avi-store/src/lib.rs`) dependen de `LOCALAPPDATA` / `XDG_DATA_HOME`, así que un daemon y un cliente arrancados con entornos distintos responden «voz no encontrada» para una voz que el cliente sí lista. El daemon no expone un endpoint para inspeccionar su vista del almacén (`GET /voices` fue podado). Con `AVI_DATA_DIR`, esa dependencia es además el mecanismo de aislamiento por instancia en tests (sandbox propio por test; en producción la resolución por defecto no cambia).

## 13. `translate`, `speech transcribe`, `speech dub` y la síntesis cross-lingual

### `translate`: texto→texto, aislado de la síntesis

`translate` no pertenece a ningún grupo nominal: es texto→texto, sin voz ni modelo TTS de por medio. `--source-language` y `--target-language` son **opcionales con defaults** (`es` y `en`) y **estrictos**: el parser solo acepta `es` o `en`; cualquier otro valor lo rechaza `clap` con exit 2 antes de llegar al handler. El vocabulario de la CLI es `es|en` en todos los comandos, y el daemon acepta las mismas claves `source_language`/`target_language` con los mismos valores. `--source-language == --target-language` es *passthrough*: devuelve el texto sin cargar el modelo. `translate` no tiene límite de longitud.

Parámetros: `--text`/`-t` **requerido** · `--source-language` (default `es`, solo `es`/`en`) · `--target-language` (default `en`, solo `es`/`en`) · `--json`. Su payload está en [`commands/TRANSLATE.md`](commands/TRANSLATE.md) y en la [referencia de esquemas](../../USAGE.md#referencia-de-esquemas---json).

### Traducción opt-in en `speech say`/`speech synthesize`

`--source-language`/`--target-language` (§3) insertan una etapa de traducción **antes** de la síntesis cuando declaran idiomas distintos; el motor de síntesis no cambia, solo recibe el texto ya traducido. Ambos son opcionales en `say`/`synthesize`: `--target-language` vale `es` por defecto y `--source-language` vale lo mismo que `--target-language` cuando se omite, por lo que sin flags no se traduce. Solo se admite `es` o `en`; cualquier otro valor sale con **2**. Ese mismo token es el idioma de la síntesis: el motor lo recibe traducido a su nombre (`es` → `Spanish`, `en` → `English`), y `POST /synthesize` del daemon rechaza un `source_language`/`target_language` que no sea `es` o `en` con `usage_error`. El modelo de traducción ausente reutiliza el exit **4** (`ExitCode::ModelMissing`), remitiendo a `setup`, en vez de un código propio: es la misma precondición de entorno que el modelo TTS. Un fallo de la inferencia de traducción, con el modelo ya cargado, sale con **9** (`ExitCode::TranslationFailed`, §9) — código distinto del **1** genérico de síntesis, porque distingue en qué etapa falló la invocación.

`--temperature` es un override opcional del muestreo en `say`/`synthesize`/`dub`: sin el flag se usa la temperatura de producción (`0.35`); con el flag debe cumplirse `0 < t <= 2.0`, y fuera de ese rango sale con **2**.

### El flag `--target-language`

`speech say`/`speech synthesize` declaran el idioma con `--source-language`/`--target-language`; ni `setup` ni `doctor` exponen flags de idioma (la provisión no traduce y el diagnóstico no filtra por idioma). `daemon` **no expone** flags de idioma (ver `docs/CLI/commands/DAEMON.md` — `native-stt`/`native-translation` son features de compilación, no flags de ejecución). El integrador de narración (§12) invoca `speech say` sin flags de idioma, así que traduce en `es` por defecto.

### Provisión y daemon

`setup` —y la provisión de `self install`, que comparte su función— descarga siempre `opus-mt-es-en` y `opus-mt-en-es`: derivados int8 de Helsinki-NLP/opus-mt (CC-BY-4.0) ya convertidos a CTranslate2 y publicados en `CristianRojaas/opus-mt-es-en-ct2-int8` y `CristianRojaas/opus-mt-en-es-ct2-int8`, sin conversión local. El motor los lee directo del snapshot de HuggingFace y los valida con `is_provisioned` (cada fichero de `MODEL_FILE_PATTERNS` presente y de más de 0 bytes: `config.json`, `model.bin`, `shared_vocabulary.json`, `source.spm`, `target.spm`); `doctor` exige ambos modelos como parte de `models_provisioned`; `cleanup --model` borra la raíz de modelos entera si es exclusiva y solo los repos fijados si es compartida (R3). Sin gating por `--language`: la provisión es determinista para la instalación por defecto. `daemon start`/`serve` aceptan `--auto-restart` y `--max-retries` (default `3`) — con `--auto-restart` el supervisor reintenta hasta `max_retries` con backoff `500ms*2^retries` capado a `4s` (cada reintento con reclamo activo del árbol propio previo con deadline de 5 s y verificación de muerte + puerto libre; el reclamo activo matar-y-rearrancar vive en `start`, nunca en `serve`; con puertos efímeros el reclamo previo al reintento pierde su objeto (el cliente ya descubre la `addr` real por el pidfile); un `daemon stop` graceful vía `shutdown_notify` no reintenta, y un fallo previo a que el daemon haya estado listo (puerto ocupado, voz de warmup inexistente, carga del estado) es terminal y no se reintenta.

### `speech transcribe`: audio→texto, verificable sin traducir ni sintetizar

`speech transcribe` es una **sub-acción del grupo `speech`** (no un comando aislado como `translate`): transcribe a texto con `parakeet-tdt-0.6b-v3` int8 vía `ort` load-dynamic (del módulo de transcripción STT, `ParakeetEngine::transcribe`), desde un archivo WAV (`--audio`) o desde el micrófono (`--mic`). La **captura corre siempre en el cliente** (al daemon viajan las muestras ya decodificadas en base64, nunca rutas); la transcripción en sí recibe el despacho al daemon en sus tres modos (§5): sin flags, transcribe por el daemon si está activo y en modo directo si no; `--daemon` exige el daemon y sale con **5** si no está; `--no-daemon` fuerza el modo directo. Es una operación **de un solo idioma por invocación** — a diferencia del par de `translate`, aquí `--source-language` (requerido, `es`/`en`) declara el único idioma hablado en el audio. `ParakeetEngine` **solo transcribe**, nunca traduce: si el usuario necesita el texto en otro idioma, encadena `translate` por separado. No hay síntesis de por medio: `speech transcribe` es verificable de forma aislada, con audio de entrada y texto de salida, sin depender del motor TTS ni del subsistema de traducción.

`--audio` y `--mic` forman un **grupo mutuamente excluyente `required=True`** (el mismo en `speech dub`), el único tipo de grupo excluyente del árbol de parsers que exige uno de sus flags: los demás — `--daemon`/`--no-daemon` en `translate`/`speech synthesize`/`speech say`/`voice clone`/`speech transcribe`/`speech dub`, y el grupo de `setup` — son opcionales, sin exigir ninguno de los dos. Con `--mic`, la captura es **push-to-talk** por defecto (Enter para terminar); `--duration N` fuerza una grabación de duración fija en segundos y solo es válido junto a `--mic` (el parser lo declara incompatible con `--audio`) — con `--audio`, o con `--mic` ausente, `--duration` sale con **2** (`ExitCode::InvalidInput`, `usage_error`), igual que la ausencia de ambos o la presencia de los dos. Sin terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con **2**, porque no hay forma de detectar la pulsación de Enter que cierra el push-to-talk. El push-to-talk tiene un techo de seguridad configurable con la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s): al alcanzarlo, la grabación se detiene, se avisa por stderr y se continúa con lo grabado hasta ese punto — sale con **0**, no es un error. Al iniciar la grabación se emite además un aviso mínimo por stderr, nunca una espera silenciosa. La captura usa el backend multiplataforma `cpal` (único, sin ramas por sistema operativo): graba a la tasa y formato nativos del dispositivo de entrada y normaliza a 16 kHz/mono/int16 (formato que `ParakeetEngine` asume), la misma normalización que aplica el WAV de `--audio`; la inferencia es `ort` load-dynamic con los 4 artefactos `MODEL_FILE_PATTERNS` (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), stack Rust nativo.

Parámetros: `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** (`es`/`en`) · `--daemon`/`--no-daemon` · `--json`.

El payload `--json` (en [`commands/SPEECH.md`](commands/SPEECH.md)) no cambia con el despacho: es el mismo en los tres modos. `source_language` es el valor de `--source-language` tal como se pasó (`es` o `en`).

Si el modelo `parakeet-tdt-0.6b-v3` no está provisionado, sale con **4** (`ExitCode::ModelMissing`), remitiendo a `ai-voice-interconnector setup`; un fallo de la inferencia con el modelo ya cargado, o de la captura del micrófono, sale con **10** (`ExitCode::TranscriptionFailed`, §9) — mismo criterio de asignación que distingue **4** de **9** en `translate`. La lectura del `--audio` no sale con 10: un archivo inexistente sale con **3** (`ExitCode::NotFound`, `audio_not_found`), un WAV corrupto, truncado o no decodificable con **2** (`ExitCode::InvalidInput`, `invalid_audio`) y un fallo de E/S al leerlo con **1** (`io_error`). El audio de `--audio` o `--mic` dura como máximo 300 s (`MAX_TRANSCRIBE_AUDIO_SECS`; `audio_too_long`, exit **2**, §6). En la ruta daemon, un fallo de comunicación —daemon inactivo— sale con **5** (`ExitCode::DaemonUnreachable`), sin degradación silenciosa a modo directo. Un error HTTP del daemon se traduce por su `reason` con la tabla única (p. ej. 404 `model_missing` → **4**, 404 `voice_not_found` → **3**); un 404 sin `reason` ni `error` (un daemon que no expone la ruta) sale con **1** (`daemon_error`) y el mensaje «El daemon no expone esta ruta (HTTP 404): reinícialo con el binario actual ejecutando `ai-voice-interconnector daemon restart`, o usa un build que incluya la función.», nunca con 5.

**Mismo shape `--json` en `translate` y `transcribe`.** `translate --json` emite `source_language`/`target_language` con los valores de `--source-language`/`--target-language` (`es`, `en`), y `speech transcribe --json` emite `source_language` con el valor de su `--source-language`: ambos preservan el token de entrada, que ya viene restringido a `es|en` por el parser.

### `speech dub`: el bucle voz→voz en un comando dedicado

`speech dub` es la **composición voz→voz**: transcribe la entrada hablada (archivo o micrófono), traduce el texto si `--source-language` difiere de `--target-language`, sintetiza con la voz elegida y reproduce el resultado. En la ruta daemon, delega a `POST /dub` mediante streaming NDJSON (`started` → latidos → `result`), sin degradación a otra composición: todo error de la ruta se traduce por su `reason` con la tabla única de códigos. Reutiliza las máquinas existentes —la etapa de transcripción de `speech transcribe` (sus tres modos de despacho), la traducción opt-in de `say`/`synthesize` y el despacho de síntesis (§5)— sin modificarlas. `say`/`synthesize` **no cambian**: siguen siendo texto→voz con `--text` requerido; la entrada de audio del bucle vive solo en `dub`. No persiste nada: no declara `--label` propio, y su `--json` es el **global** ([§2](#la-superficie-del-ciclo-de-vida-flag-a-flag)), que el comando sí honra en las dos vías: por el daemon emite el envelope del stream y en la vía directa el payload de `speech dub` (ver [`commands/SPEECH.md`](commands/SPEECH.md)).

Exige **exactamente una** de `{--audio, --mic}` (grupo mutuamente excluyente `required=True`, espejo de `speech transcribe`); `--duration N` solo es válido con `--mic` (el parser lo declara incompatible con `--audio`; con `--audio` sale con **2**, `usage_error`). Con `--mic`, aplica el mismo push-to-talk con techo `AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s, exit 0 al vencer) y aviso mínimo por stderr al iniciar la captura descritos en `speech transcribe`, por reutilizar su misma etapa de transcripción. `--source-language` es **requerido** (`es`/`en`); `--target-language` (default `es`) elige el idioma/modelo de síntesis y dispara la traducción si difiere del hablado; `-v/--voice` y `--temperature` son los de `say`. `--daemon`/`--no-daemon` aplican a la transcripción y a la síntesis (los tres modos, §5); sin flags, ambas etapas usan el daemon solo si responde.

| Parámetros | Comportamiento |
|---|---|
| `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** · `--target-language` (default `es`) · `-v/--voice` · `--temperature` · `--daemon`/`--no-daemon` · `--json` (global, [§2](#la-superficie-del-ciclo-de-vida-flag-a-flag)) | transcribe → traduce (si `source != target`) → sintetiza → reproduce |

Códigos de salida aplicables en la cadena: **4** (`ExitCode::ModelMissing`, modelo de transcripción no provisionado, remite a `setup`), **5** (`ExitCode::DaemonUnreachable`, daemon exigido pero inactivo), **9** (`ExitCode::TranslationFailed`, fallo del pipeline de traducción con el modelo cargado) y **10** (`ExitCode::TranscriptionFailed`, fallo del pipeline de transcripción con el modelo cargado o de la captura del micrófono); más **2** (uso inválido: `--duration` sin `--mic`, `--mic` sin TTY y sin `--duration`; `invalid_audio` para un WAV corrupto o truncado; `audio_too_long` si el audio o `--duration` superan 40 s; `text_too_long` si el texto traducido supera 500 caracteres) y **3** (`audio_not_found`, `--audio` inexistente). El audio de entrada del dub dura como máximo `MAX_DUB_AUDIO_SECS` (40 s): el push-to-talk se corta a ese tope (además del techo `AVI_PUSH_TO_TALK_MAX_SECS`) y la síntesis final se acota con el presupuesto de §5 sobre el texto traducido.

---

## 14. Documentación complementaria por comando

Cada comando principal de la CLI tiene una referencia por comando en [`commands/`](commands/) que cubre su diseño, implementación, flujo de ejecución y manejo de errores con referencias a los símbolos del código fuente. El índice de comandos y sus documentos está en [`README.md`](README.md#resumen-de-comandos).
