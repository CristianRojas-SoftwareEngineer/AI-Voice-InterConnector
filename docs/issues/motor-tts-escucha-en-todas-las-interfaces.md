# El motor TTS residente escucha en todas las interfaces (`0.0.0.0:8766`) sin autenticación

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | alta |
| Tipo | seguridad |
| Componente | motor vendorizado `vendor/qwen3-tts` (`qwen_tts_server.c`) y su lanzador en `avi-tts` |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11. La causa está en código C común a todas las plataformas |
| Reproducibilidad | siempre, mientras el residente está vivo |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Cuando el daemon arranca el motor Qwen3-TTS como proceso residente (`qwen_tts --serve
8766`), el motor abre su servidor HTTP en `0.0.0.0` y no en `127.0.0.1`. Cualquier
equipo de la red local puede usar sus endpoints sin autenticarse. El daemon sí escucha
solo en loopback, así que el motor es la única superficie expuesta del producto.

## Entorno

- Binario: v0.25.0 instalado, modelos provisionados.
- SO: Windows 11, con el firewall de Windows en su configuración por defecto.
- Daemon arrancado con `daemon start`, puerto por defecto.

## Precondiciones

Daemon en marcha con el residente lanzado: basta `daemon start` y esperar a que el
estado pase a `warm`.

## Pasos para reproducir

```powershell
ai-voice-interconnector daemon start
Get-NetTCPConnection -State Listen -LocalPort 8765,8766 | Select LocalAddress,LocalPort,OwningProcess
```

Desde otro equipo de la misma red:

```sh
curl http://<ip-del-equipo>:8766/v1/health
```

## Resultado observado

- `8765` (daemon): `LocalAddress 127.0.0.1`.
- `8766` (residente `qwen_tts`): `LocalAddress 0.0.0.0`.
- Al lanzar el motor, `avi-tts` escribe en stderr: «el motor Qwen3-TTS enlaza en todas
  las interfaces (INADDR_ANY), puerto 8766. El servidor es accesible desde la red
  local».
- Tras matar el daemon a la fuerza, el residente queda huérfano **y sigue escuchando**
  en `0.0.0.0:8766` hasta el siguiente `daemon start` o `daemon stop`.

## Resultado esperado

Un servicio auxiliar local, sin autenticación, que solo consumen procesos del mismo
equipo, escucha únicamente en loopback, igual que el daemon. La comunicación con el
motor ya es local: los healthchecks y las peticiones del cliente usan `127.0.0.1`.

## Impacto y workaround

Si el firewall deja pasar el puerto (una regla creada al aceptar el diálogo de Windows,
una red marcada como privada, o Linux/macOS sin firewall), un tercero de la red puede:

- consumir CPU/RAM del equipo sintetizando sin límite (`/v1/tts`, `/v1/tts/stream`,
  `/v1/audio/speech`);
- enumerar las voces del usuario (`/v1/speakers`), que pueden incluir voces clonadas de
  personas reales;
- sintetizar audio con esas voces clonadas.

No hay autenticación que lo impida.

Workaround: bloquear el puerto 8766 en el firewall para conexiones entrantes, o no usar
el daemon (`--no-daemon`). Con `--no-daemon`, el motor también se lanza en modo
servidor durante la operación, así que la exposición es más corta pero existe
(**por verificar**).

## Evidencia

- `vendor/qwen3-tts/qwen_tts_server.c`, función `setup_listen_socket`: la dirección de
  bind es `.sin_addr.s_addr = INADDR_ANY`, sin opción para cambiarla.
- `crates/avi-tts/src/lib.rs`, en el lanzamiento del servidor (`spawn`): se invoca el
  motor con `--serve <port> --int4 -j 4 --stream` y, justo antes, se imprime el aviso de
  INADDR_ANY. El exponerse a la red se aceptó como riesgo conocido en lugar de
  corregirlo.
- El comentario de ese aviso dice «Riesgo R2 documentado», y la documentación de
  `warm_voice_engine` (`crates/avi-daemon/src/lib.rs`) repite «Riesgo heredado (R2) […]
  Documentado, NO corregido (fuera de alcance)», añadiendo que el warmup mantiene vivo
  el residente y alarga esa exposición. En la documentación actual, R2 es otra regla (el
  borrado del directorio de programa). Las referencias quedaron colgadas y ningún
  documento vigente registra este riesgo.
- El motor es un snapshot vendorizado dentro del repositorio (no es un submódulo), así
  que su código se puede modificar.

## Análisis de causa

- **Confirmada.** El servidor HTTP del motor, heredado del proyecto original, se escribió
  para usarse como servicio de red, y el snapshot vendorizado conservó el bind a
  `INADDR_ANY`. Al integrarlo como residente local, el riesgo se documentó con un aviso
  en lugar de cerrarse.
- Descartado: el daemon no está afectado; su listener usa `127.0.0.1` (o la dirección
  derivada de `AVI_DAEMON_PORT`, también en loopback).

## Diagnóstico sugerido

- Cambiar el bind de `setup_listen_socket` a `INADDR_LOOPBACK`, o añadir al motor un
  flag `--bind <addr>` con loopback por defecto y que `avi-tts` pase `127.0.0.1`
  explícitamente. La primera opción es la mínima. La segunda deja explícita la intención
  en el lanzador.
- Retirar el aviso de stderr y los dos comentarios con la referencia colgada.
- Registrar el cambio en el snapshot vendorizado donde el repositorio anote sus
  divergencias respecto al original, para que no se pierda al actualizar el motor.
- Comprobar si con `--no-daemon` el motor se lanza también en modo servidor y durante
  cuánto tiempo, para acotar el workaround.

## Criterio de aceptación

- Con el daemon en marcha, `Get-NetTCPConnection -LocalPort 8766` (o `ss -ltnp` /
  `lsof -iTCP:8766`) muestra `127.0.0.1` y nunca `0.0.0.0`.
- Una conexión a `<ip-LAN>:8766` desde otro equipo es rechazada.
- La síntesis por daemon sigue funcionando.
- Una prueba de integración que lance el residente y compruebe la dirección local del
  socket en escucha.

## Relacionados

- [daemon-start-puerto-ocupado.md](daemon-start-puerto-ocupado.md): otro defecto del
  arranque del daemon y su residente.
