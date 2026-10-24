# Fase 0 — Matriz de contratos de Denoise y evidencia verificable

Artefacto de la puerta del paso 1 de `.work/plans/denoise-implementation.md` §5:
«Registrar matriz de funciones/flags/casos borde con prueba de origen y futuro
test AVI». Rama AVI: `feat/denoise-migration`. Fecha de captura: 2026-10-24.

Entorno de captura: Windows x64, `denoise 1.0.0` (`target/release/denoise.exe`,
compilado 2026-10-23), ffmpeg `9.0.1-full_build-www.gyan.dev`, modelo
`~/.cache/denoise/models/dpdfnet8_48khz_hr.onnx`. Testigo: `tests/data/voz_noisy.wav`
(3,000 s, PCM16 48 kHz mono) y derivados en `Denoise/out/fase0/media/`.
Todas las capturas viven en `Denoise/out/fase0/` (gitignored).

## 1. Superficie de flags

Prueba de origen: `src/cli.rs:27-133` + `Denoise/out/fase0/help.txt`.

| Flag / argumento | Contrato observado | Prueba de origen | Futuro test AVI |
|---|---|---|---|
| `INPUT...` | 1..n rutas; `required_unless_present = "version"` | `cli.rs:37-40`; `bad-args.log` (exit 2 sin entradas) | `denoise` sin argumentos → exit 2 |
| `-o, --output-name` | Solo si el lote expandido tiene exactamente 1 medio; si el nombre no acaba en extensión de medio, se le añade la del medio (`.mp4` vídeo, extensión original audio) | `cli.rs:283-331`; `runs/outputname-multi.log` (exit 2) | Lote de 2 → exit 2; nombre sin extensión → extensión derivada |
| `--output-dir` | Directorio base; se crea si falta (`pipeline.rs:120-126`); se excluye de escaneos del propio directorio | `cli.rs:247-252`; `runs/batch-dir.log` | Salida fuera del árbol de entrada, y `--output-dir` anidado excluido |
| `--prefix` / `--suffix` | Alfabeto `[A-Za-z0-9._-]`; `.` y `..` rechazados; sufijo por defecto `_denoised` | `cli.rs:193-206`; `runs/bad-prefix.log`, `runs/prefixed.log` | Prefijo inválido → exit 2; prefijo válido aplicado |
| `--recursive` | Recrea el árbol relativo a `cwd` bajo `--output-dir`; excluye medios cuyo stem ya acaba en el sufijo | `cli.rs:361-378,395-420`; `runs/batch-rec/` | Árbol reconstruido; reejecución no reprocesa `*_denoised` |
| `--overwrite` / `--skip-existing` | Mutuamente excluyentes (`conflicts_with`) | `cli.rs:69-77` | Ambos → exit 2 de clap |
| `--preset <PRESET>` | Único valor `studio`; ausente = defaults canónicos | `preset.rs:70-115` | `--preset studio` ≡ sin flag en AVI (decisión §8) |
| `--audio-bitrate` | Rango 64–320, defecto 192; se valida antes de ffmpeg | `cli.rs:645-653`; `runs/bad-bitrate.log` (exit 2) | Fuera de rango → exit 2 sin tocar ffmpeg |
| `--dry-blend` | 0.0–1.0, defecto 0.0 | `cli.rs:655-663` | Fuera de rango → exit 2 |
| `--pause-gate-db` | −40.0–0.0, defecto −25.0; 0 desactiva el gate | `cli.rs:665-673` | Fuera de rango → exit 2; 0 sin atenuación |
| `--highpass` | Booleano de activación; el positivo no puede desactivar el del preset (OR) | `cli.rs:894-905` | `--preset studio --highpass` y `--preset default --highpass` distinguibles |
| `--normalize` | Fija `target_lufs = Some(-16.0)` | `cli.rs:891-893` | `--normalize` ≡ `--target-lufs -16` |
| `--target-lufs` | −36.0 a −12.0 LUFS | `cli.rs:675-685` | Fuera de rango → exit 2 |
| `--stereo` | Procesa 2 canales solo si la entrada es estéreo (`pipeline.rs:186-191`); `studio` no fuerza estéreo | `cli.rs:907-909`; `preset.rs:106-113` | Entrada mono con `--stereo` sigue mono; estéreo real conserva 2 canales |
| `--model-dir` | Caché privada `~/.cache/denoise/models` | `cli.rs:113-115`; `models.rs:130-135` | **Eliminado en AVI**: flag desconocido con instrucción de `setup`/`doctor` |
| `--ffmpeg-path` | Ruta explícita, se prefiere a PATH | `cli.rs:117-119`; `ffmpeg_io.rs:28-42`; `runs/ffmpeg-missing.log` (exit 1) | **Eliminado en AVI**: solo ejecutable integrado; ruta inválida → error explícito |
| `--dry-run` | No escribe ni infiere; resume colisiones | `cli.rs:546-556`; `runs/jsonl-dryrun.jsonl` | `--dry-run` no crea archivos ni carga modelo |
| `--json` | JSONL a stdout; progreso JSONL, no barra | `cli.rs:548-556,742-860`; `runs/jsonl-wav.jsonl` | Estructura y estados idénticos (ver §4) |
| `--verbose` | Trazas de depuración a stderr y preserva temporales (`pipeline.rs:158`) | `cli.rs:129-131` | `--verbose` conserva `.tmp.*`/`.part.*` |
| `-V, --version` | `denoise <ver> + modelo DPDFNet + ffmpeg <primera línea>`; exit 0 | `cli.rs:604-611`; `out/fase0/version.txt` | Texto adaptado a AVI, exit 0 |
| `-h, --help` | Ayuda en español; exit 0 | `out/fase0/help.txt` | Subcomando AVI con las mismas opciones (menos las 2 retiradas) |

