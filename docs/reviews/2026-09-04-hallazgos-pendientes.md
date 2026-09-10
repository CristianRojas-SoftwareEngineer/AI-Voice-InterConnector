# Revisión integrada: hallazgos pendientes — paridad CLI y drifts documentales

- **Fecha de auditoría**: 2026-09-04 (hallazgos) · 2026-09-05 (auditoría contra HEAD) · 2026-09-05 (re-auditoría bidireccional falsos positivos/negativos) · 2026-09-10 (re-verificación: C-02 y M-01 cerrados por estrategia C, C-05 nuevo de F5)
- **HEAD auditado**: `79b87bb` · **Versión**: `0.18.26`
- **Estado**: 13 resueltos/purgados — 7 pendientes — 0 drift documentales
- **Alcance**: Integración de los hallazgos **pendientes** de `docs/reviews/2026-09-02-auditoria-paridad-cli-python-rust.md` (P1-P6) y `docs/PROJECT-REVIEW.md` (S3/S2/S1/S0), arbitrados por `docs/CLI/CONTRACT.md` y `src/main.rs`. **Excluidos por resueltos**: P7 `speech dub` panic `src/main.rs:942` (`55fde2e`, guarda `audio.is_none() && !mic → 2`) y P8 provisión CT2 `src/main.rs:1438`/`1848`/`1555` + `crates/avi-store/src/lib.rs:537` (`b78c3aa`, `hf_cache_dir/ct2` incondicional) y H1-H4 E2E (`1bb7fe1` `ea6472c` `d12050f`, fichero E2E eliminado `8759195`).
- **Naturaleza**: Diagnóstico documental y de código. Cada hallazgo conserva trazabilidad al ID origen.

> **Leyenda de estado**: ✅ **Resuelto** — la paridad/consistencia se verifica contra HEAD · ⏳ **Pendiente** — la discrepancia persiste · 🔄 **Drift documental** — el código es correcto pero la doc no refleja HEAD. Cada hallazgo incluye la evidencia empírica obtenida con `cargo run -- <cmd> --help`.

## Tabla de contenidos

- 1. Resumen ejecutivo
- 2. Índice reenumerado por severidad (20 hallazgos)
- 3. Hallazgos críticos — Alta / S3 (C-01..C-05)
- 4. Hallazgos medios — Media / S2 (M-01..M-06)
- 5. Hallazgos bajos e informativos — Baja / S1 / S0 (B-01..B-09)
- 6. Trazabilidad origen → nuevo ID
- 7. Orden de corrección recomendado
- 8. Método y fuentes

## 1. Resumen ejecutivo

De 21 hallazgos brutos (8 de paridad + 13 de drift sistémico) quedan **19 distintos** tras fusionar 2 duplicados (P2↔S3-01, P4↔S3-03), más **C-05** (motor CT2, origen F5 2026-09-10): **20** en el índice. Auditoría contra HEAD `79b87bb` (v0.18.26) el 2026-09-05 mediante 5 subagentes paralelos:

| Estado | Cantidad | IDs |
|---|---|---|
| ✅ Resuelto | 13 | C-01, C-02, M-01, M-03, M-04, M-05, M-06, B-04, B-05, B-06, B-07, B-08, B-09 |
| ⏳ Pendiente | 7 | C-03, C-04, C-05, M-02, B-01, B-02, B-03 |
| 🔄 Drift documental | 0 | (ninguno) |

- **Críticos (Alta/S3)**: C-01 resuelto (cleanup granular restaurado). **C-03 y C-04 re-auditados como falsos positivos parciales** (2026-09-05): C-03 — la supervisión `--auto-restart`/`--max-retries` está corregida, pero `--language`/`--with-stt` (features reales del oráculo Python, no residuos legacy) no fueron portadas al Rust; C-04 — los flags `--daemon/--no-daemon` existen como global args y el routing funciona, pero el endpoint `POST /voices/precompute` (precompute de conditionals) fue purgado del daemon, rompiendo el contrato `CONTRACT.md:238,241`. **C-02 cerrado 2026-09-10** (estrategia C, `07aab20`): `--source-language`/`--target-language` (cross-lingual) y `--temperature` reimplementados; `--exaggeration`/`--cfg-weight`/`--compute-backend` (específicos de Chatterbox) purgados del contrato.
- **Medios (S2/Media)**: M-01, M-03, M-04, M-05, M-06 cerrados/purgados. M-02 persiste: `setup` carece de `--force-update/--remove-path/--yes` con `--language` sin choices.
- **Bajos/informativo**: B-04..B-09 purgados (versiones, referencias condicionales, residuos Python). B-01, B-02, B-03 persisten (pendientes de implementación: flags de filtrado, choices, loop interactivo).

El núcleo reproducible (pines, cachés `v2`, `schema_version="3"`, `exit_codes`, `xtask release`) está sincronizado; lo pendiente: (1) **feature gap de migración** — ✅ cerrado (C-02, estrategia C `07aab20`); (2) **daemon flags migración** — `--language`/`--with-stt` perdidos en `daemon serve/start` (C-03, falso positivo: eran features reales del oráculo, no legacy); (3) **daemon precompute** — endpoint `POST /voices/precompute` purgado del daemon, rompe `CONTRACT.md:238,241` (C-04, falso positivo: flag existe pero precompute no funciona); (4) **paridad funcional** — `dub` rename sin sincronizar contrato (M-01), `setup` sin `--force-update/--remove-path/--yes` con `--language` sin choices (M-02), `list --voice` (B-01), `translate` choices (B-02); (5) **feature faltante** — `synthesize --play` debe implementar bucle interactivo de 4 opciones (CONTRACT.md §4) (B-03); (6) **motor roto** — traducción CT2 sin tokenizador en el dir convertido: gate y loader discrepan por construcción (C-05).

## 2. Índice reenumerado por severidad

### 2.1 Críticos — Alta / S3 (5)

| Nuevo ID | Origen | Título | Estado | Prioridad | Área |
|---|---|---|---|---|---|
| **C-01** | P1 | `cleanup` perdió borrado granular y `--all` cambió a `uninstall` | ✅ Resuelto | P0 | CLI/Gestión |
| **C-02** | P2+S3-01 | `speech synthesize/say` promete flags cross-lingual, overrides y payloads inexistentes | ✅ Resuelto | P0 | CLI/Implementación |
| **C-03** | P4+S3-03 | `daemon start/serve` sin flags `--language`/`--with-stt` perdidos en migración | ⏳ Pendiente | P0 | CLI/Daemon |
| **C-04** | S3-02 | `voice clone --daemon` promete precompute vía `/voices/precompute` inexistente | ⏳ Pendiente | P0 | CLI/Contrato |
| **C-05** | F5 | Motor de traducción CT2 roto por construcción: gate y loader discrepan | ⏳ Pendiente | P0 | Motor/Traducción |

### 2.2 Medios — Media / S2 (6)

| Nuevo ID | Origen | Título | Estado | Prioridad | Área |
|---|---|---|---|---|---|
| **M-01** | P3 | `speech dub` renombró `--source-language/--target-language` a `--from/--to` sin actualizar contrato | ✅ Resuelto | P1 | CLI/Contrato |
| **M-02** | P5 | `setup` sin `--force-update/--remove-path/--yes`, `--language` sin choices | ⏳ Pendiente | P1 | CLI/Setup |
| **M-03** | S2-01 | `USAGE` push-to-talk sin `--duration` contradice validación `InvalidInput` | ✅ Resuelto | P1 | CLI/Docs |
| **M-04** | S2-02 | `THIRD-PARTY-LICENSES.md` desactualizado `0.13.0` vs `0.18.1` | ✅ Resuelto | P1 | Legal/CI |
| **M-05** | S2-03 | `MANUAL-VALIDATION`/`GOAL` referencian `setup.exe` Inno Setup obsoleto | ✅ Resuelto | P2 | Distribución |
| **M-06** | S2-04 | `docs/BUILD.md` no refleja `log on drift` gcc en `build-windows-x64` | ✅ Resuelto | P2 | CI/Docs |

