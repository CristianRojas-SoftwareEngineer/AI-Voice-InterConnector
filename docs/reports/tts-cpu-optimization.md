# Informe de optimización de síntesis Qwen3-TTS 0.6B en CPU: Diagnóstico, adopción de cuantización mixta y línea base de producción

**Fecha de consolidación:** noviembre de 2026.  
**Modelo:** `Qwen3-TTS-12Hz-0.6B-Base` (clonación cero-shot con `--graft`) y `Qwen3-TTS-12Hz-0.6B-CustomVoice` (presets).  
**Hardware de referencia:** AMD Ryzen 7 5825U (8 núcleos / 16 hilos, AVX2/FMA, sin AVX-512 ni VNNI), 31,4 GB RAM, Windows 11.  
**Estado:** Integrado y adoptado en producción (`crates/avi-tts`, CLI y daemon).

---

## Tabla de contenidos

1. [Resumen ejecutivo y estado de producción](#1-resumen-ejecutivo-y-estado-de-producción)
2. [Diagnóstico experimental y descarte de alternativas](#2-diagnóstico-experimental-y-descarte-de-alternativas)
3. [Metodología de medición y evaluación de calidad](#3-metodología-de-medición-y-evaluación-de-calidad)
4. [Resultados empíricos consolidados](#4-resultados-empíricos-consolidados)
   - [4.1. Rendimiento y latencia](#41-rendimiento-y-latencia)
   - [4.2. Calidad acústica e inteligibilidad](#42-calidad-acústica-e-inteligibilidad)
   - [4.3. Evaluación subjetiva de escucha ciega](#43-evaluación-subjetiva-de-escucha-ciega)
5. [Arquitectura de integración en el producto](#5-arquitectura-de-integración-en-el-producto)
   - [5.1. Perfil predeterminado y reversibilidad (`AVI_TTS_PRECISION`)](#51-perfil-predeterminado-y-reversibilidad-avi_tts_precision)
   - [5.2. Aislamiento del entorno del residente](#52-aislamiento-del-entorno-del-residente)
6. [Límites de la optimización y puente hacia el Plan 002](#6-límites-de-la-optimización-y-puente-hacia-el-plan-002)

---

## 1. Resumen ejecutivo y estado de producción

En la configuración histórica de producción, el proceso servidor residente de inferencia de Qwen3-TTS se ejecutaba en formato de punto flotante de 16 bits (`BF16`) con 4 hilos de CPU (`-j 4`). Bajo esa configuración, la síntesis representaba el principal cuello de botella de la aplicación, con un factor de tiempo real mediano de **1,84× RTF** (generar 1 segundo de voz toma ~1,84 segundos).

Tras una campaña exhaustiva de medición y evaluación de calidad, se adoptó formalmente la **cuantización mixta (`--quant-mixed`)** como perfil predeterminado en `crates/avi-tts`:
- **Talker (28 capas):** Cuantizado a **INT4** ($Q4\_0$), reduciendo sustancialmente el coste de generación de tokens sin afectar la inteligibilidad fonética.
- **Code Predictor (5 capas):** Mantenido en **INT8**, evitando la inestabilidad y truncamientos que provoca la cuantización global a INT4.
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

---

## 2. Diagnóstico experimental y descarte de alternativas

Durante la campaña se evaluaron de forma aislada múltiples factores sobre el motor nativo en C (`vendor/qwen3-tts`). Los veredictos arquitectónicos definitivos son:

| Optimización evaluada | Decisión | Fundamento empírico |
|---|---|---|
| **Cuantización mixta (`--quant-mixed`)** | **ADOPTADA** | Acelera un ~25 % en servidor (4 hilos) y ~36 % en CLI directa. Cero degradación perceptible en escucha ciega ($p = 0,5$). Cero truncamientos en >150 peticiones continuas. |
| **Cuantización INT4 global (`--int4`)** | **DESCARTADA** | En el modelo 0.6B, cuantizar el Code Predictor a INT4 genera salidas no deterministas y truncamientos severos de audio. |
| **Truncamiento de codebooks ($K=12, 10, 8$)** | **DESCARTADO** | $K=12$ y $K=10$ degradan la inteligibilidad (WER +1,55 pp y +2,78 pp) y alteran la identidad vocal. $K=8$ no termina de sintetizar en 10 de cada 100 casos (caídas de hasta 0 puntos en MUSHRA). |
| **Pool de hilos Win32 con espera activa** | **DESCARTADO** | Razón de tiempo 0,979; no alcanza el umbral de mejora y eleva la latencia mediana de 1,69 s a 2,03 s en 4 hilos. |
| **Proyecciones QKV INT8 fusionadas** | **DESCARTADO** | Aceleración despreciable (0,996) e inconsistencias numéricas dependientes del número de hilos. |
| **Prefill B=2 del Code Predictor / `matmat_bf16`** | **DESCARTADO** | Impacto neutro o negativo en procesadores sin extensiones AVX-512 / VNNI. |

---

## 3. Metodología de medición y evaluación de calidad

La campaña de validación empleó un corpus estructurado de 35 textos (25 en español y 10 en inglés) y 5 identidades de voz (voz del usuario `default` y 4 voces clonadas de hablantes reales del dataset SLR61).

### Métricas objetivas
- **Inteligibilidad (ASR WER):** Transcripción automática con Parakeet TDT v3 (minúsculas, sin diacríticos).
- **Identidad vocal (WavLM-SV / ECAPA):** Similitud coseno mediante `microsoft/wavlm-base-plus-sv` y el extractor ECAPA nativo del motor.
- **Duración y F0:** Variación de duración en muestras y contorno fundamental en semitonos mediante `librosa.pyin`.

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
- **Para `mixed`:** `QWEN_TALKER_PREC=int4`, `QWEN_CP_PREC=int8`, `QWEN_CP_LMHEAD_PREC=""`, `QWEN_CP_LAYER_PREC=""`, `QWEN_CP_Q2_FFN=""`.
- **Para `bf16`:** `QWEN_TALKER_PREC=bf16`, `QWEN_CP_PREC=bf16` y vaciado de variables diferenciales.
- El proceso padre y las rutas de clonado (`voice clone`) mantienen su entorno limpio e intacto.

---

## 6. Límites de la optimización y puente hacia el Plan 002

La adopción de `--quant-mixed` representó una ganancia inmediata y gratuita de ~25–36 % de velocidad en CPU sin degradación acústica. Sin embargo, **establece el límite de lo que puede lograrse mediante cuantización de pesos y parámetros de servidor**:

$$\text{RTF actual en producción: } \mathbf{\sim 1,38\times} \quad (\text{Sintetizar 1 s de audio toma } \sim 1,38\text{ s})$$

### El cuello de botella estructural
El análisis detallado de tiempos por fotograma reveló que el pipeline genera audio a 12,5 Hz (80 ms por fotograma). En cada fotograma:
- El Talker genera 1 código acústico (`code0`).
- El **Code Predictor (CP)** de 5 capas genera secuencialmente los 15 códigos residuales (`code1` a `code15`), requiriendo **$16 \times 5 = 80$ evaluaciones de capa por fotograma**.
- El CP consume **61 a 70 ms por fotograma**, representando más del **60–68 % del tiempo total de cómputo**.

### Conclusión y hoja de ruta
Para alcanzar el objetivo definitivo de síntesis en tiempo real (**$\text{RTF} < 1,0$**) en CPU de consumo, no es viable aplicar más cuantización ni truncar codebooks. La solución matemática y algorítmica requerida es **reducir la profundidad del Code Predictor de 5 a 3 capas Transformer** mediante destilación de conocimiento a costo $0, tal como prescribe el **Plan 002** (`.work/plans/plan-002-cp-3layers-distillation.md`).
