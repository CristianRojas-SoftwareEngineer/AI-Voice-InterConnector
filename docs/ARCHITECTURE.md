# Arquitectura de TTS Sidecar

> **Licencia**: el código de TTS Sidecar se distribuye bajo GPL-3.0-or-later. El modelo
> Chatterbox y las dependencias empaquetadas conservan sus licencias permisivas
> (MIT/BSD/Apache); las menciones «Licencia: MIT» de este documento se refieren al **modelo**.
> Ver [THIRD-PARTY-LICENSES.md](../THIRD-PARTY-LICENSES.md).

## Tabla de contenidos

- [Visión General](#visión-general)
- [Principios de Diseño](#principios-de-diseño)
- [Modelos Disponibles](#modelos-disponibles)
- [Flujo de Síntesis](#flujo-de-síntesis)
- [Clonación de Voz](#clonación-de-voz)
- [Extensibilidad](#extensibilidad)

## Visión General

```
┌─────────────────────────────────────────────────────────────┐
│              tts-sidecar (binario ejecutable)              │
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
└──────────────────────┬──────────────────────────────────────┘
                       │
                       ▼
┌─────────────────────────────────────────────────────────────┐
│           Reproducción de audio (APIs nativas)             │
│   Windows: winsound (integrado; pycaw enumera)           │
│   Linux: sounddevice (PortAudio)                         │
│   macOS: afplay (nativo; sounddevice enumera)            │
└─────────────────────────────────────────────────────────────┘
```

## Principios de Diseño

### Python + PyInstaller

- **Python**: Motor TTS con Chatterbox
- **PyInstaller**: Empaqueta Python bytecode en ejecutable con interpreter embebido
- **Resultado**: Un solo archivo binario por SO, sin dependencias externas

### El entry point `bin/tts-sidecar`

El archivo `bin/tts-sidecar` es el **punto de entrada único** de la aplicación. Está escrito en **Python 3**, pero deliberadamente **no lleva extensión `.py`**:

- **Convención de comando CLI**: el objetivo del proyecto es exponer una herramienta invocable como `tts-sidecar speak ...`, no como `tts-sidecar.py speak ...`. Los comandos de terminal no llevan extensión (igual que `git`, `node` o `pip`), de modo que el archivo se nombra como el comando final que representa.
- **Shebang en vez de extensión**: la primera línea es `#!/usr/bin/env python3`. En Linux/macOS, con el bit de ejecución activo (`chmod +x`), el sistema operativo lee esa línea para saber con qué intérprete ejecutarlo; la extensión `.py` solo orienta a editores y humanos, el SO nunca la necesita. Por eso `./tts-sidecar speak ...` funciona sin nombrar a Python.
- **Invocación en desarrollo bajo Windows**: Windows ignora el shebang, así que en desarrollo el entry point se invoca explícitamente a través del intérprete: `python bin/tts-sidecar speak --text "Hola"`.

El archivo no contiene lógica de negocio: prepara el entorno (silencia warnings, ajusta `sys.path`, parchea `pkg_resources` para Python 3.13+) y delega en `tts_sidecar.cli.main`. Además es la **semilla de compilación** que reciben los scripts de `scripts/build_*.py`: PyInstaller lo toma como entrada y produce el bundle final. Véase `docs/BUILD.md`.

### Estructura del Proyecto

```
TTS-Sidecar/
├── src/
│   └── tts_sidecar/        # Paquete Python
│       ├── __init__.py         # Imports perezosos (lazy)
│       ├── engine.py           # Wrapper de ChatterboxTTS
│       ├── audio.py           # Reproducción de audio multiplataforma
│       ├── timing.py           # Instrumentación y timing
│       ├── cli.py             # Interfaz CLI
│       └── daemon/            # Daemon mode (FastAPI + IPC)
│           ├── daemon.py      # Gestor del ciclo de vida
│           ├── server.py      # Endpoints FastAPI
│           ├── ipc.py         # Cliente HTTP del daemon
│           ├── protocol.py    # Modelos Pydantic de request/response
│           └── run.py         # Entry point
├── bin/
│   └── tts-sidecar           # Entry point (Python, sin extensión; semilla de compilación)
├── scripts/
│   ├── build_windows.py      # Build PyInstaller para Windows
│   ├── build_linux.py       # Build PyInstaller para Linux
│   └── build_macos.py       # Build PyInstaller para macOS
│                             # (provisión del modelo: `tts-sidecar setup`)
├── tests/                    # Suite de tests pytest
├── requirements.txt           # Dependencias Python
├── pyproject.toml            # Configuración del proyecto Python
└── docs/
    ├── ARCHITECTURE.md
    ├── BUILD.md
    ├── DAEMON-MODE.md
    ├── DESIGN.md
    └── GOAL.md
```

## Modelos Disponibles

| Modelo | Descripción | Licencia |
|--------|-------------|----------|
| `es-mx-latam` | Español latinoamericano (RECOMENDADO) | MIT |

> El modelo `multilingual` es el modelo base de Chatterbox y forma parte del
> language pack. No está expuesto como opción del CLI: TTS Sidecar está
> especializado en español latinoamericano y usa siempre este modelo.

## Flujo de Síntesis

```
speak --text "Hola" -v mi_voz
       │
       ▼
┌──────────────────┐
│ ChatterboxEngine │
│ speak()         │
│                 │
│ 1. Cargar modelo│
│ 2. Generar     │
│ 3. Devolver WAV │
└────────┬────────┘
         │ bytes WAV
         ▼
┌──────────────────┐
│ AudioPlayer      │
│ play()          │
│                 │
│ API nativa del SO│
└─────────────────┘
```

## Clonación de Voz

Chatterbox permite clonar cualquier voz a partir de ~10 segundos de audio. Cada voz requiere dos archivos:

1. `voice clone --name mi_voz --timbre-reference timbre.wav --speech-reference condicion.wav` clona la voz
2. Los archivos se guardan en `voices/mi_voz/timbre-reference.wav` y `voices/mi_voz/speech-reference.wav`
   - `timbre-reference.wav` → Voice Encoder (timbre completo)
   - `speech-reference.wav` → T3 conditioning + S3Gen decoder (patrón de habla)
3. El clonado **precomputa los conditionals** de la voz a `voices/mi_voz/conditionals.pt`
   (con el modelo caliente del daemon si está activo, o cargándolo en frío si no),
   de modo que exige el modelo provisionado (`setup`). Un fallo del precómputo no
   aborta el clonado: los conditionals se computan en la primera síntesis.
4. `speak --text "..." -v mi_voz` usa la voz automáticamente, cargando los
   conditionals precomputados desde disco

## Extensibilidad

Para añadir un nuevo motor TTS:

1. Crear nuevo módulo en `src/tts_sidecar/`
2. Mantener la misma interfaz en `cli.py`
3. Re-empaquetar con PyInstaller para cada plataforma
