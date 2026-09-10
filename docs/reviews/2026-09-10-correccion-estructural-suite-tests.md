# Revisión: bloqueos de la suite de tests — causas y corrección estructural

- **Fecha**: 2026-09-10 (diagnóstico) · 2026-09-10 (ejecución F5 y refinado)
- **Estado**: Corregido y verificado salvo 2 tests bloqueados por causa motor externa (E1 → C-05 en `docs/reviews/2026-09-04-hallazgos-pendientes.md`) y 1 hallazgo pendiente de scope de producto (E2, §8).
- **Alcance**: `tests/cli_golden.rs` (44 tests físicos: 36 por defecto + 8 gateados por `native-stt`/`native-translation`), ciclo de vida del daemon (`src/main.rs`, `crates/avi-daemon/src/spawn.rs`, `crates/avi-daemon/src/lib.rs`) y régimen de ejecución de inferencia real. Solo se documentan soluciones que eliminan trabajo o defectos de raíz; no se documentan mitigaciones.
- **Síntoma que originó la revisión**: la corrida completa no terminó en 300s y dejó dos procesos huérfanos (`ai-voice-interconnector` + `qwen_tts`).

## Tabla de contenidos

- 1. Resumen ejecutivo
- 2. Mapa de estado: resueltos vs abiertos
- 3. Problemas resueltos (P1–P5)
- 4. Hipótesis descartadas
- 5. Opciones ejecutadas (O1–O3)
- 6. Criterio de cierre y veredicto
- 7. Método y fuentes
- 8. Pendiente abierto (E2)

## 1. Resumen ejecutivo

No había interbloqueo ni espera infinita: la raíz era triple —cada test pesado repetía carga de modelo (~2,5 GB) + calentamiento con su propio daemon, la vía directa recargaba el motor por invocación, y varias pruebas pagaban reproducción en tiempo real—, más un ciclo de vida frágil (pidfile de una ranura, adhesión ciega, apagados best-effort, sleeps fijos). Las tres soluciones se ejecutaron en F5 y la pesada serial termina en verde salvo 2 tests bloqueados por el motor CT2 roto (E1, otro scope). Detalle por problema en §3, mapa de estado en §2, veredicto medido en §6 y el único pendiente propio en §8.

## 2. Mapa de estado: resueltos vs abiertos

| ID | Problema | Estado | Cierre |
|---|---|---|---|
| P1 | Warmup completo por test (§3) | ✅ Resuelto | Fixture por sesión; arranques frescos 9 → ~5 |
| P2 | Recarga del modelo por invocación directa (§3) | ✅ Resuelto | 1 reenrutado a daemon caliente + 4 testigos en directo |
| P3 | Reproducción en tiempo real como verificación (§3) | ✅ Resuelto | Solo-archivo + 1 humo de audio en `say` |
| P4 | Ciclo de vida frágil (§3) | ✅ Resuelto | Dueño único, asserts explícitos, skips sin efectos |
| P5 | Sleeps fijos en vez de estado (§3) | ✅ Resuelto | Poll-hasta-`warm`; 2 sleeps residuales justificados |
| E1 | Motor CT2 roto (externo, C-05) | ⏳ Abierto | Bloquea 2 tests; scope de producto |
| E2 | Daemon retiene stdout del spawner (§8) | ⏳ Abierto | Scope de producto |

## 3. Problemas resueltos (P1–P5)

### P1 — Warmup completo por test

Ocho tests repetían `stop → start → uso → stop` (`daemon_start_exito` en `tests/cli_golden.rs:1269`, `daemon_restart_rearma` en `:1303`, `daemon_status_running` en `:1323`, `daemon_start_con_auto_restart` en `:1386`, `translate_con_daemon_delega` en `:1418`, `clone_con_daemon_delega` en `:1483`, `dub_daemon_passthrough` en `:1512`, `dub_daemon_con_traduccion` en `:1557`, más `translate_force_daemon_sin_daemon_exit5` en `:1458`). Cada `start` pagaba modelo + calentamiento (`warmup_tts`, `crates/avi-daemon/src/lib.rs:1209-1227`, en segundo plano en `:1244-1248`); costo documentado ~2,7 GB por corrida (`tests/cli_golden.rs:699-700`). **Cierre**: fixture por sesión (arranque con poll-hasta-`warm` contra `src/main.rs:35-37`, detección de rancio, apagado determinista); 4 tests de semántica conservan arranque con restauración (~5 arranques). Medido por invocación: 16 s / 2m33 s (ciclo con warmup real) / 13 s / 13 s.

### P2 — Recarga del modelo por invocación directa

La vía directa construía el motor desde cero por invocación (`Qwen3TtsEngine::new`). **Cierre**: `synthesize_exito_con_label` reenrutado a daemon caliente (32 s con WER); 4 testigos fijados con `--no-daemon` (`texto_corto` 13 s, `say` 14 s, `dub` 19 s, `clone` 1.29 s). Flags y validación comunes antes del despacho (`src/main.rs:2467-2473`).

### P3 — Reproducción en tiempo real como verificación

