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
  **no tiene** el Base → `voice clone` e2e bloqueado en Windows.
- `target/` (gitignored): clips del gate F7, renders A/B/C y de los experimentos, `bench.qvoice`,
  `ref24k.wav`.

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