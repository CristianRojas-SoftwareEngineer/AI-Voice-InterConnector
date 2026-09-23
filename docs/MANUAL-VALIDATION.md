# Validación manual de la superficie de la CLI

Este documento es el **procedimiento operativo** de la validación end-to-end que
[docs/GOAL.md](GOAL.md) §"Validación E2E" define a nivel de política: el recorrido
manual `instalar → setup → síntesis real → desinstalar` que el propietario
ejecuta en Windows sobre cada release, y que sirve de guion para el feedback de
usuarios reales en Linux y macOS. El pipeline de CI solo corre un **smoke test
automatizado** del binario congelado (`ai-voice-interconnector version`, exit 0); la matriz de
comandos de abajo es la parte que **no** cabe en un runner de CI porque exige
cargar Qwen3-TTS + Parakeet, descargar ~9 GB base (~11,5 GB con `--with-voice-cloning`),
sintetizar audio real y —en las rutas interactivas— un micrófono y una terminal
con una persona pulsando teclas.

La secuencia está en orden lógico: cada paso asume que el anterior pasó. Ejecutar
tras instalar el artefacto del release (Windows `ai-voice-interconnector-X.Y.Z-x86_64-windows.zip`,
Linux `ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz`, macOS `ai-voice-interconnector-X.Y.Z-arm64-macos.tar.gz`)
descomprimido y con `setup` ejecutado, o bien desde una terminal nueva (el instalador agrega el `PATH`
automáticamente).

## Cómo leer esta guía

Cada comprobación declara su **resultado esperado** y su **exit code**. Tras cada
comando, léelo según tu shell:

| Shell | Leer exit code |
|---|---|
| Git Bash / POSIX (Linux, macOS) | `echo $?` |
| PowerShell | `$LASTEXITCODE` |
| cmd | `echo %ERRORLEVEL%` |

> Los comandos se muestran para una shell POSIX. En Windows (`cmd`/PowerShell)
> son equivalentes salvo `which ai-voice-interconnector`, que allí es `where ai-voice-interconnector`.

**Prerrequisitos por escenario** — los iconos al inicio de cada sección o ficha indican qué hace falta además del binario instalado:

| Icono | Requiere |
|---|---|
| 🧠 | Modelos provisionados (`setup` ejecutado) |
| 🔊 | Dispositivo de salida de audio (altavoz/auriculares) |
| 🎤 | Micrófono de entrada |
| 🖥️ | Terminal interactiva **real** (TTY) — no una tubería ni `< archivo` |
| 🗣️ | Un archivo de audio de referencia (muestra de voz o clip a transcribir/doblar) |

Las secciones **1-4** no necesitan modelos; a partir de **5** asume 🧠. Las
rutas de **§8 (transcripción/doblaje)** y **§9 (interactivas)** son las que CI no
puede ejercitar y el núcleo de esta validación.

## Tabla de contenidos

