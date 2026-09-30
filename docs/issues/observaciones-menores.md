# Observaciones menores: plan de instalación, tamaño de los modelos y WAV truncado

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja; media en la observación 3 (cada ficha indica la suya) |
| Tipo | diagnóstico (plan de instalación), documentación y estimación (tamaño de los modelos), funcional (carga de audio) |
| Componente | varios; se indica en cada observación |
| Versión detectada | 0.25.0 |
| Plataforma | observación 1: Windows y Unix; observación 2: Windows medido, Unix deducido del código de `hf-hub` y por confirmar con una medida; la observación 3 no depende de la plataforma |
| Reproducibilidad | siempre, salvo que se indique otra cosa |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 (1 y 2); revisión del código al diseñar la corrección de `--audio` inexistente, 2026-09-29 (3) |

## Resumen

Tres defectos sueltos, sin causa común con otros, agrupados aquí porque ninguno
justifica, por ahora, un documento propio. En los dos primeros el comando termina con el
resultado correcto, pero el mensaje engaña o la documentación no cuadra con la realidad.
El tercero salió de leer el código y puede producir un resultado incorrecto sin aviso
(severidad media). Las tres causas están confirmadas por lectura del código y, en la
observación 2, por medida en disco y por el código de `hf-hub`. Cada observación tiene
una ficha breve (síntoma, reproducción, causa, alternativas, recomendación y criterio).
Si alguna crece, se separa a su propio documento con la plantilla completa.

Las recomendaciones no añaden ni conservan mecanismos de retrocompatibilidad: cuando el
arreglo cambia un comportamiento o una razón de error, el comportamiento anterior se
retira en vez de mantenerse como reserva o excepción.

## Entorno

Observaciones 1 y 2: binario v0.25.0 instalado desde cero en Windows 11 con
PowerShell 5.1, con todos los modelos provisionados (incluido el de clonado). La causa
de la 1 y el desglose de la 2 salen además de la lectura del código de v0.25.0, de la
medida de la raíz de modelos de esa instalación y del código de `hf-hub` 1.0.0, la
versión fijada en `Cargo.lock`.

Observación 3: lectura del código fuente de v0.25.0 y de `hound` 3.5.1, la versión
fijada en `Cargo.lock`. No se ha reproducido todavía; la ficha indica cómo hacerlo.

## Precondiciones

Las propias de cada observación.

## Pasos para reproducir

Se detallan en cada observación, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada observación.

## Resultado esperado

Se detalla en cada observación.

## Impacto y workaround

La observación 1 no bloquea nada: solo confunde al leer el plan de instalación.

La observación 2 tampoco bloquea, pero las cifras publicadas no sirven para decidir si
hay disco suficiente: con el clonado, Windows necesita ~14 GB y los documentos anuncian
~11,5 GB, mientras la confirmación de la CLI anuncia 12 o 13 GB. Workaround: reservar
~14 GB en Windows.

La observación 3 sí puede afectar al resultado: un WAV truncado se transcribe o se
dobla incompleto sin ningún aviso. Workaround: comprobar la integridad del WAV antes de
pasarlo.

## Evidencia

Recogida en cada observación.

## Análisis de causa

### Instalación

#### 1. El plan de `self install` anuncia «se añadirá» un PATH que ya existe

- **Síntoma:** al reinstalar (`self install` sobre una instalación existente, estado
  `repaired`), el resumen previo dice `PATH: se añadirá <bin> en el PATH del usuario`,
  aunque la entrada ya está en `HKCU\Environment\Path`. En Unix pasa lo mismo con el
  enlace: el resumen dice `se creará el enlace <bin>/ai-voice-interconnector` aunque ya
  exista y apunte al mismo ejecutable.
- **Causa (confirmada por lectura):** `plan_path` (`crates/avi-lifecycle/src/install.rs`)
  no consulta el estado real. Fija `registry` a `true` siempre en Windows y `symlink` a
  `true` siempre en Unix, así que `PathPlan::is_noop()` solo es cierto con
  `--no-modify-path`, caso que `compose_summary` atiende antes. La rama «ya está en el
  PATH; no se modifica» es inalcanzable en las dos plataformas. La comparación de
  entradas no tiene la culpa: `path_windows::plan_integrate` reconoce la entrada ya
  registrada con una comparación canónica, y por eso la aplicación no la duplica
  (`changed: false`). El resumen y la aplicación calculan el plan por caminos distintos.
