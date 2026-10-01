# Diseño del flujo iterativo para resolver los defectos abiertos

| Campo | Valor |
|---|---|
| Estado | G0 aprobada, C1 cerrado, C2 siguiente |
| Alcance | Los cinco informes de defectos abiertos en `docs/issues/` |
| Fecha | 2026-09-30 |
| Ciclo de vida | Este documento y su registro de progreso se eliminan cuando se cierra el último ciclo |

## Índice

1. [Propósito](#1-propósito)
2. [Vocabulario mínimo](#2-vocabulario-mínimo)
3. [Principios](#3-principios)
4. [Roles](#4-roles)
5. [Compuertas de control humano](#5-compuertas-de-control-humano)
6. [Anatomía de un ciclo](#6-anatomía-de-un-ciclo)
7. [Mapa de síntomas](#7-mapa-de-síntomas)
8. [Orden de los ciclos y dependencias](#8-orden-de-los-ciclos-y-dependencias)
9. [Decisiones (compuerta G0)](#9-decisiones-compuerta-g0)
10. [Ciclos](#10-ciclos)
11. [Registro de progreso](#11-registro-de-progreso)
12. [Riesgos](#12-riesgos)

## 1. Propósito

Este documento organiza en ciclos iterativos la resolución de todos los defectos abiertos.
Cada ciclo ataca una causa raíz común, no síntomas sueltos, y solo avanza cuando una
persona lo aprueba en unas compuertas. El reparto de papeles es este:

- **El humano** decide: elige las alternativas, aprueba cada plan y acepta o rechaza cada
  resultado.
- **El agente** prepara, explica, ejecuta y verifica.

El documento es autocontenido. Para cada problema explica sus alternativas, con
argumentos a favor y en contra y una recomendación justificada. Así se puede decidir sin
leer el código ni haber estado en la sesión en que se diseñó.

## 2. Vocabulario mínimo

- **Vía directa.** La CLI ejecuta la operación en su propio proceso: carga los modelos y
  lanza el motor de síntesis.
- **Vía daemon.** La CLI delega la operación en el daemon local (`127.0.0.1:8765`), que
  mantiene los modelos cargados y un motor de síntesis residente (`qwen_tts`, en
  `127.0.0.1:8766`).
- **Modo automático.** La CLI decide la vía en cada invocación. Con `--daemon` se fuerza
  la vía daemon.
- **Contrato.** Es el comportamiento documentado de la CLI:
  - la salida JSON: un sobre con `schema_version`, un `status` y, en caso de error, un
    `reason`;
  - los códigos de salida numéricos (0 éxito, 1 error genérico, 2 entrada inválida, 4
    modelo ausente, 5 daemon inalcanzable, 7 no aplica, etc.);
  - el protocolo HTTP del daemon, con su propia versión de esquema.
- **`reason`.** El código de máquina, estable, que identifica la causa de un error, como
  `model_missing` o `audio_too_long`.
- **Job Object.** El mecanismo de Windows que agrupa procesos. Con la opción
  `KILL_ON_JOB_CLOSE`, todos mueren cuando se cierra el último handle del grupo.

## 3. Principios

1. **El humano decide antes de que el agente implemente.** Ninguna decisión de contrato
   o de diseño se toma de forma implícita dentro de un ciclo.
   - Las decisiones que se pueden tomar de antemano se resuelven en G0.
   - Las que dependen de un diagnóstico se resuelven en la compuerta de diagnóstico del
     ciclo.
2. **Un ciclo, una causa raíz.** Los síntomas se agrupan por la corrección que comparten.
   El orden de los ciclos combina la severidad con las dependencias técnicas.
3. **Ciclos pequeños y revisables.** Si un ciclo mezcla dos causas raíz, o su resultado no
   se puede revisar en una sesión, se divide.
4. **Primero la prueba.** Antes de corregir un síntoma se escribe una prueba que lo
   reproduce y falla. El humano aprueba esas pruebas antes de que se escriba la
   corrección, porque traducen la decisión tomada a un comportamiento verificable, y
   quedan después como pruebas de regresión. Si un síntoma no admite una prueba
   automática (por ejemplo, matar un proceso a la fuerza en Windows), se escribe una
   verificación manual en `docs/MANUAL-VALIDATION.md`.
5. **Sin retrocompatibilidad.** Una corrección que cambia un comportamiento o un
   contrato retira lo anterior en el mismo cambio, sin reservas ni excepciones. El
   cambio incompatible se documenta en el contrato y en el CHANGELOG.
6. **Cierre documental en el mismo cambio.** Al resolver un síntoma:
   - su contenido vigente pasa al contrato, a la documentación del comando y a la
     sección `## [No publicado]` del CHANGELOG;
   - su ficha se elimina del informe;
   - un informe que se queda sin fichas se elimina, junto con su fila de la tabla del
     `README.md` de `docs/issues`.
7. **Desviación = parada.** Si durante la ejecución aparece un hecho que invalida algo
   aprobado, el agente se detiene y abre una compuerta de desvío; no improvisa.
8. **El trabajo del humano queda intacto.** Un ciclo empieza con el árbol de trabajo
   limpio o con los cambios ajenos identificados y registrados. Rechazar un resultado
   revierte solo los archivos que tocó el ciclo.
9. **Nada entra en git sin aprobación.** Los commits de un ciclo se hacen al aprobar su
   compuerta de resultado, con el formato de la skill `conventional-commits`. Publicar
   una versión es una compuerta aparte que sigue la skill `release`.
10. **Una rama por ciclo, un solo release al final.** Cada ciclo trabaja en su propia
    rama transitoria, creada desde `main` al abrirlo: `docs/decisiones-g0` para C0 y
    `fix/<síntoma>` para C1 a C7. Al aprobarse su G-Resultado, la rama se integra en
    `main` con `git merge --no-ff` y se elimina, de modo que cada ciclo parte de lo ya
    aprobado y queda como un merge revertible por separado. Integrar no publica: las
    entradas de cada ciclo se acumulan en `## [No publicado]` y la versión se publica una
    sola vez, cuando todos los ciclos planificados están cerrados. Si hay que corregir un
    ciclo ya cerrado, se hace en una rama `fix/` nueva, sin reabrir la anterior.

## 4. Roles

| Rol | Responsabilidad |
|---|---|
| **Humano** | Resuelve las decisiones y emite un veredicto en cada compuerta. |
| **Agente orquestador** | Prepara los paquetes de las compuertas, reparte el trabajo, razona sobre los informes de los subagentes y es el único que escribe el registro de progreso. |
| **Subagente ejecutor** | Uno por tarea: escribe pruebas, código, comentarios y documentación dentro del alcance aprobado, y devuelve un informe con la evidencia. |
| **Subagente revisor** | Es independiente del ejecutor. Antes de la compuerta de resultado, contrasta el cambio con la lista de verificación de la sección 5.3 y con el plan aprobado. |

## 5. Compuertas de control humano

Una compuerta es un punto de parada: el agente presenta un paquete autocontenido y no
avanza hasta recibir un veredicto explícito. El veredicto, su fecha y las correcciones
pedidas se anotan en el registro de progreso.

### 5.1 Catálogo

| Compuerta | Cuándo | Qué presenta el agente | Qué decide el humano |
|---|---|---|---|
| **G0 · Decisiones** | Una vez, antes de cualquier ciclo | La sección 9 | La alternativa elegida en cada decisión |
| **G-Plan** | Al abrir cada ciclo | La ficha del ciclo: síntomas dentro y fuera del alcance, decisiones que aplica, tareas y su verificación, pruebas previstas, archivos y símbolos que cambian, contratos y documentos afectados, riesgos | Si el plan resuelve bien ese conjunto de problemas |
| **G-Diag** | Solo en los ciclos cuya causa hay que diagnosticar, entre el diagnóstico y la corrección | Los hallazgos con su evidencia, la causa confirmada o las hipótesis descartadas y, si la corrección abre decisiones nuevas, sus alternativas con argumentos y recomendación | La causa aceptada y la dirección de la corrección |
| **G-Pruebas** | Tras escribir las pruebas, antes de corregir | Las pruebas nuevas y las modificadas, la evidencia de que fallan por la razón esperada y, para cada una, qué síntoma y qué decisión fija | Si las pruebas expresan el comportamiento decidido |
| **G-Resultado** | Al terminar la implementación y la verificación del agente | El paquete de resultado de la sección 5.3 | Si el código, las pruebas, los comentarios y la documentación se aceptan, se confirman en la rama del ciclo y se integran en `main` |
| **G-Desvío** | En cualquier momento, si un hecho invalida algo aprobado | El hecho, su evidencia, qué decisión o plan afecta y las alternativas | Cómo seguir |
| **G-Release** | Una sola vez, cuando todos los ciclos planificados están cerrados e integrados en `main` | El contenido de `## [No publicado]`, los cambios incompatibles y la versión propuesta | Si se publica, y con qué número |

G-Pruebas existe porque un malentendido del contrato detectado en las pruebas cuesta
unas líneas, y detectado en el resultado cuesta rehacer la implementación.

### 5.2 Veredictos

| Veredicto | Efecto |
|---|---|
| **Aprobar** | El agente avanza al paso siguiente. En G-Resultado, además, hace los commits en la rama del ciclo y la integra en `main`. |
| **Corregir** | El humano indica qué cambiar. El agente lo incorpora y vuelve a presentar en la misma compuerta, señalando qué cambió. No avanza hasta obtener una aprobación. |
| **Rechazar** | El agente descarta el trabajo del paso: <br>• en G-Plan, el ciclo se replantea o se aplaza; <br>• en G-Diag, se diagnostica con otra hipótesis; <br>• en G-Pruebas, se reescriben las pruebas desde el plan; <br>• en G-Resultado, se revierten los archivos del ciclo y se vuelve a G-Plan. <br>Si el rechazo cuestiona una decisión de G0, esa decisión se reabre con sus alternativas. |

En G0, además, cada decisión admite tres respuestas: elegir una alternativa, pedir más
información (el agente investiga y vuelve a presentar esa decisión) o aplazarla. Un
ciclo no se abre mientras tenga una decisión aplazada.

### 5.3 Paquete y lista de verificación de G-Resultado

Antes de presentar G-Resultado, el agente ejecuta su propia verificación: la batería de
pruebas del workspace (desde C2, con las dos órdenes fijas y no con órdenes ad hoc:
`cargo test --all`, que replica CI sin features, y
`cargo test --workspace --features full -- --include-ignored`, la de la puerta de la
release), el lint, el formato solo sobre los archivos tocados y la revisión
del subagente revisor. Si esa verificación falla tres veces seguidas, abre G-Desvío en
lugar de seguir intentándolo.

El paquete se organiza en cuatro dimensiones, cada una con su evidencia:

| Dimensión | Qué debe cumplirse | Evidencia |
|---|---|---|
| **Código** | • Solo cambia lo que está en el plan aprobado. <br>• No quedan reservas de compatibilidad, símbolos huérfanos ni duplicados de la lógica corregida. <br>• Los identificadores están en inglés. <br>• El lint está limpio. | El resumen del cambio por archivo y símbolo, y la salida del lint |
| **Pruebas** | • Las pruebas aprobadas en G-Pruebas pasan de rojo a verde sin haberse modificado; cualquier cambio posterior se señala y se justifica. <br>• Toda la batería del workspace pasa. <br>• Los ficheros golden solo cambian donde el contrato cambió por una decisión. | La salida de las pruebas antes y después, y la lista de goldens modificados con su motivo |
| **Comentarios** | • Están en español y son autocontenidos: sin identificadores de hallazgos ni citas a líneas. <br>• Se corrigieron los comentarios que el cambio dejó obsoletos. | La lista de comentarios añadidos, modificados y retirados |
| **Documentación** | • Están al día el contrato y la documentación de los comandos afectados. <br>• El cambio incompatible figura en `## [No publicado]` del CHANGELOG. <br>• La verificación manual está en `docs/MANUAL-VALIDATION.md`, si aplica. <br>• Las fichas resueltas se eliminaron del informe, y el informe y su fila del `README.md` de `docs/issues`, si se quedó sin fichas. | La lista de documentos tocados, con un resumen de cada cambio |

El paquete cierra con tres elementos más:

- las desviaciones respecto al plan aprobado, o la constancia de que no hubo ninguna;
- los hallazgos del subagente revisor y qué se hizo con cada uno;
- los commits propuestos, uno por cambio lógico, con su mensaje.

El humano puede aprobar el paquete entero o pedir correcciones en una dimensión
concreta. Aprobarlo autoriza los commits propuestos.

## 6. Anatomía de un ciclo

```text
Preparación ──► G-Plan ──► [Diagnóstico ──► G-Diag] ──► Pruebas en rojo ──► G-Pruebas
                                                                              │
     ┌────────────────────────────────────────────────────────────────────────┘
     ▼
Implementación (código, comentarios, documentación, CHANGELOG, cierre del informe)
     │
     ▼
Verificación del agente (pruebas, lint, formato de lo tocado, subagente revisor)
     │
     ▼
G-Resultado ──► commits ──► merge --no-ff a main ──► registro de progreso actualizado ──► siguiente ciclo
```

1. **Preparación.** Se crea la rama del ciclo desde `main`, se contrasta el informe del
   defecto con el código actual y se corrige si se ha desfasado, se comprueba el estado
   del árbol de trabajo y se redacta la ficha del ciclo.
2. **G-Plan.**
3. **Diagnóstico y G-Diag**, solo si el ciclo lo requiere.
4. **Pruebas en rojo y G-Pruebas.**
5. **Implementación.** Código, comentarios, documentación canónica, CHANGELOG y cierre
   del informe.
6. **Verificación del agente.**
7. **G-Resultado**, seguida de los commits, la integración de la rama en `main` y la
   actualización del registro de progreso.

## 7. Mapa de síntomas

| Id | Síntoma | Informe | Severidad | Ciclo |
|---|---|---|---|---|
| S1 | La vía daemon rechaza con 413 los audios de transcripción de más de unos 49 s | `daemon-rechaza-o-corta-audios-largos.md` | Media; se propone alta | C3 |
| S2 | Un kill duro del daemon deja huérfano al motor residente | `motor-residente-huerfano-y-trazas-fuera-del-log.md` | Media | C5 |
| S3 | El daemon no escribe log y los logs del motor no rotan | `residuos-en-disco-tras-comandos-correctos.md` | Media | C4 |
| S4 | La síntesis por daemon sale con exit 1 donde el contrato asigna otro código (4 si falta el modelo) | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja; media sin modelo | C1 |
| S5 | `--daemon` en los cinco comandos solo locales (`list`, `remove` y `play`) responde «Daemon inalcanzable» con exit 5 | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja; se propone media | C1 |
| S6 | `self update` deja artefactos y `doctor` falla hasta la siguiente operación de ciclo de vida | `residuos-en-disco-tras-comandos-correctos.md` | Baja; se propone media | C6 |
| S7 | El `status` JSON afirma operaciones que no ocurrieron (`daemon stop` sin daemon y los simulacros de `self uninstall` y `cleanup`) | `status-json-afirma-operaciones-no-realizadas.md` | Baja | C7 |
| S8 | `speech say` y `speech dub` dejan WAV temporales o informan en `audio_path` de uno ya borrado | `residuos-en-disco-tras-comandos-correctos.md` | Baja | C7 |
| S9 | `daemon.ready` queda en disco tras `daemon stop` | `residuos-en-disco-tras-comandos-correctos.md` | Baja | C5 |
| S10 | Trazas internas de la descarga y del motor en la terminal | `motor-residente-huerfano-y-trazas-fuera-del-log.md` | Baja | C4 |
| S11 | Prefijo `Error:` duplicado | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja | C1 |
| S12 | El evento `start` mide `text_length` en bytes, no en caracteres | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja | C1 |
| S13 | El dub por composición (daemon sin `/dub`) corta la transcripción a los 1500 ms | `daemon-rechaza-o-corta-audios-largos.md` | Baja | C3 |
| S14 | Una referencia de clonado de más de unos 1,5 MB se rechaza con 413 por la vía daemon (por reproducir) | `daemon-rechaza-o-corta-audios-largos.md` | Media | C3 |
| S15 | Un 404 de `/dub` con `reason` propio (`voice_not_found`, `model_missing`) no se traduce con la tabla: el cliente lo toma por un daemon sin `/dub` | Revisión de C1 | Baja | C3 |
| S16 | El contrato promete exit 5 cuando el daemon no tiene `/transcribe`; el cliente sale con 1 y no indica reiniciar el daemon | Revisión de C1 | Baja | C3 |
| S17 | `sudo_not_supported` se emite, pero no está en el contrato ni en el oráculo de la tabla | Revisión de C1 | Baja | C1 |
| S18 | Las guías de `devices`, `translate`, `voice` y el `status` del daemon declaran el sobre de la CLI en `"3"` | Revisión de C1 | Baja | C7 |
| S19 | `/synthesize` responde `model_missing` a una temperatura fuera de rango si falta el modelo de síntesis | Revisión de C1 | Baja | C1 |
| S20 | 53 pruebas, en 66 puntos del código, se aprueban solas cuando falta un recurso externo (modelos, binario del motor, dispositivo de audio, `ModelStore` escribible, puerto 8765 libre): imprimen `skip: …` y hacen `return`; cada una decide con sus propios auxiliares, uno de ellos duplicado, y uno de ellos ejecuta el `doctor` real sobre la instalación del mantenedor; otras cuatro comprueban un feature en tiempo de ejecución en lugar de declararlo con `cfg` | Revisión de C1 | Alta; un verde no demuestra que la prueba se ejecutó | C2 |
| S21 | Nada ejecuta las pruebas con recursos locales antes de publicar: CircleCI corre solo en tags, sin features nativas ni modelos, y ni la skill `release` ni `cargo xtask release` piden más que `cargo test --all` (o nada); esas pruebas cuentan como verdes sin haberse ejecutado | Revisión de C1 | Alta; los falsos verdes llegan hasta la puerta de la release | C2 |
| S22 | La golden de `/health` se salta sin Parakeet y, sin `native-stt`, nunca corre en CI; `voices_clone_daemon_precomputed_true` se salta por el modelo de transcripción cuando necesita el de síntesis | Revisión de C1 | Media | C2 |
| S23 | La validación de entrada de `/transcribe` (`usage_error` sin audio, `invalid_audio` con base64 inválido) y la de `/translate` (`empty_text`, mismo idioma, `unsupported_language_pair`) viven dentro de funciones que exigen `native-stt` o `native-translation` aunque no usan el motor, y sus pruebas solo corren con el feature | Revisión de C1 | Baja | C2 |
| S24 | El estado del daemon en las pruebas se construye de dos formas: a mano en las goldens, con un comentario de cabecera desfasado, y con `DaemonState::new()` en las pruebas de la biblioteca, que con `native-stt` carga Parakeet y escribe en el directorio de datos real | Revisión de C1 | Media | C2 |
| S25 | `docs/BRANCHING.md` afirma que los jobs de test corren en `main` y en ramas, y solo corren en tags | Revisión de C1 | Baja | C2 |
| S26 | La verificación de los ciclos usa órdenes ad hoc (como `cargo test -p avi-daemon --features native-stt` en C1) que dependen de la instalación real del mantenedor | Revisión de C1 | Baja | C2 |
| S27 | `self install`, `self uninstall` y `self update` detienen el daemon siempre en `127.0.0.1:8765` y `cleanup` solo respeta `AVI_DAEMON_PORT=0`; sin pidfile, `daemon stop`, `daemon status` y el resto de clientes del daemon lo buscan siempre en 8765; diez pruebas de contrato llamaban así a `daemon_stop::stop` contra el daemon real del mantenedor | Verificación de C2 | Media | C2 |

S11 no abre ninguna decisión: el mensaje del error se escribe sin prefijo y el prefijo
lo pone quien lo imprime.

S15 a S19 los detectó el revisor de C1 y no tienen informe propio: se asignan al ciclo
que ya trata su causa raíz. S17 y S19 se corrigieron dentro de C1; S18 espera a C7
porque la subida del sobre a `"5"` edita esas mismas líneas.

S20 a S26 también salieron de la revisión de C1 y no tienen informe propio: forman un ciclo
nuevo, C2, porque comparten una causa raíz que no pertenece a ningún informe: las pruebas no
declaran qué recursos necesitan y nada comprueba que se ejecuten con ellos. C2 se intercala
entre C1 y C3 para que las pruebas de C3 nazcan ya en su esquema.

S27 salió de la verificación de C2 y se corrige en C2.

## 8. Orden de los ciclos y dependencias

```text
C0 Preparación y decisiones ──G0──►
  C1 Contrato de errores (S4 S5 S11 S12 S17 S19)
    │  C3 usa la tabla única de reason→exit y la lectura del reason en los errores del daemon
    ▼
  C2 Clases de pruebas (S20 S21 S22 S23 S24 S25 S26 S27)
    │  C3 escribe sus pruebas en el esquema de clases y pone su tope de audio sobre la validación de /transcribe que C2 extrae
    ▼
  C3 Límites de la vía daemon (S1 S13 S14 S15 S16)
    │
    ▼
  C4 Observabilidad (S3 S10)
    │  C5 toca el mismo lanzamiento del clonado y se verifica con el log del daemon
    ▼
  C5 Vida de los procesos (S2 S9)
    │
    ▼
  C6 Artefactos de self update (S6)
    │
    ▼
  C7 JSON veraz y temporales (S7 S8 S18)

G-Release: una sola vez, tras cerrar e integrar C7
```

Justificación del orden:

- **C1 va antes que C3, aunque S1 es el síntoma más grave.** La corrección de S1 exige
  que la transcripción lea el `reason` de una respuesta de error y lo traduzca al código
  del contrato, y eso es justo lo que construye C1. Si C3 fuera primero, crearía otra
  tabla local que C1 tendría que deshacer. C1 es pequeño, así que el retraso es mínimo.
- **C2 va entre C1 y C3.** Las pruebas nuevas de C3 sobre `/transcribe` y sobre los topes
  deben nacer ya en el esquema de dos clases; si C3 fuera primero, habría que reclasificarlas
  después. Además, la validación de `/transcribe` que C2 extrae a una función pura es la base
  sobre la que C3 añade su tope de audio. C2 parte de lo que C1 ya dejó en `main` en las
  mismas pruebas, y desde su cierre todos los ciclos verifican con una sola orden.
- **C4 va antes que C5 por una dependencia dura.** Los dos cambian cómo se lanza el
  motor de clonado: C4 lleva su salida a un log y C5 le conecta la tubería de D11.1. Además,
  sin el log del daemon, un fallo al matarlo no deja rastro y la verificación de S2 sería
  a ciegas.
- **C6 y C7 no dependen entre sí.** C6 va antes por severidad. El humano puede
  invertirlos en el G-Plan de C6.
- **Los ciclos no se ejecutan en paralelo.** Casi todos tocan el binario principal
  (`src/main.rs`), y en paralelo la revisión humana tendría que separar cambios
  entrelazados.

## 9. Decisiones (compuerta G0)

Todas las decisiones siguen el mismo esquema: el problema, las alternativas con sus
argumentos a favor y en contra, la recomendación y el ciclo que la aplica. El campo
**Decisión** lo rellena el humano en G0. Ninguna depende de un diagnóstico, así que
ningún ciclo tiene G-Diag.

### D1 · Tope del audio de transcripción en las dos vías (S1) — C3

**Problema.**

- La transcripción por daemon viaja como PCM de 16 kHz mono en base64: unos 42 700 bytes
  por segundo de audio.
- El daemon acepta cuerpos de hasta 2 MB, unos 49 s de audio.
- El push-to-talk permite grabar hasta 300 s (`AVI_PUSH_TO_TALK_MAX_SECS`), unos 12,8 MB.
- La vía directa transcribe ficheros de cualquier duración.
- Por encima de 49 s, la vía daemon falla con un 413 sin `reason` y, en modo
  automático, no se reintenta por la vía directa.
- El dub ya tiene un tope de producto de 40 s, que se rechaza con `audio_too_long` y
  exit 2.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Límite de cuerpo explícito al nivel del techo del push-to-talk**, solo en el daemon | Cambio mínimo; cubre las grabaciones del producto | Un fichero de más de 300 s sigue fallando por daemon, pero no por la vía directa: las dos vías se comportan distinto; el límite queda desligado de las constantes del producto |
| **B. Tope de duración de producto en las dos vías.** El cliente mide la duración antes de enviar y rechaza con `audio_too_long` (exit 2); el daemon aplica la misma regla; su límite de cuerpo se calcula a partir del tope | Una sola regla y un error identificable en las dos vías; reutiliza un `reason` existente; el mismo patrón que el dub; el error llega antes de transferir nada; acota además la memoria de la vía directa | Cambio incompatible para quien transcribe ficheros más largos por la vía directa; hay que fijar el valor del tope |
| **C. Quitar el límite del cuerpo** | Las dos vías se comportan igual | La memoria de un proceso residente que ya tiene los modelos cargados queda sin cota: una hora de audio son unos 150 MB de cuerpo, más la decodificación |
| **D. Transporte binario o por trozos** en lugar de JSON con base64 | Elimina el sobrecoste de base64; escala mejor | Cambio grande de protocolo que sube su versión, y sigue haciendo falta un tope |
| **E. En modo automático, enviar a la vía directa los audios que superan el límite** | El usuario no ve el error en modo automático | Es una reserva silenciosa; no arregla `--daemon`; la vía se elige antes de leer el audio |

**Recomendación: B, con un tope de 300 s.**

- Da una sola regla en las dos vías, con un error diagnosticable y el mismo patrón que el
  dub.
- 300 s es el techo del push-to-talk, que es la fuente habitual de audios largos.
- El cambio incompatible para los ficheros más largos se documenta.

Si no se quiere acotar la vía directa, la segunda opción es A: el límite se calcula a
partir del techo del push-to-talk, el rechazo con `audio_too_long` se aplica solo por la
vía daemon y la diferencia entre vías queda documentada.

**Decisión:** B, con un tope de 300 s (2026-09-30).

### D2 · Tamaño de la referencia del clonado (S14) — C3

**Problema.**

- El clonado envía el fichero de referencia completo, tal como está en disco, en base64,
  junto con la referencia de timbre si se indica.
- No hay ningún tope de duración ni de tamaño, así que unas referencias que sumen más de
  unos 1,5 MB (una sola de unos 9 s en WAV de 44,1 kHz estéreo) chocan con el mismo
  límite de 2 MB.
- La vía directa no tiene ese límite.
- El síntoma está deducido del código. C3 lo reproduce con una prueba antes de
  corregirlo; si no se reproduce, esta decisión se descarta en su G-Plan.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Tope de duración de la referencia en las dos vías**, rechazado con `audio_too_long`, y un límite de cuerpo calculado para el peor formato que se admita | La misma regla y el mismo patrón que D1; el error llega antes de transferir nada | Cambio incompatible para las referencias más largas que el tope; hay que conocer cuánta referencia usa de verdad el motor |
| **B. Límite de cuerpo fijo y generoso** (por ejemplo, 32 MB) solo en el daemon | Simple | La regla depende del formato del fichero; las dos vías siguen comportándose distinto |
| **C. El cliente convierte la referencia a 16 kHz mono antes de enviarla** | Reduce el tamaño en cualquier formato | Cambia la entrada del motor y puede degradar el clonado; es trabajo de audio adicional |

**Recomendación: A.** El valor del tope se fija en el G-Plan de C3, a partir de la
duración de referencia que el motor aprovecha realmente. Si el motor recorta la
referencia a N segundos, el tope es N.

**Decisión:** A; el valor del tope, en el G-Plan de C3 (2026-09-30).

### D3 · Dub por composición para daemons sin `/dub` (S13) — C3

**Problema.**

- Si el daemon responde 404 a `POST /dub`, lo que ocurre con un daemon de una versión
  anterior a esa ruta, el cliente compone el dub con llamadas sueltas.
- Esa composición envuelve una transcripción completa en un plazo de 1500 ms pensado
  para conectar, así que con audios de más de unos 14 s falla como `daemon_unreachable`.
- La composición solo existe por compatibilidad con daemons antiguos.
- Eliminarla no deja ningún otro símbolo huérfano.
- El cliente no comprueba la versión del daemon.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Eliminar la composición.** Un 404 en `/dub` produce un error que pide reiniciar el daemon | Cumple la regla de no retrocompatibilidad; menos código; un daemon lanzado con el binario actual siempre tiene `/dub` | Quien mantenga vivo un daemon antiguo recibe un error en lugar de un dub (se resuelve reiniciándolo) |
| **B. Corregir el plazo**: un plazo corto solo para conectar y sin corte para la inferencia, como en la ruta principal | El dub funciona también con daemons antiguos | Conserva un camino de compatibilidad que la regla prohíbe; hay que probar y mantener dos vías |
| **C. Dejarlo como está** | Ningún trabajo | El defecto sigue ahí y el error es engañoso |

Subdecisión de A, el `reason` del error:

- **A1.** `daemon_error` (exit 1) con un mensaje que indica reiniciar el daemon. No hace
  falta ningún `reason` nuevo.
- **A2.** Un `reason` nuevo, como `daemon_outdated`, que un script podría distinguir.

**Recomendación: A con A1.** Nadie ha pedido distinguir ese caso desde un script, y
reiniciar el daemon lo resuelve siempre. Si en el futuro se añade una comprobación de
versión al conectar, será el momento de un `reason` propio.

**Decisión:** A con A1 (2026-09-30).

**Ampliación (2026-09-30, G-Resultado de C1; S15 y S16).** La regla vale para cualquier
ruta, no solo para `/dub`, y distingue dos clases de 404:

- Un 404 **sin** `reason` en el cuerpo significa que la ruta no existe, es decir, un
  daemon de una versión anterior: sale `daemon_error` (exit 1) con un mensaje que indica
  reiniciar el daemon.
- Un 404 **con** `reason` (`voice_not_found`, `model_missing`) es un error legítimo de
  la ruta y se traduce con la tabla única (3 y 4).

La regla vive en un solo sitio, la lectura común de las respuestas de error del daemon,
y el contrato deja de prometer exit 5 para `transcribe` y `dub` ante un daemon sin la
ruta.

### D4 · Traducción única de `reason` a código de salida (S4, y base de C3) — C1

**Problema.**

- No hay un único sitio que traduzca un `reason` a su código de salida:
  - la función central `exit_code_for` solo cubre los `reason` del ciclo de vida;
  - la traducción, el clonado y el dub por daemon llevan cada uno su propia tabla (el
    clonado y el dub, duplicada en dos puntos);
  - la transcripción no lee el `reason` de una respuesta de error;
  - la síntesis solo lo conserva en el error de validación.
- El resultado es que la misma causa sale con códigos distintos según la vía. Por
  ejemplo, la síntesis por daemon sin modelo sale con 1, cuando el contrato asigna 4.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Ampliar `exit_code_for` en el binario principal** y usarla en todos los clientes de la vía daemon | Cambio acotado a un archivo | La tabla queda lejos del enum de códigos, en el crate compartido del contrato |
| **B. Una tabla única junto al enum `ExitCode`, en el crate compartido del contrato.** Todos los clientes de la vía daemon leen el `reason` del cuerpo de cualquier respuesta de error; una prueba recorre todos los `reason` del contrato | El vocabulario de máquina en un solo sitio; se puede probar de forma exhaustiva; desaparecen las copias | Toca más crates que A |
| **C. El daemon envía el código de salida en el cuerpo** | El cliente solo lo copia | Acopla el protocolo del daemon a los códigos de la CLI, sube la versión del protocolo y no cubre la vía directa |

**Recomendación: B.**

- Un `reason` desconocido sale con 1, que es lo que establece el contrato para un
  `reason` ausente.
- La vía directa sigue construyendo sus errores con un código explícito. Una prueba
  comprueba que, para cada `reason` que comparten las dos vías, ese código coincide con
  el de la tabla.
- Migrar la vía directa a la tabla queda fuera de alcance, salvo que el humano lo pida.

**Decisión:** B (2026-09-30).

### D5 · `--daemon` en comandos que siempre se ejecutan en local (S5) — C1

**Problema.**

- `voice list`, `voice remove`, `speech list`, `speech play` y `speech remove` no se
  delegan al daemon.
- Con `--daemon` salen con exit 5, `daemon_unreachable` y el texto «Daemon
  inalcanzable», aunque el daemon esté activo.
- La documentación de `speech` y `voice` recoge ese exit 5.
- Un script que reintenta ante un exit 5 reintentará en vano.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Mantener exit 5 y `daemon_unreachable`, y cambiar solo el texto** | No cambia el contrato | El código y el `reason` siguen afirmando algo falso |
| **B. Exit 2 (entrada inválida) con un `reason` nuevo, como `daemon_not_supported`** | Describe el error real, una combinación de flags inválida, y sigue el patrón de las demás validaciones de flags | Cambio incompatible, que hay que documentar en `speech` y `voice`; un `reason` nuevo |
| **C. Exit 7 (no aplica: «la operación no aplica a este objetivo o entorno, y no aplicará reintentando»)** | Semántica cercana y ya existente | Hoy el 7 se usa para objetivos y entornos del ciclo de vida; mezclarlo con errores de flags diluye su significado |
| **D. Ignorar `--daemon` y ejecutar en local con un aviso** | El comando nunca falla | Oculta un error de uso; el usuario pidió explícitamente la vía daemon |

**Recomendación: B.** Es un error de la invocación que el usuario corrige quitando el
flag, que es exactamente lo que significa el código 2. El nombre del `reason` lo decide
el humano.

**Decisión:** B, con el `reason` `daemon_not_supported` (2026-09-30).

### D6 · `status` de `daemon stop` cuando no había daemon (S7) — C7

**Problema.**

- Con el daemon detenido, `daemon stop` borra el pidfile y responde con
  `status: "shutdown_sent"` y el texto «Señal de apagado enviada», aunque no envió nada.
- El exit 0 es correcto, porque la operación es idempotente.
- La función de parada ya calcula si el daemon estaba en ejecución antes de tocar nada,
  pero la CLI descarta ese dato y vuelve a sondear.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. `status: "not_running"`**, conservando `daemon: "stopped"` y exit 0, con el texto «El daemon no estaba en ejecución» | Veraz; el mismo patrón que `not_installed` en `self uninstall` | Cambio incompatible en los valores de `status` |
| **B. Mantener `shutdown_sent` y añadir la clave `was_running: false`** | Cambio aditivo, que no sube el esquema | El `status` sigue afirmando algo falso; es una reserva de compatibilidad |
| **C. Salir con otro código, por ejemplo 7** | Señal inequívoca | Rompe la idempotencia de la que dependen los scripts de parada |

**Recomendación: A.** Usa el dato que la parada ya calcula; el texto humano sigue al
`status`.

**Decisión:** A (2026-09-30).

### D7 · Los simulacros de `self uninstall` y `cleanup` (S7) — C7

**Problema.**

- `self uninstall --dry-run` responde `status: "uninstalled"` y
  `cleanup --dry-run` responde `status: "cleanup_complete"`, los dos con
  `dry_run: true`. Un consumidor que lea solo el `status` concluye que la operación
  ocurrió.
- En modo humano, `self uninstall --dry-run` imprime «Desinstalación completada».
- El simulacro de `cleanup` reutiliza a propósito el resultado de la limpieza real para
  que los dos se puedan comparar: su `removed` contiene las rutas que se borrarían.
- El de `self uninstall` no cumple esa propiedad:
  - su `removed` incluye las rutas del barrido de restos, que en la ejecución real no
    van en `removed`;
  - su `path_reverted` vale siempre `false`, aunque la ejecución real revertiría el
    PATH.

**D7.1 · `status` del simulacro.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Un único `status: "planned"` para todo simulacro** (`self uninstall` y `cleanup`) | Veraz y uniforme: un solo valor que aprender | Cambio incompatible en dos comandos |
| **B. Mantener los valores y documentar la convención** «con `dry_run: true`, el `status` describe el resultado previsto» | Ningún cambio de código ni de contrato | El `status` sigue afirmando un hecho que no ocurrió |
| **C. Corregir solo `self uninstall`** | Cambio mínimo | Dos convenciones distintas para lo mismo |
| **D. Un valor por comando** (`would_uninstall`, `would_clean`) | Explícito | Multiplica los valores sin aportar información que `planned` no dé |

**Recomendación: A, conservando las claves del resultado.** Con `status: "planned"`, el
sobre entero se lee como un plan, incluida la lista `removed`. Renombrar las claves en
el simulacro rompería la comparación entre simulacro y ejecución real, que es la razón
por la que comparten resultado. El texto humano de `self uninstall --dry-run` se
corrige en la misma tarea.

**Decisión:** A (2026-09-30).

**D7.2 · Claves del simulacro de `self uninstall` que difieren de la ejecución real.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Alinear el simulacro con la ejecución**: `removed` solo con las entradas del plan, como en `cleanup`, y `path_reverted` con el valor previsto según el recibo | El sobre entero es un plan veraz y comparable; los dos comandos siguen la misma regla | Algo más de código en el simulacro |
| **B. Documentar la diferencia** | Ningún cambio de código | El sobre `planned` contiene un dato falso y una lista que no se puede comparar |
| **C. Quitar esas claves del simulacro** | No afirma nada falso | Rompe la comparación entre simulacro y ejecución real |

**Recomendación: A.** Sin ella, D7.1 corrige el `status` y deja mentir a las claves que
lo acompañan.

**Decisión:** A (2026-09-30).

**D7.3 · La clave `dry_run`.** Con `status: "planned"` es redundante: solo un simulacro
da `planned`, y un simulacro nunca da otro valor.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Retirarla de los sobres de `self uninstall` y `cleanup`** | Cada hecho se dice en un solo sitio; sin dos campos que mantener coherentes | Otra clave retirada, dentro de la misma subida del esquema |
| **B. Conservarla como eco de la entrada** | El sobre conserva su forma | Redundante; en la ejecución real es ruido; solo se justifica por compatibilidad |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

### D8 · WAV temporales de `speech say` y `speech dub` (S8) — C7

**Problema.** `say` y `dub` sintetizan a un WAV temporal para reproducirlo y devuelven
su ruta en `audio_path` y en el texto humano («Reproduciendo: …», «Doblaje
reproducido: …»).

- `say` por la vía directa conserva el fichero.
- `say` por la vía daemon lo borra, pero después de emitir su ruta; si la reproducción
  falla, sale antes de borrarlo.
- `dub` lo conserva siempre, en sus dos vías, la directa y la daemon; C3 elimina la
  composición (D3).
- Cada vía gestiona el fichero a mano, y cualquier error intermedio lo deja atrás.

Para guardar un audio ya existe `speech synthesize`, que devuelve la ruta de un WAV
persistente.

**D8.1 · El temporal y `audio_path`.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Borrar el temporal al terminar la reproducción, también si falla, y retirar `audio_path` de `say` y `dub`**; el texto humano deja de mostrar la ruta | Sin residuos; el JSON solo afirma lo que existe; `synthesize` sigue cubriendo el caso de querer el fichero | Cambio incompatible: se retira una clave |
| **B. Conservar el fichero y la clave en todas las vías** y dejar la limpieza a `cleanup` | Ningún cambio de contrato | Los residuos se acumulan; contradice la regla de que quien crea un fichero temporal lo borra |
| **C. Borrar el fichero y conservar la clave** | Sin residuos | El JSON apunta a algo que no existe, que es justo el defecto actual de la vía daemon |
| **D. Borrar, salvo que un flag nuevo pida conservarlo** | Flexible | Funcionalidad que nadie ha pedido, y duplica `synthesize` |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

**D8.2 · Cómo se garantiza el borrado en cualquier salida.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Un guardián único del temporal**: un tipo que borra el fichero al salir de ámbito, en todas las vías | Un solo mecanismo que cubre el éxito, el fallo de la síntesis y el de la reproducción; se prueba una vez | Tras un kill duro el fichero queda, y lo recoge `cleanup` |
| **B. La vía daemon reproduce desde memoria y la directa usa el guardián** | La vía daemon no escribe en disco | Dos mecanismos; la vía directa necesita el fichero igualmente, porque el motor escribe su salida en una ruta; hay que añadir una reproducción desde bytes |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

### D9 · Log del daemon y retención de los logs (S3) — C4

**Problema.**

- El daemon en segundo plano se lanza con sus salidas descartadas, así que se pierden
  sus errores.
- El motor crea un log nuevo en cada arranque (`qwen3-tts_<pid>_<ms>.log`) y nada los
  borra.

Destino del log del daemon:

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Redirigir el stderr del daemon a un fichero** `data/logs/daemon_<pid>_<ms>.log`, uno por arranque, con el mismo patrón que el motor | Cambio mínimo; captura las trazas, los pánicos y la salida de las bibliotecas nativas | Un daemon que vive mucho tiempo escribe un solo fichero sin rotación interna; el volumen lo acota el nivel de trazas |
| **B. Un escritor de ficheros para `tracing` con rotación por tiempo** | Rota dentro de la misma ejecución | Dependencia nueva; no captura los pánicos ni la salida nativa |

Retención de las dos familias de logs, la del motor y la del daemon:

| Alternativa | A favor | En contra |
|---|---|---|
| **R1. Conservar los K más recientes de cada familia**, podando al crear uno nuevo | Determinista y fácil de probar: tras N arranques quedan K | El espacio en disco depende del tamaño de cada fichero |
| **R2. Por antigüedad** (N días) | Intuitivo | Depende del reloj; con muchos arranques en un día no acota nada |
| **R3. Por tamaño total** | Acota el disco | Más lógica; la poda depende del tamaño, no del número de ficheros |

**Recomendación: A con R1 y K = 10 por familia.** La poda la hace el código que crea el
log. Es lo más simple que cumple el criterio del informe y da el log que necesita la
verificación de C5.

**Decisión:** A con R1 y K = 10 por familia (2026-09-30).

### D10 · Trazas internas en la terminal (S10) — C4

**Problema.**

- En modo humano, `voice clone` por la vía directa deja que el motor escriba en la
  terminal.
- `setup` imprime por stderr las trazas de nivel `info` de la descarga.
- El subscriber de `tracing` no tiene ningún filtro configurable.

Salida del motor durante el clonado:

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Redirigirla a un log de la familia del motor**, con el mismo patrón y la misma retención que D9 | La terminal queda limpia y el diagnóstico se conserva | Nada relevante |
| **B. Descartarla** | Trivial | Se pierde la información para diagnosticar un clonado fallido |

Nivel de las trazas:

| Alternativa | A favor | En contra |
|---|---|---|
| **N1. Nivel `warn` fijo para los comandos de la CLI y nivel `info` para el daemon**, cuyo stderr va a su log | Sin dependencias ni variables nuevas; la terminal solo muestra avisos y errores | Para ver trazas de `info` en un comando de la CLI hay que cambiar el código |
| **N2. Filtro configurable con la variable estándar `RUST_LOG`**, `warn` por defecto | Diagnóstico a demanda | Activa una característica nueva de la dependencia y añade una variable de entorno que nadie ha pedido |

**Recomendación: A con N1.** Las trazas útiles para diagnosticar quedan en los logs del
daemon y del motor. De paso, se corrige el comentario obsoleto de la inicialización.

**Decisión:** A con N2 (2026-09-30). Sin `RUST_LOG`, los comandos de la CLI usan `warn` y
el daemon `info`, que va a su log; si la variable está definida, manda en los dos.

### D11 · Que el motor muera con el daemon (S2) — C5

**Problema.** El daemon lanza el motor en dos modos: el residente, que vive
indefinidamente, y el de clonado, que dura decenas de segundos y también se lanza desde
la CLI. Si el daemon muere de forma abrupta, los dos siguen vivos; el residente, con el
modelo en memoria y el puerto 8766 ocupado.

- **Windows.** Un Job Object debería impedirlo, pero una prueba de extremo a extremo vio
  sobrevivir al residente, y un fallo de ese mecanismo no deja rastro.
- **Linux y macOS.** No existe ningún mecanismo.

El motor es código C del repositorio y se puede modificar, registrando la divergencia.

**D11.1 · Mecanismo y alcance.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. `PR_SET_PDEATHSIG` al lanzar el motor** | No toca el motor; lo garantiza el kernel | Solo existe en Linux, no en macOS; la señal se dispara al morir el *hilo* que lanzó el proceso, no el proceso |
| **B. El motor vigila a su padre, en sus dos modos, y se retira el Job Object**: quien lanza el motor mantiene abierto el extremo de escritura de una tubería conectada a su entrada estándar, y el motor termina al leer fin de fichero | Un solo mecanismo en las tres plataformas, que cubre a los dos hijos de larga vida del daemon y también al clonado lanzado por la CLI; lo garantiza el sistema operativo al cerrar los handles del proceso muerto | Modifica el motor vendorizado; si el vigía falla no hay respaldo; un auxiliar de larga vida futuro necesitaría su propia tubería |
| **C. B, conservando el Job Object como segunda defensa** | Doble protección en Windows | Dos mecanismos para lo mismo, y el Job ya falló sin dejar rastro |
| **D. B solo para el residente, conservando el Job Object** | No toca el clonado | Dos mecanismos en Windows y el clonado huérfano en Linux y macOS |
| **E. Grupo de procesos y kill al grupo desde el supervisor** | Sencillo | No sirve si el daemon muere por `SIGKILL`, que es justo el caso |

Lo que respalda a B, comprobado en el código y con un experimento en Windows:

- Los únicos hijos de larga vida del daemon son el residente y el clonado; `taskkill`,
  `tasklist` y `kill` son instantáneos. El audio, la transcripción y la traducción
  corren dentro del proceso. Así, el Job no cubre nada que B no cubra.
- Ninguna prueba ejerce el Job; solo lo citan comentarios y documentación.
- Ningún modo del motor lee su entrada estándar. El vigía es un hilo que lee hasta fin
  de fichero y termina el proceso: unas 20-30 líneas en `main.c`, con el `pthread` que
  el motor ya enlaza en las tres plataformas.
- Tras un `TerminateProcess` del padre, sus hijos con la entrada en tubería vieron fin de
  fichero en 2-3 s en 9 ejecuciones de 9, con otros lanzamientos concurrentes: el
  extremo de escritura de una tubería de Rust no se hereda.
- Terminar de golpe no deja un `.qvoice` roto en el almacén: el clonado escribe en un
  temporal, que solo se entrega si terminó bien.

Dos condiciones de diseño:

- **El vigía se activa con un flag.** Hoy la entrada estándar del motor es nula y da
  fin de fichero inmediato; sin el flag, el vigía mataría al motor al arrancar.
- **El clonado deja de usar `Command::status()`.** Esa llamada cierra la tubería antes
  de esperar al hijo; hay que lanzarlo con `spawn()`, conservar la tubería y esperar.

**Recomendación: B.** Es el único mecanismo portable, cubre el `SIGKILL` y deja uno solo
donde hoy hay uno que falla en Windows y ninguno en Unix. Con él, averiguar por qué falló
el Job deja de ser necesario.

**Decisión:** B (2026-09-30).

**D11.2 · Herencia de la tubería en macOS.** En macOS, si el daemon lanza dos procesos a
la vez, uno podría heredar el extremo de escritura de la tubería del otro y retrasar su
cierre como mucho lo que dura un clonado. Es un riesgo teórico, sin verificar.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Aceptarlo y documentarlo** | Ningún código; el retraso está acotado y se resuelve solo | En ese caso, el motor tarda más en morir |
| **B. Lanzar los procesos del daemon de uno en uno** | Elimina la ventana | Un cerrojo para un riesgo que nadie ha observado y que no se puede verificar sin un Mac |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

### D12 · Artefactos de `self update` y el veredicto de `doctor` (S6) — C6

**Problema.** Tras `self update` en Windows quedan dos restos, y `doctor` los cuenta como
fallo (exit 1) hasta que otra operación de ciclo de vida los barre.

- **El aparcado `.old-*` queda siempre.** Contiene el exe de la CLI que ejecuta la
  actualización, y un binario en ejecución no se puede borrar. La transacción calcula la
  lista de lo que no pudo borrar, pero la instalación la descarta y nadie programa su
  borrado.
- **El staging del `.zip` quedó en la prueba de extremo a extremo** aunque su limpieza
  programa un auxiliar PowerShell que espera a que termine la CLI y lo borra. Ese
  auxiliar falla sin dejar rastro, da por arrancado un script que no llegó a ejecutarse
  si vence el plazo de espera, y no se separa del Job Object de la terminal, que lo mata
  junto con la CLI. La causa concreta no está confirmada: un antivirus que retiene el
  archivo, un Job externo o un script que no se ejecutó.
- **Hoy no hay forma barata de pedir la limpieza.** Solo barren `self update`,
  `self uninstall` y `cleanup`, y este último solo con una categoría de datos que borrar.
- En Linux y macOS no hay problema: un binario en ejecución se puede borrar.

**D12.1 · Borrado inmediato en Windows.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Corregir el auxiliar PowerShell**: programar con él el borrado del aparcado a partir de la lista de restos; que deje rastro de su resultado y no dé por bueno un arranque sin confirmar; lanzarlo separado del Job Object de la terminal, y sin separarlo si el Job no lo permite | Un único mecanismo para el staging, el aparcado y `self uninstall`; ataca las tres causas candidatas del staging residual; cambio acotado | Sigue dependiendo de PowerShell, que una directiva de grupo puede bloquear; ese caso queda para la red de D12.2 |
| **B. Un auxiliar en Rust: el binario nuevo se relanza en un modo oculto que borra** | Sin PowerShell; se prueba en Rust y escribe en el log del producto | No sirve para `self uninstall`, porque el binario está dentro del directorio que hay que borrar, así que quedarían dos mecanismos; añade un subcomando oculto al contrato |
| **C. Invertir el traspaso: la CLI vieja lanza la nueva y termina, y la nueva reemplaza** | Nadie retiene el aparcado | Reescribe la transacción, con su diario, su rollback y su recuperación; desproporcionado para un síntoma de severidad baja o media |
| **D. `MoveFileEx` con borrado al reiniciar** | Lo hace el sistema operativo | Exige privilegios de administrador, y ninguna operación del producto los pide; el resto dura hasta el siguiente reinicio |

**Recomendación: A.** Es la única que cubre las tres rutas con un solo mecanismo y sin
pedir elevación.

**Decisión:** A (2026-09-30).

**D12.2 · Red para lo que el auxiliar no borre.** Hace falta aunque el auxiliar funcione:
una directiva de grupo que bloquee PowerShell, un antivirus que retenga el archivo más
que los reintentos, o un `doctor` ejecutado en el segundo que el auxiliar tarda en borrar.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. `doctor --repair`**, que toma el bloqueo y ejecuta el barrido existente | El arreglo está donde el usuario ve el fallo; `doctor` sin el flag sigue sin modificar nada; el barrido ya distingue lo que está en uso; no cuesta nada a los demás comandos | Un flag más en el contrato; la limpieza solo ocurre si el usuario la pide |
| **B. `cleanup` sin categoría ejecuta solo el barrido** | Reutiliza un comando que ya barre | Cambia su contrato (sin categoría es `usage_error`) y mezcla la limpieza de restos con el borrado de datos del usuario, que es destructivo y pide confirmación |
| **C. Barrido automático, en cada invocación o dentro de `doctor`** | No hay que pedirlo | En cada invocación, un listado de directorio y escrituras en todos los comandos, incluidos los de solo lectura; dentro de `doctor`, un diagnóstico que modifica el sistema |
| **D. Solo el barrido actual** | Ningún código | La única pista posible es `self update`, que sale a la red, o `cleanup` con una categoría, que borra datos |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

**D12.3 · Veredicto de `doctor` sin `--repair` ante restos.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Fallo (exit 1) con la pista `doctor --repair`** | Con el auxiliar corregido, un resto que perdura es una anomalía real; el contrato de `checks` no cambia | Justo después de `self update`, mientras el auxiliar borra, falla por una carrera que la pista resuelve en un paso |
| **B. Aviso con exit 0** | Sin falsos fallos en esa carrera | Requiere un nivel de severidad nuevo en el contrato de `checks` y oculta los restos que el auxiliar no pudo borrar |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

**D12.4 · Alcance de `--repair`.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Solo lo que recoge el barrido** (diario, aparcados, stagings huérfanos y temporales); después, `doctor` vuelve a evaluar y sale según el resultado | Acotado; reutiliza la lógica que ya decide qué es seguro borrar | No arregla otras filas, como las entradas duplicadas del PATH o los modelos que faltan |
| **B. Todo lo que `doctor` sepa arreglar** | Un solo comando lo arregla todo | Descargas y cambios en el PATH desde un diagnóstico; alcance que ningún síntoma pide |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

**D12.5 · Diagnóstico previo.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Sin diagnóstico previo**: la verificación del ciclo reproduce `self update --force` y lee el rastro del auxiliar; si el staging sigue quedando, se abre un G-Desvío | La causa exacta no cambia el plan: D12.1 cubre las tres candidatas y D12.2 recoge el resto; una fase y una compuerta menos | La causa se confirma después de corregir, no antes |
| **B. Reproducción instrumentada y G-Diag antes de corregir** | La causa se confirma antes de tocar el código | Instrumenta a mano un auxiliar que se va a reescribir, sin que el resultado cambie lo que se implementa |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

### D13 · Versión de los esquemas (transversal)

**Problema.** El sobre JSON de la CLI está en el esquema 4 y el protocolo del daemon, en
el 3. Según el contrato:

- añadir una clave o un `reason` no sube la versión;
- retirar o cambiar el significado de una clave, o cambiar los valores de un `status`,
  sí la sube.

Los cambios de esta iteración que suben un esquema son estos:

- **Protocolo del daemon:** la unidad de `text_length` pasa a caracteres (C1).
- **Sobre de la CLI:** los nuevos valores de `status` (D6 y D7) y la retirada de
  `dry_run` (D7.3) y de `audio_path` (D8), todos en C7.
- **Ninguno de los dos:** el cambio de código de salida de D5 no toca ninguna clave.
  Cambia el contrato, pero no el esquema, y se documenta en el CHANGELOG.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Una subida por versión publicada**: la hace el primer ciclo que rompe un esquema, y los siguientes no la repiten hasta la publicación | El consumidor ve un solo salto por versión; sigue siendo correcta si un ciclo posterior rompe otra vez el mismo esquema o si un hotfix obliga a publicar a mitad de la iteración | Si se publica entre dos ciclos que rompen el mismo esquema, habrá dos saltos, que es lo correcto |
| **B. Una subida por ciclo** | Mecánico | Saltos de versión que ningún consumidor llega a ver |
| **C. No subir** | Ningún trabajo | Incumple la política del propio contrato |

**Recomendación: A.**

- C1 sube el protocolo del daemon de 3 a 4 y documenta `text_length` en caracteres, la
  misma unidad que el tope de 500 caracteres de `--text`.
- C7 sube el sobre de la CLI de 4 a 5.

Con un solo release al final de la iteración y cada esquema roto en un único ciclo, A y
B producen hoy el mismo resultado; A se prefiere porque sigue siendo correcta si eso
cambia.

**Decisión:** A (2026-09-30).

### D14 · Pruebas que dependen del dispositivo de audio (S20) — C2

**Problema.**

- Varias pruebas necesitan un dispositivo de salida de audio real y hoy se saltan en
  silencio si no lo hay.
- El esquema de C2 tiene dos clases: contrato, que no usa nada externo y corre en todas
  partes, y con recursos locales, marcada con `#[ignore = "requiere …"]`, que `cargo test
  --all` solo muestra como *ignored* y se ejecuta con `--include-ignored`.
- Un servidor de integración de CircleCI no tiene dispositivo de audio.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Clase con recursos locales** (`#[ignore]`, con fallo explícito si falta el dispositivo) | Una sola regla para todo recurso externo; prueba el camino real de reproducción; no cambia el código de producción | Solo se ejecutan donde hay dispositivo, es decir, en la puerta local de la release |
| **B. Simular la salida de audio con un doble** | Corren en todas partes, también en CI | Exige abstraer la salida de audio en producción solo para las pruebas, y el doble no prueba el dispositivo real |
| **C. Fuera de este ciclo** | Ningún trabajo ahora | Dejaría en el ciclo pruebas con `skip` en silencio, y el criterio de que ninguna prueba se salte no se cumpliría |

**Recomendación: A.** Es la misma regla que se aplica a los modelos y al binario del
motor, sin tocar la producción.

**Decisión:** A (2026-09-30).

### D15 · Pruebas que se saltan por `ModelStore` no escribible o por un daemon vivo en 8765 (S20) — C2

**Problema.**

- Unas pruebas se saltan si ya hay un daemon en `127.0.0.1:8765`, y otras con el mensaje
  «sin ModelStore escribible», que en realidad ejecuta el `doctor` real sobre la
  instalación del mantenedor y se salta si no sale con 0. Dependen del entorno del que las
  ejecuta, no de un recurso que haya que proveer, y la segunda cambiaría de resultado
  cuando C6 haga que `doctor` falle ante restos (D12.3).
- `cli_golden.rs` ya tiene el patrón `IsolatedInstance`, que levanta una instancia con
  directorios temporales y puerto efímero.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Aislarlas con directorios temporales y puerto efímero (patrón `IsolatedInstance`)** | Dejan de depender del entorno; no tocan datos reales ni chocan con un daemon vivo; después quedan en la clase que corresponda por lo que de verdad necesiten | Hay que adaptar cada prueba al patrón |
| **B. Clase con recursos locales (`#[ignore]`)** | Trabajo mínimo | Marca como recurso lo que es un defecto de aislamiento, y la prueba seguiría fallando si el mantenedor tiene un daemon vivo |
| **C. Fuera de este ciclo** | Ningún trabajo ahora | Quedarían pruebas con `skip` en silencio |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

### D16 · Dónde vive la puerta de release de las pruebas con recursos locales (S21) — C2

**Problema.**

- CircleCI no puede ejecutar la clase con recursos locales, porque los modelos pesan varios
  GB y habría que descargarlos en cada tag.
- Hoy la release pide `cargo test --all`, que deja esas pruebas como *ignored*; la skill
  `release` y `docs/RELEASING.md` lo describen así, y `cargo xtask release` no ejecuta
  ninguna prueba.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Dentro de `cargo xtask release`**: ejecuta `cargo test --workspace --features full -- --include-ignored` como condición previa y aborta si algo falla o falta un recurso; sin modo para saltarla | Mecánica: no se puede publicar sin haber ejecutado la suite completa; una sola orden, la misma que usa cada ciclo | La release tarda lo que dure la suite con modelos; solo puede cortarla quien tenga los recursos instalados |
| **B. Solo un paso documentado en la skill `release`** | Sin código | Depende de que el agente o el humano lo recuerden; es el origen de S21 |

**Recomendación: A.** Un paso solo documentado es lo que ya falló: lo que no se comprueba
mecánicamente se omite. La skill y `docs/RELEASING.md` se actualizan para describir la
puerta, no para sustituirla.

**Decisión:** A (2026-09-30).

### D17 · Estado del daemon en las pruebas con `native-stt` (S23, S24) — C2

**Problema.**

- Con `native-stt`, el campo `stt_engine` de `DaemonState` no es opcional: no existe un
  estado del daemon sin cargar Parakeet. La puerta de D16 compila justo así.
- Parakeet está en la selección por defecto de `setup`, y el daemon no arranca sin él: un
  daemon sin STT no es un estado del producto.
- Casi todas las pruebas del daemon atraviesan el router con ese estado.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. El contrato en funciones puras y la clase según el feature**: las validaciones de entrada se extraen a funciones puras que se prueban sin estado y son contrato con cualquier feature; las pruebas que atraviesan el router son contrato sin `native-stt` y llevan `#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]` con él | La producción no cambia; cada prueba corre en algún sitio (sin features en CI, con `full` en la puerta); la lógica de contrato queda separada del motor | La clase de las pruebas del router depende del feature |
| **B. `stt_engine` opcional, como `ct2_engine`** | El estado de prueba nunca carga modelos | Añade a la producción una rama `None` que ninguna instalación alcanza, solo para las pruebas |
| **C. El motor STT tras un trait, con un doble en las pruebas** | Prueba la ruta completa sin modelos | Abstracción en producción solo para las pruebas; el doble no prueba el motor real |

**Recomendación: A.** El defecto estructural es que la lógica de contrato vive dentro del
handler que necesita el motor; separarla resuelve la clase de esas pruebas sin inventar
estados que el producto no tiene.

**Decisión:** A (2026-09-30), resuelta por el agente a petición del humano.

### D18 · Cómo apuntan los almacenes del daemon a un temporal en las pruebas (S24) — C2

**Problema.**

- `VoiceStore` y `SpeechStore` solo leen el directorio de datos del proceso
  (`AVI_DATA_DIR` o la ruta real); solo `ModelStore` tiene un constructor con raíz propia
  (`ModelStore::at`).
- Las pruebas de la biblioteca corren en paralelo dentro del mismo proceso.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Constructores `VoiceStore::at` y `SpeechStore::at`**, que el estado de prueba recibe con un temporal | No muta el entorno del proceso; seguro en paralelo; mismo patrón que `ModelStore::at` | Dos constructores públicos más |
| **B. Fijar `AVI_DATA_DIR` antes de construir el estado** | No toca `avi-store` | La variable es global al proceso y compite entre pruebas en paralelo |

**Recomendación: A.**

**Decisión:** A (2026-09-30).

## 10. Ciclos

Cada ficha resume lo que el G-Plan del ciclo concretará y lo que el humano deberá
aprobar. Las pruebas que se enumeran son las mínimas.

### C0 · Preparación y decisiones

- **Objetivo:** dejar resueltas todas las decisiones y los informes, al día.
- **Tarea del agente antes de G0:** presentar la sección 9.
- **Compuerta:** G0. Si el humano elige una alternativa distinta de la recomendada, el
  agente ajusta las fichas de los ciclos afectados y las vuelve a presentar antes de
  abrir C1.
- **Salida:**
  - las decisiones D1 a D13, resueltas;
  - el registro de progreso, creado;
  - los informes actualizados, incluidos en el primer commit tras aprobar G0;
  - la rama `docs/decisiones-g0` integrada en `main`.

### C1 · Contrato de errores

- **Causa raíz:** no hay un único sitio que traduzca las causas de error a `reason` y a
  código de salida.
- **Síntomas:** S4, S5, S11 y S12; S17 y S19, añadidos en G-Resultado.
- **Decisiones:** D4, D5 y D13 (la subida del protocolo del daemon), resueltas en G0.
  En el G-Plan (2026-09-30), todas A:
  - P1: `/transcribe` señala sus errores con un estado HTTP de error, no con 200 y
    `status: "error"`.
  - P2: el daemon renombra `audio_decode_error` a `invalid_audio` y `audio_missing` a
    `usage_error`.
  - P3: `avi-lifecycle` depende de `avi-core` y usa la tabla única; se retira el código
    propio de `LifecycleError`.
  - P4: `DAEMON-MODE.md` deja de describir un handshake de `schema_version` que el
    cliente no hace.
- **Tareas:**
  1. Tabla única de `reason` a código de salida en `avi-core`, que sustituye a
     `exit_code_for`, a las copias de los clientes y al código de `LifecycleError`; todos
     los clientes de la vía daemon, incluidas la síntesis y la transcripción, leen el
     `reason` de cualquier respuesta de error; un `reason` desconocido sale con 1 (D4,
     P3).
  2. La síntesis por daemon sale con el código del contrato (S4).
  3. `--daemon` en los comandos solo locales (D5).
  4. Un solo prefijo `Error:` (S11).
  5. `text_length` en caracteres y protocolo del daemon en la versión 4, con los errores
     de `/transcribe` por estado HTTP y sus `reason` del contrato (S12, D13, P1, P2).
  6. Ajustes de G-Resultado (2026-09-30, veredicto «ajustar»):
     `sudo_not_supported` se declara con exit 1 en el oráculo, el contrato y las guías
     de `self` y `cleanup` (S17), y `/synthesize` valida la temperatura antes de
     comprobar el modelo (S19).
- **Pruebas en rojo:**
  - la tabla cubre todos los `reason` del contrato, y el código explícito de la vía
    directa coincide con ella en los compartidos;
  - la síntesis por daemon sin modelo sale con 4 y `model_missing`;
  - una respuesta 500 de `/transcribe` con `transcription_failed` sale con 10;
  - `/transcribe` responde 400 con `usage_error` si falta el audio y 400 con
    `invalid_audio` si el base64 no es válido;
  - `--daemon` en `voice list`, `voice remove`, `speech list`, `speech play` y
    `speech remove` sale con exit 2 y `daemon_not_supported`;
  - golden de `--temperature` fuera de rango, con un solo prefijo;
  - el `text_length` de «canción» es 7;
  - `/synthesize` con una temperatura fuera de rango responde `usage_error` aunque
    falte el modelo de síntesis.
- **Documentación:** el contrato, `speech`, `voice` y la descripción del protocolo del
  daemon (`DAEMON-MODE.md`, incluido el handshake de P4).
- **Cierra:** `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md`.

### C2 · Clases de pruebas

- **Causa raíz:** las pruebas no declaran en su definición qué recursos externos
  necesitan: lo deciden en tiempo de ejecución y, si falta algo, se aprueban solas. Nada
  ejecuta las que necesitan recursos antes de publicar.
- **Principio:** cada prueba declara su clase en su definición y ninguna se salta en
  silencio. Ninguna prueba que pueda prescindir de un modelo lo carga, y ninguna que lo
  necesite corre donde haya que descargarlo (los modelos pesan varios GB).
  - **Clase contrato:** no usa nada externo (ni modelos, ni hardware, ni la instalación
    real). Corre en todas partes: `cargo test --all` en local y en la puerta del tag.
  - **Clase con recursos locales** (modelos, binario del motor, dispositivo de audio):
    `#[ignore = "requiere …"]`. `cargo test --all` la muestra como *ignored* sin descargar
    nada, así que CircleCI no cambia. Se ejecuta con
    `cargo test --workspace --features full -- --include-ignored`; dentro, la ausencia
    del recurso es un fallo explícito (`expect` con la orden que lo provisiona, como
    `setup`), no un `return`.
  - **Clase según el feature** (D17): las pruebas que atraviesan el router del daemon son
    contrato sin `native-stt` y llevan
    `#[cfg_attr(feature = "native-stt", ignore = "requiere Parakeet")]` con él, porque
    con ese feature el estado del daemon carga el motor. Así corren sin features en CI y
    con `full` en la puerta. La lógica de contrato que no necesita el motor no depende de
    esto: vive en funciones puras que se prueban sin estado.
  - Ninguna prueba escribe en la instalación real: los recursos de la instalación se
    leen sin modificarlos y todo lo que la prueba escribe va a directorios temporales.
- **Depende de:** C1.
- **Síntomas:** S20, S21, S22, S23, S24, S25, S26 y S27.
- **Decisiones:** D14 a D18, resueltas el 2026-09-30. El hook `post-merge` que poda
  `target/` no se toca: no bloquea, es opcional y mezclaría responsabilidades.
- **Tareas:**
  1. Extraer la validación de entrada de `/transcribe` (devuelve el PCM o la respuesta de
     error) y la de `/translate` (texto vacío, mismo idioma y par no soportado) a funciones
     puras sin `cfg`; los handlers con feature solo llaman al motor. Sus pruebas pasan a la
     clase contrato con cualquier feature (S23, D17). C3 añade su tope de audio sobre la
     función de `/transcribe`.
  2. Un solo constructor `DaemonState::with_stores(voice_store, speech_store)`, que
     también usa `new()`, y `VoiceStore::at` y `SpeechStore::at`, como `ModelStore::at`
     (D18). Las goldens y las pruebas de la biblioteca construyen su estado con él y
     almacenes en un temporal; se retiran el montaje manual de las goldens, su comentario
     desfasado y el uso de `DaemonState::new()` en las pruebas. Sin `native-stt` el
     estado no carga ningún modelo, y con él carga solo Parakeet (S24, D17).
  3. Reclasificar cada prueba que hoy se omite: contrato sin compuerta (como la golden de
     `/health`), `#[ignore]` con fallo explícito, `#[cfg_attr(…, ignore)]` según D17, o
     `#[cfg(feature = …)]` sobre la prueba cuando el código que prueba solo existe con un
     feature. Las pruebas con recursos comprueban el archivo concreto que necesitan, no
     ejecutan `doctor`. Eliminar todos los `skip: … return`, los auxiliares de omisión y
     los `#[allow(unreachable_code)]`; la cabecera de `tests/cli_golden.rs` pasa a
     describir las clases (S20, S22).
  4. Aislar las pruebas que se saltan por un daemon vivo en 8765 o por el resultado del
     `doctor` real, con directorios temporales y puerto efímero (patrón
     `IsolatedInstance`); luego quedan en la clase que corresponda por lo que necesiten
     (D15, S20).
  5. Puerta mecánica en `cargo xtask release`: ejecuta
     `cargo test --workspace --features full -- --include-ignored` tras las
     validaciones de solo lectura y antes de cualquier escritura (el bump incluido), y aborta si algo falla o falta un
     recurso, sin modo para saltarla. El paso 4 de la skill `release` conserva
     `cargo test --all` como réplica local de CI sin features, y la skill y
     `docs/RELEASING.md` describen la puerta (D16, S21).
  6. Corregir `docs/BRANCHING.md`: los jobs de test corren solo en tags (S25).
  7. Que `self install`, `self uninstall`, `self update` y `cleanup` detengan el daemon en
     el puerto de `AVI_DAEMON_PORT`, como lo arranca el daemon: `daemon_stop::default_addr`
     (sobre la función pura `addr_for_port_env`) sustituye a `DEFAULT_ADDR` en los cuatro
     comandos, y las pruebas que ejecutan `cleanup::run` en su proceso fijan
     `AVI_DAEMON_PORT=0` (S27). Por la misma causa, el cliente sin pidfile
     (`daemon_stop::resolve_client_addr`, que usan `daemon stop`, `daemon status` y los
     demás comandos que contactan con el daemon) toma como respaldo `default_addr` en
     lugar de `DEFAULT_ADDR` (S27).
- **Pruebas en rojo:**
  - la función pura de dirección (`addr_for_port_env`) da `127.0.0.1:<puerto>` para un
    puerto válido, `0` incluido y con espacios recortados, y la dirección por defecto si
    falta o no es un puerto;
  - la dirección del cliente sin pidfile (`resolve_client_addr_with`) sigue el puerto de
    `AVI_DAEMON_PORT`, y con pidfile manda la dirección publicada en él (S27);
  - una prueba de contrato recorre los `.rs` de `src/`, `tests/` y `crates/*/{src,tests}`
    y falla si alguno contiene un `eprintln!` con `skip:`;
  - la golden de `/health` corre y pasa sin features nativas y sin modelos;
  - las funciones de validación de `/transcribe` (`usage_error` sin audio,
    `invalid_audio` con base64 inválido) y de `/translate` (`empty_text`, mismo idioma,
    `unsupported_language_pair`) se prueban sin ningún feature;
  - el estado de prueba del daemon tiene la raíz de sus almacenes bajo un temporal y, sin
    `native-stt`, se construye sin ningún modelo provisionado;
  - las pruebas que se saltaban por el puerto 8765 o por `doctor` pasan con un daemon
    vivo en 8765.
- **Verificación:**
  - `cargo test --all` muestra las pruebas con recursos como *ignored* y pasa sin modelos.
  - `cargo test --workspace --features full -- --include-ignored` pasa en la instalación
    del mantenedor.
  - `cargo xtask release` con `AVI_CACHE_DIR` apuntando a un temporal vacío aborta en la
    puerta y deja el árbol sin cambios (`git status` limpio). La puerta no lleva prueba
    automática porque tendría que lanzar la suite dentro de la suite.
- **Documentación:** `docs/BRANCHING.md`, `docs/RELEASING.md`, el paso 4 de la skill
  `release`, la cabecera de `tests/cli_golden.rs`, la sección «Interno» de
  `## [No publicado]` del CHANGELOG (la release exige la suite con recursos locales) y
  su sección «Corregido» (S27).
  S26 ya queda resuelto en la sección 5.3 de este documento.
- **Cierra:** ningún informe: los síntomas no tienen informe propio.

### C3 · Límites de la vía daemon

- **Causa raíz:** la vía daemon tiene límites de transporte ajenos a los del producto y
  un camino de compatibilidad con su propio límite de tiempo.
- **Depende de:** C1 y C2 (las pruebas nuevas nacen en el esquema de clases y el tope de
  audio se añade sobre la validación de `/transcribe` extraída).
- **Síntomas:** S1, S13, S14, S15 y S16.
- **Decisiones:** D1, D2 y D3, con su ampliación a cualquier ruta.
- **Tareas:**
  1. Tope de transcripción y límite de cuerpo derivado de él (D1).
  2. Reproducción de S14 y tope de la referencia del clonado (D2).
  3. Eliminar el dub por composición y aplicar la regla del 404 a todas las rutas en
     la lectura común de los errores del daemon: sin `reason`, daemon desfasado; con
     `reason`, la tabla (D3, S15, S16).
- **Pruebas en rojo:**
  - la ruta de transcripción acepta un cuerpo de más de 2 MB y por debajo del tope;
  - un audio por encima del tope se rechaza con `audio_too_long` y exit 2 en las dos
    vías;
  - una referencia de clonado mayor que 1,5 MB y dentro del tope se acepta por daemon;
  - un 404 sin `reason` en `/dub` o en `/transcribe` sale con `daemon_error`, exit 1 y
    un mensaje que indica reiniciar el daemon;
  - un 404 de `/dub` con `voice_not_found` sale con 3, y con `model_missing`, con 4.
- **Documentación:** el contrato (incluidas las secciones de `transcribe` y `dub`, que
  dejan de prometer exit 5 ante un daemon sin la ruta), `speech`, `voice` y la
  descripción del protocolo del daemon.
- **Cierra:** `daemon-rechaza-o-corta-audios-largos.md`.

### C4 · Observabilidad

- **Causa raíz:** las trazas no llegan a su destino: las del daemon se pierden y las
  internas aparecen en la terminal.
- **Síntomas:** S3 y S10.
- **Decisiones:** D9 y D10.
- **Tareas:**
  1. Log del daemon.
  2. Poda al crear un log, en las dos familias.
  3. Salida del motor durante el clonado hacia su log.
  4. Filtro de trazas configurable con `RUST_LOG`: sin la variable, `warn` en los
     comandos de la CLI e `info` en el daemon. Corrige de paso el comentario de la
     inicialización de las trazas, que cita un esquema del sobre JSON ya superado.
- **Pruebas en rojo:**
  - tras N creaciones de log quedan K por familia;
  - un error del daemon en segundo plano aparece en su log;
  - la salida humana de `voice clone` y de `setup` no contiene trazas internas;
  - con `RUST_LOG=info`, `setup` vuelve a mostrar las trazas de la descarga.
- **Documentación:** la política de logs en la documentación del daemon y de
  `cleanup`, y la variable `RUST_LOG` allí donde se documentan las variables de
  entorno.
- **Cierra:** la ficha de logs de `residuos-en-disco-tras-comandos-correctos.md` y la
  ficha de trazas de `motor-residente-huerfano-y-trazas-fuera-del-log.md`.

### C5 · Vida de los procesos

- **Causa raíz:** los procesos del daemon no dejan limpio su estado al terminar, ni de
  forma ordenada ni abrupta.
- **Depende de:** C4.
- **Síntomas:** S2 y S9.
- **Decisiones:** D11.1 (vigía por tubería en la entrada estándar y retirada del Job
  Object) y D11.2 (retraso acotado en macOS, aceptado), resueltas en G0.
- **Tareas:**
  1. Vigía en el motor: un flag que arranca un hilo que lee la entrada estándar hasta
     fin de fichero y termina el proceso, en los dos modos.
  2. El residente y el clonado se lanzan con el flag y la entrada en tubería; el
     clonado, con `spawn()` en lugar de `Command::status()`, conservando la tubería
     hasta que termina.
  3. Retirar el Job Object, su llamada y la feature de `windows-sys` si nada más la usa.
  4. `daemon stop` borra `daemon.ready` además de `daemon.pid`.
- **Pruebas en rojo:**
  - tras matar el daemon a la fuerza, con y sin `--auto-restart`, no queda ningún
    proceso del motor ni nadie escuchando en el puerto 8766; si no se puede
    automatizar, se documenta como verificación manual;
  - lo mismo con un clonado en curso, sin que quede su `.qvoice` en el almacén;
  - sin el flag, un motor lanzado con la entrada nula no termina al arrancar;
  - tras `daemon stop` no quedan ni `daemon.ready` ni `daemon.pid`;
  - la recuperación de un daemon caído a partir de `daemon.ready` sigue funcionando.
- **Documentación:** la descripción del daemon sin el Job Object, `MANUAL-VALIDATION.md`,
  las divergencias del motor, el retraso acotado de D11.2 en macOS y la entrada del
  CHANGELOG.
- **Cierra:** `motor-residente-huerfano-y-trazas-fuera-del-log.md` y la ficha de
  `daemon.ready` de `residuos-en-disco-tras-comandos-correctos.md`.

### C6 · Artefactos de `self update`

- **Causa raíz:** nadie programa el borrado del aparcado, el auxiliar de borrado
  diferido falla sin dejar rastro, y no hay forma de pedir la limpieza sin una operación
  de ciclo de vida.
- **Síntomas:** S6.
- **Decisiones:** D12.1 a D12.5, resueltas en G0. Sin diagnóstico previo (D12.5).
- **Tareas:**
  1. Programar el borrado del aparcado con el auxiliar a partir de la lista de restos de
     la transacción (D12.1).
  2. Corregir el auxiliar: deja rastro de su resultado, no da por bueno un arranque sin
     confirmar y se lanza separado del Job Object de la terminal, o sin separarlo si el
     Job no lo permite (D12.1).
  3. `doctor --repair`: toma el bloqueo, ejecuta el barrido y vuelve a evaluar (D12.2,
     D12.4).
  4. La fila de restos de `doctor` falla con la pista `doctor --repair` (D12.3).
- **Pruebas en rojo:**
  - el auxiliar borra una ruta cuando el proceso que la bloquea termina, también
    lanzado desde un proceso dentro de un Job Object con cierre por muerte;
  - el auxiliar informa del fallo si la ruta sigue bloqueada al agotar los reintentos;
  - tras `self update`, el aparcado y el staging desaparecen al terminar la CLI;
  - `doctor` con restos sale con exit 1 y la pista; `doctor --repair` los recoge, no
    toca lo que está en uso y sale con exit 0 si no queda ninguno;
  - `doctor` sin el flag no modifica nada.
- **Verificación:** reproducir `self update --force` en Windows y leer el rastro del
  auxiliar. Si el staging sigue quedando, G-Desvío.
- **Documentación:** `self`, `doctor` (el flag nuevo y su alcance) y la entrada del
  CHANGELOG.
- **Cierra:** la ficha de `self update` de `residuos-en-disco-tras-comandos-correctos.md`.

### C7 · JSON veraz y temporales

- **Causa raíz:** algunos sobres JSON describen lo que se pidió o lo que habría pasado,
  no lo que pasó, y quien crea un fichero temporal no siempre lo borra.
- **Síntomas:** S7, S8 y S18.
- **Decisiones:** D6, D7, D8 y D13 (la subida del sobre de la CLI), resueltas en G0.
- **Tareas:**
  1. `daemon stop` sin daemon responde `status: "not_running"` y «El daemon no estaba
     en ejecución», a partir del dato de la parada (D6).
  2. Los simulacros de `self uninstall` y `cleanup` responden `status: "planned"`, y el
     texto humano de `self uninstall --dry-run` deja de anunciar la desinstalación
     (D7.1).
  3. El simulacro de `self uninstall` alinea `removed` y `path_reverted` con la
     ejecución real (D7.2).
  4. Retirar `dry_run` de los sobres de `self uninstall` y `cleanup` (D7.3).
  5. Un guardián único del WAV temporal en `say` y `dub`, por todas sus vías; se retira
     `audio_path` del JSON y la ruta del texto humano (D8.1, D8.2).
  6. Sobre de la CLI en la versión 5 (D13), también en las guías que aún lo declaran en
     `"3"`: `DEVICES.md`, `TRANSLATE.md`, `VOICE.md` y el `status` de `DAEMON.md`
     (S18).
- **Pruebas en rojo:**
  - golden de `daemon stop` sin daemon, con `not_running` y exit 0;
  - golden de `self uninstall --dry-run` y de `cleanup --dry-run`, con `planned`, sin
    `dry_run` y con las mismas claves que la ejecución real;
  - el `removed` del simulacro de `self uninstall` no incluye las rutas del barrido, y
    su `path_reverted` coincide con el de la ejecución real sobre el mismo recibo;
  - tras `speech say` y `speech dub`, por la vía directa y la daemon, también cuando
    fallan la síntesis o la reproducción, no queda ningún WAV temporal y el JSON no
    contiene `audio_path`;
  - el sobre de la CLI declara `schema_version` 5.
- **Documentación:** el contrato (valores de `status`, claves retiradas y versión 5 del
  sobre), `speech`, `daemon`, `self`, `cleanup`, `devices`, `translate`, `voice` y la
  entrada del CHANGELOG.
- **Cierra:** `status-json-afirma-operaciones-no-realizadas.md` y
  `residuos-en-disco-tras-comandos-correctos.md`, que para entonces se queda sin fichas.
- **Cierre de la iteración:** el agente propone eliminar este documento y su registro
  de progreso, y lo presenta en la misma G-Resultado. Con C7 integrado en `main`, se
  abre G-Release.

## 11. Registro de progreso

El avance se registra en `docs/issues/iterative-workflow-design.progreso.json`, que se
crea al aprobar G0. Es la fuente de verdad para reanudar una sesión interrumpida y solo
lo escribe el agente orquestador, en cada transición. Al reanudar, se contrasta con el
disco y con `git log`.

```json
{
  "decisions": {
    "D1": { "status": "resolved", "choice": "B", "notes": "tope de 300 s", "date": "2026-10-01" },
    "D11.1": { "status": "resolved", "choice": "B", "notes": "vigía por tubería en stdin", "date": "2026-10-01" }
  },
  "cycles": {
    "C1": {
      "status": "in_progress",
      "phase": "tests_red",
      "gates": [
        { "gate": "plan", "verdict": "approved", "date": "2026-10-01", "notes": "" }
      ],
      "tasks": [
        { "id": "C1.1", "status": "done", "attempts": 1, "files": ["crates/avi-core/src/exit_codes.rs"] }
      ],
      "foreign_changes": ["crates/xtask/src/clean.rs"],
      "commits": []
    }
  },
  "release": { "status": "pending", "gates": [], "version": null }
}
```

- **Subdecisiones.** Las subdecisiones numeradas (las de D7, D8, D11 y D12) se
  registran con clave propia, como `D11.1` o `D12.3`, cada una con su estado, su
  elección y su fecha. Las variantes de una alternativa sin numerar, como el A1 de D3,
  van en `choice` (`"A con A1"`).
- **`gate`** toma uno de estos valores, que corresponden a las compuertas del catálogo
  de la sección 5.1: `g0`, `plan`, `diagnosis`, `tests`, `result`, `deviation` o
  `release`. G-Desvío se registra en el ciclo donde ocurre.
- **`release`** es un objeto de primer nivel, fuera de `cycles`, porque G-Release no
  pertenece a ningún ciclo: guarda su estado, sus compuertas y la versión publicada.
- **`phase`** toma uno de estos valores: `preparation`, `plan`, `diagnosis`,
  `tests_red`, `implementation`, `verification`, `result` o `closed`. Cada uno
  corresponde al paso de la sección 6 del mismo nombre: `plan` es G-Plan, `diagnosis` es
  el diagnóstico y G-Diag, `tests_red` incluye G-Pruebas, `result` es G-Resultado y
  `closed` marca el ciclo con sus commits integrados en `main`.
- **`foreign_changes`** enumera los cambios que el humano tenía en el árbol de trabajo
  al abrir el ciclo. Nunca se incluyen en los commits del ciclo, y un rechazo no los
  revierte.
- Al cerrar cada ciclo, el orquestador guarda también un resumen en la memoria
  persistente.

## 12. Riesgos

| Riesgo | Tratamiento |
|---|---|
| Tras corregir el auxiliar (C6), el staging sigue quedando en disco | G-Desvío con el rastro del auxiliar; el humano decide si se diagnostica la causa, se aplaza el ciclo o se acepta la red de `doctor --repair` como mitigación documentada |
| Una prueba con recursos locales falla al dejar de saltarse, porque llevaba tiempo sin ejecutarse | Se corrige la prueba o el código en C2 si es un defecto pequeño; si destapa un defecto del producto, G-Desvío y el humano decide si se corrige dentro del ciclo o se registra como síntoma nuevo |
| `cargo xtask release` tarda lo que dura la suite con modelos y exige tener los recursos instalados | Es el coste buscado de la puerta; la orden es la misma que se usa en cada ciclo, así que el tiempo no es una sorpresa al publicar |
| Una decisión de G0 resulta inviable al implementarla | G-Desvío; la decisión se reabre con las alternativas actualizadas |
| S14 no se reproduce | En el G-Plan de C3, D2 y su tarea salen del alcance y el síntoma se retira del informe |
| La verificación del agente falla de forma repetida | Tras tres intentos, G-Desvío en lugar de seguir intentándolo |
| El índice estructural del código queda desfasado entre ciclos | Resincronizarlo al cerrar cada ciclo que añada o elimine símbolos |
| La sesión se interrumpe a mitad de un ciclo | Reanudar desde el registro de progreso, contrastándolo con el disco y con `git log` |
| Un hotfix obliga a publicar a mitad de la iteración y reparte los cambios incompatibles entre dos versiones | La regla de D13: un salto por versión publicada y por esquema, y el CHANGELOG los acumula en `## [No publicado]` |
