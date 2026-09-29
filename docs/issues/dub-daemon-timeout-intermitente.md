# `speech dub --daemon` falla de forma intermitente por timeout

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | funcional |
| Componente | `avi-daemon` (`dub_handler`, deadlines y latidos del stream) y cliente de dub en `src/main.rs` |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 |
| Reproducibilidad | intermitente: 3 fallos en unos 10 dubs por daemon (1 de un tipo, 2 del otro) |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

`speech dub` por el daemon falla a veces con uno de estos dos síntomas:

- **A.** `synthesis_timeout`: la síntesis vence el deadline de 8 s.
- **B.** Exit 5 `daemon_unreachable`: «sin eventos del daemon en 1500ms en dub» cuando el
  dub se lanza en el primer segundo tras `daemon start`.

Los demás comandos por daemon (`translate`, `transcribe`, `synthesize`, `say`) no fallaron
en las mismas condiciones, incluidas 7 peticiones concurrentes. En modo automático el
cliente cae a la vía local y el usuario solo nota lentitud. Con `--daemon` explícito, el
comando falla.

## Entorno

- Binario: v0.25.0 instalado, todos los modelos provisionados (incluido CT2).
- SO: Windows 11. CPU sin GPU dedicada para el motor.
- Daemon en el puerto por defecto, sin `AVI_DAEMON_PORT`.
- Audio de entrada: WAV de unos 7 s en español, generado con la propia CLI.

## Precondiciones

- Síntoma A: daemon en marcha y caliente (`warm`), con una síntesis previa por daemon
  (`speech say`) justo antes del dub.
- Síntoma B: daemon recién arrancado, todavía en `warming`.

## Pasos para reproducir

Síntoma A:

```powershell
ai-voice-interconnector daemon start
# esperar a warm
ai-voice-interconnector speech say --daemon --text "Hola" --source-language es-latam
ai-voice-interconnector speech dub --daemon --audio e2e_es.wav --source-language es-latam --target-language en
```

Síntoma B:

```powershell
ai-voice-interconnector daemon stop
ai-voice-interconnector daemon start; Start-Sleep 1
ai-voice-interconnector speech dub --daemon --audio e2e_es.wav --source-language es-latam --target-language en
```

## Resultado observado

- Síntoma A: 1 de 1 en el primer dub tras `say`. Error `synthesis_timeout`, «La síntesis
  venció el deadline de 8 s», exit 1. Los tres reintentos inmediatos terminaron bien, en
  unos 12 s cada uno.
- Síntoma B: 2 de 2 lanzando el dub a 1 s del arranque. Error «Daemon inalcanzable en
  127.0.0.1:8765 (sin eventos del daemon en 1500ms en dub)», exit 5. Con el daemon en
  `warming`, otros dubs lanzados algo más tarde terminaron bien.

## Resultado esperado

- Un dub válido con el daemon activo termina bien, aunque tarde más, mientras el daemon
  siga vivo: el cliente consume el stream con 1500 ms de inactividad y un failsafe de
  120 s justo para tolerar inferencias largas, y el daemon emite latidos cada 500 ms.
- Con el daemon en `warming`, la petición espera al warmup o se rechaza con un código
  que diga que el daemon está calentando. No debe presentarse como «daemon
  inalcanzable».

## Impacto y workaround

`--daemon` falla en la operación más pesada del producto justo en los momentos típicos
de uso (tras arrancar, o tras otra síntesis). Los scripts que fuerzan el daemon reciben
un fallo espurio, y el exit 5 del síntoma B es engañoso.

Workaround: reintentar, usar el modo automático (sin `--daemon`), o esperar a que
`daemon status` muestre `warm` antes del primer dub.

## Evidencia

- `crates/avi-daemon/src/lib.rs`:
  - `STREAM_HEARTBEAT` = 500 ms. `with_heartbeats` envuelve las fases de STT y de
    traducción.
  - En `dub_handler`, antes de sintetizar, se hace
    `state.synthesis_lock.lock().await` **sin emitir latidos** durante la espera. Los
    latidos de la fase de síntesis (bucle `select!` con `STREAM_HEARTBEAT`) empiezan
    después de obtener el lock.
  - `warm_voice_engine`, el warmup del arranque, toma ese mismo lock
    (`synthesis_lock.blocking_lock()`) durante toda la síntesis testigo. Su propia
    documentación la sitúa en unos 18-20 s en frío, bajo el techo `WARMUP_DEADLINE` =
    40 s. Esa documentación considera que en el arranque el lock «no está disputado», y
    no es así si llega una petición durante el warmup.
  - El deadline de la fase de síntesis (`phase_deadline = sleep(SYNTH_DEADLINE)`) empieza
    a contar al obtener el lock.
  - `SYNTH_DEADLINE` = 8 s. Su comentario lo justifica para una síntesis **corta con el
    residente caliente** y para ganarle la carrera a un «corte ciego del cliente a los
    10 s». Ese corte ya no existe: el cliente consume un stream con inactividad de
    1500 ms y failsafe de 120 s. Además, la síntesis de un dub es la del texto transcrito
    y traducido completo, no una frase corta.
  - `synthesize_handler` también toma `synthesis_lock`, así que un `say` o un
    `synthesize` en curso retrasa la fase de síntesis del dub.
