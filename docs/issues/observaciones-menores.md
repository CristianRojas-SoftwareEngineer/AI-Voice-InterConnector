# Observaciones menores: mensajes, residuos, ruido de salida, límites y contratos entre vías

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja en general; media en las observaciones 13 y 17 (cada ficha de la 13 a la 17 indica la suya) |
| Tipo | diagnóstico (mensajes), funcional (residuos y límites), contrato y documentación |
| Componente | varios; se indica en cada observación |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11; las observaciones 13 a 17 no dependen de la plataforma |
| Reproducibilidad | siempre, salvo que se indique otra cosa |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 (1 a 12); revisión del código al diseñar la corrección de `--text` sin tope y de `--audio` inexistente, 2026-09-29 (13 a 15); revisión del código de la vía daemon de transcripción y dub, 2026-09-29 (16 y 17) |

## Resumen

Diecisiete defectos agrupados aquí porque ninguno justifica, por ahora, un documento
propio. En los doce primeros el comando termina con el resultado correcto, pero el
mensaje engaña, queda basura en disco o la documentación no cuadra con la realidad. Los
cinco últimos salieron de leer el código y no de la prueba E2E: uno puede producir un
resultado incorrecto (13, severidad media), dos son incoherencias entre la vía
directa y la vía daemon (14 y 15) y dos son límites de tamaño o de tiempo de la vía
daemon con audio largo (16 y 17, esta última de severidad media). Cada observación tiene una ficha breve (síntoma,
reproducción, esperado, causa y criterio). Si alguna crece al diagnosticarla, se separa
a su propio documento con la plantilla completa.

## Entorno

Observaciones 1 a 12: binario v0.25.0 instalado desde cero en Windows 11 con
PowerShell 5.1, con todos los modelos provisionados (incluido el de clonado), y daemon
en el puerto por defecto salvo que se indique.

Observaciones 13 a 17: lectura del código fuente de v0.25.0. Ninguna se ha reproducido
todavía; cada ficha indica cómo hacerlo.

## Precondiciones

Las propias de cada observación.

## Pasos para reproducir

Se detallan en cada observación, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada observación.

## Resultado esperado

Se detalla en cada observación.

## Impacto y workaround

Ninguna de las observaciones 1 a 12 bloquea un caso de uso. Los mensajes engañosos (1 a
5) hacen perder tiempo de diagnóstico y confunden a los scripts que leen el `status`.
Los residuos (7 a 10) ocupan disco y ensucian `doctor`.
Workaround general: `cleanup` barre los temporales y `daemon stop` o `daemon start`
reclaman el residente huérfano.

La observación 13 sí puede afectar al resultado: un WAV truncado se transcribe o se
dobla incompleto sin ningún aviso. Workaround: comprobar la integridad del WAV antes de
pasarlo. Las observaciones 14 y 15 afectan solo a los
consumidores programados: reciben un código de salida o un dato que no coincide con el
contrato.

La observación 17 impide transcribir por daemon una grabación de más de ~49 s.
Workaround: `--no-daemon`, que no tiene ese límite, o dividir el audio. La 16 solo
afecta a un daemon anterior a la ruta `/dub`; workaround: actualizar el daemon.

## Evidencia

Recogida en cada observación.

## Análisis de causa

### Mensajes engañosos o mal formados

#### 1. Prefijo `Error:` duplicado

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

#### 2. `daemon stop` sin daemon dice «Señal de apagado enviada»

- **Síntoma:** con el daemon detenido, `daemon stop` imprime «Señal de apagado enviada
  al daemon en 127.0.0.1:8765.» y el JSON devuelve `status: "shutdown_sent"`,
  `daemon: "stopped"`.
- **Causa (confirmada):** en la rama de `daemon stop` de `src/main.rs`, el caso
  «ni activo ni vivo» borra el pidfile y reutiliza el mensaje del apagado real. El exit
  0 es correcto porque la operación es idempotente.
- **Esperado:** un mensaje del tipo «El daemon no estaba en ejecución» y un `status`
  propio (por ejemplo `not_running`). Si cambia el `status`, hay que actualizar el
  contrato.
- **Criterio:** texto y `status` distintos para «no había daemon» y «se apagó».

