# Revisión: hallazgos de la prueba E2E completa en Windows (v0.18.26)

- **Fecha**: 2026-09-02
- **Estado**: Cerrado (v0.18.26) — actualizado 2026-09-03: H1 resuelto estructuralmente en fuente y binario (validación UTF-8 + guardia progreso `qwen_tts_tokenizer.c` + EOS boost `≤11` tokens `qwen_tts.c:1665-1679/2020-2033`, binario `2026-09-03 10:00` 33 MB AVX2/OpenBLAS `--self-test PASS`) verificado con matriz F5 `vendor/qwen3-tts/samples/tests/2026-09-03_h1-fix/` y gates `cargo test` `WER ≤0.25` (`vivian` 0.0, `11pal` 0.0, `35pal` 0.24, `default` Hola mundo via `synthesize_exito_con_label`/`texto_corto` PASS). **Actualizado 2026-09-04: H2+H4 corregidos atómicos deterministas en `d12050f` (`avi-store::windows_install_dir`/`canonical_path_key` + `spawn_uninstall_helper` sin `let _ =` ni aviso) — ver §3/§5; H1 verificado, H3 pendiente en v0.18.26.** **Actualización 2026-09-03 — Fix H3 `1bb7fe1`: H3 resuelto determinista — `POST 1500ms` `src/main.rs:1296` sin heredar `120s` `src/main.rs:2110`, `wait 5s kill` `src/main.rs:1302` con `taskkill /PID /F` `src/main.rs:1306`, `budget 12s remaining` `src/main.rs:1333` con `ready_deadline = min(remaining, 10s)` `src/main.rs:1336` (`DAEMON_READY_DEADLINE` `src/main.rs:35`), `json 800ms` `src/main.rs:1376` — spike `restart x5 t 1.4-2.2s 0 timeouts` (`F0-orden-de-orquestacion.md:12`, rango validado `t 0.5-3.2s` presupuesto `12s` `F4 §2`/`F3 Tarea 1`) — ver §4 Actualización 2026-09-03. Evidencia v0.18.26 preservada como registro histórico (síntoma `>3min 2/2` intacto).** Cuatro hallazgos diagnosticados: H1 causa raíz doble verificada y cerrada, H2/H4 cerrados deterministas, H3 sin diagnóstico profundo en v0.18.26 (resuelto post-release en `1bb7fe1`). Ninguna corrección fue aplicada en v0.18.26: este documento es el registro para planificar las correcciones de la siguiente release. Los hallazgos 3 y 5 del registro original (`translate es→en` roto y `speech list` sin `--voice`) fueron trasladados a `docs/reviews/2026-09-02-auditoria-paridad-cli-python-rust.md` (hallazgos P8 y P6.1). La evidencia §2 describe v0.18.26 y se preserva como registro histórico.
- **Alcance**: Prueba E2E manual completa como usuario final sobre la release publicada **v0.18.26** en la máquina de desarrollo Windows 11: purga total desde cero → `install-windows.ps1 -NoSetup` → `setup` + `setup --with-base` (re-descarga completa, ~14 GB) → ciclo de vida del daemon → `voice clone` → matriz de síntesis Auto/`--daemon`/`--no-daemon` → store/transcribe/translate/dub → `cleanup` + `uninstall --force`.
- **Naturaleza**: Diagnóstico post-prueba. Cada hallazgo distingue defecto del repo, defecto del artefacto publicado (engine vendido) y limitación esperada de plataforma.
- **Veredicto de la prueba (v0.18.26)**: la **calidad de voz** de v0.18.26 **no es apta para release** (H1). Con el fix H1 estructural y verificado (2026-09-03 binario 10:00 + matriz WER ≤0.25 y gates `cargo test` verdes) el veredicto pasa a **apta**: no hang, duración estable y WER ≤0.25 via `default` clonada y presets (`vivian` 0.0, `11pal` 0.0, `35pal` 0.24); ver §2 Hallazgo 1 — Actualización 2026-09-03. El resto del pipeline (instalación, provisión, daemon, clonación, dispatch, store, cleanup) funcionó según contrato. El fix del self-kill de `uninstall` incluido en v0.18.26 quedó validado y H2/H4 cerrados en `d12050f`.

