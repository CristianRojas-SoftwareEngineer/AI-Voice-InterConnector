# Revisión: rendimiento, caché y diseño del pipeline CI (CircleCI, tag v0.10.5)

**Fecha:** 2026-08-24
**Estado:** En resolución. El pipeline Rust nativo 3 SO está funcional (macOS, Linux **y Windows**
verdes en el pipeline #10) pero **no cumplía el objetivo de tiempo**: la iteración sobre tags
exigió 5 ciclos de push para descubrir fallos básicos de entorno, y el coste por corrida era de
decenas de minutos por plataforma. El job que aún fallaba, `coverage` (timeout silencioso en el
recompile instrumentado; ver P2/P3), se ataca de raíz con R1: con `native-stt`/`native-translation`
off por defecto, test y coverage ya no compilan C++. **R1/R6/R7 quedan implementadas en este
cambio** (validación end-to-end en CI pendiente del próximo tag); ver la Instrucción de cierre.
**Autor:** Sesión interactiva de implementación CI (`bc895d7` → `386b095`).

---

## Resumen ejecutivo

El pipeline CircleCI adaptado de Python a Rust (`bc895d7`) heredó el contrato tags-only del
pipeline legacy y lo combinó con una carga de compilación que Python nunca tuvo: tres stacks
nativos C++ (whisper.cpp, CTranslate2, oneDNN) construidos desde fuente dentro de cada job.
El resultado es un pipeline cuyo coste dominante no son los tests (segundos) sino la
compilación redundante de dependencias — hasta tres veces por plataforma (debug para tests,
release para builds, instrumentada para coverage) — sobre un diseño de caché inicial que
perdía todo en cada fallo. Este documento cristaliza los problemas observados con evidencia
de los pipelines #2–#10, sus causas raíz y recomendaciones priorizadas. Principio rector:
**los timeouts largos no son un fix**; la meta es que ningún step tarde lo suficiente como
para necesitarlos.

## Contexto

- Pipeline: `.circleci/config.yml` (14 jobs; commits `bc895d7`, `d6a8d88`, `f6c6f81`,
  `55f7b72`, `386b095`).
- Tag validado: `v0.10.5`, re-apuntado 4 veces durante la iteración (force-push).
- Contrato preservado del legacy: pipeline **tags-only** (filtro `v*` propagado a los 14
  jobs); sin ejecución en pushes de rama.
- Deps nativas compiladas desde fuente por `build.rs`: `whisper-rs-sys` (whisper.cpp vía
  CMake), `ct2rs` (CTranslate2 + onednn-src vía CMake), más `bindgen` (exige libclang).

## Evidencia: cronología de pipelines

| Pipeline | Commit | Resultado | Duración clave | Causa |
|---|---|---|---|---|
| #2 | `bc895d7` | test-linux ✗, coverage ✗, test-macos ✗, test-windows >40 min (cancelado) | — | bindgen sin `libclang.so`; ct2rs «OpenMP not found» (Apple clang); Windows frío |
| #4 | `d6a8d88` | test-macos ✗ | ~3 min | Fix libomp+env fue inútil: FindOpenMP no consume `OpenMP_*` como entrada; la feature `openmp-runtime-comp` forzaba COMP |
| #6 | `f6c6f81` | test-macos ✗ (suite corrió) | ~1 min | ct2rs compiló (features por target ✓); cli_golden 20/22: exit 4 = pesos ausentes (`models/` gitignoreado) |
| #8 | `55f7b72` | test-macos ✗ | ~2.5 min | capa siguiente: 6 unit tests de avi-translation exigen opus-mt real |
| #10 | `386b095` | test-macos ✓ **1m16s**, test-linux ✓ **8m13s**, **test-windows ✓ 28m27s**, coverage ✗ (timedout **15m35s**) | — | macOS verde por caché tibio heredado de #8; Windows completó su **primera corrida verde** con caché persistido; coverage muere en el recompile instrumentado (ver P2/P3) |

Patrón estructural: cada ciclo tag→fallo→fix→force-push tardó entre ~15 y ~50 minutos de
reloj y quemó créditos de matrices solapadas (#4/#6/#8 vivos a la vez, cancelados a mano).

## Problemas identificados

### P1 — Compilación nativa dominante (causa raíz del coste)

Los tests son mock/contrato y no ejercitan motores (pesos gitignoreados), pero **compilar la
suite exige compilar todos los sys-crates** porque `whisper-rs`/`ct2rs` son dependencies duras
del workspace. Coste medido/estimado por plataforma en Debug:

| Stack | Linux x64 (2 vCPU) | macOS M4 Pro | Windows MSVC (2 vCPU) |
|---|---|---|---|
| whisper.cpp (CMake) | ~2–3 min | segundos | ~5–8 min |
| CTranslate2 + oneDNN (CMake) | ~3–5 min | ~1 min | ~20–35 min |
| Total frío suite | ~8 min (medido) | ~1–2 min tibio / ~5 min frío | ~40+ min (nunca completado) |

oneDNN solo aporta miles de translation units; MSVC-Debug + link de estáticos gigantes sobre
2 vCPU es el peor caso posible. A esto se suma que **cada push de tag compila todo tres
veces**: debug (tests), release (builds), instrumentada (coverage en `target/llvm-cov-target`
aparte).

### P2 — Diseño de caché: pérdida total en fallo y huecos residuales

- **Original defectuoso:** `save_cache` después de `cargo test` → cualquier fallo descartaba
  descargas y compilación (~100% de retrabajo). Corregido en `d6a8d88` con commands duales y
  `when: always`.
- **Cancelaciones no guardan:** cancelar un workflow mata los steps; `when: always` no llega a
  ejecutarse. Toda la iteración por cancelación dejó a Windows sin caché cinco veces seguidas.
- **`llvm-cov-target` nunca se persiste (colisión de clave + carrera de guardado):** coverage
  instrumenta en `target/llvm-cov-target`, pero comparte la clave `target-v1-{arch}-{Cargo.lock}`
  con test-linux (mismo arch/Cargo.lock). Ambos corren en paralelo; test-linux termina primero
  (~8 min) y guarda la clave. Como las claves de CircleCI son inmutables, cuando coverage ejecuta
  su `cargo_save_target` (`when: always`, ~9 min después) la clave ya existe y el guardado es un
  no-op (step «Saving cache» 128 ms) — y la versión persistida es la de test-linux, que **no**
  contiene `llvm-cov-target`. Resultado: **cada corrida recompila la workspace instrumentada
  completa desde cero**, incluidos los sys-crates nativos (`whisper-rs-sys`, `onednn-src`, `ct2rs`
  vía CMake). En #10 el step murió aún compilando, sin correr un solo test ni emitir `lcov.info`.
- **Toolchains sin cachear:** `~/.rustup` (rustup-init en Win/macOS, ~2–3 min) y
  `~/.cargo/bin` (`cargo install cargo-llvm-cov --locked`, ~2–4 min compilando desde fuente)
  se reinstalan en cada job.
- **Claves incompletas:** `cargo-v1-{arch}-{Cargo.lock}` no incluye `rust_version` ni la
  imagen base → tras un bump hay riesgo de mezclar artefactos de toolchains distintos; el
  fallback parcial `{{arch}}-` restaura estados viejos deliberadamente (OK, documentar).
- **Efecto positivo verificado:** el caché tibio heredado de un fallo llevó test-macos de
  ~3 min (frío) a **1m16s** — demuestra el tamaño del premio si el caché se diseña bien.

### P3 — Tiempo muerto por diseño, no por lentitud intrínseca

- **Timeout silencioso default (10 min):** mató al step de coverage en #10 tras ~15m35s de
  reloj — los ~10 min finales sin output son los builds CMake nativos, mudos para cargo. Coverage
  es el **único** step de compilación pesada sin `no_output_timeout` extendido (los builds de
  release ya usan 25m). Subírselo (p. ej. 30m) habría sido un parche sintomático (rechazado); la
  cura es que el step dure menos (R1/R4) o emita progreso, no tolerar más silencio.
- **Tags-only = feedback diferido al release:** fallos triviales de entorno (libclang,
  OpenMP, rutas de modelos) se descubrieron recién etiquetando. El contrato del legacy era
  razonable para pytest-rápido; para este coste de compilación es un anti-patrón.
- **Sin auto-cancel de workflows redundantes:** cada force-push del tag disparó una matriz
  nueva sin apagar la anterior; créditos y colas de executors (Windows/macOS m4pro) saturados.

### P4 — Otros (menores, ya anotados inline)

- `cimg/rust:1.96.0` sin pin por digest (TODO marcado); reproducibilidad pendiente.
- `publish-metadata` fallará en todo release hasta generar `.dmg` o migrar el Cask a binario.
- `scripts/build_release_native.py` aún no integrado a los builds de CI (entrega F7.2).

## Recomendaciones priorizadas

| # | Recomendación | Impacto esperado | Esfuerzo |
|---|---|---|---|
| R1 | **Feature-gate de engines pesados**: mover `whisper-rs` y `ct2rs` a features opcionales (p. ej. `native-stt`/`native-translation`) o deps target-específicas ya probadas; los jobs de test corren `cargo test --all` sin features → sin C++, sin bindgen, sin CMake | test jobs de ~8–40 min → **~1–2 min frío** en los 3 SO | Medio (toca Cargo.toml de avi-stt/avi-translation y cfgs) |
| R2 | **sccache** como wrapper de `rustc` con disco cacheado por arch (restaurable vía cache CircleCI) | elimina la mayor parte del re-trabajo cruzado debug/release/instrumentada y entre runs | Bajo-Medio |
| R3 | **Cachear toolchain**: `~/.rustup` + `~/.cargo/bin` con clave `rust_version+arch` (y sumar `rust_version` a las claves existentes al bump) | −4 a −6 min por job Win/macOS | Bajo |
| R4 | **Decidir cobertura**: (a) cachear `target/llvm-cov-target` **con clave propia** (`target-cov-v1-…`, no compartida con test-linux), (b) compartirla con tests aceptando reinstrumentación, o (c) diferir el job hasta definir umbral (`--fail-under-lines`). Hoy cuesta >10 min sin gate | coverage de ∞/timeout → determinista | Bajo (decisión) |
| R5 | **Gates ligeros en rama** (fmt, clippy, `cargo check/test` sin features pesadas) + matriz completa solo en tags | feedback temprano; rompe el anti-patrón de descubrir entornos al liberar | Medio |
| R6 | **Auto-cancel** de workflows obsoletos (Project Settings → Advanced → cancel redundant workflows, o API) | evita matrices solapadas quemando créditos | Trivial |
| R7 | Pin **digest de cimg/rust** y procedimiento de bump documentado (ya esbozado en parameters) | reproducibilidad | Trivial |
| R8 | Medio plazo: **binarios prebuilt/vendored** de whisper.cpp y CTranslate2 por plataforma (o crates alternativos con build precompilado) | elimina el coste C++ de raíz incluso en builds release | Alto |

Secuencia sugerida: R6+R7 (inmediato) → R1 (mayor palanca, habilita R5) → R3+R4 → R2 → R8.

## Lecciones cristalizadas

1. Con dependencias nativas pesadas, el coste del CI vive en la compilación, no en los tests;
   diseñar cachés y features alrededor de eso, no alrededor de pytest.
2. Un pipeline tags-only convierte cada fallo en un release roto: la validación de entorno
   debe ocurrir antes del tag.
3. `when: always` en cachés es necesario pero insuficiente: las cancelaciones también deben
   considerarse en el diseño (o evitarse mediante auto-cancel ordenado).
4. Los timeouts generosos ocultan problemas de diseño; si un step necesita >10 min de
   silencio, el problema es el step.

---

Instrucción de cierre: **R1/R6/R7 implementadas** (este cambio). R1: `whisper-rs`/`ct2rs`
opcionales tras `native-stt`/`native-translation` (off por defecto) → test y coverage compilan la
workspace sin C++; los 4 `build-*` activan `--features full` para no distribuir un binario sin
motores. R6: auto-cancel de workflows redundantes activado en el panel del proyecto. R7: digest de
`cimg/rust` pinneado en sus 4 referencias y `rust_version` añadido a las claves de caché. Validación
end-to-end en CI pendiente del próximo tag. Pendientes: R4 (clave propia para
`target/llvm-cov-target`), R2/R3/R5/R8.

**Actualización #10:** test-windows completó su primera corrida verde (28m27s) con caché
persistido; la palanca crítica era `coverage`, ahora atacada de raíz por R1.