#### 3. `--daemon` en comandos solo locales dice «Daemon inalcanzable»

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

#### 4. El plan de `self install` anuncia «se añadirá» un PATH que ya existe

- **Síntoma:** al reinstalar (`self install` sobre una instalación existente, estado
  `repaired`), el resumen previo dice `PATH: se añadirá <bin> en el PATH del usuario`,
  aunque la entrada ya está en `HKCU\Environment\Path`.
- **Causa (por verificar):** `compose_summary` (`crates/avi-lifecycle/src/install.rs`)
  tiene una rama «ya está en el PATH; no se modifica» para cuando el plan no cambia
  nada (`path_plan.is_noop()`). En Windows, el plan no se reconoce como no-op con la
  entrada ya presente. Hay que ver cómo compara el plan la entrada existente
  (mayúsculas, barra final, `REG_EXPAND_SZ` con variables sin expandir).
- **Esperado:** «ya está en el PATH; no se modifica».
- **Criterio:** prueba del plan de PATH en Windows con la entrada ya registrada.

#### 5. `self uninstall --dry-run --json` devuelve `status: "uninstalled"`

- **Síntoma:** el simulacro no modifica nada (correcto), pero el sobre dice
  `status: "uninstalled"` junto a `dry_run: true`.
- **Causa (confirmada por la salida):** el `status` describe el resultado que tendría
  la operación, no lo que ha pasado. Un consumidor que mire solo `status` cree que
  desinstaló.
- **Esperado:** un `status` que no afirme el hecho, por ejemplo `planned` o
  `would_uninstall`. Cambiarlo toca el contrato JSON (`schema_version`).
- **Criterio:** decisión registrada en el contrato; prueba golden del dry-run.

### Salida ruidosa

#### 6. Logs internos en la salida humana

- **Síntoma:** en modo humano, la descarga de modelos (backend xet de Hugging Face) y
  `voice clone` vuelcan en la terminal las trazas de progreso y los logs del motor, que
  tapan el resumen del comando.
- **Causa (por verificar):** la biblioteca de descarga escribe su propio progreso y el
  motor hereda o reenvía su stderr durante el clonado.
- **Esperado:** en modo humano, una sola línea de progreso del producto; las trazas
  internas van al log.
- **Criterio:** `voice clone` y `setup` en modo humano muestran solo mensajes propios.

### Residuos en disco

#### 7. `speech say` deja WAV en `%TEMP%`

- **Síntoma:** cada `speech say` deja un `avi_say_<pid>.wav` en el directorio temporal.
  Solo desaparecen con `cleanup`.
- **Causa (confirmada):** `src/main.rs` escribe `avi_say_{pid}.wav` en
  `std::env::temp_dir()` en las vías directa y daemon, y no lo borra tras reproducirlo.
- **Esperado:** el temporal se borra al terminar la reproducción, también si falla.
- **Criterio:** tras `speech say`, `%TEMP%` no contiene `avi_say_*.wav`.

#### 8. `daemon.ready` queda en `data/` tras `daemon stop`

- **Síntoma:** después de un `daemon stop` limpio, `data/daemon.ready` sigue en disco.
- **Causa (probable):** el fichero solo se borra al empezar el siguiente `daemon start`,
  que lo invalida antes de lanzar el hijo. El apagado no lo retira.
- **Esperado:** `daemon stop` borra el fichero ready junto con el pidfile.
- **Criterio:** tras `daemon stop`, `data/` no contiene `daemon.ready` ni `daemon.pid`.

#### 9. Logs del motor sin rotación y sin log propio del daemon

- **Síntoma:** cada arranque del motor crea `data/logs/qwen3-tts_<pid>_<ms>.log`; al
  final de la sesión había 21. El daemon no escribe log propio, así que sus errores
  (por ejemplo, una caída del servidor después de estar listo) no quedan en ningún sitio.
- **Causa (confirmada):** `crates/avi-tts/src/lib.rs` nombra un archivo nuevo por
  proceso y nada los poda. El hijo del daemon se lanza con stdout y stderr descartados
  (`spawn_background`, `crates/avi-daemon/src/spawn.rs`).
