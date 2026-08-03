# Rediseño de la CLI: síntesis cross-lingual (es→en) y parametrización de la síntesis

## 1. Introducción

Esta propuesta define la integración del **modelo inglés base** (`ChatterboxTTS`) al sidecar de forma **simétrica** al modelo `es-mx-latam` actual, para habilitar la **síntesis cross-lingual**: producir audio **en inglés a partir de texto en inglés reutilizando el timbre clonado de una voz en español**. Como parte del mismo rediseño, se **parametriza la síntesis** (`exaggeration`, `cfg_weight`, `temperature`) en toda la CLI donde corresponda, para **ambas** rutas de síntesis.

**Motivación.** Hoy el sidecar sintetiza con un único modelo (`es-mx-latam`) y con la mayoría de los parámetros de generación fijados en código. Producir inglés nativo con el timbre de una voz clonada en español exige un modelo distinto (el inglés base, cuya firma de generación difiere) y control fino sobre los parámetros de prosodia. La clonación cross-lingual es viable porque el timbre lo aporta S3Gen, que es agnóstico del idioma: la misma voz registrada sirve a ambas rutas sin re-clonar.

**Alcance.** El rediseño cubre: (a) promover el inglés base a modelo de primer nivel (provisión, detección de caché, carga, síntesis); (b) un eje de **idioma** compartido por provisión, síntesis y daemon; (c) la exposición de los tres parámetros de síntesis en la CLI, con defaults por ruta; y (d) los cambios de contrato (protocolo IPC, esquema, health check) que esto implica. Queda **fuera de alcance** la traducción automática (ASR/MT): se parte de texto ya en inglés.

