# Subsistema de traducción: cierre del bucle de clonación cross-lingual

## 1. Introducción

Esta propuesta define un **subsistema de traducción de texto local** (`es ↔ en`)
que se inserta **antes** de la síntesis para cerrar el bucle de **clonación
cross-lingual** de TTS-Sidecar: que el usuario **"hable" otro idioma con su
propia voz clonada** sin necesitar conocer la gramática ni la prosodia del idioma
objetivo. El usuario escribe (o dicta) en su idioma nativo; el sistema traduce el
texto y lo sintetiza con su timbre en el idioma destino.

**Motivación.** La síntesis cross-lingual del *audio* **ya existe**: el rediseño
cerrado en v0.7.0–v0.9.0 promovió el modelo inglés base a modelo de primer nivel,
y hoy `speech say|synthesize --language en -v mi_voz` **reutiliza el timbre
clonado para producir audio en inglés** (`cli.py`, el selector `--language
{es-latam, en}`). Pero ese rediseño dejó **explícitamente fuera de alcance la
traducción automática**: *"se parte de texto ya en inglés"*. Es decir, para
"hablar inglés" hoy el usuario **debe escribir el texto ya en inglés** — justo la
barrera (gramática, ortografía, idiomática) que la clonación cross-lingual promete
eliminar. Este subsistema elimina esa barrera: es el eslabón de **texto→texto**
que faltaba entre la intención del usuario y el motor multilingüe que ya tenemos.

**Alcance.** El subsistema cubre: (a) un **pipeline de traducción por capas**
(validación → segmentación → traducción → ensamblado), CPU-first, 100% local;
(b) un **comando `translate`** (texto→texto) verificable de forma aislada; (c) la
integración de la traducción en el flujo de síntesis mediante los flags
`--source-language` y `--target-language` (rename de `--language`) en `speech
say|synthesize`; (d) la **provisión de los modelos de traducción** por el canal de
`setup` (patrón Chatterbox, fuera del bundle); (e) el **cacheo en caliente** del
modelo de traducción en el daemon; y (f) los **cambios de contrato** que esto
implica. Queda **fuera de alcance**: el reconocimiento de voz (ASR/dictado), la
traducción a idiomas distintos de `es`/`en`, la normalización técnica pesada de
texto (URLs, bloques de código) — impropia de la entrada conversacional de voz —, y
la **corrección ortográfica o gramatical** del texto (mismo-idioma): es una tarea
distinta de la traducción (spellcheck/GEC, no MT direccional), que un modelo `opus-mt`
no realiza; si se necesitara, sería una feature independiente de preproceso, no una
función del traductor.

**Restricciones heredadas del proyecto.** Todo el subsistema respeta las
restricciones del [Goal](../GOAL.md): **100% local** (sin APIs externas), **motor
y dependencias con licencia compatible con GPL-3.0-or-later**, **multiplataforma**
(Windows/Linux/macOS), **consumible por CLI**, y **empaquetable con PyInstaller**
sin fricción.

