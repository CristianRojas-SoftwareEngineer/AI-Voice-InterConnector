# Plan canónico de cierre: migración Denoise → AVI

## 1. Estado actual (árbol verificado)

- Rama: `feat/denoise-migration`, HEAD `4b049342` ("unificar plan de cierre y eliminar documentación obsoleta"), árbol limpio
- `git push` NO publicado aún: `Your branch is ahead of 'origin/feat/denoise-migration' by 1 commit`
- Archivo no trackeado: `.circleci/config.yml.bak` — backup de 3068 líneas, **no** debe commitearse (es el estado previo a la refactor de `4b049342` y sirve como referencia de diff para retirar el scaffolding CI)
- Pull request: pendiente de abrir **después** de cerrar las puertas

### Lo que **está implementado y funciona**

#### CLI y dispatcher (Windows x64 sobre paquete instalado)

| Capacidad | Detalle | Estado |
|---|---|---|
| CLI única | `ai-voice-interconnector speech denoise` — ruta única pública; rechaza `denoise` raíz con salida 2 | ✅ Implementada |
| Dispatcher | `main` → `handle_denoise` (feature-gated `native-denoise`); conserva `Outcome::Verdict` + JSONL; mapeo código 3 → 130 si flag activo | ✅ Implementada |
| Preset | `studio` por defecto; `--preset default` reproduce el origen; bitrate por códec (MP3/AAC 320, Vorbis 192, Opus 256); WAV/FLAC normalizados a 48 kHz | ✅ Implementada |
| Video | Remux `-c:v copy` + AAC; stream de vídeo idéntico (MD5 verificado); Δ 0,023 s dentro de ±0,5 s | ✅ Implementada |
| FFmpeg | 9.0.2 fijado por target en `packaging/pins.json` + SHA-256 inmutable; resuelto solo desde el ejecutable distribuido; **no** existe `--ffmpeg-path`; sin FFmpeg en `PATH` el paquete funciona | ✅ Implementada |

#### Modelo y ORT

| Capacidad | Detalle | Estado |
|---|---|---|
| DPDFNet | Catálogo `avi-store`, SHA256 verificado al descargar/reutilizar; `setup`/`doctor`/`cleanup` operan; sin caché privada; `startup_optional=true` | ✅ Implementada |
| ORT | Autoridad única `avi-shared::onnx_runtime`; carga explícita antes de `Session::builder()`; resuelve desde `ort-bundle` o `program_dir()` | ✅ Implementada |

#### Flujos internos y gate de concurrencia

| Capacidad | Detalle | Estado |
|---|---|---|
| Flujos opt-in | `--denoise` / `denoise: true` en `voice clone`, `speech dub`, `speech transcribe` (daemon/directo/automático); sin feature → `denoise_unsupported`; modelo ausente → `model_missing`; **no** fallback silencioso | ✅ Implementada |
| Gate concurrencia | 1 activa + 1 esperando ≤30 s; saturación → HTTP 503 `denoise_busy`; latidos NDJSON | ✅ Implementada |
| Límites | 30 s clonación / 300 s transcripción / 40 s doblaje (rechazo `audio_too_long`, sin degradación) | ✅ Implementada |

#### Cancelación por SIGINT (Ctrl+C) — **CORREGIDO, No es Puerta A**

El plan anterior afirmaba erróneamente que la cancelación no interrumpe el DSP. La verificación entre etapas **está implementada y funciona**:

- `src/main.rs:815-822` — handler global `SIGINT` hace `flag.swap(true)` sobre `DENOISE_CANCEL` (global `OnceLock<Arc<AtomicBool>>`) antes de iniciar `clean_file`
- `crates/denoise/src/pipeline.rs:94-98` — verificación previa antes de colisión-check
- `crates/denoise/src/pipeline.rs:182-186` — verificación **después de la extracción** ffmpeg, **antes del DSP**
- `crates/denoise/src/df/mod.rs:270-274` — verificación **dentro del bucle de inferencia** DPDFNet (cada `HOP_SIZE` frames), retornando `E::ECancelled` (código 3)
- `crates/denoise/src/pipeline.rs:208-211` — verificación **después del DSP**, **antes de la egresión**
- `src/main.rs:1200` — `code == 3 && cancel_flag.load()` → `ExitCode::Interrupted` (130)

