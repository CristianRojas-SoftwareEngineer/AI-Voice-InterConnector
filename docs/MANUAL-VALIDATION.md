# Validación manual de la superficie de la CLI

Este documento es el **procedimiento operativo** de la validación end-to-end que
[docs/GOAL.md](GOAL.md) §"Validación E2E" define a nivel de política: el recorrido
manual `instalar → setup → síntesis real → desinstalar` que el propietario
ejecuta en Windows sobre cada release, y que sirve de guion para el feedback de
usuarios reales en Linux y macOS. Qué cubre el smoke test automatizado del
pipeline y por qué la matriz de comandos de abajo queda fuera de CI está en
[Validación E2E](GOAL.md#validación-e2e); la provisión de modelos, en
[`setup`](CLI/commands/SETUP.md); y los artefactos del release, en
[Formato de los artefactos](DISTRIBUTION.md#formato-de-los-artefactos). Las rutas
interactivas exigen además un micrófono y una terminal con una persona pulsando
teclas.

La secuencia está en orden lógico: cada paso asume que el anterior pasó. Se puede
partir de una instalación con modelos provisionados o de una instalación con
`--no-setup` o `setup_failed`, cuyos modelos se provisionan en el paso 3. Esta
guía valida el artefacto instalado de usuario final; el ciclo de limpieza,
aprovisionamiento y puerta de release desde el repositorio está en
[Aprovisionamiento de los recursos antes de cortar](RELEASING.md#aprovisionamiento-de-los-recursos-antes-de-cortar).
Ejecutar desde la sesión que instaló el programa o desde una terminal nueva:
la integración del `PATH` es persistente —bloque delimitado en los perfiles de
shell en Linux y macOS, valor `Path` de `HKCU\Environment` en Windows— y por eso
una terminal ya abierta no la ve. Solo la sesión que ejecutó el bootstrap
recibe el `PATH` en el acto; con `--no-modify-path` no se integra nada y hay que
invocar el binario por su ruta completa.

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

La prueba del bootstrap desde el repositorio no forma parte del recorrido del
artefacto instalado descrito aquí; sigue la secuencia de limpieza y
aprovisionamiento indicada en [RELEASING.md](RELEASING.md#aprovisionamiento-de-los-recursos-antes-de-cortar).

**Prerrequisitos por escenario** — los iconos al inicio de cada sección o ficha indican qué hace falta además del binario instalado:

| Icono | Requiere |
|---|---|
| 🧠 | Modelos provisionados (`setup` ejecutado) |
| 🔊 | Dispositivo de salida de audio (altavoz/auriculares) |
| 🎤 | Micrófono de entrada |
| 🖥️ | Terminal interactiva **real** (TTY) — no una tubería ni `< archivo` |
| 🗣️ | Un archivo de audio de referencia (muestra de voz o clip a transcribir/doblar) |

Las secciones **1-4** permiten detectar y resolver la falta de modelos; a partir
de **5** deben estar provisionados (🧠). Las rutas de **§8 (transcripción/doblaje)**
y **§9 (interactivas)** son las que CI no puede ejercitar y el núcleo de esta
validación.

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
  - [9.3 Progreso de la instalación interactiva](#93-progreso-de-la-instalación-interactiva)
- [10. Daemon](#10-daemon)
- [11. Casos de error por exit code canónico](#11-casos-de-error-por-exit-code-canónico)
- [12. Protocolo del indicador de progreso](#12-protocolo-del-indicador-de-progreso)

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

**Esperado**: `doctor --json` emite el informe por stdout; sin `--json`, los
chequeos fallidos y el veredicto van por stderr. Con los modelos provisionados
y los demás chequeos correctos, `doctor` termina con **exit 0**. Si faltan
modelos, termina con **exit 1** y `doctor --json` incluye `models_provisioned`
en `failed`, con los modelos ausentes en `models.missing`: es el diagnóstico
previo al paso 3, no un fallo de esta validación. Cualquier otro chequeo
fallido también produce **exit 1** y debe resolverse por separado; no
atribuirlo a la falta de modelos.

## 3. Provisión de recursos

🧠 (esta sección la genera) — ejecutar `setup` si faltan modelos tras
`--no-setup` o `setup_failed`; con los modelos ya provisionados, también permite
comprobar la idempotencia.

```bash
# Descarga los 6 modelos pinneados a la caché exclusiva de la aplicación (idempotente)
ai-voice-interconnector setup

# Confirmar que el entorno quedó sano antes de continuar
ai-voice-interconnector doctor --json
```

**Esperado**: `setup` descarga (o confirma en caché) los seis modelos y termina
con **exit 0**; una segunda ejecución no vuelve a descargar. El sexto,
`dpdfnet8`, es opcional para el arranque del daemon, pero se provisiona con el
resto y lo requieren las rutas de limpieza de ruido (`--denoise` y
`speech denoise`). `doctor` es integral y no admite excepciones por canal: en
`unmanaged`, la ausencia de receipt y la falta de resolución PATH producen exit
1. Para validar recursos del bootstrap usa `cargo xtask verify-resources --json`;
para validar `doctor` con exit 0, usa una instalación registrada y un PATH que la
resuelva.

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

🧠 · el clonado requiere 🗣️ (una muestra de habla ≥10 s); `--denoise` añade el requisito de `dpdfnet8`

```bash
# Listar voces disponibles (debe aparecer 'default' de fábrica)
ai-voice-interconnector voice list
ai-voice-interconnector voice list --json

# Registrar una voz de usuario con una sola muestra (--speech-reference, ≥10s,
# es el único audio de referencia)
ai-voice-interconnector voice clone --name mi_voz --speech-reference habla.wav

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
ai-voice-interconnector speech transcribe --audio clip.wav --source-language es

# En JSON ({"text","source_language"})
ai-voice-interconnector speech transcribe --audio clip.wav --source-language es --json

# Doblaje voz→voz desde archivo: transcribe → traduce → sintetiza → reproduce
# (--target-language por defecto es)
ai-voice-interconnector speech dub --audio clip_en.wav --source-language en --target-language es --voice mi_voz
```

**Esperado**: `transcribe` imprime el texto reconocido a **stdout** (o el objeto JSON con `--json`), **exit 0**. `dub` transcribe, traduce si los idiomas difieren, sintetiza con `--voice` y reproduce, **exit 0**. Un `--audio` inexistente sale con **exit 3**; un fallo del pipeline de transcripción, **exit 10**; de traducción, **exit 9**.

### Limpieza de ruido (`--denoise`)

Opt-in, disponible en `transcribe`, `dub` y `voice clone`. Exige el feature
`native-denoise` (incluido en `full`, que es el de los artefactos) y el modelo
`dpdfnet8` provisionado.

```bash
# Transcribir y doblar sobre el audio limpio
ai-voice-interconnector speech transcribe --audio clip.wav --source-language es --denoise
ai-voice-interconnector speech dub --audio clip_en.wav --source-language en --target-language es --voice mi_voz --denoise
```

**Esperado**: el mismo resultado y los mismos códigos que sin el flag, sobre la
señal limpia. Sin `dpdfnet8` provisionado la ruta falla con **exit 4**
(`model_missing`), igual en `voice clone --denoise`.

### Limpieza de archivos (`speech denoise`)

Es el subcomando de limpieza, local y sin daemon. Exige el mismo feature y el
mismo modelo, y usa el FFmpeg incluido en el paquete: no lo busca en `PATH`.

```bash
# Limpiar un archivo y dejarlo en un directorio propio
ai-voice-interconnector speech denoise clip.wav --output-dir limpio

# Ver el plan sin procesar nada
ai-voice-interconnector --json speech denoise clip.wav --dry-run
```

**Esperado**: escribe la versión limpia en el directorio indicado y sale con
**exit 0**; `--dry-run` muestra el plan sin escribir. Una entrada inexistente
sale con **exit 3**; una entrada sin audio o un destino ya existente sin
`--overwrite`, con **exit 2**; modelo ausente, FFmpeg ausente o error de E/S,
con **exit 1**.

## 9. Rutas interactivas de audio (push-to-talk y `--play`)

🖥️ 🎤 🔊 🧠 · **no ejercitables por CI**: exigen una terminal real y una persona
pulsando teclas. Son el motivo principal de esta validación manual.

Para cubrir `voice clone --denoise`, repite el clonado de §6 con una referencia
≥10 s y el flag `--denoise`; en la ruta `--mic`, el flag se admite junto con
`--duration`. **Esperado**: clona con la referencia limpia y termina con exit 0;
la falta del modelo `dpdfnet8` falla explícitamente con exit 4, sin degradar al
audio original.

El diseño y los principios están en [UX de audio interactivo](DESIGN.md#ux-de-audio-interactivo).

### 9.1 Push-to-talk (`speech transcribe --mic` / `speech dub --mic` sin `--duration`)

**Guarda sin TTY** (rápida, no necesita micrófono ni modelos cargados para
rechazar). Redirige stdin para que **no** sea terminal:

```bash
ai-voice-interconnector speech transcribe --mic --source-language es < /dev/null ; echo "exit=$?"
```
**Esperado**: **exit 2** (`usage_error`), aviso en **stderr** pidiendo `--duration N` al no haber terminal. **Sin panic ni backtrace** (regresión histórica ya corregida).

**Duración fija sigue intacta** (no interactiva, no push-to-talk):

```bash
ai-voice-interconnector speech transcribe --mic --duration 3 --source-language es ; echo "exit=$?"
```
**Esperado**: graba 3 s fijos y transcribe a **stdout**, **exit 0**; sin panic.

**Ruta feliz push-to-talk** — en una terminal interactiva real:

```bash
ai-voice-interconnector speech transcribe --mic --source-language es
```
**Esperado**:
1. En **stderr** aparece: `Grabando… pulsa Enter para detener.`
2. Hablas; la grabación continúa mientras hablas.
3. Pulsas **Enter** → se detiene y transcribe a **stdout** el texto reconocido, **exit 0**.

**Techo de seguridad** (Enter olvidado) — baja el tope con la variable de entorno y **no** pulses Enter:

```bash
AVI_PUSH_TO_TALK_MAX_SECS=5 ai-voice-interconnector speech transcribe --mic --source-language es
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

### 9.3 Progreso de la instalación interactiva

🖥️ · conexión para descargar los seis modelos · Windows PowerShell en una ventana nueva y sin redirigir la salida. Antes de empezar, la instalación anterior debe haberse desinstalado con **su propio binario** y la caché de modelos debe estar vacía. Si existe una instalación 0.31.x, `self update` no puede leer su recibo desde la 0.32.0: usa el binario 0.31.x para desinstalar y después el one-liner de la nueva versión.

Para validar el procedimiento de release desde cero en la máquina de desarrollo
(sin instalar el artefacto publicado), sigue el apartado
[Aprovisionamiento de los recursos antes de cortar](RELEASING.md#aprovisionamiento-de-los-recursos-antes-de-cortar).
Este escenario usa la secuencia de limpieza por capas, semilla de producto y
aprovisionamiento que detalla el apartado vinculado; no debe confundirse con
esta prueba de instalación del usuario final mediante el bootstrap publicado.

```powershell
# Si aún hay una instalación, desinstálala con su binario actual.
ai-voice-interconnector self uninstall --yes
# En una ventana NUEVA de Windows PowerShell, sin redirección:
irm https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/latest/download/install.ps1 | iex
$LASTEXITCODE
ai-voice-interconnector self uninstall --dry-run
ai-voice-interconnector cleanup --all --dry-run
```

**Criterio de aceptación:** el bootstrap muestra `Descargando <archivo>...`, tamaño y segundos con `checksum verificado.`, y separa su bloque con una línea en blanco. El plan muestra `5,9 GB` y el prompt `¿Continuar? [S/n] ` deja un espacio antes de la respuesta. La provisión abre otro bloque con `Descargando 6 modelo(s):`; cada barra está etiquetada `[i/6] nombre`, muestra el total real del modelo (no el de un archivo pequeño), velocidad, `transcurrido` y `quedan`. Al completarse cada modelo queda una línea permanente `✓ [i/6]` con tamaño, duración y velocidad. Sin TTY no hay barras, pero aparecen las líneas de avance `… nombre  <completado> de <total> (<transcurrido>)` cada 15 s. Cierra con `Modelos descargados: <tamaño> en <duración>.`, deja una línea en blanco y después aparece el resumen final, con `Comando:` terminado en `.exe`, el estado del PATH y los modelos con tamaño y tiempo. No debe haber líneas pegadas ni rellenadas con espacios al copiar la salida. `El comando ya está disponible en esta ventana.` aparece después del éxito. La instalación termina con **exit 0**. Las dos simulaciones muestran cada ruta una vez, su categoría en español, tamaños con coma y un único `Simulación: no se ha modificado nada.` al final del plan; no modifican el disco.

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
`ai-voice-interconnector` ni `qwen_tts` residuales, puerto `8765` y puerto del residente (el `resident_port` que anotaba `daemon.pid`) cerrados y
`daemon.pid` sin PID vivo. El procedimiento usa los defaults (puerto 8765, `data_dir()` global);
con `AVI_DAEMON_PORT=0` el puerto es efímero y con `AVI_DATA_DIR` el pidfile es por instancia
(aislamiento de tests, no de este recorrido).

**Kill duro del daemon con un clonado en curso** 🗣️ (una muestra de habla ≥10 s). El
motor `qwen_tts` termina solo cuando muere quien lo lanzó, porque la entrada estándar
que éste le mantiene abierta se cierra. Las pruebas automáticas cubren el kill duro del
daemon sin clonado; el clonado interrumpido se verifica a mano, porque una prueba
automática no distingue si el motor del clonado murió por el kill o terminó solo.

```bash
# Arrancar el daemon y anotar su PID (solo el del daemon, no el de su árbol)
ai-voice-interconnector daemon start --json

# En otra terminal: lanzar un clonado por la vía daemon, sin esperar a que termine
ai-voice-interconnector voice clone --name voz_kill --speech-reference habla.wav --daemon

# Sin esperar a que el clonado termine, matar solo el PID del daemon (sin su árbol)
kill -9 <PID_DEL_DAEMON>

# Pasados 10 s: no debe quedar ningún motor ni voz alguna con ese nombre
sleep 10
pgrep -x qwen_tts
ai-voice-interconnector voice list
```

En PowerShell los mismos pasos:

```powershell
ai-voice-interconnector daemon start --json
ai-voice-interconnector voice clone --name voz_kill --speech-reference habla.wav --daemon
Stop-Process -Id <PID_DEL_DAEMON> -Force   # sin /T: no se mata el árbol
Start-Sleep -Seconds 10
Get-Process qwen_tts -ErrorAction SilentlyContinue
ai-voice-interconnector voice list
```

**Esperado**: en 10 s no queda ningún proceso `qwen_tts` y `voice list` no muestra
`voz_kill`; tampoco existe un directorio `voz_kill` —ni vacío— dentro de `voices/` del
almacén de datos. Un clonado que no llegó a terminar no deja una voz a medias: la
referencia se prepara junto a `voices/` y se renombra de una vez. Si el motor tarda más
de 10 s en morir, o queda una voz vacía, es un fallo. Limpieza posterior: `daemon stop`
retira el `daemon.pid` y el `daemon.ready` que dejó el daemon muerto.

Un clonado interrumpido puede dejar en el scratch del daemon
`<caché>/tmp/<token>/` el directorio `avi_daemon_clone_<sufijo>` (con la
referencia y el `.qvoice` parcial) y el archivo `avi_tts_ref24k_<sufijo>.wav`.
El siguiente proceso que crea su scratch barre los directorios hermanos cuyo
dueño está muerto; `cleanup --all` también recoge el scratch huérfano. Los
scratch de dueño vivo o no observable se conservan. Si el proceso muere durante
el renombrado de la referencia, puede quedar junto a `voices/` un directorio
`.staging-<token>-<nombre>`: la recuperación lo clasifica por identidad
y lo borra solo si confirma que su dueño murió. En macOS, un motor podría tardar
en morir como mucho lo que dura un clonado si el daemon lanzó dos procesos a la
vez (riesgo teórico, sin verificar).

> Nota de cobertura: algunas variantes del reclamo de puerto/PID (cierre del
> puerto del motor residente tras la cosecha de la instancia de prueba)
> solo se alcanzan de forma fiable en Linux/CI o en entornos rápidos, no en la
> validación local de Windows; no las esperes en este recorrido.

## 11. Casos de error por exit code canónico

Una sonda por cada código de salida de dominio que el binario produce. El
significado de cada código, su `reason` y el canal por el que sale están en
[Códigos de salida](CLI/README.md#códigos-de-salida); aquí solo se lista cómo
provocarlo a mano.

| Exit | Sonda |
|---|---|
| **1** | `speech transcribe --audio <wav.tomlado en exclusiva por otro proceso>`: existe como fichero (`validate_audio_arg` pasa) pero leerlo falla por E/S |
| **2** | `speech synthesize --text x --label y --play --json` (flags incompatibles); `speech transcribe --mic --source-language es < /dev/null` (mic sin duración ni TTY); `voice remove --name default` (voz de fábrica); `speech play --voice default --daemon` (`--daemon` en un comando que no lo declara); `speech transcribe --audio a.wav --duration 5 --source-language es` (`--duration` sin `--mic`) |
| **3** | `speech play --label etiqueta_que_no_existe`; `voice remove --name voz_que_no_existe`; `speech transcribe --audio no_existe.wav --source-language es` |
| **4** | cualquier síntesis/transcripción **sin** haber corrido `setup`; también `dub`/`say`/`synthesize`/`translate` con idiomas distintos y el modelo de traducción del par sin provisionar |
| **5** | `speech say --text x --daemon` con el daemon detenido |
| **6** | `speech synthesize --text x --label ya_existe` sin `--force` (colisión de etiqueta); `voice clone --name mi_voz --speech-reference habla.wav` dos veces sin `--force` |
| **9** | `dub`/`say`/`synthesize`/`translate` con idiomas que difieren, el modelo de traducción **ya provisionado**, y la inferencia falla en tiempo de ejecución |
| **10** | difícil de sondear a mano: un `--audio` inexistente sale con 3, ilegible con 2 y con error de E/S con 1. Se alcanza con un modelo provisionado y corrupto, o forzando un fallo de la captura |
| **11** | `setup` o `self install` sin red (o con la descarga de modelos bloqueada): instala igual y sale 11 |
| **12** | `self update` sobre una copia del canal `homebrew`: no toca nada y remite a `brew upgrade --cask` |
| **13** | `self update` en el que la copia nueva no puede reemplazar la anterior (permisos o proceso reteniendo el ejecutable): restaura y deja la versión previa en pie |
| **14** | `self install` o `self uninstall` con el enlace del `PATH` ya tomado por otro programa |
| **15** | `self install`/`self update` cuyo archivo comprimido no trae el ejecutable esperado |
| **16** | `cleanup`, `self install`, `self uninstall` o `self update` con un daemon registrado **vivo** que no se puede parar (mismo camino que el `still_running` de `daemon stop`) |
| **17** | dos comandos de ciclo de vida simultáneos sobre la misma raíz (p. ej. un `self install` lento y un `cleanup`), o un comando de uso en curso: el segundo no entra en cola |
| **18** | `self update` con un triple del host fuera de la tabla soportada (falla antes de tocar la red); los instaladores emiten el mismo `reason` en su mensaje, pero **ellos salen con 1**, no con 18 |
| **19** | `self update` cuyo artefacto descargado no arranca en este sistema o no responde a `--version` |
| **20** | `self update` sin red al resolver la última versión o al descargar el artefacto |
| **21** | `self update` contra un `SHA256SUMS.txt` manipulado (o el artefacto alterado en tránsito); el staging se borra sin tocar la instalación |
| **22** | `self uninstall` con el directorio de programa tomado (un proceso vivo lo tiene abierto o faltan permisos): borra el estado de usuario y deja el directorio |
| **130** | **Ctrl+C** durante `daemon start`/`restart`/`serve` o durante una síntesis: sale 130 con el estado preservado y el árbol del daemon reclamado |

```bash
# Ejemplos directos:
ai-voice-interconnector speech say --text "Prueba." --voice voz_que_no_existe ; echo "exit=$?"   # 3
ai-voice-interconnector voice remove --name voz_que_no_existe ; echo "exit=$?"                    # 3
ai-voice-interconnector speech play --label etiqueta_que_no_existe ; echo "exit=$?"               # 3
```

**Esperado**: cada sonda sale con el exit code de su fila y un mensaje en español a stderr; ninguna produce panic ni backtrace.

## 12. Protocolo del indicador de progreso

El indicador escribe por stderr y es el **único dueño de su línea**: cuando el producto necesita escribir, cede la línea y la vuelve a tomar. Estas comprobaciones exigen una **terminal real** (no vale redirigir stderr a un fichero: sin terminal no hay línea que ceder ni línea que se pegue).

### 12.1 La fase no queda pegada a la línea siguiente

Con un comando que capture el micrófono, en una terminal de verdad:

```bash
ai-voice-interconnector speech transcribe --mic --source-language es
```

**Esperado**: el aviso «Grabando… pulsa Enter para detener.» aparece en su
**propia línea**, y la fase «… Transcribiendo audio…» no queda pegada a
ninguna otra. Lo mismo con el menú de `speech synthesize --play`: las
opciones se leen en su propia línea, y la fase no las arrastra.

### 12.2 El mensaje final aparece una sola vez

```bash
ai-voice-interconnector speech say --text "Prueba." --voice default --no-daemon
```

**Esperado**: el resultado «Reproduciendo (N s).» aparece **una vez**, por
stdout, y **no** hay ninguna línea `[ok]` ni `[error]` por stderr: el
indicador no dice cómo terminó, lo dice el resultado. El estado se dice una
sola vez.

### 12.3 La cancelación se dice una sola vez

```bash
ai-voice-interconnector voice remove --name una_voz_que_exista
```

y responde `n` a la confirmación.

**Esperado**: «Cancelado.» aparece **una sola vez**, por stderr. No hay un
segundo «Cancelado.» por stdout.

### 12.4 Una fase, una línea en no-TTY

```bash
ai-voice-interconnector self update 2> salida.txt
```

**Esperado**: el fichero `salida.txt` no acumula líneas de la fase repetidas:
la fase se anuncia una vez y la actividad posterior no abre líneas nuevas.
Vale cualquier comando con una espera larga, porque el racionado es por fase:
también `translate --daemon` o `speech transcribe --daemon` con la salida
redirigida.

### 12.5 El tiempo visible avanza durante una espera real

```bash
ai-voice-interconnector self update
```

**Esperado**: durante la descarga, el tiempo de la fase **avanza** —los
segundos suben en la terminal— y no se queda congelado en `(0 s)`. Es la
comprobación que distingue una espera viva de una congelada.

**Excepción declarada**: durante la síntesis, el clonado local y la
captura del micrófono, el trabajo bloquea el hilo y la fase **no** puede
repintar: el tiempo se queda congelado hasta que el trabajo acaba, y el
tiempo total lo da el mensaje final. Eso no es un defecto.
