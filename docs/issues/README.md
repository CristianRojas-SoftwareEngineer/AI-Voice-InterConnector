# Registro de defectos

Cada defecto conocido y aún abierto tiene aquí un documento propio. Todos siguen la
misma estructura para que quien lo diagnostique encuentre la misma información en el
mismo sitio: qué pasa, cómo reproducirlo, qué se esperaba, qué se sabe de la causa y
cuándo se da por resuelto.

## Convenciones

- **Un defecto por archivo.** El nombre es semántico, en español y en kebab-case, y
  describe el síntoma (`daemon-start-puerto-ocupado.md`), no un identificador numérico.
- **Autocontenido.** El documento se entiende sin haber vivido la sesión en que se
  detectó: se copian la salida literal y los comandos exactos, sin remitir a registros
  transitorios.
- **Código citado por archivo y símbolo**, no por número de línea: los números cambian
  con cualquier edición y el símbolo sigue siendo encontrable.
- **Hechos separados de hipótesis.** Cada afirmación sobre la causa lleva su grado de
  confianza: *confirmada* (verificada en el código o reproducida), *probable* (encaja
  con la evidencia, sin verificar) o *por verificar*.
- **Ciclo de vida.** Cuando se corrige un defecto, se cambia su estado a `resuelto` y se
  indica la versión que lo corrige. El documento se borra al publicar esa versión: la
  corrección queda en el CHANGELOG y la prueba de regresión, en el código.

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
| [doctor-falla-tras-instalacion.md](doctor-falla-tras-instalacion.md) | Alta | `doctor` falla justo después de una instalación correcta |
| [text-sin-limite-de-longitud.md](text-sin-limite-de-longitud.md) | Media | `--text` de más de 5000 caracteres no se rechaza con exit 2 |
| [audio-inexistente-exit-10.md](audio-inexistente-exit-10.md) | Media | `--audio` inexistente sale con 10 en vez de 3 |
| [dub-daemon-timeout-intermitente.md](dub-daemon-timeout-intermitente.md) | Media | `speech dub --daemon` falla de forma intermitente por timeout |
| [daemon-start-puerto-ocupado.md](daemon-start-puerto-ocupado.md) | Baja | `daemon start` con el puerto ocupado espera 10 s y da un error opaco |
| [observaciones-menores.md](observaciones-menores.md) | Baja | Doce defectos menores de mensajes, residuos, ruido de salida y documentación |
