# Revisión: parches vs. causa raíz en el pipeline CircleCI

- **Fecha**: 2026-08-27
- **Alcance**: cambios de CI/empaquetado en `.circleci/config.yml` (paridad TTS, caché `xtask`, limpieza de `lint`/`branch-checks`).
- **Base**: rama `main` @ `775eb7d`.
- **Naturaleza**: análisis crítico posterior a la implementación. Distingue qué correcciones atacan la causa raíz y cuáles tratan síntomas.

## Tabla de contenidos

- 1. Resumen
- 2. Correcciones de raíz (sin objeción)
- 3. Hallazgos: síntoma vs. raíz
  - 3.1. `-march=native` en ARM64 (gravedad alta)
  - 3.2. `cargo_save_registry` sin `cargo fetch` en jobs `xtask` (gravedad media)
  - 3.3. Build del motor triplicado en el YAML (gravedad media, alcance mayor)
  - 3.4. `--self-test` no valida el TTS end-to-end (menor)
  - 3.5. CI sin `fmt`/`clippy` tras eliminar `lint` (menor, decisión consciente)
- 4. Recomendación

## 1. Resumen

De los tres cambios implementados, la limpieza de coherencia (`lint`/`branch-checks`) ataca la causa raíz. La existencia del binario TTS en Unix también queda bien resuelta. Sin embargo, quedan **tres puntos que tratan el síntoma en lugar de la raíz** (uno de alcance inmediato y barato, dos de alcance mayor) más dos **decisiones conscientes y cerradas** (3.4 y 3.5): no son descuidos, sino resoluciones justificadas y documentadas como tales.

> **Actualización (2026-08-28)**: los **tres** hallazgos de raíz quedaron **resueltos**. El **3.2** (alcance inmediato) con el enfoque D — ver §3.2. Los **3.1** y **3.3** con el "Camino B" (`xtask build-engine`, commit `13106b7`): interfaz uniforme de compilación del motor en las 4 plataformas que colapsa la triplicación Unix, hace compilar Windows desde fuente (retira el blob de 33 MB, purgado del historial) y fija el baseline ARM portable — ver §3.1 y §3.3. No queda deuda de raíz abierta.

## 2. Correcciones de raíz (sin objeción)

- **Coherencia (`lint`/`branch-checks`)**: eliminar el job muerto y las menciones a un workflow inexistente corrige la causa real —la deriva entre configuración y documentación embebida—. No es parche.
- **Existencia del binario TTS**: compilar y empaquetar `qwen_tts` bajo `vendor/qwen3-tts/qwen_tts` en los 3 builds Unix resuelve de raíz el "archivo ausente" que rompía el TTS en releases Linux/macOS. La *existencia* del binario está bien atacada.

## 3. Hallazgos: síntoma vs. raíz

### 3.1. `-march=native` en ARM64 (gravedad alta)

- **Síntoma tratado**: falta `vendor/qwen3-tts/qwen_tts` en el release → se compila y el `--self-test` pasa en verde en el runner.
- **Raíz sin tocar**: el `Makefile` de `qwen_tts` fija `-march=native` para ARM (Linux y macOS). El binario queda acoplado a la microarquitectura del runner de CI. En **Linux ARM64** puede provocar `SIGILL` en CPUs de campo más antiguas. Además, el `--self-test` corre en la **misma CPU que compiló**, por lo que por construcción *nunca puede detectar* este problema: el verde da confianza falsa.
- **Contraste**: en x86 el `Makefile` usa el baseline portable `-mavx2 -mfma` (Haswell 2013+), justamente para no acoplar el binario al host. En ARM no hay equivalente.
- **Raíz real**: baseline SIMD portable para ARM, pasando `SIMD=<portable>` al `make` del job ARM (la variable `SIMD` ya existe en el `Makefile`).
- **Estado**: ✅ **RESUELTO (2026-08-28, Camino B)**. La rama ARM (aarch64 no-Darwin) del `Makefile` ahora **honra `SIMD`** como ya hacía x86, con **default portable `-march=armv8-a`** (NEON es baseline obligatorio de ARMv8-A); `-march=native` queda solo bajo `SIMD=native` explícito. Con esto Linux ARM64 deja de emitir instrucciones ausentes en CPUs de campo más antiguas (fin del riesgo `SIGILL`). macOS conserva `-march=native` (rama Darwin intacta, single-vendor). Cambio quirúrgico limitado a la rama ARM no-Darwin. La política SIMD sigue centralizada en el `Makefile`; `xtask build-engine` solo pasa `SIMD=auto` por defecto.

