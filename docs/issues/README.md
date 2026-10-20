# Registro de defectos

Cada defecto conocido y aún abierto tiene aquí un documento propio. Todos siguen la
misma estructura para que quien lo diagnostique encuentre la misma información en el
mismo sitio: qué pasa, cómo reproducirlo, qué se esperaba, qué se sabe de la causa y
cuándo se da por resuelto.

## Convenciones

- **Un defecto por archivo.** El nombre es semántico, en español y en kebab-case, y
  describe el síntoma (por ejemplo, `daemon-stop-deja-fichero-ready.md`), no un
  identificador numérico. Varios síntomas con una causa común o una misma corrección
  cuentan como un solo defecto: comparten documento, con una ficha por síntoma dentro de
  «Análisis de causa». Los defectos menores sueltos se reúnen en
  `observaciones-menores.md` hasta que crecen lo bastante para tener documento propio.
- **Autocontenido.** El documento se entiende sin haber vivido la sesión en que se
  detectó: se copian la salida literal y los comandos exactos, sin remitir a registros
  transitorios.
- **Código citado por archivo y símbolo**, no por número de línea: los números cambian
  con cualquier edición y el símbolo sigue siendo encontrable.
- **Hechos separados de hipótesis.** Cada afirmación sobre la causa lleva su grado de
  confianza: *confirmada* (verificada en el código o reproducida), *probable* (encaja
  con la evidencia, sin verificar) o *por verificar*.
- **Ciclo de vida.** Cuando se resuelve un defecto, su contenido vigente se incorpora a
  los documentos canónicos (contrato, documentación del comando, CHANGELOG) y el informe
  se conserva en este mismo directorio con estado «resuelto», la fecha de la resolución,
  un enlace a la prueba de regresión que lo cubre y un enlace al documento canónico que
  incorpora su contenido. Los informes no se borran.

## Escala de severidad

| Severidad | Criterio |
|---|---|
| Crítica | Impide el flujo principal (instalar, sintetizar) sin workaround razonable, o expone el equipo del usuario |
| Alta | Rompe un caso de uso documentado o un contrato del que dependen scripts, con workaround |
| Media | Falla intermitente o diagnóstico engañoso que hace perder tiempo, sin pérdida de datos |
| Baja | Cosmético o de mensaje; el resultado final es correcto |

## Plantilla

Copiar este bloque para cada defecto nuevo, conservando todos los encabezados. Si una
sección no aplica, se escribe «No aplica» y el motivo, sin borrarla.

```markdown
# <Síntoma en una frase: qué falla y en qué condición>

| Campo | Valor |
|---|---|
| Estado | abierto \| en diagnóstico \| resuelto en X.Y.Z |
| Severidad | crítica \| alta \| media \| baja |
| Tipo | funcional \| contrato \| seguridad \| diagnóstico \| rendimiento \| documentación |
| Componente | crate, módulo o artefacto afectado |
| Versión detectada | X.Y.Z |
| Plataforma | SO y shell donde se observó; si se sabe, dónde no ocurre |
| Reproducibilidad | siempre \| intermitente (N de M intentos) |
| Detectado en | actividad y fecha (AAAA-MM-DD) |

## Resumen

Dos o tres frases: el síntoma, a quién afecta y por qué importa.

## Entorno

Versión del binario, SO, shell, canal de instalación, estado del daemon, modelos
provisionados y variables `AVI_*` relevantes.

## Precondiciones

Estado necesario antes de reproducir: qué debe estar instalado, arrancado u ocupado.

## Pasos para reproducir

Comandos exactos, numerados, que un tercero pueda ejecutar tal cual.

## Resultado observado

Salida literal (stdout/stderr), código de salida, `reason` del envelope JSON y tiempos.

## Resultado esperado

Lo que debería ocurrir y la fuente que lo establece (contrato de la CLI, especificación,
comentario del código), nombrada por su contenido.

## Impacto y workaround

A quién afecta, en qué flujo, y cómo sortearlo mientras tanto.

## Evidencia

Datos que sostienen el diagnóstico: volcados, inspección de bytes, procesos, puertos,
comparación entre plataformas o versiones.

## Análisis de causa

Hipótesis ordenadas, cada una con su confianza (confirmada, probable, por verificar), los
archivos y símbolos implicados y lo que ya se descartó.

## Diagnóstico sugerido

Siguientes pasos concretos para confirmar la causa o acotarla, cuando no está confirmada.
Si lo está, la dirección de la corrección y sus alternativas.

## Criterio de aceptación

Condiciones verificables que dan el defecto por resuelto, incluida la prueba de
regresión que debe existir para que no vuelva.

## Relacionados

Otros defectos, reglas del contrato o cambios del CHANGELOG vinculados.
```

## Defectos abiertos

Indicadores de progreso (`PhaseProgress` y los eventos del daemon), detectados el 2026-10-19 sobre la 0.34.0.

| Documento | Severidad | Causa raíz en una frase |
|---|---|---|
| [`fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md`](fase-de-progreso-deja-la-linea-abierta-y-se-pega-a-lo-que-sigue.md) | alta | No hay cierre explícito en los caminos de salida y el indicador no es dueño de la línea, de modo que muchos caminos ceden la consola a otro escritor o salen sin cerrar la fase |
| [`progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md`](progreso-imprime-una-linea-por-evento-y-no-avanza-durante-el-trabajo.md) | media | No hay un protocolo único de actualización: en no-TTY cada llamada es una línea y en TTY las fases bloqueantes no se repintan |
| [`la-rama-local-y-la-del-daemon-muestran-fases-y-cierres-distintos.md`](la-rama-local-y-la-del-daemon-muestran-fases-y-cierres-distintos.md) | media | La rama daemon pinta una o dos fases gruesas y confía en eventos que `/transcribe` y `/translate` no emiten, y que no cubren todas las etapas de la rama local |
| [`los-cierres-y-los-relojes-del-progreso-se-duplican.md`](los-cierres-y-los-relojes-del-progreso-se-duplican.md) | baja | Dos dueños para el mensaje de resultado y tres relojes distintos midiendo la misma operación |
| [`observaciones-menores.md`](observaciones-menores.md) | baja / media | Diez observaciones sueltas de la misma familia |

Este registro conserva también los documentos ya resueltos, con su estado «resuelto»
y la evidencia que los cierra. Su cuenta actual es de cuatro documentos temáticos más
`observaciones-menores.md`.
