# Subsistema de voz de entrada: cierre del bucle voz→voz cross-lingual

## 1. Introducción

Esta propuesta define un **subsistema de entrada de voz** para TTS-Sidecar: la
transcripción de **audio→texto** (STT, *speech-to-text*) local, más la **captura de
micrófono multiplataforma** que la alimenta. Es el eslabón de **entrada** que falta
para cerrar el bucle completo **voz→voz cross-lingual**: que el usuario **hable en su
idioma nativo** (p. ej. español latino) y obtenga **audio en otro idioma con su
propia voz clonada**, sin teclear ni una palabra.

**Motivación.** El bucle está hoy cerrado salvo por su primer eslabón. La **síntesis
cross-lingual** existe (v0.7.0–v0.9.0: el modelo inglés reutiliza el timbre clonado)
y la **traducción de texto** `es↔en` existe (subsistema `translation/`, `opus-mt`
sobre CTranslate2, absorbido en v0.9.x). La cadena **texto→texto→voz** ya funciona en
un solo comando: `speech say --source-language es --target-language en -v mi_voz`.
Pero **la entrada sigue siendo texto**: para "hablar" hay que **escribir**. Este
subsistema añade el eslabón **audio→texto** delante de esa cadena, de modo que la
entrada pueda ser la **voz real** del usuario:

```text
[NUEVO] Voz (audio es)  →  transcribir  →  texto es
   →  traducir (opus-mt es→en, YA EXISTE)  →  texto en
   →  sintetizar (Chatterbox, voz clonada, YA EXISTE)  →  Voz (audio en)
```

**Alcance.** El subsistema cubre: (a) un **runtime STT local** (faster-whisper sobre
el CTranslate2 **ya embarcado**); (b) la **captura de micrófono** multiplataforma
(miniaudio) con fin de grabación por **push-to-talk** y tope opcional `--duration`;
(c) un **comando `speech transcribe`** (audio→texto) verificable de forma aislada;
(d) la **composición** de la voz como entrada de síntesis mediante `--audio`/`--mic`
en `speech say|synthesize`, encadenando transcribir → traducir → sintetizar; (e) la
**provisión del modelo Whisper** por el canal de `setup`/`doctor`/`cleanup` (patrón
ya usado por Chatterbox y `opus-mt`); y (f) los **cambios de contrato** que implica.
Queda **fuera de alcance**: la traducción a idiomas distintos de `es`/`en` (la hereda
el subsistema de traducción, sin cambios), la diarización o separación de hablantes,
la detección de actividad de voz (VAD) como criterio de fin de grabación, y el
*streaming* de transcripción en tiempo real (parcial mientras se habla) — la
transcripción opera sobre una toma ya cerrada.

**Restricciones heredadas del proyecto.** Todo el subsistema respeta las
restricciones del [Goal](../GOAL.md): **100% local** (sin APIs externas), **motor y
dependencias con licencia compatible con GPL-3.0-or-later**, **multiplataforma**
(Windows/Linux/macOS), **consumible por CLI**, y **empaquetable con PyInstaller** sin
fricción nueva más allá de la ya resuelta.