### 3.2. `cargo_save_registry` sin `cargo fetch` en jobs `xtask` (gravedad media)

- **Contexto**: los jobs `validate-licenses`, `validate-changelog` y `publish-metadata` **no** tienen step de `cargo fetch --locked` (a diferencia de los 8 jobs de test/build, que sí). La caché de `xtask` se añadió colocando `cargo_save_registry` inmediatamente después del `restore`, **antes** del `cargo run` que descarga las dependencias.
- **Qué pasa**: en la primera corrida con un `Cargo.lock` nuevo, el `restore` cae al fallback por prefijo (registry de un release anterior) y lo **re-guarda bajo la clave nueva sin las crates nuevas**.
- **Por qué importa**: la clave de registry (`cargo-v1-…-{checksum Cargo.lock}`) es compartida por todos los jobs y es *first-write-wins*. Si uno de estos jobs gana la carrera de escritura, deja un registry incompleto que los demás restauran. No rompe (cada job hace su `cargo fetch` y recompleta), pero **erosiona el beneficio de caché que la tarea buscaba**.
- **Raíz real**: guardar el registry *después* de poblarlo — mover `cargo_save_registry` a después del primer `cargo run`, o añadir un `cargo fetch --locked` antes de guardarlo, como en los jobs de test.
- **Estado**: ✅ **RESUELTO (2026-08-28, enfoque D)**. Se optó por una tercera vía más quirúrgica que las dos anteriores: **eliminar** `cargo_save_registry` de los 3 jobs xtask (`validate-licenses`, `validate-changelog`, `publish-metadata`), dejándolos como *consumidores puros* del registry compartido. El registry completo ya lo pueblan los 8 jobs test/build (patrón `restore → cargo fetch --locked → save`), y el único caché propio de xtask (`target/` con `variant: xtask`) queda intacto. Al no escribir, los jobs xtask no pueden envenenar la clave compartida (`save_cache` es first-write-wins). Diff mínimo (−3 líneas), sin añadir coste de red a los jobs-gate. Descartadas: mover el save tras `cargo run` (guardaría un subconjunto xtask incompleto) y clave de registry propia por `variant` (sobredimensionada, contradice el diseño de registry sin variante). Validado con `circleci config validate`.

### 3.3. Build del motor triplicado en el YAML (gravedad media, alcance mayor)

