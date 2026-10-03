# El motor residente sobrevive a un kill duro del daemon

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media |
| Tipo | funcional, diagnóstico |
| Componente | CLI (`src/main.rs`, rama `serve`) y motor TTS (`crates/avi-tts`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1; en Unix no hay mecanismo equivalente |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Si el daemon muere de forma abrupta, el motor residente sigue vivo con el modelo en
memoria y el puerto 8766 ocupado, aunque ya existe un mecanismo para evitarlo que falla
sin interrumpir el comando.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados y el daemon en el puerto por defecto.

## Precondiciones

Daemon arrancado con el motor residente cargado.

## Pasos para reproducir

Se detallan en «Análisis de causa».

## Resultado observado

Se detalla en «Análisis de causa».

## Resultado esperado

El residente muere con el daemon, y un fallo del mecanismo que lo garantiza queda
registrado.

## Impacto y workaround

El residente huérfano retiene la memoria del modelo y el puerto 8766 hasta que un
`daemon start` o un `daemon stop` lo detecta y lo reclama, que es también el workaround.

## Evidencia

Tras `Stop-Process -Force` sobre el daemon, `qwen_tts` seguía vivo y escuchando en
`127.0.0.1:8766`, el pidfile quedaba obsoleto y `daemon status` decía `stopped`.

## Análisis de causa

### 1. Un kill duro del daemon deja huérfano al motor residente

- **Severidad:** media. El residente huérfano retiene la memoria del modelo y el puerto
  8766, y el mecanismo que debía evitarlo falla sin interrumpir el comando.
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
  registrar nada, de modo que el fallo no deja rastro en el log del daemon
  (`data/logs/daemon_*.log`). Hay que averiguar cuál de esas llamadas falla, o si el
  residente acaba fuera del Job por otra vía. `--auto-restart` supervisa dentro del
  proceso, así que no puede actuar si el proceso muere. En Unix no hay mecanismo
  equivalente.
- **Esperado:** el residente muere con el daemon. En Windows, el Job funciona y un
  fallo al crearlo o al asociarlo queda en el log del daemon. En Unix,
  `PR_SET_PDEATHSIG` (Linux) o que el residente vigile a su padre.
- **Criterio:** tras matar el daemon a la fuerza, en menos de unos segundos no queda
  ningún `qwen_tts` ni nada escuchando en 8766. Un fallo al asociar el Job aparece en el
  log del daemon.

## Diagnóstico sugerido

Reproducir el kill duro y revisar `data/logs/daemon_*.log` para ver qué paso de la
asociación al Job falla.

## Criterio de aceptación

Se cumple el criterio del síntoma.
