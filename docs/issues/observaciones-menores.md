# Observaciones menores: WAV truncado

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | funcional (carga de audio) |
| Componente | `avi-audio`, `avi-tts`, `avi-daemon` y CLI (`voice clone`) |
| Versión detectada | 0.25.0 |
| Plataforma | no depende de la plataforma |
| Reproducibilidad | siempre, salvo que se indique otra cosa |
| Detectado en | revisión del código al diseñar la corrección de `--audio` inexistente, 2026-09-29 |

## Resumen

Un defecto suelto, sin causa común con otros, agrupado aquí porque no justifica, por
ahora, un documento propio. Salió de leer el código y puede producir un resultado
incorrecto sin aviso (severidad media). La causa está confirmada por lectura del código.
La observación tiene una ficha breve (síntoma, reproducción, causa, alternativas,
recomendación y criterio). Si crece, se separa a su propio documento con la plantilla
completa.

Las recomendaciones no añaden ni conservan mecanismos de retrocompatibilidad: cuando el
arreglo cambia un comportamiento o una razón de error, el comportamiento anterior se
retira en vez de mantenerse como reserva o excepción.

## Entorno

Lectura del código fuente de v0.25.0 y de `hound` 3.5.1, la versión fijada en
`Cargo.lock`. No se ha reproducido todavía; la ficha indica cómo hacerlo.

## Precondiciones

Las propias de la observación.

## Pasos para reproducir

Se detallan en la observación, dentro de «Análisis de causa».

## Resultado observado

Se detalla en la observación.

## Resultado esperado

Se detalla en la observación.

## Impacto y workaround

Puede afectar al resultado: un WAV truncado se transcribe o se dobla incompleto sin
ningún aviso. Workaround: comprobar la integridad del WAV antes de pasarlo.

## Evidencia

Recogida en la observación.

## Análisis de causa

### Límites de entrada

#### 1. Un WAV truncado se carga incompleto sin error

- **Severidad:** media. El resultado es incorrecto y no hay aviso, aunque la entrada
  dañada es poco frecuente.
- **Síntoma:** un WAV cuya cabecera declara más datos de los que contiene el archivo se
  transcribe, se dobla, se reproduce o se usa como referencia de clonado con solo la
  parte legible, y el comando termina bien.
- **Reproducción:** cortar los últimos bytes de un WAV válido y pasarlo a
  `speech transcribe --no-daemon --audio truncado.wav --source-language es-latam`.
- **Causa (confirmada por lectura, incluido el comportamiento de `hound` 3.5.1):** el
  iterador de muestras de `hound` recorre tantas muestras como declara la cabecera, y
  cada una que ya no está en el archivo devuelve un error `UnexpectedEof`.
  `load_wav_16k_mono_pcm`, `load_wav_24k_mono_pcm` y `play_wav`
  (`crates/avi-audio/src/lib.rs`) leen con `.filter_map(Result::ok)`, que descarta ese
  error, y también cualquier otro error de lectura a mitad del archivo, y devuelve lo
  leído como si el archivo estuviera completo. La clasificación ya existe: la conversión
  de `hound::Error` a `WavLoadError` trata `UnexpectedEof` como `Invalid`, y la CLI lo
  traduce a exit 2 con `invalid_audio`. En transcripción y doblaje solo falta propagar
  el error en vez de descartarlo.
- **Clonado (confirmado por lectura):** la referencia de `voice clone
  --speech-reference` se carga con `load_wav_24k_mono_pcm`, que devuelve un error
  genérico y no `WavLoadError`. Cualquier fallo al cargarla termina como exit 1 con
  `voice_clone_failed`, tanto en la ruta local como en la del daemon, no como
  `invalid_audio`; hoy ocurre así incluso con un archivo que no es un WAV. El
  `audio_decode_error` que ya emite el daemon en esa ruta es otro caso: el campo
  `audio_b64` no es base64 válido, no que su contenido no sea un WAV. Además, el cliente
  de `voice clone` traduce la razón del daemon a código de salida con su propia tabla,
  tanto en la respuesta HTTP como en el stream NDJSON, y ninguna de las dos incluye
  `invalid_audio`: aunque el daemon la emitiera, la CLI saldría con exit 1.
