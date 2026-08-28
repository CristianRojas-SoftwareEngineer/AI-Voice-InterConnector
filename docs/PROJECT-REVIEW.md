# Revisión: Drifts documentales — documentación vs código/CI (sistémica, sin perfil)

## Resumen ejecutivo

Auditoría sistémica completa de drifts documentales con lente `documentación vs código/CI`, sin perfil de mantenimiento, tras el pipeline #96 (`v0.18.1`, `build-all` verde). Se barrió `README.md`, `CLAUDE.md`, `AGENTS.md`, `USAGE.md`, `CONTRIBUTING.md`, `docs/BUILD.md`, `docs/CLI/CONTRACT.md`, `docs/CLI/README.md`, `docs/CLI/commands/DAEMON.md`, `docs/DAEMON-MODE.md`, `docs/DESIGN.md`, `docs/GOAL.md`, `CHANGELOG.md`, `THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`, `.claude/skills/release/SKILL.md` contra `src/main.rs`, `Cargo.toml`, `.circleci/config.yml`, `crates/xtask/src/main.rs`, `tests/cli_golden.rs` y `crates/avi-daemon`. Veredicto: el núcleo reproducible (pines, cachés `v2`, `schema_version="3"`, `exit_codes`, flujos `xtask release`) está sincronizado; persisten 3 drifts críticos de contrato CLI/daemon, 4 medios de artefactos/inventario y 5 bajos de ejemplos stales. Conteo: **0 S4, 3 S3, 4 S2, 5 S1, 1 S0**.

### Índice de hallazgos

| ID | Título | Severidad | Prioridad | Área/plataforma | Decisión requerida | Estado |
|----|--------|-----------|-----------|-----------------|--------------------|--------|
| S3-01 | Contrato `speech synthesize/say` promete flags y payloads inexistentes | S3 — Alto | P0 | Documentación/CLI | Sí | Pendiente |
| S3-02 | `voice clone` promete modo daemon `--daemon/--no-daemon` inexistente | S3 — Alto | P0 | Documentación/CLI | Sí | Pendiente |
| S3-03 | `docs/CLI/commands/DAEMON.md` describe arquitectura Python legacy (FastAPI/uvicorn) | S3 — Alto | P1 | Documentación/Daemon | Sí | Pendiente |
| S2-01 | `USAGE` push-to-talk sin `--duration` contradice validación `InvalidInput` | S2 — Medio | P1 | Documentación/CLI | Sí | Pendiente |
| S2-02 | `THIRD-PARTY-LICENSES.md` desactualizado (0.13.0 vs 0.18.1) — gate `validate-licenses` fallaría | S2 — Medio | P1 | Documentación/Legal | No | Pendiente |
| S2-03 | `MANUAL-VALIDATION` y `GOAL` referencian artefacto `setup.exe` Inno Setup obsoleto | S2 — Medio | P2 | Documentación/Distribución | No | Pendiente |
| S2-04 | `docs/BUILD.md` no refleja relajación `log on drift` de gcc en `build-windows-x64` | S2 — Medio | P2 | Documentación/CI | No | Pendiente |
| S1-01 | `README` ejemplos `curl` hardcodean `0.15.1` | S1 — Bajo | P2 | Documentación | No | Pendiente |
| S1-02 | `docs/DESIGN.md` árbol y `const VERSION` comentan `0.15.1` | S1 — Bajo | P2 | Documentación | No | Pendiente |
| S1-03 | `README` omite matiz `libclang-dev` condicional (`native-translation/full`) | S1 — Bajo | P3 | Documentación | No | Pendiente |
| S1-04 | `docs/CLI/README.md` referencia `CliError hereda BaseException` (Python muerto) | S1 — Bajo | P3 | Documentación | No | Pendiente |
| S1-05 | `GOAL.md` cita `pytest 795/795` y pesos sin detalle `with-base` | S1 — Bajo | P3 | Documentación | No | Pendiente |
| S0-01 | `CLAUDE.md`/`AGENTS.md` genéricos sin referencia a flujo `xtask release` | S0 — Informativo | P3 | Documentación | No | Pendiente |

