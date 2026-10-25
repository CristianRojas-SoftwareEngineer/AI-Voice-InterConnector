# Recorrido de la migración de Denoise a AI-Voice-Interconnector

Registro final de la absorción del motor Denoise (DPDFNet + FFmpeg + preset `studio`) en el workspace Rust de AI-Voice-Interconnector (AVI). Documenta qué se hizo, en qué orden, con qué evidencia y qué queda abierto.

## Índice

- [1. Resultado en una frase](#1-resultado-en-una-frase)
- [2. Cronología por fase](#2-cronología-por-fase)
- [3. Cambios y contratos](#3-cambios-y-contratos)
- [4. Verificación ejecutada en esta revisión](#4-verificación-ejecutada-en-esta-revisión)
  - [4.1 DoD del repositorio de origen](#41-dod-del-repositorio-de-origen)
  - [4.2 DoD del workspace AVI](#42-dod-del-workspace-avi)
  - [4.3 CLI de archivos sobre el paquete instalado](#43-cli-de-archivos-sobre-el-paquete-instalado)
  - [4.4 Paso 11 — beneficio y límites en flujos](#44-paso-11--beneficio-y-límites-en-flujos)
  - [4.5 Seguridad de salida y cancelación](#45-seguridad-de-salida-y-cancelación)
- [5. Matriz requisito → implementación → evidencia → resultado](#5-matriz-requisito--implementación--evidencia--resultado)
- [6. Desviaciones frente al plan](#6-desviaciones-frente-al-plan)
- [7. Alternativas descartadas](#7-alternativas-descartadas)
- [8. Pruebas no ejecutadas o no concluyentes](#8-pruebas-no-ejecutadas-o-no-concluyentes)
- [9. Veredicto](#9-veredicto)

## 1. Resultado en una frase

La capacidad está migrada y funciona en Windows x64 sobre el paquete instalado; la migración **no se declara completa** porque la cancelación no interrumpe el trabajo en curso y porque los smokes nativos de Linux x64, Linux arm64 y macOS arm64 siguen sin ejecutarse.

## 2. Cronología por fase

| Fase | Alcance | Estado |
|---|---|---|
| 0 | Congelar contrato y medir el punto de partida: matriz de flags, precedencia, naming, JSONL, medios, modelo, FFmpeg | Cerrada; matriz en `.work/plans/denoise-fase0-matrix.md` |
| 1 | Trasplante funcional del crate, sin amputar el proyecto | Cerrada; bitácora en `.work/plans/denoise-fase1-log.md` |
| 2 | Homologación de autoridades: ORT único, DPDFNet en `avi-store`, remuestreo antialias, FFmpeg integrado | Cerrada en Windows; natividad Linux/macOS abierta |
| 3 | Producto: `speech denoise`, receta `studio`, opt-in en clonado, `/dub` y `/transcribe` | Implementada y probada; mediciones ampliadas en esta revisión |
| 4 | Verificación, distribución y entrega | Abierta para Linux/macOS; cerrada en Windows |

El paso que cambió la ruta pública fue `964970d5` (`feat(speech)!: ubicar denoise como subcomando único`): retiró `Commands::Denoise`, dejó `speech denoise` como único entrypoint y despacha directamente a `handle_denoise` para conservar el veredicto y el JSONL sin un segundo envelope.

## 3. Cambios y contratos

| Área | Contrato final |
|---|---|
| Entrypoint | `ai-voice-interconnector speech denoise <entrada>...`; el comando raíz `denoise` se rechaza |
| Features | `native-denoise` habilita la capacidad; `full` la incluye en los paquetes |
| Modelo | DPDFNet en el catálogo único `avi-store`, SHA256 verificado al descargar, reutilizar y en `doctor`; sin caché privada ni descarga en inferencia |
| FFmpeg | Integrado en el paquete, resuelto solo desde el ejecutable distribuido; `--ffmpeg-path` no existe |
| Preset | `studio` por omisión; `--preset default` reproduce el perfil original; flags individuales sobrescriben el preset |
| Vídeo | Se copia el primer stream de vídeo y se limpia el audio a AAC; la imagen no se procesa |
| Flujos internos | `--denoise` / `denoise: true` limpia **solo la entrada** de clonado, `speech dub` y `speech transcribe`; la salida TTS no recibe normalización ni recodificación |
| Daemon | Puerta de admisión: una inferencia activa, a lo sumo una esperando 30 s, el resto `denoise_busy` (HTTP 503) |
| Códigos | Paridad con el motor de origen: 2 entrada inválida o destino existente, 3 entrada ausente, 1 fallo de modelo/FFmpeg/E-S; `130` al recibir Ctrl+C |

## 4. Verificación ejecutada en esta revisión

Entorno: Windows x64, rama `feat/denoise-migration` sobre `964970d5` más las correcciones descritas en el apartado 6; ORT 1.28.0; DPDFNet y Parakeet v3 provisionados; FFmpeg 9.0.1 del sistema solo para conversiones auxiliares; FFmpeg 9.0.2 dentro del paquete.

### 4.1 DoD del repositorio de origen

Ejecutado en `C:\Users\Cristian\Desktop\Proyectos\Voices\Denoise`, sin editar sus archivos ni sus baselines:

| Comando | Resultado |
|---|---|
| `cargo build --release` | Correcto (27 s) |
| `cargo clippy --all-targets -- -D warnings` | Correcto, sin avisos |
| `cargo test --release` | Correcto; 7 + 1 + 14 + 2 + 16 + 12 + 5 pruebas en verde, 6 ignoradas |
| `cargo test --release -- --ignored` | 6/6 en verde: 1 de pipeline, 2 golden, 3 remux |
| `python tools/quality/benchmark.py run --set tests_data --work out/bench --profile all` | Ejecutado sobre los tres perfiles; **sin regresiones respecto a las baselines** `default`, `hq_speech` y `aggressive` |

### 4.2 DoD del workspace AVI

| Comprobación | Resultado |
|---|---|
| `cargo fmt --all --check` | Correcto |
| `cargo clippy --all-targets -- -D warnings` sin feature, con `native-denoise`, con `native-stt`, con ambas y con `full` | Correcto en las cinco combinaciones |
| `cargo test --release` en las mismas cinco combinaciones | Suites en verde; las pruebas de recursos quedan marcadas como ignoradas sin modelo o motor |
| `cargo test --test contract_conformance` | 8/8 tras añadir los `reason` de limpieza al contrato |
| `cargo test --release -p denoise --test test_golden --test test_remux --test test_audio_pipeline -- --ignored` | 6/6 en verde en esta revisión: golden de 3 s y de 65 s, tres de remux de vídeo y el pipeline E2E de audio |

Aviso de arnés: exportar `QWEN3_TTS_BIN` en el entorno hace fallar 12 pruebas unitarias de `avi-daemon` porque construyen un motor Qwen3-TTS real que valida la ruta del binario. No es un defecto del código: la suite propia del workspace pasa sin esas variables.

### 4.3 CLI de archivos sobre el paquete instalado

Paquete generado con `cargo xtask package --no-compress --flat`, instalado en un sandbox con `self install --no-setup --no-modify-path` y ejecutado desde su ruta instalada, fuera del repositorio y sin FFmpeg en `PATH`:

| Caso | Entrada | Resultado |
|---|---|---|
| Audio, `studio` implícito | `Speech Sampler.wav` (10 s) | Salida WAV PCM16 48 kHz mono de 10,00 s; 10,4 s de pared; JSONL con items y `summary` |
| Vídeo, `studio` implícito | MP4 vertical 1080×1920, 16,1 s | Salida MP4 con vídeo `h264` copiado y audio `aac` 48 kHz mono de 16,13 s (Δ 0,023 s, dentro de ±0,5 s) |
| Stream de imagen | mismo par | MD5 del stream de vídeo idéntico antes y después (`30f33506…cb3d1`) |
| `--preset default` | `Speech Sampler.wav` | WAV 48 kHz mono de 10,00 s, sin normalización |
| Colisión de destino | repetición sin `--overwrite` | Item `failed` con mensaje explícito, `summary.failed = 1`, salida 2 y destino intacto |
| Modelo ausente | `AVI_CACHE_DIR` vacía | Item `failed` con «El modelo dpdfnet8 no está provisionado», salida 1, sin archivo de salida |
| FFmpeg externo | `PATH` sin FFmpeg | Funciona: el paquete resuelve su propio binario |

### 4.4 Paso 11 — beneficio y límites en flujos

Detalle completo en `.work/plans/denoise-fase3-log.md`, apartado de la corrida de cierre.

| Flujo | Hallazgo |
|---|---|
| Transcripción, entrada limpia | El texto es idéntico con y sin limpieza en los cuatro clips limpios probados; la limpieza cuesta entre 3 y 13 s por archivo |
| Transcripción, entrada ruidosa | Efecto **mixto**: recupera una palabra en `car_snr0` («con» → «pon», ruta directa) y pierde contenido en `pub_snr0` y `office_snr0` (una frase completa en cada uno) |
| Clonación | Las cuatro combinaciones limpia/ruidosa y con/sin limpieza crean `reference.qvoice`; sobrecoste ≈ 2 s con referencia de 3 s |
| Doblaje | Texto idéntico en las cuatro combinaciones; el tiempo lo domina la síntesis (26-30 s para 3 s de audio) |
| Memoria | Daemon en reposo 959 MB; pico 1 140 MB durante un `/transcribe` con limpieza sobre 3 s (≈ +225 MB) |
| Concurrencia | 4 solicitudes simultáneas: 1 en curso (15,1 s), 1 en espera admitida (28,7 s), 2 con HTTP 503 `denoise_busy` en 90 ms |
| Límites | 30 s de referencia de clonado, 300 s de transcripción y 40 s de doblaje se rechazan con `audio_too_long` en ruta local y por daemon |

No se calculó WER numérico ni se midió identidad o naturalidad de la voz clonada: no hay en el workspace una métrica de locutor ni se hizo escucha ciega. Esas dos filas quedan como pendientes, no aprobadas.

### 4.5 Seguridad de salida y cancelación

- La verificación del archivo parcial ocurre antes de publicar y el destino preexistente queda intacto cuando la colisión o el modelo ausente impiden terminar.
- En las dos corridas medidas con `SIGINT` (vídeo y audio sobre el paquete instalado) el proceso **terminó el trabajo, publicó la salida completa y después salió con 130**. La señal no interrumpe el DSP en curso y el código final sugiere una cancelación que en realidad no ocurrió.

## 5. Matriz requisito → implementación → evidencia → resultado

| Requisito | Implementación en AVI | Evidencia | Resultado |
|---|---|---|---|
| DSP y perfiles | `crates/denoise` trasplantado, ORT único | Diferencial contra el origen (SI-SDR 101,94 dB en `default`), 6 pruebas ignoradas en verde, benchmark sin regresiones | Aprobado en Windows |
| `studio` y overrides | `preset.rs` con receta AVI, 48 kHz al normalizar, bitrate por códec | E2E instalado de audio y vídeo; rechazo de bitrate incompatible (código 2) | Aprobado en Windows |
| Audio por archivos | `pipeline.rs` + `ffmpeg_io.rs` | WAV, OGG, M4A y FLAC con el FFmpeg del bundle; colisión y modelo ausente en el instalado | Aprobado en Windows |
| Vídeo por archivos | Remux con `-c:v copy` y AAC | MP4 real 1080×1920 con stream de imagen idéntico y Δ 0,023 s | Aprobado en Windows |
| Lotes y CLI | `speech denoise`, JSONL sin envelope extra, rechazo de la raíz | `cli_golden` en verde; ayuda y rechazo de raíz comprobados en release e instalado | Aprobado en Windows |
| Provisión y distribución | DPDFNet en `avi-store`, FFmpeg fijado por target | `setup`/`doctor`/`cleanup`; pin inmutable de BtbN; avisos y oferta de fuente materializados | Parcial: solo Windows ejecutado |
| Clonado, `/dub`, `/transcribe` | Etapa DSP compartida, opt-in por solicitud | Textos y latencias comparados; tres pruebas de contrato | Parcial: falta identidad/naturalidad y WER |
| Cancelación | Handler de `Ctrl+C` con bandera compartida | `SIGINT` a los 1 s y a los 3 s: el trabajo se completa y se publica | **No aprobado** |

## 6. Desviaciones frente al plan

1. **El plan original no contenía la prescripción de firma/notarización macOS** que la tarea 1 iba a sustituir. Se registró el criterio de cierre macOS en el seguimiento del plan (ejecución nativa, `otool -L`, firma diferida) en lugar de editar un texto inexistente; las bitácoras históricas quedaron intactas.
2. **No se creó la CI transitoria de rama** (tareas 4 y 5). Ejecutarla exige empujar la rama y leer las corridas de CircleCI, y esta sesión no tenía autorización de push. Añadir el workflow sin poder ejecutarlo habría dejado un cambio no verificado que la tarea 8 retiraría de todos modos; queda como bloqueo explícito, no como puerta aprobada.
3. **Tres correcciones de código no planificadas** — temporales del clonado, `denoise` no propagado en la ruta del daemon y avisos de código muerto — surgieron al ejecutar las puertas de la tarea 2 y se corrigieron por mandato del propio plan («limitar correcciones a fallos reproducidos por estas puertas»).
4. **Se corrigió la documentación de códigos de salida** de `speech denoise`: describía 4/6/130 donde el motor conserva 1/2/3/130 por paridad con el origen.

## 7. Alternativas descartadas

- ORT enlazado estáticamente: no enlaza con `+crt-static` en Windows (71 símbolos `__imp_*`).
- Caché privada del modelo o `--model-dir`: duplicaría la autoridad de `avi-store` y permitiría descargas silenciosas.
- `--ffmpeg-path` o resolución por `PATH`: rompe la garantía de que el binario distribuido es el que se ejecuta.
- Cambiar `.cargo/config.toml` para que `ORT_DYLIB_PATH` nombre el archivo: rompería la resolución compartida y a los demás targets.
- Dos pasadas de `loudnorm`: acercan el objetivo de sonoridad, pero bajan PESQ en varios clips.
- Enviar denoise del daemon como subproceso del CLI original: duplicaría el motor y el envelope.

## 8. Pruebas no ejecutadas o no concluyentes

| Elemento | Estado |
|---|---|
| Smoke nativo de FFmpeg y paquete en Linux x64, Linux arm64 y macOS arm64 | **No ejecutado**: requiere runner nativo y una rama empujada |
| `otool -L`, arquitectura y dependencias dinámicas en macOS | **No ejecutado** |
| Firma y notarización macOS | Diferidas por política de producto; no bloquean el cierre |
| WER numérico | No calculado; la comparación es texto a texto |
| Identidad y naturalidad de la voz clonada | No medidas: sin métrica de locutor ni escucha ciega |
| Pico de memoria con varias peticiones concurrentes | Solo medido con una inferencia activa |
| Cancelación efectiva del DSP en curso | **Falla**: la señal no interrumpe el trabajo |
| Pruebas marcadas `#[ignore]` en AVI | Ejecutadas las que tienen modelo y motor; las de matriz de voces y audio de dispositivo quedan fuera |

## 9. Veredicto

**La migración está implementada y funciona en Windows x64 sobre el paquete instalado; no se declara completa.** Faltan dos puertas:

1. **Cancelación**: `Ctrl+C` devuelve 130 después de completar y publicar el archivo. La siguiente prueba concreta es instrumentar la bandera de cancelación entre etapas del pipeline y verificar que un `SIGINT` durante el DSP deja el destino sin crear o intacto.
2. **Natividad de los otros tres targets**: los smokes de FFmpeg, paquete, instalación y CLI en Linux x64, Linux arm64 y macOS arm64 no se ejecutaron. La siguiente prueba concreta es empujar la rama con un workflow temporal filtrado a ella, correr los cuatro jobs sin modelo, guardar enlaces y números de pipeline, comprobar `otool -L` en macOS y retirar el andamiaje antes de integrar.

El resto de criterios de la matriz de aceptación tiene evidencia positiva en Windows, y la paridad DSP con el repositorio de origen está medida sin regresiones de baseline. Ningún target sin E2E con modelo se presenta como funcionalmente verificado.
