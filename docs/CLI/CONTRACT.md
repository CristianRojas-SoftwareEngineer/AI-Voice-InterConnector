# Contrato de la CLI: el grupo `speech` y el contrato de salida

Este documento es la descripción normativa del contrato público de la CLI —comandos, flags, códigos de salida, payloads `--json`— y del almacén de audios sintetizados. Recoge también el porqué de cada decisión de diseño: los criterios generadores, las asimetrías de reversibilidad y el razonamiento detrás de cada código de salida.

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
  - [Las seis reglas, todas con exit 2](#las-seis-reglas-todas-con-exit-2)
  - [El orden de validación de la familia de síntesis](#el-orden-de-validación-de-la-familia-de-síntesis)
  - [Un solo mecanismo para la exclusión mutua, y es el declarativo](#un-solo-mecanismo-para-la-exclusión-mutua-y-es-el-declarativo)
  - [Validación de identificadores y de existencia](#validación-de-identificadores-y-de-existencia)
  - [Ningún flag queda sin efecto sin que la CLI lo diga](#ningún-flag-queda-sin-efecto-sin-que-la-cli-lo-diga)
- [7. Matrices de comportamiento](#7-matrices-de-comportamiento)
  - [`speech synthesize`](#speech-synthesize)
  - [El resto del grupo](#el-resto-del-grupo)
  - [Qué añade `--json` a las matrices](#qué-añade---json-a-las-matrices)
- [8. El almacén de audios sintetizados](#8-el-almacén-de-audios-sintetizados)
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

**El daemon no recibe rutas de archivos elegidas por el llamador.** En las peticiones, el audio viaja como bytes codificados en base64 (`audio_b64`) y las voces se seleccionan por identificador (`voice`); los audios sintetizados persistidos se nombran por `(voz, etiqueta)` en la CLI. La CLI lee los ficheros de entrada que el usuario nombra con sus flags (`speech transcribe --audio`, `speech dub --audio`, `voice clone --speech-reference`) en su propio proceso y con los permisos de quien invoca; el daemon recibe los bytes, no esas rutas. La salida de `speech synthesize` no tiene destino elegible por quien invoca: vive en el almacén de audios sintetizados, cuya ruta se calcula a partir de `(voz, etiqueta)` y la CLI expone (salida humana y clave `audio_path` de su `--json`).

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

**Coste declarado.** La CLI no tiene comando de exportación ni flag de destino: los bytes de un audio sintetizado se obtienen leyendo el fichero de `audio_path` (o de la ruta que imprime `speech synthesize`) con los permisos de quien invoca. Eso acopla al integrador al layout del almacén durante esa lectura; si la necesidad de un destino elegible aparece, la respuesta es un comando explícito con su propia decisión, no una clave en un listado.

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
| CLI | Fuente de referencia de habla de `voice clone` (~10 s recomendados; máximo 30 s) | `--speech-reference` (`-s`) o `--mic` (`--duration` opcional, solo con micrófono) |
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

**En prosa española la unidad se llama «audio sintetizado»** (en plural, «audios sintetizados»). Nunca aparece como identificador.

### Las decisiones de vocabulario de la superficie

- **El identificador de un audio sintetizado es `--label/-l`, no `--name/-n`.** Por homología con `voice` correspondería `--name`, pero dentro del grupo `speech` sería ambiguo frente a `--voice` («¿nombre de qué?»). Se acepta la divergencia con `voice --name` a cambio de que el mismo concepto no tenga dos nombres en dos comandos.
- **La voz se selecciona con `--voice/-v` en las seis sub-acciones que la toman**, no con `--voice-profile`: el concepto ya se llama «voice» en `voice list`, `voice clone` y `voice remove`, y darle un segundo nombre en otro comando es la homonimia al revés —dos palabras para una cosa— con el mismo costo.
- **`--play` y la sub-acción `play` comparten palabra a propósito.** Nombran una sola cosa —emitir audio por los parlantes— en los dos sitios donde ocurre.
- **`-t` es `--text` en `speech` y no existe en `voice clone`.** `voice clone` no declara `--text` y `speech` no declara referencias, así que ningún corto se solapa.
- **`-n` no está tomado en el grupo `speech`**, así que tiene un significado único en toda la CLI: `--name` en `voice clone` y `voice remove`.

## 3. El grupo `speech`

### Reparto de responsabilidades

| Sub-acción | Responsabilidad | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda | **sí** | sí |
| `speech say` | Sintetiza y reproduce a la vez, no guarda | no | sí |
| `speech play` | Reproduce un audio sintetizado guardado | no | **no** |
| `speech list` | Lista los audios sintetizados guardados | no | no |
| `speech remove` | Borra un audio sintetizado guardado (pide confirmación) | no | no |
| `speech transcribe` | Transcribe audio a texto | no | sí |
| `speech dub` | Compone voz→voz | no | sí |
| `speech denoise` | Limpia archivos de audio o la pista de audio de vídeo | no | sí, `native-denoise` |

`speech denoise` es local-only: procesa archivos directamente y no envía rutas al daemon. Acepta entradas de audio y vídeo; para vídeo conserva la imagen y limpia únicamente la pista de audio. Su perfil de archivos predeterminado es `studio`; `--preset default` selecciona el perfil original sin normalización. Requiere `native-denoise`, incluido en `full`; DPDFNet debe estar provisionado con `setup` y FFmpeg se resuelve desde el paquete de AVI, sin fallback al `PATH`; el binario integrado debe declarar versión 6 o superior, con o sin el prefijo de `git describe` que anteponen los builds de Linux, y una versión anterior o indescifrable se rechaza nombrando lo encontrado. La limpieza de archivos no se expone como endpoint del daemon.

`synthesize` y `say` son gemelos: misma síntesis, distinto destino —disco o parlantes—. `play`, `list` y `remove` son la gestión del almacén. **`say` es la única sub-acción que genera sin persistir, y junto con `synthesize` la única que puede exigir provisión del modelo**; esa es la contrapartida de que `play`, `list` y `remove` no lo necesiten.

El almacén etiquetado es un recurso, y el repo tiene gramática para gestionar recursos: un grupo nominal con sub-acciones. La homología con `voice` es directa:

| Registro de voces | Almacén de audios sintetizados |
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
| `speech remove` | `--label/-l` **requerido** · `--voice/-v` · `--yes/-y` · `--json` |
| `speech denoise` | `<entrada>...` · `--output-dir` · `--output-name` · `--prefix` · `--suffix` · `--recursive` · `--overwrite` · `--skip-existing` · `--dry-run` · `--json` global (JSONL) · `--verbose` · `--preset studio|default` · `--audio-bitrate` · `--highpass` · `--normalize` · `--target-lufs` · `--pause-gate-db` · `--dry-blend` · `--stereo` · `--help` |

### `speech denoise`: limpieza local de archivos

**`speech denoise` es una operación local de archivos**: el parser único del subcomando reutiliza el pipeline migrado. Las rutas `--model-dir` y `--ffmpeg-path` no forman parte de la CLI AVI y deben rechazarse como flags desconocidos; el modelo procede de `setup`/`doctor` y FFmpeg del paquete, sujeto a la misma comprobación de versión. `--json` global emite líneas JSONL de progreso y resultados sin envolverlas en otro objeto. Los flags opt-in `--denoise` de transcripción, doblaje y clonado son rutas independientes; la evaluación humana de preservación de identidad vocal en clonado no se considera validada. Para los contratos completos de opciones, entradas múltiples, nombres, colisiones, dry-run y resultados, véase [`commands/SPEECH.md`](commands/SPEECH.md#speech-denoise).

`studio` es el perfil por omisión de este subcomando de archivos, con normalización de una pasada hacia −16 LUFS (objetivo, no garantía exacta), highpass, gate −25 dB, mezcla seca 0 y mono salvo `--stereo`. Los bitrates predeterminados son MP3/AAC 320, Vorbis 192 y Opus 256 kbps; WAV/FLAC no usan bitrate. El bitrate explícito incompatible con el códec se rechaza, sin ajuste automático. `--preset default` elige el perfil original (192 kbps, highpass desactivado, sin normalización, gate −25 dB, mezcla seca 0 y mono). Se conserva la precedencia defaults → preset → flags individuales; los flags positivos no desactivan booleanos activados por el preset.

**`--voice/-v` es opcional en las cinco que lo aceptan** (`synthesize`, `say`, `play`, `list`, `remove`). En `synthesize`, `say`, `play` y `remove`, si falta, usa la voz de fábrica `default`; en `list`, sin `--voice` recorre todas las voces. La voz `default` es una voz clonada de fábrica con solo la huella de timbre ( `crates/avi-store/assets/default/reference.qvoice`, `crates/avi-store/src/lib.rs` `FACTORY_VOICES`). El catálogo de voces de fábrica es solo `default` (clonada): entrada del registro `VoiceStore`, que el almacén garantiza en cada consulta, incluida una raíz de datos recién creada. Toda voz es una entrada del registro y se sintetiza desde su `reference.qvoice` (`avi-tts/src/lib.rs` `resolve_voice_engine`). La voz de fábrica sintetiza texto corto (2-4 palabras, p. ej. `Hola mundo`) con una tasa de error de palabras ≤ 0.25.

**Matriz voces × idioma.** Toda voz habla `es` y `en`: AVI traduce el token del producto a su nombre de idioma (`es` → `Spanish`, `en` → `English`) y lo envía en cada petición de síntesis. `default` es una clonación generada con la ruta de clonación del producto a partir de `assets/audios/Speech Sampler.wav` (~10 s): guarda solo la huella de timbre. Toda voz (`default` incluida) se sintetiza con el modelo Base; sin él provisionado, sintetizar sale con `model_missing` (exit 4).

**El namespace es obligatorio en la gestión.** Las etiquetas viven bajo una voz, así que `play`, `remove` y `list` toman `--voice` con el mismo default que `synthesize` y `say`. `speech list` filtra por voz cuando se pasa `--voice/-v` (`SpeechCommands::List { voice: Option<String> }`, del binario principal; brazo `List` en el binario principal, lectura acotada vía `SpeechStore::list_by_voice` en el almacén de voces); sin `--voice` recorre todas las voces. Es un segmento más que en `voice remove --name X`, inevitable dado el layout del almacén.

**`--label` requerido en `synthesize` es lo que sostiene el reparto.** Elimina de raíz la invocación con efecto cero sin escribir ninguna regla —la rechaza el parser— y elimina la trampa de «previsualizo con un comando y guardo con otro»: como `synthesize` siempre persiste, nadie pierde la toma que acaba de oír.

**`speech play` no necesita modelo ni daemon**: lee el WAV del almacén y lo reproduce.

**El listado no vive dentro de `synthesize`.** No hay `speech synthesize --list`: el listado es `speech list`.

**Reparto con `cleanup`**: `speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que existe entre `voice remove` y `cleanup --voices`. Las dos operaciones de voz arrastran además los audios sintetizados de las voces que borran.

## 4. Síntesis y el bucle de `--play`

### Qué hace cada gemelo

Sin `--play`, `synthesize` sintetiza, guarda y termina. Con `--play`, la toma suena mientras se sintetiza y la CLI pregunta antes de guardar.

`speech say` sintetiza y reproduce a la vez (entrega en streaming: la voz empieza a sonar antes de que termine la síntesis), y no escribe nada en el almacén. Es el destino de la invocación que solo quiere oír el resultado: la que no nombra un artefacto porque no lo quiere.

**Son dos usos que no se cruzan.** `say` es lectura en voz alta continua, generada al vuelo: cada mensaje es distinto del anterior y se descarta al sonar, así que persistir no tendría sentido. `synthesize` es para grabar un mensaje reutilizable —el caso de los mensajes por defecto— y reproducirlo después sin volver a sintetizarlo. No existe un recorrido que salte de `say` a `synthesize` para «quedarse» con una toma ya oída: quien quiere conservar usa `synthesize` desde el principio. Por eso la reproducción sin re-síntesis la garantiza **el almacén** —se guarda un WAV y se reproduce ese WAV—, y no una supuesta reproducibilidad del motor entre dos llamadas. Dentro de `synthesize`, la primera toma usa el seed de producción, reproducible, y cada regeneración envía al motor un seed explícito nuevo, distinto del de producción y de los ya usados en esa invocación, por lo que suena distinta; esto vale igual en la ruta directa y en la del daemon. «Aceptar y guardar» persiste la toma que sonó, nunca una nueva, con el texto realmente sintetizado (el traducido si los idiomas difieren) en ambas rutas.

### El bucle de `--play`: cuatro opciones

| Opción | Efecto | Costo |
|---|---|---|
| Reproducir otra vez | Vuelve a sonar la misma toma | **Cero síntesis**: se repite el WAV temporal de la toma |
| Aceptar y guardar | Persiste la toma que acabas de oír, y termina con 0 | Cero |
| Rechazar y regenerar | Sintetiza otra toma con un seed nuevo, vuelve a mostrar la etapa de síntesis y vuelve a preguntar | Una síntesis completa por toma |
| Rechazar y descartar | Termina con 0 **sin guardar nada** | Cero |

**«Descartar y salir» es una salida de primera clase**, con exit 0 y sin persistencia: el rechazo es un campo del resultado, no un error. Es el mismo modelado que `cleanup`, donde responder «n» a la confirmación termina con 0. Lo que el bucle no comparte con ese comando es la forma de la elección —allí es binaria— ni el destino de su prosa: la pregunta y sus avisos respetan la separación de canales (con `--json` la información humana va a stderr y stdout queda para el payload) y la cancelación viaja como campo del resultado.

**«Descartar» y no «rechazar».** En el bucle, regenerar también rechaza la toma; la palabra del contrato no distinguiría entre las dos opciones que descartan el audio actual, y solo una de ellas termina la invocación.

**Ctrl-D es el atajo de «descartar y salir».** Con terminal presente, cerrar la entrada en la pregunta es una forma legítima de abandonar y mapea exactamente sobre la cuarta opción: exit 0, sin persistir. Es el único fin de entrada alcanzable en el bucle, y tiene significado propio.

### Cuándo persiste, y qué protege la colisión

**Cuándo persiste.** Sin `--play`, inmediatamente después de sintetizar. Con `--play`, solo al aceptar. Así «descartar» nunca es un borrado: es no haber escrito.

**Qué hacer con una etiqueta tomada lo dice una política, y solo una.** Los flags se convierten en un único punto en `CollisionPolicy` (`crates/avi-store/src/lib.rs`): sin flag es `Fail` (sale con **6** `label_exists`), `--force` es `Overwrite` (sobrescribe) y `--skip-existing` es `Skip` (conserva la existente y sale con **0** y `status` `skipped`, sin sintetizar). La decisión es una sola y **mira solo el WAV**, que es el registro: una ficha `.json` corrupta no la cambia, así que sin `--force` la salida es `label_exists` y no un `io_error`.

**La colisión se decide dos veces, y cada decisión tiene un papel distinto.**

- **Antes de sintetizar** (`check_locution`), como *fast-fail*: con `Fail` y la etiqueta tomada, el comando sale con **6** sin pagar la síntesis; con `Skip`, termina con **0** `skipped` también sin pagarla. Decidir solo después obligaría a pagar la síntesis entera para descubrir que no se puede o no hace falta guardar, y con `--play` además a recorrer el bucle hasta «aceptar».
- **Al escribir** (`persist_locution`), y **esta es la que gobierna el contrato**: entre la decisión previa y la escritura hay una ventana —el bucle puede durar minutos— y la etiqueta puede quedar tomada en ese intervalo. Si al escribir está tomada, `Fail` sale con **6** `label_exists` y `Skip` conserva la existente y sale con **0** `skipped`.

**`--skip-existing` hace idempotente la pre-síntesis.** Repetir la misma orden sobre una etiqueta ya guardada es un éxito explícito, y no una negativa que el consumidor tenga que reinterpretar: el código de salida basta para saber si el audio sintetizado está en el almacén.

**El audio sintetizado corresponde siempre al contenido actual de su voz.** La CLI resuelve la identidad de la voz (`VoiceStore::resolve`: la ruta del `reference.qvoice` y su huella sha256) antes de sintetizar, en las dos rutas, y la escritura vuelve a comprobarla. Si la voz se re-clonó o se eliminó mientras se sintetizaba, no se escribe nada y la salida es **6** con `voice_changed`. Las dos comprobaciones de escritura viven en un único punto, `avi_store::persist_locution`, que es la única forma de escribir un audio sintetizado y que comprueba la voz antes que la etiqueta; entre la comprobación y la escritura queda una ventana de milisegundos que se acepta.

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

1. **Evento `started` inmediato**: se emite tras superar las validaciones baratas (parámetros, nombres, formatos) y justo antes de iniciar el trabajo pesado o la toma de locks. Los tres endpoints lo emiten con el mismo nombre y **sin texto humano**: ni `operation` ni `message`. Conserva solo los datos de la petición (`voice` y `text_length` en `/synthesize`, `name` en `/voices/clone`, `voice` en `/dub`). El texto que el usuario lee al arrancar la operación lo pone el cliente.
2. **Latidos periódicos (`heartbeat`) y progreso (`progress`)**: cada etapa de trabajo pesado (limpieza de ruido y su espera de turno, transcripción, traducción, clonado) se anuncia con `{"event":"progress","stage":"<etapa>"}` al empezar, aunque termine antes del primer latido, y después el daemon emite `{"event":"heartbeat","stage":"<etapa>"}` cada ~500 ms mientras el trabajo en segundo plano continúa. Ambos llevan únicamente `event` y `stage`: ni `message` ni `percent`. La fase de síntesis dice en cada momento lo que el daemon hace de verdad: al entrar emite de inmediato un `progress` con la etapa real, `synthesis` si consigue el lock de síntesis al instante, `warming` si debe esperarlo mientras el daemon aún calienta el motor o `queued` si otra síntesis retiene el lock, con latidos de esa etapa hasta obtenerlo; al obtenerlo tras esperar, `progress` `synthesis`. Si dentro del trabajo el motor debe arrancar un residente (primer uso, cambio de voz o residente degradado), le sigue de inmediato `progress` `loading_voice`, los latidos pasan a esa etapa, y vuelve `progress` `synthesis` cuando el residente está sano. Los latidos usan siempre la etapa vigente. Si la petición traduce, `/synthesize` traduce fuera del runtime de síntesis bajo la etapa `translate` y entra después en la fase de síntesis; `/dub` anuncia igualmente la etapa `translate` durante su traducción. Así una petición en espera nunca queda muda ni agota el timeout de inactividad.
3. **Evento `audio` (solo en `/synthesize` y `/dub`)**: uno por cada fragmento de la entrega en streaming del motor, `{"event":"audio","pcm_b64":"<PCM s16le, 24 kHz, mono, en base64>"}`, intercalado con los latidos de la etapa `synthesis`. El audio es la concatenación, en orden, de los `pcm_b64`; el evento final ya no lo lleva.
4. **Evento final (`result` o `error`)**: transporta el resultado exitoso en su forma contractual (`{name, speech, precomputed: true, removed_speech}` en clonado; `{translated, work_ms}` en síntesis; `{status: "dubbed", text, translated, voice, work_ms}` en dub) o el objeto de error, que sí lleva `reason` y `message`. Un fallo a mitad de entrega emite `error` (`synthesis_failed` o `synthesis_timeout`) después de los eventos `audio` ya enviados, y el cliente corta la reproducción y termina con ese error.

**El cliente es el dueño de todos los textos humanos.** El stream transporta datos y etapas, nunca frases para el usuario; la CLI traduce cada `stage` a su fase con una tabla única, de modo que los textos son idénticos en la vía directa y en la del daemon y ninguna fase lleva un sufijo de ruta.

**Las etapas que el daemon nombra.** El daemon emite nueve `stage` distintos, y la CLI los traduce a su texto propio: `warming` (calentando motor de síntesis), `queued` (en cola de síntesis), `loading_voice` (el motor arranca el residente de la voz pedida), `denoise_queue` (esperando turno para limpiar el ruido), `denoise` (limpiando ruido del audio), `synthesis` (sintetizando voz), `clone` (clonando voz), `transcribe` (transcribiendo audio) y `translate` (traduciendo texto). `warming`, `queued`, `loading_voice` y `synthesis` son la misma etapa para la CLI, `synthesizing`, con distinto texto vivo (`Calentando el motor…`, `Esperando turno para sintetizar…`, `Cargando la voz…` y `Sintetizando…`) y un solo reloj. `/voices/clone` no anuncia la precarga de la voz: sigue en segundo plano tras el `result`, y `precomputed: true` significa «precarga iniciada». `/transcribe` y `/translate` no usan este transporte: responden JSON plano. Un `stage` que no está en esa lista **se ignora**: no se anuncia ni un texto genérico que confunda una etapa con otra. La etapa se anuncia **una vez** en no-TTY —la actividad repetida dentro de la misma etapa no abre línea nueva— y la fase se repinta sobre la misma línea en TTY.

**Semántica del timeout en el cliente:** el timeout de inactividad entre latidos (`STREAM_INACTIVITY_TIMEOUT` en [`src/main.rs`](../../src/main.rs)) solo salta si el bucle del daemon se atasca sin emitir señal alguna; se complementa con un plazo total de seguridad failsafe (`STREAM_TOTAL_DEADLINE`, el mismo techo `REQUEST_FAILSAFE` del cliente HTTP, definido en [`limits.rs`](../../crates/avi-core/src/limits.rs): el presupuesto del texto máximo más una holgura para el calentamiento, la transcripción y la traducción). Ante una desconexión prematura del cliente, el servidor deja de emitir y descarta el resultado; en la síntesis cancela además el trabajo del motor TTS, que queda libre para la siguiente petición. La transcripción, la traducción y el clonado terminan en segundo plano y su resultado se descarta.

**Presupuesto de síntesis.** La síntesis está acotada por `synthesis_budget` ([`limits.rs`](../../crates/avi-core/src/limits.rs)): una parte fija más una parte proporcional a los caracteres del texto que realmente se sintetiza (el ya traducido, en `dub`). La parte fija cubre la primera inferencia de un motor recién lanzado, bastante más lenta que las siguientes. El presupuesto es el plazo total de la petición al motor, no un plazo por lectura. Al vencer, el daemon emite el evento de error `synthesis_timeout` (exit 1) y cancela el trabajo en el motor TTS residente, que lo aborta y queda caliente para la siguiente petición; si el motor no responde, se reemplaza antes de la siguiente síntesis. La vía directa devuelve el mismo `synthesis_timeout` con exit 1.

### Por qué `--daemon` significa exigir y no seleccionar

Con la autodetección por defecto, «usa el daemon» deja de ser algo que haya que pedir. Sin el flag, el llamador no tendría forma de exigir la ruta rápida y el código 5 **se quedaría sin ningún productor en la síntesis**: si la ausencia del daemon siempre degrada, nunca hay «daemon inalcanzable», solo una invocación más lenta. Un consumidor con presupuesto de latencia —el narrator es el caso previsto— necesita poder decir «prefiero fallar a esperar a que cargue el modelo».

Con los dos flags declarados, la exclusión mutua entre ellos tiene sentido pleno: «exige daemon» y «prohíbe daemon» se contradicen.

### Despacho y modo directo

Con el daemon activo, `speech synthesize`/`say`/`transcribe`/`dub` y `voice clone` pueden usar modelo caliente; `--no-daemon` fuerza ruta directa sin sondeo.

## 6. Reglas de validación

### Las seis reglas, todas con exit 2

1. **`--daemon` y `--no-daemon` son excluyentes.** La resuelve el parser (`conflicts_with` en el par que comparten los subcomandos con dos vías, [§5](#5-el-despacho-al-daemon)), no una comprobación a mano. Los demás comandos no declaran ninguno de los dos y los rechazan como argumento desconocido, también por el parser.
2. **`--json` es incompatible con `--play`.** El bucle escribe la pregunta y lee la respuesta por los canales estándar, y contaminaría el payload. Aplica a `speech synthesize`.
3. **`--text` no vacío ni solo espacios.** Aplica a `speech synthesize` y `speech say`.
4. **El texto a sintetizar no excede `MAX_TEXT_LENGTH`** (500 caracteres Unicode, no bytes; `reason` `text_too_long`). Se valida **en el cliente** antes de cualquier despacho, con el mismo código por ambas vías; el tope del daemon es defensa en profundidad (responde HTTP 400 con `{schema_version, error, reason, message}` en `/synthesize`, y un evento `error` `text_too_long` en `/dub`) y no la fuente de la validación. Aplica a `speech synthesize` y `speech say` (el `--text`) y a `speech dub` (el texto ya traducido). En `speech dub`, además, **el audio de entrada no dura más de `MAX_DUB_AUDIO_SECS`** (40 s; `reason` `audio_too_long`), sea `--audio`, `--mic` o `--duration`: un `--duration` superior sale con 2 y el push-to-talk se corta a 40 s. Los otros dos puntos de entrada de audio tienen su propio tope, igual en la vía local y en la del daemon: **el audio de `speech transcribe` no dura más de `MAX_TRANSCRIBE_AUDIO_SECS`** (300 s) y **la referencia de `voice clone` no dura más de `MAX_CLONE_REFERENCE_SECS`** (30 s). Un exceso sale con 2 y `reason` `audio_too_long`, con un mensaje que nombra la operación. En la CLI la comprobación precede a la carga de modelos (`model_missing`), a `stt_unsupported` y al contacto con el daemon; la duración se mide primero por la cabecera del WAV y después por el PCM decodificado, y solo se admiten WAV. En `speech transcribe --mic`, un `--duration` superior a 300 y un `AVI_PUSH_TO_TALK_MAX_SECS` superior a 300 (push-to-talk) salen con 2 `audio_too_long` antes de grabar y, en el segundo caso, antes de exigir terminal. El daemon repite el tope como defensa en profundidad: `/transcribe` rechaza con HTTP 400 `audio_too_long` más de 300 s de muestras y `/voices/clone` rechaza con 400 `audio_too_long` una referencia de más de 30 s, antes de comprobar el modelo; además, cada ruta limita el cuerpo de la petición (`body_limit_for` en [`limits.rs`](../../crates/avi-core/src/limits.rs) dimensiona el tope de `/transcribe` y el de `/voices/clone` a partir de su duración máxima y de una tasa de bytes por segundo) y responde 413 si se excede, aunque la duración sea menor que el tope.
5. **`--play` exige terminal en la entrada estándar.** Si no la hay, se rechaza **antes de sintetizar**. Aplica a `speech synthesize`.
6. **`--skip-existing` es incompatible con `--force` y con `--play`.** Conservar el audio sintetizado existente contradice sobrescribirla, y omitir la síntesis deja al bucle sin toma que reproducir. La resuelve el parser (`conflicts_with_all`) con `usage_error`. Aplica a `speech synthesize`.

**La regla 5 es de otra clase que las demás**: las otras miran los flags, la quinta mira el entorno. La comprobación no altera ningún default —`--play` es explícito, así que la misma línea de comandos no puede significar cosas distintas según dónde corra—; solo rechaza antes una invocación que iba a fallar igual. Lo único que queda fuera de alcance es alimentar las respuestas del bucle por una tubería, un caso marginal cuyo precio, de conservarlo, sería pagar una síntesis y una reproducción completas antes de fallar.

### El orden de validación de la familia de síntesis

`speech synthesize`, `speech say` y `speech dub` validan en el mismo orden por las dos rutas, el que fija `prepare_synthesis` en `src/main.rs`: **invocación → almacén → recursos**.

1. **Invocación** (exit 2): las reglas anteriores, el texto, la temperatura, el audio de entrada de `dub` y los nombres de voz y de etiqueta.
2. **Almacén**: que la voz exista (exit 3) y, en `synthesize`, la política de colisión de la etiqueta (exit 6 con `Fail`, o exit 0 `skipped` con `--skip-existing`).
3. **Recursos**, ya decidida la ruta: el modelo (exit 4) por la ruta directa o el daemon (exit 5) por la suya.

**Con dos fallos a la vez gana el del paso anterior**, en los tres comandos y en las dos rutas: una etiqueta ilegal con el modelo ausente sale con 2, una voz inexistente con el daemon caído sale con 3 y una etiqueta ocupada con el daemon caído sale con 6. El orden va de lo barato y local a lo costoso, y el almacén se consulta en el cliente también por la ruta del daemon porque los dos comparten el directorio de datos; el daemon conserva su propia comprobación como defensa. Por eso `--skip-existing` responde `skipped` aunque el daemon esté caído o el modelo falte.

### Un solo mecanismo para la exclusión mutua, y es el declarativo

La exclusión mutua se declara con `clap` (`conflicts_with` o un `ArgGroup`) en `src/main.rs` (`Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`), junto a los flags que restringe, en todos los sitios donde exista —el grupo de tres modos de `setup` incluido. **La garantía queda en un solo lugar, no repetida por convención en cada comando.** Una comprobación manual es esa convención repetida, y no escala: en un grupo de tres modos, un cuarto añadido a mano no rompe nada y deja de cubrir una combinación en silencio; el `if` vive lejos de los flags que restringe, donde nadie que añada uno lo va a leer.

El coste es que el mensaje lo formatea `clap` en inglés, igual que el de todas las demás rutas de parseo, y ese mensaje entra íntegro en el payload de error.

### Validación de identificadores y de existencia

| Situación | Superficies | Código y `reason` |
|---|---|---|
| Etiqueta ilegal (caracteres fuera de `[A-Za-z0-9._-]`, vacía, `.` o `..`) | `synthesize`, `play`, `remove` | **2** `invalid_identifier` |
| Nombre de voz ilegal (caracteres fuera de `[A-Za-z0-9._-]`, vacío, `.` o `..`) | Las que toman `--voice` o `--name`: `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `voice clone` y `voice remove` | **2** `invalid_voice_name` |
| Voz inexistente | **Las seis que toman `--voice`**: `synthesize`, `say`, `dub`, `play`, `list`, `remove` | **3** `voice_not_found` |
| Etiqueta inexistente | `play`, `remove` | **3** `speech_not_found` |
| Colisión de etiqueta sin `--force` ni `--skip-existing` | `synthesize` | **6** `label_exists` |
| Colisión de etiqueta con `--skip-existing` | `synthesize` | **0** `skipped` |
| Colisión de nombre de voz sin `--force` | `voice clone` | **6** `voice_exists` |

**`.` y `..` no son nombres.** Un nombre se une a una ruta del almacén, y esos dos lo sacarían de él: `voice remove --name .` borraría `voices/` y `speech/` enteros. Por eso los rechaza el validador único y salen con 2 sin tocar ningún archivo, por la CLI y por el daemon.

**La voz se valida en las seis sub-acciones que toman `--voice` y sale 3 si no está** —el catálogo es el registro unificado `VoiceStore` (`crates/avi-store/src/lib.rs` `FACTORY_VOICES`): voz de fábrica (`default` —clonada con `reference.qvoice`—) más clonadas del usuario—, de modo que «voz mal escrita» nunca se disfrace de «sin resultados»: sin esa regla, `speech play --voice noexiste` devolvería un error de etiqueta inexistente y un usuario que se equivoca al escribir concluiría que sus audios sintetizados se perdieron en vez de que escribió mal el nombre. Con `--voice` opcional en las seis, la pregunta es la misma en todas y la respuesta también. (`list`, `play` y `remove` validan igual y antes de buscar nada: voz ilegal → 2 `invalid_voice_name` y etiqueta ilegal → 2 `invalid_identifier` al construir `VoiceName` y `Label`, voz inexistente → 3 `voice_not_found` vía `VoiceStore::exists` del almacén de voces; `speech list` sin `--voice` lista todas las voces. La regla es sostenible porque todo audio sintetizado guardado pertenece a una voz existente: `voice remove` y `cleanup --voices` se llevan los audios sintetizados con su voz.)

La etiqueta inexistente sale **3** y no 2: la invocación está bien formada y el recurso no está, que es exactamente lo que el 3 significa.

**La colisión de etiqueta y la de nombre de voz son el mismo hecho** —el recurso está ocupado y hay que liberarlo o forzar— y comparten código. Con el almacén etiquetado, la colisión no es un caso esporádico: ocurre cada vez que se regenera un audio sintetizado ya existente, que es flujo normal de trabajo.

### Ningún flag queda sin efecto sin que la CLI lo diga

La afirmación vale con una excepción declarada: **`--force` o `--skip-existing` sobre una etiqueta libre son un no-op**, igual que `voice clone --force` sobre un nombre libre. Fuera de ese caso, toda combinación de flags tiene efecto declarado o sale con 2, 3 o 6.

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
| `-t T -l L --skip-existing` *(L existe)* | no | no | no, conserva la existente | 0 (`skipped`) |
| `-t T -l L --skip-existing` *(L libre)* | sí | no | sí | 0 |
| `-t T -l L --skip-existing --daemon` *(L existe, daemon caído)* | no | no | no, conserva la existente | 0 (`skipped`) |
| `-t T -l L --skip-existing -f` o `--skip-existing -p` | — | — | — | **2** |
| `-t T -l L -p` *(L libre al empezar, tomada al aceptar, sin `-f`)* | sí | sí, en el bucle | no | **6** |
| `-t T -l L -p` *(sin terminal)* | — | — | — | **2** |
| `-t T -l L -p --json` | — | — | — | **2** |
| `-t T` *(sin `-l`)* | — | — | — | **2** |
| `-t T -l L` *(etiqueta ilegal)* | — | — | — | **2** |
| `-t T -l L -v V` *(V no existe)* | — | — | — | **3** |
| `-t T -l L --daemon` *(daemon caído)* | — | — | — | **5** |
| `-t T -l L` *(modelo no provisionado)* | — | — | — | **4** |
| `-t T -l L -v V` *(V no existe, daemon caído o modelo no provisionado)* | — | — | — | **3** |

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

La única interacción entre `--json` y el comportamiento es la regla 2: `--json` con `--play` es exit 2, así que bajo `--json` el bucle es inalcanzable y **el audio sintetizado de `synthesize` está en el almacén** siempre que la salida sea 0: guardada (`success`) o ya existente (`skipped`).

## 8. El almacén de audios sintetizados

### Ubicación y layout

`<data_dir>/speech/<voz>/<etiqueta>.wav` (`crates/avi-store/src/lib.rs` `SpeechStore`), **raíz hermana de `voices/`** (`VoiceStore`: `<data_dir>/voices/<nombre>/`; raíz de modelos: `models_cache_dir()`).

**Por qué no anidado en `voices/<voz>/speech/`**, que sería la opción intuitiva y ahorraría código de borrado: la voz de fábrica (`default` —clonada con `reference.qvoice`—; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) es una entrada del registro `VoiceStore` y el almacén separa la salida generada (`speech/`) del registro (`voices/`).

Coste aceptado de la raíz separada: el arrastre de los audios sintetizados al borrar o reemplazar una voz no es gratis y exige código explícito. Lo hacen los caminos que borran o reemplazan voces con el mismo criterio de propiedad —los audios sintetizados se van con su voz—: `voice remove` (`avi_store::remove_voice`, que borra el namespace de habla antes que el registro), `cleanup --voices` (que ubica `speech/<voz>` con el mismo `SpeechStore`) y `voice clone --force` (`avi_store::replace_voice`, que borra el namespace de habla antes de publicar la referencia nueva). Así valen los invariantes «audio sintetizado existente ⇒ voz existente» y «audio sintetizado existente ⇒ grabado con el contenido actual de su voz». El daemon solo toca `speech/` a través de `replace_voice`, al re-clonar; los audios sintetizados los escribe siempre la CLI. Como el arrastre borra datos que el usuario no nombró, `voice remove` de una voz con audios sintetizados es destructivo y exige confirmación (ver la sección `voice remove` de `commands/VOICE.md`).

El almacén lo escribe y lo lee **solo el cliente**: es salida de síntesis y el daemon jamás lo toca.

### El `.wav` es el recurso de registro

Cada audio sintetizado son dos archivos, y **el `.wav` manda**. El `.json` son metadatos derivados.

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
- **`speech list` tolera un sidecar ausente** mostrando el audio sintetizado sin metadatos (`text`, `created_at` y `duration_secs` a `null` en `--json`), en vez de fallar. Muestra el texto **truncado** en la salida humana y **completo** en el payload `--json`.
- **Un sidecar ilegible o corrupto no se tolera**: `speech list` y `speech play` fallan con exit 1 `speech_metadata_corrupt`, nombrando la etiqueta y la voz; la solución remite a `speech remove`, que borra el WAV y el sidecar sin interpretarlo. El fichero y la causa viajan en el texto de `error` del envelope.

### Atomicidad de la escritura

Cada archivo —el sidecar `.json` y el WAV— se escribe a un temporal en el mismo directorio y se publica con `rename` atómico, de modo que una interrupción no deje un WAV truncado que `speech list` mostraría como válido y `speech play` intentaría reproducir, ni un sidecar a medias que haría fallar `speech list`.

**El sidecar se publica antes del WAV**, así que la aparición del `.wav` implica que sus metadatos ya están completos. Combinado con que el WAV es el recurso de registro, una interrupción entre ambos `rename` deja basura inocua: el sidecar huérfano no ocupa la etiqueta, y `speech remove` lo alcanza.

### Validación de identificadores

La etiqueta y el nombre de voz son la misma clase de identificador: un segmento de ruta. Los almacenes solo construyen rutas a partir de los tipos **`VoiceName` y `Label`** (`crates/avi-store/src/lib.rs`), que solo se construyen con `parse`, el **validador único**: no vacío, solo ASCII alfanumérico, `.`, `_` o `-`, y distinto de `.` y `..`. Como el compilador exige el tipo en toda la API de los almacenes, ningún comando puede olvidar la validación, y salir de un almacén es imposible por construcción; por eso no hay una segunda comprobación de contención de rutas.

- **El tipo determina el sujeto del mensaje** —«El nombre de voz '…' no es válido» frente a «La etiqueta '…' no es válida»— y el `reason`: `invalid_voice_name` para la voz e `invalid_identifier` para la etiqueta, de modo que `speech synthesize --label "mi saludo"` no culpe a `--voice`.
- **Los nombres se normalizan a minúsculas**, porque el validador lo hace deliberadamente para evitar colisiones en filesystems case-insensitive. `--label Saludo` y `--label saludo` son la misma etiqueta, y el archivo se llama `saludo.wav`. Se declara en el help de `--label` y en `USAGE.md`.
- **Un directorio o un WAV cuyo nombre no supera el validador no se lista**: ningún comando podría nombrarlo.
- Un identificador ilegal sale con **2**, sea voz o etiqueta, por la CLI y por el daemon (HTTP 400 `invalid_voice_name` en `/synthesize`, `/dub` y `/voices/clone`).

## 9. Los códigos de salida

### La tabla

Los códigos de salida con su constante `ExitCode` y su significado están en la tabla de [`README.md`](README.md#códigos-de-salida); este documento fija las reglas que los reparten. Dos significados que esa tabla resume y que aquí son normativos: el `2` incluye `--daemon` o `--no-daemon` en un comando que no los declara (argumento desconocido, `usage_error`), y el `10` no cubre un audio ausente (3), ilegible (2) o con error de E/S (1).

**Los enteros del 11 al 22 son de la tabla cerrada del ciclo de vida, uno por `reason`**, y no se reparten por el eje de dos preguntas de §1 como los anteriores: cada uno corresponde a un `reason` que la [tabla de reglas transversales de la especificación](../specs/LIFECYCLE.md#81-reglas-transversales) declara, y la correspondencia es 1:1 con la variante de `ExitCode` (`crates/avi-core/src/exit_codes.rs`). El `17` lo reciben también los comandos de uso (`translate`, `voice`, `speech`, `setup`, `daemon start`, `daemon restart`, `daemon serve`) mientras dura una operación de ciclo de vida, sin ejecutarse. Los dos casos que rompen el patrón son deliberados y son los que hay que recordar al leer la tabla:

- **El 11 no es un error.** `setup_failed` es un **éxito parcial**: el programa está instalado, el resumen y el envelope se emiten igual, y lo único que cambia es el `reason` del envelope y el código de salida. Por eso el envelope de `self install` sale por *veredicto* y no por el objeto `error` de §10.
- **El 15 y el 17 son los que el ejecutable sin bundle alrededor y el bloqueo ya tomado producen**, y son los dos que un usuario se encuentra sin haber hecho nada mal: `target\debug\ai-voice-interconnector.exe self install` responde `bundle_invalid` (15) porque no hay bundle alrededor, no porque la instalación esté rota.

`unsupported_platform` (18), `binary_incompatible` (19), `network_error` (20), `checksum_mismatch` (21) y `program_dir_kept` (22) **tienen variante propia**: los emiten `self update`, la descarga de `setup`, la comprobación de arranque del bundle y el paso 8 de `self uninstall`, con la traducción en `ExitCode::from_reason` y las variantes en la cabecera de `crates/avi-core/src/exit_codes.rs`. `network_error` es solo fallo de transporte o del servidor (sin red, 5xx, límite de peticiones), que reintentar puede resolver. Cuando el servidor **rechaza** el recurso pedido el `reason` es otro y sale con **1**, porque su remedio no es un comando de esta CLI: `model_unavailable` cuando Hugging Face responde 401, 403 o 404 a un modelo fijado (`setup`, y anidado en `models_cause` de `self install` y `self update`), y `release_unavailable` cuando GitHub responde 404 a la versión o al asset pedido (`self update` y los instaladores).

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
| `1` | `io_error`, `model_unavailable`, `release_unavailable`, `voice_clone_failed`, `synthesis_failed`, `synthesis_timeout`, `stt_unsupported`, `translation_unsupported`, `denoise_unsupported`, `denoise_failed`, `playback_failed`, `audio_enumeration_failed`, `speech_list_failed`, `voice_list_failed`, `voice_store_init_failed`, `daemon_error`, `startup_failed`, `sudo_not_supported`, `lifecycle_failed`, `doctor_failed`, `self_install_failed`, `self_uninstall_failed`, `self_update_failed`, `speech_metadata_corrupt` |
| `2` | `confirmation_required`, `usage_error`, `empty_text`, `text_too_long`, `invalid_voice_name`, `invalid_identifier`, `cannot_remove_default`, `cannot_clone_default`, `invalid_audio`, `empty_audio`, `empty_transcript`, `audio_too_long`, `output_exists` |
| `3` | `voice_not_found`, `warm_voice_missing`, `audio_not_found`, `speech_not_found`, `not_installed`, `no_input_device`, `no_output_device`, `audio_backend_unavailable` |
| `4` | `model_missing` |
| `5` | `daemon_unreachable` |
| `6` | `voice_exists`, `label_exists`, `voice_changed`, `port_in_use`, `denoise_busy` |
| `9` | `translation_failed` |
| `10` | `transcription_failed` |
| `11` a `22` | `setup_failed`, `externally_managed`, `rolled_back`, `path_conflict`, `bundle_invalid`, `daemon_stop_failed`, `lifecycle_locked`, `unsupported_platform`, `binary_incompatible`, `network_error`, `checksum_mismatch`, `program_dir_kept`, en ese orden |

**Los tres `reason` del entorno de audio** son recursos ausentes del sistema, no fallos del producto, y salen con 3 en cualquier operación de audio (captura de micrófono, reproducción y `devices`): `no_input_device` significa que el sistema no tiene entrada (micrófono) predeterminada; `no_output_device`, que no tiene salida predeterminada; `audio_backend_unavailable`, que no tiene backend de audio (en Linux, falta ALSA, `libasound.so.2`, o no define alguna función que se usa). La captura y la reproducción solo los emiten en la CLI, que es donde se graba y se reproduce. Una captura que termina sin muestras sale con 2 `empty_audio`, y el resto de fallos conserva el `reason` de la operación: `transcription_failed` (10) en la captura, `playback_failed` (1) en la reproducción y `audio_enumeration_failed` (1) en `devices`. `devices --json` informa en `default_input` y `default_output` de esa misma entrada y salida predeterminadas, así que un consumidor puede comprobarlas antes de capturar o reproducir.

`warm_voice_missing`, `model_missing` y `startup_failed` viajan en la línea `error=<reason>` del fichero ready del daemon; la CLI traduce el primero a `voice_not_found` (3), conserva `model_missing` (4) y traduce el tercero a `daemon_error` (1).

El motor tiene un único perfil de precisión, siempre activo: no hay variable ni opción para elegirlo.

**Los tres `reason` de la limpieza de ruido** distinguen tres hechos que el consumidor trata distinto: `denoise_unsupported` (1) significa que el binario se compiló sin `native-denoise` y la limpieza no existe en esa instalación; `denoise_busy` (6) significa que hay una inferencia DPDFNet activa y otra solicitud esperando, así que la limpieza no se inició y conviene reintentar cuando termine la solicitud en curso; `denoise_failed` (1) significa que el modelo estaba provisionado y la inferencia o el DSP fallaron. En los tres casos la solicitud **no** devuelve audio sin limpiar: `model_missing` (4) es la precondición de modelo y se comprueba antes de reservar buffers o crear temporales.

**El 6 tiene un solo dueño.** «Puerto ya en uso» y «la voz ya existe» son el mismo hecho y llevan el mismo código; no hay una constante aparte para el conflicto del daemon.

### El 2 significa lo que `clap` quiere decir con él

El exit 2 es, en Unix y en `clap`, el código del error de invocación, y aquí significa exactamente eso. Como consecuencia, **todas las rutas de fallo de parseo son correctas sin escribir una línea de validación**: flag requerido ausente, valor fuera de `choices`, grupo mutuamente excluyente violado (`conflicts_with`), subcomando inválido en los tres niveles, y flag desconocido en cualquier comando.

**La raíz sin subcomando explora (0); un grupo sin subcomando es un error de uso (2).** `ai-voice-interconnector` a secas muestra la ayuda y sale con `ExitCode::Ok`, igual que `--help` (con `--json`, el payload `help`), porque quien lo invoca así, persona o agente, necesita saber cómo corregir la invocación. `ai-voice-interconnector speech` a secas (y `voice`, `daemon`, `self`) sale con 2 y trae la ayuda del grupo, porque un script que olvida el subcomando de `self` o `daemon` no debe terminar con éxito sin hacer nada.

Dos pruebas de que la convención es la correcta:

1. **La tabla la honra en otro punto**: `ExitCode::Interrupted = 130` es exactamente `128 + SIGINT`. Respetar 128+n y no respetar 2 sería incoherente dentro de la misma tabla.
2. **El proyecto hermano aplica la misma convención**: `evolutivex-agents` usa **2 = uso incorrecto** en sus tres casos —valor fuera de dominio, argumento vacío y comando desconocido— con **1 = error genérico**.

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

El payload de error usa **dos claves de primer nivel** —`error` con el mensaje y `reason` con el literal de máquina—, ambas planas y no anidadas, y deja intacto el stderr en castellano para el uso humano. El `error` termina siempre en `.`, `?` o `!` y, si el fallo tiene un detalle técnico (el texto original de una biblioteca), lo lleva al final como ` Detalle: <detalle>`; no hay una clave `detail` aparte, y la línea `Solución:` es solo del texto humano. Los ejemplos de payload están en la [referencia de esquemas `--json`](../../USAGE.md#referencia-de-esquemas---json).

`reason` es la clave de la causa fina y es **aditiva**: los `reason` del ciclo de vida (`lifecycle_locked`, `bundle_invalid`, `daemon_stop_failed`, `path_conflict`, `rolled_back`, `externally_managed`, `confirmation_required`, `usage_error`, `unsupported_platform`, `binary_incompatible`, `network_error`, `checksum_mismatch`, `program_dir_kept`, `sudo_not_supported`, `model_unavailable`, `release_unavailable`) viajan por ella; los del ciclo de vida tienen además entero propio del 11 al 22, salvo `confirmation_required` y `usage_error` (2), `model_unavailable` y `release_unavailable` (1: el servidor rechaza el modelo fijado o el release pedido, y `network_error` queda para el fallo de transporte) y `sudo_not_supported` (1), que rechaza en Unix `self install`, `self uninstall` y `cleanup` lanzados con `sudo`, y un `reason` desconocido se trata como ausente. El fallo de parseo lleva `reason: "usage_error"`.

Las tres reglas de compatibilidad y la regla de promoción son contrato **de consumo** además de emisión: [`USAGE.md`](../../USAGE.md#comandos) declara explícitamente que un `reason` desconocido se trata como ausente.

### El mecanismo: un solo punto de traducción

**La invariante no se sostiene con un `if` por sitio**, porque eso la deja en manos de que nadie olvide uno. Es la misma solución que la ruta de éxito ya tiene con `emit_raw_json` (`crates/avi-core/src/json_emitter.rs`), cuyo doc enuncia el motivo: *«la garantía queda en un solo lugar, no repetida por convención en cada comando»*. La ruta de fallo tiene la misma forma:

- Los sitios de fallo retornan **`ExitCode` + `reason` + `message`**, más un `detail` opcional con el texto técnico de una biblioteca (tipo `CliError` en `crates/avi-core/src/exit_codes.rs` / `crates/avi-core/src/json_emitter.rs`), en vez de imprimir y salir.
- **`src/main.rs` (`main` / `handle_*` / `emit_raw_json`) es el único punto que lo traduce**: en texto, `Error:` y la línea `Solución:` de la tabla única de soluciones por `reason` (`REASON_SOLUTIONS` en `crates/avi-core/src/exit_codes.rs`, que cubre los mismos `reason` y en el mismo orden que `REASON_CODES`) a stderr; en `--json`, el payload a stdout; y salida con el código. No queda otro camino hasta la salida, así que la invariante no necesita vigilancia.
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
- El progreso de la descarga de modelos (`setup` y `self install`) es un mensaje del producto, no una traza: sale siempre por stderr. Las barras muestran `  <modelo>  <barra>  <porcentaje> %  <hecho> de <total>  <velocidad>  quedan <tiempo>`, se repintan cada 250 ms aunque no lleguen bytes y calculan el tiempo restante con la velocidad media global (`quedan —` durante los primeros 2 s de cada modelo); solo se dibujan con terminal. El encabezado (`Descargando N modelos:`), la línea de cada modelo terminado (`  ✓ [i/n] <modelo>  <tamaño> en <duración> · <velocidad>/s`) y el cierre del comando salen también sin ella; sin terminal el avance es `  … [i/n] <modelo>  <hecho> de <total> · <duración>`. La línea `AVI_SETUP_HEARTBEAT <hecho> <total>` que `setup` escribe con `AVI_SETUP_WATCHDOG` es un protocolo para el bootstrap del repositorio y no se traduce.
- stderr y stdout no llevan color ANSI cuando no son una terminal, ni cuando `NO_COLOR` está definida y no vacía.
- **`AVI_NO_PROGRESS` silencia las etapas.** Con cualquier valor no vacío, el indicador no escribe las líneas de etapa —ni la activa, ni las terminadas, ni los latidos—. El alcance es **solo stderr** y **solo las etapas**: el envelope `--json`, el texto del resultado, la línea de contexto, el cierre y los códigos de salida no cambian. Es el interruptor para un pipeline o un log donde el progreso estorba.
- **Los errores de texto no llevan texto de bibliotecas.** Salen como `Error:` más `Solución:`; el detalle técnico (el texto original de reqwest, hound, cpal o ffmpeg) solo aparece en el `error` de `--json` tras ` Detalle:` y en el registro de trazas.

**Los logs viven en `data/logs/`** y hay dos familias, ambas con nombre `<familia>_<token>_<ms>.log`, donde `<token>` es el token de propietario `<pid>-<inicio>` del proceso que escribe el log y `<ms>` la marca de tiempo de creación en milisegundos:

| Familia | Fichero | Contenido |
|---|---|---|
| Daemon | `daemon_<token>_<ms>.log` | stdout y stderr del daemon; uno por cada `daemon start`/`restart` (`<token>` es el del daemon hijo) |
| Motor Qwen3-TTS | `qwen3-tts_<token>_<ms>.log` | salida del motor residente y del motor de `voice clone` (`<token>` es el del proceso que lo crea: el daemon, o la CLI en la vía directa) |

El borrado diferido de Windows no escribe ningún log: un borrado correcto no deja registro, y si falla deja `<destino>.removal-failed.log` junto al destino.

**Retención: los 10 logs más recientes por familia**, ordenados por el `<ms>` del nombre. La poda ocurre al crear un log nuevo; no toca otras familias ni ficheros ajenos, y nunca borra el log de un propietario que sigue en marcha ni el de uno cuya identidad no se puede observar: solo compiten por la retención los de dueño muerto (inexistente o con otro inicio) y los del propio proceso. Un log que no se pueda borrar se ignora.

Cuando un arranque del daemon falla, el mensaje de error de la CLI termina con `Log del daemon: <ruta>` sin cambiar `reason` ni el exit code; si el log no se puede crear, el error es `daemon_error` («No se pudo crear el log del daemon»). El error `voice_clone_failed` termina con `Log del motor: <ruta>`.

### El contrato de salida por comando

Esta sección es el **contrato de salida** de cada comando: qué escribe por stdout, qué escribe por stderr, con qué código de salida y con qué `reason`. Los destinos, los códigos y los `reason` no cambian con el protocolo del indicador; lo que el protocolo fija es **qué va por cada flujo, quién dice el estado y cuántas veces**.

**Las reglas que la gobiernan.**

1. **stdout lleva solo resultados y stderr lleva todo lo demás.** Bajo `--json`, stdout lleva un objeto JSON (§10), salvo los registros JSONL de `speech denoise`. Sin `--json`, stdout lleva únicamente los resultados, en cuanto existen: los textos transcritos o traducidos, las listas, los planes de `--dry-run` y la ruta del archivo generado. Stderr lleva la línea de contexto, las etapas, el cierre, los planes que preceden a una confirmación, las preguntas, los avisos y los errores. Con `--json` no se escriben ni la línea de contexto ni los resultados de texto ni el cierre, porque el envelope los sustituye; las etapas siguen en stderr.
2. **Un solo camino de presentación y dos clases de etapa.** Cada comando abre con una línea de contexto (por ejemplo `Doblaje es → en · voz default · archivo a.wav · daemon`) y trabaja por etapas con nombre. Una etapa **permanente** deja, al terminar, la línea `✓ <texto>  <duración>` (`✓ Audio leído  0,1 s`), con la nota opcional ` · <nota>`; una etapa **transitoria** (validación, envío al daemon, guardado, espera de turno de la limpieza de ruido) se borra al terminar y no deja línea. Con terminal, la etapa en curso se repinta debajo de las ya terminadas como `… <texto activo> <duración de la etapa>`, y la grabación y la reproducción la mantienen viva con su contador. Sin terminal, una etapa permanente escribe `… <texto activo>` una vez, un latido cada 10 s y su línea `✓` al terminar; una transitoria no escribe nada. El comando cierra con una línea común `✓ <acción> en <duración total>`, más ` · <dato>` por cada dato extra, por stderr y una sola vez. Quien cierra es el handler de la CLI, o el flujo de la librería en `install`, `uninstall` y `cleanup`; una operación fallida no deja línea `✓`.
   - **Las esperas de la síntesis no son etapas.** El turno de síntesis, el calentamiento del motor y la carga de la voz son textos vivos de la etapa `synthesizing` (`Esperando turno para sintetizar…`, `Calentando el motor…`, `Cargando la voz…`), con un solo reloj: nunca cierran la síntesis con una línea `✓ Voz sintetizada`. Sin terminal, la etapa escribe una sola vez su anuncio inicial y sus latidos de 10 s; los cambios de texto posteriores no se escriben. Todo trabajo de varios segundos tiene su etapa visible, también la limpieza de ruido de `voice clone --denoise` en local.
   - **Excepción de etapa única.** Cuando un comando solo tiene una etapa de trabajo, esa etapa no deja su línea `✓` y queda solo el cierre, porque ambos dirían casi lo mismo: `translate`, `speech play`, `voice remove`, `doctor --repair`, `cleanup`, `voice clone` con archivo y sin `--denoise`, y `speech synthesize` sin `--play` y sin traducción. Con `--json` la regla no actúa: no hay cierre, y la línea de la etapa es la única que da su duración por stderr. Sin terminal se conservan el anuncio `… <texto activo>` y los latidos.
   - **La línea de contexto va después de validar y antes de trabajar.** Se escribe tras validar la invocación (flags, textos, nombres, archivos de entrada, su existencia y sus límites) y el almacén (voz existente, colisión de etiqueta o de voz), y antes de cualquier confirmación, del primer acceso a recursos (modelos, dispositivos de audio, daemon) y de cualquier trabajo. Un error de invocación o de almacén sale sin cabecera; un error de recurso o de trabajo sale con ella. La única excepción es la sonda que decide la ruta en modo automático, que va antes porque la cabecera nombra la ruta y nunca falla.
3. **Las duraciones tienen una sola forma**: `0,7 s` y `12,4 s` por debajo de 60 s (decimal con coma), `2 min 35 s` desde 60 s y `1 h 02 min` desde 1 h. El formato `m:ss` solo lo usa el contador de grabación. Los plurales siempre concuerdan (`1 ruta`, `2 rutas`) y nunca llevan paréntesis.
4. **Color.** Verde para `✓`, rojo para `Error:` y `✗`, amarillo para `Aviso:`, atenuado para duraciones y detalles, y negrita para los resultados. Solo se emite si el flujo concreto es una terminal, `NO_COLOR` no está definida o está vacía y la consola acepta secuencias ANSI; redirigida a un archivo o a una tubería, la salida nunca lleva secuencias SGR. **`AVI_NO_PROGRESS`** (cualquier valor no vacío) silencia solo las etapas —líneas activas, terminadas y latidos—, no el contexto, los resultados ni el cierre.
5. **`Error:` y `Solución:`.** En texto, todo error sale como `Error: <mensaje>.` y, si el `reason` tiene remedio en la tabla única de soluciones, una segunda línea `Solución: <orden>.` con la invocación real (`confirmation_required` repite la orden con `--yes`). El mensaje describe qué pasó y por qué, en español y con punto final; el texto de las bibliotecas (reqwest, hound, cpal, ffmpeg) no aparece en la salida de texto: viaja en el campo `detail` del error, que en `--json` se añade al final del `error` como ` Detalle: <detalle>` y siempre queda en el registro de trazas. La tabla cubre todos los `reason` de §9, y los que no tienen remedio (`usage_error`, `io_error`, `denoise_failed`…) no llevan `Solución:`.
6. **El `reason` solo viaja en la salida no-cero**, y solo en el payload de error (§10). En el éxito es `null` o está ausente; en el veredicto (§10) el `reason` propio del comando viaja dentro del payload del comando, no en un objeto `error` aparte.

**Excepción de salida para `speech denoise`.** El flag global `--json` de este comando selecciona JSONL: stdout contiene cero o más objetos JSON independientes, uno por evento de progreso/resultado, no un único envelope `schema_version`; cada registro mantiene su forma definida por el reporte del pipeline. Un error de validación previo al lote (opciones fuera de rango, entrada inexistente, FFmpeg ausente) sale como un único envelope `{error, reason}` por el canal general; los fallos de archivos dentro del lote se expresan como registros JSONL y por el código de salida, sin segundo envelope. Sin `--json`, usa el formato común: contexto `Limpieza de ruido · N archivos`, una etapa por archivo con su porcentaje, la ruta de cada archivo generado por stdout y el cierre `✓ Limpieza de ruido completada en <total> · N archivos limpios`; con `--dry-run` el plan por stdout (`<entrada> → <salida> (se procesaría | se sobrescribiría | se omitiría | fallaría: ya existe la salida)`) y `Simulación: no se ha modificado nada.` por stderr. Esta excepción se limita a la interfaz del pipeline de archivos y no cambia el envelope `--json` ni el protocolo NDJSON del daemon para otros comandos.

**La tabla.** «Etapas» son los `label` de etapa que el comando nombra (en la vía daemon se antepone la etapa transitoria `started`, el envío al daemon); «Cierre» es la línea que dice el dueño, por stderr, con la duración total. Un guion significa que el comando no emite esa clase de salida. Los comandos de lista y de borrado individual (`voice list`, `speech list`, `devices`) no tienen línea de contexto.

| Comando | stdout (texto) | stdout (`--json`) | stderr: contexto | Etapas | Cierre (stderr) | Código y `reason` |
|---|---|---|---|---|---|---|
| `translate` (origen = destino) | el texto original | `{ "translated", "source_language", "target_language" }` | — | — | — | 0 |
| `translate` (vía daemon) | el texto traducido, sin etiqueta | `{ "translated", "source_language", "target_language" }` | `Traducción es → en · daemon` | `translating` (etapa única: sin línea propia) | `✓ Traducción completada en <total>` | 0; 9 `translation_failed`; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable` |
| `translate` (local) | el texto traducido, sin etiqueta | `{ "translated", "source_language", "target_language" }` | `Traducción es → en · local` | `translating` (etapa única: sin línea propia) | `✓ Traducción completada en <total>` | 0; 9 `translation_failed`; 2 `empty_text`, `translation_unsupported`; 4 `model_missing` |
| `voice list` | `Voces:` y una línea `  <nombre>` por voz, con ` (de fábrica)` cuando corresponde | `{ "voices": [...] }` | — | — | — | 0; 1 `voice_list_failed` |
| `voice clone` (vía daemon) | — | `{ "name", "speech", "precomputed", "removed_speech", "schema_version" }` o `{ "status": "cancelled", "name", "removed_speech" }` | `Clonación de la voz <nombre> · archivo <nombre> · daemon` (o `micrófono (máximo m:ss)`); con `--force` sobre una voz con audios sintetizados, el plan de borrado y la confirmación | `validating`, `capturing` (solo `--mic`), `denoising` (solo `--denoise`), `cloning` (con archivo y sin `--denoise` es la única etapa de trabajo y no deja línea propia) | `✓ Voz <nombre> clonada en <total>`, más ` · se borraron N audios sintetizados anteriores` si se sobrescribió con audios; si se cancela, solo `Cancelado; no se ha borrado nada.` | 0; 1 `voice_clone_failed`, `io_error`; 2 `usage_error`, `invalid_voice_name`, `invalid_audio`, `audio_too_long`, `empty_audio`, `confirmation_required`; 3 `audio_not_found`, `no_input_device`, `audio_backend_unavailable`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `voice_exists`; 10 `transcription_failed` |
| `voice clone` (local) | — | `{ "name", "speech", "precomputed": false, "removed_speech", "schema_version" }` o `{ "status": "cancelled", "name", "removed_speech" }` | `Clonación de la voz <nombre> · archivo <nombre> · local` (o `micrófono (máximo m:ss)`); con `--force` sobre una voz con audios sintetizados, el plan de borrado y la confirmación | `validating`, `capturing` (solo `--mic`), `denoising` (solo `--denoise`), `cloning` (con archivo y sin `--denoise` es la única etapa de trabajo y no deja línea propia), `saving` (transitoria) | `✓ Voz <nombre> clonada en <total>`, más ` · se borraron N audios sintetizados anteriores` si se sobrescribió con audios; si se cancela, solo `Cancelado; no se ha borrado nada.` | 0; 1 `voice_clone_failed`, `io_error`; 2 `usage_error`, `invalid_voice_name`, `invalid_audio`, `audio_too_long`, `empty_audio`, `confirmation_required`; 3 `audio_not_found`, `no_input_device`, `audio_backend_unavailable`; 4 `model_missing`; 6 `voice_exists`; 10 `transcription_failed` |
| `voice remove` | — | `{ "status": "removed"\|"cancelled", "voice", "removed_speech" }` | el plan de borrado y la pregunta `[s/N]`, siempre (con o sin audios sintetizados) | `removing` (etapa única: sin línea propia) | `✓ Voz <nombre> borrada en <total>`, más ` · y N audios sintetizados` si los tenía; si se cancela, `Cancelado; no se ha borrado nada.` | 0; 1 `io_error`; 2 `cannot_remove_default`, `invalid_voice_name`; 3 `voice_not_found` |
| `speech list` | `Audios sintetizados guardados:` y una línea `  <voz>  <etiqueta>  <duración>  «<texto>»` por audio (`(sin metadatos)` si falta el sidecar), o `No hay audios sintetizados guardados.` | `{ "speech": [...] }` | — | — | — | 0; 1 `io_error`, `speech_list_failed`; 2 `invalid_voice_name`; 3 `voice_not_found` |
| `speech transcribe` (vía daemon) | el texto transcrito, sin etiqueta | `{ "text", "source_language" }` | `Transcripción es · archivo <nombre> · daemon` (o `micrófono (máximo m:ss)`) | `capturing` (solo `--mic`), `reading` (solo `--audio`), `transcribing` | `✓ Transcripción completada en <total>` | 0; 1 `daemon_error`; 2 `usage_error`, `invalid_audio`, `audio_too_long`, `empty_audio`, `empty_transcript`; 3 `audio_not_found`, `no_input_device`, `audio_backend_unavailable`; 4 `model_missing`; 5 `daemon_unreachable`; 10 `transcription_failed` |
| `speech transcribe` (local) | el texto transcrito, sin etiqueta | `{ "text", "source_language" }` | `Transcripción es · archivo <nombre> · local` (o `micrófono (máximo m:ss)`) | `capturing` (solo `--mic`), `reading` (solo `--audio`), `transcribing` | `✓ Transcripción completada en <total>` | 0; 1 `io_error`; 2 `usage_error`, `invalid_audio`, `audio_too_long`, `empty_audio`, `empty_transcript`; 3 `audio_not_found`, `no_input_device`, `audio_backend_unavailable`; 4 `model_missing`; 10 `transcription_failed`, `stt_unsupported` |
| `speech synthesize` (vía daemon) | la ruta del WAV guardado (con `--skip-existing`, la del ya existente) | `{ "status": "success", "audio_path", "voice" }`, `{ "status": "skipped", "audio_path", "voice" }` o `{ "status": "discarded", "voice" }` | `Síntesis es → en · voz <voz> · etiqueta <etiqueta> · daemon` | `validating`, `translating` (si difieren los idiomas), `synthesizing` (sin `--play` y sin traducción es la única etapa de trabajo y no deja línea propia), `saving` | `✓ Síntesis completada en <total> · <duración> de audio`; `✓ Ya existía; se conserva sin sintetizar de nuevo.`; `✓ Audio descartado; no se ha guardado.` | 0; 1 `synthesis_failed`, `daemon_error`, `io_error`, `playback_failed` (solo `--play`); 2 `usage_error`, `text_too_long`, `invalid_voice_name`, `invalid_identifier`; 3 `voice_not_found`, `no_output_device` y `audio_backend_unavailable` (solo `--play`); 4 `model_missing`; 5 `daemon_unreachable`; 6 `label_exists`, `voice_changed`; 9 `translation_failed` |
| `speech synthesize` (local) | la ruta del WAV guardado (con `--skip-existing`, la del ya existente) | `{ "status": "success", "audio_path", "voice" }`, `{ "status": "skipped", "audio_path", "voice" }` o `{ "status": "discarded", "voice" }` | `Síntesis es → en · voz <voz> · etiqueta <etiqueta> · local` | `validating`, `translating` (si difieren los idiomas), `synthesizing` (sin `--play` y sin traducción es la única etapa de trabajo y no deja línea propia), `saving` | `✓ Síntesis completada en <total> · <duración> de audio`; `✓ Ya existía; se conserva sin sintetizar de nuevo.`; `✓ Audio descartado; no se ha guardado.` | 0; 1 `synthesis_failed`, `io_error`, `playback_failed` (solo `--play`); 2 `usage_error`, `text_too_long`, `invalid_voice_name`, `invalid_identifier`; 3 `voice_not_found`, `no_output_device` y `audio_backend_unavailable` (solo `--play`); 4 `model_missing`; 6 `label_exists`, `voice_changed`; 9 `translation_failed` |
| `speech say` (vía daemon) | — | `{ "status": "reproduced", "voice", "first_audio_ms", "stalls" }` | `Lectura en voz alta es · voz <voz> · daemon` | `validating`, `translating` (si difieren), `synthesizing`, `playing` | `✓ Reproducción completada en <total> · primer audio <t>` | 0; 1 `synthesis_failed`, `synthesis_timeout`, `daemon_error`, `playback_failed`; 2 `usage_error`, `text_too_long`, `invalid_voice_name`; 3 `voice_not_found`, `no_output_device`, `audio_backend_unavailable`; 4 `model_missing`; 5 `daemon_unreachable`; 9 `translation_failed` |
| `speech say` (local) | — | `{ "status": "reproduced", "voice", "first_audio_ms", "stalls" }` | `Lectura en voz alta es · voz <voz> · local` | `validating`, `translating` (si difieren), `synthesizing`, `playing` | `✓ Reproducción completada en <total> · primer audio <t>` | 0; 1 `synthesis_failed`, `synthesis_timeout`, `playback_failed`; 2 `usage_error`, `text_too_long`, `invalid_voice_name`; 3 `voice_not_found`, `no_output_device`, `audio_backend_unavailable`; 4 `model_missing`; 9 `translation_failed` |
| `speech dub` (vía daemon) | `Transcripción: <texto>` en cuanto se transcribe y, si los idiomas difieren, `Traducción: <texto>` en cuanto se traduce; ambas antes de que empiece a sonar el audio | `{ "status": "dubbed", "text", "translated", "first_audio_ms", "stalls" }` | `Doblaje es → en · voz <voz> · archivo <nombre> · daemon` (o `micrófono (máximo m:ss)`) | `capturing` (solo `--mic`) o `reading`, `denoising`, `transcribing`, `translating` (si difieren), `synthesizing`, `playing` | `✓ Doblaje completado en <total> · primer audio <t>` | 0; 1 `daemon_error`, `synthesis_failed`, `synthesis_timeout`, `playback_failed`; 2 `usage_error`, `empty_transcript`, `invalid_audio`, `audio_too_long`, `empty_audio`, `invalid_voice_name`; 3 `audio_not_found`, `no_input_device` (solo `--mic`), `no_output_device`, `audio_backend_unavailable`, `voice_not_found`; 4 `model_missing`; 5 `daemon_unreachable`; 9 `translation_failed`; 10 `transcription_failed` |
| `speech dub` (local) | `Transcripción: <texto>` y, si los idiomas difieren, `Traducción: <texto>`, cada una en cuanto existe y antes de que empiece a sonar el audio | `{ "status": "dubbed", "text", "translated", "first_audio_ms", "stalls" }` | `Doblaje es → en · voz <voz> · archivo <nombre> · local` (o `micrófono (máximo m:ss)`) | `capturing` (solo `--mic`) o `reading`, `denoising`, `transcribing`, `translating` (si difieren), `synthesizing`, `playing` | `✓ Doblaje completado en <total> · primer audio <t>` | 0; 1 `synthesis_failed`, `synthesis_timeout`, `playback_failed`; 2 `usage_error`, `empty_transcript`, `invalid_audio`, `audio_too_long`, `empty_audio`, `invalid_voice_name`; 3 `audio_not_found`, `no_input_device` (solo `--mic`), `no_output_device`, `audio_backend_unavailable`, `voice_not_found`; 4 `model_missing`; 9 `translation_failed`; 10 `transcription_failed`, `stt_unsupported` |
| `speech play` | — | `{ "status": "played", "label", "voice" }` | `Reproducción del audio sintetizado <etiqueta> · voz <voz>` | `playing` (etapa única: sin línea propia) | `✓ Reproducción completada en <total>` | 0; 1 `playback_failed`; 2 `invalid_voice_name`, `invalid_identifier`; 3 `speech_not_found`, `voice_not_found`, `no_output_device`, `audio_backend_unavailable` |
| `speech remove` | — | `{ "status": "removed"\|"cancelled", "label", "voice" }` | el plan de borrado y la pregunta `¿Borrar este audio sintetizado? [s/N]` (con `--yes` no se pregunta) | — | `✓ Audio sintetizado <etiqueta> de la voz <voz> borrado.`; si se cancela, `Cancelado; no se ha borrado nada.` | 0; 1 `io_error`; 2 `invalid_voice_name`, `invalid_identifier`, `confirmation_required` (sin terminal y sin `--yes`); 3 `speech_not_found`, `voice_not_found` |
| `daemon start` | — | `{ "status": "started"\|"already_running", "daemon": "running", "pid" }` | `Daemon · 127.0.0.1:8765`; el aviso de daemon residual degradado | `stopping` (si hay residual), `starting` (hasta que el proceso publica su dirección y `/health` responde), `warming` (después, hasta que el motor está listo) | con el motor listo, `✓ Daemon listo en <total> · pid N`; si la espera del calentamiento vence, `✓ Daemon en marcha en <total> · pid N · el motor sigue calentándose; ai-voice-interconnector daemon status dirá cuándo está listo`; si el calentamiento falla, `✓ Daemon en marcha en <total> · pid N · falló el calentamiento del motor; consulta ai-voice-interconnector daemon status`; o `✓ El daemon ya estaba en marcha · pid N` | 0; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `port_in_use`; 16 `daemon_stop_failed` |
| `daemon stop` | — | `{ "status", "daemon": "stopped", "was_running", "daemon_fully_stopped" }` | `Daemon · 127.0.0.1:8765` | `stopping` | `✓ Daemon detenido en <total>` o `✓ No había ningún daemon en marcha.` | 0; 5 (veredicto `still_running`, sin objeto `error`) |
| `daemon restart` | — | `{ "status": "restarted", "daemon": "running", "pid" }` | `Daemon · 127.0.0.1:8765` | `stopping`, `starting`, `warming` (como en `daemon start`) | `✓ Daemon reiniciado en <total> · pid N`, con los mismos extras que `daemon start` cuando el motor sigue calentándose o su calentamiento falló | 0; 1 `daemon_error`; 4 `model_missing`; 5 `daemon_unreachable`; 6 `port_in_use`; 16 `daemon_stop_failed` |
| `daemon status` | `Daemon: en marcha (pid N, puerto P, motor listo).`, `Daemon: en marcha, pero no responde: <motivo>.` o `Daemon: detenido.` (el estado del motor puede ser `motor listo`, `motor calentándose`, `falló el calentamiento del motor` o `estado del motor desconocido`) | el reporte de estado | — | — | — | 0; 1 `daemon_error`; 5 `daemon_unreachable` |
| `daemon serve` | — (no acepta `--json`) | — | el log del daemon y sus avisos | — | — | 0; 1 `daemon_error`, `startup_failed` |
| `setup` | — | `{ "status": "completed"\|"cancelled", "models_provisioned" }` | el plan y la pregunta, y el bloque de descarga de modelos (§ «stderr, trazas y logs») | `downloading` | `✓ Setup completado en <total> · N modelos, X descargados`; con los modelos al día, `✓ Los N modelos ya estaban descargados.`; si se cancela, `Cancelado.` | 0; 1 `voice_store_init_failed`, `io_error`, `model_unavailable`; 11 `setup_failed`; 20 `network_error` |
| `cleanup` | con `--dry-run`, el plan (sin etapa de borrado) | `{ "status", "reason", "removed", "planned", "preserved", "unreadable" }` | el plan de borrado y la pregunta (en la ejecución real) y los avisos de ruta no borrada | `cleaning` (solo en la ejecución real; etapa única: sin línea propia) | `✓ Limpieza completada en <total> · N rutas borradas`; `✓ No había nada que limpiar.` (sin confirmación); con `--dry-run`, `Simulación: no se ha modificado nada.`; si se cancela, `Cancelado; no se ha borrado nada.` | 0; 1 `io_error`; 2 `usage_error`, `confirmation_required`; 16 `daemon_stop_failed` |
| `self install` | — | `{ "status", "reason", "install_dir", "version", "channel", "path_integrated", "models" }` | el plan y la pregunta (`¿Instalar el programa?` o `¿Reparar la instalación?`) | `stopping`, `replacing` o `copying` (instalación nueva), `path`, `receipt` | `✓ ai-voice-interconnector <versión> instalado en <total>` o `reparado en <total>`, y debajo los detalles atenuados `PATH: …` y `Modelos: …` | 0; 1 `self_install_failed`, `io_error`; 2 `usage_error`, `confirmation_required`; 11 `setup_failed` (causa en `models_cause`: `network_error` o `model_unavailable`); 13 `rolled_back`; 14 `path_conflict`; 15 `bundle_invalid`; 16 `daemon_stop_failed`; 17 `lifecycle_locked`; 18 `unsupported_platform`; 19 `binary_incompatible`; 20 `network_error`; 21 `checksum_mismatch` |
| `self update` | con `--check`, `Hay una versión nueva: A → B.` o `Ya tienes la última versión (V).` (también al estar al día sin `--check`) | `{ "status", "reason", "previous_version", "version", "channel" }` | `Actualización desde la versión A`; los avisos de la recuperación | `checking`, `downloading`, `verifying`, `stopping`, `replacing` | `✓ Actualizado de A a B en <total>` | 0; 1 `self_update_failed`, `release_unavailable`, `io_error`; 2 `usage_error`, `confirmation_required`; 3 `not_installed`; 11 `setup_failed` (causa en `models_cause`: `network_error` o `model_unavailable`); 12 `externally_managed`; 13 `rolled_back`; 16 `daemon_stop_failed`; 17 `lifecycle_locked`; 18 `unsupported_platform`; 19 `binary_incompatible`; 20 `network_error`; 21 `checksum_mismatch` |
| `self uninstall` | con `--dry-run`, el plan etiquetado (`  <categoría>: <ruta> (<tamaño>)` y `  PATH: se quitará la entrada <ruta> del PATH de tu usuario.`) | `{ "status", "reason", "removed", "planned", "preserved", "path_reverted" }` | el plan de borrado y la pregunta `¿Desinstalar el programa y borrar lo indicado?` | `stopping`, `state`, `path`, `program`, `locks` | `✓ Desinstalación completada en <total>`; `✓ No había nada que desinstalar.`; con `--dry-run`, `Simulación: no se ha modificado nada.`; con restos, el `Error:` de `program_dir_kept` con su `Solución:` | 0; 1 `self_uninstall_failed`, `io_error`; 2 `usage_error`, `confirmation_required`; 17 `lifecycle_locked`; 22 `program_dir_kept` (veredicto, sin objeto `error`) |
| `doctor` | `Diagnóstico:` y todas las comprobaciones, una por línea (`  ✓ <etiqueta>: <detalle>` o `  ✗ <etiqueta>: <detalle> → <indicación>`) | el reporte (`checks`, `failed`, …) | con comprobaciones fallidas, el `Error:` de `doctor_failed` (`N comprobaciones fallan.`) sin `Solución:` | `repairing` (solo con `--repair`; etapa única: sin línea propia) | `✓ Diagnóstico sin problemas.`; con `--repair`, `✓ Reparación completada en <total>` | 0; 1 `doctor_failed`; 1 (veredicto, sin objeto `error`) cuando hay comprobaciones fallidas |
| `devices` | `Salidas de audio:`, una línea `  [<id>] <nombre> · latencia <x,y> ms` por dispositivo (con ` · predeterminada` en la salida por defecto) y `Entrada predeterminada: <nombre>` (o `ninguna`) | `{ "devices": [...], "default_input", "default_output" }` | — | — | — | 0; 1 `audio_enumeration_failed`; 3 `audio_backend_unavailable` |
| `--version` | «ai-voice-interconnector X.Y.Z» | `{ "name", "version" }` | — | — | — | 0 |
| `--help` (o raíz sin subcomando) | la ayuda | `{ "help" }` | — | — | — | 0; 2 `usage_error` (grupo sin subcomando) |

**Lo que la tabla no cubre, y por qué.** La tabla fija el contrato de **éxito y de error por `reason`**, que es lo que un consumidor programado lee. No fija el texto literal de los mensajes de error —son mensajes humanos y cambian sin romper el contrato—, ni el orden de las líneas de stderr, que es propio de cada comando. La enumeración completa de los `reason` y su código está en §9.

**El indicador y el envelope.** Un comando con indicador escribe por stderr cuatro clases de línea: el contexto, las etapas (activa y terminadas), los avisos y el cierre. Un latido del daemon no abre línea nueva: actualiza la etapa en curso. El resultado del comando —el texto o el envelope— va por stdout y el cierre por stderr; ambos los dice su dueño (el handler, o el flujo de la librería en `install`, `uninstall` y `cleanup`) **una sola vez**, con la duración de la operación.

**Envelope ya emitido.** Cuando un comando termina con fallo después de haber escrito su envelope o su JSONL —`doctor` con comprobaciones fallidas, `daemon stop` fallido, `self uninstall` con restos y un lote de `speech denoise` con fallos— no escribe un segundo documento: con `--json` sale solo con el código; en texto escribe el `Error:` correspondiente (con su `Solución:` si el `reason` la tiene; `daemon stop` fallido y `doctor_failed` no la tienen) y sale con el mismo código.

**La paridad de fases.** Las dos vías de un comando nombran las mismas fases cuando la entrada es equivalente. La paridad está acotada por lo que el daemon puede anunciar: su wire expone un conjunto de etapas, y la que la vía local nombra y el daemon no anuncia —el guardado de la voz clonada, que el daemon persiste sin evento de guardado— queda como única diferencia. La traducción de `synthesize`, `say` y `dub` aparece en las dos vías cuando los idiomas difieren, con la misma segmentación (oraciones y párrafos, por lotes), de modo que la misma entrada se traduce igual por ambas. La limpieza de ruido de `dub` (`denoising`) también se nombra en las dos. Las fases que la CLI nombra en las dos vías —la captura y la lectura del audio, que ocurren antes del despacho; el guardado del audio sintetizado y la reproducción, que hace la CLI— se nombran igual en ambas. Los textos de cada etapa son idénticos en las dos vías, de un catálogo único del cliente, y la línea de contexto solo difiere en la ruta (`local` o `daemon`). En terminal el tiempo de la etapa avanza también durante el trabajo bloqueante de la vía directa, la grabación y la reproducción. El mapa de etapas está en [`commands/DAEMON.md`](commands/DAEMON.md).

**Grabación con micrófono.** Con terminal, la etapa `capturing` muestra `● Grabando m:ss / m:ss  <medidor>` —contador, tope y un medidor de nivel de los últimos 5 sondeos— y, si la grabación se para con Enter, `Enter para detener`; sin terminal escribe una sola vez `Grabando (máximo m:ss)…`. Al llegar al tope avisa `Aviso: Se alcanzó el límite de grabación (m:ss); se detiene.` y, si el tramo de 100 ms más fuerte no supera −43 dBFS, `Aviso: No se detectó voz en la grabación (nivel máximo N dBFS); comprueba que el micrófono no esté silenciado.`; ninguno de los dos avisos corta el comando. La etapa termina como `Audio grabado` con su duración.


### Los payloads del grupo `speech`

La forma de cada payload está en la [referencia de esquemas `--json`](../../USAGE.md#referencia-de-esquemas---json) y en [`commands/SPEECH.md`](commands/SPEECH.md). Las reglas que fija el contrato:

- Solo `synthesize` emite ruta (`audio_path`), por el criterio de la ruta en los payloads: produce el WAV y no hay comando de exportación. Es el único cuyo WAV sobrevive a la invocación; `label` va implícito en `audio_path`. Su `status` es de vocabulario cerrado y uno por desenlace (`SynthesizeOutcome` en `src/main.rs`): `success` si guardó, `skipped` si `--skip-existing` conservó el audio sintetizado existente —con la ruta de ese audio en `audio_path`— y `discarded` si se descartó en el bucle `--play`. Bajo `--json` el bucle es inalcanzable y, cuando la salida es 0, el audio sintetizado está en el almacén.
- **`say`** no persiste y no emite ruta: el audio no pasa por ningún archivo, suena según llega (entrega en streaming); no repite `text` ni informa `audio_path`, y sí informa `first_audio_ms` y `stalls`, los dos datos de la entrega. `first_audio_ms` cuenta desde que se abre la etapa de síntesis hasta la primera muestra que suena, así que incluye la espera de turno, el calentamiento del motor y la carga de la voz; la etapa `Primer audio` y el extra `primer audio` del cierre parten de la misma referencia y pueden diferir unos milisegundos, porque la etapa se cierra en el fragmento siguiente al primero que suena. El texto humano (`✓ Reproducción completada en …`) tampoco muestra ruta. `say` es humo de punta a punta: afirma que sintetizó y reprodujo, sin medir la calidad del audio; la paridad de inteligibilidad la cubre `synthesize`, que persiste y verifica.
- **`dub`** tampoco persiste ni emite ruta: el audio doblado suena según llega, sin `audio_path`, e informa `first_audio_ms` y `stalls` junto a `text` y `translated`. Quien quiera conservar el audio usa `speech synthesize`. Como `say`, es humo sin puerta de calidad sobre lo que sonó.
- **`synthesize` y `say` comparten `voice`** porque si no se pasó `--voice` lo eligió el sistema (`default`); `dub` no lo emite: su consumidor necesita los textos, así que emite `text` (la transcripción del audio de entrada) y `translated` (el texto sintetizado, siempre presente; coincide con `text` cuando el idioma de origen y el de destino son el mismo). El modo texto de `dub` imprime por stdout `Transcripción: <texto>` en cuanto termina de transcribir y, solo si los idiomas difieren, `Traducción: <texto>` en cuanto termina de traducir; ambas salen antes de que empiece a sonar el audio, y el cierre (`✓ Doblaje completado en …`) va por stderr. Los nombres `text` y `translated` son los del evento `result` del stream NDJSON de `/dub`; en la vía daemon la CLI falla con `daemon_error` si a ese evento le falta cualquiera de los dos, sin valores por defecto.
- **`list`** emite el texto completo. La clave es el nombre del recurso en snake_case, siguiendo el precedente de `voice list --json` (una lista bajo el nombre del recurso) y evitando que un identificador del contrato legible por máquina contradiga el vocabulario de la superficie.
- **`remove`** no lleva campo de resultado: el código de salida ya transporta la información (0 = se borró, 3 = no existía). Un campo `removed` chocaría además con `cleanup --json`, que emite `removed` como lista de rutas, y la misma clave con dos tipos bajo una sola versión de esquema es justo lo que un consumidor tipado no puede manejar.

Los payloads de `daemon start` y `restart` no llevan clave booleana propia: el fallo se reporta por el payload de error como en el resto de la CLI. `stop` sí bindea booleanos, porque son el veredicto del motor y no un eco de la entrada. `start` distingue sano de degradado sin bump: sano responde `already_running` y degradado reclama el árbol y rearranca con `started`, ambos con salida 0. `stop` bindea el veredicto del motor (`StopOutcome`, calculado antes de tocar nada): con la muerte verificada responde `status` `not_running` (sin daemon previo) o `shutdown_sent`, con `daemon: "stopped"`, `was_running` y `daemon_fully_stopped` y exit 0; si el árbol o el residente siguen vivos sale por veredicto con `status` `still_running` y exit 5, sin objeto `error` y sin borrar la pista. Un arranque fallido de `start` o `restart` no deja proceso, pidfile ni fichero ready. El readiness lo transporta el fichero de `--ready-file` (escritura atómica), con registro de éxito (`addr`) o de fallo (nunca `addr`). El envelope de `stop`, la búsqueda del daemon, el reclamo, el arranque, la dirección y los `reason` de fallo con su exit están en [`commands/DAEMON.md`](commands/DAEMON.md#gestión-del-ciclo-de-vida) y [`commands/DAEMON.md`](commands/DAEMON.md#códigos-de-error).

**`daemon.pid` y contabilidad del residente:** `data_dir()/daemon.pid` lo escribe solo el daemon, completo y de forma atómica, tras enlazar el puerto y antes de publicar `daemon.ready`; la CLI solo lo lee y lo borra tras una parada verificada. Un PID registrado cuenta como vivo y propio solo si la identidad observada del proceso coincide con la registrada; si no coincide o no se puede observar, es un proceso ajeno: se trata como muerto y nunca se mata. Un fichero sin las claves de identidad es ilegible: se descarta con aviso y se propaga como ausencia, sin degradar a «PID solo». El residente TTS se registra al lanzarlo y se liquida por su PID registrado e identidad, nunca por imagen ni sondeando puertos; la vía directa (`--no-daemon`) no registra nada ni toca `daemon.pid`. Con `AVI_DATA_DIR` el pidfile es por instancia. Las claves del esquema y el resto del registro están en [`commands/DAEMON.md`](commands/DAEMON.md#esquema-de-daemonpid). Las pruebas del harness operan con aislamiento total por instancia (`IsolatedInstance`) y semáforo de capacidad para inferencia (file-lock exclusivo del SO sobre `target/tts-serial.lock`, tomado en `lock_tts()`).

### Las dos versiones de esquema

Son **dos, independientes**: el envelope de la CLI va por `"6"` y el protocolo del daemon va por `"7"`, y cada uno cubre su propia superficie.

- **`crates/avi-daemon/src/lib.rs` (`DaemonState`, `run_daemon_server`) — protocolo IPC del daemon, `DAEMON_SCHEMA_VERSION = "7"` (`crates/avi-core/src/json_emitter.rs`).** `/synthesize` identifica la voz por su nombre, acepta un `seed` opcional (entero entre 0 e `i32::MAX`; fuera de rango o no entero es `usage_error` con 400), entrega el audio en eventos `audio` (`pcm_b64`: PCM de 16 bits, 24 kHz, mono) y devuelve en `result` el texto realmente sintetizado (`translated`, igual al de la petición cuando los idiomas coinciden) y `work_ms`, sin ruta de destino; el paso a esta versión retiró `audio_b64` del `result` de `/synthesize` y de `/dub`, un cambio incompatible de una clave existente; `/voices/clone` recibe bytes e identificador y devuelve `speech`, la ruta calculada por el daemon en su filesystem; `/health` informa `warm` (`warming`/`warm`/`warm_failed`, con `warm_error` si falló) y, cuando están residentes, `stt` y `ct2`; `/transcribe` responde 400 (`usage_error`, `invalid_audio`, `empty_transcript`) y 500 ante errores. Los streams NDJSON de `/synthesize`, `/dub` y `/voices/clone` emiten `started`, `heartbeat` y `progress` sin texto humano —el paso a esta versión retiró las claves `message`, `percent` y `operation`— y dejan los textos al cliente. `/dub` emite además, sin cambio de versión por ser eventos nuevos que el cliente desconocido ignora, `transcript` (justo tras la transcripción) y `translation` (tras la traducción, solo si los idiomas difieren), cada uno con exactamente las claves `event`, `text` y `schema_version`: `text` es contenido del usuario, no texto de interfaz, y el cliente lo muestra en cuanto llega, antes de que suene el audio. El evento `result` no cambia. Los valores de `stage` (como `loading_voice`) tampoco mueven la versión: `stage` es un valor, no una clave, y el cliente ignora los valores que no conoce. Una limpieza de ruido sin audio (`ENoAudio`) responde `invalid_audio` con HTTP 400, y un archivo de salida existente responde `output_exists` con HTTP 400; `model_missing` sigue siendo 404 y el resto de fallos de limpieza 500. **El ciclo de vida no lo toca**: ni `self install`, ni `self uninstall`, ni `cleanup`, ni la sección de ciclo de vida de `doctor` cambian una clave del protocolo del daemon, así que su versión no se mueve.
- **`src/main.rs` / `crates/avi-core/src/json_emitter.rs` (`CLI_SCHEMA_VERSION = "6"`) — payloads `--json` de la CLI.** El payload de síntesis lleva `audio_path`, la ruta del WAV en el almacén, y no existe ninguna ruta de salida elegida por quien invoca; `doctor --json` expone la sección de ciclo de vida (`version`, `target`, `channel`, `install`, `path`, `pending`, `models`), con `checks` y `failed` como veredicto; los simulacros de `self uninstall` y `cleanup` informan `status` `planned` con su recibo `planned` sin `dry_run`, y el envelope de ambos comandos —simulacro y ejecución— lleva `preserved`, array de objetos `{"path", "reason"}` con lo que el plan conserva a propósito, siempre presente y por operación (`[commands/CLEANUP.md](commands/CLEANUP.md)` y `self uninstall` en `commands/SELF.md`); `daemon stop` bindea `was_running` y `daemon_fully_stopped` y su fallo sale por veredicto con `status` `still_running`; los fallos de parseo con `--json` salen como envelope con `reason` `usage_error` por stdout; `say`/`dub` no emiten `audio_path`; y `dub` emite `text` (la transcripción) y `translated` (el texto sintetizado), de modo que `text` dejó de ser el texto ya traducido, cambio incompatible que motivó el paso a `"6"`.

**La política de compatibilidad es la misma en ambas**: añadir claves no incrementa la versión; solo lo hace un cambio incompatible de las existentes. Y el **número de versión no es comparable entre las dos**: que cada una haya subido por motivos distintos y en momentos distintos lo muestra: son dos contratos que suben por separado, y un cambio incompatible en uno no mueve la versión del otro; cuando coinciden en valor es una coincidencia, no un acoplamiento. `preserved` en los envelopes de `cleanup` y `self uninstall` es el ejemplo vivo: clave **aditiva**, siempre presente como array de `{path, reason}`, añadida sin mover la versión del esquema CLI vigente en ese momento.

## 11. `self`, `setup`, `cleanup` y `voice`

### `self install`, `self update` y `self uninstall`

`self` es el grupo que hace que **el ciclo de vida viva en el binario que se gestiona a sí mismo**. Los tres subcomandos comparten cuatro propiedades, y las cuatro importan más que sus flags:

- **Actúan sobre la instalación registrada, no sobre la copia que se invoca.** Las raíces efectivas salen del recibo (`cleanup::Roots::from_receipt`), de modo que la operación acierta aunque `AVI_DATA_DIR` o `AVI_CACHE_DIR` ya no estén definidas ([recibo de instalación](../specs/LIFECYCLE.md#71-recibo)). `AVI_CACHE_DIR` designa el **contenedor** de caché, no la raíz de modelos: los modelos son `$AVI_CACHE_DIR/models` y los temporales del producto `$AVI_CACHE_DIR/tmp`; el recibo registra ese contenedor en `roots.cache_dir`.
- **Toman el bloqueo de SO en modo exclusivo**, sobre el archivo de bloqueo y su compuerta de traspaso (hermanos del directorio de programa registrado, el mismo para todas las operaciones), y por eso un segundo proceso concurrente sale con `lifecycle_locked` (17) en vez de esperar. Los comandos de uso (`translate`, `voice`, `speech`, `setup`, `daemon start`, `daemon restart` y `daemon serve`) lo toman en modo compartido y lo retienen toda su vida, salvo `daemon serve`, que lo suelta al quedar registrado en `daemon.pid`: coexisten entre sí y, mientras hay una operación en curso, terminan con `lifecycle_locked` (17) sin ejecutarse. No lo toman `daemon status`, `daemon stop`, `doctor` sin `--repair` ni las simulaciones `--dry-run`. `self update` retiene la compuerta durante todo el traspaso al binario nuevo, y `self install` retiene el bloqueo hasta terminar su provisión de modelos.
- **Ejecutan la recuperación antes de componer el plan**, para que el plan que el usuario ve y confirma sea el que queda después del barrido por identidad de los residuos de dueño muerto (aparcados, stagings de instalación, scratch, temporales con token y registros de borrado resueltos); los de dueño no observable se conservan y se informan.
- **El binario aporta lo que el motor no puede tener**: el control de procesos (parada del daemon) y el borrado diferido de Windows. El motor decide; el binario ejecuta esas dos primitivas de plataforma (`ProductProcesses` y `PlatformRemoval`, esta última sobre `avi_process::schedule_removal`). El borrado tiene un solo mecanismo (`avi_lifecycle::removal::PathRemover`) y dos políticas del motor con nombre: `retire` para la basura del ciclo —el staging y el aparcado `.old-*`, que se retira como unidad— y `remove_program_dir` para el programa en uso.

| Sub-acción | Qué hace | `reason` de éxito parcial o de fallo |
|---|---|---|
| `self install` | Instala el bundle del que forma parte el ejecutable, o **repara** la instalación si se ejecuta desde ella; aplica al final la misma provisión que `setup` salvo `--no-setup` | `setup_failed` (11) con el programa instalado |
| `self update` | Actualiza la instalación registrada a la última estable o a `--version X.Y.Z`, con verificación y traspaso al binario nuevo; `--check` solo informa | `already_up_to_date` (0) sin descargar; `not_installed` (3) sin instalación; `externally_managed` (12) en `homebrew`; `setup_failed` (11) parcial, con causa original en `models_cause` (`network_error` o `model_unavailable`); `release_unavailable` (1) si la versión o el asset no existe; `io_error` (1) si falla el disco local; `unsupported_platform` (18), `binary_incompatible` (19), `network_error` (20) si falla el transporte, `checksum_mismatch` (21) |
| `self uninstall` | Borra el estado, revierte el `PATH` según el recibo y borra el directorio de programa aplicando R2 | `program_dir_kept` (22) si el directorio de programa no se pudo borrar ni programar su borrado |

**`self uninstall` sin `--keep-data` borra la raíz de datos entera, no el plan de `cleanup --all`.** La diferencia es deliberada y está en `uninstall::compose_plan`: Con recibo de instalación, `cleanup --all` protege la voz de fábrica (`default`) porque va embebida en el binario y el programa sigue instalado, así que `setup` la vuelve a materializar; sin recibo, `cleanup --all` deja el estado en cero igual que `self uninstall`. Al desinstalar **el programa desaparece**, y con él la voz de fábrica: dejarla sería residuo dentro de una raíz de propiedad exclusiva. Con `--keep-data` sí se aplica el plan de `cleanup --all` **filtrado** —modelos, voces y audios sintetizados quedan fuera— y el directorio de programa se borra igualmente.

**Idempotencia**: sin instalación ni estado, `self uninstall` termina con éxito y `status` `not_installed`. En Windows, si el ejecutable en uso está dentro del directorio de programa, el borrado se programa para cuando el proceso termine y el `status` es `removal_scheduled`, que también es éxito porque el borrado diferido quedó programado (sin registro si va bien; si falla deja `<destino>.removal-failed.log` junto al destino). Si no se puede programar, el `status` es `uninstalled` con `reason` `program_dir_kept` y salida 22.

El detalle completo —los pasos de `self install` y de `self update` con traspaso y borrado diferido, el bundle y su manifiesto, el recibo y la integración de `PATH`— está en [`commands/SELF.md`](commands/SELF.md) y, como especificación, en [`LIFECYCLE.md`](../specs/LIFECYCLE.md#83-self-install).

### `cleanup`

| Flag | Qué borra / efecto |
|---|---|
| `--voices` | Las voces que puede borrar y, **con ellas, solo los namespaces de audios sintetizados de esas voces** (arrastre `speech/<voz>` excepto `default`) |
| `--synthetic-speech` | La raíz `speech/` entera, `default` incluida |
| `--model` | En la raíz de modelos **exclusiva** de la aplicación, la **raíz entera** (snapshots, locks y `xet` cuelgan de ella). En la raíz **compartida** que el usuario eligió con `HF_HUB_CACHE`/`HF_HOME`, solo los repos fijados (`MODEL_REVISIONS`) y sus locks; **`xet` y el `.locks` completo nunca se borran** (regla R3) |
| `--all` | Unión de las tres categorías **más** configuración (`config.json`), logs, el estado de ejecución del daemon (`daemon.pid` y `daemon.ready`) y la hoja de temporales (`scratch`), con la poda de los contenedores que queden vacíos (**sin programa ni PATH**) |
| `--dry-run` | Plan puro (exit 0): lista sin borrar, sin tomar el bloqueo y sin pedir confirmación; con `--json` emite `status: "planned"` con `removed` y su recibo `planned`, sin `dry_run` |
| `--yes` / `-y` | Omite la confirmación interactiva (`s/si/sí/y/yes/dale/ok`); con `--dry-run` es no-op |

**Gate `sin flags → exit 2`:** `cleanup` sin ningún flag de categoría (`--voices`, `--synthetic-speech`, `--model`, `--all`) sale con `2` `usage_error` **sin borrar nada y sin tomar el bloqueo** (`cleanup::run`, primera comprobación). `--all` equivale a `--voices --synthetic-speech --model` más configuración, logs y estado del daemon.

**Sin terminal, `cleanup` no procede por su cuenta.** Es una operación destructiva: sin TTY y sin `--yes` termina con `confirmation_required` (2) y no borra nada. La celda no destructiva de la [tabla de reglas transversales](../specs/LIFECYCLE.md#81-reglas-transversales) —lo no destructivo procede sin preguntar— es la de `self install` y `setup`, no la de `cleanup`.

**El plan es una función pura de sus entradas** (`cleanup::plan`): la usan la confirmación, el `--dry-run` y la ejecución, de modo que no puede haber divergencia entre lo que se anuncia y lo que ocurre. Antes de borrar recursos que el daemon usa, `cleanup` lo detiene; si no se detiene, `daemon_stop_failed` (16) y **nada del plan se borra**.

**`speech/default/` sobrevive a `--voices` y cae únicamente con `--synthetic-speech` o `--all`.** El criterio es el del propio flag —los audios sintetizados se van con su voz— y la voz de fábrica (`default`; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) no se va nunca: `voice remove` las protege (exit 2) y `--voices` no las borra. Importa declararlo porque `default` es la voz por defecto de `speech synthesize` y su namespace es probablemente el más poblado. Reparto con `speech remove`: el borrado individual es `speech remove --label`, el masivo es `cleanup --synthetic-speech` (ver «Reparto con `cleanup`» en [§3](#3-el-grupo-speech)).

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

**Su flujo está en el motor**: selección de modelos, idempotencia por presencia del snapshot, purga de `--force-update` **sobre la misma selección** y validación de cada modelo por presencia de sus ficheros. El orden es plan → **una sola confirmación** → purga → descarga → poda. Con `--force-update` la confirmación es destructiva (su resumen anuncia «Se purgarán los modelos descargados y se volverán a descargar.» y el tamaño de la selección completa) y sin terminal y sin `--yes` es un error, como en `cleanup`; sin él, la confirmación es no destructiva: solo informa el tamaño, se omite si no hay nada que descargar o con `--yes`, y sin terminal procede. Si el usuario cancela, no se purga ni se descarga nada: el texto es «Cancelado.» y `--json` emite `{ "status": "cancelled", "models_provisioned": [] }` con exit 0.

**Poda**: la selección es siempre la tabla de pines completa (el modelo Base de clonado incluido) y no se persiste; tras el reemplazo el `setup` provisiona el pin nuevo y poda las revisiones propias obsoletas, con R3 en raíz compartida (`crates/avi-lifecycle/src/setup.rs`).

**`setup` invocado al final de una instalación no es un error de la instalación.** `self install` aplica en el mismo proceso la misma provisión que `setup` (descarga de la selección, y `doctor` exige después los modelos de la selección); si falla, el programa **queda instalado** y el desenlace es `setup_failed` (11), con el motivo del fallo de provisión anidado en `models_cause`. En `self update`, el padre valida y conserva `models_cause.reason`/`message` emitidos por el hijo: `reason: setup_failed` describe el resultado parcial, no la causa concreta. Ver `self install` en [`commands/SELF.md`](commands/SELF.md) y §10.


### `voice`

- **`voice clone` toma una fuente exclusiva**: `--speech-reference/-s <archivo WAV>` o `--mic`. El micrófono admite `--duration` de 1 a 30 segundos; sin duración requiere TTY y usa push-to-talk con límite de 30 s si `AVI_PUSH_TO_TALK_MAX_SECS` está ausente. Un valor explícito se valida antes de abrir el dispositivo: si es inválido sale con `usage_error` y si supera 30 s con `audio_too_long` (exit 2). Duración 0 o falta de TTY sale con exit 2 `usage_error`; `--duration` mayor de 30 s sale con exit 2 `audio_too_long`. `--duration` con archivo y la falta/duplicación de fuentes salen con exit 2 `usage_error`. El WAV de archivo permanece intacto en CLI; el mic se codifica a WAV mono int16 (48 kHz si `--denoise` está activo, 24 kHz en caso contrario) y una captura que supere 30 segundos se recorta a ese límite, sin rellenar capturas cortas. Ambos producen `reference.qvoice`, sin almacenar el WAV.
- **`voice clone` guarda solo la huella de timbre (x-vector) de la referencia**: no recibe transcripción ni idioma, no depende del STT y funciona igual con o sin la feature `native-stt`. Las voces clonadas se sintetizan con el modelo Base (`qwen3-tts-0.6b-base`, que `setup` provisiona siempre); en una instalación incompleta, su ausencia sale con `model_missing` (exit 4) tanto al clonar como al sintetizar.
- **`voice clone` recibe el despacho al daemon en sus tres modos**, porque el clonado ejecuta el modelo Base (y, con el daemon, precalienta la voz nueva) y necesita un modelo cargado igual que las dos sub-acciones que sintetizan.
- **`voice clone` sobre un nombre tomado sin `--force` sale con 6**: la CLI comprueba `VoiceStore::exists` antes de capturar el micrófono o leer la referencia, en las dos rutas, porque la CLI y el daemon comparten el directorio de datos; el daemon conserva su propia comprobación como defensa. `Auto` resuelve el destino una vez y sigue el mismo contrato de la ruta elegida.
- **`voice clone --force` reemplaza la voz y sus audios sintetizados**, que se grabaron con el contenido anterior: el envelope los informa en `removed_speech`. Si la voz tiene audios sintetizados, la CLI pide la misma confirmación que `voice remove` antes de capturar o clonar, en las dos rutas (sin terminal exige `--yes` y, sin él, sale con exit 2 `confirmation_required` sin cambiar nada; cancelar sale con 0 y `status: "cancelled"` en `--json`). El daemon no confirma: lo resuelve la CLI. `--denoise` se ejecuta en la CLI para destino local y en el servidor para destino daemon, una sola vez.
- `VoiceStore` (`crates/avi-store/src/lib.rs`) reconoce una voz clonada únicamente por la presencia de `reference.qvoice`, sin fallback a WAV. La voz de fábrica es `default` (clonada, con `reference.qvoice`); `voice remove` la protege (exit 2; `FACTORY_VOICES` del almacén de voces, `avi_store::is_factory_name` definida en el almacén de voces, invocada en el binario principal).
- `voice list` muestra las tres de fábrica (`is_factory=true`) más las clonadas del usuario; `voice remove` rechaza las de fábrica con exit 2. `voice remove` de una voz del usuario pregunta siempre, tenga o no audios sintetizados, y si los tiene los borra con ella: muestra el plan y exige confirmación como `cleanup` (con terminal pregunta `[s/N]`; sin terminal exige `--yes` y, sin él, sale con exit 2 `confirmation_required` sin borrar nada; cancelar sale con 0 y `status: "cancelled"` en `--json`); el envelope de éxito lleva `removed_speech` con las etiquetas arrastradas.
- En `voice list` y `voice remove`, `-n` es `--name`; `voice remove` y `voice clone` añaden `--yes/-y` para omitir la confirmación del arrastre.

## 12. Contratos externos

### El integrador de narración

**`speech say --text "<msg>" --daemon`** es la invocación que sintetiza y reproduce, y es el contrato del integrador de narración: exige el daemon porque su presupuesto de latencia no admite cargar el modelo al vuelo. No hay alias de compatibilidad; esa es la única forma de la invocación.

El integrador que quiera además conservar el audio usa `speech synthesize --text "<msg>" --label L`, que no reproduce.

El integrador es **EvolutiveX-Agents**, un plugin de [Claude Code](https://code.claude.com) que narra por voz la actividad de la sesión: al final de cada turno y en avisos relevantes el usuario escucha un audio sintetizado corto en español. Se dispara por hooks (`Stop`, `Notification`; `SessionStart` verifica el entorno y deja el daemon caliente), nunca bloquea ni retrasa el turno y falla en silencio si el motor no está disponible. El resto de sus propiedades de diseño vive en su repositorio, que es su fuente de verdad; su contraparte de este contrato está en su [documento de integración](https://github.com/CristianRojas-SoftwareEngineer/EvolutiveX-Agents/blob/main/docs/INTEGRATION.md).

**Dependencia unidireccional.** El plugin consume a AI Voice-InterConnector; este repositorio no conoce, importa ni depende del plugin, y ningún código, test ni build de aquí sabe de su existencia. A efectos del motor, el plugin es un consumidor externo más de la CLI pública (`ai-voice-interconnector` en `PATH`), como un script de usuario.

**Superficies que consume y su compromiso de estabilidad.** El único acoplamiento es la CLI pública:

| Superficie | Qué consume el plugin | Compromiso de estabilidad |
|------------|-----------------------|----------------------------|
| `speech say --text "<msg>" --daemon` | Síntesis dinámica y reproducción de cada audio sintetizado. Usa el daemon y falla si no está levantado (no lo arranca solo). | Mantener el flag `--daemon` y su semántica (usar el daemon, no auto-arrancarlo). |
| `speech synthesize --text "<aviso>" --label <label> --skip-existing` | Pre-síntesis idempotente de avisos; se guarda en el almacén de audios sintetizados bajo un `label`, y un aviso ya guardado sale con 0 y `status` `skipped`. | Mantener los flags `--label`, `--skip-existing` y `--force`, y el `status` `skipped`. |
| `speech play --label <label>` | Reproducción de un audio sintetizado pre-cacheado por `label`. | Mantener el subcomando `play` y el flag `--label`; exit `3`/`NotFound` si el label no existe. |
| `doctor --json` | Diagnóstico integral: lee `checks` (una entrada `{name, ok, detail}` por chequeo) y `failed` (los `name` que fallan: el entorno está sano si `failed` está vacío). En `unmanaged`, la ausencia de receipt y de resolución PATH también falla; no es una gate parcial de recursos. Ver [`doctor`](commands/DOCTOR.md). | Mantener `checks`/`failed` integrales y separar gates específicas de bootstrap. |
| `daemon status --json` | Lee `daemon` (`"running"`/`"stopped"`) para saber si el daemon corre: `running` solo con un daemon propio en servicio, el mismo criterio con el que `daemon start` decide si arranca. `pid` (en servicio) y `degraded` (restos) son aditivos. Ver [`daemon`](commands/DAEMON.md). | Mantener la clave `daemon` con esos dos valores. |
| `daemon start` | Levanta el daemon para dejar los modelos en memoria. | Mantener el subcomando y su arranque desanclable. |

Además, el plugin interpreta dos exit codes semánticos (definidos en `crates/avi-core/src/exit_codes.rs`) para decidir sin parsear stderr:

- exit `5` = `DaemonUnreachable`: el daemon está caído o inalcanzable; el plugin lo usa para saber que no responde.
- exit `6` = `StateConflict` en `speech synthesize`: **siempre es un fallo** y no se guardó nada, sea `"label_exists"` (un `--label` ya existe y no se pidió `--force` ni `--skip-existing`) o `"voice_changed"` (la voz se re-clonó o se eliminó mientras se sintetizaba). Para no re-sintetizar un aviso ya guardado, el plugin pasa `--skip-existing` y recibe exit 0 con `status` `skipped`: el código de salida basta por sí solo para saber si hubo éxito, y el `reason` solo refina.

El plugin decide por el código de salida y trata como ausente un `reason` que no conoce: añadir un `reason` no sube `schema_version`. La regla está en [USAGE](../../USAGE.md#comandos).

Cambiar cualquiera de estos nombres, flags, campos o exit codes rompe la narración sin que este repositorio tenga tests que lo detecten, porque el plugin vive fuera. Por eso esta tabla es el contrato a preservar: al tocar `src/main.rs` en `speech say`, `speech synthesize`, `speech play`, `doctor` o `daemon`, o `crates/avi-core/src/exit_codes.rs`, hay que revisarla. `speech transcribe` y `speech dub` existen en la CLI pero el plugin no los consume; no forman parte de este contrato.

**Qué no comparten los dos proyectos.** Todo lo que no es el contrato público de la CLI es disjunto, y por eso el plugin vive en su propio repositorio (el modelo de distribución de plugins por marketplace también asume un repo git propio):

- **Código**: el plugin es TypeScript sobre el Node.js que trae Claude Code; no depende del árbol Rust.
- **Versionado**: este repositorio versiona el motor (binarios por SO); el plugin versiona con el campo `version` de `plugin.json`, al ritmo de Claude Code. Un fix en uno no obliga a un release del otro.
- **CI e infraestructura**: aquí `cargo test`/build y publicaciones por tag; allá toolchain TypeScript y `claude plugin validate`.

**Instalación.** Desde el lado del motor no hay nada que instalar para el plugin: basta con que `ai-voice-interconnector` esté en el `PATH` y los modelos estén en caché (`ai-voice-interconnector setup`). El repositorio del plugin dobla como su propio marketplace:

```
/plugin marketplace add CristianRojas-SoftwareEngineer/EvolutiveX-Agents
/plugin install evolutivex-agents@EvolutiveX-Agents
/evolutivex-agents:install
```

`/evolutivex-agents:install` guía la instalación del binario, la descarga de los modelos y la activación de la narración.

### La frontera del daemon

`/synthesize` recibe `voice: str` y devuelve el audio sintetizado sin ruta de destino en la petición. No hay lista de directorios de audio permitidos, ni validación de destinos de audio, ni directorio de sesión del daemon, porque la petición no proporciona un destino de archivo. `/voices/clone` tampoco recibe la ruta del WAV de entrada; su evento `result` sí emite `speech`, la ruta de `reference.qvoice` calculada al guardar la voz en el filesystem del daemon. La CLI propaga ese dato sin abrir ni escribir el fichero; la ruta solo es utilizable para leerlo si el cliente tiene acceso a ese filesystem.

**Riesgo conocido y declarado**: `data_dir()` / `models_cache_dir()` (`crates/avi-store/src/lib.rs`) dependen de `LOCALAPPDATA` / `XDG_DATA_HOME`, así que un daemon y un cliente arrancados con entornos distintos responden «voz no encontrada» para una voz que el cliente sí lista. El daemon no expone un endpoint para inspeccionar su vista del almacén (`GET /voices` fue podado). Con `AVI_DATA_DIR`, esa dependencia es además el mecanismo de aislamiento por instancia en tests (sandbox propio por test; en producción la resolución por defecto no cambia).

## 13. `translate`, `speech transcribe`, `speech dub` y la síntesis cross-lingual

### `translate`: texto→texto, aislado de la síntesis

`translate` no pertenece a ningún grupo nominal: es texto→texto, sin voz ni modelo TTS de por medio. `--source-language` y `--target-language` son **opcionales con defaults** (`es` y `en`) y **estrictos**: el parser solo acepta `es` o `en`; cualquier otro valor lo rechaza `clap` con exit 2 antes de llegar al handler. El vocabulario de la CLI es `es|en` en todos los comandos, y el daemon acepta las mismas claves `source_language`/`target_language` con los mismos valores. `--source-language == --target-language` es *passthrough*: devuelve el texto sin cargar el modelo. `translate` no tiene límite de longitud.

Parámetros: `--text`/`-t` **requerido** · `--source-language` (default `es`, solo `es`/`en`) · `--target-language` (default `en`, solo `es`/`en`) · `--json`. Su payload está en [`commands/TRANSLATE.md`](commands/TRANSLATE.md) y en la [referencia de esquemas](../../USAGE.md#referencia-de-esquemas---json).

### Traducción opt-in en `speech say`/`speech synthesize`

`--source-language`/`--target-language` (§3) insertan una etapa de traducción **antes** de la síntesis cuando declaran idiomas distintos; el motor de síntesis no cambia, solo recibe el texto ya traducido. Ambos son opcionales en `say`/`synthesize`: `--target-language` vale `es` por defecto y `--source-language` vale lo mismo que `--target-language` cuando se omite, por lo que sin flags no se traduce. Solo se admite `es` o `en`; cualquier otro valor sale con **2**. Ese mismo token es el idioma de la síntesis: el motor lo recibe traducido a su nombre (`es` → `Spanish`, `en` → `English`), y `POST /synthesize` del daemon rechaza un `source_language`/`target_language` que no sea `es` o `en` con `usage_error`. El modelo de traducción ausente reutiliza el exit **4** (`ExitCode::ModelMissing`), con la solución `setup` en la línea `Solución:`, en vez de un código propio: es la misma precondición de entorno que el modelo TTS. Un fallo de la inferencia de traducción, con el modelo ya cargado, sale con **9** (`ExitCode::TranslationFailed`, §9) — código distinto del **1** genérico de síntesis, porque distingue en qué etapa falló la invocación.

`--temperature` es un override opcional del muestreo en `say`/`synthesize`/`dub`: sin el flag se usa la temperatura de producción (`0.35`); con el flag debe cumplirse `0 < t <= 2.0`, y fuera de ese rango sale con **2**.

### El flag `--target-language`

`speech say`/`speech synthesize` declaran el idioma con `--source-language`/`--target-language`; ni `setup` ni `doctor` exponen flags de idioma (la provisión no traduce y el diagnóstico no filtra por idioma). `daemon` **no expone** flags de idioma (ver `docs/CLI/commands/DAEMON.md` — `native-stt`/`native-translation` son features de compilación, no flags de ejecución). El integrador de narración (§12) invoca `speech say` sin flags de idioma: sintetiza en `es` sin traducción, porque origen y destino coinciden.

### Provisión y daemon

`setup` —y la provisión de `self install`, que comparte su función— descarga siempre `opus-mt-es-en` y `opus-mt-en-es`: derivados int8 de Helsinki-NLP/opus-mt (CC-BY-4.0) ya convertidos a CTranslate2 y publicados en `CristianRojaas/opus-mt-es-en-ct2-int8` y `CristianRojaas/opus-mt-en-es-ct2-int8`, sin conversión local. El motor los lee directo del snapshot de HuggingFace y los valida con `is_provisioned` (cada fichero de `MODEL_FILE_PATTERNS` presente y de más de 0 bytes: `config.json`, `model.bin`, `shared_vocabulary.json`, `source.spm`, `target.spm`); `doctor` exige ambos modelos como parte de `models_provisioned`; `cleanup --model` borra la raíz de modelos entera si es exclusiva y solo los repos fijados si es compartida (R3). Sin gating por `--language`: la provisión es determinista para la instalación por defecto. `daemon start`/`serve` aceptan `--auto-restart` y `--max-retries` (default `3`) — con `--auto-restart` el supervisor reintenta hasta `max_retries` con backoff `500ms*2^retries` capado a `4s` (cada reintento con reclamo activo del árbol propio previo con deadline de 5 s y verificación de muerte + puerto libre; el reclamo activo matar-y-rearrancar vive en `start`, nunca en `serve`; con puertos efímeros el reclamo previo al reintento pierde su objeto (el cliente ya descubre la `addr` real por el pidfile); un `daemon stop` graceful vía `shutdown_notify` no reintenta, y un fallo previo a que el daemon haya estado listo (puerto ocupado, voz de warmup inexistente, carga del estado) es terminal y no se reintenta.

### `speech transcribe`: audio→texto, verificable sin traducir ni sintetizar

`speech transcribe` es una **sub-acción del grupo `speech`** (no un comando aislado como `translate`): transcribe a texto con `parakeet-tdt-0.6b-v3` int8 vía `ort` load-dynamic (del módulo de transcripción STT, `ParakeetEngine::transcribe`), desde un archivo WAV (`--audio`) o desde el micrófono (`--mic`). La **captura corre siempre en el cliente** (al daemon viajan las muestras ya decodificadas en base64, nunca rutas); la transcripción en sí recibe el despacho al daemon en sus tres modos (§5): sin flags, transcribe por el daemon si está activo y en modo directo si no; `--daemon` exige el daemon y sale con **5** si no está; `--no-daemon` fuerza el modo directo. Es una operación **de un solo idioma por invocación** — a diferencia del par de `translate`, aquí `--source-language` (requerido, `es`/`en`) declara el único idioma hablado en el audio. `ParakeetEngine` **solo transcribe**, nunca traduce: si el usuario necesita el texto en otro idioma, encadena `translate` por separado. No hay síntesis de por medio: `speech transcribe` es verificable de forma aislada, con audio de entrada y texto de salida, sin depender del motor TTS ni del subsistema de traducción.

`--audio` y `--mic` forman un **grupo mutuamente excluyente `required=True`**, igual que en `speech dub`. `voice clone` también exige una fuente exclusiva, `--speech-reference` o `--mic`; los flags de enrutamiento `--daemon`/`--no-daemon` son mutuamente excluyentes y opcionales. Con `--mic`, la captura es **push-to-talk** por defecto (Enter para terminar); `--duration N` fuerza una grabación de duración fija en segundos y solo es válido junto a `--mic` (el parser lo declara incompatible con `--audio`) — con `--audio`, o con `--mic` ausente, `--duration` sale con **2** (`ExitCode::InvalidInput`, `usage_error`), igual que la ausencia de ambos o la presencia de los dos. Sin terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con **2**, porque no hay forma de detectar la pulsación de Enter que cierra el push-to-talk. El push-to-talk tiene un techo de seguridad configurable con la variable de entorno `AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s): al alcanzarlo, la grabación se detiene, se avisa por stderr (`Aviso: Se alcanzó el límite de grabación (m:ss); se detiene.`) y se continúa con lo grabado hasta ese punto — sale con **0**, no es un error. La grabación nunca es una espera silenciosa: con terminal muestra el contador y el medidor de nivel (`● Grabando m:ss / m:ss`), y sin ella escribe una sola vez `Grabando (máximo m:ss)…`. Si el tramo de 100 ms más fuerte de la grabación no supera −43 dBFS, avisa `Aviso: No se detectó voz en la grabación (nivel máximo N dBFS); comprueba que el micrófono no esté silenciado.` y el comando sigue igual (el aviso no se aplica a archivos de audio). La captura usa el backend multiplataforma `cpal` (único, sin ramas por sistema operativo): graba a la tasa y formato nativos del dispositivo de entrada y normaliza a 16 kHz/mono/int16 (formato que `ParakeetEngine` asume), la misma normalización que aplica el WAV de `--audio`; la inferencia es `ort` load-dynamic con los 4 artefactos `MODEL_FILE_PATTERNS` (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), stack Rust nativo.

Parámetros: `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** (`es`/`en`) · `--daemon`/`--no-daemon` · `--json`.

El payload `--json` (en [`commands/SPEECH.md`](commands/SPEECH.md)) no cambia con el despacho: es el mismo en los tres modos. `source_language` es el valor de `--source-language` tal como se pasó (`es` o `en`).

Si el modelo `parakeet-tdt-0.6b-v3` no está provisionado, sale con **4** (`ExitCode::ModelMissing`), y la línea `Solución:` indica `ai-voice-interconnector setup`; un fallo de la inferencia con el modelo ya cargado, o de la captura del micrófono no atribuible al entorno, sale con **10** (`ExitCode::TranscriptionFailed`, §9) — mismo criterio de asignación que distingue **4** de **9** en `translate`. La falta de micrófono o de backend de audio sale con **3** (`no_input_device`, `audio_backend_unavailable`) y una captura sin muestras con **2** (`empty_audio`). La lectura del `--audio` no sale con 10: un archivo inexistente sale con **3** (`ExitCode::NotFound`, `audio_not_found`), un WAV corrupto, truncado o no decodificable con **2** (`ExitCode::InvalidInput`, `invalid_audio`) y un fallo de E/S al leerlo con **1** (`io_error`). El audio de `--audio` o `--mic` dura como máximo 300 s (`MAX_TRANSCRIBE_AUDIO_SECS`; `audio_too_long`, exit **2**, §6). En la ruta daemon, un fallo de comunicación —daemon inactivo— sale con **5** (`ExitCode::DaemonUnreachable`), sin degradación silenciosa a modo directo. Un error HTTP del daemon se traduce por su `reason` con la tabla única (p. ej. 404 `model_missing` → **4**, 404 `voice_not_found` → **3**); un 404 sin `reason` ni `error` (un daemon que no expone la ruta) sale con **1** (`daemon_error`) y el mensaje «El daemon no expone esta ruta (HTTP 404): es anterior a este programa o no incluye la función.», nunca con 5; la línea `Solución:` indica reiniciar el daemon.

**Mismo shape `--json` en `translate` y `transcribe`.** `translate --json` emite `source_language`/`target_language` con los valores de `--source-language`/`--target-language` (`es`, `en`), y `speech transcribe --json` emite `source_language` con el valor de su `--source-language`: ambos preservan el token de entrada, que ya viene restringido a `es|en` por el parser.

### `speech dub`: el bucle voz→voz en un comando dedicado

`speech dub` es la **composición voz→voz**: transcribe la entrada hablada (archivo o micrófono), traduce el texto si `--source-language` difiere de `--target-language`, sintetiza con la voz elegida y reproduce el resultado. En la ruta daemon, delega a `POST /dub` mediante streaming NDJSON (`started` → latidos → `result`), sin degradación a otra composición: todo error de la ruta se traduce por su `reason` con la tabla única de códigos. Reutiliza las máquinas existentes —la etapa de transcripción de `speech transcribe` (sus tres modos de despacho), la traducción opt-in de `say`/`synthesize` y el despacho de síntesis (§5)— sin modificarlas. `say`/`synthesize` **no cambian**: siguen siendo texto→voz con `--text` requerido; la entrada de audio del bucle vive solo en `dub`. No persiste nada: no declara `--label` propio, y su `--json` es el **global** ([§2](#la-superficie-del-ciclo-de-vida-flag-a-flag)), que el comando sí honra en las dos vías: por el daemon emite el envelope del stream y en la vía directa el payload de `speech dub` (ver [`commands/SPEECH.md`](commands/SPEECH.md)).

Exige **exactamente una** de `{--audio, --mic}` (grupo mutuamente excluyente `required=True`, espejo de `speech transcribe`); `--duration N` solo es válido con `--mic` (el parser lo declara incompatible con `--audio`; con `--audio` sale con **2**, `usage_error`). Con `--mic`, aplica el mismo push-to-talk con techo `AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s, exit 0 al vencer) y aviso mínimo por stderr al iniciar la captura descritos en `speech transcribe`, por reutilizar su misma etapa de transcripción. `--source-language` es **requerido** (`es`/`en`); `--target-language` (default `es`) elige el idioma/modelo de síntesis y dispara la traducción si difiere del hablado; `-v/--voice` y `--temperature` son los de `say`. `--daemon`/`--no-daemon` aplican a la transcripción y a la síntesis (los tres modos, §5); sin flags, ambas etapas usan el daemon solo si responde.

| Parámetros | Comportamiento |
|---|---|
| `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** · `--target-language` (default `es`) · `-v/--voice` · `--temperature` · `--daemon`/`--no-daemon` · `--json` (global, [§2](#la-superficie-del-ciclo-de-vida-flag-a-flag)) | transcribe → traduce (si `source != target`) → sintetiza → reproduce |

Códigos de salida aplicables en la cadena: **4** (`ExitCode::ModelMissing`, modelo de transcripción no provisionado, remite a `setup`), **5** (`ExitCode::DaemonUnreachable`, daemon exigido pero inactivo), **9** (`ExitCode::TranslationFailed`, fallo del pipeline de traducción con el modelo cargado) y **10** (`ExitCode::TranscriptionFailed`, fallo del pipeline de transcripción con el modelo cargado o de la captura del micrófono no atribuible al entorno); más **2** (uso inválido: `--duration` sin `--mic`, `--mic` sin TTY y sin `--duration`; `invalid_audio` para un WAV corrupto o truncado; `empty_audio` si la captura no entregó muestras; `audio_too_long` si el audio o `--duration` superan 40 s; `text_too_long` si el texto traducido supera 500 caracteres) y **3** (`audio_not_found`, `--audio` inexistente; `no_input_device`, `no_output_device` y `audio_backend_unavailable` si el sistema no tiene micrófono, salida o backend de audio). El audio de entrada del dub dura como máximo `MAX_DUB_AUDIO_SECS` (40 s): el push-to-talk se corta a ese tope (además del techo `AVI_PUSH_TO_TALK_MAX_SECS`) y la síntesis final se acota con el presupuesto de §5 sobre el texto traducido.

---

## 14. Documentación complementaria por comando

Cada comando principal de la CLI tiene una referencia por comando en [`commands/`](commands/) que cubre su diseño, implementación, flujo de ejecución y manejo de errores con referencias a los símbolos del código fuente. El índice de comandos y sus documentos está en [`README.md`](README.md#resumen-de-comandos).