## Hallazgos por severidad

### S3 — Altos

#### S3-01 — Contrato `speech synthesize/say` promete flags y payloads inexistentes
- **Categoría**: Documentación
- **Área/plataforma**: `docs/CLI/CONTRACT.md:166-167`, `USAGE.md:172-184` vs `src/main.rs:198-214`
- **Evidencia**: `docs/CLI/CONTRACT.md:166` lista `--compute-backend/-cb --source-language --target-language --exaggeration --cfg-weight --temperature` para `synthesize`; `src/main.rs:198-214` `Synthesize { text, voice, output, label, force, play }` no los declara. Payload `CONTRACT.md:506` `{"voice","label","t3_time","s3gen_time","daemon"}` vs `src/main.rs:770-779,829-835` `{"status":"success","audio_path","voice"}` y `src/main.rs:878-884,1993-2016` `say` `{"status":"reproduced","audio_path","voice"}`. `crates/avi-core/src/json_emitter.rs:5` `SCHEMA_VERSION="3"` no incluye `t3_time`.
- **Confianza**: Alta
- **Causa**: Contrato copiado de diseño Python/legacy sin reconciliar con `clap` real tras migración Rust.
- **Impacto**: Integración rota para consumidores `--json` (parse fallido, flags rechazados con `unrecognized argument`). Riesgo alto antes de nueva funcionalidad.
- **Corrección(es) propuesta(s)**: **Recomendada:** alinear `CONTRACT.md`/`USAGE.md` al código (eliminar flags inexistentes, documentar payload real `status/audio_path/voice`). Alternativa: implementar flags/payload faltantes en `src/main.rs` (coste alto, requiere motor).
- **Decisión requerida**: Sí — ¿contrato manda o código manda? Definir fuente de verdad.
- **Prioridad**: P0

#### S3-02 — `voice clone` promete modo daemon `--daemon/--no-daemon` inexistente
- **Categoría**: Documentación
- **Área/plataforma**: `docs/CLI/CONTRACT.md:238-240`, `USAGE.md:711-713` vs `src/main.rs:158-171,470-587`
- **Evidencia**: `CONTRACT.md:238` `--daemon/--no-daemon` para `voice clone`; `src/main.rs:158-171` `VoiceCommands::Clone { name, speech_reference, timbre_reference, force }` sin flags daemon; `handle_voice` nunca llama `route_to_daemon` (`src/main.rs:1765`).
- **Confianza**: Alta
- **Causa**: Extrapolación del patrón 3-modos de `speech` a `voice clone` sin implementación.
- **Impacto**: Usuario espera `voice clone --daemon` para precomputar conditionals vía daemon; falla con `unrecognized argument`, rompe guía de `USAGE.md:713`.
- **Corrección(es) propuesta(s)**: **Recomendada:** eliminar mención daemon de `voice clone` en `CONTRACT.md`/`USAGE.md` o documentar `local-only`. Alternativa: implementar despacho daemon en `handle_voice` (añadir `DaemonMode` y `POST /voices/precompute`).
- **Decisión requerida**: Sí — ¿`voice clone` debe ser delegable al daemon?
- **Prioridad**: P0