## 2. Precedencia de parámetros y limitación de booleanos

- Orden: **defaults → preset → flags individuales** (`cli.rs:870-911`).
- Booleanos se combinan con OR: `--preset studio` + `--highpass` no aporta nada y
  no hay `--no-highpass` en origen. AVI no debe inventar ese flag como paridad
  (plan §4.2); la desactivación del highpass de `studio` se expresa con
  `--preset default`.
- `--normalize` y `--target-lufs` pueden coexistir: gana `--target-lufs` (se aplica
  después, `cli.rs:891-895`).

## 3. Naming, colisiones y lotes

| Caso borde | Contrato observado | Prueba de origen | Futuro test AVI |
|---|---|---|---|
| Extensión de salida | Vídeo → `.mp4`; audio → extensión de entrada en minúsculas | `cli.rs:180-191` | `.MOV` → `.mp4`; `.WAV` → `.wav` |
| Mismo archivo entrada/salida | `--output-name` que resuelve a la entrada → exit 2 | `cli.rs:317-324` | Salida == entrada → exit 2 |
| Prefijo y sufijo vacíos in-place | Error explícito | `cli.rs:328-333`; `runs/affix-empty.log` | Mismo error, exit 2 |
| Colisión intra-lote | Sufijo `_1`, `_2`… y aviso por stderr | `cli.rs:389-418`; `runs/batch-dup/` | Dos entradas con el mismo stem → `_1` |
| Destino existente sin flag | `E_OUTPUT_EXISTS`, exit 2, destino intacto | `pipeline.rs:127-135`; `runs/collision.log` | Hash del destino preexistente sin cambios |
| Destino existente con `--skip-existing` | Estado `skipped`, exit 0 | `runs/skip.log` | `skipped` en JSONL y resumen |
| Orden del lote | Ordenación byte-wise de rutas normalizadas | `cli.rs:266-272,308` | Determinismo repetible |
| Duplicados de entrada | Dedup Windows case-insensitive, Unix byte-exact | `cli.rs:151-158` | Misma ruta dos veces → 1 ítem |
| Directorio sin medios | `E_INVALID_INPUT` «Lote vacío», exit 2 | `cli.rs:303-309`; `runs/batch-dir.log` | Idéntico |
| Extensión no soportada | Rechazo explícito con las listas válidas | `cli.rs:285-294` | `.txt` → exit 2 |

## 4. Reportería JSONL y códigos de salida

Estructura (`cli.rs:139-171`; captura `runs/jsonl-wav.jsonl`):

```json
{"input":"…","output":"…","status":"ok|failed|skipped|dry-run","message":"…","pct":0-100}
{"summary":{"ok":1,"failed":0,"skipped":0}}
```

Observaciones verificadas:

- En `--json` **todas** las líneas de progreso llevan `status:"ok"` y `pct`
  creciente; la línea final del archivo repite `ok` con `pct:100` y mensaje
  `"<MB> MB, <s>s"`. Solo la última línea es el `summary`.
- Las líneas van a **stdout**; en modo `--json` también el fallo de un ítem sale
  como línea `status:"failed"` en stdout, y los mensajes humanos/progreso van a
  **stderr** solo sin `--json`.
- `--dry-run --json` emite `status:"dry-run"` con mensajes `would process` /
  `would overwrite` / `would skip` / `would fail: E_OUTPUT_EXISTS` y el resumen
  cuenta en `ok`/`skipped`/`failed` (`runs/jsonl-dryrun.jsonl`).

Códigos de salida (`errors.rs:27-37`, confirmados en captura):

| Código | Clases | Captura |
|---|---|---|
| 0 | Éxito (incluye `--dry-run`, `--skip-existing` sin trabajo y `-V`/`-h`) | `runs/jsonl-dryrun.jsonl`, `runs/skip.log`, `out/fase0/version.txt` |
| 2 | `E_INVALID_INPUT`, `E_OUTPUT_EXISTS`, `E_NO_AUDIO` | `runs/collision.log`, `runs/missing-input.log`, `runs/bad-bitrate.log`, `runs/bad-args.log` |
| 1 | `E_FFMPEG_NOT_FOUND`, `E_MODEL_MISSING`, `E_FFMPEG_FAILED`, `E_IO` | `runs/ffmpeg-missing.log` |
| 3 | `E_CANCELLED` (Ctrl+C; segundo Ctrl+C fuerza salida 3) | `cli.rs:563-573,846-850` — verificado por lectura, no por captura interactiva |

