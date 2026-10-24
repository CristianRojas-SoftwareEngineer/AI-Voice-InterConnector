# Fase 1 — Trasplante del crate a AVI: adaptaciones y evidencia

Artefacto del paso 4–5 de `.work/plans/denoise-implementation.md` §5 («Registrar
cualquier adaptación necesaria por estructura workspace»). Rama AVI:
`feat/denoise-migration`. Fecha: 2026-10-24. Origen: `Denoise/` (`main` @
`2d50aa7`, sin modificar). Destino: `crates/denoise/` (19 MB).

## 1. Qué se copió y qué no

| Elemento | Copiado a | Motivo |
|---|---|---|
| `src/` completo (8 ficheros + `df/` con 4) | `crates/denoise/src/` | El plan exige el crate completo; `src/lib.rs` expone todos los módulos |
| `tests/` (8 suites + `common/`) | `crates/denoise/tests/` | Paridad de comportamiento y regresión |
| `tests/data/` — solo el par 3 s, el par 65 s y las 2 referencias congeladas + `README.md` | `crates/denoise/tests/data/` | Son los únicos que usan las suites de Rust (`test_golden`, `test_audio_pipeline`) |
| `examples/gen_vectors.rs`, `examples/process_wav.rs` | `crates/denoise/examples/` | Generador de vectores y harness de proceso |
| `LICENSE` (MIT) | `crates/denoise/LICENSE` | Atribución MIT exigida por el plan §5 paso 4 |
| `Cargo.toml` | `crates/denoise/Cargo.toml` | Manifiesto adaptado; ver §2 |

No copiado, deliberadamente:

- `tools/quality/` (harness, `clips.json`, `baseline_{perfil}.json`): sigue siendo
  la autoridad del origen. No se duplican números (plan §5 paso 4). Su referencia
  se mantiene al origen `Denoise/tools/quality/`.
- Los 10 WAV de escena (`car_`, `office_`, `pub_`, `restaurant_`, `train_`) y
  `voz65s_*` de benchmark: solo los consume el benchmark, no las suites de Rust.
- `README.md`, `docs/`, `CHANGELOG.md`, `assets/e2e_vertical_1080x1920_16s.mp4`
  (42 MB, fixture manual E2E): duplicarían autoridad documental; el origen es la
  fuente de verdad mientras dure la migración. El manifiesto trasplantado por eso
  **no** declara el campo `readme`.
- `.git`, `target/`, `out/`, DLLs, artefactos: exigido por el plan §5 paso 4.

El aviso Apache-2.0 del modelo DPDFNet ya viaja en `src/models.rs:6`, copiado tal
cual.

## 2. Adaptaciones aplicadas (y su causa)

