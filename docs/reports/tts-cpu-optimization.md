# Informe de optimización de síntesis Qwen3-TTS 0.6B en CPU: Diagnóstico, adopción de cuantización mixta y línea base de producción

**Fecha de consolidación:** noviembre de 2026.  
**Modelo:** `Qwen3-TTS-12Hz-0.6B-Base` (clonación cero-shot con `--graft`) y `Qwen3-TTS-12Hz-0.6B-CustomVoice` (presets).  
**Hardware de referencia:** AMD Ryzen 7 5825U (8 núcleos / 16 hilos, AVX2/FMA, sin AVX-512 ni VNNI), 31,4 GB RAM, Windows 11.  
**Estado:** Integrado y adoptado en producción (`crates/avi-tts`, CLI y daemon).

---

## Tabla de contenidos

1. [Resumen ejecutivo y estado de producción](#1-resumen-ejecutivo-y-estado-de-producción)
2. [Diagnóstico experimental y descarte de alternativas](#2-diagnóstico-experimental-y-descarte-de-alternativas)
   - [2.1. Reparto del tiempo del Code Predictor y techo de ancho de banda](#21-reparto-del-tiempo-del-code-predictor-y-techo-de-ancho-de-banda)
3. [Metodología de medición y evaluación de calidad](#3-metodología-de-medición-y-evaluación-de-calidad)
4. [Resultados empíricos consolidados](#4-resultados-empíricos-consolidados)
   - [4.1. Rendimiento y latencia](#41-rendimiento-y-latencia)
   - [4.2. Calidad acústica e inteligibilidad](#42-calidad-acústica-e-inteligibilidad)
   - [4.3. Evaluación subjetiva de escucha ciega](#43-evaluación-subjetiva-de-escucha-ciega)
   - [4.4. Optimización del decodificador acústico y afinidad](#44-optimización-del-decodificador-acústico-y-afinidad)
   - [4.5. Producto escalar entero en los kernels cuantizados](#45-producto-escalar-entero-en-los-kernels-cuantizados)
   - [4.6. Optimizaciones de ancho de banda de memoria](#46-optimizaciones-de-ancho-de-banda-de-memoria)
5. [Arquitectura de integración en el producto](#5-arquitectura-de-integración-en-el-producto)
   - [5.1. Perfil predeterminado y reversibilidad (`AVI_TTS_PRECISION`)](#51-perfil-predeterminado-y-reversibilidad-avi_tts_precision)
   - [5.2. Aislamiento del entorno del residente](#52-aislamiento-del-entorno-del-residente)
6. [Límites de la optimización y margen restante](#6-límites-de-la-optimización-y-margen-restante)

---

## 1. Resumen ejecutivo y estado de producción

En la configuración histórica de producción, el proceso servidor residente de inferencia de Qwen3-TTS se ejecutaba en formato de punto flotante de 16 bits (`BF16`) con 4 hilos de CPU (`-j 4`). Bajo esa configuración, la síntesis representaba el principal cuello de botella de la aplicación, con un factor de tiempo real mediano de **1,84× RTF** (generar 1 segundo de voz toma ~1,84 segundos).

Tras una campaña exhaustiva de medición y evaluación de calidad, se adoptó formalmente la **cuantización mixta (`--quant-mixed`)** como perfil predeterminado en `crates/avi-tts`:
- **Talker (28 capas):** Cuantizado a **INT4** ($Q4\_0$), reduciendo sustancialmente el coste de generación de tokens sin afectar la inteligibilidad fonética.
- **Code Predictor (5 capas):** atención y cabezas de salida en **INT8** y FFN en **Q4_0** (etiqueta de log `[INT8+Q4_0-FFN]`). La atención queda en INT8, evitando la inestabilidad y truncamientos que provoca la cuantización global a INT4; las FFN, la mayor parte de los bytes que se leen 15 veces por fotograma, toleran Q4_0 ([sección 4.6](#46-optimizaciones-de-ancho-de-banda-de-memoria)).
- **Hilos de cómputo:** Configurado en **4 hilos (`-j 4`)** para garantizar estabilidad y convivencia en procesadores estándar sin saturar el sistema del usuario.

### Métricas de producción consolidadas

| Métrica | Baseline histórico (`BF16 -j 4`) | Producción adoptada (`mixed -j 4`) | Rendimiento pico (`mixed -j 6`*) |
|---|---|---|---|
| **RTF mediano en Daemon (caliente)** | **1,84×** | **~1,38× – 1,45×** | 1,22× |
| **Tiempo relativo por petición** | 1,00 (referencia) | **0,748** (~25 % menos) | 0,643 (~36 % menos) |
| **Primera lectura en streaming** | ~2,83 s | **~2,34 s** | ~2,03 s |
| **Memoria RAM máxima del motor** | 2,51 GB | **2,88 GB** | 2,88 GB |
| **WER agregado en español/inglés** | 2,53 % | **2,20 %** | 2,04 % |
| **Identidad acústica (WavLM-SV)** | 0,9881 | **0,9844** (cambio < 3 % brecha) | 0,9844 |

*\*Nota: El perfil con 6 hilos se evaluó como pico de hardware en Ryzen 7, pero se adoptaron 4 hilos por diseño de compatibilidad universal.*

Estado final con el producto escalar entero (`mixed -j 4`, CLI, texto largo en español, mediana de 3 ejecuciones alternadas contra el motor anterior):

| Condición | RTF motor anterior | RTF motor final | Memoria máxima del daemon |
|---|---|---|---|
| Equipo libre | 0,93 | **0,76** | 2.912 MB (sin cambio) |
| 4 procesos ocupando la CPU | 1,10 | **0,88** | — |
| 2 hilos saturando el ancho de banda de memoria | 1,70 | **1,47** | — |

Sobre ese perfil se aplicaron después cuatro optimizaciones del motor que no cambian el modelo: un espacio de trabajo persistente del decodificador acústico, la convolución transpuesta con un solo GEMM, la convolución causal sin im2col y la afinidad de un hilo lógico por núcleo físico en Windows. Medidas de forma alternada contra el motor anterior en el mismo equipo, llevan la síntesis por debajo de tiempo real con el equipo libre (RTF **0,93** en CLI con 4 hilos y **0,945** en el daemon con frases largas, frente a 1,04 y 1,065; −11 %). Con carga de fondo en el equipo, la reducción llega al **20–23 %** (de 1,55 a 1,24 en CLI). Cuestan **~100 MB** más de memoria y no pierden calidad (detalle en la [sección 4.4](#44-optimización-del-decodificador-acústico-y-afinidad)).

Después, todas las multiplicaciones de pesos cuantizados del motor (int8 del Code Predictor y q4_0 del Talker) pasaron a usar producto escalar entero en todas las arquitecturas. Medido de forma alternada contra el motor anterior en el mismo equipo (Ryzen 7 5825U, `-j 4`, voz clonada, texto largo en español), el RTF mediano baja de 0,93 a **0,76** con el equipo libre y de 1,10 a **0,88** con cuatro procesos ocupando la CPU, sin reentrenar el modelo y con la misma calidad medida (WER 3,55 % en ambos, similitud de voz media de 0,978). Con dos hilos saturando el ancho de banda de memoria el RTF baja de 1,70 a 1,47 y sigue por encima de 1, porque el Code Predictor queda limitado por memoria (detalle en la [sección 4.5](#45-producto-escalar-entero-en-los-kernels-cuantizados)).

Por último, cuatro optimizaciones de ancho de banda de memoria (FFN del Code Predictor en Q4_0, PREFILL2 con cualquier Code Predictor cuantizado, anticipación de lecturas en los kernels INT8 y decodificador por tramos) bajan el RTF mediano, medido por pares alternados contra el motor anterior, de 0,79 a **0,66** sin carga, de 1,06 a **0,84** con un hilo de carga de memoria y de 1,45 a **1,10** con dos, sin pérdida de calidad medida ni audible (detalle en la [sección 4.6](#46-optimizaciones-de-ancho-de-banda-de-memoria)).

---

## 2. Diagnóstico experimental y descarte de alternativas

Durante la campaña se evaluaron de forma aislada múltiples factores sobre el motor nativo en C (`vendor/qwen3-tts`). Los veredictos arquitectónicos definitivos son:

| Optimización evaluada | Decisión | Fundamento empírico |
|---|---|---|
| **Cuantización mixta (`--quant-mixed`)** | **ADOPTADA** | Acelera un ~25 % en servidor (4 hilos) y ~36 % en CLI directa. Cero degradación perceptible en escucha ciega ($p = 0,5$). Cero truncamientos en >150 peticiones continuas. |
| **Cuantización INT4 global (`--int4`)** | **DESCARTADA** | En el modelo 0.6B, cuantizar el Code Predictor a INT4 genera salidas no deterministas y truncamientos severos de audio. |
| **Truncamiento de codebooks ($K=12, 10, 8$)** | **DESCARTADO** | $K=12$ y $K=10$ degradan la inteligibilidad (WER +1,55 pp y +2,78 pp) y alteran la identidad vocal. $K=8$ no termina de sintetizar en 10 de cada 100 casos (caídas de hasta 0 puntos en MUSHRA). |
| **Pool de hilos Win32 con espera activa** | **ADOPTADA** (revisa un descarte previo) | La primera prueba (razón de tiempo 0,979, latencia mediana de 1,69 s a 2,03 s en 4 hilos) se hizo con el kernel anterior. Con el kernel entero, el pool espera activamente un presupuesto acotado antes de dormir, y como el Code Predictor despacha unas 495 tareas al pool por fotograma, esa espera elimina el coste de despertar hilos: sin carga el RTF pasa de 0,79 (sin espera) a 0,76 (presupuesto de 4096 iteraciones) y con 4 núcleos ocupados de 0,91 a 0,87. |
| **Presupuesto de espera activa de 20000 iteraciones** | **DESCARTADO** | No mejora sobre 4096: 0,76 sin carga y 0,90 con 4 núcleos ocupados (0,87 con 4096). |
| **Proyecciones QKV INT8 fusionadas con acumulación en coma flotante** | **DESCARTADO** | Aceleración despreciable (0,996) e inconsistencias numéricas dependientes del número de hilos. Con el producto escalar entero la suma es exacta y el resultado no depende del reparto entre hilos. |
| **`matmat_bf16`** | **DESCARTADO** | Impacto neutro o negativo en procesadores sin extensiones AVX-512 / VNNI. |
| **Matmat B=2 en coma flotante en AVX2** | **DESCARTADO** | Sin ganancia medible en el Code Predictor. |
| **Prefill B=2 del Code Predictor cuantizado (PREFILL2)** | **ADOPTADA** | Con el matmat entero, igual a dos matvec, deja de ser una variante opcional: es la única ruta, y se usa siempre que todas las matrices del Code Predictor estén cuantizadas, en INT8, Q4_0 o mezcladas. |
| **Producto escalar entero en todas las rutas cuantizadas** | **ADOPTADA** | Diagnóstico en la sección 2.1 y resultados en la 4.5. |
| **q4_0 en el Code Predictor con el kernel anterior** | **DESCARTADO** | El kernel q4_0 de AVX2 estaba limitado por cálculo, no por memoria, y con `-j 4` no daba ganancia. |
| **Espacio de trabajo persistente del decodificador** | **ADOPTADA** | Evita cientos de reservas de ≥ 1 MiB por frase, cuyas páginas nuevas Windows pone a cero en cada bloque; audio bit a bit idéntico. |
| **Convolución transpuesta con un solo GEMM** | **ADOPTADA** | Multiplica sobre la vista contigua `[entrada][salida·k]` de los pesos y dispersa en orden de k: sin reordenar pesos por llamada ni RAM adicional. |
| **Convolución causal sin im2col** | **ADOPTADA** | k GEMM acumulados sobre la entrada desplazada, con pesos `W_k` reordenados al cargar (~116 MB compartidos entre workers); desaparece la tabla im2col y su troceo. |
| **Afinidad de un hilo lógico por núcleo físico (Windows)** | **ADOPTADA** | Elimina el estado lento intermitente en que dos hilos pesados comparten núcleo SMT; aplicada desde el motor, la reciben CLI y daemon. |
| **Convolución transpuesta con pesos reordenados en caché** | **DESCARTADO** | ~750 ms por frase frente a ~1.620 ms del original, pero más lenta que el GEMM único (~590 ms) y con ~107 MB de RAM adicional. |
| **Hilos dedicados a la activación Snake** | **DESCARTADO** | Sin ganancia con 4 hilos (RTF 1,06/1,06/1,04 frente a 1,04/1,06/1,04 sin ellos) y resultado mixto con 6. |

### 2.1. Reparto del tiempo del Code Predictor y techo de ancho de banda

Medido con una copia instrumentada del motor en el hardware de referencia (AVX2, `-j 4`), las multiplicaciones de pesos suponen el **96 %** del tiempo del Code Predictor (52 ms por fotograma antes de la mejora):

| Parte del Code Predictor | Cuota |
|---|---|
| MLP: proyecciones gate y up | 37 % |
| Proyecciones QKV | 27 % |
| MLP: proyección down | 17 % |
| Proyección de salida O | 13 % |
| `lm_head` | 2,5 % |
| SwiGLU | 1,7 % |
| Atención | 1 % |

Cada fotograma requiere unos 495 despachos de tareas al pool de hilos. La lectura máxima de memoria medida en el equipo es de unos 37–41 GB/s, y el paso del Code Predictor aislado en int8 iba a 31 GB/s (84 % de ese techo): lo acotaba el cálculo, porque el kernel AVX2 int8 ensanchaba los pesos a coma flotante antes de la FMA y el kernel q4_0 de AVX2 tenía el mismo defecto. De ahí que la vía fuera abaratar el cálculo por byte leído, no reducir bytes. La competencia con el decodificador acústico por la CPU cuesta además ~8 ms por fotograma.

---

## 3. Metodología de medición y evaluación de calidad

La campaña de validación empleó un corpus estructurado de 35 textos (25 en español y 10 en inglés) y 5 identidades de voz (voz del usuario `default` y 4 voces clonadas de hablantes reales del dataset SLR61).

### Métricas objetivas
- **Inteligibilidad (ASR WER):** Transcripción automática con Parakeet TDT v3 (minúsculas, sin diacríticos).
- **Identidad vocal (WavLM-SV / ECAPA):** Similitud coseno mediante `microsoft/wavlm-base-plus-sv` y el extractor ECAPA nativo del motor.
- **Duración y F0:** Variación de duración en muestras y contorno fundamental en semitonos mediante `librosa.pyin`.

### Procedimiento para el producto escalar entero
- **Corpus:** 15 frases (español e inglés) con semillas 42 y 43, y un texto largo en español de unos 18 s para los tiempos.
- **Inteligibilidad:** WER con faster-whisper large-v3-turbo sobre texto normalizado.
- **Identidad vocal:** similitud coseno con `microsoft/wavlm-base-plus-sv` frente al audio del motor anterior, con un control del propio motor anterior sintetizado con otra semilla para saber cuánto varía la similitud sin ningún cambio de código.
- **Condiciones de carga:** equipo libre; cuatro procesos `yes` que ocupan la CPU; y dos hilos que leen memoria con un bucle AVX2 y saturan el ancho de banda. Tras cada bloque con carga se comprueba que no queda ningún generador de carga en ejecución.
- **Tiempos:** ejecuciones alternadas motor anterior/final dentro de cada condición, con la mediana de 3 y el rango; el RTF se calcula sobre la duración de cada WAV.
- **Identidad de la salida:** en los cambios sin efecto numérico (por ejemplo, el prefetch) el criterio es el md5 idéntico del WAV, con el mismo `-j`.
- **Autotest de kernels:** `--self-test` del motor con `SIMD=auto` y con `SIMD=scalar`, que compara cada kernel con su referencia escalar.

### Archivos de evidencia empírica
- `docs/reports/mushra_results.json`: 12 ensayos controlados de degradación auditiva por truncamiento de codebooks.
- `docs/reports/pares_results.json`: 24 ensayos de preferencia forzada A/B a ciegas entre `BF16` y `--quant-mixed`.

---

## 4. Resultados empíricos consolidados

### 4.1. Rendimiento y latencia
En la ruta de servidor residente (`/v1/tts`), la cuantización mixta reduce de forma consistente los tiempos de síntesis por longitud de texto:
- **Frase corta (~4 s de audio):** de 6,9 s (`BF16`) a **5,6 s** (`mixed -j 4`).
- **Frase media (~8 s de audio):** de 14,2 s (`BF16`) a **10,3 s** (`mixed -j 4`).
- **Texto largo (~22 s de audio):** de 39,7 s (`BF16`) a **30,2 s** (`mixed -j 4`).

### 4.2. Calidad acústica e inteligibilidad
- **Preservación de identidad:** La distancia coseno en WavLM entre `BF16` y `mixed` es de apenas 0,0037, lo que representa menos del **3 % de la brecha** que separa dos hablantes reales distintos.
- **Inteligibilidad en producción:** En una batería de 16 muestras de verificación E2E (voces `default` y `ryan`, español e inglés, CLI directa y daemon), 14 de las 16 muestras obtuvieron **WER = 0,00** (reproducción fonética exacta).
- **Sensibilidad en CustomVoice:** Se observó que el preset `ryan` en inglés con INT4 presentó una sensibilidad puntual en la palabra final («test»), mientras que con BF16 sintetizó sin error. Esto ratifica el acierto de diseñar el sistema con reversibilidad estricta.

### 4.3. Evaluación subjetiva de escucha ciega
En la prueba formal de preferencia forzada por pares (24 ensayos pareados con control de cambio de semilla):
- **BF16 vs. `--quant-mixed`:** 5 preferencias para BF16, 5 para cuantización mixta y 2 empates.
- **Control cruzado (BF16 semilla 4 vs. BF16 semilla 7):** 4 preferencias para semilla 4, 6 para semilla 7 y 2 empates.
- **Veredicto estadístico:** Prueba de Fisher unilateral con $p = 0,50$. No se detectó ninguna degradación perceptible atribuible a la cuantización mixta.

### 4.4. Optimización del decodificador acústico y afinidad
Medición del 10 de noviembre de 2026 en el hardware de referencia (corriente, plan Equilibrado), con voz clonada, `--quant-mixed`, semilla 42 y sin afinidad externa, en dos condiciones del equipo:

- **Equipo libre:** sin Chrome ni Visual Studio Code abiertos; carga de CPU entre el 1 % y el 12 % durante la medición.
- **Equipo con carga de fondo:** Chrome, Visual Studio Code y otras herramientas abiertas; carga de CPU entre el 24 % y el 33 % en reposo.

En cada condición se alternaron el motor anterior y el optimizado, para que la carga, que varía entre ejecuciones, afecte por igual a ambos. El RTF del daemon es el que registra el propio servidor por petición con el equipo libre, y el cociente entre el tiempo total de la petición HTTP y la duración del audio con carga; ambos miden lo mismo, porque el envío del WAV cuesta menos de 1 ms.

**Equipo libre**

| Escenario | Motor anterior (mediana) | Motor optimizado (mediana) | Cambio |
|---|---|---|---|
| CLI, texto de 18 s, `-j 4` (3 ejecuciones) | 1,04 (1,03–1,07) | **0,93** (0,92–0,96) | −11 % |
| CLI, texto de 18 s, `-j 6` (3 ejecuciones) | 1,01 (1,00–1,02) | **0,87** (0,86–0,92) | −14 % |
| Daemon, frase corta de ~3 s (4 peticiones) | 1,19 (1,17–1,22) | **1,02** (1,00–1,03) | −14 % |
| Daemon, frase media de ~6 s (4 peticiones) | 1,08 (1,05–1,11) | **0,945** (0,93–0,95) | −13 % |
| Daemon, frase larga de 18 s (4 peticiones) | 1,065 (1,04–1,12) | **0,945** (0,93–0,96) | −11 % |
| Pico de memoria del daemon tras 6 peticiones | 2.813 MB | 2.912 MB | +99 MB |

Por fotograma, en CLI: Talker de 20,5–21,3 a 18,6–19,9 ms y Code Predictor de 54–60 a 47–54 ms.

**Equipo con carga de fondo**

| Escenario | Motor anterior (mediana) | Motor optimizado (mediana) | Cambio |
|---|---|---|---|
| CLI, texto de 18 s, `-j 4` (3 ejecuciones) | 1,55 (1,54–1,69) | **1,24** (1,21–1,36) | −20 % |
| CLI, texto de 18 s, `-j 6` (3 ejecuciones) | 1,42 (1,42–1,45) | **1,19** (1,17–1,36) | −16 % |
| Daemon, frase corta de ~3 s (4 peticiones) | 1,72 (1,66–1,78) | **1,32** (1,25–1,38) | −23 % |
| Daemon, frase media de ~6 s (4 peticiones) | 1,61 (1,55–1,65) | **1,20** (1,17–1,26) | −25 % |
| Daemon, frase larga de 18 s (4 peticiones) | 1,60 (1,53–1,62) | **1,235** (1,23–1,27) | −23 % |
| Pico de memoria del daemon tras 4 peticiones | 2.810–2.813 MB | 2.911–2.913 MB | +100 MB |

- **Objetivo de tiempo real:** con el equipo libre, el motor optimizado sintetiza por debajo de tiempo real en CLI con 4 y 6 hilos y en el daemon con frases medias y largas. La frase corta queda en ~1,0 por el coste fijo del prefill (~0,5–0,7 s), que pesa más cuanto más corto es el audio.
- **Sensibilidad a la carga:** la carga de fondo empeora el RTF de ambos motores entre un 30 % y un 50 %; el mismo motor anterior pasa de 1,04 a 1,55. La mejora relativa es mayor con carga (−16 % a −25 %) que con el equipo libre (−11 % a −14 %), porque sin carga el motor anterior apenas cae en el estado lento por SMT que corrige la afinidad.
- **Estabilidad:** el log de cada ejecución registra la máscara `0x5555`, y en ninguna ejecución del motor optimizado reapareció el salto bimodal característico del estado SMT. Con el equipo libre, ninguna ejecución del motor optimizado superó RTF 1,03; con carga, varias superaron 1,3 (máximo 1,38 en el daemon) por la carga ajena.
- **Memoria:** el aumento son los pesos `W_k` de la convolución sin im2col (~116 MB, compartidos entre workers) más el espacio de trabajo persistente, menos las reservas por bloque que desaparecen.

**Procedimiento de validación de calidad.** Los cambios que no alteran el cálculo (eliminación de código muerto, red unificada, espacio de trabajo, afinidad) se aceptaron con md5 idéntico del WAV frente al paso anterior, siempre con el mismo número de hilos, porque el md5 depende de `-j`. El corpus fue de 15 frases (10 en español y 5 en inglés) más el texto largo, en streaming, y la decodificación completa de un volcado fijo de códigos (`QWEN_DUMP_CODES`) con `qwen_tts_decoder_tool`. Los cambios que reordenan sumas en coma flotante (las dos convoluciones) se aceptaron con SNR ≥ 90 dB, diferencia ≤ 2 LSB, WER sin cambios (faster-whisper large-v3-turbo) y similitud WavLM-SV ≥ 0,999 sobre el mismo corpus. Resultado: SNR de 96,6 a 98,6 dB, 1 LSB como máximo, WER de 3,57 % en ambos motores, similitud WavLM de 1,00000 en todas las frases y salida determinista entre ejecuciones. Al unificar la red convolucional, la decodificación completa adoptó la LayerNorm de dos pasadas del streaming (SNR 100,9 dB, 1 LSB), para que el audio de producción conservara su md5.

### 4.5. Producto escalar entero en los kernels cuantizados

**Estrategias adoptadas** (motor `vendor/qwen3-tts`):
- **Producto escalar entero en todas las rutas cuantizadas y en todas las arquitecturas.** El int8 del Code Predictor y el q4_0 del Talker multiplican con enteros (AVX2 en x86, SDOT en ARM64, referencia escalar) y aplican las escalas una vez por bloque, en lugar de ensanchar los pesos a coma flotante. Las rutas de coma flotante cuantizada y sus interruptores de entorno desaparecen.
- **Cuantizadores de activación únicos.** Una sola función cuantiza las activaciones a int8 para todas las rutas, de modo que todas las plataformas reciben exactamente los mismos enteros.
- **Matmat exacto, igual a B×matvec.** Como la suma entera es exacta y el matmat reutiliza los kernels de fila del matvec, en AVX2 y en la compilación escalar da bit a bit lo mismo que B matvec, y una petición agrupada produce el mismo audio que la individual. En VNNI el int8 también es exacto; el q4 de i8mm difiere ~1e-7 por el orden de acumulación en coma flotante. Las plataformas comparten la misma cuantización y la misma suma entera, pero no son bit a bit iguales entre sí: la acumulación de las escalas en coma flotante sigue un orden propio de cada ISA.
- **PREFILL2 como ruta única** del Code Predictor cuantizado, sin interruptor.
- **Espera activa en el pool de hilos de Windows** antes de dormir (presupuesto de 4096 iteraciones).
- **ARM64 exige `dotprod`:** una compilación sin él falla con `#error`.
- **Comprobación de dimensiones al cargar:** un modelo con matrices cuantizadas de más de 8192 columnas o con columnas no múltiplo de 32 falla al cargar con un error claro, en lugar de producir resultados erróneos.
- **Prefetch del kernel q4_0 de AVX2.** El kernel entero consume los pesos tan deprisa que dependía de la latencia de memoria: procesa dos filas intercaladas sin prefetch por software, y con el bus saturado el prefetch por hardware no cubría ambos flujos, de modo que cada bloque esperaba un fallo de caché. Sin prefetch, con carga de memoria el Talker empeoraba frente al motor anterior (de 28,7 a 41,2 ms por fotograma). Anticipar 16 bloques en las dos filas lo corrige sin cambio numérico (md5 idéntico): con carga de memoria el Talker queda en 25,7 ms por fotograma, frente a 27,7 del motor anterior en la misma tanda, y sin carga baja de 17,1 a 14,8 ms por fotograma frente a la versión sin prefetch.

**RTF del texto largo** (mediana y rango de 3 ejecuciones alternadas; el audio del motor final dura 17,5 s y el anterior 18,2 s):

| Condición | Motor | Talker ms/f | Code Predictor ms/f | RTF |
|---|---|---|---|---|
| Equipo libre | anterior | 19,0 (18,8–19,1) | 51,5 (50,6–52,7) | 0,93 (0,92–0,96) |
| Equipo libre | final | 14,6 (14,4–14,6) | 41,3 (41,3–41,6) | **0,76** (0,76–0,76) |
| 4 procesos `yes` | anterior | 26,2 (25,1–28,4) | 57,4 (54,3–59,9) | 1,10 (1,05–1,16) |
| 4 procesos `yes` | final | 20,7 (19,1–20,9) | 44,3 (43,7–45,3) | **0,88** (0,85–0,89) |
| 2 hilos de lectura de memoria | anterior | 27,7 (27,5–29,7) | 102,7 (102,2–104,1) | 1,70 (1,69–1,75) |
| 2 hilos de lectura de memoria | final | 25,7 (25,0–27,3) | 85,8 (85,5–89,7) | **1,47** (1,46–1,55) |

**Daemon** (motor final, 2 peticiones por frase, RTF de la primera y la segunda):

| Frase | Audio | RTF |
|---|---|---|
| Corta | 3,1 s | 0,89 / 0,86 |
| Media | 6,2–8,3 s | 0,76 / 0,75 |
| Larga | 17,5–17,9 s | 0,77 / 0,76 |

El daemon queda al nivel de la línea de comandos. El pico de memoria es de 2.912,5 MB, igual que antes.

**Calidad** (15 frases, 10 en español y 5 en inglés, semillas 42 y 43; el control es el motor anterior con la semilla 43):

| Condición | WER | Similitud media | Similitud mínima | Relación de duración |
|---|---|---|---|---|
| Motor anterior | 3,55 % | 1,0000 | 1,0000 | 1,000 |
| Control (anterior, semilla 43) | 3,55 % | 0,9767 | 0,9460 | 1,079 |
| Final, semilla 42 | 3,55 % | 0,9784 | 0,9531 | 1,052 |
| Final, semilla 43 | 3,55 % | 0,9783 | 0,9586 | 1,042 |

Las frases con errores de transcripción son las mismas en todas las condiciones y con el mismo recuento. La similitud del motor final con el anterior es la misma que la de dos semillas distintas del propio motor anterior: la diferencia es de muestreo, no de calidad. En una escucha de frases del corpus (anterior frente a final) la calidad resultó buena, con timbre e identidad mantenidos y sin palabras ininteligibles.

**Con memoria saturada.** El Code Predictor pasa de 41,3 a 85,8 ms por fotograma al saturar el ancho de banda: sigue limitado por memoria, y el producto escalar entero reduce el cálculo pero no los bytes leídos. El Talker, sin el prefetch, empeoraba con esa carga; con él mejora un 7 %, aunque queda casi al doble de su coste libre.

### 4.6. Optimizaciones de ancho de banda de memoria

**Causa.** Con otras aplicaciones usando la memoria, el motor sintetizaba más lento que el tiempo real (RTF 1,06 con un hilo de carga de memoria y 1,45 con dos). Las causas eran de ancho de banda, no de cálculo:
- El Code Predictor lee sus capas 15 veces por fotograma, y sus FFN en INT8 son la mayor parte de esos bytes.
- La precisión mixta desactivaba PREFILL2, que lee los pesos de las posiciones 0 y 1 una sola vez.
- El kernel INT8 pedía cada peso justo al usarlo y, con el bus compartido, esperaba a la memoria.
- El decodificador acústico recorría varias veces señales de varios MB que no caben en la L2, y su tráfico frenaba al resto.

**Estrategias adoptadas** (motor `vendor/qwen3-tts`):
- **FFN del Code Predictor en Q4_0** (`QWEN_CP_PREC=mixed`, fijado por `--quant-mixed`): atención y cabezas de salida en INT8, `gate_up` y `down` en Q4_0. El Code Predictor cuantizado pasa de ~47 MB a ~26 MB.
- **PREFILL2 con cualquier Code Predictor cuantizado**, eligiendo el kernel matmat por matriz.
- **Anticipación de lecturas en los kernels INT8** de AVX2 y SDOT, 5120 bytes por delante en las dos filas que se procesan a la vez, sin cambio numérico.
- **Decodificador por tramos de 256 KB** en las unidades residuales de `up2` y `up3`, solo mientras la generación sigue en marcha. Al drenar los fotogramas finales se decodifica sin tramos y con todos los hilos de BLAS, y cada convolución con contexto calcula solo sus columnas nuevas; sin esas dos medidas, los tramos alargaban el drenaje final de ~200 a ~350 ms por frase.

**Procedimiento de validación.**
- **Velocidad:** pares alternados en orden ABBA frente al motor anterior, con el texto largo y el perfil de producción, sin carga y con 1 y 2 hilos que leen 512 MB de RAM de forma continua (carga de memoria sintética); 5 pares por nivel, y en el daemon tres frases en 3 rondas alternadas. Las mediciones se hicieron con el equipo enchufado y sin aplicaciones de red de fondo, que ralentizaban por igual a ambos motores.
- **Calidad** (modelo 0.6B, 15 frases y dos semillas, frente a la variación entre semillas del motor anterior): WER 3,85 % frente a 4,14 %, similitud WavLM media 0,976 y mínima 0,952, ningún final perdido, y entonación y duración dentro de la variación entre semillas. En escucha humana la voz se juzgó igual o más natural que con el motor anterior.
- **Implementación:** cada paso se aceptó reproduciendo el audio del prototipo validado (md5 idéntico, o la misma longitud y SNR ≥ 60 dB; la versión final da ~102 dB).

**RTF del texto largo** (Ryzen 7 5825U, 4 hilos, voz clonada; mediana de 5 pares, todos a favor):

| Carga de memoria | Motor | Talker ms/f | Code Predictor ms/f | Tiempo total | Drenaje final | RTF |
|---|---|---|---|---|---|---|
| Sin carga | anterior | 15,0 | 44,0 | 14,3 s | 204 ms | 0,79 |
| Sin carga | final | 15,0 | 33,0 | 11,0 s | 238 ms | **0,66** |
| 1 hilo | anterior | 17,9 | 62,0 | 19,2 s | 179 ms | 1,06 |
| 1 hilo | final | 18,4 | 43,4 | 14,0 s | 228 ms | **0,84** |
| 2 hilos | anterior | 25,6 | 85,0 | 26,3 s | 197 ms | 1,45 |
| 2 hilos | final | 25,6 | 55,7 | 18,4 s | 246 ms | **1,10** |

El tiempo hasta el primer audio no cambia (947 frente a 966 ms sin carga). El drenaje final queda unos 40 ms por encima del motor anterior porque la generación es más rápida y el decodificador tiene menos tiempo libre, mientras la frase completa termina entre 3,3 y 7,9 s antes.

**Daemon** (mediana de 3 rondas alternadas, RTF anterior / final):

| Carga de memoria | Corta | Media | Larga |
|---|---|---|---|
| Sin carga | 0,74 / **0,66** | 0,76 / **0,64** | 0,80 / **0,68** |
| 2 hilos | 1,43 / **1,06** | 1,46 / **1,09** | 1,52 / **1,15** |

Las cifras de calidad y velocidad corresponden al modelo 0.6B; solo AVX2 se midió en hardware, y la ruta SDOT se comprueba con la compilación cruzada y con el `--self-test` de la CI.

### 4.7. Entrega en streaming

**Causa.** La aplicación esperaba el WAV completo antes de reproducir: con una frase larga (450 caracteres, 26 s de audio) la voz empezaba a los 18 s, aunque el motor tuviera el primer audio listo en torno a 1 s. El motor ya ofrecía `POST /v1/tts/stream` (PCM s16le, 24 kHz, mono, por partes: un primer fragmento de 2 fotogramas, 160 ms, y después fragmentos de `chunk_frames`), y la aplicación pasó a usarlo como único camino de síntesis, con un reproductor incremental.

**Mediciones previas** (Ryzen 7 5825U, Windows 11, perfil `mixed`, voz clonada con `--graft`, semilla 4; «un hilo de carga» es un proceso que lee sin parar 512 MB de RAM):

| Medida | Resultado |
|---|---|
| Frase larga con entrega completa | 18 s hasta poder reproducir |
| Primer fragmento por `/v1/tts/stream`, sin carga | 0,58 s (frase corta), 0,68 s (media), 0,96 s (larga) |
| Hueco tras el primer fragmento con `chunk_frames` 10 / 4 / 2 | 0,58 / 0,12 / 0,02 s sin carga; 0,81 / 0,19 / 0,10 s con un hilo de carga |
| RTF de la frase larga con `chunk_frames` 10 / 4 / 2 | 0,70 / 0,72 / 0,74 sin carga; 0,91 / 0,95 / 0,99 con un hilo de carga |
| Prototipo de reproducción incremental (4 fotogramas, colchón de 300 ms) | Primer sonido a 1,23 s sin carga y 1,45 s con un hilo de carga; ningún vaciado de la cola |
| Mismo audio por `/v1/tts` y `/v1/tts/stream` | Misma longitud, SNR 98–103 dB |
| Cancelación al cerrar la conexión | Motor libre en 0,5–0,8 s; el residente sigue vivo |

**Decisiones.** Fragmentos de 4 fotogramas: el hueco tras el primer fragmento baja de 0,58 s a 0,12 s a cambio de un 4 % de RTF. Colchón de 300 ms: cubre ese hueco, medido en 0,12–0,19 s; si la cola se vacía con la entrega en curso, la reproducción se pausa y se reanuda al recuperarlo, y se cuenta en `stalls`.

**Cambios del motor ligados a la entrega.** El prefill es siempre completo: la reutilización del prefill entre peticiones recalculaba el resto del prompt con los pesos q4_0, daba audios distintos según la petición anterior y retrasaba el primer audio (1,27–1,37 s frente a 0,98–1,03 s). Una respuesta por partes sin su fragmento final es un fallo, y las peticiones inválidas reciben un error 400.

**Medición final de punta a punta** (`speech say`, frase larga): pendiente de la medición de la entrega.

---

## 5. Arquitectura de integración en el producto

### 5.1. Perfil predeterminado y reversibilidad (`AVI_TTS_PRECISION`)
El constructor central `avi_tts::resident::build_resident_command` en `crates/avi-tts/src/lib.rs` gobierna el lanzamiento del motor en todas las rutas del producto:

```rust
// crates/avi-tts/src/lib.rs
let mut args = vec![
    String::from("-d"), model_dir.to_string_lossy().into_owned(),
    String::from("--serve"), port.to_string(),
    String::from("--host"), crate::RESIDENT_HOST.to_string(),
    String::from("-j"), String::from("4"),
];
if precision == TtsPrecision::Mixed {
    args.push(String::from("--quant-mixed"));
}
args.extend([String::from("--stream"), String::from("--watch-stdin")]);
```

- **Perfil predeterminado:** `TtsPrecision::Mixed` (activo cuando `AVI_TTS_PRECISION` está ausente o vale exactamente `"mixed"`).
- **Perfil reversible:** `TtsPrecision::Bf16` (seleccionable mediante `AVI_TTS_PRECISION=bf16` al iniciar el proceso o el daemon).
- **Control de errores estricto:** Cualquier valor inválido (cadenas vacías, espacios, mayúsculas o caracteres corruptos) se rechaza de inmediato con `usage_error` (código de salida 2 en CLI) indicando el diagnóstico claro.

### 5.2. Aislamiento del entorno del residente
Para proteger la inferencia de variables de entorno heredadas que pudieran corromper la precisión, el lanzador `spawn_with_allowlist_env` inyecta de forma atómica:
- **Para `mixed`:** `QWEN_TALKER_PREC=int4` y `QWEN_CP_PREC=mixed`.
- **Para `bf16`:** `QWEN_TALKER_PREC=bf16` y `QWEN_CP_PREC=bf16`.
- El motor rechaza con un error cualquier otro valor de esas variables.
- El proceso padre y las rutas de clonado (`voice clone`) mantienen su entorno limpio e intacto.

---

## 6. Límites de la optimización y margen restante

La adopción de `--quant-mixed` dio una ganancia de ~25–36 % de velocidad en CPU sin degradación acústica, con un RTF de producción de ~1,38×. Las optimizaciones del decodificador y la afinidad de la [sección 4.4](#44-optimización-del-decodificador-acústico-y-afinidad) restaron otro 11–25 % y el producto escalar entero de la [sección 4.5](#45-producto-escalar-entero-en-los-kernels-cuantizados) otro 18–20 %, y las optimizaciones de ancho de banda de la [sección 4.6](#46-optimizaciones-de-ancho-de-banda-de-memoria) otro 16–25 %, sin reentrenar el modelo:

| Condición | RTF final (`mixed -j 4`) |
|---|---|
| Equipo libre | **0,66** |
| 1 hilo de carga de memoria | **0,84** |
| 2 hilos saturando el ancho de banda de memoria | 1,10 |

### Reparto del tiempo por fotograma
El pipeline genera audio a 12,5 Hz (80 ms por fotograma). En cada fotograma el Talker genera 1 código acústico (`code0`) y el Code Predictor (CP) de 5 capas genera secuencialmente los 15 códigos residuales (`code1` a `code15`), con **16 × 5 = 80 evaluaciones de capa por fotograma**. Con el motor final, en el texto largo y el equipo libre, el Talker tarda 15,0 ms y el CP 33,0 ms por fotograma: el CP sigue siendo la parte dominante (~69 % de ambos), y dentro de él las multiplicaciones de pesos siguen siendo la mayor parte del tiempo.

### Conclusión y margen restante
- **El RTF < 1 se logra sin reentrenar** sin carga (0,66) y con un hilo de carga de memoria (0,84). La destilación del Code Predictor de 5 a 3 capas deja de ser condición necesaria para ese objetivo.
- **Con el ancho de banda de memoria saturado el RTF sigue por encima de 1 (1,10).** Aquí el CP queda limitado por memoria (55,7 ms por fotograma frente a 33,0 sin carga). Las FFN ya están en Q4_0; cuantizar a INT4 todo el Code Predictor provoca salidas no deterministas y truncamientos, y truncar codebooks degrada la inteligibilidad (sección 2).
- **La destilación del CP de 5 a 3 capas queda como margen adicional** (`.work/plans/plan-002-cp-3layers-distillation.md`): al leer menos pesos por fotograma reduce justamente el tráfico de memoria que limita el caso saturado, y da más holgura bajo carga de CPU.
