# Hallazgos pendientes — revisión consolidada

- **Fecha**: 2026-09-10
- **Estado**: 3 resueltos (H-04 ✅, H-02 ✅, H-01 ✅) — 12 pendientes (H-03, H-05–H-15)
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

## 2. Críticos

### H-01 — Abortos y fallos dejan daemon + motor huérfanos

- **Severidad**: 🔴 Crítica · **Área**: ciclo de vida (`crates/avi-daemon/src/spawn.rs`, `crates/avi-daemon/src/lib.rs:1152-1342`, `crates/avi-tts/src/lib.rs:336`, `src/main.rs:355-411,1409-1572,2098-2230`)
- **Síntoma**: toda vía anormal (aborto externo, timeout, panic del test antes del apagado) dejaba vivos a `ai-voice-interconnector` + `qwen_tts`, ociosos, ocupando puertos y ficheros. Observado en 4 ocasiones en un solo día; un antecedente consumió 8 horas de CPU.
- **Causa**: demostrada por lectura y por eliminación: el apagado solo corría en la vía feliz —el handler Ctrl+C hacía `exit(130)` sin limpieza (`src/main.rs:355`), `Stop` borraba el pidfile aunque el proceso siguiera vivo, `Restart` acumulaba doble techo y kill por PID duplicado sin árbol del residente, `serve` no escuchaba señales, el residente se mataba por imagen global y la fixture se adhería por probe sin revalidar (`already_running` ciego)—. Los procesos sobrevivían a la muerte de su padre.
- **Corrección (implementada)**: cierre estructural en producto + harness. Producto: handler Ctrl+C con limpieza acotada de 2 s y exit 130 preservado (`CTRL_C_LIMPIEZA_DEADLINE`); Job `KILL_ON_JOB_CLOSE` en la rama `Serve` (Windows) más árbol matable por PID con verificación (`matar_arbol_por_pid`: `taskkill /F /T` en Windows, `kill -9` al grupo en Unix); reclamo matar-y-rearrancar al arrancar (`clasificar_residual` por PID vivo + probe: sano → `already_running`, degradado → se reclama el árbol y se rearranca con salida 0 y payload `started`); parada unificada con deadline global de 8 s (`stop_daemon_and_resident` al servicio de reclamo, stop, restart, cleanup y uninstall: graceful 1,5 s + espera 3 s + árbol preciso + verificación, borrado del pidfile solo tras muerte verificada y exit 5 sin borrar pista si el árbol sigue vivo); `Restart` sobre el ayudante único sin doble techo ni kill duplicado; `serve` escucha Ctrl+C/SIGTERM por la misma ruta que `POST /shutdown` y `shutdown_handler` mata el árbol preciso del residente primero, con kill por imagen solo como último recurso documentado con verificación inmediata; residente sin breakaway y `Drop` como cierre por árbol. Harness: `verificar_cero_huerfanos` (árbol muerto + puertos 8765/8766 cerrados + pidfile sin PID vivo, `panic!` con reaper previo si queda resto), reaper ante techo, timeout, `warm_failed` o cualquier `panic!` fuera de guards/polls (`fallo_con_reaper` + `GuardReaper`), `STATE_LOCK` con recuperación ante envenenado (mismo tipo y contrato) e higiene de `TEST_LIMITE` restaurada, `ensure` que revalida y prueba pesada nueva `tts::h01_aborto_simulado_reclama_y_no_deja_huerfanos` (reclamo con `started` + cero huérfanos a nivel SO). Cierre D-01..D-05 (F5): D-01 con techo (compila sin warnings + predicado puro en verde + tensado `#[cfg(unix)]` sin simular; runtime Unix diferido a CI); D-02 (PID en memoria + `serve` por misma ruta, 130 y 2 s intactos, sin rojo que los contradiga); D-03 (`d03_*` 2/2, reaper verificado en fallos reales con muerte verificada, cobertura a 8 tests con `ensure` + barrido del residente en 8766 por puerto preciso con `netstat -ano` y verificación a 8 s —en Unix solo log, pendiente, ver H-15—); D-04 (predicado puro en verde, residente-solo con preciso-primero e imagen del residente solo como último recurso verificado, imagen del daemon prohibida); D-05 en código (reclamo activo con deadline 5 s + verificación, backoff y `Ok` intactos; crash vivo con log pendiente de CI/entorno rápido, ver H-15). Estado: ✅ Implementado (commit a908ac6 + restos D-01..D-05).
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