| # | Adaptación | Causa verificada | Dónde |
|---|---|---|---|
| 1 | Features de `ort`: `std` + `copy-dylibs` + `download-binaries` + `tls-rustls` + `lax-feature-matching` → **`std` + `load-dynamic` + `preload-dylibs`** | El origen enlaza ORT **estático** (`cargo:rustc-link-lib=static=onnxruntime` en `Denoise/target/release/build/ort-sys-*/output`). AVI compila Windows con `+crt-static` por CTranslate2 (`.cargo/config.toml`), y el ORT estático exige CRT dinámico: el primer intento de enlace falló con **71 `LNK2001/LNK2019` de símbolos `__imp_*`** (`strtoul`, `wfopen`, `frexp`, …) y `LNK1120`. Se usa la configuración de ORT del workspace; `cargo tree -e features -i ort -p denoise` confirma que no queda ninguna feature del origen | `crates/denoise/Cargo.toml` |
| 2 | `[lints.clippy] disallowed_methods = "allow"` en el crate | `clippy.toml` del workspace prohíbe `std::process::exit` (13 usos en `cli.rs`, la CLI de origen termina con `exit` y su handler `Ctrl+C` forzado) y `std::env::temp_dir` (usado por los tests trasplantados). El plan (§4.1 y paso 9) factoriza la salida y conecta la cancelación al mecanismo de AVI en la **Fase 3**; hasta entonces el crate conserva el comportamiento del origen. Exención local al crate, no al workspace. **Retirar al cerrar la Fase 3** | `crates/denoise/Cargo.toml` |
| 3 | Eliminado `test_repo_design_doc_exists` (`tests/test_naming.rs`) | Verificaba `docs/design.md` del repositorio de origen, que no se trasplanta para no duplicar autoridad documental. Su hermano `test_cargo_toml_exists` (que comprueba que el cwd de los tests es la raíz del crate) se conserva y pasa. Con esto el inventario vuelve a coincidir con el del origen: los **6 `#[ignore]`** son exactamente los mismos | `crates/denoise/tests/test_naming.rs` |
| 4 | Renombrados los identificadores con el segmento `media` (59 apariciones en 9 ficheros): `MediaKind` → `StreamKind`, `media_kind` → `stream_kind`, `is_supported_media` → `is_supported_input`, `test_is_supported_media` → `test_is_supported_input`, `clean_media` → `clean_file`, `egress_media` → `egress_file` | El gancho pre-commit ejecuta `cargo xtask language --check`, que exige que ningún identificador de primera parte lleve una palabra castellana de la lista `ES_WORDS` (que incluye `media`); la convención de AVI evita ese segmento. Cambio de superficie, sin tocar lógica. **El nombre del pipeline pasa a `clean_file`** (el plan lo cita como `clean_media`): queda registrado aquí para la matriz del walkthrough | en `src/{cli,ffmpeg_io,lib,pipeline}.rs` y 5 suites de test |
| 5 | Reescritas 29 citas de sección en 21 ficheros (cabeceras `//! Contrato: docs/design.md §…`, `//! Ver specifications.md §…`, `// Zero-mean (docs/specifications.md §6)`, y los dos comentarios propios de `crates/denoise/Cargo.toml`) | La otra puerta del mismo gancho, `cargo xtask comments --check`, prohíbe citas de sección y localizadores de línea: el comentario debe enunciar la regla que aplica, no remitir a un documento. Las citas que apuntaban al origen se sustituyen por la regla autónoma o se retiran cuando la cabecera ya describía el módulo | `src/`, `tests/`, `examples/`, `Cargo.toml` |

## 3. Verificación ejecutada

Entorno: Windows x64, ffmpeg 9.0.1 (PATH), modelo
`~/.cache/denoise/models/dpdfnet8_48khz_hr.onnx`, ORT cargada desde
`ort-bundle/onnxruntime.dll` (1.28.0, bundle provisionado por `xtask bootstrap`).

| Comando | Resultado |
|---|---|
| `cargo build -p denoise` | ✓ |
| `cargo build --release -p denoise` | ✓ (1 m 23 s) |
| `cargo clippy -p denoise --all-targets -- -D warnings` | ✓ 0 avisos |
| `cargo test -p denoise` | ✓ **56 passed / 0 failed / 6 ignored** |
| `cargo test --release -p denoise -- --ignored` (con `ORT_DYLIB_PATH` al `.dll`) | ✓ **6 passed / 0 failed**: `test_golden_3s`, `test_golden_65s` (42 s), `test_audio_wav_pipeline_e2e`, `test_remux_video_copy`, `test_remux_duracion_contenedor`, `test_remux_mov_mp4_editlist_sync` |
| `cargo check --workspace` | ✓ el resto del workspace sigue compilando tras añadir la miembro |
| `cargo xtask language --check` / `comments --check` / `pins --check` / `features --check` | ✓ 0 identificadores en español, 0 citas de sección ni localizadores, pines en sincronía, cadena de features coherente (las cuatro puertas del gancho `.githooks/pre-commit`) |
| `cargo tree -e features -i ort -p ai-voice-interconnector --features native-stt` | ✓ sigue resolviendo `load-dynamic`/`preload-dylibs`/`std`, sin segunda autoridad ORT |