**La cancelación efectivamente detiene el DSP en curso.** La Puerta A del plan anterior era infundada: la función `denoise_wav_with_params` ya recibe `cancel_flag` (pipeline.rs:204) y el verifica dentro del bucle de frames en `df/mod.rs:270-274`.

### Verificación E2E ejecutada (Windows x64 sobre paquete instalado)

- Audio WAV + vídeo MP4 reales con `studio` implícito y `--preset default`
- Colisión de destino: item `failed`, salida 2, destino intacto
- Modelo ausente: item `failed`, salida 1, sin archivo de salida
- FFmpeg externo: funciona porque el paquete resuelve su propio binario
- 3 pruebas de contrato (`denoise_non_boolean_is_usage_error`, `denoise_true_without_feature_is_unsupported`, `denoise_true_with_feature_without_model_is_model_missing`)
- DoD origen: `cargo build --release`, `cargo clippy --all-targets -- -D warnings`, `cargo test --release`, benchmark sin regresiones
- AVI: `cargo fmt --check`, `clippy -D warnings` en 5 combinaciones de features, `cargo test --release` en 5 combos

## 2. Puertas abiertas (lo que falta para cerrar)

### Puerta B — Validación nativa de los 3 targets (CI temporal pendiente de push)

- **Estado real**: El workflow `denoise-migration-validation` y los 4 jobs (`denoise-validation-windows-x64`, `-linux-x64`, `-linux-arm64`, `-darwin-arm64`) **están presentes en `.circleci/config.yml`** (líneas 2345-2493 ajustadas), filtrados a `branches: only /feat\/denoise-migration.*/`.
- **Problema inmediato**: La rama **no ha sido empujada**. El HEAD local (`4b049342`) está 1 commit adelante de `origin/feat/denoise-migration`. CircleCI no tiene el commit, por lo que **ningún job ha corrido todavía**.
- **Root causes de CI fallida (investigados por subagente opus, 3 confirmados)**:
  1. **Linux (`deps.rs:198`)**: `GLIBC_PRIVATE` falso positivo. `parse_version("PRIVATE")` → `None` → `exceeds = true`. **Fix aplicado**: salto de `GLIBC_PRIVATE`/`GLIBC_INTERNAL_*` antes del `split_once`. Test `family_without_ceiling_is_allowed` actualizado (ahora `GLIBC_PRIVATE` no es violación). Tests pasan (12/12).
  2. **macOS (`package.rs:851`, `:885-886`)**: `ffmpeg` sin permiso ejecutable en compressed tar.gz/zip. `compress_zip`/`compress_tar_gz` omitían `ffmpeg_rel`. **Fix aplicado**: `ffmpeg_rel` añadido a ambas funciones, consistente con `stage_bundle`. Tests pasan (17/17).
  3. **Windows (`config.yml:2356/2399/2442/2492`)**: `sccache --show-stats` exit code 3 — step de reporte, no validación. **Fix aplicado**: `|| true` en los 4 jobs del workflow denoise (NO tocado los 8 jobs existentes de `main`).
- **Próxima prueba concreta**:
  1. `git push origin feat/denoise-migration` (dispara workflow `denoise-migration-validation`)
  2. Navegar a CircleCI → esperar a los 4 jobs
  3. Confirmar: `speech denoise --help` OK, `denoise` raíz rechazado, package + self install OK, SHA-256 registrado, `otool -L` (macOS) / `ldd` (Linux) limpio
  4. Registrar: pipeline numbers, SHA-256 artefactos, sccache stats, errores reales
  5. Retirar el andamiaje CI (jobs + workflow + `.bak`) — ver sección 3, paso 3
- **Veredicto provisional**: PENDIENTE. Bloqueado por el falta de push.

### Puerta C — Medición calidad E2E