En lote, el código final es el más grave del lote: un fallo con código 1
prevalece sobre código 2 (`cli.rs:851-857`).

## 5. Matriz de medios

Prueba de origen: `cli.rs:20-24` (listas) y `ffmpeg_io.rs` (probe/extracción/egresión).

| Capacidad | Contrato | Prueba de origen | Futuro test AVI |
|---|---|---|---|
| Entradas de audio | `wav mp3 m4a aac flac ogg opus` | `cli.rs:23` | Un archivo por extensión aceptado |
| Entradas de vídeo | `mp4 mov mkv webm avi` | `cli.rs:20` | Un archivo por extensión aceptado |
| Detección de vídeo real | Carátula `attached pic` no convierte audio en vídeo | `ffmpeg_io.rs:107-113` + test unitario `attached_picture_is_not_a_video_stream` | MP3 con carátula se procesa como audio |
| Extracción | `-map 0:a:0 -vn -ac <1|2> -ar 48000 -sample_fmt s16` | `ffmpeg_io.rs:240-272` | WAV temporal 48 kHz PCM16 mono/estéreo |
| Egresión vídeo | MP4, `-c:v copy`, audio AAC, `-t <dur>`, `loudnorm` opcional | `ffmpeg_io.rs:277-330` | Stream de vídeo idéntico (hash de paquete), audio AAC, duración ±0,5 s |
| Egresión audio | `audio_codec_for`: wav→pcm_s16le, mp3→libmp3lame, m4a/aac/mp4/mov→aac, flac→flac, ogg→libvorbis, opus→libopus | `ffmpeg_io.rs:333-346` | Matriz completa códec/contenedor |
| Bitrate | Solo en códecs con pérdida; PCM16/FLAC lo ignoran | `ffmpeg_io.rs:390-400` | `--audio-bitrate` con WAV/FLAC no cambia tamaño ni falla |
| Normalización | `loudnorm=I=<lufs>:TP=-1.5:LRA=11`, una pasada, al egresar | `ffmpeg_io.rs:318-324,401-406` | −16,0 ±0,5 LUFS y TP ≈ −1,5 dBTP |
| Verificación de salida | Existe, >0 B, stream de audio, vídeo si era vídeo, duración ±0,5 s | `ffmpeg_io.rs:432-485` | Salida truncada → fallo |
| Sin pista de audio | `E_NO_AUDIO` (exit 2) tras `probe` | `ffmpeg_io.rs:196-198` | Vídeo mudo → exit 2 |
| Archivo 0 B o ausente | `E_INVALID_INPUT` (exit 2) | `ffmpeg_io.rs:141-152` | Idéntico |
| Entrada corrupta | «sin streams de medios reconocidos», exit 2 | `ffmpeg_io.rs:188-193` | Idéntico |

## 6. Evidencia de egresión capturada (6 corridas, exit 0)

Todas sobre el mismo audio de 3,000 s; `default` = sin preset, `studio` = `--preset studio`.

| Salida | Códec | Tasa | Canales | Bitrate medido | Duración | LUFS integrado | True peak |
|---|---|---|---|---|---|---|---|
| `default-wav.wav` | pcm_s16le | 48 kHz | 1 | 768 kbps | 3,000 s | −24,8 | −8,6 dBTP |
| `studio-wav.wav` | pcm_s16le | **192 kHz** | 1 | 3072 kbps | 3,000 s | −16,2 | −1,5 dBTP |
| `default-mp3.mp3` | mp3 | 48 kHz | 1 | 192 kbps | 3,000 s | −25,6 | −9,7 dBTP |
| `studio-mp3.mp3` | mp3 | 48 kHz | 1 | 320 kbps | 3,000 s | −16,2 | −1,5 dBTP |
| `default-mp4.mp4` | h264 + aac | 48 kHz | 1 | 135,5 kbps audio | 3,020 s | −25,3 | −9,2 dBTP |
| `studio-mp4.mp4` | h264 + aac | **96 kHz** | 1 | 214,6 kbps audio | 3,020 s | −16,0 | −1,5 dBTP |

Hallazgos que la Fase 3 debe resolver explícitamente (no reabren decisiones cerradas):

1. **`studio` sobre WAV produce 192 kHz** (loudnorm sin `-ar`). Confirma el hallazgo
   ya registrado en el plan §2 y la decisión §8 de fijar 48 kHz en WAV/FLAC AVI.
2. **`studio` sobre vídeo produce AAC a 96 kHz**: el mismo efecto aparece en la ruta
   de remux. El plan §8 fija 48 kHz solo para WAV/FLAC y pide verificar vídeo por
   contenedor; AVI debería fijar la tasa también aquí y registrarlo como divergencia
   deliberada.
