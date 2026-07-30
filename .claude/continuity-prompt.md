---
name: speech-redesign-evaluation
description: Evaluación del design doc completada; desviaciones menores identificadas, próxima tarea: resolverlas
metadata:
  type: project
---

# Prompt de continuidad — Evaluación del design doc `cli-redesign.md`

## Objetivo activo

La evaluación de `docs/proposals/cli-redesign.md` contra la implementación actual está completa. El rediseño está **completamente implementado** (12/12 sub-secciones de §2 cubiertas, 615 tests verdes). Se identificaron desviaciones menores no funcionales que deben resolverse de forma coherente y consistente en todo el proyecto. La próxima tarea es resolver esas desviaciones.

## Progreso verificado

- **Movimiento 1**: completo (limpieza de superficie, `speak` eliminado del código `.py`).
- **Movimiento 2**: completo y pusheado. Dos commits en `main`.
- **Movimiento 3**: completo en `0751ad5`. 6 archivos, +1812 −171 líneas.
- **Suite**: 615 tests, 0 fallos.
- **Evaluación del design doc** (`docs/proposals/cli-redesign.md`):
  - §2.1–§2.12: 12/12 cubiertos sin desviaciones materiales en el código.
  - Desviaciones menores encontradas (ver «Desviaciones identificadas»).

## Desviaciones identificadas

| # | Archivo | Desviación | Tipo |
|---|---|---|---|
| 1 | `main()` en `cli.py:2113` | `sys.exit(EXIT_INTERRUPTED)` no pasa por `CliError` — difiere del principio declarado de que toda salida no-cero pase por `main()` vía `CliError` | Diseño |
| 2 | `docs/DAEMON-MODE.md` | Describe la arquitectura vieja (`speak` como comando único, `cmd_speak`, diagrama con `(cmd_speak)`) — no es código zombie pero es documentación desactualizada | Documentación |
| 3 | `scripts/create_installer_windows.py:190` | Referencia al comando viejo `speak` en un mensaje de progreso del instalador | Documentación |
| 4 | `SECURITY.md` | La propuesta pedía su retirada (el sandbox de rutas ya no aplica), pero el archivo sobrevive con otro alcance (firmware/code-signing) | Alcance |
| 5 | `tests/test_cli.py` (MockArgs) | Campo `all` duplicado (líneas 52 y 60) — funciona pero es redundante | Código |
| 6 | `CHANGELOG.md` | No registra explícitamente el Movimiento 1 como la eliminación original de `speak`; la eliminación se registra solo en la entrada consolidada de 0.9.0 | Documentación |

## Decisiones y trade-offs cerrados

- `CliError(BaseException)` — NO es subclass de `Exception`; `except Exception` no lo atrapa. Contratos cerrados.
- `speech remove` con sidecar huérfano (WAV ausente, JSON presente): `remove()` borra el JSON y devuelve True → exit 0 en vez de exit 3. Es la decisión actual documentada.
- El design doc `docs/proposals/cli-redesign.md` es la fuente de verdad del comportamiento: un test verde que fija un comportamiento distinto del doc es un test incorrecto.
- Los 5 payloads de `speech` tienen las claves exactas fijadas por el plan (§2.10).
- El despacho de 3 modos compartido por `synthesize`, `say` y `voice clone` via `_dispatch_synthesis` y `_precompute_cloned_voice`.
- **Las desviaciones son menores y no funcionales**: ninguna afecta la corrección del contrato de salida ni la coherencia de la CLI. Resolverlas es cuestión de consistencia interna, no de corrección.

## Insights de sesión

- La evaluación con 3 sub-agentes en paralelo es eficiente para auditorías de cobertura: código zombie, cobertura del design doc, y docs+tests pueden verificarse independientemente.
- `sys.exit(EXIT_INTERRUPTED)` en `main()` por `KeyboardInterrupt` es la única salida no-cero que no pasa por `CliError` — es una decisión de diseño para la señal del SO, pero difiere del principio declarado.
- `MockArgs.all` duplicado en `test_cli.py` es un código muerto que no causa bugs pero sí confusión.
- `docs/DAEMON-MODE.md` es el archivo más desactualizado: describe `speak` como superficie única, `cmd_speak` como función, y el sandbox de rutas como arquitectura vigente.
- `SECURITY.md` no debe retirarse completamente: su contenido sobre modelo de amenaza del daemon, firma de código y falsos positivos de antivirus sigue vigente. Solo la sección de sandbox de rutas debió eliminarse, pero el sandbox se eliminó en el Movimiento 1 y el archivo sobrevivió con otro alcance legítimo.

