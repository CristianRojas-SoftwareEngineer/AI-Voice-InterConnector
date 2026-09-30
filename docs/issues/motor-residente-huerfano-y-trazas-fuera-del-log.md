# El motor residente sobrevive a un kill duro del daemon y las trazas internas acaban en la terminal

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media por el síntoma 1; baja en el síntoma 2 |
| Tipo | funcional, diagnóstico |
| Componente | CLI (`src/main.rs`, rama `serve`), motor TTS (`crates/avi-tts`), descarga de modelos (`setup`) y `voice clone` |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1; en Unix no hay mecanismo equivalente al del síntoma 1 |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Las trazas y la vida de los procesos no acaban donde deberían. Si el daemon muere de
forma abrupta, el motor residente sigue vivo con el modelo en memoria y el puerto 8766
ocupado, aunque ya existe un mecanismo para evitarlo que falla sin dejar rastro. En el
otro sentido, las trazas internas de la descarga y del motor salen por la terminal en
lugar de ir al log y tapan el resumen del comando.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados (incluido el de clonado) y el daemon en el puerto por defecto.

## Precondiciones

Síntoma 1: daemon arrancado con el motor residente cargado. Síntoma 2: modelos sin
descargar, o una referencia de voz para clonar.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

El residente muere con el daemon, y un fallo del mecanismo que lo garantiza queda
registrado. En modo humano la terminal muestra solo mensajes del producto; las trazas
internas van al log.

## Impacto y workaround

Síntoma 1: el residente huérfano retiene la memoria del modelo y el puerto 8766 hasta
que un `daemon start` o un `daemon stop` lo detecta y lo reclama, que es también el
workaround. Síntoma 2: solo estorba la lectura de la salida.

## Evidencia

Tras `Stop-Process -Force` sobre el daemon, `qwen_tts` seguía vivo y escuchando en
`127.0.0.1:8766`, el pidfile quedaba obsoleto y `daemon status` decía `stopped`.

## Análisis de causa

### 1. Un kill duro del daemon deja huérfano al motor residente

- **Severidad:** media. El residente huérfano retiene la memoria del modelo y el puerto
  8766, y el mecanismo que debía evitarlo falla sin dejar rastro.
- **Síntoma:** con `Stop-Process -Force` sobre el daemon (con o sin `--auto-restart`),
  el `qwen_tts` residente sigue vivo escuchando en `127.0.0.1:8766`, el pidfile queda
  obsoleto y `daemon status` dice `stopped`. El siguiente `daemon start` o `daemon stop`
  lo detecta y lo reclama.
- **Reproducción:** arrancar el daemon, esperar a que el residente esté listo, ejecutar
  `Stop-Process -Force` sobre el PID del daemon y comprobar los procesos `qwen_tts` y el
  puerto 8766.
- **Causa (por diagnosticar):** en Windows el mecanismo ya existe y falla. El proceso
  longevo del daemon (`serve`) se asocia a un Job Object con `KILL_ON_JOB_CLOSE`
  (`install_job_with_tree_kill`, `src/main.rs`), y el motor se lanza a propósito sin
  breakaway ni grupo propio (`crates/avi-tts/src/lib.rs`) para heredar ese Job. Con eso,
  matar el daemon debería cerrar el Job y matar al residente, pero la prueba E2E lo vio
  sobrevivir. La asociación es silenciosa: si `CreateJobObjectW`,
  `SetInformationJobObject` o `AssignProcessToJobObject` fallan, la función vuelve sin
  registrar nada, y como el daemon no tiene log propio el fallo no deja rastro. Hay que
  averiguar cuál de esas llamadas falla, o si el residente acaba fuera del Job por otra
  vía. `--auto-restart` supervisa dentro del proceso, así que no puede actuar si el
  proceso muere. En Unix no hay mecanismo equivalente.
- **Esperado:** el residente muere con el daemon. En Windows, el Job funciona y un
  fallo al crearlo o al asociarlo queda en el log del daemon. En Unix,
  `PR_SET_PDEATHSIG` (Linux) o que el residente vigile a su padre.
- **Criterio:** tras matar el daemon a la fuerza, en menos de unos segundos no queda
  ningún `qwen_tts` ni nada escuchando en 8766. Un fallo al asociar el Job aparece en el
  log del daemon.

### 2. Logs internos en la salida humana

- **Síntoma:** en modo humano, la descarga de modelos (backend xet de Hugging Face) y
  `voice clone` vuelcan en la terminal las trazas de progreso y los logs del motor, que
  tapan el resumen del comando.
- **Causa (por verificar):** la biblioteca de descarga escribe su propio progreso y el
  motor hereda o reenvía su stderr durante el clonado.
- **Esperado:** en modo humano, una sola línea de progreso del producto; las trazas
  internas van al log.
- **Criterio:** `voice clone` y `setup` en modo humano muestran solo mensajes propios.

## Diagnóstico sugerido

El síntoma 1 empieza por el log del daemon: primero se añade, después se reproduce el
kill duro para ver qué paso de la asociación al Job falla. El síntoma 2 necesita una
reproducción instrumentada para separar lo que escribe la biblioteca de descarga de lo
que reenvía el motor.

## Criterio de aceptación

Se cumplen los criterios de los dos síntomas.

## Relacionados

- [residuos-en-disco-tras-comandos-correctos.md](residuos-en-disco-tras-comandos-correctos.md):
  su síntoma de logs sin rotación incluye la falta de log del daemon, requisito del
  diagnóstico del síntoma 1.
