# Observaciones menores: mensajes, residuos y ruido de salida

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | baja |
| Tipo | diagnóstico (mensajes), funcional (residuos) y documentación |
| Componente | varios; se indica en cada observación |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 |
| Reproducibilidad | siempre, salvo que se indique otra cosa |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Doce defectos pequeños en los que el comando termina con el resultado correcto, pero el
mensaje engaña, queda basura en disco o la documentación no cuadra con la realidad. Se
agrupan aquí porque ninguno justifica un documento propio. Cada observación tiene una
ficha breve (síntoma, reproducción, esperado, causa y criterio). Si alguna crece al
diagnosticarla, se separa a su propio documento con la plantilla completa.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados (incluido el de clonado). Daemon en el puerto por defecto salvo
que se indique.

## Precondiciones

Las propias de cada observación.

## Pasos para reproducir

Se detallan en cada observación, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada observación.

## Resultado esperado

Se detalla en cada observación.

## Impacto y workaround

Ninguna observación bloquea un caso de uso. Los mensajes engañosos (1 a 5) hacen perder
tiempo de diagnóstico y confunden a los scripts que leen el `status`. Los residuos (7 a
10) ocupan disco y ensucian `doctor`.
Workaround general: `cleanup` barre los temporales y `daemon stop` o `daemon start`
reclaman el residente huérfano.

## Evidencia

Recogida en cada observación.

## Análisis de causa

### Mensajes engañosos o mal formados

#### 1. Prefijo `Error:` duplicado

- **Síntoma:** `speech say --text hola --temperature 5` imprime `Error: Error:
  --temperature debe ser mayor que 0 y como máximo 2.0.` (exit 2, correcto).
- **Causa (confirmada):** `validate_temperature` (`src/main.rs`) construye el `CliError`
  con un mensaje que ya empieza por `Error:`. El manejador de errores de `main` añade
  su propio `Error: {}`. Lo mismo ocurre con el mensaje equivalente del daemon
  (`crates/avi-daemon/src/lib.rs`), que viaja en el JSON.
- **Esperado:** un único prefijo. El mensaje del error no debe llevar prefijo; lo pone
  quien lo imprime.
- **Criterio:** ningún mensaje de `CliError` ni del daemon empieza por `Error:`; una
  prueba golden del caso de `--temperature`.

#### 2. `daemon stop` sin daemon dice «Señal de apagado enviada»

- **Síntoma:** con el daemon detenido, `daemon stop` imprime «Señal de apagado enviada
  al daemon en 127.0.0.1:8765.» y el JSON devuelve `status: "shutdown_sent"`,
  `daemon: "stopped"`.
- **Causa (confirmada):** en la rama de `daemon stop` de `src/main.rs`, el caso
  «ni activo ni vivo» borra el pidfile y reutiliza el mensaje del apagado real. El exit
  0 es correcto porque la operación es idempotente.
- **Esperado:** un mensaje del tipo «El daemon no estaba en ejecución» y un `status`
  propio (por ejemplo `not_running`). Si cambia el `status`, hay que actualizar el
  contrato.
- **Criterio:** texto y `status` distintos para «no había daemon» y «se apagó».

#### 3. `--daemon` en comandos solo locales dice «Daemon inalcanzable»

- **Síntoma:** con el daemon **activo**, `voice list --daemon`, `speech list --daemon` y
  `speech play --daemon` salen con exit 5 y el mensaje «Daemon inalcanzable en
  127.0.0.1:8765».
- **Causa (confirmada):** `require_local` (`src/main.rs`) rechaza `--daemon` en los
  comandos que no se delegan, pero reutiliza el código, el `reason` y el texto del
  daemon inalcanzable.
- **Esperado:** rechazarlo es lo previsto. El mensaje debe decir que ese comando se
  ejecuta siempre en local y no admite `--daemon`, y el código más adecuado sería el de
  uso inválido (2). Hay que decidirlo contra el contrato.
- **Criterio:** con el daemon activo o detenido, el mensaje no afirma que el daemon sea
  inalcanzable.

#### 4. El plan de `self install` anuncia «se añadirá» un PATH que ya existe

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

#### 5. `self uninstall --dry-run --json` devuelve `status: "uninstalled"`

- **Síntoma:** el simulacro no modifica nada (correcto), pero el sobre dice
  `status: "uninstalled"` junto a `dry_run: true`.
- **Causa (confirmada por la salida):** el `status` describe el resultado que tendría
  la operación, no lo que ha pasado. Un consumidor que mire solo `status` cree que
  desinstaló.
- **Esperado:** un `status` que no afirme el hecho, por ejemplo `planned` o
  `would_uninstall`. Cambiarlo toca el contrato JSON (`schema_version`).
- **Criterio:** decisión registrada en el contrato; prueba golden del dry-run.

### Salida ruidosa

#### 6. Logs internos en la salida humana

- **Síntoma:** en modo humano, la descarga de modelos (backend xet de Hugging Face) y
  `voice clone` vuelcan en la terminal las trazas de progreso y los logs del motor, que
  tapan el resumen del comando.
- **Causa (por verificar):** la biblioteca de descarga escribe su propio progreso y el
  motor hereda o reenvía su stderr durante el clonado.
- **Esperado:** en modo humano, una sola línea de progreso del producto; las trazas
  internas van al log.
- **Criterio:** `voice clone` y `setup` en modo humano muestran solo mensajes propios.

### Residuos en disco

#### 7. `speech say` deja WAV en `%TEMP%`