### 2.3 Bajos e informativos — Baja / S1 / S0 (9)

| Nuevo ID | Origen | Título | Estado | Prioridad | Área |
|---|---|---|---|---|---|
| **B-01** | P6.1 | `speech list --voice` rechazado (variante unitaria, sin filtro) | ⏳ Pendiente | P1 | CLI |
| **B-02** | P6.2 | `translate --from/--to` opcionales sin choices `es\|en` | ⏳ Pendiente | P2 | CLI |
| **B-03** | P6.3 | `speech synthesize --play` sin bucle interactivo | ⏳ Pendiente | P2 | CLI/Implementación |
| **B-04** | S1-01 | `README` ejemplos `curl` hardcodean `0.15.1` | ✅ Resuelto | P2 | Docs |
| **B-05** | S1-02 | `docs/DESIGN.md` árbol y `const VERSION` en `0.15.1` | ✅ Resuelto | P2 | Docs |
| **B-06** | S1-03 | `README` omite matiz `libclang-dev` condicional | ✅ Resuelto | P3 | Docs |
| **B-07** | S1-04 | `docs/CLI/README.md` referencia `CliError hereda BaseException` (Python) | ✅ Resuelto | P3 | Docs |
| **B-08** | S1-05 | `GOAL.md` cita `pytest 795/795` y pesos sin desglose `with-base` | ✅ Resuelto | P3 | Docs |
| **B-09** | S0-01 | `CLAUDE.md`/`AGENTS.md` genéricos sin `xtask release` | ✅ Resuelto | P3 | Docs |

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

- **Evidencia de auditoría HEAD (2026-09-05)**: ✅ `cargo run -- cleanup --help` muestra los 6 flags (`--voices`, `--synthetic-speech`, `--model`, `--all`, `--dry-run`, `-y/--yes`). ✅ `cargo run -- uninstall --help` confirma comando separado (`-f/--force`, `--yes`, `--json`). ✅ `cleanup` sin flags → exit 2 (test `tests/cli_golden.rs:207-215`). ✅ `tests/cli_golden.rs:259-267` (`cleanup_all_coincide_con_fixture`) verde.
- **Confianza**: Alta (inventarios completos + suite `cargo test --lib`/`cli_golden` verde + reality check binario real F5).
- **Causa**: Port fiel en rutas calientes, pérdida sistemática en periferia de gestión no ejercitada por E2E.
- **Impacto (residual)**: Ninguno — superficie restaurada; docs reconciliados en F6 (`CONTRACT.md §11`, `USAGE.md`, `CLEANUP.md`, transversales).
- **Corrección aplicada**: Restaurados `--voices/--synthetic-speech/--model/--dry-run/--yes` con semántica de arrastre §11 y desacoplado `--all` de `handle_uninstall`; docs reconciliados en `.claude/orchestration/cleanup-granular-2026-09-04/F6-drift-docs.md`.
- **Decisión requerida**: No — cerrada: `--all` = unión (no alias), conjunto 6 flags, `CONTRACT.md §11` fuente de verdad (F0).
- **Prioridad**: P0 — **cerrado**

### C-02 — `speech synthesize/say` sin cross-lingual ni payload prometido (ex-P2 + S3-01 → H-02) — ✅ **Resuelto en estrategia C (2026-09-10, `07aab20`)**

- **Categoría**: ✅ **Resuelto** — flags reimplementados/purgados; el análisis de migración queda cerrado.
- **Área/plataforma**: `docs/CLI/CONTRACT.md:166,598-604,630` vs `src/main.rs:196,205-220,222-227`
- **Síntoma**: `synthesize`/`say` solo aceptan `--text/--voice` (+ `--label/--output/--force/--play` en `synthesize` y globales `--json/--daemon/--no-daemon`). El contrato promete `--source-language` (traduce antes de sintetizar), `--compute-backend` (`auto|cpu|cuda|mps`), `--exaggeration`, `--cfg-weight`, `--temperature`.
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `cargo run -- speech synthesize --help` muestra: `--text/-t`, `--label/-l`, `--voice/-v`, `--output/-o`, `--play`, `--force/-f`, `--json`, `--daemon`, `--no-daemon`. **No** muestra `--source-language`, `--target-language`, `--compute-backend`, `--exaggeration`, `--cfg-weight`, `--temperature`.
  - ✅ `cargo run -- speech say --help` muestra: `--text/-t`, `--voice/-v`, `--json`, `--daemon`, `--no-daemon`. **No** muestra ninguna de las flags anteriores.
  - ✅ `CONTRACT.md:167` (tabla parámetros `synthesize`) y `CONTRACT.md:515-516` (payloads) **coinciden** con el código real (`src/main.rs:875-879` `{"status":"success","audio_path","voice"}` y `src/main.rs:923-927` `{"status":"reproduced","audio_path","voice"}`). **Las tablas principales están reconciliadas; el gap es el análisis de flags perdidos.**
  - 🔄 `CONTRACT.md:598-600` (§13 narrativa) describe `--source-language`/`--target-language` insertando etapa de traducción antes de la síntesis — **no existe** en `src/main.rs:205-220`.
  - 🔄 `CONTRACT.md:602-604` afirma que `speak say/speech synthesize` reemplazan `--language` por `--target-language` — **ninguno existe**.
  - 🔄 `CONTRACT.md:630` afirma que `speech dub` tiene `--source-language`/`--target-language` y `--compute-backend/-cb`, `--exaggeration`, `--cfg-weight`, `--temperature` como flags de `say` — **ninguno existe** en el binario.
- **Análisis de flags perdidos** (migración Chatterbox-V3 → Qwen3):

| Flag | Motor original | ¿Aplica a Qwen3? | Acción |
|---|---|---|---|
| `--source-language`/`--target-language` | Chatterbox (cross-lingual) | ✅ Funcionalmente válido | Reimplementar |
| `--temperature` | Chatterbox (randomness) | ✅ Qwen3 admite `-T 0.35` | Exponer |
| `--compute-backend` | Chatterbox (device) | ⚠️ Qwen3 usa `ct2` con `cpu`/`cuda` | Analizar |
| `--exaggeration` | Chatterbox (style) | ❌ Sin equivalente | Purgar |
| `--cfg-weight` | Chatterbox (CFG) | ❌ Sin equivalente | Purgar |

- **Confianza**: Alta — comparación directa código vs documento.
- **Causa**: la migración Python/Chatterbox-V3 → Rust/Qwen3-TTS (v0.10-0.12) no determinó qué flags eran motor-specific (purgar) vs funcionales transversales (reimplementar).
- **Impacto**: (1) consumers `--json` que esperan `t3_time`/`s3gen_time` no los encuentran; (2) flags §13 fallan con `unrecognized argument`; (3) cross-lingual e `temperature` no accesibles sin workaround E2E.
- **Corrección propuesta**: (1) Analizar cuadro de flags; (2) restaurar `--source-language`/`--target-language` y `--temperature` en `Synthesize`/`Say`/`Dub`; (3) purgar `--exaggeration`/`--cfg-weight`/`--compute-backend` del §13 y documentar payload real.
- **Decisión requerida**: No — cerrada en estrategia C: `--source-language`/`--target-language` + `--temperature` reimplementados; `--exaggeration`/`--cfg-weight`/`--compute-backend` purgados.
- **Evidencia de cierre (2026-09-10, árbol actual)**: ✅ `speech synthesize --help` y `speech say --help` exponen `--source-language`/`--target-language`/`--temperature` (choices `es-latam|en`) y no muestran `--compute-backend`/`--exaggeration`/`--cfg-weight`; ✅ `speech dub --help` exige `--source-language` con los mismos choices + `--temperature`; ✅ `CONTRACT.md:167-168` (tablas) y `:600-602` (§13) describen el comportamiento implementado; ✅ doradas contrato↔`--help` y flags nuevos en verde (commit `07aab20`).
- **Prioridad**: P0 — **cerrado**