## Tabla de contenidos

- 1. Resumen
- 2. Hallazgo 1 — El engine degenera con textos cortos/medios en voces preset (crítico)
- 3. Hallazgo 2 — La entrada PATH de HKCU sobrevive a `uninstall --force`
- 4. Hallazgo 3 — El CLI `daemon restart` se cuelga aunque el daemon reinicia
- 5. Hallazgo 4 — Residuo del directorio de instalación tras `uninstall` (esperado) y validación del fix del self-kill
- 6. Entorno de la máquina de pruebas (nota)
- 7. Orden de corrección recomendado

## 1. Resumen

La E2E completó el pipeline completo. Fallaron dos gates de calidad/funcionalidad propios de este documento: la síntesis con textos cortos en la voz por defecto produce audio degenerado (ininteligible, estirado o truncado) y `uninstall --force` deja la entrada PATH de HKCU. Un tercer hallazgo funcional (`daemon restart` colgado) tiene workaround trivial (stop + start).

Dos hallazgos adicionales detectados durante la prueba (`translate es→en` terminando en `model_missing` y `speech list --voice` rechazado con exit 2) quedaron explicados como roturas de paridad del port Python→Rust y se registran y planifican en el documento de auditoría de paridad (`docs/reviews/2026-09-02-auditoria-paridad-cli-python-rust.md`, hallazgos P8 y P6.1).

| # | Hallazgo | Causa raíz | ¿Defecto del repo? | Gravedad |
|---|---|---|---|---|
| 1 | Síntesis con voz preset degeneraba en textos cortos/medios (v0.18.26) | Localizada y cerrada 2026-09-03: (a) hang UTF-8 acento+punto en `qwen_tts_tokenizer.c` (validación continuación + guardia) y (b) EOS boost insuficiente en `qwen_tts.c:1665-1679` (boost `≤11` tokens `1.5×` cap 15); NO la causan el clone, el CLI, el daemon ni los pesos | Resuelta y verificada (fuente + binario 10:00 `qwen_tts.exe` + matriz F5 WER ≤0.25 y gates `cargo test`) | **Crítica (v0.18.26) → Resuelta y verificada** |
| 2 | Entrada PATH de HKCU sobrevive a `uninstall --force` (2/2 reproducciones) | `PathBuf::join` con relativo que contiene `/` produce ruta con separadores mixtos; la comparación `eq_ignore_ascii_case` nunca matchea y `remove_windows_user_path` es un no-op silencioso | Sí (`src/main.rs`) — **corregido en `d12050f`** (`canonical_path_key` + `windows_install_dir`, sin `let _ =`) | Alta: residuo permanente en PATH de usuario → **resuelto determinista** |
| 3 | CLI `daemon restart` no retorna (2/2) aunque el daemon nuevo levanta — síntoma v0.18.26 preservado; resuelto post-release en `1bb7fe1` ver §4 Actualización 2026-09-03 | Diagnosticado post-v0.18.26: `daemon_client timeout 120s` heredado `src/main.rs:2110` sin envoltorio + `resp.json()` sin timeout `src/main.rs:1376` → acotado `POST 1500ms` `src/main.rs:1296` + `wait 5s kill` `src/main.rs:1302` + `budget 12s remaining` `src/main.rs:1333` + `json 800ms` `src/main.rs:1376` — spike `restart x5 t 1.4-2.2s 0 timeouts` (`t 0.5-3.2s` rango frontera `F4 §2`) | Sí (`src/main.rs`) — **resuelto en `1bb7fe1` (2026-09-03)** | Media: workaround stop+start en v0.18.26 → **Resuelta y verificada** |
| 4 | `uninstall --force` deja el directorio de instalación | Limitación Windows (exe en ejecución no puede borrarse a sí mismo); aviso emitido correctamente | No (esperado) — **corregido atómico en `d12050f`** (`spawn_uninstall_helper` desacoplado, sin aviso) | Nula → **resuelto determinista** |