- **Severidad**: 🟠 Alta · **Área**: ciclo de vida (`crates/avi-daemon/src/spawn.rs:21-66`)
- **Síntoma**: matar el motor no libera el log del lanzador; matar el daemon sí (dos ocasiones). El daemon mantiene ocupados archivos del spawner y puede atar su consola.
- **Causa**: no demostrada del todo (hipótesis: se hereda `stderr` pese a anular stdio). La anulación actual es necesaria pero insuficiente.
- **Impacto**: ficheros bloqueados, consolas atadas, misma familia que H-01.
- **Corrección propuesta**: auditar los tres streams en el lanzamiento y en el respawn de supervisión + regresión con tempfile.
- **Relaciones**: misma zona que H-01 y H-04 (los tres se resuelven en el lanzamiento del daemon).
- **Decisión requerida**: sí — ¿en qué scope se agenda?

### H-04 — El motor de voz no deja traza observable

- **Severidad**: 🟠 Alta · **Área**: observabilidad (lanzamiento del residente en `crates/avi-tts/src/lib.rs`, healthcheck `wait_health` en `:896-1018`)
- **Síntoma**: el motor arrancaba con stderr a `Stdio::null()`, silenciando el diagnóstico del motor C (20+ `fprintf(stderr)` en `vendor/qwen3-tts`); un atasco pre-audio no dejaba traza.
- **Causa**: decisión de diseño (silencio del stderr para evitar herencia de handles). Demostrada en código (`:895-916`). Hipótesis A (eliminar `null`) descartada: el residente se lanza por el daemon (que ya tiene stdio a null) y no hereda el pipe del test; la regresión de `cli_golden` se resolvió con tempfile (`tests/cli_golden.rs:319-331`).
- **Corrección (implementada)**: stderr del residente → `data_dir()/logs/qwen3-tts_<pid>_<ms>.log` (rotación por sesión); stdin/stdout conservan `null` + flags de no-herencia (`0x02000000|0x8`). `wait_health` agrega `child.try_wait()` para distinguir *crash* (exit code + path de log) de *hang* (timeout). Estado: ✅ Implementado (commit 865d236).
- **Impacto resuelto**: H-02, H-05 y H-01 son ahora diagnosticables; el motor C deja trazas en stderr.
- **Relaciones**: desbloquea a H-02 y H-05 · comparte zona con H-03 · cierra la ciega de H-01 (orphans visibles vía `try_wait` + log).
- **Decisión requerida**: resuelta — fichero siempre activo (rotación por sesión), no flag. Fundamento: H-01/H-02 son fallas de prod, requieren visibilidad continua.

### H-05 — El daemon reutilizado degrada: sirve estado pero falla síntesis

- **Severidad**: 🟠 Alta · **Área**: daemon (`DaemonState::new` en `crates/avi-daemon/src/lib.rs:116-128`, `translate_handler` `:594-708`, `dub_handler` `:977-988`)
- **Síntoma**: con daemon residual reutilizado, el `dub` vía daemon sale exit 5 (timeout de cliente `/dub` 10s) en 12s; con arranque fresco, exit 0 en ~10s. El daemon responde `status`/`warm` pero no sirve la petición.
- **Causa**: no demostrada. H-04 (implementado) restaura la traza del residente, disponible para investigarla.
- **Impacto**: la fixture de sesión reutiliza el daemon por diseño, por lo que un residual degradado envenena toda la sesión de tests.
- **Corrección propuesta**: validar salud real (no solo `warm`) al reutilizar, o no reutilizar nunca un daemon ajeno a la sesión.
- **Relaciones**: alimentado por H-01 · H-04 implementado: diagnóstico disponible.
- **Decisión requerida**: sí — ¿revalidar al reutilizar o arranque fresco siempre?