- **Reproducción (`play_wav`):** `speech play` reproduce una locución guardada con
  `play_wav`; si su archivo está truncado, hoy suena solo la parte legible y el comando
  termina bien. `play_wav` recoge todas las muestras antes de abrir el stream de salida,
  así que con una carga estricta el error llega antes de que suene nada.
- **Cabeceras mal escritas por grabadoras en streaming:** hay dos casos. Con tamaño 0,
  `hound` lee cero muestras; es otro problema (audio vacío) y este arreglo no lo cambia.
  Con un tamaño sobredimensionado (el centinela `0xFFFFFFFF`), hoy el archivo se procesa
  por accidente y con el arreglo estricto pasa a rechazarse.
- **Alternativas:**
  - *A. Estricta:* el primer error de muestra aborta la carga. A favor: una línea por
    rama (recoger en `Result<Vec<_>, _>` y propagar), ninguna API nueva y coherente con
    el criterio que ya se aplica a un archivo que no es un WAV válido. En contra:
    rechaza los WAV de streaming con la cabecera sobredimensionada, de los que no hay
    ningún reporte.
  - *B. Tolerante con aviso:* conservar lo leído y avisar. En contra: obliga a cambiar
    la firma de los cargadores y a añadir un aviso al contrato de la CLI y del daemon,
    y el resultado sigue siendo parcial.
  - *C. Estricta salvo con el centinela `0xFFFFFFFF`.* Descartada: es una excepción
    para conservar un comportamiento accidental que el arreglo retira, y no hay ningún
    reporte que la justifique.
  - *D. Cortar en el primer error sin avisar.* Mantiene el defecto; descartada.
- **Recomendación:** A en los tres cargadores. El mensaje de `invalid_audio` puede
  sugerir reescribir el archivo con una herramienta de audio. El clonado se reclasifica
  en el mismo cambio: `load_wav_24k_mono_pcm` devuelve `WavLoadError`, y `voice clone`,
  en la ruta local y en la del daemon, reporta una referencia inválida como
  `invalid_audio` (exit 2) en lugar de `voice_clone_failed`. Así una misma entrada
  inválida recibe la misma razón en todos los comandos; `voice_clone_failed` queda para
  los fallos del propio clonado. Detalles del clonado:
  - `avi_tts::clone_voice` no cambia de firma: el error de carga se propaga con `?` y
    `anyhow` conserva su tipo, así que la CLI y el daemon lo recuperan con
    `downcast_ref::<WavLoadError>()`.
  - La clasificación es la misma que en transcripción: `Invalid` da `invalid_audio`
    (exit 2) e `Io` da `io_error` (exit 1). `NotFound` no se alcanza: la ruta local ya
    comprueba que el archivo existe y la del daemon recibe el audio en `audio_b64`.
  - Las dos tablas del cliente de `voice clone` (HTTP y NDJSON) añaden
    `invalid_audio` con exit 2; sin eso, la ruta del daemon seguiría saliendo con
    exit 1.

  Es un cambio incompatible del contrato de `voice clone`: se actualiza su
  documentación y se anota como tal en el CHANGELOG. En
  `play_wav`, el error se reporta como `playback_failed` (exit 1), la razón que ya usa
  cualquier fallo de reproducción.
- **Esperado:** un error de lectura de muestras interrumpe la carga. En transcripción,
  doblaje y clonado se clasifica como audio inválido (exit 2, `invalid_audio`); en la
  reproducción, como `playback_failed` (exit 1).
- **Criterio:** una prueba unitaria con un WAV truncado comprueba el error en cada
  cargador; una prueba de `voice clone` con una referencia truncada comprueba
  `invalid_audio` y exit 2 en la ruta local y en la del daemon; y ningún camino de carga
  descarta muestras en silencio.

## Diagnóstico sugerido

La causa está confirmada por lectura del código; no queda ninguna medida pendiente.

## Criterio de aceptación

La observación tiene su criterio en su ficha. El documento se da por resuelto cuando
está cerrada o separada a su propio documento.

## Relacionados

- Contrato de la CLI, reglas de `--audio` (la clasificación de un audio inválido) y de
  `voice clone` (la razón de una referencia inválida pasa de `voice_clone_failed` a
  `invalid_audio`).