- **Alternativas:**
  - *A. Calcular el plan con la misma función que lo aplica.* En Windows,
    `registry = plan_integrate(read_path(subkey), bin_dir).changed`; en Unix,
    `symlink` es falso si `path_unix::classify_existing` devuelve `Ours`, la misma
    clasificación que usa `create_symlink`. A favor: el resumen no puede divergir de la
    aplicación y la rama no-op vuelve a ser alcanzable. En contra: una lectura del
    registro al planificar (barata y de solo lectura), y `plan_path` necesita la ruta del
    ejecutable del directorio de programa para clasificar el enlace, que hoy no recibe.
  - *B. Redactar el mensaje en neutro* («se asegurará <bin> en el PATH»). A favor:
    trivial. En contra: oculta la información en vez de corregirla y deja la rama no-op
    muerta.
  - *C. Usar el PATH de la sesión*, como hace Unix con el bloque del perfil. En contra:
    en Windows es la fuente equivocada; justo después de instalar, la sesión todavía no
    ve el cambio del registro y el mensaje mentiría en la otra dirección.
- **Recomendación:** A en las dos plataformas, porque el síntoma y el arreglo son
  simétricos. Si la lectura del registro falla al planificar, el plan falla con el mismo
  error que daría la aplicación (`path_conflict`, exit 14), en lugar de volver al
  anuncio incondicional actual: la aplicación leería el mismo valor y fallaría igual,
  así que no hay nada que salvar con ese anuncio. Tres detalles de la implementación:
  - El valor del registro se lee al construir `Env`, que ya lleva `path_env` y
    `registry_subkey`, y `plan_path` lo recibe ya leído. Así la función sigue siendo
    pura y la prueba del criterio no toca el registro real; el fallo de esa lectura es el
    que produce el `path_conflict` anterior.
  - En Unix, `apply_path` vuelve a calcular el plan después de crear el enlace, así que
    ahí `symlink` sale falso; no importa, porque en ese punto solo usa `block_file`, y
    debe seguir siendo así.
  - En Windows, el mensaje no-op dice «ya está en el PATH del usuario»: la entrada está
    en el registro, pero una terminal abierta antes de la primera instalación todavía no
    la ve.
- **Fuera de alcance:** en Unix, un enlace ajeno sin `--force` se sigue anunciando como
  «se creará» aunque la aplicación falle después con `path_conflict`. La clasificación
  de A permitiría anunciarlo, pero es una funcionalidad aparte.
- **Esperado:** «ya está en el PATH; no se modifica» (en Windows, «en el PATH del
  usuario») cuando no hay nada que cambiar.
- **Criterio:** una prueba del plan con la entrada ya registrada (Windows) y otra con el
  enlace ya correcto (Unix) afirman que `is_noop()` es cierto y que el resumen lo dice.

### Documentación

#### 2. Tamaño de los modelos subestimado

- **Síntoma:** README, `USAGE.md`, `docs/CLI/commands/SETUP.md`, `docs/GOAL.md`,
  `docs/MANUAL-VALIDATION.md`, `docs/CLI/commands/CLEANUP.md`, `docs/BUILD.md` y
  `docs/specs/sdlc-lifecycle.md` hablan de ~9 GB para la selección base y ~11,5 GB con
  `--with-voice-cloning`. Con el clonado provisionado, la raíz de modelos ocupaba 14 GB
  en disco. La confirmación de la CLI da una tercera cifra: 12 GB en `setup` y 13 GB en
  `self install` para la selección base.
- **Causa (confirmada por medida en Windows y por el código de `hf-hub` 1.0.0):** al
  crear el puntero de cada archivo en `snapshots/`, `hf-hub` copia el blob en Windows
  (los enlaces simbólicos exigen privilegios elevados) y crea un enlace simbólico
  relativo en Unix. En Windows, por tanto, todo el contenido ocupa el doble; en Unix, el
  disco ocupa lo mismo que la descarga. La duplicación viene de la dependencia, no del
  código del producto. Ni los derivados CT2 (158 MB) ni la caché `xet` (vacía) explican
  la diferencia. Medida de la instalación de la prueba:

  | Repo | Descarga | En disco (Windows) |
  |---|---|---|
  | Qwen3-TTS CustomVoice | 2,4 GB | 4,7 GB |
  | Qwen3-TTS Base (clonado) | 2,4 GB | 4,7 GB |
  | opus-mt en-es | 0,9 GB | 1,8 GB |
  | opus-mt es-en | 0,6 GB | 1,2 GB |
  | Parakeet TDT v3 | 0,64 GB | 1,3 GB |
  | Derivado CT2 | (se genera) | 0,16 GB |
  | **Base** | **≈ 4,5 GB** | **≈ 9,2 GB** |
  | **Con clonado** | **≈ 6,9 GB** | **≈ 13,9 GB** |

  Los totales suman las filas, que están redondeadas; las cifras que se publiquen deben
  salir del tamaño en bytes de cada repo.

  Así se explica cada cifra publicada:
  - «~9 GB base» coincide con Windows por casualidad. En Unix, el disco ronda la
    descarga (~4,5 GB).
  - «~11,5 GB con clonado» es incorrecta en las dos plataformas: el clonado suma
    ~4,7 GB en Windows y ~2,4 GB en Unix, sobre bases de ~9,2 GB y ~4,5 GB.
  - `USAGE.md` mezcla criterios en su desglose: Qwen3-TTS «~4,7 GB» y Marian «~3 GB»
    son cifras duplicadas, y Parakeet «~0,6 GB» no lo es.
  - La confirmación de la CLI multiplica una estimación fija de 3 GB por repo
    (`MODEL_DOWNLOAD_ESTIMATE`, `crates/avi-lifecycle/src/setup.rs`), lo que da 12 GB
    para los cuatro repos base; `self install` además trunca a GB y suma 1, y anuncia
    13 GB. El comentario de la constante dice que el total ronda los 9 GB, y no es así.