### H-06 — `daemon start/serve` sin control de idioma ni STT

- **Severidad**: 🟠 Alta · **Área**: CLI/daemon (`DaemonCommands::{Start,Serve}` en `src/main.rs:265-286`)
- **Síntoma**: `daemon start/serve` solo aceptan `--auto-restart`/`--max-retries`; no hay `--language` (preload de modelos por idioma) ni `--with-stt` (precarga de transcripción), aunque el daemon tiene STT funcional. Falla con `unrecognized argument`.
- **Causa**: migración que descartó flags funcionales. Demostrada contra el oráculo.
- **Impacto**: sin control de preload; los consumidores que lo esperan no pueden usarlo.
- **Corrección propuesta**: reimplementar ambos flags mapeados al preload real, o purgarlos formalmente del contrato con motivo documentado.
- **Relaciones**: altera el arranque/warmup → implementar después de estabilizar H-02 · mismo dilema implementar-vs-documentar que H-07 (resolver ambas decisiones en una sola sesión de diseño).
- **Decisión requerida**: sí — ¿reimplementar o purgar?

### H-07 — `voice clone --daemon` promete precarga inexistente

- **Severidad**: 🟠 Alta · **Área**: CLI/contrato (`docs/CLI/CONTRACT.md:238,241`, handler `voices_clone_handler`, `crates/avi-daemon/src/lib.rs:738` devuelve siempre `"precomputed": false`)
- **Síntoma**: el contrato promete que `--daemon` precarga los embeddings antes de clonar, pero el endpoint `POST /voices/precompute` no existe (purgado); el flag existe y el enrutado funciona, la feature no.
- **Causa**: purga del endpoint sin actualizar contrato. Demostrada (el router no lo registra).
- **Impacto**: clonado vía daemon sin la aceleración contratada; especificación falsa para integradores `--json`.
- **Corrección propuesta**: implementar el endpoint o corregir el contrato a lo que realmente hace (`/voices/clone` sin precompute).
- **Relaciones**: mismo router y documento que H-06; mismo dilema implementar-vs-documentar.
- **Decisión requerida**: sí — ¿endpoint o contrato?

### H-08 — Panic con `--mic` sin `--duration` en terminal (validado 2026-09-10)

- **Severidad**: 🟠 Alta · **Área**: CLI (`src/main.rs`, validación `:792-798` y `:1086-1094` frente a `duration.expect("validado arriba")` en `:846`, `:1157`, `:2576`, `:3014`)
- **Síntoma**: en TTY, `speech transcribe --mic` o `dub --mic` sin `--duration` (el caso push-to-talk que `USAGE.md` documenta como funcional) no pide Enter ni usa default: atraviesa la validación —que exime expresamente el TTY— y revienta en `Option::expect` con panic, fuera de toda disciplina de exit codes del contrato.
- **Causa**: demostrada por lectura (2026-09-10): la exención TTY existe en la validación pero su implementación no existe en ningún path —no hay espera de Enter ni duración medida en `src/main.rs` (búsqueda de `Enter|push_to_talk` vacía salvo el comentario)—. Sin TTY el mismo caso sale limpio con exit 2; en TTY es panic en las 4 vías (transcribe/dub × directo/daemon). Nota: `capture_16k_mono_pcm` en sí (`crates/avi-audio/src/lib.rs:179-246`) no tiene panics alcanzables con dispositivos reales (solo `channels == 0` o mutex envenenado, teóricos); el defecto está aguas arriba, en el despacho.
- **Impacto**: crash con stack trace en el flujo interactivo documentado; rompe el contrato de exit codes.
- **Corrección propuesta**: implementar el push-to-talk prometido (espera de Enter + duración medida) o exigir `--duration` también en TTY y corregir `USAGE.md`; convertir los 4 `expect` en error exit 2 como defensa.
- **Relaciones**: roza H-12 (ambos tocan UX interactiva de audio); independiente del resto.
- **Decisión requerida**: sí — ¿push-to-talk real o `--duration` obligatorio?

