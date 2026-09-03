# Contrato de la CLI: el grupo `speech` y el contrato de salida

Este documento es la descripción normativa del contrato público de la CLI —comandos, flags, códigos de salida, payloads `--json`— y del almacén de habla sintética. Recoge también el porqué de cada decisión de diseño: los criterios generadores, las asimetrías de reversibilidad y el razonamiento detrás de cada código de salida.

---

## Tabla de contenidos

- [1. Invariantes y criterios generadores](#1-invariantes-y-criterios-generadores)
- [2. La superficie y el vocabulario](#2-la-superficie-y-el-vocabulario)
- [3. El grupo `speech`](#3-el-grupo-speech)
- [4. Síntesis y el bucle de `--play`](#4-síntesis-y-el-bucle-de---play)
- [5. El despacho al daemon](#5-el-despacho-al-daemon)
- [6. Reglas de validación](#6-reglas-de-validación)
- [7. Matrices de comportamiento](#7-matrices-de-comportamiento)
- [8. El almacén de habla sintética](#8-el-almacén-de-habla-sintética)
- [9. Los códigos de salida](#9-los-códigos-de-salida)
- [10. El canal de error y los payloads](#10-el-canal-de-error-y-los-payloads)
- [11. `cleanup`, `setup` y `voice`](#11-cleanup-setup-y-voice)
- [12. Contratos externos](#12-contratos-externos)
- [13. El comando `translate` y la síntesis cross-lingual](#13-el-comando-translate-y-la-síntesis-cross-lingual)

---

## 1. Invariantes y criterios generadores

Cinco criterios gobiernan el resto del diseño. No son conclusiones: son las reglas con las que se resuelven las preguntas que el diseño todavía no ha visto.

#### Ninguna superficie acepta rutas del llamador

**El sistema no lee ni escribe `.wav` en rutas elegidas por quien invoca.** Ni en escritura, ni en lectura, ni por el protocolo del daemon. Toda ruta de audio la computa el sistema.

El almacén de habla sintética no viola el invariante: su ruta se deriva de `(voz, etiqueta)`, que son identificadores del contrato y no rutas. El registro de voces resuelve las suyas igual, a partir del nombre de la voz.

La consecuencia sobre el daemon es estructural y no una validación: `/synthesize` recibe `voice: str`, así que no hay nada que sanear. La superficie de ataque «leer un `.wav` de una ruta elegida por el llamador» se cierra en el protocolo, no en un comprobador. El patrón ya está establecido en el mismo módulo del protocolo por `PrecomputeVoiceRequest`, que lleva solo `name: str` y cuyo docstring enuncia el razonamiento.

#### Una responsabilidad por sub-acción

Un comando cuyo comportamiento lo deciden los flags no tiene una responsabilidad con opciones: tiene varias acciones disfrazadas de una. **Producir un artefacto** y **emitir sonido** son responsabilidades distintas, y cada una tiene su propia sub-acción.

De ahí sale la forma del grupo `speech`, y de ahí sale que no haya reglas que tapen combinaciones malas: **las combinaciones malas no son expresables**. Cuando una regla de validación existe solo para impedir que un flag quede sin objeto, el defecto está en el reparto de responsabilidades y no en la falta de la regla.

Corolario de legibilidad: **el nombre de cada sub-acción declara su costo.** Sintetizar paga GPU y puede exigir provisión del modelo; reproducir paga una lectura de archivo. Desde fuera se sabe cuál se pagó sin leer los flags.

#### El eje de dos preguntas que genera la tabla de códigos de salida

Son **dos preguntas encadenadas, no una**. La primera forma las clases; la segunda decide cuáles merecen un entero propio. Separarlas es lo que vuelve la tabla derivable: un eje único mezcla dos trabajos distintos —clasificar y repartir— y toda formulación que los funde acierta en una mitad y falla en la otra.

1. **Clasificación: ¿qué tipo de hecho impidió la operación?** Da seis clases: invocación mal formada, recurso ausente, recurso ocupado, precondición de entorno incumplida, imposibilidad permanente e imprevisto.
2. **Admisión: ¿un consumidor programado cambiaría su siguiente llamada al distinguir esta clase de las demás?** Si sí, la clase gana entero propio; si no, comparte entero y la distinción baja al `reason` del payload de error. Se responde diciendo qué se invocaría a continuación, sin apelar a la intuición de quien redacta.

**El dominio del eje son los códigos de fallo.** Quedan fuera, y es deliberado: el `0`, que no es un fallo y por tanto no tiene remedio del que hablar; el `130`, que es convención de señales (`128 + SIGINT`) y es correcto por otra razón; y el `1` de `doctor`, que usa el entero como canal de **veredicto** y no de fallo, porque el trabajo de ese comando *es* diagnosticar.

**Corolario que gobierna toda clasificación: la ausencia de consumidor no valida ninguna clasificación.** Un código que nadie lee y que miente seguirá mintiendo cuando lo lean, y para entonces corregirlo será una ruptura en vez de un refinamiento. La tabla se define por el tipo de causa y por la siguiente llamada del consumidor, no por quién consume el código ni por si alguien lo consume.

#### Cuándo un payload transporta una ruta del filesystem

**Un payload emite una ruta solo cuando el recurso no tiene otro nombre en el contrato.**

| Payload | Emite ruta | Por qué |
|---|---|---|
| `voice list --json` | No: `{"voices": [nombres]}` | La voz tiene handle propio —su nombre—, así que el directorio nunca sale |
| `cleanup --json` | Sí: `removed` como lista de rutas | Los directorios de caché del modelo y de voces no tienen ningún handle en la CLI; la ruta es su único nombre |

La locución tiene `(voz, etiqueta)`, y las cinco sub-acciones del grupo `speech` operan exactamente sobre ese par: cae del lado de `voice list`. Emitir además la ruta le daría al integrador un **segundo handle, no gobernado**, sobre un recurso que ya tiene el suyo — y nada le impediría usarlo, momento en el cual el invariante de las rutas sería decorativo: no lo violaría el sistema, lo violaría el consumidor con lo que el sistema le entregó.

**La asimetría de reversibilidad que respalda el criterio.** Las dos opciones no cuestan lo mismo si resultan equivocadas: **añadir una clave después es aditivo** y está cubierto por la política de compatibilidad del esquema `--json`; **retirarla es incompatible** y obliga a subir `schema_version="3"` (`src/main.rs` / `crates/avi-core/src/json_emitter.rs`). Con esa asimetría, el lado seguro se conoce de antemano y no hay opcionalidad que comprar aplazando la decisión.

**Coste declarado.** Ninguna superficie saca los bytes de una locución fuera de la CLI: `speech play` la reproduce y no hay ningún comando de exportación. Un orquestador que quiera el WAV no lo tiene. Eso es un hueco de la superficie de comandos; la respuesta, si la necesidad aparece, es un comando explícito con su propia decisión, no una clave en un listado.

#### El canal de la causa fina, y la regla que decide entre código y razón

El entero no puede llevar la causa fina y no debe intentarlo. Una misma reacción del consumidor puede corresponder a varias acciones distintas del destinatario humano: liberar disco, corregir permisos, renovar un token, desbloquear la red o instalar una dependencia inducen todas la misma siguiente llamada, y son cinco cosas distintas que hacer antes de repetirla.

El proyecto tiene dos canales legibles por máquina y usa los dos: el entero, que es un espacio cerrado, y el payload JSON, que es **aditivo por contrato** y tiene un punto único de emisión, `emit_json()`. La distinción fina va por el canal abierto.

**Tres reglas de compatibilidad**, que son lo que impide reabrir la misma brecha un nivel más allá:

1. **El entero siempre basta por sí solo.** `reason` refina; nunca contradice ni condiciona. Un consumidor que ignore la clave se comporta correctamente, solo que con menos resolución. Sin esta regla el segundo canal sería una segunda tabla congelada.
2. **Añadir un `reason` nuevo no incrementa `schema_version`**, igual que añadir una clave. Es contrato de emisión **y de consumo**: un `reason` desconocido se trata como ausente, es decir, se degrada al entero.
3. **Regla de promoción.** Un código de salida nuevo solo se justifica cuando cambia **la siguiente llamada del consumidor** —la segunda pregunta del eje—; cuando la llamada siguiente es la misma y lo que cambia es la acción concreta que alguien ejecuta antes de repetirla, es un `reason`. Su árbitro es único y comprobable: se responde diciendo qué se invocaría a continuación, no sopesando importancia.

## 2. La superficie y el vocabulario

#### Nueve comandos de nivel superior

| Comando | Sub-acciones | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `play`, `list`, `remove`, `transcribe`, `dub` | Síntesis de habla, gestión del almacén, transcripción de audio a texto y composición voz→voz |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
| `translate` | — | Traducción de texto `es↔en`, aislada de la síntesis |
| `devices` | — | Lista dispositivos de audio |
| `doctor` | — | Diagnósticos |
| `setup` | — | Provisión del runtime |
| `cleanup` | — | Borrado de modelo, voces y/o habla sintética |
| `daemon` | `start`, `stop`, `restart`, `status`, `serve` | Ciclo de vida del daemon |
| `version` | — | Versión |

**Tres de ellos son grupos nominales de gestión** —`speech`, `voice` y `daemon`—: tienen sub-acciones y ninguna acción propia.

Todos los subcomandos salvo `daemon serve` declaran `--json`, y la garantía es mecánica: un test recorre el parser real para descubrir cuáles lo declaran, de modo que una sub-acción nueva sin `--json` lo hace fallar.

#### El qualifier `synthetic` y la resolución del vocabulario

`speech` nombra el **género**: habla. El qualifier `synthetic` marca la dirección del flujo de datos —lo que el sistema produce frente a lo que el usuario aporta— y mantiene separadas las tres capas donde el término aparece.

| Capa | Elemento | Nombre |
|---|---|---|
| CLI | Grupo de síntesis y gestión de la salida | `speech synthesize/say/play/list/remove` |
| CLI | Entrada de referencia de timbre de `voice clone` (opcional) | `--timbre-reference` (`-t`) |
| CLI | Entrada de referencia de habla de `voice clone` (obligatoria, ≥10s) | `--speech-reference` (`-s`) |
| CLI | Borrado masivo de la salida | `cleanup --synthetic-speech` |
| Filesystem | Almacén de la salida generada | `<data_dir>/speech/<voz>/<etiqueta>.wav` (`crates/avi-store/src/lib.rs` `SpeechStore`) |
| Filesystem | Archivos de referencia de una voz | `timbre-reference.wav`, `speech-reference.wav` (`crates/avi-store/src/lib.rs` `VoiceStore`) |
| Payload | Clave del listado | `synthetic_speech` |
| Interno | Parámetro del timbre en el motor y el protocolo | `timbre` |

El orden de palabras respeta la convención del repo, con el núcleo al final (`--compute-backend`, `--timbre-reference`). El qualifier vive solo en el directorio y en el flag de `cleanup` —las dos operaciones de gestión—, no en la ruta caliente, que es `speech synthesize`.

En disco los dos sentidos quedan separados por nombre y no por posición:

```
<data_dir>/
  voices/<voz>/timbre-reference.wav     ← entrada aportada (opcional) (`VoiceStore`)
  voices/<voz>/speech-reference.wav     ← entrada aportada (obligatoria) (`VoiceStore`)
  speech/<voz>/<etiqueta>.wav           ← salida generada (`SpeechStore`)
```

**En prosa española la unidad se llama «locución».** Nunca aparece como identificador.

#### Las decisiones de vocabulario de la superficie

- **El identificador de una locución es `--label/-l`, no `--name/-n`.** Por homología con `voice` correspondería `--name`, pero dentro del grupo `speech` sería ambiguo frente a `--voice` («¿nombre de qué?»). Se acepta la divergencia con `voice --name` a cambio de que el mismo concepto no tenga dos nombres en dos comandos.
- **La voz se selecciona con `--voice/-v` en las cinco sub-acciones**, no con `--voice-profile`: el concepto ya se llama «voice» en `voice list`, `voice clone` y `voice remove`, y darle un segundo nombre en otro comando es la homonimia al revés —dos palabras para una cosa— con el mismo costo.
- **`--play` y la sub-acción `play` comparten palabra a propósito.** Nombran una sola cosa —emitir audio por los parlantes— en los dos sitios donde ocurre.
- **`-t` es `--text` en `speech` y `--timbre-reference` en `voice clone`.** Cada corto vive en su subcomando, sigue a su flag largo y no se solapa: `voice clone` no declara `--text` y `speech` no declara referencias.
- **`-n` no está tomado en el grupo `speech`**, así que tiene un significado único en toda la CLI: `--name` en `voice clone` y `voice remove`.

## 3. El grupo `speech`

#### Reparto de responsabilidades

| Sub-acción | Responsabilidad | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda | **sí** | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech play` | Reproduce una locución guardada | no | **no** |
| `speech list` | Lista las locuciones guardadas | no | no |
| `speech remove` | Borra una locución guardada | no | no |

`synthesize` y `say` son gemelos: misma síntesis, distinto destino —disco o parlantes—. `play`, `list` y `remove` son la gestión del almacén. **`say` es la única sub-acción que genera sin persistir, y junto con `synthesize` la única que puede exigir provisión del modelo**; esa es la contrapartida de que `play`, `list` y `remove` no lo necesiten.

El almacén etiquetado es un recurso, y el repo tiene gramática para gestionar recursos: un grupo nominal con sub-acciones. La homología con `voice` es directa:

| Registro de voces | Almacén de habla sintética |
|---|---|
| `voice list` | `speech list` |
| `voice clone` | `speech synthesize` |
| `voice remove` | `speech remove` |
| — | `speech play` |
| — | `speech say` |

#### Parámetros

| Sub-acción | Parámetros |
|---|---|
| `speech synthesize` | `--text/-t` **requerido** · `--label/-l` **requerido** · `--voice/-v` · `--output/-o` · `--play` · `--force/-f` · `--json` · `--daemon`/`--no-daemon` |
| `speech say` | `--text/-t` **requerido** · `--voice/-v` · `--json` · `--daemon`/`--no-daemon` |
| `speech play` | `--label/-l` **requerido** · `--voice/-v` · `--json` |
| `speech list` | `--voice/-v` (filtro) · `--json` |
| `speech remove` | `--label/-l` **requerido** · `--voice/-v` · `--json` |

**`--voice/-v` es opcional en las cinco** y, si falta, usa la voz de fábrica `default` (voz clonada de fábrica con `.qvoice` graft, `crates/avi-store/assets/default/reference.qvoice`, `crates/avi-store/src/lib.rs` `FACTORY_VOICES`). El catálogo de voces de fábrica es `default` (clonada), `ryan` y `vivian` (presets del motor `qwen_tts.c:spk_table`): todas entradas del registro `VoiceStore`. La distinción interna preset/clonada (`avi-tts/src/lib.rs` `resolve_voice_motor`, presencia de `reference.qvoice`) es detalle no-normativo; para el contrato toda voz es una entrada del registro. Las tres voces de fábrica sintetizan texto corto (2-4 palabras, p. ej. `Hola mundo`) con `WER ≤0.25`.

**El namespace es obligatorio en la gestión.** Las etiquetas viven bajo una voz, así que `play` y `remove` toman `--voice` con el mismo default que `synthesize` y `say`; `list` lo admite como filtro y sin él recorre todas las voces. Es un segmento más que en `voice remove --name X`, inevitable dado el layout del almacén.

**`--label` requerido en `synthesize` es lo que sostiene el reparto.** Elimina de raíz la invocación con efecto cero sin escribir ninguna regla —la rechaza el parser— y elimina la trampa de «previsualizo con un comando y guardo con otro»: como `synthesize` siempre persiste, nadie pierde la toma que acaba de oír.

**`speech play` no necesita modelo ni daemon**: lee el WAV del almacén y lo reproduce.

**El listado no vive dentro de `synthesize`.** No hay `speech synthesize --list`: el listado es `speech list`.

**Reparto con `cleanup`**: `speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que existe entre `voice remove` y `cleanup --voices`.

## 4. Síntesis y el bucle de `--play`

#### Qué hace cada gemelo

Sin `--play`, `synthesize` sintetiza, guarda y termina. Con `--play`, reproduce la toma y pregunta antes de guardar.

`speech say` sintetiza y reproduce, y no escribe nada en el almacén. Es el destino de la invocación que solo quiere oír el resultado: la que no nombra un artefacto porque no lo quiere.

**Son dos usos que no se cruzan, y el diseño no supone que la síntesis sea determinista.** `say` es locución continua, generada al vuelo: cada mensaje es distinto del anterior y se descarta al sonar, así que persistir no tendría sentido. `synthesize` es para grabar un mensaje reutilizable —el caso de los mensajes por defecto— y reproducirlo después sin volver a sintetizarlo. No existe un recorrido que salte de `say` a `synthesize` para «quedarse» con una toma ya oída: quien quiere conservar usa `synthesize` desde el principio. Por eso la reproducción sin re-síntesis la garantiza **el almacén** —se guarda un WAV y se reproduce ese WAV—, y no una supuesta reproducibilidad del motor entre dos llamadas. Dentro de `synthesize`, la variación entre tomas es esperada y es justo lo que «rechazar y regenerar» aprovecha; «aceptar y guardar» persiste la toma que sonó, nunca una nueva.

#### El bucle de `--play`: cuatro opciones

| Opción | Efecto | Costo |
|---|---|---|
| Reproducir otra vez | Vuelve a sonar la misma toma | **Cero síntesis**: los bytes están en memoria |
| Aceptar y guardar | Persiste la toma que acabas de oír, y termina con 0 | Cero |
| Rechazar y regenerar | Sintetiza otra toma y vuelve a preguntar | T3+S3Gen, **nada** de la Etapa 1: los conditionals de una voz del registro están precomputados |
| Rechazar y descartar | Termina con 0 **sin guardar nada** | Cero |

**«Descartar y salir» es una salida de primera clase**, con exit 0 y sin persistencia: el rechazo es un campo del resultado, no un error. Es el mismo modelado que `cleanup`, donde responder «n» a la confirmación termina con 0. Lo que el bucle no comparte con ese comando es la forma de la elección —allí es binaria— ni el destino de su prosa: la pregunta y sus avisos respetan la separación de canales (con `--json` la información humana va a stderr y stdout queda para el payload) y la cancelación viaja como campo del resultado.

**«Descartar» y no «rechazar».** En el bucle, regenerar también rechaza la toma; la palabra del contrato no distinguiría entre las dos opciones que descartan el audio actual, y solo una de ellas termina la invocación.

**Ctrl-D es el atajo de «descartar y salir».** Con terminal presente, cerrar la entrada en la pregunta es una forma legítima de abandonar y mapea exactamente sobre la cuarta opción: exit 0, sin persistir. Es el único fin de entrada alcanzable en el bucle, y tiene significado propio.

#### Cuándo persiste, y qué protege la colisión

**Cuándo persiste.** Sin `--play`, inmediatamente después de sintetizar. Con `--play`, solo al aceptar. Así «descartar» nunca es un borrado: es no haber escrito.

**La colisión de etiqueta se comprueba dos veces, y cada comprobación tiene un papel distinto.**

- **Antes de sintetizar**, como *fast-fail*: si la etiqueta está tomada y no hay `--force`, el comando sale con **6** sin gastar GPU. Comprobarla solo después obligaría a pagar la síntesis entera para descubrir que no se puede guardar, y con `--play` además a recorrer el bucle hasta «aceptar» para fallar ahí.
- **Al escribir**, y **esta es la que gobierna el contrato**: entre la comprobación previa y la escritura hay una ventana —el bucle puede durar minutos— y la etiqueta puede quedar tomada en ese intervalo. Si al escribir está tomada y no hay `--force`, la salida es **6**.

## 5. El despacho al daemon

#### Tres modos

| Invocación | Qué hace |
|---|---|
| Sin flags | **Comprueba el daemon.** Si está activo, sintetiza por él; si no, carga el modelo al vuelo |
| `--no-daemon` | Fuerza la síntesis directa aunque el daemon esté activo |
| `--daemon` | **Exige** el daemon: si no está activo, sale con **5** en vez de degradar |

La autodetección es el único camino por defecto: un comportamiento especificado, no una rama a la que se cae cuando el llamador no dice nada.

**No hay degradación silenciosa.** `--no-daemon` es un opt-out explícito del usuario, categóricamente distinto de una degradación automática que elude una restricción sin que nadie la pida.

#### Qué superficies lo reciben

**Las cinco que necesitan un modelo cargado: `speech synthesize`, `speech say` y `voice clone` (el TTS) y `speech transcribe` y `speech dub` (el de transcripción).** `voice clone` precomputa los conditionals de la voz al clonarla, así que necesita el modelo igual que las dos que sintetizan, y recibe los tres modos por simetría: con `--daemon` lo exige y sale 5 si no está, y con `--no-daemon` fuerza la ruta directa.

`speech play`, `speech list` y `speech remove` no lo reciben porque no tocan el modelo.

#### Por qué `--daemon` significa exigir y no seleccionar

Con la autodetección por defecto, «usa el daemon» deja de ser algo que haya que pedir. Sin el flag, el llamador no tendría forma de exigir la ruta rápida y el código 5 **se quedaría sin ningún productor en la síntesis**: si la ausencia del daemon siempre degrada, nunca hay «daemon inalcanzable», solo una invocación más lenta. Un consumidor con presupuesto de latencia —el narrator es el caso previsto— necesita poder decir «prefiero fallar a esperar a que cargue el modelo».

Con los dos flags declarados, la exclusión mutua entre ellos tiene sentido pleno: «exige daemon» y «prohíbe daemon» se contradicen.

#### Despacho y modo directo

Con el daemon activo, `speech synthesize`/`say`/`transcribe`/`dub` y `voice clone` pueden usar modelo caliente; `--no-daemon` fuerza ruta directa sin sondeo.

## 6. Reglas de validación

#### Las cinco reglas, todas con exit 2

1. **`--daemon` y `--no-daemon` son excluyentes.** La resuelve el grupo mutuamente excluyente del parser, no una comprobación a mano. Aplica a `speech synthesize`, `speech say` y `voice clone`.
2. **`--json` es incompatible con `--play`.** El bucle escribe la pregunta y lee la respuesta por los canales estándar, y contaminaría el payload. Aplica a `speech synthesize`.
3. **`--text` no vacío ni solo espacios.** Aplica a `speech synthesize` y `speech say`.
4. **`--text` no excede `MAX_TEXT_LENGTH`** (5000). Se valida **en el cliente** antes de cualquier despacho, con el mismo código por ambas vías; el tope del daemon es defensa en profundidad y no la fuente de la validación. Aplica a `speech synthesize` y `speech say`.
5. **`--play` exige terminal en la entrada estándar.** Si no la hay, se rechaza **antes de sintetizar**. Aplica a `speech synthesize`.

**La regla 5 es de otra clase que las cuatro anteriores**: las cuatro primeras miran los flags, la quinta mira el entorno. La comprobación no altera ningún default —`--play` es explícito, así que la misma línea de comandos no puede significar cosas distintas según dónde corra—; solo rechaza antes una invocación que iba a fallar igual. Lo único que queda fuera de alcance es alimentar las respuestas del bucle por una tubería, un caso marginal cuyo precio, de conservarlo, sería pagar una síntesis y una reproducción completas antes de fallar.

#### Un solo mecanismo para la exclusión mutua, y es el declarativo

La exclusión mutua se declara con `clap` (`conflicts_with`) en `src/main.rs` (`Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`), junto a los flags que restringe, en todos los sitios donde exista —el grupo de tres modos de `setup` incluido. **La garantía queda en un solo lugar, no repetida por convención en cada comando.** Una comprobación manual es esa convención repetida, y no escala: en un grupo de tres modos, un cuarto añadido a mano no rompe nada y deja de cubrir una combinación en silencio; el `if` vive lejos de los flags que restringe, donde nadie que añada uno lo va a leer.

El coste es que el mensaje lo formatea `clap` en inglés, igual que el de todas las demás rutas de parseo, y ese mensaje entra íntegro en el payload de error.

#### Validación de identificadores y de existencia

| Situación | Superficies | Código |
|---|---|---|
| Etiqueta con caracteres ilegales | `synthesize`, `play`, `remove` | **2** |
| Nombre de voz con caracteres ilegales | Todas las que toman `--voice` | **2** |
| Voz inexistente | **Las cinco**: `synthesize`, `say`, `play`, `list`, `remove` | **3** |
| Etiqueta inexistente | `play`, `remove` | **3** |
| Colisión de etiqueta sin `--force` | `synthesize` | **6** |
| Colisión de nombre de voz sin `--force` | `voice clone` | **6** |

**La voz se valida en las cinco sub-acciones y sale 3 si no está** —el catálogo es el registro unificado `VoiceStore` (`crates/avi-store/src/lib.rs` `FACTORY_VOICES`): voces de fábrica (`default` —clonada con `reference.qvoice`—, `ryan`, `vivian`) más clonadas del usuario—, de modo que «voz mal escrita» nunca se disfrace de «sin resultados»: sin esa regla, `speech list --voice noexiste` devolvería una lista vacía y un usuario que se equivoca al escribir concluiría que sus locuciones se perdieron. Con `--voice` opcional en las cinco, la pregunta es la misma en todas y la respuesta también.

La etiqueta inexistente sale **3** y no 2: la invocación está bien formada y el recurso no está, que es exactamente lo que el 3 significa.

**La colisión de etiqueta y la de nombre de voz son el mismo hecho** —el recurso está ocupado y hay que liberarlo o forzar— y comparten código. Con el almacén etiquetado, la colisión no es un caso esporádico: ocurre cada vez que se regenera una locución ya existente, que es flujo normal de trabajo.

#### Ningún flag queda sin efecto sin que la CLI lo diga

La afirmación vale con una excepción declarada: **`--force` sobre una etiqueta libre es un no-op**, igual que `voice clone --force` sobre un nombre libre. Fuera de ese caso, toda combinación de flags tiene efecto declarado o sale con 2, 3 o 6.

## 7. Matrices de comportamiento

#### `speech synthesize`

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

#### El resto del grupo

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

#### Qué añade `--json` a las matrices

`--json` no cambia ninguna fila de éxito: el comando hace lo mismo y además emite su payload por stdout. **Bajo `--json`, toda salida no-cero de las tablas anteriores emite además el payload de error** con su `code` y su `message`. El fallo tiene forma observable, y por tanto verificable, en cada fila.

La única interacción entre `--json` y el comportamiento es la regla 2: `--json` con `--play` es exit 2, así que bajo `--json` el bucle es inalcanzable y **la persistencia de `synthesize` es cierta** siempre que la salida sea 0.

## 8. El almacén de habla sintética

#### Ubicación y layout

`<data_dir>/speech/<voz>/<etiqueta>.wav` (`crates/avi-store/src/lib.rs` `SpeechStore`), **raíz hermana de `voices/`** (`VoiceStore`: `<data_dir>/voices/<nombre>/`; caché HF: `hf_cache_dir()`).

**Por qué no anidado en `voices/<voz>/speech/`**, que sería la opción intuitiva y ahorraría código de borrado: las voces de fábrica (`default` —clonada con `reference.qvoice`—, `ryan` y `vivian` —presets puros sin referencia—; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) son entradas del registro `VoiceStore` —la resolución preset/clonada (`avi-tts/src/lib.rs` `resolve_voice_motor`, presencia de `reference.qvoice`) es detalle interno no-normativo— y el almacén separa la salida generada (`speech/`) del registro (`voices/`).

Coste aceptado de la raíz separada: el arrastre de las locuciones al borrar una voz no es gratis y exige código explícito.

El almacén lo escribe y lo lee **solo el cliente**: es salida de síntesis y el daemon jamás lo toca.

#### El `.wav` es el recurso de registro

Cada locución son dos archivos, y **el `.wav` manda**. El `.json` son metadatos derivados.

| Pregunta | La decide |
|---|---|
| ¿La etiqueta existe? | El WAV |
| ¿Hay colisión (exit 6)? | El WAV |
| ¿`speech play` / `speech remove` salen 3? | El WAV |
| ¿Qué enumera `speech list`? | Los WAV |

**`speech remove` borra ambos archivos si están**, de modo que un sidecar huérfano sea removible por su etiqueta aunque `speech list` no lo muestre.

#### El sidecar de metadatos

Junto a cada `<etiqueta>.wav` se escribe `<etiqueta>.json` con tres campos: `text`, `voice` y `created_at`. Sin él las etiquetas son opacas: pasadas unas semanas, `saludo2` no le dice nada a nadie.

- **`created_at` en ISO 8601 UTC.**
- **El sidecar es formato interno y no lleva versión de esquema propia.** Su única superficie estable es el payload `--json`, gobernado por `schema_version="3"` (`src/main.rs` / `crates/avi-core/src/json_emitter.rs`). Darle versión propia daría al proyecto tres versiones de esquema donde hay dos.
- **Un lector que encuentre un campo desconocido lo ignora**, igual que hacen los modelos del protocolo IPC con `extra="ignore"`.
- **`speech list` tolera un sidecar ausente** mostrando la locución sin metadatos, en vez de fallar. Muestra el texto **truncado** en la salida humana y **completo** en el payload `--json`.

#### Atomicidad de la escritura

Cada archivo se escribe a un temporal en el mismo directorio y se publica con `rename` atómico, de modo que una interrupción no deje un WAV truncado que `speech list` mostraría como válido y `speech play` intentaría reproducir.

**El sidecar se publica antes del WAV**, así que la aparición del `.wav` implica que sus metadatos ya están completos. Combinado con que el WAV es el recurso de registro, una interrupción entre ambos `rename` deja basura inocua: el sidecar huérfano no ocupa la etiqueta, y `speech remove` lo alcanza.

#### Validación de identificadores

La etiqueta y el nombre de voz son la misma clase de identificador: un segmento de ruta. Los valida **`crates/avi-store/src/lib.rs` `VoiceStore::validate_name`** (validador único parametrizado por `kind="voz" | "etiqueta"`), que `VoiceStore` y `SpeechStore` invocan en vez de duplicar la regla.

- **El parámetro `kind` determina el sustantivo del mensaje** —«Nombre de voz inválido» frente a «Nombre de etiqueta inválido»—, de modo que `speech synthesize --label "mi saludo"` no culpe a `--voice`. Sin eso, el mensaje de error más frecuente del flag más usado apuntaría a otra cosa.
- **Las etiquetas se normalizan a minúsculas**, porque el validador lo hace deliberadamente para evitar colisiones en filesystems case-insensitive. `--label Saludo` y `--label saludo` son la misma etiqueta, y el archivo se llama `saludo.wav`. Se declara en el help de `--label` y en `USAGE.md`.
- **La defensa anti-escape por `realpath`** corre sobre **ambos** segmentos.
- Un identificador ilegal sale con **2**, sea voz o etiqueta.

## 9. Los códigos de salida

#### La tabla

| Código | Constante | Significado |
|---|---|---|
| `0` | `EXIT_OK` | Éxito |
| `1` | `EXIT_ERROR` | Error genérico |
| `2` | `EXIT_INVALID_INPUT` | Uso incorrecto: la invocación está mal formada |
| `3` | `EXIT_NOT_FOUND` | El recurso nombrado no existe |
| `4` | `EXIT_MODEL_MISSING` | Modelo no provisionado |
| `5` | `EXIT_DAEMON_UNREACHABLE` | Daemon inalcanzable |
| `6` | `EXIT_STATE_CONFLICT` | El recurso existe o está ocupado; la operación no procede sin liberarlo o forzarla |
| `7` | `EXIT_NOT_APPLICABLE` | La operación no aplica a este objetivo o entorno, y no aplicará reintentando |
| `8` | `EXIT_PRECONDITION_FAILED` | Una precondición del entorno no se cumple; el remedio está fuera del programa y la operación es reintentable una vez corregida |
| `9` | `EXIT_TRANSLATION_FAILED` | El pipeline de traducción falló con el modelo ya cargado |
| `10` | `EXIT_TRANSCRIPTION_FAILED` | El pipeline de transcripción falló con el modelo ya cargado |
| `130` | `EXIT_INTERRUPTED` | Interrupción del usuario |

#### Cómo se reparten los enteros

La tabla se deriva del eje de dos preguntas. La segunda es la que reparte los enteros:

| Código | Clase de causa | Siguiente llamada del consumidor |
|---|---|---|
| **1** | Imprevisto | Reintentar a ciegas, registrar o escalar |
| **2** | Invocación mal formada | Corregir el comando y reintentar |
| **3** | Recurso ausente | Crearlo, o nombrar otro |
| **4** | Precondición de entorno: el modelo | `ai-voice-interconnector setup`, luego el mismo comando |
| **5** | Precondición de entorno: el daemon | `ai-voice-interconnector daemon start`, luego el mismo comando |
| **6** | Recurso ocupado | `--force`, otro nombre, `daemon stop`, o esperar a que se libere |
| **7** | Imposibilidad permanente | **Ninguna** — no reintentar nunca |
| **8** | Precondición de entorno: el resto | Ninguna propia: delegar y reintentar el mismo comando |
| **9** | Imprevisto, pero en la etapa de traducción | Distinguirlo del fallo de síntesis (**1**) es lo que cambia la siguiente llamada: el modelo TTS puede seguir intentándose sin traducir |

**Los dos casos límite son inversos, y esa simetría es lo que valida el criterio.** El 4, el 5 y el 8 son **una** clase por causa —modelo ausente, daemon caído, disco lleno y token vencido son el mismo tipo de hecho— repartida en **tres** enteros, porque lo único que un consumidor puede convertir en una llamada distinta es un comando de esta CLI: `setup` y `daemon start` se separan y el resto colapsa en el 8. El 6 es lo contrario: **tres** remedios de naturaleza distinta (`--force`, `daemon stop`, cerrar un proceso externo) plegados en **un** entero, porque ninguno cambia lo que el consumidor distingue —«ocupado» frente a «ausente» y «mal escrito»—. La resolución del entero es la de lo que este programa puede nombrar como paso ejecutable.

**El 1 y el 7 no son vecinos**: en el 1 no se conoce remedio; en el 7 se sabe que no lo hay. Fundirlos borraría la única señal que importa, que es *no reintentar*.

**El 6 tiene un solo dueño.** «Puerto ya en uso» y «la voz ya existe» son el mismo hecho y llevan el mismo código; no hay una constante aparte para el conflicto del daemon.

#### El 2 significa lo que `clap` quiere decir con él

El exit 2 es, en Unix y en `clap`, el código del error de invocación, y aquí significa exactamente eso. Como consecuencia, **todas las rutas de fallo de parseo son correctas sin escribir una línea de validación**: flag requerido ausente, valor fuera de `choices`, grupo mutuamente excluyente violado (`conflicts_with`), subcomando inválido en los tres niveles, y flag desconocido en cualquier comando.

**Ausente = exploración (0), inválido = error (2).** `ai-voice-interconnector` a secas y `ai-voice-interconnector speech` a secas no son un error: imprimen la ayuda y salen con `EXIT_OK`, igual que `--help`, porque una invocación sin subcomando es exploratoria. La regla no es «ausente o inválido → 2».

Dos pruebas de que la convención es la correcta:

1. **La tabla la honra en otro punto**: `EXIT_INTERRUPTED = 130` es exactamente `128 + SIGINT`. Respetar 128+n y no respetar 2 sería incoherente dentro de la misma tabla.
2. **El proyecto hermano aplica la misma convención**: `tts-sidecar-narrator` usa **2 = uso incorrecto** en sus tres casos —valor fuera de dominio, argumento vacío y comando desconocido— con **1 = error genérico**.

#### Dónde viven las constantes, y por qué eso es parte del contrato

**Las constantes viven en `crates/avi-core/src/exit_codes.rs` (`ExitCode`), sin dependencias circulares.** Un crate hoja sin imports del binario **no puede** cerrar un ciclo, así que la justificación que empujaría una constante a declararse fuera del módulo no está disponible ni siquiera como pretexto. `crates/avi-core/src/json_emitter.rs` (`emit_raw_json`) y `src/main.rs` (`Cli::parse`, `handle_*`) reexportan el contrato, de modo que `ExitCode::InvalidInput` es el nombre canónico.

**Un contrato cerrado sin un lugar legítimo donde crecer no impide el crecimiento: lo empuja fuera del campo de visión.** El dueño es el crate `avi-core`, no una advertencia.

**Dos invariantes de gobernanza lo sostienen**, y son distintos:

1. **Ningún `ExitCode` puede definirse fuera de `crates/avi-core/src/exit_codes.rs`.** Un test recorre los crates y falla ante una definición con ese prefijo en cualquier otro archivo.
2. **La tabla de `USAGE.md` y el módulo dicen lo mismo.** Compara los pares valor/variante con las filas de la tabla pública. Un código declarado por fuera y además sin documentar es invisible dos veces.

La reexportación desde `src/main.rs` crea dos sitios donde *parecen* vivir las constantes; el primer invariante lo desactiva —cualquier definición fuera del crate hoja falla—, así que la reexportación es un alias y no una segunda declaración. La distinción queda escrita en el crate.

**El comentario del crate** enuncia el criterio generador en sus dos tiempos —clase de causa y admisión por la siguiente llamada del consumidor—, fecha el congelamiento de la tabla **en la 1.0**, advierte que un intercambio de valores es indetectable para un consumidor, y recoge el criterio de revisión que no puede ser test. La versión del esquema es `schema_version="3"` (`src/main.rs` / `crates/avi-core/src/json_emitter.rs`).

**Dos reglas transversales, y solo una es mecanizable.**

- **Test**: ningún `ExitCode::Error` puede alcanzarse por una causa prevista con remedio declarado en su propio mensaje. Un `EXIT_ERROR` cuyo mensaje contenga «reintenta» es por construcción un olvido.
- **Criterio de revisión, no test**: ningún `EXIT_INVALID_INPUT` puede alcanzarse con una invocación bien formada. «Bien formada» no tiene definición ejecutable, y escribirla como test produciría una aserción que no afirma nada. Su lugar es el comentario del módulo, junto al criterio generador.

## 10. El canal de error y los payloads

#### La invariante del canal

**Bajo `--json`, toda salida no-cero emite el payload de error, salvo la salida por veredicto.** `code` y `message` son obligatorios; `reason` es opcional en cualquier código y se define donde la distinción **ya existe calculada** en el código.

El canal tiene **tres formatos**, y cada invocación emite **exactamente un objeto JSON**:

1. **Éxito**: el payload propio del comando, vía `emit_json()`, con salida 0.
2. **Error**: el objeto `{"error": {…}}`, vía `CliError` traducido por `main()`, con salida ≠ 0.
3. **Veredicto**: código ≠ 0 con el payload **propio** del comando ya emitido y **sin** objeto `error`. Es un dictamen, no un fallo: el comando corrió sin error pero su resultado es negativo. El único caso es **`doctor`**, cuyo exit 1 con FAIL (§9) emite solo el reporte (`checks`, `failed`) y sale con 1.

El payload de error usa una clave de primer nivel `error`, emitida solo bajo `--json`, y deja intacto el stderr en castellano para el uso humano:

```json
{"schema_version": "3", "error": {"code": 8, "reason": "disk_full", "message": "…"}}
```

El único código con `reason` poblado es el **8**: la clasificación de por qué falló la provisión —dependencia del runtime ausente, credenciales, red, permisos y disco lleno— ya se calcula, y `reason` es el nombre estable de esa distinción. El 6 y el 7 agrupan subcausas sin nombrar; añadírselas más adelante es aditivo. El fallo de parseo lleva `reason: "usage_error"`.

Las tres reglas de compatibilidad y la regla de promoción son contrato **de consumo** además de emisión: `USAGE.md` declara explícitamente que un `reason` desconocido se trata como ausente.

#### El mecanismo: un solo punto de traducción

**La invariante no se sostiene con un `if` por sitio**, porque eso la deja en manos de que nadie olvide uno. Es la misma solución que la ruta de éxito ya tiene con `emit_raw_json` (`crates/avi-core/src/json_emitter.rs`), cuyo doc enuncia el motivo: *«la garantía queda en un solo lugar, no repetida por convención en cada comando»*. La ruta de fallo tiene la misma forma:

- Los sitios de fallo retornan **`ExitCode` + `reason` + `message`** (tipo `CliError` en `crates/avi-core/src/exit_codes.rs` / `crates/avi-core/src/json_emitter.rs`) en vez de imprimir y salir.
- **`src/main.rs` (`main` / `handle_*` / `emit_raw_json`) es el único punto que lo traduce**: mensaje humano a stderr, payload a stdout si se pidió `--json`, y salida con el código. No queda otro camino hasta la salida, así que la invariante no necesita vigilancia.
- El invariante que la protege es mecanizable: **ninguna salida no-cero fuera de `src/main.rs`**.

**La salida por veredicto entra por el mismo punto único.** Un comando que ya emitió su payload propio y quiere salir con código ≠ 0 sin adjuntar objeto `error` **devuelve el entero** del código; `src/main.rs` honra un retorno `ExitCode` ≠ 0 y sale con ese código. No hay tipo de error nuevo disperso: la salida sigue pasando por `main`, así que **ninguna salida no-cero fuera de `src/main.rs`** se mantiene. `doctor` es el caso que lo usa: emite su reporte y retorna `ExitCode::Error` cuando hay FAIL.

**`CliError` es señal de control de flujo, no error de dominio.** Una señal de flujo no debe ser capturable por un manejador genérico; en Rust se modela como tipo propio en `avi-core` que `src/main.rs` traduce, sin propagación silenciosa por handlers genéricos. Un test afirma la separación entre error de dominio y señal de salida.

**El fallo de parseo entra por el mismo canal.** `src/main.rs` (`Cli::parse` con `clap`) traduce el error de parseo a `ExitCode::InvalidInput` (`"usage_error"`) en vez de imprimir y salir: así el texto que `clap` ya calcula entra al payload en vez de perderse, y el 2 —el fallo más frecuente que verá un consumidor programado— deja stdout tan poblado como cualquier otro. `Cli::parse` corre dentro del mismo handler. Queda un residuo honesto: al fallar el parseo no existe `Cli`, así que hay que inspeccionar los args crudos para saber si se pidió `--json`; decide *si* emitir, no qué, y vive en un único sitio.

**El render deja pasar intacto el exit 0.** `--help` sale por esa vía sin pasar nunca por error, así que un handler que no discrimine por código emitiría payload de error en la invocación más común de toda la CLI. Es el único caso, y tiene test de regresión propio.

**`daemon serve` queda fuera del mecanismo, y por una razón concreta: no acepta `--json`.** No hay payload que emitir, así que la invariante del canal no tiene alcance ahí y ese comando sale directamente. Esa es la condición que lo autoriza y ninguna otra: darle `--json` reabriría el hueco.

#### Los cinco payloads del grupo `speech`

Ninguno emite ruta, por el criterio de la ruta en los payloads. Todos llevan además los campos transversales del sobre.

| Sub-acción | Payload |
|---|---|
| `speech synthesize` | `{"status":"success", "audio_path", "voice"}` |
| `speech say` | `{"status":"reproduced", "audio_path", "voice"}` |
| `speech list` | `{"speech": [{"voice", "label", "text", "created_at", "duration_secs"}]}` |
| `speech play` | `{"status":"played", "voice", "label"}` |
| `speech remove` | `{"status":"removed", "voice", "label"}` |

- **`synthesize`** persiste y emite `audio_path` (ruta del WAV en el almacén) y `voice`; `label` va implícito en `audio_path`. Bajo `--json` el bucle es inalcanzable y la persistencia es cierta cuando la salida es 0.
- **`say`** no persiste: emite `audio_path` temporal y `voice`; no repite `text`.
- **Ambos comparten `voice`** porque si no se pasó `--voice` la eligió el sistema (`default`).
- **`list`** emite el texto completo. La clave es el nombre del recurso en snake_case, siguiendo el precedente de `voice list --json`, que emite `{"voices": [...]}` — y evitando que un identificador del contrato legible por máquina contradiga el vocabulario de la superficie.
- **`remove`** no lleva campo de resultado: el código de salida ya transporta la información (0 = se borró, 3 = no existía). Un campo `removed` chocaría además con `cleanup --json`, que emite `removed` como lista de rutas, y la misma clave con dos tipos bajo una sola versión de esquema es justo lo que un consumidor tipado no puede manejar.

Los payloads de `daemon start`, `stop` y `restart` no llevan clave booleana propia: el fallo se reporta por el payload de error como en el resto de la CLI.

#### Las dos versiones de esquema

Son **dos, independientes**, y ambas valen `"3"`:

- **`crates/avi-daemon/src/lib.rs` (`DaemonState`, `run_daemon_server`) — protocolo IPC del daemon.** Subió a `"2"` porque `/synthesize` identifica la voz por su nombre y no transporta rutas: una forma que no es aditiva y por tanto exige versión propia. Subió otra vez a `"3"` con el rediseño cross-lingual: `model_loaded` pasó de `bool` a `dict[str, bool]` (un modelo cargado por idioma en vez de uno solo), un cambio incompatible de un campo existente (`crates/avi-core/src/engine.rs` `SttEngine`/`TtsEngine`, estados `warm`/`warm_failed`).
- **`src/main.rs` / `crates/avi-core/src/json_emitter.rs` (`schema_version="3"`) — payloads `--json` de la CLI.** Subió a `"2"` porque el payload de síntesis no lleva clave de ruta de salida. Subió otra vez a `"3"` por la misma razón que el protocolo del daemon: `daemon status --json` refleja el mismo cambio de `model_loaded` de booleano a objeto por idioma.

Son dos causas independientes que coinciden en el mismo hecho generador. Los payloads del grupo `speech` no influyen en ninguna: añadir subcomandos es aditivo, y añadir la clave `error` también lo es.

**La política de compatibilidad es la misma en ambas**: añadir claves no incrementa la versión; solo lo hace un cambio incompatible de las existentes.

## 11. `cleanup`, `setup` y `voice`

#### `cleanup`

| Modo | Qué borra |
|---|---|
| `--synthetic-speech` | La raíz `synthetic-speech/` entera |
| `--voices` | Las voces que puede borrar y, **con ellas, solo los namespaces de habla sintética de esas voces** |
| `--all` | Modelo + voces + habla sintética |
| `--dry-run` | Cubre las locuciones en los tres modos anteriores |

**`synthetic-speech/default/` (y `ryan`/`vivian`) sobrevive a `--voices` y cae únicamente con `--synthetic-speech` o `--all`.** El criterio es el del propio flag —las locuciones se van con su voz— y las voces de fábrica (`default`, `ryan`, `vivian`; `crates/avi-store/src/lib.rs` `FACTORY_VOICES`) no se van nunca: `voice remove` las protege (exit 2) y `--voices` no las borra. Importa declararlo porque `default` es la voz por defecto de `speech synthesize` y su namespace es probablemente el más poblado.

`--all` incluye la habla sintética por necesidad: si no la incluyera dejaría residuo tras una desinstalación completa, que es justo lo que ese flag existe para evitar.

Con la raíz separada del registro de voces, el arrastre de `--voices` es código explícito y no un efecto del `rmtree`.

#### `setup`

El chequeo de audio degrada a WARN en vez de FAIL, **con la premisa que lo sostiene**: el sidecar es instalable en hosts headless, SSH y CI porque existe un sumidero que no necesita subsistema de sonido —`speech synthesize --text T --label L` sintetiza y persiste sin reproducir nada—. `setup` es provisión, no diagnóstico.

**`--with-stt`** provisiona el modelo de transcripción (`parakeet-tdt-0.6b-v3` int8, runtime `ort` load-dynamic `crates/avi-stt/src/parakeet.rs:1-10` vía `ParakeetEngine`). Es **opt-in** (no se descarga por defecto) y **ortogonal a `--language`**: no cuelga de la taxonomía de idioma porque el modelo Parakeet no está partido por par de idiomas — un solo modelo cubre `es`/`en`. `setup --with-stt` sin más flags provisiona únicamente el modelo de transcripción; se combina libremente con `--language` para provisionar ambos en la misma invocación.

#### `voice`

- **`voice clone` toma `--timbre-reference/-t` (opcional) y `--speech-reference/-s`** (obligatorio, ≥10s, validado en runtime), y los archivos en disco se llaman `timbre-reference.wav` y `speech-reference.wav`. Sin `--timbre-reference`, el habla cubre también el Voice Encoder. Internamente el timbre es un solo nombre: `timbre`.
- **`voice clone` recibe el despacho al daemon en sus tres modos**, porque precomputa los conditionals de la voz al clonarla y necesita el modelo cargado igual que las dos sub-acciones que sintetizan.
- **`voice clone` sobre un nombre tomado sin `--force` sale con 6**, y sobre un nombre libre `--force` es un no-op declarado.
- `VoiceStore` (`crates/avi-store/src/lib.rs`) reconoce una voz clonada por `reference.qvoice` (o `speech-reference.wav` legado); `timbre-reference.wav` es legado. Las voces de fábrica son `default` (clonada, con `reference.qvoice` graft) y `ryan`/`vivian` (presets puros sin referencia); `voice remove` las protege a las tres (exit 2; `crates/avi-store/src/lib.rs:16` `FACTORY_VOICES`, `src/main.rs:606-621` `is_factory_name`).
- `voice list` muestra las tres de fábrica (`is_factory=true`) más las clonadas del usuario; `voice remove` rechaza las de fábrica con exit 2.
- En `voice list` y `voice remove`, `-n` es `--name`.

## 12. Contratos externos

#### El integrador de narración

**`speech say --text "<msg>" --daemon`** es la invocación que sintetiza y reproduce, y es el contrato del integrador de narración: exige el daemon porque su presupuesto de latencia no admite cargar el modelo al vuelo. No hay alias de compatibilidad; esa es la única forma de la invocación.

El integrador que quiera además conservar el audio usa `speech synthesize --text "<msg>" --label L`, que no reproduce.

#### La frontera del daemon

`/synthesize` recibe `voice: str`. No hay lista de directorios de audio permitidos, ni validación de rutas de audio, ni directorio de sesión del daemon, porque no hay rutas que validar.

**Riesgo conocido y declarado**: `data_dir()` / `hf_cache_dir()` (`crates/avi-store/src/lib.rs`) depende de `LOCALAPPDATA` / `XDG_DATA_HOME`, así que un daemon y un cliente arrancados con entornos distintos responden «voz no encontrada» para una voz que el cliente sí lista. Está atenuado porque `/voices` permite inspeccionar la vista del daemon.

## 13. El comando `translate`, `speech transcribe`, `speech dub` y la síntesis cross-lingual

#### `translate`: texto→texto, aislado de la síntesis

`translate` no pertenece a ningún grupo nominal: es texto→texto, sin voz ni modelo TTS de por medio. `--from {es, en}` y `--to {es, en}` son **ambos requeridos** — a diferencia de los flags opcionales de `speech`, aquí traducir es la única función del comando, así que no hay default que lo excuse. `--from == --to` es *passthrough*: devuelve el texto sin cargar el modelo.

| Parámetros | Payload `--json` |
|---|---|
| `--text` **requerido** (sin alias `-t`, a diferencia de `speech`) · `--from` **requerido** · `--to` **requerido** · `--json` | `{"translated", "source", "target"}` |

#### Traducción opt-in en `speech say`/`speech synthesize`

`--source-language`/`--target-language` (§3) insertan una etapa de traducción **antes** de la síntesis cuando declaran idiomas distintos; el motor de síntesis no cambia, solo recibe el texto ya traducido. El modelo de traducción ausente reutiliza el exit **4** (`EXIT_MODEL_MISSING`), remitiendo a `setup`, en vez de un código propio: es la misma precondición de entorno que el modelo TTS. Un fallo de la inferencia de traducción, con el modelo ya cargado, sale con **9** (`EXIT_TRANSLATION_FAILED`, §9) — código distinto del **1** genérico de síntesis, porque distingue en qué etapa falló la invocación.

#### El rename `--language` → `--target-language`

**Cambio incompatible y deliberado, sin alias de transición.** `speech say`/`speech synthesize` reemplazan `--language` por `--target-language`; `setup`, `daemon` y `doctor` **conservan** `--language`, porque ahí no hay ambigüedad origen/destino (la provisión no traduce). El integrador de narración (§12) no se ve afectado: sus invocaciones nunca pasan `--language` en `speech say`, así que el rename no le rompe ningún flag en uso, aunque sí es parte del mismo contrato versionado.

#### Provisión y daemon

`setup` descarga `Marian` (`opus-mt-es-en`/`en-es`) y convierte incondicionalmente su derivado obligatorio `CT2` `INT8` en `hf_cache_dir/ct2/opus-mt-{es-en,en-es}/model.bin` (`crates/avi-store/src/lib.rs:ct2_model_dir`, idempotente por `mtime`); `doctor` exige `model.bin` para ambas direcciones (Falla con `CT2 es→en/en→es no provisionado` si `Marian HF` está pero `CT2` no); `cleanup` purga `hf_cache_dir/ct2` junto a `hub/xet`. Sin gating por `--language`: la provisión es determinista para la instalación por defecto.

#### `speech transcribe`: audio→texto, verificable sin traducir ni sintetizar

`speech transcribe` es una **sub-acción del grupo `speech`** (no un comando aislado como `translate`): transcribe a texto con `parakeet-tdt-0.6b-v3` int8 vía `ort` load-dynamic (`crates/avi-stt/src/parakeet.rs:1-10`, `ParakeetEngine::transcribe`), desde un archivo WAV (`--audio`) o desde el micrófono (`--mic`). La **captura corre siempre en el cliente** (al daemon viajan las muestras ya decodificadas en base64, nunca rutas); la transcripción en sí recibe el despacho al daemon en sus tres modos (§5): sin flags, transcribe por el daemon si está activo y en modo directo si no; `--daemon` exige el daemon y sale con **5** si no está; `--no-daemon` fuerza el modo directo. Es una operación **de un solo idioma por invocación** — a diferencia del par `--from`/`--to` de `translate`, aquí `--source-language` (requerido, `es-latam`/`en`, misma taxonomía que `speech say`/`synthesize`) declara el único idioma hablado en el audio. `ParakeetEngine` **solo transcribe**, nunca traduce: si el usuario necesita el texto en otro idioma, encadena `translate` por separado. No hay síntesis de por medio: `speech transcribe` es verificable de forma aislada, con audio de entrada y texto de salida, sin depender del motor TTS ni del subsistema de traducción.

`--audio` y `--mic` forman un **grupo mutuamente excluyente `required=True`** (el mismo en `speech dub`), el único tipo de grupo excluyente del árbol de parsers que exige uno de sus flags: los demás — `--daemon`/`--no-daemon` en `speech synthesize`/`speech say`/`voice clone`/`speech transcribe`/`speech dub`, y el grupo de `setup` — son opcionales, sin exigir ninguno de los dos. Con `--mic`, la captura es **push-to-talk** por defecto (Enter para terminar); `--duration N` fuerza una grabación de duración fija en segundos y solo es válido junto a `--mic` — con `--audio`, o con `--mic` ausente, `--duration` sale con **2** (`EXIT_INVALID_INPUT`). Sin terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con **2**, porque no hay forma de detectar la pulsación de Enter que cierra el push-to-talk. La captura llega ya a 16 kHz/mono/int16 (formato que `ParakeetEngine` asume) y no pasa por remuestreo; el backend de captura es `miniaudio` (único, sin ramas por sistema operativo) y la inferencia es `ort` load-dynamic con los 4 artefactos `MODEL_FILE_PATTERNS` (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), stack Rust nativo.

| Parámetros | Payload `--json` |
|---|---|
| `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** (`es-latam`\|`en`) · `--daemon`/`--no-daemon` · `--json` | `{"text", "source"}` |

El shape `--json` no cambia con el despacho: emite `{"text", "source"}` en los tres modos, sin campo `daemon`.

Si el modelo `parakeet-tdt-0.6b-v3` no está provisionado, sale con **4** (`EXIT_MODEL_MISSING`), remitiendo a `ai-voice-interconnector setup --with-stt`; un fallo de la inferencia con el modelo ya cargado sale con **10** (`EXIT_TRANSCRIPTION_FAILED`, §9) — mismo criterio de asignación que distingue **4** de **9** en `translate`. Un `--audio` inexistente sale con **3** (`EXIT_NOT_FOUND`); en la ruta daemon, un fallo de comunicación —daemon inactivo o de versión antigua sin `/transcribe` (404, skew de `schema_version` sin bump)— sale con **5** (`EXIT_DAEMON_UNREACHABLE`), sin degradación silenciosa a modo directo.

**Divergencia deliberada del shape `--json` frente a `translate` (D5).** `translate --json` emite `source`/`target` como los códigos **ISO crudos** que recibieron `--from`/`--to` (`es`, `en`): ahí el ISO es exacto porque el parámetro mismo está restringido a `choices=["es","en"]`. `speech transcribe --json`, en cambio, emite `source` como el **token CLI verbatim** de `--source-language` (p. ej. `es-latam`, sin resolver a `es`) — no lo normaliza. La razón es de simetría con el resto de `speech`: `speech say`/`synthesize` aceptan y exponen `es-latam` en su propia taxonomía de idioma (nunca lo colapsan a ISO de cara al usuario), y `speech transcribe` es una sub-acción de ese mismo grupo, no un primo de `translate`. Colapsar `source` a ISO ahí introduciría una inconsistencia dentro del propio grupo `speech` a cambio de una consistencia superficial con un comando de otro grupo. La resolución a ISO (`resolve_language`) sigue ocurriendo internamente para seleccionar el idioma que Whisper recibe; solo la salida `--json` preserva el token de entrada.

#### `speech dub`: el bucle voz→voz en un comando dedicado

`speech dub` es la **composición voz→voz**: transcribe la entrada hablada (archivo o micrófono), traduce el texto si `--source-language` difiere de `--target-language`, sintetiza con la voz elegida y reproduce el resultado. Reutiliza las máquinas existentes —`_transcribe_stage` (los tres modos de `speech transcribe`), la traducción opt-in de `say`/`synthesize` y el despacho de síntesis (§5)— sin modificarlas. `say`/`synthesize` **no cambian**: siguen siendo texto→voz con `--text` requerido; la entrada de audio del bucle vive solo en `dub`. No persiste nada: no declara `--label` ni `--json`.

Exige **exactamente una** de `{--audio, --mic}` (grupo mutuamente excluyente `required=True`, espejo de `speech transcribe`); `--duration N` solo es válido con `--mic` (con `--audio` sale con **2**); `--source-language` es **requerido** (`es-latam`/`en`); `--target-language` (default `es-latam`) elige el idioma/modelo de síntesis y dispara la traducción si difiere del hablado; `-v/--voice`, `--compute-backend/-cb`, `--exaggeration`, `--cfg-weight` y `--temperature` son los de `say`. `--daemon`/`--no-daemon` aplican a la transcripción y a la síntesis (los tres modos, §5); sin flags, ambas etapas usan el daemon solo si responde.

| Parámetros | Comportamiento |
|---|---|
| `--audio` **o** `--mic` (mutuamente excluyentes, uno requerido) · `--duration N` (solo con `--mic`) · `--source-language` **requerido** · `--target-language` (default `es-latam`) · `-v/--voice` · `--compute-backend/-cb` · `--exaggeration` · `--cfg-weight` · `--temperature` · `--daemon`/`--no-daemon` | transcribe → traduce (si `source != target`) → sintetiza → reproduce |

Códigos de salida aplicables en la cadena: **4** (`EXIT_MODEL_MISSING`, modelo de transcripción no provisionado, remite a `setup --with-stt`), **5** (`EXIT_DAEMON_UNREACHABLE`, daemon exigido pero inactivo o de versión antigua sin `/transcribe`), **9** (`EXIT_TRANSLATION_FAILED`, fallo del pipeline de traducción con el modelo cargado) y **10** (`EXIT_TRANSCRIPTION_FAILED`, fallo del pipeline de transcripción con el modelo cargado); más **2** (uso inválido: `--duration` sin `--mic`, `--mic` sin TTY y sin `--duration`) y **3** (`--audio` inexistente).

---

## Documentación complementaria por comando

Cada comando principal de la CLI tiene un documento de investigación dedicado en [`commands/`](commands/) que cubre su diseño, implementación, flujo de ejecución y manejo de errores con citas a líneas del código fuente.

| Comando | Documento | Subcomandos |
|---|---|---|
| `speech` | [`commands/SPEECH.md`](commands/SPEECH.md) | `synthesize`, `say`, `dub`, `play`, `list`, `remove`, `transcribe` |
| `voice` | [`commands/VOICE.md`](commands/VOICE.md) | `list`, `clone`, `remove` |
| `devices` | [`commands/DEVICES.md`](commands/DEVICES.md) | — |
| `doctor` | [`commands/DOCTOR.md`](commands/DOCTOR.md) | — |
| `setup` | [`commands/SETUP.md`](commands/SETUP.md) | — |
| `cleanup` | [`commands/CLEANUP.md`](commands/CLEANUP.md) | — |
| `daemon` | [`commands/DAEMON.md`](commands/DAEMON.md) | `start`, `stop`, `restart`, `status`, `serve` |
| `version` | [`commands/VERSION.md`](commands/VERSION.md) | — |
| `translate` | [`commands/TRANSLATE.md`](commands/TRANSLATE.md) | — |