- **Síntoma tratado**: faltaba el motor en 3 releases → se replicó el mismo bloque `make blas` + `--self-test` + copia al staging **tres veces** en el config.
- **Raíz**: el motor `qwen_tts` no forma parte del grafo de compilación del proyecto; es un subproyecto C vendorizado invocado a mano. No existe automatización de su build fuera de `.circleci` (ni en `xtask/` ni en `scripts/`). Además, Windows usa un **binario precompilado versionado en git** (`vendor/qwen3-tts/qwen_tts.exe`), con provenance opaca, mientras que Unix lo compila fresco: dos mecanismos distintos para el mismo fin.
- **Raíz real**: centralizar el build del motor (p. ej. `xtask build-engine` o un `build.rs`) que CI, la documentación y el dev local reutilicen. Elimina la triplicación y la asimetría Windows-blob / Unix-compile.
- **Estado**: ✅ **RESUELTO (2026-08-28, Camino B, commit `13106b7`)**. Se creó el subcomando `cargo run -p xtask -- build-engine --self-test` como **interfaz uniforme** usada por los 4 jobs de build; `xtask` encapsula el mecanismo por plataforma (Unix: `make`; Windows: `mingw32-make` con entorno UCRT64 augmentado —PATH + `MSYSTEM`, raíz `MSYS2_ROOT`—). Con ello: (1) los **tres bloques Unix se colapsan** en un único step idéntico; (2) **Windows compila el motor fresco desde fuente** —se retira el blob `vendor/qwen3-tts/qwen_tts.exe` (33 MB), se deja de versionar (`.gitignore`/`.gitattributes`) y se **purga del historial** con `git filter-repo` + force-push a `origin/main`—; (3) `xtask` pasa a ser la fuente única reutilizable por CI, dev local y `docs/BUILD.md §9`. Bootstrap MSYS2 en Windows **pineado + cacheado** (release-base `2026-06-11`, assert `gcc 16.1.0`, evidencia RLE vía `pacman -Q`/`gcc --version`); su validación end-to-end requiere un dry-run del pipeline. Archivos: `crates/xtask/src/main.rs`, `.circleci/config.yml`, `.gitignore`, `.gitattributes`, `docs/BUILD.md`.

### 3.4. `--self-test` como oráculo de compilación, no de aceptación (menor — decisión consciente y cerrada)

**Qué hace**: `--self-test` prueba la **corrección numérica de los kernels** del motor (`matvec` vs. una referencia f32), **sin cargar pesos del modelo**. Es un oráculo de *compilación*: verifica que la maquinaria de cálculo compiló bien y produce los números esperados en la ISA actual.

**Evidencia** (`vendor/qwen3-tts/main.c`):

- `:949` — `--self-test: kernel numeric self-test (matvec vs f32 ref)`.
- `:1249-1252` — comentario del motor: *"prove the dispatched matvec kernels are numerically correct vs an f32 reference (**no model needed**). This is the **cross-ISA correctness gate** for the AVX-512/VNNI paths…"*.
- `:1253-1255` — con `--self-test` ejecuta `qwen_kernel_selftest(stdout)` y sale; nunca carga el modelo.

**La limitación**: `"smoke verde" ≠ "el TTS funciona para el usuario"`. Un verde garantiza que el binario compiló y sus kernels son numéricamente correctos, pero **no** ejercita una síntesis real texto→audio (carga de pesos, pipeline completo, integración). Un `--self-test` verde puede coexistir con un TTS roto para el usuario.

**Matiz respecto a 3.1**: el self-test sí es, por diseño, un gate de *corrección numérica cross-ISA* (detecta si una ruta SIMD **calcula mal**). Lo que NO puede detectar es el problema de 3.1, de naturaleza distinta: `-march=native` en ARM64 genera instrucciones que **no existen** en CPUs de campo más antiguas → `SIGILL` antes de calcular nada. Como corre en la **misma CPU que compiló** (que sí soporta esas instrucciones), pasa en verde por construcción. Cubre *"el kernel da números correctos en esta ISA"*, no *"el binario arranca en una microarquitectura más vieja"* ni *"el pipeline produce voz utilizable"*.

**Estado**: ✅ **decisión consciente y cerrada**. Se acepta `--self-test` como oráculo de compilación y **no** se añade un test de aceptación end-to-end en CI. Justificación: un e2e real exigiría versionar/descargar los pesos del modelo (peso considerable) y validar audio sintetizado en cada corrida, coste desproporcionado para un *build gate*; el self-test cumple bien su rol acotado —corrección numérica de kernels cross-ISA sin pesos— y la síntesis real se valida en el proceso de release y en el uso. No es un descuido: es el alcance deliberado del gate, documentado aquí para dejar explícito qué garantiza y qué no.

