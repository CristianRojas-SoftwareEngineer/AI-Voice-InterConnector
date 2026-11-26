# Diseño del sistema AI Voice-InterConnector (Rust + Qwen3-TTS)

## Tabla de contenidos

- [Resumen ejecutivo](#resumen-ejecutivo)
- [Arquitectura](#arquitectura)
- [Estructura del proyecto](#estructura-del-proyecto)
- [Entry point `src/main.rs`](#entry-point-srcmainrs)
- [Motor Qwen3-TTS 0.6B (Rust)](#motor-qwen3-tts-06b-rust)
- [Traducción cross-lingual (opus-mt / CTranslate2)](#traducción-cross-lingual-opus-mt--ctranslate2)
- [Transcripción STT (Parakeet TDT v3 int8 / ort)](#transcripción-stt-parakeet-tdt-v3-int8--ort)
- [Limpieza de audio y vídeo por CLI](#limpieza-de-audio-y-vídeo-por-cli)
- [Flujo de síntesis](#flujo-de-síntesis)
- [UX de audio interactivo](#ux-de-audio-interactivo)
- [Salida en terminal](#salida-en-terminal)
- [Modelo de voces](#modelo-de-voces)
- [Comandos CLI](#comandos-cli)
- [Compilación Rust (cargo)](#compilación-rust-cargo)
- [Extensibilidad](#extensibilidad)
- [Referencias](#referencias)

## Resumen ejecutivo

AI Voice-InterConnector es un sistema de voz con IA **100% local** en español e inglés: transcripción con Parakeet TDT 0.6B v3, síntesis con clonación de voz sobre **Qwen3-TTS 0.6B** (las voces clonadas se sintetizan con el modelo Base), traducción con opus-mt y traducción de voz a voz con `speech dub`. El usuario puede clonar su propia voz a partir de ~10 segundos de audio y generar narración de alta calidad.

- **Licencia**: MPL-2.0 (código del proyecto, copyleft a nivel de archivo); modelo y dependencias conservan sus licencias (MIT/BSD/Apache), salvo traducción `opus-mt` (CC-BY-4.0)
- **Idiomas**: español (`es`) e inglés (`en`)
- **Clonación**: referencia de voz vía archivo (`--speech-reference`) o micrófono en vivo (`--mic`; ~10 s recomendados, máximo 30 s)
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
│  │  · toda voz    → qwen3-tts-0.6b-base (Base)            │  │
│  │    (--load-voice <qvoice>)                             │  │
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
│   ├── avi-term/                       # Núcleo de presentación en terminal: paleta, formatos, línea de etapa, catálogo de etapas e indicador de fases (crate hoja)
│   ├── avi-process/                    # Flags de creación de procesos y borrado diferido (Windows); sondas de loopback
│   ├── avi-store/                      # VoiceStore, SpeechStore, ModelStore (hf-hub; progreso por observador, sin dibujo; cache_dir propio; los pines `MODEL_REVISIONS` viven en avi-shared)
│   │   └── assets/default/             # reference.qvoice de la voz de fábrica `default` (huella de timbre), embebido
│   ├── avi-daemon/                     # Servidor HTTP del daemon (axum)
│   ├── avi-stt/                        # ParakeetEngine (ort, load-dynamic)
│   ├── avi-translation/                # Ct2TranslationEngine (CTranslate2/ct2rs)
│   ├── avi-lifecycle/                  # Motor del ciclo de vida: install, uninstall, cleanup, doctor, recibo, bloqueo, PATH; vista de progreso de la provisión (indicatif 0.18)
│   ├── denoise/                        # Limpieza de ruido DPDFNet para archivos de audio y vídeo (native-denoise)
│   └── xtask/                          # tooling Rust: release, cask, licenses, gates de deriva, bootstrap, package, clean
├── packaging/
│   ├── bundle-manifest.json            # Lista canónica de archivos del bundle (la validan self install y cargo xtask package)
│   ├── pins.json                       # Versiones fijadas del toolchain y de las dependencias nativas (lo consume cargo xtask doctor)
│   └── bootstrap/                      # Los dos one-liners del release, únicos ejecutados antes de que exista el binario
│       ├── install.sh                  # Linux + macOS (POSIX sh)
│       └── install.ps1                 # Windows (PowerShell 5.1+ y 7+)
├── vendor/                             # Copias vendorizadas y parches [patch.crates-io]
│   ├── alsa-sys-0.4.0/                 # Parche dlopen para Linux sin ALSA estático
│   ├── cmake-0.1.58/                   # Soporte CMake para builds nativos
│   ├── ct2rs-0.10.0/                   # CTranslate2 con runtime C++/OpenMP enlazado
│   ├── hf-hub-1.0.0/                   # Descarga HF con hardlinks en Windows
│   └── onednn-src-0.1.15/              # oneDNN estático
├── tests/
│   ├── cli_golden.rs                   # Harness dorado del CLI
│   ├── contract_conformance.rs         # Conformidad del contrato que se lee de docs, fixtures y código
│   ├── self_install_contract.rs        # Contrato de self install (recibo, bloqueo, integracion de PATH)
│   ├── test_classes.rs                 # Regla de clases de pruebas: ninguna se omite en silencio
│   └── bootstrap/                      # bats (openssl s_server) + Pester (Serve.ps1) de los dos bootstrap
├── Cargo.toml                          # Workspace Rust (version = X.Y.Z, espejo de src/main.rs)
├── Cargo.lock
├── rust-toolchain.toml                 # Versión de Rust fijada para desarrollo y CI
├── .circleci/config.yml                # Pipeline Rust (cargo test/build + publish-release)
└── docs/                               # ver el índice en README.md#documentación
```

> Las voces de **fábrica** `default` están embebidas en el binario (`crates/avi-store/assets/default/`) y se materializan en `data_dir()/voices/default/` en `VoiceStore::ensure_initialized()`, que también las reinstala, con sus audios sintetizados borrados, cuando el binario trae otra versión. Las voces de **usuario** viven en `data_dir()/voices/<nombre>/` (user-data-dir por SO).

## Entry point `src/main.rs`

`src/main.rs` es el **punto de entrada único** del binario. Usa `clap` para parsear los subcomandos (`version`, `devices`, `translate`, `voice`, `speech`, `daemon`, `setup`, `cleanup`, `self`, `doctor`) y hace dispatch a los crates (`avi-tts`, `avi-store`, etc.) o al daemon vía HTTP. Es también la **fuente de verdad de la versión** (`const VERSION = "X.Y.Z"`, espejo de `Cargo.toml` y `tests/golden/cli_version.json`).

En desarrollo se invoca como `cargo run -- <args>` o `./target/release/ai-voice-interconnector <args>`.

## Motor Qwen3-TTS 0.6B (Rust)

| Aspecto | Detalle |
|---------|---------|
| **Modelo** | `qwen3-tts-0.6b-base` (síntesis y clonado) |
| **Licencia** | MPL-2.0 (binario de `qwen-tts-engine`) / Apache-2.0 (pesos del modelo) |
| **Parámetros** | 0.6B |
| **Clonación de voz** | referencia de voz vía archivo (`--speech-reference`) o micrófono (`--mic`; ~10 s recomendados, máximo 30 s); se guarda solo la huella de timbre (sin transcripción) |
| **Inferencia** | CPU con BLAS (binario de `qwen-tts-engine` fijado en `packaging/pins.json` y aprovisionado en `engine-bundle/`) |
| **Opciones de generación** | `GenerationOptions::production()` temp 0.35 seed 4 (`PRODUCTION_SEED`, primera toma; las regeneraciones de `--play` lo sustituyen con `with_seed_override`) |

Ver `crates/avi-tts/src/lib.rs` para el contrato de invocación: el residente se lanza como `-d <modelo Base> --serve <puerto> --host 127.0.0.1 -j 4 --watch-stdin --load-voice <qvoice>` (siempre con la voz, `default` incluida), y el clonado por proceso local como `-d <modelo Base> --ref-audio <wav> --save-voice <out> -j 4 --watch-stdin`. El idioma del producto (`es`, `en`) viaja como token y `avi-tts` lo traduce al nombre que entiende el motor (`Spanish`, `English`); un token desconocido es error. El residente sirve una sola voz y atiende una petición a la vez —AVI las serializa—, y el idioma viaja en todas las peticiones, así que la síntesis no depende del idioma de la voz. El texto de un error del motor es informativo —el campo `error` llega al usuario en español— y no es contrato: el contrato de AVI es el `reason` de su salida `--json`.

**Entrega en streaming.** Toda síntesis usa `POST /v1/tts/stream` del residente, que entrega PCM s16le de 24 kHz mono por partes: el primer fragmento es de 2 fotogramas (160 ms de audio; un fotograma son 80 ms) y los siguientes de 4. `avi-tts` los entrega a quien llama (`synthesize_streaming`) o los acumula en un WAV (`synthesize_to_wav`); una respuesta que se corta sin su fragmento final de tamaño cero es `SynthesisTruncated`, nunca un audio corto válido. Cuatro fotogramas por fragmento dejan un hueco de ~0,12 s tras el primer fragmento, frente a ~0,58 s con 10, a cambio de un 4 % de factor de tiempo real. El reproductor incremental (`StreamPlayer`, en `avi-audio`) remuestrea cada fragmento a la tasa del dispositivo y espera 300 ms de audio en cola antes de sonar, lo que cubre el hueco entre el primer y el segundo fragmento (medido en 0,12–0,19 s); si la cola se vacía con la entrega en curso, pausa y reanuda al recuperar el colchón, y cuenta un corte (`stalls`). `first_audio_ms` son los milisegundos desde que se abre la etapa de síntesis hasta la primera muestra entregada al dispositivo, e incluyen la espera de turno, el calentamiento del motor y la carga de la voz. Por el daemon, el audio viaja en eventos `audio` del flujo NDJSON. La CLI abre el dispositivo de salida antes de pedir la síntesis y, si esta falla con la voz ya sonando, corta la reproducción y termina con error.

**Precisión del residente.** El motor tiene un único perfil de precisión, siempre activo; AVI no lo elige.

El hijo residente y el de clonado reciben `TMP`, `TEMP` y `TMPDIR` apuntando al scratch de AVI, y el número de hilos va solo en `-j` (4, la constante `ENGINE_THREADS`). El contrato del motor exige al cliente no definir las variables de hilos de OpenBLAS (`OPENBLAS_NUM_THREADS`, `GOTO_NUM_THREADS` y `OMP_NUM_THREADS`), así que AVI las retira del entorno heredado del hijo en todas las plataformas (`spawn_with_allowlist_env` con `env_remove`); el entorno del padre no cambia.

**Clonación solo con la huella de timbre.** El motor solo guarda el x-vector de la referencia, de modo que toda voz clonada (la de fábrica y las del usuario) guarda en su `reference.qvoice` únicamente esa huella, sin transcripción, códigos ni idioma: la clonación no depende del STT y es igual con o sin `native-stt`. La síntesis arranca el residente sobre el modelo Base con `--load-voice`, único modelo del motor; la falta del modelo Base es `model_missing` (exit 4). El idioma lo fija cada petición de síntesis. La voz de fábrica `default` se generó con la ruta de clonación del producto a partir de `assets/audios/Speech Sampler.wav` (~10 s). El `--watch-stdin` es una divergencia propia del motor respecto al upstream y es lo que ata su vida a la de quien lo lanzó: el lanzador mantiene abierta la tubería de su entrada estándar y el motor termina solo al cerrarse.

**Resolución del binario y de los modelos.** `avi-tts` busca el binario del motor por capas y se queda con la primera que lo halla: la variable `QWEN3_TTS_BIN` (si está definida debe apuntar a un archivo existente), la raíz del bundle junto al ejecutable instalado (`qwen_tts[.exe]`) y, en desarrollo, `engine-bundle/` bajo el directorio de trabajo. El directorio de pesos sigue un orden análogo: la variable `QWEN3_TTS_BASE_MODEL_DIR` (modelo `qwen3-tts-0.6b-base`) y el snapshot no vacío de la raíz de modelos de AVI. Ningún modelo vive junto al binario, ni en el bundle ni en `engine-bundle/`. Así `daemon start` calienta desde cualquier directorio de trabajo, sin `QWEN3_TTS_BIN`, cuando se usa el binario instalado. El orden vive en `resolve_binary` y `resolve_named_model_dir` (`crates/avi-tts/src/lib.rs`).

## Traducción cross-lingual (opus-mt / CTranslate2)

Subsistema `crates/avi-translation` que traduce `es<->en` antes de la síntesis (`--source-language`/`--target-language` en `speech say`/`synthesize`) o de forma aislada (`translate`). Usa los derivados int8 de `Helsinki-NLP/opus-mt-es-en` / `opus-mt-en-es` (CC-BY-4.0), convertidos una sola vez a CT2 y publicados en `CristianRojaas/opus-mt-es-en-ct2-int8` y `CristianRojaas/opus-mt-en-es-ct2-int8`; `setup` solo los descarga.

## Transcripción STT (Parakeet TDT v3 int8 / ort)

Subsistema `crates/avi-stt` que transcribe WAV vía `speech transcribe` (audio→texto). Usa `ParakeetEngine` vía `ort` `load-dynamic` (ONNX Runtime 1.28.0) con 4 artefactos acotados (`encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `nemo128.onnx`, `vocab.txt`), incluido por defecto (setup base).

## Limpieza de audio y vídeo por CLI

La única ruta pública de limpieza de archivos es `ai-voice-interconnector speech denoise`. El subcomando local reutiliza el pipeline DPDFNet migrado para archivos de audio y para extraer, limpiar y remuxar la pista de audio de vídeo; copia el primer stream de vídeo sin procesar la imagen. No crea un endpoint del daemon ni un binario raíz paralelo.

El feature `native-denoise` contiene esta capacidad y forma parte de `full`. El modelo DPDFNet se provisiona con `setup`; FFmpeg se localiza desde el paquete verificado de AVI, nunca mediante una búsqueda de `PATH`: el ejecutable integrado debe declarar versión 6 o superior, con o sin el prefijo de `git describe` de los builds de Linux. La limpieza de archivos usa `studio` por omisión y admite `--preset default` para el perfil original; los parámetros de egresión no se aplican al preprocesamiento opt-in de clonado, STT o doblaje.

El `--json` global emite JSONL, una línea por evento, y conserva progreso y resultados aptos para scripting. La referencia de opciones, compatibilidad, errores y ejemplos está en [`docs/CLI/commands/SPEECH.md`](CLI/commands/SPEECH.md#speech-denoise) y el contrato normativo en [`docs/CLI/CONTRACT.md`](CLI/CONTRACT.md#speech-denoise-limpieza-local-de-archivos).

## Flujo de síntesis

```
1. Usuario: ai-voice-interconnector speech say --text "Hola" -v mi_voz
                     │
                     ▼
2. CLI parsea args y prepara la síntesis con prepare_synthesis, en el orden
   «invocación → almacén → recursos»: construye VoiceName (y Label en
   synthesize), comprueba que la voz exista y aplica la CollisionPolicy; solo
   después comprueba el modelo o el daemon. Resuelve la voz con
   VoiceStore::resolve → VoiceReference (ruta de reference.qvoice + huella
   sha256), o None si la voz no tiene reference.qvoice
                     │
                     ▼
3. Qwen3TtsEngine::synthesize_streaming(texto, VoiceProfile { name, reference }, GenerationOptions con idioma, cancelación, destino de los fragmentos) → resolve_voice_engine
   - default → VoiceReference (reference.qvoice embebido en el binario)
   - clonada → VoiceReference con reference.qvoice
   - sin reference.qvoice → VoiceNotFound
   El residente se reutiliza solo si la voz resuelta es igual a la que tiene
   cargada: la misma identidad de contenido. Si no, o si
   no hay residente o está degradado, el motor arranca uno (2 a 6 s) y lo
   avisa a quien llama con `SynthesisStep::LoadingVoice` justo antes y con
   `SynthesisStep::Synthesizing` cuando el residente está sano; el daemon lo
   anuncia por el wire como `progress{loading_voice}` y la CLI como el texto
   vivo `Cargando la voz…` de la etapa de síntesis
                     │
                     ▼
4. Residente HTTP en loopback (puerto efímero anotado en `daemon.pid`), único camino de síntesis:
   `POST /v1/tts/stream` con fragmentos de hasta 10 fotogramas, que el motor hace crecer desde 2 y 4 fotogramas según un calendario fijo (entrega en streaming)
                     │
                     ▼
5. Fragmentos PCM → StreamPlayer (cpal), que suena con 300 ms de colchón, y/o
   acumulados en un WAV (hound) que se guarda con avi_store::persist_locution,
   que comprueba la identidad de la voz y aplica la CollisionPolicy; el
   desenlace es un SynthesizeOutcome (success, skipped o discarded)
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

`capture_mono_pcm_until_enter(target_rate, max_secs, observer)` convive con `capture_mono_pcm(target_rate, duration_secs, observer)`. Ambas comparten el post-procesado (mono, remuestreo a `target_rate` Hz: 16, 24 o 48 kHz, i16) en el helper privado `finish(recorded, channels, sample_rate, target_rate)` y producen el mismo formato; difieren solo en el disparador de fin: dormir la duración fija frente a esperar una línea de stdin. El callback de `cpal` llena el buffer en su hilo de audio mientras otro hilo lee stdin y avisa por un canal; la espera aplica un deadline con `recv_timeout`, de modo que la lectura bloqueante no impide el techo.

La grabación con micrófono, de duración fija o con push-to-talk, mantiene viva la etapa de captura: con terminal muestra `● Grabando m:ss / m:ss`, un medidor de nivel y, si se para con Enter, `Enter para detener`; sin terminal escribe una vez `Grabando (máximo m:ss)…`. La biblioteca de audio no escribe en la terminal: informa de su progreso a un observador (`CaptureObserver`) que la CLI traduce a la línea de la etapa.

El techo es el menor entre el que fija quien llama (`max_secs`) y `AVI_PUSH_TO_TALK_MAX_SECS`. Al alcanzarlo la captura se detiene sola, avisa por stderr y devuelve lo grabado: no es error. Un valor inválido de la variable se rechaza como `usage_error` antes de abrir el micrófono.

### Selector de captura (`src/main.rs`)

Las rutas de captura de `transcribe`, `dub` (16 kHz) y `voice clone` (48 kHz con `--denoise`, 24 kHz sin él), directas y vía daemon, convergen en `capture_mic_pcm(target_rate, duration, max_secs, observer)`: `Some(d)` captura de duración fija y `None` push-to-talk (ambas con la etapa de captura viva y su contador avanzando durante la grabación) (solo alcanzable en TTY por las guardas previas, sin `expect` en la rama mic). La captura corre dentro de `tokio::task::spawn_blocking` para no bloquear el reactor durante la espera de Enter. Ctrl-C durante la grabación lo atiende el manejador global de señales del CLI, sin tratamiento específico.

### Bucle de revisión (`src/main.rs`)

`synthesize_play_loop(...)` recibe la primera toma (WAV temporal, que ya sonó mientras se sintetizaba) más el contexto (voz, etiqueta, política de colisión, almacenes, referencia resuelta de la voz, texto final) y devuelve una decisión; su llamador, `play_menu_loop`, resuelve la opción 3 con una implementación del rasgo `PlayResynthesizer` (`LocalResynthesizer` o `DaemonResynthesizer`) que produce una toma nueva, reproducida en streaming mientras se sintetiza, sobre el mismo temporal. La ruta directa y la del daemon comparten el bucle, que vive en el cliente porque la reproducción y los prompts son client-side. La re-síntesis re-despacha la síntesis por la misma vía que el camino no interactivo, de modo que traducción y daemon se heredan sin lógica especial. La toma se conserva en el cliente para las opciones 1 y 2. Al aceptar, el audio sintetizado se guarda con `avi_store::persist_locution`, que recomprueba en el punto de guardar la identidad de la voz (`voice_changed`) y aplica la política de colisión (`label_exists` con `Fail`). Las precondiciones (`--play` con `--json`, `--play` sin TTY, `--play` con `--skip-existing`) se validan en la invocación, antes de sintetizar.

Invariantes de eficiencia: la opción 1 no re-sintetiza; la opción 3 sintetiza exactamente una vez; la toma suena mientras se sintetiza (la primera y cada regeneración), el menú no la repite al entrar, y la reproducción del archivo ocurre solo en cada `1`, nunca en `2` ni `4`.

### Verificación

Las guardas no interactivas (`--mic` sin `--duration` y sin TTY, `--duration` sin `--mic`, `--play` con `--json`, `--play` sin TTY) se cubren con golden tests en `tests/cli_golden.rs`, que corren sin TTY. Las rutas interactivas (push-to-talk activo, techo `AVI_PUSH_TO_TALK_MAX_SECS`, las opciones del bucle `--play`, la colisión al aceptar y los conteos de síntesis por secuencia) requieren terminal y persona, y se validan según [MANUAL-VALIDATION §9](MANUAL-VALIDATION.md#9-rutas-interactivas-de-audio-push-to-talk-y---play).

### No-objetivos

- No reintroducir el endpoint `/voices/precompute` ni otra superficie de daemon: la captura push-to-talk y el bucle `--play` son client-side (P6).
- No cambiar el formato de audio, el pipeline de traducción ni los exit codes canónicos.

## Salida en terminal

Convenciones del texto para personas. No alcanzan al envelope `--json`, que es contrato máquina y conserva sus literales. El núcleo de presentación es el crate `avi-term`: es el único sitio que da formato a duraciones, tamaños y plurales, emite color, gestiona la línea de etapa y dibuja las etapas, y todos los comandos lo usan.

- **Flujos.** stdout lleva solo resultados, en cuanto existen: los textos transcritos o traducidos, las listas, los planes de `--dry-run` y la ruta del archivo generado. Stderr lleva la línea de contexto, las etapas, el cierre, los planes que preceden a una confirmación, las preguntas, los avisos y los errores. Con `--json` no se escriben ni el contexto, ni los resultados de texto, ni el cierre; las etapas siguen por stderr.
- **Contexto, etapas y cierre.** Cada comando con etapas abre con una línea de contexto que dice qué hará y por qué ruta (`Doblaje es → en · voz default · archivo a.wav · daemon`), trabaja por etapas con nombre y cierra con `✓ <acción> en <duración total>`, más ` · <dato>` por cada dato extra. Una etapa permanente deja `✓ <texto>  <duración>` al terminar; una transitoria (validación, envío al daemon, espera de turno de la limpieza de ruido) se borra. Las esperas de la síntesis (turno, calentamiento del motor, carga de la voz) son textos vivos de la etapa de síntesis y nunca la cierran como hecha. Cuando un comando solo tiene una etapa de trabajo, esa etapa no deja su línea y queda solo el cierre. La línea de contexto se escribe tras validar la invocación y el almacén y antes del primer acceso a recursos, de modo que esos errores salen sin ella y los de recursos y de trabajo con ella. Un fallo no deja línea `✓`.
- **El indicador es el dueño de su línea.** `PhaseProgress` (`avi-term`) es el único escritor de secuencias de control de línea del workspace. Ninguna otra función imprime en esa línea: cuando el producto necesita escribir, **cede** la línea (`yield_line`) y la vuelve a tomar al continuar (`reclaim`). Por eso una etapa nunca queda pegada a lo que se escribe después. Los avisos pasan por `avi_term::line::warn`, que borra la línea de etapa abierta antes de escribir `Aviso: …` y la deja cedida; el indicador la vuelve a pintar en su siguiente repintado. No hay línea en blanco inicial.
- **Etapa en curso.** Con terminal se repinta debajo de las terminadas como `… <texto activo> <duración>`; la grabación (`● Grabando m:ss / m:ss`, con medidor de nivel) y la reproducción mantienen vivo su contador. Sin terminal, una etapa permanente escribe `… <texto>` una vez, un latido cada 10 s y su línea `✓` al terminar; una transitoria no escribe nada. El reloj de la etapa, el de la actividad del daemon y el del cierre salen del mismo indicador, de modo que las cifras cuadran entre sí, y avanzan también durante el trabajo bloqueante de las rutas directas (un hilo auxiliar repinta mientras el trabajo corre).
- **Color.** Verde para `✓`, rojo para `Error:` y `✗`, amarillo para `Aviso:`, atenuado para duraciones y detalles y negrita para los resultados. Solo se emite si el flujo concreto es una terminal, `NO_COLOR` no está definida o está vacía y la consola acepta secuencias ANSI. `AVI_NO_PROGRESS` (cualquier valor no vacío) silencia solo las etapas.
- **Errores.** Todo error en texto sale como `Error: <mensaje>.` y, si su `reason` tiene remedio en la tabla única, `Solución: <orden>.`. El texto de las bibliotecas no aparece en la salida de texto: viaja en el campo `detail` del error, que sale en `--json` (` Detalle: …`) y en el registro de trazas.
- **Duraciones, tamaños y plurales.** Una sola función de duraciones: `0,7 s` y `12,4 s` por debajo de 60 s, `2 min 35 s` desde 60 s y `1 h 02 min` desde 1 h (`m:ss` solo para el contador de grabación). Los tamaños (escala decimal, un decimal con coma: `5,9 GB`) salen de `avi_term::fmt::bytes`, también en las barras. Los plurales concuerdan (`avi_term::fmt::count`: `1 ruta`, `2 rutas`) y nunca llevan paréntesis. En el vocabulario, «audio sintetizado» sustituye a cualquier otra denominación del audio que el producto genera.
- **Prompt.** Termina con un espacio, para que la respuesta no quede pegada, y nombra lo que se borra o hace: `¿Borrar lo indicado? [s/N] ` (destructivas) y `¿Instalar el programa? [S/n] ` (no destructivas). Lo escribe solo `confirm::confirm`.
- **Cancelación.** `Cancelado; no se ha borrado nada.` (destructivas) o `Cancelado.` (las demás) lo escribe **solo** `confirm::confirm`, por stderr. Los handlers no lo repiten: el estado se dice una vez.
- **Simulación.** Toda simulación (`--dry-run`) escribe el plan por stdout y después, una sola vez, `Simulación: no se ha modificado nada.` por stderr (`confirm::simulation_notice`).
- **Planes.** El plan de `cleanup` y el de `self uninstall` salen de una única función de composición por operación (`plan_lines`), la misma para la simulación y para la confirmación. Cada ruta aparece una vez, con su categoría en español y su tamaño: `  modelos: C:\…\cache\models (3,4 GB)`.
- **Progreso de la provisión.** `avi-store` calcula los bytes completados y el total de cada modelo a partir de los eventos de `hf-hub` y los entrega a un observador, sin dibujar. La vista (`avi-lifecycle/src/progress.rs`) abre el bloque con `Descargando N modelos:`, dibuja una barra por modelo con porcentaje (`  [3/5] opus-mt-en-es  ████████░░░░   58 %  48,0 MB / 82,5 MB  12,1 MB/s  quedan 3,0 s`), repintada cada 250 ms, la borra al terminar y deja una línea permanente (`  ✓ [1/5] qwen3-tts-0.6b-base  2,5 GB en 49,0 s · 51,0 MB/s`). El cierre con el total lo escribe el comando que provisiona. Sin terminal no hay barras, pero el encabezado y las líneas por modelo salen igual; cada descarga activa emite una línea visible cada 15 s. `avi-store` distingue el avance por archivo y por grupo xet y filtra las muestras sin bytes nuevos; la vista transmite ese avance al watchdog de `bootstrap` como máximo una vez cada 5 s por modelo. El watchdog no usa el tamaño de los blobs, porque pueden estar preextendidos.
- **Sin literales de contrato.** El texto nunca imprime un literal del contrato `--json` (`status`, `reason`, categorías): cada estado y cada categoría tiene su descripción en español (`describe`).

## Modelo de voces

Todas las voces viven en un único nivel, `data_dir()/voices/<nombre>/`, y se resuelven por nombre (`avi-store/src/lib.rs`).

**Nombres validados por tipo.** Los almacenes solo construyen rutas a partir de `VoiceName` y `Label`, que solo se construyen con `parse`: el único validador acepta ASCII alfanumérico, `.`, `_` y `-`, rechaza el nombre vacío, `.` y `..`, y normaliza a minúsculas. El compilador obliga a cada llamador —la CLI y el daemon— a construir el tipo antes de tocar el disco, así que salir de un almacén es imposible por construcción. Un nombre ilegal es `invalid_voice_name` para la voz e `invalid_identifier` para la etiqueta.

**Política de colisión.** `CollisionPolicy { Fail, Overwrite, Skip }` decide qué hacer con una etiqueta ocupada en un único punto que solo mira el WAV; `check_locution` la aplica antes de sintetizar y `persist_locution` al guardar, y devuelve `Persisted::{Saved, Skipped}`. `--skip-existing` es `Skip`, que hace idempotente la pre-síntesis.

Las voces son de dos clases:

- **Fábrica**: `default`, clonada, cuyo `reference.qvoice` va embebido en el binario (`include_bytes!`) y se materializa en `VoiceStore::ensure_initialized()`, que la reinstala si el `reference.qvoice` del almacén difiere del embebido: así una versión nueva la trae por cualquier canal de instalación o actualización. `is_factory_name` la protege de `voice remove` y de `voice clone`.
- **Usuario**: registradas con `voice clone`. `voice clone --force` sobre un nombre existente, de fábrica incluido, sustituye su referencia en el mismo directorio.

**Identidad de contenido.** `VoiceStore::resolve` devuelve la referencia de una voz clonada como `VoiceReference { path, fingerprint }`, con la huella sha256 del `reference.qvoice` completo. Todo lo derivado de una voz corresponde a su contenido actual: el residente TTS se reutiliza solo con la misma identidad; `avi_store::replace_voice` es la única forma de cambiar el contenido de una voz y borra sus audios sintetizados antes de publicar la referencia nueva; y `avi_store::persist_locution` es la única forma de escribir un audio sintetizado y comprueba que la voz conserve la identidad con la que se sintetizó.

Sin `--voice` la CLI usa `default`.

## Comandos CLI

La referencia de comandos y flags vive en [USAGE.md](../USAGE.md), el índice y los códigos de salida en [CLI/README.md](CLI/README.md) y la invocación desde otros lenguajes en el [README](../README.md#invocación-desde-cualquier-lenguaje).

## Compilación Rust (cargo)

La compilación, los requisitos de desarrollo y el pipeline están en [BUILD.md](BUILD.md); los tests, en [CONTRIBUTING.md](../CONTRIBUTING.md#tests).

## Extensibilidad

Para añadir un nuevo motor TTS:

1. Crear nuevo crate en `crates/` o módulo en `crates/avi-tts`
2. Ofrecer la misma interfaz de entrega en streaming que `Qwen3TtsEngine` (`synthesize_streaming` y `synthesize_to_wav`) y adaptar el dispatch en `src/main.rs`
3. Recompilar con `cargo build --release --features full` y validar con `cargo test --all`

## Referencias

- [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)
- [CTranslate2](https://github.com/OpenNMT/CTranslate2)
- [ONNX Runtime](https://onnxruntime.ai/)