3. **El bitrate AAC medido (135/215 kbps) queda por debajo del solicitado** (192/320).
   Es comportamiento de FFmpeg con audio corto/mono, no un fallo del contrato;
   el test AVI debe comparar bitrate *solicitado* en la línea de comandos y no
   exigir un bitrate medido exacto.
4. Duración en MP4 (3,020 s) excede la entrada (3,000 s) por relleno de contenedor,
   dentro de la tolerancia ±0,5 s del verificador.

## 7. Modelo y runtime

| Elemento | Origen (Denoise) | Destino (AVI) | Evidencia |
|---|---|---|---|
| Artefacto | `dpdfnet8_48khz_hr.onnx`, 14.857.107 B, SHA256 `7b3afbb2…5631`, Apache-2.0 | Catálogo `avi-store` (§5 paso 7) | `models.rs:19-26` |
| Resolución | `~/.cache/denoise/models` o `--model-dir` | Directorio canónico AVI + `setup` | `models.rs:130-135` |
| Revalidación al reutilizar | **Ninguna**: basta tamaño >0 | SHA256 en descarga **y** reutilización | `models.rs:68-75` |
| Red en inferencia | Sí, descarga silenciosa si falta | Prohibida; error `model_missing` | `models.rs:60-125` |
| Espacio libre | Exige 50 MB | No aplica | `models.rs:155-178` |
| Binding | `ort =2.0.0-rc.13` con `copy-dylibs`, `download-binaries`, `tls-rustls`, `lax-feature-matching` | `ort =2.0.0-rc.13` con `load-dynamic` | `Denoise/Cargo.toml:22`; `cargo tree -e features -i ort --features native-stt` |
| Runtime nativo | Copiado por `ort` al directorio de build | Pin `1.28.0` + 4 SHA-256 por target | `packaging/pins.json:3-8` |
| Carga en STT | — | `ORT_DYLIB_PATH` o `program_dir()/onnxruntime.dll`, `OnceLock` | `crates/avi-stt/src/parakeet.rs:76-137` |

`cargo tree -e features -i ort --features native-stt` (AVI) resuelve únicamente
`ort v2.0.0-rc.13` con las features `load-dynamic`, `preload-dylibs` y `std`:
no hay segunda autoridad ORT ni `copy-dylibs`/`download-binaries`.

### 7.1. Provisionamiento y carga real — verificado el 2026-10-24

`cargo xtask bootstrap --yes` completó los 6 pasos (motor TTS incluido) y dejó
`ort-bundle/` con `.ort-version` = `1.28.0`, `onnxruntime.dll` y el CRT de VC++.

**`pins.ort_sha256` es el SHA-256 del archivo de release**
(`onnxruntime-win-x64-1.28.0.zip`), **no** el de la DLL extraída: `bootstrap`
verifica el archivo descargado y solo después extrae y escribe el marcador
(`crates/xtask/src/package.rs:400-426`). Comparar el hash de
`ort-bundle/onnxruntime.dll` (`18370c37…0257`) contra `pins.json`
(`abef733d…f8bc`) es por tanto una comparación inválida; el pin se cumple en el
archivo descargado y el `.ort-version` solo existe tras esa verificación.

Cadena del pin verificada de forma independiente: la descarga directa del archivo
`onnxruntime-win-x64-1.28.0.zip` (78.796.801 B) tiene SHA-256
`abef733dacbe2f571547a7150b479b5cb9cc0df22f96c24983a42cadb1b4f8bc`, idéntico a
la entrada `x86_64-pc-windows-msvc` de `packaging/pins.json`.

Carga real observada, con `./target/debug/ai-voice-interconnector.exe`
(`cargo build --features native-stt`) y el comando `doctor`, que invoca
`ensure_onnx_runtime()` en proceso (`src/main.rs:3980-3991`):

| Caso | `ORT_DYLIB_PATH` | Resultado | exit |
|---|---|---|---|
| 1. Sin variable | ausente | `✗ no se pudo cargar … desde <repo>\target\debug\onnxruntime.dll: el fichero no existe` | 1 |
| 2. Bundle real | `<repo>/ort-bundle` | `✓ ONNX Runtime cargada desde <repo>/ort-bundle\onnxruntime.dll` | 0 |
| 3. DLL truncada a 4 KiB | directorio temporal | `✗ … failed to load from …: LoadLibraryExW failed` | 1 |
| 4. Copia buena fuera del repo | directorio temporal | `✓ ONNX Runtime cargada desde <temp>\onnxruntime.dll` | 0 |

Los casos 1 y 3 son el control negativo que el plan exige: **no hay fallback** al
`PATH` ni a la `onnxruntime.dll` de `System32`; si la ruta resuelta no existe o no
carga, el proceso falla nombrando esa ruta. El caso 4 demuestra que la ruta
reportada es exactamente el archivo cargado.

