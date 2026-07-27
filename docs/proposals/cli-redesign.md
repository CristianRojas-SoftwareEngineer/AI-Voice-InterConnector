# Rediseño de la CLI: el grupo `speech` y el contrato de salida

**Estado**: propuesta, sin implementar. Ninguna decisión de este documento está en el código.
**Alcance**: contrato público de la CLI (comandos, flags, códigos de salida, payloads `--json`) y el almacén de habla sintética.
**Base**: commit `26735cd`, el último que toca `src/`. Todo lo descrito en la sección 1 está verificado contra el árbol de trabajo.
**Sustituye a**: `generate-speech-redesign.md`, que se conserva solo por trazabilidad. Este documento es la fuente única; el anterior no debe consultarse para saber qué va a existir.

---

## Tabla de contenidos

- [1. Estado actual](#1-estado-actual)
  - [1.1. Superficie de comandos](#11-superficie-de-comandos)
  - [1.2. `speak` en detalle](#12-speak-en-detalle)
  - [1.3. El contrato de salida](#13-el-contrato-de-salida)
  - [1.4. Los canales legibles por máquina](#14-los-canales-legibles-por-máquina)
  - [1.5. El despacho al daemon](#15-el-despacho-al-daemon)
  - [1.6. El almacén de voces, el sandbox y el vocabulario](#16-el-almacén-de-voces-el-sandbox-y-el-vocabulario)
- [2. Estado objetivo](#2-estado-objetivo)
  - [2.1. Invariantes y criterios generadores](#21-invariantes-y-criterios-generadores)
  - [2.2. La superficie y el vocabulario](#22-la-superficie-y-el-vocabulario)
  - [2.3. El grupo `speech`](#23-el-grupo-speech)
  - [2.4. Síntesis y el bucle de `--play`](#24-síntesis-y-el-bucle-de---play)
  - [2.5. El despacho al daemon](#25-el-despacho-al-daemon)
  - [2.6. Reglas de validación](#26-reglas-de-validación)
  - [2.7. Matrices de comportamiento](#27-matrices-de-comportamiento)
  - [2.8. El almacén de habla sintética](#28-el-almacén-de-habla-sintética)
  - [2.9. Los códigos de salida](#29-los-códigos-de-salida)
  - [2.10. El canal de error y los payloads](#210-el-canal-de-error-y-los-payloads)
  - [2.11. `cleanup`, `setup` y `voice`](#211-cleanup-setup-y-voice)
  - [2.12. Contratos externos](#212-contratos-externos)
- [3. El puente](#3-el-puente)
  - [3.1. El orden y por qué](#31-el-orden-y-por-qué)
  - [3.2. Movimiento 1 — limpieza](#32-movimiento-1--limpieza)
  - [3.3. Movimiento 2 — el contrato de salida](#33-movimiento-2--el-contrato-de-salida)
  - [3.4. Movimiento 3 — la feature](#34-movimiento-3--la-feature)
  - [3.5. Puertas de verificación](#35-puertas-de-verificación)
  - [3.6. Documentación pública](#36-documentación-pública)

---

## 1. Estado actual

Descripción del contrato que existe hoy, verificada contra el árbol de trabajo. Donde el estado actual contiene un defecto de hecho —no una preferencia de diseño— se enuncia en una línea, sin análisis.

### 1.1. Superficie de comandos

El parser se construye entero en `cli.py:1719` (`build_parser()`). Hay ocho comandos de primer nivel; dos de ellos (`voice`, `daemon`) son grupos con sub-acciones, y un grupo invocado sin sub-acción imprime su ayuda y sale 0 (`cli.py:1886`).

| Comando | Sub-acción | Propósito | `--json` |
|---|---|---|---|
| `speak` | — | Sintetiza texto y **reproduce** el audio, o lo **guarda** si se pasa `--output` | sí |
| `voice` | `list` | Lista las voces registradas (usuario + fábrica) | sí |
| | `clone` | Registra una voz desde dos audios y precomputa sus conditionals | sí |
| | `remove` | Elimina una voz de usuario | sí |
| `devices` | — | Enumera los dispositivos de salida de audio | sí |
| `doctor` | — | Ejecuta los chequeos de entorno y reporta PASS/FAIL/WARN/SKIP | sí |
| `setup` | — | Provisiona el runtime: chequeos, descarga del modelo, integración de PATH | sí |
| `cleanup` | — | Desaprovisiona datos: modelo descargado y/o voces de usuario | sí |
| `daemon` | `start` / `stop` / `restart` / `status` | Gestiona el ciclo de vida del daemon | sí |
| | `serve` | Ejecuta el servidor en primer plano (proceso servidor, no cliente) | **no** |
| `version` | — | Muestra la versión del paquete | sí |

`setup` tiene tres modos mutuamente excluyentes que cortan el flujo normal de provisión (`cli.py:1801`): `--remove-path`, `--force-update` y `--uninstall`. `cleanup` selecciona qué borrar con `--model`, `--voices` o `--all`, y admite `--dry-run` y `--yes`.

Todos los comandos declaran `--json` salvo `daemon serve`, que es el proceso servidor y no un cliente.

### 1.2. `speak` en detalle

`speak` es la única superficie de síntesis. Su declaración vive en `cli.py:1735-1760` y su implementación en `cmd_speak` (`cli.py:260`).

#### Flags

| Flag | Corto | Efecto |
|---|---|---|
| `--text` | `-t` | Texto a sintetizar. **Obligatorio** (lo exige argparse) |
| `--voice` | `-v` | Nombre de la voz; resuelve `reference.wav` + `speech.wav` del registro |
| `--output` | `-o` | Ruta del WAV de salida. Si se omite, el audio se reproduce |
| `--compute-backend` | `-cb` | `auto` (default) / `cpu` / `cuda` / `mps` |
| `--voice-audio` | — | Ruta de audio cruda para el embedding de timbre |
| `--speech-audio` | — | Ruta de audio cruda para el conditioning del T3 y el decoder S3Gen |
| `--daemon` | — | Usa el daemon sin sondeo previo; un fallo se reporta |
| `--no-daemon` | — | Fuerza modo directo, sin sondear |
| `--json` | — | Emite a stdout un payload de metadatos y métricas |

`--voice-audio` y `--speech-audio` aceptan **rutas arbitrarias del llamador**: es la única superficie del CLI que lo hace, y la que obliga a que exista una sandbox (§1.6).

#### Reglas de validación, en orden de evaluación

| # | Condición | Resultado | Sitio |
|---|---|---|---|
| 1 | `--daemon` y `--no-daemon` juntos | exit 4 | `cli.py:268` |
| 2 | `--json` sin `--output` | exit 4 | `cli.py:275` |
| 3 | `--text` vacío o solo espacios | exit 4 | `cli.py:283` |
| 4 | Texto > `MAX_TEXT_LENGTH` (5000, `daemon/protocol.py:22`) | exit 4 | `cli.py:292` |
| 5 | Texto > 2000 caracteres | **advertencia** a stderr, no aborta | `cli.py:304` |
| 6 | Modelo `es-mx-latam` no está en caché | exit 2 | `cli.py:314` |
| 7 | Un audio resuelto no existe o no termina en `.wav` | exit 3 | `cli.py:326` |
| 8 | Con `--daemon`, la ruta cae fuera de la sandbox | exit 4 | `cli.py:343` |

La regla 1 se valida a mano en vez de con `add_mutually_exclusive_group()` porque el exit 2 nativo de argparse colisionaría con `EXIT_MODEL_MISSING` (§1.3).

Sin `--voice` ni audios explícitos, `_resolve_voice_paths` (`cli.py:86`) recurre a la voz de fábrica `default`, de modo que `speak --text "Hola"` funciona sin más argumentos.

#### Matriz de comportamiento

| `--output` | `--json` | Salida de audio | stdout | Exit |
|---|---|---|---|---|
| no | no | Reproducción por el dispositivo default | vacío | 0 |
| no | sí | — | vacío | **4** |
| sí | no | Archivo WAV en la ruta dada | vacío | 0 |
| sí | sí | Archivo WAV en la ruta dada | un objeto JSON | 0 |

#### Payload `--json`

`_emit_speak_json` (`cli.py:243`) emite cinco claves más la que inyecta `emit_json`:

```json
{
  "output": "<ruta absoluta resuelta>",
  "voice": "<nombre de la voz, o \"default\">",
  "t3_time": 0.0,
  "s3gen_time": 0.0,
  "daemon": true,
  "schema_version": "1"
}
```

**Defecto de hecho**: una sola sub-acción cubre dos responsabilidades —reproducir y persistir—, y su nombre solo describe la primera.

### 1.3. El contrato de salida

El bloque de constantes vive en `cli.py:43-49`, declarado en el docstring del módulo (`cli.py:9-17`) como **contrato público congelado**.

| Código | Constante | Significado declarado |
|---|---|---|
| 0 | `EXIT_OK` | éxito |
| 1 | `EXIT_ERROR` | error genérico (incluye chequeos fallidos de `doctor`) |
| 2 | `EXIT_MODEL_MISSING` | modelo no provisionado (ejecutar `setup`) |
| 3 | `EXIT_NOT_FOUND` | voz o archivo de audio no encontrado |
| 4 | `EXIT_INVALID_INPUT` | entrada inválida (texto vacío, nombre de voz ilegal, colisión) |
| 5 | `EXIT_DAEMON_UNREACHABLE` | daemon inalcanzable o no gestionable |
| 130 | `EXIT_INTERRUPTED` | interrupción por el usuario (128 + SIGINT) |

#### Un séptimo código fuera del bloque

`daemon/run.py:33` declara `EXIT_DAEMON_PORT_IN_USE = 6`, con el comentario de que vive en el paquete `daemon` y no en `cli` para evitar un ciclo de import. Es un código del contrato que no está en la tabla del contrato: ni en el bloque de `cli.py` ni en el docstring que lo enumera.

#### La colisión del 2

argparse sale con código 2 ante cualquier error de parseo (flag desconocida, argumento obligatorio ausente, valor fuera de `choices`). El contrato asigna el 2 a «modelo no provisionado». Un orquestador que reciba un 2 no puede distinguir «falta ejecutar `setup`» de «escribí mal una flag». La colisión ya condiciona el código: la regla 1 de `speak` (§1.2) se valida a mano precisamente para no delegar en el 2 de argparse.

#### Recuento

**44** llamadas a `sys.exit()` en el paquete: **40** en `cli.py` y **4** en `daemon/run.py`. En `cli.py` se reparten así: 15 con `EXIT_INVALID_INPUT`, 12 con `EXIT_ERROR`, 4 con `EXIT_NOT_FOUND`, 4 con `EXIT_DAEMON_UNREACHABLE`, 2 con `EXIT_OK`, 2 con `EXIT_MODEL_MISSING` y 1 con `EXIT_INTERRUPTED`.

**Defecto de hecho**: el código de salida se decide en 44 sitios dispersos, cada uno con su `print(..., file=sys.stderr)` inmediatamente antes; no hay un punto único que asocie una causa con su código.

### 1.4. Los canales legibles por máquina

`emit_json()` (`cli.py:58`) es el único punto que serializa un payload a stdout. Inyecta `schema_version` si el llamador no la trae y garantiza exactamente un objeto JSON por invocación. `SCHEMA_VERSION` vale `"1"` (`cli.py:55`) y se describe como campo aditivo: añadir claves no la incrementa, solo lo haría un cambio incompatible de las existentes.

#### Reparto de las 17 llamadas

Hay **17** llamadas a `emit_json()`, todas en `cli.py`. **13** solo se alcanzan en rutas de éxito. Las otras cuatro emiten un payload que puede ir seguido de una salida no-cero:

| Sitio | Comando | Código posible tras emitir |
|---|---|---|
| `cli.py:821` | `doctor` | 1, si algún chequeo es FAIL |
| `cli.py:1651` | `daemon start` | 5, si no arrancó |
| `cli.py:1663` | `daemon stop` | 5, si no se detuvo |
| `cli.py:1680` | `daemon restart` | 5, si no reinició |

Ninguna de las 17 está en una ruta de error propiamente dicha: ni en un `except`, ni tras una validación fallida. Todo diagnóstico de error sale por stderr como texto libre.

#### Un fallo con `--json` deja stdout vacío

`speak --json` exige `--output` (§1.2) y emite su payload como último paso del camino feliz (`cli.py:400`). Cualquier salida anterior a ese punto —las ocho reglas de validación, el `FileNotFoundError` de `cli.py:403`, el `except Exception` de `cli.py:412`— termina el proceso con stdout **vacío** y un mensaje en stderr. Un consumidor programático que pidió JSON recibe, ante el error, solo un código de salida y prosa.

#### Tres payloads con clave de estado improvisada

`daemon start`, `stop` y `restart` incluyen una clave `ok` booleana (`cli.py:1646`, `1663`, `1675`) que ningún otro payload tiene. Los demás comandos expresan el resultado con claves propias del dominio —`removed`, `precomputed`, `downloaded`, `passed`/`failed`— o simplemente con el código de salida.

**Defecto de hecho**: el canal legible por máquina existe solo para el éxito. La forma de un error nunca está definida, y `ok` es la única convención de estado, presente en tres de los diecisiete payloads.

### 1.5. El despacho al daemon

`speak` es hoy la **única** superficie que despacha al daemon. El despacho vive en `cli.py:337-401`, tras las ocho reglas de validación, y tiene tres ramas.

| Rama | Se activa con | Comportamiento |
|---|---|---|
| Explícita | `--daemon` | No sondea. Comprueba la sandbox en el cliente y, si la ruta cae fuera, sale 4 (`cli.py:343`). Si pasa, sintetiza vía daemon; cualquier fallo de comunicación se reporta, sin fallback a directo |
| Directa | `--no-daemon` | No sondea. Carga `ChatterboxEngine` en el proceso y sintetiza (`cli.py:376`) |
| Automática | ninguna de las dos | Sondea con `is_daemon_running()` (`cli.py:360`). Si responde **y** la ruta está dentro de la sandbox, va vía daemon; en cualquier otro caso cae a modo directo con un aviso por stderr |

La rama automática degrada por dos motivos distintos y lo dice con dos mensajes distintos: «No disponible; usando modo directo» cuando el sondeo falla (`cli.py:373`), y «La ruta de audio está fuera de los directorios permitidos por el daemon; usando modo directo» cuando la sandbox no admite la ruta (`cli.py:367`).

#### El sondeo

`DaemonIPCClient.is_running()` (`daemon/ipc.py:55`) hace un `GET /health` con 5 s de timeout y **valida el cuerpo** contra el modelo `HealthResponse` de `daemon/protocol.py:110`. Un 200 no basta: si otro servicio ocupara el puerto 8765 y respondiera, un chequeo por status code lo confundiría con el daemon. Un cuerpo que no conforme el esquema se trata como «no es nuestro daemon» y devuelve `False`. Es el único consumidor IPC que no eleva `DaemonIPCError`: discriminar un servicio ajeno es su contrato, no un fallo silenciado.

#### El transporte

`POST /synthesize` (`daemon/server.py:149`) responde con un flujo NDJSON: N líneas `progress` (etapa y tokens del T3 en vivo), luego una línea `result` con el WAV en base64 y los tiempos por sub-etapa, o una línea `error` si la síntesis falla en el hilo worker. Los tres esquemas están en `daemon/protocol.py:83-107` y ambos extremos los validan. `SynthesizeRequest` (`daemon/protocol.py:60`) lleva solo `text`, `voice_audio` y `speech_audio`: el daemon fija modelo y compute backend al arrancar, así que un `--compute-backend` explícito en esta ruta no tiene efecto y se avisa por stderr (`cli.py:182`).

Cualquier `DaemonIPCError` que escape de la síntesis se traduce a exit 5 (`cli.py:416`).

#### El resto de la superficie IPC

El daemon expone además `GET /voices` (`daemon/server.py:287`), `POST /voices/precompute` (`daemon/server.py:296`) y `POST /shutdown` (`daemon/server.py:327`). `voice clone` ya usa el precómputo con el modelo caliente cuando hay daemon activo (`cli.py:453`), pero lo hace como optimización interna: no acepta `--daemon` ni `--no-daemon`.

**Defecto de hecho**: la elección de transporte es una decisión del llamador expresada en flags de `speak`, y ninguna otra superficie que ejecute el modelo la ofrece.

### 1.6. El almacén de voces, el sandbox y el vocabulario

#### El registro de voces

`voices.py` es el hogar único de las rutas del registro y de las operaciones puras de sistema de archivos; ninguna de sus funciones importa ni carga el modelo. El modelo es de **dos niveles**, uniforme en los tres modos de ejecución (fuente, pip/uv-installed, congelado):

| Nivel | Raíz | Función |
|---|---|---|
| Usuario | `data_root()/voices` (escribible) | `voices.voices_root()` (`voices.py:47`) |
| Fábrica | subdirectorio `voices` del paquete, o `sys._MEIPASS/tts_sidecar/voices` congelado | `voices.factory_voices_root()` (`voices.py:52`) → `paths.bundled_voices_dir()` (`paths.py:82`) |

La resolución de un nombre busca primero en usuario y luego en fábrica (`_resolve_voice_dir`, `voices.py:132`), de modo que una voz de usuario puede sobrescribir una de fábrica homónima. Hoy hay **una sola** voz de fábrica: `default/`, con sus dos WAV.

Una voz es válida solo si es un directorio que contiene `reference.wav` **y** `speech.wav`, y ninguno de los tres componentes es un symlink (`_is_valid_voice_dir`, `voices.py:113`). El mismo predicado gobierna `list_voices`, `voice_paths` y `remove_voice`, así que listar y resolver no pueden discrepar.

`_validate_voice_name` (`voices.py:28`) es la puerta de todo nombre antes de componer una ruta con él: exige `^[A-Za-z0-9._-]+$`, rechaza el vacío, `..` y `.`, y normaliza a minúsculas para evitar colisiones en filesystems case-insensitive. `voice_dir` (`voices.py:89`) añade una defensa en profundidad por `realpath` contra escapes del registro.

#### El sandbox

`allowed_audio_dirs()` (`voices.py:76`) declara los tres únicos directorios desde los que el daemon acepta leer audio de entrada:

1. el registro de voces de usuario,
2. el de fábrica,
3. `<tempdir>/tts-sidecar/` (`daemon_session_dir()`, `voices.py:57`), para el staging de audio de sesión IPC.

El tempdir compartido general **no** está permitido: acotarlo evita que cualquier proceso local plante un `.wav` en `%TEMP%` o `/tmp` para que el daemon lo lea.

La frontera real está en el servidor: `_validate_audio_path` (`daemon/server.py:111`) exige extensión `.wav`, archivo existente, contención por `realpath` y header RIFF/WAVE de 12 bytes, y devuelve el `realpath` que el llamador reusa —una sola resolución, sin ventana de symlink swap—. El cliente la anticipa con dos funciones hermanas que responden preguntas distintas y no se colapsan en un booleano: `_paths_allowed_by_daemon` (`cli.py:129`, contención) y `_check_audio_paths_present` (`cli.py:155`, existencia y extensión).

**No hay almacén de salidas.** El audio sintetizado se escribe donde diga `--output`, sin registro, sin metadatos y sin comprobación de colisión: un `speak --output` sobre un archivo existente lo sobrescribe.

#### El vocabulario: `speech` ya está tomado

El término `speech` nombra hoy, en tres capas distintas, el audio de **referencia** que condiciona la síntesis —nunca su resultado—:

| Capa | Identificador | Qué nombra |
|---|---|---|
| CLI | `voice clone --speech` (`cli.py:1774`) | El audio de habla limpia que se registra como referencia |
| Filesystem | `speech.wav` dentro del directorio de la voz (`voices.py:126`) | El mismo audio, ya en el registro |
| Protocolo IPC | `speech_audio` (`daemon/protocol.py:68`), expuesto como `speak --speech-audio` | La ruta de ese audio cruzando la frontera de proceso |

**Defecto de hecho**: `speech` no está libre para nombrar el habla sintética; cualquier uso nuevo del término colisiona con tres significados ya establecidos.

**Defecto de hecho**: `clone_voice_files` (`voices.py:176`) eleva un único `ValueError` para tres causas distintas —audio ilegible, nombre de voz inválido y colisión sin `--force`— y `cmd_voice_clone` las colapsa en un solo exit 4 (`cli.py:472`), pese a que la acción correctiva de cada una es diferente.

---

## 2. Estado objetivo

Lo que va a existir: la superficie de comandos, el almacén de habla sintética y los dos canales legibles por máquina, el código de salida y el payload `--json`. Todo se deriva de los criterios que abren la sección.

### 2.1. Invariantes y criterios generadores

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

**La asimetría de reversibilidad que respalda el criterio.** Las dos opciones no cuestan lo mismo si resultan equivocadas: **añadir una clave después es aditivo** y está cubierto por la política de compatibilidad del esquema `--json`; **retirarla es incompatible** y obliga a subir `cli.SCHEMA_VERSION`. Con esa asimetría, el lado seguro se conoce de antemano y no hay opcionalidad que comprar aplazando la decisión.

**Coste declarado.** Ninguna superficie saca los bytes de una locución fuera de la CLI: `speech play` la reproduce y no hay ningún comando de exportación. Un orquestador que quiera el WAV no lo tiene. Eso es un hueco de la superficie de comandos; la respuesta, si la necesidad aparece, es un comando explícito con su propia decisión, no una clave en un listado.

#### El canal de la causa fina, y la regla que decide entre código y razón

El entero no puede llevar la causa fina y no debe intentarlo. Una misma reacción del consumidor puede corresponder a varias acciones distintas del destinatario humano: liberar disco, corregir permisos, renovar un token, desbloquear la red o instalar una dependencia inducen todas la misma siguiente llamada, y son cinco cosas distintas que hacer antes de repetirla.

El proyecto tiene dos canales legibles por máquina y usa los dos: el entero, que es un espacio cerrado, y el payload JSON, que es **aditivo por contrato** y tiene un punto único de emisión, `emit_json()`. La distinción fina va por el canal abierto.

**Tres reglas de compatibilidad**, que son lo que impide reabrir la misma brecha un nivel más allá:

1. **El entero siempre basta por sí solo.** `reason` refina; nunca contradice ni condiciona. Un consumidor que ignore la clave se comporta correctamente, solo que con menos resolución. Sin esta regla el segundo canal sería una segunda tabla congelada.
2. **Añadir un `reason` nuevo no incrementa `schema_version`**, igual que añadir una clave. Es contrato de emisión **y de consumo**: un `reason` desconocido se trata como ausente, es decir, se degrada al entero.
3. **Regla de promoción.** Un código de salida nuevo solo se justifica cuando cambia **la siguiente llamada del consumidor** —la segunda pregunta del eje—; cuando la llamada siguiente es la misma y lo que cambia es la acción concreta que alguien ejecuta antes de repetirla, es un `reason`. Su árbitro es único y comprobable: se responde diciendo qué se invocaría a continuación, no sopesando importancia.

### 2.2. La superficie y el vocabulario

#### Ocho comandos de nivel superior

| Comando | Sub-acciones | Propósito |
|---|---|---|
| `speech` | `synthesize`, `say`, `play`, `list`, `remove` | Síntesis de habla y gestión del almacén |
| `voice` | `list`, `clone`, `remove` | Gestión del registro de voces |
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
| CLI | Entrada de referencia de timbre de `voice clone` | `--timbre-reference` (`-t`) |
| CLI | Entrada de referencia de habla de `voice clone` | `--speech-reference` (`-s`) |
| CLI | Borrado masivo de la salida | `cleanup --synthetic-speech` |
| Filesystem | Almacén de la salida generada | `data_root()/synthetic-speech/` |
| Filesystem | Archivos de referencia de una voz | `timbre-reference.wav`, `speech-reference.wav` |
| Payload | Clave del listado | `synthetic_speech` |
| Interno | Parámetro del timbre en el motor y el protocolo | `timbre` |

El orden de palabras respeta la convención del repo, con el núcleo al final (`--compute-backend`, `--timbre-reference`). El qualifier vive solo en el directorio y en el flag de `cleanup` —las dos operaciones de gestión—, no en la ruta caliente, que es `speech synthesize`.

En disco los dos sentidos quedan separados por nombre y no por posición:

```
data_root()/
  voices/<voz>/timbre-reference.wav     ← entrada aportada
  voices/<voz>/speech-reference.wav     ← entrada aportada
  synthetic-speech/<voz>/<etiqueta>.wav ← salida generada
```

**En prosa española la unidad se llama «locución».** Nunca aparece como identificador.

#### Las decisiones de vocabulario de la superficie

- **El identificador de una locución es `--label/-l`, no `--name/-n`.** Por homología con `voice` correspondería `--name`, pero dentro del grupo `speech` sería ambiguo frente a `--voice` («¿nombre de qué?»). Se acepta la divergencia con `voice --name` a cambio de que el mismo concepto no tenga dos nombres en dos comandos.
- **La voz se selecciona con `--voice/-v` en las cinco sub-acciones**, no con `--voice-profile`: el concepto ya se llama «voice» en `voice list`, `voice clone` y `voice remove`, y darle un segundo nombre en otro comando es la homonimia al revés —dos palabras para una cosa— con el mismo costo.
- **`--play` y la sub-acción `play` comparten palabra a propósito.** Nombran una sola cosa —emitir audio por los parlantes— en los dos sitios donde ocurre.
- **`-t` es `--text` en `speech` y `--timbre-reference` en `voice clone`.** Cada corto vive en su subcomando, sigue a su flag largo y no se solapa: `voice clone` no declara `--text` y `speech` no declara referencias.
- **`-n` no está tomado en el grupo `speech`**, así que tiene un significado único en toda la CLI: `--name` en `voice clone` y `voice remove`.

### 2.3. El grupo `speech`

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
| `speech synthesize` | `--text/-t` **requerido** · `--label/-l` **requerido** · `--voice/-v` · `--play/-p` · `--force/-f` · `--compute-backend/-cb` · `--json` · `--daemon`/`--no-daemon` |
| `speech say` | `--text/-t` **requerido** · `--voice/-v` · `--compute-backend/-cb` · `--json` · `--daemon`/`--no-daemon` |
| `speech play` | `--label/-l` **requerido** · `--voice/-v` · `--json` |
| `speech list` | `--voice/-v` (filtro) · `--json` |
| `speech remove` | `--label/-l` **requerido** · `--voice/-v` · `--json` |

**`--voice/-v` es opcional en las cinco** y, si falta, usa la voz de fábrica `default`.

**El namespace es obligatorio en la gestión.** Las etiquetas viven bajo una voz, así que `play` y `remove` toman `--voice` con el mismo default que `synthesize` y `say`; `list` lo admite como filtro y sin él recorre todas las voces. Es un segmento más que en `voice remove --name X`, inevitable dado el layout del almacén.

**`--label` requerido en `synthesize` es lo que sostiene el reparto.** Elimina de raíz la invocación con efecto cero sin escribir ninguna regla —la rechaza el parser— y elimina la trampa de «previsualizo con un comando y guardo con otro»: como `synthesize` siempre persiste, nadie pierde la toma que acaba de oír.

**`--compute-backend/-cb` lo declaran las dos que sintetizan**, con valores `auto` (default), `cpu`, `cuda` y `mps`. Solo surte efecto en la ruta directa; su interacción con el despacho está en 2.5.

**`speech play` no necesita modelo ni daemon**: lee el WAV del almacén y lo reproduce.

**El listado no vive dentro de `synthesize`.** No hay `speech synthesize --list`: el listado es `speech list`.

**Reparto con `cleanup`**: `speech remove` cubre el borrado individual y `cleanup --synthetic-speech` el masivo, exactamente el reparto que existe entre `voice remove` y `cleanup --voices`.

### 2.4. Síntesis y el bucle de `--play`

#### Qué hace cada gemelo

Sin `--play`, `synthesize` sintetiza, guarda y termina. Con `--play`, reproduce la toma y pregunta antes de guardar.

`speech say` sintetiza y reproduce, y no escribe nada en el almacén. Es el destino de la invocación que solo quiere oír el resultado: la que no nombra un artefacto porque no lo quiere.

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

### 2.5. El despacho al daemon

#### Tres modos

| Invocación | Qué hace |
|---|---|
| Sin flags | **Comprueba el daemon.** Si está activo, sintetiza por él; si no, carga el modelo al vuelo |
| `--no-daemon` | Fuerza la síntesis directa aunque el daemon esté activo |
| `--daemon` | **Exige** el daemon: si no está activo, sale con **5** en vez de degradar |

La autodetección es el único camino por defecto: un comportamiento especificado, no una rama a la que se cae cuando el llamador no dice nada.

**No hay degradación silenciosa.** `--no-daemon` es un opt-out explícito del usuario, categóricamente distinto de una degradación automática que elude una restricción sin que nadie la pida.

#### Qué superficies lo reciben

**Las tres que necesitan el modelo cargado: `speech synthesize`, `speech say` y `voice clone`.** `voice clone` precomputa los conditionals de la voz al clonarla, así que necesita el modelo igual que las dos que sintetizan, y recibe los tres modos por simetría: con `--daemon` lo exige y sale 5 si no está, y con `--no-daemon` fuerza la ruta directa.

`speech play`, `speech list` y `speech remove` no lo reciben porque no tocan el modelo.

#### Por qué `--daemon` significa exigir y no seleccionar

Con la autodetección por defecto, «usa el daemon» deja de ser algo que haya que pedir. Sin el flag, el llamador no tendría forma de exigir la ruta rápida y el código 5 **se quedaría sin ningún productor en la síntesis**: si la ausencia del daemon siempre degrada, nunca hay «daemon inalcanzable», solo una invocación más lenta. Un consumidor con presupuesto de latencia —el narrator es el caso previsto— necesita poder decir «prefiero fallar a esperar a que cargue el modelo».

Con los dos flags declarados, la exclusión mutua entre ellos tiene sentido pleno: «exige daemon» y «prohíbe daemon» se contradicen.

#### `--compute-backend` y el despacho

**`--compute-backend` solo surte efecto en la ruta directa.** El daemon fija modelo y compute backend al arrancar, así que con el daemon activo un valor explícito se avisa por stderr y se ignora. La vía para imponer un backend distinto del que el daemon fijó es `--no-daemon`, que es también la razón documentada de ese flag.

`voice clone` recibe los tres modos de despacho, pero **no** declara `--compute-backend`.

### 2.6. Reglas de validación

#### Las cinco reglas, todas con exit 2

1. **`--daemon` y `--no-daemon` son excluyentes.** La resuelve el grupo mutuamente excluyente del parser, no una comprobación a mano. Aplica a `speech synthesize`, `speech say` y `voice clone`.
2. **`--json` es incompatible con `--play`.** El bucle escribe la pregunta y lee la respuesta por los canales estándar, y contaminaría el payload. Aplica a `speech synthesize`.
3. **`--text` no vacío ni solo espacios.** Aplica a `speech synthesize` y `speech say`.
4. **`--text` no excede `MAX_TEXT_LENGTH`** (5000). Se valida **en el cliente** antes de cualquier despacho, con el mismo código por ambas vías; el tope del daemon es defensa en profundidad y no la fuente de la validación. Aplica a `speech synthesize` y `speech say`.
5. **`--play` exige terminal en la entrada estándar.** Si no la hay, se rechaza **antes de sintetizar**. Aplica a `speech synthesize`.

**La regla 5 es de otra clase que las cuatro anteriores**: las cuatro primeras miran los flags, la quinta mira el entorno. La comprobación no altera ningún default —`--play` es explícito, así que la misma línea de comandos no puede significar cosas distintas según dónde corra—; solo rechaza antes una invocación que iba a fallar igual. Lo único que queda fuera de alcance es alimentar las respuestas del bucle por una tubería, un caso marginal cuyo precio, de conservarlo, sería pagar una síntesis y una reproducción completas antes de fallar.

#### Un solo mecanismo para la exclusión mutua, y es el declarativo

La exclusión mutua se declara con `add_mutually_exclusive_group`, junto a los flags que restringe, en todos los sitios donde exista —el grupo de tres modos de `setup` incluido. **La garantía queda en un solo lugar, no repetida por convención en cada comando.** Una comprobación manual es esa convención repetida, y no escala: en un grupo de tres modos, un cuarto añadido a mano no rompe nada y deja de cubrir una combinación en silencio; el `if` vive lejos de los flags que restringe, donde nadie que añada uno lo va a leer.

El coste es que el mensaje lo formatea argparse en inglés, igual que el de todas las demás rutas de parseo, y ese mensaje entra íntegro en el payload de error.

#### Validación de identificadores y de existencia

| Situación | Superficies | Código |
|---|---|---|
| Etiqueta con caracteres ilegales | `synthesize`, `play`, `remove` | **2** |
| Nombre de voz con caracteres ilegales | Todas las que toman `--voice` | **2** |
| Voz inexistente | **Las cinco**: `synthesize`, `say`, `play`, `list`, `remove` | **3** |
| Etiqueta inexistente | `play`, `remove` | **3** |
| Colisión de etiqueta sin `--force` | `synthesize` | **6** |
| Colisión de nombre de voz sin `--force` | `voice clone` | **6** |

**La voz se valida en las cinco sub-acciones y sale 3 si no está**, de modo que «voz mal escrita» nunca se disfrace de «sin resultados»: sin esa regla, `speech list --voice noexiste` devolvería una lista vacía y un usuario que se equivoca al escribir concluiría que sus locuciones se perdieron. Con `--voice` opcional en las cinco, la pregunta es la misma en todas y la respuesta también.

La etiqueta inexistente sale **3** y no 2: la invocación está bien formada y el recurso no está, que es exactamente lo que el 3 significa.

**La colisión de etiqueta y la de nombre de voz son el mismo hecho** —el recurso está ocupado y hay que liberarlo o forzar— y comparten código. Con el almacén etiquetado, la colisión no es un caso esporádico: ocurre cada vez que se regenera una locución ya existente, que es flujo normal de trabajo.

#### Ningún flag queda sin efecto sin que la CLI lo diga

La afirmación vale con una excepción declarada: **`--force` sobre una etiqueta libre es un no-op**, igual que `voice clone --force` sobre un nombre libre. Fuera de ese caso, toda combinación de flags tiene efecto declarado o sale con 2, 3 o 6.

### 2.7. Matrices de comportamiento

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

### 2.8. El almacén de habla sintética

#### Ubicación y layout

`data_root()/synthetic-speech/<voz>/<etiqueta>.wav`, **raíz hermana de `voices/`**.

**Por qué no anidado en `voices/<voz>/synthetic-speech/`**, que sería la opción intuitiva y ahorraría código de borrado: `default` es una voz de **fábrica**, en un directorio empaquetado de solo lectura. Sus locuciones tendrían que ir a un espejo en el registro de usuario: un directorio con `synthetic-speech/` pero sin `timbre-reference.wav` ni `speech-reference.wav`. Ese directorio sería invisible para `list_voices` e indeleble por `voice remove`, porque `_is_valid_voice_dir` es el guard que protege el `rmtree` y exige ambos WAV.

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
- **El sidecar es formato interno y no lleva versión de esquema propia.** Su única superficie estable es el payload `--json`, gobernado por `cli.SCHEMA_VERSION`. Darle versión propia daría al proyecto tres versiones de esquema donde hay dos.
- **Un lector que encuentre un campo desconocido lo ignora**, igual que hacen los modelos del protocolo IPC con `extra="ignore"`.
- **`speech list` tolera un sidecar ausente** mostrando la locución sin metadatos, en vez de fallar. Muestra el texto **truncado** en la salida humana y **completo** en el payload `--json`.

#### Atomicidad de la escritura

Cada archivo se escribe a un temporal en el mismo directorio y se publica con `os.replace`, de modo que una interrupción no deje un WAV truncado que `speech list` mostraría como válido y `speech play` intentaría reproducir.

**El sidecar se publica antes del WAV**, así que la aparición del `.wav` implica que sus metadatos ya están completos. Combinado con que el WAV es el recurso de registro, una interrupción entre ambos `os.replace` deja basura inocua: el sidecar huérfano no ocupa la etiqueta, y `speech remove` lo alcanza.

#### Validación de identificadores

La etiqueta y el nombre de voz son la misma clase de identificador: un segmento de ruta. Los valida **un solo validador parametrizado**, `_validate_path_segment(value, kind="voz" | "etiqueta")`, que ambos invocan en vez de duplicar la regex.

- **El parámetro `kind` determina el sustantivo del mensaje** —«Nombre de voz inválido» frente a «Nombre de etiqueta inválido»—, de modo que `speech synthesize --label "mi saludo"` no culpe a `--voice`. Sin eso, el mensaje de error más frecuente del flag más usado apuntaría a otra cosa.
- **Las etiquetas se normalizan a minúsculas**, porque el validador lo hace deliberadamente para evitar colisiones en filesystems case-insensitive. `--label Saludo` y `--label saludo` son la misma etiqueta, y el archivo se llama `saludo.wav`. Se declara en el help de `--label` y en `USAGE.md`.
- **La defensa anti-escape por `realpath`** corre sobre **ambos** segmentos.
- Un identificador ilegal sale con **2**, sea voz o etiqueta.

### 2.9. Los códigos de salida

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
| `130` | `EXIT_INTERRUPTED` | Interrupción del usuario |

#### Cómo se reparten los enteros

La tabla se deriva del eje de dos preguntas. La segunda es la que reparte los enteros:

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

**Los dos casos límite son inversos, y esa simetría es lo que valida el criterio.** El 4, el 5 y el 8 son **una** clase por causa —modelo ausente, daemon caído, disco lleno y token vencido son el mismo tipo de hecho— repartida en **tres** enteros, porque lo único que un consumidor puede convertir en una llamada distinta es un comando de esta CLI: `setup` y `daemon start` se separan y el resto colapsa en el 8. El 6 es lo contrario: **tres** remedios de naturaleza distinta (`--force`, `daemon stop`, cerrar un proceso externo) plegados en **un** entero, porque ninguno cambia lo que el consumidor distingue —«ocupado» frente a «ausente» y «mal escrito»—. La resolución del entero es la de lo que este programa puede nombrar como paso ejecutable.

**El 1 y el 7 no son vecinos**: en el 1 no se conoce remedio; en el 7 se sabe que no lo hay. Fundirlos borraría la única señal que importa, que es *no reintentar*.

**El 6 tiene un solo dueño.** «Puerto ya en uso» y «la voz ya existe» son el mismo hecho y llevan el mismo código; no hay una constante aparte para el conflicto del daemon.

#### El 2 significa lo que argparse quiere decir con él

El exit 2 es, en Unix y en argparse, el código del error de invocación, y aquí significa exactamente eso. Como consecuencia, **todas las rutas de fallo de parseo son correctas sin escribir una línea de validación**: flag requerido ausente, valor fuera de `choices`, grupo mutuamente excluyente violado, subcomando inválido en los tres niveles, y flag desconocido en cualquier comando.

**Ausente = exploración (0), inválido = error (2).** `tts-sidecar` a secas y `tts-sidecar speech` a secas no son un error: imprimen la ayuda y salen con `EXIT_OK`, igual que `--help`, porque una invocación sin subcomando es exploratoria. La regla no es «ausente o inválido → 2».

Dos pruebas de que la convención es la correcta:

1. **La tabla la honra en otro punto**: `EXIT_INTERRUPTED = 130` es exactamente `128 + SIGINT`. Respetar 128+n y no respetar 2 sería incoherente dentro de la misma tabla.
2. **El proyecto hermano aplica la misma convención**: `tts-sidecar-narrator` usa **2 = uso incorrecto** en sus tres casos —valor fuera de dominio, argumento vacío y comando desconocido— con **1 = error genérico**.

#### Dónde viven las constantes, y por qué eso es parte del contrato

**Las constantes viven en un módulo hoja, `exit_codes.py`, sin imports del paquete.** Un módulo sin dependencias internas **no puede** cerrar un ciclo de import, así que la justificación que empujaría una constante a declararse fuera del bloque no está disponible ni siquiera como pretexto. `cli.py` reexporta las constantes, de modo que `cli.EXIT_*` es un nombre válido; el paquete `daemon` importa del módulo hoja en vez de arrastrar `cli` entero.

**Un contrato cerrado sin un lugar legítimo donde crecer no impide el crecimiento: lo empuja fuera del campo de visión.** El dueño es el módulo, no una advertencia.

**Dos invariantes de gobernanza lo sostienen**, y son distintos:

1. **Ningún `EXIT_*` puede definirse fuera de `exit_codes.py`.** Un test recorre los módulos del paquete y falla ante una asignación con ese prefijo en cualquier otro archivo.
2. **La tabla de `USAGE.md` y el módulo dicen lo mismo.** Compara los pares valor/constante con las filas de la tabla pública. Un código declarado por fuera y además sin documentar es invisible dos veces.

La reexportación desde `cli.py` crea dos sitios donde *parecen* vivir las constantes; el primer invariante lo desactiva —cualquier definición fuera del módulo hoja falla—, así que la reexportación es un alias y no una segunda declaración. La distinción queda escrita en el módulo.

**El comentario del módulo** enuncia el criterio generador en sus dos tiempos —clase de causa y admisión por la siguiente llamada del consumidor—, fecha el congelamiento de la tabla **en la 1.0**, advierte que un intercambio de valores es indetectable para un consumidor, y recoge el criterio de revisión que no puede ser test.

**Dos reglas transversales, y solo una es mecanizable.**

- **Test**: ningún `sys.exit(EXIT_ERROR)` puede alcanzarse por una causa prevista con remedio declarado en su propio mensaje. Un `EXIT_ERROR` cuyo mensaje contenga «reintenta» es por construcción un olvido.
- **Criterio de revisión, no test**: ningún `EXIT_INVALID_INPUT` puede alcanzarse con una invocación bien formada. «Bien formada» no tiene definición ejecutable, y escribirla como test produciría una aserción que no afirma nada. Su lugar es el comentario del módulo, junto al criterio generador.

### 2.10. El canal de error y los payloads

#### La invariante del canal

**Bajo `--json`, toda salida no-cero emite el payload de error.** `code` y `message` son obligatorios; `reason` es opcional en cualquier código y se define donde la distinción **ya existe calculada** en el código.

El payload de error usa una clave de primer nivel `error`, emitida solo bajo `--json`, y deja intacto el stderr en castellano para el uso humano:

```json
{"schema_version": "2", "error": {"code": 8, "reason": "disk_full", "message": "…"}}
```

El único código con `reason` poblado es el **8**: la clasificación de por qué falló la provisión —dependencia del runtime ausente, credenciales, red, permisos y disco lleno— ya se calcula, y `reason` es el nombre estable de esa distinción. El 6 y el 7 agrupan subcausas sin nombrar; añadírselas más adelante es aditivo. El fallo de parseo lleva `reason: "usage_error"`.

Las tres reglas de compatibilidad y la regla de promoción son contrato **de consumo** además de emisión: `USAGE.md` declara explícitamente que un `reason` desconocido se trata como ausente.

#### El mecanismo: un solo punto de traducción

**La invariante no se sostiene con un `if` por sitio**, porque eso la deja en manos de que nadie olvide uno. Es la misma solución que la ruta de éxito ya tiene con `emit_json()`, cuyo docstring enuncia el motivo: *«la garantía queda en un solo lugar, no repetida por convención en cada comando»*. La ruta de fallo tiene la misma forma:

- Los sitios de fallo levantan **`CliError(code, reason, message)`** en vez de imprimir y salir.
- **`main()` es el único punto que lo traduce**: mensaje humano a stderr, payload a stdout si se pidió `--json`, y salida con el código. No queda otro camino hasta la salida, así que la invariante no necesita vigilancia.
- El invariante que la protege es mecanizable: **ninguna salida no-cero fuera de `main()`**.

**`CliError` hereda de `BaseException`, no de `Exception`, y esa elección no es estilística.** Una señal de control de flujo no debe ser capturable por un manejador de errores de dominio — es la razón por la que `SystemExit` tampoco lo es. Con `Exception` como base, las salidas envueltas en un `except Exception` de su propio comando quedarían capturadas y saldrían con 1, y alguna además pasaría por la función clasificadora de fallos de provisión y se diagnosticaría como imprevisto. Un test afirma que `CliError` **no** desciende de `Exception`, porque el invariante de salidas comprueba la forma de la salida y no su destino.

**El fallo de parseo entra por el mismo canal.** Una subclase de `ArgumentParser` sobrescribe `error()` para que levante `CliError(EXIT_INVALID_INPUT, "usage_error", message)` en vez de imprimir y salir: así el texto que argparse ya calcula entra al payload en vez de perderse, y el 2 —el fallo más frecuente que verá un consumidor programado— deja stdout tan poblado como cualquier otro. `parse_args()` corre dentro del mismo handler. Queda un residuo honesto: al fallar el parseo no existe `args`, así que hay que mirar `sys.argv` para saber si se pidió `--json`; decide *si* emitir, no qué, y vive en un único sitio.

**El render deja pasar intacto cualquier `SystemExit` de código 0.** `--help` sale por esa vía sin pasar nunca por `error()`, así que un handler que no discrimine por código emitiría payload de error en la invocación más común de toda la CLI. Es el único caso, y tiene test de regresión propio en los tres niveles de parser.

**`daemon serve` queda fuera del mecanismo, y por una razón concreta: no acepta `--json`.** No hay payload que emitir, así que la invariante del canal no tiene alcance ahí y ese comando sale directamente. Esa es la condición que lo autoriza y ninguna otra: darle `--json` reabriría el hueco.

#### Los cinco payloads del grupo `speech`

Ninguno emite ruta, por el criterio de la ruta en los payloads. Todos llevan además los campos transversales del sobre.

| Sub-acción | Payload |
|---|---|
| `speech synthesize` | `{"voice", "label", "t3_time", "s3gen_time", "daemon"}` |
| `speech say` | `{"voice"}` |
| `speech list` | `{"synthetic_speech": [{"voice", "label", "text", "created_at"}]}` |
| `speech play` | `{"voice", "label"}` |
| `speech remove` | `{"voice", "label"}` |

- **`synthesize`** lleva `label` siempre, porque `--label` es requerido. No hace falta ningún campo de persistencia: bajo `--json` el bucle es inalcanzable y la persistencia es cierta cuando la salida es 0.
- **`say`** no lleva `label` porque no produce artefacto, y **no repite el `text`**: el llamador acaba de mandarlo, y devolver la entrada no es información. Lo único que el llamador puede no saber es qué voz se usó, porque si no pasó `--voice` la eligió el sistema.
- **La asimetría entre los dos gemelos es deliberada**: `synthesize` emite los tiempos de síntesis y `say` no, pese a que el llamador de `say` tampoco los conoce. Quedarse en un solo campo es la aplicación de la asimetría de reversibilidad: añadir después la duración del audio o los tiempos de síntesis no cuesta nada, y retirarlos sí.
- **`list`** emite el texto completo. La clave es el nombre del recurso en snake_case, siguiendo el precedente de `voice list --json`, que emite `{"voices": [...]}` — y evitando que un identificador del contrato legible por máquina contradiga el vocabulario de la superficie.
- **`remove`** no lleva campo de resultado: el código de salida ya transporta la información (0 = se borró, 3 = no existía). Un campo `removed` chocaría además con `cleanup --json`, que emite `removed` como lista de rutas, y la misma clave con dos tipos bajo una sola versión de esquema es justo lo que un consumidor tipado no puede manejar.

Los payloads de `daemon start`, `stop` y `restart` no llevan clave booleana propia: el fallo se reporta por el payload de error como en el resto de la CLI.

#### Las dos versiones de esquema

Son **dos, independientes**, y ambas valen `"2"`:

- **`protocol.SCHEMA_VERSION`** — forma de los mensajes IPC del daemon. Vale `"2"` porque `/synthesize` identifica la voz por su nombre y no transporta rutas: una forma que no es aditiva y por tanto exige versión propia.
- **`cli.SCHEMA_VERSION`** — forma de los payloads `--json` de la CLI. Vale `"2"` porque el payload de síntesis no lleva clave de ruta de salida.

Son dos causas independientes. Los payloads del grupo `speech` no influyen en ninguna: añadir subcomandos es aditivo, y añadir la clave `error` también lo es.

**La política de compatibilidad es la misma en ambas**: añadir claves no incrementa la versión; solo lo hace un cambio incompatible de las existentes.

### 2.11. `cleanup`, `setup` y `voice`

#### `cleanup`

| Modo | Qué borra |
|---|---|
| `--synthetic-speech` | La raíz `synthetic-speech/` entera |
| `--voices` | Las voces que puede borrar y, **con ellas, solo los namespaces de habla sintética de esas voces** |
| `--all` | Modelo + voces + habla sintética |
| `--dry-run` | Cubre las locuciones en los tres modos anteriores |

**`synthetic-speech/default/` sobrevive a `--voices` y cae únicamente con `--synthetic-speech` o `--all`.** El criterio es el del propio flag —las locuciones se van con su voz— y la voz de fábrica no se va nunca: es de solo lectura y `--voices` no la borra. Importa declararlo porque `default` es la voz por defecto de `speech synthesize` y su namespace es probablemente el más poblado.

`--all` incluye la habla sintética por necesidad: si no la incluyera dejaría residuo tras una desinstalación completa, que es justo lo que ese flag existe para evitar.

Con la raíz separada del registro de voces, el arrastre de `--voices` es código explícito y no un efecto del `rmtree`.

#### `setup`

El chequeo de audio degrada a WARN en vez de FAIL, **con la premisa que lo sostiene**: el sidecar es instalable en hosts headless, SSH y CI porque existe un sumidero que no necesita subsistema de sonido —`speech synthesize --text T --label L` sintetiza y persiste sin reproducir nada—. `setup` es provisión, no diagnóstico.

#### `voice`

- **`voice clone` toma `--timbre-reference/-t` y `--speech-reference/-s`**, y los archivos en disco se llaman `timbre-reference.wav` y `speech-reference.wav`. Internamente el timbre es un solo nombre: `timbre`.
- **`voice clone` recibe el despacho al daemon en sus tres modos**, porque precomputa los conditionals de la voz al clonarla y necesita el modelo cargado igual que las dos sub-acciones que sintetizan.
- **`voice clone` sobre un nombre tomado sin `--force` sale con 6**, y sobre un nombre libre `--force` es un no-op declarado.
- `_is_valid_voice_dir` reconoce una voz por sus dos WAV de referencia.
- En `voice list` y `voice remove`, `-n` es `--name`.

### 2.12. Contratos externos

#### El integrador de narración

**`speech say --text "<msg>" --daemon`** es la invocación que sintetiza y reproduce, y es el contrato del integrador de narración: exige el daemon porque su presupuesto de latencia no admite cargar el modelo al vuelo. No hay alias de compatibilidad; esa es la única forma de la invocación.

El integrador que quiera además conservar el audio usa `speech synthesize --text "<msg>" --label L`, que no reproduce.

#### La frontera del daemon

`/synthesize` recibe `voice: str`. No hay lista de directorios de audio permitidos, ni validación de rutas de audio, ni directorio de sesión del daemon, porque no hay rutas que validar.

**Riesgo conocido y declarado**: `data_root()` depende de `LOCALAPPDATA` / `XDG_DATA_HOME`, así que un daemon y un cliente arrancados con entornos distintos responden «voz no encontrada» para una voz que el cliente sí lista. Está atenuado porque `/voices` permite inspeccionar la vista del daemon.

---

## 3. El puente

> ⏳ **Pendiente de redacción.** Cómo se llega del estado actual al objetivo, en tres movimientos.
>
> ⚠️ **El índice que sigue es un placeholder, no el índice definitivo.** Esta sección se deriva de la sección 2 ya resuelta —la distancia entre el estado actual y el objetivo *es* el trabajo—, así que no puede tener forma antes que ella. Se **reestructura y reescribe entera** una vez cerrada la sección 2. Los títulos de abajo no comprometen nada.

### 3.1. El orden y por qué

> ⏳ **Pendiente de redacción.** El argumento de no-invertibilidad que fija el orden de los tres cortes.

### 3.2. Movimiento 1 — limpieza

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con su verificación.

### 3.3. Movimiento 2 — el contrato de salida

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con la tabla de reclasificación de códigos.

### 3.4. Movimiento 3 — la feature

> ⏳ **Pendiente de redacción.** Pasos del movimiento, con su verificación.

### 3.5. Puertas de verificación

> ⏳ **Pendiente de redacción.** Una puerta por movimiento, con sus comprobaciones ejecutables.

### 3.6. Documentación pública

> ⏳ **Pendiente de redacción.** Qué cambia en `USAGE.md`, `docs/DAEMON-MODE.md`, `docs/NARRATION-INTEGRATION.md` y `CHANGELOG.md`.
