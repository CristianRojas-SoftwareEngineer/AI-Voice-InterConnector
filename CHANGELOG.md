# Changelog

Todos los cambios notables de AI Voice InterConnector se documentan en este archivo.

El formato se basa en [Keep a Changelog](https://keepachangelog.com/es-ES/1.1.0/)
y el proyecto adhiere a [Versionado Semántico](https://semver.org/lang/es/).

## Tabla de contenidos

- [0.18.12 — 2026-08-31](#01812-20260831)
- [0.18.11 — 2026-08-31](#01811-20260831)
- [0.18.10 — 2026-08-31](#01810-20260831)
- [0.18.9 — 2026-08-31](#0189-20260831)
- [0.18.8 — 2026-08-31](#0188-20260831)
- [0.18.7 — 2026-08-31](#0187-20260831)
- [0.18.6 — 2026-08-30](#0186-20260830)
- [0.18.5 — 2026-08-30](#0185-20260830)
- [0.18.4 — 2026-08-30](#0184-20260830)
- [0.18.3 — 2026-08-30](#0183-20260830)
- [0.18.2 — 2026-08-28](#0182-20260828)
- [0.18.1 — 2026-08-28](#0181-20260828)
- [0.18.0 — 2026-08-28](#0180-20260828)
- [0.17.1 — 2026-08-28](#0171-20260828)
- [0.17.0 — 2026-08-28](#0170-20260828)
- [0.16.0 — 2026-08-27](#0160-20260827)
- [0.15.2 — 2026-08-27](#0152-20260827)
- [0.15.1 — 2026-08-27](#0151-20260827)
- [0.15.0 — 2026-08-27](#0150-20260827)
- [0.14.0 — 2026-08-27](#0140-20260827)
- [0.13.0 — 2026-08-27](#0130--2026-08-27)
- [0.12.0 — 2026-08-27](#0120--2026-08-27)
- [0.11.3 — 2026-08-26](#0113--2026-08-26)
- [0.11.2 — 2026-08-26](#0112--2026-08-26)
- [0.11.1 — 2026-08-26](#0111--2026-08-26)
- [0.11.0 — 2026-08-26](#0110--2026-08-26)
- [0.10.7 — 2026-08-24](#0107--2026-08-24)
- [0.10.6 — 2026-08-24](#0106--2026-08-24)
- [0.10.5 — 2026-08-11](#0105--2026-08-11)
- [0.10.4 — 2026-08-10](#0104--2026-08-10)
- [0.10.3 — 2026-08-10](#0103--2026-08-10)
- [0.10.2 — 2026-08-10](#0102--2026-08-10)
- [0.10.1 — 2026-08-10](#0101--2026-08-10)
- [0.10.0 — 2026-08-09](#0100--2026-08-09)
- [0.9.1 — 2026-07-31](#091--2026-07-31)
- [0.9.0 — 2026-07-31](#090--2026-07-31)
- [0.8.0 — 2026-07-22](#080--2026-07-22)
- [0.7.8 — 2026-07-22](#078--2026-07-22)
- [0.7.7 — 2026-07-22](#077--2026-07-22)
- [0.7.6 — 2026-07-20](#076--2026-07-20)
- [0.7.5 — 2026-07-17](#075--2026-07-17)
- [0.7.4 — 2026-07-15](#074--2026-07-15)
- [0.7.3 — 2026-07-15](#073--2026-07-15)
- [0.7.2 — 2026-07-14](#072--2026-07-14)
- [0.7.0 — 2026-07-14](#070--2026-07-14)
- [0.6.0 — 2026-07-11](#060--2026-07-11)
- [0.5.0 — 2026-07-10](#050--2026-07-10)
- [0.4.0 — 2026-07-10](#040--2026-07-10)
- [0.3.0 — 2026-07-10](#030--2026-07-10)
- [0.2.1 — 2026-07-08](#021--2026-07-08)
- [0.2.0 — 2026-07-08](#020--2026-07-08)
- [0.1.1 — 2026-07-07](#011--2026-07-07)
- [0.1.0 — 2026-07-03](#010--2026-07-03)





















## [0.18.12] — 2026-08-31

`v0.18.11` (`15.8m`) eliminó el envenenamiento `sccache 107 B` pero `test-windows 510s` aún paga `save 1.1 GiB 57s` sin beneficio neto (`build 371s` ya hereda `1.1 GiB` del `pipeline` anterior). Esta patch omite `sccache_save_cache` por completo en `test-*`/`coverage`/`validate`/`publish-metadata` y deja solo `build-*` `841,1051,1180,1343` con `save` para alimentar el siguiente `pipeline`; `test-*` restauran el `1.1 GiB` del `pipeline` anterior, ahorra `~57s` en `test-windows` y `~30s` en `test-linux`/`macos` y debería acercar el `wall-clock` al óptimo `v0.18.8 13.9m` con `triple puerta` intacta.

### Cambiado

- `ci`: `test-linux/windows/macos`, `coverage`, `validate-licenses`, `validate-changelog` y `publish-metadata` ya no guardan `sccache`; solo `build-linux-x64/arm64`, `build-windows-x64` y `build-darwin-arm64` lo guardan — `.circleci/config.yml`.

## [0.18.11] — 2026-08-31

`v0.18.10` (`17.1m`) mostró que revertir `sccache_save_cache_conditional` solo en `test-*` arregla `build-windows` (`10.2m→5.0m`) pero `test-windows` pasa a `670s 11.2m` por `save 1.1 GiB 57s` sin beneficio. El condicional en `coverage/validate` también añade complejidad sin aportar (`save 1 GiB` ahí no envenena `build`). Esta patch elimina `sccache_save_cache_conditional` por completo — `sccache_save_cache` incondicional en `test-*`/`coverage`/`validate`/`publish-metadata` — volviendo a `heterogéneo puro` `8a94363+9116480` que dio `13.9m` con `triple puerta` intacta y `sccache` no envenenado.

### Corregido

- `ci`: `sccache_save_cache_conditional` removido, `coverage/validate/publish-metadata` vuelven a `sccache_save_cache` incondicional — `.circleci/config.yml:610,642,665,1608`.

## [0.18.10] — 2026-08-31

`v0.18.9` (`19.8m`) mostró `test-windows` con `hit 88.77%` vaciando `1.1 GiB→107 B` y `build-windows` restaurando `107 B 53ms` con `miss 100%` compilando `458s` vs `178s` previos, llevando `build 10.2m` vs `5.5m`. Esta patch revierte `sccache_save_cache_conditional` a `sccache_save_cache` incondicional en `test-linux/windows/macos` — `test-*` alimentan `build` del mismo `workflow` y no deben condicionar — mantiene condicional solo en `coverage/validate` donde no alimenta `build`, restaurando `build 5.5m` y `workflow ~14m` sin perder el ahorro `heterogéneo`.

### Corregido

- `ci`: `test-linux/windows/macos` vuelven a `sccache_save_cache` incondicional para no envenenar `build-windows` (`107 B`→`1.1 GiB`), `coverage/validate` mantienen condicional `85%` — `.circleci/config.yml:390,464,536`.

## [0.18.9] — 2026-08-31

`v0.18.8` (`13.9m`) equilibró `test-linux` con `target` (`195s`) y `test-windows` sin `target` (`413s`), pero `docs/BUILD.md` seguía describiendo cacheo uniforme, `SKILL.md` permanecía en `0.18.6` con pipeline homogéneo, `CASK` limpiaba `Chatterbox` muerto y `CONTRACT` prometía `faster-whisper` inexistente, y `THIRD-PARTY-LICENSES` declaraba `0.18.1`. Esta patch corrige todos los drifts documentales y de comentarios inline a la realidad heterogénea `0.18.8` y añade guard `sccache 85%`/`CASK zap` para que la suite falle si regresa, dejando `AGENTS.md` con política `timeout 300s` para `cargo test --all`.

### Corregido

- `docs`: `BUILD.md` heterogéneo `target-v2` solo `linux/coverage` + `sccache_save_cache_conditional 85%`, `SKILL.md` `0.18.6→0.18.8`, `CASK` `Chatterbox→Qwen/parakeet/opus-mt/xet`, `CONTRACT`/`commands` purga `faster-whisper`/`Chatterbox` — `docs/BUILD.md:320, SKILL.md:19, xtask:27`.
- `ci`: `AGENTS.md`/`CLAUDE.md` política `timeout 300s` para `cargo test --all` vs `120s` por defecto — `AGENTS.md:151`.
- `tests`: `xtask` `11/11` con `test_cask_zap_no_chatterbox` y `test_pipeline_heterogeneo_y_sccache_condicional` que afirman `pipeline` heterogéneo y `zap` real — `crates/xtask/src/main.rs:871`.

## [0.18.8] — 2026-08-31

`v0.18.7` (`14m36s`) mejoró `test-windows` (`-2.5m`) al quitar `target`, pero `test-linux` empeoró (`+57s`) por `hit Rust 15% 230 miss` sin `target` — `restore 8s` barato en `docker ext4` ahorra `50s` de `compile`. Esta patch heterogeneiza: reintroduce `target-v2` solo en `test-linux/coverage` donde `coste < beneficio`, mantiene `registry+sccache` en `test-windows/macos` donde `upload 235s NTFS` domina, equilibrando ambos sin tocar validaciones.

### Cambiado

- `ci`: heterogeneiza `target` — `test-linux`/`coverage` restauran `target-v2 test/cov` `8-15s`, `test-windows/macos` quedan `registry+sccache` — ` .circleci/config.yml:315,523`.

## [0.18.7] — 2026-08-31

El pipeline `v0.18.6` (#111 `17m26s`) mostraba `test-windows` como cuello con `9m53s` (`56%` del total) por el artefacto derivado `target` (`770 MiB restore` + `831 MiB save` `235s` `NTFS`) que duplicaba `sccache` (`1.0 GiB`, `82-97%` hit) y contenía `first-write-wins` entre los 3 `test-*` paralelos. Esta patch elimina `target-v2` de `test-*/coverage/validate` y deja solo `registry+sccache` content-addressed, coherente con el `hit-rate` ya medido y sin tocar la triple puerta ni los gates de licencias; `build-*` conservan `target-v2-full` para `release`.

### Cambiado

- `ci`: elimina `target` cache de `test-linux/windows/macos`, `coverage` y `validate`, deja `cargo registry + sccache` (`1 GiB`) como única caché de compilación en `test` — ` .circleci/config.yml:140,315,394,456,518`.

## [0.18.6] — 2026-08-30

La limpieza manual de `hub` sin `xet` dejaba `shard-cache` huérfano y el siguiente `setup` marcaba éxito en `1s` con `parakeet` solo `vocab.txt` y `blobs/*.incomplete`, validado como `is_provisioned` y `doctor ok` falso que derivaba en `daemon_unreachable`; además `cleanup`/`uninstall` dejaban `daemon.pid` vivo y no purgaban `xet`. Esta patch endurece la provisión a validación por ficheros críticos y atomicidad con rollback, purga `hub+xet` y detiene el daemon en `cleanup`/`uninstall` en Windows, Linux y macOS, y refina la skill E2E a purga inicial desde cero para un gate determinista.

### Corregido

- `store`: `is_provisioned` exige ficheros críticos `MODEL_FILE_PATTERNS` con `size>0` y `ensure_downloaded` valida/rollback de `snapshot`+`blobs` — `crates/avi-store/src/lib.rs:529,636`.
- `store`: `xet_cache_dir` y `remove_xet_cache` (+ `locks`) para coherencia `hub`/`xet` — `crates/avi-store/src/lib.rs:446,618`.
- `cli`: `cleanup`/`uninstall` paran daemon graceful `5s` + `remove_daemon_pid_file` y purgan `hub+xet+temp` — `src/main.rs:1387,1471`.
- `skill`: `test-windows-e2e-as-final-user` `0.18.6` con `0a Limpieza inicial` desde cero y `xet` — `.claude/skills/test-windows-e2e-as-final-user/SKILL.md`.
- `docs`: `SELF-HOSTED-INSTALL.md` con desinstalación `hub+xet+daemon` y contrato `install → setup reintentable`.

## [0.18.5] — 2026-08-30

El daemon instalado quedaba en `warm_failed` con `No está provisionado` y simulaba cuelgue al lanzarse desde un `CWD` distinto al `install_dir`, porque `resolve_binary` solo miraba `<cwd>/vendor` y `PATH` y no `<exe_dir>/vendor` donde vive `qwen_tts.exe` vendido; además el skill `test-windows-e2e-as-final-user` prescribía `daemon start` con pipe capturado (`| Out-String`/`Start-Job`) que hereda el write-end y deja al padre colgado 10s. Esta patch corrige la resolución a `env → exe_dir/vendor → cwd/vendor → PATH/HF` (con fallback HF para `qwen3-tts-0.6b`) y el skill a lanzamiento detached sin pipe con `Start-Process` y poll de `/health`, desbloqueando `warm: warm` desde cualquier `CWD` sin `QWEN3_TTS_BIN`.

### Corregido

- `tts`: resuelve `qwen_tts` y modelos `qwen3-tts-0.6b{,-base}` relativo a `current_exe` antes que `cwd` y añade fallback HF para `qwen3-tts-0.6b` — `crates/avi-tts/src/lib.rs:151-176,182-240,253-262`.
- `skill`: corrige drift en `test-windows-e2e-as-final-user` — `T3` con `Start-Process` detached sin pipe y `RedirectStandardOutput` + poll `/health`, overview a `0.18.5` y constraint contra pipe capture — `.claude/skills/test-windows-e2e-as-final-user/SKILL.md`.

## [0.18.4] — 2026-08-30

El pipeline `v0.18.3` (#100) finalizó en `success` con los cachés `tts-v1`/`ort-v1` guardados correctamente, pero el step `Generar .engine-cachekey` emitía tres líneas `File not found - *.c` por el sombreado del `find` de Windows (`System32`) sobre el de MSYS2 y el uso de `xargs` sin `-r`. Esta patch fuerza `/usr/bin/find` + `/usr/bin/sort` + `xargs -r` para un agregado silencioso y determinista, eliminando el ruido sin alterar la clave.

### Corregido

- `ci`: elimina ruido `File not found - *.c` en `Generar .engine-cachekey` forzando `/usr/bin/find`/`sort`/`xargs -r` — `.circleci/config.yml:844`.

## [0.18.3] — 2026-08-30

El pipeline `build-all` en `v0.18.2` (#97, 17m31s) estaba dominado por `build-windows-x64` (10m08s) y por una doble ejecución encadenada de la suite en `coverage` (4m33s) que mantenía el gate en 5m56s, además de dos pasos sin caché en Windows que re-descargaban ONNX Runtime y recompilaban el motor TTS en cada tag. Esta patch corrige la cobertura para rehusar los `.profraw` con `cargo llvm-cov report` y añade dos cachés con claves estables (`ort-v1` por `ort_version` y `tts-v1` por `.engine-cachekey` + pines de toolchain), reduciendo el wall a ~13m50s sin invalidar cachés `v2` vigentes y sin tocar `sccache`/`CARGO_INCREMENTAL`.

### Cambiado

- `ci`: corrige doble ejecución en `coverage` — `cargo llvm-cov report` en vez de segunda corrida completa (`273199` → ~3m50s, -~25s de wall) — `.circleci/config.yml:540`.
- `ci`: cachea `ort-bundle` con `ort-v1-win-x64-<< pipeline.parameters.ort_version >>` y guarda tras el bundling, con guarda `Test-Path onnxruntime.dll` — `.circleci/config.yml:884`.
- `ci`: cachea `qwen_tts.exe` con `tts-v1-{{ arch }}-gcc<<...>>-ob<<...>>-{{ checksum ".engine-cachekey" }}` y generación determinista de `.engine-cachekey` — `.circleci/config.yml:844`.
- `ci`: parametriza `ORT_VERSION` en los 4 builds vía `pipeline.parameters.ort_version` — `.circleci/config.yml:1077,1206,1370`.
- `docs`: actualiza `BUILD.md` con matriz de invalidación para `ort-bundle`/`tts` y pines reproducibles.

## [0.18.2] — 2026-08-28

La auditoría sistémica post-`v0.18.1` (pipeline #96 verde) reveló divergencia entre la documentación y el binario Rust: `speech synthesize` prometía flags `compute-backend` y métricas `t3_time` inexistentes, `voice clone` documentaba modo daemon sin implementación, `DAEMON.md` describía `FastAPI/uvicorn` legacy, y guías de validación referenciaban artefacto `setup.exe` y ejemplos `0.15.1`. Esta release sincroniza la doc al código verificado (`src/main.rs`, `crates/avi-daemon`), implementa el despacho `voice clone` 3-modos decidido y restaura `push-to-talk` en `transcribe/dub`, sin reintroducir superficie Python/Chatterbox.

### Corregido

- `src/main.rs:470` `voice clone` ahora respeta `--daemon/--no-daemon` (3 modos, `ForceDaemon` → `DaemonUnreachable` si no hay daemon, `Auto` con aviso de modelo caliente).
- `src/main.rs:681,937` `speech transcribe`/`dub` permiten `--mic` sin `--duration` en TTY (push-to-talk), exigen `--duration` solo sin TTY.
- `THIRD-PARTY-LICENSES.md:88` `0.13.0` → `0.18.1` y `README.md:103`/`docs/DESIGN.md:84` `0.15.1` → `X.Y.Z` placeholder.

### Cambiado

- `docs/CLI/CONTRACT.md:167` y `USAGE.md:177` corrigen contrato `speech synthesize/say` al payload real (`status/audio_path/voice`) y eliminan flags inexistentes.
- `docs/CLI/commands/DAEMON.md` reescrito a `Axum` (5 subcomandos, `/health`, `spawn_background` con `CREATE_NO_HANDLE_INHERIT`).
- `docs/MANUAL-VALIDATION.md:15` y `docs/GOAL.md:162` artefactos `setup.exe` → `zip/tar.gz`; `docs/BUILD.md:414` documenta `log on drift` de gcc (`v0.17.1`); `docs/CLI/README.md:34,79` y `CLAUDE.md:145` referencias muertas.

## [0.18.1] — 2026-08-28

El job `test-windows` fallaba de forma determinista en `daemon_status_coincide_con_fixture` con `panic` en `read_to_string` del `tempfile`, solo en `win/server-2022`, mientras `test-linux` y `test-macos` pasaban. La causa no estaba en `daemon status` sino en el harness `tests/cli_golden.rs:run_json`: `File::create` trunca sin atomicidad y, con la resolución gruesa de `SystemTime` en Windows más `cargo test` en paralelo, dos hilos generaban el mismo `tmp` y el `remove_file` de uno borraba el del otro. Esta patch reemplaza la creación por `OpenOptions::create_new(true)` (`O_CREAT|O_EXCL`) con reintento atómico, garantizando exclusión del SO sin añadir dependencia ni sobreingenierizar el harness.

### Corregido

- `tests/cli_golden.rs:run_json` pasa de `File::create` a creación atómica `O_EXCL` con contador y reintento en `AlreadyExists`, eliminando la carrera de `tempfile` que hacía fallar `daemon_status` solo en Windows.

## [0.18.0] — 2026-08-28

El checksum que formaba la clave exacta de las cachés de `cargo registry` y
`target/` en CI incluía la versión del propio crate dentro de `Cargo.lock`.
Como cada release bumpea esa versión, la clave cambiaba en cada corte aunque
las dependencias no variaran: nunca había acierto exacto entre releases y todo
dependía de un fallback por prefijo frágil, anclado a una caché vieja y casi
vacía, que forzaba recompilación en frío de las dependencias nativas pesadas y
builds de decenas de minutos. Esta release deriva la clave de un `Cargo.lock`
normalizado (versión del crate neutralizada) generado por un transform de texto
(`perl`, en `shell: bash`) en el primer step de `cargo_restore_caches`, y re-keya
`registry` y `target/` a un namespace limpio `v2`:
la clave exacta se vuelve estable entre releases y solo cambia ante cambios
reales de dependencias, restaurando el reuso de caché de forma predecible.

### Cambiado

- Deriva la clave de caché de `registry`/`target/` de un `Cargo.lock`
  normalizado (versión del crate neutralizada, generado por un transform de
  texto `perl` en vez de compilar `xtask` desde cero) en vez del `Cargo.lock`
  crudo, re-keyando ambas familias a `v2`; `toolchain`/`msys2`/`sccache`
  permanecen en `v1`.

## [0.17.1] — 2026-08-28

El guardrail de versión de gcc en el job `build-windows-x64` abortaba el build cuando la versión que `pacman -Sy` instalaba desde MSYS2 UCRT64 difería del pin declarado. Como `pacman -Sy` sincroniza siempre la versión vigente mientras el pin es una constante fija, cada avance de gcc upstream rompía el release y obligaba a un bump manual del parámetro para desbloquearlo. Esta release cambia el comportamiento a "log on drift": ante un desfase se advierte y se continúa, tomando la versión realmente instalada como fuente de verdad de la evidencia auditable de la GCC Runtime Library Exception, eliminando el mantenimiento manual obligatorio sin perder trazabilidad del compilador.

### Cambiado

- El bootstrap de MSYS2 en `build-windows-x64` ya no aborta cuando gcc difiere del pin declarado: registra una advertencia y continúa, usando la versión instalada como evidencia RLE. El pin se conserva como referencia esperada y clave de caché.

## [0.17.0] — 2026-08-28

El motor TTS `qwen_tts` se producía de forma inconsistente: los tres jobs Unix de CircleCI compilaban desde fuente con un bloque triplicado idéntico, mientras que Windows no compilaba y arrastraba un binario precompilado versionado en git (`vendor/qwen3-tts/qwen_tts.exe`, 33 MB) de origen opaco. Esta release unifica la producción del motor tras una única interfaz `cargo run -p xtask -- build-engine --self-test`, usada por los cuatro jobs de build: elimina la triplicación, hace que Windows compile fresco desde fuente, centraliza la política SIMD y convierte a `xtask` en la fuente única reutilizable por CI, dev local y documentación.

### Añadido

- Subcomando `xtask build-engine` (`--self-test`/`--simd`/`--jobs`) como driver uniforme de compilación del motor en las 4 plataformas, encapsulando el mecanismo por SO (Unix: `make`; Windows: `mingw32-make` con entorno UCRT64 augmentado) — `crates/xtask/src/main.rs`.

### Cambiado

- CI: los tres bloques Unix de compilación del motor se colapsan en un único step; `build-windows-x64` aprovisiona MSYS2 pineado + cacheado y compila el motor desde fuente — `.circleci/config.yml`.
- Baseline SIMD portable en ARM64 (aarch64): la rama ARM del `Makefile` honra `SIMD` con default `-march=armv8-a` (antes `-march=native`, que acoplaba el binario a la microarquitectura del runner y arriesgaba `SIGILL` en CPUs más antiguas) — `vendor/qwen3-tts/Makefile`.

### Notas de release

- El binario del motor deja de versionarse: `vendor/qwen3-tts/qwen_tts.exe` fue eliminado del árbol y **purgado del historial de git** (reescritura con `git filter-repo` + force-push a `main`). Los clones y ramas existentes deben re-clonar o rebasarse sobre el nuevo historial.

## [0.16.0] — 2026-08-27

Esta release corrige tres defectos de empaquetado que bloqueaban la validación E2E del daemon en Windows: onnxruntime.dll fallaba con ERROR 1114 por runtime VC++ VS2017 incorrecto, qwen_tts.exe ausente del release zip, y ruta del modelo STT CWD-relative vacía en vez del cache HuggingFace. El bump también elimina artefactos de sesión (prompt de continuidad desplazado, script de humo temporal).

### Corregido

- CRT: fuente de VC++ runtimes desde onnxruntime-win-x64 zip (`Microsoft.VC14[34]\.CRT` = VS2022+) en vez de VS Redist/System32 (VS2017 14.16 → ERROR 1114 en onnxruntime.dll).
- `qwen_tts.exe`: paso de empaquetado que copia `vendor/qwen3-tts/qwen_tts.exe` al stage del release.
- Ruta modelo STT: `ModelStore::model_dir("parakeet-tdt-v3")` (HF cache) en lugar de `models/parakeet-tdt-v3` CWD-relative; eliminada const muerta `STT_MODEL_DIR` en `src/main.rs` y `crates/avi-daemon/src/lib.rs`.
- `crates/avi-store/src/lib.rs`: eliminada función `model_dir()` asociada duplicada (error de compilación preexistente).
- `crates/avi-stt/src/lib.rs`: guards de tests Parakeet verifican existencia de `nemo128.onnx` (skip limpio en vez de panic).

### Cambiado

- `docs/session.md` eliminado — el prompt de continuidad vive en `.claude/continuity-prompt.md` por contrato del skill.
- `_smoke.ps1` eliminado — script de humo temporal tras validación end-to-end del daemon.

## [0.15.2] — 2026-08-27

La auditoría del 2026-08-27 contrastó el ground truth Rust 0.15.1 (Qwen3-TTS 0.6B + Parakeet TDT v3 int8 `ort` + CTranslate2 `ct2rs` + `cpal` + `tar.gz/zip`) contra 17 documentos y halló 20 drifts: `docs/BUILD.md` describía `whisper.cpp/GGUF` y `libclang` incondicional, `docs/DESIGN.md` diagramaba `whisper.cpp` en 0.10.7, `docs/CLI/CONTRACT.md` citaba rutas Python inexistentes y `SECURITY.md` modelaba `PyPI`. Esta patch reescribe cada documento al estado canónico sin notas históricas, condiciona `libclang` a `native-translation/full` y limpia artefactos `AppImage/PyInstaller`.

### Corregido

- `docs/BUILD.md`, `docs/DESIGN.md`, `docs/CLI/CONTRACT.md`, `SECURITY.md`, `USAGE.md`, `README.md`, `docs/GOAL.md`, `docs/MANUAL-VALIDATION.md`, `CONTRIBUTING.md`: sincronizados al stack Parakeet/`ct2rs`/`cpal` 0.15.1 y pesos `~9GB` base.
- `.circleci/config.yml`, `.cargo/config.toml`, `.gitignore`, `docs/DISTRIBUTION.md`, `docs/SELF-HOSTED-INSTALL.md`: comentarios y artefactos `whisper/AppImage/patchelf` a `ct2rs/ort`.

### Eliminado

- `.npmignore` (23 líneas) — reglas `src/tests/docs/.circleci` del canal `npm` retirado en Fase 7.

## [0.15.1] — 2026-08-27

El `cargo xtask release` anterior mutaba antes de validar el rango, dejando el working tree a medio bumpear cuando el rango estaba vacío. Este patch hace el corte atómico en ejecución —pre-valida árbol limpio y `v{last}..HEAD` antes de tocar archivos— y refina la skill `/release` para ser descubrible pero exigir confirmación explícita antes de `tag/push`.

### Corregido

- `crates/xtask/src/main.rs`: pre-validación atómica en `Commands::Release` que aborta sin mutar si el árbol está sucio o el rango `v{last}..HEAD` está vacío.
- `.claude/skills/release/SKILL.md`: descripción ampliada para descubrimiento ante `pipeline de tag fallido` y gate de confirmación explícita antes de bump/tag/push.

## [0.15.0] — 2026-08-27

El corte manual exigía tres acciones desacopladas (bump, regeneración de `SOURCE-OFFER.md` y `push --tags`) que se olvidaban y forzaban re-tagueos (pipelines `v0.13.0`/`v0.14.0` fallidos). Esta minor vuelve atómico el bump dentro de `cargo xtask release` —incluida la oferta GPLv3— y guía el flujo vía skill `/release`, eliminando la causa raíz de los re-tagueos.

### Añadido

- `crates/xtask/src/main.rs`: `bump_version` ahora escribe `SOURCE-OFFER.md` vía `render_source_offer` en la misma transacción del bump; `release` lista el artefacto entre los generados.
- `.claude/skills/release/SKILL.md`: skill orquestadora que valida `X.Y.Z`, ejecuta `xtask release`, guía la curación de `TODO`, valida gates y ejecuta `commit`/`tag`/`push --tags`.

## [0.14.0] — 2026-08-27

Incorporación del subcomando `cargo xtask release X.Y.Z` que une atómicamente el
bump de versión (src/main.rs, Cargo.toml, Cargo.lock, cli_version.json) con el
corte del borrador de CHANGELOG.md, pre-rellenado desde `git log` y dejado con
marcadores `TODO: curar` para que el humano cierre la narrativa editorial sin
reescribir desde cero. Añadido el job `validate-changelog` en CircleCI como
defensa temprana ante un tag creado sin su sección de CHANGELOG.

### Añadido

- `crates/xtask/src/main.rs`: subcomando `release X.Y.Z` con `bump_version`
  (regex dirigido, conserva el patrón de `get_version()`) y `scaffold_changelog`
  (último tag vía `git describe`, commits con `git log --pretty=format` y cuerpo
  parseado para extraer `Resumen de cambios:`; sección `## [X.Y.Z] — fecha`,
  entrada en el TOC y definición de enlace al final).
- `crates/xtask/src/main.rs`: subcomando `changelog --check` que falla con
  mensaje explícito si falta la sección `## [X.Y.Z]` para la versión de
  `Cargo.toml`.

### Cambiado

- `docs/RELEASING.md`: el prerequisito de corte manual del CHANGELOG se
  reemplaza por `cargo run -p xtask -- release X.Y.Z`.
- `.circleci/config.yml`: job `validate-changelog` requerido por los 4 builds;
  el `awk` de `publish-release` se mantiene como segunda barrera.

## [0.13.0] — 2026-08-27

Fix estructural del arranque del daemon: readiness inmediato tras enlazar el
puerto y warmup en segundo plano, desacoplando *readiness* (enlazado + motor
construido) de *warm* (pre-calentamiento). Elimina de raíz el acoplamiento
`warmup→bind` que retrasaba el `bind` hasta terminar una síntesis TTS real.

### Cambiado

- `crates/avi-daemon/src/lib.rs`: `run_daemon_server` enlaza el `TcpListener`
  antes del warmup; el warmup real corre en `tokio::task::spawn_blocking` sin
  bloquear el arranque. Un warmup fallido degrada (`warm_failed`) pero no derriba
  el daemon: sigue sirviendo y la primera petición paga cold-start.
- `src/main.rs`: el sondeo de arranque (`await_daemon_ready`) espera solo a que el
  daemon sea alcanzable (spawn+bind acotado), sin el deadline dimensionado para la
  síntesis; `daemon status` expone el estado `warm` para diagnóstico.

### Añadido

- Estado `warm` (`warming`/`warm`/`warm_failed`, con `warm_error` cuando falla) en
  `DaemonState`, expuesto en `/health` y en `daemon status`.
- Los tres E2E de daemon (`daemon_start_exito`, `daemon_restart_rearma`,
  `daemon_status_running`) vuelven a la suite por defecto (des-ignorados) al dejar
  `start` de pagar síntesis real.

### Corregido

- Depuración de warnings de clippy en `avi-tts`, `avi-stt`, `avi-translation` y
  `xtask` (default y `--all-features`): `doc_lazy_continuation`,
  `needless_range_loop`, `field_reassign_with_default`, dead code, entre otros.

### Notas de release

- **BREAKING**: `/health` cambia de `{status:"healthy"}` a
  `{status:"ready", warm, engine}`. El único consumidor es el CLI raíz, alineado
  en el mismo release.

## [0.12.0] — 2026-08-27

Arranque en segundo plano fiable del daemon: se corrige el falso negativo de
`daemon start`/`restart` (el sondeo se agotaba antes de que el warmup en frío
terminara, devolviendo `daemon_unreachable` con el daemon sano) y se añade
`spawn_background` desacoplado del proceso padre, con el modelo Base de clonado
resuelto desde el snapshot HF de `setup --with-base`.

### Añadido

- `crates/avi-daemon/src/spawn.rs`: `spawn_background` (Stdio nulo +
  `DETACHED_PROCESS` en Windows, `setsid` en Unix) para desacoplar el arranque en
  segundo plano del proceso padre.
- `crates/avi-tts/src/lib.rs`: capa de resolución HF en `resolve_base_model_dir`
  para el modelo Base de clonado desde el snapshot descargado por
  `setup --with-base`.
- `crates/avi-store/src/lib.rs`: pin auditable del modelo Base
  `qwen3-tts-0.6b-base` verificado + test.

### Corregido

- `src/main.rs`: `daemon start`/`restart` esperan readiness contra un deadline por
  reloj de pared (techo 60 s, retorno inmediato al primer `/health` sano),
  eliminando el falso negativo `daemon_unreachable`; `daemon.pid` se persiste solo
  tras confirmar readiness (sin PID stale ante un warmup fallido).

### Notas de release

- Los E2E de daemon quedan `#[ignore]` en el CI por defecto (disponibles bajo
  `--ignored`); el modelo Base de clonado pesa ~2,5 GB.

## [0.11.3] — 2026-08-26

Aislamiento de caches por SO en CircleCI: `sccache`/`target`/`toolchain` OS-específicos para evitar 0% hits en Windows (40m → ~2m).

### Corregido

- `.circleci/config.yml:34-130`: claves `sccache-v1-{{ arch }}-...`, `target-v1-...`, `toolchain-v1-...` ahora incluyen `<< parameters.os >>` (linux/windows/macos).

## [0.11.2] — 2026-08-26

Sincronización de fixture dorada `version` tras bump `0.11.1` (el tag anterior falló por golden desactualizada).

### Corregido

- `tests/golden/cli_version.json:4` `0.11.0` → `0.11.2` para alinear con `const VERSION` y `Cargo.toml`.

## [0.11.1] — 2026-08-26

Reconciliación de residuos de la migración STT `whisper/GGUF` → **Parakeet TDT 0.6B v3** (post-0.11.0).

### Corregido

- `src/main.rs:1503-1504` (`doctor`): verificación de `whisper-gguf` ausente en `MODEL_REVISIONS` → `parakeet-tdt-v3`; `doctor` ya no reporta falso negativo en instalación sana.
- `tests/cli_golden.rs:354-368` y helpers: `wer_vs_texto` de `Ct2SttEngine` (`whisper-rs`) → `ParakeetEngine` (`ort`); `whisper_model_disponible()` → `parakeet_model_disponible()` y skips actualizados — `cargo test --features native-stt` vuelve a compilar.

### Documentado

- `README.md:45,132,219-220`, `USAGE.md:120,260,329,563,606,814,925,999`, `CONTRIBUTING.md:23`, `.cargo/config.toml:11-15`, `THIRD-PARTY-LICENSES.md:27`: descripciones, tablas y comentarios de `whisper/GGUF` → `Parakeet TDT v3`.

## [0.11.0] — 2026-08-26

Motor de transcripción STT migrado de `whisper-rs`/`whisper.cpp` (formato GGUF)
a **Parakeet TDT 0.6B v3 int8** vía ONNX Runtime (feature `native-stt`, opt-in).
El crate `ort` opera en modo `load-dynamic`: la librería ONNX Runtime se empaqueta
junto al binario en los artefactos de release, manteniendo la app autocontenida.

### Añadido

- Motor STT nativo Parakeet TDT 0.6B v3 int8 (`crates/avi-stt/parakeet.rs`) sobre
  `ort =2.0.0-rc.13` ↔ ONNX Runtime 1.28.0 en modo `load-dynamic` (carga la
  librería dinámica en runtime desde el directorio del binario). Pipeline
  `nemo128 → encoder → decoder_joint` con decodificador TDT greedy; la salida
  `encoder_outputs` se consume con layout `[B, 1024, T']` sin transponer,
  `targets`/`target_length` como int32, y los estados LSTM del predictor se
  indexan por nombre `output_states_1`/`2`. Guardia heurística de idioma
  (`detectar_idioma`, `EN-SOSPECHOSO`) y campo aditivo `language_warning` en
  `/transcribe`.
- CLI raíz (`src/main.rs`): `speech transcribe`/`speech dub` usan ParakeetEngine
  in-process; Parakeet no necesita chunking VAD (RTF ~0.11 lineal sobre audio >22 s).
- Empaquetado de la librería ONNX Runtime 1.28.0 junto al binario en los cuatro
  jobs de build de CircleCI (`onnxruntime.dll` / `libonnxruntime.so` /
  `libonnxruntime.dylib` según plataforma). En Windows se incluyen además las
  DLLs del runtime VC++ (`vcruntime140.dll`, `vcruntime140_1.dll`, `msvcp140.dll`)
  para que la app sea autocontenida sin exigir el Visual C++ Redistributable.
- `crates/avi-store`: `MODEL_REVISIONS` → `parakeet-tdt-v3`
  (sha `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`).

### Cambiado

- `crates/avi-stt/Cargo.toml` y `crates/avi-daemon/Cargo.toml`: `whisper-rs`
  eliminado; feature `native-stt = ["dep:ort"]` (off por defecto).
- Docs: `docs/BUILD.md` documenta el consumo de ONNX Runtime vía `load-dynamic`
  y su empaquetado en release; el drift documental legacy en `docs/CLI/` y
  `docs/DAEMON-MODE.md` se preserva (subsistema Python).

### Notas de release

- WER real ~0.08–0.21 sobre corpus de voz sintética (umbral de paridad ≤0.25);
  el fixture `whisper_sample` (saludo breve) se emite en inglés y dispara la
  guardia de idioma → `EN-SOSPECHOSO`.
- `native-stt` es **opt-in**: `cargo build --release --features native-stt`;
  el build featureless no compila ONNX Runtime (R1 — tests/CI sin C++).

## [0.10.7] — 2026-08-24

Optimización de la infraestructura de CI (CircleCI). No hay cambios de contrato
ni de comportamiento del CLI ni del binario distribuido: los `build-*` siguen
compilando con `--features full` y el gating de los engines nativos es idéntico.
Los artefactos son funcionalmente idénticos a 0.10.6.

### Cambiado

- **Clave de caché `target` discriminada por variante de feature set**
  (`test`/`full`/`cov`): la clave `target-v1-…` codificaba arquitectura,
  toolchain y `Cargo.lock`, pero no el feature set, de modo que los jobs
  featureless (`test-linux`, `coverage`) y los `build-*` con `--features full`
  colisionaban en amd64 sobre una clave inmutable (first-write-wins). El job
  featureless ganaba la carrera y dejaba un `target/` sin C++ que obligaba al
  build a recompilar ~275 crates y los tres stacks C++ desde cero (pipeline #15:
  `build-linux-x64` 17m26s, `build-windows-x64` 38m56s vs `build-linux-arm64`
  1m20s). Cada familia de job restaura y guarda ahora su propia clave, y de paso
  el `target/llvm-cov-target` de `coverage` persiste bajo clave propia (antes
  nunca se cacheaba). La clave de registry (`cargo-v1-…`) permanece sin variante.
- **`sccache` como `RUSTC_WRAPPER` con directorio cacheado por arquitectura**:
  los objetos Rust se reutilizan por contenido entre corridas y variantes,
  sobreviviendo a bumps de `Cargo.lock` que invalidan `target/`. Envuelve
  `rustc`, no las builds CMake de whisper.cpp/CTranslate2; `sccache --show-stats`
  deja el hit-rate visible en el log. `CARGO_INCREMENTAL=0` acompaña al wrapper.
- **Caché de toolchain (`~/.rustup`, `~/.cargo/bin`)**: los executors
  Windows/macOS dejan de reinstalar el toolchain con rustup-init en cada corrida
  y `coverage` restaura `cargo-llvm-cov` en vez de recompilarlo desde fuente.

### Añadido

- **Workflow `branch-checks` de feedback temprano en rama**: un job `lint`
  (`cargo fmt --check` + `cargo clippy` sin features) y `test-linux` corren en
  pushes de rama, rompiendo el anti-patrón por el que el contrato tags-only
  difería todo el feedback al momento de etiquetar. El workflow `build-all`
  permanece tags-only e intacto como única puerta del release.

## [0.10.6] — 2026-08-24

### Cambiado

- **Engines nativos ahora opcionales tras Cargo features** (`native-stt` y
  `native-translation`, off por defecto): `whisper-rs` (whisper.cpp) y `ct2rs`
  (CTranslate2 + oneDNN) pasan a dependencias opcionales, de modo que
  `cargo test` y `cargo llvm-cov` compilan la workspace sin C++. Los builds de
  release activan `--features full`, por lo que el binario distribuido conserva
  STT y traducción con comportamiento byte-idéntico: el gating es en tiempo de
  compilación, de coste cero en runtime. Con esto el job `coverage` deja de
  morir por `no_output_timeout` mientras compilaba en silencio los stacks
  nativos, desbloqueando la cadena de release.
- **CI reproducible**: la imagen `cimg/rust:1.96.0` queda fijada por digest en
  sus cuatro referencias y las claves de caché de registry y target incorporan
  `rust_version`, para no mezclar imágenes ni artefactos de toolchains distintos.

## [0.10.5] — 2026-08-11

### Corregido

- **Provisión del modelo de traducción es↔en fallaba siempre**: el snapshot
  de HuggingFace se descargaba con un `allow_patterns` que omitía
  `vocab.json`, archivo que `TransformersConverter` necesita para construir
  el `MarianTokenizer` del par opus-mt. Sin él, la conversión CT2 abortaba
  con `TypeError: expected str, bytes or os.PathLike object, not NoneType`.
  `allow_patterns` ahora incluye `vocab.json` y `tokenizer_config.json`,
  cubriendo ambas direcciones (es→en y en→es).
- **El instalador de Windows reportaba "Instalación completa" pese a que la
  provisión de modelos hubiera fallado**: `install-windows.ps1` invoca
  `setup` como proceso nativo, y PowerShell no aborta automáticamente ante
  un exit code distinto de cero de un `.exe` (a diferencia de los
  instaladores Unix, que ya abortaban por `set -eu`). `Invoke-AIVoiceInterConnectorSetup`
  ahora comprueba `$LASTEXITCODE` y advierte de forma reintentable en vez de
  reportar éxito, sin abortar la instalación del binario.

## [0.10.4] — 2026-08-10

### Corregido

- **`setup --uninstall` roto en instalaciones nativas de Windows**: el
  instalador Inno Setup se generaba con el `AppId={{E8A1B2C3-...}}` de doble
  llave final, que Inno Setup no escapa: la clave de desinstalación quedaba
  como `{E8A1B2C3-...}}_is1` y la búsqueda de `cli.py` no la encontraba,
  abortando con exit 7 pese a una instalación legítima (había que ir a
  Configuración → Aplicaciones). El `AppId` ahora cierra con una sola llave:
  la clave `{AppId}_is1` coincide y `setup --uninstall --yes` vuelve a
  desinstalar en un comando en las instalaciones futuras.
- **Tests de Windows frágiles al orden de selección (numpy + `sys.platform`
  parcheado)**: los tests que simulan Linux parcheando `sys.platform`
  (p. ej. `TestSetupUninstall`) podían importar numpy por primera vez dentro
  de esa ventana, y numpy 2.x llama `os.uname()` cuando `sys.platform` es
  `"linux"`, atributo inexistente en Windows: 8 tests fallaban con
  `AttributeError` al correr aislados (la suite completa lo enmascaraba por
  el caché de `sys.modules`). `tests/conftest.py` fija
  `NUMPY_MADVISE_HUGEPAGE=0` y la rama `os.uname` queda excluida de raíz.

## [0.10.3] — 2026-08-10

### Optimizado

- **Descarga del language pack es-mx-latam ~1 GB más liviana**: se elimina
  `s3gen_v3.pt` de `MODEL_ALLOW_PATTERNS`. El `.pt` era el mismo checkpoint
  del vocoder S3Gen que `s3gen_v3.safetensors` (~1.008 GB c/u) en formato
  duplicado, y el loader (`model_loader.py`) siempre prefería el safetensors,
  que ya incluye verificación de integridad en la carga. El fallback al `.pt`
  solo se activaba si el safetensors no existía en caché (no ante un archivo
  dañado), así que no aportaba robustez real. El modelo pasa de ~4 GB a ~3 GB;
  con `en` (~3 GB), el total descargado baja de ~8 GB a ~6 GB. `en` mantiene
  `ve.pt` + `ve.safetensors` (~5.4 MB c/u, tamaños menores): no se toca en
  este cambio.

## [0.10.2] — 2026-08-10

### Añadido

- **Descarga ligera de modelos con `allow_patterns`**: `setup` y el engine
  descargan solo los checkpoints de inferencia que el runtime consume, en vez
  del snapshot completo. `MODEL_ALLOW_PATTERNS` (`model_cache.py`) fija los
  archivos por modelo: `es-mx-latam` y `en` dejan de bajar las variantes T3
  no usadas del repo base (varios GB de ahorro), y los modelos de traducción
  opus-mt ya no descargan `tf_model.h5` (~298 MB) ni `flax_model.msgpack`
  (~296 MB), que la conversión CT2 nunca consume.

## [0.10.1] — 2026-08-10

### Corregido

- **`speech transcribe --mic` roto por dependencia ausente**: `miniaudio`
  (backend único de captura de micrófono, CFFI) se añadió en su día solo a
  `requirements.txt` y a los hooks de PyInstaller, pero no a `pyproject.toml`
  (fuente única de runtime), por lo que los lockfiles nunca lo incluyeron y el
  comando fallaba con `ModuleNotFoundError` en producción (los tests usan
  dobles). Se declara `miniaudio>=1.71` en `pyproject.toml` y se regeneran los
  lockfiles: el venv vuelve a instalar `miniaudio==1.71` y la captura queda
  verificada funcionalmente (enumeración de micrófonos y grabación real).
  Bonus de la regeneración: `requirements-lock-linux-cpu.txt` ya no arrastra
  entradas win32/darwin (se generaba con `--universal` en vez del comando
  linux con `--python-platform`).

## [0.10.0] — 2026-08-09

### Añadido

- **`speech dub`, la composición voz→voz en un comando dedicado**: transcribe
  la entrada hablada (`--audio`/`--mic`, mutuamente excluyentes y exactamente
  una requerida), traduce si `--source-language` difiere de
  `--target-language` y sintetiza y reproduce con la voz elegida.
  `say`/`synthesize` no cambian: siguen siendo texto→voz con `--text`
  requerido. `speech transcribe` gana `--daemon`/`--no-daemon` (despacho de
  tres modos, como la síntesis); su `--json` conserva el shape `{text, source}`.
- **`TranscribeRequest` en el IPC del daemon, aditivo sin bump de
  `schema_version`**: `POST /transcribe` recibe las muestras PCM int16 en
  base64 (nunca rutas) y devuelve el texto; `schema_version` permanece en
  `"3"`. Un daemon de versión antigua (404 en `/transcribe`) se reporta como
  daemon inalcanzable (exit 5), sugiriendo actualizar el daemon o usar
  `--no-daemon`, sin degradación silenciosa.
- **Precarga opt-in del modelo de transcripción en el daemon**:
  `daemon start --with-stt` (simétrico a `setup --with-stt`) calienta
  `faster-whisper-small` en RAM con gate de provisión en disco (exit 4 si falta
  `setup --with-stt`), y `/health` lo reporta como `"transcribe:small"` en
  `model_loaded`.

## [0.9.1] — 2026-07-31

### Corregido

- **Doble emisión JSON en el canal `--json` (una clase de defecto, no un caso
  aislado)**: cuatro comandos (`doctor` y `daemon start/stop/restart`) violaban la
  promesa de `emit_json()` de «exactamente un objeto JSON por invocación»: emitían su
  payload y **luego** levantaban `CliError`, que `main()` traducía en un **segundo**
  objeto `{"error":{…}}` concatenado en stdout. El síntoma verificado: `doctor --json`
  con un FAIL escribía dos objetos JSON y reventaba el `JSON.parse` de cualquier
  consumidor programático. La raíz era doble: (a) el orden emit-then-raise en `daemon`
  (bug puro) y (b) un hueco de contrato en `doctor`, cuyo exit 1 es un **veredicto**
  (no un fallo), pero la arquitectura solo sabía salir con código ≠ 0 vía `CliError`,
  que siempre adjunta el objeto `error`.
- **Salida por veredicto como tercer formato del canal `--json`**: `main()` ahora honra
  un retorno entero ≠ 0 de un comando como `sys.exit(code)` sin emitir objeto `error`,
  preservando la invariante «ninguna salida no-cero fuera de `main()`». `doctor --json`
  con FAIL emite solo el reporte (`checks`, `failed`) y sale con 1; su ruta humana ya no
  duplica la línea de resumen en stderr.
- **`daemon start/stop/restart` levantan antes de emitir**: en fallo, cada subcomando
  emite solo el objeto `error` (vía `main()`) y sale con 5; en éxito, solo su payload de
  acción. Antes emitían el payload de acción incluso en fallo.

## [0.9.0] — 2026-07-31

### Cambiado

- **Cambio incompatible acumulado en los tres movimientos del rediseño**: recoge las tres rupturas de contrato que se consolidan en 0.9.0: (1) **desaparición del comando `speak`**, eliminado en el Movimiento 1 y reemplazado sucesivamente por `speech say` (Movimiento 2) y `speech synthesize` (Movimiento 3), sin que quede ningún alias de `speak` en la superficie de la CLI; (2) **remapeo de códigos de salida a enteros**, con las constantes `EXIT_*` en camelCase de `exit_codes.py` sustituyendo al antiguo sistema de nombres, y `main()` como único traductor de causas a enteros; (3) **clave `"error"` en los payloads `--json` del canal de error**, de modo que `main()` traduce toda salida no-cero a un objeto `{"schema_version","error":{"code","reason","message"}}` en stdout bajo `--json`, en lugar de dejar stdout vacío como ocurría con los errores de `speak`.

- **`voice clone` precomputa los conditionals en el momento de clonar**: antes el
  clonado solo validaba y copiaba los audios, y la preparación de la voz (cómputo
  de conditionals) se difería a cada síntesis — que, sin `conditionals.pt` en
  disco, la recomputaba **cada vez**. Ahora `voice clone` precomputa y guarda
  `conditionals.pt` al clonar, de modo que toda síntesis posterior los carga desde
  disco (latencia estable, sin sobrecosto en la primera reproducción). Con un
  daemon activo, el precómputo aprovecha el modelo caliente vía el nuevo endpoint
  IPC `POST /voices/precompute`; sin daemon, carga el modelo en modo directo.
- **`voice clone` ahora exige el modelo provisionado** (revierte el «clonado libre
  de modelo» de S2-15): el precómputo ejecuta el modelo, así que sin `setup`
  previo el comando aborta con exit 2. Un fallo del precómputo **no** aborta el
  clonado: la voz queda registrada con un aviso por stderr y sus conditionals se
  computan en la primera síntesis (red de seguridad on-the-fly conservada). El
  payload `--json` incluye la clave `precomputed`.

### Añadido

- **`peft` como dependencia directa**: se añade `peft>=0.13.0` a
  `pyproject.toml`. Con `peft` instalado, diffusers usa el backend PEFT
  en lugar de la clase legada `LoRACompatibleLinear`, eliminando el
  `FutureWarning` asociado desde la fuente (no a través de supresión).

### Corregido

- **Lista de warnings silenciados en `bootstrap.py`**: el filtro allow-list
  declaraba `(None, DeprecationWarning, r"^diffusers\.")` pero el warning
  real de diffusers es `FutureWarning`; la categoría errónea hacía que el
  filtro nunca lo alcanzara. Corregido a `FutureWarning`. Se añade una
  entrada para `torch.backends.cuda.sdp_kernel` (FutureWarning), advertencia
  deprecación de PyTorch que es transitiva vía chatterbox. El filtro usa un
  patrón de mensaje (`.*torch\.backends\.cuda\.sdp_kernel`) en lugar de un
  filtro por módulo, porque PyTorch usa un valor de `stacklevel` alto en ese
  warning que hace que el frame reportado sea `contextlib` y no un módulo
  `torch.*`. La entrada de LoRACompatibleLinear ya no necesita estar en la
  lista de supresión porque `peft` la elimina desde la raíz (diffusers usa
  el backend PEFT cuando peft está instalado).
- **Guía accionable cuando falta `libportaudio2` en Linux (canal PyPI)**: el
  wheel de `sounddevice` no trae PortAudio embebido (a diferencia del AppImage),
  y su ausencia levantaba un `OSError` no capturado (traceback crudo) con un
  mensaje engañoso que decía «Instala sounddevice» (sounddevice sí está; falta
  la librería del sistema). Ahora `_init_linux` captura ese `OSError` con un
  mensaje que remite a `apt install libportaudio2` / `dnf install portaudio`,
  `_play_audio` lo traduce a una `CliError` limpia con exit code coherente, y el
  detalle degradado de `doctor`/`setup` en Linux nombra libportaudio2 y su
  remediación.

### Eliminado
  habla sintética (`data_root()/synthetic-speech/`), incluida la voz
  `default`. Complementa `--voices` (que preserva `default`) y `--model`.
- **`--voices` arrastra las locuciones de habla sintética**: al borrar
  voces de usuario con `cleanup --voices`, las locuciones guardadas en los
  namespaces de habla sintética de esas voces se eliminan en la misma
  operación, excepto las de `default` (voz de fábrica de solo lectura).
  Anteriormente esos archivos huérfanos quedaban como residuo.
- **`voice clone --daemon` y `--no-daemon`**: el precómputo de conditionals
  al clonar una voz ahora despacha según los tres modos (autodetect,
  `--daemon`, `--no-daemon`), igual que las sub-acciones de `speech`. Sin
  flags se sondea el daemon y se usa solo si responde; con `--daemon` se
  exige y sale 5 si no está activo; con `--no-daemon` se fuerza modo
  directo.

### Eliminado

- **Método muerto `ChatterboxEngine.clone_voice`**: mezclaba copia de audios y
  precómputo y ningún llamador de producción lo usaba (solo sus tests). Se
  reemplazó por `ChatterboxEngine.precompute_voice(name)`, que precomputa una voz
  ya registrada, eliminando la divergencia de «dos caminos» de clonado.

## [0.8.0] — 2026-07-22

### Cambiado

- **`voice add` renombrado a `voice clone`**: el subcomando, la función
  `cmd_voice_add`, el método `add_voice` del motor y `register_voice_files`
  en `voices.py` se renombraron para reflejar con precisión la operación
  de clonado de voz. Toda la prosa, documentación y tests se actualizaron
  consistentemente.

## [0.7.8] — 2026-07-22

### Corregido

- **Hiss de alta frecuencia al final de la locución**: el fade-out fijo de 15 ms
  introducido en v0.7.7 era insuficiente para el hiss (~70 ms, 4-8 kHz) que el
  vocoder S3Gen produce en la cola de la síntesis. Se reemplaza por un fade
  adaptativo que detecta dinámicamente dónde termina el habla real (RMS > 0.004)
  y aplica un fade lineal completo desde ese punto, eliminando el "pss" audible
  al final de la reproducción en ambos backends (winsound y sounddevice).

## [0.7.7] — 2026-07-22

### Corregido

- **Eliminación de ruido/estática al final de la síntesis de audio**: se aplica un desvanecimiento suave (*fade-out* lineal de 15 ms) y un relleno de silencio de cola (50 ms) en `AudioWriter` (`_to_wav_bytes`) para prevenir la discontinuidad de fase y evitar estallidos de estática/interferencia al finalizar la locución en reproductores como `winsound` (WASAPI).

## [0.7.6] — 2026-07-20

### Corregido

- **Ventana de consola visible al iniciar el daemon en Windows**: el subproceso
  del daemon se lanzaba con `DETACHED_PROCESS | CREATE_NO_WINDOW`, pero
  `CREATE_NO_WINDOW` es ignorado por Windows cuando se combina con
  `DETACHED_PROCESS`, así que el daemon obtenía una consola nueva y visible
  (parpadeo). Se reemplaza por `CREATE_NO_WINDOW | CREATE_NEW_PROCESS_GROUP`:
  `CREATE_NO_WINDOW` suprime la consola por completo (sin ventana ni parpadeo) y
  `CREATE_NEW_PROCESS_GROUP` aísla al daemon en su propio grupo de proceso para
  que sobreviva al cierre de la terminal que lo lanzó. Sin cambios de contrato
  del CLI.

## [0.7.5] — 2026-07-17

Corrección de robustez del empaquetado (PyInstaller). No hay cambios de
contrato ni de comportamiento del CLI: los artefactos son funcionalmente
idénticos a 0.7.4 (las voces de fábrica ya viajaban por `--add-data`).

### Corregido

- **Fuente única de las voces de fábrica en el bundle**: se elimina
  `--collect-all ai_voice_interconnector` de los args de PyInstaller y se documenta que
  `--add-data` es la única fuente real de las voces en CI. Como el proyecto no
  se instala editable en el venv del build (solo `requirements-lock*.txt`),
  PyInstaller emitía *"collect_data_files - skipping data collection for module
  'ai_voice_interconnector' as it is not a package"* y `--collect-all` no aportaba nada; la
  redundancia era falsa. `--add-data` falla ruidosamente si falta el dir fuente,
  en vez de enmascarar la ausencia de voces en silencio.

## [0.7.4] — 2026-07-15

### Corregido

- **Ventana de consola visible al iniciar el daemon en Windows**: `daemon
  start` lanzaba el subproceso con `DETACHED_PROCESS`, que evita heredar la
  consola del padre pero no evita que Windows le asigne una consola nueva y
  visible (el ejecutable es de subsistema consola). Se agrega
  `CREATE_NO_WINDOW` junto a `DETACHED_PROCESS` para que el daemon arranque
  sin ninguna ventana visible. Sin cambios de contrato del CLI.

## [0.7.3] — 2026-07-15

Release de documentación que acompaña el lanzamiento del plugin de narración
para Claude Code. No hay cambios de contrato ni de comportamiento del CLI: los
artefactos son funcionalmente idénticos a 0.7.2.

### Añadido

- **Plugin de narración por voz para Claude Code**: se lanza
  [`tts-sidecar-narrator`](https://github.com/CristianRojas-SoftwareEngineer/tts-sidecar-narrator),
  un plugin que narra la actividad de la sesión de Claude Code usando este
  motor como sintetizador (verificado contra esta versión del CLI). Vive en su
  propio repositorio con ciclo de vida y versionado independientes.
- **`docs/NARRATION-INTEGRATION.md`**: el contrato de integración con el
  plugin, desde la perspectiva del motor (superficies de la CLI consumidas,
  estabilidad esperada). Contraparte del `docs/INTEGRATION.md` del plugin.

### Cambiado

- **`docs/CLAUDE-CODE-PLUGIN.md`** pasa a ser un puntero histórico: el diseño
  original del plugin se conserva como referencia, pero la fuente de verdad es
  el repositorio del plugin (el árbol `claude-plugin/` se extrajo del repo).
- **Documentación**: tablas de contenido en los documentos largos y formato
  refinado; `CLAUDE.md` se convierte en guía de comportamiento del agente con
  `AGENTS.md` como hardlink.

## [0.7.2] — 2026-07-14

Corrección del smoke test de release en Windows. No hay cambios de contrato ni
de comportamiento del CLI: los artefactos son funcionalmente idénticos a 0.7.0.
Se salta 0.7.1 (etiqueta de un intento previo, nunca publicada).

### Corregido

- **Smoke test de voces de fábrica en Windows (CI)**: el chequeo usaba
  `$voiceList -notmatch 'default'`, pero `-notmatch` sobre un **array** en
  PowerShell *filtra* los elementos que no coinciden en vez de devolver un
  booleano; la línea de encabezado `«Voces registradas:»` disparaba un falso
  negativo que abortaba `build-windows-x64` aunque la voz `default` sí estuviera
  empaquetada. Se reemplaza por `-notcontains '  - default'` (prueba de
  membresía exacta), espejo del `grep -qx '  - default'` que ya usaban los smoke
  de Linux y macOS. El empaquetado de la voz de fábrica nunca estuvo roto.

## [0.7.0] — 2026-07-14

Limpieza post-auditoría: eliminación de `docs/PRODUCTION-READINESS-AUDIT.md` (archivo de auditoría cerrada) y remoción de identificadores de hallazgos (`S<N>-<XY>`) filtrados en el código fuente y documentación. Los cambios son puramente documentales; no afectan el contrato del CLI ni el comportamiento funcional.

### Añadido

### Cambiado

- Los cambios de contrato anteriores son **aditivos**: `schema_version` del CLI y del protocolo NDJSON permanecen en `"1"`.

### Corregido

- **`--json` de la síntesis** acoplado a
  `--output` (el archivo es el canal de datos; `--json` solo emite metadatos a
  stdout), emite `{"schema_version","output","voice","t3_time","s3gen_time","daemon"}`
  a stdout, idéntico campo a campo en modo directo y vía daemon. `--json` sin
  `--output` falla con exit 4 antes de cualquier trabajo.
- **`daemon start/stop/restart --json`** payload de resultado de la
  acción `{"schema_version","action","ok","pid"?}` a stdout; los mensajes
  informativos pasan a stderr en modo `--json`. `daemon serve` queda
  deliberadamente sin `--json` (su contrato es el stream NDJSON del server).
- **Versionado del protocolo NDJSON del daemon**: los 5 modelos de
  `daemon/protocol.py` (`ProgressEvent`, `ResultEvent`, `ErrorEvent`,
  `HealthResponse`, `VoicesResponse`) heredan de una clase base común
  (`ProtocolModel`) con `schema_version` y `extra="ignore"` explícitos;
  `HealthResponse`/`/health` gana el campo `version` (la del paquete), que
  permite diagnosticar el skew entre un daemon residente y un CLI actualizado.
  Política de compatibilidad documentada en `docs/DAEMON-MODE.md`.
- **Test estructural del contrato `--json`**: `build_parser()` se
  extrajo de `main()` para ser introspeccionable; un test nuevo descubre desde
  el parser real qué subcomandos declaran `--json` y lo compara contra la
  cobertura declarada en los tests, rompiendo ante un comando nuevo sin cubrir
  o un flag retirado.
- **Oferta de código fuente GPLv3 §6 en los 4 artefactos**:
  `SOURCE-OFFER.md` (generado por `scripts/render_source_offer.py` desde la
  versión single-source, con la URL del tarball del tag y el enlace al
  release) viaja ahora dentro de los 3 bundles nativos (vía `LICENSE_FILES`)
  y del wheel/sdist de PyPI (vía `license-files`), de modo que la oferta
  acompaña al binario por cualquier vía de redistribución. Un test de
  consistencia byte-exacto falla si el archivo commiteado diverge del
  generador o de la versión.
- **Cobertura de tests medida y gateada por módulo**: `pytest-cov`
  pineado vía `pipeline.parameters.pytest_cov_version` (mismo mecanismo que el
  pin de `pytest`), configuración única en `[tool.coverage.*]` de
  `pyproject.toml`. Nuevo job `coverage` independiente en CI (Linux, no
  duplicado en los tres SO) que corre la suite bajo `pytest --cov` y aplica
  `scripts/check_coverage.py`, un gate diferenciado por módulo (`MODULE_FLOORS`
  como fuente única de los pisos, fijados por ratchet-desde-lo-medido) para los
  módulos de contrato (`cli.py`, `daemon/*`, `model_cache.py`, `voices.py`,
  `paths.py`); el resto se reporta sin gatear. Publica `coverage.xml` como
  artefacto. Coverage queda opt-in: `pytest tests/ -v` sigue verde sin
  `pytest-cov` instalado.
- **Verificación automatizada del inventario de licencias**:
  `scripts/check_third_party_licenses.py` compara el conjunto de paquetes de
  `requirements-lock.txt` contra la tabla de `THIRD-PARTY-LICENSES.md`
  (nombres normalizados PEP 503) y un test de la suite falla con diff legible
  ante cualquier faltante o sobrante — la desincronización del inventario
  legal deja de ser silenciosa.
- **Caveats de licencia en el Cask de Homebrew**: el Cask informa la licencia
  GPL-3.0-or-later y la ubicación de `SOURCE-OFFER.md` y
  `THIRD-PARTY-LICENSES.md` dentro del `.app` instalado (la stanza `license`
  no existe en el DSL de Casks).

### Cambiado

- Todos los cambios de contrato anteriores son **aditivos**: `schema_version`
  del CLI y del protocolo NDJSON permanecen en `"1"`. Internamente,
  el método de síntesis de la fachada, `SynthesisOrchestrator.synthesize()` y
  `DaemonIPCClient.synthesize()` ahora retornan un objeto de resultado
  `SynthesisResult` (audio + métricas `t3`/`s3gen`) en vez de `bytes` desnudos,
  unificando la fuente de métricas de ambas rutas de síntesis; y los emisores
  `--json` existentes del CLI se migraron a un helper único `emit_json()`
  (mismos payloads, sin cambios de clave).
- **create-dmg pineado por contenido**: el build de macOS ya no
  instala create-dmg vía `brew install` sin versión; `build_macos.py` descarga
  el tarball del release v1.3.0 pineado por URL + SHA-256 (`fetch_pinned_asset`,
  misma política que appimagetool) y ejecuta el script extraído. El step de
  Homebrew desapareció de `.circleci/config.yml` y la dependencia pasa a ser
  dura (su fallo aborta el build en vez de degradar con warning): el 100% del
  tooling de build queda fijado por contenido.
- **Wheel PyPI con inventario legal completo**: `THIRD-PARTY-LICENSES.md`
  (antes ausente del canal PyPI) y `SOURCE-OFFER.md` se incluyen ahora en el
  wheel y el sdist junto a `LICENSE`.

### Arreglado

- **Normalización de la criticidad de Inno Setup**: `create_installer_windows.py` declaraba Inno
  Setup con `required=False` y luego lo hacía fatal con un `sys.exit(1)` manual
  redundante, desacoplando la criticidad real del mecanismo declarativo. Ahora
  se resuelve con `required=True` en `ensure_build_dependency` y se elimina el
  `sys.exit` manual: el aborto por dependencia faltante queda gobernado en un
  único punto (igual que PyInstaller y sounddevice). Se añaden los tests de
  rama de fallo `tests/test_create_installer_windows.py::
  test_main_inno_missing_is_fatal` (Inno ausente → aborta) y
  `tests/test_build_linux.py::test_appimage_tooling_missing_degrades_without_abort`
  (tooling del AppImage ausente → degrada sin abortar), y se corrige el drift de
  `docs/BUILD.md`, que aún clasificaba a Inno Setup como empaquetador que degrada.

- **Cancelación cooperativa de la síntesis al desconectar el cliente**:
  en el modo daemon, `/synthesize` ahora detecta la desconexión del cliente y
  aborta la síntesis en curso en vez de malgastar GPU/CPU hasta completarla. El
  generador del stream setea un `threading.Event` al detectar la desconexión
  (vía `GeneratorExit`/`OSError`), el callback de progreso del worker eleva
  `SynthesisCancelled` (nueva excepción compartida en `exceptions.py`) y el
  engine la re-lanza selectivamente desde `_emit_progress`/`_token_counting_iter`
  sin romper el contrato best-effort para otras excepciones del callback. El
  `finally` del worker sigue liberando el semáforo de admisión y la memoria. Es
  la opción A híbrida: cancelación cooperativa en la fase T3, sin instrumentar
  S3Gen.

- **Test dedicado de `scripts/pyinstaller_wrapper.py`**: el componente crítico que evita el cuelgue COM
  del build Windows (`sys.coinit_flags = 0x8` antes del `import` de comtypes +
  `os._exit` para saltar el `CoUninitialize()` de `atexit`) ahora tiene
  `tests/test_build_utils.py::TestPyinstallerWrapper`. Cubre `main()` (propagación
  del `returncode` vía `os._exit` y limpieza del archivo temporal bootstrap) y el
  `_BOOTSTRAP` (la fijación de `coinit_flags` antes del `import` de PyInstaller).
  Es puramente aditivo y usa mocks, sin ejecutar PyInstaller ni tocar red/disco.
  Complementa `tests/test_build_utils.py::TestRunPyinstaller`, que ya ejercía la
  rama de timeout de `run_pyinstaller` (mata el árbol de procesos y retorna 1),
  dejando el timeout cubierto de extremo a extremo.

## [0.6.0] — 2026-07-11

Cierra la última brecha accionable de paridad de experiencia entre los 3 SO
registrada en `docs/PARITY.md`: la de *desinstalación en un comando*. Con ella,
`ai-voice-interconnector setup --uninstall` deja de ser solo-Linux y pasa a ser un comando
único en los tres SO (dispatch por SO sobre un contrato compartido: datos → PATH
→ binario, con cancelación atómica y guard de canal nativo). MINOR: capacidad
nueva en macOS y Windows, más un cambio de comportamiento deliberado en Linux.
Solo la brecha de *firma de código* (SmartScreen/Gatekeeper, binarios sin firmar)
queda diferida al goal a largo plazo.

### Añadido

- **`setup --uninstall` en macOS** (`_uninstall_macos`): desinstalación de un
  comando que encadena `cleanup --all`, quita el symlink per-user de
  `~/.local/bin` y borra el `.app` (localizado desde `sys.executable` con
  `resolve()`; cubre `~/Applications`, `/Applications` y el Cask con una sola
  expresión, con guard de sufijo `.app`). Si la instalación proviene de Homebrew
  (metadata del Caskroom), aborta sin borrar nada y remite a `brew uninstall
  --cask --zap ai-voice-interconnector` para no dejar el Caskroom inconsistente.
- **`setup --uninstall` en Windows** (`_uninstall_windows`): valida primero el
  `QuietUninstallString` del registro (HKCU, clave `{AppId}_is1`) sin efectos,
  borra los datos en proceso con `cleanup --all` y **delega** el binario y la
  reversión del PATH al desinstalador de Inno, lanzado desacoplado con
  `subprocess.Popen` (el SO mantiene el lock del `.exe`). El payload `--json`
  atestigua las rutas de datos en `removed` y el directorio de instalación en el
  campo aditivo `delegated`.
- **Guard de canal nativo** (`is_frozen`) en `setup --uninstall`, común a los
  tres SO: desde fuente o desde una instalación pip/uv, aborta remitiendo a `pip
  uninstall ai-voice-interconnector` en lugar de operar sobre rutas que no le pertenecen.

### Cambiado

- **Reorden de la rama Linux de `setup --uninstall`** al orden unificado del
  contrato compartido (`cleanup --all` → symlink → directorio de instalación, en
  vez de symlink → directorio → cleanup). Habilita la **cancelación atómica**:
  cancelar la confirmación del cleanup aborta la desinstalación sin borrar nada
  (salida 0), imposible con el orden anterior (el binario caía antes de la
  pregunta). Además el uninstall borra ahora el directorio raíz de datos
  (`data_root()`) si queda vacío tras el cleanup.
- **Payload `--json` de `setup --uninstall`**: `removed` incluye ahora las rutas
  de datos del `cleanup` encadenado (corrección de una omisión de la rama Linux)
  y el `data_root()` si se eliminó. En Windows se añade el campo `delegated`
  (directorio de instalación, borrado por Inno tras la salida del proceso).
  Ambos son cambios aditivos: `schema_version` no cambia.

## [0.5.0] — 2026-07-10

Cierra las brechas de paridad de experiencia entre los 3 SO registradas en
`docs/PARITY.md`: iguala macOS y Linux con la experiencia objetivo de Windows
(instalación de una línea sin admin, actualización sin residuo, desinstalación
con residuo cero). MINOR: añade capacidades y cambia el comportamiento de
instalación en macOS. Solo la brecha de *firma de código* (SmartScreen/Gatekeeper,
binarios sin firmar) queda diferida al goal a largo plazo.

### Añadido

- **Instalador macOS de una línea** (`install-macos.sh`, `curl | sh`): vía sin
  Homebrew ni `sudo`, homóloga a `install.sh`. Descarga el `.dmg` de arm64 y
  `SHA256SUMS.txt`, verifica el checksum con `shasum -a 256 -c` (aborta si no
  coincide), monta con `hdiutil`, copia el `.app` a `~/Applications` con
  `ditto`, limpia la cuarentena de Gatekeeper con `xattr`, crea el symlink
  per-user en `~/.local/bin` (con aviso de PATH) y encadena `setup`. Guard de
  arquitectura arm64 (Mac Intel no soportado). Smoke-test `bats` en el job CI
  `test-installer-macos` (executor macOS) como puerta de los 4 builds.
- **Desinstalador Linux de un paso** (`ai-voice-interconnector setup --uninstall`): quita
  el symlink de PATH, borra `~/.local/opt/ai-voice-interconnector/` y encadena `cleanup
  --all` (con confirmación; `--yes` la omite). Mutuamente excluyente con
  `--remove-path`/`--force-update`, con guard de SO y contrato `--json`
  (requiere `--yes`). Reemplaza los tres pasos manuales anteriores.

### Cambiado

- **Scripts `.command` del `.dmg` sin `sudo`**: la instalación y desinstalación
  incluidas en el `.dmg` de macOS crean/eliminan el symlink per-user en
  `~/.local/bin` en lugar de `/usr/local/bin` con `sudo`. Ninguna vía de
  instalación del proyecto pide ya la contraseña de administrador. **Nota de
  transición**: quien tenga un symlink legado en `/usr/local/bin` (de una
  versión anterior a 0.5.0) verá en el script de desinstalación la instrucción
  para quitarlo (`sudo rm /usr/local/bin/ai-voice-interconnector`).
- **`install.sh` limpia los AppImages anteriores**: tras instalar y dar
  permisos al AppImage nuevo, elimina los `ai-voice-interconnector-*.AppImage` previos de
  `~/.local/opt/ai-voice-interconnector/`, que antes se acumulaban (~1-2 GB por versión).
  Re-ejecutar el one-liner es ahora la vía de actualización limpia de Linux.

### Corregido

- **`zap` del Cask completo**: la stanza `zap trash:` del Cask de Homebrew ahora
  lista los **dos** repos del modelo en la caché de HuggingFace (el multilingüe
  `Chatterbox-Multilingual-es-mx-latam` y el base `chatterbox` del Voice
  Encoder); antes omitía el segundo, dejando cientos de MB de residuo a quien
  desinstalara con `brew uninstall --zap`. Se propaga al tap con este release
  vía `publish-metadata`.

## [0.4.0] — 2026-07-10

Extiende la instalación auto-hospedada de una línea a Windows y migra el
instalador Inno Setup a per-user, sin tocar el contrato del CLI (códigos de
salida, esquemas `--json`). MINOR: cambia el comportamiento de instalación
en Windows.

### Añadido

- **Instalador Windows de una línea** (`install.ps1`, `irm | iex`): resuelve
  el release más reciente, descarga el instalador x86_64 y `SHA256SUMS.txt`,
  verifica el checksum SHA-256 antes de instalar (aborta si no coincide),
  instala en silencio sin UAC y ejecuta `ai-voice-interconnector setup`. La descarga por
  CLI no aplica el Mark-of-the-Web, así que no dispara SmartScreen (detalle
  en `docs/SELF-HOSTED-INSTALL.md` y `SECURITY.md`). Smoke-test Pester en CI
  (`test-installer-windows`) como puerta de los 4 builds.

### Cambiado

- **Instalador de Windows per-user**: Inno Setup pasa de per-machine a
  per-user — `PrivilegesRequired=lowest` (sin prompt de UAC), instalación en
  `%LOCALAPPDATA%\Programs\ai-voice-interconnector` (antes Program Files) y PATH de
  usuario en `HKCU\Environment` (antes HKLM), incluida su reversión al
  desinstalar. **Nota de migración**: si tienes instalada una versión
  per-machine (anterior a 0.4.0), desinstálala primero desde el Panel de
  control (con admin) antes de instalar 0.4.0+; instalar la per-user encima
  puede dejar dos instalaciones y PATH duplicado.

## [0.3.0] — 2026-07-10

Extiende el canal nativo con instalación auto-hospedada por SO y reduce los
falsos positivos de antivirus en los binarios PyInstaller, sin tocar el
contrato del CLI (códigos de salida, esquemas `--json`) ni los canales
existentes (nativo/PyPI).

### Añadido

- **Instalador Linux de una línea** (`install.sh`): resuelve el release más
  reciente, elige el `.AppImage` por arquitectura (`uname -m`), verifica el
  checksum SHA-256 contra `SHA256SUMS.txt` antes de instalar, integra el PATH
  vía la variable `APPIMAGE` y ejecuta `setup`. Documentado en `README.md` y
  `USAGE.md` con su desinstalación limpia de 3 pasos.
- **Cask de Homebrew propio** (`homebrew-ai-voice-interconnector`): `brew install --cask
  ai-voice-interconnector` instala desde el `.dmg` del release; `publish-metadata` en CI
  reescribe el Cask con la versión y el `sha256` del `.dmg` en cada release
  (idempotente). Ver `docs/RELEASING.md` y `docs/DISTRIBUTION.md`.
- **Runbook de reporte de falso positivo** a Windows Defender Security
  Intelligence (WDSI) en `SECURITY.md`, acotado a la detección de Defender
  Antivirus (no afecta SmartScreen).

### Cambiado

- **Endurecimiento del build PyInstaller**: `--noupx` en los flags compartidos
  y metadata PE (`--version-file`) en el `.exe` de Windows, para mitigar las
  heurísticas de antivirus sobre el patrón de empaquetado. Cubierto por tests.

## [0.2.1] — 2026-07-08

Corrige la instalación vía `uv tool install ai-voice-interconnector`: quedaba rota por un
conflicto de resolución de dependencias entre `numpy` y `numba` (transitiva de
`librosa`/`chatterbox-tts`).

### Corregido

- **`uv tool install ai-voice-interconnector` fallaba** al intentar compilar
  `llvmlite==0.36.0` desde fuente en Python 3.13 (`RuntimeError: Cannot
  install on Python version 3.13.14; only versions >=3.6,<3.10 are
  supported`). Causa: `chatterbox-tts` declara `numpy>=2.0.0` sin tope
  superior para Python ≥3.13, mientras que `numba` (dependencia transitiva vía
  `librosa`) exige `numpy<2.5`. El resolvedor de `uv` fijaba primero la
  versión más reciente de `numpy` (sin tope) y, al no poder satisfacer el tope
  de `numba`, retrocedía sobre `numba` hasta versiones sin soporte para Python
  3.13, en vez de retroceder sobre `numpy`. `pip` no caía en esta trampa por
  las heurísticas de su propio resolvedor, por lo que el smoke test de CI
  (que usa `pip install`) no lo detectó. Fix: se fija explícitamente
  `numpy<2.5` como dependencia directa en `pyproject.toml`, acotando el rango
  antes de que cualquier resolvedor tenga que elegir entre `numpy` y `numba`.

## [0.2.0] — 2026-07-08

Añade un segundo canal de distribución (PyPI / `uv tool install` / `pipx`)
junto al canal nativo de binarios PyInstaller existente, sin afectar su
funcionamiento. Requirió reestructurar la ubicación de las voces de fábrica y
el modelo de rutas para que sean válidos en los tres modos de ejecución
(fuente, pip-installed, congelado) sin bifurcaciones. Registra además la
estrategia de firma de código (SignPath + notarización Apple) como compromiso
de roadmap en `docs/GOAL.md`.

### Añadido

- **Canal de distribución PyPI**: `uv tool install ai-voice-interconnector` / `pipx
  install ai-voice-interconnector` instala el CLI completo, incluida la voz `default`.
  Publicación automática en cada tag `v*` vía el job `publish-pypi` de CI, en
  paralelo a los cuatro builds nativos. Documentado en el nuevo
  `docs/DISTRIBUTION.md`, con la matriz de trade-offs frente al canal nativo
  y el registro de la decisión de mantener ambos canales en paralelo.
- **`src/ai_voice_interconnector/bootstrap.py`**: consolida en una única función
  idempotente (`apply()`) la supresión de warnings, las variables de entorno y
  el mock de `pkg_resources` que antes solo vivían en `bin/ai-voice-interconnector`,
  duplicados parcialmente en `daemon/run.py`. Corre igual desde el entry point
  de pip, `bin/ai-voice-interconnector`, `python -m ai_voice_interconnector` y el daemon.
- **`src/ai_voice_interconnector/__main__.py`**: habilita `python -m ai_voice_interconnector` como
  vía de invocación adicional.

### Cambiado

- **Voces de fábrica reubicadas** a `src/ai_voice_interconnector/voices/` (antes `voices/`
  en la raíz del repo), para que setuptools pueda empaquetarlas en el wheel
  (`package-data`); el bundle PyInstaller (`--add-data`) se actualizó al mismo
  origen.
- **Modelo de rutas uniforme** (`paths.py`): `bundled_voices_dir()` resuelve
  siempre relativa al paquete (sin distinguir fuente/congelado) y `data_root()`
  devuelve siempre el user-data-dir por SO, incluso en modo fuente (antes
  caía dentro del propio checkout).
- **`pyproject.toml` publicable**: entry point `ai-voice-interconnector = "ai_voice_interconnector.cli:main"`,
  versión dinámica (`dynamic = ["version"]`) resuelta desde
  `ai_voice_interconnector.__version__` como fuente única, metadata de PyPI completa
  (`readme`, `urls`, `classifiers`, `keywords`) y `package-data` para las
  voces de fábrica.

## [0.1.1] — 2026-07-07

Ciclo perfectivo que corrige los 12 hallazgos Menores 
identificados durante la revisión final del release `0.1.0`, más el ciclo
correctivo: cierra la única grieta
funcional (el release gate del pin de revisiones en la carga del engine) y siete
Menores (gate de `daemon serve`, identidad del health check, sandbox acotado del
daemon, exactitud documental y procedencia del lockfile). Todos los cambios
de contrato son aditivos: los códigos de salida existentes no cambian y
`schema_version` permanece en `"1"`.

### Añadido

- **`--json` en los cuatro comandos de escritura**: `voice clone`
  (`{name, reference, speech}`), `voice remove` (`{name, removed}`), `setup`
  (`{model, already_cached, downloaded, cache_dir}`, con variante para
  `--remove-path`) y `cleanup` (`{removed, dry_run}`). El contrato programático
  queda simétrico: ningún comando obliga a parsear texto. `cleanup --json`
  exige `--yes` o `--dry-run` (exit 4 sin ellos) y envía sus listados
  informativos a stderr.
- **Referencia de esquemas `--json` en `USAGE.md`**: las claves de los
  nueve payloads (tipo y significado) declaradas por escrito como parte del
  contrato, sin necesidad de ingeniería inversa.
- **Revisión fijada del modelo por release**: `setup` descarga ambos
  repos de HuggingFace con `revision=` (commit hash auditado, constantes
  `MODEL_REVISIONS`/`BASE_MODEL_REVISION` en `model_cache.py`) y la detección
  de caché valida el snapshot de esa revisión en ambos repos (ciclo posterior
  cerró el residuo del repo base); el bump del pin es un paso del
  runbook de release (`docs/RELEASING.md`) y su alcance está descrito en
  `SECURITY.md`.
- **Plantillas de Issue/PR en `.github/`**: formularios de bug (versión,
  SO, comando reproducible, salida) y de propuesta, `blank_issues_enabled:
  false` con la vía de seguridad señalizada, y checklist de PR alineado a
  `CONTRIBUTING.md`.

### Cambiado

- **Documentación del bloqueo SmartScreen/Gatekeeper ampliada**: `README.md`,
  `USAGE.md`, `SECURITY.md` y `docs/BUILD.md` explican por qué el sistema
  bloquea el primer arranque (binario sin firma + sin reputación por ser cada
  release un archivo nuevo), que no indica malware, cómo proceder paso a paso
  (incluido el bloqueo del navegador y la cuarentena de antivirus de terceros,
  siempre tras verificar el SHA-256) y la ruta prevista de firma de código vía
  SignPath Foundation (gratuita para proyectos open source).
- **`daemon stop` honesto durante la ventana de arranque**: detecta el
  daemon en arranque por cmdline (sin PID file), avisa «arrancando; aún no
  acepta conexiones» y termina con exit 5 en vez de reportar un éxito falso;
  no mata el proceso. Documentado en `docs/DAEMON-MODE.md`.
- **CI con imágenes fijadas por digest y pip pineado**: las tres
  referencias `cimg/python:3.13` usan `@sha256:<digest>` (manifest list
  multi-arch) y los siete `pip install --upgrade pip` pasaron a versión exacta.
  Excepciones documentadas: `brew upgrade pyenv` (necesario para el parche
  3.13.14; no altera el artefacto) y `create-dmg` (Homebrew no pinea).
  Implicaciones y procedimiento de bump en `docs/BUILD.md` §Reproducibilidad.
- **Exactitud documental**: stack de reproducción real
  (winsound/sounddevice/afplay) en `docs/DESIGN.md`
  (antes describían pycaw-WASAPI/pyalsaaudio/AVFoundation); árboles de
  estructura con `voices.py`, `paths.py` y `model_cache.py`; CI descrito como
  Linux/Windows/macOS en `CONTRIBUTING.md`; conteo de tests actualizado a
  **268** en `docs/GOAL.md`; inventario de licencias consistente (dependencias
  copyleft-compatibles mencionadas en `USAGE.md`/`CLAUDE.md`; los runtimes
  NVIDIA no van en ningún artefacto distribuido — `README.md` y
  `THIRD-PARTY-LICENSES.md`).

- **El engine honra las revisiones fijadas en la carga y en las descargas de
  respaldo** (release gate): la
  resolución de snapshot en tiempo de carga (language pack y snapshot base del
  Voice Encoder) y las dos redes de seguridad de descarga (`snapshot_download`
  del modelo, `hf_hub_download` de `ve.safetensors`) pasan `revision=`
  (`MODEL_REVISIONS`/`BASE_MODEL_REVISION`). Cierra la asimetría por la que la
  detección honraba el pin pero la carga caía al fallback `refs/main`→mtime: tras
  un bump futuro de revisión ya no puede producirse síntesis silenciosa con el
  modelo viejo. Simétrico con `setup` y la detección de caché.
- **`daemon serve` exige el modelo en caché antes de arrancar**: mismo gate que `daemon start`; sin modelo provisionado
  falla rápido remitiendo a `setup` (exit 2) sin cargar el engine ni disparar la
  descarga de su red de seguridad. Ningún subcomando descarga de forma implícita.
- **Sandbox de audio del daemon acotado a un subdirectorio namespaced**: `/synthesize` acepta audio bajo los
  directorios de voces (fábrica/usuario) y `<tempdir>/ai-voice-interconnector/`, pero ya no
  bajo el tempdir compartido general (`%TEMP%`/`/tmp`), reduciendo la superficie
  de temp compartido preservando el staging IPC. `docs/DAEMON-MODE.md` y
  `USAGE.md` describen la superficie real.
- **Exactitud documental de estados, conteos y `doctor`**: `daemon status` documenta los valores reales
  (`"healthy"`/`"initializing"`, ya no `"ready"`) en prosa y en la tabla del
  esquema JSON de `USAGE.md`; el ejemplo de `doctor` incluye el chequeo de RAM y
  el total de chequeos coherente (5); conteo de tests a **268**.
- **`requirements-lock.txt` regenerado con el comando canónico**: sin el `--constraint` a un archivo externo al repo; su
  header ya no referencia ningún override y la procedencia vuelve a ser
  reproducible desde `CLAUDE.md`/`docs/BUILD.md`. La resolución de versiones es
  idéntica a la anterior; instala con `--require-hashes`.

### Corregido

- **`voice list` ante un directorio de voces ilegible**: el mensaje
  apunta al directorio de voces de usuario implicado en vez de remitir a
  `ai-voice-interconnector setup` (que no resuelve un problema de filesystem); conserva
  exit 3.
- **`--daemon --no-daemon` en la síntesis**: los flags contradictorios producen
  un error claro en stderr y exit 4 antes de cualquier trabajo, en vez de que
  `--daemon` gane en silencio.
- **Validación de integridad de los tres checkpoints**: `is_model_cached`
  valida el header safetensors también de `s3gen_v3.safetensors` y
  `ve.safetensors` (antes solo del T3): una descarga truncada de cualquiera se
  reporta como «no cacheado» y `doctor` remite a `setup`, en vez de reventar
  con un error críptico en la primera síntesis.
- **Fixture `mock_daemon_client` alineada con el cliente real**: la
  firma de `synthesize` coincide con `DaemonIPCClient.synthesize`
  (`on_progress` en vez de los inexistentes `model`/`compute_backend`).
- **La detección de vida del daemon valida la identidad del servicio**: `DaemonIPCClient.is_running` ya no acepta
  cualquier `200` en `/health`, sino que valida el cuerpo contra `HealthResponse`;
  si otro servicio local ocupara el puerto 8765 y respondiera `200`, ya no se
  confunde con un falso «daemon ya corriendo» (que derivaba en síntesis fallidas
  con exit 5 difícil de atribuir). `DaemonManager` delega en el mismo chequeo.
- **Detección del Voice Encoder honra la revisión fijada del repo base**
  (residuo): `is_ve_cached` resuelve el snapshot del
  repo `ResembleAI/chatterbox` exclusivamente contra `BASE_MODEL_REVISION` (un
  VE de otra revisión ya no cuenta como caché válida), simétrico con la
  descarga de `setup` y con la rama del language pack. Cobertura nueva: caso
  positivo bajo `BASE_MODEL_REVISION` y caso negativo (revisión distinta) en
  `tests/test_engine_cache.py`. Párrafo de `USAGE.md` sobre actualización
  anclado al mecanismo real (revisión fijada por release + deduplicación por
  blob de la caché de HF).
- **Decisión de validación E2E documentada** (criterios 1-3, 9 de
  `docs/GOAL.md`): la validación end-to-end de los instaladores por SO es
  externa al pipeline por diseño (consume demasiada cuota de runner al cargar
  el modelo y los ~2 GB de pesos en cada build). El pipeline mantiene el smoke
  test automatizado del binario congelado. Fuera del pipeline: Windows la
  realiza el propietario sobre su equipo local; Linux y macOS dependen de
  feedback de usuarios reales. La decisión completa está en `docs/GOAL.md`
  §"Decisión de validación E2E" y `docs/BUILD.md` §"Verificación post-build".

## [0.1.0] — 2026-07-03

Release inaugural. Al ser la primera versión publicada, no hay base previa
respecto de la cual registrar cambios o correcciones: esta sección describe el
estado con el que nace el producto.

### Añadido

- **Motor de síntesis offline** con Chatterbox Multilingual (alias
  `es-mx-latam`, español latinoamericano): voz por defecto empaquetada
  (`default`, de fábrica) y clonación de voz vía `voice clone` con modelo
  dual-audio (`reference.wav` para timbre + `speech.wav` para conditioning).
  El audio generado no lleva marca de agua (watermark de PerthNet desactivado
  por diseño; ver «Uso ético y responsable»).
- **CLI multiplataforma** (Windows/Linux/macOS, idéntica en los tres SO) con
  los comandos `speak`, `voice` (`clone`/`list`/`remove`), `daemon`
  (`start`/`stop`/`restart`/`status`/`serve`), `devices`, `doctor`, `setup`,
  `cleanup` y `version`; salidas de usuario en español.
- **Contrato programático para orquestadores** (consumo vía `subprocess`):
  stdout reservado para datos y diagnóstico/progreso por stderr (UTF-8 forzado);
  mapa de códigos de salida congelado — `0` éxito, `1` error genérico, `2`
  modelo no provisionado, `3` voz/audio no encontrado, `4` entrada inválida,
  `5` daemon inalcanzable, `130` interrupción (Ctrl+C, sin traceback) —;
  `--json` con `schema_version` en los comandos de lectura (`version`,
  `doctor`, `devices`, `voice list`, `daemon status`).
- **Progreso real en vivo durante la síntesis**: eventos de etapa (conditionals →
  T3 → S3Gen → encoding → guardado) y conteo de tokens del T3 alimentan un
  indicador sobre stderr (solo en TTY), en modo directo y daemon.
- **Modo daemon**: servidor HTTP persistente en loopback (puerto fijo 8765,
  sin autenticación — control de acceso delegado al SO) que mantiene el modelo
  en memoria entre invocaciones; `/synthesize` responde un stream NDJSON
  (`progress` → `result`/`error`, modelos Pydantic en `daemon/protocol.py`);
  sandbox de rutas de audio (solo directorios de voces) con degradación
  automática a modo directo o error accionable según el despacho; auto-reinicio
  opcional (`--autorestart`, `--max-retries`).
- **Validación de entrada de la síntesis**: `--text` acotado a 5000 caracteres
  (exit 4 en ambas rutas, directa y daemon) con advertencia no bloqueante por
  encima de 2000; `--compute-backend` (`auto`/`cpu`/`cuda`/`mps`) con aviso
  cuando el daemon lo ignora; `--output` crea los directorios padres.
- **Ciclo de vida de provisión completo**: `setup` idempotente (chequeos de
  entorno + descarga ligera vía `snapshot_download`, sin cargar el modelo en
  RAM; incluye `ve.safetensors` para que ninguna síntesis posterior necesite
  red), pre-chequeo de espacio en disco, `--force-update` para re-descarga
  limpia, e integración de PATH en Linux/AppImage (`--remove-path` la
  revierte); la síntesis/`daemon start` fallan rápido remitiendo a `setup` sin
  descargas silenciosas; `cleanup` desaprovisiona quirúrgicamente
  (`--model`/`--voices`/`--all`/`--dry-run`, confirmación interactiva, `--yes`
  y EOF tratado como cancelación limpia para uso programático).
- **Distribución por SO**: instalador de Windows (Inno Setup, PATH + casilla de
  `setup`), AppImages de Linux x86_64/arm64 (runtime estático, sin `libfuse2`;
  requiere glibc ≥ 2.35, documentado con troubleshooting) y `.dmg` de macOS
  arm64 con scripts de instalación/desinstalación y `LSMinimumSystemVersion`
  derivada dinámicamente del toolchain.
- **Cadena de suministro y CI**: lockfile universal con hashes
  (`requirements-lock.txt`, instalado con `--require-hashes`) y lock CPU-only
  de Linux x86_64 (sin el stack `nvidia-*-cu12`, AppImage de cientos de MB en
  vez de GB); triple puerta de tests en CI (Linux/Windows/macOS nativos) que
  bloquea los 4 builds; smoke test del binario congelado en cada build; SHA-256
  de cada artefacto emitido en el log y `SHA256SUMS.txt` en el Release; runbook
  de publicación en `docs/RELEASING.md`.
- **Documentación y gobernanza**: `USAGE.md` (guía por caso de uso),
  `docs/DESIGN.md`, `docs/DAEMON-MODE.md`,
  `docs/BUILD.md`, `docs/RELEASING.md`, sección de uso ético y responsable
  (README/USAGE), `CONTRIBUTING.md`, `SECURITY.md`, este `CHANGELOG.md` y
  `THIRD-PARTY-LICENSES.md` (inventario de licencias generado del lockfile).
  Código propio bajo GPL-3.0-or-later; modelo MIT.

[0.10.5]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.10.4...v0.10.5
[0.10.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.10.3...v0.10.4
[0.10.3]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.10.2...v0.10.3
[0.10.2]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.10.1...v0.10.2
[0.10.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.10.0...v0.10.1
[0.10.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.9.1...v0.10.0
[0.9.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.9.0...v0.9.1
[0.9.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.8.0...v0.9.0
[0.8.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.8...v0.8.0
[0.7.8]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.7...v0.7.8
[0.7.7]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.6...v0.7.7
[0.7.6]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.5...v0.7.6
[0.7.5]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.4...v0.7.5
[0.7.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.3...v0.7.4
[0.7.3]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.2...v0.7.3
[0.7.2]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.7.0...v0.7.2
[0.7.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.4.0
[0.3.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.3.0
[0.2.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.2.1
[0.2.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.2.0
[0.1.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/releases/tag/v0.1.0
[0.14.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.12.0...v0.14.0
[0.15.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.14.0...v0.15.0
[0.15.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.15.0...v0.15.1
[0.15.2]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.15.1...v0.15.2
[0.16.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.15.2...v0.16.0
[0.17.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.16.0...v0.17.0
[0.17.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.17.0...v0.17.1
[0.18.0]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.17.1...v0.18.0
[0.18.1]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.0...v0.18.1
[0.18.2]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.1...v0.18.2
[0.18.3]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.2...v0.18.3
[0.18.4]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.3...v0.18.4
[0.18.5]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.4...v0.18.5
[0.18.6]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.5...v0.18.6
[0.18.7]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.6...v0.18.7
[0.18.8]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.7...v0.18.8
[0.18.9]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.8...v0.18.9
[0.18.10]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.9...v0.18.10
[0.18.11]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.10...v0.18.11
[0.18.12]: https://github.com/CristianRojas-SoftwareEngineer/AI-Voice-InterConnector/compare/v0.18.11...v0.18.12