#### S3-03 — `docs/CLI/commands/DAEMON.md` describe arquitectura Python legacy
- **Categoría**: Documentación
- **Área/plataforma**: `docs/CLI/commands/DAEMON.md:1-11,23-30` vs `src/main.rs:257-268,1140-1293`, `crates/avi-daemon/src/lib.rs:1-8,614-628`, `crates/avi-daemon/src/spawn.rs:21-42`
- **Evidencia**: `DAEMON.md:9` `FastAPI/uvicorn`, `daemon/server.py:2708`, `protocol.py`, `Pydantic ProtocolModel`, `DaemonManager.start() O_CREAT|O_EXCL`; código real usa `Axum Router`, `TcpListener::bind` → `spawn_blocking(warmup_tts)`, `WarmState::Warming`, `tokio::sync::Notify` shutdown, `spawn_background` con `CREATE_NO_HANDLE_INHERIT|CREATE_NO_WINDOW`. Tabla `DAEMON.md:27` `start --autorestart --max-retries --language --with-stt` vs `src/main.rs:257` `Start` sin args.
- **Confianza**: Alta
- **Causa**: Doc no reescrito tras migración Python→Rust (0.10-0.12).
- **Impacto**: Onboarding y contribuciones parten de modelo mental falso; riesgo de reintroducir bugs de herencia de handles/pipe ya corregidos en `tests/cli_golden.rs:40-54`.
- **Corrección(es) propuesta(s)**: **Recomendada:** reescribir `DAEMON.md` desde `src/main.rs:1140` y `crates/avi-daemon/src/lib.rs:158-176,614-644` (5 subcomandos reales, `DAEMON_ADDR 127.0.0.1:8765`, estados `warming/warm/warm_failed`). Mantener `DAEMON-MODE.md` como referencia (ya sincronizado `DAEMON-MODE.md:25-34`).
- **Decisión requerida**: Sí — ¿archivar `DAEMON.md` legacy o reescribirlo como contrato Rust?
- **Prioridad**: P1

### S2 — Medios

#### S2-01 — `USAGE` push-to-talk sin `--duration` contradice validación
- **Categoría**: Documentación
- **Área/plataforma**: `USAGE.md:381-386` vs `src/main.rs:652-658`
- **Evidencia**: `USAGE.md:381` describe `speech transcribe --mic` push-to-talk sin `--duration` en TTY; `src/main.rs:652` `if mic && duration.is_none() → InvalidInput (2) " --mic requiere --duration"` exige duración siempre.
- **Confianza**: Alta
- **Causa**: Doc arrastrado de comportamiento Python `miniaudio` sin alinear con guard Rust.
- **Impacto**: Ejemplo de `USAGE.md` falla con `exit 2`, frustra validación manual.
- **Corrección(es) propuesta(s)**: **Recomendada:** alinear `USAGE.md` al guard (`--duration` obligatorio) o relajar `src/main.rs` a push-to-talk si se reimplementa captura. 
- **Decisión requerida**: Sí — ¿restaurar push-to-talk o documentar requisito?
- **Prioridad**: P1

#### S2-02 — `THIRD-PARTY-LICENSES.md` desactualizado
- **Categoría**: Documentación
- **Área/plataforma**: `THIRD-PARTY-LICENSES.md:82,88` vs `Cargo.toml:3`, `Cargo.lock`, `crates/xtask/src/main.rs:826-836`, `.circleci/config.yml:557-591`
- **Evidencia**: `THIRD-PARTY-LICENSES.md:88` `ai-voice-interconnector | 0.13.0 | GPL` vs `Cargo.toml:3` `0.18.1`, `SOURCE-OFFER.md:3` `0.18.1`; `THIRD-PARTY-LICENSES.md:82` `455 crates` vs `Cargo.lock` actual. `xtask:check_licenses` y job `validate-licenses` comparan `Cargo.lock` vs tabla y fallarían si se ejecutara.
- **Confianza**: Alta
- **Causa**: `bump_version` (`crates/xtask/src/main.rs:434-467`) no regenera `THIRD-PARTY-LICENSES.md` (solo `SOURCE-OFFER.md`).
- **Impacto**: Riesgo legal/GPL y gate CI roto; release futuro bloqueado si se activa validación.
- **Corrección(es) propuesta(s)**: **Recomendada:** `cargo run -p xtask -- licenses` (o `cargo metadata` + render) y commit; añadir regeneración a `xtask release` para atomizar. 
- **Decisión requerida**: No
- **Prioridad**: P1

