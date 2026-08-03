# Prompt de continuidad — Orquestación del refactor cross-lingual (es→en) vía sub-agentes

## Objetivo activo
Orquestar, como **agente principal (orquestador)**, el ciclo completo de exploración → diseño → implementación → verificación → cierre del rediseño de la CLI documentado en `docs/proposals/cli-redesign.md`: integración del **modelo inglés base** (`ChatterboxTTS`) simétrica a `es-mx-latam` para **síntesis cross-lingual** (audio en inglés con timbre de voz clonada en español) + **parametrización** de `exaggeration`/`cfg_weight`/`temperature` en ambas rutas. El orquestador **delega** en sub-agentes especializados y **no** ejecuta él mismo las fases de investigación/diseño/implementación.

## Progreso verificado
- `docs/proposals/cli-redesign.md` existe (untracked, sin commitear), 377 líneas, 4 secciones: §1 Introducción, §2 as-is, §3 to-be, §4 proceso de implementación (Fases 0–5). Ya pasó por `/unwrap-markdown-paragraphs` (sin hard wraps artificiales) y por limpieza de drift.
- La propuesta es el diseño de referencia; las Fases 0–5 con sus criterios «Verificar» son el plan de alto nivel a refinar/ejecutar.

## Decisiones y trade-offs cerrados
- **Flujo de orquestación fijado por el usuario** (esta es la tarea): (1) sub-agente con skill **`investigate`** explora el código fuente y produce un reporte; (2) a partir de ese reporte, sub-agente con skill **`create-plan`** diseña el plan de implementación; (3) el **orquestador refina** ese plan; (4) tercer sub-agente hace la **implementación exhaustiva**; (5) el orquestador **verifica**, corre y **corrige tests**; (6) un último sub-agente **corrige drifts documentales**; (7) el orquestador hace **commit y push** para cristalizar.
- **Modelo por rol** (fijado por el usuario): orquestador (agente principal) = **Fable 5** (`claude-fable-5`); exploración/`investigate` = **Sonnet 5** (`claude-sonnet-5`); diseño/`create-plan` = **Opus 4.8** (`claude-opus-4-8`); implementación exhaustiva = **Sonnet 5**; corrección de drifts documentales = **Sonnet 5**. La verificación/corrección de tests (paso 5) y el refinamiento del plan (paso 3) los ejecuta el propio orquestador (Fable 5).
- Diseño técnico ya cerrado en la propuesta (no reabrir): default `setup --language all`; default síntesis `--language es-latam` (retrocompat); `all` inválido en el eje de síntesis; ternas de defaults por ruta (es-latam 0.75/0.5/0.8 = comportamiento efectivo actual, en 0.65/0.3/0.7); `schema_version` `"2"`→`"3"`; `HealthResponse.model_loaded` bool→estructura por idioma; exit codes sin cambios (idioma va en `reason`/mensaje).
- Invariantes: no pasar `language_id` al inglés base; prohibir `cfg_weight=0.0`; no parchar la librería `chatterbox`; ASR/MT fuera de alcance.

## Insights de sesión
- Contratos pre-release, un solo dueño → la única restricción dura es la **consistencia entre repos**, no la retrocompatibilidad. El salto de esquema a `"3"` es admisible.
- Windows: `python`/`python3` resuelven a un stub del Microsoft Store (exit 49). Intérprete real: `/c/Users/Cristian/AppData/Local/Programs/Python/Python313/python`.
- Para PowerShell usar siempre `pwsh` (7), nunca 5.1 (PSModulePath contaminado).
- Editar bloques multi-línea con acentos UTF-8 puede fallar el match de `Edit`; partir en fragmentos únicos cortos o usar `cat -A` para inspeccionar bytes.

## Estado en curso
- Documento de diseño: `docs/proposals/cli-redesign.md` (untracked).
- Rama: `main`. `git status` inicial mostraba `M .claude/continuity-prompt.md` (este archivo) + el doc untracked.
- Framework de tests configurado (suite mencionada en 615 tests verdes en trabajo previo del repo); enfoque **test-first** donde haya framework (AGENTS.md §2).
- Ningún sub-agente lanzado todavía; el flujo aún no ha comenzado.

