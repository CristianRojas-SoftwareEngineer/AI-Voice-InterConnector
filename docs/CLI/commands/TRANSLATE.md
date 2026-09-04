## Recorrido

La investigación examinó la implementación completa de `translate` explorando seis fuentes principales: la definición del parser CLI (`cli.py:2750-2759`), el handler `cmd_translate` (`cli.py:1045-1096`), el orquestador `TranslationService` (`translation/service.py`), el traductor `MarianTranslator` (`translation/translator.py`), el segmentador `SentenceSegmenter` (`translation/segmenter.py`), el ensamblador `SegmentAssembler` (`translation/assembler.py`), el loader `TranslationModelLoader` con la resolución de idiomas (`translation/model_loader.py`), y los códigos de salida (`exit_codes.py`). Se leyeron también las excepciones de dominio (`exceptions.py`) y la función `emit_json` (`cli.py:69-80`). No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `translate`:** Es un comando standalone de texto→texto que orquesta un pipeline de cuatro etapas (validar → segmentar → traducir → ensamblar) con un atajo de passthrough cuando origen y destino coinciden. No invoca audio ni motor TTS; delega al daemon (`POST /translate` con CT2 residente, `crates/avi-daemon/src/lib.rs:553`) cuando está activo (3 modos, `src/main.rs:398`), o ejecuta CT2 local si no.

**Implementación:** El handler `cmd_translate` (`cli.py:1045`) instancia el pipeline completo con colaboradores concretos (`TranslationModelLoader`, `SentenceSegmenter`, `MarianTranslator`, `SegmentAssembler`) y delega la traducción a `TranslationService.translate`. Las tres excepciones de dominio (`TranslationModelMissingError`, `UnsupportedLanguagePairError`, `TranslationFailedError`) se mapean a códigos de salida existentes.

**Divergencia ISO vs CLI:** `translate` acepta códigos ISO (`es`/`en`), no la taxonomía CLI (`es-latam`/`en`). El resto de la CLI usa `es-latam`. Esta divergencia es deliberada (ratificada como D5): el traductor MarianMT solo entiende ISO 639-1, y `resolve_language` normaliza internamente.

---

## Hallazgos por tema

### Definición CLI

El parser de `translate` se define en `cli.py:2750-2759` como subcomando de la CLI:

| Parámetro | Tipo | Requerido | Descripción |
|---|---|---|---|
| `--text` | str | Sí | Texto a traducir |
| `--from` | `es` \| `en` | Sí | Idioma de origen (ISO) |
| `--to` | `es` \| `en` | Sí | Idioma destino (ISO) |
| `--json` | flag | No | Emitir JSON legible por máquina |

**Detalle de implementación:** `--from` se almacena como `from_lang` y `--to` como `to_lang` via `dest=` (`cli.py:2754, 2756`) para evitar colisión con la palabra reservada Python `from`.

### Handler: cmd_translate

El handler (`cli.py:1045-1096`) ejecuta este flujo:

1. **Validación de entrada:** texto no vacío (exit 2) y longitud ≤ `MAX_TEXT_LENGTH` (5000 chars, exit 2) — `cli.py:1050-1060`
2. **Construcción del pipeline:** instancia colaboradores concretos — `cli.py:1073-1076`
3. **Traducción:** `service.translate(text, source, target)` — `cli.py:1079`
4. **Salida:** `emit_json` si `--json`, `print` si no — `cli.py:1093-1096`

### Pipeline de traducción

`TranslationService.translate` (`translation/service.py:35-54`) orquesta cuatro etapas:

```
Texto de entrada
    │
    ▼
origen == destino? ──sí──► return texto (passthrough, sin modelo)
    │ no
    ▼
resolve_language(origen/destino)        ← normaliza "es-latam" → "es"
    │
    ▼
model_loader.load(cache_dir)           ← fail-fast: modelo existe?
    │
    ▼
segmenter.segment(texto, source)       ← list[list[str]]: párrafos → segmentos
    │
    ▼
translator.translate(segment, ...)     ← por cada segmento, inferencia CT2
    │
    ▼
assembler.assemble(translated)         ← reensambla texto destino
```

### Passthrough: source == target

Cuando `origen == destino`, `TranslationService.translate` devuelve el texto intacto sin cargar ningún modelo (`service.py:38-39`). Esto es explícito en el docstring del handler: "`--from == --to` es passthrough" (`cli.py:1047`).

### Segmentación jerárquica (4 niveles)

`SentenceSegmenter` (`segmenter.py:25-99`) particiona el texto en segmentos que no exceden `max_length` (default 512 caracteres) siguiendo una jerarquía de 4 niveles:

| Nivel | Método | Descripción |
|---|---|---|
| 1 | `text.split("\n\n")` | Párrafos separados por línea en blanco |
| 2 | `pysbd.Segmenter.segment()` | Oraciones (depende del idioma) |
| 3 | `_STRONG_PUNCTUATION.split()` | Puntuación fuerte: `,;:` seguidos de espacio |
| 4 | `text.split(" ")` o tokenizer inyectado | Tokens como último recurso |

Cada nivel solo se aplica si el fragmento del nivel anterior excede `max_length` (`segmenter.py:49-50, 54-57`). Si un párrafo completo cabe en ≤ 512 chars, se deja intacto. Si una oración excede, se divide por puntuación fuerte. Si eso no basta, se agrupan tokens.

**Detalle de implementación:** los segmentadores `pysbd` se cachean por idioma en `_pysbd_segmenters` (`segmenter.py:39, 61-64`). La función `_token_split` (`segmenter.py:81-98`) agrupa tokens sin perder texto, calculando longitud acumulada incluyendo espacios.

