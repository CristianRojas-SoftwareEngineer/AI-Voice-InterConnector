# Observaciones menores: plan de instalación, tamaño de los modelos y WAV truncado

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja; media en la observación 3 (cada ficha indica la suya) |
| Tipo | diagnóstico (plan de instalación), documentación, funcional (carga de audio) |
| Componente | varios; se indica en cada observación |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 (observaciones 1 y 2); la observación 3 no depende de la plataforma |
| Reproducibilidad | siempre, salvo que se indique otra cosa |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 (1 y 2); revisión del código al diseñar la corrección de `--audio` inexistente, 2026-09-29 (3) |

## Resumen

Tres defectos sueltos, sin causa común con otros, agrupados aquí porque ninguno
justifica, por ahora, un documento propio. En los dos primeros el comando termina con el
resultado correcto, pero el mensaje engaña o la documentación no cuadra con la realidad.
El tercero salió de leer el código y puede producir un resultado incorrecto sin aviso
(severidad media). Cada observación tiene una ficha breve (síntoma, reproducción,
esperado, causa y criterio). Si alguna crece al diagnosticarla, se separa a su propio
documento con la plantilla completa.

## Entorno

Observaciones 1 y 2: binario v0.25.0 instalado desde cero en Windows 11 con
PowerShell 5.1, con todos los modelos provisionados (incluido el de clonado).

Observación 3: lectura del código fuente de v0.25.0. No se ha reproducido todavía; la
ficha indica cómo hacerlo.

## Precondiciones

Las propias de cada observación.

## Pasos para reproducir

Se detallan en cada observación, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada observación.

## Resultado esperado

Se detalla en cada observación.

## Impacto y workaround

Las observaciones 1 y 2 no bloquean ningún caso de uso: una confunde al leer el plan de
instalación y la otra subestima el disco necesario.

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
  aunque la entrada ya está en `HKCU\Environment\Path`.
- **Causa (por verificar):** `compose_summary` (`crates/avi-lifecycle/src/install.rs`)
  tiene una rama «ya está en el PATH; no se modifica» para cuando el plan no cambia
  nada (`path_plan.is_noop()`). En Windows, el plan no se reconoce como no-op con la
  entrada ya presente. Hay que ver cómo compara el plan la entrada existente
  (mayúsculas, barra final, `REG_EXPAND_SZ` con variables sin expandir).
- **Esperado:** «ya está en el PATH; no se modifica».
- **Criterio:** prueba del plan de PATH en Windows con la entrada ya registrada.

### Documentación

#### 2. Tamaño de los modelos subestimado

- **Síntoma:** README, `docs/CLI/commands/SETUP.md`, `docs/GOAL.md`,
  `docs/MANUAL-VALIDATION.md`, `docs/CLI/commands/CLEANUP.md`, `docs/BUILD.md` y
  `docs/specs/sdlc-lifecycle.md` hablan de ~9 GB para la
  selección base y ~11,5 GB con `--with-voice-cloning`. Con el clonado provisionado, la
  raíz de modelos ocupaba 14 GB en disco.
- **Causa (por verificar):** las cifras probablemente no cuentan los derivados CT2 ni la
  caché del backend xet de descarga. Hay que medir cada componente.
- **Esperado:** cifras que coincidan con lo que se ocupa en disco tras `setup`, con un
  margen explícito.
- **Criterio:** los documentos citados dan la misma cifra, medida sobre una instalación
  limpia.

### Límites de entrada

#### 3. Un WAV truncado se carga incompleto sin error

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

## Diagnóstico sugerido

Las observaciones 1 y 2 necesitan una reproducción instrumentada: la 1, el plan de PATH
con la entrada ya registrada; la 2, la medida de cada componente sobre una instalación
limpia. La 3 necesita decidir la tolerancia con cabeceras mal escritas antes de tocar
la carga.

## Criterio de aceptación

Cada observación tiene su criterio en su ficha. El documento se da por resuelto cuando
todas están cerradas o separadas a su propio documento.

## Relacionados

- Contrato de la CLI, reglas de `--audio` (observación 3: la clasificación de un audio
  inválido).