#### S2-03 — Artefacto `setup.exe` obsoleto en `MANUAL-VALIDATION`/`GOAL`
- **Categoría**: Documentación
- **Área/plataforma**: `docs/MANUAL-VALIDATION.md:13-16`, `docs/GOAL.md:152` vs `.circleci/config.yml:925`, `docs/BUILD.md:45-51`
- **Evidencia**: `MANUAL-VALIDATION.md:13` `ai-voice-interconnector-X.Y.Z-x86_64-setup.exe`; artefacto real `config.yml:925` `ai-voice-interconnector-$Version-x86_64-windows.zip` (+ `tar.gz` Unix) con `LICENSE, THIRD-PARTY-LICENSES.md, SOURCE-OFFER.md, README.md` + `ort-bundle` + `vendor/qwen3-tts/qwen_tts.exe`.
- **Confianza**: Alta
- **Causa**: Residuo Inno Setup (eliminado en 0.16.0) no purgado de guías de validación.
- **Impacto**: Validación manual sigue pasos inexistentes; confusión en distribución.
- **Corrección(es) propuesta(s)**: Reescribir `MANUAL-VALIDATION.md` y `GOAL.md:152` al layout plano actual.
- **Decisión requerida**: No
- **Prioridad**: P2

#### S2-04 — `docs/BUILD.md` no refleja `log on drift` de gcc
- **Categoría**: Documentación
- **Área/plataforma**: `docs/BUILD.md:398-420` vs `.circleci/config.yml:769-780`, `CHANGELOG.md:93-99` (0.17.1)
- **Evidencia**: `config.yml:772-778` `if ($GccInstalled -ne $GccPin) { Write-Host "[WARN] drift..." }` no aborta; `docs/BUILD.md:419` aún describe determinismo sin mencionar modo warn.
- **Confianza**: Media
- **Causa**: Doc no actualizado tras relajación del guard `build-windows-x64` en 0.17.1.
- **Impacto**: Expectativa de `fail-fast` vs comportamiento real `warn-continue`; dificulta diagnóstico de drift.
- **Corrección(es) propuesta(s)**: Añadir nota `log on drift` en `docs/BUILD.md:419`.
- **Decisión requerida**: No
- **Prioridad**: P2

### S1 — Bajos

#### S1-01 — `README` ejemplos `curl` hardcodean `0.15.1`
- **Categoría**: Documentación
- **Área/plataforma**: `README.md:103,107,111` vs `src/main.rs:27`, `Cargo.toml:3`
- **Evidencia**: `README.md:103` `ai-voice-interconnector-0.15.1-x86_64-*` vs versión real `0.18.1`.
- **Confianza**: Alta
- **Causa**: Bump `cargo xtask release` no parametriza ejemplos de descarga.
- **Impacto**: Reproducibilidad rota en copy-paste.
- **Corrección(es) propuesta(s)**: Parametrizar con `v$(grep VERSION src/main.rs)` o `X.Y.Z` placeholder.
- **Decisión requerida**: No
- **Prioridad**: P2

#### S1-02 — `docs/DESIGN.md` árbol y `const VERSION` comentan `0.15.1`
- **Categoría**: Documentación
- **Área/plataforma**: `docs/DESIGN.md:84,99`
- **Evidencia**: `DESIGN.md:84` `version = 0.15.1`, `DESIGN.md:99` `const VERSION = "0.15.1"`.
- **Confianza**: Alta
- **Causa**: Bump no actualiza comentarios de diseño.
- **Impacto**: Confusión menor de arquitectura.
- **Corrección(es) propuesta(s)**: Sincronizar a `0.18.1` o usar placeholder.
- **Decisión requerida**: No
- **Prioridad**: P2

#### S1-03 — `README` omite matiz `libclang-dev` condicional
- **Categoría**: Documentación
- **Área/plataforma**: `README.md:144` vs `docs/BUILD.md:26`
- **Evidencia**: `README.md:144` `libclang-dev` como requisito general; `docs/BUILD.md:26` `solo con --features native-translation/full`.
- **Confianza**: Media
- **Causa**: Generalización de requisito condicional.
- **Impacto**: Instalación innecesaria en featureless.
- **Corrección(es) propuesta(s)**: Añadir matiz en `README.md`.
- **Decisión requerida**: No
- **Prioridad**: P3

