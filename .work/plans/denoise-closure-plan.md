# Plan canónico de cierre: migración Denoise → AVI

## 1. Estado actual (árbol verificado)

- Rama: `feat/denoise-migration`, HEAD `7c4f2814`, árbol limpio
- Archivos no trackeados: `.circleci/config.yml.bak`
- Pull request: [feat/denoise-migration](https://github.com/.../pull/...) (si aplica)

### Lo que **está implementado y funciona** en Windows x64 sobre el paquete instalado

| Capacidad | Detalle |
|---|---|
| CLI única | `ai-voice-interconnector speech denoise` — ruta única pública; rechaza `denoise` raíz |
| Dispatcher | `main` → `handle_denoise` directamente (no `handle_speech`), conserva `Outcome::Verdict` + JSONL |
| FFmpeg | 9.0.2 fijado por target en `packaging/pins.json` + SHA-256 inmutable; resuelto solo desde el ejecutable distribuido; **no** existe `--ffmpeg-path`; sin FFmpeg en `PATH` el paquete funciona |
| Preset | `studio` por defecto; `--preset default` reproduce el origen; bitrate por códec (MP3/AAC 320, Vorbis 192, Opus 256); WAV/FLAC normalizados a 48 kHz |
| Video | Remux `-c:v copy` + AAC; stream de vídeo idéntico (MD5 verificado); Δ 0,023 s dentro de ±0,5 s |
| Modelo | DPDFNet en catálogo `avi-store`, SHA256 verificado al descargar/reutilizar; `setup`/`doctor`/`cleanup` operan; sin caché privada; `startup_optional=true` (daemon arranca sin él) |
| ORT | Autoridad única `avi-shared::onnx_runtime`; carga explícita antes de `Session::builder()`; resuelve desde `ort-bundle` o `program_dir()` |
| Flujos internos | `--denoise` / `denoise: true` opt-in en `voice clone`, `speech dub`, `speech transcribe` (daemon/directo/automático); sin feature → `denoise_unsupported`; modelo ausente → `model_missing`; **no** fallback silencioso |
| Gate concurrencia | 1 activa + 1 esperando ≤30 s; saturación → HTTP 503 `denoise_busy`; latidos NDJSON |
| Límites | 30 s clonación / 300 s transcripción / 40 s doblaje (rechazo `audio_too_long`, sin degradación) |

### Verificación E2E ejecutada (Windows x64 sobre paquete instalado)

- Audio WAV + vídeo MP4 reales con `studio` implícito y `--preset default`
- Colisión de destino: item `failed`, salida 2, destino intacto
- Modelo ausente: item `failed`, salida 1, sin archivo de salida
- FFmpeg externo: funciona porque el paquete resuelve su propio binario
- 3 pruebas de contrato (`denoise_non_boolean_is_usage_error`, `denoise_true_without_feature_is_unsupported`, `denoise_true_with_feature_without_model_is_model_missing`)
- DoD origen: `cargo build --release`, `cargo clippy --all-targets -- -D warnings`, `cargo test --release`, benchmark sin regresiones
- AVI: `cargo fmt --check`, `clippy -D warnings` en 5 combinaciones de features, `cargo test --release` en 5 combos

## 2. Puertas abiertas (lo que falta para cerrar)

### Puerta A — Cancelación efectiva de DSP

- **Problema**: `SIGINT` (Ctrl+C) a 1 s y 3 s: el proceso termina el trabajo, publica la salida completa y **después** devuelve 130. No interrumpe el DSP en curso.
- **Código**: `src/main.rs:1188` (`handle_denoise`) crea `cancel_flag` y lo pasa a `denoise::cli::execute(..., Some(cancel_flag))`, pero el flag no se verifica entre etapas del pipeline.
- **Próxima prueba concreta**: instrumentar el flag entre etapas (`probe → extracción → DSP → egresión → verificación`) y verificar que `SIGINT` durante el DSP deja el destino sin crear o intacto.
- **Veredicto provisional**: NO aprobado. No se puede declarar completa sin esto.

### Puerta B — Natividad de los 3 targets restantes

- **Problema**: smokes nativos de FFmpeg, paquete, instalación y CLI en Linux x64, Linux arm64 y macOS arm64 **no se ejecutaron**.
- **Código**: `.circleci/config.yml` tiene un workflow temporal `denoise-migration-validation` (agregado en `7c4f2814`) filtrado a `feat/denoise-migration*`, listo pero **sin evidencia de ejecución**.
- **Próxima prueba concreta**: empujar la rama, ejecutar los 4 jobs (sin modelo), registrar links, números de pipeline, SHA-256, sccache stats, `otool -L` en macOS, y retirar el andamiaje antes del merge.
- **Veredicto provisional**: PENDIENTE. No se puede declarar completa sin esto.

### Puerta C — Medición calidad E2E

- **Problema**: Paso 11 parcial. Se midieron latencias (3–13 s por archivo), concurrencia (1 activa + 1 esperando + 503) y memoria (+225 MB). Pero **no** se calculó WER numérico ni se midió identidad/naturalidad de la voz clonada.
- **Próxima prueba concreta**: corridas con corpus limpio/ruidoso comparado texto-a-texto; WER sobre referencia limpia; métrica de locutor (embedding/verificación) o escucha ciega para clonación.
- **Verdicto provisional**: PARCIAL. Las filas restantes quedan pendientes, no aprobadas.

## 3. Orden de ejecución del cierre

```
Puerta A (cancelación) → empujar rama → Puerta B (CI nativa) → Puerta C (calidad) → retirar CI temporal → docs finales + walkthrough + commit
```

**Detalle operativo**:

1. **Instrumentar cancelación DSP**: modificar `crates/denoise/src/pipeline.rs` para verificar `cancel_flag` entre etapas (`extract → dsp → encode → verify`). Propagar `E::ECancelled`. En `src/main.rs`, mapear código 3 a `ExitCode::Interrupted` (130) **solo si** el flag está activo. Agregar test en `crates/denoise/tests/test_audio_pipeline.rs`.

2. **Empujar rama y disparar CI**:
   - `git push origin feat/denoise-migration`
   - Navegar a CircleCI → workflow `denoise-migration-validation` → los 4 jobs
   - Registrar: pipeline numbers, SHA-256 artefactos, sccache stats, errores reales
   - macOS: `otool -L`, arquitectura, encoders/decoders/muxers
   - No usar tag ni publicar release

3. **Retirar CI temporal** (Tarea 8 del plan original):
   - Eliminar jobs/workflow temporal de `.circleci/config.yml`
   - Verificar que solo queda `build-all` y los smokes permanentes de `speech denoise`
   - Comparar diff con versión pre-Tarea 4

4. **Documentación final**:
   - Curar `[No publicado]` en `CHANGELOG.md`
   - `docs/CLI/CONTRACT.md`, `docs/CLI/commands/SPEECH.md`, `docs/DESIGN.md`, `docs/BUILD.md`, `docs/DISTRIBUTION.md`, `README.md`, `USAGE.md`
   - `Cargo.toml`: comentario `speech denoise`, feature `full`
   - **Recrear** `docs/denoise-migration-walkthrough.md` con el veredicto final

5. **Commit final** (solo con autorización explícita del usuario):
   - Mensaje conventional: `feat(denoise): instrumentar cancelación DSP y cerrar veredicto de migración`
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