- **Síntoma:** cada `speech say` deja un `avi_say_<pid>.wav` en el directorio temporal.
  Solo desaparecen con `cleanup`.
- **Causa (confirmada):** `src/main.rs` escribe `avi_say_{pid}.wav` en
  `std::env::temp_dir()` en las vías directa y daemon, y no lo borra tras reproducirlo.
- **Esperado:** el temporal se borra al terminar la reproducción, también si falla.
- **Criterio:** tras `speech say`, `%TEMP%` no contiene `avi_say_*.wav`.

#### 8. `daemon.ready` queda en `data/` tras `daemon stop`

- **Síntoma:** después de un `daemon stop` limpio, `data/daemon.ready` sigue en disco.
- **Causa (probable):** el fichero solo se borra al empezar el siguiente `daemon start`,
  que lo invalida antes de lanzar el hijo. El apagado no lo retira.
- **Esperado:** `daemon stop` borra el fichero ready junto con el pidfile.
- **Criterio:** tras `daemon stop`, `data/` no contiene `daemon.ready` ni `daemon.pid`.

#### 9. Logs del motor sin rotación y sin log propio del daemon

- **Síntoma:** cada arranque del motor crea `data/logs/qwen3-tts_<pid>_<ms>.log`; al
  final de la sesión había 21. El daemon no escribe log propio, así que sus errores
  (por ejemplo, un bind fallido) no quedan en ningún sitio.
- **Causa (confirmada):** `crates/avi-tts/src/lib.rs` nombra un archivo nuevo por
  proceso y nada los poda. El hijo del daemon se lanza con stdout y stderr descartados
  (`spawn_background`, `crates/avi-daemon/src/spawn.rs`).
- **Esperado:** retención acotada de los logs del motor (por número o por antigüedad) y
  un log del daemon en `data/logs/`.
- **Criterio:** tras N arranques quedan como mucho los K logs más recientes; existe un
  log del daemon con el arranque, el bind y los errores.

#### 10. `self update` deja `.old-*` y el `.zip` en el directorio del programa

- **Síntoma:** tras `self update --force`, el directorio del programa contiene el
  binario anterior aparcado (`.old-*`) y el `.zip` descargado. `doctor` informa
  `pending_artifacts` con un aparcado hasta que el barrido lo recoge, un rato después.
- **Causa (por verificar):** el binario anterior no se puede borrar en caliente en
  Windows y se aparca por diseño; queda por ver por qué el `.zip` no se borra tras
  extraerlo y por qué el barrido tarda.
- **Esperado:** el `.zip` se borra al extraerlo, y el aparcado se recoge en la siguiente
  invocación, sin que `doctor` lo marque como fallo mientras tanto.
- **Criterio:** tras `self update` y otra invocación cualquiera, el directorio del
  programa solo contiene lo instalado y `doctor` pasa.

### Procesos

#### 11. Un kill duro del daemon deja huérfano al motor residente

- **Síntoma:** con `Stop-Process -Force` sobre el daemon (con o sin `--auto-restart`),
  el `qwen_tts` residente sigue vivo escuchando en `127.0.0.1:8766`, el pidfile queda
  obsoleto y `daemon status` dice `stopped`. El siguiente `daemon start` o `daemon stop`
  lo detecta y lo reclama.
- **Causa (confirmada por el comportamiento):** el residente es un proceso
  independiente y no muere con su padre. `--auto-restart` supervisa dentro del proceso,
  así que no puede actuar si el proceso muere.
- **Esperado:** el residente muere con el daemon. En Windows se puede asociar a un Job
  Object con `KILL_ON_JOB_CLOSE`; en Unix, con `PR_SET_PDEATHSIG` (Linux) o que el
  residente vigile a su padre.
- **Criterio:** tras matar el daemon a la fuerza, en menos de unos segundos no queda
  ningún `qwen_tts` ni nada escuchando en 8766.

### Documentación

#### 12. Tamaño de los modelos subestimado

- **Síntoma:** README, `docs/CLI/commands/SETUP.md`, `docs/GOAL.md`,
  `docs/MANUAL-VALIDATION.md` y `docs/CLI/commands/CLEANUP.md` hablan de ~9 GB para la
  selección base y ~11,5 GB con `--with-voice-cloning`. Con el clonado provisionado, la
  raíz de modelos ocupaba 14 GB en disco.
- **Causa (por verificar):** las cifras probablemente no cuentan los derivados CT2 ni la
  caché del backend xet de descarga. Hay que medir cada componente.
- **Esperado:** cifras que coincidan con lo que se ocupa en disco tras `setup`, con un
  margen explícito.
- **Criterio:** los documentos citados dan la misma cifra, medida sobre una instalación
  limpia.

## Diagnóstico sugerido

Resolver primero las observaciones de causa confirmada y corrección local (1, 2, 3, 7 y
8), que caben en un mismo parche. Las que tocan el contrato JSON (2 y 5) necesitan una
decisión sobre el `status` antes de implementarlas. Las que están por verificar (4, 6,
10 y 12) necesitan una reproducción instrumentada.

## Criterio de aceptación

Cada observación tiene su criterio en su ficha. El documento se da por resuelto cuando
todas están cerradas o separadas a su propio documento.

## Relacionados

- [daemon-start-puerto-ocupado.md](daemon-start-puerto-ocupado.md) y
  [dub-daemon-timeout-intermitente.md](dub-daemon-timeout-intermitente.md)
  (observación 9: falta de log del daemon).
