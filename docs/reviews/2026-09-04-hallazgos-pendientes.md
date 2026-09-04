# Revisión integrada: hallazgos pendientes — paridad CLI y drifts documentales

- **Fecha**: 2026-09-04
- **Estado**: Abierto — solo pendientes (sin resueltos)
- **Alcance**: Integración de los hallazgos **pendientes** de `docs/reviews/2026-09-02-auditoria-paridad-cli-python-rust.md` (P1-P6) y `docs/PROJECT-REVIEW.md` (S3/S2/S1/S0), arbitrados por `docs/CLI/CONTRACT.md` y `src/main.rs`. **Excluidos por resueltos**: P7 `speech dub` panic `src/main.rs:942` (`55fde2e`, guarda `audio.is_none() && !mic → 2`) y P8 provisión CT2 `src/main.rs:1438`/`1848`/`1555` + `crates/avi-store/src/lib.rs:537` (`b78c3aa`, `hf_cache_dir/ct2` incondicional) y H1-H4 E2E (`1bb7fe1` `ea6472c` `d12050f`, fichero E2E eliminado `8759195`).
- **Naturaleza**: Diagnóstico documental y de código. Cada hallazgo conserva trazabilidad al ID origen.
- **Deduplicación**: P2 ↔ S3-01 (synthesize/say) y P4 ↔ S3-03 (daemon) describen el mismo drift contrato↔código y se fusionan en C-02 y C-03 para evitar doble conteo. El resto se mantiene 1:1.
- **Numeración**: reenumeración por severidad; cada severidad reinicia en 01. Críticos `C-01..C-04`, Medios `M-01..M-06`, Bajos `B-01..B-09`.

## Tabla de contenidos

- 1. Resumen ejecutivo
- 2. Índice reenumerado por severidad (19 hallazgos)
- 3. Hallazgos críticos — Alta / S3 (C-01..C-04)
- 4. Hallazgos medios — Media / S2 (M-01..M-06)
- 5. Hallazgos bajos e informativos — Baja / S1 / S0 (B-01..B-09)
- 6. Trazabilidad origen → nuevo ID
- 7. Orden de corrección recomendado
- 8. Método y fuentes

## 1. Resumen ejecutivo

De 21 hallazgos brutos (8 de paridad + 13 de drift sistémico) quedan **19 distintos pendientes** tras fusionar 2 duplicados. Ninguno fue corregido en `v0.18.26`; el trabajo se detuvo tras P7/P8 y no hay rama en curso.

- **4 críticos (Alta/S3)**: el contrato promete superficie que el binario no implementa. Bloquean integraciones `--json` y onboarding: `cleanup` granular (C-01), `synthesize/say` cross-lingual + payload (C-02), `daemon` sin flags + doc legacy (C-03), `voice clone` modo daemon (C-04).
- **6 medios (S2/Media)**: pérdida funcional o CI/governance roto: `dub` renombrado (M-01), `setup` sin `--force-update/--language` (M-02), `push-to-talk` (M-03), licencias (M-04), artefacto `setup.exe` (M-05), `BUILD.md` drift gcc (M-06).
- **9 bajos/informativo**: detalles stales que rompen copy-paste o dejan ruido: `list --voice` (B-01), `translate` choices (B-02), `--play` interactivo (B-03), `README`/`DESIGN` `0.15.1` (B-04/B-05), `libclang-dev` (B-06), `BaseException` (B-07), `pytest`/`GOAL` (B-08), `CLAUDE.md` genérico (B-09).

El núcleo reproducible (pines, cachés `v2`, `schema_version="3"`, `exit_codes`, `xtask release`) está sincronizado; el drift está confinado a superficie CLI y docs.

## 2. Índice reenumerado por severidad

### 2.1 Críticos — Alta / S3 (4)

| Nuevo ID | Origen | Título | Prioridad | Área | Decisión requerida |
|---|---|---|---|---|---|
| **C-01** | P1 | `cleanup` perdió borrado granular y `--all` cambió a `uninstall` | P0 | CLI/Gestión | Sí |
| **C-02** | P2+S3-01 | `speech synthesize/say` promete flags cross-lingual, overrides y payloads inexistentes | P0 | CLI/Contrato | Sí |
| **C-03** | P4+S3-03 | `daemon start/serve` sin flags y `DAEMON.md` describe arquitectura Python legacy | P1 | CLI/Daemon | Sí |
| **C-04** | S3-02 | `voice clone` promete `--daemon/--no-daemon` inexistente | P0 | CLI/Contrato | Sí |

