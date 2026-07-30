# Prompt de continuidad — Movimiento 2 cerrado; Movimiento 3 pendiente

## Objetivo activo
Ninguna tarea en curso. El **Movimiento 2 (contrato de salida) está completo, con la puerta pasada**. El siguiente trabajo del rediseño de la CLI es el **Movimiento 3 (la feature)**, que aún NO se ha empezado y queda fuera de alcance hasta que el usuario lo pida.

## Progreso verificado
- **Movimiento 1: completo** (limpieza de superficie). Puertas verificadas en su momento.
- **Movimiento 2: completo y pusheado.** Dos commits en `main`:
  - `0866b10 feat(cli)!:` — grueso del contrato: `exit_codes.py` (módulo hoja, 10 constantes + `CliError(BaseException)`), swap 2↔4, `EXIT_DAEMON_PORT_IN_USE` eliminado (puerto→`EXIT_STATE_CONFLICT`), `VoiceExistsError(ValueError)` (colisión→6 / audio ilegible→2), 35 `sys.exit`→`raise CliError`, `main()` traductor único, clave `ok` fuera de payloads daemon, despacho convergido (una sola `_play_audio`/`_emit_speak_json`), tabla USAGE.md remapeada, 2 tests de gobernanza.
  - `b0dfab2 fix(cli):` — cierre del paso 2.1: `_describe_provision_failure` devuelve la terna `(code, reason, message)` (precondiciones→8 con reason credentials/network/permissions/disk_full; fallback→`EXIT_ERROR`/provision_failed); exclusión mutua declarativa `--daemon`/`--no-daemon` vía `add_mutually_exclusive_group()` (conflicto→exit 2 de argparse); help strings corregidos; `TestDescribeProvisionFailure` (6 casos) + test de conflicto migrado a test de parser.
- **Puerta del Movimiento 2 (Tarea 4): PASS 6/6.** Suite 552 verde; `_play_audio`/`_emit_speak_json` en 2 líneas c/u; ninguna `EXIT_*` fuera de `exit_codes.py` y swap 2↔4 aplicado; `not issubclass(CliError, Exception)`; ninguna `sys.exit` no-cero fuera de `main()` en `cli.py`; superficie de comandos intacta (`cleanup, daemon, devices, doctor, setup, speech, version, voice`).

## Decisiones y trade-offs cerrados
- `CliError(BaseException)`, `main()` traductor único, `"ok"` fuera de payloads daemon, swap 2↔4, `VoiceExistsError(ValueError)`, terna de `_describe_provision_failure`, exclusión mutua declarativa.
- Se commitea directo en `main` (repo pre-release, un solo dueño). Un commit por unidad cohesiva; el del contrato se marcó breaking (`!`).
- `version` en la superficie de comandos es legítimo: viene de `cc1092f` (rename previo), no lo introdujo el Movimiento 2.

## Insights de sesión
- Un test verde no prueba conformidad con el design doc: los tests pueden fijar el comportamiento viejo/incorrecto (p. ej. el antiguo `test_voice_clone_collision_exits_4`). Verificar SIEMPRE contra `docs/proposals/cli-redesign.md`.
- El esquema del protocolo es el campo `schema_version: str = "2"` en `protocol.py:53`, NO una constante de módulo `protocol.SCHEMA_VERSION`.
- `top_level_subparsers(build_parser())` devuelve un `_SubParsersAction`; los comandos se leen vía `.choices.keys()`.
- Las `sys.exit` de `daemon/run.py` (`signal_handler`, bind por puerto/genérico) son del punto de entrada del proceso daemon, fuera del alcance de la invariante «ninguna salida no-cero fuera de `main()`», que aplica a `cli.py`.

## Estado en curso
- Rama `main`, al día con `origin/main` en `b0dfab2`. Árbol de trabajo limpio.
- Plan de referencia del Movimiento 2 (ya ejecutado): `C:\Users\Cristian\.claude\plans\sunny-drifting-willow.md`.

## Bloqueadores y preguntas abiertas
- Ninguno. El Movimiento 2 está cerrado. No hay trabajo pendiente asignado.

## Próximos pasos (ordenados)
1. Esperar la dirección del usuario. El candidato natural es **arrancar el Movimiento 3 (la feature)**: `speech synthesize`, `speech play` y las sub-acciones de habla sintética (§3.4 del design doc), que añaden superficie por primera vez. No empezar sin que el usuario lo pida y, llegado el caso, planificarlo con `/create-plan` a partir de §3.4.

## Fuentes a re-leer post-compactación
- `docs/proposals/cli-redesign.md` §3.4 (Movimiento 3) — alcance de la feature, cuando el usuario decida arrancarla.
- `src/tts_sidecar/cli.py` — `build_parser` (superficie actual) y `main()` (traductor único) como base sobre la que se monta el Movimiento 3.
- `src/tts_sidecar/exit_codes.py` — contrato de salida vigente (10 constantes + `CliError`).

## No repetir
- No re-ejecutar los Movimientos 1 y 2 ni su puerta: completos, pusheados (`b0dfab2`) y verificados.
- No confiar en inventarios de «completado» sin contrastar contra el design doc.
- No arrancar el Movimiento 3 sin petición explícita del usuario; no inventar superficie de la feature para satisfacer tablas objetivo de §2.

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» sin reabrir decisiones ya cerradas salvo nueva evidencia.
