# Optimización de la síntesis Qwen3-TTS 0.6B en CPU: validación y recomendación

**Fecha:** noviembre de 2026.  
**Modelo:** `Qwen3-TTS-12Hz-0.6B-Base` (clonación de voz con `--graft`, muestreo de producción `temperature=0.35`, `seed=4`).  
**Equipo medido:** AMD Ryzen 7 5825U (8 núcleos, 16 hilos, AVX2/FMA, sin AVX-512 ni VNNI), 31,4 GB de RAM, Windows 11, plan de energía «Equilibrado», conectado a la corriente.  
**Alcance:** campaña histórica de medición controlada sobre el motor C y su ruta de servidor. Sus resultados y recomendación de seis hilos se conservan como evidencia histórica; la integración posterior usa cuatro hilos y se detalla a continuación.

## Estado de integración posterior — 8 de noviembre de 2026

El código en desarrollo selecciona por defecto `mixed` (Talker INT4, Code Predictor INT8, `-j 4`) y permite volver a BF16 para ambos mediante `AVI_TTS_PRECISION=bf16` al crear el motor; el daemon debe reiniciarse para cambiar de perfil. No se ha publicado ni desplegado. La adopción no está completada: la comparación integral y la escucha humana de ocho pares aún no están verificadas. La prueba acotada no sustituye a la campaña histórica ni demuestra equivalencia perceptiva.

El motor local probado tiene SHA-256 `b153d167057842559323561b6893bb2e11218368e98929a8c4f92e5b59d85620`, reconoce `--quant-mixed`, informa AVX2/FMA y OpenBLAS y superó `--self-test`; no se ha establecido equivalencia con el binario histórico `eb81d296…`. Los snapshots requeridos estaban presentes con sus archivos no vacíos, sin verificación criptográfica. La voz instalada y el asset del test coincidían en SHA-256 `02488c75bbcfa749b49347ad81616f171e59bea4aca04503b54a773805e84c86`. La prueba de recursos `synthesize_short_text_word_error_rate_gate` con `mixed` falló inicialmente antes de calcular WER; en una repetición diagnóstica pasó exactamente un caso. El primer fallo no mostró su diagnóstico JSON y su causa sigue desconocida. No se infiere estabilidad a partir de una única repetición exitosa.

Las cifras históricas del servidor para cuatro hilos son 1,84× RTF en BF16 y 1,38× en mixto, aproximadamente 25 % menos tiempo por petición en este equipo. La cifra histórica de 1,22× y la reducción cercana al 36 % en servidor corresponden a seis hilos, no a esta integración. Los resultados de cuatro hilos de calidad inicial incumplieron las reglas predefinidas; la confirmación ampliada y la escucha histórica son de seis hilos y no acreditan calidad del nuevo perfil de cuatro hilos ni de los presets.

Las suites aisladas posteriores a la revisión ejecutaron `avi-tts` (45 aprobadas, 2 ignoradas), `avi-daemon` (82 aprobadas entre unitarias y golden, 5 ignoradas) y `cli_golden` (157 aprobadas, 22 ignoradas). Las omitidas necesitan recursos de inferencia o un dispositivo de audio y no cuentan como aprobadas. La compilación local `cargo build --release --features full --offline` terminó correctamente. La definición de CircleCI compila el motor con autoprueba o restaura la caché y empaqueta por plataforma; inspeccionar esa definición no verifica los binarios publicados en otras plataformas.

Con el release local y una raíz aislada se sintetizó `default` en español e inglés, tanto en directo como mediante daemon `mixed`; los logs de la ruta directa registraron cuantización Talker Q4, CP INT8 y servidor escuchando. El primer arranque del daemon falló porque `target/release` no contenía ONNX Runtime; al apuntar `ORT_DYLIB_PATH` a la biblioteca instalada, el daemon arrancó, alcanzó `warm` y fue detenido mediante su propia raíz y puerto. El endpoint separado del motor `/v1/tts/stream` devolvió HTTP 200, `audio/pcm`, s16le mono a 24 kHz y 49 920 bytes, con tres recepciones de datos de audio en la traza. Esto no convierte el NDJSON del daemon en streaming PCM. Evidencias temporales: `C:\Users\Cristian\AppData\Local\Temp\opencode\quant-e2e-29d418d981464c9fb8b30f2c84930cf6`.