- **Esperado:** retención acotada de los logs del motor (por número o por antigüedad) y
  un log del daemon en `data/logs/`.
- **Criterio:** tras N arranques quedan como mucho los K logs más recientes; existe un
  log del daemon con el arranque, el bind y los errores.

#### 10. `self update` deja `.old-*` y el `.zip` en el directorio del programa

- **Síntoma:** tras `self update --force`, el directorio del programa contiene el
  binario anterior aparcado (`.old-*`) y el `.zip` descargado. `doctor` informa
  `pending_artifacts` con un aparcado hasta que el barrido lo recoge, un rato después.
- **Causa (por verificar):** el binario anterior no se puede borrar en caliente en
  Windows y se aparca por diseño; queda por ver por qué el `.zip` no se borra tras
  extraerlo y por qué el barrido tarda.
- **Esperado:** el `.zip` se borra al extraerlo, y el aparcado se recoge en la siguiente
  invocación, sin que `doctor` lo marque como fallo mientras tanto.
- **Criterio:** tras `self update` y otra invocación cualquiera, el directorio del
  programa solo contiene lo instalado y `doctor` pasa.

### Procesos

#### 11. Un kill duro del daemon deja huérfano al motor residente

- **Síntoma:** con `Stop-Process -Force` sobre el daemon (con o sin `--auto-restart`),
  el `qwen_tts` residente sigue vivo escuchando en `127.0.0.1:8766`, el pidfile queda
  obsoleto y `daemon status` dice `stopped`. El siguiente `daemon start` o `daemon stop`
  lo detecta y lo reclama.
- **Causa (confirmada por el comportamiento):** el residente es un proceso
  independiente y no muere con su padre. `--auto-restart` supervisa dentro del proceso,
  así que no puede actuar si el proceso muere.
- **Esperado:** el residente muere con el daemon. En Windows se puede asociar a un Job
  Object con `KILL_ON_JOB_CLOSE`; en Unix, con `PR_SET_PDEATHSIG` (Linux) o que el
  residente vigile a su padre.
- **Criterio:** tras matar el daemon a la fuerza, en menos de unos segundos no queda
  ningún `qwen_tts` ni nada escuchando en 8766.

### Documentación

#### 12. Tamaño de los modelos subestimado

- **Síntoma:** README, `docs/CLI/commands/SETUP.md`, `docs/GOAL.md`,
  `docs/MANUAL-VALIDATION.md` y `docs/CLI/commands/CLEANUP.md` hablan de ~9 GB para la
  selección base y ~11,5 GB con `--with-voice-cloning`. Con el clonado provisionado, la
  raíz de modelos ocupaba 14 GB en disco.
- **Causa (por verificar):** las cifras probablemente no cuentan los derivados CT2 ni la
  caché del backend xet de descarga. Hay que medir cada componente.
- **Esperado:** cifras que coincidan con lo que se ocupa en disco tras `setup`, con un
  margen explícito.
- **Criterio:** los documentos citados dan la misma cifra, medida sobre una instalación
  limpia.

### Límites de entrada

#### 13. Un WAV truncado se carga incompleto sin error

- **Severidad:** media. El resultado es incorrecto y no hay aviso, aunque la entrada
  dañada es poco frecuente.
- **Síntoma (por verificar):** un WAV cuya cabecera declara más datos de los que contiene
  el archivo se transcribe, se dobla o se usa como referencia de clonado con solo la
  parte legible, y el comando termina bien.
- **Reproducción:** cortar los últimos bytes de un WAV válido y pasarlo a
  `speech transcribe --no-daemon --audio truncado.wav --source-language es-latam`.
- **Causa (confirmada por lectura; el comportamiento exacto de `hound` ante el corte
  está por verificar):** `load_wav_16k_mono_pcm` y `load_wav_24k_mono_pcm`
  (`crates/avi-audio/src/lib.rs`) leen las muestras con `.filter_map(Result::ok)`, así
  que descartan en silencio cualquier muestra que no se pueda leer y devuelven lo demás
  como si el archivo estuviera completo. La carga para reproducción del mismo archivo
  usa el mismo patrón.
