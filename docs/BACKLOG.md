# Backlog

Funcionalidades pedidas o diseñadas que se dejaron para más adelante, con lo necesario para retomarlas sin conocer la conversación en que surgieron. Los defectos no van aquí, sino en `docs/issues/`.

## Convenciones

- **Una entrada por funcionalidad**, con estas secciones: qué es, por qué no se hace ahora, cuándo merece la pena retomarla, de qué depende, lo que ya está decidido, lo que falta por analizar y el trabajo que supone.
- **Autocontenida.** Se entiende sin conocer la sesión en que se escribió.
- **Código citado por archivo y símbolo**, no por número de línea.
- **Lo decidido, separado de lo pendiente.** Lo que no se ha analizado no se presenta como resuelto.
- **Al empezar una entrada**, pasa a un plan de trabajo y se borra de aquí; al descartarla, se borra y el motivo queda en el commit.

## Cambiar de voz sin reiniciar el motor

### Qué es

Hoy cada voz exige un residente propio: `start_resident` (`crates/avi-tts/src/lib.rs`) lanza el motor con `--load-voice <voz>` y, si la siguiente síntesis pide otra voz, AVI detiene ese residente y arranca otro, con el evento `progress{loading_voice}` y el texto «Cargando la voz…». La funcionalidad consiste en que un único residente atienda todas las voces y la voz viaje en cada petición.

### Por qué no se hace ahora

- **No es un caso de uso recurrente.** La narración del plugin EvolutiveX-Agents usa una sola voz activa, que el usuario elige en `/config` (`evolutivex-agents.voice`) y cambia de forma ocasional, y `SessionStart` arranca el daemon con esa voz ya cargada. La pre-síntesis de anuncios (`presynth --voice <voz>`) se hace una vez por voz, en lote.
- **El beneficio es pequeño.** Un cambio de voz cuesta lo que tarda el residente en estar listo: medido como el arranque del residente hasta que `/v1/health` responde (mediana de 6 lanzamientos con la voz `default` en el equipo de referencia, Ryzen 7 5825U, 8 núcleos y 16 hilos), 1,47 s con el motor C y 1,59 s con el motor Rust.
- **El coste es alto:** necesita una funcionalidad nueva del motor y, en AVI, cambiar el formato del almacén de voces en el disco del usuario y el ciclo de vida del residente.

### Cuándo retomarla

Cuando la voz cambie a menudo entre síntesis seguidas, por ejemplo si cada agente narra con una voz distinta: cada mensaje pagaría entonces el reinicio del residente.

### De qué depende

De la entrada «Selección de voz por petición en el servidor HTTP» del backlog del motor (`docs/BACKLOG.md` de `qwen-tts-engine`): una opción `--voices-dir <carpeta>` y un campo `voice` con el nombre de la voz, que resuelve a `<carpeta>/<nombre>.qvoice`. AVI la usa cuando esté publicada en una versión del motor.

### Lo que ya está decidido

- El almacén pasa de `voices/<nombre>/reference.qvoice` a `voices/<nombre>.qvoice`, porque el motor toma el nombre de la voz del nombre de su archivo. El residente arranca con `--voices-dir <data_dir>/voices` y sin `--load-voice`.
- `build_tts_body` manda siempre `voice`, también para `default`: si no, el motor usaría su voz incorporada y no la de AVI.
- El evento `progress{loading_voice}` pasa a `progress{starting_engine}`, con el texto «Arrancando el motor…», y se emite solo al arrancar el residente (primer uso y rearranque por degradación). `DAEMON_SCHEMA_VERSION` no cambia: los clientes ignoran los valores de `stage` que no conocen. El plugin no usa `loading_voice`.
- La huella sha256 de `VoiceStore::resolve` se conserva: `persist_locution` la usa para detectar que una voz se re-clonó durante una síntesis (`voice_changed`).

### Lo que falta por analizar

- **Migración del almacén** al formato plano en las instalaciones existentes: nombres de los temporales (`crates/avi-shared/src/staging_names.rs`), recuperación (`crates/avi-lifecycle/src/recovery.rs`) y el cambio del valor de `speech` en `voice clone --json`, que lee el plugin.
- **Reemplazo atómico** de `voices/<nombre>.qvoice` al re-clonar mientras el residente puede estar leyéndolo, sobre todo en Windows.

### Trabajo que supone

- `crates/avi-store/src/lib.rs`: `VoiceStore` con el formato plano (`list`, `exists`, `find_reference`, `resolve`, `remove`, `ensure_initialized`, `stage_reference`), más los temporales y la recuperación.
- `crates/avi-tts/src/lib.rs`: el residente único (`start_resident` y el camino de cambio de voz), `build_tts_body` con `voice` y `SynthesisStep::LoadingVoice` → `SynthesisStep::StartingEngine`.
- El renombrado de `loading_voice` en el daemon (`StepSignal` en `crates/avi-daemon/src/lib.rs`), la CLI (`src/main.rs`) y `SYNTHESIS_LOADING_VOICE_TEXT` (`crates/avi-term/src/stage.rs`), con sus pruebas.
- Documentación: `docs/CLI/CONTRACT.md`, `docs/CLI/commands/` (`DAEMON`, `SPEECH`, `VOICE`, `SETUP`), `docs/DESIGN.md`, `docs/specs/VOICE_CLONING.md`, `docs/MANUAL-VALIDATION.md` y `USAGE.md`, y el CHANGELOG.
