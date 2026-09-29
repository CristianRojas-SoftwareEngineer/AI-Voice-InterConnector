# `daemon start` con el puerto ocupado espera 10 s y falla con un error que no menciona el puerto

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja |
| Tipo | diagnóstico |
| Componente | binario principal (`src/main.rs`, `daemon start`) y `avi-daemon` (`spawn_background`, bind del listener) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11. Por el análisis de causa, afecta a todas las plataformas |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Si otro proceso ocupa el puerto del daemon (8765 por defecto), `daemon start` espera el
deadline completo de 10 s y sale con exit 5 `daemon_unreachable`. El mensaje habla de
un «fichero ready» vacío y no menciona el puerto ni el conflicto. La causa real (el
proceso hijo murió al no poder enlazar) se pierde. El resultado final es correcto (no
arranca, no deja huérfanos ni pidfile), pero el usuario no sabe qué hacer.

## Entorno

- Binario: v0.25.0 instalado.
- SO: Windows 11.
- Sin `AVI_DAEMON_PORT`: puerto por defecto 8765.

## Precondiciones

- Daemon detenido.
- Un proceso ajeno escuchando en `127.0.0.1:8765`.

## Pasos para reproducir

```powershell
$l = [System.Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback, 8765); $l.Start()
Measure-Command { ai-voice-interconnector daemon start }
echo $LASTEXITCODE
$l.Stop()
```

## Resultado observado

- Unos 10 s de espera sin salida.
- Mensaje: «el fichero ready <data>\daemon.ready no publicó addr válida tras 10s (último
  contenido: "")».
- Exit 5, `reason` `daemon_unreachable`.
- No quedan procesos del daemon ni pidfile.

## Resultado esperado

- Fallo inmediato (en cuanto el hijo muere), sin agotar el deadline.
- Un mensaje que nombre el puerto y el conflicto (por ejemplo, «el puerto 8765 está en
  uso por otro proceso») y proponga `AVI_DAEMON_PORT` como salida.
- Exit 6 (`StateConflict`), que la tabla de códigos del contrato define como «el
  recurso existe o está ocupado; la operación no procede sin liberarlo». El exit 5
  significa «daemon inalcanzable» y hace pensar en un daemon que existe pero no
  responde.

## Impacto y workaround

Quien tiene otro servicio en 8765 (o un daemon de otra instalación) pierde tiempo
buscando por qué falla, porque el mensaje apunta a un fichero interno. Los scripts
reciben 5 y pueden reintentar en bucle algo que nunca va a funcionar.

Workaround: liberar el puerto o arrancar en otro con `AVI_DAEMON_PORT=<puerto>` (`0`
para un puerto efímero). El cliente descubre la dirección real por el pidfile.

## Evidencia

- `src/main.rs`, rama de `daemon start`:
  1. borra el fichero ready previo;
  2. llama a `daemon::spawn_background(…)`, que devuelve el PID del hijo;
  3. espera con `await_ready_file_addr(&ready_path, DAEMON_READY_DEADLINE)`
     (`DAEMON_READY_DEADLINE` = 10 s);
  4. mapea cualquier error de esa espera a `ExitCode::DaemonUnreachable`,
     `daemon_unreachable`.
- `await_ready_file_addr` solo sondea el fichero. No comprueba si el proceso hijo sigue
  vivo, aunque el PID se conoce (se guarda en `IN_MEMORY_PID`).
- `crates/avi-daemon/src/spawn.rs`, `spawn_background`: el hijo se lanza con stdin,
  stdout y stderr en `Stdio::null()`, así que el error del bind nunca llega al padre.
- `crates/avi-daemon/src/lib.rs`: el hijo hace `TcpListener::bind(addr).await?` antes de
  escribir el fichero ready. Con el puerto ocupado, el `?` propaga `AddrInUse` y el
  proceso termina sin escribirlo.

## Análisis de causa

- **Confirmada por lectura del código.** El protocolo de arranque solo tiene un canal
  de éxito (el fichero ready con la dirección). No existe un canal de fallo, así que el
  padre no distingue «el hijo tarda» de «el hijo murió», y el único error disponible es
  el timeout genérico.
- **Confirmada.** El stderr del hijo va a `Stdio::null()` y el daemon no tiene log
  propio, así que el `AddrInUse` no queda registrado en ningún sitio.
- Descartado: no deja estado inconsistente. No quedan huérfanos ni pidfile tras el
  fallo, así que el defecto es solo de diagnóstico y de código de salida.

## Diagnóstico sugerido

Estas opciones se complementan:

- **Comprobación previa en el padre.** Antes de lanzar el hijo, intentar un bind de
  prueba a la dirección resuelta (salvo con el puerto `0`). Si falla con `AddrInUse`,
  salir de inmediato con exit 6 y un mensaje que nombre el puerto y `AVI_DAEMON_PORT`.
  Es la opción mínima, pero deja una carrera entre la comprobación y el bind del hijo.
- **Canal de fallo en el protocolo ready.** Que el hijo escriba en el fichero ready un
  error estructurado (por ejemplo `error:addr_in_use:<addr>`) cuando el bind falla, y
  que `await_ready_file_addr` lo reconozca y termine en el acto con el código adecuado.
  Cierra la carrera.
- **Detección de hijo muerto.** En el bucle de `await_ready_file_addr`, comprobar si el
  PID sigue vivo y, si no, cortar la espera con un mensaje de «el daemon terminó
  durante el arranque».
- Opcional: redirigir el stderr del hijo a un log del daemon en `data/logs/`, lo que
  también ayudaría a diagnosticar otros fallos del daemon
  ([observaciones-menores.md](observaciones-menores.md), observación 9).

## Criterio de aceptación

- Con el puerto ocupado, `daemon start` sale en menos de 2 s con exit 6 y un mensaje que
  contiene el puerto y menciona `AVI_DAEMON_PORT`.
- La salida `--json` lleva un `reason` específico del conflicto de puerto, documentado
  en la tabla de `reason` del contrato.
- No quedan procesos ni pidfile.
- Una prueba de integración que ocupe un puerto, arranque el daemon con
  `AVI_DAEMON_PORT` apuntando a él y verifique el código, el `reason` y el tiempo.

## Relacionados

- Contrato de la CLI, tabla de códigos de salida (5 frente a 6).
- [observaciones-menores.md](observaciones-menores.md) (observación 9): la falta de log
  propio del daemon también dificulta diagnosticar este caso.