### Modelo Marian CT2

`_MarianCT2Model` (`model_loader.py:47-84`) envuelve `ctranslate2.Translator` con tokenización SentencePiece:

- **Carga:** importa `ctranslate2` y `sentencepiece` de forma diferida (dentro del `__init__`) para no arrastrar librerías pesadas en comandos que no traducen (`model_loader.py:90-93`)
- **Tokenización:** `source.spm` para tokenizar la entrada, `target.spm` para detokenizar la salida (`model_loader.py:65-70`)
- **Token `</s>`:** Se añade manualmente al final de los tokens fuente (`model_loader.py:81`). Sin este token, el encoder nunca recibe marca de fin de secuencia y el decoder entra en loop de repetición (`model_loader.py:73-79`)
- **Inferencia:** `translate_batch([tokens])` → `results[0].hypotheses[0]` → detokenización (`model_loader.py:82-84`)

### Caché de modelos

`TranslationModelLoader` (`model_loader.py:98-132`) cachea modelos cargados en un diccionario interno (`_cache`). La clave es la ruta absoluta del directorio. La ruta por defecto es `{data_root}/translation-models/opus-mt-{source}-{target}` (`service.py:18-22`).

### Divergencia ISO vs taxonomía CLI

El resto de la CLI usa `es-latam` / `en` como identificadores de idioma. `translate` usa `es` / `en` (ISO 639-1). Esta divergencia se resuelve en la capa de carga del modelo:

- `_LANGUAGE_ALIASES = {"es-latam": "es"}` (`model_loader.py:23`)
- `resolve_language()` normaliza `es-latam` → `es`, cualquier otro valor se devuelve intacto (`model_loader.py:26-32`)

La definición del parser acepta solo `["es", "en"]` como choices (`cli.py:2754, 2756`), por lo que `es-latam` no es válido como argumento de `--from`/`--to`. Esta es la divergencia deliberada D5: el traductor MarianMT solo opera con ISO 639-1.

### Contrato JSON

Cuando `--json` está activo, `emit_json` (`cli.py:69-80`) emite un único objeto JSON a stdout con:

| Campo | Tipo | Descripción |
|---|---|---|
| `translated` | str | Texto traducido |
| `source` | str | Idioma origen (ISO: `es` o `en`) |
| `target` | str | Idioma destino (ISO: `es` o `en`) |
| `schema_version` | str | Versión del schema (inyectada por `emit_json`) |

**Contraste con `speech transcribe`:** este último emite `source` con el token CLI verbatim (`es-latam`), no el ISO resuelto (`cli.py:1120-1122`). `translate` emite el ISO porque así fue ratificado en D5.

### Manejo de errores

| Excepción | Código exit | Reason | Mensaje / Acción |
|---|---|---|---|
| Texto vacío | 2 | `usage_error` | "Error: --text no puede estar vacío." |
| Texto > 5000 chars | 2 | `usage_error` | "Error: el texto tiene N caracteres; el máximo..." |
| `TranslationModelMissingError` | 4 | `model_missing` | "Ejecuta 'ai-voice-interconnector setup --language en' primero." |
| `UnsupportedLanguagePairError` | 2 | `usage_error` | "Error: {e}" |
| `TranslationFailedError` | 9 | `translation_failed` | "Error: {e}" |

**Mapeo de códigos** (`exit_codes.py`): `EXIT_INVALID_INPUT=2`, `EXIT_MODEL_MISSING=4`, `EXIT_TRANSLATION_FAILED=9`.

### Ensamblado

`SegmentAssembler.assemble` (`assembler.py:16-20`) une los segmentos de cada párrafo con un espacio (`" ".join(segments)`) y los párrafos entre sí con una línea en blanco (`"\n\n".join(...)`), preservando exactamente el separador que `SentenceSegmenter` usa para partir el texto de entrada (`segmenter.py:45`).

---

### Despacho al daemon (T5)

`translate` es delegable en 3 modos (`--daemon`/`--no-daemon`/auto) vía `handle_translate` (`src/main.rs:398`) + `translate_via_daemon` (timeout 1500ms) → `POST /translate` (`crates/avi-daemon/src/lib.rs:580` `translate_handler`) con CT2 residente (`DaemonState:ct2_engine` `Option<HashMap>`). Passthrough `source==target` sin motor; `unsupported_language_pair`/`empty_text` validaciones puras antes del despacho.

## Conclusiones

`translate` es un comando standalone de texto→texto que implementa un pipeline de traducción `es<->en` sin dependencias de audio ni motor TTS, ahora delegable al daemon con CT2 residente. Su diseño es notable por: (1) la arquitectura de colaboradores inyectables — `TranslationService` recibe loader, segmenter, translator y assembler via constructor, lo que permite tests sin runtime CT2; (2) la segmentación jerárquica de 4 niveles (párrafos → oraciones → puntuación → tokens) que adapta dinámicamente textos largos al límite de ~512 tokens de MarianMT sin romper oraciones innecesariamente; (3) el manejo explícito del token `</s>` en el encoder que previene loops de repetición en el decoder; (4) la divergencia ISO vs taxonomía CLI como decisión deliberada (D5) — `translate` usa códigos ISO porque MarianMT solo opera con ISO 639-1, mientras el resto de la CLI usa `es-latam` para síntesis; y (5) el atajo de passthrough que evita cargar modelos cuando origen y destino coinciden, manteniendo la interfaz uniforme.
