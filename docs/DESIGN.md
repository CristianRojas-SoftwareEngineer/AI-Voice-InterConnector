# Diseño del Sistema TTS Sidecar con Chatterbox Multilingual V3

## Tabla de contenidos

- [Resumen ejecutivo](#resumen-ejecutivo)
- [Arquitectura](#arquitectura)
- [Estructura del proyecto](#estructura-del-proyecto)
- [El entry point `bin/tts-sidecar`](#el-entry-point-bintts-sidecar)
- [Motor Chatterbox Multilingual V3](#motor-chatterbox-multilingual-v3)
- [Traducción cross-lingual (opus-mt / CTranslate2)](#traducción-cross-lingual-opus-mt--ctranslate2)
- [Flujo de síntesis](#flujo-de-síntesis)
- [Modelo de voces de dos niveles](#modelo-de-voces-de-dos-niveles)
- [Comandos CLI](#comandos-cli)
- [Compilación PyInstaller](#compilación-pyinstaller)
- [Extensibilidad](#extensibilidad)
- [Warnings silenciados](#warnings-silenciados)
- [Referencias](#referencias)

## Resumen ejecutivo

TTS Sidecar es un motor de síntesis de voz (TTS) **100% local** que usa **Chatterbox Multilingual V3** para clonación de voz en español latinoamericano. El usuario puede clonar su propia voz a partir de ~10 segundos de audio y generar narración de alta calidad.

- **Licencia**: GPL-3.0-or-later (código del proyecto); el modelo y las dependencias conservan sus licencias permisivas (MIT/BSD/Apache), salvo el par de traducción `opus-mt` (CC-BY-4.0)
- **Idiomas**: 23+ incluyendo Español (es)
- **Clonación**: `speech-reference.wav` obligatorio (≥10 segundos); `timbre-reference.wav` opcional para separar timbre y prosodia
- **Parámetros del modelo**: 500M
- **Hardware**: CPU, CUDA, MPS (Apple Silicon)

---

## Arquitectura

```
┌─────────────────────────────────────────────────────────────┐
│              tts-sidecar (binario CLI)                     │
│   Instalador por SO (Windows, Linux, macOS)                │
│   Bundle PyInstaller --onedir con intérprete embebido      │
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│           Chatterbox Multilingual V3                         │
│   Modelo: es-mx-latam (caché de HuggingFace)              │
│   Licencia: MIT                                           │
│   Idiomas: 23+ (español, inglés, francés, etc.)            │
│   Inferencia: CPU / CUDA / MPS                            │
└─────────────────────────────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│           Reproducción de audio (APIs nativas)              │
│   Windows: winsound (integrado; pycaw enumera)           │
│   Linux: sounddevice (PortAudio)                         │
│   macOS: afplay (nativo; sounddevice enumera)            │
└─────────────────────────────────────────────────────────────┘
```

## Estructura del proyecto

```
TTS-Sidecar/
├── src/
│   └── tts_sidecar/           # Paquete Python
│       ├── __init__.py            # Imports perezosos (lazy)
│       ├── __main__.py            # Entry point de `python -m tts_sidecar`
│       ├── bootstrap.py           # apply() idempotente: warnings, env vars, logging, mock pkg_resources
│       ├── engine.py              # Façade / composition root de síntesis
│       ├── compute_backend.py     # ComputeBackendResolver: detección/resolución de backend (cuda/mps/cpu)
│       ├── audio_writer.py        # AudioWriter: audio → bytes WAV PCM 16-bit mono
│       ├── synthesis.py           # SynthesisOrchestrator: flujo speech synthesize (conditionals → generate → encode → save)
│       ├── model_loader.py        # ModelLoader: carga del checkpoint según caché (inyectable)
│       ├── conditionals.py        # ConditionalsPreparer: cómputo/carga de conditionals (inyectable)
│       ├── exceptions.py          # Excepciones compartidas del motor y del daemon (sin imports pesados)
│       ├── audio.py               # Reproducción de audio multiplataforma
│       ├── timing.py              # Instrumentación y timing
│       ├── cli.py                 # Interfaz CLI
│       ├── exit_codes.py          # Códigos de salida del CLI — contrato público congelado
│       ├── voices.py              # Resolución de voces usuario→fábrica
│       ├── synthetic_speech.py    # Almacén de habla sintética grabada por `speech` (WAV + sidecar JSON)
│       ├── paths.py               # Rutas por SO (user-data-dir, modo congelado)
│       ├── model_cache.py         # Detección de los modelos en la caché de HF
│       ├── translation/           # Subsistema de traducción cross-lingual es<->en
│       │   ├── service.py         # TranslationService: orquesta segmentar → traducir → ensamblar
│       │   ├── segmenter.py       # SentenceSegmenter (pysbd)
│       │   ├── translator.py      # MarianTranslator (runtime CT2)
│       │   ├── model_loader.py    # TranslationModelLoader + resolve_language
│       │   └── assembler.py       # SegmentAssembler
│       ├── transcription/         # Subsistema de transcripción STT (audio→texto, faster-whisper)
│       │   ├── service.py         # TranscriptionService: orquesta resolver idioma → cargar modelo → leer WAV → transcribir
│       │   ├── model_loader.py    # WhisperModelLoader
│       │   └── transcriber.py     # WhisperTranscriber
│       ├── voices/                # Voces de FÁBRICA (commiteadas, empaquetadas, solo lectura)
│       │   └── default/           # Voz por defecto (derivada de assets/audios/)
│       │       ├── timbre-reference.wav # Timbre de voz (cualquier largo)
│       │       └── speech-reference.wav # Conditioning (10s+)
│       └── daemon/                # Daemon mode (FastAPI + IPC)
│           ├── __init__.py        # Ensambla las exportaciones públicas del paquete daemon
│           ├── daemon.py          # Gestor del ciclo de vida
│           ├── server.py          # Endpoints FastAPI
│           ├── ipc.py             # Cliente HTTP del daemon
│           ├── protocol.py        # Modelos Pydantic
│           └── run.py             # Entry point
│   # Las voces de USUARIO viven en el user-data-dir por SO, no en el repo
├── bin/
│   └── tts-sidecar               # Script de entry point
├── scripts/
│   ├── build_windows.py          # Build PyInstaller para Windows
│   ├── build_linux.py            # Build PyInstaller para Linux
│   ├── build_macos.py            # Build PyInstaller para macOS
│   ├── build_utils.py            # Utilidades compartidas de build
│   ├── check_coverage.py         # Verificación de cobertura
│   ├── check_third_party_licenses.py  # Verificación de licencias
│   ├── clean_build.py            # Limpieza de artefactos de build
│   ├── create_installer_windows.py    # Generador de instalador Inno Setup
│   ├── pyinstaller_wrapper.py    # Wrapper de PyInstaller
│   ├── render_cask.py            # Generador de Cask de Homebrew
│   └── render_source_offer.py    # Generador de SOURCE-OFFER.md
│                                  # (provisión del modelo: `tts-sidecar setup`)
├── assets/                       # Material fuente (audios, logo)
│   ├── audios/                   # Audios fuente (voz default) y de prueba
│   │   ├── Voice Sampler.wav
│   │   └── Speech Sampler.wav
│   └── images/                   # Logo del proyecto (fuente de los iconos de build)
│       └── TTS Sidecar - Logo.png
├── tests/                        # Pytest test suite
├── requirements.txt               # Python dependencies
├── pyproject.toml                # Python project config
└── docs/
    ├── DESIGN.md                 # Este documento
    ├── GOAL.md                   # Meta del proyecto
    ├── ROADMAP.md                # Roadmap del proyecto
    ├── DAEMON-MODE.md            # Daemon mode
    ├── BUILD.md                  # Build y distribución
    ├── DISTRIBUTION.md           # Estrategia de distribución
    ├── PARITY.md                 # Paridad multiplataforma
    ├── RELEASING.md              # Proceso de release
    ├── SELF-HOSTED-INSTALL.md    # Instaladores auto-hospedados
    ├── CLI/                      # Documentación de la CLI
    │   ├── README.md             # Índice de la CLI
    │   ├── CONTRACT.md           # Contrato normativo
    │   └── commands/             # Investigación por comando
    └── proposals/                # Propuestas de diseño
```

> El modelo `es-mx-latam` no vive en el repo ni en el bundle: reside en la caché
> de HuggingFace del usuario (`~/.cache/huggingface/hub`) tras `tts-sidecar setup`.

## El entry point `bin/tts-sidecar`

El archivo `bin/tts-sidecar` es el **punto de entrada único** de la aplicación. Está escrito en **Python 3**, pero deliberadamente **no lleva extensión `.py`**:

- **Convención de comando CLI**: el objetivo del proyecto es exponer una herramienta invocable como `tts-sidecar speech say ...`, no como `tts-sidecar.py speak ...`. Los comandos de terminal no llevan extensión (igual que `git`, `node` o `pip`), de modo que el archivo se nombra como el comando final que representa.
- **Shebang en vez de extensión**: la primera línea es `#!/usr/bin/env python3`. En Linux/macOS, con el bit de ejecución activo (`chmod +x`), el sistema operativo lee esa línea para saber con qué intérprete ejecutarlo; la extensión `.py` solo orienta a editores y humanos, el SO nunca la necesita. Por eso `./tts-sidecar speech say ...` funciona sin nombrar a Python.
- **Invocación en desarrollo bajo Windows**: Windows ignora el shebang, así que en desarrollo el entry point se invoca explícitamente a través del intérprete: `python bin/tts-sidecar speech say --text "Hola"`.

El archivo no contiene lógica de negocio: prepara el entorno (silencia warnings, ajusta `sys.path`, parchea `pkg_resources` para Python 3.13+) y delega en `tts_sidecar.cli.main`. Además es la **semilla de compilación** que reciben los scripts de `scripts/build_*.py`: PyInstaller lo toma como entrada y produce el bundle final. Véase `docs/BUILD.md`.

## Motor Chatterbox Multilingual V3

| Aspecto | Detalle |
|---------|---------|
| **Modelos** | `es-mx-latam` (`ResembleAI/Chatterbox-Multilingual-es-mx-latam`, ~4 GB) y `en` (`ResembleAI/chatterbox`, ~3.2 GB descargados vía `allow_patterns`) |
| **Licencia** | MIT |
| **Parámetros** | 500M |
| **Idiomas** | 23+ (es, en, fr, de, pt, etc.) |
| **Clonación de voz** | `speech-reference.wav` obligatorio (≥10s); `timbre-reference.wav` opcional (dual-audio como optimización de timbre y prosodia por separado) |
| **Inferencia** | CPU, CUDA, MPS |

## Traducción cross-lingual (opus-mt / CTranslate2)

Subsistema independiente de texto→texto (`src/tts_sidecar/translation/`) que
traduce `es<->en` antes de la síntesis (flag opcional `--source-language` en
`speech say`/`synthesize`) o de forma aislada (comando `translate`, sin voz ni
motor TTS de por medio). No forma parte del motor Chatterbox: es una etapa
previa y opcional.

| Aspecto | Detalle |
|---------|---------|
| **Modelos** | `Helsinki-NLP/opus-mt-es-en` y `Helsinki-NLP/opus-mt-en-es` (proyecto OPUS-MT, Universidad de Helsinki) |
| **Licencia** | CC-BY-4.0 (atribución en [THIRD-PARTY-LICENSES.md](../THIRD-PARTY-LICENSES.md)) |
| **Runtime de inferencia** | CTranslate2 (CT2), embarcado; los pesos se convierten una sola vez con `ctranslate2.converters.TransformersConverter` (cuantización int8) durante `setup --language en/all` |
| **Segmentación** | `pysbd` (puro Python, MIT) — segmenta el texto en oraciones antes de traducir, para no exceder la ventana del modelo |
| **Ensamblado** | `SegmentAssembler` recompone las oraciones traducidas en un solo texto |
| **Provisión** | Fuera del bundle, igual que los modelos de síntesis: se descarga y convierte en `setup`, se verifica en `doctor`, se limpia con `cleanup --model` |
| **Passthrough** | `TranslationService.translate` devuelve el texto intacto sin cargar ningún modelo si origen == destino |

## Transcripción STT (faster-whisper / CTranslate2)

Subsistema independiente de audio→texto (`src/tts_sidecar/transcription/`) que
transcribe un archivo WAV vía la sub-acción `speech transcribe`, aislada del
motor Chatterbox y del subsistema de traducción. Espeja el patrón de
colaboradores inyectables de `translation/` (`ModelLoader` con factory
inyectable + caché por ruta, orquestador `Service` con fail-fast al cargar el
modelo antes de decodificar el audio). Whisper **solo transcribe**
(`task="transcribe"`), nunca traduce: si el usuario necesita el texto en otro
idioma, encadena `translate` por separado.

| Aspecto | Detalle |
|---------|---------|
| **Modelo** | `Systran/faster-whisper-small` (conversión CTranslate2 de `openai/whisper-small`), ya en formato CT2 — sin paso de conversión propio, a diferencia de los modelos de traducción |
| **Licencia** | MIT |
| **Runtime de inferencia** | CTranslate2 (CT2), el mismo runtime embarcado que usa `translation/`; sin runtime nuevo |
| **Resolución de idioma** | Reutiliza `resolve_language` de `translation/model_loader.py` (`es-latam`→`es`) — un solo idioma por invocación, no un par |
| **Lectura de audio** | `_default_audio_reader` (`transcription/service.py`) decodifica el WAV con `wave` de la stdlib (sin PyAV), downmixea a mono si es estéreo y normaliza int16→float32 |
| **Remuestreo a 16 kHz** | `faster_whisper.WhisperModel.transcribe` asume 16 kHz cuando recibe un `np.ndarray`; a otra frecuencia (44.1/48/8 kHz) la transcripción degrada a texto ininteligible — verificado empíricamente. `_default_audio_reader` remuestrea siempre a `WHISPER_SAMPLE_RATE = 16000` con `_resample_to_whisper_rate` (interpolación lineal vía `numpy.interp`, no-op si el WAV ya está a 16 kHz), sin añadir ninguna dependencia nueva |
| **Provisión** | Fuera del bundle, igual que los demás modelos: se descarga con `setup --with-stt` (opt-in, ortogonal a `--language`), se verifica en `doctor`, se limpia con `cleanup --model` |
| **Passthrough** | No aplica (N/A): a diferencia de la traducción, no existe un caso "origen == destino" que evite cargar el modelo — toda invocación transcribe |

## Flujo de síntesis

```
1. El usuario ejecuta: tts-sidecar speech say --text "Hola" -v mi_voz
                    │
                    ▼
2. La CLI parsea argumentos y carga ChatterboxEngine
                    │
                    ▼
3. ChatterboxTTS.generate(text, language=es,
       timbre-reference.wav → Voice Encoder (timbre, opcional),
       speech-reference.wav    → T3 conditioning + S3Gen decoder
       (sin timbre-reference.wav, speech-reference.wav cubre también el Voice Encoder))
                    │
                    ▼
4. El modelo produce audio WAV (24kHz, mono)
                    │
                    ▼
5. AudioPlayer.play() → API de audio nativa del SO
                    │
                    ▼
6. El usuario escucha el habla en español con la voz clonada
```

## Modelo de voces de dos niveles

Las voces se separan en dos orígenes y se resuelven por nombre con precedencia
**usuario→fábrica** (`voices.py`):

- **Fábrica**: `src/tts_sidecar/voices/`, versionadas y empaquetadas en el
  ejecutable vía `--add-data`; de solo lectura. Se resuelven en
  `paths.bundled_voices_dir()`, siempre relativa al paquete: en modo fuente y
  pip/uv-installed es `tts_sidecar/voices/` dentro del árbol del paquete; en
  modo congelado (PyInstaller) es el mismo subdirectorio dentro de
  `sys._MEIPASS`. Incluye la voz `default`, derivada de `assets/audios/`.
- **Usuario**: `data_root()/voices` (user-data-dir por SO; escribible),
  registradas con `voice clone`. Una voz de usuario homónima sobrescribe a la de
  fábrica.

Sin `--voice` ni audios explícitos, la CLI usa la voz `default`, de modo que
`tts-sidecar speech synthesize --text "Hola" --label NUEVA` funciona sin registrar nada.

## Comandos CLI

La referencia de comandos y flags no vive aquí para evitar deriva: el manual de
usuario ([USAGE.md](../USAGE.md)) documenta cada comando y su uso, y el contrato
normativo ([CONTRACT.md](CLI/CONTRACT.md)) fija la superficie estable (exit
codes, esquema `--json`, payloads). La invocación desde otros lenguajes
(`subprocess`, `child_process`, `std::process`) está en el
[README](../README.md#invocación-desde-cualquier-lenguaje).

## Compilación PyInstaller

```bash
# Windows
python scripts/build_windows.py

# Linux
python scripts/build_linux.py --arch x86_64
python scripts/build_linux.py --arch arm64

# macOS (Apple Silicon)
python scripts/build_macos.py --arch arm64
```

## Extensibilidad

Para añadir un nuevo motor TTS:

1. Crear nuevo módulo en `src/tts_sidecar/`
2. Mantener la misma interfaz CLI en `cli.py`
3. Re-empaquetar con PyInstaller para cada plataforma

## Warnings silenciados

`src/tts_sidecar/bootstrap.py` (`apply()`) silencia mediante una **allow-list explícita**
(`_SILENCED_WARNINGS`), **no** un catch-all global `warnings.filterwarnings("ignore")`
ni `PYTHONWARNINGS=ignore` (para no enmascarar deprecaciones propias
ni de terceros). La allow-list acota solo dos warnings benignos del módulo `warnings`:

- `pkg_resources is deprecated` — por **mensaje**; lo emite `perth` al importar
  `pkg_resources` en Python 3.13. Con `category=Warning` (no `DeprecationWarning`)
  porque `perth` lo emite como `UserWarning` en este entorno; así queda acotado por
  mensaje y cubre ambas categorías.
- `diffusers LoRACompatibleLinear` — por **módulo** (`r"^diffusers\."`), al importar
  `chatterbox`, para no atarse al texto exacto del mensaje.

Las tres supresiones siguientes son de `logging` (no las gobierna el catch-all) y se
conservan intactas:
- `huggingface_hub` HTTP warnings
- `chatterbox.models.tokenizers.tokenizer` pkuseg
- `chatterbox.models.t3.inference.alignment_stream_analyzer` repetition

---

## Referencias

- [Chatterbox TTS - Resemble AI](https://huggingface.co/ResembleAI/chatterbox-multilingual)
- [PyInstaller - Python to Executable](https://pyinstaller.org/)
- [Chatterbox GitHub](https://github.com/resemble-ai/chatterbox)