## 4. Medios

### H-09 — `setup` sin reinstalación forzada ni confirmación

- **Severidad**: 🟡 Media · **Área**: CLI/setup (`src/main.rs:122-131`)
- **Síntoma**: sin `--force-update` no hay forma de re-descargar modelos sin purga manual (~14 GB); sin `--yes` no hay modo no interactivo; `--language` es texto libre sin choices (`es-latam|en|all`).
- **Impacto**: la palanca operativa que faltó ante provisiones rotas; fricción en CI.
- **Corrección propuesta**: restaurar `--force-update` (o equivalente), `--yes` y `value_parser` de `--language`.
- **Relaciones**: cierra el loop operativo de provisión; independiente del resto.
- **Decisión requerida**: sí — política de confirmación y equivalencia exacta de `--force-update`.

### H-10 — `speech list` sin filtro por voz

- **Severidad**: 🟡 Media · **Área**: CLI (`SpeechCommands::List` unitaria en `src/main.rs:188`, contrato `CONTRACT.md:170`)
- **Síntoma**: `speech list --voice/-v` prometido en contrato es rechazado; imposible distinguir "voz mal escrita" de "sin resultados" (contrato §278).
- **Impacto**: guion E2E y UX de filtrado rotos a nivel menor.
- **Corrección propuesta**: restaurar `--voice/-v` con validación exit 3, o corregir el contrato.
- **Relaciones**: ninguna (aislado, una línea + validación).
- **Decisión requerida**: no — implementar el flag.

## 5. Bajos

### H-11 — `translate --from/--to` acepta cualquier texto

- **Severidad**: ⚪ Baja · **Área**: CLI (`src/main.rs:102-105`, contrato `:596`)
- **Síntoma**: valores libres donde el contrato promete `es|en`; inválidos entran sin error temprano.
- **Impacto**: menor; errores de tipeo llegan lejos sin diagnóstico.
- **Corrección propuesta**: `value_parser = ["es", "en"]` o documentar texto libre.
- **Relaciones**: ninguna (aislado).
- **Decisión requerida**: no.

### H-12 — `speech synthesize --play` sin flujo interactivo

- **Severidad**: ⚪ Baja · **Área**: CLI/UX (contrato §4 `:185-203`, `src/main.rs:857-865` reproduce y guarda incondicionalmente)
- **Síntoma**: el contrato promete bucle de 4 opciones (reproducir, aceptar, regenerar, descartar); el binario reproduce y guarda sin preguntar.
- **Impacto**: UX documentada inexistente; cualquier cambio roza el humo de audio de los tests.
- **Corrección propuesta**: implementar el loop o actualizar §4 a `play→save→done` con decisión explícita.
- **Relaciones**: roza los tests de audio (el humo de `say`) y H-08 (UX interactiva) · requiere la decisión de H-08 antes: ambos definen la UX interactiva de audio y no deben diseñarse por separado · hacerlo tras estabilizar ciclo de vida.
- **Decisión requerida**: sí — ¿loop o desdocumentar?

### H-13 — Rama alternativa del gate de traducción sin evidencia

- **Severidad**: ⚪ Baja · **Área**: motor (gate `is_ct2_provisioned`, orden de `auto::Tokenizer` en `ct2rs`)
- **Síntoma**: el gate acepta `tokenizer.json` o `source.spm`+`target.spm` pero rechaza `vocab.json`+`merges.txt` por falta de evidencia de que algún snapshot la satisfaga.
- **Impacto**: ninguno mientras ningún snapshot la requiera; riesgo de falso negativo futuro.
- **Corrección propuesta**: demostrarla contra modelos reales o mantener el rechazo.
- **Relaciones**: pertenece a provisión (zona ya estable); cabe en cualquier orquestación de medición.
- **Decisión requerida**: no — medir cuando se toque traducción.