- **Esperado:** un error de lectura de muestras interrumpe la carga y se clasifica como
  audio inválido (exit 2, `invalid_audio`, el mismo criterio que para un archivo que
  existe pero no es un WAV válido). Antes de endurecerlo hay que decidir qué tolerancia
  tienen los WAV reales con el tamaño de datos mal escrito en la cabecera, como los de
  algunas grabadoras que escriben en streaming.
- **Criterio:** una prueba unitaria con un WAV truncado comprueba el resultado decidido
  (error o aviso), y ningún camino de carga descarta muestras en silencio.

### Contratos entre la vía directa y la vía daemon

#### 14. La síntesis por daemon devuelve exit 1 para errores que el contrato asigna a otro código

- **Severidad:** baja; sube a media si se usa el daemon con modelos ausentes.
- **Síntoma (por verificar):** con el daemon en marcha y el modelo TTS sin provisionar,
  `speech synthesize --daemon --text hola --label x` sale con 1 y `reason`
  `model_missing`, cuando el contrato asigna 4 a un modelo no provisionado.
- **Reproducción:** arrancar el daemon, retirar o renombrar el modelo TTS y lanzar el
  comando anterior. Anotar el código de salida y el `reason`.
- **Causa (confirmada por lectura):** `daemon_synthesize_wav` (`src/main.rs`), que usan
  `speech synthesize`, `speech say` y la composición del dub por daemon, convierte
  cualquier evento `error` del stream en `ExitCode::Error`. Conserva el `reason` pero no
  el código que le corresponde. Solo el 400 de validación de entrada (`empty_text`,
  `text_too_long`) conserva su `reason` y sale con 2; cualquier otra respuesta distinta
  de 2xx sale con exit 1 y `daemon_error`, sin `reason`. Es alcanzable: en `speech
  synthesize`, `require_model_provisioned` se ejecuta después de la rama del daemon, así
  que con el daemon activo el chequeo de modelo lo hace el `synthesize_handler`, que
  emite `model_missing`. `translate_via_daemon` sí traduce `reason` a código
  (`model_missing` → 4, `empty_text` → 2, etc.), pero con su propia copia del mapeo, y la
  copia de la síntesis no lo tiene.
- **Esperado:** un único mapeo de `reason` a código de salida, compartido por todos los
  clientes de la vía daemon, que también lea el `reason` del cuerpo de las respuestas
  distintas de 2xx, no solo el del 400 de validación.
- **Criterio:** con el daemon activo y el modelo TTS ausente, `speech synthesize` y
  `speech say` salen con 4 y `model_missing`. Una prueba golden lo cubre.

#### 15. El evento `start` de la síntesis por daemon mide `text_length` en bytes

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

### Límites de tamaño y de tiempo en la vía daemon

#### 16. El dub por daemon antiguo corta la transcripción a los 1500 ms

- **Severidad:** baja. Solo afecta a un daemon anterior a la ruta `/dub`, que hoy no
  debería quedar en uso.
- **Síntoma (probable):** con un daemon que responde 404 a `POST /dub`, `speech dub
  --daemon` con un audio de más de unos 14 s falla con exit 5 y `reason`
  `daemon_unreachable` («Daemon inalcanzable en 127.0.0.1:8765 (timeout 1500ms)»),
  aunque el daemon esté sano y siga transcribiendo.
- **Reproducción:** levantar un daemon sin la ruta `/dub` y lanzar `speech dub --daemon`
  con un WAV de 20 s o más. No se ha reproducido porque exige ese daemon antiguo.
- **Causa (confirmada por lectura; la consecuencia con audio largo es probable):** cuando
  `POST /dub` responde 404, `speech dub` degrada a `dub_compose_via_daemon`
  (`src/main.rs`). Esa función envía el `POST /transcribe` (respuesta única, no stream)
  envuelto en `tokio::time::timeout` de 1500 ms, y tanto el vencimiento como cualquier
  error de red se mapean a `daemon_unreachable` (exit 5). `docs/DAEMON-MODE.md` documenta
  que Parakeet transcribe en una sola pasada con RTF lineal de ~0,11, es decir, ~0,11 s
  por segundo de audio: 1,5 s alcanzan para unos 14 s de audio.