- `src/main.rs`, cliente de dub:
  - la respuesta inicial tiene un `tokio::time::timeout` de 1500 ms («timeout dub
    1500ms»);
  - el stream usa `STREAM_INACTIVITY_TIMEOUT` = 1500 ms («sin eventos del daemon en
    1500ms en {}»). El síntoma B dio este segundo mensaje, así que la conexión y la
    respuesta inicial funcionaron y lo que faltó fueron los latidos.
- La síntesis de un dub correcto tarda unos 12 s en total, cerca del deadline de 8 s
  para la fase de síntesis.

## Análisis de causa

1. **Síntoma B: probable, con el mecanismo confirmado en el código.** Un dub lanzado a
   1 s del arranque termina STT y traducción con latidos y después se queda esperando
   `synthesis_lock`, que el warmup retiene durante su síntesis testigo (~18-20 s). Esa
   espera no emite latidos, así que a los 1500 ms el cliente corta con «sin eventos del
   daemon» y lo presenta como `daemon_unreachable`. Encaja con que solo falle en los
   primeros segundos tras `daemon start` y con que los dubs algo posteriores funcionen.
   Falta confirmarlo con marcas de tiempo.
2. **Síntoma A: probable.** `SYNTH_DEADLINE` es demasiado corto para la síntesis de un
   dub. Se dimensionó para frases cortas con el residente caliente y contra un corte del
   cliente que ya no existe. El texto del dub sale del audio de entrada y la síntesis
   ronda los 8 s. Como el residente es de una sola voz y la síntesis anterior (`say`)
   puede dejarlo en otro estado, el primer dub supera el deadline y los siguientes no.
   Falta medir la duración real de la fase.
3. **Por verificar.** `synthesize_handler` también espera `synthesis_lock` antes de su
   bucle de latidos. Si su cliente aplica el mismo corte de inactividad, `synthesize` y
   `say` podrían fallar igual durante el warmup. No se observó en la prueba, pero en esa
   vía no se lanzaron peticiones en el primer segundo tras el arranque.
4. Descartado: no es un problema de red ni de descubrimiento de dirección. Las demás
   operaciones concurrentes por daemon funcionaron en el mismo arranque.

## Diagnóstico sugerido

1. Instrumentar `dub_handler` con marcas de tiempo por fase (decodificación, STT,
   traducción, síntesis) y por latido emitido, en el log del daemon. Hoy el daemon no
   tiene log propio; los únicos logs en `data/logs/` son los del motor.
2. Repetir el síntoma A 20 veces con audio de 5, 10 y 20 s, midiendo la fase de
   síntesis, para dimensionar el deadline o hacerlo proporcional a la longitud del texto.
3. Para el síntoma B, lanzar un dub cada 250 ms durante el warmup y confirmar que el
   último evento antes del corte es el latido de traducción y que el siguiente habría
   llegado al liberar el warmup el lock.
4. Emitir latidos también durante la espera de `synthesis_lock`, en `dub_handler` y en
   `synthesize_handler`, con un `stage` que diga que la petición está en cola o que el
   daemon está calentando.
5. Revisar si `SYNTH_DEADLINE` debe existir, crecer con la longitud del texto, o dejarse
   al failsafe de 120 s del cliente, y actualizar su comentario, que se apoya en un corte
   del cliente que ya no existe.

## Criterio de aceptación

- 20 de 20 dubs por `--daemon` con el daemon `warm`, alternados con `say`, terminan con
  exit 0.
- Un dub lanzado a menos de 1 s de `daemon start` termina con exit 0 (tras esperar al
  warmup) o sale con un código y mensaje que indiquen que el daemon está calentando,
  nunca con «daemon inalcanzable».
- Una prueba del daemon que simule un motor lento (síntesis > 8 s) y compruebe que el
  stream sigue emitiendo latidos y termina bien.

## Relacionados

- [daemon-start-puerto-ocupado.md](daemon-start-puerto-ocupado.md).
- [motor-tts-escucha-en-todas-las-interfaces.md](motor-tts-escucha-en-todas-las-interfaces.md):
  el mismo residente.
