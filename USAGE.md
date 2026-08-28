# Guía de Uso de AI Voice InterConnector

## Tabla de contenidos

- [Instalación](#instalación)
  - [Usuario del binario](#usuario-del-binario)
  - [Compilar desde el código fuente (Rust)](#compilar-desde-el-código-fuente-rust)
  - [Desarrollador (desde el código fuente)](#desarrollador-desde-el-código-fuente)
- [Primer uso: provisionar el modelo (`setup`)](#primer-uso-provisionar-el-modelo-setup)
- [Comandos](#comandos)
  - [Referencia de esquemas `--json`](#referencia-de-esquemas-json)
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
  - [`voice clone`](#voice-clone)
  - [`voice list`](#voice-list)
  - [`voice remove`](#voice-remove)
  - [`translate`](#translate)
  - [`cleanup`](#cleanup)
- [Desinstalación completa](#desinstalación-completa)
- [Actualizar de versión](#actualizar-de-versión)
- [Modo Daemon](#modo-daemon)
  - [Gestión del daemon](#gestión-del-daemon)
  - [Uso con daemon](#uso-con-daemon)
  - [Requisitos de hardware](#requisitos-de-hardware)
- [Clonación de voz: recorrido completo](#clonación-de-voz-recorrido-completo)
- [Experiencia unificada entre sistemas operativos](#experiencia-unificada-entre-sistemas-operativos)
- [Formato de Audio](#formato-de-audio)
- [Solución de Problemas](#solución-de-problemas)
  - ["modelo ... no provisionado (exit 4)"](#modelo--no-provisionado-exit-4)
  - ["GLIBC_2.35 not found" (o similar) al ejecutar el binario en Linux](#glibc_235-not-found-o-similar-al-ejecutar-el-binario-en-linux)
  - ["OneDrive user-data-dir" [WARN] en doctor (Windows)](#onedrive-user-data-dir-warn-en-doctor-windows)
  - ["Voice 'x' not found"](#voice-x-not-found)
  - ["La voz 'x' ya existe"](#la-voz-x-ya-existe)
  - ["timbre-reference.wav/speech-reference.wav not found"](#timbre-referencewavspeech-referencewav-not-found)
  - ["Voz 'x' es una voz de fábrica (solo lectura)"](#voz-x-es-una-voz-de-fábrica-solo-lectura)
  - [Error al eliminar una voz: "uno de sus archivos parece estar en uso"](#error-al-eliminar-una-voz-uno-de-sus-archivos-parece-estar-en-uso)
  - [Sin audio de salida](#sin-audio-de-salida)
  - [El sistema bloquea el primer arranque (binarios sin firmar)](#el-sistema-bloquea-el-primer-arranque-binarios-sin-firmar)
- [Uso ético y responsable](#uso-ético-y-responsable)
- [Licencia](#licencia)

AI Voice InterConnector es un sintetizador de voz (TTS) 100 % local con clonación de voz en
español latinoamericano. Esta guía recorre cada caso de uso desde la perspectiva
del usuario: qué comando ejecutar, qué ocurre y qué salida esperar.

Todos los comandos funcionan **de forma idéntica en Windows, Linux y macOS**: la
misma sintaxis, la misma salida y los mismos códigos de retorno. Las diferencias
internas por plataforma (backend de reproducción, ubicación de datos) se detallan
en [Experiencia unificada entre sistemas operativos](#experiencia-unificada-entre-sistemas-operativos).

## Instalación

Hay dos flujos según la audiencia: el del **usuario del binario** (canal nativo:
one-liner o descarga desde Releases) y el del **desarrollador** (compila con
`cargo` desde el código fuente). El canal PyPI fue retirado; detalle en
[docs/DISTRIBUTION.md](docs/DISTRIBUTION.md).

### Usuario del binario

Instala el ejecutable de tu plataforma desde Releases y déjalo accesible en el
PATH (en Windows el instalador lo agrega automáticamente al PATH de usuario,
HKCU). Luego invoca:

```bash
ai-voice-interconnector <comando>
```

En **Linux**, `install-linux.sh` automatiza toda la descarga/verificación/instalación
con una sola línea (detalle en [README.md](README.md#instalación-de-una-línea)):

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-linux.sh | sh
```

En **Windows**, `install-windows.ps1` hace lo análogo desde PowerShell (instalación
per-user, sin UAC; termina ejecutando `ai-voice-interconnector setup`):

```powershell
irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/main/install-windows.ps1 | iex
```

**Desinstalación limpia**, en **un comando** en los tres SO: `ai-voice-interconnector
uninstall` encadena la limpieza de datos (`data_dir()` + snapshots HF), revierte la
integración de PATH y borra el binario, en ese orden. Usa `uninstall --force` (o
`cleanup --all`) para omitir la confirmación. Con Homebrew Cask, la vía idiomática es
`brew uninstall --cask --zap`. Ver «Desinstalación completa» más abajo.

### Compilar desde el código fuente (Rust)

```bash
# Requisitos: Rust 1.96, cmake, pkg-config (libasound2-dev/libclang-dev en Linux)
cargo build --release --features full
./target/release/ai-voice-interconnector <comando>
```

A partir de aquí, todos los ejemplos usan `ai-voice-interconnector <comando>`; si trabajas
desde el código fuente, sustituye por `cargo run -- <comando>` o ejecuta el binario de
`target/release/`. El comportamiento es el mismo. Detalle completo en
[docs/BUILD.md](docs/BUILD.md) y [CONTRIBUTING.md](CONTRIBUTING.md).

## Primer uso: provisionar el/los modelo(s) (`setup`)

`setup` descarga **los 4 modelos base + 1 opt-in** desde HuggingFace Hub de forma nativa
(crate `hf-hub`, TLS rustls; sin Python) a la caché canónica
(`~/.cache/huggingface/hub`; respeta `HF_HUB_CACHE`/`HF_HOME` si las defines):

| Modelo | Repo HF | Uso |
|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Síntesis TTS |
| `marian-es-en` / `marian-en-es` | `Helsinki-NLP/opus-mt-*` | Traducción es↔en |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (~600 MB, ONNX int8) |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` (~2,5 GB, opt-in) | Clonado de voz (Base) |

Las revisiones están pineadas por commit hash en `MODEL_REVISIONS`
(`crates/avi-store/src/lib.rs`): mismo binario → mismos pesos. El Base es opt-in por peso (~11,5 GB total).

```bash
ai-voice-interconnector setup                        # descarga los 4 base (idempotente)
ai-voice-interconnector setup --with-base            # incluye Base para voice clone (~2,5 GB)
ai-voice-interconnector setup --with-stt             # aceptado; redundante: STT ya va incluido
ai-voice-interconnector setup --with-base --with-stt # ambos flags combinables
```

**Qué esperar:** barra de progreso por bytes con ETA, resume automático si se
interrumpe, e índice de estado en `data_dir()/models/<name>/manifest.json`.
Si lo vuelves a ejecutar con los snapshots presentes, termina al instante sin
descargar nada. La limpieza posterior corresponde a `cleanup` (snapshots + datos)
o `uninstall --force` (además binario + PATH).

**Provisión por SO** (experiencia homóloga):

- **Windows**: `install-windows.ps1` registra el directorio en el PATH de usuario
  (HKCU) y ejecuta `setup` al terminar.
- **Linux / macOS**: `install-linux.sh` / `install-macos.sh` crean el symlink
  `~/.local/bin/ai-voice-interconnector` y encadenan `setup` al terminar.
  Si `~/.local/bin` no está en tu PATH, el instalador te lo avisa con la línea
  exacta a añadir al shell profile.

> **Importante**: hasta que los modelos estén provisionados, `speech say` y `daemon start`
> **abortan de inmediato** (exit 4) con un mensaje que remite a `ai-voice-interconnector setup`. Nunca
> disparan una descarga silenciosa.

## Comandos

Tanto los comandos de lectura (`version`, `doctor`, `devices`, `voice list`,
`daemon status`) como los de escritura (`voice clone`, `voice remove`, `setup`,
`cleanup`) aceptan `--json` para salida legible por máquina, útil al invocar
`ai-voice-interconnector` desde otro programa: ningún comando obliga a parsear texto.

Todo payload `--json` incluye el campo **`"schema_version"`** (actualmente
`"3"`), que identifica la forma del esquema. Es un campo aditivo: añadir claves
nuevas no lo incrementa; solo un cambio incompatible de las claves existentes lo
haría. Un consumidor puede leerlo para detectar cambios de contrato.

### Referencia de esquemas `--json`

Los payloads siguientes son **parte del contrato programático**: sus claves son
estables (los cambios solo pueden ser aditivos mientras `schema_version` sea
`"3"`). En todos los casos, stdout contiene exactamente un objeto JSON y el
diagnóstico/progreso va a stderr. La clave `schema_version` (string) se omite de
las tablas por brevedad: está presente en todos.

**`speech synthesize --json`** — el bucle interactivo de `--play` es
incompatible con `--json` (exit 2 si se combinan), así que bajo `--json` la
persistencia es siempre cierta cuando la salida es 0. El payload es idéntico
campo a campo en modo directo y vía daemon.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | Siempre `"success"` |
| `audio_path` | string | Ruta del WAV en el almacén (`speech/<voz>/<etiqueta>.wav`) |
| `voice` | string | Nombre de la voz efectivamente usada (`"default"` si no se dio `--voice`) |

**`speech say --json`** — no persiste nada; emite ruta temporal y voz.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | Siempre `"reproduced"` |
| `audio_path` | string | Ruta del WAV temporal reproducido |
| `voice` | string | Nombre de la voz efectivamente usada (`"default"` si no se dio `--voice`) |

**`speech play --json`** / **`speech remove --json`** — identifican la locución y el resultado.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"played"` / `"removed"` |
| `voice` | string | Nombre de la voz de la locución |
| `label` | string | Etiqueta de la locución |

**`speech list --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `speech` | array de objetos | Un objeto por locución guardada: `voice` (string), `label` (string), `text` (string, texto completo sin truncar), `created_at` (string, ISO 8601 UTC), `duration_secs` (number) |

**`daemon start` / `stop` / `restart --json`** — payload de resultado de la
acción (no de estado; para eso está `daemon status --json`). Los mensajes
informativos van a stderr. El éxito o fallo lo transporta el exit code: un
fallo emite el payload de error (`error`) y sale no-cero, no una clave `ok`.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `action` | string | `"start"`, `"stop"` o `"restart"` |
| `pid` | number | Solo en `start`/`restart` con éxito, si el gestor expone el PID del daemon lanzado |

`daemon serve` (servidor en primer plano) no tiene `--json`: su contrato es el
stream NDJSON de `/synthesize`, no un payload de una sola línea.

**`version --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Siempre `"ai-voice-interconnector"` |
| `version` | string | Versión del programa (p. ej. `"0.18.1"`) |

**`doctor --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"ok"` si no hay issues; `"failed"` en caso contrario (exit 1) |
| `data_dir` | string | Ruta del directorio de datos del usuario |
| `hf_cache` | string | Ruta de la caché HF resuelta (auditoría de ubicación) |
| `issues` | array de strings | Descripciones de los chequeos fallidos (vacío si todo correcto) |

**`devices --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `devices` | array de objetos | Un objeto por dispositivo de salida: `id` (number), `name` (string), `latency` (number, segundos) |

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
| `status` | string | `"completed"` |
| `language` | string | El `--language` pedido (aceptado por compatibilidad; el set de modelos es fijo) |
| `with_stt` | boolean | Espejo del flag `--with-stt` (redundante: STT ya va incluido) |
| `with_base` | boolean | Espejo del flag `--with-base` (opt-in Base) |
| `models_provisioned` | array de strings | Los 4 modelos base + 1 opt-in si `--with-base` (`qwen3-tts-0.6b`, `marian-*`, `parakeet-tdt-v3`, `qwen3-tts-0.6b-base`) |

**`cleanup --json` / `uninstall --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"cleanup_complete"` / `"uninstalled"` (uninstall emite `"cancelled"` si se aborta la confirmación) |

**`voice clone --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Nombre de la voz registrada |
| `timbre` | string | Ruta absoluta del `timbre-reference.wav` copiado |
| `speech` | string | Ruta absoluta del `.qvoice` generado (`reference.qvoice`) |
| `precomputed` | boolean | Siempre `false` (el clonado genera `.qvoice` bajo demanda) |

**`voice remove --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"removed"` |
| `voice` | string | Nombre de la voz eliminada |

**`translate --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `translated` | string | Texto traducido (igual al de entrada si `--from == --to`, passthrough) |
| `source` | string | El `--from` pedido |
| `target` | string | El `--to` pedido |

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

Verifica que todos los componentes estén disponibles: la librería TTS, el
subsistema de audio, los modelos descargados y las voces.

```bash
ai-voice-interconnector doctor
ai-voice-interconnector doctor --json
```

**Qué esperar** (entorno sano):

```
Diagnóstico: todo correcto.
Cache HF: C:\Users\<tu-usuario>\.cache\huggingface\hub
```

`doctor` verifica el directorio de datos, los **4 modelos pinneados** (TTS
Qwen3-TTS, traducción Marian es→en y en→es, STT Parakeet TDT v3) y el almacén de
voces. Si falta alguno, lista cada issue con `✗` y remite a
`ai-voice-interconnector setup`.

Termina con código de salida 0 si todo pasa, y 1 si algún chequeo falla.

---

### `devices`

Lista los dispositivos de salida de audio disponibles.

```bash
ai-voice-interconnector devices
ai-voice-interconnector devices --json
```

**Qué esperar:**

```
Dispositivos de salida de audio:
  [0] Altavoces (Realtek High Definition Audio) (latency: 10.0ms)
  [1] Auriculares (latency: 8.0ms)
```

---

### El grupo `speech`

Seis sub-acciones sobre el habla: dos que sintetizan (`synthesize`, `say`), tres que gestionan el almacén de locuciones guardadas (`play`, `list`, `remove`) y una que compone el bucle voz→voz (`dub`). Cada una tiene una sola responsabilidad, y el nombre declara su costo: sintetizar paga GPU y puede exigir el modelo provisionado; gestionar el almacén no.

| Sub-acción | Qué hace | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda una locución | sí | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech dub` | Composición voz→voz: transcribe, traduce si procede, sintetiza y reproduce | no | sí |
| `speech play` | Reproduce una locución guardada | no | no |
| `speech list` | Lista las locuciones guardadas | no | no |
| `speech remove` | Borra una locución guardada | no | no |

**En las seis, `--voice/-v` es opcional**; si se omite, usa la voz de fábrica
`default`. **La voz y la etiqueta (`--label/-l`) se normalizan a minúsculas**
antes de resolver rutas: `--label Saludo` y `--label saludo` son la misma
locución (el archivo se llama `saludo.wav`), y lo mismo aplica al nombre de la
voz.

**Despacho al daemon (`synthesize`, `say`, `transcribe` y `dub`, las que
necesitan un modelo cargado):** tres modos, iguales a los de `voice clone`:
- Sin flags: sondea el daemon y lo usa si responde; si no, sintetiza en modo
  directo (carga el modelo al vuelo).
- `--daemon`: exige el daemon; si no está activo, sale con exit **5** en vez de
  degradar.
- `--no-daemon`: fuerza el modo directo, sin sondear.

`--daemon` y `--no-daemon` son mutuamente excluyentes: combinarlos sale con
exit **2** antes de cualquier trabajo. `speech play`, `speech list` y `speech
remove` no tocan el modelo ni el daemon: no declaran estos flags.

#### `speech synthesize`

Sintetiza texto y lo guarda en el almacén de habla sintética
(`data_root()/synthetic-speech/<voz>/<etiqueta>.wav`); a diferencia de
`speech say`, siempre persiste.

```bash
ai-voice-interconnector speech synthesize --text "Bienvenido" --label saludo
ai-voice-interconnector speech synthesize --text "Bienvenido" --label saludo --voice mi_voz
```

**Qué esperar:** las mismas etapas de síntesis que `speech say` (ver más
abajo) y, al terminar:

```
Locución 'saludo' guardada (voz 'default').
```

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--label, -l` (requerido): Etiqueta de la locución en el almacén (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz a usar (default: `default`)
- `--output, -o`: Copia adicional del WAV a la ruta indicada
- `--play`: Reproduce el WAV tras guardar
- `--force, -f`: Sobrescribe la locución si la etiqueta ya existe para la voz
- `--json`: Emite `{status, audio_path, voice}`

**Colisión de etiqueta:** sin `--force`, guardar sobre una etiqueta que ya
existe para la voz sale con exit **6**, sin gastar GPU (la comprobación es
previa a la síntesis). Con `--play`, la etiqueta se revalida también al
aceptar, por si quedó ocupada mientras el bucle esperaba una respuesta;
`--force` sobre una etiqueta libre es un no-op.

**El bucle de `--play`:** con `--play`, tras sintetizar el audio se reproduce
y aparece un menú de cuatro opciones (por stderr; `--json` es incompatible con
`--play`):

```
¿Qué quieres hacer con esta toma?
  1) Reproducir otra vez
  2) Aceptar y guardar
  3) Rechazar y regenerar
  4) Rechazar y descartar
Opción [1-4]:
```

- **Reproducir otra vez**: repite los mismos bytes en memoria, sin volver a sintetizar.
- **Aceptar y guardar**: persiste la toma que acabas de oír y termina con exit 0.
- **Rechazar y regenerar**: sintetiza otra toma (T3+S3Gen; los conditionals de una voz registrada ya están precomputados) y vuelve a preguntar.
- **Rechazar y descartar**: termina con exit 0 sin guardar nada.

Ctrl-D en la pregunta equivale a «rechazar y descartar». `speech synthesize
--play` requiere una terminal interactiva en la entrada estándar; sin ella,
sale con exit 2 antes de sintetizar.

#### `speech say`

Sintetiza texto y reproduce el audio inmediatamente por los altavoces, sin
guardar nada en el almacén.

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
con Qwen3-TTS y el audio suena por los altavoces. Con el daemon activo, el CLI
delega vía HTTP y el modelo ya está caliente en memoria. Salida típica (stderr):

```
Reproduciendo: C:\Users\<u>\AppData\Local\Temp\avi_say_<pid>.wav
```

**Orígenes de voz (resolución usuario→fábrica):**
- **Fábrica**: voz `default` embebida en el binario (`crates/avi-store/assets/default/`),
  materializada en el primer uso; de solo lectura.
- **Usuario**: voces registradas con `voice clone` (`.qvoice`), escribibles,
  guardadas en `data_dir()/voices/<nombre>/`. Una voz de usuario con el mismo
  nombre que una de fábrica la sobrescribe.

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--voice, -v`: Nombre de la voz a usar (default: `default`)
- `--daemon`: Usar el daemon sin sondeo previo; si falla, el error se reporta (sin fallback a directo)
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

Reproduce una locución ya guardada; no toca el modelo ni el daemon.

```bash
ai-voice-interconnector speech play --label saludo
ai-voice-interconnector speech play --label saludo --voice mi_voz --json
```

**Opciones:**
- `--label, -l` (requerido): Etiqueta de la locución (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--json`: Emite `{"voice", "label"}`

Una etiqueta inexistente para la voz sale con exit **3**.

#### `speech list`

Lista las locuciones guardadas, opcionalmente filtradas por voz.

```bash
ai-voice-interconnector speech list
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech list --json
```

**Qué esperar:**

```
[default] saludo: Bienvenido
[mi_voz] despedida: Hasta luego, gracias por tu visita a nuestra tien...
```

El texto se muestra truncado a 60 caracteres en la salida humana; el payload
`--json` (`{"synthetic_speech": [...]}`) lleva el texto completo. Una locución
sin sidecar de metadatos se muestra como `(sin metadatos)`.

**Opciones:**
- `--voice, -v`: Filtra por voz; sin él lista las locuciones de todas las voces
- `--json`: Emite `{"synthetic_speech": [{"voice", "label", "text", "created_at"}]}`

Con `--voice` apuntando a una voz inexistente, el comando sale con exit **3**
(para que «voz mal escrita» no se confunda con «sin locuciones»).

#### `speech remove`

Borra una locución guardada (el WAV y su sidecar de metadatos, si existen).

```bash
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector speech remove --label saludo --voice mi_voz
```

**Opciones:**
- `--label, -l` (requerido): Etiqueta de la locución (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--json`: Emite `{"voice", "label"}`

Una etiqueta inexistente sale con exit **3**. El borrado masivo es tarea de
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
fija en segundos y solo es válido junto a `--mic`.

```bash
ai-voice-interconnector speech transcribe --audio grabacion.wav --source-language es-latam
ai-voice-interconnector speech transcribe --audio recording.wav --source-language en --json
ai-voice-interconnector speech transcribe --audio recording.wav --source-language en --daemon
ai-voice-interconnector speech transcribe --mic --source-language es-latam
ai-voice-interconnector speech transcribe --mic --duration 5 --source-language en
```

**Qué esperar:**

```
Hola, ¿cómo estás?
```

Con `--mic` y sin `--duration`, el comando espera en silencio a que el
usuario presione Enter antes de transcribir.

Con `--json`, emite `{"text", "source"}` y nada por stdout salvo ese objeto.
`source` es el **token CLI verbatim** de `--source-language` (p. ej.
`es-latam`, sin normalizar a ISO) — a diferencia de `translate --json`, que
emite `source`/`target` como códigos ISO. La divergencia es deliberada: esta
sub-acción pertenece al grupo `speech`, cuyo resto de comandos (`say`,
`synthesize`) también expone `es-latam` en su propia taxonomía de idioma sin
colapsarla a ISO; internamente el idioma sí se resuelve a ISO
(`resolve_language`) antes de invocar el modelo, solo la salida `--json`
preserva el token de entrada.

**Opciones:**
- `--audio`: Ruta del archivo WAV a transcribir (mutuamente excluyente con `--mic`; uno de los dos es requerido)
- `--mic`: Transcribe desde el micrófono en vez de un archivo (mutuamente excluyente con `--audio`; uno de los dos es requerido)
- `--duration N`: Duración fija de grabación en segundos; solo válido junto a `--mic`
- `--source-language` (requerido): Idioma hablado en el audio (`es-latam` o `en`)
- `--daemon` / `--no-daemon`: igual que en `speech say` (despacho de tres modos; la captura del audio siempre ocurre en el cliente)
- `--json`: Emite `{"text", "source"}`

La captura de micrófono es directa a 16 kHz/mono/int16 (formato que Parakeet
asume), sin remuestreo posterior; el backend de captura es `miniaudio`
(único, sin ramas por sistema operativo). El WAV pasado con `--audio`, en
cambio, sí se remuestrea internamente a esos mismos 16 kHz sin importar la
frecuencia de origen del archivo — no requiere ninguna preparación previa.

`--duration` sin `--mic` sale con exit **2** (`EXIT_INVALID_INPUT`). Sin
terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con
exit **2**, porque no hay forma de detectar la pulsación de Enter. Un archivo
de audio inexistente (ruta `--audio`) sale con exit **3**. Si el modelo de
transcripción no está provisionado, falla remitiendo a
`ai-voice-interconnector setup --with-stt` con exit **4**; si la transcripción falla con
el modelo ya cargado, sale con exit **10**. En la ruta daemon, un fallo de
comunicación (daemon inactivo o de versión antigua sin `/transcribe`) sale con
exit **5**.

---

#### `speech dub`

Composición voz→voz: transcribe la entrada hablada (archivo o micrófono),
traduce si `--from` difiere de `--to`, sintetiza con
la voz elegida y reproduce el resultado. Reutiliza las etapas de
`speech transcribe`, la traducción de `speech say`/`synthesize` y el despacho
de síntesis; no guarda nada en el almacén (sin `--label` ni `--json`).

`--audio` y `--mic` son **mutuamente excluyentes y exactamente una de las dos
es requerida**. Con `--mic`, la grabación es **push-to-talk** por defecto
(termina al presionar Enter); `--duration N` fuerza una grabación de duración
fija en segundos y solo es válido junto a `--mic`.

```bash
ai-voice-interconnector speech dub --mic --from es --to en -v mi_voz
ai-voice-interconnector speech dub --audio grabacion.wav --from en --to es
```

**Qué esperar:** transcribe tu habla al texto, lo traduce si procede y
reproduce la síntesis con la voz (`default` si no pasas `-v`). Con `--mic` y
sin `--duration`, el comando espera en silencio a que presiones Enter antes de
transcribir.

**Opciones:**
- `--audio, -a`: Ruta del archivo WAV hablado (mutuamente excluyente con `--mic`; exactamente una de las dos es requerida; alias: `--file`)
- `--mic`: Graba desde el micrófono (mutuamente excluyente con `--audio`; exactamente una de las dos es requerida)
- `--duration N`: Duración fija de grabación en segundos; solo válido con `--mic`
- `--from` (default `es`) / `--to` (default `en`): Idioma hablado y destino (`es`/`en`; si difieren, se traduce antes de sintetizar)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--daemon` / `--no-daemon`: aplican a la transcripción y a la síntesis

`--duration` sin `--mic` sale con exit **2**, y `--mic` sin `--duration` en
una terminal no interactiva (no TTY) también sale con exit **2**. Un `--audio`
inexistente sale con exit **3**. Códigos de fallo de la cadena: exit **4**
(modelo de transcripción no provisionado, remite a
`ai-voice-interconnector setup --with-stt`), **5** (daemon exigido pero inactivo o de
versión antigua sin `/transcribe`), **9** (fallo de traducción con el modelo
cargado) y **10** (fallo de transcripción con el modelo cargado).

---

### `voice clone`

Clona una voz a partir de un audio de referencia (requiere modelo Base).

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav
# Si falta Base: ai-voice-interconnector setup --with-base
```

**Qué esperar:** el comando valida que el audio sea cargable, genera `reference.qvoice` vía
`avi_tts::clone_voice` con el modelo Base, y confirma (error `model_missing` → `setup --with-base`):

```
Iniciando voice_clone...
Voz 'mi_voz' clonada:
  timbre (reference): <ruta>/voices/mi_voz/timbre-reference.wav
  habla (conditioning): <ruta>/voices/mi_voz/speech-reference.wav
  conditionals: precomputados
Finalizado en 3.1s
```

A partir de ese momento la voz aparece en `voice list` y puede usarse con
`speech say --voice mi_voz`.

El clonado **precomputa los conditionals** en el momento de clonar, de modo que
toda síntesis posterior con `speech synthesize --voice mi_voz` (o `speech say --voice mi_voz`) los carga desde disco en vez
de recomputarlos (latencia estable, sin sobrecosto en la primera reproducción).
Por eso `voice clone` requiere el modelo provisionado (`ai-voice-interconnector setup`): el
precómputo ejecuta el modelo. Si hay un [daemon](#modo-daemon) activo, el
precómputo aprovecha el modelo ya caliente y es casi inmediato; si no, el
comando carga el modelo una vez (unos segundos) para precomputar.

Si el precómputo falla (por ejemplo, un audio problemático), el clonado **no se
aborta**: la voz queda registrada con un aviso por stderr y sus conditionals se
computarán en la primera síntesis.

**Opciones:**
- `--name, -n` (requerido): Nombre para la voz
- `--timbre-reference, -t` (opcional): Audio para timbre (cualquier largo — el audio completo se usa para el embedding)
- `--speech-reference, -s` (requerido): Audio para conditioning (10+ segundos de habla limpia)
- `--force, -f`: Sobrescribir la voz si ya existe (incluida una de fábrica homónima)
- `--daemon` / `--no-daemon`: igual que en las sub-acciones de `speech`;
  con `--daemon` el precómputo aprovecha el modelo caliente; sin flags se
  sondea el daemon y se usa solo si responde
- `--json`: Emitir el resultado como JSON (nombre y rutas registradas; ver la
  referencia de esquemas más arriba)

**¿Por qué dos archivos?**
- `--timbre-reference` captura el **timbre** de la voz (cómo suena)
- `--speech-reference` provee el **patrón de habla** (ritmo, entonación)

Pueden ser el mismo archivo si solo tienes una grabación, pero separar ambos da
mejores resultados.

**Requisitos del audio:**
- Duración: 10+ segundos recomendados para `--speech-reference`; `--timbre-reference` puede ser de cualquier largo
- Idioma: Español latinoamericano
- Calidad: Sin ruido de fondo, habla clara
- Formato: WAV 16-bit

---

### `voice list`

Lista las voces disponibles, tanto las de fábrica como las registradas por ti.

```bash
ai-voice-interconnector voice list
ai-voice-interconnector voice list --json
```

**Qué esperar:**

```
Voces registradas:
  - default
  - mi_voz
```

La voz `default` siempre está presente (viene de fábrica).

---

### `voice remove`

Elimina una voz registrada por el usuario.

```bash
ai-voice-interconnector voice remove --name mi_voz
```

**Qué esperar:**

```
Voz 'mi_voz' eliminada.
```

Las voces de fábrica (como `default`) son de solo lectura y no pueden
eliminarse; el comando lo indica y termina con error si lo intentas.

---

### `translate`

Traduce texto `es↔en`, aislado de la síntesis: sin voz ni modelo TTS de por
medio. A diferencia de `--source-language`/`--target-language` en `speech
say`/`speech synthesize` (opcionales, opt-in), aquí `--from` y `--to` son
**ambos requeridos** — traducir es la única función del comando.

```bash
ai-voice-interconnector translate --text "Hola, ¿cómo estás?" --from es --to en
ai-voice-interconnector translate --text "Hello there" --from en --to es --json
```

**Qué esperar:**

```
Good morning.
```

Con `--json`, emite `{"translated", "source", "target"}` (ver la referencia
de esquemas) y nada por stdout salvo ese objeto.

**Opciones:**
- `--text` (requerido, sin alias `-t`): Texto a traducir (mismo límite de 5000 caracteres que `speech say`/`synthesize`)
- `--from` (requerido): Idioma de origen del texto (`es` o `en`, códigos ISO — no `es-latam`)
- `--to` (requerido): Idioma destino de la traducción (`es` o `en`)
- `--json`: Emite `{"translated", "source", "target"}`

**Passthrough:** si `--from` y `--to` coinciden, devuelve el texto intacto sin
cargar ningún modelo. Si el modelo de traducción no está provisionado, falla
remitiendo a `ai-voice-interconnector setup --language en`; si la traducción falla con el
modelo ya cargado, sale con exit **9**.

---

### `cleanup`

Limpia los datos del proyecto: snapshots HF de los modelos pinneados, índice en
`data_dir()/models`, voces y locuciones. Es la contraparte de `setup` y completa
el ciclo de vida instalación→desinstalación.

```bash
ai-voice-interconnector cleanup            # borra data_dir()/models + speech + voices
ai-voice-interconnector cleanup --all      # alias de `uninstall`: además binario + PATH
```

**Qué esperar:** borra `data_dir()/models|speech|voices` y los snapshots HF de
los repos de `MODEL_REVISIONS` (`Qwen/Qwen3-TTS…`, `Helsinki-NLP/opus-mt-*`,
`istupakov/parakeet-tdt-0.6b-v3-onnx`). El borrado es quirúrgico: nunca toca modelos de otros
proyectos en la caché. Todo es recuperable: `setup` re-descarga los modelos y
`voice clone` vuelve a clonar voces.

---

## Desinstalación completa

**Canal nativo (los tres SO), en un comando**: `ai-voice-interconnector uninstall`
(alias: `cleanup --all`) encadena la limpieza de datos (snapshots HF +
`data_dir()`), revierte la integración de PATH y borra el binario, **en ese
orden**. Pide confirmación interactiva salvo con `--force`/`--yes`; cancelar
aborta sin borrar nada (`{"status":"cancelled"}`, exit 0). Con `--json` emite
`{"schema_version","status"}`.

- **Linux**: quita el symlink `~/.local/bin/ai-voice-interconnector`, borra
  `~/.local/opt/ai-voice-interconnector/` y los datos.
- **macOS**: igual que Linux en la vía one-liner; con Homebrew Cask la vía
  idiomática es `brew uninstall --cask --zap ai-voice-interconnector`.
- **Windows**: borra los datos y el directorio
  `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, quita esa entrada del PATH
  de usuario (`HKCU\Environment`) y notifica el cambio al sistema. Si el binario
  está en uso, avisa y deja el borrado final para después de cerrar la terminal.

---

## Actualizar de versión

`ai-voice-interconnector` no tiene auto-actualización: cada nueva versión se instala
manualmente sobre (o junto a) la anterior. Los modelos y las voces en el
directorio de datos de usuario no se ven afectados por la actualización del
binario.

- **Windows**: repite el one-liner `irm | iex`; reemplaza la instalación per-user
  anterior en `%LOCALAPPDATA%\Programs\ai-voice-interconnector` y conserva el PATH.
- **Linux / macOS**: repite el one-liner `curl -fsSL …/install-linux.sh | sh`
  (o `install-macos.sh`); limpia la versión anterior de
  `~/.local/opt/ai-voice-interconnector/`, extrae la nueva y reapunta el symlink
  `~/.local/bin/ai-voice-interconnector`.
- **macOS (Homebrew)**: `brew upgrade --cask ai-voice-interconnector`.

Los modelos descargados (`~/.cache/huggingface/hub`) se reutilizan tal cual.
Cada versión del binario fija las revisiones exactas de los modelos que usa
(`MODEL_REVISIONS`): si tu caché contiene otra revisión, `setup` la detecta como
no provisionada y descarga la requerida (la caché deduplica por contenido).

---

## Modo Daemon

El daemon mantiene el modelo cargado en memoria, evitando el tiempo de carga en
cada invocación (~15–30 s de overhead). Es el modo recomendado cuando vas a
sintetizar varias veces seguidas.

### Gestión del daemon

```bash
# Iniciar daemon (background; puerto fijo: 8765 en loopback, no configurable)
ai-voice-interconnector daemon start

# Ver estado
ai-voice-interconnector daemon status
ai-voice-interconnector daemon status --json

# Reiniciar
ai-voice-interconnector daemon restart

# Detener
ai-voice-interconnector daemon stop

# Auto-reinicio en caso de crash
ai-voice-interconnector daemon start --autorestart --max-retries 3

# Precargar solo un idioma (default: all = ambos, es-latam y en)
ai-voice-interconnector daemon start --language es-latam

# Precargar también el modelo de transcripción (requiere setup --with-stt; opt-in)
ai-voice-interconnector daemon start --with-stt
```

**Qué esperar:** `daemon start` verifica que los modelos estén provisionados, lanza el servidor en segundo plano
con `spawn_background` + PID file `data_dir()/daemon.pid` + poll `30×200ms`, y confirma con
`Daemon iniciado correctamente (pid ...)`. Luego `daemon status` muestra:

```
Daemon en ejecución:
  Estado: healthy
  Modelos cargados: es-latam, en
  Tiempo activo: 42.3s
```

«Modelos cargados» lista los idiomas calientes en RAM (los precargados al
arrancar); un idioma no listado se cargaría perezosamente en la primera
síntesis que lo pida.

Tras precargar los pesos, `daemon start` ejecuta además una **síntesis
descartable por idioma precargado** con la voz de fábrica. La precarga solo
carga los pesos, nunca ejecuta un forward, así que la inicialización perezosa
del runtime (contexto CUDA + autotune cuDNN en GPU; pool oneDNN/MKL en CPU) se
dispararía recién en la primera síntesis real como latencia sorpresa; el warmup
la paga en el arranque —que ya se asume lento—. Es best-effort: un warmup
fallido (por ejemplo, la voz de fábrica ausente) se registra y no aborta el
arranque.

Con `--language en`/`all` (default), `daemon start` además precarga en RAM el
par de traducción `opus-mt` es↔en (ambas direcciones), calentándolo desde el
arranque en vez de esperar a la primera síntesis con `--source-language`
distinto de `--target-language`; `daemon status --json` lo expone como la
clave `"translate:es-en"` de `model_loaded` (ver la referencia de esquemas).

Con `--with-stt`, `daemon start` precarga en RAM el modelo de transcripción
`parakeet-tdt-v3` (opt-in y simétrico a `setup --with-stt`; sin el flag,
la primera transcripción vía daemon paga la carga fría). Exige el modelo
provisionado en disco: si falta, `daemon start --with-stt` sale con exit **4**
remitiendo a `ai-voice-interconnector setup --with-stt`, sin lanzar el proceso;
`daemon status --json` lo expone como la clave `"transcribe:small"` de
`model_loaded`.

`daemon stop` responde `Daemon detenido` (borra `daemon.pid` incluso si ya estaba caído) y `daemon restart` orquesta `POST /shutdown` → espera caída `5s` → `spawn_background` → poll `running`.

### Uso con daemon

`speech say` despacha según tres ramas:

- **Sin flags**: sondea el daemon con un health check corto y lo usa si responde;
  si no, cae al modo directo sin error.
- **`--daemon`**: asume el daemon disponible y le envía la síntesis sin sondeo
  previo; un fallo se reporta como error (sin fallback silencioso).
- **`--no-daemon`**: modo directo, sin ningún sondeo.

```bash
# El daemon se usa automáticamente si está disponible
ai-voice-interconnector speech say --text "Hola" --voice mi_voz

# Forzar modo daemon (falla si el daemon no responde)
ai-voice-interconnector speech say --text "Hola" --voice mi_voz --daemon

# Forzar modo directo (sin daemon)
ai-voice-interconnector speech say --text "Hola" --voice mi_voz --no-daemon
```

**Qué esperar** con el daemon activo: `speech say` omite la etapa de carga del modelo
y la síntesis empieza de inmediato. Aunque la síntesis ocurre en el proceso del
daemon, su **progreso real** viaja al cliente por el stream de `/synthesize`
(etapa actual y conteo de tokens del T3 en vivo):

```
Iniciando speech say...
[10:05:01] [Servidor] Enviando solicitud de síntesis...
[10:05:19]    [Etapa 2a] T3 autoregresivo: 12.0s...
[10:05:19]    [Etapa 2b] S3Gen vocoder:   6.0s...
[10:05:19] [Servidor] Síntesis completada (18.0s)...
[10:05:19] [Reproducción] Reproduciendo audio...
[10:05:22] [Reproducción] Reproducción finalizada
Finalizado en 21.3s
```

Los tiempos de `[Etapa 2a]` (generación de tokens) y `[Etapa 2b]` (vocoder) se
muestran con el **mismo formato en ambos modos** (directo y daemon), para que
puedas comparar el rendimiento.

**Progreso en vivo (solo en terminal interactiva):** en una TTY, mientras dura la
síntesis `speech say` muestra sobre **stderr** un indicador giratorio que se actualiza
con la etapa y el avance de tokens del T3 (p. ej. `Generando voz · 210 tokens`,
subiendo), tanto en modo daemon como directo. Es un indicador de etapa y avance,
**no un porcentaje** del total. Si la salida está redirigida a un archivo o pipe,
o corre en CI, el indicador se desactiva por completo y stdout queda intacto
(contrato del CLI: stdout = datos, stderr = progreso). Ver `docs/DAEMON-MODE.md`
para el detalle del protocolo NDJSON que transporta estos eventos.

### Requisitos de hardware

La síntesis corre en CPU por defecto (sin GPU). Requisitos orientativos:

- **CPU**: x86-64 (o ARM64) moderna con soporte **AVX2**. La mayoría de los
  procesadores de escritorio/portátil desde ~2015 lo tienen; en CPUs muy antiguas
  sin AVX2, PyTorch puede fallar al cargar o correr mucho más lento. *(`doctor`
  lo detecta best-effort: en Linux por `/proc/cpuinfo` y en macOS Intel por
  `sysctl`, con `[WARN]` si falta; en Windows no hay vía estándar de detección y
  el chequeo se reporta como `[SKIP]` informativo — si tu CPU es de antes de
  2015, verifícalo en las especificaciones del fabricante. En ARM64 no aplica.)*
- **RAM**: **8 GB recomendados**, **4 GB mínimo**. Con menos memoria la síntesis
  funciona pero puede paginar (ralentizarse) en textos largos. `doctor` emite un
  `[WARN]` de RAM por debajo de 8 GB (no bloquea nada).
- **Disco**: ~9 GB para los modelos descargados (Qwen3-TTS ~4,7 GB + Marian
  es↔en ~3 GB + Parakeet TDT v3 ~0,6 GB). El binario instalado ocupa ~40 MB.
- **GPU (opcional)**: el motor usa CPU por defecto; no es necesaria para el
  funcionamiento.
- **Linux — glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+, Fedora 36+ o equivalente):
  ver la entrada correspondiente en «Solución de Problemas» más abajo.

---

## Clonación de voz: recorrido completo

De principio a fin, desde grabar tu voz hasta escucharla sintetizada:

```bash
# 1. Graba dos audios en español (WAV 16-bit, sin ruido de fondo):
#    timbre.wav  - cualquier largo, captura tu timbre
#    habla.wav   - 10+ segundos de habla limpia y continua

# 2. Clona la voz
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav
# → Voz 'mi_voz' clonada: (rutas de los dos archivos copiados)

# 3. Verifica que aparece
ai-voice-interconnector voice list
# → Voces registradas: default, mi_voz

# 4. Escúchala
ai-voice-interconnector speech say --text "Hola, esto es una prueba" --voice mi_voz
# → etapas de síntesis + reproducción por los altavoces

# 5. O genera un archivo
ai-voice-interconnector speech synthesize --text "Hola, esto es una prueba" --label prueba --voice mi_voz
# → Locución 'prueba' guardada (voz 'mi_voz').
```

La voz queda guardada de forma permanente: en futuras sesiones basta con
`--voice mi_voz`, sin volver a clonar nada.

---

## Experiencia unificada entre sistemas operativos

Todos los casos de uso de esta guía se ejecutan **con los mismos comandos, la
misma salida y los mismos códigos de retorno** en Windows, Linux y macOS, tanto
desde el binario como desde el código fuente. En concreto:

- **Sintaxis idéntica**: no hay flags ni subcomandos exclusivos de una plataforma.
- **Contrato de salida estable**: los datos van a stdout y los diagnósticos y
  errores a stderr, siempre en UTF-8. Esto hace a `ai-voice-interconnector` consumible por
  scripts de forma idéntica en los tres SO.
- **Códigos de salida (contrato público congelado)**: un orquestador distingue la
  causa del fallo sin parsear texto en español. Los valores son estables entre SO
  y versiones:

  | Código | Significado | Ejemplo |
  |--------|-------------|---------|
  | `0` | Éxito | Síntesis o comando completado |
  | `1` | Error genérico | Fallo inesperado; `doctor` con algún chequeo fallido |
  | `2` | Entrada inválida | `--text` vacío; nombre de voz ilegal; uso incorrecto (argparse) |
  | `3` | Voz o audio no encontrado | `--voice inexistente`; `voice remove` de una voz ausente |
  | `4` | Modelo no provisionado | `speech say`/`daemon start` sin ejecutar `setup` |
  | `5` | Daemon inalcanzable | `speech say --daemon` sin daemon; `daemon start/stop/restart` fallido |
  | `6` | Conflicto de estado | Colisión en `voice clone` sin `--force`; voz ocupada; puerto del daemon en uso |
  | `7` | Operación no aplicable | Voz de fábrica de solo lectura; plataforma no soportada |
  | `8` | Precondición de entorno incumplida | Credenciales, red, permisos o disco insuficientes al provisionar |
  | `9` | Fallo del pipeline de traducción | `translate` con el modelo cargado pero la inferencia falla |
  | `10` | Fallo del pipeline de transcripción | `speech transcribe`/`speech dub` con el modelo cargado pero la inferencia falla (directo o vía daemon) |
  | `130` | Interrupción del usuario | Ctrl+C (128 + SIGINT) durante cualquier comando |
- **La voz `default` y el modelo** son los mismos en todas las plataformas: el
  audio generado para un mismo texto y voz es equivalente en cualquier SO.
- **El motor de audio** es nativo por SO (`cpal`: WASAPI/CoreAudio/ALSA); no
  requiere configuración ni selección de backend por el usuario.

Las únicas diferencias son internas y no cambian la forma de usar la aplicación:

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| Reproducción de audio | cpal (WASAPI) | cpal (ALSA) | cpal (CoreAudio) |
| Enumeración de dispositivos | cpal | cpal | cpal |
| Voces de usuario (binario) | `%LOCALAPPDATA%\ai-voice-interconnector\voices` | `~/.local/share/ai-voice-interconnector/voices` | `~/Library/Application Support/ai-voice-interconnector/voices` |
| Caché del modelo | `~/.cache/huggingface/hub` | `~/.cache/huggingface/hub` | `~/.cache/huggingface/hub` |

> La caché del modelo respeta las variables de entorno `HF_HUB_CACHE` y `HF_HOME`
> si están definidas (misma resolución que usa HuggingFace Hub); la ruta de la
> tabla es el valor por defecto.

---

## Formato de Audio

- **Generación**: 24000 Hz, Mono
- **Exportación WAV**: 16-bit PCM, 24000 Hz, Mono

## Solución de Problemas

### "El modelo … no está provisionado. Ejecuta 'setup' primero." (exit 4)

`speech say`, `speech synthesize`, `daemon` y `translate` exigen los modelos
pinneados provisionados, y nunca los descargan por sí mismos. Provisiónalos con:

```bash
ai-voice-interconnector setup
```

### "GLIBC_2.35 not found" (o similar) al ejecutar el binario en Linux

El binario Linux requiere **glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+, Fedora 36+
o equivalente): se compila contra la glibc del runner de build y `crt-static` no
enlaza glibc estáticamente. En una distro más antigua (p. ej. Ubuntu 20.04,
Debian 11) el binario no arranca — `install-linux.sh` lo detecta y aborta antes.
Actualiza la distro o compila desde código fuente en tu distro actual (ver
[docs/BUILD.md](docs/BUILD.md)).

### "OneDrive user-data-dir" [WARN] en doctor (Windows)

En perfiles corporativos, `LOCALAPPDATA` (donde `ai-voice-interconnector` guarda las voces de
usuario) puede caer bajo una jerarquía de **OneDrive**. Eso expone los archivos de
voz a *file locks* y a *placeholders* «a petición» (Files On-Demand), que causan
fallos de lectura esporádicos e inatribuibles al cargar una voz.

`doctor` emite `[WARN] OneDrive user-data-dir` cuando detecta que `data_root()`
está bajo la sincronización de OneDrive (vía las variables de entorno
`OneDrive`/`OneDriveCommercial`, o por patrón de ruta). Es un aviso informativo:
no bloquea nada ni cambia dónde se guardan las voces. Para mitigarlo:

- **Excluye** la carpeta de datos de `ai-voice-interconnector` (`%LOCALAPPDATA%\ai-voice-interconnector`)
  de la sincronización de OneDrive, o
- **Deshabilita Files On-Demand** para esa carpeta, de modo que sus archivos se
  descarguen siempre y no queden como marcadores bajo demanda.

### "Voice 'x' not found"

Verifica que la voz existe:

```bash
ai-voice-interconnector voice list
```

### "La voz 'x' ya existe"

`voice clone` no sobrescribe voces por accidente. Si quieres reemplazarla:

```bash
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav --force
```

### "timbre-reference.wav/speech-reference.wav not found"

La voz no tiene los archivos necesarios. Puede que se registró con el formato
antiguo. Vuelve a clonar:

```bash
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav --force
```

### "Voz 'x' es una voz de fábrica (solo lectura)"

Las voces empaquetadas (como `default`) no pueden eliminarse con `voice remove`.
Si quieres reemplazar su sonido, clona una voz de usuario con el mismo nombre
usando `voice clone --force`: la tuya toma precedencia.

### Error al eliminar una voz: "uno de sus archivos parece estar en uso"

Otro proceso (el daemon, un reproductor de audio) tiene abierto alguno de los
archivos de la voz. Ciérralo (p. ej. `ai-voice-interconnector daemon stop`) y reintenta.

### Sin audio de salida

1. Verifica que `ai-voice-interconnector devices` detecta tu dispositivo
2. Comprueba que el volumen del sistema no está en mute
3. Verifica que el dispositivo de audio predeterminado es correcto
4. Ejecuta `ai-voice-interconnector doctor`: el chequeo "Audio library" falla si el host no
   tiene un subsistema de audio funcional (p. ej. sesiones remotas o headless)

En un host sin audio puedes seguir usando la síntesis a archivo
(`ai-voice-interconnector speech synthesize --text T --label L`); `setup` también funciona
allí (degrada el chequeo de audio a `[WARN]` y provisiona igual).

### El sistema bloquea el primer arranque (binarios sin firmar)

Al abrir el instalador por primera vez es **esperable** que el sistema lo
bloquee. No significa que el archivo contenga malware: los binarios
distribuidos no están firmados ni notarizados, y los sistemas de reputación
(SmartScreen en Windows, Gatekeeper en macOS) tratan todo ejecutable de «editor
desconocido» y sin historial de descargas como no confiable por defecto. Cada
release es un archivo nuevo, así que la advertencia reaparece con cada versión.

Cómo proceder:

- **Windows (SmartScreen)**: en el diálogo «Windows protegió tu PC», pulsa
  **Más información** → **Ejecutar de todas formas**. (Si el navegador ya
  bloqueó la descarga, consérvala desde el menú de descargas: **Conservar** →
  **Conservar de todas formas**.)

- **macOS (Gatekeeper)**: al abrir el binario por primera vez, haz clic
  derecho sobre él → **Abrir** y confirma; o quita la cuarentena desde una
  terminal:

  ```bash
  xattr -d com.apple.quarantine ai-voice-interconnector
  ```

Esto solo ocurre en el primer arranque; las ejecuciones posteriores no vuelven a
pedir confirmación. Los one-liners (`curl | sh` / `irm | iex`) descargan por CLI
y **no disparan ninguno de los dos avisos** (sin Mark-of-the-Web).

Antes de aceptar, puedes comprobar objetivamente que el archivo es el que
publicó el proyecto cotejando su SHA-256 contra el `SHA256SUMS.txt` del
Release (ver [SECURITY.md](SECURITY.md#artefactos-sin-firmar)):

```powershell
# Windows (PowerShell)
Get-FileHash .\ai-voice-interconnector-X.Y.Z-x86_64-setup.exe -Algorithm SHA256
```

```bash
# Linux / macOS
sha256sum -c SHA256SUMS.txt --ignore-missing
```

Si un antivirus de terceros pone el instalador en cuarentena, restáuralo y
añade una exclusión **solo después** de verificar el hash. El plan del
proyecto es eliminar esta fricción firmando los binarios a través de
[SignPath Foundation](https://signpath.org/) (firma de código gratuita para
proyectos open source) en una versión futura.

## Uso ético y responsable

`ai-voice-interconnector` permite clonar voces arbitrarias a partir de unos segundos de audio.
Por diseño, **el audio generado no contiene marca de agua**: el watermark de
PerthNet está desactivado en el motor (tanto en modo directo como en el daemon),
de modo que la salida no es distinguible por medios técnicos de una grabación
real. Esta capacidad exige diligencia por parte de quien la usa:

- **Consentimiento explícito**: clona únicamente voces para las que
  cuentes con el permiso de la persona titular. Clonar la voz de alguien sin su
  autorización puede ser ilegal en tu jurisdicción y es, en todo caso, una falta
  de respeto a su identidad.
- **Prohibición de suplantación**: no emplees la herramienta para hacerte pasar
  por otra persona, cometer fraude, eludir sistemas de verificación por voz,
  difamar, acosar ni generar desinformación.
- **Divulgación del contenido sintético**: cuando publiques o compartas audio
  generado, decláralo como sintético. Dado que **no lleva marca de agua**, la
  transparencia depende enteramente de ti; no existe un mecanismo automático que
  identifique la salida como generada por IA.
- **Canal de reporte**: si detectas un uso indebido de este proyecto o de
  material producido con él, abre un
  [Issue](https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/issues)
  describiendo la situación.

AI Voice InterConnector es software libre y no impone barreras técnicas al uso (serían
triviales de sortear); establece, en cambio, la diligencia debida esperada en la
comunidad de IA de código abierto. La responsabilidad del uso legítimo recae en
la persona que ejecuta la herramienta.

## Licencia

`ai-voice-interconnector` se distribuye bajo **GPL-3.0-or-later** (ver [LICENSE](LICENSE)). El
motor Qwen3-TTS se distribuye bajo MIT/Apache-2.0 y el par de traducción
`opus-mt` (Helsinki-NLP) bajo CC-BY-4.0; las dependencias empaquetadas conservan
sus propias licencias, en su mayoría permisivas (MIT/Apache-2.0/BSD/ISC),
detalladas en [THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md) (inventario de `Cargo.lock`).
