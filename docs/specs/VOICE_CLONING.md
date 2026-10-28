# Especificación funcional y de diseño: clonación de voz desde archivo y micrófono en vivo

> **Estado**: contrato de implementación · **Versión base del motor**: v0.36.2 ·
> **Repositorio**: `AI-Voice-InterConnector` · **Fecha**: 2026-10-27

Este documento especifica funcional y técnicamente la extensión del comando
`voice clone` en el motor **AI Voice-InterConnector** para admitir captura de audio
en vivo desde el micrófono (`--mic`), completando la simetría con los comandos
`speech transcribe` y `speech dub`. Define los requisitos, el análisis de
alternativas, la arquitectura de ejecución, el manejo de errores y los criterios
de aceptación de la funcionalidad.

## Tabla de contenidos

- [1. Propósito](#1-propósito)
- [2. Alcance](#2-alcance)
- [3. Contexto actual](#3-contexto-actual)
- [4. Glosario](#4-glosario)
- [5. Requisitos](#5-requisitos)
- [6. Evaluación de alternativas y decisiones](#6-evaluación-de-alternativas-y-decisiones)
- [7. Arquitectura y diseño técnico](#7-arquitectura-y-diseño-técnico)
- [8. Invariantes y reglas transversales](#8-invariantes-y-reglas-transversales)
- [9. Manejo de errores y códigos de salida](#9-manejo-de-errores-y-códigos-de-salida)
- [10. Estrategia de pruebas](#10-estrategia-de-pruebas)
- [11. Criterios de aceptación](#11-criterios-de-aceptación)
- [12. Documentación y contratos afectados](#12-documentación-y-contratos-afectados)
- [13. Referencias](#13-referencias)

---

## 1. Propósito

Habilitar la **clonación de voz directamente desde el micrófono del usuario**
mediante la CLI de `ai-voice-interconnector`, eliminando la fricción de requerir
un archivo `.wav` pregrabado.

La solución debe:

- reutilizar la infraestructura existente de captura de audio multiplataforma
  (`avi-audio` con `cpal`);
- mantener la estricta simetría de flags ya establecida en `speech transcribe` y
  `speech dub`: `[--speech-reference <archivo> | --mic]`;
- preservar la arquitectura del daemon y el protocolo IPC sin cambios en
  `DAEMON_SCHEMA_VERSION`;
- garantizar el mismo comportamiento y códigos de salida tanto en la ruta directa
  local (`--no-daemon`) como en la ruta acelerada por daemon (`--daemon` o `Auto`).

---

## 2. Alcance

### Dentro del alcance

- Extender el parser CLI (`VoiceCommands::Clone`) para aceptar un grupo
  mutuamente excluyente y requerido: `--speech-reference <archivo>` o `--mic`.
- Soportar el flag `--duration <segs>` (1..30 s), exclusivo de `--mic`.
- Soportar captura *push-to-talk* (Enter para finalizar) en terminal interactiva
  (TTY) cuando no se pase `--duration`, acotada por `MAX_CLONE_REFERENCE_SECS`
  (30 s); un `AVI_PUSH_TO_TALK_MAX_SECS` superior a 30 se rechaza.
- Rechazar `--mic` sin `--duration` en entornos no interactivos (no-TTY) con
  código de salida `2` (`InvalidInput`, `usage_error`).
- Enviar el audio capturado por micrófono a la ruta daemon (`POST /voices/clone`)
  codificado en base64 (`audio_b64`), sin mutar el protocolo IPC del daemon.
- Procesar el audio capturado por micrófono en la ruta local (`--no-daemon`)
  mediante `avi_tts::clone_voice` sobre un archivo temporal en scratch.
- Integrar la limpieza de ruido opcional (`--denoise` con la feature
  `native-denoise`) sobre la referencia grabada por micrófono antes de extraer
  el timbre.
- Mantener la invariante de almacenamiento: el motor solo persiste la huella de
  timbre `reference.qvoice` en `<data_dir>/voices/<nombre>/`; ningún WAV se
  conserva en `VoiceStore`. La ruta local usa scratch efímero con limpieza RAII.
- Actualizar pruebas de CLI (`cli_golden.rs`), pruebas unitarias y documentación
  de contrato (`CONTRACT.md` y `VOICE.md`).

### Fuera del alcance

- Capturar micrófono dentro del proceso daemon. El daemon es un servicio de fondo
  que puede correr desacoplado o headless; la captura de audio pertenece siempre
  al proceso CLI que interactúa con el usuario.
- Modificar la duración máxima de la referencia de clonado
  (`MAX_CLONE_REFERENCE_SECS = 30`).
- Streaming continuo de clonación durante la captura de audio: el modelo Base
  requiere el x-vector completo tras finalizar la grabación.
- Edición, recorte o reproducción previa interactiva del audio grabado antes de
  clonar en la CLI básica (se delega a capas de integración/plugins).

---

## 3. Contexto actual

### 3.1 Asimetría en la CLI

Actualmente, el motor implementa captura por micrófono en dos de sus tres
comandos de entrada de audio:

| Subcomando | Flag de archivo | Flag de micrófono | Soporta `--duration` | Estado |
| --- | --- | --- | --- | --- |
| `speech transcribe` | `--audio <archivo>` | `--mic` | Sí (exclusivo `--mic`) | Vigente |
| `speech dub` | `--audio <archivo>` | `--mic` | Sí (exclusivo `--mic`) | Vigente |
| `voice clone` | `--speech-reference <archivo>` (obligatorio) | *No implementado* | *No implementado* | **Asimétrico** |

En `voice clone`, la falta de `--mic` obliga a los consumidores externos (como el
plugin `Agent-Voice-InterConnector`) o a los usuarios finales a depender de
herramientas externas de grabación para generar un archivo `.wav` antes de poder
crear una voz.

### 3.2 Infraestructura existente reutilizable

El motor ya cuenta con los componentes necesarios:

1. **`avi-audio` (`AudioService`)**:
   - `capture_16k_mono_pcm(secs)`: captura una cantidad exacta de segundos a
     16 kHz mono int16.
   - `capture_16k_mono_pcm_until_enter(max_secs)`: captura interactiva
     push-to-talk hasta pulsar Enter, con tope failsafe en segundos. `capture_mic_pcm`
  aplica el mapeo de errores compartido con los demás comandos de captura.
2. **`acquire_pcm()` en `src/main.rs`**:
   - Unifica la lógica de adquisición (archivo vs micrófono) y traduce fallos de
     hardware/dispositivo a `ExitCode::TranscriptionFailed` (10) o
     `ExitCode::InvalidInput` (2).
3. **`POST /voices/clone` en `avi-daemon`**:
   - Recibe `{ "name": String, "audio_b64": String, "force": bool, "denoise": bool }`.
   - Ya opera sobre bytes codificados en base64; no depende de rutas locales del
     cliente.
   - Con `denoise: true`, un DPDFNet ausente produce el evento final
     `error/model_missing`; los errores tipados del DSP conservan `invalid_audio`,
     `io_error` o `denoise_failed` sin clasificar por el texto del mensaje.

---

## 4. Glosario

| Término | Definición |
| --- | --- |
| **`voice clone`** | Subcomando del motor que extrae el x-vector de timbre de un locutor y lo guarda en `reference.qvoice`. |
| **`reference.qvoice`** | Huella de timbre derivada y persistida en `<data_dir>/voices/<nombre>/`; el WAV de entrada no se almacena en `VoiceStore`. |
| **`speech_reference`** | Archivo WAV de entrada, cuya ruta se conserva en CLI al clonar. |
| **Push-to-talk** | Modo de grabación interactiva donde el sistema graba desde el micrófono hasta que el usuario pulsa Enter en la terminal. |
| **`MAX_CLONE_REFERENCE_SECS`** | Límite superior estricto para el audio de referencia de clonación (constante: 30 segundos). |
| **`AVI_PUSH_TO_TALK_MAX_SECS`** | Techo configurable (default: 300 s). Para clonado, el valor debe ser ≤30 s; un valor mayor produce `audio_too_long` antes de abrir el dispositivo. |
| **`warm-on-clone`** | Proceso en segundo plano del daemon que precarga el modelo TTS con la voz recién clonada para eliminar la latencia de la primera síntesis. |
| **`cpal`** | Backend de audio multiplataforma en Rust usado por `avi-audio` para interactuar con CoreAudio, ALSA o WASAPI. |

---

## 5. Requisitos

### 5.1 Requisitos funcionales

| ID | Requisito |
| --- | --- |
| **RF-01** | `voice clone` debe aceptar el grupo mutuamente excluyente y requerido `input`: `--speech-reference <archivo>` (`-s`) o `--mic`. |
| **RF-02** | Omitir tanto `--speech-reference` como `--mic`, o suministrar ambos simultáneamente, debe fallar con `ExitCode::InvalidInput` (2, `usage_error`). |
| **RF-03** | El flag `--duration <segs>` debe ser opcional y aceptarse únicamente junto a `--mic` (`conflicts_with = "speech_reference"`). Usarlo con `--speech-reference` debe fallar con exit 2 (`usage_error`). |
| **RF-04** | `--duration=0` falla con exit 2 (`usage_error`); valores `> 30` fallan con exit 2 (`audio_too_long`). La duración es un objetivo de captura, no una cantidad exacta de muestras. Micrófono >480000 muestras se recorta; una captura más corta no se rellena. |
| **RF-05** | En terminal interactiva (TTY), `--mic` sin `--duration` debe iniciar grabación push-to-talk (hasta pulsar Enter o el menor techo entre 30 s y `AVI_PUSH_TO_TALK_MAX_SECS`). El techo de entorno se valida antes de abrir el dispositivo. |
| **RF-06** | En entorno no interactivo (no-TTY), `--mic` sin `--duration` debe abortar antes de resolver destino o abrir el micrófono con exit 2 (`usage_error`). |
| **RF-07** | Al iniciar la grabación de micrófono, se debe emitir un aviso por `stderr` (`"Grabando del micrófono…"` o fase `capturing`), nunca permanecer en espera silenciosa. |
| **RF-08** | Si la captura de micrófono entrega cero muestras, debe salir con exit 2 (`empty_audio`); no se detecta ni rechaza silencio. |
| **RF-09** | Si el dispositivo de entrada falla (sin permisos, desconectado, error de backend `cpal`), debe salir con exit 10 (`transcription_failed`). |
| **RF-10** | En la ruta daemon, archivo envía sus bytes WAV originales y mic envía un WAV 16 kHz/mono/int16 codificado una vez desde PCM; ambos usan `audio_b64` en el `POST /voices/clone` existente. |
| **RF-11** | En la ruta local, mic se escribe una vez a un `avi_process::scratch::temp_wav("avi_clone_mic_")` con guard RAII retenido durante clonado; archivo conserva la ruta original. |
| **RF-12** | Con `--denoise` y feature `native-denoise`, el destino local limpia una vez en CLI y el daemon limpia una vez en servidor; el cliente envía `denoise: true` y no hay fallback local tras fallo del POST. |
| **RF-13** | Se valida el nombre antes de capturar. En destino local, `VoiceStore::exists` detecta colisión antes de capturar si no hay `--force`; en daemon no se consulta el store cliente y la comprobación autoritativa ocurre después de capturar. No se agrega una reserva especial para nombres factory. |
| **RF-14** | La salida JSON global (`--json`) debe mantener el envelope CLI `{"name": "<nombre>", "speech": "<ruta_qvoice>", "precomputed": bool, "schema_version": "5"}`. `precomputed` es obligatorio; `speech` puede pertenecer al filesystem del proceso que clonó. Este envelope no es el JSON protocolario del endpoint daemon. |

### 5.2 Requisitos no funcionales

| ID | Requisito |
| --- | --- |
| **RNF-01** | **Cero dependencias externas adicionales**: Reutilizar `avi-audio` y `cpal` existentes sin añadir crates nuevos. |
| **RNF-02** | **Estabilidad del protocolo IPC**: `DAEMON_SCHEMA_VERSION = 5` no se modifica. El endpoint `POST /voices/clone` sigue recibiendo `audio_b64`. |
| **RNF-03** | El WAV scratch local se limpia por RAII en salidas controladas. Una terminación forzosa o un fallo del filesystem puede dejar residuos para el barrido posterior; el WAV no se guarda en `VoiceStore`. |
| **RNF-04** | La captura ocurre en CLI; en destino daemon, el WAV se transmite al daemon mediante `POST /voices/clone`. No se conserva el WAV en `VoiceStore`; `reference.qvoice` sí es un dato derivado persistente. |
| **RNF-05** | **Paridad multiplataforma**: La captura mantiene el mismo contrato CLI; la disponibilidad de dispositivos y E2E se verifica en las plataformas accesibles. |
| **RNF-06** | La conversión PCM → WAV → base64 puede medirse como observación informativa; 20 ms no es un umbral de aceptación ni de CI. |

---

## 6. Evaluación de alternativas y decisiones

### 6.1 ¿Dónde debe residir la captura de micrófono?

| Alternativa | Evaluación | Decisión |
| --- | --- | --- |
| **A. En el proceso CLI (Front)** | El proceso CLI tiene acceso a la TTY del usuario, variables de sesión y contexto del dispositivo de audio. Envía los bytes resultantes al daemon. | **Elegida.** Coherente con `transcribe` y `dub`. Permite que el daemon corra headless o sin acceso a TTY. |
| **B. En el proceso Daemon (Back)** | El daemon abriría el micrófono. Falla si el daemon corre como servicio, en contenedor o sin TTY para push-to-talk. | **Descartada.** Rompe el modelo de permisos y aislamiento del daemon. |

### 6.2 Formato de paso al daemon

| Alternativa | Evaluación | Decisión |
| --- | --- | --- |
| **A. Reutilizar `audio_b64` (WAV en memoria)** | El CLI codifica el PCM 16k mono capturado a WAV en RAM y lo envía en `audio_b64`. No altera la API REST ni el esquema del daemon. | **Elegida.** 100 % compatible con el contrato existente del daemon. |
| **B. Endpoint nuevo `POST /voices/clone/mic`** | Obligaría a versionar el protocolo IPC (`DAEMON_SCHEMA_VERSION = 6`) sin beneficio técnico. | **Descartada.** Incurre en deuda de compatibilidad innecesaria. |

### 6.3 Manejo de push-to-talk vs `--duration`

| Alternativa | Evaluación | Decisión |
| --- | --- | --- |
| **A. Push-to-talk interactivo con tope de 30 s + `--duration` opcional** | Idéntico a `speech transcribe` y `speech dub`. En TTY el usuario habla y presiona Enter; en scripts/no-TTY se exige `--duration`. | **Elegida.** Experiencia de usuario consistente en toda la CLI. |
| **B. Exigir siempre `--duration`** | Elimina la comodidad de pulsar Enter al terminar de hablar. | **Descartada.** Mayor fricción para el usuario en terminal. |

---

## 7. Arquitectura y diseño técnico

### 7.1 Definición del Parser CLI (`clap`)

En `src/main.rs`, la estructura `VoiceCommands::Clone` se actualiza:

```rust
#[derive(Subcommand)]
enum VoiceCommands {
    /// Listar voces registradas
    List,
    /// Clonar una voz desde audio de referencia o micrófono
    #[command(group(
        clap::ArgGroup::new("input")
            .required(true)
            .multiple(false)
            .args(["speech_reference", "mic"])
    ))]
    Clone {
        #[arg(short, long)]
        name: String,
        /// Audio de referencia de habla (archivo WAV; mutuamente excluyente con --mic)
        #[arg(short = 's', long, group = "clone_input")]
        speech_reference: Option<String>,
        /// Capturar audio de referencia directamente desde el micrófono
        #[arg(long, group = "clone_input")]
        mic: bool,
        /// Duración fija de grabación en segundos; solo válido con --mic (máx 30 s)
        #[arg(long, conflicts_with = "speech_reference")]
        duration: Option<u64>,
        /// Sobrescribir una voz existente con el mismo nombre
        #[arg(short, long)]
        force: bool,
        /// Limpiar ruido de fondo antes de clonar (solo con `native-denoise`)
        #[cfg(feature = "native-denoise")]
        #[arg(long)]
        denoise: bool,
        #[command(flatten)]
        routing: Routing,
    },
    /// Eliminar una voz clonada y sus locuciones guardadas
    Remove { ... },
}
```

### 7.2 Flujo de ejecución unificado

```text
voice clone --name <nombre> [--speech-reference <wav> | --mic [--duration <N>]]
  │
  ├─ 1. Validar nombre de voz (validate_name) ── fallo → exit 2 (invalid_voice_name)
  ├─ 2. Archivo: conservar precedencia de validaciones actual. Mic: validar nombre, duration=0 / >30, techo PTT (>30 → audio_too_long) y no-TTY antes de abrir dispositivo.
  ├─ 3. No-TTY sin duration → usage_error antes del probe. Resolver destino una vez; local comprueba colisión si !force y daemon no consulta el store cliente
  ├─ 4. Colisión local mic antes de capturar. Capturar mic, recortar >480000 muestras y codificar una vez; archivo WAV intacto
  ├─ 5. Destino daemon: capturar antes de POST; el servidor valida la colisión autoritativa; sin fallback a local
  ├─ 6. Clonado local: mic se escribe en avi_process::scratch::temp_wav; archivo usa su ruta original
  ├─ 7. Denoise único: CLI en ruta local; daemon con denoise:true para ejecución del servidor
  ├─ 8. avi_tts::clone_voice y voice_store.save_reference; JSON CLI conserva name, speech, precomputed, schema_version
  │
  └─ 8. Cierre y Emisión:
        ├─ Salida humana: "Voz '<nombre>' clonada (N s)."
        └─ Salida JSON global (--json, antes de `voice`): { "name": "...", "speech": "...", "precomputed": bool, "schema_version": "5" }
```

La tabla de errores de archivo conserva el orden previo del CLI: nombre, límite de referencia, routing/modelo y después comprobación de existencia/lectura según destino. Las guardas nuevas de micrófono no cambian ese orden.

### 7.3 Preparación de la fuente para clonación

La preparación de micrófono reutiliza `capture_mic_pcm` en `src/main.rs`; el WAV de archivo no se decodifica ni reempaqueta:

```rust
// Nombre validado antes de preparar la fuente.
// Para micrófono: validar duration y AVI_PUSH_TO_TALK_MAX_SECS; en no-TTY sin duration, usage_error precede al probe y a la captura.
let use_daemon = route_to_daemon(routing.mode(), &client).await?;

if mic && !use_daemon && !force {
    ensure_voice_does_not_exist(&name)?; // Colisión local antes de capturar.
}

let (speech_path, daemon_audio, mic_scratch) = if mic {
    let mut pcm = capture_mic_pcm(duration, avi_core::MAX_CLONE_REFERENCE_SECS).await?;
    truncate_clone_pcm(&mut pcm); // Máximo 480000 muestras, sin rellenar capturas cortas.
    let wav = avi_audio::encode_pcm_16k_mono_wav(&pcm)?;
    if use_daemon {
        (None, Some(wav), None) // El daemon recibe bytes desde memoria.
    } else {
        let guard = avi_process::scratch::temp_wav("avi_clone_mic_")?;
        std::fs::write(guard.path(), wav)?;
        (Some(guard.path().clone()), None, Some(guard)) // El guard vive hasta terminar el clonado.
    }
} else {
    let path = std::path::PathBuf::from(speech_reference.expect("clap garantiza una fuente"));
    (Some(path), None, None) // El archivo conserva ruta y bytes originales.
};
```

### 7.4 Codificación WAV y despacho al daemon

Para evitar escribir temporales innecesarios en disco cuando se usa la ruta daemon con `--mic`, el PCM capturado se codifica como WAV 16 kHz mono directamente en un buffer de memoria:

```rust
fn encode_pcm_16k_mono_wav(pcm: &[i16]) -> Result<Vec<u8>, hound::Error> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: 16_000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
    for &sample in pcm {
        writer.write_sample(sample)?;
    }
    writer.finalize()?;
    Ok(cursor.into_inner())
}
```

`clone_via_daemon` se actualiza para aceptar los bytes del audio (`audio_bytes: &[u8]`):

```rust
async fn clone_via_daemon(
    client: &reqwest::Client,
    name: &str,
    audio_bytes: &[u8],
    force: bool,
    denoise: bool,
    progress: &mut PhaseProgress,
) -> Result<(String, String, bool), CliError> {
    progress.phase("cloning", "Clonando voz (daemon)…");
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(audio_bytes);
    let mut payload = serde_json::json!({
        "name": name,
        "audio_b64": audio_b64,
        "force": force,
    });
    if denoise {
        payload["denoise"] = serde_json::json!(true);
    }
    let client_addr = lifecycle::daemon_stop::resolve_client_addr(&effective_data_dir()?);
    let fut = client
        .post(format!("http://{}/voices/clone", client_addr))
        .json(&payload)
        .send();
    let resp = tokio::time::timeout(std::time::Duration::from_millis(1500), fut)
        .await
        .map_err(|_| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {} (timeout 1500ms)", client_addr),
            )
        })?
        .map_err(|e| {
            CliError::new(
                ExitCode::DaemonUnreachable,
                "daemon_unreachable",
                format!("Daemon inalcanzable en {}: {}", client_addr, e),
            )
        })?;
    if !resp.status().is_success() {
        return Err(daemon_error_from_response(resp).await);
    }
    let val: Value = consume_ndjson_stream(resp, StreamResult::Clone, progress).await?;
    let cloned_name = val["name"].as_str().ok_or_else(|| {
        CliError::new(ExitCode::Error, "daemon_protocol_error", "Falta campo 'name'")
    })?;
    let cloned_speech = val["speech"].as_str().ok_or_else(|| {
        CliError::new(ExitCode::Error, "daemon_protocol_error", "Falta campo 'speech'")
    })?;
    let precomputed = val["precomputed"].as_bool().ok_or_else(|| CliError::new(ExitCode::Error, "daemon_error", "El daemon no devolvió 'precomputed' en la clonación."))?;
    Ok((cloned_name.to_string(), cloned_speech.to_string(), precomputed))
}
```

---

## 8. Invariantes y reglas transversales

1. **Mutua exclusión estricta**: `--speech-reference` y `--mic` nunca pueden coexistir en la misma invocación (garantizado por el grupo `clap::ArgGroup`).
2. **Límites del micrófono**: `--duration` > 30 falla con exit 2 (`audio_too_long`). Push-to-talk queda limitado por el menor entre 30 s y `AVI_PUSH_TO_TALK_MAX_SECS`; el PCM por encima de 480000 muestras se recorta y las capturas cortas no se rellenan.
3. **No-TTY seguro**: Ejecutar `--mic` sin `--duration` en entornos desatendidos (CI, tuberías, cron) falla inmediatamente en fase de validación con exit 2 sin bloquear ni abrir el micrófono.
4. **Almacenamiento**: El WAV no se almacena en `VoiceStore`; solo persiste `reference.qvoice`. En local, el WAV scratch se elimina mediante RAII en salidas controladas. Una terminación forzosa o fallo del filesystem puede dejar residuos.
5. **Compatibilidad total del Daemon**: El endpoint `POST /voices/clone` no sufre cambios; sigue validando la duración máxima por cabecera WAV y bytes recibidos.
6. **Colisión según destino**: la comprobación se rige por `VoiceStore::exists`; local ocurre antes de capturar y daemon después de recibir el audio. No existe una política especial de reserva de nombres factory para clone.

---

## 9. Manejo de errores y códigos de salida

| Condición | Origen | Código de salida | Reason JSON | Mensaje / Comportamiento |
| --- | --- | --- | --- | --- |
| Sin `--speech-reference` ni `--mic` | Parser clap | `2` | `usage_error` | Mensaje de ayuda clap. |
| Ambos `--speech-reference` y `--mic` | Parser clap | `2` | `usage_error` | Argumentos mutuamente excluyentes. |
| `--duration` con `--speech-reference` | Parser clap | `2` | `usage_error` | `--duration` solo es válido con `--mic`. |
| `--duration` > 30 s o techo PTT configurado > 30 s | Validación | `2` | `audio_too_long` | «La duración de grabación supera el límite máximo de clonado (30 s).» |
| `--mic` sin `--duration` en no-TTY | Validación | `2` | `usage_error` | «`--mic` requiere `--duration` en entornos no interactivos.» |
| Nombre de voz ilegal | Validación | `2` | `invalid_voice_name` | «Nombre de voz inválido: ...» |
| Voz ya existe sin `--force` | `VoiceStore` | `6` | `voice_exists` | «La voz '...' ya existe (usa `--force` para sobrescribirla).» |
| Archivo `--speech-reference` ausente | Filesystem | `3` | `audio_not_found` | «El audio de referencia '...' no existe.» |
| Fallo de hardware / mic cerrado | `avi-audio` | `10` | `transcription_failed` | «Fallo de captura de audio: ...» |
| Captura vacía (0 bytes) | `avi-audio` | `2` | `empty_audio` | «El audio capturado está vacío.» |
| Modelo Base no provisionado | `avi-tts` | `4` | `model_missing` | «El modelo Base de clonado TTS no está provisionado. Ejecuta 'setup'.» |
| Daemon inalcanzable (vía daemon) | IPC client | `5` | `daemon_unreachable` | «El daemon no responde.» |
| Error interno de clonado | `qwen3-tts` | `1` | `voice_clone_failed` | «Fallo al clonar la voz: ...» |

---

## 10. Estrategia de pruebas

### 10.1 Pruebas unitarias PCM y WAV

- `crates/avi-audio/src/lib.rs`: round-trip PCM→WAV verifica cabecera 16 kHz/mono/int16 y muestras; la guarda vacía se prueba sin dispositivo.
- `src/main.rs`: recorte 480001→480000, exacto sin cambio y menor sin padding; round-trip PCM→WAV→base64 y benchmark de 30 s informativo ignorado.

### 10.2 Pruebas de parser e integración (`tests/cli_golden.rs`)

- Mantener sin cambios las pruebas de regresión de archivo, incluido `voice clone -n test -s audio.wav`.
- Verificar `--mic --duration 10` mediante el parser y las guardas previas; la captura real queda para E2E con dispositivo.
- En ejecución no-TTY, verificar que `voice clone -n test --mic` sin duración falla antes de abrir el dispositivo.
- Verificar que `voice clone -n test` (sin -s ni --mic) es rechazado con error clap.
- Verificar que `voice clone -n test -s audio.wav --mic` es rechazado por exclusión mutua.
- Verificar que `voice clone -n test -s audio.wav --duration 10` es rechazado por conflicto.

### 10.3 Pruebas E2E por recursos disponibles

La matriz se ejecuta en cada host y plataforma accesibles con micrófono, modelos y binario provisionados. La latencia PCM→WAV→base64 es una observación, no una puerta CI. Registrar explícitamente las rutas y plataformas no ejecutadas.

- **Parser CLI**: cubrir fuente requerida/exclusiva y `--duration` con archivo, incluidas las cuatro claves del envelope versionado cuando se usa `--json`.
- **Guardas sin dispositivo**: no-TTY sin duración y duración 0 salen con `usage_error`; duración 31 sale con `audio_too_long`, bajo `--daemon` y `--no-daemon`, antes de probe/captura.
- **`voice clone --name invalido# --mic --duration 5`**: Verificar que sale con exit `2` (`invalid_voice_name`) antes de capturar audio.
- **Colisión local**: sembrar una voz clonada y verificar exit `6` antes de captura con `--no-daemon`; las voces factory no son una regla adicional de colisión.
- La captura real y la clonación con modelos quedan como E2E por plataforma y recurso, no se simulan con stubs nuevos. Las pruebas deterministas inspeccionan parser, WAV codificado y el payload del daemon con los helpers ya existentes.

---

## 11. Criterios de aceptación

| ID | Dado | Cuando | Entonces |
| --- | --- | --- | --- |
| **AC-01** | La CLI `ai-voice-interconnector` | Se ejecuta `voice clone --help` | La ayuda documenta el grupo `[--speech-reference <archivo> | --mic]` y `--duration`. |
| **AC-02** | Un entorno no-TTY (CI / pipe) | Se ejecuta `voice clone --name v1 --mic` sin `--duration` | Sale con exit `2` (`usage_error`) antes de abrir el micrófono. |
| **AC-03** | Una invocación con duración excesiva | Se ejecuta `voice clone --name v1 --mic --duration 31` | Sale con exit `2` (`audio_too_long`) sin capturar audio. |
| **AC-04** | Ambas fuentes suministradas | Se ejecuta `voice clone --name v1 -s ref.wav --mic` | Sale con exit `2` (`usage_error`) por exclusión mutua. |
| **AC-05** | `--duration` con archivo | Se ejecuta `voice clone --name v1 -s ref.wav --duration 10` | Sale con exit `2` (`usage_error`) por incompatibilidad de flags. |
| **AC-06** | TTY y dispositivos/modelos disponibles | Se ejecuta `voice clone --name v1 --mic` | Anuncia `capturing`, termina con Enter o el menor límite efectivo y completa el clonado. La E2E se ejecuta solo en hosts con dispositivo. |
| **AC-07** | Una captura fija de 5 s | Se ejecuta `voice clone --name v1 --mic --duration 5` | La duración es objetivo; el PCM resultante se limita a 480000 muestras, no se rellena y alimenta el flujo convergente de clonado. |
| **AC-08** | Una voz existente en el store local | Se clona con `--mic --no-daemon` sin `--force` | Sale con exit `6` antes de grabar; por daemon la comprobación sucede después del POST. |
| **AC-09** | Una voz ya existente | Se clona con `--mic --force` por daemon | Sobrescribe la voz en `VoiceStore` del daemon e inicia warm-on-clone (exit 0); la ruta local no mantiene un residente que precalentar. |
| **AC-10** | El flag global `--json` | Se ejecuta `ai-voice-interconnector --json voice clone --name v1 --mic --duration 5` | Stdout contiene únicamente el envelope CLI `{ name, speech, precomputed, schema_version }`; `speech` puede apuntar al filesystem del proceso que clonó. |
| **AC-11** | La ruta local explícita | Se ejecuta `voice clone --name v1 --mic --duration 5 --no-daemon` | Genera `reference.qvoice`, elimina temporales de scratch y emite `precomputed: false`. |
| **AC-12** | La feature `native-denoise` | Se pasa `--denoise` con `--mic` | El audio capturado se procesa con DPDFNet antes de la extracción de x-vector. |
| **AC-13** | Dispositivo de audio inaccesible | Se ejecuta `--mic` | Sale con exit `10` (`transcription_failed`) con mensaje descriptivo. |
| **AC-14** | Una clonación local completada con `--mic` | Se inspecciona el scratch tras una salida controlada | Existe `<data_dir>/voices/v1/reference.qvoice` y se limpió el WAV scratch; terminación forzosa o fallo filesystem puede dejarlo para barrido posterior. |
| **AC-15** | WAV pregrabado y captura del mismo locutor, con modelos y dispositivo disponibles | Se clonan y sintetizan mediante ambas rutas | Mismo contrato y síntesis funcional; no se espera identidad de las tomas ni igualdad binaria de `.qvoice`. |

---

## 12. Documentación y contratos afectados

| Documento | Ubicación | Modificación |
| --- | --- | --- |
| **`CONTRACT.md`** | `docs/CLI/CONTRACT.md` | Actualizar la tabla de flags de `voice clone` para incluir `--mic` y `--duration`, y documentar las reglas de validación en §3 y §12. |
| **`VOICE.md`** | `docs/CLI/commands/VOICE.md` | Añadir ejemplos de uso de `voice clone --mic`, push-to-talk y `--duration`. |
| **`README.md`** | `README.md` | Mencionar la capacidad de clonar directamente desde el micrófono en la sección de características. |
| **`CHANGELOG.md`** | `CHANGELOG.md` | Registrar la adición de `--mic` en `voice clone`. |

---

## 13. Referencias

- `AI-Voice-InterConnector/docs/CLI/CONTRACT.md` §3 (Grupo speech y voice) y §12 (Contratos externos).
- `AI-Voice-InterConnector/docs/CLI/commands/VOICE.md` — Comando `voice clone`.
- `AI-Voice-InterConnector/docs/CLI/commands/SPEECH.md` — Implementación de `--mic` en `transcribe` y `dub`.
- `AI-Voice-InterConnector/crates/avi-audio/src/lib.rs` — Servicio de captura `AudioService`.
- `Agent-Voice-InterConnector/docs/specs/VOICE_CLONING.md` — Especificación contraparte desde la perspectiva del plugin.
