# Fase 2 — Homologación de autoridades: evidencia del paso 6 (ORT compartido)

Artefacto del paso 6 de `.work/plans/denoise-implementation.md` §5. Rama AVI:
`feat/denoise-migration` (HEAD `f549dc40` + cambios sin commitear). Fecha:
2026-10-24. El criterio de este tramo es que ONNX Runtime tenga **una sola
autoridad de carga** en el workspace y que la resolución en modo directorio
funcione sin que `ort` aborte el proceso.

## 1. Qué se movió

| Elemento | Origen | Destino |
|---|---|---|
| Resolución y carga de ONNX Runtime (`onnx_runtime_path`, `resolve_ort_path`, `ensure_onnx_runtime`, `load_onnx_runtime`, `OnnxRuntimeUnavailable`) | `crates/avi-stt/src/parakeet.rs` | `crates/avi-shared/src/onnx_runtime.rs` |
| Arranque del motor STT | `ParakeetEngine::new` con la copia local | llama a `avi_shared::onnx_runtime::ensure_onnx_runtime()?` antes de la primera sesión |
| Carga de la sesión DPDFNet | `ort` resolvía la librería por su cuenta | `DpdfNetSession::load` llama al módulo compartido **antes** de `Session::builder()` |
| Chequeo `onnx_runtime` de `doctor` | `avi_stt::parakeet::ensure_onnx_runtime` | `avi_shared::onnx_runtime::ensure_onnx_runtime` |
| Error de carga del crate migrado | no existía variante propia | `E::EOnnxRuntimeUnavailable(String)`, código de salida 1, misma familia que modelo/ffmpeg/E/S |

## 2. Adaptaciones aplicadas (y su causa)

| # | Adaptación | Causa verificada | Dónde |
|---|---|---|---|
| 1 | `avi-shared` gana la dependencia **opcional** `ort` con `default-features = false` y `features = ["load-dynamic"]`, expuesta por la feature `native-ort` | El micro-crate debe seguir siendo puro por defecto: `xtask` depende de él y no puede arrastrar ONNX Runtime. `load-dynamic` no enlaza nada en build-time, así que solo activa la carga en ejecución | `crates/avi-shared/Cargo.toml`, `crates/avi-shared/src/lib.rs` |
| 2 | `avi-stt/native-stt` activa `avi-shared/native-ort` | El motor STT no debe depender de una copia local del resolvedor; la feature del crate encadena la del módulo compartido | `crates/avi-stt/Cargo.toml` |
| 3 | `denoise` depende de `avi-shared` con `features = ["native-ort"]` | `denoise` no puede depender de `avi-stt` (feature `native-stt` y acoplamiento inverso); necesita el módulo compartido para cargar ORT antes de su sesión | `crates/denoise/Cargo.toml` |
| 4 | `native-stt` del paquete raíz activa explícitamente `avi-shared/native-ort` | El chequeo `onnx_runtime` de `doctor` (`src/main.rs`) usa el módulo directamente; la feature raíz declara la capacidad que ese código consume en vez de confiar en una activación transitiva | `Cargo.toml` |
| 5 | Variante `E::EOnnxRuntimeUnavailable` en el crate migrado | El fallo de carga debe ser un error tipado del dominio, no un pánico ni un mensaje de `ort`; se mapea desde el error compartido conservando la ruta buscada | `crates/denoise/src/errors.rs`, `crates/denoise/src/df/net.rs` |

El resto del trasplante no se toca: no se cambian features de `ort` ajenas a
`avi-shared`, no se elimina `preload-dylibs` (ya lo arrastra `load-dynamic`), no
se añade DLL privada ni búsqueda en `PATH`.

## 3. Verificación ejecutada

Entorno: Windows x64; modelo `~/.cache/denoise/models/dpdfnet8_48khz_hr.onnx`;
FFmpeg 9.0.1 en `PATH`; `ort-bundle/onnxruntime.dll` 1.28.0. La entrada
`ORT_DYLIB_PATH = "ort-bundle"` de `.cargo/config.toml` entrega a los procesos
lanzados por `cargo` la variable en **modo directorio**, que es justo el caso que
antes hacía abortar el trasplante.

