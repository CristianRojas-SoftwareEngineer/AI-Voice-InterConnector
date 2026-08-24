# Revisión: calidad de síntesis del TTS nativo (Fase 5) — NO cerrada

**Fecha:** 2026-08-14
**Estado:** Fase 5 del plan de migración implementada y cristalizada, **calidad de síntesis pendiente**. Investigación abierta (sección final).
**Autor:** Orquestación `orchestrate` (gate F7, veredicto del usuario).

---

## Resumen ejecutivo

La Fase 5 («TTS nativo Qwen3-TTS») quedó implementada y verificada en el plano funcional
(CLI `speech synthesize|say|dub`, `voice clone`, motor residente HTTP, golden +27 tests, suite
75/75). El gate humano de calidad (F7) **falló**: el audio producido por el binario del motor
compilado para Windows (`vendor/qwen3-tts/qwen_tts.exe`, gcc 16.1.0 MSYS2/UCRT64) suena
degradado (no conserva el timbre de la voz de referencia y resulta robótico), mientras que el
mismo motor compilado en el stack del benchmark (WSL Ubuntu, gcc 15.2.0) produce la calidad
validada. La causa raíz es una **divergencia numérica sistemática entre los dos builds**: mismo
código, mismos pesos, mismo seed y mismos flags generan audio diferente (mel-corr 0.25-0.66 vs
umbral 0.98 del propio motor). Con decodificación greedy (temp 0), cualquier diferencia en los
primeros pasos bifurca la trayectoria completa.

## Contexto

- Plan de migración: `docs/proposals/PLAN-DE-MIGRACIÓN.md`, Fase 5 (§2.4 y §5).
- Ejecución por orquestación controlada (`orchestrate`), gates F0-F7; plan en
  `.claude/orchestration/fase5-tts-nativo/F3-plan-refinado.md` (+ enmienda A1: parche
  `WSAStartup` que arregló `--serve` en Windows).
- Implementación verificada: suite `cargo test --all` 75/75 (baseline 48/48); golden TTS con
  inferencia real vía el residente HTTP (`127.0.0.1:8766`); `voice clone` implementado con
  validaciones (exit 2/3/6) y contrato `{name, timbre, speech, precomputed}`.
- Gate F7 (escucha humana): rechazado — calidad insuficiente; veredicto del usuario:
  reportar, cristalizar el estado, y abrir investigación de calidad de voz y clonación.

## El problema

1. **Clip 1-3 del gate F7** (`target/tts-verification-clips/clip{1,2,3}_*.wav`, voces preset
   `ryan`/`vivian`, `--int8` 1 hilo): «mala calidad» por oído del usuario.
2. **A/B/C (mismo texto corto del benchmark, voz clonada del benchmark, flags de producción
   `--int4 -j4 -T 0 --seed 42 --stream`)**: ni A (preset int8), ni B (preset int4), ni
   C (voz clonada int4) alcanzan la calidad de la referencia del benchmark → el problema no
   era la voz preset ni los flags: es el **binario Windows**.
3. **Candidato scalar** (`win_scalar_int4.wav`, build `SIMD=scalar`): «no conserva el timbre y
   es un poco robótico» (oído del usuario) → descartado.

## Evidencia y experimentos

Todos los renders usan el texto corto del benchmark
(`TTS-CPU-BENCHMARK\shared\texts\corto_es.txt`, ASCII-izado) y la voz clonada del benchmark
(`bench.qvoice`), `-l Spanish -j4 -T 0 --seed 42 --stream` salvo indicación.

> **Nota de conservación (2026-08-14, decisión del usuario):** todos los WAV citados en este
> documento como resultados (clips del gate F7 en `target/tts-verification-clips/`, A/B/C y
> renders de los experimentos en `target/`) fueron **eliminados**. Este documento conserva el
> análisis y sus métricas; los archivos son regenerables repitiendo los comandos documentados.
> Se conservan solo los insumos de producción: `target/bench.qvoice` y `target/ref24k.wav`.