## Estado en curso

- Rama `main`, último commit `0751ad5`.
- No hay tareas en curso del Movimiento 3 ni del rediseño.
- El working tree está limpio de cambios funcionales (solo `.claude/continuity-prompt.md` varía según regeneraciones).

## Bloqueadores y preguntas abiertas

- No hay bloqueadores. La resolución de las 6 desviaciones identificadas es el trabajo siguiente y no requiere ninguna decisión de diseño nueva: cada una tiene una corrección obvia y coherente con el diseño actual.

## Próximos pasos (ordenados)

1. **Resolver la desviación #1**: `sys.exit(EXIT_INTERRUPTED)` en `main()` — decidir si se envuelve en `CliError(EXIT_INTERRUPTED, ...)` para unificar el camino de todas las salidas no-cero, o se documenta como excepción deliberada. Releer `cli.py:2084-2113` y `exit_codes.py`.
2. **Resolver la desviación #2**: actualizar `docs/DAEMON-MODE.md` para reflejar la arquitectura actual (`speech say`/`speech synthesize`, sin `speak`, sin sandbox de rutas, sin `cmd_speak`). Releer `daemon/protocol.py`, `daemon/server.py`, `daemon/ipc.py` para obtener la arquitectura actual.
3. **Resolver la desviación #3**: actualizar `scripts/create_installer_windows.py:190` para eliminar la referencia al comando `speak` obsoleto.
4. **Resolver la desviación #4**: decidir si `SECURITY.md` se retira o se le elimina la sección de sandbox de rutas. Si se retira, añadir `.gitignore` si procede. Si se queda, actualizar para reflejar que el sandbox ya no aplica.
5. **Resolver la desviación #5**: eliminar la duplicación de `self.all` en `MockArgs` (`tests/test_cli.py` líneas 52 y 60).
6. **Resolver la desviación #6**: decidir si el CHANGELOG.md registra el Movimiento 1 como la eliminación original de `speak`, o si la entrada consolidada de 0.9.0 es suficiente.
7. **Re-run de la suite** tras cada corrección para verificar que no introduce regresiones.

## Fuentes a re-leer post-compactación

- `docs/proposals/cli-redesign.md` — diseño autoridad (§2.3–§2.11 contrato, §3.4 plan Movimiento 3). Re-leer antes de cualquier decisión de implementación o test.
- `src/tts_sidecar/cli.py` — funciones `_dispatch_synthesis`, `cmd_speech_synthesize`, `cmd_speech_play/list/remove`, `_precompute_cloned_voice`, `cmd_cleanup`, `cmd_setup`, `main()`. Re-leer si se toca el despacho o las salidas de error.
- `src/tts_sidecar/synthetic_speech.py` — funciones `store_root`, `voice_store_dir`, `_resolve`, `_atomic_write`, `save`, `exists`, `wav_path`, `remove`, `list_entries`. Re-leer si se toca la persistencia.
- `src/tts_sidecar/exit_codes.py` — contrato de salida vigente (10 constantes + `CliError`).
- `tests/test_cli.py` — 36 tests de matriz del Movimiento 3; `MockArgs` con campos ampliados; `DaemonDispatchTests`.
- `tests/test_synthetic_speech.py` — 19 tests de aislamiento del almacén.
- `daemon/run.py` — gestión del daemon y `EXIT_STATE_CONFLICT`.
- `docs/DAEMON-MODE.md` — documentación desactualizada que necesita actualización.

## No repetir

- No re-ejecutar los Movimientos 1 y 2 ni su puerta: completos, pusheados y verificados.
- No confiar en inventarios de «completado» sin contrastar contra `docs/proposals/cli-redesign.md`.
- No arrancar el Movimiento 3 sin petición explícita del usuario; no inventar superficie de la feature para satisfacer tablas objetivo de §2.
- No monkeypatchear `_dispatch_synthesis` con `MagicMock()` vacío al testear rutas de error de daemon (causa "not enough values to unpack").
- No omitir `import json` en tests de `cmd_speech_list`/`cmd_speech_remove` que usan `json.loads`.
- No añadir `side_effect=` directamente a `monkeypatch.setattr`; envolver en `MagicMock(side_effect=...)`.
- La desviación de `speech remove` con sidecar huérfano está documentada y es intencional; no "corregirla" sin reabrir el diseño.
- No retirar `SECURITY.md` por completo sin evaluar si su contenido sobre firma de código y falsos positivos de antivirus sigue siendo relevante para el proyecto.

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» sin reabrir decisiones ya cerradas salvo nueva evidencia.