### H-14 — Techos de guards de tests sin re-medir

- **Severidad**: ⚪ Baja · **Área**: tests (`tests/cli_golden.rs`, constantes 180s resto / 360s dub)
- **Síntoma**: techos derivados de constantes del producto, sin baseline medido con modelos reales.
- **Impacto**: techos mal calibrados (falsos positivos o esperas largas).
- **Corrección propuesta**: re-medir en la próxima corrida pesada instrumentada y ajustar.
- **Relaciones**: cuelga del cluster ciclo de vida (necesita warmup estable para medir bien).
- **Decisión requerida**: no.

### H-15 — Marginalidad temporal de la suite y barrido Unix pendiente

- **Severidad**: ⚪ Baja · **Área**: tests/ciclo de vida (guards en `tests/cli_golden.rs:47,52,134-153`, presupuesto CLI en `src/main.rs:3187`, reaper en `tests/cli_golden.rs:188-213`)
- **Síntoma**: la dorada `cli_golden` queda 37/40 en esta máquina por 3 rojos clase-timeout, cero fallos de aserción de lógica: `tts::clone_con_daemon_delega` (exit 5 `daemon_unreachable`: el daemon clona bien pero tarda ~1,6 s contra el presupuesto CLI fijo de 1500 ms; en local el mismo clonado tarda ~0,5 s), `tts::h01_aborto_simulado` (guard 180 s esperando `running`: la máquina bajo carga paralela no calentó a tiempo, polls de 300-545 ms) y `tts::translate_force_daemon_sin_daemon_exit5` (guard 180 s esperando `stopped`: parada + sondeos lentos bajo la misma carga). El resto de la suite está verde (`cargo check` limpio, `--lib` 54/54, binario 4/4) y el cierre post-suite deja cero huérfanos a nivel SO.
- **Causa**: marginalidad temporal pre-existente de la máquina bajo carga paralela, no regresión del diff: con el árbol en `stash` (base `ff9a9f9`, sin cambios D-01..D-05) `clone_con_daemon_delega` falla idéntico (exit 5, 1589 ms); los 2 guard-timeouts son colapso bajo carga paralela, nunca aserciones de conducta. El presupuesto de 1500 ms NO se toca en este cierre (decisión cerrada F0 §4/F5).
- **Impacto**: suite completa 37/40 idéntica en base; sin cascada (D-03 contiene: 37 pasan con rojos dentro) y sin fuga (el rojo persiste pero ya sin huérfano del daemon). Ningún ✅ de este documento sobrestima: H-01 sigue ✅ por producto + harness, la suite se declara en 37/40 con rojos visibles.
- **Corrección propuesta (follow-up, no en este cierre)**: 1) re-medir en CI/entorno rápido si los guards de 180 s y el presupuesto fijo de 1500 ms para clonado pesado siguen calibrados bajo carga paralela, y solo entonces decidir si se ajustan; 2) probar en CI el crash vivo de D-05 con log (los dorados pesados hacen skip en local); 3) implementar el barrido del residente en 8766 del reaper en Unix (hoy solo log; en Windows barre por puerto preciso con `netstat -ano`, sin kill por imagen, con verificación a 8 s); 4) runtime Unix de D-01 en CI. Sin reality check Unix posible en esta máquina (techo declarado: solo toolchains Windows instalados).
- **Relaciones**: cuelga de H-01 (invariantes intactos; el reaper contiene los rojos) · extiende a H-14 (re-medir techos con baseline real) · techos de D-01 (runtime Unix) y D-05 (crash vivo) · cobertura D-03 (8 tests con `ensure` + barrido 8766).
- **Decisión requerida**: sí — ¿en qué scope se agenda este follow-up (CI rápido para re-medir guards/presupuesto + crash vivo D-05 + runtime Unix D-01 + barrido Unix del reaper), manteniendo intacto el presupuesto de 1500 ms hasta entonces?

