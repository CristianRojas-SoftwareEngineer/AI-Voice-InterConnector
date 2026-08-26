# Prompt de continuidad — Limpiezas de residuos de la migración Python/Chatterbox → Rust/Qwen3

## Objetivo activo
Eliminar los **residuos de la migración** (referencias vivas al antiguo motor STT whisper/GGUF y
al stack Python) que quedaron en el árbol Rust, para luego incluir esas limpiezas en un **bump de
versión** con el que el usuario quiere medir cuánto tarda el pipeline `build-all` completo.
La investigación ya está cerrada; ahora toca implementar los arreglos.

## Progreso verificado
- Pipeline `build-all` v0.11.0: los 14 jobs quedaron **verdes** (release publicado, Cask en el tap,
  artefactos autocontenidos con ONNX Runtime empaquetado). Ese trabajo está terminado.
- Investigación de migración completada. Distingue dos clases de pendientes:
  - **Residuos/limpiezas** (tarea activa; se detallan abajo con evidencia file:line).
  - **Funcionalidades del stack original aún incompletas**: YA documentadas en
    `docs/reviews/2026-08-26-funcionalidades-pendientes-tras-migracion-rust.md` (clonado de voz sin
    modelo Base aprovisionado; arranque del daemon en segundo plano; reinicio del daemon que apaga
    pero no rearma). NO es la tarea activa.
- Confirmado en código: el motor STT SÍ está migrado a Parakeet (`crates/avi-stt/src/lib.rs` solo
  exporta `ParakeetEngine`/`detectar_idioma`/`normalizar_texto`). El TTS es Qwen3 vendorizado
  (`vendor/qwen3-tts/`) envuelto por `crates/avi-tts`.

## Decisiones y trade-offs cerrados
- Candidatas al bump = las tres limpiezas quirúrgicas de abajo (no tocan motores).
- El clonado de voz (modelo Base) queda **fuera del bump**: es el único hueco funcional sustantivo,
  más grande; merece su propio cambio, no colgarse del bump.
- No tratar como "residuo" (son referencias vivas e intencionales): rutas de caché
  `ResembleAI--Chatterbox*` en `crates/avi-store/src/lib.rs:29-30` y `crates/xtask/src/main.rs:29-30`
  (las usa `cleanup`/`uninstall` para borrar restos del canal Python del usuario); menciones
  históricas en `CHANGELOG.md`; el feature de compilación `native-stt`.
- Al escribir docs/commits/prompt, describir cada pendiente **por su naturaleza**, sin etiquetas
  ni términos inventados en sesión (regla explícita del usuario).

## Insights de sesión
- `const VERSION` en `src/main.rs` es la fuente de verdad del nombrado de artefactos; el staging del
  pipeline falla-rápido si `VERSION` ≠ tag (sin la `v`). `Cargo.toml` sigue en `0.1.0`. Un bump toca
  ese `const`.
- Los jobs de test de CI corren **sin** `native-stt` → por eso el test muerto (abajo) no rompe CI
  hoy; solo reventaría `cargo test --features native-stt` en local.
- El benchmark STT (`crates/avi-stt/tests/benchmark_latencia.rs:23-44`) usa una **lista fija** de 4
  fixtures (`whisper_sample_16k`, `corpus_sintesis_16k`, `corpus_watermark_16k`,
  `corpus_respuestas_16k`); no descubre por glob.

## Estado en curso
- Rama `main`.
- El working tree se dejó limpio antes de arrancar la remediación (ver «No repetir» para lo que se
  decidió NO commitear).
- Archivos clave para los arreglos: `src/main.rs`, `tests/cli_golden.rs`, `README.md`, `USAGE.md`,
  `CONTRIBUTING.md`, `.cargo/config.toml`, `THIRD-PARTY-LICENSES.md`.

## Bloqueadores y preguntas abiertas
- Test muerto: confirmar si `wer_vs_texto` (`tests/cli_golden.rs:354`) se **llama** en algún test. Si
  es código muerto → eliminarlo; si se usa → reescribir a `ParakeetEngine`. Decidir con la evidencia.
- ¿El bump se hace solo con las tres limpiezas, o el usuario querrá también atacar algún hueco
  funcional? (Intención declarada: medir el pipeline con las limpiezas dentro.)

