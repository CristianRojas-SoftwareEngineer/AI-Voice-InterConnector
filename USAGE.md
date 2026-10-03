# Guía de uso de AI Voice InterConnector

## Tabla de contenidos

- [Instalación](#instalación)
  - [Requisitos de hardware](#requisitos-de-hardware)
  - [Usuario del binario](#usuario-del-binario)
  - [Compilar desde el código fuente (Rust)](#compilar-desde-el-código-fuente-rust)
- [Primer uso: provisionar el/los modelo(s) (`setup`)](#primer-uso-provisionar-ellos-modelos-setup)
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
- [Solución de problemas](#solución-de-problemas)
  - ["El modelo … no está provisionado. Ejecuta 'setup' primero." (exit 4)](#el-modelo--no-está-provisionado-ejecuta-setup-primero-exit-4)
  - ["GLIBC_2.35 not found" (o similar) al ejecutar el binario en Linux](#glibc_235-not-found-o-similar-al-ejecutar-el-binario-en-linux)
  - ["La voz 'x' no existe." (exit 3)](#la-voz-x-no-existe-exit-3)
  - ["La voz 'x' ya existe"](#la-voz-x-ya-existe)
  - ["El modelo Base de clonado TTS no está provisionado"](#el-modelo-base-de-clonado-tts-no-está-provisionado)
  - ["La voz 'x' no se puede eliminar." (exit 2)](#la-voz-x-no-se-puede-eliminar-exit-2)
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
`cargo` desde el código fuente). Detalle en
[docs/DISTRIBUTION.md](docs/DISTRIBUTION.md).

### Requisitos de hardware

La síntesis corre en CPU por defecto (sin GPU). Requisitos orientativos:

- **CPU**: x86-64 o ARM64 moderna. Toda la inferencia corre en CPU, así que en
  procesadores antiguos la síntesis es más lenta.
- **RAM**: **8 GB recomendados**, **4 GB mínimo**. Con menos memoria la síntesis
  funciona pero puede paginar (ralentizarse) en textos largos. `doctor` no mide
  ni la CPU ni la RAM.
- **Disco**: 3,3 GB para los modelos descargados (Qwen3-TTS 2,5 GB + opus-mt
  es↔en 0,17 GB + Parakeet TDT v3 0,7 GB), y 5,85 GB con `--with-voice-cloning`. El disco
  ocupa lo mismo que la descarga, salvo en sistemas de archivos sin enlaces duros (FAT32,
  exFAT o algunos recursos de red), donde `hf-hub` copia cada archivo y el espacio se
  duplica. El binario instalado ocupa ~40 MB.
- **GPU (opcional)**: el motor usa CPU por defecto; no es necesaria para el
  funcionamiento.
- **Linux — glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+, Fedora 36+ o equivalente):
  ver la entrada correspondiente en «Solución de problemas» más abajo.

### Usuario del binario

Instala el ejecutable de tu plataforma desde Releases y déjalo accesible en el
PATH (en Windows el instalador lo agrega automáticamente al PATH de usuario,
HKCU). Luego invoca:

```bash
ai-voice-interconnector <comando>
```

En **Linux y macOS**, el bootstrap `install.sh` (asset versionado del release) automatiza toda la descarga/verificación/instalación
con una sola línea (detalle en [README.md](README.md#instalación-de-una-línea)):

```bash
curl -fsSL https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.sh | sh
```

En **Windows**, el bootstrap `install.ps1` hace lo análogo desde PowerShell (instalación
per-user, sin UAC; delega en `self install`, que a su vez ejecuta `setup`):

```powershell
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
```

**Desinstalación limpia**, en **un comando** en los tres SO: `ai-voice-interconnector
self uninstall` borra el estado, revierte la integración de PATH y borra el programa, en
ese orden. Usa `--yes` para omitir la confirmación, `--keep-data` para conservar modelos,
voces y habla, y `--dry-run` para ver el plan sin tocar el disco. Sin terminal, `--yes` es
**obligatorio**: sin él la operación termina con `confirmation_required` y no borra nada.
Con Homebrew Cask, la vía idiomática es `brew uninstall --cask --zap`. Ver «Ciclo de vida
de la instalación» más abajo.

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
(crate `hf-hub`, TLS rustls) a la **caché exclusiva de la aplicación**
(`~/.cache/ai-voice-interconnector/models` en Linux,
`~/Library/Caches/ai-voice-interconnector/models` en macOS,
`%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows; respeta
`HF_HUB_CACHE`/`HF_HOME` si las defines, y en ese caso esa raíz pasa a ser compartida):

| Modelo | Repo HF | Uso |
|---|---|---|
| `qwen3-tts-0.6b` | `Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice` | Síntesis TTS |
| `opus-mt-es-en` / `opus-mt-en-es` | `CristianRojaas/opus-mt-es-en-ct2-int8` / `CristianRojaas/opus-mt-en-es-ct2-int8` | Traducción es↔en (0,08 GB cada uno; derivados int8 de Helsinki-NLP/opus-mt, CC-BY-4.0, ya convertidos a CTranslate2) |
| `parakeet-tdt-v3` | `istupakov/parakeet-tdt-0.6b-v3-onnx` | STT (0,7 GB, ONNX int8) |
| `qwen3-tts-0.6b-base` | `Qwen/Qwen3-TTS-12Hz-0.6B-Base` (2,5 GB, opt-in) | Clonado de voz (Base) |

Las revisiones están pineadas por commit hash en `MODEL_REVISIONS`
(`crates/avi-shared/src/paths.rs`): mismo binario → mismos pesos. El Base es opt-in por peso (5,85 GB en total con él). Una descarga interrumpida no se reanuda: `setup` repite el archivo completo.

```bash
ai-voice-interconnector setup                        # descarga los 4 base (idempotente)
ai-voice-interconnector setup --with-voice-cloning   # incluye Base para voice clone (2,5 GB)
ai-voice-interconnector setup --force-update         # purga los snapshots pinneados + xet y re-descarga
ai-voice-interconnector setup --force-update --yes   # ídem, sin confirmación interactiva
```

**Qué esperar:** barra de progreso por bytes con ETA y resume automático si se
interrumpe. La provisión se decide solo por presencia del snapshot HF (no hay
índice `manifest.json` que consultar): si lo vuelves a ejecutar con los
snapshots presentes, termina al instante sin descargar nada. Si la provisión falla al
final de una instalación, el programa **queda instalado** y el comando sale con `11`
(`setup_failed`): es un éxito parcial y basta reintentar con `setup`. La limpieza
posterior corresponde a `cleanup` (datos) o `self uninstall` (además programa y PATH).

**Provisión por SO** (experiencia homóloga):

- **Windows**: el bootstrap `install.ps1` delega en `self install`, que registra
  el directorio en el PATH de usuario
  (HKCU) y ejecuta `setup` al terminar.
- **Linux / macOS**: el bootstrap `install.sh` (POSIX, común a ambos SO) delega
  en `self install`, que crea el symlink
  `~/.local/bin/ai-voice-interconnector` y encadena `setup` al terminar.
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
`"4"`), que identifica la forma del esquema. Es un campo aditivo: añadir claves
nuevas no lo incrementa; solo un cambio incompatible de las claves existentes lo
haría. Un consumidor puede leerlo para detectar cambios de contrato.

**Ojo: el protocolo del daemon sigue en `"3"`.** Son dos contratos
independientes —el sobre de la CLI y el IPC del daemon— y suben por separado: el
ciclo de vida cambió el sobre (retiró cuatro claves de `doctor`) y **no** tocó el
protocolo del daemon.

### Referencia de esquemas `--json`

Los payloads siguientes son **parte del contrato programático**: sus claves son
estables (los cambios solo pueden ser aditivos mientras `schema_version` sea
`"4"`). En todos los casos, stdout contiene exactamente un objeto JSON y el
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
| `version`, `target`, `channel` | string | Versión del binario, tripla del host y canal de instalación |
| `install` | objeto | `dir`, `data_dir` (raíz de datos efectiva), `receipt` (`valid`/`absent`) y `version` |
| `path` | objeto | Resolución y duplicados en el `PATH`, integración y coexistencia |
| `pending` | objeto | Restos de operaciones anteriores (transacción, aparcados, stagings, temporales) |
| `models` | objeto | `root`, `shared_root`, `provisioned`, `missing` (solo la selección guardada), `base` (`ready`/`missing_opt_in`), `size_bytes` |
| `checks` | array de objetos | `{name, ok, detail}` por chequeo |
| `failed` | array de strings | Nombres de los chequeos fallidos (vacío si todo correcto); exit 1 si no está vacío |

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
| `models_provisioned` | array de strings | Los 4 modelos base + 1 opt-in si `--with-voice-cloning` (`qwen3-tts-0.6b`, `opus-mt-*`, `parakeet-tdt-v3`, `qwen3-tts-0.6b-base`) |

**`cleanup --json` / `self uninstall --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `cleanup`: `"cleanup_complete"` / `"cancelled"` (cancelación de la confirmación, exit 0). `self uninstall`: `"uninstalled"` / `"removal_scheduled"` (Windows, borrado diferido) / `"not_installed"` (idempotencia) / `"cancelled"` |
| `reason` | string \| null | `null` en el desenlace normal; con `--json`, un fallo emite el objeto `error` + `reason` de §10 del contrato |
| `removed` | array de strings | Rutas efectivamente eliminadas (o las del plan con `--dry-run`); vacío si no había nada |
| `dry_run` | boolean | `cleanup`: `true` con `--dry-run`, `false` en borrado real |
| `path_reverted` | boolean | Solo `self uninstall`: `true` si se revirtió la integración de `PATH` registrada en el recibo |

**`self install --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `status` | string | `"installed"` o `"repaired"` (reparación: el ejecutable se invoca desde el propio directorio de programa) |
| `reason` | string \| null | `null` en éxito; `"setup_failed"` si el programa quedó instalado pero la provisión no se completó (**exit 11**, éxito parcial) |
| `install_dir` / `version` / `channel` | string | Lo que queda en el recibo de la instalación |
| `path_integrated` | boolean | Si la integración de `PATH` está en pie (estado, no diff de esta pasada) |
| `models` | string | `"skipped"` (`--no-setup`), `"already_provisioned"`, `"provisioned"` o `"failed"` |
| `models_cause` | objeto | **Solo si `models` es `"failed"`**: `{reason, message}` con `network_error` |

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
| `timbre` | string \| null | Solo ruta daemon: siempre `null` (el timbre queda fundido en `reference.qvoice`; no se persiste WAV de referencia separado). Ausente en ruta local |
| `speech` | string | Ruta absoluta del `.qvoice` generado (`reference.qvoice`) |
| `precomputed` | boolean | Ruta local: siempre `false` (motor efímero, sin residente que calentar). Ruta daemon: `true` (warm-on-clone iniciado; completitud en `GET /health`) |

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

Verifica el entorno local sin tocar el daemon ni el audio: el directorio de
datos, los modelos provisionados y el almacén de voces.

```bash
ai-voice-interconnector doctor
ai-voice-interconnector doctor --json
```

**Qué esperar** (entorno sano):

```
Diagnóstico: todo correcto.
Cache HF: C:\Users\<tu-usuario>\.cache\huggingface\hub
```

`doctor` ejecuta cinco chequeos: recibo de instalación, resolución y duplicados en
el `PATH`, artefactos pendientes y `models_provisioned` (los modelos de la
selección guardada: TTS Qwen3-TTS, traducción opus-mt es→en y en→es y STT
Parakeet TDT v3, más el Base si activaste el clonado). Si alguno falla, lo lista y remite a
`ai-voice-interconnector setup`.

El modelo Base de clonado es opcional: si no lo pediste, su ausencia no es un
fallo y `--json` la informa en `models.base` (`ready` o `missing_opt_in`).

El veredicto es el código de salida: 0 si todos los chequeos pasan (tras una
instalación correcta sin clonado, `doctor` sale con 0) y 1 si alguno falla.
Referencia canónica: `docs/CLI/commands/DOCTOR.md`.

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

Siete sub-acciones sobre el habla: dos que sintetizan (`synthesize`, `say`), tres que gestionan el almacén de locuciones guardadas (`play`, `list`, `remove`), una que transcribe audio a texto (`transcribe`) y una que compone el bucle voz→voz (`dub`). Cada una tiene una sola responsabilidad, y el nombre declara su costo: sintetizar paga una inferencia pesada (CPU + RAM) y puede exigir el modelo provisionado; gestionar el almacén no.

| Sub-acción | Qué hace | Persiste | Necesita el modelo |
|---|---|---|---|
| `speech synthesize` | Sintetiza y guarda una locución | sí | sí |
| `speech say` | Sintetiza y reproduce, no guarda | no | sí |
| `speech transcribe` | Transcribe audio a texto | no | sí |
| `speech dub` | Composición voz→voz: transcribe, traduce si procede, sintetiza y reproduce | no | sí |
| `speech play` | Reproduce una locución guardada | no | no |
| `speech list` | Lista las locuciones guardadas | no | no |
| `speech remove` | Borra una locución guardada | no | no |

**En las que aceptan voz, `--voice/-v` es opcional**; si se omite, usa la voz de fábrica
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

En las operaciones pesadas (`synthesize`, `say`, `voice clone` y `dub`), la comunicación con el daemon opera mediante streaming NDJSON con latidos periódicos cada 500 ms y timeout de inactividad de 1500 ms (`STREAM_INACTIVITY_TIMEOUT`), evitando cuelgues o timeouts prematuros en tareas de larga duración.

`--daemon` y `--no-daemon` son mutuamente excluyentes: combinarlos sale con
exit **2** antes de cualquier trabajo. `speech play`, `speech list` y `speech
remove` no tocan el modelo ni el daemon: no declaran estos flags.

#### `speech synthesize`

Sintetiza texto y lo guarda en el almacén de habla sintética
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
Locución 'saludo' guardada (voz 'default').
```

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--label, -l` (requerido): Etiqueta de la locución en el almacén (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz a usar (default: `default`)
- `--output, -o`: Copia adicional del WAV a la ruta indicada
- `--play`: Reproduce el WAV tras guardar
- `--force, -f`: Sobrescribe la locución si la etiqueta ya existe para la voz
- `--source-language`: Idioma del texto de entrada (`es-latam` o `en`; por defecto igual a `--target-language`, sin traducir)
- `--target-language`: Idioma/modelo de síntesis (`es-latam` o `en`; default `es-latam`; si difiere del origen, el texto se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
- `--json`: Emite `{status, audio_path, voice}`

**Colisión de etiqueta:** sin `--force`, guardar sobre una etiqueta que ya
existe para la voz sale con exit **6**, sin pagar la síntesis (la comprobación es
previa). Con `--play`, la etiqueta se revalida también al
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
- **Rechazar y regenerar**: sintetiza otra toma y vuelve a preguntar.
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
- `--source-language`: Idioma del texto de entrada (`es-latam` o `en`; por defecto igual a `--target-language`, sin traducir)
- `--target-language`: Idioma/modelo de síntesis (`es-latam` o `en`; default `es-latam`; si difiere del origen, el texto se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
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
- `--json`: Emite `{"status", "voice", "label"}`

Una etiqueta inexistente para la voz sale con exit **3**.

#### `speech list`

Lista las locuciones guardadas. `--voice/-v` es opcional, sin default:
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
[default] saludo: Bienvenido
[mi_voz] despedida: Hasta luego, gracias por tu visita a nuestra tien...
```

El texto se muestra truncado a 60 caracteres en la salida humana; el payload
`--json` (`{"speech": [...]}`) lleva el texto completo. Una locución
sin sidecar de metadatos se muestra como `(sin metadatos)`.

Con `--voice`, el identificador se valida antes de leer: una voz con
caracteres ilegales sale con exit **2** (`invalid_identifier`) y una voz
inexistente con exit **3** (`voice_not_found`).

**Opciones:**
- `--voice, -v`: Filtra por voz (default: todas las voces)
- `--json`: Emite `{"speech": [{"voice", "label", "text", "created_at"}]}`

#### `speech remove`

Borra una locución guardada (el WAV y su sidecar de metadatos, si existen).

```bash
ai-voice-interconnector speech remove --label saludo
ai-voice-interconnector speech remove --label saludo --voice mi_voz
```

**Opciones:**
- `--label, -l` (requerido): Etiqueta de la locución (normalizada a minúsculas)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--json`: Emite `{"status", "voice", "label"}`

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
fija en segundos y solo es válido junto a `--mic`. El push-to-talk tiene un
techo de seguridad configurable con la variable de entorno
`AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s): al alcanzarlo, la grabación se
detiene, se avisa por stderr y se devuelve lo grabado hasta ese punto con
exit **0** (no es un error).

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

Con `--mic` y sin `--duration`, el comando avisa por stderr al iniciar la
grabación y captura en modo push-to-talk hasta que el usuario presiona Enter
(o hasta el techo `AVI_PUSH_TO_TALK_MAX_SECS`) antes de transcribir.

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
transcripción no está provisionado, falla remitiendo a
`ai-voice-interconnector setup` con exit **4**; si la transcripción falla con
el modelo ya cargado, sale con exit **10**. En la ruta daemon, un fallo de
comunicación (daemon inactivo o de versión antigua sin `/transcribe`) sale con
exit **5**.

---

#### `speech dub`

Composición voz→voz: transcribe la entrada hablada (archivo o micrófono),
traduce si `--source-language` difiere de `--target-language`, sintetiza con
la voz elegida y reproduce el resultado. Reutiliza las etapas de
`speech transcribe`, la traducción de `speech say`/`synthesize` y el despacho
de síntesis; no guarda nada en el almacén (sin `--label` ni `--json`).

`--audio` y `--mic` son **mutuamente excluyentes y exactamente una de las dos
es requerida**. Con `--mic`, la grabación es **push-to-talk** por defecto
(termina al presionar Enter); `--duration N` fuerza una grabación de duración
fija en segundos y solo es válido junto a `--mic`. El mismo techo
`AVI_PUSH_TO_TALK_MAX_SECS` (default 300 s) aplica aquí: al vencer, detiene la
grabación, avisa por stderr y continúa con lo grabado (exit **0**).

```bash
ai-voice-interconnector speech dub --mic --source-language es-latam --target-language en -v mi_voz
ai-voice-interconnector speech dub --audio grabacion.wav --source-language en --target-language es-latam
```

**Qué esperar:** transcribe tu habla al texto, lo traduce si procede y
reproduce la síntesis con la voz (`default` si no pasas `-v`). Con `--mic` y
sin `--duration`, el comando avisa por stderr al iniciar la grabación y
captura en modo push-to-talk hasta que presiones Enter (o hasta el techo
`AVI_PUSH_TO_TALK_MAX_SECS`) antes de transcribir.

**Opciones:**
- `--audio, -a`: Ruta del archivo WAV hablado (mutuamente excluyente con `--mic`; exactamente una de las dos es requerida; alias: `--file`)
- `--mic`: Graba desde el micrófono (mutuamente excluyente con `--audio`; exactamente una de las dos es requerida)
- `--duration N`: Duración fija de grabación en segundos; solo válido con `--mic`
- `--source-language` (requerido): Idioma hablado en el audio (`es-latam` o `en`)
- `--target-language`: Idioma/modelo de síntesis (`es-latam` o `en`; default `es-latam`; si difiere del origen, se traduce antes de sintetizar)
- `--temperature`: Override del muestreo (`0 < t <= 2.0`; sin el flag se usa la temperatura de producción)
- `--voice, -v`: Nombre de la voz (default: `default`)
- `--daemon` / `--no-daemon`: aplican a la transcripción y a la síntesis

`--duration` sin `--mic` sale con exit **2**, y `--mic` sin `--duration` en
una terminal no interactiva (no TTY) también sale con exit **2**. Un `--audio`
inexistente sale con exit **3**. Códigos de fallo de la cadena: exit **4**
(modelo de transcripción no provisionado, remite a
`ai-voice-interconnector setup`), **5** (daemon exigido pero inactivo o de
versión antigua sin `/transcribe`), **9** (fallo de traducción con el modelo
cargado) y **10** (fallo de transcripción con el modelo cargado).

---

### El grupo `voice`

Tres sub-acciones sobre el registro de voces: `clone` registra una voz a partir
de audio de referencia (requiere el modelo Base), `list` muestra las voces de
fábrica y de usuario, y `remove` elimina una voz de usuario. Las voces de fábrica
(`default`, `ryan`, `vivian`) no se pueden eliminar.

#### `voice clone`

Clona una voz a partir de un audio de referencia (requiere modelo Base).

```bash
ai-voice-interconnector voice clone --name mi_voz --speech-reference condicion.wav
# Si falta Base: ai-voice-interconnector setup --with-voice-cloning
```

**Qué esperar:** el comando valida que el audio sea cargable, genera `reference.qvoice` vía
`avi_tts::clone_voice` con el modelo Base, y confirma (error `model_missing` → `setup --with-voice-cloning`):

```
Voz 'mi_voz' clonada.
```

Internamente el timbre y el habla quedan fundidos en un único
`reference.qvoice` (bajo `data_dir()/voices/mi_voz/`); no se persisten WAV de
referencia separados.

A partir de ese momento la voz aparece en `voice list` y puede usarse con
`speech say --voice mi_voz`.

El clonado **extrae la representación de la voz** con el modelo Base y la guarda
como `reference.qvoice`; toda síntesis posterior con `--voice mi_voz` la carga
desde disco, sin volver a procesar los audios de referencia. Por eso `voice clone`
requiere el modelo Base provisionado (`ai-voice-interconnector setup --with-voice-cloning`).
Si hay un [daemon](#modo-daemon) activo, el clonado corre en él y además precalienta
la voz nueva en segundo plano (`"precomputed": true` con `--json`); en modo directo
`precomputed` es siempre `false`.

Si la referencia no es un WAV válido o está truncada, el comando termina con el
código `invalid_audio` (exit 2) y no registra la voz; si falla la lectura del
archivo, con `io_error` (exit 1); si falla el propio clonado, con `voice_clone_failed`.

**Opciones:**
- `--name, -n` (requerido): Nombre para la voz
- `--timbre-reference, -t` (opcional): Audio para timbre (cualquier largo — el audio completo se usa para el embedding)
- `--speech-reference, -s` (requerido): Audio de habla (10+ segundos de habla limpia)
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

#### `voice list`

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

#### `voice remove`

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
say`/`speech synthesize` (taxonomía `es-latam`/`en`), aquí `--from` y `--to`
son **opcionales con defaults** (`es` y `en`) y **estrictos**: solo aceptan
`es` o `en` — no `es-latam` (el parser rechaza cualquier otro valor con
exit 2 antes del handler; `es-latam` solo sigue vivo en la vía IPC del
daemon).

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
- `--from` (opcional, default `es`): Idioma de origen del texto (`es` o `en`; `es-latam` lo rechaza el parser)
- `--to` (opcional, default `en`): Idioma destino de la traducción (`es` o `en`)
- `--json`: Emite `{"translated", "source", "target"}`

**Passthrough:** si `--from` y `--to` coinciden, devuelve el texto intacto sin
cargar ningún modelo. El modelo de traducción exigido es `opus-mt-<par>`, con sus
cinco ficheros presentes y de más de 0 bytes. Si no está provisionado, falla
remitiendo a `ai-voice-interconnector setup` (exit **4**); si la traducción falla con el
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
ai-voice-interconnector cleanup --all --dry-run       # lista sin borrar (exit 0, --json con removed/dry_run)
ai-voice-interconnector cleanup --voices --yes        # omite confirmación ( -y alias)
```

**Qué esperar:** según el flag, borra selectivamente `data_dir()/voices` (preservando `FACTORY_VOICES`), `data_dir()/speech`, o la raíz de modelos. En la raíz de modelos **exclusiva** el borrado es de directorio entero (snapshots, locks y `xet` cuelgan de ella); si el usuario eligió una caché HF compartida con `HF_HUB_CACHE`/`HF_HOME`, solo se borran los repos de `MODEL_REVISIONS` (`Qwen/Qwen3-TTS…`, `CristianRojaas/opus-mt-*-ct2-int8`, `istupakov/parakeet-tdt-0.6b-v3-onnx`) y sus locks, y **`xet` y el `.locks` completo se conservan y se anuncian** (`doctor` dice si la raíz es compartida con `models.shared_root`). `--all` es la unión de las tres categorías **sin programa ni PATH** — solo `self uninstall` borra el programa y el `PATH`. El borrado es quirúrgico: nunca toca modelos de otros proyectos en una caché compartida. `--dry-run` lista el plan sin borrar **y sin tomar el bloqueo**; `--yes/-y` omite la confirmación interactiva, y **sin terminal es obligatorio**: sin él sale con `confirmation_required` (2) y no borra nada. Con `--json` emite `{"schema_version":"4","status":"cleanup_complete","reason":null,"removed":[...],"dry_run":bool}`. Todo es recuperable: `setup` re-descarga los modelos y
`voice clone` vuelve a clonar voces.

---

## Ciclo de vida de la instalación (`self`)

**Canal nativo (los tres SO), en un comando**: `ai-voice-interconnector self uninstall`
borra el estado, revierte la integración de `PATH` y borra el programa, **en ese orden**.

```bash
ai-voice-interconnector self uninstall --dry-run      # imprime el plan con tamaños; no borra nada
ai-voice-interconnector self uninstall               # pide confirmación [s/N]
ai-voice-interconnector self uninstall --yes         # no interactivo (obligatorio sin terminal)
ai-voice-interconnector self uninstall --keep-data   # conserva modelos, voces y habla sintetizada
```

**Qué esperar:** el plan lista, con tamaños, lo que se va a borrar y lo que **no** se tocará
—la integración de `PATH` se *retira*, no se borra el archivo, y una caché HF compartida se
conserva—. Sin terminal, `--yes` es obligatorio: sin él sale con `confirmation_required`
(exit 2) y no borra nada. Cancelar la confirmación no es un error: `status` `cancelled` y
exit 0. Con `--json` emite `{"schema_version":"4","status":…,"reason":null,"removed":[…],"path_reverted":bool,"dry_run":bool}`.

**Sin `--keep-data` se borra la raíz de datos entera**, no el plan de `cleanup --all`, y la
diferencia es deliberada: `cleanup --all` protege las voces de fábrica porque van embebidas
en el binario y el programa sigue instalado, pero **al desinstalar desaparece el programa**, y
dejarlas sería residuo dentro de una raíz de propiedad exclusiva. Con `--keep-data` sí se
aplica el plan de `cleanup --all` filtrado (modelos, voces y habla fuera; dentro
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
recibo sin copiar archivos. Un ejecutable **sin bundle alrededor** (por ejemplo
`target\debug`) responde `bundle_invalid` (15) indicando `cargo xtask install`: no es una
instalación rota, es que no hay nada alrededor que instalar.

Si la provisión de modelos falla al final, el programa **queda instalado**, el `status` es
`installed`, el `reason` es `setup_failed`, el código de salida es **11** y el motivo del
fallo viaja anidado en `models_cause` (`network_error`). Es un
**éxito parcial**: basta reintentar con `setup`.

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

**Qué esperar:** resolución de la versión objetivo (sin API, con respaldo), comparación semántica (iguales → `already_up_to_date` sin descargar, éxito con 0), resumen y confirmación, descarga con SHA-256 y arranque verificado en staging, parada del daemon con el binario actual, traspaso al binario nuevo con las preferencias del recibo y resultado `anterior → nueva`. Sin reinicio automático del daemon: si estaba activo, el resumen indica cómo relanzarlo. Sin instalación → `not_installed` (3) con el one-liner; en canal `homebrew` o `dev` → `externally_managed` (12) con el comando correcto. Si la provisión del binario nuevo falla, el programa **queda actualizado** con `reason` `setup_failed` y salida **11**: es un éxito parcial y basta reintentar con `setup`.

- **macOS (Homebrew)**: `brew upgrade --cask ai-voice-interconnector`.

Los modelos descargados (en la caché de la aplicación, `~/.cache/ai-voice-interconnector/models` en Linux) se reutilizan tal cual.
Cada versión del binario fija las revisiones exactas de los modelos que usa
(`MODEL_REVISIONS`): si tu caché contiene otra revisión, `setup` la detecta como
no provisionada y descarga la requerida (los archivos que no cambian entre revisiones no se duplican: `snapshots/` los enlaza al mismo blob, con symlinks en Unix y enlaces duros en Windows, salvo en FAT32/exFAT, donde se copian). Tras el reemplazo, el `setup` de la versión nueva lee la selección guardada (`setup-selection.json`) y poda las revisiones propias obsoletas.

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
`Daemon iniciado correctamente (pid ...)`. Si falla, sale con un código que nombra la causa y no deja proceso ni PID file. Luego `daemon status` muestra estado `running`/`stopped` + `warm` (`warming`/`warm`/`warm_failed`).

Supervisor: con `--auto-restart`, el daemon reintenta hasta `max_retries` (default `3`) tras un crash con backoff `500ms*2^retries` capado a `4s` y protección por watchdog de supervisión contra bucles de reinicio rápidos; un apagado graceful vía `daemon stop` (`POST /shutdown` + `shutdown_notify`) no reintenta. Tampoco se reintentan los fallos previos a que el daemon esté listo (puerto ocupado, voz inexistente): son de configuración y salen al instante con su código. Sin `--auto-restart`, el daemon es `fail-stop`.

Puerto ocupado: si otro proceso usa o reserva el puerto del daemon, `daemon start`, `daemon restart` y `daemon serve` salen al instante con exit 6 (`port_in_use`) y un mensaje que nombra el puerto. Libera el puerto o arranca el daemon en otro con `AVI_DAEMON_PORT=<puerto>` (`0` = puerto efímero).

Trazas y logs: `RUST_LOG` (sintaxis de `tracing`) fija el nivel de traza de la CLI y de `daemon serve` y manda sobre los valores por defecto; sin ella, la CLI muestra `warn` y superiores (trazas `info`, como la descarga de `setup`, se piden con `RUST_LOG=info`) y el daemon registra `info` de los crates propios. Cada `daemon start`/`restart` escribe `data/logs/daemon_<pid>_<ms>.log` y, si el arranque falla, el error termina con `Log del daemon: <ruta>`. El motor de síntesis escribe en `data/logs/qwen3-tts_<pid>_<ms>.log`, y la salida de `voice clone` ya no se vuelca en la terminal. Se conservan los 10 logs más recientes de cada familia.

Warmup: tras enlazar la dirección resuelta (default `127.0.0.1:8765`), el daemon precalienta la voz elegida por `--warm-voice` (default `default`) vía `spawn_blocking(warm_voice_engine)` — best-effort, no aborta el arranque si falla (degrada a `warm_failed` pero sigue sirviendo; la primera petición paga el cold-start). Una `--warm-voice` inexistente sí aborta el arranque con exit 3 (`voice_not_found`), antes de enlazar el puerto y de cargar los modelos. El residente TTS es de una sola voz: clonar por daemon recalienta la voz nueva (warm-on-clone), evicciónando la anterior.

`daemon stop` responde `Señal de apagado enviada al daemon en <addr>.` (parada unificada de daemon y residente con verificación y borrado de `daemon.pid` y `daemon.ready`; si el residente registrado sigue vivo, la parada no se da por completa y sale con exit 5) y `daemon restart` hace la misma parada unificada seguida del mismo lanzamiento que `daemon start`, con los mismos códigos de fallo.

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

**Qué esperar** con el daemon activo: `speech say` omite la carga del modelo y
la síntesis empieza de inmediato. Mientras sintetiza, el daemon mantiene viva la
conexión con latidos NDJSON (ver [docs/DAEMON-MODE.md](docs/DAEMON-MODE.md#streaming-ndjson));
el cliente no muestra progreso intermedio. Al terminar la reproducción imprime
la ruta del WAV temporal reproducido, igual que en modo directo:

```
Reproduciendo: <ruta del WAV temporal>
```

Con `--json`, en lugar de esa línea emite el payload
`{"status":"reproduced","audio_path":…,"voice":…}` en stdout.

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
  | `2` | Entrada inválida | `--text` vacío; nombre de voz ilegal; uso incorrecto (clap) |
  | `3` | Voz o audio no encontrado | `--voice inexistente`; `voice remove` de una voz ausente |
  | `4` | Modelo no provisionado | `speech say`/`daemon start` sin ejecutar `setup` |
  | `5` | Daemon inalcanzable | `speech say --daemon` sin daemon; `daemon start/stop/restart` fallido |
  | `6` | Conflicto de estado | Colisión en `voice clone` sin `--force`; voz ocupada; puerto del daemon en uso |
  | `7` | Operación no aplicable | Voz de fábrica de solo lectura; plataforma no soportada |
  | `8` | Precondición de entorno incumplida | Credenciales, red, permisos o disco insuficientes al provisionar |
  | `9` | Fallo del pipeline de traducción | `translate` con el modelo cargado pero la inferencia falla |
  | `10` | Fallo del pipeline de transcripción | `speech transcribe`/`speech dub` con el modelo cargado pero la inferencia falla (directo o vía daemon) |
  | `11` | Provisión de modelos fallida, **programa instalado** | `self install` o `self update` terminan con la provisión sin completar: éxito parcial, reintentable con `setup` |
  | `12` | La copia la gestiona otra herramienta | `self uninstall` sobre una instalación de Homebrew (o `self update` en canal `dev`) |
  | `13` | Reemplazo revertido | Fallo al reemplazar la versión anterior; la anterior queda restaurada |
  | `14` | Conflicto en la ruta del enlace del `PATH` | Hay un archivo ajeno donde va el enlace (salvo `--force`) |
  | `15` | Bundle incompleto | `self install` desde un ejecutable sin bundle alrededor, p. ej. `target\debug` |
  | `16` | No se pudo detener el daemon | Nada del plan de `cleanup`/`self uninstall` se aplicó (`self update` tampoco toca nada) |
  | `17` | Otra operación de ciclo de vida en curso | El bloqueo de §6 del ciclo de vida ya está tomado |
  | `18` | Plataforma no soportada | Target no soportado; compilar desde el código fuente |
  | `19` | Binario descargado incompatible | `self update` verifica el arranque y la versión antes del traspaso, con diagnóstico |
  | `20` | Fallo de red | Descarga tras reintentos acotados (`self update`, `setup`) |
  | `21` | Hash no coincidente | `self update` con `SHA256SUMS.txt`; nada modificado |
  | `22` | Directorio de programa no borrado | `self uninstall` completó el resto; bórralo a mano (`program_dir_kept`) |
  | `130` | Interrupción del usuario | Ctrl+C (128 + SIGINT) durante cualquier comando |

  Los códigos 0–10 y el 130 son los del contrato de la CLI y no cambian. Los del 11 al 22 son
  la tabla cerrada del ciclo de vida, **uno por `reason`**, y hay que leer el 11 con cuidado:
  **no es un error**, es un éxito parcial con el programa ya instalado (o actualizado).
- **La voz `default` y el modelo** son los mismos en todas las plataformas: el
  audio generado para un mismo texto y voz es equivalente en cualquier SO.
- **El motor de audio** es nativo por SO (`cpal`: WASAPI/CoreAudio/ALSA); no
  requiere configuración ni selección de backend por el usuario.

Las únicas diferencias son internas y no cambian la forma de usar la aplicación:

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| Reproducción de audio | cpal (WASAPI) | cpal (ALSA) | cpal (CoreAudio) |
| Enumeración de dispositivos | cpal | cpal | cpal |
| Voces de usuario (binario) | `%LOCALAPPDATA%\ai-voice-interconnector\data\voices` | `~/.local/share/ai-voice-interconnector/voices` | `~/Library/Application Support/ai-voice-interconnector/voices` |
| Caché del modelo | `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` | `~/.cache/ai-voice-interconnector/models` | `~/Library/Caches/ai-voice-interconnector/models` |
| Directorio del programa | `%LOCALAPPDATA%\Programs\ai-voice-interconnector` | `~/.local/opt/ai-voice-interconnector` | `~/.local/opt/ai-voice-interconnector` |

> La caché del modelo respeta las variables de entorno `HF_HUB_CACHE` y `HF_HOME`
> si están definidas (misma resolución que usa HuggingFace Hub); la ruta de la
> tabla es el valor por defecto.

---

## Formato de audio

- **Generación**: 24000 Hz, Mono
- **Exportación WAV**: 16-bit PCM, 24000 Hz, Mono

## Solución de problemas

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
Debian 11) el binario no arranca — el bootstrap comprueba que el binario arranca
antes de delegar y aborta con `binary_incompatible` (lo instalado queda intacto).
Actualiza la distro o compila desde código fuente en tu distro actual (ver
[docs/BUILD.md](docs/BUILD.md)).

### "La voz 'x' no existe." (exit 3)

Verifica que la voz existe:

```bash
ai-voice-interconnector voice list
```

### "La voz 'x' ya existe"

`voice clone` no sobrescribe voces por accidente. Si quieres reemplazarla:

```bash
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav --force
```

### "El modelo Base de clonado TTS no está provisionado"

`voice clone` requiere el modelo Base (`--with-voice-cloning` en `setup`). Sin
él, el comando falla con `model_missing` antes de tocar el store:

```bash
ai-voice-interconnector setup --with-voice-cloning
ai-voice-interconnector voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav --force
```

### "La voz 'x' no se puede eliminar." (exit 2)

Las voces de fábrica (`default`, `ryan`, `vivian`) no pueden eliminarse con
`voice remove`: el comando sale con exit 2 y el código `cannot_remove_default`.
Si quieres reemplazar su sonido, clona una voz de usuario con el mismo nombre
usando `voice clone --force`: la tuya toma precedencia.

### Sin audio de salida

1. Verifica que `ai-voice-interconnector devices` detecta tu dispositivo
2. Comprueba que el volumen del sistema no está en mute
3. Verifica que el dispositivo de audio predeterminado es correcto
4. Si la reproducción falla, la CLI termina con el código `playback_failed` y
   el mensaje del sistema de audio indica la causa (p. ej. sesiones remotas o
   headless sin dispositivo de salida)

En un host sin audio puedes seguir usando la síntesis a archivo
(`ai-voice-interconnector speech synthesize --text T --label L`); ni `setup` ni
`doctor` dependen del audio.

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
Get-FileHash .\ai-voice-interconnector-X.Y.Z-x86_64-windows.zip -Algorithm SHA256
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