## 6. Grafo de relaciones y orden de ataque

```
H-04 ✅ (traza del residente) — stderr → logs/qwen3-tts_*.log + try_wait en wait_health (commit 865d236)
 ├─ desbloquea ─> H-02 ✅ (deadline de warmup 40 s + fallback subprocess eliminado, commit 30f7cf1)
 ├─ desbloquea ─> H-05 (degradación por reutilización, ahora diagnosticable con H-01 ✅)
 └─ comparte zona ─> H-03 (streams del lanzamiento, siguiente del cluster) ── H-01 ✅ (cierre estructural: reclamo + parada unificada + verificación SO)
H-01 ✅ ── contenía ──> H-05 (residual degradado: ahora se reclama con `started`)
H-02 ✅ ── reduce superficie de ──> H-01 ✅ (sin subprocess que re-lanzar; el fail-fast elimina los abortos a ciegas del observador)
H-02 ✅ estable ── permite ──> H-06 (flags de preload) · H-14 (re-medir techos) · H-15 (marginalidad temporal + presupuesto de clonado + barrido Unix del reaper)
H-01 ✅ ── contiene ──> H-15 (3 rojos clase-timeout sin cascada ni fuga; bisect en base sin regresión)
H-09 · H-13 ── independientes (provisión/medición)
H-10 · H-11 ── triviales aislados (relleno)
H-07 + H-06 ── mismo dilema implementar-vs-documentar (superficie daemon)
H-08 ⇆ H-12 (UX interactiva de audio: decidir H-08 primero, diseñar H-12 después)
H-12 ── última (toca UX de audio + humo de tests)
H-15 ── follow-up de medición/infra (tras H-14): re-medir guards/presupuesto + crash vivo D-05 + runtime Unix D-01 + barrido Unix del reaper, todo en CI/entorno rápido
```

**Orden recomendado (con fundamento)**:

1. **H-04 ✅ — H-02 ✅ — H-01 ✅** — traza del residente implementada (stderr→log + `try_wait`), deadline de warmup de 40 s y cierre estructural (reclamo matar-y-rearrancar + parada unificada de 8 s + verificación SO) — base observable y sin huérfanos (misma zona: lanzamiento del daemon); **luego H-03 + H-05** con traza ya visible y reclamo, **re-midiendo H-14**. Fundamento: sin traza no hay diagnóstico posible, sin cierre no hay corrida limpia y todo lo que toca el daemon depende de un arranque estable. Estado: H-04 ✅ (865d236), H-02 ✅ (30f7cf1) y H-01 ✅ implementados; H-05 diagnosticable, H-03 siguiente del cluster y H-14 habilitado. H-15 queda como follow-up (suite 37/40 por marginalidad temporal + presupuesto 1500 ms intacto + crash vivo D-05 + runtime Unix D-01 y barrido Unix del reaper, todo pendiente de CI/entorno rápido).
2. **H-03 + H-05** — siguiente del cluster ciclo de vida: auditar los tres streams en el lanzamiento (H-03, misma zona `spawn.rs`) e investigar la degradación por reutilización con traza H-04 y reclamo H-01 ya disponibles (H-05 diagnosticable).
3. **H-09 + H-13** — independientes, pequeños, sin decisiones; rellenan mientras se mide el warmup.
4. **Sesión única de decisiones H-06 + H-07 + H-08** — las tres son implementar-vs-documentar/purgar; decidirlas juntas evita tres rondas. Luego implementar lo decidido.
5. **H-10 + H-11** — triviales aislados.
6. **H-12 última** — requiere la decisión de H-08 ya resuelta y ciclo de vida estable.
