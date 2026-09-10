# Revisión: hallazgos abiertos tras C-05 — ciclo de vida y warmup del daemon

- **Fecha**: 2026-09-10
- **Estado**: 0 resueltos — 2 abiertos (E2, E3) + 2 pendientes de medición (G1, G2)
- **Origen**: orquestación `c05-motor-ct2` (C-05/E1 motor CT2 ✅ resuelto y verificado; este documento preserva el rastro de lo que quedó abierto al cerrarla).
- **Fuentes**: `docs/reviews/2026-09-10-correccion-estructural-suite-tests.md` §8 (E2) y §9 (E3), actas en `.claude/orchestration/c05-motor-ct2/F5-ground-truth.md`, `F4b-harness-notas.md`.

> **Leyenda**: ⏳ **Abierto** — defecto o flakiness observado con evidencia, sin fix · 📏 **Pendiente de medición** — decisión técnica tomada por regla de evidencia, falta re-medir con modelos reales.

## Tabla de contenidos

- 1. Resumen ejecutivo
- 2. Mapa de estado
- 3. E2 — Daemon retiene stdout del spawner (⏳ Abierto)
- 4. E3 — Ciclo de vida/warmup: flakiness + huérfanos (⏳ Abierto)
- 5. G1/G2 — Pendientes de medición (📏)
- 6. Orden de ataque recomendado

## 1. Resumen ejecutivo

El motor de traducción quedó reparado y verificado (C-05/E1). Lo que la orquestación dejó al descubierto —y no pudo cerrar en su scope— es el ciclo de vida del daemon: retiene el stdio del spawner (E2, previo), se degrada al reutilizarse (exit 5 donde en fresco da exit 0), deja huérfanos en toda vía anormal (abortos, fallos, timeouts) y su warmup varía entre 27s-ok y 150s+-atascado sin dejar traza (el hijo `qwen` arranca con stdio a `null`). Ninguna de estas cuatro manifestaciones tiene causa demostrada; todas tienen evidencia de observación con hora y PIDs.

## 2. Mapa de estado

| ID | Hallazgo | Estado | Origen |
|---|---|---|---|
| E2 | Daemon retiene stdout del spawner | ⏳ Abierto | Review 2026-09-10 §8 (previo a C-05) |
| E3-i | Dub vía daemon reutilizado → exit 5 (12s FAILED 15:23Z) | ⏳ Abierto | F5 c05-motor-ct2 |
| E3-ii | Abortos dejan `ai-voice-interconnector` + `qwen_tts` huérfanos | ⏳ Abierto | F5 c05-motor-ct2 (4 ocasiones en un día) |
| E3-iii | Varianza 27s-ok / 150s+-atasco-preaudio entre corridas idénticas | ⏳ Abierto | F5 c05-motor-ct2 |
| E3-iv | `qwen` con stdio a `null` = atascos invisibles | ⏳ Abierto | Código (`spawn_background`, `avi-tts`) |
| G1 | Techos de guards F4b (180s resto, 360s dub) | 📏 Pendiente de medición | F4b (derivados de constantes, sin baseline real) |
| G2 | Rama `vocab.json`+`merges.txt` del gate | 📏 Pendiente de medición | F3 regla de evidencia (rechazada por defecto) |

## 3. E2 — Daemon retiene stdout del spawner (⏳ Abierto)

**El daemon retiene el stdout del spawner** — evidenciado en F5 por eliminación (matar qwen no liberó el log; matar el daemon PID 12380 sí, dos ocasiones). `spawn_background` anula stdio pero el daemon retuvo el destino de redirección (`>> log 2>&1`; hipótesis no demostrada: se hereda stderr). Un daemon así mantiene ocupados archivos del spawner y puede atar su consola — la misma familia de los incidentes originales (2 huérfanos en la corrida que originó la revisión estructural).

Fix propuesto (producto): auditar los tres streams en `spawn_background` y respawn de supervisión + regresión con tempfile. Decisión requerida: ¿en qué scope se agenda?

## 4. E3 — Ciclo de vida/warmup: flakiness + huérfanos (⏳ Abierto)

Mismo comando en las tres corridas (`cargo test --features native-stt,native-translation --test cli_golden -- --test-threads=1 --exact tts::dub_daemon_con_traduccion --nocapture`):

- **E3-i Degradación por reutilización**: con daemon residual reutilizado (15:23Z), 12s FAILED — dub vía daemon exit 5 (timeout cliente `/dub` 10s). Con arranque fresco (~15:20Z), exit 0 en 9.97s (total 27.36s, warm intento 29/50, 17.38s). El daemon responde `status`/`warm` pero no sirve `/dub`.
- **E3-ii Huérfanos tras abortos**: 4 ocasiones en un día; cada corrida abortada o fallida antes del apagado deja la pareja daemon+qwen viva y ociosa (CPU congelada), ocupando puertos y ficheros y contaminando la siguiente corrida (la fixture la detecta y re-arranca, pero paga el costo).
- **E3-iii Varianza entre corridas idénticas**: 27s ok frente a 150s+ sin audio (15:34Z, arranque limpio verificado, qwen 316s CPU en 2.7 min de pared). Atasco pre-audio genuino, fase exacta desconocida.
- **E3-iv Atascos invisibles**: el hijo qwen arranca con stdio a `null`, por lo que (iii) no deja traza observable en ningún canal.

No se afirma causa más allá de lo observado.

## 5. G1/G2 — Pendientes de medición (📏)

- **G1**: los techos de guards (180s resto, 360s dub, `tests/cli_golden.rs`) se derivaron de constantes del producto (cliente 120s, `/dub` 10s, arranque 10s), sin baseline medido. Re-medir con modelos reales y ajustar.
- **G2**: la rama `vocab.json`+`merges.txt` del gate se rechaza por defecto (orden real de `auto::Tokenizer` no demostrada para nuestros snapshots). Aceptarla solo con evidencia.

## 6. Orden de ataque recomendado

1. **E3-iv primero** (observabilidad del residente: log de qwen a fichero en vez de `null`): sin esto, E3-i/E3-iii son imposibles de diagnosticar y toda futura F5 repite el día de hoy.
2. **E2** (auditoría de streams en `spawn_background`): misma zona de código que E3-iv; probable causa compartida con E3-ii.
3. **E3-i/E3-iii** con trazas ya visibles: warmup con deadline diagnosticado, degradación por reutilización.
4. **G1/G2** como cierre de medición dentro de esa misma orquestación.
