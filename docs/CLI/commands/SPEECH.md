## Recorrido

La investigación examinó la implementación completa de `speech dub` explorando tres fuentes principales: el handler CLI (`cli.py:559-620`), el daemon FastAPI (`daemon/server.py`), y los subsistemas de transcripción, traducción y engine. Se leyeron en paralelo las implementaciones del handler principal, los helpers de despacho tri-modal (`_transcribe_stage`, `_dispatch_synthesis`), los endpoints del daemon, los protocolos compartidos (`protocol.py`), y los módulos de transcription/translation/engine/audio. No hubo desviaciones del plan ni fuentes faltantes.

---

## Respuestas a los objetivos

**Diseño de `speech dub`:** Es un pipeline de cuatro etapas (transcribe → traduce → sintetiza → reproduce) orquestado íntegramente en el lado del CLI. El daemon no expone un endpoint `/dub`; en su lugar, el CLI compone dos llamadas atómicas (`/transcribe` y `/synthesize`) en secuencia. La traducción está incrustada dentro de `/synthesize` cuando `source_language != target_language`.

**Implementación:** El handler `cmd_speech_dub` (line 560) reutiliza helpers compartidos con `speech transcribe` y `speech synthesize` — no existen funciones auxiliares específicas de dub. El despacho tri-modal (daemon explícito / autodetección / directo) se aplica tanto a la transcripción como a la síntesis de forma independiente.

**Proceso de ejecución:** Validación de inputs → captura de audio (archivo WAV o micrófono) → transcripción vía Whisper → validación del texto transcrito → traducción condicional → síntesis con la voz target → reproducción del resultado.

---

## Hallazgos por tema

### Pipeline de ejecución

`cmd_speech_dub` ejecuta este flujo exacto (`cli.py:559-620`):

```
Entrada (archivo WAV o micrófono)
    │
    ▼
_transcribe_stage(args)              ← 3 modos: --daemon / --no-daemon / auto
    │                                  → POST /transcribe (daemon) o modo directo
    ▼
texto transcrito
    │
    ▼
_validate_synthesis_text(text)       ← validación de longitud y contenido
    │
    ▼
args.text = text                     ← reemplaza args.text con la transcripción
_dispatch_synthesis(args, voice)     ← 3 modos: --daemon / --no-daemon / auto
    │                                  → POST /synthesize (daemon) o modo directo
    │                                  → traducción integrada si source ≠ target
    ▼
result (SynthesisResult: audio_bytes + metrics)
    │
    ▼
_play_audio(result.audio_bytes)      ← reproducción inmediata
```

### Despacho tri-modal

Tanto `_transcribe_stage` (`cli.py:451-513`) como `_dispatch_synthesis` (`cli.py:361-418`) implementan el mismo patrón de tres modos:

| Flag | Comportamiento |
|---|---|
| `--daemon` | Exige daemon activo; si no responde, exit 5 |
| `--no-daemon` | Fuerza modo directo, sin sondear daemon |
| Sin flags | Autodetección: daemon si responde, directo si no |

**Invariante:** la captura/lectura del audio SIEMPRE ocurre en el cliente. El daemon nunca recibe rutas de archivo; recibe muestras PCM int16 codificadas en base64 (`_transcribe_via_daemon`, `cli.py:421-448`).

### Parámetros requeridos

| Parámetro | Tipo | Descripción |
|---|---|---|
| `--audio` **o** `--mic` | exclusive group | Archivo WAV o grabación desde micrófono (uno u otro, requerido) |
| `--source-language` | `es-latam` \| `en` | Idioma hablado en el audio de entrada |

### Parámetros opcionales

| Parámetro | Tipo | Default | Descripción |
|---|---|---|---|
| `--duration` | int | None | Duración fija de grabación en segundos (solo con `--mic`) |
| `--target-language` | `es-latam` \| `en` | `es-latam` | Idioma/modelo de síntesis; si difiere de source, se traduce |
| `--voice, -v` | str | `default` | Nombre de la voz a usar |
| `--compute-backend, -cb` | `auto\|cpu\|cuda\|mps` | `auto` | Backend de cómputo (solo en ruta directa) |
| `--exaggeration` | float | None | Override de expresividad emocional |
| `--cfg-weight` | float | None | Override de guidance (no permite 0.0) |
| `--temperature` | float | None | Override de temperatura |
| `--daemon` | flag | False | Exige daemon |
| `--no-daemon` | flag | False | Fuerza modo directo |

### Etapa 1: Captura de audio

**Modo archivo** (`cli.py:573-576`): valida existencia del WAV con `Path.exists()`.

**Modo micrófono** (`cli.py:568-571`): usa `AudioRecorder` (`audio.py`) que captura a 16 kHz mono int16 vía `miniaudio.CaptureDevice`. Dos modos:
- `record_until_enter()`: push-to-talk, Enter para terminar (requiere TTY)
- `record_fixed(seconds)`: grabación de duración fija

**Restricción:** `--mic` sin TTY y sin `--duration` produce exit 2. `--duration` sin `--mic` también produce exit 2.

### Etapa 2: Transcripción

`_transcribe_stage` (`cli.py:451-513`) despacha a:

- **Daemon:** `_transcribe_via_daemon` (`cli.py:421-448`) → codifica audio a base64 PCM int16 → `POST /transcribe` → daemon decodifica, convierte a float32, delega a `WhisperTranscriber` → `faster_whisper.WhisperModel.transcribe(audio, language, task="transcribe")` → texto
- **Directo:** instancia `TranscriptionService` localmente → mismo pipeline sin HTTP

**Detalle clave:** `task="transcribe"` está hardcodeado — Whisper solo transcribe, nunca traduce. La traducción es responsabilidad exclusiva del subsistema de traducción.