Observación a nivel de **módulo del proceso STT vivo** (`daemon serve` con
`native-stt`, `/health` → `stt: warm`, PID del daemon enumerado con
`Get-Process -Module`; el daemon se detuvo después con `daemon stop`):

```
Path:    C:\…\AI-Voice-InterConnector\ort-bundle\onnxruntime.dll
Version: 1.28.0.20260724.14.da9b5e3
SHA256:  18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257
```

La ruta y la versión de archivo coinciden con el bundle provisionado y con el
pin `1.28.0`. El motor STT carga ORT por esa ruta porque `ParakeetEngine::new`
llama a `ensure_onnx_runtime()` como primera operación
(`crates/avi-stt/src/parakeet.rs:161-166`).

Consecuencia para la Fase 2: el trasplante de DPDFNet debe reutilizar
`ensure_onnx_runtime()` y heredar este mismo bundle; **no** debe declarar su
propio pin ni comparar hashes de DLL contra `pins.json`, que solo fija el
archivo de release.

## 8. Tests ignorados de origen (6)

Prueba: `grep -rn "#\\[ignore" tests src`.

| Test | Marcador | Motivo |
|---|---|---|
| `tests/test_golden.rs:24` | `#[ignore]` | golden SI-SDR con vectores PCM16 deterministas |
| `tests/test_golden.rs:84` | `#[ignore]` | segunda verificación golden |
| `tests/test_audio_pipeline.rs:16` | `#[ignore]` | E2E del pipeline sobre archivo real (ffmpeg + modelo) |
| `tests/test_remux.rs:86` | `#[ignore]` | remux real de vídeo |
| `tests/test_remux.rs:134` | `#[ignore]` | remux real de vídeo |
| `tests/test_remux.rs:175` | `#[ignore]` | remux real de vídeo |

Ninguno cuenta como aprobado (plan §5 paso 2 y §6). Deben portarse y ejecutarse
manualmente en la Fase 4 con modelo y ffmpeg disponibles.

`Denoise/docs/metrics.md` leído: SI-SDR (limpieza, invariante a escala), STOI
(inteligibilidad), PESQ (naturalidad), DNSMOS (SIG/BAK/OVR), columnas extra
(LUFS, RTF, p5) y §5 «lo que las métricas no dicen». Consecuencia para AVI: no
aprobar DSP con una sola métrica ni equiparar SI-SDR con calidad percibida.

## 9. Estado de los repositorios en la captura

| Raíz | Rama | `git status` | Último commit |
|---|---|---|---|
| AVI | `feat/denoise-migration` (creada en este tramo desde `main`) | `?? .work/` únicamente | `c9f11f4c docs(goal): ampliar el alcance a TTS, STT, traducción y doblaje` |
| Denoise | `main` | `?? .work/` únicamente | `2d50aa7 fix(benchmark): actualiza baselines con RTF reproducible via ORT 1.28.0` |

`git diff` vacío en ambas raíces: no hay cambios del usuario en juego. Las
capturas se escribieron solo en `Denoise/out/fase0/` (gitignored).

Divergencia detectada: `Denoise/AGENTS.md` documenta «45 passed / 6 ignored»
mientras el estado registrado del tramo anterior es «57 passed / 6 ignored».
El conteo de ignorados coincide con disco (6); el número de AGENTS.md está
desactualizado. No se edita el origen sin decisión explícita.

## 10. Smoke del FFmpeg empaquetado

Evidencia en `Denoise/out/fase0/ffmpeg-smoke/` (gitignored): `version.txt`,
`codecs-{encoders,decoders,muxers,demuxers}.txt`, `filters.txt`, `runs/` y el
bundle emulado en `bundle/`.

### 10.1. Paquetes fijados por target

| Target | Artefacto | URL exacta | Tamaño | SHA-256 verificado | Ejecutado |
|---|---|---|---|---|---|
| Windows x64 | `ffmpeg-9.0.2-essentials_build.zip` | `https://www.gyan.dev/ffmpeg/builds/packages/ffmpeg-9.0.2-essentials_build.zip` | 114.768.076 B | `60f467265b1e312373dbcd92200c2618a74850f98d3d078e94296bb3fa2047ba` ✓ | **Sí** |
| Linux x64 | `ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz` | `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz` | 151.204.652 B | `3b4c62d62adbaf07cf5d8c8d9fdc4406de031c133e171c5ddff1bec49e1c843d` ✓ | No (sin entorno nativo) |
| macOS arm64 | `ffmpeg.zip` (release 9.0.2, build server de Martin Riedl) | `https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip` | 28.395.699 B | `c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924` ✓ | No (sin entorno nativo) |

Corrección de URL: el paquete de gyan vive en `/builds/packages/`, no en
`/builds/`; la ruta corta devuelve 404. Su sidecar
`….zip.sha256` publica exactamente el hash fijado, y la descarga propia coincide
byte a byte con él (doble verificación: upstream y local).

