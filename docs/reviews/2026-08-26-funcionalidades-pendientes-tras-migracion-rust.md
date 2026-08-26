# Revisión: funcionalidades del stack original aún no cubiertas por la versión Rust (v0.11.0)

**Fecha:** 2026-08-26
**Estado:** Abierto. Los tres puntos son funcionalidades presentes en la implementación
Python original (Chatterbox + daemon) cuyo equivalente en la reescritura Rust está cableado
parcialmente o devuelve explícitamente "no implementado". Ninguno bloquea la síntesis, la
transcripción ni la traducción, que están completas.
**Alcance:** Comportamiento de tiempo de ejecución del binario `ai-voice-interconnector` y del
daemon nativo; no cubre limpiezas cosméticas ni drift de documentación.
**Autor:** Sesión interactiva de investigación post-migración.

---

## Resumen ejecutivo

La reescritura a Rust/Qwen3 alcanza paridad funcional en el núcleo (síntesis, transcripción
sobre Parakeet TDT, traducción Marian, contratos `--json` y exit codes). Quedan tres
capacidades del canal Python que no están disponibles de extremo a extremo:

1. **El clonado de voz está implementado en código pero no es usable sin intervención manual**,
   porque el modelo que exige no se aprovisiona.
2. **El arranque del daemon en segundo plano devuelve "no implementado"**; solo existe el modo
   en primer plano (`daemon serve`).
3. **El reinicio del daemon apaga pero no rearma**: no hay un mecanismo de relanzado
   automático.

## Contexto

- Motor TTS: Qwen3-TTS vendorizado en `vendor/qwen3-tts/`, envuelto por `crates/avi-tts`.
- Daemon nativo: `crates/avi-daemon` (router `axum`, puerto 8765).
- Catálogo de modelos aprovisionados por `setup`: `MODEL_REVISIONS` en
  `crates/avi-store/src/lib.rs:381`.

---

## Hallazgo 1 — El clonado de voz exige un modelo que `setup` no aprovisiona

### Síntoma

El comando `voice clone` y el endpoint `POST /voices/precompute` existen y el código de clonado
está completo, pero al ejecutarlos sobre una instalación aprovisionada de forma estándar el
motor falla porque el modelo requerido no está en disco. El test de éxito del clonado se salta
por esta misma razón.

### Evidencia

- Ruta de clonado implementada: `avi-tts::clone_voice()` en
  `crates/avi-tts/src/lib.rs:680`, invocada desde `synthesize_with_options`
  (`crates/avi-tts/src/lib.rs:405-407`).
- El clonado resuelve un **modelo distinto** del usado por la síntesis general:
  `resolve_base_model_dir` (`crates/avi-tts/src/lib.rs:206-212`) documenta que
  `--ref-audio` exige el modelo Base (`vendor/qwen3-tts/main.c:1848`), separado del CustomVoice
  que usa la síntesis normal.
- Cuando ese modelo no está resuelto, el flujo aborta con el error
  `"El modelo Base de clonado Qwen3-TTS no está provisionado."` (`crates/avi-tts/src/lib.rs:405`).
- El catálogo `MODEL_REVISIONS` (`crates/avi-store/src/lib.rs:381`) pinnea `qwen3-tts-0.6b`
  (CustomVoice), `parakeet-tdt-v3` y los dos modelos Marian, pero **no** incluye el modelo Base
  del clonado; por tanto `setup` nunca lo descarga.
- El test `voice_clone_exito` se salta cuando el clonado no está aprovisionado
  (`tests/cli_golden.rs:668-677`), con la nota: *"el clonado exige el modelo Base del motor
  Qwen3-TTS (el vendored es CustomVoice); pendiente de F6/F7"*.

### Diferencia respecto al original

La implementación Python de Chatterbox ofrecía clonado/precómputo de voz funcional de fábrica.
En la versión Rust el código está presente y probado a nivel de argumentos y de cuerpo HTTP,
pero la cadena completa (aprovisionar → clonar → sintetizar con la voz clonada) no se puede
recorrer sin obtener manualmente el modelo Base.

### Qué falta para cerrarlo

Añadir el modelo Base del clonado al catálogo `MODEL_REVISIONS`, extender `setup` para
descargarlo (o hacerlo opt-in explícito) y reactivar `voice_clone_exito`.

---

## Hallazgo 2 — El arranque del daemon en segundo plano no está implementado

### Síntoma

`daemon start` no lanza el servicio: imprime un aviso y devuelve un error de "no implementado".
El único modo operativo es `daemon serve`, que ejecuta el daemon en primer plano (bloquea la
terminal).

### Evidencia

- `DaemonCommands::Start` (`src/main.rs:1142-1149`) emite
  `"Daemon: inicio en segundo plano no implementado aún (use 'daemon serve')."` y retorna
  `ExitCode::NotApplicable` con razón `not_implemented`.
- El modo funcional es `daemon serve`, que llama a `daemon::run_daemon_server(addr)` de forma
  bloqueante (`src/main.rs:1138`).

### Diferencia respecto al original

El daemon del canal Python podía dejarse corriendo como proceso de fondo. En la versión Rust
mantener el daemon vivo requiere que el usuario gestione el proceso en primer plano por su
cuenta (o lo lance de fondo con herramientas del sistema).

### Qué falta para cerrarlo

Implementar el lanzamiento desacoplado del proceso (spawn en segundo plano con registro del PID
y desconexión de la terminal).

---

## Hallazgo 3 — El reinicio del daemon apaga pero no vuelve a arrancar

### Síntoma

`daemon restart` envía la señal de apagado al daemon en ejecución, pero no lo relanza: informa
explícitamente que el rearme queda pendiente.

### Evidencia

- `DaemonCommands::Restart` (`src/main.rs:1178-1206`) hace `POST /shutdown` y, tras confirmarlo,
  responde `"Daemon reiniciado (rearmado en background: pendiente)."` (`src/main.rs:1203`) o el
  JSON `{ "status": "restart_requested", "daemon": "stopped" }` (`src/main.rs:1201`).
- El comentario del handler lo explica: *"El daemon nativo no expone `/restart`; se emite Stop
  (shutdown) y se reporta el rearme como pendiente"* (`src/main.rs:1179-1180`).
- El router del daemon solo registra `/health`, `/voices`, `/voices/precompute`, `/synthesize`,
  `/shutdown` y (con `native-stt`) `/transcribe` — no hay ruta de reinicio
  (`crates/avi-daemon/src/lib.rs:461-470`).

### Diferencia respecto al original

Un `restart` completo (apagar y volver a levantar) no está disponible; el comando queda como un
apagado con aviso. Depende directamente del Hallazgo 2: sin arranque en segundo plano, no hay
forma de rearmar tras el apagado.

### Qué falta para cerrarlo

Resolver primero el arranque en segundo plano (Hallazgo 2) y encadenar apagado → relanzado en el
handler de `restart`.

---

## Nota de alcance

Estas tres son diferencias funcionales frente al original. No entran aquí las capacidades que la
versión Rust difiere por diseño y documenta como tales (cobertura de arquitecturas asimétrica,
el Cask de Homebrew que no aprovisiona el modelo, los motores nativos tras features opt-in de
compilación), que no constituyen carencias sino decisiones de diseño.