### Etapa 3: Traducción (condicional)

La traducción NO es una etapa separada en `cmd_speech_dub`. Está incrustada en `_dispatch_synthesis` → `_translate_stage` (`cli.py:192-223`) en modo directo, o dentro del endpoint `POST /synthesize` del daemon (`server.py:284-289`).

**Condición:** solo se traduce si `source_language != target_language`. Si son iguales, el texto pasa sin modificar (shortcut en `TranslationService.translate`, `translation/service.py`).

**Pipeline de traducción** (`translation/`):
1. `SentenceSegmenter` divide en párrafos → oraciones → puntuación fuerte → tokens (4 niveles jerárquicos para el límite de ~512 tokens de MarianMT)
2. `MarianTranslator` carga modelo CT2 (`opus-mt-{src}-{dst}`) y traduce por lotes, añadiendo `</s>` a los tokens fuente para evitar loops infinitos
3. `SegmentAssembler` reconstruye el texto preservando saltos de párrafo

**Idiomas soportados:** conjunto cerrado `{es, en}`.

### Etapa 4: Síntesis

`_dispatch_synthesis` (`cli.py:361-418`) despacha a:

- **Daemon:** `_synthesize_via_daemon` → `POST /synthesize` con `SynthesizeRequest` → daemon selecciona engine por idioma, serializa con `_synthesis_lock`, emite NDJSON (progress events + result), limpieza de memoria post-síntesis
- **Directo:** instancia `ChatterboxEngine.get_instance(model, backend)` → lazy imports de torch/chatterbox → `engine.synthesize(text, timbre_ref, speech_ref, ...)`

**Parámetros de síntesis del engine** (`engine.py`):

| Parámetro | Default es-latam | Default en | Efecto |
|---|---|---|---|
| `exaggeration` | 0.75 | 0.65 | Expresividad emocional |
| `cfg_weight` | 0.5 | 0.3 | Classifier-free guidance |
| `temperature` | 0.8 | 0.7 | Muestreo |

**Optimizaciones monkey-patch** (`engine.py:_apply_synthesis_optimizations`):
- `tts.t3.inference` → inyecta `max_new_tokens=500` (vs default 1000), mide timing T3, limpia hooks de `AlignmentStreamAnalyzer` (previene memory leak)
- `tts.s3gen.inference` → inyecta `n_cfm_timesteps=4` (vs default 10), mide timing S3Gen
- `tts.tqdm` → wrapper con throttling (~10 eventos/s, cada 10 tokens) para progress callback

**Watermark (PerthNet):** deshabilitado completamente — `tts.watermarker.apply_watermark` reemplazado por noop.

### Etapa 5: Reproducción

`_play_audio` (`cli.py:126-138`) instancia `AudioPlayer` (vía `miniaudio`) y reproduce los bytes de audio resultantes. Fallo si la librería de audio no está disponible → exit 8.

### Daemon: no hay endpoint `/dub`

El daemon (`daemon/server.py`) expone 6 endpoints, ninguno es de dubbing:

| Endpoint | Propósito |
|---|---|
| `GET /health` | Estado del daemon, modelos cargados, uptime |
| `POST /synthesize` | Síntesis (con traducción integrada si source≠target) |
| `POST /transcribe` | Transcripción vía faster-whisper |
| `GET /voices` | Lista de voces |
| `POST /voices/precompute` | Precomputa conditionals de una voz |
| `POST /shutdown` | Apaga el daemon |

El CLI compone `dub` orquestando `/transcribe` + `/synthesize` en secuencia.

### Validaciones previas a la síntesis

Antes de llegar a `_dispatch_synthesis`, `cmd_speech_dub` ejecuta (`cli.py:583-593`):

1. `_validate_synthesis_text(text)` — texto no vacío, ≤ `MAX_TEXT_LENGTH` (5000 chars), warning si > 2000 chars
2. `_validate_synthesis_params(args)` — `cfg_weight > 0.0`, `exaggeration >= 0.0`
3. `_validate_identifier(voice_name)` — normaliza y valida el nombre de voz
4. `_require_model_cached(model_for(target_language))` — verifica que el modelo esté descargado
5. `_require_voice_exists(voice_name)` — verifica que la voz exista (usuario o fábrica)

### Manejo de errores

| Excepción | Código exit | Razón |
|---|---|---|
| Archivo WAV no encontrado | 3 | `not_found` |
| Modelo no descargado | 4 | `model_missing` |
| Daemon inalcanzable | 5 | `daemon_unreachable` |
| `cfg_weight <= 0.0` | 2 | `usage_error` |
| `--duration` sin `--mic` | 2 | `usage_error` |
| Modelo transcripción faltante | 4 | `model_missing` |
| Fallo de transcripción | 10 | `transcription_failed` |
| `DaemonIPCError` | 5 | `daemon_unreachable` |
| Error genérico | 1 | `generic` |

---

## Conclusiones

`speech dub` es un pipeline de composición voz→voz que orquesta cuatro etapas (transcripción, traducción condicional, síntesis, reproducción) reutilizando los helpers compartidos de `speech transcribe` y `speech synthesize`. Su diseño es notable por: (1) no duplicar lógica — no existen `_dub_direct` ni `_dub_via_daemon`, el despacho tri-modal se reutiliza íntegramente; (2) el invariante sin-paths — el audio siempre se captura en el cliente y se envía como base64 PCM int16; (3) la traducción transparente — cuando `source_language != target_language`, la traducción ocurre automáticamente dentro de la etapa de síntesis sin intervención explícita del usuario; y (4) la separación de responsabilidades entre CLI (composición) y daemon (primitivas atómicas de transcripción y síntesis).