### 3.5. CI sin `fmt`/`clippy` tras eliminar `lint` (menor — decisión consciente y cerrada)

**Contexto**: el job `lint` estaba **huérfano** (referenciaba un workflow inexistente); eliminarlo fue parte de la limpieza de §2.

**Qué se pierde**: `cargo fmt` verifica el formato estándar de Rust y `cargo clippy` es el linter oficial (antipatrones, bugs potenciales). Tras borrar `lint`, **no queda ninguno** de los dos en CI.

**Evidencia** (`.circleci/config.yml`):

- `grep` de `clippy`/`cargo fmt`/`rustfmt` → solo 2 coincidencias, **ambas en comentarios** (`:4` y `:1436`); cero steps ejecutables.
- `grep` de `lint` → **cero coincidencias** (job eliminado).
- `:4` y `:1436` documentan que *"la validación previa al tag (fmt/clippy/test) se documenta en `docs/BUILD.md`"* — responsabilidad manual del desarrollador, no puerta de CI.

**Justificación**: borrar el job huérfano fue **correcto** —código muerto + diseño release-only-por-tag intencional—. La ausencia de `fmt`/`clippy` en CI es **coherente con ese diseño**: el pipeline no tiene gates de rama por decisión explícita (comentarios `:1-4`), y la validación de formato/lint se delega al desarrollador antes del tag, documentada en `docs/BUILD.md`. No es un descuido ni un parche encubierto: no se oculta ningún problema, se acepta deliberadamente que no hay puerta rápida de rama porque el modelo de CI no la contempla.

**Estado**: ✅ **decisión consciente y cerrada**. Reintroducir un gate `fmt`/`clippy` en cada push es una posibilidad futura *opcional* (no una obligación pendiente): solo tendría sentido si se decide cambiar el modelo de CI para incluir validación en rama. Mientras el diseño siga siendo release-only-por-tag, la política actual es la correcta y queda cerrada.

## 4. Recomendación

| # | Hallazgo | Gravedad | Alcance | Acción sugerida |
|---|----------|----------|---------|-----------------|
| 3.1 | `-march=native` en ARM64 | Alta | Política SIMD del vendor | ✅ Resuelto (2026-08-28, Camino B): rama ARM honra `SIMD`, default `-march=armv8-a` portable |
| 3.2 | `save_registry` sin `fetch` | Media | Inmediato | ✅ Resuelto (2026-08-28): eliminado el save en los 3 jobs xtask (consumidores puros) |
| 3.3 | Build del motor triplicado | Media | Arquitectura de build | ✅ Resuelto (2026-08-28, Camino B): centralizado en `xtask build-engine`; blob Windows retirado y purgado |
| 3.4 | `--self-test` no e2e | Menor | Validación | ✅ Cerrada: aceptado como oráculo de compilación; e2e en CI es desproporcionado (§3.4) |
| 3.5 | Sin `fmt`/`clippy` en CI | Menor | Política de CI | ✅ Cerrada: coherente con el diseño release-only-por-tag; validación pre-tag en `docs/BUILD.md` (§3.5) |

El hallazgo **3.2** —único arreglo barato, seguro y dentro del alcance de este trabajo— quedó **resuelto** el 2026-08-28 (enfoque D, ver §3.2). Los hallazgos **3.1** y **3.3**, registrados en su momento como deuda por tocar la política SIMD del vendor y la arquitectura de build, quedaron también **resueltos** el 2026-08-28 con el "Camino B" (`xtask build-engine`, commit `13106b7`; ver §3.1 y §3.3): baseline ARM portable e interfaz uniforme de compilación en las 4 plataformas con el blob Windows retirado y purgado del historial. **No queda deuda de raíz abierta** (3.4 y 3.5 son decisiones conscientes cerradas). Pendiente operativo: validar el bootstrap MSYS2 de Windows con un dry-run del pipeline.