### C-03 — `daemon start/serve` sin flags `--language`/`--with-stt` (ex-P4 + S3-03 → H-04) — ⏳ **Pendiente de migración — falsos positivos tras re-auditoría**

- **Categoría**: ✅ **Supervisión corregida** (2026-09-04) + ⏳ **Flags `--language`/`--with-stt` pendientes** — **falso positivo parcial tras re-auditoría** (2026-09-05)
- **Área/plataforma**: `DaemonCommands::{Start,Serve}` `src/main.rs:265-271,280-286` con `--auto-restart`/`--max-retries` (default 3) vs `docs/CLI/commands/DAEMON.md:1` `crates/avi-daemon/src/lib.rs:614` `run_supervised` `crates/avi-daemon/src/spawn.rs:21` `spawn_background` y oráculo `daemon/run.py`
- **Síntoma (histórico)**: 5 variantes unitarias sin flags; oráculo `start: --autorestart --max-retries` y `serve: --auto-restart --max-retries (0=infinito)` + `--language` + `--with-stt`.
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `cargo run -- daemon start --help` muestra: `--auto-restart`, `--max-retries <MAX_RETRIES>` [default: 3], `--json`.
  - ✅ `cargo run -- daemon serve --help` muestra: idénticos flags `--auto-restart`, `--max-retries [default: 3]`.
  - ✅ `src/main.rs:265-271` `Start { auto_restart: bool, max_retries: u32 }` y `src/main.rs:280-286` `Serve { auto_restart: bool, max_retries: u32 }`.
  - ✅ `DAEMON.md:11,15` describe correctamente estos flags.
  - ✅ `DAEMON.md:64` afirma explícitamente "7 rutas públicas (podadas `GET /voices` y `POST /voices/precompute`; **sin legado**)" — describe arquitectura actual.
  - 🔄 **Drift documental menor**: `DAEMON.md:50` referencia `crates/avi-daemon/src/lib.rs:614` para `run_daemon_server`, pero la función real está en `crates/avi-daemon/src/lib.rs:1117` (desfase de línea tras refactors). No afecta funcionalidad.
- **Confianza**: Alta
- **Causa**: Supervisión nunca portada (0.10-0.12) — corregida en `src/main.rs:262` (parser unificado `--auto-restart`/`--max-retries`), `crates/avi-daemon/src/lib.rs:1117` (`run_supervised`), `src/main.rs:1205` integración `Start/Serve`. **Pero** `--language`/`--with-stt` eran **features reales** del oráculo Python (`daemon/run.py:50-55`, `cli.py:2749-2756`), no residuos legacy: `--language` controlaba el preload de modelos por idioma (`es-latam\|en\|all`), `--with-stt` precargaba `faster-whisper-small` para transcripción en el daemon. El Rust discardeó estos flags sin portar su funcionalidad a la CLI. El daemon Rust tiene STT funcional (`native-stt`) pero carece de flags CLI para controlarlo. **Falso positivo**: el auditólogo original los descartó como "legacy"; no lo eran.
- **Impacto (residual)**: Problemas 1 y 2 (supervisión) resueltos. **Pendiente**: `--language`/`--with-stt` perdidos — `daemon serve/start` no aceptan estos flags aunque el daemon tenga STT funcional. Los consumers E2E que esperaban `daemon serve --language es-latam --with-stt` fallan con `unrecognized argument`.
- **Corrección aplicada**: Solo supervisión (F4/F6, 2026-09-04). `--language`/`--with-stt` permanecen pendientes.
- **Decisión requerida**: Sí — ¿se reimplementan `--language`/`--with-stt` (mapeando a model preload del daemon) o se purgan formalmente documentando por qué?
- **Prioridad**: P0 — **supervisión cerrada; --language/--with-stt pendientes de migración**

### C-04 — `voice clone --daemon` promete precompute vía `/voices/precompute` inexistente (ex-S3-02 → H-09) — ⏳ **Pendiente — falso positivo tras re-auditoría**

- **Categoría**: **⚠️ Falso positivo parcial — flags existen pero feature no funciona**
- **Área/plataforma**: `docs/CLI/CONTRACT.md:238`, `USAGE.md:711` vs `src/main.rs:55-64` (global args) y `src/main.rs:524` (`route_to_daemon` en `handle_voice`)
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `src/main.rs:55-64`: `daemon` y `no_daemon` son `#[arg(long, global = true)]` en el struct `Cli`, propagándose a **todos** los subcomandos incluyendo `voice clone`.
  - ✅ `cargo run -- voice clone --help` muestra `--daemon` y `--no-daemon` en las opciones.
  - ✅ `src/main.rs:524`: `handle_voice` para `Clone` SÍ llama `route_to_daemon`:
    ```rust
    if route_to_daemon(daemon_mode, &client).await {
        return clone_via_daemon(...);
    }
    ```
  - ✅ `src/main.rs:81-89`: `Cli::daemon_mode()` computa `DaemonMode` a partir de `self.daemon`/`self.no_daemon`.
  - ✅ `USAGE.md:709` documenta correctamente `--daemon`/`--no-daemon` para `voice clone`.
  - ℹ️ `src/main.rs:158` (referencia del hallazgo original) señala `Doctor` (última variante de `VoiceCommands`), **no** el struct `Clone`. Las líneas de referencia del hallazgo son ligeramente imprecisas. El struct `Clone` (`src/main.rs:165-170`) no tiene campos `daemon`/`no_daemon` propios porque heredan del `Cli` global — esto es por diseño de clap (`global = true`), no un bug.
  - ⚠️ **❌ False positive — feature precompute inexistente**: el oráculo Python (`7542962:src/ai_voice_interconnector/cli.py:924`) implementaba `voice clone --daemon` llamando a `DaemonIPCClient().precompute_voice(args.name)` que hittea `POST /voices/precompute`. En Rust, `DAEMON.md:64` afirma: *"7 rutas públicas (podadas `GET /voices` y `POST /voices/precompute`)"* — el endpoint fue **purgado** deliberadamente del daemon. El handler `voices_clone_handler` (`crates/avi-daemon/src/lib.rs:738`) devuelve siempre `"precomputed": false`. `clone_via_daemon` (`src/main.rs:525-535`) existe y llama a `POST /voices/clone` (que existe), pero **no existe endpoint para precompute**. El flag `--daemon` nombra la feature (clone asistido + precompute de conditionals) pero solo la primera mitad está implementada.
  - ⚠️ `CONTRACT.md:241` documenta: *"voice clone --daemon precarga los embeddings condicionales del daemon antes de clonar"* — esta parte del contrato no funciona en HEAD.
- **Confianza**: Alta — flags verificados con `--help`; endpoint `/voices/precompute` verificado inexistente en `DAEMON.md:64` y `crates/avi-daemon/src/lib.rs:738`.
- **Causa**: El auditólogo original se centró en la **existencia de flags globales** y el routing `route_to_daemon`, pero no verificó que el **endpoint precompute** existiera en el daemon. El flag `--daemon` existe pero la feature principal que promete (precompute de conditionals) fue purgada del daemon sin actualizar el contrato ni el handler.
- **Impacto**: `voice clone --daemon` delega a `/voices/clone` (funciona parcialmente) pero **no precomputa** — el comportamiento contratado en `CONTRACT.md:238,241` (precompute antes de clonar) no se cumple. Consumers que esperan voice clone acelerado vía daemon no obtienen la aceleración.
- **Corrección aplicada**: Ninguna — el endpoint `/voices/precompute` no existe (purgado).
- **Corrección propuesta**: Implementar `POST /voices/precompute` en `crates/avi-daemon/src/lib.rs` (o actualizar `CONTRACT.md:238-241` y `USAGE.md:711` para reflejar que `--daemon` solo usa `/voices/clone`, sin precompute).
- **Decisión requerida**: Sí — ¿se implementa el endpoint precompute o se documenta su ausencia?
- **Prioridad**: P0 — **pendiente — falso positivo: flag existe pero feature no