`say`/`dub` reproducían por los altavoces (`tests/cli_golden.rs:797`) existiendo ya `wav_valido_24k` (`:720`) y `wer_vs_texto` (`:733`). **Cierre**: solo `say_exito_reproduce` conserva una reproducción corta (`"Hola mundo"`); resto solo-archivo (WAV + texto; sin WER nuevo a ciegas). Matiz: el producto `dub` reproduce siempre; el test ya no lo usa como señal.

### P4 — Ciclo de vida frágil

Pidfile de una ranura (`src/main.rs:2339-2357`), adhesión ciega (`already_running`, `:1359-1368`), stops que un panic saltaba (2 huérfanos observados), `let _` en starts y aserción condicional. **Cierre**: todo el ciclo pasa por helpers de la fixture (cero ciclos dispersos, cero `let _` sobre resultados del ciclo, cero retornos silenciosos ni condicionales); skips sin efectos; `exit5` con precondición auto-establecida; barridos sin residuos.

### P5 — Sleeps fijos en vez de estado

Sleeps de 300–500 ms tras cada `start`/`stop` asumiendo el warmup. **Cierre**: quedan 2 (`tests/cli_golden.rs:109` intervalo del poll, `:500` sandbox de `uninstall`); la espera observa `warm == "warm"` en `/health` — F5 demostró que bind ≠ servible (frío fallaba con exit 5, tibio pasaba).

## 4. Hipótesis descartadas

- **Interbloqueo de locks**: `STATE_LOCK` (`:27`) y `TTS_LOCK` (`:702`, vía `lock_tts` en `:704`) siempre en orden STATE→TTS, nunca dos veces. Imposible por construcción.
- **Pipes heredados**: corrección en `crates/avi-daemon/src/spawn.rs:21-66` (clase en `:3-20`). **Matiz F5**: el daemon retuvo igual el destino del spawner — reabierto parcial, ver §8.
- **`daemon serve` en foreground**: ningún test lo invoca salvo `--help` (`:1369`); toda espera del producto tiene deadline.
- **Paralelismo total**: inviable por capacidad —hilos a núcleos físicos (`crates/avi-core/src/engine.rs:185-187`), ~2,7 GB y puertos fijos. La serie se conserva por diseño.

## 5. Opciones ejecutadas (O1–O3)

### O1 — Fixture por sesión

Poll-hasta-`warm` (10 s + 250 ms del producto, reintentos 50/75), detección de rancio (nunca adhiere a tibio), apagado determinista; `etiqueta_unica` (`:709`) contiene el estado compartido. Desviación: apagado único literal inviable con 4 arranques de semántica y sin teardown en libtest → centralización + restauración (~5 arranques). Elimina N−1 warmups y la clase P4/P5.

### O2 — Vía daemon caliente + testigos

Reenrute sin fallback silencioso; pins `--no-daemon` obligatorios (sin ellos el `Auto` reenruta a los testigos bajo fixture). Elimina las recargas del reenrutado sin perder la ruta directa.

### O3 — Solo-archivo + humo

WAV + WER donde existían; humo único acotado. Elimina la pared de tiempo real con señal más estricta.

## 6. Criterio de cierre y veredicto

Orden seguido: O1 → O2+O3+colaterales en paralelo, serie conservada. Baselines: lib 14/14, daemon 6/6+7/7, rápidas 29/29 en segundos, `cargo check` sin warnings nuevos. Pesada serial en verde test por test (tabla en F5 §2 de `.claude/orchestration/harness-estructural/F5-ground-truth.md`), con re-verificación en frío del readiness (35 s). **No verde total por E1**: `translate_es_a_en` (exit 9) y `dub_daemon_con_traduccion` (exit 1) — `failed to create a tokenizer` a nivel crate (C-05).

## 7. Método y fuentes

Diagnóstico sin mutaciones (ciclo `src/main.rs:1353-1571`, pidfile `:2339-2357`, warmup `lib.rs:1209-1227` + `:1244-1248`, hilos `engine.rs:185-187`, harness `:27,699-797`, ciclos (primero `:1269`, último `:1557`)) + corrida interrumpida original (>300 s, 2 huérfanos). Ejecución F5: una invocación por pesada (`--test-threads=1 --exact`) con pared registrada, modelos y daemon reales, barridos sin residuos. Base de los tiempos: internos del test (`finished in`), salvo mención explícita a pared de invocación. Detalle: acta F5 y `F4-implementacion-notas.md` en `.claude/orchestration/harness-estructural/`.

## 8. Pendiente abierto (E2)

**El daemon retiene el stdout del spawner** — evidenciado en F5 por eliminación (matar qwen no liberó el log; matar el daemon PID 12380 sí, dos ocasiones). Revisa el descartado de §4: `spawn_background` anula stdio pero el daemon retuvo el destino de redirección (`>> log 2>&1`; hipótesis: se hereda stderr). Un daemon así mantiene ocupados archivos del spawner y puede atar su consola — la misma familia de los incidentes originales. Fix (producto): auditar los tres streams en `spawn_background` y respawn de supervisión + regresión con tempfile. Decisión requerida: ¿en qué scope se agenda?
