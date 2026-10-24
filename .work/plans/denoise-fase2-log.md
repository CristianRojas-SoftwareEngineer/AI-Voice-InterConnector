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

**Estado del árbol:** cambios del paso 6 commiteados en `62015764`.

## 6. Paso 7 — DPDFNet en el catálogo, integridad y resolvedor único

### 6.1 Decisiones aplicadas

| Decisión | Implementación | Motivo verificado |
|---|---|---|
| Pin inmutable | `Ceva-IP/DPDFNet` @ `dd6818d00f50c836fed43a6243ebe49116de5964`; `onnx/dpdfnet8_48khz_hr.onnx`; 14 857 107 B; SHA256 `7b3afbb2…5631` | La API de HF devolvió ese `sha` para `main`; el árbol de la revisión confirmó `lfs.oid` y `size`, y el archivo local coincide |
| Dos campos nuevos en `ModelPin` | `artifact_sha256: Option<&str>` y `startup_optional: bool` | El hash es opcional porque los repos multiarchivo ya se validan por revisión/LFS; el arranque opcional es una propiedad del pin, no del daemon |
| DPDFNet opcional al arrancar | `startup_optional: true`; `load_engines` lo omite; `setup` lo provisiona igual | El daemon debe servir STT/TTS sin DPDFNet; solo las operaciones opt-in de limpieza lo exigen |
| Integridad sin cambiar `is_provisioned` | `ModelStore::verify_artifact` hashea el único artefacto; se usa tras descargar, antes de inferir y en `doctor` | `is_provisioned` sigue siendo estructural para no romper los sandboxes que siembran snapshots falsos; el hash se exige donde importa |
| Reparación sin caché privada | `ensure_downloaded` borra el repo completo si el hash no coincide; el resolvedor de denoise solo lee el almacén canónico | Una corrupción del mismo tamaño no puede pasar; no hay descarga silenciosa durante el procesamiento |
| Resolvedor único | `AviModelsProvider` en `denoise` usa `avi_store::ModelStore`; se retiraron `DefaultModelsProvider`, `~/.cache/denoise`, `reqwest`, `home` y `sysinfo` | El modelo se resuelve por `setup`/`doctor`; `--model-dir` deja de existir en la CLI transitoria |

### 6.2 Verificación ejecutada

Entorno: Windows x64; raíz canónica
`%LOCALAPPDATA%\ai-voice-interconnector\cache\models`; FFmpeg 9.0.1 en
`PATH`; `ort-bundle/onnxruntime.dll` 1.28.0.

| Comando / prueba | Resultado |
|---|---|
| `cargo test -p avi-store` | V 28 tests de lib + 1 de layout HF; incluye SHA256 conocido de `abc` y rechazo de corrupción del mismo tamaño |
| `cargo test -p denoise` | V **56 passed / 6 ignored / 0 failed** |
| `cargo test --release -p denoise -- --ignored` | V **6 passed**: golden 2, E2E WAV 1, remux 3 |
| `cargo test -p avi-shared --features native-ort` | V 44 passed |
| `cargo test -p avi-lifecycle` | V 183 + 20 + 2 + 25 + 8 + 10 + 6 + 11 + 5, 0 failed |
| `cargo test -p avi-daemon` | V 50 + 20 (6 ignored), 0 failed |
| `cargo test --test self_install_contract` | V 3 passed |
| `cargo test --test cli_golden` | V 132 passed / 21 ignored |
| `cargo check --workspace --all-targets` | V |
| `cargo clippy --workspace --features full --all-targets -- -D warnings` | V 0 avisos |
| `cargo xtask language/comments/pins/features --check` | V 0 hallazgos en las cuatro puertas |
| `setup --json` con DPDFNet ausente | V descargó 1 modelo (14,9 MB en 7 s) y publicó `dpdfnet8` entre los provisionados |
| `doctor --json` con DPDFNet sano | V `models_provisioned: true`; `missing: []`; `size_bytes: 5 865 044 386` |
| `denoise` con raíz vacía | V código 1, «El modelo dpdfnet8 no está provisionado; ejecuta `ai-voice-interconnector setup`…», sin crear directorios de modelo |
| `denoise` con DPDFNet corrupto del mismo tamaño | V código 1, SHA256 esperado/obtenido y `setup --force-update` como reparación |
| `denoise` con `HF_ENDPOINT` muerto y `HF_HUB_OFFLINE=1` | V código 0, salida de **288 078 B** (sin red en inferencia) |
| Paridad de inferencia | V `voz_noisy.wav` → 288 078 B, SHA256 `465777a96c44b11d098b861ccf8e50c43b038decef8408bbab7a84ffd4d491f4`; tamaño idéntico a la referencia de Fase 1 (la diferencia de bytes frente al origen ya documentada es del build de ORT, no del proveedor de modelo) |
| Daemon sin DPDFNet | V con el repo apartado, `daemon serve` publicó `addr`, `/health` dio `status: ready` y `warm: warm`; `POST /shutdown` terminó con código 0 |
| `cleanup --model --yes --json` con DPDFNet sembrado | V borró la raíz de modelos del sandbox, DPDFNet incluido |
| `setup --json` con `HF_ENDPOINT` muerto | V código 20, `reason: network_error`, error `dpdfnet8: HTTP request error…` |
| `doctor --json` con DPDFNet corrupto | V `dpdfnet8` en `missing` aunque el snapshot existe estructuralmente |
| `denoise --model-dir …` | V código 2; `--model-dir` ya no aparece en la ayuda ni se acepta |