### C-05 — Motor de traducción CT2 roto por construcción: gate y loader discrepan (E1 de F5-harness-estructural, 2026-09-10) — ✅ **Resuelto (2026-09-10, C-05-motor-CT2)**

- **Categoría**: Rotura funcional total — feature contratada (`translate`, `dub` con traducción) que falla en el 100% de los casos, en cualquier máquina.
- **Área/plataforma**: `crates/avi-translation/src/lib.rs:26-38` (`Ct2TranslationEngine::new` → `Translator::new`) vs `crates/avi-store/src/lib.rs:543-545` (`is_ct2_provisioned`) vs `src/main.rs:1684-1707` (conversor Marian→CT2).
- **Síntoma** (histórico, antes de la corrección): `translate` directo salía 9 (`translation_failed`), vía daemon salía 1; `dub` con traducción salía 1. El loader fallaba con `failed to create a tokenizer`: el dir convertido `hf_cache/ct2/opus-mt-<par>/` traía `model.bin` + `shared_vocabulary.json` + `config.json` pero ningún activo de tokenizador (`source.spm`/`tokenizer.json`), y nada los depositaba ahí (el snapshot HF sí trae `source.spm`/`target.spm`/`vocab.json`).
- **Evidencia (F5 2026-09-10)**:
  - ✅ `cargo test -p avi-translation --features native-translation` 6/6 en rojo con el mismo error — independiente de CLI, harness y estrategia C.
  - ✅ Doradas `translate_es_a_en_produce_traduccion` (exit 9) y `tts::dub_daemon_con_traduccion` (exit 1) bloqueadas por causa motor, no harness.
  - ✅ El gate `is_ct2_provisioned` (entonces solo exigía `model.bin`) no skipeaba: gate y loader exigían cosas distintas por construcción.
- **Confianza**: Alta — reproducido a nivel crate, CLI directo, daemon y tests dorados.
- **Causa** (histórica): el pipeline de provisión nunca depositaba el tokenizador en el dir convertido; ningún paso de `setup` lo copiaba (verificado por búsqueda exhaustiva). Re-ejecutar `setup` reproducía el mismo dir roto: no era entorno, era defecto de producto.
- **Impacto** (histórico): `translate`, `dub` con traducción y las rutas con idiomas distintos de estrategia C (etapa `traducir_si_difiere` en `say`/`synthesize`/`dub`) inejecutables en la práctica; 2 tests dorados en rojo por causa motor.
- **Corrección propuesta**: (a) el conversor copia `source.spm` al dir CT2 + regresión con los unit tests existentes; (b) alternativa: fallback al `.spm` del snapshot HF; (c) mientras tanto: el gate exige presencia de tokenizador para skipear con honestidad en vez de fallar.
- **Decisión requerida**: No — cerrada: gate `is_ct2_provisioned` exige `model.bin` más tokenizador (`tokenizer.json` o `source.spm`+`target.spm`), `setup` repara el dir roto por reconversión atómica, exits 4/9 intactos.
- **Prioridad**: P0 — **cerrado**
- **Evidencia de cierre (F5 2026-09-10, `.claude/orchestration/c05-motor-ct2/F5-ground-truth.md`)**: ✅ `setup` repara ambos dirs (`opus-mt-es-en` y `opus-mt-en-es` con `model.bin` + `source.spm` + `target.spm`); ✅ CLI es→en (`Hola, ¿cómo estás?` → `Hey, how are you?`, exit 0) y en→es (exit 0); ✅ `cargo test -p avi-translation --features native-translation` 17/17; ✅ dorada `translate_es_a_en_produce_traduccion` en verde y `tts::dub_daemon_con_traduccion` verificada por el usuario (27.36s, exit 0).

## 4. Hallazgos medios — Media / S2

### M-01 — `speech dub` renombró flags de idioma sin reflejarlo en el contrato (ex-P3 → H-03) — ✅ **Resuelto en estrategia C (2026-09-10, `07aab20`)**

- **Categoría**: ✅ **Resuelto** — nombres largos restaurados con contrato sincronizado.
- **Área/plataforma**: `docs/CLI/CONTRACT.md:630` §13 vs `src/main.rs:235-238`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ⏳ `cargo run -- speech dub --help` muestra: `--from <FROM> [default: es]`, `--to <TO> [default: en]`. **No** muestra `--source-language`/`--target-language`.
  - ⏳ `src/main.rs:235-238` declara `from: String` (default "es") y `to: String` (default "en") **sin** `value_parser`.
  - ⏳ `CONTRACT.md:630` afirma que `speech dub` usa `--source-language` **requerido** (`es-latam|en`) y `--target-language` (default `es-latam`) — **no existe** en el binario.
  - ℹ️ El `value_parser` con choices existe **solo** en `speech transcribe`'s `source_language` (`src/main.rs`), no en `dub`.
- **Confianza**: Alta
- **Causa**: Renombrado razonable para paridad con `translate`, pero sin sincronizar contrato y sin `value_parser`.
- **Impacto**: Flags documentados en §13 inexistentes; validación `es-latam/en` → texto libre. Afecta también C-02.
- **Corrección propuesta**: Actualizar `CONTRACT.md` §13 (línea 630) al código real (`--from`/`--to`) o restaurar `value_parser` con valores válidos.
- **Decisión requerida**: No — cerrada: nombres largos restaurados con `value_parser = ["es-latam", "en"]`.
- **Evidencia de cierre (2026-09-10, árbol actual)**: ✅ `speech dub --help` muestra `--source-language` (requerido), `--target-language` y `--temperature`, sin `--from/--to`; ✅ `CONTRACT.md` §13 y `SPEECH.md` reconciliados.
- **Prioridad**: P1 — **cerrado**

### M-02 — `setup` sin `--force-update/--remove-path/--yes`, `--language` sin choices (ex-P5 → H-05)

- **Categoría**: Rotura de paridad (parcial: `--uninstall` rediseñado a comando propio)
- **Área/plataforma**: `src/main.rs:122-131` vs oráculo `7542962` y `docs/CLI/CONTRACT.md:559-563,593`
- **Evidencia de auditoría HEAD (2026-09-05)**:

| Superficie | Oráculo | Rust (HEAD) | Naturaleza |
|---|---|---|---|
| `--force-update` | re-descarga ambos modelos | ❌ | **Pérdida funcional**: E2E tuvo que purgar manualmente ~14 GB |
| `--remove-path` | quita symlink PATH y termina | ❌ (subsumido por `uninstall`) | Rediseño aceptable |
| `--uninstall` | desinstala en un paso | ✅ (`cargo run -- uninstall --help` confirma `-f/--force`, `--yes`, `--json`) | Rediseño deliberado a comando separado |
| `--yes` | omite confirmación | ❌ (en `setup`; existe en `uninstall` y `cleanup`) | Perdido en `setup` |
| `--language` | choices `es-latam|en|all` default `all` | texto libre `String` default `"es"` `src/main.rs:124` | Divergente: contrato 593 conserva `--language` como texto libre |

- **Confianza**: Alta
- **Causa**: Port fiel en modelos, pérdida en modos de gestión no ejercitados por E2E.
- **Impacto**: Sin forma de re-descargar sin purga manual (~14 GB); `setup --language all` del contrato no existe como choices.
- **Corrección propuesta**: Restaurar `--force-update` (o equivalente) y `value_parser` de `--language`; decidir política de confirmación.
- **Decisión requerida**: Sí
- **Prioridad**: P1 — **pendiente**

