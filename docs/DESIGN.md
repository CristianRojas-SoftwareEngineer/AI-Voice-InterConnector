# Diseño del sistema AI Voice InterConnector (Rust + Qwen3-TTS)

## Tabla de contenidos

- [Resumen ejecutivo](#resumen-ejecutivo)
- [Arquitectura](#arquitectura)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Entry point `src/main.rs`](#entry-point-srcmainrs)
- [Motor Qwen3-TTS 0.6B (Rust)](#motor-qwen3-tts-06b-rust)
- [Traducción cross-lingual (opus-mt / CTranslate2)](#traducción-cross-lingual-opus-mt--ctranslate2)
- [Transcripción STT (Parakeet TDT v3 int8 / ort)](#transcripción-stt-parakeet-tdt-v3-int8--ort)
- [Flujo de síntesis](#flujo-de-síntesis)
- [UX de audio interactivo](#ux-de-audio-interactivo)
- [Salida en terminal](#salida-en-terminal)
- [Modelo de voces de dos niveles](#modelo-de-voces-de-dos-niveles)
- [Comandos CLI](#comandos-cli)
- [Compilación Rust (cargo)](#compilación-rust-cargo)
- [Extensibilidad](#extensibilidad)
- [Referencias](#referencias)

## Resumen ejecutivo

AI Voice InterConnector es un motor de síntesis de voz (TTS) **100% local** que usa **Qwen3-TTS 0.6B CustomVoice** para clonación de voz en español. El usuario puede clonar su propia voz a partir de ~10 segundos de audio y generar narración de alta calidad.

- **Licencia**: MPL-2.0 (código del proyecto, copyleft a nivel de archivo); modelo y dependencias conservan sus licencias (MIT/BSD/Apache), salvo traducción `opus-mt` (CC-BY-4.0)
- **Idiomas**: los 10 que soporta Qwen3-TTS, entre ellos Español (es)
- **Clonación**: un único audio de referencia, `--speech-reference` (~10 s recomendados, máximo 30 s)
- **Parámetros del modelo**: 0.6B (Qwen3-TTS)
- **Hardware**: CPU con BLAS (Accelerate en macOS, OpenBLAS en Linux y Windows) — inferencia local sin APIs externas

---

## Arquitectura

```
┌──────────────────────────────────────────────────────────────┐
│  CLI: ai-voice-interconnector (binario Rust, clap)           │
└───────────────┬───────────────────────────────┬──────────────┘
                │ despacho directo              │ despacho vía daemon
                │ (--no-daemon, o Auto sin      │ (--daemon, o Auto si
                │  daemon que responda)         │  GET /health responde)
                │                               ▼
                │               ┌──────────────────────────────────────┐
                │               │  Daemon (Axum, 127.0.0.1:8765)       │
                │               │  mismo binario con `daemon serve`;   │
                │               │  mantiene los motores calientes      │
                │               └───────────────┬──────────────────────┘
                ▼                               ▼
┌──────────────────────────────────────────────────────────────┐
│  Motores (en el proceso que despacha: CLI o daemon)          │
│                                                              │
│  ┌────────────────────────────────────────────────────────┐  │
│  │ TTS: Qwen3-TTS residente (subproceso nativo qwen_tts,  │  │
│  │ HTTP en loopback, puerto efímero)                      │  │
│  │  · voz preset  → qwen3-tts-0.6b (CustomVoice)          │  │
│  │  · voz clonada → qwen3-tts-0.6b-base (Base, --graft)   │  │
│  └────────────────────────────────────────────────────────┘  │
│  ┌──────────────────────────┐  ┌──────────────────────────┐  │
│  │ STT: Parakeet TDT v3     │  │ Traducción: opus-mt es↔en│  │
│  │ (ort load-dynamic,       │  │ (CTranslate2 vía ct2rs,  │  │
│  │  en proceso)             │  │  en proceso)             │  │
│  └──────────────────────────┘  └──────────────────────────┘  │
└──────────────────────────────┬───────────────────────────────┘
                               ▼
┌──────────────────────────────────────────────────────────────┐
│  Audio: cpal (WASAPI / CoreAudio / ALSA) en el lado del CLI  │
└──────────────────────────────────────────────────────────────┘
```

El binario expone el CLI (`src/main.rs` + crates) y gestiona el daemon HTTP en `127.0.0.1:8765` por defecto (`lifecycle::daemon_stop::default_addr()`, que aplica `AVI_DAEMON_PORT`, `0` = efímero; su reserva es `lifecycle::daemon_stop::DEFAULT_ADDR`, en `crates/avi-lifecycle/src/daemon_stop.rs`, la fuente única de la que también la toma el lanzador). Los modelos se provisionan vía `ai-voice-interconnector setup` en la **caché exclusiva de la aplicación** (`models_cache_dir()`: `%LOCALAPPDATA%\ai-voice-interconnector\cache\models` en Windows, `~/Library/Caches/ai-voice-interconnector/models` en macOS, `$XDG_CACHE_HOME/ai-voice-interconnector/models` en Linux) y `data_dir()` por SO (con override `AVI_DATA_DIR` por instancia; sin él, resolución idéntica). Solo si el usuario define `HF_HUB_CACHE` o `HF_HOME` esa raíz pasa a ser compartida.

## Estructura del proyecto

```
AI-Voice-InterConnector/
├── src/
│   └── main.rs                         # CLI (clap), dispatch daemon/directo, handle_*
├── crates/
│   ├── avi-core/                       # Tipos, exit codes, json emitter
│   ├── avi-audio/                      # AudioService (cpal, hound)
│   ├── avi-tts/                        # Qwen3TtsEngine, GenerationOptions, resident
│   ├── avi-shared/                     # Fuente única de rutas y canonicalización (la consumen los crates directamente)
│   ├── avi-process/                    # Flags de creación de procesos y borrado diferido (Windows)
│   ├── avi-store/                      # VoiceStore, SpeechStore, ModelStore (hf-hub; progreso por observador, sin dibujo; cache_dir propio; los pines `MODEL_REVISIONS` viven en avi-shared)
│   │   └── assets/default/             # reference.qvoice de la voz de fábrica `default` (huella de timbre), embebido
│   ├── avi-daemon/                     # Servidor HTTP del daemon (axum)
│   ├── avi-stt/                        # ParakeetEngine (ort, load-dynamic)
│   ├── avi-translation/                # Ct2TranslationEngine (CTranslate2/ct2rs)
│   ├── avi-lifecycle/                  # Motor del ciclo de vida: install, uninstall, cleanup, doctor, recibo, bloqueo, PATH; vista de progreso de la provisión (indicatif)
│   └── xtask/                          # tooling Rust: release, cask, licenses, gates de deriva, bootstrap, package, clean
├── packaging/
│   ├── bundle-manifest.json            # Lista canónica de archivos del bundle (la validan self install y cargo xtask package)
│   ├── pins.json                       # Versiones fijadas del toolchain y de las dependencias nativas (lo consume cargo xtask doctor)
│   └── bootstrap/                      # Los dos one-liners del release, únicos ejecutados antes de que exista el binario
│       ├── install.sh                  # Linux + macOS (POSIX sh)
│       └── install.ps1                 # Windows (PowerShell 5.1+ y 7+)
├── vendor/
│   └── qwen3-tts/                      # Binario y pesos Qwen3-TTS (no commiteados todos)
├── tests/
│   ├── cli_golden.rs                   # Harness dorado del CLI
│   ├── contract_conformance.rs         # Conformidad del contrato que se lee de docs, fixtures y código
│   ├── self_install_contract.rs        # Contrato de self install (recibo, bloqueo, integracion de PATH)
│   ├── test_classes.rs                 # Regla de clases de pruebas: ninguna se omite en silencio
│   └── bootstrap/                      # bats (openssl s_server) + Pester (Serve.ps1) de los dos bootstrap
├── Cargo.toml                          # Workspace Rust (version = X.Y.Z, espejo de src/main.rs)
├── Cargo.lock
├── rust-toolchain.toml                 # Versión de Rust fijada para desarrollo y CI (único archivo nuevo en la raíz)
├── .circleci/config.yml                # Pipeline Rust (cargo test/build + publish-release)
└── docs/                               # ver el índice en README.md#documentación
```

> Las voces de **fábrica** `default` están embebidas en el binario (`crates/avi-store/assets/default/`) y se materializan en `data_dir()/voices/default/` en `VoiceStore::ensure_initialized()`. Las voces de **usuario** viven en `data_dir()/voices/<nombre>/` (user-data-dir por SO).

## Entry point `src/main.rs`

`src/main.rs` es el **punto de entrada único** del binario. Usa `clap` para parsear los subcomandos (`version`, `devices`, `translate`, `voice`, `speech`, `daemon`, `setup`, `cleanup`, `self`, `doctor`) y hace dispatch a los crates (`avi-tts`, `avi-store`, etc.) o al daemon vía HTTP. Es también la **fuente de verdad de la versión** (`const VERSION = "X.Y.Z"`, espejo de `Cargo.toml` y `tests/golden/cli_version.json`).

En desarrollo se invoca como `cargo run -- <args>` o `./target/release/ai-voice-interconnector <args>`.

## Motor Qwen3-TTS 0.6B (Rust)

| Aspecto | Detalle |
|---------|---------|
| **Modelos** | `qwen3-tts-0.6b` (síntesis) y `qwen3-tts-0.6b-base` (clonado) |
| **Licencia** | MIT / Apache-2.0 |
| **Parámetros** | 0.6B |
| **Clonación de voz** | un único audio de referencia, `--speech-reference` (~10 s recomendados, máximo 30 s); se guarda solo la huella de timbre (sin transcripción) |
| **Inferencia** | CPU con BLAS (motor nativo `qwen_tts` compilado con `xtask build-engine`) |
| **Opciones de generación** | `GenerationOptions::production()` temp 0.35 seed 4 |

Ver `crates/avi-tts/src/lib.rs` y `vendor/qwen3-tts/CLAUDE.md` para el contrato de invocación: el residente se lanza como `--serve <puerto> --host 127.0.0.1 -j 4 --stream --watch-stdin` (más `--load-voice <qvoice> --graft` cuando se le carga una voz clonada), y el clonado por proceso local como `-d <modelo> --ref-audio <wav> --xvector-only --save-voice <out> --voice-name <nombre> --watch-stdin`. El residente no lleva `--int4`: en el modelo 0.6B cuantizaría el code predictor y la salida dejaría de ser determinista y saldría truncada (p. ej. con la voz `ryan`). El idioma del producto (`es`, `en`) viaja como token y `avi-tts` lo traduce al nombre que entiende el motor (`Spanish`, `English`); un token desconocido es error.

**Clonación solo con la huella de timbre.** `clone_voice` lanza el motor con `--xvector-only`, de modo que toda voz clonada (la de fábrica y las del usuario) guarda en su `reference.qvoice` únicamente el x-vector de la referencia, sin transcripción, códigos ni idioma: la clonación no depende del STT y es igual con o sin `native-stt`. La síntesis de una voz clonada arranca el residente sobre el modelo Base con `--graft` (aplica solo la huella de timbre y deja la prosodia al modelo), mientras que los presets usan CustomVoice; la única regla de elección es `select_model_dir`, y la falta del modelo que exige la voz es `model_missing` (exit 4), sin caer al otro modelo. El idioma lo fija cada petición de síntesis. La voz de fábrica `default` se generó con la ruta de clonación del producto a partir de `assets/audios/Speech Sampler.wav` (~10 s). El `--watch-stdin` es una divergencia propia del motor respecto al upstream y es lo que ata su vida a la de quien lo lanzó: el lanzador mantiene abierta la tubería de su entrada estándar y el motor termina solo al cerrarse.

**Resolución del binario y de los modelos.** `avi-tts` busca el binario del motor por capas y se queda con la primera que lo halla: la variable `QWEN3_TTS_BIN` (si está definida debe apuntar a un archivo existente), la carpeta `vendor/qwen3-tts` junto al ejecutable instalado, la misma carpeta bajo el directorio de trabajo y, por último, el `PATH`. Cada directorio de pesos (`qwen3-tts-0.6b` con `QWEN3_TTS_MODEL_DIR`, `qwen3-tts-0.6b-base` con `QWEN3_TTS_BASE_MODEL_DIR`) sigue un orden análogo: la variable, el directorio hermano del binario resuelto, `vendor/qwen3-tts` junto al ejecutable, el snapshot no vacío de la raíz de modelos y, al final, `vendor/qwen3-tts` bajo el directorio de trabajo. La capa del directorio de trabajo solo participa con `AVI_ALLOW_CWD_VENDOR=1`: sin la variable, si habría resuelto, se avisa por stderr qué ruta se ignoró y cómo habilitarla, de modo que un `vendor/` ajeno no puede suplantar al motor sin un acto explícito. Así `daemon start` calienta desde cualquier directorio de trabajo, sin `QWEN3_TTS_BIN`, cuando se usa el binario instalado. El orden vive en `resolve_binary` y `resolve_named_model_dir` (`crates/avi-tts/src/lib.rs`).

## Traducción cross-lingual (opus-mt / CTranslate2)

Subsistema `crates/avi-translation` que traduce `es<->en` antes de la síntesis (`--source-language`/`--target-language` en `speech say`/`synthesize`) o de forma aislada (`translate`). Usa los derivados int8 de `Helsinki-NLP/opus-mt-es-en` / `opus-mt-en-es` (CC-BY-4.0), convertidos una sola vez a CT2 y publicados en `CristianRojaas/opus-mt-es-en-ct2-int8` y `CristianRojaas/opus-mt-en-es-ct2-int8`; `setup` solo los descarga.

## Transcripción STT (Parakeet TDT v3 int8 / ort)

Subsistema `crates/avi-stt` que transcribe WAV vía `speech transcribe` (audio→texto). Usa `ParakeetEngine` vía `ort` `load-dynamic` (ONNX Runtime 1.28.0) con 4 artefactos acotados (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), incluido por defecto (setup base).

## Flujo de síntesis

```
1. Usuario: ai-voice-interconnector speech say --text "Hola" -v mi_voz
                     │
                     ▼
2. CLI parsea args y resuelve VoiceStore (default embebida o clonada)
                     │
                     ▼
3. Qwen3TtsEngine::synthesize_with_options(texto, VoiceProfile, GenerationOptions con idioma) → resolve_voice_engine
   - default → Clonada (reference.qvoice embebido en el binario)
   - ryan/vivian → Preset (voces puras del motor, sin qvoice)
   - clonada → Clonada(PathBuf) con reference.qvoice
                     │
                     ▼
4. Residente HTTP en loopback (puerto efímero anotado en `daemon.pid`), único camino de síntesis
                     │
                     ▼
5. PCM → WAV (hound) → AudioService::play_wav (cpal) o guardado en SpeechStore
                     │
                     ▼
6. Usuario escucha habla en español con voz clonada
```

## UX de audio interactivo

Dos capacidades necesitan interacción con la persona en la terminal: la captura por voz sin duración fija (`speech transcribe --mic` y `speech dub --mic` sin `--duration`, **push-to-talk**: graba hasta que se pulsa Enter) y la revisión de la toma sintetizada (`speech synthesize --play`, un menú de cuatro opciones en lugar de reproducir y persistir de forma incondicional). El comportamiento observable (flags, matriz de `--mic`, bucle de `--play`, revalidación de colisión, errores y exit codes) lo fijan [CONTRACT §4](CLI/CONTRACT.md#4-síntesis-y-el-bucle-de---play) y [`speech`](CLI/commands/SPEECH.md); aquí quedan los principios y el diseño de implementación.

### Principios

- **P1 — Validación previa pura.** Toda guarda (TTY, exclusión de flags, incompatibilidad con `--json`) se evalúa antes de abrir dispositivos, sintetizar o transcribir: un caso rechazado nunca produce efectos secundarios.
- **P2 — Nunca panic por ruta alcanzable.** Una precondición violada es siempre un `CliError` con exit code de dominio.
- **P3 — Separación de canales.** Avisos, menús y prompts van a stderr; stdout queda para el payload de datos y para los mensajes finales de confirmación en modo humano (como el aviso de descarte al salir del bucle `--play`).
- **P4 — TTY como guarda de interactividad.** Push-to-talk y el bucle `--play` solo se habilitan con stdin en terminal; sin TTY el comportamiento es determinista o un error de uso explícito, nunca un cuelgue esperando entrada que no llegará.
- **P5 — Exit codes canónicos.** Sin códigos nuevos.
- **P6 — Captura siempre client-side.** El micrófono está en la máquina del cliente: incluso en modo daemon el CLI captura el PCM y lo envía. Push-to-talk no toca el daemon.
- **P7 — Salvaguardas audibles.** Cualquier salvaguarda que altere el flujo (techo de duración, aviso al grabar) se comunica por stderr al ocurrir; nunca deja a la persona sin señal ni contamina el payload.

### Captura push-to-talk (`crates/avi-audio`)

`capture_16k_mono_pcm_until_enter(max_secs)` convive con `capture_16k_mono_pcm(duration_secs)`. Ambas comparten el post-procesado (mono, remuestreo a 16 kHz, i16) en el helper privado `finish(recorded, channels, sample_rate)` y producen el mismo formato; difieren solo en el disparador de fin: dormir la duración fija frente a esperar una línea de stdin. El callback de `cpal` llena el buffer en su hilo de audio mientras otro hilo lee stdin y avisa por un canal; la espera aplica un deadline con `recv_timeout`, de modo que la lectura bloqueante no impide el techo.

El techo es el menor entre el que fija quien llama (`max_secs`) y `AVI_PUSH_TO_TALK_MAX_SECS`. Al alcanzarlo la captura se detiene sola, avisa por stderr y devuelve lo grabado: no es error. Un valor inválido de la variable se rechaza como `usage_error` antes de abrir el micrófono.

### Selector de captura (`src/main.rs`)

Las rutas de captura de `transcribe` y `dub`, directas y vía daemon, convergen en `capture_mic_pcm(duration, max_secs)`: `Some(d)` captura de duración fija y `None` push-to-talk (solo alcanzable en TTY por las guardas previas, sin `expect` en la rama mic). La captura corre dentro de `tokio::task::spawn_blocking` para no bloquear el reactor durante la espera de Enter. Ctrl-C durante la grabación lo atiende el manejador global de señales del CLI, sin tratamiento específico.

### Bucle de revisión (`src/main.rs`)

`synthesize_play_loop(...)` recibe la primera toma (WAV temporal) más el contexto (voz, etiqueta, `force`, almacén, texto final) y una clausura `resynthesize` que produce una toma nueva. La ruta directa y la del daemon comparten el bucle, que vive en el cliente porque la reproducción y los prompts son client-side. La opción 3 (regenerar) invoca la clausura, que re-despacha la síntesis por la misma vía que el camino no interactivo, de modo que traducción y daemon se heredan sin lógica especial. La toma se conserva en el cliente para las opciones 1 y 2. La recomprobación de colisión al aceptar reutiliza `speech_store.find(&voice, &label)` en el punto de guardar. Las precondiciones (`--play` con `--json`, `--play` sin TTY) se validan al inicio del brazo `Synthesize`, antes de sintetizar.

Invariantes de eficiencia: la opción 1 no re-sintetiza; la opción 3 sintetiza exactamente una vez; se reproduce al entrar, en cada `1` y en cada `3`, nunca en `2` ni `4`.

### Verificación

Las guardas no interactivas (`--mic` sin `--duration` y sin TTY, `--duration` sin `--mic`, `--play` con `--json`, `--play` sin TTY) se cubren con golden tests en `tests/cli_golden.rs`, que corren sin TTY. Las rutas interactivas (push-to-talk activo, techo `AVI_PUSH_TO_TALK_MAX_SECS`, las opciones del bucle `--play`, la colisión al aceptar y los conteos de síntesis por secuencia) requieren terminal y persona, y se validan según [MANUAL-VALIDATION §9](MANUAL-VALIDATION.md#9-rutas-interactivas-de-audio-push-to-talk-y---play).

### No-objetivos

- No reintroducir el endpoint `/voices/precompute` ni otra superficie de daemon: la captura push-to-talk y el bucle `--play` son client-side (P6).
- No cambiar el formato de audio, el pipeline de traducción ni los exit codes canónicos.

## Salida en terminal

Convenciones del texto para personas (prompts, progreso y avisos por stderr; resultado por stdout). No alcanzan al envelope `--json`, que es contrato máquina y conserva sus literales.

- **Bloques.** Cada bloque que no es el primero de la salida va precedido de una línea en blanco, y dentro de un bloque no hay ninguna. En `self install` el orden es bloque del bootstrap → plan → progreso → resumen final, y cada separación tiene un dueño: el bootstrap (tras su bloque), el encabezado de la vista de progreso y `main.rs` (antes del resumen final).
- **Prompt.** Termina con un espacio, para que la respuesta no quede pegada: `¿Continuar? [S/n] ` y `Esto eliminará lo indicado. ¿Continuar? [s/N] `. Lo escribe solo `confirm::confirm`.
- **Simulación.** Toda simulación (`--dry-run`) imprime el plan y después, una sola vez, `Simulación: no se ha modificado nada.` (`confirm::simulation_notice`).
- **Planes.** El plan de `cleanup` y el de `self uninstall` salen de una única función de composición por operación (`plan_lines`), la misma para la simulación y para la confirmación. Cada ruta aparece una vez, con su categoría en español y su tamaño: `  modelos: C:\…\cache\models (5,9 GB)`.
- **Tamaños y duraciones.** Los tamaños salen de `human_bytes` (escala decimal, un decimal con coma: `5,9 GB`), también en las barras. Las duraciones, de `progress::format_duration` (`7 s`, `1 min 59 s`, `1 h 02 min`).
- **Progreso de la provisión.** `avi-store` calcula los bytes completados y el total de cada modelo a partir de los eventos de `hf-hub` y los entrega a un observador, sin dibujar. La vista (`avi-lifecycle/src/progress.rs`) abre el bloque con `Descargando N modelo(s):`, dibuja una barra por modelo (`  [3/5] opus-mt-en-es  ████████░░░░  48,0 MB/82,5 MB  12,1 MB/s  transcurrido 0:04  quedan 0:03`), la borra al terminar y deja una línea permanente (`  ✓ [1/5] qwen3-tts-0.6b  2,5 GB en 49 s (51,0 MB/s)`), y cierra con `Modelos descargados: 5,9 GB en 1 min 59 s.`. Sin terminal no hay barras, pero el encabezado, las líneas por modelo y el cierre salen igual.
- **Sin literales de contrato.** El texto nunca imprime un literal del contrato `--json` (`status`, `reason`, categorías): cada estado y cada categoría tiene su descripción en español (`describe`).

## Modelo de voces de dos niveles

Las voces se resuelven por nombre con precedencia **usuario→fábrica** (`avi-store/src/lib.rs`):

- **Fábrica**: `voices/default/` embebida en el binario (`include_bytes!`); se materializa en `data_dir()/voices/default/` en `VoiceStore::ensure_initialized()`. Solo lectura, protege `remove("default")`.
- **Usuario**: `data_dir()/voices/<nombre>/` (escribible), registradas con `voice clone`. Homónima sobrescribe a la de fábrica.

Sin `--voice` la CLI usa `default`.

## Comandos CLI

La referencia de comandos y flags vive en [USAGE.md](../USAGE.md), el índice y los códigos de salida en [CLI/README.md](CLI/README.md) y la invocación desde otros lenguajes en el [README](../README.md#invocación-desde-cualquier-lenguaje).

## Compilación Rust (cargo)

La compilación, los requisitos de desarrollo y el pipeline están en [BUILD.md](BUILD.md); los tests, en [CONTRIBUTING.md](../CONTRIBUTING.md#tests).

## Extensibilidad

Para añadir un nuevo motor TTS:

1. Crear nuevo crate en `crates/` o módulo en `crates/avi-tts`
2. Mantener la interfaz `TtsEngine` y el dispatch en `src/main.rs`
3. Recompilar con `cargo build --release --features full` y validar con `cargo test --all`

## Referencias

- [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)
- [CTranslate2](https://github.com/OpenNMT/CTranslate2)
- [ONNX Runtime](https://onnxruntime.ai/)