### 6.3 Adaptaciones de pruebas

| Archivo | Cambio | Causa |
|---|---|---|
| `crates/avi-lifecycle/src/setup.rs` | total fijado 5 865 044 386 B y resumen de 6 modelos | El pin nuevo entra en la selección |
| `tests/self_install_contract.rs`, `tests/cli_golden.rs` | los sembradores crean el directorio padre de cada patrón | DPDFNet declara `onnx/dpdfnet8_48khz_hr.onnx`, que no es un archivo raíz |
| `crates/denoise/tests/test_audio_pipeline.rs`, `examples/process_wav.rs` | usan `AviModelsProvider` y ya no pasan `model_dir` | El modelo se resuelve por el almacén canónico |
| `crates/denoise/Cargo.toml` | `avi-store`; fuera `reqwest`, `home`, `sysinfo`; `sha2` a dev-dependencies | El crate no descarga ni mantiene caché propia; `sha2` solo lo usa `test_remux` |

### 6.4 Puerta del paso 7

**Cubierta:** el artefacto DPDFNet está fijado por revisión inmutable y SHA256;
`setup` lo descarga y valida, `doctor` lo verifica, `cleanup` lo barre y el
daemon arranca sin él. La CLI y el resolvedor de denoise leen únicamente el
almacén canónico: no hay descarga silenciosa ni caché privada, y una corrupción
del mismo tamaño se detecta antes de inferir. La inferencia offline conserva el
tamaño de la referencia.

**No verificado todavía:** Linux/macOS (requiere entorno nativo); el anidado
`models_cause` de `self install`/`self update` con DPDFNet en concreto (el
contrato existente lo ejerce con `opus-mt-es-en`, y el camino de provisión es el
mismo); STT y DPDFNet infiriendo a la vez en un proceso (pasos 8 y 10).

**Estado del árbol:** cambios del paso 7 commiteados en `5989ede7`; cambios del
paso 8-audio se commitean a continuación en un tramo separado.

## 7. Paso 8 - Remuestreo antialias de los flujos internos

### 7.1 Decisión aplicada

`avi_audio::resample_antialias` sustituye a la ruta específica de 24 kHz y
generaliza la autoridad de conversión PCM interna a 16, 24 y 48 kHz. La usan la
carga WAV→16 kHz de STT, la referencia WAV→24 kHz del clonado y el
post-procesado de captura de micrófono; la reproducción por dispositivo
conserva su interpolación lineal porque no alimenta STT ni clonado y cambiar su
contrato de latencia exige una medición aparte.

Propiedades conservadas por las pruebas: longitud exacta
`round(muestras × destino / origen)`, retardo entero del filtro FFT
compensado con un residuo submuestra, identidad cuando origen y destino
coinciden, y rechazo explícito de ratios no construibles (sin degradar a
lineal).

### 7.2 Verificación ejecutada

Entorno: Windows x64; `ort-bundle/onnxruntime.dll` 1.28.0; Parakeet v3
provisionado; FFmpeg 9.0.1 en `PATH` para la comprobación externa.

| Comando / prueba | Resultado |
|---|---|
| `cargo test -p avi-audio` | V 33 passed, 0 failed; incluye longitud exacta en 9 combinaciones, retardo en 16/24/48, identidad y aliasing |
| `cargo test -p avi-stt` | V 0 tests (el crate no declara unitarios; el contrato se ejerce desde la CLI) |
| `cargo test -p avi-daemon` | V 50 + 20 (6 ignored), 0 failed |
| STT con `voz_noisy.wav` (48→16 nuevo) | V «Bien, me envió correos electrónicos recientemente.» (idéntico al lineal anterior) |
| STT con `voz_clean.wav` (48→16 nuevo) | V «Bien me envió correos electrónicos recientemente» (idéntico) |
| STT con 7 clips limpios del EvalSet | V 6 transcripciones idénticas al lineal; `car_snr0` gana puntuación final; `train_snr5` deja de alucinar «me gustaría escuchar» y conserva el resto |
| `voz_3s_48k.wav` | ! el antialias omite «me»; FFmpeg `swr` a 16 kHz reproduce la misma omisión, así que procede de la señal bandlimited y no de la implementación |
| Latencia STT (frío, proceso local) | ~3,3-5,4 s dominada por la carga del modelo; sin señal de regresión atribuible al remuestreo |

La comparación con el camino lineal se hizo generando WAV de 16 kHz con
interpolación lineal en un temporal externo al repositorio y transcribiéndolos
por la ruta identidad; no se añadieron fixtures ni artefactos.

### 7.3 Puerta del paso 8 (audio)

**Cubierta:** todas las solicitudes WAV y de micrófono que alimentan STT o
clonado pasan por la misma autoridad antialias; la longitud y el retardo se
conservan, el aliasing se atenúa y las transcripciones del corpus no empeoran
de forma atribuible al cambio.

**No verificado todavía:** el tramo 16→48→16 con denoise activado (paso 10);
la reproducción por dispositivo sigue con interpolación lineal por contrato
propio.

**Pendiente de ffmpeg:** por decisión del usuario, la localización, errores,
licencia y empaquetado de FFmpeg se cierran en el paso 9 (Fase 3); el paso 8 no
añade `pins.json` ni bundle. La puerta de Fase 2 no se declara completa hasta
entonces.