### M-03 — `USAGE` push-to-talk sin `--duration` contradice validación `InvalidInput` (ex-S2-01 → H-10)

- **Categoría**: **❌ Contradicción errónea — resuelto en HEAD**
- **Área/plataforma**: `USAGE.md:569,588-590,615-616` vs `src/main.rs:694-701,244`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `USAGE.md:569`: describe `speech transcribe --mic` con push-to-talk en TTY (presiona Enter para detener).
  - ✅ `USAGE.md:588-589`: "Con `--mic` y sin `--duration`, el comando espera en silencio a que el usuario presione Enter antes de transcribir."
  - ✅ `USAGE.md:615-616`: "`--duration` sin `--mic` sale con exit **2**. Sin terminal interactiva (no TTY) y sin `--duration`, `--mic` también sale con exit **2**."
  - ✅ `src/main.rs:694-701`: la validación implementa exactamente esto:
    ```rust
    // T4: push-to-talk sin --duration permitido en TTY (S2-01); sin TTY se exige --duration
    if mic && duration.is_none() && !std::io::stdin().is_terminal() {
        return Err(CliError::new(ExitCode::InvalidInput, "usage_error", "--mic requiere --duration en este host."));
    }
    ```
  - ✅ `src/main.rs:244`: `duration: Option<u64>` — el campo es **opcional**, no requerido.
  - ✅ `cargo run -- speech transcribe --help` muestra `--duration <DURATION>` como opcional (sin `required`).
- **Confianza**: Alta
- **Causa**: Las referencias de línea del hallazgo (`USAGE.md:381`, `src/main.rs:652`) eran imprecisas. La línea 381 de USAGE.md está en la sección de despacho daemon, no sobre `--duration`. El código relevante está en `src/main.rs:694-701` y la doc relevante en `USAGE.md:569-590`.
- **Impacto**: Ninguno — USAGE.md y el código son consistentes. Push-to-talk funciona en TTY; `--duration` es obligatorio solo en no-TTY (CI/containers).
- **Corrección aplicada**: Ninguna necesaria — la validación y la documentación coinciden.
- **Decisión requerida**: No — cerrada: el comportamiento está documentado y validado correctamente.
- **Prioridad**: P1 — **cerrado**

### M-04 — `THIRD-PARTY-LICENSES.md` desactualizado (ex-S2-02 → H-11)

- **Categoría**: Documentación/Legal
- **Área/plataforma**: `THIRD-PARTY-LICENSES.md:80,88` vs `Cargo.toml:3` `Cargo.lock` `SOURCE-OFFER.md:3` `crates/xtask/src/main.rs:442`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `THIRD-PARTY-LICENSES.md:88`: `\| ai-voice-interconnector \| 0.18.26 \| GPL-3.0-or-later` — coincide con `Cargo.toml:3` (`version = "0.18.26"`).
  - ✅ `SOURCE-OFFER.md:3`: `**AI Voice InterConnector 0.18.26**` — coincide.
  - ✅ `CHANGELOG.md:385`: registra que en v0.18.1 se corrigió `THIRD-PARTY-LICENSES.md:88` de `0.13.0` → `0.18.1` (drift original yace resuelto).
  - 🔄 **Drift menor (conteo)**: `THIRD-PARTY-LICENSES.md:80` dice "455 crates únicos"; `Cargo.lock` real tiene 503 entradas totales, **451 únicas** (diff = 4). El comentario en `crates/xtask/src/main.rs:442` (`const VERSION: &str = "0.13.0"`) es solo un ejemplo ilustrativo dentro de un regex, no afecta funcionalidad.
- **Confianza**: Alta (versiones) / Media (conteo)
- **Causa**: `bump_version` (`crates/xtask/src/main.rs`) actualiza `VERSION` y `SOURCE-OFFER.md` pero no regenera `THIRD-PARTY-LICENSES.md` (conteo de crates).
- **Impacto**: Riesgo legal mitigado (versiones alineadas). El drift de conteo (455 vs 451) es cosmético.
- **Corrección propuesta**: `cargo run -p xtask -- licenses` (o `cargo metadata` + render) y commit; corregir conteo 455→451.
- **Decisión requerida**: No
- **Prioridad**: P1 — **resuelto (versiones)** con drift menor en conteo

### M-05 — Artefacto `setup.exe` obsoleto en `MANUAL-VALIDATION`/`GOAL` (ex-S2-03 → H-12)

- **Categoría**: Documentación/Distribución
- **Área/plataforma**: `docs/MANUAL-VALIDATION.md:13` `docs/GOAL.md:152` vs `.circleci/config.yml:925` `docs/BUILD.md:45`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `docs/MANUAL-VALIDATION.md:14`: `Windows ai-voice-interconnector-X.Y.Z-x86_64-windows.zip` — ya no menciona `setup.exe`.
  - ✅ `docs/GOAL.md:162`: `ai-voice-interconnector-X.Y.Z-x86_64-windows.zip` — ya no menciona `setup.exe`.
  - ✅ `docs/BUILD.md:41`: `Windows x64 | .zip | Compress-Archive (PowerShell)` — formato zip.
  - ✅ `CHANGELOG.md:391`: registra que en v0.18.1 se corrigió `MANUAL-VALIDATION.md:15` y `GOAL.md:162` de `setup.exe` → `zip/tar.gz`.
  - ✅ `grep "setup.exe\|Inno Setup"` en `docs/` y `.circleci/` → **sin coincidencias** en HEAD.
  - ℹ️ `.circleci/config.yml:925` menciona `vendor\qwen3-tts\qwen_tts.exe` — es el binario del **motor TTS compilado**, no el instalador. El artefacto de release real es `.zip` (Windows) / `.tar.gz` (Linux/macOS).
- **Confianza**: Alta
- **Causa**: Residuo Inno Setup (eliminado en 0.16.0) purgado en v0.18.1.
- **Impacto**: Ninguno — validación manual y distribución usan zip/tar.gz.
- **Corrección aplicada**: Docs reconciliados en v0.18.1; layout plano actual.
- **Decisión requerida**: No
- **Prioridad**: P2 — **cerrado**

### M-06 — `docs/BUILD.md` no refleja `log on drift` gcc (ex-S2-04 → H-13)

- **Categoría**: Documentación/CI
- **Área/plataforma**: `docs/BUILD.md:418,408-419` vs `.circleci/config.yml:772,776-777` vs `CHANGELOG.md:424-430`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ✅ `docs/BUILD.md:418`: "`log on drift` desde `v0.17.1`, no `fail-fast`" — **documenta** el comportamiento.
  - ✅ `docs/BUILD.md:408-419`: describe toolchain vigente + mecanismo de drift con `[WARN]` y continuación.
  - ✅ `.circleci/config.yml:772`: "El pin de choco + la caché fijan el resultado; si upstream driftó gcc, se loguea (no aborta)".
  - ✅ `.circleci/config.yml:776-777`: "si upstream driftó gcc, se loguea (no aborta) y la versión instalada manda como evidencia RLE".
  - ✅ `CHANGELOG.md:424-430`: `## [0.17.1]` describe el cambio a "log on drift".
  - ℹ️ `CHANGELOG.md:93` (referencia del hallazgo original) es del índice/TOC, no contiene contenido sobre 0.17.1. El contenido real está en línea 424.
- **Confianza**: Alta
- **Causa**: Doc no actualizado tras relajación del guard `build-windows-x64` en 0.17.1 — **corregido**.
- **Impacto**: Ninguno — expectativa `warn-continue` vs real `warn-continue`.
- **Corrección aplicada**: `docs/BUILD.md:418` documenta `log on drift`.
- **Decisión requerida**: No
- **Prioridad**: P2 — **cerrado**