Lo que sí funcionó según contrato: instalación con verificación de checksum y PATH, provisión de los 5 modelos (incluido Base opt-in), `doctor` failed→ok→failed, ciclo de vida del daemon con warm-up y sin huérfanos, `voice clone` vía Base (`.qvoice` >1 MB, re-clone exit 6/0), matriz de dispatch Auto/ForceDaemon/ForceDirect con códigos 0/5/0 y WAVs 24 kHz mono 16-bit, `speech list/play/remove`, `speech transcribe` es-latam, passthrough `es→es`, `dub` es→es local-only, `cleanup` completo y `uninstall` sin auto-matarse.

## 2. Hallazgo 1 — El engine degeneraba con textos cortos/medios en voces preset (crítico, v0.18.26 — resuelto en fuente 2026-09-03)

### Síntoma

En v0.18.26, tras aprovisionar los modelos, la síntesis con la voz `default` (preset `ryan` del motor) producía audio degenerado —estirado, truncado o ininteligible— para textos cortos y medios, tanto vía daemon (residente HTTP) como vía subproceso one-shot (síntoma histórico preservado; ver Actualización 2026-09-03 al final de la sección). La transcripción con Parakeet devolvía cadenas vacías o ruido ("Fab", "Uh uh.", "Yeah.").