**Linux: pin cambiado a la variante GPL.** Se descarta
`ffmpeg-n9.0-latest-linux64-lgpl-9.0.tar.xz` (137.933.912 B, SHA-256
`7a484dae7fb08eaf6e69d8be466bc3557edc438ef2242aeaf7e72df68f1ce582`), que estaba
verificado pero dejaba a Linux con una licencia distinta de Windows y macOS.
El motivo y la auditoría del sustituto están en §10.6.

**macOS: fuente re-sustituida.** El artefacto antes fijado (evermeet
`ffmpeg-7.1.zip`) ya no se sirve: responde 302 → 403, y el índice de evermeet
solo ofrece `ffmpeg-9.0.2.zip`, cuya descarga directa también da 403 (hotlink)
incluso con User-Agent y Referer. Se adopta el build 9.0.2 para arm64 del build
server de Martin Riedl, con URL inmutable, sidecar `.sha256` y manifiestos de
códecs; justificación y alcance en §10.5.

**Hallazgo de reproducibilidad en Linux:** la URL de BtbN usa el alias
`latest`, que se republica en el mismo nombre. El hash verificado protege la
integridad (una descarga distinta falla), pero el mismo URL puede devolver otro
binario tras la siguiente publicación, así que la reproducibilidad no está
garantizada por la URL. Antes de distribuir hay que fijar una release inmutable
(tag `autobuild-…`) o espejar el artefacto ya verificado en los assets de AVI.
BtbN **no** publica macOS (solo Linux x64/arm64 y Windows x64/arm64, verificado
en los assets de la release).

### 10.2. Licencias y redistribución

| Target | Build | Licencia |
|---|---|---|
| Windows x64 | gyan `essentials` | **GPL v3** (`--enable-gpl --enable-version3`; libx264, libx265, libaom) |
| Linux x64 | BtbN `gpl` | **GPL v3** (`--enable-gpl --enable-version3`; libx264, libx265, libaom) |
| macOS arm64 | Riedl release 9.0.2 | **GPL v3** (`--enable-gpl --enable-version3`; libx264, libx265, libaom) |

**Licencia unificada: los tres targets son FFmpeg 9.0.2 GPL v3.** El pin de
Linux pasa de la variante LGPL a la GPL (§10.6) para que Windows, Linux y macOS
compartan versión, licencia y obligaciones; un solo juego de avisos y de oferta
de código fuente. Se aplica la decisión ya cerrada del plan (§8): GPL v3 es
aceptable como **agregación** (proceso externo, sin enlazar con el binario de
AVI), no como enlace.

Obligaciones que la Fase 3 debe materializar en los tres targets: entrada en
`NOTICE` y `THIRD-PARTY-LICENSES.md`, oferta de código fuente vía
`cargo xtask source-offer` (fuente de FFmpeg en la versión 9.0.2 más la
configuración y el script de compilación de cada proveedor), y mención en las
notas de release. El bundle no enlaza FFmpeg: lo ejecuta como proceso, por lo
que el resto de AVI sigue en MPL-2.0.

### 10.3. Auditoría de capacidad del binario (Windows, 9.0.2)

```
ffmpeg version 9.0.2-essentials_build-www.gyan.dev (gcc 16.2.0, MSYS2)
bin/ffmpeg.exe: 105.423.872 B · paquete extraído: 315 MB (ffmpeg, ffprobe, ffplay, doc)
```

- Encoders requeridos por `audio_codec_for`: `aac`, `flac`, `pcm_s16le`,
  `libmp3lame`, `libvorbis`, `libopus` — **todos presentes**.
- Decoders de la matriz de entrada: `mp3`, `mp3float`, `aac`, `flac`, `vorbis`,
  `opus`, `pcm_s16le`, `h264`, `mpeg4`, `vp8`, `vp9` — presentes.
- Muxers: `wav`, `mp3`, `ipod`, `adts`, `flac`, `ogg`, `opus`, `mp4`, `matroska`,
  `webm`, `avi`; demuxers: `mov,mp4,m4a,3gp,3g2,mj2`, `matroska,webm`, `avi`,
  `mp3`, `aac`, `flac`, `ogg`. La matriz de medios de Denoise queda cubierta sin
  añadir formatos.
- Filtros: `loudnorm`, `highpass`, `aresample` — presentes.

### 10.4. Carga desde bundle y ejecución end-to-end

Bundle emulado (`bundle/`: `denoise.exe` + `ffmpeg.exe`, 129 MB) ejecutado con
`env -i` y `PATH=/c/Windows/system32`, es decir **sin ningún ffmpeg en el PATH**:

| Caso (entrada) | Salida | exit |
|---|---|---|
| `--ffmpeg-path ./ffmpeg.exe -V` | `denoise 1.0.0 + modelo DPDFNet + ffmpeg version 9.0.2-essentials_build-www.gyan.dev` | 0 |
| `input.wav --preset studio` | `studio-wav.wav` | 0 |
| `input.mp3 --preset studio` | `studio-mp3.mp3` | 0 |
| `input.mp4 --preset studio` | `studio-mp4.mp4` | 0 |
| `input.wav` (default) | `default-wav.wav` | 0 |

