# Guía de uso de AI Voice-InterConnector

## Tabla de contenidos

- [Modelos (`setup`)](#modelos-setup)
- [Comandos](#comandos)
  - [Referencia de esquemas `--json`](#referencia-de-esquemas---json)
  - [`version`](#version)
  - [`doctor`](#doctor)
  - [`devices`](#devices)
  - [El grupo `speech`](#el-grupo-speech)
    - [`speech synthesize`](#speech-synthesize)
    - [`speech say`](#speech-say)
    - [`speech play`](#speech-play)
    - [`speech list`](#speech-list)
    - [`speech remove`](#speech-remove)
    - [`speech transcribe`](#speech-transcribe)
    - [`speech dub`](#speech-dub)
    - [`speech denoise`](#speech-denoise)
  - [El grupo `voice`](#el-grupo-voice)
    - [`voice clone`](#voice-clone)
    - [`voice list`](#voice-list)
    - [`voice remove`](#voice-remove)
  - [`translate`](#translate)
  - [`cleanup`](#cleanup)
- [Ciclo de vida de la instalación (`self`)](#ciclo-de-vida-de-la-instalación-self)
  - [Instalar y reparar: `self install`](#instalar-y-reparar-self-install)
- [Actualizar de versión](#actualizar-de-versión)
- [Modo daemon](#modo-daemon)
  - [Gestión del daemon](#gestión-del-daemon)
  - [Uso con daemon](#uso-con-daemon)
- [Clonación de voz: recorrido completo](#clonación-de-voz-recorrido-completo)
- [Experiencia unificada entre sistemas operativos](#experiencia-unificada-entre-sistemas-operativos)
- [Formato de audio](#formato-de-audio)
- [Variables de entorno (`AVI_*`)](#variables-de-entorno-avi_)
- [Solución de problemas](#solución-de-problemas)
  - ["El nombre de voz '…' no es válido" (exit 2)](#el-nombre-de-voz--no-es-válido-exit-2)
  - [`speech synthesize` sale con exit 6 `label_exists`](#speech-synthesize-sale-con-exit-6-label_exists)
  - ["Falta el modelo de …" (exit 4)](#falta-el-modelo-de--exit-4)
  - ["GLIBC_2.28 not found" (o similar) al ejecutar el binario en Linux](#glibc_228-not-found-o-similar-al-ejecutar-el-binario-en-linux)
  - [El binario no arranca en macOS (`binary_incompatible`)](#el-binario-no-arranca-en-macos-binary_incompatible)
  - ["La voz 'x' no existe." (exit 3)](#la-voz-x-no-existe-exit-3)
  - ["La voz 'x' ya existe"](#la-voz-x-ya-existe)
  - ["El modelo Base de clonado TTS no está provisionado"](#el-modelo-base-de-clonado-tts-no-está-provisionado)
  - ["La voz 'x' no se puede eliminar." (exit 2)](#la-voz-x-no-se-puede-eliminar-exit-2)
  - [Sin audio de salida](#sin-audio-de-salida)
  - [El sistema bloquea el primer arranque (binarios sin firmar)](#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar)

AI Voice-InterConnector es un sistema de voz con IA 100 % local en español e inglés:
transcripción, síntesis con clonación de voz, traducción y doblaje. Esta guía recorre cada caso de uso desde la perspectiva
del usuario: qué comando ejecutar, qué ocurre y qué salida esperar.

Todos los comandos funcionan **de forma idéntica en Windows, Linux y macOS**: la
misma sintaxis, la misma salida y los mismos códigos de retorno. Las diferencias
por plataforma (backend de reproducción, nombres de dispositivo, ubicación de
datos, señales del daemon, rendimiento de la traducción y dependencias del
sistema) se detallan en [Experiencia unificada entre sistemas operativos](#experiencia-unificada-entre-sistemas-operativos).

Para instalar, ver [Instalación en el README](README.md#instalación). Si trabajas desde el código fuente, sustituye `ai-voice-interconnector` por `cargo run --`; ver [CONTRIBUTING.md](CONTRIBUTING.md).

## Modelos (`setup`)

`setup` descarga **los 5 modelos** desde HuggingFace Hub de forma nativa
(crate `hf-hub`, TLS rustls) a la **caché exclusiva de la aplicación**
(rutas por SO y efecto de `HF_HUB_CACHE`/`HF_HOME` en
[Ubicaciones en disco por SO](docs/BUILD.md#ubicaciones-en-disco-por-so)):

| Modelo | Repo HF | Uso |
|---|---|---|
| `opus-mt-es-en` | `CristianRojaas/opus-mt-es-en-ct2-int8` | Traducción español → inglés (derivado int8 de Helsinki-NLP/opus-mt, CC-BY-4.0, ya convertido a CTranslate2) |
| `opus-mt-en-es` | `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción inglés → español (derivado int8 de Helsinki-NLP/opus-mt, CC-BY-4.0, ya convertido a CTranslate2) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (ONNX int8) |
| `dpdfnet8` | `Ceva-IP/DPDFNet` | Limpieza de ruido (`--denoise` y `speech denoise`); no bloquea el arranque del daemon |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` | Clonado de voz y síntesis de todas las voces (Base) |

Las revisiones están pineadas por commit hash en `MODEL_REVISIONS`
(`crates/avi-shared/src/paths.rs`): mismo binario → mismos pesos. Tamaño de la descarga y detalle de cada modelo en [`setup`](docs/CLI/commands/SETUP.md); el espacio en disco, en [Requisitos del README](README.md#requisitos). Una descarga interrumpida no se reanuda: `setup` repite el archivo completo.

```bash
ai-voice-interconnector setup                        # descarga los 5 modelos (idempotente)
ai-voice-interconnector setup --force-update         # purga los snapshots pinneados + xet y re-descarga
ai-voice-interconnector setup --force-update --yes   # ídem, sin confirmación interactiva
```

**Qué esperar:** el encabezado `Descargando N modelos:`, una barra por modelo con su
porcentaje, tamaño real, velocidad y tiempo restante (repintada de forma continua, sin
valores absurdos: `quedan —` durante los primeros 2 s), y una línea por modelo terminado
(`  ✓ [1/5] qwen3-tts-0.6b-base  2,5 GB en 44,0 s · 56,7 MB/s`). Al terminar cierra por stderr con
`✓ Setup completado en <total> · N modelos, X descargados`, o con `✓ Los N modelos ya estaban
descargados.` si no había nada que descargar; ya no hay una línea «Total:» aparte. Una descarga
interrumpida se repite entera. La provisión se decide solo por presencia del snapshot HF (no hay
índice `manifest.json` que consultar): si lo vuelves a ejecutar con los
snapshots presentes, termina al instante sin descargar nada. Si la provisión falla al
final de una instalación, el programa **queda instalado** y el comando sale con `11`
(`setup_failed`): es un éxito parcial. Si la causa es un fallo de red (`network_error`)
basta reintentar con `setup`; si Hugging Face rechaza un modelo fijado
(`model_unavailable`), reintentar no lo resuelve y hace falta una versión que vuelva a
fijarlo. `setup` no requiere cuenta: descarga sin credenciales e ignora `HF_TOKEN`,
`HF_TOKEN_PATH` y el token guardado. Pide una sola confirmación antes de actuar: con `--force-update` es destructiva (anuncia que se purgarán los modelos descargados y el tamaño de la selección completa; sin terminal exige `--yes`) y sin él solo informa el tamaño; si la cancelas no se purga ni se descarga nada y solo aparece «Cancelado.» (o «Cancelado; no se ha borrado nada.» con `--force-update`). La limpieza
posterior corresponde a `cleanup` (datos) o `self uninstall` (además programa y PATH).

> **Importante**: hasta que los modelos estén provisionados, `speech say` y `daemon start`
> **abortan de inmediato** (exit 4) con `Error: Falta el modelo …` y la línea `Solución:` que indica `ai-voice-interconnector setup`. Nunca
> disparan una descarga silenciosa.

## Comandos

Tanto los comandos de lectura (`version`, `doctor`, `devices`, `voice list`,
`daemon status`) como los de escritura (`voice clone`, `voice remove`, `setup`,
`cleanup`) aceptan `--json` para salida legible por máquina, útil al invocar
`ai-voice-interconnector` desde otro programa: ningún comando obliga a parsear texto.

El programa sin subcomando muestra la ayuda (exit 0); un grupo sin subcomando
(`speech`, `voice`, `daemon`, `self`) sale con 2 mostrando la ayuda del grupo.
La regla está en [CONTRACT §9](docs/CLI/CONTRACT.md).

Todo payload `--json` incluye el campo **`"schema_version"`**, que identifica la
forma del esquema; su valor vigente y la regla de cuándo sube están en
[las dos versiones de esquema](docs/CLI/CONTRACT.md#las-dos-versiones-de-esquema).
Es un campo aditivo: añadir claves nuevas no lo incrementa. Un consumidor puede
leerlo para detectar cambios de contrato.

Un `reason` desconocido se trata como ausente y se decide por el código de
salida: añadir un `reason` no sube `schema_version`, así que un consumidor debe
tolerar los que aún no conoce. La regla normativa está en
[el canal de la causa fina](docs/CLI/CONTRACT.md#el-canal-de-la-causa-fina-y-la-regla-que-decide-entre-código-y-razón).

**Ojo: el protocolo del daemon tiene su propia versión**, declarada en
[las dos versiones de esquema](docs/CLI/CONTRACT.md#las-dos-versiones-de-esquema).
Son dos contratos independientes —el envelope de la CLI y el IPC del daemon— y suben por separado; el
número de cada uno se lee de su propia constante, `CLI_SCHEMA_VERSION` y
`DAEMON_SCHEMA_VERSION` (`crates/avi-core/src/json_emitter.rs`), y el historial de
subidas de cada uno vive en el [CHANGELOG](CHANGELOG.md).

### Referencia de esquemas `--json`

Los payloads siguientes son **parte del contrato programático**: sus claves son
estables (los cambios solo pueden ser aditivos mientras `schema_version` sea
`"6"`). En todos los casos, stdout contiene exactamente un objeto JSON y el
diagnóstico/progreso va a stderr; no se escriben la línea de contexto, los resultados de texto ni el cierre.
El `error` de un payload de error termina siempre en `.`, `?` o `!` y puede llevar al final
` Detalle: <texto técnico de la biblioteca>`; la línea `Solución:` es solo del texto humano. La clave `schema_version` (string) se omite de
las tablas por brevedad: está presente en todos. Estos esquemas se repiten a
propósito aquí y en la página de cada comando en [`docs/CLI/commands/`](docs/CLI/commands/),
para que quien consume la salida de un comando lo tenga a mano sin cambiar de documento.

**`speech synthesize --json`** — el bucle interactivo de `--play` es
incompatible con `--json` (exit 2 si se combinan), así que bajo `--json` el
audio sintetizado está en el almacén siempre que la salida sea 0. El payload es idéntico
campo a campo en modo directo y vía daemon.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"success"` si se guardó, o `"skipped"` si `--skip-existing` conservó el audio sintetizado existente sin sintetizar |
| `audio_path` | string | Ruta del WAV en el almacén (`speech/<voz>/<etiqueta>.wav`): la guardada o la que ya existía |
| `voice` | string | Nombre de la voz efectivamente usada (`"default"` si no se dio `--voice`) |

**`speech say --json`** — no persiste nada y **no informa de ruta**: el audio suena
según se sintetiza y no pasa por ningún archivo.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | Siempre `"reproduced"` |
| `voice` | string | Nombre de la voz efectivamente usada (`"default"` si no se dio `--voice`) |
| `first_audio_ms` | número | Milisegundos desde que se abre la etapa de síntesis hasta que suena la primera muestra; incluye la espera de turno, el calentamiento del motor y la carga de la voz |
| `stalls` | número | Veces que la cola de reproducción se vació con la entrega en curso y la voz se pausó; `0` si sonó continua |

**`speech play --json`** / **`speech remove --json`** — identifican el audio sintetizado y el resultado.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"played"` / `"removed"` / `"cancelled"` (solo `remove`, si cancelas la confirmación: exit 0, nada borrado) |
| `voice` | string | Nombre de la voz del audio sintetizado |
| `label` | string | Etiqueta del audio sintetizado |

**`speech list --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `speech` | array de objetos | Un objeto por audio sintetizado guardado: `voice` (string), `label` (string), `text` (string, texto completo sin truncar), `created_at` (string, ISO 8601 UTC), `duration_secs` (number); los tres son `null` si el audio sintetizado no tiene sidecar de metadatos |

**`daemon start` / `stop` / `restart --json`** — payload de resultado de la
acción (no de estado; para eso está `daemon status --json`). Los mensajes
informativos van a stderr. El éxito o fallo lo transporta el exit code: un
fallo emite el payload de error (`error`) y sale no-cero, no una clave `ok`.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `start`: `"started"` (arrancado) o `"already_running"` (ya sano, sin tocarlo). `restart`: `"restarted"`. `stop`: `"not_running"` si no había daemon, `"shutdown_sent"` si lo había y se verificó detenido |
| `daemon` | string | `"running"` en `start`/`restart`; `"stopped"` en `stop` |
| `pid` | number | Solo en `start`/`restart` con éxito, si el gestor expone el PID del daemon lanzado |
| `was_running` | boolean | Solo en `stop`: el veredicto del motor sobre si había daemon antes de parar |
| `daemon_fully_stopped` | boolean | Solo en `stop`: el veredicto completo de la verificación a nivel de sistema (sin respuesta, sin PID vivo y sin residente vivo) |

`daemon serve` (servidor en primer plano) no tiene `--json`: su contrato es el
stream NDJSON de `/synthesize`, no un payload de una sola línea.

**Ayuda `--json`** (`--help`, `-h` o el programa sin subcomando) — la ayuda es
una respuesta válida y sale como payload, no como texto.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `help` | string | El texto de la ayuda del nivel invocado, el mismo que ve una persona con `--help` |

**`version --json`** — `--json --version` y `--json -V` emiten el mismo payload.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Siempre `"ai-voice-interconnector"` |
| `version` | string | Versión del programa (p. ej. `"X.Y.Z"`) |

**`doctor --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `version`, `target`, `channel` | string | Versión del binario, tripla del host y canal de instalación |
| `install` | objeto | `dir`, `data_dir` (raíz de datos efectiva), `receipt` (`valid`/`absent`/`invalid`) y `version` |
| `path` | objeto | Resolución y duplicados en el `PATH`, integración y coexistencia |
| `pending` | objeto | Restos de operaciones anteriores (transacción, aparcados, stagings, temporales) |
| `models` | objeto | `root`, `shared_root`, `provisioned`, `missing` (modelos sin provisionar), `size_bytes` |
| `checks` | array de objetos | `{name, ok, detail}` por chequeo |
| `failed` | array de strings | Nombres de los chequeos fallidos (vacío si todo correcto); exit 1 si no está vacío |

**`devices --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `devices` | array de objetos | Un objeto por dispositivo de salida: `id` (number), `name` (string o `null` si el backend no lo expone), `latency` (number o `null`, segundos; `null` si el backend no la expone) |
| `default_input` | objeto o `null` | Entrada predeterminada (micrófono) que usaría la captura: `{ "name" }` (string o `null` si el backend no lo expone), o `null` si el sistema no tiene ninguna |
| `default_output` | objeto o `null` | Salida predeterminada que usaría la reproducción: `{ "name" }` (string o `null` si el backend no lo expone), o `null` si el sistema no tiene ninguna |

**`voice list --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `voices` | array de strings | Nombres de las voces disponibles (fábrica + usuario) |

**`daemon status --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `daemon` | string | `"running"` si `/health` responde; `"stopped"` en caso contrario (exit 0) |
| `engine` | string | Solo con `running`: motor reportado por el daemon |

**`setup --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"completed"`, o `"cancelled"` si cancelas la confirmación (exit 0, `models_provisioned` vacío, nada purgado ni descargado) |
| `models_provisioned` | array de strings | Los 5 modelos (`opus-mt-*`, `parakeet-tdt-v3`, `qwen3-tts-0.6b-base`, `dpdfnet8`) |

**`cleanup --json` / `self uninstall --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `cleanup`: `"cleanup_complete"` / `"planned"` (simulacro) / `"cancelled"` (cancelación de la confirmación, exit 0). `self uninstall`: `"uninstalled"` / `"removal_scheduled"` (Windows, borrado diferido) / `"not_installed"` (idempotencia) / `"planned"` (simulacro) / `"cancelled"` |
| `reason` | string \| null | `null` en el desenlace normal; con `--json`, un fallo emite el objeto `error` + `reason` de §10 del contrato |
| `removed` | array de strings | Rutas efectivamente eliminadas; con `--dry-run` copia `planned`; vacío si no había nada |
| `planned` | array de strings | Lo que la operación iba a borrar, también en el borrado real: simulacro y ejecución salen del mismo cálculo, así que no pueden divergir. Con `--dry-run` es lo único que dice el envelope, y su `status` es `"planned"` |
| `path_reverted` | boolean | Solo `self uninstall`: `true` si se revirtió la integración de `PATH` registrada en el recibo |

El simulacro **no lleva `dry_run`**: con `status` `"planned"` y su recibo `planned`, la
clave sería redundante.

**`self install --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"installed"` o `"repaired"` (reparación: el ejecutable se invoca desde el propio directorio de programa) |
| `reason` | string \| null | `null` en éxito; `"setup_failed"` si el programa quedó instalado pero la provisión no se completó (**exit 11**, éxito parcial) |
| `install_dir` / `version` / `channel` | string | Lo que queda en el recibo de la instalación |
| `path_integrated` | boolean | Si la integración de `PATH` está en pie (estado, no diff de esta pasada) |
| `models` | string | `"skipped"` (`--no-setup`), `"already_provisioned"`, `"provisioned"` o `"failed"` |
| `models_cause` | objeto | **Solo si `models` es `"failed"`**: `{reason, message}` con `network_error` (fallo de transporte; reintentar con `setup` puede resolverlo) o `model_unavailable` (Hugging Face rechaza el modelo fijado; reintentar no lo resuelve) |

**`self update --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"updated"` (reemplazo aplicado), `"already_up_to_date"` (sin descarga, exit 0) o `"check"` (`--check`, sin cambios) |
| `reason` | string \| null | `null` en éxito; `"setup_failed"` si el programa quedó actualizado pero la provisión no se completó (**exit 11**, éxito parcial) |
| `previous_version` / `version` | string | Versión anterior y versión objetivo del reemplazo |
| `current` / `latest` / `update_available` | string, string, boolean | Solo en `"check"` y `"already_up_to_date"`: versión instalada, objetivo y si hay actualización |
| `channel` | string | Canal de la instalación registrada |
| `models_cause` | objeto | **Solo en el parcial**: causa anidada del fallo de provisión |

**`voice clone --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Nombre de la voz registrada |
| `speech` | string | Ruta absoluta del `.qvoice` generado (`reference.qvoice`) |
| `precomputed` | boolean | Ruta local: siempre `false` (motor efímero, sin residente que calentar). Ruta daemon: `true` (warm-on-clone iniciado; completitud en `GET /health`) |
| `removed_speech` | array de strings | Etiquetas de los audios sintetizados que se borraron al sobrescribir la voz con `--force` (`[]` si no tenía) |

Si el usuario cancela la confirmación de `--force`, la salida es `{"status": "cancelled", "name": "<nombre>", "removed_speech": []}`.

**`voice remove --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"removed"` |
| `voice` | string | Nombre de la voz eliminada |
| `removed_speech` | array de strings | Etiquetas de los audios sintetizados que se borraron con la voz (`[]` si no tenía) |

Si el usuario cancela la confirmación, `status` es `"cancelled"` y `removed_speech` es `[]`.

**`translate --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `translated` | string | Texto traducido (igual al de entrada si origen y destino coinciden, passthrough) |
| `source_language` | string | El `--source-language` pedido |
| `target_language` | string | El `--target-language` pedido |

---

### `version`

Muestra la versión del programa.

```bash
ai-voice-interconnector version
ai-voice-interconnector version --json
```

**Qué esperar:**

```
ai-voice-interconnector X.Y.Z
```

---

### `doctor`

Verifica el entorno local sin tocar el daemon ni el audio: el directorio de
datos, los modelos provisionados y el almacén de voces.

```bash
ai-voice-interconnector doctor
ai-voice-interconnector doctor --json
```

**Qué esperar** (entorno sano):

```
Diagnóstico:
  ✓ Recibo de instalación: recibo válido en <directorio-de-programa>
  ✓ Modelos: todos los modelos de la selección están provisionados
✓ Diagnóstico sin problemas.
```

Las comprobaciones salen por stdout con etiquetas en español (los identificadores
internos solo salen en `--json`); la línea final sale por stderr. Los chequeos que
ejecuta `doctor` están en
[su tabla de chequeos](docs/CLI/commands/DOCTOR.md#los-chequeos). Si alguno
falla, lo marca con `✗` y añade al final de su línea la indicación con la orden
concreta (por ejemplo, ``→ ejecuta `ai-voice-interconnector setup` ``), y el comando
escribe `Error: N comprobaciones fallan.` por stderr.

El veredicto es el código de salida: 0 si todos los chequeos pasan (tras una
instalación correcta, `doctor` sale con 0) y 1 si alguno falla.
Referencia canónica: `docs/CLI/commands/DOCTOR.md`.

---

### `devices`

Lista los dispositivos de salida de audio disponibles y muestra la entrada
(micrófono) y la salida predeterminadas, que son las que usan la captura con
`--mic` y la reproducción.

```bash
ai-voice-interconnector devices
ai-voice-interconnector devices --json
```

**Qué esperar:**

```
Salidas de audio:
  [0] Altavoces (Realtek High Definition Audio) · latencia 10,0 ms · predeterminada
  [1] Auriculares · latencia 8,0 ms
Entrada predeterminada: Micrófono (Realtek High Definition Audio)
```

La salida predeterminada se marca con ` · predeterminada` en su propia línea.
Una entrada predeterminada que el sistema no tiene se muestra como `ninguna`, y
un nombre que el sistema no expone, como `desconocido` (una latencia que no
expone, como `latencia desconocida`).

---

### El grupo `speech`

Ocho sub-acciones sobre el habla: dos que sintetizan (`synthesize`, `say`), tres que gestionan el almacén de audios sintetizados guardados (`play`, `list`, `remove`), una que transcribe audio a texto (`transcribe`), una que compone el bucle voz→voz (`dub`) y una que limpia archivos de audio/vídeo (`denoise`). Cada una tiene una sola responsabilidad y el nombre declara su propósito.

| Sub-acción | Qué hace | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda un audio sintetizado | sí | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech transcribe` | Transcribe audio a texto | no | sí |
| `speech dub` | Composición voz→voz: transcribe, traduce si procede, sintetiza y reproduce | no | sí |
| `speech denoise` | Limpia archivos de audio y la pista de audio de vídeo; no procesa imagen | no | sí, `native-denoise` |
| `speech play` | Reproduce un audio sintetizado guardado | no | no |
| `speech list` | Lista los audios sintetizados guardados | no | no |
| `speech remove` | Borra un audio sintetizado guardado (pregunta antes; `--yes` omite la pregunta) | no | no |

**En las que aceptan voz, `--voice/-v` es opcional**; si se omite, usa la voz de fábrica
`default`. **La voz y la etiqueta (`--label/-l`) se normalizan a minúsculas**
antes de resolver rutas: `--label Saludo` y `--label saludo` son el mismo
audio sintetizado (el archivo se llama `saludo.wav`), y lo mismo aplica al nombre de la
voz.

**Despacho al daemon (`synthesize`, `say`, `transcribe` y `dub`, las que
necesitan un modelo cargado):** tres modos, iguales a los de `voice clone`:
- Sin flags: sondea el daemon y lo usa si responde; si no, sintetiza en modo
  directo (carga el modelo al vuelo).
- `--daemon`: exige el daemon; si no está activo, sale con exit **5** en vez de
  degradar.
- `--no-daemon`: fuerza el modo directo, sin sondear.

**Precisión de síntesis:** el motor tiene un único perfil de precisión, siempre activo; no hay variable ni opción para elegirlo.

En las operaciones pesadas (`synthesize`, `say`, `voice clone` y `dub`), la comunicación con el daemon opera mediante streaming NDJSON con latidos periódicos cada 500 ms y timeout de inactividad de 1500 ms (`STREAM_INACTIVITY_TIMEOUT`), evitando cuelgues o timeouts prematuros en tareas de larga duración. Los eventos del stream no llevan texto humano (solo `event`, `stage` y datos de la petición): el cliente traduce cada etapa a una etapa de su catálogo, con los mismos textos que en la ruta directa. El stream de `/dub` añade dos eventos con texto del usuario, `transcript` y `translation` (claves `event`, `text` y `schema_version`), que el cliente muestra en cuanto llegan.

**Salida en la terminal.** Todo comando con etapas abre con una línea de contexto por stderr (por ejemplo `Doblaje es → en · voz default · archivo a.wav · daemon`), trabaja por etapas con su duración (`✓ Audio leído  0,1 s`, la etapa en curso se repinta debajo como `… Sintetizando… 1,3 s`) y cierra con `✓ <acción> en <duración total>`, también por stderr. Las esperas de la síntesis (turno, calentamiento del motor, carga de la voz) son textos vivos de la etapa de síntesis y nunca la cierran como hecha; cuando un comando solo tiene una etapa de trabajo, esa etapa no deja su línea y queda solo el cierre; y la línea de contexto se escribe después de validar la invocación y antes de tocar recursos, de modo que un error de invocación sale sin ella. Los resultados (textos, listas, rutas y planes de `--dry-run`) salen por stdout. Los errores salen como `Error: <qué pasó>.` y, cuando hay remedio, `Solución: <qué hacer>.`; el texto técnico de las bibliotecas solo aparece en el `error` de `--json` (tras ` Detalle:`). El color (verde, rojo, amarillo, atenuado) solo se emite en una terminal; `NO_COLOR` lo desactiva y la salida redirigida nunca lo lleva. `AVI_NO_PROGRESS=1` silencia solo las etapas, no el contexto, los resultados ni el cierre. Las duraciones se escriben `0,7 s`, `2 min 35 s` y `1 h 02 min`.

`--daemon` y `--no-daemon` son mutuamente excluyentes: combinarlos sale con
exit **2** antes de cualquier trabajo. `speech play`, `speech list` y `speech
remove` no tocan el modelo ni el daemon: no declaran estos flags.

#### `speech synthesize`

Sintetiza texto y lo guarda en el almacén de audios sintetizados
(`data_dir()/speech/<voz>/<etiqueta>.wav`); a diferencia de
`speech say`, persiste por defecto. La excepción es `--play`: su bucle
interactivo permite rechazar y descartar la toma, en cuyo caso el comando
termina con exit 0 sin guardar nada (ver «El bucle de `--play`» más abajo).

```bash
ai-voice-interconnector speech synthesize --text "Bienvenido" --label saludo
ai-voice-interconnector speech synthesize --text "Bienvenido" --label saludo --voice mi_voz
```

**Qué esperar:** las mismas etapas de síntesis que `speech say` (ver más
abajo) y, al terminar:

```
stdout: <ruta del WAV persistido>
stderr: Síntesis es · voz default · etiqueta saludo · local
        ✓ Síntesis completada en 2,1 s · 3,4 s de audio
```

Sin `--play` y sin traducción, la síntesis es la única etapa de trabajo y no deja su
línea `✓ Voz sintetizada`: queda solo el cierre.

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--label, -l` (requerido): Etiqueta del audio sintetizado en el almacén (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz a usar (default: `default`)
- `--play`: Reproduce la toma y abre un menú interactivo; solo guarda al aceptar
- `--force, -f`: Sobrescribe el audio sintetizado si la etiqueta ya existe para la voz
- `--skip-existing`: Si la etiqueta ya existe para la voz, la conserva sin sintetizar y sale con exit **0** (`status` `skipped` con `--json`; en texto, la ruta del existente por stdout y `✓ Ya existía; se conserva sin sintetizar de nuevo.` por stderr). Incompatible con `--force` y con `--play`
- `--source-language`: Idioma del texto de entrada (`es` o `en`; por defecto igual a `--target-language`, sin traducir)
- `--target-language`: Idioma/modelo de síntesis (`es` o `en`; default `es`; si difiere del origen, el texto se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
- `--json`: Emite `{status, audio_path, voice}`

**Colisión de etiqueta:** sin `--force` ni `--skip-existing`, guardar sobre una
etiqueta que ya existe para la voz sale con exit **6** y `reason` `label_exists`,
sin pagar la síntesis (la comprobación es previa). Con `--skip-existing`, la
misma orden es idempotente: sale con exit **0** sin sintetizar ni tocar el
audio sintetizado existente, también con el daemon caído o el modelo sin provisionar. La
etiqueta se revalida también al guardar, por si quedó ocupada mientras se
sintetizaba o el bucle de `--play` esperaba una respuesta; la colisión la decide
el WAV, así que una ficha `.json` corrupta no la cambia. `--force` o
`--skip-existing` sobre una etiqueta libre son un no-op. El exit **6** es siempre
un fallo: no se guardó nada.

```bash
ai-voice-interconnector speech synthesize --text "Bienvenido" --label saludo --skip-existing
```

**Orden de validación:** `synthesize`, `say` y `dub` validan en el mismo orden por
las dos rutas: primero la invocación (exit **2**: flags, texto, nombres), después
el almacén (la voz existe, exit **3**; la etiqueta, exit **6**) y por último los
recursos (el modelo, exit **4**, o el daemon, exit **5**). Con dos fallos a la vez
se informa el primero de ese orden.

**Voz cambiada durante la síntesis:** el audio sintetizado se guarda solo si la voz sigue
siendo la misma con la que se sintetizó. Si se re-clonó o se eliminó mientras
tanto, no se guarda nada y sale con exit **6** y `reason` `voice_changed`: repite
la síntesis.

**El bucle de `--play`:** con `--play`, la toma suena mientras se sintetiza
(sin esperar a que termine) y, al terminar, aparece un menú de cuatro opciones
(por stderr; `--json` es incompatible con `--play`). El menú no repite la toma al
entrar:

```
¿Qué quieres hacer con esta toma?
  1) Reproducir otra vez
  2) Aceptar y guardar
  3) Rechazar y regenerar
  4) Rechazar y descartar
Opción [1-4]:
```

- **Reproducir otra vez**: repite el mismo WAV temporal completo, sin volver a sintetizar.
- **Aceptar y guardar**: persiste la toma que acabas de oír y termina con exit 0.
- **Rechazar y regenerar**: sintetiza una toma nueva (suena distinta de las anteriores y también suena mientras se sintetiza), vuelve a mostrar la etapa de síntesis (también vía daemon) y vuelve a preguntar. Si la síntesis falla con la toma ya sonando, se corta la reproducción y el comando termina con error: la toma incompleta no se ofrece ni se guarda.
- **Rechazar y descartar**: termina con exit 0 sin guardar nada y escribe `✓ Audio descartado; no se ha guardado.`.

Ctrl-D en la pregunta equivale a «rechazar y descartar». `speech synthesize
--play` requiere una terminal interactiva en la entrada estándar; sin ella,
sale con exit 2 antes de sintetizar.

#### `speech say`

Sintetiza texto y reproduce el audio por los altavoces a la vez, sin esperar a
que termine la síntesis (entrega en streaming), sin guardar nada en el almacén.

Sin `--voice`, `speech say` usa la voz de fábrica **`default`** (empaquetada,
de solo lectura), por lo que el ejemplo mínimo funciona recién instalado, sin
clonar nada:

```bash
# Reproducir con la voz de fábrica 'default'
ai-voice-interconnector speech say --text "Hola mundo"

# Usar una voz registrada
ai-voice-interconnector speech say --text "Hola mundo" --voice mi_voz
```

**Qué esperar:** en modo directo (sin daemon) se resuelve la voz, se sintetiza
con Qwen3-TTS y el audio suena por los altavoces según se genera: la voz empieza
en torno a un segundo tras la síntesis (más el arranque del motor en modo
directo), no al final. Con el daemon activo, el CLI delega vía HTTP y el modelo
ya está caliente en memoria. Salida típica (stderr; stdout queda vacío):

```
Lectura en voz alta es · voz default · local
✓ Primer audio  1,2 s
✓ Reproducido  3,0 s
✓ Reproducción completada en 4,3 s · primer audio 1,2 s
```

El audio no pasa por ningún archivo, así que el texto no dice ruta; quien quiera
el fichero en disco usa `speech synthesize`, que lo persiste y sí devuelve su
`audio_path`. Si la síntesis falla con la voz ya sonando, se corta la
reproducción y el comando termina con error. `say` es humo de punta a punta, sin
puerta de calidad sobre lo que sonó.

**Orígenes de voz:** todas las voces viven en `data_dir()/voices/<nombre>/`, en
un único nivel.
- **Fábrica**: `default`, clonada, cuyo `reference.qvoice` va embebido en el
  binario (`crates/avi-store/assets/default/`) y se materializa en el primer uso.
  No se puede eliminar.
- **Usuario**: voces registradas con `voice clone` (`reference.qvoice`).
  `voice clone --force` con el nombre de una voz existente, de fábrica incluida,
  sustituye su referencia en el mismo directorio.

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--voice, -v`: Nombre de la voz a usar (default: `default`)
- `--source-language`: Idioma del texto de entrada (`es` o `en`; por defecto igual a `--target-language`, sin traducir)
- `--target-language`: Idioma/modelo de síntesis (`es` o `en`; default `es`; si difiere del origen, el texto se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
- `--daemon`: Usar el daemon sin sondeo previo; si el daemon no responde, sale con 5 (`daemon_unreachable`), sin fallback a directo (ver [el despacho al daemon](docs/CLI/CONTRACT.md#5-el-despacho-al-daemon))
- `--no-daemon`: Forzar modo directo, sin sondear el daemon

`--daemon` y `--no-daemon` son **mutuamente excluyentes**: combinarlos produce
un error en stderr y exit 2 (`INVALID_INPUT`), antes de cualquier trabajo.

**Ejemplos:**
```bash
# Usando voz registrada
ai-voice-interconnector speech say --text "Hola mundo" --voice mi_voz

# Forzar modo directo
ai-voice-interconnector speech say --text "Hola" --voice mi_voz --no-daemon
```

#### `speech play`

Reproduce un audio sintetizado ya guardado; no toca el modelo ni el daemon.
Abre con `Reproducción del audio sintetizado <etiqueta> · voz <voz>`, avanza el
contador de `Reproduciendo…` hasta el final y cierra con
`✓ Reproducción completada en <total>`, todo por stderr. Es su única etapa, así que
no deja la línea `✓ Reproducido`.

```bash
ai-voice-interconnector speech play --label saludo
ai-voice-interconnector speech play --label saludo --voice mi_voz --json
```

**Opciones:**
- `--label, -l` (requerido): Etiqueta del audio sintetizado (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--json`: Emite `{"status", "voice", "label"}`

Una voz ilegal sale con exit **2** (`invalid_voice_name`), una etiqueta ilegal con
exit **2** (`invalid_identifier`) y una voz inexistente con exit **3**
(`voice_not_found`), antes de buscar la etiqueta. Son ilegales el nombre vacío,
los caracteres fuera de `[A-Za-z0-9._-]`, `.` y `..`. Una etiqueta inexistente de
una voz que existe sale con exit **3** (`speech_not_found`).

#### `speech list`

Lista los audios sintetizados guardados. `--voice/-v` es opcional, sin default:
con valor, filtra por esa voz (`SpeechCommands::List { voice:
Option<String> }`, del binario principal; lectura acotada vía
`SpeechStore::list_by_voice`, del almacén de voces); sin
el flag, lista todas las voces.

```bash
ai-voice-interconnector speech list
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech list --json
ai-voice-interconnector --json speech list --voice mi_voz
```

**Qué esperar:**

```
Audios sintetizados guardados:
  default  saludo  3,2 s  «Bienvenido»
  mi_voz  despedida  4,0 s  «Hasta luego, gracias por tu visita»
```

Sin ningún audio, una sola línea: `No hay audios sintetizados guardados.`. La lista
sale por stdout. El texto se muestra completo en la salida humana; el payload
`--json` (`{"speech": [...]}`) lo lleva también completo. Un audio sintetizado
sin sidecar de metadatos se muestra como `  <voz>  <etiqueta>  (sin metadatos)`. Un sidecar ilegible
o corrupto hace fallar `speech list` y `speech play` con exit **1**
(`speech_metadata_corrupt`); el mensaje nombra la etiqueta y la voz, y la
línea `Solución:` remite a `speech remove`, que borra el audio sintetizado sin leer el sidecar.

Con `--voice`, el nombre se valida antes de leer: una voz ilegal (vacía, con
caracteres fuera de `[A-Za-z0-9._-]`, `.` o `..`) sale con exit **2**
(`invalid_voice_name`) y una voz inexistente con exit **3** (`voice_not_found`).

**Opciones:**
- `--voice, -v`: Filtra por voz (default: todas las voces)
- `--json`: Emite `{"speech": [{"voice", "label", "text", "created_at", "duration_secs"}]}`

#### `speech remove`

Borra un audio sintetizado guardado (el WAV y su sidecar de metadatos, si existen).
Es destructivo, así que con terminal pregunta antes de borrar:

```bash
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector speech remove --label saludo --voice mi_voz
ai-voice-interconnector speech remove --label saludo --yes   # sin preguntar
```

```
Se borrará el audio sintetizado saludo de la voz default:
  <ruta del WAV>
¿Borrar este audio sintetizado? [s/N] s
✓ Audio sintetizado saludo de la voz default borrado.
```

Responder que no escribe `Cancelado; no se ha borrado nada.` y sale con exit **0**
(con `--json`, `{"status": "cancelled", "voice", "label"}`). Sin terminal y sin
`--yes`, sale con exit **2** (`confirmation_required`) y la línea `Solución:`
muestra la orden repetida con `--yes`.

**Opciones:**
- `--label, -l` (requerido): Etiqueta del audio sintetizado (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--yes, -y`: No pedir confirmación
- `--json`: Emite `{"status", "voice", "label"}`

Una voz ilegal sale con exit **2** (`invalid_voice_name`), una etiqueta ilegal con
exit **2** (`invalid_identifier`) y una voz inexistente con exit **3**
(`voice_not_found`), antes de buscar la etiqueta, sin tocar ningún archivo. Una
etiqueta inexistente de una voz que existe sale con exit **3** (`speech_not_found`). El borrado masivo es tarea de
`cleanup --synthetic-speech` (ver más abajo).

---

#### `speech transcribe`

Transcribe a texto desde un archivo WAV (`--audio`) o desde el micrófono
(`--mic`). La **captura corre siempre en el cliente** (al daemon viajan las
muestras, nunca rutas); la transcripción en sí se despacha al daemon con el
mismo patrón de tres modos que la síntesis: sin flags sondea el daemon y lo usa
si responde, `--daemon` lo exige (exit 5 si no está activo) y `--no-daemon`
fuerza el modo directo. Es una sub-acción del grupo `speech`, aislada de la
síntesis y de `translate`: el STT solo transcribe (nunca traduce), así que si
necesitas el texto en otro idioma, encadena `translate` por separado.

`--audio` y `--mic` son **mutuamente excluyentes y uno de los dos es
obligatorio**. Con `--mic`, la grabación es **push-to-talk** por defecto
(termina al presionar Enter); `--duration N` fuerza una grabación de duración
fija en segundos y solo es válido junto a `--mic`. El push-to-talk tiene un
techo de seguridad configurable con la variable de entorno
`AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s): al alcanzarlo, la grabación se
detiene, se avisa por stderr y se devuelve lo grabado hasta ese punto con
exit **0** (no es un error).

```bash
ai-voice-interconnector speech transcribe --audio grabacion.wav --source-language es
ai-voice-interconnector speech transcribe --audio recording.wav --source-language en --json
ai-voice-interconnector speech transcribe --audio recording.wav --source-language en --daemon
ai-voice-interconnector speech transcribe --mic --source-language es
ai-voice-interconnector speech transcribe --mic --duration 5 --source-language en
```

**Qué esperar:**

```
Hola, ¿cómo estás?
```

Con `--mic`, la etapa de captura muestra por stderr `● Grabando m:ss / m:ss`
con un medidor de nivel y, en push-to-talk, `Enter para detener`; sin terminal
escribe una sola vez `Grabando (máximo m:ss)…`. El push-to-talk captura hasta
que el usuario presiona Enter (o hasta el techo `AVI_PUSH_TO_TALK_MAX_SECS`, que
avisa `Se alcanzó el límite de grabación`) antes de transcribir. Si la grabación
no contiene voz (el tramo más fuerte no supera −43 dBFS) avisa
`No se detectó voz en la grabación` y continúa. El texto transcrito sale por
stdout; el contexto (`Transcripción es · archivo a.wav · local`), las etapas y el
cierre `✓ Transcripción completada en <total>` salen por stderr.

Con `--json`, emite `{"text", "source_language"}` y nada por stdout salvo ese objeto.
`source_language` es el valor de `--source-language` (`es` o `en`).

Si el audio no contiene voz reconocible, el comando sale con código 2 y
`empty_transcript` («No se reconoció voz en el audio.») con su línea
`Solución:`, en la vía directa y en la del daemon.

**Opciones:**
- `--audio`: Ruta del archivo WAV a transcribir (mutuamente excluyente con `--mic`; uno de los dos es requerido)
- `--mic`: Transcribe desde el micrófono en vez de un archivo (mutuamente excluyente con `--audio`; uno de los dos es requerido)
- `--duration N`: Duración fija de grabación en segundos; solo válido junto a `--mic`
- `--source-language` (requerido): Idioma hablado en el audio (`es` o `en`)
- `--daemon` / `--no-daemon`: igual que en `speech say` (despacho de tres modos; la captura del audio siempre ocurre en el cliente)
- `--json`: Emite `{"text", "source_language"}`

La captura de micrófono usa el backend multiplataforma `cpal` (único, sin
ramas por sistema operativo): graba a la tasa y formato nativos del
dispositivo de entrada y luego normaliza a mono, remuestrea a 16 kHz y
convierte a int16 (el formato que Parakeet asume). El WAV pasado con
`--audio` pasa por esa misma normalización a 16 kHz/mono/int16 sin importar
la frecuencia de origen del archivo — ninguna de las dos rutas requiere
preparación previa.

`--duration` sin `--mic` sale con exit **2** (`EXIT_INVALID_INPUT`). Sin
terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con
exit **2**, porque no hay forma de detectar la pulsación de Enter. Un archivo
de audio inexistente (ruta `--audio`) sale con exit **3**. Si el modelo de
transcripción no está provisionado, falla con exit **4** y una línea `Solución:`
que indica `ai-voice-interconnector setup`; si la transcripción falla con
el modelo ya cargado, sale con exit **10**. En la ruta daemon, un fallo de
comunicación con el daemon (inactivo o inalcanzable) sale con
exit **5**.

---

#### `speech denoise`

Limpia archivos de audio o solo la pista de audio de vídeo. La imagen se conserva sin procesamiento.

```bash
ai-voice-interconnector speech denoise entrada.mp4
ai-voice-interconnector speech denoise voz.wav --preset default --json
```

`studio` es el preset por omisión; `--preset default` selecciona el perfil sin normalización. El modelo DPDFNet debe estar provisionado con `setup`; FFmpeg se resuelve desde el paquete de AVI, sin fallback a `PATH`: el binario integrado debe declarar versión 6 o superior, con o sin el prefijo de `git describe` de los builds de Linux. El flag global `--json` produce JSONL de eventos y resultados (y un único envelope `{error, reason}` si la validación previa al lote falla). En texto, abre con `Limpieza de ruido · N archivos`, muestra por stderr una etapa por archivo con su porcentaje (`… [1/2] entrada.mp4  42 %  <etapa>  3,1 s`), escribe por stdout la ruta de cada archivo generado, y cierra con `✓ Limpieza de ruido completada en <total> · N archivos limpios`; `--dry-run` escribe el plan por stdout (`<entrada> → <salida> (se procesaría)`) y `Simulación: no se ha modificado nada.` por stderr. Un modelo ausente sale con exit **4** (`model_missing`) y un archivo sin audio con exit **2** (`invalid_audio`). Para opciones de lote, colisiones, formatos y límites, consulta la [referencia completa](docs/CLI/commands/SPEECH.md#speech-denoise).

---

#### `speech dub`

Composición voz→voz: transcribe la entrada hablada (archivo o micrófono),
traduce si `--source-language` difiere de `--target-language`, sintetiza con
la voz elegida y reproduce el resultado. Reutiliza las etapas de
`speech transcribe`, la traducción de `speech say`/`synthesize` y el despacho
de síntesis; no guarda nada en el almacén (no tiene `--label`). En modo texto
imprime por stdout `Transcripción: <texto>` en cuanto termina de transcribir y,
si los idiomas difieren, `Traducción: <texto>` en cuanto termina de traducir:
ambas salen antes de que empiece a sonar el audio. El contexto, las etapas y el
cierre (`✓ Doblaje completado en <total> · primer audio <t>`) salen por stderr.
Con `--json` emite
`{"status":"dubbed","text","translated","first_audio_ms","stalls"}`, donde `text` es la
transcripción y `translated` el texto sintetizado (siempre presente), y **sin
`audio_path`**: el audio suena según se sintetiza y no se guarda. Como `say`,
`dub` es humo sin puerta de calidad sobre lo que sonó.

`--audio` y `--mic` son **mutuamente excluyentes y exactamente una de las dos
es requerida**. Con `--mic`, la grabación es **push-to-talk** por defecto
(termina al presionar Enter); `--duration N` fuerza una grabación de duración
fija en segundos y solo es válido junto a `--mic`. El mismo techo
`AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s) aplica aquí: al vencer, detiene la
grabación, avisa por stderr y continúa con lo grabado (exit **0**).

```bash
ai-voice-interconnector speech dub --mic --source-language es --target-language en -v mi_voz
ai-voice-interconnector speech dub --audio grabacion.wav --source-language en --target-language es
```

**Qué esperar:** transcribe tu habla al texto, lo traduce si procede y
reproduce la síntesis con la voz (`default` si no pasas `-v`) mientras se genera. Con `--mic`, la
etapa de captura muestra `● Grabando m:ss / m:ss` con un medidor de nivel (y, sin `--duration`, captura en
modo push-to-talk hasta que presiones Enter o hasta el techo
`AVI_PUSH_TO_TALK_MAX_SECS`) antes de transcribir; si no se detecta voz, avisa y continúa.

**Opciones:**
- `--audio, -a`: Ruta del archivo WAV hablado (mutuamente excluyente con `--mic`; exactamente una de las dos es requerida)
- `--mic`: Graba desde el micrófono (mutuamente excluyente con `--audio`; exactamente una de las dos es requerida)
- `--duration N`: Duración fija de grabación en segundos; solo válido con `--mic`
- `--source-language` (requerido): Idioma hablado en el audio (`es` o `en`)
- `--target-language`: Idioma/modelo de síntesis (`es` o `en`; default `es`; si difiere del origen, se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--daemon` / `--no-daemon`: aplican a la transcripción y a la síntesis

`--duration` sin `--mic` sale con exit **2**, y `--mic` sin `--duration` en
una terminal no interactiva (no TTY) también sale con exit **2**. Un `--audio`
inexistente sale con exit **3**. Códigos de fallo de la cadena: exit **4**
(modelo de transcripción no provisionado; la línea `Solución:` indica
`ai-voice-interconnector setup`), **5** (daemon exigido pero inactivo o
inalcanzable), **9** (fallo de traducción con el modelo
cargado) y **10** (fallo de transcripción con el modelo cargado).

---

### El grupo `voice`

Tres sub-acciones sobre el registro de voces: `clone` registra una voz a partir
de audio de referencia (requiere el modelo Base), `list` muestra las voces de
fábrica y de usuario, y `remove` elimina una voz de usuario. La voz de fábrica
(`default`) no se puede eliminar.

#### `voice clone`

Clona una voz a partir de un audio de referencia (requiere modelo Base).

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav
# Si falta Base: ai-voice-interconnector setup
```

**Qué esperar:** el comando valida que el audio sea cargable, genera `reference.qvoice` vía
`avi_tts::clone_voice` con el modelo Base, y confirma por stderr (error `model_missing` → `setup`):

```
Clonación de la voz mi_voz · archivo condicion.wav · local
✓ Voz mi_voz clonada en 23,7 s
```

Con archivo y sin `--denoise`, el clonado es la única etapa de trabajo y no deja su
línea `✓ Voz clonada`. Con `--denoise` se añade la etapa `Limpiando el ruido…`
(`✓ Ruido limpiado`) y cada etapa conserva su línea.

La única referencia de habla produce un `reference.qvoice` (bajo
`data_dir()/voices/mi_voz/`); el WAV de referencia no se persiste.

A partir de ese momento la voz aparece en `voice list` y puede usarse con
`speech say --voice mi_voz`.

El clonado **extrae la representación de la voz** con el modelo Base y la guarda
como `reference.qvoice`; toda síntesis posterior con `--voice mi_voz` la carga
desde disco, sin volver a procesar los audios de referencia. Por eso `voice clone`
requiere el modelo Base provisionado (`ai-voice-interconnector setup`).
Si hay un [daemon](#modo-daemon) activo, el clonado corre en él y además precalienta
la voz nueva en segundo plano (`"precomputed": true` con `--json`); en modo directo
`precomputed` es siempre `false`. El daemon reutiliza su motor residente solo
mientras la voz conserva el mismo contenido, así que una voz re-clonada con el
mismo nombre se recarga en la siguiente síntesis.

**Re-clonar una voz reemplaza también sus audios sintetizados.** Los audios guardados
con `speech synthesize` se grabaron con el contenido anterior de la voz, así que
`--force` los borra y el resultado los nombra (`removed_speech` con `--json`; en
texto, el cierre añade ` · se borraron N audios sintetizados anteriores`). Si
la voz tiene audios sintetizados, el comando muestra el plan y pide confirmación antes de
capturar o clonar; sin terminal exige `--yes`, y sin él sale con exit 2
(`confirmation_required`) sin cambiar nada.

Si la referencia no es un WAV válido o está truncada, el comando termina con el
código `invalid_audio` (exit 2) y no registra la voz; si falla la lectura del
archivo, con `io_error` (exit 1); si falla el propio clonado, con `voice_clone_failed`.

**Opciones:**
- `--name, -n` (requerido): Nombre para la voz
- `--speech-reference, -s` (requerido): Audio de referencia de habla (10+ segundos de habla limpia); es el único audio de referencia
- `--force, -f`: Sobrescribir la voz si ya existe (incluida una de fábrica homónima); borra también sus audios sintetizados guardados
- `--yes, -y`: No pedir confirmación al borrar los audios sintetizados de la voz sobrescrita
- `--daemon` / `--no-daemon`: igual que en las sub-acciones de `speech`;
  con `--daemon` el precómputo aprovecha el modelo caliente; sin flags se
  sondea el daemon y se usa solo si responde
- `--json`: Emitir el resultado como JSON (nombre y rutas registradas; ver la
  referencia de esquemas más arriba)

**Requisitos del audio:**
- Duración: 10+ segundos recomendados para `--speech-reference`
- Idioma: Español
- Calidad: Sin ruido de fondo, habla clara
- Formato: WAV 16-bit

---

#### `voice list`

Lista las voces disponibles, tanto las de fábrica como las registradas por ti.

```bash
ai-voice-interconnector voice list
ai-voice-interconnector voice list --json
```

**Qué esperar:**

```
Voces:
  default (de fábrica)
  mi_voz
```

La voz `default` siempre está presente (viene de fábrica).

---

#### `voice remove`

Elimina una voz registrada por el usuario **junto con sus audios sintetizados guardados**.

```bash
ai-voice-interconnector voice remove --name mi_voz
ai-voice-interconnector voice remove --name mi_voz --yes
```

**Qué esperar:**

```
Se borrará la voz mi_voz y sus 2 audios sintetizados:
  voz: <ruta> (…)
¿Borrar la voz mi_voz y sus 2 audios sintetizados? [s/N] s
✓ Voz mi_voz borrada en 0,1 s · y 2 audios sintetizados
```

El comando pregunta siempre, tenga o no audios sintetizados: muestra el plan y
pide confirmación `[s/N]` (por defecto, no; sin audios, la pregunta es
`¿Borrar la voz mi_voz?`). Sin terminal exige `--yes/-y`; sin él sale
con exit **2** (`confirmation_required`) y no borra nada. Si cancelas, sale con exit
**0** y `Cancelado; no se ha borrado nada.`.

Las voces de fábrica (como `default`) son de solo lectura y no pueden
eliminarse; el comando lo indica y termina con error si lo intentas. Un nombre
ilegal —vacío, con caracteres fuera de `[A-Za-z0-9._-]`, `.` o `..`— sale con exit
**2** (`invalid_voice_name`) sin tocar ningún archivo: `voice remove --name .` no
borra el almacén.

---

### `translate`

Traduce texto `es↔en`, aislado de la síntesis: sin voz ni modelo TTS de por
medio. `--source-language` y `--target-language` son **opcionales con
defaults** (`es` y `en`) y **estrictos**: solo aceptan `es` o `en` (el parser
rechaza cualquier otro valor con exit 2 antes del handler).

```bash
ai-voice-interconnector translate --text "Hola, ¿cómo estás?" --source-language es --target-language en
ai-voice-interconnector translate --text "Hello there" --source-language en --target-language es --json
```

**Qué esperar:**

```
stdout: Good morning.
stderr: Traducción es → en · local
        ✓ Traducción completada en 0,5 s
```

La traducción es la única etapa del comando y no deja su línea `✓ Traducido`; con
`--json` no hay cierre y esa línea sí sale por stderr.

Con `--json`, emite `{"translated", "source_language", "target_language"}` (ver la referencia
de esquemas) y nada por stdout salvo ese objeto.

**Opciones:**
- `--text`/`-t` (requerido): Texto a traducir, sin límite de longitud
- `--source-language` (opcional, default `es`): Idioma de origen del texto (`es` o `en`)
- `--target-language` (opcional, default `en`): Idioma destino de la traducción (`es` o `en`)
- `--json`: Emite `{"translated", "source_language", "target_language"}`

**Passthrough:** si origen y destino coinciden, devuelve el texto intacto sin
cargar ningún modelo. El modelo de traducción exigido es `opus-mt-<par>`, con sus
cinco ficheros presentes y de más de 0 bytes. Si no está provisionado, falla
con exit **4** y una línea `Solución:` que indica `ai-voice-interconnector setup`; si la traducción falla con el
modelo ya cargado, sale con exit **9**.

---

### `cleanup`

Limpia los datos del proyecto de forma **granular** (fuente de verdad: `docs/CLI/CONTRACT.md §11`). Es la contraparte de `setup` y completa
el ciclo de vida instalación→desinstalación. **Sin flags → exit `2` `usage_error` sin borrar.**

```bash
ai-voice-interconnector cleanup --voices              # voces no-fábrica + arrastre speech/<voz> (excepto default)
ai-voice-interconnector cleanup --synthetic-speech    # raíz speech/ entera (incluye default)
ai-voice-interconnector cleanup --model               # la raíz de modelos: entera si es exclusiva, o solo lo atribuible si es compartida
ai-voice-interconnector cleanup --all                 # las tres categorías + configuración, logs y estado del daemon (sin programa ni PATH)
ai-voice-interconnector cleanup --all --dry-run       # lista sin borrar (exit 0, --json con status planned y su recibo planned)
ai-voice-interconnector cleanup --voices --yes        # omite confirmación ( -y alias)
```

**Qué esperar:** según el flag, borra selectivamente `data_dir()/voices` (preservando `FACTORY_VOICES`), `data_dir()/speech`, o la raíz de modelos. En la raíz de modelos **exclusiva** el borrado es de directorio entero (snapshots, locks y `xet` cuelgan de ella); si el usuario eligió una caché HF compartida con `HF_HUB_CACHE`/`HF_HOME`, solo se borran los repos de `MODEL_REVISIONS` (`Qwen/Qwen3-TTS…`, `CristianRojaas/opus-mt-*-ct2-int8`, `istupakov/parakeet-tdt-0.6b-v3-onnx`) y sus locks, y **`xet` y el `.locks` completo se conservan y se anuncian** (`doctor` dice si la raíz es compartida con `models.shared_root`). `--all` es la unión de las tres categorías **sin programa ni PATH** — solo `self uninstall` borra el programa y el `PATH`. El borrado es quirúrgico: nunca toca modelos de otros proyectos en una caché compartida. `--dry-run` escribe el plan por stdout sin borrar **y sin tomar el bloqueo** (y termina con `Simulación: no se ha modificado nada.` por stderr); `--yes/-y` omite la confirmación interactiva (`¿Borrar lo indicado? [s/N]`), y **sin terminal es obligatorio**: sin él sale con `confirmation_required` (2) y no borra nada. Al borrar cierra con `✓ Limpieza completada en <total> · N rutas borradas` por stderr, y si no hay nada que borrar, con `✓ No había nada que limpiar.` sin pedir confirmación. Con `--json` emite `{"schema_version":"6","status":"cleanup_complete","reason":null,"removed":[...],"planned":[...]}` al borrar de verdad, y `{"schema_version":"6","status":"planned","reason":null,"removed":[...],"planned":[...]}` con `--dry-run` (el mismo plan como recibo, sin `dry_run`: el `planned` ya dice que nada se modificó). Todo es recuperable: `setup` re-descarga los modelos y
`voice clone` vuelve a clonar voces.

---

## Ciclo de vida de la instalación (`self`)

**Canal nativo (los tres SO), en un comando**: `ai-voice-interconnector self uninstall`
borra el estado, revierte la integración de `PATH` y borra el programa, **en ese orden**.

```bash
ai-voice-interconnector self uninstall --dry-run      # escribe el plan con tamaños por stdout; no borra nada
ai-voice-interconnector self uninstall               # pide confirmación [s/N]
ai-voice-interconnector self uninstall --yes         # no interactivo (obligatorio sin terminal)
ai-voice-interconnector self uninstall --keep-data   # conserva modelos, voces y audios sintetizados
```

**Qué esperar:** el plan lista, con tamaños, lo que se va a borrar y lo que **no** se tocará
—la integración de `PATH` se *retira* (`PATH: se quitará la entrada <ruta> del PATH de tu usuario.`), no se borra el archivo, y una caché HF compartida se
conserva—, con cada ruta etiquetada por su categoría (`modelos: <ruta> (5,9 GB)`). Pregunta
`¿Desinstalar el programa y borrar lo indicado? [s/N]` y cierra con `✓ Desinstalación completada en <total>`
por stderr (o `✓ No había nada que desinstalar.`). Con `--dry-run`, el plan sale por stdout y
`Simulación: no se ha modificado nada.` por stderr. Sin terminal, `--yes` es obligatorio: sin él sale con `confirmation_required`
(exit 2) y no borra nada. Cancelar la confirmación no es un error: `status` `cancelled` y
exit 0. Con `--json` emite `{"schema_version":"6","status":…,"reason":null,"removed":[…],"planned":[…],"path_reverted":bool}`, y con `--dry-run` el `status` es `planned` y `removed` copia `planned` (sin clave `dry_run`).

**Sin `--keep-data` se borra la raíz de datos entera**, no el plan de `cleanup --all`, y la
diferencia es deliberada: `cleanup --all` protege las voces de fábrica porque van embebidas
en el binario y el programa sigue instalado, pero **al desinstalar desaparece el programa**, y
dejarlas sería residuo dentro de una raíz de propiedad exclusiva. Con `--keep-data` sí se
aplica el plan de `cleanup --all` filtrado (modelos, voces y audios sintetizados fuera; dentro
configuración, logs y estado del daemon), y el programa se borra igual.

- **Linux/macOS**: retira el enlace `~/.local/bin/ai-voice-interconnector` (solo si apunta al
  directorio de programa) y los bloques delimitados de los perfiles, y borra
  `~/.local/opt/ai-voice-interconnector/`.
- **Windows**: borra `%LOCALAPPDATA%\Programs\ai-voice-interconnector` y quita su entrada del
  `PATH` de usuario (`HKCU\Environment`) conservando el **tipo** del valor y las entradas
  `%VAR%`, y difundiendo `WM_SETTINGCHANGE`. Si el ejecutable en uso está dentro del
  directorio, un proceso auxiliar lo borra al terminar el comando y el `status` es
  `removal_scheduled` (que es éxito). Si el borrado no se puede programar, el resto se completa y
  el comando termina con `reason` `program_dir_kept` y código 22.
- **Idempotente**: repetirla en un sistema ya limpio termina con éxito y `status`
  `not_installed`.
- **Con Homebrew Cask** la vía idiomática es `brew uninstall --cask --zap
  ai-voice-interconnector`, y el comando responde `externally_managed` (12) con esa
  instrucción.

### Instalar y reparar: `self install`

```bash
ai-voice-interconnector self install                  # instala el bundle del que forma parte el ejecutable
ai-voice-interconnector self install --no-setup       # no provisiona modelos
ai-voice-interconnector self install --no-modify-path # no toca perfiles ni registro
ai-voice-interconnector self install --force          # resuelve un conflicto en la ruta del enlace
```

Ejecutado **desde dentro** del directorio de programa, `self install` **repara** en vez de
instalar: reaplica la integración de `PATH`, los permisos, la limpieza de cuarentena y el
recibo sin copiar archivos. Antes de actuar pregunta `¿Instalar el programa?` (o
`¿Reparar la instalación?`) y, al terminar, cierra por stderr con
`✓ ai-voice-interconnector <versión> instalado en <total>` (o `reparado en`), seguido de dos
detalles atenuados, `PATH: …` y `Modelos: …`. Una instalación nueva muestra la etapa
`Copiando el programa…`; una actualización o reparación, `Reemplazando el programa…`. Un ejecutable **sin bundle alrededor** (por ejemplo
`target\debug`) responde `bundle_invalid` (15) indicando `cargo xtask install`: no es una
instalación rota, es que no hay nada alrededor que instalar.

Si la provisión de modelos falla al final, el programa **queda instalado**, el `status` es
`installed`, el `reason` es `setup_failed`, el código de salida es **11** y el motivo del
fallo viaja anidado en `models_cause`: `network_error` si falló el transporte o el
servidor, o `model_unavailable` si Hugging Face rechaza el modelo fijado. Es un
**éxito parcial**: con `network_error` basta reintentar con `setup`; con
`model_unavailable` reintentar no lo resuelve y hace falta una versión que vuelva a fijar
el modelo.

---

## Actualizar de versión

`self update` actualiza la instalación registrada a la última estable o a una concreta, con verificación de integridad y traspaso al binario nuevo. Los modelos y las voces en el directorio de datos de usuario no se ven afectados por el reemplazo del programa.

```bash
ai-voice-interconnector self update --check              # informa anterior → nueva sin modificar nada
ai-voice-interconnector self update                      # actualiza a la última estable
ai-voice-interconnector self update --version X.Y.Z       # fija la versión objetivo
ai-voice-interconnector self update --force               # reinstala la misma versión o degrada a una anterior
ai-voice-interconnector self update --no-setup --yes      # sin provisión y sin confirmación
```

**Qué esperar:** resolución de la versión objetivo (sin API, con respaldo), comparación semántica (iguales → `already_up_to_date` sin descargar, éxito con 0), resumen y confirmación, descarga con SHA-256 y arranque verificado en staging, parada del daemon con el binario actual, traspaso al binario nuevo con las preferencias del recibo y cierre `✓ Actualizado de <anterior> a <nueva> en <total>` por stderr (abre con `Actualización desde la versión <anterior>`; `--check` escribe por stdout `Hay una versión nueva: A → B.` o `Ya tienes la última versión (V).`). Sin reinicio automático del daemon: si estaba activo, un aviso indica cómo relanzarlo. Sin instalación → `not_installed` (3) con el one-liner; en canal `homebrew` → `externally_managed` (12) con el comando correcto. Un recibo de instalación ilegible hace fallar el comando (`self_update_failed`) con un mensaje que nombra el fichero. Si la provisión del binario nuevo falla, el programa **queda actualizado** con `reason` `setup_failed` y salida **11**: es un éxito parcial, y el consejo depende de la causa en `models_cause` (con `network_error` basta reintentar con `setup`; con `model_unavailable` no). Una versión pedida que no existe en GitHub (404) sale con `release_unavailable` (1), y un fallo del disco local durante la descarga con `io_error` (1).

- **macOS (Homebrew)**: `brew upgrade --cask ai-voice-interconnector`.

Los modelos descargados (en la caché de la aplicación, `~/.cache/ai-voice-interconnector/models` en Linux) se reutilizan tal cual.
Cada versión del binario fija las revisiones exactas de los modelos que usa
(`MODEL_REVISIONS`): si tu caché contiene otra revisión, `setup` la detecta como
no provisionada y descarga la requerida (los archivos que no cambian entre revisiones no se duplican: `snapshots/` los enlaza al mismo blob, con symlinks en Unix y enlaces duros en Windows, salvo en FAT32/exFAT, donde se copian). Tras el reemplazo, el `setup` de la versión nueva poda las revisiones propias obsoletas.

---

## Modo daemon

El daemon mantiene el modelo cargado en memoria, evitando el tiempo de carga en
cada invocación (~15–30 s de overhead). Es el modo recomendado cuando vas a
sintetizar varias veces seguidas.

### Gestión del daemon

```bash
# Iniciar daemon (background; puerto 8765 en loopback por defecto, desviable por instancia con `AVI_DAEMON_PORT`, `0` = efímero)
ai-voice-interconnector daemon start

# Ver estado
ai-voice-interconnector daemon status
ai-voice-interconnector daemon status --json

# Reiniciar
ai-voice-interconnector daemon restart

# Detener
ai-voice-interconnector daemon stop

# Auto-reinicio configurable en caso de crash (supervisado)
ai-voice-interconnector daemon start --auto-restart --max-retries 3
ai-voice-interconnector daemon serve --auto-restart --max-retries 3
```

**Qué esperar:** `daemon start` verifica que los modelos estén provisionados, lanza el servidor en segundo plano
y espera hasta `10s` el resultado de su arranque: listo (el daemon publica su dirección real, que se confirma por `/health`), un fallo con su causa (puerto ocupado, voz inexistente) o la muerte del proceso. Si arranca, escribe el PID file `data_dir()/daemon.pid` (con `resident_pid` y la dirección real) y confirma con
el cierre por stderr cuando el motor está listo: `✓ Daemon listo en <total> · pid N`. «Iniciando el daemon…» dura lo que dura el arranque real y «Calentando el motor…» empieza después, con hasta 10 s de espera. Si ese plazo vence o el calentamiento falla, el daemon sigue en marcha y el propio cierre lo dice en una línea (`✓ Daemon en marcha en <total> · pid N · el motor sigue calentándose; ...` o `... · falló el calentamiento del motor; ...`), sin aviso aparte; `daemon status` dice cuándo está listo. Si falla, sale con un código que nombra la causa y no deja proceso ni PID file. Luego `daemon status` escribe por stdout una línea como `Daemon: en marcha (pid 1234, puerto 8765, motor listo).`, `Daemon: en marcha, pero no responde: <motivo>.` o `Daemon: detenido.`; en `--json`, estado `running`/`stopped` + `warm` (`warming`/`warm`/`warm_failed`).

Supervisor: con `--auto-restart`, el daemon reintenta hasta `max_retries` (default `3`) tras un crash con backoff `500ms*2^(retries-1)` (0,5 s, 1 s, 2 s…) capado a `4s` y protección por watchdog de supervisión contra bucles de reinicio rápidos; un apagado graceful vía `daemon stop` (`POST /shutdown` + `shutdown_notify`) no reintenta. Tampoco se reintentan los fallos previos a que el daemon esté listo (puerto ocupado, voz inexistente): son de configuración y salen al instante con su código. Sin `--auto-restart`, el daemon es `fail-stop`.

Puerto ocupado: si otro proceso usa o reserva el puerto del daemon, `daemon start`, `daemon restart` y `daemon serve` salen al instante con exit 6 (`port_in_use`) y un mensaje que nombra el puerto. Libera el puerto o arranca el daemon en otro con `AVI_DAEMON_PORT=<puerto>` (`0` = puerto efímero).

Trazas y logs: `RUST_LOG` (sintaxis de `tracing`) fija el nivel de traza de la CLI y de `daemon serve` y manda sobre los valores por defecto; sin ella, la CLI muestra `warn` y superiores (trazas `info`, como la descarga de `setup`, se piden con `RUST_LOG=info`) y el daemon registra `info` de los crates propios. Cada `daemon start`/`restart` escribe `data/logs/daemon_<pid>_<ms>.log` (con el PID del daemon hijo) y, si el arranque falla, el error termina con `Log del daemon: <ruta>`. El motor de síntesis escribe en `data/logs/qwen3-tts_<pid>_<ms>.log`, y la salida de `voice clone` no se vuelca en la terminal. Se conservan los 10 logs más recientes de cada familia, y un log cuyo PID corresponde a un proceso vivo se conserva siempre.

Warmup: tras enlazar la dirección resuelta (default `127.0.0.1:8765`), el daemon precalienta la voz elegida por `--warm-voice` (default `default`) vía `spawn_blocking(warm_voice_engine)` — best-effort, no aborta el arranque si falla (degrada a `warm_failed` pero sigue sirviendo; la primera petición paga el cold-start). Una `--warm-voice` inexistente sí aborta el arranque con exit 3 (`voice_not_found`), antes de enlazar el puerto y de cargar los modelos. El residente TTS es de una sola voz y se reutiliza solo con una voz del mismo contenido: clonar por daemon recalienta la voz nueva (warm-on-clone), evicciónando la anterior, y re-clonar una voz con el mismo nombre la recarga.

`daemon stop` cierra con `✓ Daemon detenido en <total>` —o `✓ No había ningún daemon en marcha.` si no había ninguno, porque decir que se detuvo algo que no corría sería mentir— (parada unificada de daemon y residente con verificación y borrado de `daemon.pid` y `daemon.ready`; si el residente registrado sigue vivo, la parada no se da por completa y sale por veredicto con `still_running` y exit 5, sin borrar la pista) y `daemon restart` hace la misma parada unificada seguida del mismo lanzamiento que `daemon start`, con los mismos códigos de fallo.

### Uso con daemon

`speech say` despacha según tres ramas:

- **Sin flags**: sondea el daemon con un health check corto y lo usa si responde;
  si no, cae al modo directo sin error.
- **`--daemon`**: asume el daemon disponible y le envía la síntesis sin sondeo
  previo; si el daemon no responde, sale con 5 (`daemon_unreachable`), sin
  fallback silencioso (ver
  [el despacho al daemon](docs/CLI/CONTRACT.md#5-el-despacho-al-daemon)).
- **`--no-daemon`**: modo directo, sin ningún sondeo.

```bash
# El daemon se usa automáticamente si está disponible
ai-voice-interconnector speech say --text "Hola" --voice mi_voz

# Forzar modo daemon (falla si el daemon no responde)
ai-voice-interconnector speech say --text "Hola" --voice mi_voz --daemon

# Forzar modo directo (sin daemon)
ai-voice-interconnector speech say --text "Hola" --voice mi_voz --no-daemon
```

**Qué esperar** con el daemon activo: `speech say` omite la carga del modelo y
la síntesis empieza de inmediato. Mientras sintetiza, el daemon mantiene viva la
conexión con latidos NDJSON (ver [docs/CLI/CONTRACT.md](docs/CLI/CONTRACT.md#transporte-streaming-ndjson-y-presupuesto-de-inactividad));
el cliente muestra cada etapa con su tiempo (traduciendo, sintetizando) y, mientras la síntesis espera, lo dice en la misma línea viva sin cerrarla como hecha: «Esperando turno para sintetizar…», «Calentando el motor…» o «Cargando la voz…» (cuando el motor arranca la voz, por ejemplo al cambiar de voz); y, en cuanto suena la primera muestra, «Reproduciendo…»: el daemon envía el audio en eventos `audio` del mismo flujo, que el cliente reproduce según llegan. Al terminar la reproducción cierra con lo
mismo que en modo directo, sin ruta y por stderr:

```
✓ Reproducción completada en 4,3 s · primer audio 1,2 s
```

Con `--json`, en lugar de esa línea emite el payload
`{"status":"reproduced","voice":…,"first_audio_ms":…,"stalls":…}` en stdout.

---

## Clonación de voz: recorrido completo

De principio a fin, desde grabar tu voz hasta escucharla sintetizada:

```bash
# 1. Graba un audio en español (WAV 16-bit, sin ruido de fondo):
#    habla.wav   - 10+ segundos de habla limpia y continua

# 2. Clona la voz
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav
# → ✓ Voz mi_voz clonada en 23,7 s

# 2b. Con --json, el único artefacto que se guarda
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav --json
# → {"name":"mi_voz","speech":"…/voices/mi_voz/reference.qvoice","precomputed":false,"removed_speech":[]}

# 3. Verifica que aparece
ai-voice-interconnector voice list
# → Voces: default (de fábrica), mi_voz

# 4. Escúchala
ai-voice-interconnector speech say --text "Hola, esto es una prueba" --voice mi_voz
# → etapas de síntesis + reproducción por los altavoces

# 5. O genera un archivo
ai-voice-interconnector speech synthesize --text "Hola, esto es una prueba" --label prueba --voice mi_voz
# → stdout: <ruta del WAV persistido>; stderr: ✓ Síntesis completada en 2,1 s · 3,4 s de audio
```

Lo que queda en disco es **un único artefacto por voz**, `voices/<nombre>/reference.qvoice`:
se genera a partir de la referencia de habla y no se guarda ninguna copia del
WAV de referencia. `voice clone` cierra con `✓ Voz <nombre> clonada en <total>` por stderr, o con
`--json` emite el envelope de arriba por stdout, cuyo `speech` es esa ruta y cuyo `precomputed` es `false` en la
vía directa (el motor es efímero por proceso): solo la vía del daemon lo deja en `true`.

La voz queda guardada de forma permanente: en futuras sesiones basta con
`--voice mi_voz`, sin volver a clonar nada.

---

## Experiencia unificada entre sistemas operativos

Todos los casos de uso de esta guía se ejecutan **con los mismos comandos, la
misma salida y los mismos códigos de retorno** en Windows, Linux y macOS, tanto
desde el binario como desde el código fuente. En concreto:

- **Sintaxis idéntica**: no hay flags ni subcomandos exclusivos de una plataforma.
- **Contrato de salida estable**: los resultados van a stdout y el contexto, las
  etapas, el cierre, los avisos y los errores a stderr, siempre en UTF-8. Esto hace a `ai-voice-interconnector` consumible por
  scripts de forma idéntica en los tres SO.
- **Códigos de salida (contrato público congelado)**: un orquestador distingue la
  causa del fallo sin parsear texto en español. Los valores son estables entre SO
  y versiones, y la tabla completa con su significado está en
  [Códigos de salida](docs/CLI/README.md#códigos-de-salida). Para orientarse: `2` es
  una entrada inválida (`--text` vacío, nombre de voz ilegal, uso incorrecto); `3`,
  una voz o un audio que no existe (`--voice inexistente`); `4`, un modelo sin
  provisionar (`speech say` o `daemon start` antes de `setup`); `5`, un daemon
  inalcanzable (`speech say --daemon` sin daemon); `6`, un conflicto de estado
  (`voice clone` sobre una voz existente sin `--force`); `20`, un fallo de red en
  `setup` o `self update`; y `130`, una interrupción con Ctrl+C. Los códigos del `11`
  al `22` pertenecen al ciclo de vida, y el `11` hay que leerlo con cuidado: **no es un
  error**, es un éxito parcial con el programa ya instalado y la provisión de modelos
  reintentable con `setup` salvo que su causa sea `model_unavailable`.
- **La voz `default` y el modelo** son los mismos en todas las plataformas: el
  audio generado para un mismo texto y voz es equivalente en cualquier SO.
- **El motor de audio** es nativo por SO (`cpal`: WASAPI/CoreAudio/ALSA); no
  requiere configuración ni selección de backend por el usuario.

Las diferencias por plataforma no cambian los comandos ni los códigos de
salida, pero algunas se ven en la salida, en el rendimiento o en lo que el
sistema debe aportar:

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| Reproducción de audio | cpal (WASAPI) | cpal (ALSA) | cpal (CoreAudio) |
| Enumeración de dispositivos (`devices`) | cpal; el nombre es el descriptivo del sistema (`FriendlyName`) | cpal; sigue el criterio de `aplay -L` y puede listar más entradas | cpal; no abre el micrófono para enumerar |
| Paralelismo de la traducción (CTranslate2) | OpenMP (`vcomp140.dll`, incluida en el archivo) | OpenMP (enlazado en el binario) | Sin OpenMP: sin paralelismo dentro de cada operación, más lenta con el mismo resultado |
| Señales que paran `daemon serve` | Ctrl+C | Ctrl+C y `SIGTERM` | Ctrl+C y `SIGTERM` |
| Identidad de proceso del daemon | API del sistema | `/proc/<pid>/stat` | `proc_pidinfo` |
| Librerías que hay que instalar | Ninguna | Ninguna (glibc ≥ 2.28; sin ALSA solo falla el audio) | Ninguna (macOS 26 o posterior) |
Las rutas por SO de las voces de usuario y de la caché del modelo están en
[Ubicaciones en disco por SO](docs/BUILD.md#ubicaciones-en-disco-por-so) (con el efecto
de `HF_HUB_CACHE` y `HF_HOME`), y la del directorio del programa, en la
[tabla de rutas del ciclo de vida](docs/specs/LIFECYCLE.md).

---

## Formato de audio

- **Generación**: 24000 Hz, Mono
- **Exportación WAV**: 16-bit PCM, 24000 Hz, Mono

## Variables de entorno (`AVI_*`)

Estas variables ajustan el comportamiento de la CLI. Todas son opcionales.

| Variable | Efecto |
|---|---|
| `AVI_NO_PROGRESS` | Silencia las **etapas** por stderr: la línea activa, las líneas terminadas y los latidos. Cualquier valor no vacío la activa; el valor vacío no hace nada. **Solo afecta a las etapas**: la línea de contexto, los resultados, el cierre, el envelope `--json` y los códigos de salida no cambian. Útil en un pipeline o en un log donde el progreso estorba. |
| `NO_COLOR` | Con cualquier valor no vacío, desactiva el color de la salida. Sin ella, el color (verde, rojo, amarillo, atenuado) solo se emite cuando el flujo es una terminal que lo admite; redirigida a un archivo o a una tubería, la salida nunca lleva color. |
| `AVI_PUSH_TO_TALK_MAX_SECS` | Techo de seguridad de la grabación push-to-talk, en segundos (default 300). Al alcanzarlo, la grabación se detiene y se devuelve lo grabado con exit 0. |
| `AVI_DAEMON_PORT` | Puerto en el que escucha el daemon. `0` elige un puerto efímero. |
| `AVI_DATA_DIR` | Directorio de datos de la instancia, para aislar varios equipos en la misma máquina. |
| `RUST_LOG` | Filtro de trazas (`info`, `debug`, …). Sin ella, la CLI muestra `warn` y superiores. |

## Solución de problemas

### "El nombre de voz '…' no es válido" (exit 2)

Los nombres de voz y las etiquetas solo admiten letras ASCII, dígitos, `.`, `_` y
`-`, y no pueden estar vacíos ni ser `.` o `..`. Una voz ilegal sale con
`invalid_voice_name` y una etiqueta ilegal con `invalid_identifier`, sin tocar
ningún archivo. Usa un nombre como `mi_voz` o `saludo-1`.

### `speech synthesize` sale con exit 6 `label_exists`

La etiqueta ya está guardada para esa voz. Para sobrescribirla, añade `--force`;
para conservarla y que la orden sea idempotente —el caso de la pre-síntesis de
avisos—, añade `--skip-existing`, que sale con exit 0 y `status` `skipped`.

### "Falta el modelo de …" (exit 4)

Todos los errores salen como `Error: <qué pasó>.` y, cuando hay remedio, una línea
`Solución: <qué hacer>.` con la orden concreta; para un modelo ausente la solución es
`ai-voice-interconnector setup`. `speech say`, `speech synthesize`, `daemon` y
`translate` exigen los modelos pinneados provisionados, y nunca los descargan por
sí mismos. Provisiónalos con:

```bash
ai-voice-interconnector setup
```

### "GLIBC_2.28 not found" (o similar) al ejecutar el binario en Linux

El binario Linux requiere **glibc ≥ 2.28** (Debian 10+, Ubuntu 20.04+, RHEL,
Rocky y Alma 8+, Fedora 29+ o equivalente): se compila contra glibc 2.28 y
`crt-static` no enlaza glibc estáticamente. En una distro más antigua (p. ej.
CentOS 7, Debian 9) el binario no arranca — el bootstrap comprueba que el binario arranca
antes de delegar y aborta con `binary_incompatible` (lo instalado queda intacto).
Actualiza la distro o compila desde código fuente en tu distro actual (ver
[docs/BUILD.md](docs/BUILD.md)).

Sin ALSA el binario arranca igual: solo `devices` y las órdenes que reproducen
o capturan audio fallan, con un error que nombra la librería de ALSA ausente,
el código `audio_backend_unavailable` y exit 3 (un recurso ausente del
sistema, no un fallo del programa).
Si necesitas audio en ese sistema, instala el paquete de runtime de ALSA con el
gestor de paquetes de la distribución.

### El binario no arranca en macOS (`binary_incompatible`)

El binario de macOS exige **macOS 26 (Tahoe)** o posterior: en una versión
anterior el sistema rechaza cargarlo, y el bootstrap aborta con
`binary_incompatible` nombrando la versión mínima (lo instalado queda intacto).
Actualiza macOS: macOS 26 corre en todos los Mac con Apple Silicon.

### "La voz 'x' no existe." (exit 3)

Verifica que la voz existe:

```bash
ai-voice-interconnector voice list
```

### "La voz 'x' ya existe"

`voice clone` no sobrescribe voces por accidente. Si quieres reemplazarla:

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav --force
```

Si la voz tiene audios sintetizados guardados, `--force` los borra con ella y pide
confirmación; en un script, añade `--yes`.

### "El modelo Base de clonado TTS no está provisionado"

`voice clone` requiere el modelo Base, que `setup` provisiona siempre. Sin
él (instalación incompleta), el comando falla con `model_missing` antes de tocar el store:

```bash
ai-voice-interconnector setup
ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav --force
```

### "La voz 'x' no se puede eliminar." (exit 2)

La voz de fábrica (`default`) no puede eliminarse con
`voice remove`: el comando sale con exit 2 y el código `cannot_remove_default`.
Si quieres reemplazar su sonido, vuelve a clonarla con el mismo nombre usando
`voice clone --force`: su referencia se sustituye en el mismo directorio.

### Sin audio de salida

1. Verifica que `ai-voice-interconnector devices` detecta tu dispositivo y
   marca una salida con «predeterminada»
2. Comprueba que el volumen del sistema no está en mute
3. Verifica que el dispositivo de audio predeterminado es correcto
4. Sin salida predeterminada (p. ej. sesiones remotas o headless), la CLI
   termina con el código `no_output_device` y exit 3; si la reproducción falla
   por otra causa, termina con `playback_failed` y el mensaje del sistema de
   audio indica la causa

En un host sin audio puedes seguir usando la síntesis a archivo
(`ai-voice-interconnector speech synthesize --text T --label L`); ni `setup` ni
`doctor` dependen del audio.

### El sistema bloquea el primer arranque (binarios sin firmar)

Es esperable en el primer arranque de un binario descargado por navegador: los pasos para continuar están en [Primer arranque en el README](README.md#primer-arranque-smartscreen--gatekeeper), y el porqué, en [SECURITY.md](SECURITY.md#artefactos-sin-firmar).