| Comparación (archivo1 vs archivo2) | mel-corr | Duración (s) | Veredicto |
|---|---|---|---|
| `win_bf16.wav` vs `wsl_bf16.wav` (bf16) | 0.38181 | 7.20 vs 6.80 | Divergencia total, sin cuantización |
| `win_int8.wav` vs `wsl_int8.wav` (int8) | 0.25380 | 6.64 vs 5.84 | Divergencia total |
| `ab_c_clone_int4.wav` vs `wsl_corto_clone.wav` (int4) | 0.66379 | 6.56 vs 6.24 | Divergencia |
| `win_scalar_bf16.wav` vs `wsl_bf16.wav` (scalar) | 0.61635 | 6.72 vs 6.80 | Cercano en duración, aún divergente |
| `win_avx2_nofm.wav` vs `wsl_bf16.wav` (-fno-fast-math) | 0.32809 | 7.44 vs 6.80 | Peor que el build estándar |
| `win_bf16.wav` vs `win_bf16_r2.wav` (mismo build 2×) | 1.00000 | 7.20 vs 7.20 | Determinista intra-plataforma |
| `wsl_bf16.wav` vs `wsl_bf16_r2.wav` (mismo build 2×) | 1.00000 | 6.80 vs 6.80 | Determinista intra-plataforma |

- Umbral del propio motor (golden): mel-corr ≥ 0.98 (`vendor/qwen3-tts/tests/compare_audio.py`).
- `qwen_tts.exe --self-test` en Windows: **PASS** (los kernels son autoconsistentes).
- Modelos **byte-idénticos** entre plataformas: `model.safetensors` 1,811,626,576 B
  (`vendor/qwen3-tts/qwen3-tts-0.6b/` = `/root/models/qwen3-tts-0.6b/`).
- Código **idéntico** (SHA-256): `qwen_tts.c`, `qwen_tts_kernels.c`, `qwen_tts_server.c`;
  `main.c` difiere solo por el parche A1 (`WSAStartup`, `#ifdef _WIN32`).
- Compiladores: Windows = gcc 16.1.0 (MSYS2 UCRT64, `-mavx2 -mfma -ffast-math -static`,
  OpenBLAS estático MSYS2); WSL = gcc 15.2.0 (Ubuntu 15.2.0-16ubuntu1, `make blas`).

## Archivos y directorios de referencia