Se generaron los ocho pares previstos: dos perfiles, rutas directa y daemon, `default` y `ryan`, y las frases española e inglesa del plan. Los 16 WAV tienen formato RIFF PCM16 mono a 24 kHz y bloque `data` no vacío; los logs de cada muestra confirman Talker INT4 y CP INT8 en `mixed`, y BF16 para ambos sin cuantización en `bf16`. Para cada muestra caliente del daemon se calentó primero la misma voz y se comprobó el mismo PID antes y después. Los dos daemons aislados finalizaron y no quedó `daemon.pid`. `OPENBLAS_NUM_THREADS` y `OMP_NUM_THREADS` estaban ausentes; la DLL ONNX instalada tiene SHA-256 `18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257`.

| Ruta | Perfil | Mediana del tiempo de CLI / duración WAV | Muestras |
|---|---|---:|---:|
| Directa, incluye carga | mixed | 3,915 | 4 |
| Directa, incluye carga | bf16 | 3,856 | 4 |
| Daemon, misma voz caliente | mixed | 1,448 | 4 |
| Daemon, misma voz caliente | bf16 | 1,878 | 4 |

Los valores son mediciones pequeñas de la CLI completa, no el RTF histórico del handler del motor ni una garantía de rendimiento. Cada tiempo, duración calculada desde el bloque RIFF `data`, voz, idioma, ruta, perfil, PID y WAV está en `evidencias/mediciones.jsonl`; los logs por muestra están junto a ese archivo. La escucha de los ocho pares, la inteligibilidad y la ausencia de cortes o silencios anómalos siguen pendientes de evaluación humana; por tanto la puerta de calidad no está aprobada.

Se evaluaron programáticamente los 16 archivos WAV mediante transcripción STT con Parakeet TDT v3 (`evidencias/wer-pares-20261108.json`): todas las muestras presentaron cero recortes digitales y formato RIFF PCM16 mono a 24 kHz. En 14 de las 16 muestras el WER fue 0,00 (reproducción fonética íntegra). La voz clonada `default` obtuvo WER 0,00 en español e inglés por ambas rutas; `ryan` en español obtuvo WER 0,00 en ambas rutas. En cambio, `ryan` en inglés con `mixed` truncó la palabra final «test» (transcribiendo «Hello. This is» / «Hello, this is a» con WER 0,40), mientras que con `bf16` transcribió la frase completa sin errores («Hello, this is a test.», WER 0,00). Esto confirma la sensibilidad diferencial del modelo CustomVoice y la necesidad de mantener el perfil reversible `AVI_TTS_PRECISION=bf16`.

Se inició un daemon aislado con `AVI_TTS_PRECISION=bf16`; una solicitud posterior desde un cliente con `AVI_TTS_PRECISION=invalid` se completó usando ese daemon y el mismo residente (PID 3364 en esa prueba). La variable del cliente no cambia ni valida el perfil del daemon ya iniciado. En cambio, `speech dub --no-daemon` transcribió primero el audio y, al crear su propio motor con el valor inválido, devolvió `usage_error` y salida 2, antes de sintetizar o reproducir. Si falla una validación o etapa anterior a la creación del motor, se informa ese fallo primero.

## Tabla de contenidos

