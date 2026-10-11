# Deuda técnica vigente tras C10

## 1. Introducción y contexto

El ciclo C10 ("cierre integral de deuda técnica pre-release", rama
`fix/deuda-pre-release`) dejó deuda verificada que un paquete posterior cerró
con cambios estructurales: plazo y ruta fija en las primitivas de procesos,
una sola regla de dirección para escucha y parada, observación única en el
veredicto de parada, reserva del nombre del log, recuperación del lock de
Parakeet y descartes de error con traza más una lint que los prohíbe. Este
documento recoge solo lo que **sigue vigente** después de ese paquete: lo que
ningún cambio de código cierra.

**Método.** Cada hallazgo cita evidencia `ruta:línea` y lleva marca
**VERIFICADO** (hecho de código) o **HIPÓTESIS** (requiere ejecución para
confirmar).

## Tabla de contenidos

- [1. Introducción y contexto](#1-introducción-y-contexto)
- [Tabla de contenidos](#tabla-de-contenidos)
- [2. Hallazgos vigentes](#2-hallazgos-vigentes)
  - [H4 — Código solo-Unix sin ejecutar](#h4--código-solo-unix-sin-ejecutar)
- [3. Hallazgos cubiertos sin cambio de código](#3-hallazgos-cubiertos-sin-cambio-de-código)
  - [H14 — Pata full solo en máquina provisionada](#h14--pata-full-solo-en-máquina-provisionada)
- [4. Hipótesis refutadas](#4-hipótesis-refutadas)
  - [Sleeps restantes en cli_golden](#sleeps-restantes-en-cli_golden)
  - [Allows y matices de código muerto](#allows-y-matices-de-código-muerto)

## 2. Hallazgos vigentes

### H4 — Código solo-Unix sin ejecutar

`pre_exec(setsid)` (`crates/avi-process/src/restricted_spawn.rs:328-339`), las
primitivas `cfg(unix)` de `crates/avi-process/src/lib.rs` —`process_identity`
con `/bin/ps` por ruta fija y plazo (`:91`), el parser de ancho fijo de 24
caracteres sobre `lstart` (`:122`), `pid_alive_exact` (`:172`), `fits_pid_t`
(`:188`) y `kill_tree_by_pid_exact` (`:288`)—, el enlace simbólico atómico de
`crates/avi-lifecycle/src/path_unix.rs`, el `xattr` por ruta fija de
`crates/avi-lifecycle/src/quarantine.rs:184` y la prueba `cfg(unix)` de
`crates/avi-process/tests/restricted_spawn.rs:207`. Los de `avi-process` están
verificados por clippy cruzado a `aarch64-apple-darwin`; los de
`avi-lifecycle` ni eso desde Windows, porque `ring` exige un compilador C del
target. Ningún cambio de código lo cierra; solo el pase manual en
Linux x64 y macOS arm64, abierto (recorrido en `docs/MANUAL-VALIDATION.md`).
**VERIFICADO.**

## 3. Hallazgos cubiertos sin cambio de código

### H14 — Pata full solo en máquina provisionada

Los contratos del daemon construyen `DaemonState` y `with_stores` carga
Parakeet bajo `native-stt`, así que la evidencia de "sin intermitencias" de la
pata `full` solo puede venir de una máquina con modelos. Queda cubierto por la
puerta de release: `cargo xtask release` ejecuta
`cargo test --features full -- --include-ignored` con los recursos provisionados
y aborta el corte si falla, de modo que ninguna versión se publica sin esa
pata en verde. **VERIFICADO.**

## 4. Hipótesis refutadas

No son deuda; se registran para no reabrirlas.

### Sleeps restantes en cli_golden

Refutado para ese fichero: los 10 `sleep` verificados son cadencias de sondeo
acotadas, el estímulo deliberado de un `#[should_panic]` y un backoff acotado;
siguen el patrón prescrito.

### Allows y matices de código muerto

Legítimos: `unix_claim_verified` (`src/main.rs:2782`) se llama bajo
`cfg(unix)` en `src/main.rs:2878`; `stdin_holder` se puebla en
`crates/avi-process/src/restricted_spawn.rs:358` y se mueve al struct en
`:409` (retenido solo por `Drop`, lo declarado). Y precisión: `extra_allowed`
no es código muerto sino API viva que solo puebla la prueba de la allowlist.