### 2.2 Medios — Media / S2 (6)

| Nuevo ID | Origen | Título | Prioridad | Área | Decisión requerida |
|---|---|---|---|---|---|
| **M-01** | P3 | `speech dub` renombró `--source-language/--target-language` a `--from/--to` sin actualizar contrato | P1 | CLI/Contrato | Sí |
| **M-02** | P5 | `setup` sin `--force-update/--remove-path/--yes`, `--language` sin choices | P1 | CLI/Setup | Sí |
| **M-03** | S2-01 | `USAGE` push-to-talk sin `--duration` contradice validación `InvalidInput` | P1 | CLI/Docs | Sí |
| **M-04** | S2-02 | `THIRD-PARTY-LICENSES.md` desactualizado `0.13.0` vs `0.18.1` | P1 | Legal/CI | No |
| **M-05** | S2-03 | `MANUAL-VALIDATION`/`GOAL` referencian `setup.exe` Inno Setup obsoleto | P2 | Distribución | No |
| **M-06** | S2-04 | `docs/BUILD.md` no refleja `log on drift` gcc en `build-windows-x64` | P2 | CI/Docs | No |

### 2.3 Bajos e informativos — Baja / S1 / S0 (9)

| Nuevo ID | Origen | Título | Prioridad | Área | Decisión requerida |
|---|---|---|---|---|---|
| **B-01** | P6.1 | `speech list --voice` rechazado (variante unitaria, sin filtro) | P1 | CLI | Sí |
| **B-02** | P6.2 | `translate --from/--to` opcionales sin choices `es|en` | P2 | CLI | No |
| **B-03** | P6.3 | `speech synthesize --play` sin bucle interactivo | P2 | CLI/Contrato §4 | No |
| **B-04** | S1-01 | `README` ejemplos `curl` hardcodean `0.15.1` | P2 | Docs | No |
| **B-05** | S1-02 | `docs/DESIGN.md` árbol y `const VERSION` en `0.15.1` | P2 | Docs | No |
| **B-06** | S1-03 | `README` omite matiz `libclang-dev` condicional | P3 | Docs | No |
| **B-07** | S1-04 | `docs/CLI/README.md` referencia `CliError hereda BaseException` (Python) | P3 | Docs | No |
| **B-08** | S1-05 | `GOAL.md` cita `pytest 795/795` y pesos sin desglose `with-base` | P3 | Docs | No |
| **B-09** | S0-01 | `CLAUDE.md`/`AGENTS.md` genéricos sin `xtask release` | P3 | Docs | No |

## 3. Hallazgos críticos — Alta / S3

### C-01 — `cleanup` perdió borrado granular y `--all` cambió de semántica (ex-P1 → H-01) — ✅ **Resuelto en F4/F6 (2026-09-04)**
- **Categoría**: Rotura de paridad + cambio de semántica — **corregido**
- **Área/plataforma**: `src/main.rs:132`/`318`/`1569` vs `docs/CLI/CONTRACT.md:534` §11 y oráculo `7542962` — ver `F6-drift-docs.md`
- **Síntoma (histórico)**: `cleanup` en Rust aceptaba solo `--all` `src/main.rs:132`; los 5 modos granulares del oráculo no existían. **Corregido en `src/main.rs:132` (6 flags: `voices, synthetic_speech, model, all, dry_run, yes`), `src/main.rs:318` desacoplado (solo `Uninstall` toca binario/PATH), `src/main.rs:1569` gates `sin flags→2`, `dry-run`, `yes`/confirmación.**
- **Evidencia (post-fix)**:

| Superficie | Oráculo `7542962` | `CONTRACT.md §11` | Rust post-F4 |
|---|---|---|---|
| `--synthetic-speech` | ✅ borra raíz de habla sintética | ✅ 534 | ✅ `src/main.rs:132,1652` |
| `--voices` | ✅ voces + locuciones (arrastra namespaces) | ✅ 535, 539 | ✅ `src/main.rs:132,1628` (preserva `FACTORY_VOICES`, arrastre excepto `default`) |
| `--model` | ✅ modelos HF | ✅ 536 | ✅ `src/main.rs:132,1603` (`MODEL_REVISIONS`+xet+ct2) |
| `--dry-run` | ✅ lista sin borrar | ✅ 537 | ✅ `src/main.rs:132,1678` (`removed`/`dry_run:true`) |
| `--yes` | ✅ omite confirmación | ✅ 537 | ✅ `src/main.rs:132,1699` (`-y` alias) |
| `--all` | modelos+voces+habla sintética (sin binario/PATH) | 536 "Modelo + voces + habla sintética — sin binario ni PATH" | ✅ unión en `src/main.rs:1596`, **no delega** en `handle_uninstall` |

   Triple divergencia cerrada: (1) borrado granular restaurado con arrastre §11; (2) `cleanup` sin flags → `2` `usage_error` (`src/main.rs:1589`, `tests/cli_golden.rs` `cleanup_sin_flags_es_exit_2`); (3) `--all` = unión sin binario/PATH, `uninstall` = único con binario/PATH (`src/main.rs:318`). Tests `tests/cli_golden.rs` + fixtures `tests/golden/cli_cleanup_*.json` verdes (F5).