## 5. Hallazgos bajos e informativos — Baja / S1 / S0

### B-01 — `speech list --voice` rechazado (ex-P6.1 → H-06)

- **Categoría**: Rotura de paridad menor
- **Área/plataforma**: `SpeechCommands::List` unitaria `src/main.rs:188` vs `docs/CLI/CONTRACT.md:170,278,321`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ⏳ `cargo run -- speech list --help` muestra: `--json`, `--daemon`, `--no-daemon`, `-h/--help`. **No** muestra `--voice/-v`.
  - ⏳ `src/main.rs:188`: el variant `List` del enum `SpeechCommands` tiene **cero campos** — `List { /* sin campos */ }`.
  - ⏳ `CONTRACT.md:170` documenta `speech list | --voice/-v (filtro) · --json` — el flag está prometido pero no implementado.
- **Confianza**: Alta — hallazgo que originó la auditoría de paridad (H5 E2E).
- **Causa**: Port omitió flag no ejercitado por tests dorados.
- **Impacto**: Guion E2E falla paso 6; UX de distinguir "voz mal escrita" de "sin resultados" (contrato 278) perdida.
- **Corrección propuesta**: Restaurar `--voice/-v` con validación exit 3 y filtrado en `SpeechCommands::List` (`src/main.rs:188`), o corregir `CONTRACT.md:170` eliminando `--voice/-v`.
- **Decisión requerida**: Sí
- **Prioridad**: P1 — **pendiente**

### B-02 — `translate --from/--to` sin choices (ex-P6.2 → H-07)

- **Categoría**: Drift menor
- **Área/plataforma**: `src/main.rs:102-105` vs oráculo `required choices es|en`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ⏳ `src/main.rs:102-105`: `from: String` y `to: String` con `default_value` pero **sin** `value_parser`. Cualquier string es aceptado.
  - ⏳ `cargo run -- translate --help` muestra `--from <FROM> [default: es]` y `--to <TO> [default: en]` — sin restricción de valores.
  - ⏳ `CONTRACT.md:596` describe `--from {es, en}` y `--to {es, en}` como si tuvieran restricted choices.
- **Confianza**: Alta
- **Causa**: Default razonable añadido sin documentar choices ni enforzar `value_parser`.
- **Impacto**: Menor; texto libre permite valores inválidos sin error temprano.
- **Corrección propuesta**: Documentar choices en `CONTRACT.md:596` o restaurar `value_parser = ["es", "en"]` en `src/main.rs:102-105`.
- **Decisión requerida**: No
- **Prioridad**: P2 — **pendiente**

### B-03 — `speech synthesize --play` sin bucle interactivo (ex-P6.3 → H-08)

- **Categoría**: **Pendiente de implementación** — feature prometida en contrato no implementada
- **Área/plataforma**: `docs/CLI/CONTRACT.md:185-203` §4 vs `src/main.rs:857-865`
- **Evidencia de auditoría HEAD (2026-09-05)**:
  - ⏳ `CONTRACT.md:185-203` (§4) documenta 4 opciones interactivas: "Reproducir otra vez", "Aceptar y guardar", "Rechazar y regenerar", "Rechazar y descartar".
  - ⏳ `src/main.rs:857-865`: el comportamiento real reproduce y guarda inmediatamente:
    ```rust
    if play {
        audio::AudioService::new().play_wav(&tmp_wav)?;
    }
    let saved = speech_store.save(&voice, &label, &text, &tmp_wav)?;
    ```
    No hay prompt, no loop, no ramificación interactiva.
  - ⏳ `cargo run -- speech synthesize --help` tampoco indica que `--play` active un modo interactivo.
- **Confianza**: Alta — el código fuente muestra claramente ausencia de loop; el CONTRACT.md describe un flujo interactivo que no existe.
- **Causa**: El bucle interactivo de 4 opciones es el diseño original (oráculo Python) y el comportamiento real (play→save→done) lo reemplazó sin decisión documentada. El contrato §4 sigue especificando el loop como comportamiento esperado.
- **Impacto**: La funcionalidad interactiva prometida no existe — `synthesize --play` reproduce y guarda incondicionalmente, sin opciones de regenerar, rechazar o descartar. Rompe la UX documentada en §4.
- **Corrección propuesta**: **Implementar** el bucle interactivo de 4 opciones descrito en `CONTRACT.md:185-203` en el handler de `SpeechCommands::Synthesize` (`src/main.rs:853`), usando `inquire`/similar para el prompt post-reproducción. Alternativa: si el loop fue un diseño descartado, actualizar §4 a "play→save→done" — pero esto requiere decisión explícita.
- **Decisión requerida**: Sí — ¿se implementa el loop interactivo o se desdocumenta?
- **Prioridad**: P2 — **pendiente de implementación**

### B-04 — `README` ejemplos `curl` hardcodean `0.15.1` (ex-S1-01 → H-14)

- **Categoría**: ✅ **Resuelto**
- **Área**: `README.md:103,107,111` vs `src/main.rs:27` `Cargo.toml:3`
- **Evidencia de auditoría HEAD (2026-09-05)**: `README.md:103,107,111` usan placeholder `X.Y.Z` (ej. `ai-voice-interconnector-X.Y.Z-x86_64-linux.tar.gz`). `grep "0.15.1"` en README → 0 coincidencias. `src/main.rs:27` = `const VERSION: &str = "0.18.26"`, `Cargo.toml:3` = `version = "0.18.26"`.
- **Confianza**: Alta — **pendiente de ninguna acción**. El hardcodeo a `0.15.1` no existe en HEAD.

### B-05 — `docs/DESIGN.md` árbol y `const VERSION` en `0.15.1` (ex-S1-02 → H-15)

- **Categoría**: ✅ **Resuelto**
- **Área**: `docs/DESIGN.md:99` vs `src/main.rs:27`
- **Evidencia de auditoría HEAD (2026-09-05)**: `grep "0.15.1"` en DESIGN.md → **0 coincidencias**. `DESIGN.md:99` usa `const VERSION = "X.Y.Z"` (placeholder). `src/main.rs:27` = `0.18.26`.
- **Confianza**: Alta — **pendiente de ninguna acción**.

### B-06 — `README` omite matiz `libclang-dev` condicional (ex-S1-03 → H-16)

- **Categoría**: ✅ **Resuelto**
- **Área**: `README.md:144` vs `docs/BUILD.md:26`
- **Evidencia de auditoría HEAD (2026-09-05)**: `README.md:144`: "`libclang-dev` solo con `--features native-translation/full` (traducción). Ver `docs/BUILD.md`." — README sí califica como condicional. `BUILD.md:26`: "`libclang-dev` (solo con `--features native-translation`/`full`, para `bindgen` de `ct2rs`; no requerido para `featureless` ni `native-stt`)" — detallado.
- **Confianza**: Alta — **pendiente de ninguna acción**. Ambos documentos son consistentes y precisos.

### B-07 — `docs/CLI/README.md` referencia Python muerta (ex-S1-04 → H-17)

- **Categoría**: ✅ **Resuelto**
- **Área**: `docs/CLI/README.md:79`
- **Evidencia de auditoría HEAD (2026-09-05)**: `CLI/README.md:79`: "`CliError` vive en `crates/avi-core/src/exit_codes.rs` y se traduce en `src/main.rs` (`ExitCode` + `reason`), **sin herencia Python**." — niega explícitamente la herencia Python. `grep "Base Exception\|hereda de Python\|Python.*Base"` en CLI/README.md → **0 coincidencias**. `crates/avi-core/src/exit_codes.rs` existe (confirmado via glob).
- **Confianza**: Alta — **pendiente de ninguna acción**. La referencia a Python muerta no existe.