**Stack validado (producción de referencia, calidad aprobada por oído):**
- `C:\Users\Cristian\Desktop\TTS-CPU-BENCHMARK\` — benchmark completo.
  - `qwen3-tts\notes.md` — memoria del benchmark (clonado con Base, `--int4 -j4`, WSL).
  - `qwen3-tts\run.py`, `qwen3-tts\.wsl\engine.sh` — invocaciones exactas del motor.
  - `qwen3-tts\.clone_cache\50d38ebe64e6f9fb\` — `voice.qvoice` (16.8 MB), `ref24k.wav`, `text.txt`.
  - `results\qwen3-tts\qwen3-tts_corto_es_warm_r1.wav` — clip oficial de referencia (voz clonada).
  - `shared\reference\speech_test.wav` — clip de referencia para el clonado (48 kHz estéreo → 24 kHz mono).
- WSL Ubuntu: `/root/qwen3-tts/qwen_tts` (motor gcc 15.2.0), `/root/models/qwen3-tts-0.6b`
  (CustomVoice), `/root/models/qwen3-tts-0.6b-base` (Base, 1,829,344,272 B), `/root/models/bench.qvoice`,
  `/root/models/test_es.wav`.

**Stack Windows (el que produce la calidad rechazada):**
- `vendor/qwen3-tts/qwen_tts.exe` — build gcc 16.1.0 UCRT64 + parche A1.
- `vendor/qwen3-tts/Makefile` — rama MinGW (líneas 56-77: shims, OpenBLAS estático, `-lws2_32`).
- `vendor/qwen3-tts/third_party/ingot/mingw_shim/` — shims POSIX (socket/mmap/unistd).
- `vendor/qwen3-tts/main.c:1848-1854` — `--ref-audio` exige modelo Base; el runtime Windows
  **no tenía** el Base en el momento del gate F7 → `voice clone` e2e bloqueado en Windows. El Base
  se provisionó posteriormente (`vendor/qwen3-tts/qwen3-tts-0.6b-base/`) y el driver normaliza la
  referencia a 24 kHz mono antes de clonar, desbloqueando `voice clone` e2e y el golden
  `voice_clone_exito` (véase `docs/proposals/progress.md`, Fase 5).
- `target/` (gitignored): solo insumos de producción conservados — `bench.qvoice` (voz clonada
  validada del benchmark) y `ref24k.wav` (audio de referencia del clonado); los renders y clips
  del gate fueron eliminados (ver nota de conservación).

**Artefactos de la orquestación:** `.claude/orchestration/fase5-tts-nativo/` (F0-F6, gitignored).

## Análisis de causa raíz

1. **Divergencia numérica build-dependiente (confirmada).** Renders idénticos (mismo binario,
   mismo seed, temp 0) son bit-deterministas (mel 1.0); renders entre builds divergen desde
   temprano. El `--self-test` pasa en ambos, los kernels son correctos a nivel aislado; la
   bifurcación nace de diferencias de redondeo acumuladas (codegen AVX2/FMA, OpenBLAS estático,
   o libm UCRT vs glibc) que voltean un `argmax` de la decodificación greedy y re-prosodian
   todo el resto.
2. **El build AVX2 gcc 16 es el peor** (7.20-7.44 s vs 6.80 s de WSL); `-fno-fast-math` empeora
   (7.44 s); **scalar se acerca** (6.72-5.84 s) pero aún no conserva timbre → sospechosos
   primarios: codegen de los kernels AVX2 en gcc 16.1.0 y/o OpenBLAS estático MSYS2.
3. **Clonado bloqueado en Windows** (sin modelo Base): la funcionalidad fundamental de
   producción (voz clonada) no pudo verificarse e2e en Windows; el clip 2 del gate usó preset
   `vivian` en lugar de voz clonada.

## Impacto en el plan de migración

- **Fase 5 NO cerrada** (veredicto del usuario en el gate F7, 2026-08-14).
- Implementación cristalizada en el commit actual (código + parche A1 + docs + este reporte).
- Pendiente: calidad de síntesis equivalente al stack del benchmark en el runtime Windows,
  y verificación e2e del clonado con el modelo Base provisionado.

## Reconciliación con el fix posterior (2026-08-14)

> **Acta histórica preservada:** este documento registra el veredicto del gate F7 y su diagństico
> de build. El fix que siguió **no invalida** ese diagństico empírico (las renders A/B/C efectivamente
> divergen entre builds, mel-corr 0.25-0.66), pero **lo replantea de causa**: la degradación que el
> gate F7 oyó no era de build, era del **contrato de invocación del driver Rust**. El driver llamaba
> al motor con `--int8` 1 hilo, `temperature 0.5`, voz preset y sin modelo Base —ninguna de las
> condiciones del benchmark validado (`--int4 -j4 --temperature 0 --seed 42`, voz clonada, Base).
>
> Al alinear el driver a ese contrato (ver `F3-plan-refinado.md`), la inferencia reproduce la calidad
> aprobada por oído y el WER de los golden vuelve verde —sin recompilar el motor—, demostrando que la
> causa de la degradación escuchada era invocación/voz/muestreo, no divergencia numérica entre builds.
> La comparación **bit-a-bit / mel-corr (≥ 0.98) Windows-vs-WSL** sigue abierta como eje de
> investigación (H1-H3): el fix no midió paridad de redondeo entre builds, y no impide que coexistan
> ambas hipótesis sobre la naturaleza de la divergencia de build documentada arriba.

## Investigación preparada (siguiente paso)

**Objetivos:** (1) producir en Windows audio con la calidad del stack WSL (timbre fiel, no
robótico); (2) clonado e2e funcional en Windows (provisión del modelo Base).

**Hipótesis (orden de costo):**
- **H1 — codegen gcc 16.1.0 (kernels AVX2).** Prueba: E1 build con clang64 (MSYS2); E2
  cross-compile del motor para Windows con gcc 15 mingw-w64 desde WSL; E3 (control) build WSL
  con un gcc 16 si está disponible, para confirmar la direccionalidad del efecto.
- **H2 — OpenBLAS estático MSYS2 vs Ubuntu.** Prueba: E4 build Windows sin `-DUSE_BLAS`
  (edición temporal del Makefile, revertida tras el experimento) o con otro openblas.
- **H3 — libm UCRT vs glibc.** Queda cubierta por E1/E2 si la divergencia persiste en ambos.

**Criterios de éxito (todos):**
1. mel-corr ≥ 0.98 del build candidato vs el render WSL del mismo texto/voz/flags/seed
   (`vendor/qwen3-tts/tests/compare_audio.py`).
2. Prueba de oído: timbre conservado y sin artefactos vs la referencia del benchmark.
3. Clonado e2e en Windows: `voice clone` con Base provisionado + `synthesize` con `.qvoice`
   (WER ≤ 0.25 y oído).

**Riesgos y decisión de respaldo:** si la divergencia resulta intrínseca del runtime
(UCRT/libm/OpenBLAS de MSYS2), la alternativa es adoptar el motor WSL como motor de producción
(invocación vía `wsl.exe` desde el CLI Rust, como hace el benchmark) — decisión de arquitectura
a plantear al usuario.

---

## Actualización 2026-08-24 — Gate híbrido formalizado, medido y veredicto

El gate original de cierre (`mel-corr ≥ 0.98` build Windows vs render WSL) se redefinió como
**criterio híbrido**, por tres motivos: (1) la degradación del gate F7 era del contrato de
invocación del driver, ya corregido; (2) el propio `compare_audio.py` (líneas 9-12, 46-47)
documenta que un build cross-ISA no es bit-idéntico y usa `--min-corr 0.95`; (3) con `temperature
0.35` (muestreo estocástico) la comparación bit-a-bit pierde sentido. Nuevo criterio:

| Id | Criterio | Rol | Resultado (corpus 3 frases ES, contrato producción) |
|----|----------|-----|------|
| **D** | mel-corr ≥ 0.95, temp 0 greedy, Windows vs WSL | Diagnóstico (no bloquea) | 🔴 **0.526 / 0.321 / 0.202** + drift de duración (s3: 6.0 s vs 12.2 s) → confirma la divergencia de build |
| **C1** | WER ≤ 0.25 (contrato producción, Whisper GGUF) | Gate funcional | 🟢 golden `synthesize`/`say`/`voice_clone`/`dub` con Whisper real (4/4) |
| **C2** | speaker-similarity ≥ 0.70 (x-vector del motor, centrado por cohorte) | Gate de timbre | 🟢 Windows **0.892 / 0.897 / 0.871**, indistinguible de WSL (0.888-0.904); techo otro-locutor ~0.50 |
| **C3** | A/B ciega humana (Windows vs WSL) | Gate perceptual | ⚠️ **preferencia WSL 3/3**, suave, por prosodia; sin degradación/robótico en ninguno |

**Hallazgo central:** la divergencia de build (D rojo) **no se traduce en pérdida de timbre**
(C2 mide Windows ≈ WSL; C3 confirma que ninguno suena robótico). El modo de fallo del gate F7 no
reaparece. Lo que queda es una **ventaja suave y consistente de prosodia de WSL** (firma
perceptual residual de la misma divergencia numérica que mide D).

**Métrica C2 (nueva):** `vendor/qwen3-tts/tests/speaker_similarity.py` + asset
`speaker_cohort_mean.npy`. Extrae el x-vector 1024-d del speaker encoder del propio motor
(`--xvector-only --save-voice .bin`) y compara referencia-vs-salida por cosine. Los x-vectors
crudos no discriminan (mismo-locutor ~0.98 vs otro-locutor ~0.94, margen ~0.04); el centrado por
la media de una cohorte de los 9 presets CustomVoice sube el margen a ~0.37 (umbral 0.70).

**Veredicto (decisión del usuario, 2026-08-24):** Fase 5 **NO se cierra**. La preferencia
consistente por WSL justifica **atacar la divergencia de build (H1-H3)** para igualar la prosodia
nativa Windows a la de WSL antes de cerrar. El respaldo (adoptar el motor WSL) queda descartado
por ahora: contradice el objetivo de motor nativo del plan de migración.

**Nota de encoding (bug aparte, no del producto):** pasar texto con acentos/`ñ` como argumento de
línea de comandos al `qwen_tts.exe` en Windows (Git Bash) produce mojibake (`�`). El corpus del
banco se ASCII-izó para esquivarlo, lo que además eliminó la `ñ` (fonema distinto: mañana ≠
manana). El **camino de producción no usa argv**: el driver envía el texto en el cuerpo JSON
UTF-8 al residente HTTP, donde `ñ`/acentos sobreviven (los golden de WER pasan sobre español
acentuado). El motor tokeniza `"Mañana"` (6 tokens de contenido) distinto de `"Manana"` (7).