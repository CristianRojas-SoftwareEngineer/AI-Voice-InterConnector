# La vía daemon rechaza o corta audios largos que la vía directa procesa

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | funcional, diagnóstico |
| Componente | daemon (`crates/avi-daemon/src/lib.rs`, router y handlers) y CLI (`src/main.rs`, clientes de la vía daemon) |
| Versión detectada | 0.25.0 |
| Plataforma | no depende de la plataforma |
| Reproducibilidad | por verificar; ninguno de los dos síntomas se ha reproducido |
| Detectado en | revisión del código de la vía daemon de transcripción y dub, 2026-09-29 |

## Resumen

En la vía daemon hay dos límites de transporte que no tienen nada que ver con los
límites del producto y que deciden qué audio se puede procesar: el tamaño máximo por
defecto del cuerpo de las peticiones y un plazo de 1500 ms pensado para conectar que
acota una inferencia completa. El resultado es que un audio largo que la vía directa
transcribe bien falla por daemon con un error que no explica la causa. El síntoma 1
afecta al daemon vigente y a grabaciones legítimas.

## Entorno

Lectura del código fuente de v0.25.0, con axum 0.7.9 según `Cargo.lock`.

## Precondiciones

Daemon activo y un WAV de 16 kHz mono de la duración indicada en cada síntoma.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

La duración del audio no convierte un daemon sano en un error. El daemon acepta audios
hasta el techo que admite el producto, o el cliente rechaza antes de enviar con un
`reason` claro.

## Impacto y workaround

Síntoma 1: impide transcribir por daemon una grabación de más de ~49 s, incluida una de
push-to-talk, cuyo techo es de 300 s. Workaround: `--no-daemon`, que no tiene ese
límite, o dividir el audio.

Síntoma 2: solo afecta a un daemon anterior a la ruta `/dub`. Workaround: actualizar el
daemon.

## Evidencia

Recogida en cada síntoma.

## Análisis de causa

### 1. El daemon rechaza con 413 los audios de más de ~49 s en `/transcribe`

- **Severidad:** media. `speech transcribe --daemon` (y el modo automático con el daemon
  activo) falla con un mensaje sin utilidad ante grabaciones largas, que el producto
  permite.
- **Síntoma (probable):** `speech transcribe --audio largo.wav` (o `--mic` de más de
  ~49 s) con el daemon activo termina en exit 1 con `reason` `daemon_error` y el mensaje
  «El daemon devolvió 413 Payload Too Large». Con `--no-daemon` el mismo audio se
  transcribe bien.
- **Reproducción:** con el daemon activo, ejecutar `speech transcribe --daemon --audio
  largo.wav --source-language es-latam` con un WAV de 16 kHz mono de 60 s.
- **Causa (confirmada por lectura del código; el 413 concreto es probable, deducido del
  comportamiento por defecto de la biblioteca):** `build_router_with_state`
  (`crates/avi-daemon/src/lib.rs`) construye el `Router` sin `DefaultBodyLimit`, y
  `transcribe_handler` (igual que `dub_handler`) recibe el cuerpo con el extractor
  `Json`. En axum 0.7 los extractores de cuerpo aplican por defecto un límite de 2 MB.
  `transcribe_via_daemon` (`src/main.rs`) envía el PCM i16 little-endian de 16 kHz mono
  (32 000 bytes por segundo) en base64, que aumenta el tamaño un tercio: 2 MB de cuerpo
  equivalen a unos 49 s de audio. Un push-to-talk de 300 s (`AVI_PUSH_TO_TALK_MAX_SECS`
  por defecto) genera ~12,8 MB de cuerpo. En modo automático no hay reintento local:
  `route_to_daemon` decide antes de leer el audio y `transcribe_via_daemon` devuelve el
  error tal cual, sin cuerpo con `reason`.
- **Esperado:** fijar un `DefaultBodyLimit` explícito en el router, coherente con el
  techo del push-to-talk (300 s ≈ 12,8 MB en base64, con margen), o comprobar la
  duración en el cliente. El dub queda acotado aparte por su propio tope de duración,
  así que este síntoma se centra en `/transcribe`.
- **Criterio:** una prueba de integración del router envía a `/transcribe` un cuerpo de
  más de 2 MB y de menos que el techo decidido y no recibe 413; otro cuerpo por encima
  del techo recibe un rechazo con `reason` identificable.

### 2. El dub por daemon antiguo corta la transcripción a los 1500 ms

- **Severidad:** baja. Solo afecta a un daemon anterior a la ruta `/dub`, que hoy no
  debería quedar en uso.
- **Síntoma (probable):** con un daemon que responde 404 a `POST /dub`, `speech dub
  --daemon` con un audio de más de unos 14 s falla con exit 5 y `reason`
  `daemon_unreachable` («Daemon inalcanzable en 127.0.0.1:8765 (timeout 1500ms)»),
  aunque el daemon esté sano y siga transcribiendo.
- **Reproducción:** levantar un daemon sin la ruta `/dub` y lanzar `speech dub --daemon`
  con un WAV de 20 s o más.
- **Causa (confirmada por lectura; la consecuencia con audio largo es probable):** cuando
  `POST /dub` responde 404, `speech dub` degrada a `dub_compose_via_daemon`
  (`src/main.rs`). Esa función envía el `POST /transcribe` (respuesta única, no stream)
  envuelto en `tokio::time::timeout` de 1500 ms, y tanto el vencimiento como cualquier
  error de red se mapean a `daemon_unreachable` (exit 5). Parakeet transcribe en una
  sola pasada con un RTF lineal de ~0,11, es decir, ~0,11 s por segundo de audio: 1,5 s
  alcanzan para unos 14 s de audio.
- **Esperado:** usar el mismo esquema de consumo que la ruta principal (el plazo corto
  solo para conectar, sin corte para la inferencia) o un plazo proporcional a la
  duración del audio.
- **Criterio:** una prueba con un daemon simulado que tarda más de 1,5 s en responder a
  `/transcribe` comprueba que el dub por composición no falla con `daemon_unreachable`.

## Diagnóstico sugerido

Confirmar los dos síntomas con una reproducción: el 1 con un WAV de más de 49 s contra
el daemon actual y el 2 con un daemon sin `/dub`. El 1 es el prioritario porque afecta
al daemon vigente. Su corrección necesita decidir el techo del cuerpo y que el cliente
lea el `reason` del rechazo, lo que enlaza con el mapeo único de errores.

## Criterio de aceptación

Se cumplen los criterios de los dos síntomas.

## Relacionados

- [mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md](mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md):
  el mapeo único de `reason` a código de salida que necesita el cliente para dar un
  error identificable.
- Techo de duración del push-to-talk (`AVI_PUSH_TO_TALK_MAX_SECS`) y tope de duración del
  dub.