### B-08 — `GOAL.md` cita `pytest 795/795` y pesos sin desglose (ex-S1-05 → H-18)

- **Categoría**: ✅ **Resuelto**
- **Área**: `docs/GOAL.md:175,202`
- **Evidencia de auditoría HEAD (2026-09-05)**: `grep "pytest"` en GOAL.md → **0 coincidencias**. `GOAL.md:175`: "descarga de **~9 GB base** (**~11,5 GB con `--with-base`**)" — desglose explícito presente.
- **Confianza**: Alta — **pendiente de ninguna acción**. Ni `pytest` ni el falta de desglose de pesos persisten.

### B-09 — `CLAUDE.md`/`AGENTS.md` genéricos sin `xtask release` (ex-S0-01 → H-19)

- **Categoría**: ✅ **Resuelto**
- **Área**: `CLAUDE.md:1` vs `.claude/skills/release/SKILL.md` `docs/RELEASING.md`
- **Evidencia de auditoría HEAD (2026-09-05)**: `CLAUDE.md` contiene instrucciones específicas de proyecto (7 secciones con `xtask release`, `cargo run -p xtask -- release X.Y.Z`, timeout policies). Sección 5: "`release` skill (`.claude/skills/release/SKILL.md`, `cargo run -p xtask -- release X.Y.Z`) is the source of truth; see `docs/RELEASING.md`" — referencia explícita. Sección 6: referencia a `codebase-memory-mcp` MCP server. `docs/RELEASING.md` y `.claude/skills/release/SKILL.md` existen (confirmado via glob). CI mencionado a través de `BUILD.md` y `GOAL.md`.
- **Confianza**: Media — **pendiente de ninguna acción**. CLAUDE.md es específico, no genérico; todos los archivos referenciados existen.

## 6. Trazabilidad origen → nuevo ID

| Nuevo ID | Origen | ID origen | ID previo H- | Estado | Nota |
|---|---|---|---|---|---|
| **C-01** | Paridad | P1 | H-01 | ✅ Resuelto | Cerrado 2026-09-04 |
| **C-02** | Paridad+Drift | P2 + S3-01 | H-02 | ✅ Resuelto | Cerrado 2026-09-10 (estrategia C `07aab20`): flags reimplementados + purga + contrato reconciliado |
| **C-03** | Paridad+Drift | P4 + S3-03 | H-04 | ⏳ Pendiente | Supervisión corregida (2026-09-04); **--language/--with-stt falsos positivos**: eran features reales del Python oráculo, no legacy |
| **C-04** | Drift | S3-02 | H-09 | ⏳ Pendiente | Falso positivo: flags `--daemon/--no-daemon` existen y routing funciona, pero `POST /voices/precompute` purgado del daemon — precompute de conditionals no funciona |
| **C-05** | Motor (F5) | E1 | — | ✅ Resuelto | Cerrado 2026-09-10 (gate == loader, `setup` repara, CLI es↔en exit 0, 17/17, dorada translate) |
| **M-01** | Paridad | P3 | H-03 | ✅ Resuelto | Cerrado 2026-09-10: nombres largos restaurados, §13 reconciliado |
| **M-02** | Paridad | P5 | H-05 | ⏳ Pendiente | `setup` sin `--force-update/--yes`; `--language` texto libre |
| **M-03** | Drift | S2-01 | H-10 | ✅ Resuelto | USAGE y código coinciden (TTY permite push-to-talk) |
| **M-04** | Drift | S2-02 | H-11 | ✅ Resuelto | Versiones alineadas; drift menor 455→451 crates |
| **M-05** | Drift | S2-03 | H-12 | ✅ Resuelto | Inno Setup purgado v0.18.1 |
| **M-06** | Drift | S2-04 | H-13 | ✅ Resuelto | BUILD.md:418 documenta log on drift |
| **B-01** | Paridad | P6.1 | H-06 | ⏳ Pendiente | `--voice` no implementado en `SpeechCommands::List` |
| **B-02** | Paridad | P6.2 | H-07 | ⏳ Pendiente | `translate --from/--to` sin `value_parser` |
| **B-03** | Paridad | P6.3 | H-08 | ⏳ Pendiente | §4 promete loop interactivo 4-opciones no implementado en `src/main.rs:857-865` |
| **B-04** | Drift | S1-01 | H-14 | ✅ Resuelto | README usa `X.Y.Z` placeholder |
| **B-05** | Drift | S1-02 | H-15 | ✅ Resuelto | DESIGN.md usa `X.Y.Z`; sin `0.15.1` |
| **B-06** | Drift | S1-03 | H-16 | ✅ Resuelto | README califica `libclang-dev` condicional |
| **B-07** | Drift | S1-04 | H-17 | ✅ Resuelto | CLI/README.md niega herencia Python; refiere exit_codes.rs |
| **B-08** | Drift | S1-05 | H-18 | ✅ Resuelto | GOAL.md sin `pytest`; pesos desglosados |
| **B-09** | Drift | S0-01 | H-19 | ✅ Resuelto | CLAUDE.md específico con `xtask release`, CI, MCP |

Excluidos por resueltos: P7 (`55fde2e`), P8 (`b78c3aa`+`4fbe77e`), H1-H4 E2E (`1bb7fe1` `ea6472c` `d12050f`).

## 7. Orden de corrección recomendado

**Fase 1 — P0 bloqueante de contrato (sin esto toda integración `--json` parte de spec falsa):**
- **C-02** — ✅ cerrado 2026-09-10 (estrategia C, `07aab20`): tabla de flags en §3 aplicada — reimplementados `--source-language`/`--target-language`/`--temperature` en `Synthesize`/`Say`/`Dub`, purgados `--exaggeration`/`--cfg-weight`/`--compute-backend` del §13; payload real documentado.
- **C-03** — la supervisión `--auto-restart`/`--max-retries` está corregida; **pendiente** reimplementar `--language`/`--with-stt` en `DaemonCommands::{Start,Serve}` (`src/main.rs:279-286`). Ver §3. **Falso positivo**: estos flags eran features reales del oráculo Python (`daemon/run.py:50-55`), no residuos legacy.
- **C-04** — **falso positivo**: el flag `--daemon` existe y el routing funciona, pero el endpoint `POST /voices/precompute` fue purgado del daemon (`crates/avi-daemon/src/lib.rs`), rompiendo `CONTRACT.md:238,241`. Implementar el endpoint o actualizar el contrato para reflejar su ausencia.
- **C-05** — ✅ cerrado 2026-09-10: gate `is_ct2_provisioned` == loader (derivado con tokenizador), `setup` repara por reconversión atómica (ver §3).

**Fase 2 — P1 habilita paridad funcional y `value_parser`:**
- **M-01** — ✅ cerrado 2026-09-10: nombres largos restaurados con `value_parser`, §13 reconciliado.
- **M-02** — restaurar `--force-update` y `value_parser` de `--language` en `setup`; decidir política de confirmación.
- **B-01** — restaurar `--voice/-v` con validación exit 3 y filtrado en `SpeechCommands::List` (`src/main.rs:188`).
- **B-02** — agregar `value_parser = ["es", "en"]` a `translate --from/--to` (`src/main.rs:102-105`).
- **B-03** — implementar el bucle interactivo de 4 opciones en `speech synthesize --play` (`src/main.rs:857-865`) según `CONTRACT.md` §4 (líneas 185-203): "Reproducir otra vez", "Aceptar y guardar", "Rechazar y regenerar", "Rechazar y descartar".

**Fase 3 — P2 distribución y coherencia de docs (residual):**
- **M-04** (resuelto) — drift menor solo en conteo de crates: `THIRD-PARTY-LICENSES.md:80` dice 455 vs 451 real en `Cargo.lock`. Regenerar con `cargo run -p xtask -- licenses`.
- Drift menor en `DAEMON.md:50` (línea de referencia `614`→`1117`) — corregir en próxima revisión docs.