## Bloqueadores y preguntas abiertas
- Confirmar los nombres exactos de las skills `investigate` y `create-plan` disponibles en el entorno antes de delegar (verificar en el listado de skills invocables).
- Decidir si el trabajo se hace sobre `main` directamente o en una rama nueva antes del commit/push final (AGENTS.md §5: si estás en la default branch, ramificar primero).

## Próximos pasos (ordenados)
1. **Delegar la exploración** (**Sonnet 5**): lanzar un sub-agente que use la skill `investigate` sobre el código fuente relevante (`cli.py`, `model_cache.py`, `model_loader.py`, `engine.py`, `synthesis.py`, `daemon/run.py`, `daemon/server.py`, `protocol.py`, `ipc.py`, `exit_codes.py`) guiado por `docs/proposals/cli-redesign.md`; recoger su reporte.
2. **Delegar el diseño del plan** (**Opus 4.8**): a partir del reporte, lanzar un sub-agente con la skill `create-plan` para producir el plan de implementación detallado (respetando las Fases 0–5 y sus criterios «Verificar»).
3. **Refinar el plan** (orquestador, **Fable 5**): revisar y ajustar el plan del sub-agente antes de ejecutarlo.
4. **Delegar la implementación exhaustiva** (**Sonnet 5**): entregar el plan refinado a un tercer sub-agente que implemente el refactor + features.
5. **Verificar** (orquestador, **Fable 5**): correr la suite; comprobar y **corregir tests** hasta verde.
6. **Delegar corrección de drifts documentales** (**Sonnet 5**): último sub-agente que reconcilie `USAGE.md`, `README.md`, `DAEMON-MODE.md` y demás docs con la implementación.
7. **Cristalizar** (orquestador): `git` commit (mensaje en español, termina en «Resumen de cambios», sin trailer Co-Authored-By) y push. Ramificar antes si se decide no commitear directo en `main`.

## Fuentes a re-leer post-compactación
- `docs/proposals/cli-redesign.md` — diseño completo; Fases 0–5 (líneas 300–377) son el esqueleto del plan.
- `src/tts_sidecar/synthesis.py:92` — única llamada `generate(...)`; punto de ramificación por idioma + enhebrado de 3 params.
- `src/tts_sidecar/daemon/run.py:102,168` — `get_instance(model="es-mx-latam")` hardcodeado + evicción de auto-restart.
- `src/tts_sidecar/model_cache.py:14,32,152,222` — `MODELS`, `MODEL_REVISIONS`/`BASE_MODEL_REVISION`, `is_model_cached` (rama por-archivo solo es-mx-latam), `model_cache_dirs`.
- `src/tts_sidecar/cli.py` — flags de síntesis, gate `_require_model_cached`, handlers `FileNotFoundError`, `schema_version`.
- `src/tts_sidecar/protocol.py`, `ipc.py`, `daemon/server.py` — contrato IPC a extender (`SynthesizeRequest`, `HealthResponse`).
- `CLAUDE.md` / `AGENTS.md` — §3 simplicidad, §4 cambios quirúrgicos, §5 commits; no crear artefactos no aprobados.
- Índice de memoria (`MEMORY.md`) — convenciones de commit, pwsh7, contratos no son restricción dura.

## No repetir
- No arrancar la implementación tú mismo saltándote la cadena de sub-agentes: el usuario definió explícitamente la delegación investigate → create-plan → refinar → implementar → verificar → drift-docs → commit/push.
- No reabrir decisiones de diseño ya cerradas en la propuesta (defaults, esquema `"3"`, invariantes) salvo evidencia nueva.
- No re-ejecutar `/unwrap-markdown-paragraphs` sobre el doc (ya está procesado, es no-op).
- No commitear nada sin que el flujo llegue al paso 7; no añadir scripts/docs no solicitados.
- No usar `python`/`python3` directos en Windows (stub del Store); no usar PowerShell 5.1.

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» sin reabrir decisiones ya cerradas salvo nueva evidencia.