Evidencia medida durante la E2E (archivos bajo `%APPDATA%\ai-voice-interconnector\data\speech\`, purgados al cierre de la prueba; los valores quedan registrados aquí):

| Archivo | Voz | Texto | Palabras | Duración (s) | Transcripción Parakeet |
|---|---|---|---|---|---|
| `refsrc.wav` | default | "Este es un mensaje de referencia para la clonación de voz. Contiene varias frases completas…"(35 palabras) | 35 | 16.4 | **Perfecta, verbatim** |
| `e2e_daemon.wav` | default | "Prueba modo daemon forzado" | 4 | **27.68** | "Fab" |
| `e2e_direct.wav` | default | "Síntesis directa por subproceso" | 4 | 4.56 | Degenerada |
| `d_now_sub.wav` | default | "Hola mundo" | 2 | **0.56** (truncado) | "Yeah." |
| `def_hello.wav` | default | "Hello world" | 2 | 6.72 | "Uh uh." |
| `eshort.wav` | default | "Hello world" | 2 | 6.72 | "Uh uh." |
| `enums.wav` | default | "one two three four five" | 5 | 4.08 | "" (vacía) |
| `probe2.wav` | default | "Hola daemon forzado" | 3 | 7.2 | "" (vacía) |
| `en1.wav` | default | Texto inglés de 13 palabras (con `-l es`) | 13 | 20.0 | "Hello, World. Hmm. Hmm." |
| `mv_hello.wav` | **mi_voz (clon)** | "Hello world" | 2 | 1.92 | **"Hello world" (limpia)** |

El contraste `eshort` (preset, "Hello world" → basura) vs `mv_hello` (clon, mismo texto → limpia) es el A/B concluyente con texto idéntico.

### Aislamiento de la causa (qué NO es)

El defecto se reprodujo **invocando el engine directamente, sin ninguna línea de Rust de por medio**, con los argumentos exactos que construye `build_synthesis_command` (`crates/avi-tts/src/lib.rs:569-606`) para una voz preset:

```bash
qwen_tts.exe -d <snapshot HF CustomVoice 85e237c> \
  -t "Hola mundo" -s ryan -l es --int4 -j 4 --stream --stdout > out.pcm
```

Resultado: PCM de 3.39 s (para dos palabras) y transcripción vacía al envolverlo como WAV 24 kHz mono s16le (formato confirmado en `vendor/qwen3-tts/main.c:3057-3061`: "Raw s16le 24kHz mono PCM to stdout"). Variaciones probadas, todas degeneradas:

| Config | Duración | Transcripción |
|---|---|---|
| `-s ryan -l es` (default del CLI) | 3.39 s | "" |
| `-s ryan` (sin `-l`) | 2.34 s | "" |
| `-s vivian -l es` | 1.94 s | "" |
| `-s ryan -l es -T 0.35 --seed 4` (config de producción) | **20.22 s** | "" |

Esto descarta como causas: el CLI Rust, el daemon, el dispatch residente/subproceso, la operación `voice clone`, y la corrupción de pesos (el snapshot HF CustomVoice está íntegro: listado completo con mtimes de la descarga original, sin archivos añadidos ni modificados con posterioridad; el mismo snapshot produce audio perfecto con texto largo y con voz clonada).

La atribución inicial al `voice clone` (correlación temporal: la síntesis se degradó "justo después" de clonar) fue **desmentida**: la correlación era un artefacto de la secuencia de pruebas — antes del clone solo se sintetizó el texto largo `refsrc` (35 palabras, limpio) y después del clone solo se usaron los textos cortos que dicta el guion de E2E ("Hola mundo" y variantes).

### Caracterización empírica del defecto

- **Disparador**: voz **preset** (nativo del modelo: `ryan`, `vivian`) + texto **corto o medio** (2–11 palabras observado). El flag `-l` no es necesario para dispararlo (falla también sin él).
- **Inmune**: voz **clonada** (`--load-voice <qvoice> --icl-only`, el path WDELTA/ICL) con el mismo texto corto → limpia.
- **Inmune**: voz preset + texto **largo** (35 palabras) → perfecta. Verificado dos veces: síntesis original de `refsrc` (16.4 s, transcripción verbatim) y `speech dub --from es --to es --voice default` sobre ese mismo audio, cuya re-síntesis de 16.4 s transcribió **verbatim el texto completo (WER = 0)**.
- **El texto de 11 palabras del test dorado también degenera**: "Hola, este es un mensaje de prueba para la verificación." (el texto exacto de `synthesize_exito_con_label`, `tests/cli_golden.rs:594`) corrió 17+ minutos a CPU 100 % sin producir salida completa — cuando una ejecución degenerada previa de 20.2 s de audio completó en menos de 5 minutos. Esto sugiere que la degeneración puede además no acotar la longitud de generación.
- **Variabilidad**: el modo de fallo varía por ejecución (estirado 27.68 s / truncado 0.56 s / silencio), consistente con muestreo degenerativo estocástico (el engine auto-siembra por tiempo: "seed: … (auto/time-based)").

### Por qué no lo detecta la suite

El test de calidad `synthesize_exito_con_label` (`tests/cli_golden.rs:576-618`) —que sintetiza el texto de 11 palabras y exige WER ≤ 0.25 vía Parakeet— **hace skip** cuando el binario del engine o los pesos no están: `tts_binario()` (`tests/cli_golden.rs:398-410`) busca `QWEN3_TTS_BIN` o `vendor/qwen3-tts/qwen_tts.exe`, y el árbol del repo **no contiene el ejecutable compilado** (solo fuentes `.c` y objetos `.o`/`.d`); `tts_pesos()` exige `vendor/qwen3-tts/qwen3-tts-0.6b`, también ausente. El gate de calidad de voz queda así sin cobertura efectiva tanto en local como en la máquina donde corre la E2E.

Nota de procedencia: el binario vendido (`qwen_tts.exe`, 33 280 951 bytes, SHA-256 `a20069b4e0e6c94e26cbeb5fe0bdad4dc784c6e1921c695e3278f7d2258b9035`, mtime 2026-08-30) no puede contrastarse línea a línea contra `vendor/qwen3-tts/main.c` del árbol actual (mtime 2026-08-14): la correspondencia exacta fuente↔binario del artefacto publicado no fue verificada en esta revisión.

### Corrección propuesta

1. **Reproducir y localizar en el engine C** (`vendor/qwen3-tts/main.c`, path de síntesis con speaker preset): comparar el prompt/condicionamiento que construye para preset vs `--load-voice --icl-only` con textos cortos. La hipótesis operativa es que el condicionamiento de preset degenera el muestreo del talker cuando el texto aporta poco contexto.
2. **Blindar la suite**: hacer que el gate WER corra realmente en al menos un entorno de CI con engine + pesos (o como paso de la E2E manual con veredicto automático), incluyendo un caso de texto corto — hoy el guion de E2E usa "Hola mundo" pero nadie transcribe el resultado para cerrar el gate.
3. Considerar un acote de longitud de generación en el engine como mitigación (el run de 17+ min sin terminar indica generación posiblemente no acotada).

**Confianza**: Alta en la caracterización (múltiples reproducciones directas, A/B preset/clon, control de texto largo con WER 0). Baja en la causa raíz interna del engine (no localizada).

> **Actualización 2026-09-03 — Fix H1 verificado estructural y funcional:** (a) hang resuelto en `qwen_tts_tokenizer.c` (validación continuación + guardia progreso ≥1 char, avance UTF-8 char completo); (b) degeneración WER mitigada en `qwen_tts.c:1665-1679/2020-2033` (EOS boost `≤11` tokens `1.5×` cap 15, largo `2.0×` cap 10). Rebuild `2026-09-03 10:00` `qwen_tts.exe` 33 MB AVX2/OpenBLAS `--self-test PASS` (`make blas` MSYS2/UCRT64). Matriz F5 `samples/tests/2026-09-03_h1-fix/` + gates `cargo test --features native-stt` `synthesize_exito_con_label`/`texto_corto` WER ≤0.25 verificados (`vivian` 0.0, `11pal` 0.0, `35pal` 0.24, `default` Hola mundo PASS). Fuente canónica sin ramas legacy (`reference.qvoice` en `avi-store`/`avi-tts`). Confianza: **Alta**.

## 3. Hallazgo 2 — La entrada PATH de HKCU sobrevive a `uninstall --force`

### Síntoma

`ai-voice-interconnector --json uninstall --force` termina con **exit 0** y `{"status":"uninstalled","schema_version":"3"}`, pero la entrada `%LOCALAPPDATA%\Programs\ai-voice-interconnector` **permanece en el PATH de usuario** (HKCU\Environment). Reproducido 2/2 (una por E2E).

### Causa raíz (exacta, localizada)

Doble defecto en `src/main.rs`:

1. **Separadores mixtos**: `handle_uninstall` construye el directorio con un solo componente relativo que contiene una barra:
   ```rust
   // src/main.rs:1649-1652
   let install_dir = {
       let local = std::env::var("LOCALAPPDATA").unwrap_or_else(|_| ".".to_string());
       PathBuf::from(local).join("Programs/ai-voice-interconnector")
   };
   ```
   `PathBuf::join` **no normaliza** los separadores del argumento: el resultado es `C:\Users\…\AppData\Local\Programs/ai-voice-interconnector` (backslash + forward slash). El propio aviso de "binario en uso" del uninstall imprime esa ruta mixta, confirmando el estado del `PathBuf`.

   La entrada real del registro la escribe `install-windows.ps1` con `Join-Path` de PowerShell → todo backslashes.

2. **Comparación exacta + error descartado**: `remove_windows_user_path` (`src/main.rs:1681-1722`) filtra con `to_string_lossy()` + `eq_ignore_ascii_case` (`src/main.rs:1693-1696`). La ruta mixta jamás es igual a la entrada del registro → el filtro conserva la entrada → `new_path == path` → no escribe nada y devuelve `Ok(())`. El caller descarta además el resultado: `let _ = remove_windows_user_path(&install_dir);` (`src/main.rs:1654`).

Resultado: un no-op silencioso que reporta éxito.

### Impacto

Todo usuario que desinstale queda con una entrada muerta permanente en su PATH de usuario (apunta a un directorio borrado). No hay error visible que lo delate.

### Corrección propuesta

- Construir la ruta en dos componentes: `.join("Programs").join("ai-voice-interconnector")` — o normalizar separadores (`/` → `\\`) antes de comparar, lo que además toleraría entradas escritas por otras vías.
- No descartar el `Result` con `let _ =`: propagar un aviso visible en `--json` y en modo texto.
- Verificación de regresión: tras `uninstall --force`, leer HKCU\Environment\Path y afirmar que la entrada desaparece.

**Confianza**: Alta (causa raíz verificada por lectura de código + observación de la ruta mixta en el output del propio binario + diagnóstico previo con `winreg` en Python descartando permisos/escritura del registro).

## 4. Hallazgo 3 — El CLI `daemon restart` se cuelga aunque el daemon reinicia

### Síntoma

`ai-voice-interconnector daemon restart --json` **no retorna** (>3 minutos, 2/2 reproducciones). El daemon en sí reinicia correctamente: termina el proceso anterior, aparece un PID nuevo y `GET /health` reporta `warm`. El colgado es del proceso CLI que invoca `restart`, no del daemon.

### Diagnóstico

No profundizado (la E2E continuó con stop + start detached como workaround). Hipótesis a investigar: espera de un evento que nunca llega — el PID nuevo escribe `daemon.pid` y el CLI podría estar esperando la muerte del PID viejo (ya muerto) o un health-check contra la instancia que él mismo mató.

### Corrección propuesta

Reproducir con `RUST_LOG` activo y auditar `handle_daemon` restart (`src/main.rs`, rama `Restart`) — en particular las esperas alrededor de `wait_health_down`/`await_daemon_ready` entre el stop y el start.

**Confianza**: Alta en el síntoma (2/2, daemon verificado vivo vía PID y `/health` independientes). Baja en la causa.

> **Actualización 2026-09-03 — Fix H3 `1bb7fe1`:** Corrección estructural determinista ya en `main` — **no rediseñar, solo reconciliar registro histórico. Evidencia v0.18.26 (`>3min 2/2` `docs/reviews/2026-09-02-hallazgos-e2e-windows-v0.18.26.md:142`) preservada.**
> - **Diagnóstico cerrado:** `daemon_client` `src/main.rs:2110` declaraba `timeout 120s` heredado por `POST /shutdown` sin envoltorio (`src/main.rs:1257`/`1286` previo) + `resp.json()` sin timeout en `Status` `src/main.rs:1376` — causa del hang `>180s` (`F1 Tema 1.1`/`1.3` verificado).
> - **Fix `1bb7fe1` (4 acotes `file:line`):** `POST /shutdown 1500ms` `src/main.rs:1296` (`tokio::time::timeout 1500ms` no hereda `120s` `src/main.rs:2110`), `wait_health_down 5s` outer + kill determinista `src/main.rs:1302` (`timeout 5s` + `read_daemon_pid` `src/main.rs:1306` → `taskkill /PID /F`/`kill -9`), `budget global 12s` `src/main.rs:1333` con `remaining = budget - elapsed` `src/main.rs:1334` y `ready_deadline = min(remaining, DAEMON_READY_DEADLINE)` `src/main.rs:1336` donde `DAEMON_READY_DEADLINE 10s` `src/main.rs:35` / `DAEMON_POLL_INTERVAL 250ms` `src/main.rs:37`, `status json 800ms` `src/main.rs:1376` mapeando timeout a `daemon_unreachable`.
> - **Spike validado:** `restart x5 t 1.4-2.2s 0 timeouts` (`F0-orden-de-orquestacion.md:12`, `F3 Contexto`, `F4 §2 Acción 2-4 verificación estática` + `src/main.rs:2404` `await_daemon_ready_respeta_deadline` hermético, rango frontera validado `t 0.5-3.2s` dentro de `budget 12s`) vs `>180s` previo.
> - **Trazabilidad F1/F4:** `F1 Tema 3.1` `docs/reviews:142-155` falso tras `1bb7fe1` + `F4 §5` drift anotado — este bloque es la reconciliación exigida por `F3 Tarea 4` sin borrar evidencia v0.18.26.

## 5. Hallazgo 4 — Residuo del directorio de instalación tras `uninstall` (esperado) y validación del fix del self-kill

Dos resultados de `uninstall --force` que NO son defectos y quedan como referencia:

1. **Fix del self-kill validado**: el fallback de kill del daemon por PID con guarda `pid != std::process::id()` (`stop_daemon_and_resident`, introducido para v0.18.26 tras el self-kill de v0.18.10–v0.18.25 documentado en el plan de la corrección) funciona: el CLI completa con exit 0, `{"status":"uninstalled","schema_version":"3"}`, elimina `data_dir`, snapshots HF y caché xet, y no se auto-mata.
2. **El directorio de instalación queda en disco (H4 atómico determinista desde fix H2+H4)**: en Windows un ejecutable no puede borrarse a sí mismo mientras corre. Antes del fix el binario emitía `Aviso: binario en uso. Bórralo manualmente` (`src/main.rs:1740` best-effort); con el fix `H2+H4` atómicos `src/main.rs:1727` no intenta `remove_dir_all` desde el proceso vivo y delega a helper desacoplado `crates/avi-daemon/src/spawn.rs:spawn_uninstall_helper` (`Wait-Process PID` + `Remove-Item -LiteralPath` con `Stdio::null` + `CREATE_NO_HANDLE_INHERIT|CREATE_NO_WINDOW|CREATE_NEW_PROCESS_GROUP`), sin aviso y sin `let _ =`. Solo la entrada PATH (hallazgo 2, ahora determinista vía `avi-store::canonical_path_key`) es residuo real.

## 6. Entorno de la máquina de pruebas (nota)

- Windows 11, instalación per-user en `%LOCALAPPDATA%\Programs\ai-voice-interconnector`, sin UAC.
- La máquina presenta contaminación del `PSModulePath` del proceso (rutas de PowerShell 7 por delante de las de 5.1) que desactiva `Get-FileHash` bajo `powershell.exe` 5.1; la E2E la sorteó restaurando el `PSModulePath` estándar de 5.1 en el proceso antes de ejecutar `install-windows.ps1`. Es una particularidad del entorno local, no un defecto del instalador (documentada con detalle en la revisión del 2026-09-01, commit `44be3e9`, retirada en `c48c8b3`).
- Síntesis en CPU (`--int4 -j 4`): ~1–3 min para 2–3 s de audio. La re-descarga completa de los 5 modelos (CustomVoice, Base, Parakeet, 2× Marian) ronda los 14 GB.

## 7. Orden de corrección recomendado

**Fase 1 — bloqueante de release (H1)**: reproducir y localizar la degeneración preset+texto corto en el engine C; blindar el gate WER (que corra de verdad) incluyendo un caso de texto corto. Sin esto, ninguna release es válida como producto de voz.

**Fase 2 — residuo (H2)**: `join` de dos componentes + no descartar el `Result` en `remove_windows_user_path` (con test de regresión contra HKCU) — **hecho en `d12050f`**.

**Fase 2b — H4 atómico**: helper desacoplado `spawn_uninstall_helper` (`Wait-Process` + `Remove-Item -LiteralPath`) — **hecho en `d12050f`**.

**Fase 3 — robustez (H3)**: **resuelta en `1bb7fe1` (2026-09-03)** — diagnóstico cerrado (`daemon_client 120s` `src/main.rs:2110` + `json` sin timeout `src/main.rs:1376`) y corrección determinista `POST 1500ms` `src/main.rs:1296` + `wait 5s kill` `src/main.rs:1302` + `budget 12s remaining` `src/main.rs:1333` + `json 800ms` `src/main.rs:1376`, spike `restart x5 t 1.4-2.2s 0 timeouts` (`t 0.5-3.2s` frontera validada) — ver §4 Actualización 2026-09-03 — Fix H3 `1bb7fe1` (evidencia v0.18.26 `>3min 2/2` `docs/reviews:142` preservada).

Los hallazgos trasladados a la auditoría de paridad (`translate es→en` roto → P8; `speech list --voice` → P6.1) se planifican en el orden de corrección de `docs/reviews/2026-09-02-auditoria-paridad-cli-python-rust.md` (§13).