Salidas del 9.0.2 empaquetado, medidas igual que las del 9.0.1 local:

| Salida | Códec / tasa | Duración | LUFS | True peak |
|---|---|---|---|---|
| `default-wav.wav` | pcm_s16le 48 kHz | 3,000 s | −24,8 | −8,6 dBTP |
| `studio-wav.wav` | pcm_s16le **192 kHz** | 3,000 s | −16,2 | −1,5 dBTP |
| `studio-mp3.mp3` | mp3 48 kHz | 3,000 s | −16,2 | −1,5 dBTP |
| `studio-mp4.mp4` | h264 + aac 48/96 kHz | 3,020 s | −16,0 | −1,5 dBTP |

**Paridad de comportamiento:** tasas, canales, duraciones y LUFS coinciden
exactamente con las capturas del 9.0.1 local; los tamaños de archivo también
(288.078 / 1.152.102 / 121.964 / 86.823 B). Los **hashes difieren** entre 9.0.1 y
9.0.2 (bitstreams de encoder distintos), así que la paridad se juzga por
propiedades y métricas, nunca por hash byte a byte entre builds de FFmpeg.

Esto confirma la receta `studio` del plan §8 sobre el binario que se distribuirá
(9.0.2) y reproduce los dos defectos de tasa ya registrados: WAV a 192 kHz y AAC
de vídeo a 96 kHz, ambos a corregir con `-ar 48000` en la Fase 3.

### 10.5. Decisión: fuente de FFmpeg para macOS arm64

Evidencia en `Denoise/out/fase0/ffmpeg-smoke/macos/` (gitignored): `manifest.txt`,
`codecs.txt`, `formats.txt`, `protocols.txt`, `versions.txt`, `ffmpeg.zip` y su
`.sha256`.

**Elegido:** build **9.0.2** del FFmpeg Build Server de Martin Riedl para
macOS arm64, formato `zip` (un único binario `ffmpeg`, 66.332.880 B).

| Dato | Valor |
|---|---|
| URL inmutable | `https://ffmpeg.martin-riedl.de/download/macos/arm64/1789931890_9.0.2/ffmpeg.zip` |
| Tamaño | 28.395.699 B |
| SHA-256 | `c8ed4c4e6978a03c485edbfe4e0a5dc2380f8a30bba5150531b31b094492d924` (sidecar publicado = descarga local) |
| Arquitectura | Mach-O 64-bit **arm64** (verificado con `file`) |
| Versión | `ffmpeg version 9.0.2` (creado 2026-09-20), Apple clang 14.0.0 |
| Licencia | GPL v3 (`--enable-gpl --enable-version3`) |

Por qué cumple el requisito de la puerta:

- **URL inmutable**: la ruta incluye el sello de tiempo y la versión
  (`1789931890_9.0.2`), no un alias `latest`; la misma URL no se republica.
- **Hash doble**: el sidecar `.sha256` del autor y el hash de la descarga local
  coinciden, y el tamaño queda registrado.
- **Capacidad auditable sin ejecutar el binario**: el build publica los
  manifiestos `codecs.txt`, `formats.txt`, `protocols.txt` y `versions.txt`.
  Verificado en ellos: encoders `aac` (nativo y `aac_at`), `libmp3lame`,
  `flac`, `pcm_s16le`, `libopus`, `libvorbis`; decoders `mp3`/`mp3float`, `aac`,
  `flac`, `vorbis`, `opus`, `h264`, `mpeg4`, `vp8`, `vp9`; muxers `wav`, `mp3`,
  `ipod`, `adts`, `flac`, `ogg`, `opus`, `mp4`, `mov`, `matroska`, `webm`,
  `avi`; demuxers `mov,mp4,m4a,3gp,3g2,mj2`, `matroska,webm`, `avi`, `mp3`,
  `aac`, `flac`, `ogg`. La matriz de medios de Denoise queda cubierta.
- **Misma versión que Windows** (9.0.2), lo que reduce divergencias de
  comportamiento entre targets; la paridad se juzga por propiedades y métricas,
  nunca por hash de salida.

Alternativas descartadas:

| Alternativa | Motivo |
|---|---|
| evermeet `ffmpeg-7.1.zip` (pin anterior) | Ya no se sirve: 302 → 403; el índice solo ofrece 9.0.2 y su descarga directa también da 403 |
| evermeet `ffmpeg-9.0.2.zip` | No descargable por HTTP (bloqueo de hotlink, también con UA y Referer); además no publica hash en el índice |
| BtbN | No publica macOS (solo Linux x64/arm64 y Windows x64/arm64) |
| Homebrew (bottle) | Enlaces dinámicos a decenas de dylibs; exigiría empaquetar el árbol de dependencias y no entrega un ejecutable autocontenido |
| Compilación propia en CI macOS | Máximo control, pero añade toolchain, tiempos y mantenimiento de build sin entorno macOS disponible para verificar ahora; queda como plan B si se rechaza un binario de terceros |