#### S1-04 — `docs/CLI/README.md` referencia Python muerta
- **Categoría**: Documentación
- **Área/plataforma**: `docs/CLI/README.md:79`
- **Evidencia**: `README.md:79` `CliError hereda de BaseException`.
- **Confianza**: Alta
- **Causa**: Residuo Python no limpiado.
- **Impacto**: Ruido menor.
- **Corrección(es) propuesta(s)**: Eliminar línea o referenciar `crates/avi-core/src/exit_codes.rs`.
- **Decisión requerida**: No
- **Prioridad**: P3

#### S1-05 — `GOAL.md` cita `pytest 795/795` y pesos sin detalle `with-base`
- **Categoría**: Documentación
- **Área/plataforma**: `docs/GOAL.md:175,202`
- **Evidencia**: `GOAL.md:202` `pytest pasan (795/795)` vs `cargo test` Rust; `GOAL.md:175` descarga sin desglose `~9GB / ~11.5GB con --with-base`.
- **Confianza**: Media
- **Causa**: Roadmap no actualizado tras migración Rust.
- **Impacto**: Expectativa de stack obsoleta.
- **Corrección(es) propuesta(s)**: Actualizar a `cargo test` y desglose de pesos.
- **Decisión requerida**: No
- **Prioridad**: P3

### S0 — Informativos

#### S0-01 — `CLAUDE.md`/`AGENTS.md` genéricos
- **Categoría**: Documentación
- **Área/plataforma**: `CLAUDE.md:1`, `AGENTS.md:1` vs `.claude/skills/release/SKILL.md`
- **Evidencia**: Ambos contienen solo guías de estilo (`language, think-before-coding, simplicity`), sin mención a `xtask release`, CI o `codebase-memory-mcp` no usado.
- **Confianza**: Media
- **Causa**: Plantilla sin adaptación a flujo Rust actual.
- **Impacto**: No bloqueante; fuente de verdad es `release/SKILL.md`.
- **Corrección(es) propuesta(s)**: Añadir referencia a `docs/RELEASING.md`/`release` skill.
- **Decisión requerida**: No
- **Prioridad**: P3

## Orden de corrección recomendado

**Fase 1 — P0 (bloqueante de contrato):** `S3-01`, `S3-02` — alinear `CONTRACT.md`/`USAGE.md` al `src/main.rs` real (definir si `synthesize/say` y `voice clone` deben exponer flags daemon/payloads). Sin esto toda nueva integración `--json` parte de especificación falsa.

**Fase 2 — P1 (alto, habilita CI/governance):** `S3-03`, `S2-01`, `S2-02` — reescribir `DAEMON.md` a Axum, corregir push-to-talk en `USAGE`, regenerar `THIRD-PARTY-LICENSES.md` y gatear `validate-licenses`.

**Fase 3 — P2 (medios, distribución):** `S2-03`, `S2-04`, `S1-01`, `S1-02` — artefactos `zip/tar.gz`, `log on drift`, ejemplos y `DESIGN.md` stales.

**Fase 4 — P3 (backlog cosmético):** `S1-03`, `S1-04`, `S1-05`, `S0-01`.

## Confirmación en CI

Hallazgos probados por lectura de código; confirmación multiplataforma en ejecución:
- `S3-01`/`S3-02`/`S2-01`: `cargo test --all` en `test-windows`/`test-linux`/`test-macos` ya validan `unrecognized argument` y `exit 2` para flags inexistentes; verificar que `CONTRACT.md` corregido no reintroduce flags no declarados en `src/main.rs:198-214,158-171`.
- `S3-03`: `build-windows-x64` y `test-windows` validan `spawn_background` (`crates/avi-daemon/src/spawn.rs:42`) y `health/warm` (`crates/avi-daemon/src/lib.rs:614`); doc reescrito debe coincidir con `DAEMON-MODE.md:25` ya verde.
- `S2-02`: `validate-licenses` (`config.yml:557`) confirmará `THIRD-PARTY-LICENSES.md` tras regeneración.
- `S2-03`/`S1-01`: `build-*` `Preparar artefacto versionado` (`config.yml:889`) confirmará artefactos `zip/tar.gz` en siguiente tag `v*`.