**Estructura del documento.** La [sección 2](#2-estado-actual-as-is) especifica el
estado actual (as-is); la [sección 3](#3-estado-objetivo-to-be) define el estado
objetivo (to-be); la [sección 4](#4-proceso-de-implementación) describe el proceso
de implementación por fases; la [sección 5](#5-clasificación-de-la-spec) clasifica
la spec según el criterio del Goal.

## Tabla de contenidos

- [1. Introducción](#1-introducción)
- [2. Estado actual (as-is)](#2-estado-actual-as-is)
  - [2.1 La síntesis cross-lingual ya existe, la traducción no](#21-la-síntesis-cross-lingual-ya-existe-la-traducción-no)
  - [2.2 Superficie de síntesis y taxonomía de idiomas](#22-superficie-de-síntesis-y-taxonomía-de-idiomas)
  - [2.3 Contrato congelado](#23-contrato-congelado)
  - [2.4 Provisión y daemon](#24-provisión-y-daemon)
- [3. Estado objetivo (to-be)](#3-estado-objetivo-to-be)
  - [3.1 El pipeline de traducción por capas](#31-el-pipeline-de-traducción-por-capas)
  - [3.2 Motor de traducción, runtime y licencia](#32-motor-de-traducción-runtime-y-licencia)
  - [3.3 Segmentación con pysbd](#33-segmentación-con-pysbd)
  - [3.4 Ubicación en la arquitectura](#34-ubicación-en-la-arquitectura)
  - [3.5 Comando `translate` (texto→texto)](#35-comando-translate-textotexto)
  - [3.6 Flags de síntesis: `--source-language` y `--target-language`](#36-flags-de-síntesis---source-language-y---target-language)
  - [3.7 Integración en el flujo de síntesis](#37-integración-en-el-flujo-de-síntesis)
  - [3.8 Provisión de los modelos de traducción](#38-provisión-de-los-modelos-de-traducción)
  - [3.9 Daemon: traducción en caliente](#39-daemon-traducción-en-caliente)
  - [3.10 Cambios de contrato](#310-cambios-de-contrato)
  - [3.11 Invariantes (lo que NO cambia)](#311-invariantes-lo-que-no-cambia)
- [4. Proceso de implementación](#4-proceso-de-implementación)
- [5. Clasificación de la spec](#5-clasificación-de-la-spec)

---

## 2. Estado actual (as-is)

### 2.1 La síntesis cross-lingual ya existe, la traducción no

El motor es intrínsecamente multilingüe. `ChatterboxEngine` (`engine.py`) sirve
dos rutas de síntesis, seleccionadas por el alias de idioma que recibe en `model`
(`self._language`, `engine.py:222`):

- **`es-mx-latam`** → `ChatterboxMultilingualTTS.generate(..., language_id="es")`.
- **`en`** → `ChatterboxTTS.generate(...)` (el inglés base, sin `language_id`).

La clonación cross-lingual del **audio** funciona porque el timbre lo aporta S3Gen,
que es **agnóstico del idioma**: la misma voz registrada con `voice clone` sirve a
ambas rutas **sin re-clonar**. `SynthesisOrchestrator.synthesize` (`synthesis.py:33`)
ramifica por idioma y solo añade `language_id="es"` cuando `language != "en"`
(`synthesis.py:130`).

**El hueco.** El texto que entra a `generate()` se sintetiza **tal cual**. No hay
ninguna etapa de traducción: si el usuario pasa texto en español y `--language en`,
el modelo inglés intentará leer español con fonética inglesa. El contrato vigente
asume que **el llamador ya tradujo el texto**. Este es el hueco que la propuesta
cierra.

### 2.2 Superficie de síntesis y taxonomía de idiomas

`speech synthesize` y `speech say` (`cli.py`) exponen hoy un **único** eje de
idioma, `--language {es-latam, en}` (default `es-latam`), que selecciona **a la vez**
el modelo TTS y el idioma del audio de salida (`cli.py:2034` y `cli.py:2063`). Los
tres overrides de síntesis (`--exaggeration`, `--cfg-weight`, `--temperature`)
resuelven contra `ChatterboxEngine.SYNTHESIS_DEFAULTS` por ruta (`engine.py:166`).

La taxonomía de idiomas del proyecto es `{es-latam, en, all}` en provisión/daemon y
`{es-latam, en}` en síntesis. **No existe** un concepto de "idioma del texto de
entrada" distinto del "idioma de salida": hoy son el mismo eje.

### 2.3 Contrato congelado

- **Exit codes**: centralizados en `exit_codes.py` (contrato público congelado).
- **Esquema IPC**: `schema_version = "3"` (`protocol.py:53`), con `extra="ignore"`
  para compatibilidad aditiva hacia adelante/atrás.
- **`SynthesizeRequest`** (`protocol.py:56`): `text` (≤ `MAX_TEXT_LENGTH`=5000),
  `voice`, `language="es-latam"`, y los tres overrides. **No** lleva ningún campo
  de idioma de origen.
- **`--json`**: payloads legibles por máquina con clave `error`.

El contrato lo consume el repo hermano **tts-sidecar-narrator** (plugin de Claude
Code). Cualquier cambio incompatible en la CLI obliga a actualizarlo en lockstep
(ver [3.10](#310-cambios-de-contrato)).

### 2.4 Provisión y daemon

- **`setup --language {es-latam, en, all}`** (`cli.py:2165`) descarga los modelos
  Chatterbox a la caché de HuggingFace. **El modelo no vive en el bundle**: se
  provisiona aquí (patrón que este subsistema reutiliza).
- **`daemon start --language {es-latam, en, all}`** (`cli.py:2201`) precarga los
  modelos en RAM; `HealthResponse.model_loaded` (`protocol.py:118`) reporta qué
  modelos están calientes por idioma.

---

## 3. Estado objetivo (to-be)

### 3.1 El pipeline de traducción por capas

El subsistema es una cadena de responsabilidades separadas, CPU-first y 100% local:

```text
Texto en el idioma de origen
   ↓  Validación          (largo, encoding — reutiliza límites existentes)
   ↓  Segmentación         (pysbd: párrafo → oración → sub-oración)
   ↓  Traducción           (MarianMT es↔en, por segmento)
   ↓  Ensamblado           (reconstruye orden, párrafos y saltos de línea)
Texto en el idioma destino  →  Síntesis (motor multilingüe ya existente)
```

La separación segmentación/traducción evita acoplar reglas lingüísticas al motor y
permite sustituir el motor sin tocar el resto. La prioridad es la **naturalidad
oral** del resultado (importa la longitud de los segmentos y la preservación del
ritmo textual), no la literalidad, porque el consumidor final es la voz.

### 3.2 Motor de traducción, runtime y licencia

**Motor: MarianMT (Helsinki-NLP `opus-mt`).** Pares `opus-mt-es-en` y
`opus-mt-en-es`. Es la combinación fuerte para CPU, latencia y portabilidad, con el
linaje OPUS bien cubierto para `es↔en`.

**Requerimiento no funcional: traducir lo más rápido posible en CPU-only.** La
traducción no debe percibirse como una espera añadida antes de la voz. Ese NFR
gobierna la elección del *runtime* de inferencia, que es una decisión **distinta**
de la del *motor*: el mismo modelo `opus-mt` corre bajo runtimes diferentes con
velocidades muy distintas.

**Runtime: CTranslate2.** CT2 es el runtime canónico para MarianMT rápido en CPU
(int8, mejor threading; se reportan ~2–4× de velocidad y de memoria). Se adopta
**directamente** —no tras un gate de medición— porque el NFR de velocidad ya está
declarado: interponer un baseline intercambiable solo para decidir un runtime ya
decidido sería infraestructura sin beneficio (Simplicity First). Sus propiedades
encajan con las restricciones del proyecto:

- **Licencia MIT** (compatible GPL) y **wheels precompilados** para Windows x64,
  Linux x86_64/aarch64 y macOS x86_64/arm64 — cubre los targets del proyecto.
- **Costo de empaquetado, acotado:** trae una **librería nativa** que PyInstaller
  debe recolectar (hook `collect_dynamic_libs('ctranslate2')` más su OpenMP),
  **validado por SO** — el mismo tipo de empaquetado nativo que `torch` **ya
  resolvió**, no uno nuevo.
- **Conversión del modelo:** los pesos `opus-mt` se convierten al formato CT2
  (`ct2-transformers-converter`) **una sola vez, en `setup`**
  ([3.8](#38-provisión-de-los-modelos-de-traducción)); el formato CT2 es portable, no
  se reconvierte por plataforma.

**Alcance del speedup (Amdahl).** La ganancia perceptible vive en el comando
`translate` **aislado**, donde el tiempo de traducción se traslada directo al
usuario. En la ruta `speech say|synthesize` el TTS **domina** el tiempo total, así
que acelerar la traducción de una oración corta se diluye: el bucle de síntesis
cross-lingual no se siente más rápido por CT2, y no es su objetivo que lo sea.

**Backend inyectable — por testabilidad, no por indecisión.** `MarianTranslator`
([3.4](#34-ubicación-en-la-arquitectura)) mantiene el runtime como colaborador
inyectable. El runtime **embarcado es CT2**; la costura se conserva para poder correr
`transformers` (sobre el `torch` ya presente, sin arrastrar la librería nativa) en
los tests y para aislar si un fallo es del modelo o del runtime. No hay dos runtimes
en producción: hay **uno** (CT2) y una salida de pruebas.

**Descartado: LibreTranslate / Argos Translate.** LibreTranslate es un **servidor**
(Flask, **AGPL-3.0**): forma arquitectónica equivocada para un binario único local, y
con licencia más viral que la del proyecto. Su motor, **Argos Translate**, ya es
*opus + CTranslate2* (mismo runtime y por tanto **misma velocidad** que la vía
directa), pero arrastra un **gestor de modelos propio** y un **splitter propio**
(históricamente Stanza, pesado) que **duplican y contradicen** decisiones ya tomadas:
la caché de HuggingFace ([3.8](#38-provisión-de-los-modelos-de-traducción)) y `pysbd`
([3.3](#33-segmentación-con-pysbd)). Como ya se usa CT2 como runtime, la vía es
**convertir `opus-mt` directamente** con `ct2-transformers-converter`, no adoptar el
empaquetado de Argos.

**La licencia decide el motor, no es una elección abierta.** El Goal exige un motor
con licencia **compatible con GPL-3.0-or-later**:

| Motor | Licencia | ¿Compatible GPL-3.0? |
| --- | --- | --- |
| **Helsinki-NLP `opus-mt`** | **CC-BY-4.0** (algunos Apache-2.0) | **Sí** — solo exige atribución |
| NLLB-200 | CC-BY-**NC**-4.0 (no comercial) | **No** — la cláusula NC viola la libertad 0 de la GPL |

Por eso **NLLB queda descartado**, pese a su cobertura de 200 idiomas: su licencia
no comercial es incompatible con la redistribución libre bajo GPL. Se debe
**verificar la licencia del par concreto** en su model card antes de fijarlo (la
colección `opus-mt` no es uniforme) y registrar la atribución CC-BY correspondiente.

### 3.3 Segmentación con pysbd

MarianMT tiene un límite de entrada (≈512 tokens); un texto largo debe partirse
**sin cortar oraciones a la mitad** (un corte a media oración degrada la traducción
y, en cascada, la naturalidad de la voz). La estrategia es **jerárquica**:

1. intentar el **párrafo** completo;
2. si no cabe, dividir en **oraciones**;
3. si no cabe, dividir por **puntuación fuerte**;
4. como último recurso, dividir por **tokens** del propio tokenizer Marian.

La detección de límites de oración (paso 2) usa **`pysbd`** (Python Sentence
Boundary Disambiguation): **puro Python, licencia MIT, soporta es/en**, robusto ante
abreviaturas, decimales y siglas. Se eligió sobre **spaCy** (mejor calidad, pero
dependencia pesada con binarios compilados `blis`/`thinc` y fricción conocida con
PyInstaller) y sobre **reglas regex ligeras** (sin dependencias, pero frágiles ante
abreviaturas). `pysbd` da la robustez lingüística de spaCy **sin fricción de
empaquetado ni modelos descargables**, alineado con "Simplicity First".

### 3.4 Ubicación en la arquitectura

El subsistema vive en un subpaquete nuevo `src/tts_sidecar/translation/`, espejando
el patrón de `daemon/`, con **colaboradores inyectables** (como `ModelLoader` /
`ConditionalsPreparer` del motor de síntesis):

```text
src/tts_sidecar/translation/
├── __init__.py          # Exportaciones públicas del paquete
├── service.py           # TranslationService: orquesta el pipeline (validar→segmentar→traducir→ensamblar)
├── segmenter.py         # SentenceSegmenter: segmentación jerárquica (pysbd + tokenizer)
├── translator.py        # MarianTranslator: traducción por segmento (runtime inyectable; CT2 embarcado, transformers solo en tests)
├── model_loader.py      # TranslationModelLoader: carga/caché de opus-mt (inyectable)
└── assembler.py         # SegmentAssembler: reconstrucción del texto destino
```

Las excepciones del subsistema se añaden a `exceptions.py` (compartido, sin imports
pesados). La resolución de idiomas normaliza la taxonomía TTS a ISO para el
traductor: **`es-latam` → `es`**; `en` → `en`. El par de modelo se deriva del par
`(origen, destino)` normalizado: `opus-mt-{src}-{tgt}`.

### 3.5 Comando `translate` (texto→texto)

Subsistema autónomo, verificable sin audio ni modelo TTS:

```bash
tts-sidecar translate --text "Hola, buenos días" --from es --to en
# → "Good morning"

tts-sidecar translate --text "Good morning" --from en --to es --json
# → {"translated": "Buenos días", "source": "en", "target": "es"}
```

- Flags: `--text` (requerido, ≤ `MAX_TEXT_LENGTH`), `--from {es, en}` (**requerido**),
  `--to {es, en}` (**requerido**), `--json`. `--from` y `--to` son obligatorios porque
  traducir es la única función del comando: no hay acción por defecto ni detección
  automática de idioma (fuera de alcance), así que ambos extremos deben ser explícitos.
- **Taxonomía ISO `{es, en}`, no `es-latam`:** la traducción opera sobre **texto**,
  donde el acento latino es irrelevante; `es-latam` es una ruta de *síntesis*, no una
  variedad de texto. La normalización `es-latam → es`
  ([3.4](#34-ubicación-en-la-arquitectura)) reconcilia ambas superficies cuando
  `speech` invoca al traductor.
- **Nombres `--from/--to`, deliberadamente distintos de los de `speech`:** en una
  utilidad texto→texto que se teclea seguido, `--from/--to` es más ergonómico y es
  convención común en CLIs de traducción; evita además la redundancia
  `translate --source-language`. Los nombres largos
  `--source-language`/`--target-language` se reservan para `speech`
  ([3.6](#36-flags-de-síntesis---source-language-y---target-language)), donde el eje
  origen/destino es del audio.
- Si `--from == --to`, es **passthrough** (devuelve el texto sin cargar modelo).
- Errores con identidad propia (no colapsados en un genérico): modelo de traducción
  ausente (`model_missing`, remite a `setup`), par no soportado, y **fallo de
  traducción** (nuevo exit code, ver [3.10](#310-cambios-de-contrato)).

### 3.6 Flags de síntesis: `--source-language` y `--target-language`

`speech say|synthesize` pasan de **un** eje de idioma a **dos**:

| Flag | Significado | Default |
| --- | --- | --- |
| `--source-language {es-latam, en}` | Idioma del **texto de entrada** | igual a `--target-language` (⇒ no traduce) |
| `--target-language {es-latam, en}` | Idioma del **audio de salida** (rename de `--language`) | `es-latam` |

**Ambos opcionales; la traducción es opt-in.** A diferencia de `translate` (donde
`--from`/`--to` son requeridos), aquí la acción primaria es **sintetizar**: omitir
ambos reproduce el comportamiento actual (`--target-language` cae en `es-latam`,
`--source-language` hereda el destino ⇒ sin traducción). La traducción solo se activa
cuando declaras **explícitamente** un `--source-language` distinto del destino.

- Si `source == target`: comportamiento actual, **sin traducción**.
- Si `source != target`: el texto se **traduce** de `source` a `target` **antes** de
  sintetizar con la voz clonada en `target`. Un solo comando cierra el bucle:

```bash
tts-sidecar speech say \
  --text "Hola, ¿cómo estás?" \
  --source-language es --target-language en \
  -v mi_voz
# → traduce ES→EN y sintetiza en inglés con TU voz clonada, en un paso
```

El default de `--source-language` (heredar `--target-language`) preserva el
comportamiento de todo invocador que no conozca el flag nuevo.

### 3.7 Integración en el flujo de síntesis

La traducción se inserta como **etapa previa** a `SynthesisOrchestrator.synthesize`,
sin tocar la ramificación por idioma del motor:

```text
CLI (cmd_speech_*) → [NUEVO] TranslationService.translate (si source != target)
  → _dispatch_synthesis → ChatterboxEngine.get_instance(target)
  → SynthesisOrchestrator.synthesize(texto_traducido) → engine._tts.generate(...)
```

El motor de síntesis **no cambia**: sigue recibiendo texto ya en el idioma destino;
solo cambia **quién** produjo ese texto (antes el usuario, ahora el traductor cuando
`source != target`).

### 3.8 Provisión de los modelos de traducción

Los modelos `opus-mt-*` **no se empaquetan en el bundle** (igual que Chatterbox):
se descargan a la caché de HuggingFace en `setup`. `setup --language en` (o `all`)
descarga, junto al modelo TTS inglés, el par de traducción necesario para el bucle
`es ↔ en`, y lo **convierte al formato CT2** (`ct2-transformers-converter`) una sola
vez tras la descarga; el artefacto convertido es portable entre plataformas
([3.2](#32-motor-de-traducción-runtime-y-licencia)). `doctor` reporta la presencia
del modelo convertido; `cleanup` lo incluye en el barrido de caché. `pysbd` (puro
Python, sin modelos) **sí** viaja en el bundle sin fricción.

### 3.9 Daemon: traducción en caliente

El daemon cachea el modelo de traducción en RAM igual que los modelos TTS, para no
pagar la carga en cada petición. `daemon start --language {en, all}` precarga
también el par `opus-mt`. `HealthResponse.model_loaded` (`protocol.py:118`) añade la
señal del modelo de traducción con una **clave de par propia** (p. ej.
`translate:es-en`), sin colisionar con las claves de idioma TTS (`es-latam`/`en`) del
mismo dict. La carga es **perezosa** si no se precargó: un par ausente del dict se
carga al primer uso.

### 3.10 Cambios de contrato

1. **Comando nuevo `translate`** — superficie aditiva; su contrato `--json`
   (`{translated, source, target}`) se documenta en `CLI-CONTRACT.md`.
2. **`SynthesizeRequest.source_language`** (`protocol.py:56`) — campo nuevo con
   default (`= language`, ⇒ no traduce). Es **aditivo**: por la propia política de
   `schema_version`, un campo nuevo con default **no** obliga a subir la versión.
3. **Exit code nuevo** `EXIT_TRANSLATION_FAILED` en `exit_codes.py` — aditivo;
   distingue el fallo de traducción del de síntesis (errores distintos ⇒ identidad
   propia). El modelo de traducción ausente reutiliza `model_missing` remitiendo a
   `setup`.
4. **⚠ Rename incompatible `--language` → `--target-language`** en `speech
   say|synthesize` — **este sí es un cambio incompatible** del contrato público que
   `CLI-CONTRACT.md` congela. Como **tts-sidecar-narrator** consume la CLI, el rename
   obliga a **actualizar el narrator en lockstep**: la única restricción dura entre
   los repos es su **consistencia**. **Decisión: hard rename con release coordinado**,
   sin alias de transición. Se elige el corte limpio —no un `--language` deprecado—
   porque ambos repos son **pre-release y de un solo dueño**: no hay consumidores
   externos que justifiquen arrastrar superficie duplicada, y mantener un alias sería
   deuda de deprecación sin beneficio (Simplicity First). Ambos repos se publican
   coordinadamente. El resto del eje de idioma (`setup`, `daemon`, `doctor`)
   **conserva** `--language`, que allí no es ambiguo (no hay origen/destino en
   provisión).

### 3.11 Invariantes (lo que NO cambia)

- La ramificación por idioma del motor (`synthesis.py:130`) y los
  `SYNTHESIS_DEFAULTS` por ruta (`engine.py:166`).
- La agnosticidad de idioma de la voz clonada: **una** voz sirve a ambas rutas sin
  re-clonar. La traducción es de **texto**; no toca timbre ni conditionals.
- El almacén de voces de dos niveles (usuario→fábrica, `voices.py`).
- 100% local, sin red en tiempo de síntesis/traducción (solo `setup` descarga).
- El stream NDJSON de `/synthesize` y sus tres eventos (`progress`/`result`/`error`).

---

## 4. Proceso de implementación

Fases ordenadas por dependencia; cada una con su criterio de verificación.

- **Fase 0 — Fundaciones (modelo + segmentación).** Añadir `pysbd` y el runtime de
  traducción `ctranslate2` a dependencias, con su hook de PyInstaller
  (`collect_dynamic_libs('ctranslate2')` + OpenMP) validado por SO;
  `TranslationModelLoader` con caché sobre el formato CT2; mapeo `es-latam→es` y
  derivación del par `opus-mt-{src}-{tgt}`.
  → *verify*: tests de carga/caché y de resolución de par (sin red, con modelo ya
  convertido en caché).
- **Fase 1 — Pipeline (motor aislado).** `SentenceSegmenter` (jerárquico),
  `MarianTranslator` con runtime CT2 embarcado (`transformers` disponible tras la
  costura inyectable solo para tests, ver
  [3.2](#32-motor-de-traducción-runtime-y-licencia)), `SegmentAssembler`,
  `TranslationService`.
  → *verify*: tests unitarios + regression con corpus fijo `es↔en`; passthrough
  cuando `source == target`.
- **Fase 2 — Comando `translate`.** Superficie CLI texto→texto, `--json`, exit
  codes.
  → *verify*: tests de CLI (salida, `--json`, errores con identidad).
- **Fase 3 — Provisión (`setup`).** Descarga del par `opus-mt` y **conversión al
  formato CT2** (`ct2-transformers-converter`, una sola vez) en `setup --language
  {en, all}`; `doctor`/`cleanup` cubren tanto los pesos descargados como el modelo
  convertido.
  → *verify*: tests de detección de caché (modelo convertido presente) y del barrido
  de `cleanup`.
- **Fase 4 — Integración en síntesis + daemon.** Etapa previa en el flujo directo;
  `SynthesizeRequest.source_language`; precarga y `model_loaded` en el daemon; flags
  `--source-language`/`--target-language` (hard rename de `--language`, ver
  [3.10](#310-cambios-de-contrato)).
  → *verify*: tests directo y daemon del bucle `es→en` extremo a extremo (texto en
  español → audio en inglés con la voz).
- **Fase 5 — Cierre.** Actualizar `narrator` en lockstep; sincronizar
  `USAGE.md`, `CLI-CONTRACT.md`, `DESIGN.md`, `GOAL.md`; registrar la atribución
  CC-BY del modelo.
  → *verify*: suite verde + smoke test de bundle (empaquetado con `pysbd` arranca).

---

## 5. Clasificación de la spec

Aplicando el [criterio de clasificación del Goal](../GOAL.md#clasificación-de-specs)
**sin heredar** la clasificación de specs vecinas:

- **¿Gate externo / dependencia de un tercero?** No: los modelos `opus-mt` se
  descargan sin aprobación ni alta de terceros.
- **¿Condición de madurez / cristalización?** No: no requiere un producto ya
  estabilizado; se apoya en la ruta cross-lingual ya cerrada.
- **¿Impedimento activo que bloquee el desarrollo inmediato?** No.

Como **no cumple ninguno** de los tres impedimentos, la spec pertenece al **Goal
inmediato** y se trabaja ya — aunque su priorización relativa sea una decisión
aparte (priorizar no expulsa la spec del Goal inmediato). Al absorberse en la
documentación canónica (`GOAL.md`/`DESIGN.md`/`CLI-CONTRACT.md`), este documento se
retira de `docs/proposals/`, como se hizo con el rediseño de la CLI que lo precede.
