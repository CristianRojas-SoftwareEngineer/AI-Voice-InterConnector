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
  - [2.2. Superficie de comandos y vocabulario](#22-superficie-de-comandos-y-vocabulario)
  - [2.3. El grupo `speech`: cinco sub-acciones](#23-el-grupo-speech-cinco-sub-acciones)
  - [2.4. `speech synthesize` y el bucle de `--play`](#24-speech-synthesize-y-el-bucle-de---play)
  - [2.5. El despacho al daemon](#25-el-despacho-al-daemon)
  - [2.6. Reglas de validación](#26-reglas-de-validación)
  - [2.7. Matrices de comportamiento](#27-matrices-de-comportamiento)
  - [2.8. El almacén de habla sintética](#28-el-almacén-de-habla-sintética)
  - [2.9. El contrato de salida](#29-el-contrato-de-salida)
  - [2.10. El canal de error y los payloads `--json`](#210-el-canal-de-error-y-los-payloads---json)
  - [2.11. Cambios en `cleanup`, `setup` y `voice`](#211-cambios-en-cleanup-setup-y-voice)
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

> ⏳ **Pendiente de redacción.** Lo que va a existir. Se lee sin conocer el estado actual ni el camino: esta sección es autosuficiente.

### 2.1. Invariantes y criterios generadores

> ⏳ **Pendiente de redacción.** Ninguna superficie acepta rutas del llamador; una responsabilidad por sub-acción; el eje de dos preguntas que ordena los códigos de salida; la asimetría de reversibilidad.

### 2.2. Superficie de comandos y vocabulario

> ⏳ **Pendiente de redacción.** Comandos y sub-acciones resultantes, más la tabla de resolución del vocabulario.

### 2.3. El grupo `speech`: cinco sub-acciones

> ⏳ **Pendiente de redacción.** Las cinco sub-acciones y los parámetros de cada una.

### 2.4. `speech synthesize` y el bucle de `--play`

> ⏳ **Pendiente de redacción.** Comportamiento de la sub-acción de síntesis y el bucle de aceptación.

### 2.5. El despacho al daemon

> ⏳ **Pendiente de redacción.** Los tres modos de despacho y qué superficies los reciben.

### 2.6. Reglas de validación

> ⏳ **Pendiente de redacción.** Las cinco reglas, con el código de salida de cada una.

### 2.7. Matrices de comportamiento

> ⏳ **Pendiente de redacción.** Las dos matrices, con sus filas de salida legible por máquina.

### 2.8. El almacén de habla sintética

> ⏳ **Pendiente de redacción.** Qué archivo es el recurso de registro, la forma del sidecar y cómo se cierra la ventana entre comprobación y escritura.

### 2.9. El contrato de salida

> ⏳ **Pendiente de redacción.** La tabla de códigos, con el criterio generador que decide a cuál pertenece un fallo nuevo.

### 2.10. El canal de error y los payloads `--json`

> ⏳ **Pendiente de redacción.** Forma de los payloads, reglas de compatibilidad e invariante del canal.

### 2.11. Cambios en `cleanup`, `setup` y `voice`

> ⏳ **Pendiente de redacción.** Qué arrastra cada bandera de limpieza y qué cambia en los otros dos comandos.

---

## 3. El puente

> ⏳ **Pendiente de redacción.** Cómo se llega del estado actual al objetivo, en tres movimientos.

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
