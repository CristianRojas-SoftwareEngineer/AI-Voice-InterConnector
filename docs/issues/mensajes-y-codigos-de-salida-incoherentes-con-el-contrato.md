# Mensajes, códigos de salida y datos de eventos que no cuadran con el contrato

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja; media en la síntesis por daemon con el modelo ausente |
| Tipo | contrato, diagnóstico |
| Componente | CLI (`src/main.rs`) y daemon (`crates/avi-daemon/src/lib.rs`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1; ninguna de las causas depende de la plataforma |
| Reproducibilidad | siempre (síntomas 1 y 2); por verificar (síntomas 3 y 4) |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 (síntomas 1 y 2); revisión del código al diseñar la corrección de `--text` sin tope, 2026-09-29 (síntomas 3 y 4) |

## Resumen

La CLI no tiene un único punto donde traducir un error a su código de salida, su
`reason` y su texto. Cada cliente de la vía daemon lleva su propia copia del mapeo, uno
no lleva ninguna, y `daemon_unreachable` se reutiliza como comodín para errores que no
tienen que ver con la conexión. El resultado son mensajes con el prefijo duplicado,
diagnósticos que culpan al daemon sin motivo y códigos de salida distintos para la misma
causa según la vía. En el mismo cliente de síntesis, un campo informativo del stream usa
otra unidad que la regla que describe.

## Entorno

Síntomas 1 y 2: binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1,
con todos los modelos provisionados y el daemon en el puerto por defecto.

Síntomas 3 y 4: lectura del código fuente de v0.25.0. No se han reproducido todavía.

## Precondiciones

Las propias de cada síntoma.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

Un único mapeo de `reason` a código de salida, compartido por la vía directa y por todos
los clientes de la vía daemon; mensajes sin prefijo propio, que lo pone quien los
imprime; y cada error clasificado por su causa real, no por la vía que lo produjo.

## Impacto y workaround

Los mensajes engañosos hacen perder tiempo de diagnóstico. Los códigos de salida
equivocados afectan a los consumidores programados, que reciben un código que no
coincide con el contrato. Workaround para la síntesis con el modelo ausente: ejecutar
`doctor` antes, o `--no-daemon`, que sí sale con el código del contrato.

## Evidencia

Recogida en cada síntoma.

## Análisis de causa

### Causa común (confirmada por lectura)

La traducción de `reason` a código de salida está copiada en cada cliente de la vía
daemon de `src/main.rs`: la traducción (`translate_via_daemon`), el clonado, y la
transcripción y el dub. `daemon_synthesize_wav`, que usan `speech synthesize`,
`speech say` y la composición del dub, no tiene ninguna. Además, `daemon_unreachable`
se emplea para errores que no son de conexión.

### 1. Prefijo `Error:` duplicado

- **Síntoma:** `speech say --text hola --temperature 5` imprime `Error: Error:
  --temperature debe ser mayor que 0 y como máximo 2.0.` (exit 2, correcto).
- **Causa (confirmada):** `validate_temperature` (`src/main.rs`) construye el `CliError`
  con un mensaje que ya empieza por `Error:`. El manejador de errores de `main` añade
  su propio `Error: {}`. Lo mismo ocurre con el mensaje equivalente del daemon
  (`crates/avi-daemon/src/lib.rs`), que viaja en el JSON.
- **Esperado:** un único prefijo. El mensaje del error no debe llevar prefijo; lo pone
  quien lo imprime.
- **Criterio:** ningún mensaje de `CliError` ni del daemon empieza por `Error:`; una
  prueba golden del caso de `--temperature`.

### 2. `--daemon` en comandos solo locales dice «Daemon inalcanzable»

- **Síntoma:** con el daemon **activo**, `voice list --daemon`, `speech list --daemon` y
  `speech play --daemon` salen con exit 5 y el mensaje «Daemon inalcanzable en
  127.0.0.1:8765».
- **Causa (confirmada):** `require_local` (`src/main.rs`) rechaza `--daemon` en los
  comandos que no se delegan, pero reutiliza el código, el `reason` y el texto del
  daemon inalcanzable.
- **Esperado:** rechazarlo es lo previsto. El mensaje debe decir que ese comando se
  ejecuta siempre en local y no admite `--daemon`, y el código más adecuado sería el de
  uso inválido (2). Hay que decidirlo contra el contrato.
- **Criterio:** con el daemon activo o detenido, el mensaje no afirma que el daemon sea
  inalcanzable.

### 3. La síntesis por daemon devuelve exit 1 para errores que el contrato asigna a otro código

- **Severidad:** baja; sube a media si se usa el daemon con modelos ausentes.
- **Síntoma (por verificar):** con el daemon en marcha y el modelo TTS sin provisionar,
  `speech synthesize --daemon --text hola --label x` sale con 1 y `reason`
  `model_missing`, cuando el contrato asigna 4 a un modelo no provisionado.
- **Reproducción:** arrancar el daemon, retirar o renombrar el modelo TTS y lanzar el
  comando anterior. Anotar el código de salida y el `reason`.
- **Causa (confirmada por lectura):** `daemon_synthesize_wav` (`src/main.rs`) convierte
  cualquier evento `error` del stream en `ExitCode::Error`. Conserva el `reason` pero no
  el código que le corresponde. Solo el 400 de validación de entrada (`empty_text`,
  `text_too_long`) conserva su `reason` y sale con 2; cualquier otra respuesta distinta
  de 2xx sale con exit 1 y `daemon_error`, sin `reason`. Es alcanzable: en `speech
  synthesize`, `require_model_provisioned` se ejecuta después de la rama del daemon, así
  que con el daemon activo el chequeo de modelo lo hace el `synthesize_handler`, que
  emite `model_missing`. `translate_via_daemon` sí traduce `reason` a código
  (`model_missing` → 4, `empty_text` → 2, etc.), pero con su propia copia del mapeo, y la
  síntesis no tiene ninguna.
- **Esperado:** un único mapeo de `reason` a código de salida, compartido por todos los
  clientes de la vía daemon, que también lea el `reason` del cuerpo de las respuestas
  distintas de 2xx, no solo el del 400 de validación.
- **Criterio:** con el daemon activo y el modelo TTS ausente, `speech synthesize` y
  `speech say` salen con 4 y `model_missing`. Una prueba golden lo cubre.

### 4. El evento `start` de la síntesis por daemon mide `text_length` en bytes

- **Severidad:** baja. Es un campo informativo y no se conoce ningún consumidor que
  dependa de él.
- **Síntoma:** para un texto con acentos, el `text_length` del evento `start` del stream
  de `/synthesize` es mayor que el número de caracteres del texto. Con «canción», da 8
  en vez de 7.
- **Causa (confirmada por lectura):** `synthesize_handler`
  (`crates/avi-daemon/src/lib.rs`) emite `text_owned.len()`, que en Rust es la longitud
  en bytes UTF-8. El tope de longitud del contrato se mide en caracteres, así que el dato
  del evento y la regla no usan la misma unidad.
- **Esperado:** `text_length` en caracteres (`chars().count()`), la misma unidad del
  tope. Como el evento es un contrato de máquina, el cambio se anota en el CHANGELOG.
- **Criterio:** una prueba del handler con un texto con acentos comprueba
  `text_length` igual al número de caracteres.

## Diagnóstico sugerido

Extraer una única función de `reason` a código de salida y usarla en todos los clientes
de la vía daemon, incluida la síntesis, leyendo el `reason` del cuerpo de cualquier
respuesta distinta de 2xx. En el mismo parche caben el prefijo duplicado, el mensaje de
`require_local` y el cambio de unidad de `text_length`, que es de una línea. El código
de salida de `--daemon` en comandos solo locales necesita antes una decisión contra el
contrato.

## Criterio de aceptación

Se cumplen los criterios de los cuatro síntomas y no queda más de un mapeo de `reason` a
código de salida en la CLI.

## Relacionados

- [daemon-rechaza-o-corta-audios-largos.md](daemon-rechaza-o-corta-audios-largos.md):
  sus dos síntomas acaban en `daemon_unreachable` o en `daemon_error` sin `reason`, y
  dependen del mapeo único para dar un error identificable.
- Contrato de la CLI, reglas de `--text` (la unidad del tope) y tabla de códigos de
  salida.
