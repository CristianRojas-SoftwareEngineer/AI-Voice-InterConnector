# Hallazgos pendientes — revisión consolidada

- **Fecha**: 2026-09-10
- **Estado**: 10 resueltos (H-05 ✅, H-04 ✅, H-03 ✅, H-02 ✅, H-01 ✅, H-13 ✅, H-09 ✅, H-16 ✅, H-06 ✅, H-07 ✅) — 6 pendientes (H-08, H-10–H-12, H-14, H-15)
- **Alcance**: todos los defectos, gaps y deudas de medición pendientes del producto, unificados en un solo índice. Sin historia, sin referencias cruzadas a revisiones previas, sin identificadores heredados.
- **Orden**: IDs secuenciales por severidad (críticos → bajos); dentro de cada sección, primero ciclo de vida, luego superficie CLI, luego medición.

> **Leyenda de severidad**: 🔴 **Crítica** — fuga de recursos o cuelgue en producción · 🟠 **Alta** — feature contratada inexistente, crash o degradación silenciosa · 🟡 **Media** — palanca operativa ausente o gap funcional menor · ⚪ **Baja** — pulido de superficie o medición pendiente.

## Tabla de contenidos

- [1. Índice](#1-índice)
- [2. Críticos](#2-críticos)
  - [H-01 — Huérfanos tras abortos y fallos](#h-01--abortos-y-fallos-dejan-daemon--motor-huérfanos)
  - [H-02 — Warmup sin deadline](#h-02--el-warmup-se-cuelga-sin-deadline-visible)
- [3. Altos](#3-altos)
  - [H-03 — Daemon retiene stdio](#h-03--el-daemon-retiene-el-stdio-del-proceso-que-lo-lanzó)
  - [H-04 — Residente inobservable](#h-04--el-motor-de-voz-no-deja-traza-observable)
  - [H-05 — Degradación por reutilización](#h-05--el-daemon-reutilizado-degrada-sirve-estado-pero-falla-síntesis)
  - [H-06 — Sin control de idioma/STT en daemon](#h-06--daemon-startserve-sin-control-de-idioma-ni-stt)
  - [H-07 — Precarga de clone inexistente](#h-07--voice-clone---daemon-promete-precarga-inexistente)
  - [H-08 — Panic con `--mic` sin `--duration`](#h-08--panic-con---mic-sin---duration-en-terminal)
- [4. Medios](#4-medios)
  - [H-09 — Setup sin reinstalación forzada](#h-09--setup-sin-reinstalación-forzada-ni-confirmación)
  - [H-10 — List sin filtro por voz](#h-10--speech-list-sin-filtro-por-voz)
- [5. Bajos](#5-bajos)
  - [H-11 — Translate sin choices](#h-11--translate---from--to-acepta-cualquier-texto)
  - [H-12 — Play sin flujo interactivo](#h-12--speech-synthesize---play-sin-flujo-interactivo)
  - [H-13 — Rama alternativa del gate](#h-13--rama-alternativa-del-gate-de-traducción-sin-evidencia)
  - [H-14 — Techos de guards sin re-medir](#h-14--techos-de-guards-de-tests-sin-re-medir)
  - [H-15 — Marginalidad temporal de la suite y barrido Unix pendiente](#h-15--marginalidad-temporal-de-la-suite-y-barrido-unix-pendiente)
  - [H-16 — Drift Python en los docs de comando](#h-16--drift-python-en-los-docs-de-comando)
- [6. Grafo de relaciones y orden de ataque](#6-grafo-de-relaciones-y-orden-de-ataque)

## 1. Índice

| ID | Título | Severidad | Área |
|---|---|---|---|
| H-01 | Abortos y fallos dejan daemon + motor huérfanos | 🔴 Crítica | Ciclo de vida |
| H-02 | El warmup se cuelga sin deadline visible | 🔴 Crítica | Warmup |
| H-03 | El daemon retiene el stdio del proceso que lo lanzó | 🟠 Alta | Ciclo de vida |
| H-04 | El motor de voz no deja traza observable (stdio a `null`) | 🟠 Alta | Observabilidad |
| H-05 | El daemon reutilizado degrada: sirve estado pero falla síntesis | 🟠 Alta | Daemon |
| H-06 | `daemon start/serve` sin control de idioma ni STT | 🟠 Alta | CLI/Daemon |
| H-07 | `voice clone --daemon` promete precarga inexistente | 🟠 Alta | CLI/Contrato |
| H-08 | Panic con `--mic` sin `--duration` en terminal | 🟠 Alta | CLI |
| H-09 | `setup` sin reinstalación forzada ni confirmación | 🟡 Media | CLI/Setup |
| H-10 | `speech list` sin filtro por voz | 🟡 Media | CLI |
| H-11 | `translate --from/--to` acepta cualquier texto | ⚪ Baja | CLI |
| H-12 | `speech synthesize --play` sin flujo interactivo | ⚪ Baja | CLI/UX |
| H-13 | Rama alternativa del gate de traducción sin evidencia | ⚪ Baja | Motor |
| H-14 | Techos de guards de tests sin re-medir | ⚪ Baja | Tests |
| H-15 | Marginalidad temporal de la suite y barrido Unix pendiente | ⚪ Baja | Tests/Ciclo de vida |
| H-16 | Drift Python en los docs de comando | ⚪ Baja | Documentación |

## 2. Críticos

### H-01 — Abortos y fallos dejan daemon + motor huérfanos

- **Severidad**: 🔴 Crítica · **Área**: ciclo de vida (`crates/avi-daemon/src/spawn.rs`, `crates/avi-daemon/src/lib.rs:1152-1349`, `crates/avi-tts/src/lib.rs:336`, `src/main.rs:363-424,1422-1591,2119-2332`)
- **Síntoma**: toda vía anormal (aborto externo, timeout, panic del test antes del apagado) dejaba vivos a `ai-voice-interconnector` + `qwen_tts`, ociosos, ocupando puertos y ficheros. Observado en 4 ocasiones en un solo día; un antecedente consumió 8 horas de CPU.
- **Causa**: demostrada por lectura y por eliminación: el apagado solo corría en la vía feliz —el handler Ctrl+C hacía `exit(130)` sin limpieza (`src/main.rs:363`), `Stop` borraba el pidfile aunque el proceso siguiera vivo, `Restart` acumulaba doble techo y kill por PID duplicado sin árbol del residente, `serve` no escuchaba señales, el residente se mataba por imagen global y la fixture se adhería por probe sin revalidar (`already_running` ciego)—. Los procesos sobrevivían a la muerte de su padre.
- **Corrección (implementada)**: cierre estructural en producto + harness. Producto: handler Ctrl+C con limpieza acotada de 2 s y exit 130 preservado (`CTRL_C_LIMPIEZA_DEADLINE`); Job `KILL_ON_JOB_CLOSE` en la rama `Serve` (Windows) más árbol matable por PID con verificación (`matar_arbol_por_pid`: `taskkill /F /T` en Windows, `kill -9` al grupo en Unix); reclamo matar-y-rearrancar al arrancar (`clasificar_residual` por PID vivo + probe: sano → `already_running`, degradado → se reclama el árbol y se rearranca con salida 0 y payload `started`); parada unificada con deadline global de 8 s (`stop_daemon_and_resident` al servicio de reclamo, stop, restart, cleanup y uninstall: graceful 1,5 s + espera 3 s + árbol preciso + verificación, borrado del pidfile solo tras muerte verificada y exit 5 sin borrar pista si el árbol sigue vivo); `Restart` sobre el ayudante único sin doble techo ni kill duplicado; `serve` escucha Ctrl+C/SIGTERM por la misma ruta que `POST /shutdown` y `shutdown_handler` mata el árbol preciso del residente primero, con kill por imagen solo como último recurso documentado con verificación inmediata; residente sin breakaway y `Drop` como cierre por árbol. Harness: `verificar_cero_huerfanos` (árbol muerto + puertos 8765/8766 cerrados + pidfile sin PID vivo, `panic!` con reaper previo si queda resto), reaper ante techo, timeout, `warm_failed` o cualquier `panic!` fuera de guards/polls (`fallo_con_reaper` + `GuardReaper`), `STATE_LOCK` con recuperación ante envenenado (mismo tipo y contrato) e higiene de `TEST_LIMITE` restaurada, `ensure` que revalida y prueba pesada nueva `tts::h01_aborto_simulado_reclama_y_no_deja_huerfanos` (reclamo con `started` + cero huérfanos a nivel SO). Cierre D-01..D-05 (F5): D-01 con techo (compila sin warnings + predicado puro en verde + tensado `#[cfg(unix)]` sin simular; runtime Unix diferido a CI); D-02 (PID en memoria + `serve` por misma ruta, 130 y 2 s intactos, sin rojo que los contradiga); D-03 (`d03_*` 2/2, reaper verificado en fallos reales con muerte verificada, cobertura a 8 tests con `ensure` + barrido del residente en 8766 por puerto preciso con `netstat -ano` y verificación a 8 s —en Unix solo log, pendiente, ver H-15—); D-04 (predicado puro en verde, residente-solo con preciso-primero e imagen del residente solo como último recurso verificado, imagen del daemon prohibida); D-05 en código (reclamo activo con deadline 5 s + verificación, backoff y `Ok` intactos; crash vivo con log pendiente de CI/entorno rápido, ver H-15). Estado: ✅ Implementado (commit a908ac6 + restos D-01..D-05 en commit 193eeac).
- **Impacto resuelto**: invariantes intactos (sin cascada ante rojos —D-03 contiene—; cero huérfanos post-suite a nivel SO; exit 130 preservado). Estado de suite F5 (2026-09-14, esta máquina): `cargo check --all-targets` limpio sin warnings; `cargo test --lib` 54/54; binario 4/4 (incluidos 2 unitarios D-01/D-04); dorada `cli_golden` 37/40 — 3 rojos clase-timeout sin fallo de aserción de lógica (marginalidad temporal pre-existente, ver H-15): `tts::clone_con_daemon_delega` (exit 5, clonado ~1,6 s contra presupuesto CLI fijo de 1500 ms), `tts::h01_aborto_simulado` (guard 180 s esperando `running` bajo carga paralela) y `tts::translate_force_daemon_sin_daemon_exit5` (guard 180 s esperando `stopped` bajo la misma carga); bisect en base `ff9a9f9` (sin cambios) reproduce el rojo semilla idéntico (exit 5, 1589 ms): no hay regresión demostrada del diff.
- **Relaciones**: alimentaba a H-05 (ahora diagnosticable con traza H-04 + reclamo H-01) · comparte zona con H-03 (siguiente del cluster: stdio del lanzamiento) · H-02 reduce su superficie (sin subprocess que re-lanzar) · H-04 lo vuelve detectable a tiempo.
- **Decisión requerida**: resuelta — producto + harness; matar-y-rearrancar ante residual degradado; exit 130 preservado con limpieza acotada; `started` tras reclamo; kill por imagen solo como último recurso documentado (residente).

### H-02 — El warmup se cuelga sin deadline visible

- **Severidad**: 🔴 Crítica · **Área**: warmup (`crates/avi-daemon/src/lib.rs`, warmup en segundo plano tras el bind)
- **Síntoma**: corridas idénticas del mismo comando varían entre warm en ~17s y 150s+ quemando CPU sin llegar al audio, sin mensaje ni fase identificable.
- **Causa**: demostrada por investigación (2026-09-11): el motor C vendido se cuelga intermitentemente (~17% de las corridas observadas) tras cargar el tokenizer, durante la fase de síntesis — un black box no reparable desde este repo; y la cadena de fallos en Rust carecía de deadlines en dos puntos: el `spawn_blocking(warmup_tts)` del daemon y el fallback subprocess `cmd.output()` que reintentaba lanzando el mismo binario colgante sin plazo alguno.
- **Corrección (implementada)**: eliminado íntegramente el fallback subprocess (el residente HTTP queda como único camino de síntesis, con healthcheck y POST ambos acotados a 30 s; también se cierra el bug de mal-tokenización UTF-8 acentuado por argv en Windows) y añadido `WARMUP_DEADLINE` de 40 s (~2× el TTFN feliz medido de ~18-20 s, muy por debajo del hang histórico de 150 s+) sobre el `JoinHandle` del `spawn_blocking`: al expirar, `set_warm_failed` con diagnóstico citando el log del motor y `shutdown()` termina al residente colgado. Estado: ✅ Implementado (commit 30f7cf1).
- **Impacto resuelto**: el cuelgue intermitente del motor es ahora un fallo rápido y observable (`/health` reporta `warm_failed` con causa); smoke del daemon confirma `warming` → `warm` en ~20 s y sin huérfanos.
- **Relaciones**: desbloqueado por H-04 (traza del motor) · reduce la superficie de H-01 (sin subprocess que re-lanzar) · desbloquea H-06 (preload) y H-14 (re-medir).
- **Decisión requerida**: resuelta — techo de 40 s medido (2× TTFN feliz, lejos del hang histórico), fail-fast sin reintento: decisión explícita del usuario.

## 3. Altos

### H-03 — El daemon retiene el stdio del proceso que lo lanzó

- **Severidad**: 🟠 Alta · **Área**: ciclo de vida (`src/main.rs` `desheredar_handles_estandar` + llamada en `handle_daemon`, `crates/avi-daemon/src/spawn.rs:24-71,193-235`, `crates/avi-tts/src/lib.rs:847-854,1014-1027`)
- **Síntoma**: matar el motor no libera el log del lanzador; matar el daemon sí (dos ocasiones). El daemon mantiene ocupados archivos del spawner y puede atar su consola. Reproducido empíricamente por `tts::h03_pipe_stdio_no_debe_quedar_retenido`: bajo `Command::output()` (write-end de pipe heredable), el daemon —y transitivamente `qwen_tts.exe`— heredaban ese handle; `output()` no retornaba hasta que **todos** los holders lo cerraran (matar solo el motor no bastaba; matar el árbol del daemon sí).
- **Causa (demostrada)**: verificada contra docs de Microsoft y rust#146407. En Rust estable `Command::spawn` llama a `CreateProcessW` con `bInheritHandles=TRUE` sin exponer ponerlo en FALSE; con ese flag **todo** handle heredable del padre —incluido el `stdout` = write-end del pipe del lanzador— se duplica al hijo, no solo los 3 STD. NO existe una creation flag `CREATE_NO_HANDLE_INHERIT`: el `0x02000000` que el código usaba con ese nombre es en realidad `CREATE_PRESERVE_CODE_AUTHZ_LEVEL` (no-op para herencia). `Stdio::null` fija los STD del hijo pero no impide heredar otros handles del padre; la herencia se controla por handle con `SetHandleInformation(HANDLE_FLAG_INHERIT, 0)`.
- **Corrección (implementada)**: corte de la herencia en la raíz. Nuevo helper `desheredar_handles_estandar()` (`#[cfg(windows)]`, modelado sobre `instalar_job_con_cierre_de_arbol`) que quita `HANDLE_FLAG_INHERIT` de `STD_INPUT/OUTPUT/ERROR_HANDLE` del proceso vía `SetHandleInformation`, llamado al inicio de `handle_daemon` (antes del `match action`): cubre el CLI (`Start`/`Restart` → `spawn_background`) y el propio daemon (`Serve` → motor), incluido `serve` lanzado directamente bajo un pipe. La rama `Uninstall` —que spawnea `spawn_uninstall_helper` fuera de `handle_daemon`— replica el corte llamando a `desheredar_handles_estandar()` antes del spawn, cerrando la última vía de exposición. Eliminado el bit inerte `0x02000000` en los 4 sitios de spawn (`spawn_background`, `spawn_uninstall_helper`, `Qwen3TtsResident::spawn`, `kill_resident_process`), dejando solo flags con efecto real (`DETACHED_PROCESS 0x8`, `CREATE_NEW_PROCESS_GROUP 0x200`) y corregidos los comentarios que nombraban el flag inexistente. Cero dependencias nuevas (`windows-sys` ya trae `Win32_System_Console` + `Win32_Foundation`) y cero cambios en `Cargo.toml`. Respeta H-04: el stderr del motor sigue yendo al fichero de log vía `Stdio::from` (handle explícito, mecanismo aparte, no afectado). Sin cambios en la ruta Unix (`Stdio::null` + `setsid` + `FD_CLOEXEC` ya resolvían el análogo). Estado: ✅ Implementado (commit 131b109).
- **Impacto resuelto**: el CLI y el daemon dejan de exponer sus STD a los procesos hijos; el pipe del lanzador se libera al terminar el CLI, sin necesidad de matar el árbol. Verificado por la regresión `tts::h03_pipe_stdio_no_debe_quedar_retenido` en aislado (pasa por el camino "H-03 no reproduce", pipe liberado en ~1-2 s sin matar nada; ~24 s con modelo provisionado); `cargo check --all-targets` limpio. Nota: la suite completa conserva la marginalidad temporal de H-15 (guards de 180 s vs. espera del lock global de estado bajo carga; los tests afectados pasan en aislado), no relacionada con este fix.
- **Relaciones**: cierra el cluster de lanzamiento del daemon junto a H-01 ✅ y H-04 ✅ (los tres se resolvían en el lanzamiento) · el corte de herencia por handle es ortogonal al log del motor (H-04) y al cierre del árbol por Job/PID (H-01).
- **Decisión requerida**: resuelta — opción (c) `SetHandleInformation`, huella mínima sin dependencias nuevas, elegida por el usuario. La exposición residual de `spawn_uninstall_helper` (rama `Uninstall`, fuera de `handle_daemon`) también quedó cerrada aplicando el mismo corte antes del spawn.

### H-04 — El motor de voz no deja traza observable

- **Severidad**: 🟠 Alta · **Área**: observabilidad (lanzamiento del residente en `crates/avi-tts/src/lib.rs`, healthcheck `wait_health` en `:987-1023`)
- **Síntoma**: el motor arrancaba con stderr a `Stdio::null()`, silenciando el diagnóstico del motor C (20+ `fprintf(stderr)` en `vendor/qwen3-tts`); un atasco pre-audio no dejaba traza.
- **Causa**: decisión de diseño (silencio del stderr para evitar herencia de handles). Demostrada en código (lanzamiento actual en `crates/avi-tts/src/lib.rs:780-808`). Hipótesis A (eliminar `null`) descartada: el residente se lanza por el daemon (que ya tiene stdio a null) y no hereda el pipe del test; la regresión de `cli_golden` se resolvió con tempfile (`tests/cli_golden.rs:648-665`, `open_atomic_tmp` en `:694`).
- **Corrección (implementada)**: stderr del residente → `data_dir()/logs/qwen3-tts_<pid>_<ms>.log` (rotación por sesión); stdin/stdout conservan `null` + `DETACHED_PROCESS` (`0x8`); la no-herencia del pipe la garantiza el corte en la raíz (`desheredar_handles_estandar`, H-03), no una creation flag. `wait_health` agrega `child.try_wait()` para distinguir *crash* (exit code + path de log) de *hang* (timeout). Estado: ✅ Implementado (commit 865d236).
- **Impacto resuelto**: H-02, H-05 y H-01 son ahora diagnosticables; el motor C deja trazas en stderr.
- **Relaciones**: desbloquea a H-02 y H-05 · comparte zona con H-03 · cierra la ciega de H-01 (orphans visibles vía `try_wait` + log).
- **Decisión requerida**: resuelta — fichero siempre activo (rotación por sesión), no flag. Fundamento: H-01/H-02 son fallas de prod, requieren visibilidad continua.

### H-05 — El daemon reutilizado degrada: sirve estado pero falla síntesis

- **Severidad**: 🟠 Alta · **Área**: daemon (`synthesize_via_residente` en `crates/avi-tts/src/lib.rs:402-434`, `synthesize_handler`/`dub_handler` en `crates/avi-daemon/src/lib.rs`)
- **Síntoma**: con daemon residual reutilizado, el `dub` vía daemon sale exit 5 (timeout de cliente `/dub` 10s) en 12s; con arranque fresco, exit 0 en ~10s. El daemon responde `status`/`warm` pero no sirve la petición.
- **Causa**: demostrada por lectura de código: el flag `WarmState` (`crates/avi-daemon/src/lib.rs:43-66`) es append-only —se escribe una sola vez en el warmup de arranque y nunca refleja degradación posterior— y está desacoplado de la salud real del residente; `synthesize_via_residente` (`crates/avi-tts/src/lib.rs:402-434`) reutilizaba el residente por `voz_key` sin ningún chequeo de vida antes de la petición; ni `dub_handler` ni `synthesize_handler` tenían deadline propio, dependiendo enteramente del timeout de cliente de 10 s (`src/main.rs:3293`).
- **Corrección (implementada)**: salud observada por petición (D-2): antes de reutilizar el residente por `voz_key`, `synthesize_via_residente` ejecuta un healthcheck real (`Qwen3TtsResident::health_check` → `wait_health`: `try_wait` del `Child` para detectar *crash* + `GET /v1/health` para detectar *hang*); si el residente está degradado, lo mata por árbol (`matar_arbol_residente_por_pid`) y rearranca uno fresco de forma determinista, sin fallback best-effort. Deadline de handler `SYNTH_DEADLINE = 8s` (`tokio::time::timeout` + `spawn_blocking`) en `synthesize_handler` y `dub_handler`; al vencer devuelve `synthesis_timeout` sin matar el residente (`translate_handler` no se tocó, fuera de alcance). Reality check (F5): un sumidero TCP real (acepta y no responde) se detecta en ~2 s, muy por debajo del timeout de cliente de 10 s. Estado: ✅ Implementado (commit 5d1dfca).
- **Impacto resuelto**: la reutilización por sesión ya no envenena la fixture ni la sesión: un residente degradado se detecta y se rearranca antes de servir la síntesis, y un cuelgue del motor C falla rápido y determinista en vez de agotar el timeout de cliente.
- **Relaciones**: alimentado por H-01 · H-04 implementado: diagnóstico disponible · H-01 (`clasificar_residual`) cubre el residual *entre* sesiones/arranques; H-05 cierra el cuelgue *intra-sesión* que H-01 no cubría.
- **Decisión requerida**: resuelta — revalidar al reutilizar (salud observada por petición), D-2; descartado el arranque fresco siempre.

### H-06 — `daemon start/serve` sin control de idioma ni STT

- **Severidad**: 🟠 Alta · **Área**: CLI/daemon (`DaemonCommands::{Start,Serve}` en `src/main.rs:314-339`)
- **Síntoma**: `daemon start/serve` solo aceptan `--auto-restart`/`--max-retries`; no hay `--language` (preload de modelos por idioma) ni `--with-stt` (precarga de transcripción), aunque el daemon tiene STT funcional. Falla con `unrecognized argument`.
- **Causa (demostrada)**: ambos flags pertenecían al contrato heredado del oráculo de la CLI Python migrada, nunca al parser Rust (`DaemonCommands::{Start,Serve}` en `src/main.rs:318-343` solo declaran `--auto-restart`/`--max-retries`, default 3) — mismo patrón de deuda documental que cerró H-09 (superficie prometida por la documentación heredada, no por la implementación). `DaemonState::new` (`crates/avi-daemon/src/lib.rs:110`) construye `ParakeetEngine` (STT) siempre, de forma eager e incondicional, y precarga el derivado CT2 de traducción si ya está provisionado; el set de modelos (`MODEL_REVISIONS`) es fijo, sin gating por idioma ni por flag.
- **Corrección (implementada)**: purga documental (Opción 1) — no se reimplementan los flags. Se formaliza en el contrato el comportamiento real: `daemon start`/`serve` precargan STT (Parakeet) + TTS (Qwen3) + el derivado CT2 de traducción (si está provisionado) de forma eager al arrancar, sobre un set de modelos fijo, sin control de idioma ni de STT por flag; `--language` queda como local a `translate`/`dub`/`say`/`synthesize`/`setup`/`doctor`, y `native-stt`/`native-translation` son features de compilación, no flags de ejecución. `docs/CLI/CONTRACT.md:606,610` y `docs/CLI/commands/DAEMON.md:3,94` ya declaraban esta ausencia (auditados y confirmados sin drift adicional en `docs/`, `USAGE.md`, `README.md`, `docs/DESIGN.md`); se deja constancia en esta ficha y en `CHANGELOG.md` para cerrar la brecha oráculo-Rust. Documentación pura, cero cambios de runtime. Estado: ✅ Implementado.
- **Impacto resuelto**: el contrato queda alineado con la implementación Rust — ya no promete una palanca de preload que nunca existió en el parser; los integradores dejan de intentar `--language`/`--with-stt` contra `daemon start/serve` guiados por el oráculo Python.
- **Relaciones**: espeja el precedente de H-09 (deuda del oráculo Python migrado, cerrada por saneo documental); H-07 conserva su propio dilema implementar-vs-documentar de forma independiente (ya no comparte sesión de decisión con H-06).
- **Decisión requerida**: resuelta — purga documental (Opción 1); sin reimplementación de `--language`/`--with-stt` en `daemon start/serve`, comportamiento eager real formalizado en el contrato.

### H-07 — `voice clone --daemon` promete precarga inexistente

- **Severidad**: 🟠 Alta · **Área**: CLI/contrato · **Estado**: ✅ resuelto por adición (restauración de la optimización de hot-path, 2026-09-20)
- **Síntoma**: el contrato prometía que `--daemon` precargaba antes de clonar, pero el endpoint `POST /voices/precompute` fue purgado; `precomputed` quedó vestigial (siempre `false` en tres sitios) y sobrevivía un comentario muerto que lo referenciaba (con errata "carriba").
- **Causa**: purga del endpoint sin restaurar la precarga en su nuevo hogar; la optimización (precalentar la voz para no pagar el cold-start del residente en la primera síntesis) quedó eliminada, no migrada.
- **Corrección aplicada**: restaurada la precarga en caliente en dos ejes complementarios — **A (warm-on-clone)**: `voices_clone_handler` dispara el precalentamiento en segundo plano de la voz recién clonada y devuelve `precomputed: true` («precarga en caliente iniciada», completitud en `GET /health`); la ruta local mantiene `false` (motor efímero, sin residente que calentar). **D (warm-voice configurable)**: `daemon serve`/`start` aceptan `--warm-voice <nombre>` (default `default`), propagado `start`→`serve`, con validación *fail-fast* antes del bind. Se generalizó `warmup_tts`→`precalentar_voz(state, voz)` (primitiva compartida) y se saneó el comentario muerto de `DEFAULT_CLONE_LANGUAGE`.
- **Impacto resuelto**: eliminado el cold-start en los flujos clonar→sintetizar y reinicio-y-reutilizar; `precomputed` y `/health` reportan la verdad; sin residuos del endpoint purgado. Cobertura: `voices_clone_daemon_precomputed_true` y `warm_voice_fail_fast_y_aceptacion` (`crates/avi-daemon/tests/golden.rs`).
- **Decisión requerida**: resuelta — restaurar por adición (no reimplementar el endpoint `/voices/precompute`; la precarga vive en el handler de clonado y en el arranque configurable). `schema_version` sin bump (misma forma del envelope, solo se amplía el dominio de `precomputed`).

### H-08 — Panic con `--mic` sin `--duration` en terminal (validado 2026-09-10)

- **Severidad**: 🟠 Alta · **Área**: CLI (`src/main.rs`, validación `:859-875` y `:1163-1176` frente a `duration.expect("validado arriba")` en `:923`, `:1234`, `:2820`, `:3258`)
- **Síntoma**: en TTY, `speech transcribe --mic` o `dub --mic` sin `--duration` (el caso push-to-talk que `USAGE.md` documenta como funcional) no pide Enter ni usa default: atraviesa la validación —que exime expresamente el TTY— y revienta en `Option::expect` con panic, fuera de toda disciplina de exit codes del contrato.
- **Causa**: demostrada por lectura (2026-09-10): la exención TTY existe en la validación pero su implementación no existe en ningún path —no hay espera de Enter ni duración medida en `src/main.rs` (búsqueda de `Enter|push_to_talk` vacía salvo el comentario)—. Sin TTY el mismo caso sale limpio con exit 2; en TTY es panic en las 4 vías (transcribe/dub × directo/daemon). Nota: `capture_16k_mono_pcm` en sí (`crates/avi-audio/src/lib.rs:179-246`) no tiene panics alcanzables con dispositivos reales (solo `channels == 0` o mutex envenenado, teóricos); el defecto está aguas arriba, en el despacho.
- **Impacto**: crash con stack trace en el flujo interactivo documentado; rompe el contrato de exit codes.
- **Corrección propuesta**: implementar el push-to-talk prometido (espera de Enter + duración medida) o exigir `--duration` también en TTY y corregir `USAGE.md`; convertir los 4 `expect` en error exit 2 como defensa.
- **Relaciones**: roza H-12 (ambos tocan UX interactiva de audio); independiente del resto.
- **Decisión requerida**: sí — ¿push-to-talk real o `--duration` obligatorio?

## 4. Medios

### H-09 — `setup` sin reinstalación forzada ni confirmación

- **Severidad**: 🟡 Media · **Área**: CLI/setup (`src/main.rs`, variante `Setup` y `handle_setup`)
- **Síntoma**: sin `--force-update` no había forma de re-descargar modelos sin purga manual (~9–11,5 GB); sin `--yes` no había modo no interactivo; `--language` era texto libre sin choices (default `"es"`, ajeno a la taxonomía documentada), ignorado salvo para imprimirse y emitirse en JSON; el flag de clonado `--with-base` nombraba el artefacto interno en vez de la capacidad.
- **Causa (demostrada)**: la superficie del comando arrastraba deuda respecto a la implementación real: `--language` prometía una selección de modelos que `handle_setup` nunca aplicaba (el conjunto de `MODEL_REVISIONS` es fijo), y `--force-update`/`--yes` figuraban en la documentación heredada (`SETUP.md` describía la CLI Python `cli.py`) pero no existían en el parser Rust. `--yes` solo vivía en `cleanup`/`uninstall`.
- **Corrección (implementada)**: eliminación total de `--language` (parser, dispatch, `handle_setup`, salida `--json` sin la clave `language`, mensaje humano); rename de `--with-base` a `--with-voice-cloning` sin alias (`--with-clone`/`--clone` retirados) — `src/main.rs`, comentarios de `avi-tts` y `doctor`; `--force-update` que purga los snapshots pinneados (mismo filtro de selección de clonado) más la caché xet vía `remove_hf_snapshot`/`remove_xet_cache` y re-provisiona (la re-conversión CT2 por `mtime` ocurre sola tras la re-descarga), con confirmación destructiva salvo `--yes`/no-TTY (patrón de `handle_cleanup`); `--yes`/`-y` no-op sin `--force-update`. Tests de contrato en `tests/cli_golden.rs` (`setup_help_lista_superficie_vigente`, `setup_json_sin_clave_language`). Documentación de usuario sincronizada: reescritura íntegra de `docs/CLI/commands/SETUP.md` desde la implementación Rust y saneo de drift en `DOCTOR.md`, `TRANSLATE.md`, `USAGE.md`, `README.md`, `DESIGN.md`, `DISTRIBUTION.md`, `MANUAL-VALIDATION.md`; entrada nueva en `CHANGELOG.md`. Estado: ✅ Implementado.
- **Impacto resuelto**: la palanca operativa de re-descarga forzada queda disponible (fricción en CI resuelta con `--yes`), y la superficie de `setup` es coherente, auto-descriptiva y sincronizada extremo a extremo entre código, tests y documentación.
- **Relaciones**: cerraba el loop operativo de provisión; independiente del resto.
- **Decisión requerida**: resuelta — sin retrocompatibilidad (proyecto en desarrollo, sin dependientes externos): alias eliminados y `--language` retirado sin sinónimos; confirmación destructiva replicando el patrón TTY de `cleanup`.

### H-10 — `speech list` sin filtro por voz

- **Severidad**: 🟡 Media · **Área**: CLI (`SpeechCommands::List` unitaria en `src/main.rs:216`, contrato `CONTRACT.md:170`)
- **Síntoma**: `speech list --voice/-v` prometido en contrato es rechazado; imposible distinguir "voz mal escrita" de "sin resultados" (contrato §278).
- **Impacto**: guion E2E y UX de filtrado rotos a nivel menor.
- **Corrección propuesta**: restaurar `--voice/-v` con validación exit 3, o corregir el contrato.
- **Relaciones**: ninguna (aislado, una línea + validación).
- **Decisión requerida**: no — implementar el flag.

## 5. Bajos

### H-11 — `translate --from/--to` acepta cualquier texto

- **Severidad**: ⚪ Baja · **Área**: CLI (`src/main.rs:130-133`, contrato `:596`)
- **Síntoma**: valores libres donde el contrato promete `es|en`; inválidos entran sin error temprano.
- **Impacto**: menor; errores de tipeo llegan lejos sin diagnóstico.
- **Corrección propuesta**: `value_parser = ["es", "en"]` o documentar texto libre.
- **Relaciones**: ninguna (aislado).
- **Decisión requerida**: no.

### H-12 — `speech synthesize --play` sin flujo interactivo

- **Severidad**: ⚪ Baja · **Área**: CLI/UX (contrato §4 `:185-203`, `src/main.rs:1049-1059` reproduce y guarda incondicionalmente)
- **Síntoma**: el contrato promete bucle de 4 opciones (reproducir, aceptar, regenerar, descartar); el binario reproduce y guarda sin preguntar.
- **Impacto**: UX documentada inexistente; cualquier cambio roza el humo de audio de los tests.
- **Corrección propuesta**: implementar el loop o actualizar §4 a `play→save→done` con decisión explícita.
- **Relaciones**: roza los tests de audio (el humo de `say`) y H-08 (UX interactiva) · requiere la decisión de H-08 antes: ambos definen la UX interactiva de audio y no deben diseñarse por separado · hacerlo tras estabilizar ciclo de vida.
- **Decisión requerida**: sí — ¿loop o desdocumentar?

### H-13 — Rama alternativa del gate de traducción sin evidencia

- **Severidad**: ⚪ Baja · **Área**: motor (gate `is_ct2_provisioned` en `crates/avi-store/src/lib.rs:551-577`, conversor `convert_marian_to_ct2` en `src/main.rs:1822-1901`, loader `Translator<ct2rs::tokenizers::auto::Tokenizer>` en `crates/avi-translation/src/lib.rs:20`)
- **Síntoma**: el gate acepta `tokenizer.json` o `source.spm`+`target.spm` pero rechaza `vocab.json`+`merges.txt` por falta de evidencia de que algún snapshot la satisfaga.
- **Causa (demostrada)**: la evidencia estaba en el propio pipeline, no en una corrida pesada: `convert_marian_to_ct2` es la única vía que crea un derivado CT2 y fija la salida a `source.spm`+`target.spm` vía `--copy_files` (`src/main.rs:1850-1852`), con fallback verificado que copia los `.spm` desde el snapshot y aborta si el snapshot no los trae (`:1869-1881`), revalidando con el propio gate antes del `rename` atómico (`:1883`). El layout BPE (`vocab.json`+`merges.txt`) no lo produce este pipeline; el gate era, además, más estricto que el loader `auto::Tokenizer` (superconjunto), de ahí el drift documental "gate == loader".
- **Corrección (implementada)**: cierre por evidencia, sin ampliar superficie (opción "mantener el rechazo, demostrado"). Gate intacto (no se añade la rama BPE especulativa); documentado el invariante real en `ct2_dir_faltantes` y en el doc de `ct2_cache_dir` (gate == salida de `setup` ⊆ lo que el loader carga, con referencia a `convert_marian_to_ct2`); test unitario `ct2_dir_faltantes_contrato_del_gate` que fija el contrato (acepta `.spm` y `tokenizer.json`, rechaza BPE-only y layouts a medias). Drift documental saneado: `docs/CLI/commands/SETUP.md:72` y `docs/CLI/commands/TRANSLATE.md:92` corrigen la equivalencia imprecisa "gate == loader" a "gate == salida de `setup`, subconjunto de lo que el loader carga". Estado: ✅ Implementado.
- **Impacto resuelto**: el riesgo de falso negativo queda cerrado por conocimiento y trazado en el sitio correcto; si un pin de modelo futuro publicara CT2 con layout BPE, la conversión fallaría en `convert_marian_to_ct2` (`:1872-1877`) antes del gate, señalizando la condición que justificaría añadir la rama.
- **Relaciones**: pertenece a provisión (zona ya estable); independiente del resto.
- **Decisión requerida**: resuelta — no ampliar el gate; cierre por evidencia del pipeline + test + saneo de drift.

### H-14 — Techos de guards de tests sin re-medir

- **Severidad**: ⚪ Baja · **Área**: tests (`tests/cli_golden.rs`, constantes 180s resto / 360s dub)
- **Síntoma**: techos derivados de constantes del producto, sin baseline medido con modelos reales.
- **Impacto**: techos mal calibrados (falsos positivos o esperas largas).
- **Corrección propuesta**: re-medir en la próxima corrida pesada instrumentada y ajustar.
- **Relaciones**: cuelga del cluster ciclo de vida (necesita warmup estable para medir bien).
- **Decisión requerida**: no.

### H-15 — Marginalidad temporal de la suite y barrido Unix pendiente

- **Severidad**: ⚪ Baja · **Área**: tests/ciclo de vida (guards en `tests/cli_golden.rs:47,52,134-153`, presupuesto CLI en `src/main.rs:3184`, reaper en `tests/cli_golden.rs:211-236` + barrido `barrer_residente_por_puerto` en `:253-300`)
- **Síntoma**: la dorada `cli_golden` queda 37/40 en esta máquina por 3 rojos clase-timeout, cero fallos de aserción de lógica: `tts::clone_con_daemon_delega` (exit 5 `daemon_unreachable`: el daemon clona bien pero tarda ~1,6 s contra el presupuesto CLI fijo de 1500 ms; en local el mismo clonado tarda ~0,5 s), `tts::h01_aborto_simulado` (guard 180 s esperando `running`: la máquina bajo carga paralela no calentó a tiempo, polls de 300-545 ms) y `tts::translate_force_daemon_sin_daemon_exit5` (guard 180 s esperando `stopped`: parada + sondeos lentos bajo la misma carga). El resto de la suite está verde (`cargo check` limpio, `--lib` 54/54, binario 4/4) y el cierre post-suite deja cero huérfanos a nivel SO.
- **Causa**: marginalidad temporal pre-existente de la máquina bajo carga paralela, no regresión del diff: con el árbol en `stash` (base `ff9a9f9`, sin cambios D-01..D-05) `clone_con_daemon_delega` falla idéntico (exit 5, 1589 ms); los 2 guard-timeouts son colapso bajo carga paralela, nunca aserciones de conducta. El presupuesto de 1500 ms NO se toca en este cierre (decisión cerrada F0 §4/F5).
- **Impacto**: suite completa 37/40 idéntica en base; sin cascada (D-03 contiene: 37 pasan con rojos dentro) y sin fuga (el rojo persiste pero ya sin huérfano del daemon). Ningún ✅ de este documento sobrestima: H-01 sigue ✅ por producto + harness, la suite se declara en 37/40 con rojos visibles.
- **Corrección propuesta (follow-up, no en este cierre)**: 1) re-medir en CI/entorno rápido si los guards de 180 s y el presupuesto fijo de 1500 ms para clonado pesado siguen calibrados bajo carga paralela, y solo entonces decidir si se ajustan; 2) probar en CI el crash vivo de D-05 con log (los dorados pesados hacen skip en local); 3) implementar el barrido del residente en 8766 del reaper en Unix (hoy solo log; en Windows barre por puerto preciso con `netstat -ano`, sin kill por imagen, con verificación a 8 s); 4) runtime Unix de D-01 en CI. Sin reality check Unix posible en esta máquina (techo declarado: solo toolchains Windows instalados).
- **Relaciones**: cuelga de H-01 (invariantes intactos; el reaper contiene los rojos) · extiende a H-14 (re-medir techos con baseline real) · techos de D-01 (runtime Unix) y D-05 (crash vivo) · cobertura D-03 (8 tests con `ensure` + barrido 8766).
- **Decisión requerida**: sí — ¿en qué scope se agenda este follow-up (CI rápido para re-medir guards/presupuesto + crash vivo D-05 + runtime Unix D-01 + barrido Unix del reaper), manteniendo intacto el presupuesto de 1500 ms hasta entonces?

### H-16 — Drift Python en los docs de comando

- **Severidad**: ⚪ Baja · **Área**: documentación (`docs/CLI/commands/{CLEANUP,DEVICES,DOCTOR,SPEECH,TRANSLATE,VERSION,VOICE}.md`)
- **Síntoma**: tras la migración Python→Rust, siete docs de comando describían la implementación como si fuera la CLI Python inexistente (citaban `cli.py`, `audio.py`, `server.py`, `test_cli.py`, funciones `cmd_*`, `emit_json`, `psutil`, `miniaudio`, `pycaw`, `shutil.rmtree`, etc.): 107 ocurrencias de símbolos Python. Un lector concluiría que la herramienta es Python.
- **Causa (demostrada)**: drift heredado de la migración. La reescritura de `SETUP.md`/`DAEMON.md` ancló solo esos dos docs a la implementación Rust real; los otros siete quedaron sin sincronizar. En el repo ya no queda ningún `.py` ni `src/ai_voice_interconnector/`.
- **Corrección (implementada)**: reescritura desde la implementación Rust de los 7 docs (delegada a subagentes, uno por doc), tomando `SETUP.md`/`DAEMON.md` como exemplars de estructura, tono y nivel de detalle; cada contrato `--json` verificado contra la serialización real del handler y las fixtures `tests/golden/*`; anclas `archivo:línea` reales del árbol Rust. `CLEANUP.md` recibió corrección ligera (ya anclado a Rust; solo se retiró la mención Python del preámbulo, conservando la nota de divergencia deliberada `cli.py:2177`). Gate anti-drift repo-wide sobre `docs/CLI/commands/`: el único hit admisible restante es esa divergencia explícitamente etiquetada. Documentación pura, cero cambios de runtime. Estado: ✅ Implementado.
- **Impacto resuelto**: los 7 docs describen la superficie y el flujo Rust reales; los gaps de contrato aún pendientes (H-07 precompute, H-08 `--mic` sin `--duration`, H-10 `list --voice`, H-12 `--play`) quedan documentados como comportamiento real, sin prometer features inexistentes ni confundirse con divergencias deliberadas.
- **Relaciones**: continúa el saneo de drift documental iniciado en H-09 (`SETUP.md`) y H-13 (matiz del gate CT2 en `TRANSLATE.md`/`SETUP.md`, preservado en esta reescritura); independiente del resto.
- **Decisión requerida**: resuelta — reescritura desde Rust; el oráculo Python solo se cita cuando está explícitamente etiquetado como divergencia deliberada (D1–D5).

## 6. Grafo de relaciones y orden de ataque

```
H-04 ✅ (traza del residente) — stderr → logs/qwen3-tts_*.log + try_wait en wait_health (commit 865d236)
 ├─ desbloquea ─> H-02 ✅ (deadline de warmup 40 s + fallback subprocess eliminado, commit 30f7cf1)
 ├─ desbloquea ─> H-05 ✅ (degradación por reutilización, cerrada con salud observada por petición, commit 5d1dfca)
 └─ comparte zona ─> H-03 ✅ (herencia de handles cortada en la raíz: SetHandleInformation en handle_daemon; 0x02000000 inerte eliminado) ── H-01 ✅ (cierre estructural: reclamo + parada unificada + verificación SO)
H-01 ✅ ── contenía ──> H-05 ✅ (residual degradado: ahora se reclama con `started`; H-05 cierra el cuelgue intra-sesión)
H-02 ✅ ── reduce superficie de ──> H-01 ✅ (sin subprocess que re-lanzar; el fail-fast elimina los abortos a ciegas del observador)
H-02 ✅ estable ── permite ──> H-14 (re-medir techos) · H-15 (marginalidad temporal + presupuesto de clonado + barrido Unix del reaper)
H-01 ✅ ── contiene ──> H-15 (3 rojos clase-timeout sin cascada ni fuga; bisect en base sin regresión)
H-09 ✅ (superficie de flags de `setup` saneada: `--language` eliminado, `--with-base`→`--with-voice-cloning`, `--force-update`/`--yes` implementados + tests + saneo de drift) · H-13 ✅ (gate del derivado CT2: cierre por evidencia del pipeline + test + saneo de drift) · H-16 ✅ (drift Python en los 7 docs de comando restantes: reescritura desde Rust delegada a subagentes + gate anti-drift; continúa el saneo de H-09/H-13) · H-06 ✅ (purga documental: `--language`/`--with-stt` retirados del contrato de `daemon start/serve`, comportamiento eager real formalizado, mismo patrón que H-09)
H-10 · H-11 ── triviales aislados (relleno)
H-07 ✅ (precarga en caliente restaurada por adición: warm-on-clone `precomputed:true` + `--warm-voice` configurable con fail-fast; `precalentar_voz` compartida; comentario muerto saneado) ── cerrado sin reimplementar `/voices/precompute`
H-08 ⇆ H-12 (UX interactiva de audio: decidir H-08 primero, diseñar H-12 después)
H-12 ── última (toca UX de audio + humo de tests)
H-15 ── follow-up de medición/infra (tras H-14): re-medir guards/presupuesto + crash vivo D-05 + runtime Unix D-01 + barrido Unix del reaper, todo en CI/entorno rápido
```

**Orden recomendado (con fundamento)**:

1. **H-04 ✅ — H-02 ✅ — H-01 ✅ — H-05 ✅ — H-03 ✅** — traza del residente implementada (stderr→log + `try_wait`), deadline de warmup de 40 s, cierre estructural (reclamo matar-y-rearrancar + parada unificada de 8 s + verificación SO), salud observada por petición (revalidación + rearranque determinista del residente reutilizado) y corte de herencia de handles en la raíz (`SetHandleInformation` en `handle_daemon`) — cluster de lanzamiento/reutilización del daemon cerrado: base observable, sin huérfanos, sin degradación silenciosa y sin retención de stdio del lanzador. Fundamento: sin traza no hay diagnóstico posible, sin cierre no hay corrida limpia y todo lo que toca el daemon depende de un arranque estable. Estado: H-04 ✅ (865d236), H-02 ✅ (30f7cf1), H-01 ✅ (a908ac6+193eeac), H-05 ✅ (5d1dfca) y H-03 ✅ (131b109) implementados; **cluster ciclo de vida completo**, H-14 habilitado. H-15 queda como follow-up (marginalidad temporal de la suite + presupuesto 1500 ms intacto + crash vivo D-05 + runtime Unix D-01 y barrido Unix del reaper, todo pendiente de CI/entorno rápido).
2. **H-09 ✅** (+ **H-13 ✅** + **H-06 ✅**) — independientes, pequeños, sin decisiones pendientes. H-09 ✅ cerrado (superficie de flags de `setup` saneada: `--language` eliminado, `--with-base`→`--with-voice-cloning` sin alias, `--force-update`/`--yes` implementados, tests de contrato y saneo de drift documental). H-13 ✅ cerrado (gate del derivado CT2: cierre por evidencia del pipeline + test + saneo de drift documental). H-06 ✅ cerrado (purga documental: `--language`/`--with-stt` retirados del contrato de `daemon start/serve`, sin reimplementación; comportamiento eager real —STT/TTS/CT2 precargados al arrancar, set fijo— formalizado, mismo patrón que H-09).
3. **H-07 ✅** — cerrado por restauración de la optimización de hot-path (warm-on-clone A + `--warm-voice` configurable D), no por purga documental: `precomputed:true` en ruta daemon («precarga iniciada», completitud en `/health`), `false` en local; `precalentar_voz` compartida por arranque y clonado; comentario muerto de `/voices/precompute` saneado. Queda **H-08** como única sesión implementar-vs-documentar pendiente (decidir antes de diseñar H-12).
4. **H-10 + H-11** — triviales aislados.
5. **H-12 última** — requiere la decisión de H-08 ya resuelta y ciclo de vida estable.