- **Problema**: Se midieron latencias (3–13 s por archivo), concurrencia (1 activa + 1 esperando + 503) y memoria (+225 MB). Pero **no** se calculó WER numérico ni se midió identidad/naturalidad de la voz clonada.
- **Veredicto provisional**: PARCIAL. Requiere corpus limpio/ruidoso comparado texto-a-texto, WER sobre referencia limpia, métrica de locutor (embedding/verificación) o escucha ciega.

### Puerta D — Retiro del andamiaje CI temporal

- **Estado real**: El `.bak` (3068 líneas) contiene el estado previo a `4b049342`, incluyendo el scaffolding Denoise obsoleto. El `config.yml` actual (2699 líneas) ya tiene integrados los 4 jobs de validación + el workflow temporal, pero también contiene el diff de `cargo xtask package` con fallback para `CIRCLE_TAG` (no-DRY).
- **Problema**: El `.bak` no debe commitearse. Los 4 jobs + workflow temporal deben retirarse una vez validados.
- **Veredicto provisional**: PENDIENTE.

## 3. Orden de ejecución del cierre

```
Puerta B (push + CI nativa) → [verificación] → retirar andamiaje CI (Puerta D) → Puerta C (calidad) → docs finales + walkthrough → commit → push → PR
```

**Detalle operativo**:

1. **Empujar rama y disparar CI** (Puerta B):
   - `git push origin feat/denoise-migration`
   - Navegar a CircleCI → workflow `denoise-migration-validation` → los 4 jobs
   - Para cada job: confirmar `speech denoise --help` OK, `denoise` raíz rechazado, package + self install OK, SHA-256 registrado, `otool -L` (macOS) / `ldd` (Linux) limpio
   - Registrar: pipeline numbers, SHA-256 artefactos, sccache stats, errores reales
   - No usar tag ni publicar release

2. **Retirar CI temporal** (Puerta D):
   - Eliminar los 4 jobs `denoise-validation-*` (líneas 2268-2492) de `.circleci/config.yml`
   - Eliminar el workflow `denoise-migration-validation` (líneas 2494-2513)
   - Eliminar `.circleci/config.yml.bak` (3068 líneas, backup obsoleto)
   - Verificar que `.circleci/config.yml` vuelve a su estado previo a `7c4f2814` en la sección de jobs/workflows (diferencia contra `.bak` debe ser solo el fallback `CIRCLE_TAG` en `cargo xtask package`)
   - `git add .circleci/config.yml && git rm .circleci/config.yml.bak`

3. **Medición calidad E2E** (Puerta C):
   - Corridas con corpus limpio/ruidoso comparado texto-a-texto
   - WER sobre referencia limpia
   - Métrica de locutor (embedding/verificación) o escucha ciega para clonación

4. **Documentación final**:
   - Curar `[No publicado]` en `CHANGELOG.md`
   - Verificar `docs/CLI/CONTRACT.md`, `docs/CLI/commands/SPEECH.md`, `docs/DESIGN.md`, `docs/BUILD.md`, `docs/DISTRIBUTION.md`, `README.md`, `USAGE.md`
   - `Cargo.toml`: comentario `speech denoise`, feature `full`
   - **Recrear** `docs/denoise-migration-walkthrough.md` con el veredicto final

5. **Commit final + push + PR** (solo con autorización explícita del usuario):
   - Mensajes conventional:
     - `ci: retirar scaffolding temporal de validación Denoise` (elimina jobs + `.bak`)
     - `docs(denoise): walkthrough definitivo de migración a speech denoise`
   - `Co-Authored-By: Claude Code <noreply@anthropic.com>`

## 4. Restricciones

- **No commitear/pushear sin solicitud explícita**
- Toda documentación en español; identificadores en inglés
- Comentarios autónomos (nada de citas a secciones volátiles)
- `cargo xtask language/comments/features/licenses/source-offer/pins --check` en verde
- `cargo fmt --all --check` y `cargo clippy --all-targets -- -D warnings` en verde
- Los smokes CI no validan inferencia real — no equiparar con E2E funcional
- Sin retrocompatibilidad: no conservar `--ffmpeg-path`, `--model-dir`, alias raíz, o fallback silencioso
- No tocar `Cargo.toml` workspaces ni `.cargo/config.toml` (rompería ORT estático)