**Estructura del documento.** La [sección 2](#2-estado-actual-as-is) especifica el
estado actual (as-is); la [sección 3](#3-estado-objetivo-to-be) define el estado
objetivo (to-be); la [sección 4](#4-proceso-de-implementación) describe el proceso de
implementación por fases; la [sección 5](#5-clasificación-de-la-spec) clasifica la
spec según el criterio del Goal.

## Tabla de contenidos

- [1. Introducción](#1-introducción)
- [2. Estado actual (as-is)](#2-estado-actual-as-is)
  - [2.1 El bucle está cerrado salvo la entrada de voz](#21-el-bucle-está-cerrado-salvo-la-entrada-de-voz)
  - [2.2 El audio es hoy solo de salida](#22-el-audio-es-hoy-solo-de-salida)
  - [2.3 La entrada no es simétrica con la salida](#23-la-entrada-no-es-simétrica-con-la-salida)
  - [2.4 Contrato congelado](#24-contrato-congelado)
  - [2.5 Provisión y daemon](#25-provisión-y-daemon)
- [3. Estado objetivo (to-be)](#3-estado-objetivo-to-be)
  - [3.1 El pipeline de entrada por capas](#31-el-pipeline-de-entrada-por-capas)
  - [3.2 Runtime STT, motor y licencia](#32-runtime-stt-motor-y-licencia)
  - [3.3 Whisper transcribe, no traduce](#33-whisper-transcribe-no-traduce)
  - [3.4 Captura de micrófono con miniaudio](#34-captura-de-micrófono-con-miniaudio)
  - [3.5 Fin de grabación: push-to-talk y `--duration`](#35-fin-de-grabación-push-to-talk-y---duration)
  - [3.6 Ubicación en la arquitectura](#36-ubicación-en-la-arquitectura)
  - [3.7 Comando `speech transcribe` (audio→texto)](#37-comando-speech-transcribe-audiotexto)
  - [3.8 Composición: la voz como entrada de síntesis](#38-composición-la-voz-como-entrada-de-síntesis)
  - [3.9 Provisión del modelo Whisper](#39-provisión-del-modelo-whisper)
  - [3.10 Daemon: transcripción en caliente, captura en cliente](#310-daemon-transcripción-en-caliente-captura-en-cliente)
  - [3.11 Cambios de contrato](#311-cambios-de-contrato)
  - [3.12 Invariantes (lo que NO cambia)](#312-invariantes-lo-que-no-cambia)
- [4. Proceso de implementación](#4-proceso-de-implementación)
- [5. Clasificación de la spec](#5-clasificación-de-la-spec)

---

## 2. Estado actual (as-is)

### 2.1 El bucle está cerrado salvo la entrada de voz

Las dos etapas finales del bucle voz→voz ya viven en el repo:

- **Síntesis cross-lingual del audio.** `ChatterboxEngine` (`engine.py`) es
  intrínsecamente multilingüe: `es-mx-latam` enruta al multilingüe con
  `language_id="es"` y `en` al inglés base, ramificado en `synthesis.py:130`. El
  timbre lo aporta S3Gen, **agnóstico del idioma**: la misma voz clonada sirve a
  ambas rutas sin re-clonar.
- **Traducción de texto `es↔en`.** El subpaquete `translation/` orquesta
  validación → segmentación → traducción (`opus-mt` sobre CT2) → ensamblado. El
  comando `translate` (`cli.py:885`) lo expone aislado, y `_translate_stage`
  (`cli.py:191`) lo inserta antes de la síntesis cuando `--source-language !=
  --target-language` en `speech say|synthesize`.

**El hueco.** La única forma de *iniciar* el bucle es **texto tecleado**: `--text` es
requerido en `speech say` (`cli.py:2249`) y `speech synthesize` (`cli.py:2208`). No
existe ninguna vía para partir de **audio hablado**. La promesa de "hablar otro
idioma con tu voz" queda a medias: el sistema ya presta la voz y el idioma, pero
exige que el usuario **escriba** lo que quiere decir. Este es el hueco que la
propuesta cierra.

### 2.2 El audio es hoy solo de salida

`audio.py` es una capa **exclusivamente de salida**. `AudioPlayer` reproduce con el
backend nativo de cada SO —**winsound** (built-in) en Windows, **afplay** (nativo) en
macOS, **sounddevice/PortAudio** en Linux— y normaliza PCM int16→float32 con
`INT16_MAX_F = 32768.0`. `get_audio_devices_with_status` enumera **solo dispositivos
de reproducción** (filtra `eRender` vía pycaw en Windows, `max_output_channels > 0` en
sounddevice) y degrada a "Default" en entornos sin audio (RDP, headless, CI).

**No hay ninguna ruta de entrada.** Ni captura, ni enumeración de dispositivos de
grabación, ni lectura de micrófono. Las coincidencias de `sounddevice` en el repo son
todas de *playback*, no de captura.

### 2.3 La entrada no es simétrica con la salida

La salida se apoya en que **grabar y reproducir no son simétricos** a nivel de
plataforma. La reproducción tiene un built-in por SO (winsound/afplay); **la captura
no tiene equivalente built-in** en ninguna plataforma. Esto tiene tres consecuencias
que el diseño debe absorber, y que la salida nunca enfrentó:

- **Dependencia nativa de audio real.** Capturar exige una librería que hable con la
  API de audio del SO (WASAPI/CoreAudio/ALSA-PulseAudio). No hay atajo built-in
  equivalente a `winsound`.
- **Permisos del SO.** El micrófono está tras un *gate* de privacidad: TCC en macOS
  (diálogo de consentimiento), *toggle* de privacidad de micrófono en Windows. La
  reproducción no pide permiso; la captura sí.
- **Criterio de fin de grabación.** Reproducir termina cuando el archivo se acaba;
  **grabar no tiene final natural** — hay que decidir cuándo parar (duración fija,
  push-to-talk, VAD).

### 2.4 Contrato congelado

- **Exit codes**: centralizados en `exit_codes.py` (contrato público congelado). El
  último asignado es `EXIT_TRANSLATION_FAILED = 9`; el siguiente libre es `10`.
- **Esquema IPC**: `schema_version = "3"` (`protocol.py:53`), con `extra="ignore"`
  para compatibilidad aditiva. `SynthesizeRequest` (`protocol.py:56`) lleva `text`
  (≤ `MAX_TEXT_LENGTH`=5000), `voice`, `target_language`, `source_language`
  (`protocol.py:77`, aditivo) y los overrides. **No** existe ninguna operación que
  reciba audio.
- **`--json`**: payloads legibles por máquina con clave `error`.

El contrato lo consume el repo hermano **tts-sidecar-narrator** (plugin de Claude
Code). Cualquier cambio incompatible obliga a actualizarlo en lockstep (ver
[3.11](#311-cambios-de-contrato)).

### 2.5 Provisión y daemon

- **`setup --language {es-latam, en, all}`** (`cli.py:1669`) descarga los modelos a
  la caché de HuggingFace; con `en`/`all`, `_provision_translation_pairs`
  (`cli.py:1788`) descarga **y convierte a CT2** el par `opus-mt`. Ningún modelo vive
  en el bundle: patrón que este subsistema reutiliza.
- **`doctor`** (`cli.py:1086`) reporta la presencia de los modelos, incluido el de
  traducción (`cli.py:1111`); **`cleanup`** (`cli.py:1924`) los incluye en el barrido
  de caché (`cli.py:1991`).
- **`daemon start --language {…}`** precarga los modelos en RAM;
  `HealthResponse.model_loaded` (`protocol.py:132`) reporta qué está caliente.

---

## 3. Estado objetivo (to-be)

### 3.1 El pipeline de entrada por capas

El subsistema es una cadena de responsabilidades separadas, CPU-first y 100% local,
que produce **texto** para entregarlo al pipeline ya existente:

```text
Micrófono (o archivo WAV)
   ↓  Captura              (miniaudio: 16 kHz, mono, PCM int16 — o lectura de WAV)
   ↓  Fin de grabación     (push-to-talk / --duration)
   ↓  Transcripción        (faster-whisper sobre CT2: audio → texto en el idioma hablado)
Texto en el idioma de origen  →  (Traducción + Síntesis: subsistemas ya existentes)
```

Las dos capas nuevas son **ortogonales**, y esa separación es deliberada porque sus
costes son de naturaleza distinta:

- **Transcripción** = cómputo puro (audio decodificado → texto). Multiplataforma sin
  matices, cacheable en el daemon, testeable con un WAV fijo sin hardware.
- **Captura** = I/O de hardware. Depende de la API de audio del SO, de permisos y de
  un criterio de fin de grabación; **inherentemente del lado del cliente** (como la
  reproducción de `audio.py`), no del daemon.

Mantenerlas separadas permite transcribir un archivo sin tocar hardware (la ruta
`--audio`) y capturar sin acoplar el fin de grabación al transcriptor.

### 3.2 Runtime STT, motor y licencia

**Motor: Whisper (OpenAI), variante multilingüe.** Whisper es el estándar de facto de
STT abierto, con cobertura sólida de **español** y del **español latino**. Los pesos
son **MIT**.

**Requerimiento no funcional: transcribir lo más rápido posible en CPU-only.** La
transcripción no debe percibirse como una espera larga antes de la voz. Ese NFR
gobierna la elección del *runtime*, decisión distinta de la del *motor*.

**Runtime: faster-whisper (Whisper sobre CTranslate2).** CT2 es el runtime canónico
para Whisper rápido en CPU (int8, mejor threading; se reportan ~4× de velocidad y
menos memoria que la implementación de referencia). Se adopta **directamente** —no
tras un gate de medición— porque **el runtime ya está en el repo**: el subsistema de
traducción embarcó CTranslate2 y resolvió su empaquetado nativo con PyInstaller. STT
reutiliza **ese mismo runtime**, sin introducir un motor de inferencia nuevo. Sus
propiedades encajan con las restricciones del proyecto:

- **Licencia MIT** (faster-whisper y CT2) y **pesos MIT** (Whisper) — todo compatible
  con GPL-3.0-or-later, **sin siquiera la cláusula de atribución** que `opus-mt`
  (CC-BY) sí exige.
- **Modelos ya en formato CT2.** A diferencia de `opus-mt` (que hay que convertir con
  `ct2-transformers-converter` en `setup`), los modelos `Systran/faster-whisper-*` de
  HuggingFace **ya vienen convertidos a CT2**: `setup` solo los **descarga**, sin paso
  de conversión.
- **Costo de empaquetado, acotado y honesto.** El *runtime* de inferencia (CT2) **no
  es nuevo**. Pero el paquete `faster-whisper` arrastra dos nativos que hoy no están:
  **PyAV** (bindings de ffmpeg, para decodificar audio de archivo) y **onnxruntime**
  (para su VAD integrado). Ambos son empaquetables con PyInstaller (el mismo tipo de
  recolección nativa que `torch`/`ctranslate2` ya resolvieron), pero son coste real.
  Se **mitigan por diseño**: (i) alimentando a faster-whisper un `numpy.float32` ya
  decodificado —desde miniaudio en captura, desde el módulo `wave` de la stdlib en
  archivo— para **no ejercer la ruta de PyAV**; (ii) **sin usar el VAD** integrado
  (el fin de grabación es push-to-talk/`--duration`, [3.5](#35-fin-de-grabación-push-to-talk-y---duration)),
  evitando ejercer onnxruntime. Siguen siendo dependencias de instalación, pero no
  entran en la ruta caliente. La Fase 0 valida el bundle real por SO.

**Backend inyectable — por testabilidad, no por indecisión.** El colaborador que
carga y ejecuta Whisper mantiene el runtime como dependencia inyectable, espejando
`TranslationModelLoader` (`model_loader.py`): en producción, `faster_whisper.WhisperModel`
(CT2 embarcado); en tests, un doble que devuelve texto fijo sin tocar el runtime
nativo. No hay dos runtimes en producción: hay **uno** (CT2) y una salida de pruebas.

**Descartado: whisper.cpp.** Es un motor de inferencia **distinto** (C++ con su
propio formato `ggml`, su propio empaquetado y su propio binding). Teniendo **ya CT2
embarcado y validado**, adoptar un segundo runtime nativo solo para STT duplicaría
superficie de empaquetado sin beneficio (Simplicity First). faster-whisper da la misma
familia de modelos sobre el runtime que el proyecto **ya mantiene**.

**Tamaño del modelo: `small` multilingüe fijo, sin flag por invocación.** Se fija
**`small` multilingüe** como único tamaño, equilibrio entre calidad en español y
latencia CPU. No se expone un knob de tamaño por invocación: el único eje de modelo del
sistema es el idioma, y no se prolifera otro. La comparación `small` vs `large-v3` es
**calibración empírica de la Fase 1** (medible con un corpus de audio fijo), no una
decisión de usuario ni algo que reabra el diseño. Los **distil-whisper** quedan fuera:
son **solo-inglés**, y aquí el idioma hablado de entrada es típicamente español.

### 3.3 Whisper transcribe, no traduce

Whisper puede, por sí mismo, **traducir** audio a inglés (`task="translate"`). Esta
propuesta **no lo usa**: Whisper se emplea **solo para transcribir**
(`task="transcribe"`, texto en el mismo idioma hablado). Tres razones:

1. **Direccionalidad.** El `translate` nativo de Whisper solo va **X→inglés**; no
   produce `en→es`. El bucle del proyecto es **bidireccional** (`es↔en`), y eso ya lo
   cubre `opus-mt`.
2. **Ortogonalidad de subsistemas.** Transcribir con Whisper y traducir con `opus-mt`
   mantiene STT y MT como piezas independientes y componibles; fundir ambos en Whisper
   acoplaría la calidad de traducción al modelo de STT.
3. **Reutilización.** La etapa de traducción, con su segmentación (`pysbd`) y su
   ensamblado orientados a **naturalidad oral**, ya existe y está probada. Sustituirla
   por el `translate` de Whisper sería tirar trabajo cerrado.

Whisper produce texto en el idioma de origen; **`opus-mt` (ya existente) lo traduce**
si el destino difiere. Cada subsistema hace una sola cosa.

### 3.4 Captura de micrófono con miniaudio

**Librería: miniaudio (pyminiaudio, CFFI, MIT).** Es la única de las candidatas que
cubre **limpiamente la matriz de release real del proyecto**. Los scripts de build
publican cuatro targets: **Windows x86_64**, **Linux x86_64**, **Linux arm64
genérico** (AppImage aarch64) y **macOS arm64**. miniaudio es **C de un solo archivo
compilado vía CFFI**, con backends nativos por SO (WASAPI/CoreAudio/ALSA-PulseAudio):
compila para **cualquier arquitectura** y no depende de ninguna librería nativa
externa preinstalada.

Se configura para entregar audio **listo para Whisper**, sin resampling posterior:

```python
# CaptureDevice de miniaudio → PCM crudo directo al formato que Whisper espera
CaptureDevice(sample_rate=16000, input_format=SampleFormat.SIGNED16, nchannels=1)
# int16 → float32 normalizado con el mismo INT16_MAX_F = 32768.0 de audio.py
```

**Descartadas:**

- **PvRecorder (Picovoice, Apache-2.0).** Entrega 16 kHz/16-bit sin resampling —su
  única ventaja frente a miniaudio, que ya se configura igual— pero **no cubre el
  target `linux-arm64` genérico**: solo soporta x86_64 y Raspberry Pi (identifica el
  SoC por CPU *part ID* y lanza `NotImplementedError` en aarch64 de escritorio no
  reconocido). El AppImage aarch64 del proyecto se quedaría sin captura. Es un hueco
  descalificador, y el ahorro de resampling no lo compensa porque miniaudio ya evita
  el resampling configurando la tasa de captura.
- **sounddevice/PortAudio.** Ya es la dependencia de *playback* en Linux, pero
  elevarla a la captura la convertiría en **dependencia nativa externa en las tres
  plataformas** (hoy Windows y macOS usan built-ins). miniaudio evita esa elevación:
  su C viaja en el propio wheel/bundle.
- **PyAudio** (peor DX, PortAudio externo) y **ffmpeg/sox** (binario externo pesado,
  no una librería) — descartados de entrada.

`audio.py` gana su contraparte de entrada: la enumeración de dispositivos de captura
(filtrando `eCapture`/`max_input_channels > 0`, simétrica a la de salida) y la misma
degradación a "Default" en entornos headless. La captura, como el playback, es
**siempre del lado del cliente**.

### 3.5 Fin de grabación: push-to-talk y `--duration`

La grabación no tiene final natural ([2.3](#23-la-entrada-no-es-simétrica-con-la-salida)),
así que el criterio de parada es explícito, con **dos modos**:

- **Push-to-talk (por defecto).** Se graba hasta que el usuario **pulsa Enter** para
  parar, vía `sys.stdin.isatty()` (exigir TTY) + `input()` (esperar la línea) —el
  mismo precedente de interactividad del resto del CLI, sin captura de teclado cruda
  por SO (`msvcrt`/`termios`). Es el modo interactivo natural, pero **requiere un
  terminal controlador (TTY)**: leer de stdin no tiene sentido en un pipe, en CI o
  dentro del daemon.
- **`--duration N` (tope fijo).** Graba **N segundos** y para sola. Es el modo
  **determinista y no interactivo**: funciona sin TTY, apto para automatización y para
  invocadores programáticos.

Cuando no hay TTY y no se pasa `--duration`, el comando **falla con un error de uso
explícito** (`EXIT_INVALID_INPUT`) remitiendo a `--duration`, en vez de bloquearse
esperando una tecla que nadie pulsará.

**Descartado: VAD (detección de actividad de voz).** Parar "cuando el usuario deja de
hablar" suena cómodo, pero añade una dependencia y una heurística (umbral de silencio,
falsos cortes en pausas) impropia de una primera iteración. push-to-talk +
`--duration` cubre los dos casos reales —interactivo y automatizado— sin dependencias
extra (Simplicity First). VAD sería una mejora posterior, no un requisito.

### 3.6 Ubicación en la arquitectura

El subsistema vive en un subpaquete nuevo `src/tts_sidecar/transcription/`, espejando
`translation/`, con **colaboradores inyectables**:

```text
src/tts_sidecar/transcription/
├── __init__.py          # Exportaciones públicas del paquete
├── service.py           # TranscriptionService: orquesta captura/lectura → transcripción
├── model_loader.py      # WhisperModelLoader: carga/caché del modelo CT2 (inyectable; espeja TranslationModelLoader)
└── transcriber.py       # WhisperTranscriber: audio (numpy float32) → texto (runtime inyectable)
```

La **captura** vive junto a la capa de audio existente, no dentro de `transcription/`,
porque es I/O de cliente simétrica al playback: `audio.py` gana un `AudioRecorder`
(miniaudio) contraparte de `AudioPlayer`. Así `transcription/` permanece como
**cómputo puro** (audio decodificado → texto), cacheable en el daemon, y la captura
queda donde vive su gemela de salida.

Las excepciones del subsistema se añaden a `exceptions.py` (compartido, sin imports
pesados): `TranscriptionModelMissingError` (remite a `setup`) y
`TranscriptionFailedError`. La resolución de idiomas reutiliza `resolve_language` de
`translation/` (`es-latam → es`): Whisper opera sobre el idioma hablado en taxonomía
ISO.

### 3.7 Comando `speech transcribe` (audio→texto)

Subsistema autónomo, verificable sin traducir ni sintetizar:

```bash
# Desde archivo
tts-sidecar speech transcribe --audio nota.wav --source-language es
# → "Hola, buenos días"

# Desde micrófono (push-to-talk: para con Enter)
tts-sidecar speech transcribe --mic --source-language es --json
# → {"text": "Hola, buenos días", "source": "es"}

# Desde micrófono con tope fijo (no interactivo)
tts-sidecar speech transcribe --mic --duration 5 --source-language es
```

- **Fuente de audio (una, requerida, mutuamente excluyentes):** `--audio PATH`
  (archivo WAV) o `--mic` (captura en vivo). Que sean explícitas y excluyentes evita
  la ambigüedad de un `--audio` con valor opcional.
- **`--source-language {es-latam, en}` (requerido).** Es el idioma **hablado**. Se
  exige explícito por la misma razón que `translate` exige `--from`/`--to`: la
  transcripción es la única función del comando; no hay acción por defecto. (Whisper
  sabe autodetectar idioma, pero la autodetección queda fuera de alcance: declararlo
  es más fiable y encadena limpio con la traducción aguas abajo.)
- **`--duration N`** solo aplica a `--mic` (tope de segundos; ver
  [3.5](#35-fin-de-grabación-push-to-talk-y---duration)).
- **`--json`** emite `{text, source}`.
- **Errores con identidad propia:** modelo Whisper ausente (`model_missing`, remite a
  `setup`), fuente inválida o falta de TTY sin `--duration` (`EXIT_INVALID_INPUT`), y
  **fallo de transcripción** (`EXIT_TRANSCRIPTION_FAILED`, ver
  [3.11](#311-cambios-de-contrato)).

### 3.8 Composición: la voz como entrada de síntesis

`speech say|synthesize` ganan la voz como **fuente de entrada alternativa al texto**.
Hoy `--text` es la única entrada y es requerida; pasa a ser **una de tres fuentes
mutuamente excluyentes**, exactamente una requerida:

| Fuente | Entrada | Comportamiento |
| --- | --- | --- |
| `--text "…"` | texto tecleado (actual) | idéntico a hoy |
| `--audio PATH` | archivo WAV hablado | se **transcribe** y sigue como si fuera `--text` |
| `--mic` | micrófono en vivo | se **captura y transcribe**, luego sigue como `--text` |

Cuando la entrada es voz, el texto transcrito **entra en la cadena ya existente**: si
`--source-language != --target-language`, se traduce; luego se sintetiza con la voz
clonada. Un solo comando cierra el bucle **voz→voz**:

```bash
tts-sidecar speech say \
  --mic --source-language es --target-language en -v mi_voz
# Hablas en español → transcribe → traduce ES→EN → sintetiza en inglés con TU voz
```

`--source-language` conserva su significado (idioma de la **entrada**), solo que ahora
la entrada puede ser audio en vez de texto. El eje `--target-language` no cambia. El
flujo interno inserta la transcripción **antes** de `_translate_stage` (`cli.py:191`):

```text
CLI (cmd_speech_*) → [NUEVO] TranscriptionService (si --audio/--mic)
  → _translate_stage (si source != target, YA EXISTE)
  → _dispatch_synthesis → SynthesisOrchestrator.synthesize (YA EXISTE)
```

Ni la traducción ni el motor de síntesis cambian: solo cambia **quién** produjo el
texto de entrada (antes el usuario tecleando, ahora la transcripción de su voz).

### 3.9 Provisión del modelo Whisper

El modelo Whisper **no se empaqueta en el bundle** (igual que Chatterbox y `opus-mt`):
se descarga a la caché de HuggingFace en `setup`. A diferencia de `opus-mt`, **no
requiere conversión**: los pesos `Systran/faster-whisper-{size}` ya están en formato
CT2 ([3.2](#32-runtime-stt-motor-y-licencia)), así que `_provision_whisper_model`
(nuevo, espejo de `_provision_translation_pairs`, `cli.py:1788`) solo hace
`snapshot_download`. `doctor` reporta su presencia; `cleanup` lo incluye en el barrido
de caché. `miniaudio` (nativo compilado en el wheel/bundle, sin modelos descargables)
**sí** viaja en el bundle.

**Ligadura de la provisión: flag opt-in `--with-stt`, fuera del default.** El STT es
**ortogonal al idioma**: es útil con cualquier idioma de entrada, no solo en el bucle
inglés. A diferencia de la traducción —que `--language en`/`all` arrastra como
**dependencia dura** vía `_provision_translation_pairs` (`cli.py:1788`), porque el
bucle cross-lingual la exige— ningún idioma requiere el STT. Por eso su provisión se
ata a un **flag opt-in propio, `setup --with-stt`, fuera del default**: combinable con
`--language` (ejes ortogonales, no un valor más de su taxonomía), para no imponer la
descarga del modelo Whisper a la mayoría que solo hace TTS. La promesa de "offline
desde el primer uso" aplica **dentro** de la capacidad elegida; la *discoverability* se
cubre reportando el modelo en `doctor`.

### 3.10 Daemon: transcripción en caliente, captura en cliente

La asimetría [3.1](#31-el-pipeline-de-entrada-por-capas) determina el reparto:

- **Transcripción (cómputo) → daemon.** El daemon cachea el modelo Whisper en RAM
  igual que los modelos TTS y de traducción, para no pagar la carga en cada petición.
  `HealthResponse.model_loaded` (`protocol.py:132`) añade la señal del modelo Whisper
  con una **clave propia** (p. ej. `transcribe:small`), sin colisionar con las claves
  de idioma TTS ni con las de par de traducción del mismo dict. Carga **perezosa** si
  no se precargó.
- **Captura (I/O de hardware) → siempre cliente.** El micrófono, los permisos del SO
  y el push-to-talk viven **donde está el usuario**, nunca en el proceso daemon (que
  puede correr headless). El cliente captura, y sobre el resultado decide: transcribir
  local o —si el daemon está activo— enviarle las **muestras ya decodificadas** para
  aprovechar el modelo caliente. Es exactamente el reparto de `audio.py`, cuya
  reproducción también es siempre de cliente.

### 3.11 Cambios de contrato

1. **Subcomando nuevo `speech transcribe`** — superficie aditiva; su contrato
   `--json` (`{text, source}`) se documenta en `CLI-CONTRACT.md`.
2. **Fuentes de entrada `--audio`/`--mic` en `speech say|synthesize`** — aditivo pero
   **con un matiz de contrato**: `--text` deja de ser incondicionalmente requerido y
   pasa a ser "exactamente una de `{--text, --audio, --mic}`". Un invocador existente
   que siempre pasa `--text` **no se ve afectado**; la regla nueva solo abre
   alternativas. Se documenta en `CLI-CONTRACT.md`.
3. **Exit code nuevo `EXIT_TRANSCRIPTION_FAILED = 10`** en `exit_codes.py` — aditivo;
   distingue el fallo de transcripción del de traducción (`9`) y del de síntesis
   (errores distintos ⇒ identidad propia). El modelo Whisper ausente reutiliza
   `model_missing` (código `4`) remitiendo a `setup`.
4. **`TranscribeRequest` en el IPC** (`protocol.py`) — operación nueva del daemon que
   recibe las **muestras de audio ya decodificadas, en base64** más `source_language`,
   y devuelve el texto. El daemon **nunca recibe una ruta del cliente** (invariante
   sin-paths de `protocol.py`, igual que `SynthesizeRequest`/`PrecomputeVoiceRequest`):
   el audio viaja como base64 en el JSON, simétrico a `ResultEvent.audio_b64`
   (`ipc.py:198`), con un tope propio (`MAX_AUDIO_BYTES`, análogo a `MAX_TEXT_LENGTH`).
   Es **aditiva**: una operación nueva no obliga a subir `schema_version` (política de
   compatibilidad de `protocol.py:53`). La captura **no** entra en el IPC: es de cliente.
5. **Ninguno de estos cambios es incompatible.** A diferencia del rename
   `--language`→`--target-language` del subsistema de traducción, aquí no se renombra
   ni se retira nada. Aun así, **tts-sidecar-narrator** debe conocer la superficie
   nueva para exponerla (STT cubrirá toda la CLI del narrator), por lo que el cierre
   incluye actualizarlo en lockstep.

### 3.12 Invariantes (lo que NO cambia)

- El motor de síntesis y su ramificación por idioma (`synthesis.py:130`), los
  `SYNTHESIS_DEFAULTS` por ruta (`engine.py:166`) y la agnosticidad de idioma de la
  voz clonada.
- El subsistema de traducción (`translation/`) y su comando `translate`: la
  transcripción produce texto que **entra** a la traducción sin modificarla.
- La capa de **salida** de `audio.py` (backends por SO, `INT16_MAX_F`): la captura se
  **añade** como contraparte, sin tocar la reproducción.
- 100% local, sin red en tiempo de transcripción/traducción/síntesis (solo `setup`
  descarga).
- El stream NDJSON de `/synthesize` y sus tres eventos (`progress`/`result`/`error`).

---

## 4. Proceso de implementación

Fases ordenadas por dependencia; cada una con su criterio de verificación.

- **Fase 0 — Fundaciones (runtime + empaquetado).** Añadir `faster-whisper` y
  `miniaudio` a dependencias, con los hooks de PyInstaller (recolección nativa de
  faster-whisper: PyAV/onnxruntime; nativo de miniaudio) **validados por SO** en la
  matriz real (win-x64, linux-x64, linux-arm64, macos-arm64); `WhisperModelLoader` con
  caché sobre el formato CT2.
  → *verify*: smoke test de bundle por SO (arranca e importa sin fallos de nativo) +
  tests de carga/caché del loader (sin red, con modelo ya en caché).
- **Fase 1 — Transcripción (motor aislado).** `WhisperTranscriber` (runtime CT2
  embarcado, doble en tests), `TranscriptionService` sobre archivo (lectura WAV con
  `wave`, sin ejercer PyAV). Calibrar tamaño de modelo por defecto con corpus de audio
  `es` fijo.
  → *verify*: tests con WAV fijo → texto esperado; passthrough de errores con
  identidad; medición de latencia/exactitud por tamaño de modelo.
- **Fase 2 — Comando `speech transcribe` (solo `--audio`).** Superficie CLI
  audio→texto desde archivo, `--json`, exit codes.
  → *verify*: tests de CLI (salida, `--json`, `model_missing`, `EXIT_TRANSCRIPTION_FAILED`).
- **Fase 3 — Provisión (`setup`/`doctor`/`cleanup`).** `_provision_whisper_model`
  (descarga sin conversión), enganche opt-in en la taxonomía de `setup`; `doctor` y
  `cleanup` cubren el modelo Whisper.
  → *verify*: tests de detección de caché y del barrido de `cleanup`.
- **Fase 4 — Captura de micrófono.** `AudioRecorder` (miniaudio, 16 kHz/mono/int16),
  enumeración de dispositivos de captura en `audio.py`, `--mic` y `--duration` en
  `speech transcribe`, push-to-talk con guardia de TTY.
  → *verify*: test de degradación headless (sin dispositivo → error claro), test de
  `--duration` determinista; validación manual de push-to-talk por SO (permisos TCC en
  macOS, toggle en Windows).
- **Fase 5 — Composición + daemon.** `--audio`/`--mic` como fuentes en `speech
  say|synthesize` (regla "una de tres"); `TranscribeRequest` en el IPC; precarga y
  `model_loaded` del modelo Whisper en el daemon.
  → *verify*: test extremo a extremo del bucle voz→voz (`--mic --source-language es
  --target-language en` → audio en inglés con la voz), directo y vía daemon.
- **Fase 6 — Cierre.** Actualizar `narrator` en lockstep; sincronizar `USAGE.md`,
  `CLI-CONTRACT.md`, `DESIGN.md`, `GOAL.md`, `ROADMAP.md`; registrar la licencia MIT
  del modelo Whisper.
  → *verify*: suite verde + smoke test de bundle final.

---

## 5. Clasificación de la spec

Aplicando el [criterio de clasificación del Goal](../GOAL.md#clasificación-de-specs)
**sin heredar** la clasificación de specs vecinas:

- **¿Gate externo / dependencia de un tercero?** No: los modelos Whisper se descargan
  sin aprobación ni alta de terceros.
- **¿Condición de madurez / cristalización?** No: se apoya en las rutas de traducción
  y síntesis cross-lingual ya cerradas; no requiere un producto aún inestable.
- **¿Impedimento activo que bloquee el desarrollo inmediato?** No.

Como **no cumple ninguno** de los tres impedimentos, la spec pertenece al **Goal
inmediato** y se trabaja ya —aunque su priorización relativa sea una decisión aparte
(priorizar no expulsa la spec del Goal inmediato). Al absorberse en la documentación
canónica (`GOAL.md`/`DESIGN.md`/`CLI-CONTRACT.md`), este documento se retira, como se
hizo con la propuesta de traducción.