- **Esperado:** que la duración del audio no convierta un daemon sano en «inalcanzable».
  Usar el mismo esquema de consumo que la ruta principal (sin corte de 1500 ms para la
  inferencia; el plazo corto solo para conectar) o un plazo proporcional a la duración
  del audio.
- **Criterio:** una prueba con un daemon simulado que tarda más de 1,5 s en responder a
  `/transcribe` comprueba que el dub por composición no falla con `daemon_unreachable`.

#### 17. El daemon rechaza con 413 los audios de más de ~49 s en `/transcribe`

- **Severidad:** media. `speech transcribe --daemon` (y el modo automático con el daemon
  activo) falla con un mensaje sin utilidad ante grabaciones largas, que el producto
  permite (techo de 300 s en push-to-talk).
- **Síntoma (probable):** `speech transcribe --audio largo.wav` (o `--mic` de más de
  ~49 s) con el daemon activo termina en exit 1 con `reason` `daemon_error` y el mensaje
  «El daemon devolvió 413 Payload Too Large». Con `--no-daemon` el mismo audio se
  transcribe bien.
- **Reproducción:** con el daemon activo, ejecutar `speech transcribe --daemon --audio
  largo.wav --source-language es-latam` con un WAV de 16 kHz mono de 60 s. No se ha
  reproducido.
- **Causa (confirmada por lectura del código; el 413 concreto es probable, deducido del
  comportamiento por defecto de la biblioteca):** `build_router_with_state`
  (`crates/avi-daemon/src/lib.rs`) construye el `Router` sin `DefaultBodyLimit`, y
  `transcribe_handler` (igual que `dub_handler`) recibe el cuerpo con el extractor
  `Json`. El proyecto usa axum 0.7.9 (`Cargo.lock`), cuyos extractores de cuerpo aplican
  por defecto un límite de 2 MB. `transcribe_via_daemon` (`src/main.rs`) envía el PCM
  i16 little-endian de 16 kHz mono (32 000 bytes por segundo) en base64, que aumenta el
  tamaño un tercio: 2 MB de cuerpo equivalen a unos 49 s de audio. Un push-to-talk de
  300 s (`AVI_PUSH_TO_TALK_MAX_SECS` por defecto) genera ~12,8 MB de cuerpo. En modo
  automático no hay reintento local: `route_to_daemon` decide antes de leer el audio y
  `transcribe_via_daemon` devuelve el error tal cual, sin cuerpo con `reason`.
- **Esperado:** el daemon acepta audios hasta el techo del push-to-talk, o el cliente
  rechaza con un `reason` claro antes de enviar. Fijar un `DefaultBodyLimit` explícito en
  el router, coherente con ese techo (300 s ≈ 12,8 MB en base64, con margen), o
  comprobar la duración en el cliente. El dub queda acotado aparte por su propio tope
  de duración, así que esta ficha se centra en `/transcribe`.
- **Criterio:** una prueba de integración del router envía un cuerpo de más de 2 MB y de
  menos que el techo decidido a `/transcribe` y no recibe 413; otro cuerpo por encima
  del techo recibe un rechazo con `reason` identificable.

## Diagnóstico sugerido

Resolver primero las observaciones de causa confirmada y corrección local (1, 2, 3, 7 y
8), que caben en un mismo parche. Las que tocan el contrato JSON (2 y 5) necesitan una
decisión sobre el `status` antes de implementarlas. Las que están por verificar (4, 6,
10 y 12) necesitan una reproducción instrumentada.

La 13 necesita decidir la tolerancia con cabeceras mal escritas antes de tocar la carga.
La 15 es un cambio de una línea que puede ir en el mismo parche que la 14.

La 16 y la 17 se confirman con una reproducción: la 16 con un daemon sin `/dub` y la 17
con un WAV de más de 49 s contra el daemon actual. La 17 es la prioritaria: afecta al
daemon vigente y a grabaciones legítimas.

## Criterio de aceptación

Cada observación tiene su criterio en su ficha. El documento se da por resuelto cuando
todas están cerradas o separadas a su propio documento.

## Relacionados

- Contrato de la CLI, reglas de `--text` y de `--audio` (observaciones 13 a 15: la
  clasificación de un audio inválido, y la unidad y el rechazo del tope de `--text`).