- **Alternativas:**
  - *A. Corregir solo los documentos*, con cifras por plataforma. A favor: barato. En
    contra: la confirmación de la CLI sigue contradiciendo a los documentos.
  - *B. A, más un tamaño aproximado por repo en lugar de la constante fija.* A favor: la
    CLI y los documentos dan la misma cifra, y el tamaño de un repo fijado a una
    revisión es estable. En contra: un dato más que mantener junto a cada revisión
    fijada.
  - *B′. Consultar al Hub el tamaño real antes de confirmar.* A favor: cifra exacta sin
    tabla. En contra: una llamada de red antes de la confirmación, que falla sin
    conexión; es desproporcionado para una estimación.
  - *C. Eliminar la duplicación en Windows* (enlaces duros entre snapshots y blobs, que
    en NTFS no exigen privilegios). A favor: ahorra ~4,6 GB con la selección base y
    ~7 GB con el clonado. En contra: modifica la disposición de la caché de `hf-hub`,
    arriesgado con una raíz compartida (`HF_HOME`) que usan otras herramientas, y
    `cleanup` y `doctor` contarían dos veces cada archivo enlazado. Es una
    funcionalidad nueva, no una corrección, y su sitio natural es `hf-hub`.
  - *D. Publicar una sola cifra de caso peor* («hasta ~14 GB»). A favor: nunca
    subestima. En contra: sobreestima en Unix y no explica de dónde sale.
- **Recomendación:** B. Los documentos distinguen la descarga (~4,5 GB base, ~6,9 GB con
  clonado) del espacio en disco (el doble en Windows, igual a la descarga en Unix), con
  un margen explícito del 10 %. El tamaño de cada repo es un campo más de
  `avi_store::MODEL_REVISIONS`, junto a su revisión: el compilador exige que cada repo
  fijado tenga el suyo, sin prueba aparte, y quien cambia una revisión ve el tamaño en la
  misma entrada. La confirmación anuncia las dos cifras, porque es el momento en que se
  decide si hay disco suficiente: la descarga, suma del tamaño de cada repo, y el
  espacio en disco de la plataforma en curso (el doble en Windows, igual en Unix). El
  factor refleja el comportamiento de `hf-hub`, fijado en `Cargo.lock`, y se revisa al
  actualizarlo. Las dos cifras se redondean en lugar de truncar y sumar 1.
  `MODEL_DOWNLOAD_ESTIMATE` se elimina: no queda como estimación genérica de reserva
  para un repo sin tamaño. Las cifras de Unix se publican ya, deducidas del
  código de `hf-hub`, y una medida en Linux las confirma. C se registra aparte como
  propuesta, preferiblemente para `hf-hub`: tiene más impacto que este defecto, pero es
  otro tipo de cambio.
- **Esperado:** cifras que coincidan con lo que se descarga y se ocupa en disco tras
  `setup`, con un margen explícito, y una confirmación de la CLI que anuncia la descarga
  y el espacio en disco, coherentes con ellas.
- **Criterio:** todos los documentos citados dan las mismas cifras, medidas sobre una
  instalación limpia en Windows y en Linux, y las dos estimaciones de la confirmación
  quedan dentro de su margen.

### Límites de entrada

#### 3. Un WAV truncado se carga incompleto sin error

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

Las tres causas están confirmadas. Queda una medida de confirmación, que no bloquea la
corrección: la 2 mide el espacio en disco sobre una instalación limpia en Linux para
contrastar las cifras deducidas del código de `hf-hub`.

## Criterio de aceptación

Cada observación tiene su criterio en su ficha. El documento se da por resuelto cuando
todas están cerradas o separadas a su propio documento.

## Relacionados

- Contrato de la CLI, reglas de `--audio` (observación 3: la clasificación de un audio
  inválido) y de `voice clone` (observación 3: la razón de una referencia inválida pasa
  de `voice_clone_failed` a `invalid_audio`).
- Propuesta pendiente de registrar: eliminar la duplicación de blobs y snapshots en la
  caché de modelos de Windows, preferiblemente en `hf-hub` (observación 2,
  alternativa C).
