# Comandos que terminan bien dejan residuos en disco

| Campo | Valor |
|---|---|
| Estado | abierto |
| Severidad | media por el síntoma 3; baja en los demás |
| Tipo | funcional |
| Componente | CLI (`src/main.rs`), motor TTS (`crates/avi-tts`), daemon (`crates/avi-daemon`) y ciclo de vida (`self update`) |
| Versión detectada | 0.25.0 |
| Plataforma | Windows 11 con PowerShell 5.1 |
| Reproducibilidad | siempre |
| Detectado en | prueba E2E de v0.25.0, 2026-09-28 |

## Resumen

Varios comandos que terminan con el resultado correcto dejan en disco ficheros que ya no
sirven: WAV temporales, el fichero de listo del daemon, logs del motor sin límite y
artefactos de la actualización. Todo queda a cargo de `cleanup`, cuando el criterio
debería ser que quien crea un fichero lo borre. Además, el daemon no escribe log propio,
así que sus fallos no quedan registrados en ningún sitio.

## Entorno

Binario v0.25.0 instalado desde cero en Windows 11 con PowerShell 5.1, con todos los
modelos provisionados y el daemon en el puerto por defecto.

## Precondiciones

Las propias de cada síntoma.

## Pasos para reproducir

Se detallan en cada síntoma, dentro de «Análisis de causa».

## Resultado observado

Se detalla en cada síntoma.

## Resultado esperado

Cada comando retira lo que crea al terminar, también si falla. Los logs tienen una
retención acotada y el daemon escribe el suyo.

## Impacto y workaround

Los residuos ocupan disco y ensucian `doctor`. Sin log del daemon no queda rastro de sus
fallos, entre ellos el que deja huérfano al motor residente. Workaround: `cleanup` barre
los temporales.

## Evidencia

Al final de la sesión E2E había 21 logs del motor en `data/logs/`.

## Análisis de causa

### 1. `speech say` y `speech dub` dejan WAV en `%TEMP%` o informan de uno ya borrado

- **Síntoma:** los dos comandos sintetizan a un WAV temporal para reproducirlo y
  devuelven su ruta en `audio_path` (en modo humano, en «Reproduciendo: …» y «Doblaje
  reproducido: …»). Según la vía:
  - `speech say` por la vía directa deja `avi_say_<pid>.wav` en el directorio temporal;
  - `speech say` por la vía daemon lo borra, pero después de emitir su ruta, así que
    `audio_path` apunta a un fichero que ya no existe; si la reproducción falla, sale
    antes de borrarlo y el WAV queda en disco;
  - `speech dub` deja `avi_dub_<pid>.wav` en todas sus vías: directa, daemon y la
    composición para daemons sin `/dub`.

  Los que quedan solo desaparecen con `cleanup`.
- **Causa (confirmada):** `src/main.rs` escribe los temporales en `std::env::temp_dir()`
  y cada vía gestiona su vida por su cuenta: solo `say_via_daemon` borra el fichero, y lo
  hace al final, sin cubrir el fallo de la reproducción ni retirar la ruta del JSON.
- **Esperado:** quien crea el temporal lo borra al terminar la reproducción, también si
  falla, en todas las vías, y el JSON no informa de una ruta que no va a existir.
- **Criterio:** tras `speech say` y `speech dub`, por todas sus vías y también cuando
  falla la reproducción, `%TEMP%` no contiene `avi_say_*.wav` ni `avi_dub_*.wav`, y el
  JSON no contiene una ruta a un fichero inexistente.

### 2. `daemon.ready` queda en `data/` tras `daemon stop`

- **Síntoma:** después de un `daemon stop` limpio, `data/daemon.ready` sigue en disco.
- **Causa (probable):** el fichero solo se borra al empezar el siguiente `daemon start`,
  que lo invalida antes de lanzar el hijo. El apagado no lo retira.
- **Esperado:** `daemon stop` borra el fichero ready junto con el pidfile. Hay que
  respetar un uso existente: cuando se pierde el pidfile tras una caída del padre,
  `classify_residual` (`src/main.rs`) lee el PID del árbol desde `daemon.ready` para
  reclamar al residente. Borrarlo en un apagado limpio no afecta a esa recuperación.
- **Criterio:** tras `daemon stop`, `data/` no contiene `daemon.ready` ni `daemon.pid`.

### 3. Logs del motor sin rotación y sin log propio del daemon

- **Severidad:** media. Sin log del daemon no queda rastro de sus fallos, entre ellos el
  que deja huérfano al motor residente.
- **Síntoma:** cada arranque del motor crea `data/logs/qwen3-tts_<pid>_<ms>.log`; al
  final de la sesión había 21. El daemon no escribe log propio, así que sus errores
  (por ejemplo, una caída del servidor después de estar listo) no quedan en ningún sitio.
- **Causa (confirmada):** `crates/avi-tts/src/lib.rs` nombra un archivo nuevo por
  proceso y nada los poda. El hijo del daemon se lanza con stdout y stderr descartados
  (`spawn_background`, `crates/avi-daemon/src/spawn.rs`).
- **Esperado:** retención acotada de los logs del motor (por número o por antigüedad) y
  un log del daemon en `data/logs/`.
- **Criterio:** tras N arranques quedan como mucho los K logs más recientes; existe un
  log del daemon con el arranque, el bind y los errores.

### 4. `self update` deja el binario aparcado y el staging con el `.zip`

- **Síntoma:** tras `self update --force` quedan el binario anterior aparcado (`.old-*`)
  en el directorio del programa y el `.zip` descargado. `doctor` informa
  `pending_artifacts` con un aparcado hasta que el barrido lo recoge, un rato después.
- **Causa (parcial):**
  - El binario anterior no se puede borrar en caliente en Windows y se aparca por
    diseño. Solo lo recoge el barrido que ejecutan las operaciones de ciclo de vida
    (`self update`, instalación, desinstalación y `cleanup`); mientras tanto, `doctor`
    lo cuenta como fallo.
  - El `.zip` no se descarga en el directorio del programa, sino en un directorio de
    staging hermano (`.ai-voice-interconnector-staging-update-<versión>`, junto al
    directorio del programa). Al terminar, `cleanup_staging`
    (`crates/avi-lifecycle/src/update.rs`) intenta borrarlo; si está en uso, en Windows
    programa su borrado con un proceso auxiliar desacoplado que espera a que termine el
    proceso en curso, y si tampoco puede, lo deja para el barrido. Queda por ver cuál de
    esos pasos falló para que el staging sobreviviera.
- **Esperado:** el staging desaparece al terminar la actualización, y el aparcado se
  recoge sin esperar a otra operación de ciclo de vida, sin que `doctor` lo marque como
  fallo mientras tanto.
- **Criterio:** tras `self update` y otra invocación cualquiera, junto al directorio del
  programa no queda ningún staging, el directorio del programa solo contiene lo
  instalado y `doctor` pasa.

## Diagnóstico sugerido

Los síntomas 1 y 2 tienen corrección local y caben en un mismo parche. El 3 es el
prioritario porque el log del daemon es requisito para diagnosticar el residente
huérfano. El 4 necesita una reproducción instrumentada de `self update` que registre el
resultado de la limpieza del staging.

## Criterio de aceptación

Se cumplen los criterios de los cuatro síntomas.

## Relacionados

- [motor-residente-huerfano-y-trazas-fuera-del-log.md](motor-residente-huerfano-y-trazas-fuera-del-log.md):
  el diagnóstico del residente huérfano depende del log del daemon del síntoma 3.
- Documentación de `cleanup` y de `doctor` (`pending_artifacts`).
