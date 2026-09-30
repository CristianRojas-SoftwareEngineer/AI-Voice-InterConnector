# Diseño del flujo iterativo para resolver los defectos abiertos

| Campo | Valor |
|---|---|
| Estado | Pendiente de la compuerta de decisiones (G0) |
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
9. [Decisiones abiertas (compuerta G0)](#9-decisiones-abiertas-compuerta-g0)
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
| **G-Resultado** | Al terminar la implementación y la verificación del agente | El paquete de resultado de la sección 5.3 | Si el código, las pruebas, los comentarios y la documentación se aceptan y se confirman en git |
| **G-Desvío** | En cualquier momento, si un hecho invalida algo aprobado | El hecho, su evidencia, qué decisión o plan afecta y las alternativas | Cómo seguir |
| **G-Release** | Cuando el humano quiera, tras cualquier ciclo cerrado | El contenido de `## [No publicado]`, los cambios incompatibles y la versión propuesta | Si se publica, y con qué número |

G-Pruebas existe porque un malentendido del contrato detectado en las pruebas cuesta
unas líneas, y detectado en el resultado cuesta rehacer la implementación.

### 5.2 Veredictos

| Veredicto | Efecto |
|---|---|
| **Aprobar** | El agente avanza al paso siguiente. En G-Resultado, además, hace los commits. |
| **Corregir** | El humano indica qué cambiar. El agente lo incorpora y vuelve a presentar en la misma compuerta, señalando qué cambió. No avanza hasta obtener una aprobación. |
| **Rechazar** | El agente descarta el trabajo del paso: <br>• en G-Plan, el ciclo se replantea o se aplaza; <br>• en G-Diag, se diagnostica con otra hipótesis; <br>• en G-Pruebas, se reescriben las pruebas desde el plan; <br>• en G-Resultado, se revierten los archivos del ciclo y se vuelve a G-Plan. <br>Si el rechazo cuestiona una decisión de G0, esa decisión se reabre con sus alternativas. |

En G0, además, cada decisión admite tres respuestas: elegir una alternativa, pedir más
información (el agente investiga y vuelve a presentar esa decisión) o aplazarla. Un
ciclo no se abre mientras tenga una decisión aplazada.

### 5.3 Paquete y lista de verificación de G-Resultado

Antes de presentar G-Resultado, el agente ejecuta su propia verificación: la batería de
pruebas del workspace, el lint, el formato solo sobre los archivos tocados y la revisión
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
G-Resultado ──► commits ──► registro de progreso actualizado ──► siguiente ciclo
```

1. **Preparación.** Se contrasta el informe del defecto con el código actual y se
   corrige si se ha desfasado, se comprueba el estado del árbol de trabajo y se redacta
   la ficha del ciclo.
2. **G-Plan.**
3. **Diagnóstico y G-Diag**, solo si el ciclo lo requiere.
4. **Pruebas en rojo y G-Pruebas.**
5. **Implementación.** Código, comentarios, documentación canónica, CHANGELOG y cierre
   del informe.
6. **Verificación del agente.**
7. **G-Resultado**, seguida de los commits y la actualización del registro de progreso.

## 7. Mapa de síntomas

| Id | Síntoma | Informe | Severidad | Ciclo |
|---|---|---|---|---|
| S1 | La vía daemon rechaza con 413 los audios de transcripción de más de unos 49 s | `daemon-rechaza-o-corta-audios-largos.md` | Media; se propone alta | C2 |
| S2 | Un kill duro del daemon deja huérfano al motor residente | `motor-residente-huerfano-y-trazas-fuera-del-log.md` | Media | C4 |
| S3 | El daemon no escribe log y los logs del motor no rotan | `residuos-en-disco-tras-comandos-correctos.md` | Media | C3 |
| S4 | La síntesis por daemon sale con exit 1 donde el contrato asigna otro código (4 si falta el modelo) | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja; media sin modelo | C1 |
| S5 | `--daemon` en los cinco comandos solo locales (`list`, `remove` y `play`) responde «Daemon inalcanzable» con exit 5 | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja; se propone media | C1 |
| S6 | `self update` deja artefactos y `doctor` falla hasta la siguiente operación de ciclo de vida | `residuos-en-disco-tras-comandos-correctos.md` | Baja; se propone media | C5 |
| S7 | El `status` JSON afirma operaciones que no ocurrieron (`daemon stop` sin daemon y los simulacros de `self uninstall` y `cleanup`) | `status-json-afirma-operaciones-no-realizadas.md` | Baja | C6 |
| S8 | `speech say` y `speech dub` dejan WAV temporales o informan en `audio_path` de uno ya borrado | `residuos-en-disco-tras-comandos-correctos.md` | Baja | C6 |
| S9 | `daemon.ready` queda en disco tras `daemon stop` | `residuos-en-disco-tras-comandos-correctos.md` | Baja | C4 |
| S10 | Trazas internas de la descarga y del motor en la terminal | `motor-residente-huerfano-y-trazas-fuera-del-log.md` | Baja | C3 |
| S11 | Prefijo `Error:` duplicado | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja | C1 |
| S12 | El evento `start` mide `text_length` en bytes, no en caracteres | `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md` | Baja | C1 |
| S13 | El dub por composición (daemon sin `/dub`) corta la transcripción a los 1500 ms | `daemon-rechaza-o-corta-audios-largos.md` | Baja | C2 |
| S14 | Una referencia de clonado de más de unos 1,5 MB se rechaza con 413 por la vía daemon (por reproducir) | `daemon-rechaza-o-corta-audios-largos.md` | Media | C2 |

S11 no abre ninguna decisión: el mensaje del error se escribe sin prefijo y el prefijo
lo pone quien lo imprime.

## 8. Orden de los ciclos y dependencias

```text
C0 Preparación y decisiones ──G0──►
  C1 Contrato de errores (S4 S5 S11 S12)
    │  C2 usa la tabla única de reason→exit y la lectura del reason en los errores del daemon
    ▼
  C2 Límites de la vía daemon (S1 S13 S14)
    │
    ▼
  C3 Observabilidad (S3 S10)
    │  el diagnóstico de C4 necesita el log del daemon
    ▼
  C4 Vida de los procesos (S2 S9) ········· con G-Diag
    │
    ▼
  C5 Artefactos de self update (S6) ······· con G-Diag
    │
    ▼
  C6 JSON veraz y temporales (S7 S8)

G-Release: a criterio del humano, tras cualquier ciclo cerrado
```

Justificación del orden:

- **C1 va antes que C2, aunque S1 es el síntoma más grave.** La corrección de S1 exige
  que la transcripción lea el `reason` de una respuesta de error y lo traduzca al código
  del contrato, y eso es justo lo que construye C1. Si C2 fuera primero, crearía otra
  tabla local que C1 tendría que deshacer. C1 es pequeño, así que el retraso es mínimo.
- **C3 va antes que C4 por una dependencia dura.** Sin el log del daemon, un fallo del Job
  Object no deja rastro, y el diagnóstico de S2 sería a ciegas.
- **C5 y C6 no dependen entre sí.** C5 va antes por severidad. El humano puede
  invertirlos en el G-Plan de C5.
- **Los ciclos no se ejecutan en paralelo.** Casi todos tocan el binario principal
  (`src/main.rs`), y en paralelo la revisión humana tendría que separar cambios
  entrelazados.

## 9. Decisiones abiertas (compuerta G0)

Todas las decisiones siguen el mismo esquema: el problema, las alternativas con sus
argumentos a favor y en contra, la recomendación y el ciclo que la aplica. El campo
**Decisión** lo rellena el humano en G0, salvo las partes que dependen de un
diagnóstico (la de Windows en D11 y la combinación con C en D12), que se deciden en el
G-Diag de su ciclo.

### D1 · Tope del audio de transcripción en las dos vías (S1) — C2

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

**Decisión:** pendiente.

### D2 · Tamaño de la referencia del clonado (S14) — C2

**Problema.**

- El clonado envía el fichero de referencia completo, tal como está en disco, en base64,
  junto con la referencia de timbre si se indica.
- No hay ningún tope de duración ni de tamaño, así que unas referencias que sumen más de
  unos 1,5 MB (una sola de unos 9 s en WAV de 44,1 kHz estéreo) chocan con el mismo
  límite de 2 MB.
- La vía directa no tiene ese límite.
- El síntoma está deducido del código. C2 lo reproduce con una prueba antes de
  corregirlo; si no se reproduce, esta decisión se descarta en su G-Plan.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Tope de duración de la referencia en las dos vías**, rechazado con `audio_too_long`, y un límite de cuerpo calculado para el peor formato que se admita | La misma regla y el mismo patrón que D1; el error llega antes de transferir nada | Cambio incompatible para las referencias más largas que el tope; hay que conocer cuánta referencia usa de verdad el motor |
| **B. Límite de cuerpo fijo y generoso** (por ejemplo, 32 MB) solo en el daemon | Simple | La regla depende del formato del fichero; las dos vías siguen comportándose distinto |
| **C. El cliente convierte la referencia a 16 kHz mono antes de enviarla** | Reduce el tamaño en cualquier formato | Cambia la entrada del motor y puede degradar el clonado; es trabajo de audio adicional |

**Recomendación: A.** El valor del tope se fija en el G-Plan de C2, a partir de la
duración de referencia que el motor aprovecha realmente. Si el motor recorta la
referencia a N segundos, el tope es N.

**Decisión:** pendiente.

### D3 · Dub por composición para daemons sin `/dub` (S13) — C2

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

**Decisión:** pendiente.

### D4 · Traducción única de `reason` a código de salida (S4, y base de C2) — C1

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

**Decisión:** pendiente.

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

**Decisión:** pendiente.

### D6 · `status` de `daemon stop` cuando no había daemon (S7) — C6

**Problema.** Con el daemon detenido, `daemon stop` borra el pidfile y responde con
`status: "shutdown_sent"` y el texto «Señal de apagado enviada», aunque no envió nada.
El exit 0 es correcto, porque la operación es idempotente.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. `status: "not_running"`**, conservando `daemon: "stopped"` y exit 0, con el texto «El daemon no estaba en ejecución» | Veraz; el mismo patrón que `not_installed` en `self uninstall` | Cambio incompatible en los valores de `status` |
| **B. Mantener `shutdown_sent` y añadir la clave `was_running: false`** | Cambio aditivo, que no sube el esquema | El `status` sigue afirmando algo falso; es una reserva de compatibilidad |
| **C. Salir con otro código, por ejemplo 7** | Señal inequívoca | Rompe la idempotencia de la que dependen los scripts de parada |

**Recomendación: A.**

**Decisión:** pendiente.

### D7 · `status` de los simulacros (S7) — C6

**Problema.** `self uninstall --dry-run` responde `status: "uninstalled"` y
`cleanup --dry-run` responde `status: "cleanup_complete"`, los dos con `dry_run: true`.
Un consumidor que lea solo el `status` concluye que la operación ocurrió. En `cleanup`,
además, la lista `removed` contiene las rutas que se borrarían: el simulacro reutiliza a
propósito el resultado de la limpieza real para que los dos se puedan comparar.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Un único `status: "planned"` para todo simulacro** (`self uninstall` y `cleanup`), conservando `dry_run` como eco de la entrada | Veraz y uniforme: un solo valor que aprender | Cambio incompatible en dos comandos |
| **B. Mantener los valores y documentar la convención** «con `dry_run: true`, el `status` describe el resultado previsto» | Ningún cambio de código ni de contrato | El `status` sigue afirmando un hecho que no ocurrió |
| **C. Corregir solo `self uninstall`** | Cambio mínimo | Dos convenciones distintas para lo mismo |
| **D. Un valor por comando** (`would_uninstall`, `would_clean`) | Explícito | Multiplica los valores sin aportar información que `planned` no dé |

**Recomendación: A, conservando las claves del resultado.** Con `status: "planned"`, el
sobre entero se lee como un plan, incluida la lista `removed` de `cleanup`. Renombrar
las claves en el simulacro rompería la comparación entre simulacro y ejecución real,
que es la razón por la que comparten resultado.

**Decisión:** pendiente.

### D8 · WAV temporales de `speech say` y `speech dub` (S8) — C6

**Problema.** `say` y `dub` sintetizan a un WAV temporal para reproducirlo y devuelven
su ruta en `audio_path`.

- `say` por la vía directa conserva el fichero.
- `say` por la vía daemon lo borra, pero después de emitir su ruta; si la reproducción
  falla, sale antes de borrarlo.
- `dub` lo conserva siempre, en sus tres vías.

Para guardar un audio ya existe `speech synthesize`, que devuelve la ruta de un WAV
persistente.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Borrar el temporal al terminar la reproducción, también si falla, y retirar `audio_path` de `say` y `dub`** | Sin residuos; el JSON solo afirma lo que existe; `synthesize` sigue cubriendo el caso de querer el fichero | Cambio incompatible: se retira una clave |
| **B. Conservar el fichero y la clave en todas las vías** y dejar la limpieza a `cleanup` | Ningún cambio de contrato | Los residuos se acumulan; contradice la regla de que quien crea un fichero temporal lo borra |
| **C. Borrar el fichero y conservar la clave** | Sin residuos | El JSON apunta a algo que no existe, que es justo el defecto actual de la vía daemon |
| **D. Borrar, salvo que un flag nuevo pida conservarlo** | Flexible | Funcionalidad que nadie ha pedido, y duplica `synthesize` |

**Recomendación: A.**

**Decisión:** pendiente.

### D9 · Log del daemon y retención de los logs (S3) — C3

**Problema.**

- El daemon en segundo plano se lanza con sus salidas descartadas, así que se pierden
  sus errores, incluidos los del Job Object que investiga C4.
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
log. Es lo más simple que cumple el criterio del informe y da el log que necesita el
diagnóstico de C4.

**Decisión:** pendiente.

### D10 · Trazas internas en la terminal (S10) — C3

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

**Decisión:** pendiente.

### D11 · Que el motor residente muera con el daemon (S2) — C4

**Problema.** Si el daemon muere de forma abrupta, el motor residente sigue vivo, con el
modelo en memoria y el puerto 8766 ocupado. Los dos sistemas están en situaciones
distintas:

- **Windows.** Existe un Job Object que debería impedirlo, pero una prueba de extremo a
  extremo vio sobrevivir al motor, y un fallo de ese mecanismo no deja rastro.
- **Linux y macOS.** No existe ningún mecanismo.

El motor es código C del repositorio y se puede modificar, registrando la divergencia.

**Windows: la decisión se aplaza a la compuerta G-Diag de C4.** No hay causa confirmada.
El ciclo registra en el log del daemon cada fallo de la asociación al Job y reproduce el
kill duro. Hipótesis que contrastar:

- **H1.** Falla una de las llamadas al Job, por ejemplo porque el proceso ya pertenece a
  otro Job que impide la asociación.
- **H2.** Otro proceso retiene por herencia un handle al Job, y el Job no se cierra al
  morir el daemon.
- **H3.** El proceso que se mató no es el asociado al Job; por ejemplo, con
  `--auto-restart`.
- **H4.** Un Job padre con permisos de salida deja al motor fuera del Job del daemon.

**Unix: decisión que se puede tomar ya.**

| Alternativa | A favor | En contra |
|---|---|---|
| **A. `PR_SET_PDEATHSIG` al lanzar el motor** | No toca el motor; lo garantiza el kernel | Solo existe en Linux, no en macOS; la señal se dispara al morir el *hilo* que lanzó el proceso, no el proceso, así que hay que lanzar el motor desde un hilo que viva tanto como el daemon |
| **B. El motor vigila a su padre**: el daemon mantiene abierto el extremo de escritura de una tubería conectada a la entrada estándar del motor, y el motor termina cuando lee fin de fichero | Funciona igual en Linux, macOS y Windows, así que también sirve de segunda defensa si el Job Object falla; lo garantiza el sistema operativo al cerrar los handles del proceso muerto | Modifica el motor vendorizado, lo que hay que registrar en sus divergencias; falta comprobar cómo trata hoy el motor su entrada estándar, y con ello el coste real del cambio |
| **C. Grupo de procesos y kill al grupo desde el supervisor** | Sencillo | No sirve si el daemon muere por `SIGKILL`, que es justo el caso |
| **D. No hacer nada en Unix** y documentar que el siguiente `daemon start` o `daemon stop` reclama al huérfano | Ningún trabajo | El huérfano retiene la memoria y el puerto hasta entonces |

**Recomendación: B.** Es el único mecanismo portable y cubre el caso del `SIGKILL`. Si
en G-Diag se confirma que B también basta en Windows, el humano puede decidir allí si
se conserva el Job Object como segunda defensa o se sustituye.

**Decisión (Unix):** pendiente. **Decisión (Windows):** en el G-Diag de C4.

### D12 · Artefactos de `self update` y el veredicto de `doctor` (S6) — C5

**Problema.**

- En Windows, un binario en ejecución no se puede borrar, así que `self update` aparca
  el anterior como `.old-*`.
- Ese aparcado solo lo recoge el barrido de la siguiente operación de ciclo de vida.
  Mientras tanto, `doctor` lo cuenta como fallo y sale con exit 1.
- El `.zip` descargado vive en un directorio de staging hermano del directorio del
  programa. Su limpieza intenta borrarlo en el acto; si falla, programa un proceso
  auxiliar que espera a que termine la CLI y lo borra; si también falla, lo deja para la
  recuperación. En la prueba de extremo a extremo el staging quedó en disco, así que
  alguno de esos pasos falló sin que se sepa cuál; se diagnostica en C5.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Barrer los aparcados al inicio de cualquier invocación** | El residuo desaparece en la invocación siguiente, `doctor` incluido; `doctor` sigue siendo estricto | Añade un listado de directorio a cada comando y una escritura a comandos de solo lectura; no debe tocar un binario que otra instancia esté usando |
| **B. `doctor` trata los aparcados como aviso, no como fallo** | `doctor` no modifica nada | El residuo persiste hasta la próxima operación de ciclo de vida, que puede no llegar nunca |
| **C. Programar el borrado del aparcado con el auxiliar que ya se usa para el staging** | Limpieza inmediata, sin esperar a otra invocación; reutiliza un mecanismo existente, pensado ya para los aparcados | Ese auxiliar no evitó que el staging quedara en disco en la prueba de extremo a extremo; hasta que el diagnóstico de C5 explique por qué, no se puede confiar en él |
| **D. `doctor` barre antes de comprobar** | Arregla el caso de `doctor` | Un comando de diagnóstico que modifica el sistema; los demás comandos siguen sin barrer |

**Recomendación: A, y decidir en el G-Diag de C5 si se combina con C.**

- A reutiliza el barrido existente, que ya distingue qué aparcados se pueden recoger, y
  hace de red de seguridad para cualquier residuo que el auxiliar no llegue a borrar.
- C solo se añade si el diagnóstico de C5 encuentra y corrige el fallo del auxiliar;
  entonces el aparcado desaparece sin esperar a la invocación siguiente.
- Si el cambio deja sin uso la lista de restos de la transacción, se elimina en el mismo
  ciclo.

**Decisión:** la parte A, pendiente en G0; la combinación con C, en el G-Diag de C5.

### D13 · Versión de los esquemas (transversal)

**Problema.** El sobre JSON de la CLI está en el esquema 4 y el protocolo del daemon, en
el 3. Según el contrato:

- añadir una clave o un `reason` no sube la versión;
- retirar o cambiar el significado de una clave, o cambiar los valores de un `status`,
  sí la sube.

Los cambios de esta iteración que suben un esquema son estos:

- **Protocolo del daemon:** la unidad de `text_length` pasa a caracteres (C1).
- **Sobre de la CLI:** los nuevos valores de `status` (D6 y D7) y la retirada de
  `audio_path` (D8), los tres en C6.
- **Ninguno de los dos:** el cambio de código de salida de D5 no toca ninguna clave.
  Cambia el contrato, pero no el esquema, y se documenta en el CHANGELOG.

| Alternativa | A favor | En contra |
|---|---|---|
| **A. Una subida por versión publicada**: la hace el primer ciclo que rompe un esquema, y los siguientes no la repiten hasta la publicación | El consumidor ve un solo salto por versión; los ciclos pueden publicarse por separado sin coordinarse | Si se publica entre dos ciclos que rompen el mismo esquema, habrá dos saltos, que es lo correcto |
| **B. Una subida por ciclo** | Mecánico | Saltos de versión que ningún consumidor llega a ver |
| **C. No subir** | Ningún trabajo | Incumple la política del propio contrato |

**Recomendación: A.**

- C1 sube el protocolo del daemon de 3 a 4 y documenta `text_length` en caracteres, la
  misma unidad que el tope de 500 caracteres de `--text`.
- C6 sube el sobre de la CLI de 4 a 5.

**Decisión:** pendiente.

## 10. Ciclos

Cada ficha resume lo que el G-Plan del ciclo concretará y lo que el humano deberá
aprobar. Las pruebas que se enumeran son las mínimas.

### C0 · Preparación y decisiones

- **Objetivo:** dejar resueltas todas las decisiones que no dependen de un diagnóstico,
  y los informes, al día.
- **Tarea del agente antes de G0:** presentar la sección 9.
- **Compuerta:** G0. Si el humano elige una alternativa distinta de la recomendada, el
  agente ajusta las fichas de los ciclos afectados y las vuelve a presentar antes de
  abrir C1.
- **Salida:**
  - las decisiones D1 a D10, D13, la parte A de D12 y la parte Unix de D11, resueltas;
  - el registro de progreso, creado;
  - los informes actualizados, incluidos en el primer commit tras aprobar G0.

### C1 · Contrato de errores

- **Causa raíz:** no hay un único sitio que traduzca las causas de error a `reason` y a
  código de salida.
- **Síntomas:** S4, S5, S11 y S12.
- **Decisiones:** D4, D5 y D13 (la subida del protocolo del daemon).
- **Tareas:**
  1. Tabla única de `reason` a código de salida y lectura del `reason` en cualquier
     respuesta de error del daemon (D4).
  2. La síntesis por daemon sale con el código del contrato (S4).
  3. `--daemon` en los comandos solo locales (D5).
  4. Un solo prefijo `Error:` (S11).
  5. `text_length` en caracteres y protocolo del daemon en la versión 4 (S12, D13).
- **Pruebas en rojo:**
  - la tabla cubre todos los `reason` del contrato, y el código explícito de la vía
    directa coincide con ella en los compartidos;
  - la síntesis por daemon sin modelo sale con 4 y `model_missing`;
  - `--daemon` en `voice list`, `voice remove`, `speech list`, `speech play` y
    `speech remove` sale con el código y el `reason` decididos;
  - golden de `--temperature` fuera de rango, con un solo prefijo;
  - el `text_length` de «canción» es 7.
- **Documentación:** el contrato, `speech`, `voice` y la descripción del protocolo del
  daemon.
- **Cierra:** `mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md`.

### C2 · Límites de la vía daemon

- **Causa raíz:** la vía daemon tiene límites de transporte ajenos a los del producto y
  un camino de compatibilidad con su propio límite de tiempo.
- **Depende de:** C1.
- **Síntomas:** S1, S13 y S14.
- **Decisiones:** D1, D2 y D3.
- **Tareas:**
  1. Tope de transcripción y límite de cuerpo derivado de él (D1).
  2. Reproducción de S14 y tope de la referencia del clonado (D2).
  3. Eliminar el dub por composición (D3).
- **Pruebas en rojo:**
  - la ruta de transcripción acepta un cuerpo de más de 2 MB y por debajo del tope;
  - un audio por encima del tope se rechaza con `audio_too_long` y exit 2 en las dos
    vías;
  - una referencia de clonado mayor que 1,5 MB y dentro del tope se acepta por daemon;
  - un 404 en `/dub` produce el error decidido.
- **Documentación:** el contrato, `speech`, `voice` y la descripción del protocolo del
  daemon.
- **Cierra:** `daemon-rechaza-o-corta-audios-largos.md`.

### C3 · Observabilidad

- **Causa raíz:** las trazas no llegan a su destino: las del daemon se pierden y las
  internas aparecen en la terminal.
- **Síntomas:** S3 y S10.
- **Decisiones:** D9 y D10.
- **Tareas:**
  1. Log del daemon.
  2. Poda al crear un log, en las dos familias.
  3. Salida del motor durante el clonado hacia su log.
  4. Nivel de trazas por proceso, corrigiendo de paso el comentario de la inicialización
     de las trazas, que cita un esquema del sobre JSON ya superado.
- **Pruebas en rojo:**
  - tras N creaciones de log quedan K por familia;
  - un error del daemon en segundo plano aparece en su log;
  - la salida humana de `voice clone` y de `setup` no contiene trazas internas.
- **Documentación:** la política de logs en la documentación del daemon y de
  `cleanup`.
- **Cierra:** la ficha de logs de `residuos-en-disco-tras-comandos-correctos.md` y la
  ficha de trazas de `motor-residente-huerfano-y-trazas-fuera-del-log.md`.

### C4 · Vida de los procesos

- **Causa raíz:** los procesos del daemon no dejan limpio su estado al terminar, ni de
  forma ordenada ni abrupta.
- **Depende de:** C3.
- **Síntomas:** S2 y S9.
- **Decisiones:** D11, la parte Unix ya resuelta en G0 y la de Windows en G-Diag.
- **Diagnóstico:**
  - registrar en el log cada fallo de la asociación al Job;
  - reproducir el kill duro con y sin `--auto-restart`;
  - contrastar las hipótesis H1 a H4.
- **G-Diag:** la causa confirmada y, si la corrección abre alternativas, su explicación
  con argumentos y recomendación.
- **Pruebas en rojo:**
  - tras matar el daemon a la fuerza no queda ningún proceso del motor ni nadie
    escuchando en el puerto 8766; si no se puede automatizar, se documenta como
    verificación manual;
  - un fallo de la asociación al Job queda en el log;
  - tras `daemon stop` no quedan ni `daemon.ready` ni `daemon.pid`;
  - la recuperación de un daemon caído a partir de `daemon.ready` sigue funcionando.
- **Documentación:** la descripción del daemon, `MANUAL-VALIDATION.md` y las
  divergencias del motor, si se aplica D11-B.
- **Cierra:** `motor-residente-huerfano-y-trazas-fuera-del-log.md` y la ficha de
  `daemon.ready` de `residuos-en-disco-tras-comandos-correctos.md`.

### C5 · Artefactos de `self update`

- **Causa raíz:** los artefactos de la actualización solo se recogen en la siguiente
  operación de ciclo de vida, y `doctor` los cuenta como fallo mientras tanto.
- **Síntomas:** S6.
- **Decisiones:** la parte A de D12, resuelta en G0, y su combinación con C, en G-Diag.
- **Diagnóstico:** reproducir `self update --force` registrando el resultado de cada
  paso de la limpieza del staging (borrado inmediato, borrado programado o conservación)
  para ver cuál falla y por qué.
- **G-Diag:** la causa del staging residual, la dirección de la corrección y si el
  borrado programado es fiable para combinar D12-A con D12-C.
- **Pruebas en rojo:**
  - tras `self update` y cualquier invocación posterior, solo queda lo instalado;
  - `doctor` pasa.
- **Documentación:** `self` y `doctor`.
- **Cierra:** la ficha de `self update` de `residuos-en-disco-tras-comandos-correctos.md`.

### C6 · JSON veraz y temporales

- **Causa raíz:** algunos sobres JSON describen lo que se pidió o lo que habría pasado,
  no lo que pasó, y quien crea un fichero temporal no siempre lo borra.
- **Síntomas:** S7 y S8.
- **Decisiones:** D6, D7, D8 y D13 (la subida del sobre de la CLI).
- **Pruebas en rojo:**
  - golden de `daemon stop` sin daemon, con el `status` decidido;
  - golden de `self uninstall --dry-run` y de `cleanup --dry-run`, con el `status`
    decidido y las mismas claves que la ejecución real;
  - tras `speech say` (vía directa y daemon) y `speech dub` (vía directa, daemon y
    composición), también cuando falla la reproducción, no queda ningún WAV temporal y
    el JSON no contiene `audio_path`.
- **Documentación:** el contrato, `speech`, `daemon`, `self` y `cleanup`.
- **Cierra:** `status-json-afirma-operaciones-no-realizadas.md` y
  `residuos-en-disco-tras-comandos-correctos.md`, que para entonces se queda sin fichas.
- **Cierre de la iteración:** el agente propone eliminar este documento y su registro
  de progreso, y lo presenta en la misma G-Resultado.

## 11. Registro de progreso

El avance se registra en `docs/issues/iterative-workflow-design.progreso.json`, que se
crea al aprobar G0. Es la fuente de verdad para reanudar una sesión interrumpida y solo
lo escribe el agente orquestador, en cada transición. Al reanudar, se contrasta con el
disco y con `git log`.

```json
{
  "decisions": {
    "D1": { "status": "resolved", "choice": "B", "notes": "tope de 300 s", "date": "2026-10-01" }
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
  }
}
```

- **`phase`** toma uno de estos valores: `preparation`, `plan`, `diagnosis`,
  `tests_red`, `implementation`, `verification`, `result` o `closed`.
- **`foreign_changes`** enumera los cambios que el humano tenía en el árbol de trabajo
  al abrir el ciclo. Nunca se incluyen en los commits del ciclo, y un rechazo no los
  revierte.
- Al cerrar cada ciclo, el orquestador guarda también un resumen en la memoria
  persistente.

## 12. Riesgos

| Riesgo | Tratamiento |
|---|---|
| Un diagnóstico (C4, C5) no confirma ninguna hipótesis | G-Diag con los hallazgos parciales; el humano decide si se amplía el diagnóstico, se aplaza el ciclo o se acepta una mitigación documentada |
| Una decisión de G0 resulta inviable al implementarla | G-Desvío; la decisión se reabre con las alternativas actualizadas |
| S14 no se reproduce | En el G-Plan de C2, D2 y su tarea salen del alcance y el síntoma se retira del informe |
| La verificación del agente falla de forma repetida | Tras tres intentos, G-Desvío en lugar de seguir intentándolo |
| El índice estructural del código queda desfasado entre ciclos | Resincronizarlo al cerrar cada ciclo que añada o elimine símbolos |
| La sesión se interrumpe a mitad de un ciclo | Reanudar desde el registro de progreso, contrastándolo con el disco y con `git log` |
| Los cambios incompatibles quedan repartidos entre varias versiones publicadas | La regla de D13: un salto por versión publicada y por esquema, y el CHANGELOG los acumula en `## [No publicado]` |