- [Estado de integración posterior](#estado-de-integración-posterior--8-de-noviembre-de-2026)
- [1. Resumen y recomendación](#1-resumen-y-recomendación)
- [2. Entorno, trazabilidad y metodología](#2-entorno-trazabilidad-y-metodología)
  - [2.1. Binarios, modelo y datos](#21-binarios-modelo-y-datos)
  - [2.2. Corpus y voces](#22-corpus-y-voces)
  - [2.3. Diseño experimental](#23-diseño-experimental)
  - [2.4. Métricas](#24-métricas)
  - [2.5. Criterio de calidad y márgenes](#25-criterio-de-calidad-y-márgenes)
- [3. Resultados](#3-resultados)
  - [3.1. Calibración del baseline](#31-calibración-del-baseline)
  - [3.2. Velocidad de cada factor aislado](#32-velocidad-de-cada-factor-aislado)
  - [3.3. Calidad de las candidatas](#33-calidad-de-las-candidatas)
  - [3.4. Truncamiento de codebooks](#34-truncamiento-de-codebooks)
  - [3.5. Confirmación del perfil candidato](#35-confirmación-del-perfil-candidato)
  - [3.6. Escucha ciega](#36-escucha-ciega)
  - [3.7. Ruta real: servidor y daemon](#37-ruta-real-servidor-y-daemon)
  - [3.8. Correctitud de los binarios](#38-correctitud-de-los-binarios)
- [4. Decisión por optimización](#4-decisión-por-optimización)
- [5. Perfil recomendado](#5-perfil-recomendado)
- [6. Requisitos de integración](#6-requisitos-de-integración)
- [7. Límites de la evidencia](#7-límites-de-la-evidencia)

## 1. Resumen y recomendación

Esta sección conserva la recomendación formulada al finalizar la campaña, no el perfil implementado posteriormente. La síntesis era el cuello de botella del pipeline de voz: con la receta medida (BF16, 4 hilos) el motor tarda 1,7 a 1,8 veces la duración del audio.

**Recomendación: adoptar la cuantización mixta (`--quant-mixed`) con 6 hilos (`-j 6`) en el motor residente, manteniendo BF16 con 4 hilos como perfil de referencia reversible.** No requiere parches al motor ni variables de entorno: son dos argumentos del lanzamiento.

| Medida | BF16, 4 hilos (referencia histórica) | `--quant-mixed -j 6` |
|---|---|---|
| Tiempo de síntesis en CLI (100 elementos) | 1,00 | 0,600 [0,584–0,624] |
| Tiempo por petición en el servidor | 1,00 | 0,643 [0,612–0,682] |
| RTF por petición en el servidor (mediana) | 1,84 | 1,22 |
| Primera lectura en streaming (mediana) | 2,8 s | 2,0 s |
| Memoria máxima del motor | 2,51 GB | 2,88 GB |
| Frase media `es_m1` (servidor, mediana) | 14,2 s | 9,3 s |

Lo que sustenta que la calidad se conserva en la medida en que se pudo verificar:

- La identidad de la voz cambia en torno al 3 % de la distancia que separa dos hablantes distintas, medida con dos extractores independientes.
- El WER agregado sube 0,36 puntos porcentuales (límite fijado: 1,0) en 350 síntesis.
- En una comparación a ciegas por pares, un oyente no distinguió esta configuración de BF16: 5 preferencias para BF16, 5 para la candidata y 2 empates, frente a 4, 6 y 2 en el control BF16 contra BF16 con otra semilla (p = 0,5).

Lo que no consigue y debe conocerse antes de adoptarla:

- **No alcanza tiempo real.** El RTF queda en 1,2 a 1,3: sintetizar sigue tardando más que reproducir. En streaming habrá pausas, aunque menos que con BF16.
- **Con las reglas de calidad fijadas de antemano, la candidata no pasa.** Incumple el margen de identidad de ECAPA y la regla por elemento del WER, dominada por textos con cifras. Los márgenes eran más estrictos que la diferencia que el oyente puede percibir (sección 2.5). La decisión descansa en la escucha, no en el cumplimiento de esas reglas.
- **Salidas anómalas.** 2 de 350 síntesis duraron más del doble de lo normal, frente a 0 de 525 con BF16. La diferencia no es estadísticamente significativa (p = 0,16), pero la cota no descarta una tasa cercana al 2 %.

Las demás optimizaciones se descartan: el truncamiento de codebooks acelera hasta un 20 % pero degrada la pronunciación y la identidad, y uno de sus niveles no termina de sintetizar en 1 de cada 10 casos; el resto de factores no acelera en este equipo.

## 2. Entorno, trazabilidad y metodología

### 2.1. Binarios, modelo y datos

Todas las cifras se derivan de archivos de la campaña registrados con su `sha256` en un manifiesto: binarios, parches, fragmentos de datos, voces, resultados por síntesis y tablas de análisis. Los archivos de análisis que sustentan las cifras son:

| Tema | Archivo de análisis |
|---|---|
| Márgenes y escala entre hablantes | `analysis/margenes.json` |
| Puertas de correctitud | `analysis/gates.json`, `analysis/gateF_detail.json` |
| Velocidad y calidad del cribado | `analysis/screening/verdicts.json`, `analysis/screening_tiempos.json`, `analysis/screening_calidad.json` |
| Confirmación del perfil | `analysis/finalists_calidad.json` |
| Escucha ciega | `analysis/mushra/verdict.json` |
| Ruta del servidor y daemon | `analysis/route/summary.json`, `analysis/route/wer_servidor.json` |

- **Binario de producción:** `qwen_tts.exe`, `sha256` `eb81d296…`. El binario compilado localmente desde `vendor/qwen3-tts` (`blas`, `SIMD=auto`, MSYS2 UCRT64, GCC 16.2.0, OpenBLAS 0.3.34) produce WAV idénticos byte a byte en 10 de 10 elementos de comprobación.
- **Modelo:** snapshot `5d83992436eae1d760afd27aff78a71d676296fc`. **ASR:** Parakeet TDT v3, snapshot `8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce`. **Extractor de identidad independiente:** `microsoft/wavlm-base-plus-sv`, snapshot `feb593a6c23c1cc3d9510425c29b0a14d2b07b1e`.
- **Parches evaluados** (copias del motor fuera del repositorio): truncamiento de codebooks (`QWEN_CP_MAX_CB`), proyecciones QKV int8 fusionadas (`QWEN_FUSED_INT8_QKV`) y pool de hilos Win32 reescrito (`QWEN_WIN_SPIN`). Ninguno se adopta.

### 2.2. Corpus y voces

- 35 textos: 25 en español (5 cortos, 8 medios, 6 largos, 3 con cifras, 3 con nombres propios o siglas; 4 interrogativos o exclamativos) y 10 en inglés (3 cortos, 4 medios, 2 largos, 1 con cifras).
- Subconjunto de cribado: 10 textos (7 en español, 3 en inglés).
- 5 voces: la del usuario (`default`, sin grabación de referencia conservada) y 4 clonadas de hablantes reales del conjunto de datos de voces de Chile y Argentina (SLR61, CC BY-SA 4.0), con unos 18 a 22 s de referencia cada una.

### 2.3. Diseño experimental

- **Un factor por vez:** cada condición cambia un solo factor respecto a su referencia.
- **Orden intercalado:** por elemento (texto, voz, semilla) las condiciones se ejecutan en un orden aleatorio con semilla fija, para neutralizar la deriva temporal.
- **Un proceso por síntesis**, con el daemon detenido, el equipo enchufado y el entorno de cada ejecución limpio de variables `QWEN_*` ajenas.
- **Tiempo:** `T_synth_ms` es la suma de `Prefill`, `Talker step`, `Code Predictor`, `Embed`, `Codec head+sampling` y el `drain` del decoder, extraída del resumen del motor.
- **Estadística:** razón de tiempo emparejada por elemento, mediana con IC 95 % por bootstrap sobre unidades (texto, voz).
- **Reglas de decisión de velocidad:** un factor es favorable si el extremo superior del IC 95 % de su razón de tiempo es menor que 0,97.

### 2.4. Métricas

- **WER:** transcripción con Parakeet, minúsculas, sin diacríticos, tokens `\w+`.
- **Identidad:** similitud coseno entre embeddings de ECAPA (motor) y de WavLM-SV. ECAPA separa mal hablantes distintas; WavLM las separa mejor.
- **F0 y duración:** media y desviación de F0 en semitonos (`librosa.pyin`, 60–400 Hz) y duración del WAV.
- **Validación del harness:** reproduce la similitud ECAPA de la referencia (0,9919), la similitud WavLM entre dos configuraciones previas (0,9796), el WER nulo de la frase de referencia y un WAV idéntico al de producción con la semilla 4.

### 2.5. Criterio de calidad y márgenes

No hay umbrales absolutos. Los márgenes se fijaron antes de evaluar ninguna optimización, a partir del ruido entre semillas del propio baseline (BF16, 4 hilos, 525 síntesis: 35 textos × 5 voces × semillas 4, 7 y 11):

- **Identidad frente al baseline:** el extremo inferior del IC 95 % de la diferencia entre la similitud condición–baseline y la mediana del ruido debe ser ≥ −(mitad del rango intercuartílico del ruido).
- **Identidad frente a la referencia:** la misma diferencia frente a la referencia debe ser ≥ −P95 de la variación entre semillas.
- **WER:** aumento agregado ≤ 1 punto y ningún elemento con más de 2 palabras erradas que el baseline.
- **Duración y F0:** diferencias medianas dentro del P95 del ruido.
- **Veredicto:** `pasa` si se cumplen en ambos extractores; `falla` si alguna se incumple en ambos o en WER, duración o F0; `dudoso` si los extractores discrepan.

El ruido es muy pequeño en ECAPA (rango intercuartílico 0,0017), por lo que su margen de identidad es de ±0,0009. Además, el WER por elemento es sensible al azar: el 1,6 % de los pares de semillas distintas del propio baseline ya supera las 2 palabras de diferencia. Por eso un incumplimiento estricto no equivale a una degradación audible, y la escucha ciega actúa como árbitro.

## 3. Resultados

### 3.1. Calibración del baseline

| Magnitud | ECAPA | WavLM |
|---|---|---|
| Similitud entre semillas, mismo texto y voz (mediana) | 0,9969 | 0,9881 |
| Rango intercuartílico del ruido | 0,0017 | 0,0128 |
| Salidas cruzadas entre voces distintas (mediana) | 0,9416 | 0,7466 |
| Salida frente a su propia referencia (mediana) | 0,9865 | 0,9653 |

- WER del baseline: 2,53 %, concentrado en los textos con cifras.
- La dispersión de `T_synth_ms` entre semillas tiene un coeficiente de variación mediano de 3,8 %.

### 3.2. Velocidad de cada factor aislado

Razón de `T_synth_ms` frente a QM (`--quant-mixed`, 4 hilos), 100 elementos (10 textos × 5 voces × semillas 4 y 7) salvo indicación. QM frente a BF16: 0,643 [0,627–0,664], es decir, 36 % menos.

| Factor | Razón [IC 95 %] | ¿Favorable? |
|---|---|---|
| Truncar a K=12 codebooks | 0,866 [0,850–0,885] | Sí |
| Truncar a K=10 | 0,817 [0,799–0,854] | Sí |
| Truncar a K=8 | 0,798 [0,759–0,846] | Sí (con falla de terminación, ver 3.4) |
| 6 hilos | 0,935 [0,914–0,961] | Sí |
| Pool Win32 con espera activa (`QWEN_WIN_SPIN=4096`) | 0,979 [0,966–0,985] | No |
| Prefill del Talker con `matmat_bf16` (n=42) | 0,988 [0,943–1,013] | No |
| Prefill B=2 del Code Predictor (n=41) | 1,005 [1,003–1,012] | No |
| QKV int8 fusionado (n=33) | 0,996 [0,995–0,999] | No |
| Pool Win32 sin espera activa (n=33) | 0,999 [0,997–1,002] | No |
| Precisión asimétrica por capa del Code Predictor (n=33) | 1,042 | No (más lenta) |
| 2 hilos BLAS en generación (n=33) | 1,026 [1,021–1,034] | No (más lenta) |
| 8 hilos (n=33) | 0,953 [0,947–0,964] | Dominado por 6 hilos |
| 6 hilos con 2 hilos BLAS (n=33) | 0,959 | Dominado por 6 hilos |

Los factores medidos con 33 elementos se descartaron con esa muestra porque sus intervalos eran estrechos y quedaban en 1,0 o por encima. El prefill B=2 y el prefill con `matmat_bf16` están pensados para CPU con AVX-512/VNNI, que este equipo no tiene; su ausencia de efecto aquí no implica ausencia de efecto en otro procesador.

### 3.3. Calidad de las candidatas

Frente a BF16 con la misma semilla y elemento (100 elementos). «Brecha» es la fracción de la distancia entre el ruido y una hablante distinta que representa el cambio de identidad.

| Condición | ECAPA: dif. vs baseline (brecha) | WavLM: dif. vs baseline (brecha) | WER: aumento / peor elemento | Veredicto con reglas fijadas |
|---|---|---|---|---|
| QM | −0,0018 (3,3 %) | −0,0063 (2,6 %) | +0,82 pp / +6 palabras | falla |
| QM + 6 hilos | −0,0018 (3,2 %) | −0,0059 (2,4 %) | +0,37 pp / +3 | falla |
| QM + K12 | −0,0063 (11,4 %) | −0,0146 (6,1 %) | +1,55 pp / +15 | falla |
| QM + K10 | −0,0126 (22,8 %) | −0,0216 (8,9 %) | +2,78 pp / +26 | falla |
| QM + K8 | −0,0268 (48,4 %) | −0,0479 (19,8 %) | +9,85 pp / +54 | falla |

- Cambiar a 6 hilos equivale a cambiar de semilla: la similitud entre QM y QM + 6 hilos (ECAPA 0,997; WavLM 0,987) coincide con la del ruido entre semillas del baseline.
- El deterioro por truncamiento crece de forma monótona al bajar K.

### 3.4. Truncamiento de codebooks

Se descarta en los tres niveles:

- K12 y K10 superan el límite de WER (+1,55 y +2,78 puntos frente a 1,0) y desplazan la identidad entre el 6 % y el 23 % de la brecha.
- K8 no termina de sintetizar en una fracción relevante de casos: 10 de 100 salidas (9 de 99 completadas y 1 que agotó 600 s sin emitir fin de secuencia) duran más del doble de la mediana del baseline, con un máximo de 12,9 veces. QM, K12, K10 y 6 hilos registran 1 o 2 de cada 100; BF16, ninguna de 525.
- En la primera sesión de escucha (puntuación de 0 a 100 frente a una referencia), K12 obtuvo una mediana de 75, con dos ensayos que cayeron 50 y 75 puntos; el perfil sin truncamiento obtuvo 85. El ensayo más bajo de K12 usa la voz `default` y la frase del informe previo, y las métricas automáticas no lo detectaron (similitud WavLM 0,974, WER sin cambio).

La descomposición de la pérdida entre decoder y trayectoria del Talker no se midió: con K descartado no cambia la decisión.

### 3.5. Confirmación del perfil candidato

QM + 6 hilos en 350 síntesis (los 10 textos del cribado más los otros 25 textos × 5 voces × 2 semillas) frente a BF16 con el mismo elemento:

| Magnitud | Resultado |
|---|---|
| Razón de tiempo | 0,612 [0,604–0,623] |
| Identidad ECAPA | −0,0019 [−0,0022; −0,0017]; límite −0,0009: incumple |
| Identidad WavLM | −0,0037 [−0,0060; −0,0022]; límite −0,0064: cumple |
| WER agregado | +0,36 pp (cumple); peor elemento +16 palabras (incumple) |
| Duración, F0 | dentro del P95 del ruido |
| Salidas con más del doble de fotogramas | 2 de 350 (ambas `es_p1`) frente a 0 de 525; Fisher unilateral p = 0,16 |

- **Peor elemento de WER:** el caso de +16 palabras es un texto con cifras en inglés. La transcripción de BF16 escribe «$1250» y la de la candidata escribe los números en palabras; los peores casos son todos textos con cifras, donde el ASR varía. No se pudo determinar si hay un error real de pronunciación.
- **Las dos salidas largas** transcriben el texto completo y correcto, y duran el doble, probablemente por silencios largos (no medido).

Los extractores discrepan (WavLM cumple, ECAPA no): es el caso `dudoso` que el criterio manda arbitrar con escucha.

### 3.6. Escucha ciega

Un solo oyente. Dos sesiones:

1. **Puntuación de 0 a 100 frente a una referencia** (12 ensayos, 5 voces, textos cortos y medios, referencia oculta y ancla filtrada a 3,5 kHz). La sesión es válida (referencia oculta 100 en 12 de 12; ancla 0 en 12 de 12). Mediana frente a la referencia: 85 para QM + 6 hilos y 75 para QM + K12. Este diseño se juzgó poco discriminante: mezcla el parecido de la frase con su calidad y no controla el efecto de re-muestrear.
2. **Preferencia forzada por pares, con control** (24 pares: 12 de BF16 contra QM + 6 hilos y 12 de BF16 contra BF16 con otra semilla). Resultado:

| Par | Gana BF16 (semilla 4) | Gana el otro | Sin diferencia |
|---|---|---|---|
| BF16 contra QM + 6 hilos | 5 | 5 | 2 |
| Control: BF16 contra BF16 (semilla 7) | 4 | 6 | 2 |

La regla fijada de antemano (BF16 gana más en la prueba que en el control, Fisher unilateral p < 0,10) no se cumple: p = 0,5. No se detectó degradación audible. La prueba solo detectaría diferencias grandes (10 pares decisivos por brazo y un oyente).

### 3.7. Ruta real: servidor y daemon

Medida con los argumentos del lanzamiento residente (`-d`, `--serve`, `--host`, `-j`, `--stream`, `--watch-stdin`, `--load-voice`, `--graft`), voz `default`, 10 textos, 5 peticiones consecutivas por `/v1/tts` y 1 por `/v1/tts/stream` por texto, texto enviado en UTF-8.

| Magnitud | BF16, 4 hilos | QM, 4 hilos | QM, 6 hilos |
|---|---|---|---|
| Tiempo por petición frente a BF16 | 1,00 | 0,748 [0,709–0,785] | 0,643 [0,612–0,682] |
| RTF por petición (mediana) | 1,84 | 1,38 | 1,22 |
| RTF en streaming (mediana) | 1,82 | 1,37 | 1,20 |
| Primera lectura en streaming (mediana) | 2,83 s | 2,34 s | 2,03 s |
| Lecturas de ≤8 KiB que llegan con el audio ya agotado (media por stream) | 45,3 | 40,0 | 25,4 |
| Memoria máxima del proceso | 2,51 GB | 2,88 GB | 2,88 GB |

- **Los 6 hilos sí ayudan en el servidor:** 0,869 [0,852–0,901] frente a QM con 4 hilos.
- **Tiempos por texto** (mediana en servidor, BF16 / QM / QM + 6 hilos): frase corta de 4 s de audio, 6,9 / 5,6 / 4,4 s; frase media, 14,2 / 10,3 / 9,3 s; texto largo en español, 39,7 / 30,2 / 26,8 s.
- **Sin truncamientos en el servidor:** WER de las respuestas 4,08 % (BF16), 2,20 % (QM) y 2,04 % (QM + 6 hilos); ninguna con menos del 80 % de las palabras esperadas.
- **Carga continua de 10 minutos:** sin errores ni deriva del RTF (pendiente 0,00 % y +0,01 % por minuto); memoria estable en 2,5 y 2,9 GB. Esta medición se hizo con texto escapado y solo se interpreta como deriva relativa.
- **Estado entre peticiones:** en las cinco peticiones consecutivas con el mismo texto y semilla, la primera difiere de las otras cuatro en 27 de 30 combinaciones de configuración y texto, y las peticiones 2 a 5 son idénticas en las 30. Ocurre también con BF16: la salida depende de la petición anterior, y no es consecuencia exclusiva de la optimización. La ejecución directa observada en esa campaña inició un proceso por síntesis; estas mediciones no garantizan determinismo general de la CLI.
- **Sobrecoste del daemon:** `speech synthesize --daemon` añade una mediana de 73 ms (rango 31–128 ms, 12 mediciones) al tiempo del motor.
- **Texto con acentos:** el camino del producto transcribe correctamente textos con acentos («Buenos días, ¿cómo estás hoy?»: 0 errores en 5 palabras).

### 3.8. Correctitud de los binarios

- Los cuatro binarios compilados (base, truncamiento, QKV fusionado, pool) superan `--self-test`.
- El binario base local es idéntico a producción en 10 de 10 elementos.
- **QKV fusionado:** da códigos idénticos con 1 y 4 hilos y, con 6 hilos, solo en 1 de 5 textos (primer fotograma divergente entre el 6 y el 14). La causa no se determinó. No se adopta, de modo que no afecta a la recomendación.
- **Pool Win32 reescrito:** 8 configuraciones de estrés (2, 4, 6 y 8 hilos; espera activa 0 y 4096), de 150 a 500 peticiones seguidas cada una, sin fallos ni peticiones por encima de 30 s. Con espera activa y 4 hilos la mediana de latencia subió de 1,69 a 2,03 s.

## 4. Decisión por optimización

| Optimización | Decisión | Cifra que la sustenta |
|---|---|---|
| Cuantización mixta (`--quant-mixed`) | **Adoptar** | 0,643 [0,627–0,664] frente a BF16; identidad −3 % de la brecha; sin diferencia audible a ciegas |
| 6 hilos (`-j 6`) | **Adoptar, condicionado** | 0,869 [0,852–0,901] frente a QM en el servidor; 0,935 en CLI; condicionado a que el equipo destino tenga núcleos libres |
| Truncar a K=12 | Descartar | WER +1,55 pp (límite 1,0); mediana de escucha 75 con caídas de 50 y 75 puntos |
| Truncar a K=10 | Descartar | WER +2,78 pp; identidad −23 % de la brecha (ECAPA) |
| Truncar a K=8 | Descartar | No termina en 10 de 100 casos; WER +9,85 pp |
| Pool Win32 con espera activa | Descartar | 0,979 [0,966–0,985]; no cumple 0,97 |
| Pool Win32 sin espera activa | Descartar | 0,999 [0,997–1,002] |
| QKV int8 fusionado | Descartar | 0,996 [0,995–0,999]; resultado dependiente del número de hilos |
| Prefill B=2 del Code Predictor, prefill `matmat_bf16` | Descartar en este equipo | 1,005 y 0,988 (no favorables); pensados para AVX-512/VNNI |
| Capas asimétricas, 2 hilos BLAS, 8 hilos, 6 hilos con 2 BLAS | Descartar | Más lentas o dominadas por 6 hilos |
| Factor de reutilización de estado del servidor (`QWEN_NO_DELTA_STEP`) | No evaluado | — |

## 5. Perfil recomendado

Argumentos del motor residente (sustituyen a `-j 4` y añaden `--quant-mixed`):

```
-d <modelo> --serve <puerto> --host 127.0.0.1 -j 6 --quant-mixed --stream --watch-stdin
  --load-voice <qvoice> --graft
```

- Sin variables de entorno `QWEN_*` ni parches al motor.
- Muestreo sin cambios: `temperature=0.35`, `seed=4`, `top_k=50`, `top_p=1.0`, `rep_penalty=1.05`.
- `--quant-mixed` cuantiza el Talker a int4 y mantiene el Code Predictor en int8.
- Referencia reversible: el perfil BF16 con `-j 4`.

Efecto esperado en el equipo medido: unos 36 % menos de tiempo por petición, RTF de 1,2, primera lectura de unos 2 s y 0,4 GB más de memoria.

## 6. Requisitos de integración

Los siguientes requisitos y observaciones describen el estado al finalizar la campaña. La integración posterior, su control del entorno del hijo y sus límites se describen al inicio de este informe; en particular, `spawn_with_allowlist_env` permite overrides sin ampliar `RestrictedSpawnRequest`.

1. **Paso de opciones por argumentos.** `RestrictedSpawnRequest` expone `program`, `args`, `stdin`, `stdout_file`, `stderr_file`, `creation_flags` y `extra_allowed`; no tiene campo de entorno. El perfil recomendado solo necesita argumentos, de modo que no hay que mutar el entorno del daemon. Un perfil futuro que requiera variables `QWEN_*` exigiría añadir un campo de entorno al lanzamiento restringido.
2. **Perfiles reversibles.** El perfil BF16 con `-j 4` debe seguir disponible y la selección debe ser explícita. `build_resident_command` hoy produce siempre `-j 4` sin cuantización, y las pruebas que fijan el argumento exacto deberán actualizarse con el perfil elegido.
3. **No usar `--int4` global.** En el modelo 0.6B cuantiza también el Code Predictor y en las pruebas previas produjo salidas truncadas. `--quant-mixed` mantiene ese predictor en int8; en 150 respuestas del servidor no se observó truncamiento, pero esto no garantiza su ausencia universal ni el determinismo.
4. **Parches al motor.** Ninguno se adopta. El motor del repositorio no cambia.
5. **Codebooks.** Si en el futuro se retomara el truncamiento, K debe contar el total de codebooks, el Code Predictor generaría K−1 salidas y las omitidas se rellenan con −1; el alcance en rutas de una petición y por lotes debe decidirse explícitamente. No se evaluó la ruta por lotes.
6. **Texto en UTF-8 sin escapar.** El motor no decodifica secuencias de escape unicode en el JSON de `/v1/tts`: las lee como texto y las pronuncia. Todo cliente debe enviar los caracteres no ASCII como UTF-8 directo.
7. **Plazo de síntesis.** Los textos largos tardan hasta unos 27 s con el perfil recomendado (unos 40 s con BF16). El plazo total de síntesis del adaptador debe cubrir ese caso.
8. **Alcance de las voces.** Se evaluaron solo voces clonadas con el modelo Base; las voces preset y el modelo CustomVoice no se evaluaron.
9. **Determinismo en servidor.** La salida de una petición depende de la anterior; es un comportamiento previo a esta optimización que conviene tener presente en pruebas de regresión.

## 7. Límites de la evidencia

- **Un solo equipo** (Ryzen 7 5825U enchufado, plan «Equilibrado»). Las razones de tiempo no se transfieren a otras CPU.
- **Un solo oyente** y pocos pares: la escucha detecta diferencias grandes, no pequeñas. La comparación por pares no incluyó textos largos.
- **Cuántos textos:** el cribado cubre 10 de los 35 textos con 2 semillas; la confirmación cubre los 35 con 2 semillas. El baseline usa 3.
- **Los márgenes de identidad fijados son más estrictos que lo audible.** La candidata no los cumple; la recomendación se apoya en la escucha ciega y en que el cambio de identidad es del orden de un 3 % de la brecha entre hablantes.
- **Tasa de salidas anómalas** de la candidata (2 de 350): no se distingue de cero con esta muestra.
- **No medidos:** consumo en batería, factor `QWEN_NO_DELTA_STEP`, segunda voz en el servidor, carga continua de 20 minutos con texto correcto, independencia del daemon respecto al orden de las peticiones, descomposición de la pérdida del truncamiento entre decoder y trayectoria, causa de la divergencia del QKV fusionado con 6 hilos, y por qué la cuantización mixta acelera menos en el servidor (25 %) que en la CLI (36 %).
- **Transcripción con cifras:** el WER de los textos con cifras mezcla variación del ASR y posibles errores reales; se interpretó solo de forma relativa al baseline.