## Próximos pasos (ordenados)
1. **Corregir el bug vivo del comando `doctor`:** `src/main.rs:1503-1504` chequea
   `is_provisioned("whisper-gguf")`, modelo que ya NO está en `MODEL_REVISIONS`
   (`crates/avi-store/src/lib.rs:381` pinnea `qwen3-tts-0.6b`, `parakeet-tdt-v3`, `marian-es-en`,
   `marian-en-es`). Efecto: `doctor` reporta SIEMPRE "Modelo STT no provisionado" y sale con error
   aun en instalación sana. Arreglo: chequear `parakeet-tdt-v3` y corregir la etiqueta de salida
   "Whisper GGUF" → Parakeet.
2. **Retirar el test muerto que referencia el motor STT eliminado:** `tests/cli_golden.rs:352-358`
   (`wer_vs_texto`) usa `avi_stt::Ct2SttEngine::new("models/whisper/ggml-medium-q8_0.bin")`; ese
   tipo ya no existe (el crate solo exporta `ParakeetEngine`). Rompe
   `cargo test --features native-stt`. Eliminar si es código muerto, o reescribir a `ParakeetEngine`.
3. **Reconciliar la documentación con el motor real (whisper → Parakeet):** `README.md:45,132`;
   `USAGE.md:120,260,814,925`; `CONTRIBUTING.md:23`; comentario stale en `.cargo/config.toml:11-15`
   (el `/MT` de whisper.cpp; el `crt-static` sigue aplicando a ct2rs, solo el comentario está viejo);
   `THIRD-PARTY-LICENSES.md:27` (`ggml-medium-q8_0.bin` — revisar si aún corresponde listarlo).
4. Verificar: `cargo build -p ai-voice-interconnector` y, para el test muerto,
   `cargo test --features native-stt` (o el subconjunto afectado) compila/pasa. Confirmar que
   `doctor` deja de fallar en una instalación sana.
5. Commit en español (skill `conventional-commits`, `Co-Authored-By: Claude`).
6. Bump de versión (`const VERSION` en `src/main.rs`), tag `vX.Y.Z`, y medir la duración del pipeline
   `build-all` completo (objetivo declarado del usuario).
7. (Posterior, aparte) Atacar el clonado de voz del review: pinnear/aprovisionar el modelo Base de
   Qwen3-TTS en `MODEL_REVISIONS` + `setup`, y reactivar `voice_clone_exito`.

## Fuentes a re-leer post-compactación
- `src/main.rs:1499-1540` — `handle_doctor` (bug del `doctor`) y `src/main.rs:1142-1206` (daemon
  start/restart, contexto del review de funcionalidades).
- `crates/avi-store/src/lib.rs:381-413` — `MODEL_REVISIONS` / `MODEL_FILE_PATTERNS`.
- `tests/cli_golden.rs:340-369` — `wer_vs_texto`/`Ct2SttEngine` (test muerto) y `:668-699` (skip del
  clonado de voz).
- `crates/avi-tts/src/lib.rs:206-212, 405-407, 680` — clonado y modelo Base.
- `docs/reviews/2026-08-26-funcionalidades-pendientes-tras-migracion-rust.md` — huecos funcionales
  ya escritos.

## No repetir
- No re-lanzar sub-agentes Explore mientras dure el límite de sesión (caen por "session limit");
  investigar en directo con Grep/Read.
- No tocar las rutas de caché `Chatterbox*` de `cleanup`/`uninstall` ni las menciones históricas del
  `CHANGELOG` — no son residuos.
- No meter el clonado de voz (modelo Base) en el bump: es cambio propio, más grande.
- No filtrar etiquetas ni términos inventados en sesión a docs/commits/prompt: describir por el
  hallazgo.
- Fixtures huérfanos `corpus_timbre_16k.{wav,oraculo.txt}` (creados el 25-ago, 1.6 MB, NO usados por
  ningún test): se decidió no commitearlos. No volver a añadirlos sin cablearlos a un test.
- `.claude/continuity-prompt.md` es de sesión: no commitear (gitignorado).

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» (corregir el bug del comando `doctor`) sin reabrir decisiones ya cerradas salvo nueva evidencia.