Conteo: el origen documenta 57 passed / 6 ignored; aquí hay 56 passed / 6
ignored. La diferencia unitaria es el test estructural eliminado (§2 #3), no una
prueba de comportamiento perdida. Las cifras de esta tabla se **re-midieron
tras los renombrados y reescrituras de comentarios de §2 #4 y #5**: mismas
resultados (56/6/0 en debug y 6/6 ignorados en release), es decir, los cambios
de superficie no alteraron comportamiento. Los 6 ignorados **se ejecutaron y pasaron** con
modelo y ffmpeg; siguen marcados como `#[ignore]` por el mismo motivo de origen
(requieren modelo y ffmpeg), no por fallo.

## 4. Diferencial contra el origen (mismo testigo, mismo modelo)

Origen `Denoise/target/release/denoise.exe` vs trasplantado
`target/debug/denoise.exe`, ambos sobre `tests/data/voz_noisy.wav` (3,000 s,
PCM16 48 kHz mono):

| Perfil | Salida | Tamaño | Códec / tasa | Canales | Duración | LUFS integrado | True peak |
|---|---|---|---|---|---|---|---|
| `--preset studio` | trasplantado | 1.152.102 B | pcm_s16le 192 kHz | 1 | 3,000 s | −16,18 | −1,50 dBTP |
| `--preset studio` | origen | 1.152.102 B | pcm_s16le 192 kHz | 1 | 3,000 s | −16,18 | −1,50 dBTP |
| default | trasplantado | 288.078 B | pcm_s16le 48 kHz | 1 | 3,000 s | −24,79 | −8,62 dBTP |
| default | origen | 288.078 B | pcm_s16le 48 kHz | 1 | 3,000 s | −24,79 | −8,62 dBTP |

Los tamaños reproducen exactamente los del registro de Fase 0
(`denoise-fase0-matrix.md` §6). El PCM decodificado difiere en ≤3 LSB
(SI-SDR entre salidas: **100,85 dB** studio y **101,94 dB** default, RMS de la
diferencia 0,046 / 0,015 sobre un rango de ±32768): atribuible a que el origen
enlaza estáticamente otro build del runtime ONNX y el trasplantado carga el
1.28.0 del bundle. Igual que con los builds de FFmpeg, la paridad se juzga por
propiedades y métricas, nunca por hash byte a byte.

También se reprodujo el defecto conocido del origen registrado en Fase 0
(`studio` sobre WAV → 192 kHz): el trasplante lo conserva sin tocar, porque su
corrección (`-ar 48000`) pertenece a la Fase 3.

## 5. Desviación abierta (no bloqueante, absorbida por la Fase 2)

El trasplante todavía **no** inicializa ORT de forma explícita: `DpdfNetSession::load`
llega a `Session::builder()` y deja que `ort` cargue por su cuenta. Consecuencia
verificada: `.cargo/config.toml` de AVI define `ORT_DYLIB_PATH = "ort-bundle"`
(directorio, que `ensure_onnx_runtime()` resuelve a archivo), y `ort` con
`load-dynamic` espera la ruta del **archivo**; con el directorio la inferencia
aborda con `Failed to load ONNX Runtime dylib: LoadLibraryExW { source: 126 },
path: "ort-bundle"` y el proceso muere sin producir salida.

- Evidencia a favor del trasplante: con `ORT_DYLIB_PATH=<repo>/ort-bundle/onnxruntime.dll`
  todo funciona (§3 y §4). `doctor` y el daemon de AVI ya usan esa resolución vía
  `ensure_onnx_runtime()`.
- Por qué no se resolvió aquí: la única autoridad correcta es la compartida
  (`avi_shared::onnx_runtime`, plan §5 **paso 6**), y este tramo no duplica
  resolvedores de ORT para luego borrarlos.
- Efecto: hasta la Fase 2, cualquier corrida manual del binario trasplantado
  requiere que `ORT_DYLIB_PATH` nombre el `.dll`, no el directorio.

## 6. Puerta de la Fase 1

**Cubierta:** comportamiento inicial preservado — todas las suites pasan, los 6
tests ignorados se ejecutan y pasan, y el diferencial contra el origen coincide
en tamaño, tasa, canales, duración, LUFS y true peak, con ≤3 LSB de diferencia
numérica atribuible al build del runtime ONNX. La única desviación de
comportamiento es la de §5, documentada y acotada a la inicialización de ORT, que
la Fase 2 cierra en el paso que ya tiene asignado.

**No verificado en este tramo:** ejecución del binario instalado desde un bundle
(`cargo xtask package`), que es alcance de la Fase 3, y corrección en Linux/macOS,
que requiere entorno nativo.
