# Guía de Uso de TTS Sidecar

## Tabla de contenidos

- [Instalación](#instalación)
  - [Usuario del binario](#usuario-del-binario)
  - [Usuario de PyPI (`uv tool install` / `pipx`)](#usuario-de-pypi-uv-tool-install-pipx)
  - [Desarrollador (desde el código fuente)](#desarrollador-desde-el-código-fuente)
- [Primer uso: provisionar el modelo (`setup`)](#primer-uso-provisionar-el-modelo-setup)
- [Comandos](#comandos)
  - [Referencia de esquemas `--json`](#referencia-de-esquemas-json)
  - [`version`](#version)
  - [`doctor`](#doctor)
  - [`devices`](#devices)
  - [`speech say`](#speech-say)
  - [`voice clone`](#voice-clone)
  - [`voice list`](#voice-list)
  - [`voice remove`](#voice-remove)
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
  - ["el modelo 'es-mx-latam' no está descargado"](#el-modelo-es-mx-latam-no-está-descargado)
  - ["GLIBC_2.35 not found" (o similar) al ejecutar el AppImage en Linux](#glibc_235-not-found-o-similar-al-ejecutar-el-appimage-en-linux)
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

TTS Sidecar es un sintetizador de voz (TTS) 100 % local con clonación de voz en
español latinoamericano. Esta guía recorre cada caso de uso desde la perspectiva
del usuario: qué comando ejecutar, qué ocurre y qué salida esperar.

Todos los comandos funcionan **de forma idéntica en Windows, Linux y macOS**: la
misma sintaxis, la misma salida y los mismos códigos de retorno. Las diferencias
internas por plataforma (backend de reproducción, ubicación de datos) se detallan
en [Experiencia unificada entre sistemas operativos](#experiencia-unificada-entre-sistemas-operativos).

## Instalación

Hay tres flujos según la audiencia: el del **usuario del binario** (instala el
ejecutable distribuido por SO), el del **usuario de PyPI** (Python 3.13+ ya
instalado) y el del **desarrollador** (ejecuta desde el código fuente con
dependencias Python). Matriz de trade-offs completa entre los dos primeros en
[docs/DISTRIBUTION.md](docs/DISTRIBUTION.md).

### Usuario del binario

Instala el ejecutable de tu plataforma desde Releases y déjalo accesible en el
PATH (en Windows el instalador lo agrega automáticamente al PATH de usuario,
HKCU). Luego invoca:

```bash
tts-sidecar <comando>
```

En **Linux**, `install-linux.sh` automatiza toda la descarga/verificación/instalación
con una sola línea (detalle en [README.md](README.md#instalación-de-una-línea)):

```bash
curl -fsSL https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/main/install-linux.sh | sh
```

En **Windows**, `install-windows.ps1` hace lo análogo desde PowerShell (instalación
per-user, sin UAC; termina ejecutando `tts-sidecar setup`):

```powershell
irm https://raw.githubusercontent.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/main/install-windows.ps1 | iex
```

**Desinstalación limpia**, en **un comando** en los tres SO: `tts-sidecar setup
--uninstall` encadena `tts-sidecar cleanup --all` (caché del modelo + datos de
usuario), revierte la integración de PATH y borra el binario, en ese orden.
Añade `--yes` para omitir la confirmación del cleanup. En Windows el binario y el
PATH los borra el desinstalador de Inno; con Homebrew Cask el comando remite a
`brew uninstall --cask --zap`. Ver «Desinstalación completa» más abajo.

### Usuario de PyPI (`uv tool install` / `pipx`)

```bash
uv tool install tts-sidecar
# o: pipx install tts-sidecar

tts-sidecar setup
tts-sidecar <comando>
```

Linux requiere la librería del sistema `libportaudio2` para reproducir audio
(`sudo apt install libportaudio2` / `sudo dnf install portaudio`); no es
necesaria para `speak --output` a archivo. Este canal no dispara
SmartScreen/Gatekeeper (ver más abajo).

### Desarrollador (desde el código fuente)

```bash
# 1. Instalar dependencias (runtime + herramientas de build)
pip install -r requirements.txt

# 2. Ejecutar desde el código fuente
python bin/tts-sidecar <comando>
```

A partir de aquí, todos los ejemplos usan `tts-sidecar <comando>`; si trabajas
desde el código fuente, sustituye por `python bin/tts-sidecar <comando>`. El
comportamiento es el mismo.

## Primer uso: provisionar el modelo (`setup`)

El modelo de voz `es-mx-latam` (~4 GB) no viene incluido: se
descarga una sola vez a la caché de HuggingFace (`~/.cache/huggingface/hub` por
defecto; si defines `HF_HOME` o `HF_HUB_CACHE`, se respeta esa ubicación)
mediante el comando `setup`.

```bash
tts-sidecar setup
```

**Qué esperar:** `setup` integra primero el comando en el PATH (solo en Linux,
ejecutado desde el AppImage; en Windows y macOS ese paso lo cubren el instalador
y el script del `.dmg`), después corre los chequeos de entorno (los mismos que
`doctor`) y por último descarga el modelo solo si falta. Un fallo del chequeo de
audio **no detiene la provisión**: `setup` lo degrada a `[WARN]` y continúa,
porque la síntesis a archivo (`speak --output`) funciona sin subsistema de
sonido (p. ej. en hosts headless o sesiones SSH); `doctor`, en cambio, lo sigue
reportando como `[FAIL]` con salida 1, como señal diagnóstica. En la primera
ejecución verás algo como:

```
=== TTS Sidecar Setup ===

[PASS] Chatterbox TTS: 0.3.x
[PASS] Audio library: ... — N dispositivo(s)

Descargando el modelo es-mx-latam...
(Puede tardar varios minutos en la primera ejecución)

[PASS] ¡Modelo descargado correctamente!
  Ubicación: ~/.cache/huggingface/hub
```

Si lo vuelves a ejecutar con el modelo ya descargado, termina de inmediato sin
descargar nada (es idempotente):

```
[PASS] El modelo 'es-mx-latam' ya está en caché en: ~/.cache/huggingface/hub
Provisión completa. No hay nada que descargar.
```

**Forzar una re-descarga limpia (`--force-update`):** si necesitas re-descargar
el modelo (p. ej. una caché corrupta o para actualizarlo a la revisión fijada por
una versión nueva), `setup --force-update` elimina primero el modelo en caché y
luego lo vuelve a descargar, informando el espacio liberado:

```bash
tts-sidecar setup --force-update
# [force-update] Eliminando el modelo en caché para re-descargarlo...
# [force-update] Espacio liberado total: ... MB
# → a continuación, la descarga normal de setup
```

Es un modo mutuamente excluyente con `--remove-path` y `--uninstall`. Sin el flag,
`setup` es idempotente y no re-descarga un modelo ya presente.

**Provisión por SO** (experiencia homóloga):

- **Windows**: el instalador agrega `tts-sidecar` al PATH de usuario (HKCU) y
  ofrece una casilla post-instalación que ejecuta `setup` en tu contexto de
  usuario (con `install-windows.ps1`, el propio script ejecuta `setup` al terminar).
- **Linux**: `setup` es el punto único de provisión. Ejecutado desde el AppImage,
  además de descargar el modelo crea el symlink `~/.local/bin/tts-sidecar`
  apuntando al AppImage, dejando el comando invocable por nombre:

  ```bash
  # Primer uso con el AppImage descargado
  chmod +x tts-sidecar-0.1.0-x86_64.AppImage
  ./tts-sidecar-0.1.0-x86_64.AppImage setup
  # → symlink en ~/.local/bin + chequeos + descarga del modelo

  # Desde entonces, en una terminal nueva:
  tts-sidecar speak --text "Hola"

  # Desinstalación limpia en un comando (datos + symlink + AppImage):
  tts-sidecar setup --uninstall
  # Reversión fina de solo el symlink de PATH (sin borrar nada más):
  tts-sidecar setup --remove-path
  ```

  Si `~/.local/bin` no está en tu PATH, `setup` te lo advierte con la línea a
  añadir al shell profile.
- **macOS**: el `.dmg` incluye `Instalar (PATH + modelo).command`, que enlaza
  `tts-sidecar` en `~/.local/bin` (per-user, **sin sudo**), avisa si esa ruta no
  está en tu PATH y a continuación ofrece ejecutar `setup` como tu usuario. Para
  desinstalar, el mismo `.dmg` trae `Desinstalar (quitar del PATH).command`, que
  elimina el symlink (y detecta un symlink legado en `/usr/local/bin` de
  versiones anteriores a 0.5.0, indicando cómo quitarlo); el `.app` se borra
  arrastrándolo a la Papelera.

> **Importante**: hasta que el modelo esté provisionado, `speak` y `daemon start`
> **abortan de inmediato** con un mensaje que remite a `tts-sidecar setup`. Nunca
> disparan una descarga silenciosa.

## Comandos

Tanto los comandos de lectura (`version`, `doctor`, `devices`, `voice list`,
`daemon status`) como los de escritura (`voice clone`, `voice remove`, `setup`,
`cleanup`) aceptan `--json` para salida legible por máquina, útil al invocar
`tts-sidecar` desde otro programa: ningún comando obliga a parsear texto.

Todo payload `--json` incluye el campo **`"schema_version"`** (actualmente
`"1"`), que identifica la forma del esquema. Es un campo aditivo: añadir claves
nuevas no lo incrementa; solo un cambio incompatible de las claves existentes lo
haría. Un consumidor puede leerlo para detectar cambios de contrato.

### Referencia de esquemas `--json`

Los payloads siguientes son **parte del contrato programático**: sus claves son
estables (los cambios solo pueden ser aditivos mientras `schema_version` sea
`"1"`). En todos los casos, stdout contiene exactamente un objeto JSON y el
diagnóstico/progreso va a stderr. La clave `schema_version` (string) se omite de
las tablas por brevedad: está presente en todos.

**`speak --json`** — requiere `--output` (el archivo es el canal de datos;
`--json` solo emite metadatos/métricas a stdout). Sin `--output`, error en
stderr y exit 4. El payload es idéntico campo a campo en modo directo y vía
daemon.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `output` | string | Ruta absoluta del archivo WAV escrito |
| `voice` | string | Nombre de la voz efectivamente usada (`"default"` si no se dio `--voice`) |
| `t3_time` | number | Segundos del T3 autoregresivo |
| `s3gen_time` | number | Segundos del vocoder S3Gen |
| `daemon` | boolean | `true` si la síntesis se despachó vía daemon, `false` en modo directo |

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
| `name` | string | Siempre `"tts-sidecar"` |
| `version` | string | Versión del programa (p. ej. `"0.1.0"`) |

**`doctor --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `python` | string | Versión de Python del runtime |
| `platform` | string | Sistema y versión (p. ej. `"Windows 11"`) |
| `checks` | array de objetos | Un objeto por chequeo: `status` (`"PASS"`/`"FAIL"`/`"WARN"`/`"SKIP"`), `name` (string), `detail` (string) |
| `passed` | number | Conteo de chequeos `PASS` |
| `failed` | number | Conteo de chequeos `FAIL` (si > 0, exit 1) |

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
| `running` | boolean | Si el daemon responde al health check |
| `status` | string | Solo con `running: true`: estado reportado (`"healthy"`, `"initializing"`, `"unknown"`) |
| `model_loaded` | boolean | Solo con `running: true`: si el modelo está cargado |
| `uptime_seconds` | number | Solo con `running: true`: segundos desde el arranque |

**`voice clone --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Nombre de la voz registrada |
| `reference` | string | Ruta absoluta del `timbre-reference.wav` copiado (timbre) |
| `speech` | string | Ruta absoluta del `speech-reference.wav` copiado (conditioning) |

**`voice remove --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `name` | string | Nombre de la voz eliminada |
| `removed` | boolean | `true` en la rama de éxito (los errores conservan su mensaje en stderr y su exit code, sin payload) |

**`setup --json`**

| Clave | Tipo | Significado |
|-------|------|-------------|
| `model` | string | Alias del modelo provisionado (`"es-mx-latam"`) |
| `already_cached` | boolean | `true` si no hubo nada que descargar (idempotencia) |
| `downloaded` | boolean | `true` si esta ejecución descargó el modelo |
| `cache_dir` | string | Raíz de la caché de HuggingFace usada |

Con `--remove-path` (Linux), el payload es distinto: `remove_path` (boolean,
siempre `true`) y `removed` (boolean, `true` si el symlink existía y se quitó).

Con `--uninstall` (los tres SO; requiere `--yes`, como `cleanup --json`), el
payload es: `uninstall` (boolean, siempre `true`) y `removed` (lista de las rutas
eliminadas **en proceso**: las de datos del `cleanup` encadenado, el directorio
raíz de datos si quedó vacío, el symlink de PATH y —en Unix— el binario). En
**Windows** el binario lo borra el desinstalador de Inno tras la salida del
proceso, así que no entra en `removed` sino en un campo adicional `delegated`
(lista con el directorio de instalación); afirmarlo en `removed` sería falso
porque aún existe cuando se emite el payload.

**`cleanup --json`** — requiere `--yes` o `--dry-run` (la confirmación
interactiva contaminaría stdout); sin ellos, error en stderr y exit 4. Los
listados informativos van a stderr.

| Clave | Tipo | Significado |
|-------|------|-------------|
| `removed` | array de strings | Rutas eliminadas (o que se eliminarían, con `--dry-run`) |
| `dry_run` | boolean | `true` si no se borró nada (solo listado) |

---

### `version`

Muestra la versión del programa.

```bash
tts-sidecar version
tts-sidecar version --json
```

**Qué esperar:**

```
tts-sidecar 0.1.0
```

---

### `doctor`

Verifica que todos los componentes estén disponibles: la librería TTS, el
subsistema de audio, el modelo descargado y las voces.

```bash
tts-sidecar doctor
tts-sidecar doctor --json
```

**Qué esperar** (entorno sano):

```
=== TTS Sidecar Doctor ===

Python: 3.13.x ...
Plataforma: Windows 11 / Linux 6.x / Darwin 24.x

[PASS] Chatterbox TTS: 0.3.x
[PASS] Audio library: pycaw (Windows) — 2 dispositivo(s)
[PASS] Chatterbox model: es-mx-latam presente en la caché
[PASS] Voices directory: 1 voz(voces) disponible(s)
[PASS] RAM: 16.0 GB

Chequeos: 5 exitosos, 0 fallidos
```

Termina con código de salida 0 si todo pasa, y 1 si algún chequeo falla (cada
`[FAIL]` indica cómo corregirlo, p. ej. `ejecuta: tts-sidecar setup`).

> El número y los nombres de chequeos varían según el SO: por ejemplo, Windows
> añade el chequeo informativo `OneDrive user-data-dir` (ver solución de
> problemas), y Linux/macOS añaden `CPU AVX2` (SKIP en ARM). Un `[WARN]` o
> `[SKIP]` nunca cuenta como fallido ni altera el código de salida.

---

### `devices`

Lista los dispositivos de salida de audio disponibles.

```bash
tts-sidecar devices
tts-sidecar devices --json
```

**Qué esperar:**

```
Dispositivos de salida de audio:
  [0] Altavoces (Realtek High Definition Audio) (latency: 10.0ms)
  [1] Auriculares (latency: 8.0ms)
```

---

### `speech say`

Sintetiza texto y reproduce el audio inmediatamente por los altavoces.

Sin `--voice`, `speech say` usa la voz de fábrica **`default`** (empaquetada,
de solo lectura), por lo que el ejemplo mínimo funciona recién instalado, sin
clonar nada:

```bash
# Reproducir con la voz de fábrica 'default'
tts-sidecar speech say --text "Hola mundo"

# Usar una voz registrada
tts-sidecar speech say --text "Hola mundo" --voice mi_voz
```

**Qué esperar:** el comando reporta su progreso por etapas con timestamps. En
modo directo (sin daemon), la primera etapa es la carga del modelo (~15–30 s) y
luego la síntesis; al final, el audio suena por los altavoces. En una terminal
interactiva verás además un indicador de progreso en vivo (etapa y avance de
tokens del T3) sobre stderr; ver «Progreso en vivo» más abajo. El siguiente
ejemplo es la salida capturada (sin TTY):

```
Iniciando speech say...
[10:00:01] Usando modelo en caché: es-mx-latam (...)...
[10:00:20] Modelo cargado: es-MX-Latam (vocab=2454, compute_backend=cpu, builtin_voice=sí)...
[10:00:20] [1-Speak] Etapa 1/4: Cargando conditionals...
[10:00:21] [1-Speak] -> Done (1.0s)
[10:00:21] [2-Speak] Etapa 2/4: Generando audio (TTS)...
[10:00:39]    [Etapa 2a] T3 autoregresivo: 12.0s...
[10:00:39]    [Etapa 2b] S3Gen vocoder:   6.0s...
[10:00:39] [2-Speak] -> Done (18.2s)
[10:00:39] [3-Speak] Etapa 3/4: Convirtiendo a WAV...
[10:00:39] [3-Speak] -> Done (0.1s)
[10:00:39] [Reproducción] Reproduciendo audio...
[10:00:42] [Reproducción] Reproducción finalizada
Finalizado en 41.5s
```

**Orígenes de voz (resolución usuario→fábrica):**
- **Fábrica**: voces empaquetadas en el ejecutable, de solo lectura (incluye
  `default`). Idénticas en desarrollo y en cualquier instalación.
- **Usuario**: voces registradas con `voice clone`, escribibles, guardadas en el
  directorio de datos de usuario por SO (estables entre ejecuciones). Una voz de
  usuario con el mismo nombre que una de fábrica la sobrescribe.

**Opciones:**
- `--text, -t` (requerido): Texto a sintetizar
- `--voice, -v`: Nombre de la voz registrada a usar (auto-carga sus dos audios)
- `--daemon`: Usar el daemon sin sondeo previo; si falla, el error se reporta (sin fallback a directo)
- `--no-daemon`: Forzar modo directo, sin sondear el daemon

`--daemon` y `--no-daemon` son **mutuamente excluyentes**: combinarlos produce
un error en stderr y exit 4 (`INVALID_INPUT`), antes de cualquier trabajo.
- `--compute-backend, -cb`: Backend de cómputo para la inferencia (`auto`, `cpu`, `cuda`, `mps`; default: `auto`)

Sin valor, `tts-sidecar` detecta automáticamente el mejor backend disponible
(CUDA → MPS → CPU) y lo usa durante toda la sesión. El backend elegido se
muestra en el log de arranque del motor (`Modelo cargado: …,
compute_backend=…`). Si quieres forzar uno concreto (p. ej. CPU para
reproducibilidad), pasa el valor explícito. **Nota:** vía daemon el backend
queda fijado desde su arranque; un `--compute-backend` distinto de `auto` en
una invocación individual se ignora y `speech say` avisa por stderr. Para forzar
un backend distinto usa `--no-daemon`, o reinicia el daemon con la variable
de entorno correspondiente.

**Límite de longitud del texto:** `--text` acepta hasta 5000 caracteres; por
encima de ese límite `speech say` falla con exit 4 (`INVALID_INPUT`) antes de
intentar sintetizar, en modo directo o vía daemon. Por encima de 2000
caracteres (sin llegar a 5000) se emite una advertencia no bloqueante por
stderr: el T3 topa la generación en 500 tokens, así que un texto muy largo
puede truncarse en el audio resultante — se recomienda fragmentar el texto en
varias llamadas a `speech say`.

No hay opción de modelo: TTS Sidecar está especializado en español
latinoamericano y usa siempre el modelo `es-mx-latam` provisionado por `setup`.
Tú solo gestionas las voces.

**Ejemplos:**
```bash
# Usando voz registrada
tts-sidecar speech say --text "Hola mundo" --voice mi_voz

# Forzar modo directo
tts-sidecar speech say --text "Hola" --voice mi_voz --no-daemon
```

---

### `voice clone`

Clona una voz a partir de dos archivos de audio.

```bash
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav
```

**Qué esperar:** el comando valida que ambos audios sean cargables, copia los
archivos al directorio de voces de usuario, **precomputa los conditionals** de
la voz (`conditionals.pt`) y confirma:

```
Iniciando voice_clone...
Voz 'mi_voz' clonada:
  timbre (reference): <ruta>/voices/mi_voz/timbre-reference.wav
  habla (conditioning): <ruta>/voices/mi_voz/speech-reference.wav
  conditionals: precomputados
Finalizado en 3.1s
```

A partir de ese momento la voz aparece en `voice list` y puede usarse con
`speak --voice mi_voz`.

El clonado **precomputa los conditionals** en el momento de clonar, de modo que
toda síntesis posterior con `speak --voice mi_voz` los carga desde disco en vez
de recomputarlos (latencia estable, sin sobrecosto en la primera reproducción).
Por eso `voice clone` requiere el modelo provisionado (`tts-sidecar setup`): el
precómputo ejecuta el modelo. Si hay un [daemon](#modo-daemon) activo, el
precómputo aprovecha el modelo ya caliente y es casi inmediato; si no, el
comando carga el modelo una vez (unos segundos) para precomputar.

Si el precómputo falla (por ejemplo, un audio problemático), el clonado **no se
aborta**: la voz queda registrada con un aviso por stderr y sus conditionals se
computarán en la primera síntesis.

**Opciones:**
- `--name, -n` (requerido): Nombre para la voz
- `--timbre-reference, -t` (requerido): Audio para timbre (cualquier largo — el audio completo se usa para el embedding)
- `--speech-reference, -s` (requerido): Audio para conditioning (10+ segundos de habla limpia)
- `--force, -f`: Sobrescribir la voz si ya existe (incluida una de fábrica homónima)
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
tts-sidecar voice list
tts-sidecar voice list --json
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
tts-sidecar voice remove --name mi_voz
```

**Qué esperar:**

```
Voz 'mi_voz' eliminada.
```

Las voces de fábrica (como `default`) son de solo lectura y no pueden
eliminarse; el comando lo indica y termina con error si lo intentas.

---

### `cleanup`

Desaprovisiona los datos del proyecto: el modelo descargado y/o las voces de
usuario. Es la contraparte de `setup` y completa el ciclo de vida
instalación→desinstalación.

```bash
tts-sidecar cleanup --model      # elimina el modelo descargado
tts-sidecar cleanup --voices     # elimina las voces de usuario
tts-sidecar cleanup --all        # ambos
tts-sidecar cleanup --all --dry-run   # lista lo que se borraría, sin borrar
tts-sidecar cleanup --all --yes       # borra sin pedir confirmación (uso programático)
tts-sidecar cleanup --all --yes --json   # salida JSON (requiere --yes o --dry-run)
```

**Qué esperar:** el comando lista las rutas exactas a eliminar y pide
confirmación (`s/n`) antes de borrar; con `--dry-run` solo lista. Sin flags
muestra la ayuda y no borra nada. `--yes`/`-y` omite la confirmación
interactiva — pensado para invocar `cleanup` vía `subprocess` con stdin
cerrado; sin `--yes` y con stdin cerrado, la falta de respuesta se trata como
cancelación limpia («Cancelado: no se borró nada.», exit 0), no como error.

El borrado es **quirúrgico**: dentro de la caché de HuggingFace solo se
eliminan las carpetas de los dos repos que usa el proyecto
(`Chatterbox-Multilingual-es-mx-latam` y `chatterbox` de ResembleAI), nunca
modelos de otros proyectos; `--voices` elimina únicamente el directorio de
voces de usuario. Todo es recuperable: `setup` reprovisiona el modelo y
`voice clone` vuelve a clonar voces.

---

## Desinstalación completa

**Canal nativo (los tres SO), en un comando**: `tts-sidecar setup --uninstall`
encadena `cleanup --all` (modelo + voces), revierte la integración de PATH y
borra el binario, **en ese orden** (datos independientes primero, ancla al
final). Cancelar el cleanup aborta la desinstalación sin borrar nada (salida 0).
Con `--yes` se omite la confirmación; con `--json` (que exige `--yes`) emite un
payload con `schema_version`, `uninstall` y `removed`. Solo aplica a la
instalación nativa (AppImage / `.app` / Inno); desde fuente o pip/uv aborta
remitiendo a `pip uninstall tts-sidecar`.

- **Linux (AppImage)**: quita el symlink de PATH y borra
  `~/.local/opt/tts-sidecar/`. (`setup --remove-path` se conserva como reversión
  fina de solo el symlink, sin borrar nada más.)
- **macOS (`.app`)**: quita el symlink de `~/.local/bin` y borra el `.app`
  (resuelto desde el ejecutable, cubre `~/Applications`, `/Applications` y el
  Cask). Si instalaste con **Homebrew Cask**, el comando lo detecta por la
  metadata del Caskroom y **remite a `brew uninstall --cask --zap tts-sidecar`**
  sin borrar nada, para no dejar el Caskroom inconsistente (su `zap` ya cubre los
  datos). **Nota de transición**: si instalaste una versión anterior a 0.5.0, el
  symlink vivía en `/usr/local/bin`; el `.command` de desinstalación del `.dmg` lo
  detecta e indica cómo quitarlo (`sudo rm /usr/local/bin/tts-sidecar`).
- **Windows (Inno)**: borra los datos en proceso y **delega** el binario y la
  reversión del PATH (HKCU) al desinstalador de Inno, lanzado desacoplado (el SO
  mantiene el lock del `.exe`). El directorio de instalación se reporta en el
  campo `delegated` del payload, no en `removed`. La vía idiomática (Configuración
  → Aplicaciones, sin admin) sigue disponible como alternativa.
- **PyPI**: `uv tool uninstall tts-sidecar` / `pipx uninstall tts-sidecar` (más
  `cleanup --all` para los datos, que la desinstalación del paquete no toca).

---

## Actualizar de versión

`tts-sidecar` no tiene auto-actualización: cada nueva versión se instala
manualmente sobre (o junto a) la anterior. El modelo y las voces en el
directorio de datos de usuario no se ven afectados por la actualización del
binario.

- **Windows**: descarga el nuevo instalador y ejecútalo (o repite el one-liner
  `irm | iex`); Inno Setup reemplaza la instalación per-user anterior en el
  mismo directorio y conserva el PATH. Si vienes de una versión per-machine
  (anterior a 0.4.0, instalada en Program Files), desinstálala primero desde el
  Panel de control (con admin): instalar la per-user encima dejaría dos
  instalaciones y PATH duplicado.
- **Linux**: repite el one-liner `curl -fsSL …/install-linux.sh | sh`. Instala el
  `.AppImage` nuevo, reapunta el symlink `~/.local/bin/tts-sidecar` y **elimina
  los AppImages anteriores** de `~/.local/opt/tts-sidecar/` (ya no acumulan
  gigabytes). En la vía manual (descargar el `.AppImage` a mano), vuelve a
  correr `setup` **desde el archivo nuevo**:
  `./tts-sidecar-<versión-nueva>-x86_64.AppImage setup`; esto reapunta el
  symlink — si solo reemplazas el archivo sin volver a ejecutar `setup`, el
  symlink sigue apuntando a la ruta del AppImage viejo y el comando deja de
  funcionar. Borra el `.AppImage` anterior una vez confirmado que el nuevo
  funciona.
- **macOS**: repite el one-liner `curl -fsSL …/install-macos.sh | sh` (opción
  primaria: descarga, verifica, reemplaza el `.app` en `~/Applications` y
  reapunta el symlink); o `brew upgrade --cask tts-sidecar` si instalaste con
  Homebrew. En la vía manual, monta el `.dmg` nuevo, arrastra el `.app` a
  Aplicaciones (sobrescribiendo el anterior) y vuelve a ejecutar el script
  `Instalar (PATH + modelo).command` del volumen nuevo.
- **PyPI**: `uv tool upgrade tts-sidecar` / `pipx upgrade tts-sidecar`.

En los cuatro casos, el modelo descargado (`~/.cache/huggingface/hub`) se
reutiliza tal cual. Cada versión del binario fija la revisión exacta del modelo
que usa: si tu caché contiene otra revisión (por ejemplo, la de una versión
anterior), `setup` la detecta como no provisionada y descarga la revisión
requerida, reutilizando los archivos que no cambiaron entre revisiones (la
caché de HuggingFace deduplica por contenido).

---

## Modo Daemon

El daemon mantiene el modelo cargado en memoria, evitando el tiempo de carga en
cada invocación (~15–30 s de overhead). Es el modo recomendado cuando vas a
sintetizar varias veces seguidas.

### Gestión del daemon

```bash
# Iniciar daemon (background; puerto fijo: 8765 en loopback, no configurable)
tts-sidecar daemon start

# Ver estado
tts-sidecar daemon status
tts-sidecar daemon status --json

# Reiniciar
tts-sidecar daemon restart

# Detener
tts-sidecar daemon stop

# Auto-reinicio en caso de crash
tts-sidecar daemon start --autorestart --max-retries 3
```

**Qué esperar:** `daemon start` verifica que el modelo esté descargado, lanza el
servidor en segundo plano y confirma con `Daemon iniciado correctamente`. Luego
`daemon status` muestra:

```
Daemon en ejecución:
  Estado: healthy
  Modelo cargado: True
  Tiempo activo: 42.3s
```

`daemon stop` responde `Daemon detenido` y `daemon restart`, `Daemon reiniciado`.

### Uso con daemon

`speak` despacha según tres ramas:

- **Sin flags**: sondea el daemon con un health check corto y lo usa si responde;
  si no, cae al modo directo sin error.
- **`--daemon`**: asume el daemon disponible y le envía la síntesis sin sondeo
  previo; un fallo se reporta como error (sin fallback silencioso).
- **`--no-daemon`**: modo directo, sin ningún sondeo.

```bash
# El daemon se usa automáticamente si está disponible
tts-sidecar speak --text "Hola" --voice mi_voz

# Forzar modo daemon (falla si el daemon no responde)
tts-sidecar speak --text "Hola" --voice mi_voz --daemon

# Forzar modo directo (sin daemon)
tts-sidecar speak --text "Hola" --voice mi_voz --no-daemon
```

**Qué esperar** con el daemon activo: `speak` omite la etapa de carga del modelo
y la síntesis empieza de inmediato. Aunque la síntesis ocurre en el proceso del
daemon, su **progreso real** viaja al cliente por el stream de `/synthesize`
(etapa actual y conteo de tokens del T3 en vivo):

```
Iniciando speak...
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
síntesis `speak` muestra sobre **stderr** un indicador giratorio que se actualiza
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
- **Disco**: ~4 GB para el modelo descargado (`setup` aborta si hay menos de 6 GB
  libres). El bundle PyInstaller `--onedir` ocupa del orden de 1-2 GB adicionales
  sin comprimir (Windows/macOS y el AppImage `arm64` de Linux, que resuelven
  `torch` desde el lock universal); el AppImage `x86_64` de Linux es más liviano
  al instalar `torch`/`torchaudio` CPU-only sin el stack `nvidia-*-cu12` (ver
  [docs/BUILD.md](docs/BUILD.md) — tamaño exacto pendiente de medición en CI).
- **GPU (opcional)**: con `--compute-backend cuda` (NVIDIA) o `mps` (Apple Silicon)
  la inferencia es más rápida; no es necesaria para el funcionamiento.
- **Linux — glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+, Fedora 36+ o equivalente):
  requisito de los wheels manylinux empaquetados (torch, onnxruntime). Ver la
  entrada correspondiente en «Solución de Problemas» más abajo.

---

## Clonación de voz: recorrido completo

De principio a fin, desde grabar tu voz hasta escucharla sintetizada:

```bash
# 1. Graba dos audios en español (WAV 16-bit, sin ruido de fondo):
#    timbre.wav  - cualquier largo, captura tu timbre
#    habla.wav   - 10+ segundos de habla limpia y continua

# 2. Clona la voz
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav
# → Voz 'mi_voz' clonada: (rutas de los dos archivos copiados)

# 3. Verifica que aparece
tts-sidecar voice list
# → Voces registradas: default, mi_voz

# 4. Escúchala
tts-sidecar speak --text "Hola, esto es una prueba" --voice mi_voz
# → etapas de síntesis + reproducción por los altavoces

# 5. O genera un archivo
tts-sidecar speak --text "Hola, esto es una prueba" --voice mi_voz --output mi_voz.wav
# → [Archivo] Audio guardado: mi_voz.wav
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
  errores a stderr, siempre en UTF-8. Esto hace a `tts-sidecar` consumible por
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
  | `4` | Modelo no provisionado | `speak`/`daemon start` sin ejecutar `setup` |
  | `5` | Daemon inalcanzable | `speak --daemon` sin daemon; `daemon start/stop/restart` fallido |
  | `6` | Conflicto de estado | Colisión en `voice clone` sin `--force`; voz ocupada; puerto del daemon en uso |
  | `7` | Operación no aplicable | Voz de fábrica de solo lectura; plataforma no soportada; `setup --uninstall` fuera del canal nativo |
  | `8` | Precondición de entorno incumplida | Credenciales, red, permisos o disco insuficientes al provisionar |
  | `130` | Interrupción del usuario | Ctrl+C (128 + SIGINT) durante cualquier comando |
- **La voz `default` y el modelo** son los mismos en todas las plataformas: el
  audio generado para un mismo texto y voz es equivalente en cualquier SO.
- **El motor auto-detecta el mejor backend de cómputo disponible** (CUDA en
  NVIDIA, MPS en Apple Silicon, CPU en otro caso); se puede forzar uno
  concreto con `--compute-backend` (`auto` por default). La detección corre
  una sola vez al cargar el motor (en cada `speak` sin daemon, y una vez por
  arranque del daemon).

Las únicas diferencias son internas y no cambian la forma de usar la aplicación:

| Aspecto | Windows | Linux | macOS |
|---------|---------|-------|-------|
| Reproducción de audio | winsound (integrado) | sounddevice (PortAudio) | afplay (integrado) |
| Enumeración de dispositivos | pycaw | sounddevice | sounddevice |
| Voces de usuario (binario) | `%LOCALAPPDATA%\tts-sidecar\voices` | `~/.local/share/tts-sidecar/voices` | `~/Library/Application Support/tts-sidecar/voices` |
| Caché del modelo | `~/.cache/huggingface/hub` | `~/.cache/huggingface/hub` | `~/.cache/huggingface/hub` |

> La caché del modelo respeta las variables de entorno `HF_HUB_CACHE` y `HF_HOME`
> si están definidas (misma resolución que usa HuggingFace Hub); la ruta de la
> tabla es el valor por defecto.

---

## Formato de Audio

- **Generación**: 24000 Hz, Mono
- **Exportación WAV**: 16-bit PCM, 24000 Hz, Mono

## Solución de Problemas

### "el modelo 'es-mx-latam' no está descargado"

`speak` y `daemon start` requieren el modelo provisionado y nunca lo descargan
por sí mismos. Ejecuta la provisión una vez:

```bash
tts-sidecar setup
```

### "GLIBC_2.35 not found" (o similar) al ejecutar el AppImage en Linux

El AppImage requiere **glibc ≥ 2.35** (Ubuntu 22.04+, Debian 12+, Fedora 36+ o
equivalente): es la versión mínima que soportan los wheels manylinux de las
dependencias empaquetadas (torch, onnxruntime). En una distro más antigua
(p. ej. Ubuntu 20.04, Debian 11) el binario no arranca. No hay solución desde
el AppImage mismo: actualiza la distro a una versión con glibc ≥ 2.35, o
compila `tts-sidecar` desde código fuente en tu distro actual (ver
[docs/BUILD.md](docs/BUILD.md)).

### "OneDrive user-data-dir" [WARN] en doctor (Windows)

En perfiles corporativos, `LOCALAPPDATA` (donde `tts-sidecar` guarda las voces de
usuario) puede caer bajo una jerarquía de **OneDrive**. Eso expone los archivos de
voz a *file locks* y a *placeholders* «a petición» (Files On-Demand), que causan
fallos de lectura esporádicos e inatribuibles al cargar una voz.

`doctor` emite `[WARN] OneDrive user-data-dir` cuando detecta que `data_root()`
está bajo la sincronización de OneDrive (vía las variables de entorno
`OneDrive`/`OneDriveCommercial`, o por patrón de ruta). Es un aviso informativo:
no bloquea nada ni cambia dónde se guardan las voces. Para mitigarlo:

- **Excluye** la carpeta de datos de `tts-sidecar` (`%LOCALAPPDATA%\tts-sidecar`)
  de la sincronización de OneDrive, o
- **Deshabilita Files On-Demand** para esa carpeta, de modo que sus archivos se
  descarguen siempre y no queden como marcadores bajo demanda.

### "Voice 'x' not found"

Verifica que la voz existe:

```bash
tts-sidecar voice list
```

### "La voz 'x' ya existe"

`voice clone` no sobrescribe voces por accidente. Si quieres reemplazarla:

```bash
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference habla.wav --force
```

### "timbre-reference.wav/speech-reference.wav not found"

La voz no tiene los archivos necesarios. Puede que se registró con el formato
antiguo. Vuelve a clonar:

```bash
tts-sidecar voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav --force
```

### "Voz 'x' es una voz de fábrica (solo lectura)"

Las voces empaquetadas (como `default`) no pueden eliminarse con `voice remove`.
Si quieres reemplazar su sonido, clona una voz de usuario con el mismo nombre
usando `voice clone --force`: la tuya toma precedencia.

### Error al eliminar una voz: "uno de sus archivos parece estar en uso"

Otro proceso (el daemon, un reproductor de audio) tiene abierto alguno de los
archivos de la voz. Ciérralo (p. ej. `tts-sidecar daemon stop`) y reintenta.

### Sin audio de salida

1. Verifica que `tts-sidecar devices` detecta tu dispositivo
2. Comprueba que el volumen del sistema no está en mute
3. Verifica que el dispositivo de audio predeterminado es correcto
4. Ejecuta `tts-sidecar doctor`: el chequeo "Audio library" falla si el host no
   tiene un subsistema de audio funcional (p. ej. sesiones remotas o headless)

En un host sin audio puedes seguir usando la síntesis a archivo
(`tts-sidecar speak --text "Hola" --output audio.wav`); `setup` también funciona
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

- **macOS (Gatekeeper)**: al abrir el `.app`/`.dmg` por primera vez, haz clic
  derecho sobre él → **Abrir** y confirma; o quita la cuarentena desde una
  terminal:

  ```bash
  xattr -d com.apple.quarantine /Applications/tts-sidecar-arm64.app
  ```

Esto solo ocurre en el primer arranque; las ejecuciones posteriores no vuelven a
pedir confirmación.

> El canal PyPI (`uv tool install tts-sidecar`) no dispara ninguno de los dos
> avisos: el launcher lo genera `uv`/`pipx` localmente, sin Mark-of-the-Web ni
> cuarentena. Ver [docs/DISTRIBUTION.md](docs/DISTRIBUTION.md) si prefieres ese
> canal.

Antes de aceptar, puedes comprobar objetivamente que el archivo es el que
publicó el proyecto cotejando su SHA-256 contra el `SHA256SUMS.txt` del
Release (ver [SECURITY.md](SECURITY.md#artefactos-sin-firmar)):

```powershell
# Windows (PowerShell)
Get-FileHash .\tts-sidecar-X.Y.Z-x86_64-setup.exe -Algorithm SHA256
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

`tts-sidecar` permite clonar voces arbitrarias a partir de unos segundos de audio.
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
  [Issue](https://github.com/CristianRojas-SoftwareEngineer/TTS-Sidecar/issues)
  describiendo la situación.

TTS Sidecar es software libre y no impone barreras técnicas al uso (serían
triviales de sortear); establece, en cambio, la diligencia debida esperada en la
comunidad de IA de código abierto. La responsabilidad del uso legítimo recae en
la persona que ejecuta la herramienta.

## Licencia

`tts-sidecar` se distribuye bajo **GPL-3.0-or-later** (ver [LICENSE](LICENSE)). El modelo
Chatterbox se distribuye bajo MIT; las dependencias empaquetadas conservan sus propias
licencias, en su mayoría permisivas (MIT/BSD/Apache/ISC/PSF) y algunas de copyleft
compatible con GPLv3 (LGPL-2.1+, MPL-2.0 y el GPLv3+ de pykakasi), detalladas en
[THIRD-PARTY-LICENSES.md](THIRD-PARTY-LICENSES.md).