- [Cómo leer esta guía](#cómo-leer-esta-guía)
- [1. Entorno y versión](#1-entorno-y-versión)
- [2. Diagnóstico del entorno](#2-diagnóstico-del-entorno)
- [3. Provisión del modelo](#3-provisión-del-modelo)
- [4. Dispositivos de audio](#4-dispositivos-de-audio)
- [5. Síntesis y reproducción](#5-síntesis-y-reproducción)
- [6. Gestión de voces](#6-gestión-de-voces)
- [7. Gestión de habla sintética](#7-gestión-de-habla-sintética)
- [8. Transcripción y doblaje (no interactivos)](#8-transcripción-y-doblaje-no-interactivos)
- [9. Rutas interactivas de audio (push-to-talk y `--play`)](#9-rutas-interactivas-de-audio-push-to-talk-y---play)
  - [9.1 Push-to-talk (`speech transcribe --mic` / `speech dub --mic` sin `--duration`)](#91-push-to-talk-speech-transcribe---mic--speech-dub---mic-sin---duration)
  - [9.2 Bucle interactivo de `speech synthesize --play`](#92-bucle-interactivo-de-speech-synthesize---play)
- [10. Daemon](#10-daemon)
- [11. Casos de error por exit code canónico](#11-casos-de-error-por-exit-code-canónico)

## 1. Entorno y versión

```bash
# Verificar que el comando está en el PATH (Windows: where ai-voice-interconnector)
which ai-voice-interconnector

# Versión legible por humano
ai-voice-interconnector version

# Versión en JSON (contrato legible por máquina)
ai-voice-interconnector version --json
```

**Esperado**: `which`/`where` resuelve la ruta del binario; `version` imprime la versión a stdout (**exit 0**); `--json` imprime un objeto con el mismo dato.

## 2. Diagnóstico del entorno

```bash
# Diagnóstico completo: audio, modelo, dispositivos
ai-voice-interconnector doctor

# Diagnóstico en JSON
ai-voice-interconnector doctor --json
```

**Esperado**: informe de audio/modelo/dispositivos a stdout, **exit 0** si el entorno está sano. Un fallo de precondición de entorno sale con **exit 8**.

## 3. Provisión del modelo

🧠 (esta sección la genera) — solo si no se hizo desde el instalador.

```bash
# Descarga los 4 modelos pinneados a ~/.cache/huggingface/hub (idempotente)
ai-voice-interconnector setup
```

**Esperado**: descarga (o confirma en caché) los modelos; **exit 0**. Es idempotente: una segunda ejecución no vuelve a descargar.

## 4. Dispositivos de audio

```bash
# Listar solo dispositivos de salida (render)
ai-voice-interconnector devices

# En JSON
ai-voice-interconnector devices --json
```

**Esperado**: lista de dispositivos de salida a stdout, **exit 0**.

## 5. Síntesis y reproducción

🧠 🔊

`say` sintetiza y reproduce **sin** persistir. `synthesize` persiste una locución
reutilizable: **por defecto** guarda siempre; **con `--play`** la persistencia es
condicional al bucle interactivo (ver §9).

```bash
# Reproducir con la voz de fábrica 'default' (no persiste)
ai-voice-interconnector speech say --text "Hola mundo, esto es una prueba de síntesis de voz."

# Sintetizar y guardar como locución reutilizable (sin --play: persiste siempre)
ai-voice-interconnector speech synthesize --text "Guardando a archivo." --label prueba

# Forzar modo directo (sin daemon)
ai-voice-interconnector speech say --text "Modo directo." --no-daemon
```

**Esperado**: en `say` se oye el audio y **no** queda locución (no aparece en `speech list`), **exit 0**. En `synthesize` sin `--play` se persiste la locución `prueba` (aparece en `speech list`), **exit 0**. `--no-daemon` produce el mismo resultado por la vía directa.

## 6. Gestión de voces

🧠 · el clonado requiere 🗣️ (una muestra de habla ≥10 s)

```bash
# Listar voces disponibles (debe aparecer 'default' de fábrica)
ai-voice-interconnector voice list
ai-voice-interconnector voice list --json

# Registrar una voz de usuario con una sola muestra (caso base: --speech-reference,
# ≥10s, es el único obligatorio; el habla cubre también el Voice Encoder)
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav

# Registrar una voz de usuario con timbre y habla por separado (--timbre-reference
# es opcional; útil para separar timbre y prosodia)
ai-voice-interconnector voice clone --name mi_voz_dual --timbre-reference timbre.wav --speech-reference habla.wav

# Verificar que aparece la nueva voz
ai-voice-interconnector voice list

# Sintetizar con la voz registrada
ai-voice-interconnector speech say --text "Esta es mi voz clonada." --voice mi_voz

# Guardar síntesis con voz registrada como locución reutilizable
ai-voice-interconnector speech synthesize --text "Guardando con mi voz." --label saludo --voice mi_voz

# Eliminar la voz de usuario
ai-voice-interconnector voice remove --name mi_voz

# Confirmar que se eliminó
ai-voice-interconnector voice list
```

**Esperado**: `clone` registra la voz (aparece en `voice list`), **exit 0**; `say`/`synthesize --voice mi_voz` usan su timbre; `remove` la elimina y deja de aparecer, **exit 0**. Intentar `voice remove` sobre una voz de fábrica (`default`/`ryan`/`vivian`) sale con **exit 2**.

## 7. Gestión de habla sintética

🧠 · reproducir requiere 🔊

El almacén de locuciones (`speech synthesize` las persiste; estas sub-acciones
operan sobre ellas sin re-sintetizar).

```bash
# Listar locuciones guardadas (todas las voces; --voice/-v opcional filtra por voz)
ai-voice-interconnector speech list
ai-voice-interconnector speech list --json
ai-voice-interconnector speech list --voice mi_voz
ai-voice-interconnector speech list --voice mi_voz --json

# Sondas del filtro: identificador ilegal → exit 2, voz inexistente → exit 3
ai-voice-interconnector speech list --voice 'mala voz!' ; echo "exit=$?"
ai-voice-interconnector speech list --voice voz_que_no_existe ; echo "exit=$?"

# Reproducir una locución guardada sin re-sintetizar
ai-voice-interconnector speech play --label prueba

# Eliminar una locución guardada
ai-voice-interconnector speech remove --label prueba
```

**Esperado**: `list` muestra las locuciones persistidas, **exit 0**; `play` reproduce sin re-sintetizar, **exit 0**; `remove` la borra (deja de aparecer en `list`), **exit 0**. `play`/`remove` con una etiqueta inexistente salen con **exit 3**.

## 8. Transcripción y doblaje (no interactivos)

🧠 🗣️ · el doblaje reproduce, así que también 🔊 · requiere un build con la
feature `full` (o `native-stt`); los artefactos de release ya la traen.

Estas rutas usan un **archivo** de entrada (`--audio`), sin micrófono ni
interacción; validan el pipeline de transcripción y de doblaje de forma
scriptable. La captura por micrófono se valida en §9.

```bash
# Transcribir un WAV a texto (--source-language es obligatorio)
ai-voice-interconnector speech transcribe --audio clip.wav --source-language es-latam

# En JSON ({"text","source"})
ai-voice-interconnector speech transcribe --audio clip.wav --source-language es-latam --json

# Doblaje voz→voz desde archivo: transcribe → traduce → sintetiza → reproduce
# (--file es alias de --audio; --target-language por defecto es-latam)
ai-voice-interconnector speech dub --audio clip_en.wav --source-language en --target-language es-latam --voice mi_voz
```

**Esperado**: `transcribe` imprime el texto reconocido a **stdout** (o el objeto JSON con `--json`), **exit 0**. `dub` transcribe, traduce si los idiomas difieren, sintetiza con `--voice` y reproduce, **exit 0**. Un `--audio` inexistente sale con **exit 3**; un fallo del pipeline de transcripción, **exit 10**; de traducción, **exit 9**.

## 9. Rutas interactivas de audio (push-to-talk y `--play`)

🖥️ 🎤 🔊 🧠 · **no ejercitables por CI**: exigen una terminal real y una persona
pulsando teclas. Son el motivo principal de esta validación manual. La
especificación completa está en [docs/UX-AUDIO-INTERACTIVA.md](UX-AUDIO-INTERACTIVA.md).

### 9.1 Push-to-talk (`speech transcribe --mic` / `speech dub --mic` sin `--duration`)

**Guarda sin TTY** (rápida, no necesita micrófono ni modelos cargados para
rechazar). Redirige stdin para que **no** sea terminal:

```bash
ai-voice-interconnector speech transcribe --mic --source-language es-latam < /dev/null ; echo "exit=$?"
```
**Esperado**: **exit 2** (`usage_error`), aviso en **stderr** pidiendo `--duration N` al no haber terminal. **Sin panic ni backtrace** (regresión histórica ya corregida).

**Duración fija sigue intacta** (no interactiva, no push-to-talk):

```bash
ai-voice-interconnector speech transcribe --mic --duration 3 --source-language es-latam ; echo "exit=$?"
```
**Esperado**: graba 3 s fijos y transcribe a **stdout**, **exit 0**; sin panic.

**Ruta feliz push-to-talk** — en una terminal interactiva real:

```bash
ai-voice-interconnector speech transcribe --mic --source-language es-latam
```
**Esperado**:
1. En **stderr** aparece: `Grabando… pulsa Enter para detener.`
2. Hablas; la grabación continúa mientras hablas.
3. Pulsas **Enter** → se detiene y transcribe a **stdout** el texto reconocido, **exit 0**.

**Techo de seguridad** (Enter olvidado) — baja el tope con la variable de entorno y **no** pulses Enter:

```bash
AVI_PUSH_TO_TALK_MAX_SECS=5 ai-voice-interconnector speech transcribe --mic --source-language es-latam
```
**Esperado**: a los 5 s la captura se corta sola, en **stderr** aparece `Límite de grabación alcanzado (5 s); deteniendo.`, transcribe lo grabado y sale con **exit 0** (el techo **no** es error).

**`dub` se comporta igual**: repite las cuatro comprobaciones con `speech dub --mic --source-language … --target-language …`. Las cuatro vías (transcribe/dub × directo/daemon) comparten la misma captura.

### 9.2 Bucle interactivo de `speech synthesize --play`

**Guardas** (rápidas):

```bash
# --play con --json es incompatible:
ai-voice-interconnector speech synthesize --text "hola" --label p1 --play --json ; echo "exit=$?"
# --play sin TTY:
ai-voice-interconnector speech synthesize --text "hola" --label p1 --play < /dev/null ; echo "exit=$?"
```
**Esperado**: ambos **exit 2** (`usage_error`); el segundo con `--play requiere una terminal interactiva (TTY).`. No sintetiza ni guarda.

**Ruta feliz del bucle** — terminal real:

```bash
ai-voice-interconnector speech synthesize --text "Prueba del bucle interactivo." --label demo_loop --play
```
**Esperado**: sintetiza **una vez**, reproduce la toma, y en **stderr** muestra:
```
Opciones: [1] mantener (reproducir de nuevo)  [2] guardar  [3] repetir (nueva síntesis)  [4] descartar
Opción [1-4]:
```
Prueba cada opción:
- **`1`** → vuelve a reproducir la **misma** toma (sin re-sintetizar) y reaparece el menú.
- **`3`** → **re-sintetiza** una toma nueva, la reproduce y reaparece el menú.
- **`2`** → guarda la locución (**exit 0**); verifícalo con `ai-voice-interconnector speech list --json` (debe aparecer `demo_loop`).
- **`4`** o **Ctrl-D (EOF)** → imprime `Descartado.` y sale **exit 0** **sin** guardar nada.

**Recomprobación de colisión al guardar** (con una etiqueta que ya exista, sin `--force`):

```bash
ai-voice-interconnector speech synthesize --text "Otra toma." --label demo_loop --play
# entra al bucle y elige la opción 2 (guardar)
```
**Esperado**: al elegir `2`, **exit 6** (`label_exists`), sin sobrescribir — la colisión se recomprueba **en el instante de guardar**, no solo al inicio. Con `--force` en el comando, la opción `2` sí sobrescribe (**exit 0**).

## 10. Daemon

🧠 · síntesis vía daemon requiere 🔊

```bash
# Iniciar el daemon en segundo plano
ai-voice-interconnector daemon start

# Ver estado
ai-voice-interconnector daemon status

# Síntesis vía daemon (automático si está corriendo)
ai-voice-interconnector speech say --text "Síntesis con modelo en memoria." --daemon

# Reiniciar
ai-voice-interconnector daemon restart

# Detener
ai-voice-interconnector daemon stop

# Confirmar que se detuvo
ai-voice-interconnector daemon status
```

**Esperado**: `start` deja el daemon corriendo (`status` lo confirma), **exit 0**; `say --daemon` sintetiza con el modelo ya en memoria (más rápido que la vía directa), **exit 0**; `restart`/`stop` gestionan el ciclo de vida, **exit 0**. Un `--daemon` explícito contra un daemon inalcanzable sale con **exit 5**.

**Cero huérfanos tras `stop`** (comprobación a nivel de SO): sin procesos
`ai-voice-interconnector` ni `qwen_tts` residuales, puertos `8765`/`8766` cerrados y
`daemon.pid` sin PID vivo. El procedimiento usa los defaults (puerto 8765, `data_dir()` global);
con `AVI_DAEMON_PORT=0` el puerto es efímero y con `AVI_DATA_DIR` el pidfile es por instancia
(aislamiento de tests, no de este recorrido).

> Nota de cobertura: algunas variantes del reclamo de puerto/PID (reclamo por
> grupo ante líder muerto en Unix, barrido del puerto de control por el reaper)
> solo se alcanzan de forma fiable en Linux/CI o en entornos rápidos, no en la
> validación local de Windows; no las esperes en este recorrido.

## 11. Casos de error por exit code canónico

Cada exit code de dominio con una sonda que lo dispara. Todos escriben el mensaje
en **español a stderr** (con `reason` legible por máquina) y dejan stdout limpio.

| Exit | Significado | Sonda |
|---|---|---|
| **2** | Entrada inválida (`usage_error`) | `speech synthesize --text x --label y --play --json` (flags incompatibles); `speech transcribe --mic --source-language es-latam < /dev/null` (mic sin duración ni TTY); `voice remove --name default` (voz de fábrica) |
| **3** | Recurso no encontrado | `speech play --label etiqueta_que_no_existe`; `voice remove --name voz_que_no_existe`; `speech transcribe --audio no_existe.wav --source-language es-latam` |
| **4** | Modelo no provisionado | cualquier síntesis/transcripción **sin** haber corrido `setup`; también `dub`/`say`/`synthesize`/`translate` con idiomas distintos y el derivado CT2 del par sin provisionar (`model_missing`) |
| **5** | Daemon inalcanzable | `speech say --text x --daemon` con el daemon detenido |
| **6** | Conflicto de estado | `speech synthesize --text x --label ya_existe` sin `--force` (colisión de etiqueta) |
| **9** | Fallo del pipeline de traducción | `dub`/`say`/`synthesize`/`translate` con idiomas que difieren, el derivado CT2 **ya provisionado**, y la inferencia falla en tiempo de ejecución (`translation_failed`) |
| **10** | Fallo del pipeline de transcripción | `speech transcribe --audio corrupto.wav --source-language es-latam` (audio ilegible) |

```bash
# Ejemplos directos:
ai-voice-interconnector speech say --text "Prueba." --voice voz_que_no_existe ; echo "exit=$?"   # 3
ai-voice-interconnector voice remove --name voz_que_no_existe ; echo "exit=$?"                    # 3
ai-voice-interconnector speech play --label etiqueta_que_no_existe ; echo "exit=$?"               # 3
```

**Esperado**: cada sonda sale con el exit code de su fila y un mensaje en español a stderr; ninguna produce panic ni backtrace.
