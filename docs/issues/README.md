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
  se elimina en ese mismo cambio, sin esperar a publicar la versión: la corrección queda
  en el CHANGELOG y la prueba de regresión, en el código.

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
| Tipo | funcional \| contrato \| seguridad \| diagnóstico \| rendimiento |
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

Salida literal (stdout/stderr), código de salida, `reason` del sobre JSON y tiempos.

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

| Documento | Severidad | Síntoma |
|---|---|---|
| [daemon-rechaza-o-corta-audios-largos.md](daemon-rechaza-o-corta-audios-largos.md) | Media | La vía daemon rechaza con 413 los audios de más de ~49 s y el dub antiguo corta la transcripción a los 1500 ms |
| [motor-residente-huerfano-y-trazas-fuera-del-log.md](motor-residente-huerfano-y-trazas-fuera-del-log.md) | Media | Un kill duro del daemon deja vivo al motor residente pese al Job Object, y las trazas internas salen por la terminal |
| [residuos-en-disco-tras-comandos-correctos.md](residuos-en-disco-tras-comandos-correctos.md) | Media | WAV temporales, `daemon.ready`, logs sin rotación ni log del daemon, y artefactos de `self update` |
| [mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md](mensajes-y-codigos-de-salida-incoherentes-con-el-contrato.md) | Baja (media en la síntesis por daemon sin modelo) | Sin mapeo único de `reason` a código de salida: prefijo `Error:` duplicado, `daemon_unreachable` como comodín, síntesis por daemon con exit 1 y `text_length` en bytes |
| [status-json-afirma-operaciones-no-realizadas.md](status-json-afirma-operaciones-no-realizadas.md) | Baja | `daemon stop` sin daemon dice `shutdown_sent` y el simulacro de `self uninstall` dice `uninstalled` |
| [observaciones-menores.md](observaciones-menores.md) | Media | WAV truncado cargado sin error |