Riesgos y límites aceptados:

1. **Proveedor tercero** (build server personal, no oficial de FFmpeg), el mismo
   modelo de confianza que gyan.dev y BtbN ya aceptado en el plan §8: HTTPS +
   hash publicado + versión fijada. Aviso de procedencia en `NOTICE`.
2. **Ejecución no verificada**: no hay macOS en este entorno. Queda pendiente el
   smoke real (versión, `-map 0:v:0 -c:v copy`, códecs y pipeline completo),
   igual que en Linux.
3. **Dependencias dinámicas sin verificar**: el build declara
   `--pkg-config-flags=--static`, pero sin `otool -L` no se confirma que el
   binario no arrastre dylibs fuera de `/usr/lib`. Comprobarlo en el primer E2E
   macOS.
4. **Firma/notarización**: el binario no está firmado por AVI; el paquete de
   macOS tendrá que firmarlo/notarizarlo o resolverse con la política vigente de
   Gatekeeper en la Fase 3.

### 10.6. Linux x64: pin unificado a la variante GPL

Evidencia en `Denoise/out/fase0/ffmpeg-smoke/linux/` (gitignored): `manifest.txt`
y `LICENSE.txt` extraído del propio paquete.

| Dato | Valor |
|---|---|
| URL | `https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-n9.0-latest-linux64-gpl-9.0.tar.xz` |
| Tamaño | 151.204.652 B |
| SHA-256 | `3b4c62d62adbaf07cf5d8c8d9fdc4406de031c133e171c5ddff1bec49e1c843d` |
| Binario | `bin/ffmpeg`, 172.864.808 B, ELF 64-bit x86-64, dinámico contra glibc |
| glibc mínima | **2.28** (versión de símbolo más alta referenciada: `GLIBC_2.28`) |
| Contenido | `bin/` (ffmpeg, ffprobe, ffplay), `doc/`, `man/`, `presets/`, `LICENSE.txt` |
| Licencia | **GPL v3**: `LICENSE.txt` del paquete + tokens `--enable-gpl` y `--enable-version3` en el binario |

Capacidad verificada **sin ejecutar** el binario (los tokens de configuración
quedan embebidos y se leen con `grep -a`): `--enable-libmp3lame`,
`--enable-libvorbis`, `--enable-libopus`, `--enable-gpl`, `--enable-version3`,
`--enable-libx264`, `--enable-libx265`, `--enable-libaom`. Los códecs nativos
requeridos (`aac`, `flac`, `pcm_s16le`) forman parte del núcleo de FFmpeg y no
dependen de una bandera de configuración; la matriz de contenedores y códecs es
la misma versión 9.0.2 que en los otros dos targets.

Coste de la unificación, medido: el paquete pasa de 137.933.912 B (LGPL) a
151.204.652 B (GPL), es decir **+13.270.740 B**. El beneficio es una sola
licencia y un solo juego de avisos y obligaciones de fuente en los tres targets.
El binario depende de glibc en tiempo de ejecución (enlace dinámico del build de
BtbN, no una elección de AVI), con **glibc 2.28** como versión mínima de
símbolos.

Pendiente y no verificado: ejecución real en Linux (versión, matriz de códecs y
pipeline completo con `denoise`), igual que en macOS. El requisito de glibc
2.28 debe declararse en la documentación de distribución y comprobarse en el
primer E2E Linux (Ubuntu 20.04 o posterior).

## 11. Cobertura de la puerta

Cubierto con evidencia: flags y defaults, precedencia, naming y colisiones,
lotes/directorios/recursividad, JSONL y códigos de salida, matriz de medios,
egresión audio/vídeo, modelo y binding ORT, tests ignorados, estado de ambos
repositorios.

Pendiente y **no** presentado como verde:

1. Smoke de ejecución del FFmpeg empaquetado en **Linux x64** y **macOS arm64**:
   requiere el entorno nativo de cada SO. En Windows x64 está ejecutado (§10.4).
2. macOS arm64: fuente, hash y capacidad **resueltos** (§10.5); falta ejecución,
   `otool -L` (dependencias dinámicas) y firma/notarización del paquete.
3. Cancelación con Ctrl+C observada en ejecución (solo verificado por lectura de
   `cli.rs:563-573,846-850`).
4. Licencia (resuelta: los tres targets son FFmpeg 9.0.2 GPL v3, §10.2/§10.6):
   queda materializar los avisos y la oferta de fuente en la Fase 3.
5. Reproducibilidad del pin de Linux: la URL de BtbN usa el alias `latest`;
   fijar un tag inmutable o espejar el artefacto verificado en los assets de AVI.

Cerrado en este tramo: runtime ORT provisionado y con carga real verificada en
proceso y a nivel de módulo (§7.1); smoke de FFmpeg empaquetado en Windows x64
(§10.4), con hashes verificados de Windows y Linux (§10.1).