- **Confianza**: Alta (inventarios completos + suite `cargo test --lib`/`cli_golden` verde + reality check binario real F5).
- **Causa**: Port fiel en rutas calientes, pérdida sistemática en periferia de gestión no ejercitada por E2E.
- **Impacto (residual)**: Ninguno — superficie restaurada; docs reconciliados en F6 (`CONTRACT.md §11`, `USAGE.md`, `CLEANUP.md`, transversales).
- **Corrección aplicada**: Restaurados `--voices/--synthetic-speech/--model/--dry-run/--yes` con semántica de arrastre §11 y desacoplado `--all` de `handle_uninstall`; docs reconciliados en `.claude/orchestration/cleanup-granular-2026-09-04/F6-drift-docs.md`.
- **Decisión requerida**: No — cerrada: `--all` = unión (no alias), conjunto 6 flags, `CONTRACT.md §11` fuente de verdad (F0).
- **Prioridad**: P0 — **cerrado**

### C-02 — `speech synthesize/say` sin cross-lingual ni payload prometido (ex-P2 + S3-01 → H-02)
- **Categoría**: Rotura de paridad (parcialmente deliberada por cambio de engine) + drift documental
- **Área/plataforma**: `docs/CLI/CONTRACT.md:166` `506` `587` `619`, `USAGE.md:172` vs `src/main.rs:196` `Synthesize {text,voice,output,label,force,play}` y `src/main.rs:878` `Say` y `crates/avi-core/src/json_emitter.rs:5`
- **Síntoma**: `synthesize`/`say` solo aceptan `--text/--voice` (+ `--label/--output/--force/--play` en `synthesize` y globales `--json/--daemon/--no-daemon`). El oráculo ofrecía `--target-language` (`es-latam|en` default `es-latam`), `--source-language` (traduce antes de sintetizar), `--compute-backend` (`auto|cpu|cuda|mps`), `--exaggeration`, `--cfg-weight`, `--temperature`.
- **Evidencia**: `CONTRACT.md:166` lista `--compute-backend/-cb --source-language --target-language --exaggeration --cfg-weight --temperature` para `synthesize`; `src/main.rs:196` no los declara. Payload `CONTRACT.md:506` `{"voice","label","t3_time","s3gen_time","daemon"}` vs real `src/main.rs:770` `{"status":"success","audio_path","voice"}` y `src/main.rs:878` `{"status":"reproduced","audio_path","voice"}`; `SCHEMA_VERSION="3"` no incluye `t3_time`.
- **Análisis**: Dos naturalezas: (1) cross-lingual integrado — pérdida funcional real, contrato §13 *"`speech say/speech synthesize` reemplazan `--language` por `--target-language"`* (589) pero Rust no tiene ninguno; solo sobrevive vía `dub` (exige audio, no texto); E2E solo ejercitó `dub es→es`; (2) overrides de engine — `--exaggeration/--cfg-weight` son de Chatterbox (engine anterior) y no aplican a Qwen3, pero contrato §13 (619) los sigue prometiendo para `dub`; Qwen3 sí acepta `temperature` (`-T 0.35` en prod E2E) hoy inaccesible.
- **Confianza**: Alta
- **Causa**: Contrato copiado de diseño Python legacy sin reconciliar con `clap` tras migración Rust (0.10-0.12).
- **Impacto**: `unrecognized argument` y parse `--json` roto para consumidores; cross-lingual integrado inutilizable.
- **Corrección propuesta**: **Recomendada** alinear `CONTRACT.md`/`USAGE.md` al código (eliminar flags inexistentes, documentar payload real `status/audio_path/voice`) o implementar flags faltantes en `src/main.rs` (coste alto). Para cross-lingual, restaurar `--target-language/--source-language` en `synthesize/say`; desdocumentar overrides Chatterbox; evaluar exponer `temperature` Qwen3.
- **Decisión requerida**: Sí — ¿contrato o código manda? ¿restaurar cross-lingual?
- **Prioridad**: P0

### C-03 — `daemon start/serve` sin flags y `DAEMON.md` legacy (ex-P4 + S3-03 → H-04) — ✅ **Resuelto problemas 1 y 2 (2026-09-04) — `--auto-restart`/`--max-retries`**
- **Categoría**: Rotura de paridad + drift documental — **corregido para supervisión** (`--language`/`--with-stt` eliminados como residuos legacy, sin retrocompat)
- **Área/plataforma**: `DaemonCommands::{Start,Serve}` `src/main.rs:254` con `--auto-restart`/`--max-retries` (default 3) vs `docs/CLI/commands/DAEMON.md:1` `crates/avi-daemon/src/lib.rs:614` `run_supervised` `crates/avi-daemon/src/spawn.rs:21` `spawn_background` y oráculo `daemon/run.py`
- **Síntoma (histórico)**: 5 variantes unitarias sin flags; oráculo `start: --autorestart --max-retries` y `serve: --auto-restart --max-retries (0=infinito)` + `--language` + `--with-stt`. **Corregido en `src/main.rs:254` (`Start/Serve {auto_restart: bool, max_retries: u32}`), `src/main.rs:1205` `run_supervised` con backoff `500ms*2^retries` capado a 4s, `crates/avi-daemon/src/spawn.rs:21` `spawn_background(auto_restart,max_retries)` con forwarding `--auto-restart`/`--max-retries`. `--language`/`--with-stt` no restaurados — eliminados como legacy (daemon es solo TTS; `language` es de `translate`/`dub`, `with-stt` es feature `native-stt`).**
- **Evidencia (post-fix)**: `DAEMON.md:9` `Axum Router` `crates/avi-daemon/src/lib.rs:1`, `TcpListener::bind→spawn_blocking(warmup_tts)` `crates/avi-daemon/src/lib.rs:614`, `WarmState::Warming`, `tokio::sync::Notify`, `spawn_background` `crates/avi-daemon/src/spawn.rs:21` con `CREATE_NO_HANDLE_INHERIT`; `DAEMON.md:27` ahora `start --auto-restart --max-retries` y `serve --auto-restart --max-retries`; `src/main.rs:262` unificado a `--auto-restart` (kebab, sin alias `--autorestart`); `CONTRACT.md:596` y `USAGE.md:891` sincronizados a `--auto-restart`/`--max-retries` (default 3, graceful `stop` no reintenta). Tests `tests/cli_golden.rs` `daemon_help_lista_auto_restart` y `daemon_start_con_auto_restart` verdes.
- **Confianza**: Alta
- **Causa**: Doc no reescrito tras migración Python→Rust (0.10-0.12) — supervisión nunca portada.
- **Impacto (residual)**: Ninguno para problemas 1 y 2 — supervisión configurable restaurada; onboarding sincronizado; riesgo de reintroducir bugs de handles mitigado por `run_supervised` + `shutdown_notify`.
- **Corrección aplicada**: `src/main.rs:262` parser unificado, `crates/avi-daemon/src/lib.rs:614` `run_supervised` + `spawn.rs:21` forwarding, `src/main.rs:1205` integración `Start/Serve`, docs `CONTRACT.md:596`/`DAEMON.md:1`/`USAGE.md:891` reconciliados, tests `cli_golden` con `--help` y `start --auto-restart`.
- **Decisión requerida**: No — cerradas: nombre `--auto-restart` (sin alias), default `3` (no `0` infinito), ubicación `crates/avi-daemon` supervisando ambas vías.
- **Prioridad**: P1 — **cerrado para 1 y 2; --language/--with-stt descartados como legacy**

### C-04 — `voice clone` promete `--daemon/--no-daemon` inexistente (ex-S3-02 → H-09)
- **Categoría**: Documentación
- **Área/plataforma**: `docs/CLI/CONTRACT.md:238`, `USAGE.md:711` vs `src/main.rs:158` `VoiceCommands::Clone {name,speech_reference,timbre_reference,force}` y `src/main.rs:1765`
- **Evidencia**: `CONTRACT.md:238` `--daemon/--no-daemon` para `voice clone`; `src/main.rs:158` sin flags daemon; `handle_voice` nunca llama `route_to_daemon` `src/main.rs:1765`.
- **Confianza**: Alta
- **Causa**: Extrapolación del patrón 3-modos de `speech` a `voice clone` sin implementación.
- **Impacto**: `voice clone --daemon` → `unrecognized argument`, rompe guía `USAGE.md:713` (precomputar conditionals vía daemon).
- **Corrección propuesta**: **Recomendada** eliminar mención daemon de `voice clone` en `CONTRACT.md`/`USAGE.md` o documentar `local-only`; alternativa implementar despacho daemon (`DaemonMode` + `POST /voices/precompute`).
- **Decisión requerida**: Sí — ¿`voice clone` debe ser delegable al daemon?
- **Prioridad**: P0

## 4. Hallazgos medios — Media / S2

### M-01 — `speech dub` renombró flags de idioma sin reflejarlo en el contrato (ex-P3 → H-03)
- **Categoría**: Renombrado no documentado
- **Área/plataforma**: `docs/CLI/CONTRACT.md:619` §13 vs `src/main.rs:226` `Dub {from,to}`
- **Síntoma**: Oráculo `--source-language` (requerido `es-latam|en`) y `--target-language` (default `es-latam`); contrato §13 (619) documenta esos nombres; Rust usa `--from` (default `es`) y `--to` (default `en`) sin `value_parser` `src/main.rs:226`.
- **Confianza**: Alta
- **Causa**: Renombrado razonable para paridad con `translate`, pero sin sincronizar contrato y sin choices.
- **Impacto**: Flags documentados inexistentes; validación `es-latam/en` → texto libre.
- **Corrección propuesta**: Actualizar `CONTRACT.md` §13 y/o restaurar `value_parser` con valores válidos.
- **Decisión requerida**: Sí — ¿mantener `--from/--to` o restaurar nombres largos?
- **Prioridad**: P1

### M-02 — `setup` sin `--force-update/--remove-path/--yes`, `--language` sin choices (ex-P5 → H-05)
- **Categoría**: Rotura de paridad (parcial: `--uninstall` rediseñado a comando propio)
- **Área/plataforma**: `src/main.rs:127` vs oráculo `7542962` y `docs/CLI/CONTRACT.md:593`
- **Evidencia**:

| Superficie | Oráculo | Rust | Naturaleza |
|---|---|---|---|
| `--force-update` | re-descarga ambos modelos | ❌ | **Pérdida funcional**: E2E tuvo que purgar manualmente ~14 GB |
| `--remove-path` | quita symlink PATH y termina | ❌ (subsumido por `uninstall`) | Rediseño aceptable |
| `--uninstall` | desinstala en un paso | ❌ (rediseñado como `uninstall` + `cleanup --all`) | Rediseño deliberado |
| `--yes` | omite confirmación | ❌ | Perdido |
| `--language` | choices `es-latam|en|all` default `all` | texto libre default `"es"` `src/main.rs:127` | Divergente: contrato 593 promete `setup --language {en,all}` con CT2 — justo la provisión que motivó P8 |

- **Confianza**: Alta
- **Causa**: Port fiel en modelos, pérdida en modos de gestión no ejercitados.
- **Impacto**: Sin forma de re-descargar sin purga manual; `setup --language all` del contrato no existe.
- **Corrección propuesta**: Restaurar `--force-update` (o equivalente) y `value_parser` de `--language`; decidir política de confirmación.
- **Decisión requerida**: Sí
- **Prioridad**: P1

### M-03 — `USAGE` push-to-talk sin `--duration` contradice validación (ex-S2-01 → H-10)
- **Categoría**: Documentación
- **Área/plataforma**: `USAGE.md:381` vs `src/main.rs:652`
- **Evidencia**: `USAGE.md:381` describe `speech transcribe --mic` push-to-talk sin `--duration` en TTY; `src/main.rs:652` `if mic && duration.is_none() → InvalidInput (2) " --mic requiere --duration"` exige duración siempre.
- **Confianza**: Alta
- **Causa**: Doc arrastrado de comportamiento Python `miniaudio` sin alinear con guard Rust.
- **Impacto**: Ejemplo de `USAGE.md` falla con `exit 2`, frustra validación manual.
- **Corrección propuesta**: Alinear `USAGE.md` al guard (`--duration` obligatorio) o relajar `src/main.rs` a push-to-talk si se reimplementa captura.
- **Decisión requerida**: Sí — ¿restaurar push-to-talk o documentar requisito?
- **Prioridad**: P1

### M-04 — `THIRD-PARTY-LICENSES.md` desactualizado (ex-S2-02 → H-11)
- **Categoría**: Documentación/Legal
- **Área/plataforma**: `THIRD-PARTY-LICENSES.md:82` `88` vs `Cargo.toml:3` `0.18.1` `Cargo.lock` `crates/xtask/src/main.rs:434` `826` `.circleci/config.yml:557`
- **Evidencia**: `THIRD-PARTY-LICENSES.md:88` `ai-voice-interconnector | 0.13.0 | GPL` vs `Cargo.toml:3` `0.18.1` y `SOURCE-OFFER.md:3` `0.18.1`; `82` `455 crates` vs `Cargo.lock` actual; `xtask:check_licenses` y job `validate-licenses` comparan `Cargo.lock` vs tabla y fallarían si se ejecutara.
- **Confianza**: Alta
- **Causa**: `bump_version` `crates/xtask/src/main.rs:434` no regenera `THIRD-PARTY-LICENSES.md` (solo `SOURCE-OFFER.md`).
- **Impacto**: Riesgo legal/GPL y gate CI roto; release futuro bloqueado si se activa validación.
- **Corrección propuesta**: `cargo run -p xtask -- licenses` (o `cargo metadata` + render) y commit; añadir regeneración a `xtask release`.
- **Decisión requerida**: No
- **Prioridad**: P1

### M-05 — Artefacto `setup.exe` obsoleto en `MANUAL-VALIDATION`/`GOAL` (ex-S2-03 → H-12)
- **Categoría**: Documentación/Distribución
- **Área/plataforma**: `docs/MANUAL-VALIDATION.md:13` `docs/GOAL.md:152` vs `.circleci/config.yml:925` `docs/BUILD.md:45`
- **Evidencia**: `MANUAL-VALIDATION.md:13` `ai-voice-interconnector-X.Y.Z-x86_64-setup.exe`; artefacto real `config.yml:925` `ai-voice-interconnector-$Version-x86_64-windows.zip` (+ `tar.gz` Unix) con `LICENSE, THIRD-PARTY-LICENSES.md, SOURCE-OFFER.md, README.md` + `ort-bundle` + `vendor/qwen3-tts/qwen_tts.exe`.
- **Confianza**: Alta
- **Causa**: Residuo Inno Setup (eliminado en 0.16.0) no purgado.
- **Impacto**: Validación manual sigue pasos inexistentes; confusión en distribución.
- **Corrección propuesta**: Reescribir `MANUAL-VALIDATION.md` y `GOAL.md:152` al layout plano actual.
- **Decisión requerida**: No
- **Prioridad**: P2

### M-06 — `docs/BUILD.md` no refleja `log on drift` gcc (ex-S2-04 → H-13)
- **Categoría**: Documentación/CI
- **Área/plataforma**: `docs/BUILD.md:398` vs `.circleci/config.yml:772` `CHANGELOG.md:93` (0.17.1)
- **Evidencia**: `config.yml:772` `if ($GccInstalled -ne $GccPin) { Write-Host "[WARN] drift..." }` no aborta; `docs/BUILD.md:419` describe determinismo sin mencionar modo warn.
- **Confianza**: Media
- **Causa**: Doc no actualizado tras relajación del guard `build-windows-x64` en 0.17.1.
- **Impacto**: Expectativa `fail-fast` vs real `warn-continue`; dificulta diagnóstico de drift.
- **Corrección propuesta**: Añadir nota `log on drift` en `docs/BUILD.md:419`.
- **Decisión requerida**: No
- **Prioridad**: P2

## 5. Hallazgos bajos e informativos — Baja / S1 / S0

### B-01 — `speech list --voice` rechazado (ex-P6.1 → H-06)
- **Categoría**: Rotura de paridad menor
- **Área/plataforma**: `SpeechCommands::List` unitaria `src/main.rs:181` vs `docs/CLI/CONTRACT.md:170` `278` `321` y oráculo `_require_voice_exists` + `list_entries(voice=...)`
- **Síntoma E2E**: `speech list --voice mi_voz --json` rechazado exit 2; guion `.claude/skills/test-windows-e2e-as-final-user` paso 6 lo ordena esperando lista filtrada client-side sobre `{"speech":[{label,voice,…}]}`. Oráculo validaba exit 3 y filtraba; contrato lo promete (170, 278, 321).
- **Confianza**: Alta — hallazgo que originó la auditoría de paridad (H5 E2E).
- **Causa**: Port omitió flag no ejercitado por tests dorados.
- **Impacto**: Guion E2E falla; UX de distinguir "voz mal escrita" de "sin resultados" (contrato 278) perdida.
- **Corrección propuesta**: Restaurar `--voice` con validación exit 3 y filtrado; no desdocumentar.
- **Decisión requerida**: Sí
- **Prioridad**: P1

### B-02 — `translate --from/--to` sin choices (ex-P6.2 → H-07)
- **Categoría**: Drift menor
- **Área/plataforma**: `src/main.rs:105` vs oráculo `required choices es|en`
- **Evidencia**: Rust opcionales `default es/en` sin `value_parser` `src/main.rs:105`; oráculo requeridos con choices.
- **Confianza**: Alta
- **Causa**: Default razonable añadido sin documentar choices.
- **Impacto**: Menor; texto libre permite valores inválidos sin error temprano.
- **Corrección propuesta**: Documentar choices o restaurar `value_parser`; el default puede quedarse.
- **Decisión requerida**: No
- **Prioridad**: P2

### B-03 — `speech synthesize --play` sin bucle interactivo (ex-P6.3 → H-08)
- **Categoría**: Cambio de semántica no reflejado
- **Área/plataforma**: `docs/CLI/CONTRACT.md:4` §4 vs `src/main.rs:853`
- **Evidencia**: Contrato §4 documenta bucle interactivo (reproduce y pregunta antes de guardar, incompatible con `--json`); Rust reproduce y guarda incondicionalmente `src/main.rs:853` y es compatible con `--json`.
- **Confianza**: Alta
- **Causa**: Cambio de diseño no sincronizado con contrato.
- **Impacto**: Comportamiento documentado inexistente.
- **Corrección propuesta**: Sincronizar `CONTRACT.md` §4 con el comportamiento real o restaurar bucle.
- **Decisión requerida**: No
- **Prioridad**: P2

### B-04 — `README` ejemplos `curl` hardcodean `0.15.1` (ex-S1-01 → H-14)
- **Área**: `README.md:103` `107` `111` vs `src/main.rs:27` `Cargo.toml:3` `0.18.1` — `ai-voice-interconnector-0.15.1-x86_64-*` vs versión real. Causa: `cargo xtask release` no parametriza ejemplos. Impacto: copy-paste roto. Corrección: parametrizar con `v$(grep VERSION src/main.rs)` o placeholder `X.Y.Z`.
- **Prioridad**: P2 — Decisión No — Confianza Alta

### B-05 — `docs/DESIGN.md` árbol y `const VERSION` en `0.15.1` (ex-S1-02 → H-15)
- **Área**: `docs/DESIGN.md:84` `99` `version = 0.15.1` `const VERSION = "0.15.1"` — bump no actualiza comentarios. Corrección: sincronizar a `0.18.1` o placeholder.
- **Prioridad**: P2 — Decisión No — Confianza Alta

### B-06 — `README` omite matiz `libclang-dev` condicional (ex-S1-03 → H-16)
- **Área**: `README.md:144` vs `docs/BUILD.md:26` `solo con --features native-translation/full` — `README` lo lista como requisito general. Causa: generalización. Corrección: añadir matiz en `README.md`.
- **Prioridad**: P3 — Decisión No — Confianza Media

### B-07 — `docs/CLI/README.md` referencia Python muerta (ex-S1-04 → H-17)
- **Área**: `docs/CLI/README.md:79` `CliError hereda de BaseException` — residuo Python no limpiado. Corrección: eliminar o referenciar `crates/avi-core/src/exit_codes.rs`.
- **Prioridad**: P3 — Decisión No — Confianza Alta

### B-08 — `GOAL.md` cita `pytest 795/795` y pesos sin desglose (ex-S1-05 → H-18)
- **Área**: `docs/GOAL.md:175` `202` `pytest pasan (795/795)` vs `cargo test` Rust; `GOAL.md:175` descarga sin desglose `~9GB / ~11.5GB con --with-base`. Causa: roadmap no actualizado tras migración Rust. Corrección: actualizar a `cargo test` y desglose de pesos.
- **Prioridad**: P3 — Decisión No — Confianza Media

### B-09 — `CLAUDE.md`/`AGENTS.md` genéricos (ex-S0-01 → H-19)
- **Área**: `CLAUDE.md:1` `AGENTS.md:1` vs `.claude/skills/release/SKILL.md` — solo guías de estilo (`language, think-before-coding, simplicity`) sin mención a `xtask release`, CI o `codebase-memory-mcp`. Causa: plantilla sin adaptación. Corrección: añadir referencia a `docs/RELEASING.md`/`release` skill.
- **Prioridad**: P3 — Decisión No — Confianza Media — Informativo

## 6. Trazabilidad origen → nuevo ID

| Nuevo ID | Origen | ID origen | ID previo H- | Nota fusión |
|---|---|---|---|---|
| **C-01** | Paridad | P1 | H-01 | — |
| **C-02** | Paridad+Drift | P2 + S3-01 | H-02 | Fusionado (mismo drift contrato) |
| **C-03** | Paridad+Drift | P4 + S3-03 | H-04 | Fusionado (flags daemon + doc legacy) |
| **C-04** | Drift | S3-02 | H-09 | — |
| **M-01** | Paridad | P3 | H-03 | — |
| **M-02** | Paridad | P5 | H-05 | — |
| **M-03** | Drift | S2-01 | H-10 | — |
| **M-04** | Drift | S2-02 | H-11 | — |
| **M-05** | Drift | S2-03 | H-12 | — |
| **M-06** | Drift | S2-04 | H-13 | — |
| **B-01** | Paridad | P6.1 | H-06 | Desglosado de P6 |
| **B-02** | Paridad | P6.2 | H-07 | Desglosado de P6 |
| **B-03** | Paridad | P6.3 | H-08 | Desglosado de P6 |
| **B-04** | Drift | S1-01 | H-14 | — |
| **B-05** | Drift | S1-02 | H-15 | — |
| **B-06** | Drift | S1-03 | H-16 | — |
| **B-07** | Drift | S1-04 | H-17 | — |
| **B-08** | Drift | S1-05 | H-18 | — |
| **B-09** | Drift | S0-01 | H-19 | — |

Excluidos por resueltos: P7 (`55fde2e`), P8 (`b78c3aa`+`4fbe77e`), H1-H4 E2E (`1bb7fe1` `ea6472c` `d12050f`).

## 7. Orden de corrección recomendado

**Fase 1 — P0 bloqueante de contrato (sin esto toda integración `--json` parte de spec falsa):** C-01, C-02, C-04 — definir fuente de verdad (¿contrato o código?) y restaurar o desdocumentar `cleanup` granular, `synthesize/say` cross-lingual/payload y `voice clone` daemon. `C-01 --all` debe dejar de significar dos cosas.

**Fase 2 — P1 habilita CI/governance y paridad funcional:** C-03, M-02, B-01, M-03, M-04 — `daemon` (C-03) y `setup --force-update/--language` (M-02) + `list --voice` (B-01, hallazgo E2E original), `USAGE` push-to-talk (M-03) y `THIRD-PARTY-LICENSES` (M-04) con gate `validate-licenses` `config.yml:557`.

**Fase 3 — P2 distribución y coherencia:** M-01, B-02, B-03, M-05, M-06, B-04, B-05 — renombrados `dub`/`translate` (M-01/B-02) + `--play` (B-03), artefactos `zip/tar.gz` (M-05), `log on drift` (M-06), ejemplos `README`/`DESIGN` (B-04/B-05).

**Fase 4 — P3 backlog cosmético:** B-06, B-07, B-08, B-09 — matices `libclang-dev`, `BaseException`, `pytest`→`cargo test`, `CLAUDE.md`→`release` skill.

Todo cambio de contrato debe acompañarse de un **drift-detector**: test que afirme `CONTRACT.md` contra `--help` del binario y gates E2E que cubran provisión (lección P8: un diff de flags no ve comportamiento).

## 8. Método y fuentes

- **Oráculo Python**: `7542962` (2026-08-25) `src/ai_voice_interconnector/cli.py` + `daemon/run.py`; superficie `add_parser/add_argument` verificada idéntica tras rename `ca7d00c`.
- **CLI Rust**: `HEAD` `src/main.rs:55` (`Cli` + `Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`) — ningún otro binario aporta superficie.
- **Árbitro contrato**: `docs/CLI/CONTRACT.md` (normativo, ya en stack Rust `crates/avi-stt/src/parakeet.rs` §11).
- **Drifts documentales**: `README.md`, `USAGE.md`, `docs/BUILD.md`, `docs/CLI/commands/DAEMON.md`, `docs/DESIGN.md`, `docs/GOAL.md`, `THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`, `docs/CLI/README.md`, `CLAUDE.md`/`AGENTS.md` contra `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `.circleci/config.yml`, `crates/xtask/src/main.rs`, `tests/cli_golden.rs`, `crates/avi-daemon`.
- **Limitación**: comparación de superficie de parseo/validación visible; no audita fidelidad de payload runtime ni daemon HTTP interno.