| Comando | Resultado |
|---|---|
| `cargo check --workspace` | ✓ |
| `cargo check --workspace --features native-stt` | ✓ |
| `cargo check --workspace --features full` | ✓ (2 m 24 s, compilación en frío) |
| `cargo clippy --workspace --features full --all-targets -- -D warnings` | ✓ 0 avisos |
| `cargo test -p avi-shared` | ✓ 41 passed (sin la feature, el módulo no se compila) |
| `cargo test -p avi-shared --features native-ort` | ✓ **44 passed** (incluye los 3 tests nuevos del módulo) |
| `cargo test -p denoise` | ✓ **56 passed / 6 ignored / 0 failed** |
| `cargo test --release -p denoise -- --ignored` (con `ORT_DYLIB_PATH=ort-bundle`, directorio) | ✓ **6 passed**: golden 3 s y 65 s (50,35 s), E2E WAV, remux×3 |
| `target\release\denoise.exe … ` con `ORT_DYLIB_PATH=ort-bundle` (directorio) | ✓ salida de 288.078 B en 2,7 s, idéntica a la referencia `default` de Fase 1; código 0 |
| `target\release\denoise.exe …` **sin** `ORT_DYLIB_PATH` | ✓ error explícito: `no se pudo cargar ONNX Runtime desde …\target\release\onnxruntime.dll: el fichero no existe`; código **1**; sin fallback ni aborto |
| `cargo xtask language/comments/pins/features --check` | ✓ 0 hallazgos en las cuatro puertas del pre-commit |
| `cargo run --features native-stt -- doctor --json` | ✓ chequeo `onnx_runtime`: `ok: true` |
| El mismo `doctor --json` con `ORT_DYLIB_PATH` apuntando a un fichero inexistente | ✓ `onnx_runtime`: `ok: false`, `detail`: `no se pudo cargar ONNX Runtime desde target\no-existe\onnxruntime.dll: el fichero no existe`; el chequeo aparece en `failed` y el comando sale con código 1 |
| `cargo xtask doctor` | ✓ entorno listo; `onnx` resuelve `ort-bundle 1.28.0` (checks opcionales de `sccache` y tamaño de `target` no afectan) |
| `daemon serve` con `AVI_DAEMON_PORT=0`, `--ready-file`, `ORT_DYLIB_PATH=ort-bundle` y `AVI_ALLOW_CWD_VENDOR=1` | ✓ `/health`: `status: ready`, `stt: warm`, `warm: warm`; `POST /shutdown` → `shutting_down` y el proceso termina |

## 4. Incidente del arranque del daemon (no es regresión)

El primer arranque se hizo desde la raíz del repositorio **sin**
`AVI_ALLOW_CWD_VENDOR=1`. Resultado: `stt: warm` y `warm: warm_failed` con
`Warmup TTS falló: El binario Qwen3-TTS no está provisionado`.

Causa: la capa `<cwd>/vendor/qwen3-tts` es opt-in por diseño
(`crates/avi-tts/src/lib.rs:292-330`; `docs/BUILD.md:280-300`). Sin la variable,
el binario del directorio de trabajo se ignora y se avisa una sola vez; el motor
STT no depende de esa capa y por eso quedó `warm`. Con la variable documentada,
el segundo arranque alcanzó `warm: warm` en 8 s. No se toca el guard: habilitar
`vendor/` automáticamente debilitaría la protección contra un `vendor/` ajeno en
el CWD.

Conclusión operativa: al verificar el daemon en desarrollo hay que exportar
`AVI_ALLOW_CWD_VENDOR=1`; `daemon serve` publica la dirección en el ready file
antes de terminar el warmup (`warm=warming`), de modo que la comprobación debe
sondear `/health` hasta que `warm` deje de ser `warming`, y cerrar con
`POST /shutdown` en el mismo tramo.

## 5. Puerta del paso 6

**Cubierta:** ONNX Runtime se inicializa una sola vez por proceso desde
`avi_shared::onnx_runtime`; STT, DPDFNet y el chequeo de `doctor` comparten esa
autoridad; la ruta puede ser archivo o directorio; sin variable se usa el
directorio real del programa; una librería ausente produce error tipado con la
ruta buscada (código 1) y **no** aborta el proceso ni cae a `PATH`. La
inferencia DPDFNet y el motor STT arrancan en procesos distintos con la misma
resolución, y el daemon sirve `/health` con STT caliente.

**No verificado todavía:** STT y DPDFNet infiriendo **a la vez en el mismo
proceso** (no hay aún un fixture que los combine; lo ejercitarán los pasos 8 y
10 al integrar el preprocesamiento), y la resolución en Linux/macOS, que exige
entorno nativo.

**Estado del árbol:** cambios del paso 6 sin commitear; el commit queda a la
espera de autorización explícita del usuario.