**Estructura del documento.** La [sección 2](#2-estado-actual-as-is) especifica el diseño actual de la CLI (as-is); la [sección 3](#3-estado-objetivo-to-be) define el diseño objetivo tras la integración (to-be); la [sección 4](#4-proceso-de-implementación) describe el proceso de refactor para ir de uno a otro.

## Tabla de contenidos

- [1. Introducción](#1-introducción)
- [2. Estado actual (as-is)](#2-estado-actual-as-is)
  - [2.1 Superficie de comandos](#21-superficie-de-comandos)
  - [2.2 Modelo y ruta de síntesis](#22-modelo-y-ruta-de-síntesis)
  - [2.3 Provisión y detección de caché](#23-provisión-y-detección-de-caché)
  - [2.4 Daemon](#24-daemon)
  - [2.5 Contrato (congelado)](#25-contrato-congelado)
- [3. Estado objetivo (to-be)](#3-estado-objetivo-to-be)
  - [3.1 Taxonomía de idiomas (compartida)](#31-taxonomía-de-idiomas-compartida)
  - [3.2 Dos ejes con la misma taxonomía](#32-dos-ejes-con-la-misma-taxonomía)
  - [3.3 Provisión — `setup --language {es-latam, en, all}`](#33-provisión--setup---language-es-latam-en-all)
  - [3.4 Selector de síntesis — `--language {es-latam, en}`](#34-selector-de-síntesis----language-es-latam-en)
  - [3.5 Parametrización de la síntesis](#35-parametrización-de-la-síntesis--exaggeration--cfg-weight--temperature)
  - [3.6 Defaults de parámetros — por ruta](#36-defaults-de-parámetros--por-ruta)
  - [3.7 Carga y generación por ruta](#37-carga-y-generación-por-ruta)
  - [3.8 Mecánica de overrides (dos cadenas)](#38-mecánica-de-overrides-dos-cadenas)
  - [3.9 Daemon multi-idioma](#39-daemon-multi-idioma--daemon-start---language-es-latam-en-all)
  - [3.10 Mapa de modelos y detección de caché](#310-mapa-de-modelos-y-detección-de-caché)
  - [3.11 Cambios de contrato](#311-cambios-de-contrato)
  - [3.12 Reconciliación de errores](#312-reconciliación-de-errores)
  - [3.13 Invariantes (lo que NO cambia)](#313-invariantes-lo-que-no-cambia)
- [4. Proceso de implementación](#4-proceso-de-implementación)
  - [Fase 0 — Fundaciones (modelo + caché)](#fase-0--fundaciones-sin-comportamiento-nuevo-modelo--caché)
  - [Fase 1 — Carga y generación por ruta (motor)](#fase-1--carga-y-generación-por-ruta-motor)
  - [Fase 2 — Provisión (`setup`)](#fase-2--provisión-setup)
  - [Fase 3 — Superficie CLI de síntesis (directa)](#fase-3--superficie-cli-de-síntesis-directa)
  - [Fase 4 — Contrato del protocolo y daemon](#fase-4--contrato-del-protocolo-y-daemon)
  - [Fase 5 — Cierre](#fase-5--cierre)

---

## 2. Estado actual (as-is)

### 2.1 Superficie de comandos

La CLI (`tts-sidecar`, `src/tts_sidecar/cli.py`) expone estos grupos de comandos:

| Grupo / comando | Propósito |
| --- | --- |
| `speech synthesize` | Sintetiza y **persiste** la locución en el almacén (con `--play` opcional, bucle interactivo). |
| `speech say` | Sintetiza y **reproduce** (no persiste). |
| `speech play` | Reproduce una locución guardada (no toca modelo ni daemon). |
| `speech list` | Lista las locuciones guardadas (filtro opcional por voz). |
| `speech remove` | Borra una locución guardada. |
| `voice clone` | Clona una voz desde dos audios (timbre + habla) y precomputa sus conditionals. |
| `voice list` / `voice remove` | Gestión de voces registradas. |
| `devices` | Lista dispositivos de audio. |
| `doctor` | Diagnóstico de entorno + presencia del modelo en caché. |
| `setup` | Provisiona el runtime: chequeos + descarga del modelo (+ integración de PATH / desinstalación). |
| `cleanup` | Desaprovisiona datos (modelo, voces, habla sintética). |
| `daemon start\|stop\|restart\|status\|serve` | Ciclo de vida del daemon. |
| `version` | Versión del paquete. |

**Flags de síntesis actuales** (`speech synthesize` / `speech say`): `--text`, `--voice`, `--compute-backend {auto,cpu,cuda,mps}`, el grupo mutuamente excluyente `--daemon`/`--no-daemon`, `--json` (y en `synthesize`: `--label`, `--play`, `--force`). **No existe ningún selector de idioma** ni flags para `exaggeration`/`cfg_weight`/`temperature`.

### 2.2 Modelo y ruta de síntesis

El sistema usa **un solo modelo de síntesis**: el language pack **`es-mx-latam`** (`ResembleAI/Chatterbox-Multilingual-es-mx-latam`), sobre la arquitectura `ChatterboxMultilingualTTS` (vocab 2454). El **modelo inglés base** (`ResembleAI/chatterbox`, `ChatterboxTTS`) hoy **solo se usa como fuente de `ve.safetensors`** (Voice Encoder), que el language pack no incluye; nunca sintetiza.

Cadena de síntesis (modo directo):

```
CLI (cmd_speech_*) → _dispatch_synthesis → ChatterboxEngine.get_instance
  → ModelLoader.load → SynthesisOrchestrator.synthesize → _synthesize_impl
  → engine._tts.generate(...)
```

- **`ModelLoader.load`** (`model_loader.py:48`) enruta por la **cadena de la ruta
  de caché**: si contiene `"es-mx-latam"` usa `_load_es_latam` (ensamblado manual
  de T3 + S3Gen + VE + tokenizer + conds); en cualquier otro caso cae a
  `_load_multilingual` → `ChatterboxTTS.from_local` (path efectivamente muerto,
  nunca seleccionado por el CLI).
- **`ChatterboxEngine`** (`engine.py:135`) fija los parámetros optimizados como
  constantes de clase: `MAX_NEW_TOKENS=500`, `N_CFM_TIMESTEPS=4`,
  `EXAGGERATION=0.75`, `EMOTION_ADV=0.5`, y neutraliza el watermark.
- **`SynthesisOrchestrator._synthesize_impl`** (`synthesis.py:92`) hace la única
  llamada de generación:

  ```python
  wav = engine._tts.generate(text, language_id="es", exaggeration=engine.EXAGGERATION)
  ```

**Consecuencia clave del estado actual:**

- `language_id="es"` está **hardcodeado** → la ruta es intrínsecamente multilingüe.
- Solo se cablea `exaggeration` (=0.75). `cfg_weight` y `temperature` **ni se
  pasan**: corren con los **defaults de fábrica** del modelo (`cfg_weight=0.5`,
  `temperature=0.8`). Ese es el comportamiento efectivo de la ruta española hoy.

### 2.3 Provisión y detección de caché

- **`cmd_setup`** (`cli.py:1458`) descarga `es-mx-latam` completo vía
  `snapshot_download` y, aparte, baja **solo `ve.safetensors`** del repo base vía
  `hf_hub_download`. El pre-chequeo de disco es fijo (~4 GB, `MIN_FREE_DISK_BYTES`).
- **`is_model_cached`** (`model_cache.py:152`) valida por archivo **solo la rama
  `es-mx-latam`** (`t3_es_mx_latam.safetensors`, `s3gen_v3.safetensors` + VE con
  header safetensors); cualquier otro modelo cae al `return True` superficial.
- **`model_cache_dirs`** (`model_cache.py:222`) ya incluye la carpeta del repo
  base (cleanup ya lo cubre). `BASE_MODEL_REVISION` ya existe como pin; el mapa
  `MODEL_REVISIONS` **solo pinea `es-mx-latam`**.
- **`MODELS`** (`model_cache.py:14`) mapea `multilingual` y `es-mx-latam`; el
  inglés base **no** es una entrada de primer nivel (solo `BASE_MODEL_REPO`).

### 2.4 Daemon

- **`run.py:102`** arranca con `get_instance(model="es-mx-latam")`
  **hardcodeado**; sirve **un único modelo** fijado al arrancar. La evicción de
  caché en auto-restart (`run.py:168`) también está keyed a `es-mx-latam`.
- **`server.py`** resuelve el nombre de voz contra su registro y llama a
  `engine.synthesize(...)`. La síntesis se serializa con `_synthesis_lock`.
- **`ChatterboxEngine._cache`** (`engine.py:159`) ya está keyed por
  `model+backend`, así que múltiples engines pueden coexistir en RAM (mecanismo
  reutilizable, pero hoy sin explotar en el daemon).

### 2.5 Contrato (congelado)

- **Exit codes** (`exit_codes.py`): `0-8` + `130`. Relevantes aquí:
  `2` EXIT_INVALID_INPUT, `3` EXIT_NOT_FOUND, `4` EXIT_MODEL_MISSING,
  `5` EXIT_DAEMON_UNREACHABLE, `8` EXIT_PRECONDITION_FAILED.
- **`schema_version="2"`** (`cli.py:64` y `protocol.py:53`), emitido en todos los
  payloads JSON del CLI y en los modelos del protocolo daemon.
- **Protocolo IPC** (`protocol.py`):
  - `SynthesizeRequest` = `{text, voice}` (no lleva `model` ni
    `compute_backend`: el daemon los fija al arrancar). No hereda de
    `ProtocolModel`; validación estricta.
  - Stream NDJSON de `/synthesize`: N×`ProgressEvent` → 1×`ResultEvent`
    (o 1×`ErrorEvent`).
  - `HealthResponse.model_loaded` = **bool** (modelo cargado sí/no).
- **Gate de modelo**: `_require_model_cached("es-mx-latam")` corre **client-side
  antes del despacho** en `speech say`/`synthesize` (`cli.py:323,380`),
  `voice clone`, y `daemon start`/`serve`. Los handlers de `FileNotFoundError`
  (`cli.py:337,417`) también hardcodean `is_model_cached("es-mx-latam")`. Un
  modelo ausente → exit `4`, remitiendo a `tts-sidecar setup`.

---

## 3. Estado objetivo (to-be)

Habilitar una **segunda ruta de síntesis** (inglés base) simétrica a la existente, seleccionable por idioma, y **parametrizar `exaggeration`/`cfg_weight`/`temperature`** en **ambas** rutas. La síntesis en inglés reutiliza el **timbre clonado** de una voz en español: S3Gen (que aporta el timbre) es agnóstico de idioma, por eso la clonación cross-lingual funciona sin re-clonar.

### 3.1 Taxonomía de idiomas (compartida)

Un único vocabulario de valores gobierna dos ejes distintos:

- **`es-latam`** — español latinoamericano (modelo `es-mx-latam`).
- **`en`** — inglés (modelo inglés base).
- **`all`** — ambos (solo válido en los ejes de **provisión** y **precarga**, no
  en el de **síntesis**: se sintetiza hacia un solo idioma por invocación).

### 3.2 Dos ejes con la misma taxonomía

`setup --language` (**qué se instala**) y el selector de síntesis (**qué modelo se usa por invocación**) son **ejes separados**. `setup --language en` no le dice a la síntesis que sintetice en inglés; y `speech say --language en` sin el modelo instalado debe fallar remitiendo a `setup --language en` (no a un `setup` genérico).

### 3.3 Provisión — `setup --language {es-latam, en, all}`

- **Default sin flag = `all`**: baja **ambos** modelos. Garantiza offline es+en
  desde el primer uso; el flag sirve para **reducir** el alcance, no ampliarlo.
- **Descarga del inglés base** = `snapshot_download` del repo completo
  (`ResembleAI/chatterbox`), mismo mecanismo que `es-mx-latam`. El snapshot ya
  incluye `ve.safetensors`, así que la descarga selectiva de `ve` de hoy queda
  **redundante para la ruta `en`/`all`** y se consolida.
- **Pre-chequeo de disco**: escala por nº de modelos a provisionar (umbral =
  tamaño estimado × modelos), en vez del fijo actual.

### 3.4 Selector de síntesis — `--language {es-latam, en}`

- En `speech synthesize` y `speech say`. **Sin `all`** (un solo idioma por
  síntesis). **Default = `es-latam`** (preserva el comportamiento actual,
  retrocompatible).
- `speech synthesize --language en …` produce audio en inglés reutilizando el
  timbre de la voz (`--voice`) clonada en español.

### 3.5 Parametrización de la síntesis — `--exaggeration` / `--cfg-weight` / `--temperature`

- **Los tres** disponibles en `speech synthesize` y `speech say`, para **ambas
  rutas**.
- Semántica de override: valor ausente = **default de la ruta** (§3.6); valor
  presente = override.
- **Validación client-side** (en el parseo del CLI): rechazar `cfg_weight=0.0`
  (crash conocido en el inglés base) y valores fuera de rango → exit `2`.

### 3.6 Defaults de parámetros — por ruta

Los defaults **no se comparten** entre rutas (la terna del inglés no está validada para español):

| Ruta | exaggeration | cfg_weight | temperature | Origen |
| --- | --- | --- | --- | --- |
| `es-latam` | 0.75 | 0.5 | 0.8 | Comportamiento efectivo actual (cero regresión). |
| `en` | 0.65 | 0.3 | 0.7 | Configuración ganadora del fine-tuning cross-lingual. |

> La terna de `es-latam` es exactamente su comportamiento efectivo de hoy
> (`exaggeration=0.75` cableado; `cfg_weight`/`temperature` heredados de fábrica).
> Cablearlos explícitamente **no cambia** el resultado, solo lo hace controlable.

### 3.7 Carga y generación por ruta

- **Ruta `en`**: cargar `ChatterboxTTS` (inglés base) desde local
  (`ChatterboxTTS.from_local`). Su `generate` **no acepta `language_id`**; su
  firma es `generate(text, …, exaggeration, cfg_weight, temperature)`. Patrón:
  `prepare_conditionals(ref, exaggeration=…)` una vez → `generate(text,
  exaggeration=, cfg_weight=, temperature=)`.
- **Ruta `es-latam`**: `ChatterboxMultilingualTTS.generate` **ya acepta** los tres
  parámetros; su única diferencia de firma es `language_id` (obligatorio). Solo
  falta **cablear `cfg_weight`/`temperature`** en la llamada de generación (hoy
  pasa únicamente `exaggeration`).
- **Divergencia a resolver en `_synthesize_impl`**: la llamada de generación pasa
  a ramificar por idioma — con `language_id` para `es-latam`, sin él para `en` —
  y a enhebrar los tres parámetros (con el default de la ruta cuando el override
  es `None`).

### 3.8 Mecánica de overrides (dos cadenas)

Enhebrar **tres kwargs opcionales** (`exaggeration`, `cfg_weight`, `temperature`) por las dos cadenas de síntesis:

- **Directa**: `engine.synthesize()` → `SynthesisOrchestrator.synthesize()` →
  `_synthesize_impl()` → `generate()`. Hoy ninguna tiene slots para estos
  parámetros.
- **Daemon**: añadir los tres campos (más `language`) a `SynthesizeRequest`
  (`protocol.py`), al cliente IPC (`ipc.py`) y al worker (`server.py`).
- Un override de `exaggeration` **no invalida** el cache de conditionals: la clave
  del cache es `(voice_dir, mtime)`, y `generate` parcha `emotion_adv`
  internamente cuando difiere.

### 3.9 Daemon multi-idioma — `daemon start --language {es-latam, en, all}`

- **Default = `all`** (consistente con `setup`): precarga en caliente ambos
  modelos.
- **Carga perezosa desde disco**: cualquier idioma no precargado se carga al
  primer uso **sin reiniciar**, reutilizando `ChatterboxEngine._cache` (keyed por
  `model+backend`). La carga perezosa **carga desde disco, NO descarga**: si el
  modelo no está *instalado*, el daemon falla remitiendo a `setup --language <x>`
  (descargar es tarea exclusiva de `setup`). En la práctica, el gate client-side
  (§3.12) hace que el daemon nunca vea "no instalado": solo enfrenta "en disco
  pero no en RAM".

### 3.10 Mapa de modelos y detección de caché

- **Promover el inglés base a entrada de primer nivel**: alias `en` →
  `ResembleAI/chatterbox` en `MODELS`, y su pin en `MODEL_REVISIONS`
  (reutilizando `BASE_MODEL_REVISION`). Un solo mecanismo uniforme para
  selector/detección/pins.
- **`is_model_cached` estricta por archivo para `en`**: validar existencia +
  header safetensors de `t3_cfg`, `s3gen`, `tokenizer.json`, `conds.pt` y `ve`
  (espejo de la rama `es-mx-latam`), en vez del `return True` superficial.
- Matiz: `en` y la fuente de `ve` son el **mismo repo** → unificar con la lógica
  de `ve` existente para evitar doble descarga/validación.
- **`doctor`** deriva de esto: valida por idioma lo que hay en caché.

### 3.11 Cambios de contrato

| Elemento | Hoy | Objetivo |
| --- | --- | --- |
| `schema_version` | `"2"` | **`"3"`** — rechaza el skew CLI↔daemon (un daemon viejo descartaría `language=en` en silencio y sintetizaría en español). |
| `SynthesizeRequest` | `{text, voice}` | `+ {language, exaggeration?, cfg_weight?, temperature?}` |
| `HealthResponse.model_loaded` | `bool` | **estructura por idioma** (qué modelos están calientes). |
| Exit codes | `0-8/130` | **Sin cambios**. "Modelo de idioma X no instalado" → `4`; "parámetro inválido" (`cfg=0.0`) → `2`. El código identifica la clase; el idioma va en el `reason`/mensaje. |

> El salto de esquema a `"3"` es admisible: los contratos son pre-release, de un
> solo dueño; la restricción dura es la **consistencia entre repos**, no la
> compatibilidad hacia atrás. Con el esquema subido, redefinir `model_loaded` como
> estructura (rompiendo su forma) no incurre costo adicional.

### 3.12 Reconciliación de errores

- Parametrizar el gate client-side por idioma:
  `_require_model_cached(model_for(args.language))`. Ya lanza `CliError(4,
  "model_missing", …)`; cambiar el mensaje a `setup --language <x>`.
- Parametrizar también los handlers de `FileNotFoundError` que hoy hardcodean
  `is_model_cached("es-mx-latam")`.
- Como el gate corre en ambos modos (directo y daemon), el daemon nunca ve "no
  instalado": su carga perezosa solo enfrenta "en disco pero no en RAM".

### 3.13 Invariantes (lo que NO cambia)

- Los dos modelos **conviven**; ninguno reemplaza al otro. Cada uno sirve su
  idioma.
- **No** se pasa `language_id` al inglés base (rompe).
- **No** se permite `cfg_weight=0.0`.
- **No** se parcha la librería `chatterbox`.
- **No** se re-barre la terna del inglés base (ya decidida) ni se reabre un spike
  para afinar español ahora.
- Fuera de alcance: traducción automática (ASR/MT). Se parte de texto ya en inglés.

---

## 4. Proceso de implementación

Orden de fases pensado para **cero regresión** en la ruta española en cada paso, y enfoque test-first donde haya framework de tests. Cada fase enuncia su criterio de verificación.

### Fase 0 — Fundaciones sin comportamiento nuevo (modelo + caché)

Promover el inglés base a modelo de primer nivel, sin tocar aún la síntesis.

1. `MODELS['en'] = "ResembleAI/chatterbox"` + pin en `MODEL_REVISIONS`
   (`BASE_MODEL_REVISION`).
2. `is_model_cached` con rama real para `en` (validación estricta por archivo,
   espejo de `es-mx-latam`, unificada con la lógica de `ve`).
3. Helper `model_for(language)` (`es-latam` → `es-mx-latam`, `en` → `en`).

**Verificar:** tests de `model_cache` para ambas ramas (cacheado/truncado/ausente); la detección de `es-mx-latam` no cambia de comportamiento.

### Fase 1 — Carga y generación por ruta (motor)

1. `ModelLoader`: rama de carga para `en` (`ChatterboxTTS.from_local`), simétrica
   a `_load_es_latam`.
2. `ChatterboxEngine`: aceptar `model="en"` y propagar la elección hasta el
   orquestador. Confirmar que `_cache` (keyed `model+backend`) admite ambos
   engines en RAM.
3. `SynthesisOrchestrator._synthesize_impl`: ramificar la llamada `generate` por
   idioma (con/sin `language_id`) y **enhebrar los tres parámetros**; `None` =
   default de la ruta (§3.6). Cablear `cfg_weight`/`temperature` también en la
   ruta `es-latam`.

**Verificar:** un test de síntesis directa en `en` produce audio; la síntesis en `es-latam` con overrides `None` reproduce byte-a-byte (o métrica equivalente) el resultado actual (cero regresión).

### Fase 2 — Provisión (`setup`)

1. `setup --language {es-latam, en, all}`, default `all`.
2. Descarga del inglés base vía `snapshot_download` completo; consolidar la
   descarga redundante de `ve`.
3. Pre-chequeo de disco escalado por nº de modelos (estimar el tamaño del inglés
   base).
4. `doctor` valida por idioma.

**Verificar:** `setup --language en` deja el inglés base íntegro y `doctor` lo reporta PASS; `setup` a secas baja ambos; el disco se chequea proporcional.

### Fase 3 — Superficie CLI de síntesis (directa)

 1. Añadir `--language {es-latam, en}` (default `es-latam`) y
    `--exaggeration`/`--cfg-weight`/`--temperature` a `speech synthesize` y
    `speech say`, con validación (rechazo de `cfg=0.0`, rangos) → exit `2`.
 2. Parametrizar el gate: `_require_model_cached(model_for(args.language))` y los
    handlers de `FileNotFoundError`; mensaje → `setup --language <x>`.
 3. Enhebrar `language` + los tres overrides por la cadena directa
    (`_dispatch_synthesis` → `engine.synthesize`).

**Verificar:** `speech say --language en --text "…"` sintetiza en inglés en modo directo; sin `--language` el comportamiento es idéntico al actual; `--cfg-weight 0` sale `2`; idioma sin modelo instalado sale `4` remitiendo a `setup --language <x>`.

### Fase 4 — Contrato del protocolo y daemon

 1. Subir `schema_version` a `"3"` (`cli.py` y `protocol.py`).
 2. `SynthesizeRequest += {language, exaggeration?, cfg_weight?, temperature?}`;
    propagar en `ipc.py` (cliente) y `server.py` (worker).
 3. Redefinir `HealthResponse.model_loaded` como estructura por idioma; ajustar
    `is_running()`/`status` y el chequeo de skew (exigir coincidencia de versión
    CLI↔daemon).
 4. `daemon start --language {es-latam, en, all}` (default `all`): precarga en
    caliente; carga perezosa desde disco al primer uso (sin descargar); des-
    hardcodear `get_instance(model="es-mx-latam")` en `run.py` y la evicción del
    auto-restart.

**Verificar:** `daemon start --language all` precarga ambos; `speech say --language en --daemon` sintetiza en inglés vía daemon; un idioma no precargado se carga perezosamente sin reiniciar; un idioma no instalado falla remitiendo a `setup --language <x>`; CLI y daemon con esquemas distintos se rechazan.

### Fase 5 — Cierre

 1. Barrido de los puntos hardcodeados a `es-mx-latam` restantes (gates, doctor,
    mensajes).
 2. Actualizar documentación de usuario (`USAGE.md`, `README.md`, `DAEMON-MODE.md`)
    con el eje de idioma y los parámetros.
 3. Suite completa en verde; verificación manual de una síntesis cross-lingual
    real (voz española → audio inglés) por juicio auditivo.

**Verificar:** suite verde; audio cross-lingual aceptable; sin drift residual de `es-mx-latam` hardcodeado fuera de su rama legítima.