Todo cambio de contrato debe acompañarse de un **drift-detector**: test que afirme `CONTRACT.md` contra `--help` del binario y gates E2E que cubran provisión (lección P8: un diff de flags no ve comportamiento).

### Apéndice A — Re-auditoría de falsos positivos (2026-09-05)

Tras la corrección de B-03 y C-02 (features perdidas en migración confundidas con doc drift), se re-auditaron los 13 hallazgos marcados como **"resueltos"** para identificar falsos positivos — casos donde la doc describe correctamente una feature del Python oráculo, pero el código Rust la perdió.

| Hallazgo | Falso positivo? | Evidencia | Acción |
|---|---|---|---|
| C-01 | No | `cleanup` 6 flags + tests `cli_golden.rs` verdes | Ninguna |
| **C-03** | **Sí (parcial)** | `--language`/`--with-stt` existían en Python (`daemon/run.py:50-55`); Rust los discardeó como "legacy" | ⏳ Pendiente |
| **C-04** | **Sí (parcial)** | Flag `--daemon` existe, pero endpoint `POST /voices/precompute` purgado (`DAEMON.md:64`); `voices_clone_handler` siempre devuelve `"precomputed": false` | ⏳ Pendiente |
| M-03 | No | `capture_16k_mono_pcm` (`crates/avi-audio/src/lib.rs:179`) captura del mic; hay un bug de panic pero no es false positive | ⏳ (bug separado) |
| M-04 | No | Versión alineada 0.18.26; conteo 455→451 cosmético | Ninguna |
| M-05 | No | Inno Setup purgado v0.18.1; docs usan zip/tar.gz | Ninguna |
| M-06 | No | `BUILD.md:418` documenta `log on drift` | Ninguna |
| B-04..B-09 | No | Todos cosméticos (versiones, docstrings, refs condicionales) | Ninguna |

**Lección aprendida**: la mera existencia de un flag (`--daemon` como global arg) **no implica** que la feature que promete esté implementada. La auditoría debe verificar endpoints HTTP internos y handlers de daemon, no solo la superficie de parseo CLI. La regla "flags perdidos en migración = pendiente (no drift)" se aplica retroactivamente: C-03 y C-04 pasaron de ✅ Resuelto a ⏳ Pendiente.

### Apéndice B — Segunda auditoría bidireccional (2026-09-05, ronda 3)

Se re-verificaron todos los 19 hallazgos en **ambas direcciones**: resueltos → falsos positivos, y pendientes → falsos negativos.

#### Resultado: **0 falsos positivos, 0 falsos negativos adicionales**

| ID | Estado | Verificado | Detalle |
|---|---|---|---|
| C-01 | ✅ Resuelto | ✅ Genuine | `cleanup --help` + `cli_golden.rs:207-267` verdes |
| C-02 | ⏳ Pendiente | ✅ Genuine | Flags cross-lingual/temperature NO existen en `--help`; existen en Python `cli.py` |
| C-03 | ⏳ Pendiente | ✅ Genuine | `--auto-restart`/`--max-retries` funcionan; `--language`/`--with-stt` ausentes (era features del Python) |
| C-04 | ⏳ Pendiente | ✅ Genuine | Flag `--daemon` existe, pero `build_router_with_state()` (`lib.rs:1049-1062`) no registra `/voices/precompute` |
| M-01 | ⏳ Pendiente | ✅ Genuine | `dub --from/--to` texto libre vs §13 |
| M-02 | ⏳ Pendiente | ✅ Genuine + nota | Falta `--force-update`/`--yes`/`value_parser`; `--with-stt` SÍ existe en `setup --help` (no era parte del gap) |
| M-03 | ✅ Resuelto | ✅ Genuine | `src/main.rs:694-701` + `USAGE.md:569-590` consistentes |
| M-04 | ✅ Resuelto | ✅ Genuine | Versiones alineadas 0.18.26 |
| M-05 | ✅ Resuelto | ✅ Genuine | Inno Setup purgado v0.18.1 |
| M-06 | ✅ Resuelto | ✅ Genuine | `BUILD.md:418` documenta log on drift |
| B-01 | ⏳ Pendiente | ✅ Genuine | `List` tiene cero campos, sin `--voice/-v` |
| B-02 | ⏳ Pendiente | ✅ Genuine | `translate --from/--to` sin `value_parser` |
| B-03 | ⏳ Pendiente | ✅ Genuine | `synthesize --play` reproduce+guarda sin loop interactivo |
| B-04..B-09 | ✅ Resuelto | ✅ Genuine | Todos cosméticos |

**Nota 2026-09-10**: este cuadro registra la ronda del 2026-09-05 y queda intacto como acta; C-02 y M-01 se cerraron después por estrategia C (`07aab20`, ver §3) y C-05 es nuevo de F5.

**Precisiones adicionales descubiertas**:
- `setup --help` muestra `--with-stt` como flag existente — M-02 no lo incluye como gap (correcto).
- `DAEMON.md:50` referencia `lib.rs:614` pero la función `run_daemon_server`/`run_supervised` está en `lib.rs:1117` (drift de línea documentado en C-03 §, no requiere acción funcional).
- El oráculo Python `daemon/run.py` **no existe** en el path `daemon/run.py` dentro del commit `7542962` — la ruta cambió durante refactors. Los flags `--language`/`--with-stt` para daemon se encuentran en `cli.py` (no `daemon/run.py`), pero el daemon Rust carece de estos flags CLI.

## 8. Método y fuentes

- **Auditoría 2026-09-05**: 5 subagentes paralelos (AG-SPEECH, AG-DAEMON, AG-RUNTIME, AG-GOV, AG-DOCS) verificando cada hallazgo contra `src/main.rs`, `docs/CLI/CONTRACT.md`, `USAGE.md`, otros docs, y la salida empírica de `cargo run -- <comando> --help`.
- **Oráculo Python**: `7542962` (2026-08-25) `src/ai_voice_interconnector/cli.py` + `daemon/run.py`; superficie `add_parser/add_argument` verificada idéntica tras rename `ca7d00c`.
- **CLI Rust**: `HEAD 79b87bb` `src/main.rs:55` (`Cli` + `Commands`/`VoiceCommands`/`SpeechCommands`/`DaemonCommands`) — ningún otro binario aporta superficie.
- **Árbitro contrato**: `docs/CLI/CONTRACT.md` (normativo, ya en stack Rust `crates/avi-stt/src/parakeet.rs` §11).
- **Drifts documentales**: `README.md`, `USAGE.md`, `docs/BUILD.md`, `docs/CLI/commands/DAEMON.md`, `docs/DESIGN.md`, `docs/GOAL.md`, `THIRD-PARTY-LICENSES.md`, `SOURCE-OFFER.md`, `docs/CLI/README.md`, `CLAUDE.md`/`AGENTS.md` contra `src/main.rs`, `Cargo.toml`, `Cargo.lock`, `.circleci/config.yml`, `crates/xtask/src/main.rs`, `tests/cli_golden.rs`, `crates/avi-daemon`.
- **Limitación**: comparación de superficie de parseo/validación visible; no audita fidelidad de payload runtime ni daemon HTTP interno.
- **Proveniencia de líneas**: las citas `file:line` refieren al HEAD auditado `79b87bb` salvo C-02/M-01 (re-verificados 2026-09-10 contra el árbol actual) y C-05 (nuevo de F5).
- **Ground truth F5-harness-estructural (2026-09-10)**: corrida pesada serial ejecutada contra modelos y daemon reales (ver `.claude/orchestration/harness-estructural/F5-ground-truth.md`); de ahí proviene C-05, con prueba a nivel crate (`cargo test -p avi-translation --features native-translation` 6/6 en rojo).