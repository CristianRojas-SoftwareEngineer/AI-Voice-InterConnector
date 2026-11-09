# Optimización del Code Predictor de Qwen3-TTS para CPU: investigación consolidada y hoja de ruta revisada

**Fecha de corte:** 9 de noviembre de 2026  
**Implementación de referencia:** [`gabriele-mastrapasqua/qwen3-tts`](https://github.com/gabriele-mastrapasqua/qwen3-tts)  
**Formato de implementación objetivo:** motor de inferencia C, CPU convencional y ruta portable sin extensiones de hardware obligatorias  
**Estado del trabajo:** análisis de arquitectura, código, documentación y antecedentes públicos. No se han realizado nuevos benchmarks sobre el equipo de destino ni se ha entrenado un checkpoint modificado.

---

## Resumen ejecutivo

Esta investigación examina cómo reducir el *real-time factor* (RTF) de **Qwen3-TTS**, utilizado principalmente para síntesis de **español e inglés**, manteniendo como motor de despliegue la implementación en C de Gabriele Mastrapasqua. La prioridad es que el sistema funcione bien en **CPU tradicional**, sin GPU, CUDA, TensorRT, Apple Silicon ni instrucciones SIMD particulares como requisito. Se pueden aprovechar optimizaciones de CPU opcionales cuando existan, pero siempre debe permanecer una ruta portable.

El **Code Predictor** es un cuello de botella relevante: genera **15 códigos residuales RVQ** por frame, condicionados por un código inicial producido por el *Talker*. Al trabajar aproximadamente a **12,5 frames por segundo**, debe resolver alrededor de **187,5 códigos residuales por segundo de audio**, mediante una secuencia interna autorregresiva. El predictor original utiliza **cinco capas Transformer**. El motor C ya tiene cuantización, kernels especializados con fallback, caché local, buffers persistentes, paralelismo e incluso un prefill agrupado para sus dos posiciones iniciales. Por tanto, no conviene rehacer esas capacidades: hay que **medirlas, afinarlas y aprovecharlas** [R1–R4].

La investigación conduce a **dos líneas complementarias**, que no deben confundirse:

1. **Aceleración sin reentrenamiento:** explotar las opciones de precisión por componente y por capa ya presentes; estudiar localidad de pesos, paralelismo adaptativo y, condicionalmente, *speculative decoding* con verificación causal por lotes.
2. **Compresión estructural mediante destilación:** estudiar reducciones progresivas del Code Predictor de **5→4→3 capas**. Es una hipótesis de alto interés para CPU porque disminuye el número de bloques Transformer ejecutados y el volumen de pesos de ese componente; **sí exige modificar la arquitectura y recuperar calidad mediante entrenamiento**.

**Decisión recomendada:** comenzar por perfilado y experimentos de bajo costo con las capacidades existentes; abrir **en paralelo** un estudio de viabilidad de poda de capas y un estudio *go/no-go* de speculative decoding. Si se dispone de recursos de entrenamiento, la **poda estructurada + destilación** merece una prioridad de investigación **igual o superior a la implementación completa de speculative decoding**, dadas las incertidumbres de aceptación y de costo de verificación en CPU. La *sparsity* dinámica queda como exploración de menor prioridad.

No existe evidencia verificable, al corte de este reporte, de un **checkpoint público de Qwen3-TTS con Code Predictor de tres capas, destilado para español e inglés y validado sobre el motor C objetivo**. Tampoco hay un benchmark público que demuestre un speedup neto de speculative decoding sobre esta implementación C en CPU genérica. Cualquier RTF futuro en este documento es una **proyección condicionada**, no un resultado medido.

---

## 1. Objetivo, contexto y requisitos

### 1.1. Origen de la hipótesis

El punto de partida fue buscar un fine-tuning de Qwen3-TTS Base especializado en **español e inglés** con la expectativa de que la especialización permitiera una inferencia más rápida. El análisis mostró que **limitar los idiomas no reduce por sí solo la cantidad de capas, la complejidad del Transformer ni los pasos autorregresivos**. El ahorro debe proceder de una modificación demostrable del modelo o de su ejecución.

De ahí surgió la hipótesis concreta: **reducir el Code Predictor de cinco a tres capas Transformer**, mediante poda estructurada y, si es necesario, destilación para recuperar calidad en español e inglés. La búsqueda evolucionó hacia optimizaciones sin reentrenamiento, pero esa preferencia debe entenderse como una **vía de menor costo**, no como un veto definitivo a entrenar un predictor comprimido.

### 1.2. Requisitos obligatorios del despliegue

| Dimensión | Requisito |
|---|---|
| Motor | Mantener `gabriele-mastrapasqua/qwen3-tts` como base principal. |
| Lenguaje de ejecución | C; no introducir Python como dependencia de inferencia. |
| Hardware | CPU convencional; no depender de GPU, aceleradores, marcas o ISA particulares. |
| Portabilidad | Ruta escalar/portable siempre disponible; SIMD y BLAS acelerado opcionales. |
| Función | Síntesis correcta en español e inglés; preservar clonación de voz y demás modalidades que realmente se utilicen. |
| Objetivo | Reducir RTF total, latencia inicial y tiempo por frame del Code Predictor, sin degradación inaceptable de calidad. |
| Reproducibilidad | Comparar mismo hardware, pesos, precisión, hilos, corpus, voz y modo de ejecución. |

**Distinción crítica:** la **inferencia** debe mantenerse portable y solo-CPU; una línea de **entrenamiento o destilación offline** podría emplear herramientas diferentes y hardware de entrenamiento si está disponible, **sin trasladar esa dependencia al producto desplegado**. Si también se prohibiera el uso de GPU durante el entrenamiento, la línea de destilación seguiría siendo conceptualmente posible, pero tendría un costo computacional considerablemente mayor; esa condición se debe evaluar separadamente.

### 1.3. Criterios para aceptar una optimización

Una propuesta solo se recomienda para producción si: (i) reduce de forma reproducible el **RTF extremo a extremo** en CPU real; (ii) no destruye la ventaja con preparación, carga o sincronización; (iii) mantiene calidad perceptual, inteligibilidad e identidad vocal dentro de tolerancias acordadas; y (iv) permite mantener una ruta portable y una forma sencilla de revertirla.

---

## 2. Arquitectura y origen del costo

### 2.1. Flujo simplificado

```text
Texto / voz de referencia / configuración
                  │
                  ▼
                Talker
         código inicial c0
                  │
                  ▼
      Code Predictor (5 capas)
  c1 → c2 → c3 → ... → c15
                  │
                  ▼
      16 códigos RVQ por frame
                  │
                  ▼
        Decodificador acústico
                  │
                  ▼
                 Audio
```

Qwen3-TTS usa un codec de aproximadamente **12,5 frames/s** y **16 grupos de códigos** por frame. El *Talker* proporciona el primero; el Code Predictor completa los **15 restantes**. La definición oficial del predictor incluye, como valores por defecto, `num_hidden_layers=5`, `hidden_size=1024`, `intermediate_size=3072`, 16 cabezas de atención y ocho cabezas K/V [R5–R6].

La cantidad de **códigos residuales generados** por segundo es:

```text
12,5 frames/s × 15 códigos residuales/frame = 187,5 códigos residuales/s
```

Este número describe los **códigos producidos**, no necesariamente la cantidad exacta de llamadas de bajo nivel de cualquier implementación, porque prefill y otros trabajos internos pueden agruparse de formas distintas.

### 2.2. Por qué el predictor es costoso en CPU

Cada paso vuelve a recorrer parte importante de los pesos de las capas y de las cabezas de salida. Para una sola secuencia y dimensiones relativamente pequeñas, el **movimiento de datos a través de cachés y memoria** puede limitar más que la capacidad aritmética máxima de la CPU. Las dependencias entre códigos impiden fusionar ingenuamente los 15 pasos en una sola operación [R2].

Por ello existen dos palancas fundamentales:

- **Menor costo por paso:** menos capas/pesos, formatos más compactos, mejor localidad, menos overhead.
- **Menos rondas secuenciales efectivas:** propuestas especulativas y verificación de varias posiciones en conjunto, siempre que la aceptación compense el costo.

Estas palancas podrían combinarse, pero **sus speedups no se multiplican automáticamente**.

### 2.3. Qué confirma el código C objetivo

En [`qwen_tts_code_predictor.c`](https://github.com/gabriele-mastrapasqua/qwen3-tts/blob/main/qwen_tts_code_predictor.c) aparecen `qwen_cp_predict()`, `cp_transformer_step()`, la caché K/V local (`cp_kv_len`) y `cp_prefill2()`. La ruta de generación revisada utiliza **`argmax` por codebook** en BF16, INT8 o INT4, de modo que su verificación especulativa debe respetar inicialmente esa **semántica greedy**, aunque otros runtimes implementen sampling estocástico [R3].

La cantidad de capas se lee mediante `c->cp_num_layers` y el cargador recorre `talker.code_predictor.model.layers.%d...`. Esto facilita adaptar un predictor comprimido en cuanto al **bucle de capas**, pero **no significa** que baste cambiar un número para usar correctamente un checkpoint de cinco capas como si tuviera tres: se necesita escoger/reindexar capas, construir un checkpoint compatible y verificar el resultado [R3].

---

## 3. Evidencia de rendimiento: hechos frente a extrapolaciones

### 3.1. Desglose publicado por el motor C

Para el benchmark **0.6B BF16** del autor [R2]:

| Etapa | Texto corto | Texto largo |
|---|---:|---:|
| Prefill | 1.647 ms | ~1.040 ms |
| Talker | 24,6 ms/frame | ~22 ms/frame |
| Code Predictor | 76,3 ms/frame | ~60 ms/frame |
| Decodificación acústica | Se solapa con generación | Se solapa con generación |

El Code Predictor representa aproximadamente **73–76 % del tiempo recurrente Talker + Code Predictor** en esas mediciones. **No equivale necesariamente a ese porcentaje del tiempo total**, pues se deben incluir prefill, drenaje final del decodificador y otras etapas. En otra cuantización, CPU, longitud de texto o modalidad, la proporción puede variar.

El repositorio también publica ejemplos de RTF **0,52 para 0.6B INT4 en Apple M1** y **2,02 en Ryzen 7 6800H** con la mejor configuración de su tabla. Son **mediciones del propio proyecto en máquinas distintas**, no bases universales ni resultados del presente estudio [R2].

### 3.2. RTF y metodología

\[
RTF=\frac{\text{tiempo de ejecución hasta completar el audio}}{\text{duración del audio generado}}
\]

RTF menor que uno implica generación más rápida que tiempo real. No debe confundirse con throughput agregado de múltiples solicitudes concurrentes ni con la latencia hasta escuchar el primer fragmento.

Para un experimento válido deben informarse conjuntamente: RTF, tiempo hasta primer audio, milisegundos/frame del predictor, descomposición de tiempo del predictor, tamaño de los pesos residentes, cuantización y calidad de salida. Separar ejecución fría y caliente es obligatorio.

### 3.3. Instrumentación disponible

El código contiene mecanismos como `CP_MICROBENCH`, `QWEN_CP_PROFILE`, `QWEN_DUMP_CODES`, `QWEN_FFN_SPARSITY` y trazas de tiempo. **No debe asumirse que todos cubren idénticas rutas de ejecución**: algunas instrumentaciones distinguen generación individual y batched, y el microbenchmark puede cambiar la selección de `cp_prefill2()` [R3]. Sus mediciones sirven para diagnóstico; el benchmark final debe ejecutarse sin instrumentación intrusiva.

---

## 4. Capacidades existentes: aprovechar antes que reinventar

La implementación de Mastrapasqua **ya ofrece**:

- Cuantización **INT8/INT4**, más optimizaciones específicas disponibles para CPU;
- kernels ARM/x86 con selección por capacidades y rutas alternativas;
- buffers alineados y persistentes, `mmap`, caché local del predictor y solapamiento del decodificador;
- pool de hilos y opciones para medir configuraciones;
- `cp_prefill2()`, que procesa conjuntamente dos posiciones de entrada ya conocidas cuando la configuración lo permite;
- operaciones matriciales para pequeños lotes, útiles como referencia al construir un verificador causal.

Estas capacidades descartan como proyecto inicial una reimplementación general de cuantización, SIMD, KV Cache, sampling rápido o prefill [R1–R3].

### 4.1. Hallazgo incorporado en esta revisión: precisión configurable del predictor

El código permite **ajustar la precisión del Code Predictor independientemente del Talker**, y también variar la precisión por capa o de sus `lm_head` [R3]:

| Variable existente | Utilidad | Observaciones |
|---|---|---|
| `QWEN_CP_PREC` | Seleccionar `int8` o `int4` para el predictor | Comparar sin cambiar todo el modelo. |
| `QWEN_CP_LAYER_PREC` | Lista separada por comas para asignar `int4`, `int8` o `bf16` a las capas del predictor | Ya soportado por el código; validar calidad por capa. |
| `QWEN_CP_LMHEAD_PREC` | Seleccionar precisión de cabezas de salida | Evaluar posible sensibilidad de `argmax`. |
| `QWEN_CP_Q2_FFN` | Explorar Q2 en operaciones FFN de la ruta de cuantización correspondiente | Muy aproximado; debe validarse cuidadosamente. |
| `QWEN_CP_PREFILL2` | Controlar la ruta de prefill de dos posiciones, condicionada por formato y backend | No asumir que está habilitada o que siempre gana. |

**Implicación para las prioridades:** antes de escribir un kernel nuevo o entrenar una versión reducida, conviene construir una **frontera calidad–RTF** a partir de las configuraciones de precisión que el repositorio ya permite. Esta exploración es de **bajo costo de desarrollo**, aunque no es «sin cambios numéricos» ni garantiza mayor velocidad en cada CPU.

---

## 5. Estrategias de optimización: propuesta revisada

Separamos **orden de ejecución** (qué estudiar primero para obtener información útil con poco costo) de **valor estratégico** (qué podría ofrecer la mayor reducción estable de RTF). No son lo mismo.

| Orden operativo | Línea | Modifica pesos/arquitectura | Costo | Papel en el proyecto |
|---:|---|---|---|---|
| **0** | Baseline, perfilado y precisión por componente/capa ya disponible | Cuantiza; no cambia topología | Bajo | **Obligatorio e inmediato** |
| **1** | Localidad de pesos y paralelismo adaptativo | No | Bajo–medio | Mejoras incrementales/medibles |
| **2** | **Poda progresiva 5→4→3 + destilación** | **Sí** | Alto | **Principal investigación de compresión** |
| **3** | **Speculative decoding + verificación batched** | No | Medio–alto | **Principal investigación algorítmica sin entrenamiento**; avanzar solo tras go/no-go |
| **4** | Sparsity dinámica del FFN | No cambia pesos; aproxima cómputo | Alto para un kernel eficiente | Exploratoria, última prioridad |

**Estas filas no constituyen una promesa de speedup.** El orden operativo 2 y 3 puede transcurrir **en paralelo**. Si no existen recursos para entrenamiento offline, se pospone la línea 2 sin alterar las restricciones del motor de despliegue.

### 5.1. Línea A: precisión mixta existente + perfilado (prioridad operativa 0)

**Hipótesis:** una distribución selectiva de INT4/INT8/BF16 entre capas del predictor y cabezas de salida puede mejorar la relación RTF/calidad frente a usar una sola precisión uniforme.

**Pasos:** fijar una línea base; comparar las precisiones globales; ensayar combinaciones por capa **sin rediseñar kernels**; medir discrepancias en códigos RVQ, WER y similitud vocal; conservar solo configuraciones dominantes en la frontera de Pareto.

**Referencia directa:** `qwen_tts_code_predictor.c`, `qwen_cp_load()`, `cp_layer_quantize()`, `cp_lmhead_quantize()`, `QWEN_CP_PREC`, `QWEN_CP_LAYER_PREC`, `QWEN_CP_LMHEAD_PREC` [R3].

**Riesgos:** INT4 o Q2 pueden empeorar latencia si el desempaquetado cuesta demasiado; la cuantización puede modificar `argmax` y degradar voz en ciertas posiciones. La configuración óptima depende de CPU y corpus; la implementación del algoritmo sigue siendo portable.

### 5.2. Línea B: localidad de pesos y paralelismo por etapa (prioridad operativa 1)

**Hipótesis:** aún puede disminuirse algo del tráfico a memoria o del overhead de hilos, pese al considerable trabajo ya realizado por el autor.

**Experimentos:** identificar si los pesos del CP son continuamente expulsados por el Talker/decoder; evaluar reordenamientos de acceso, reutilización de buffers y empaquetado para pequeñas multiplicaciones; medir `-j1`, `-j2`, `-j4` y otras configuraciones adecuadas, diferenciando GEMV, FFN, atención y eventual GEMM del verificador.

**Alcance realista:** **mejoras incrementales**, no la hipótesis de un speedup grande. Aislar pesos en un proceso separado **no genera automáticamente** una mejora; debe modificar de forma favorable el conjunto residente y el patrón de accesos. `darkautism/qwen3-tts` ofrece un antecedente útil de medición, pero emplea otra arquitectura de software [R2, R10].

**Portabilidad:** los algoritmos y heurísticas pueden implementarse en C portable y elegir configuraciones de modo dinámico; no se exige afinidad fija a un chipset ni instrucciones especiales.

### 5.3. Línea C: poda estructurada 5→4→3 y destilación (prioridad estratégica alta)

**Hipótesis central:** el Code Predictor tiene suficiente redundancia estructural como para que una reducción de profundidad disminuya RTF en CPU, siempre que una destilación recupere calidad en español e inglés.

Esta es la **hipótesis original del proyecto**, y **no debe desaparecer del reporte**. Es especialmente atractiva porque el ahorro potencial ataca **cada paso de cada frame** y podría beneficiar a distintos tipos de CPU, al reducir cómputo y volumen de pesos, sin obligar a cambiar el backend.

**Qué significa exactamente «40 % menos capas»:** cinco a tres elimina **dos de cada cinco bloques Transformer**, no el 40 % del tiempo del predictor completo ni del RTF. Permanecen operaciones del predictor fuera de las capas, embeddings, cabezas RVQ, el Talker, el codec y las dependencias secuenciales. El ahorro observado puede ser menor —o diferente— según caché, cuantización y estructura del código.

**Diseño experimental:**

1. Conservar un **teacher de cinco capas** como referencia congelada.
2. Medir la importancia de las capas mediante ablaciones y error por código; comparar distintos subconjuntos, evitando seleccionar «las primeras tres» por simple conveniencia.
3. Construir estudiantes de **cuatro capas** y, posteriormente, **tres capas**; ajustar el mapeo de pesos y nombres de capas para que el cargador C sea coherente.
4. Entrenar únicamente el predictor reducido (al principio con Talker y codec congelados) usando entropía cruzada sobre códigos reales y destilación de distribuciones/estados del teacher. Verificar que la condición que recibe el estudiante coincide con la del runtime.
5. Comparar **5 vs. 4 vs. 3 capas** tanto en calidad como en latencia **en CPU**, con los mismos formatos INT8/INT4 que permita el motor; buscar el mejor punto de Pareto, no asumir que tres capas será siempre superior.
6. Validar los quince pasos autorregresivos completos y la compatibilidad de voz clonada, textos largos, español regional e inglés; no confiar exclusivamente en pérdidas bajo *teacher forcing*.

**Especialización bilingüe:** es una **hipótesis adicional**, no una garantía. El Code Predictor opera sobre códigos acústicos condicionados por el Talker; no es un clasificador directo de idiomas. El entrenamiento con español e inglés puede recuperar mejor calidad dentro de esos idiomas, pero no prueba que requiera menos profundidad. Evaluar si una mezcla bilingüe supera a una mezcla multilingüe de igual tamaño y calidad.

**Antecedente reutilizable:** [SPADE](https://github.com/ml-inory/SPADE) introduce poda de capas basada en importancia y destilación multinivel para **otros modelos LLM-TTS**. Su artículo informa reducción de profundidad y recuperación perceptual en sus experimentos, **no una validación directa del Code Predictor de Qwen3-TTS** [R8]. El [fine-tuning oficial de Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS/tree/main/finetuning) sirve como fuente para preparar códigos y adaptar entrenamiento, pero su procedimiento documentado se centra en **fine-tuning de un hablante** y no aporta automáticamente poda ni un pipeline teacher–student del predictor [R7].

**Resultado esperado de la investigación:** un checkpoint comprimido y un conversor de pesos reproducible, más una ruta C que lo cargue y ejecute sin dependencias nuevas. **No se dispone hoy de un checkpoint público validado que cumpla todas esas condiciones.**

### 5.4. Línea D: speculative decoding + verificación causal batched (prioridad condicional)

**Hipótesis:** utilizar las cabezas `lm_head` ya existentes para proponer futuros códigos residuales y verificarlos en grupos causales podría reducir rondas secuenciales sin otro modelo ni entrenamiento [R9].

**Diferenciar propuesta de verificación:** speculative decoding **solo ahorra** si el costo de obtener propuestas, verificar varios códigos, aceptar/rechazar y administrar el estado es **menor** que el costo de generar esos mismos códigos secuencialmente.

El diseño de NVIDIA aplica una cabeza de un codebook futuro al estado oculto actual para proponer códigos; verifica en una pasada y utiliza una regla exacta de aceptación en el caso de sampling probabilístico. No es un binario CPU reutilizable: **se adapta su algoritmo, no TensorRT ni sus kernels GPU** [R9].

**Condición particular del motor C:** la ruta revisada utiliza `argmax`. Para un primer prototipo **greedy**, la aceptación sería mediante comparación contra el `argmax` correcto del modelo target. No equivale automáticamente a la garantía probabilística de una implementación basada en top-k sampling; las diferencias numéricas pueden producir divergencias cerca de empates.

**Prototipo mínimo antes de un port completo:**

1. Recolectar, sobre salidas de referencia, propuestas para las siguientes profundidades RVQ usando `lm_head` existentes y estados permitidos.
2. Medir **longitud de prefijo aceptado** por ventana de tamaño 2, 3 y 4; separar por profundidad y por corpus español/inglés.
3. Microbenchmarkear una ruta C que ejecute una **verificación causal por lotes** para entradas conocidas, con matrices pequeñas y conservación/rollback del estado K/V.
4. Evaluar el equilibrio entre costos reales: propuesta + verificación + manejo de caché vs. pasos del bucle convencional.
5. Solo si supera el umbral experimental, integrar una ruta de ejecución opcional con fallback al CP actual.

**Antecedentes positivos:** NVIDIA documenta un método sin modelo auxiliar; el artículo **RVQ Position Aware Speculative Decoding for On Device Text to Speech** informa aceleraciones de **2–2,2× en generación de tokens RVQ** sobre dispositivos Apple evaluados con otra estrategia, **no como resultado transferible a C/CPU genérica** [R9, R12].

**Antecedente negativo:** un experimento de SGLang-Omni sobre Qwen3-TTS 1.7B, H100 y un draft Transformer entrenado informa **2,46 pasos aceptados por ronda**, frente a un umbral de rentabilidad **7,35**, y **0,33×** de speedup: un *slowdown* cercano a 3× en ese escenario. Es una evidencia de riesgo, no la refutación de NVIDIA ni un resultado de la ruta greedy en CPU [R11].

**Decisión:** línea de investigación de **alto potencial, pero alta incertidumbre**. No invertir primero en un port completo sin datos de aceptación y costo de GEMM pequeño en el motor real.

### 5.5. Línea E: sparsity dinámica del FFN (baja prioridad)

**Hipótesis:** ciertas activaciones post-SwiGLU cercanas a cero podrían omitirse para disminuir trabajo.

**Evidencia disponible:** el motor incorpora `QWEN_FFN_SPARSITY` para medir valores bajo un umbral [R3]. **No es, por sí sola, un kernel sparse acelerado.** Activaciones pequeñas no son exactamente nulas; saltarse contribuciones altera logits y puede cambiar códigos posteriores.

**Primer paso:** medir sparsity por capa e idioma; luego estimar ahorro neto tras selección de índices, accesos irregulares a pesos y sincronización. Si no hay margen claro, no desarrollar kernels complejos. Considerarla después de las líneas A–D.

---

## 6. Estimación de RTF: límites matemáticos y escenarios

### 6.1. Ley de Amdahl aplicada a cualquier estrategia sobre CP

Definamos:

- `R0`: RTF original en la **misma CPU y configuración**.
- `f`: fracción del tiempo **extremo a extremo** atribuible al Code Predictor.
- `r`: reducción **efectiva y medida** del tiempo de ese predictor.

Entonces, como aproximación (sin nuevos solapamientos ni costos ocultos):

\[
R_{nuevo}=R_0(1-f\,r).
\]

Ejemplo **ilustrativo**, no benchmark: `R0=0,80` y `f=0,70`.

| Reducción efectiva del tiempo CP | RTF nuevo ilustrativo | Cambio del RTF total |
|---:|---:|---:|
| 10 % | 0,744 | −7 % |
| 20 % | 0,688 | −14 % |
| 30 % | 0,632 | −21 % |
| 40 % | 0,576 | −28 % |

La cifra `f=0,70` es una **hipótesis de sensibilidad** y no debe copiarse como fracción universal: depende de hardware, precisión, texto y del solapamiento con el decodificador.

### 6.2. Caso específico: reducir de cinco a tres capas

Introduzcamos `p`: fracción del tiempo del **Code Predictor** atribuible a los **bloques Transformer que realmente se reducen**. Si su tiempo descendiera linealmente al pasar de cinco a tres capas, el costo de esos bloques disminuiría un 40 %. Una primera aproximación sería:

\[
R_{3\,capas}\approx R_0[1-f\,p\,(1-3/5)]
=R_0(1-0,4fp).
\]

Con `R0=0,80`, `f=0,70` y escenarios para `p`:

| Proporción del tiempo CP en capas (`p`) | Reducción estimada de RTF total | RTF ilustrativo |
|---:|---:|---:|
| 60 % | 16,8 % | 0,666 |
| 75 % | 21,0 % | 0,632 |
| 90 % | 25,2 % | 0,598 |
| 100 % (límite ideal) | 28,0 % | 0,576 |

**Corrección fundamental frente a la interpretación inicial:** `0,576` corresponde al **límite idealizado** en que todo el tiempo del predictor escala con las capas, no a un RTF previsto de un modelo destilado. El valor real puede ser distinto por la participación de `lm_head`, embeddings, cuantización, localidad, caché y overhead; también puede haber mejoras no lineales si los pesos pasan a caber en niveles de caché antes inaccesibles.

### 6.3. Qué no es válido estimar sin experimentos

No existen datos suficientes para asignar **rangos confiables de RTF** a localidad de memoria, paralelismo, sparsity o speculative decoding en este fork. Los porcentajes usados en conversaciones exploratorias anteriores eran escenarios hipotéticos; **no se incorporan como pronósticos**.

Tampoco debe sumarse un supuesto 20 % por poda con otro 20 % por especulación: ambas técnicas interactúan y requieren medición conjunta. Un modelo comprimido puede alterar la calidad de las propuestas y la aceptación especulativa.

---

## 7. Proyectos y código en los que basarse

| Referencia | Qué aporta | Qué se puede reutilizar | Qué NO demuestra |
|---|---|---|---|
| **Mastrapasqua / qwen3-tts** [R1–R3] | Base C, CP, formatos, cache, `cp_prefill2`, precisión mixta | **Código directamente compatible**, instrumentación, bucles de ejecución | Que las nuevas mejoras propuestas ya estén validadas |
| **QwenLM / Qwen3-TTS** [R5–R7] | Arquitectura oficial, configuraciones, preparación de códigos y fine-tuning | Contratos del modelo y datos; base de adaptación del teacher/student | Pipeline ya preparado para 5→3 capas |
| **SPADE** [R8] | Poda estructurada por importancia y destilación en LLM-TTS | **Método de investigación y entrenamiento** | Speedup y calidad específicos de Qwen3-TTS/CPU |
| **NVIDIA TensorRT Edge-LLM** [R9] | Speculative decoding de CP sin draft adicional | Algoritmo de propuesta, verificación y rechazo | Port C/CPU o rendimiento en este motor |
| **llama.cpp** [R13] | Ingeniería de speculative decoding y batching sobre CPU | Gestión de ramas, estado, aceptación, pruebas | Compatibilidad directa con codebooks RVQ; código es principalmente C++ |
| **SGLang-Omni, issue #3583** [R11] | Experimento negativo para otro draft y runtime | Diseño de criterio de rentabilidad y medición | Inviabilidad universal de la especulación |
| **darkautism/qwen3-tts** [R10] | Aislamiento y medición del predictor en Rust | Ideas para ensayos de localidad y memoria | Ganancia automáticamente transferible al motor C |
| **qwen3-tts.cpp / qwentts.cpp** [R14] | Implementaciones alternativas GGML/C++ | Comparar diseños y rutas CPU | Necesidad de migrar de motor ni métricas comparables entre plataformas |

### Módulos concretos de partida

**Para la optimización inmediata:**

- [`qwen_tts_code_predictor.c`](https://github.com/gabriele-mastrapasqua/qwen3-tts/blob/main/qwen_tts_code_predictor.c): `qwen_cp_load`, `cp_layer_quantize`, `cp_lmhead_quantize`, `cp_transformer_step`, `cp_prefill2`, `qwen_cp_predict`.
- [`docs/performance.md`](https://github.com/gabriele-mastrapasqua/qwen3-tts/blob/main/docs/performance.md): benchmarks y sensibilidad a CPU, precisión y número de hilos.

**Para la destilación 5→4→3:**

- [Qwen3-TTS: configuración del Code Predictor](https://github.com/QwenLM/Qwen3-TTS/blob/main/qwen_tts/core/models/configuration_qwen3_tts.py).
- [Qwen3-TTS: fine-tuning oficial](https://github.com/QwenLM/Qwen3-TTS/tree/main/finetuning).
- [SPADE: repositorio](https://github.com/ml-inory/SPADE) y [artículo](https://arxiv.org/abs/2509.20802).

**Para speculative decoding:**

- [NVIDIA: CodePredictor Speculative Decoding](https://nvidia.github.io/TensorRT-Edge-LLM/latest/user_guide/features/codepredictor-speculative-decoding.html).
- [llama.cpp: `common/speculative.cpp`](https://github.com/ggml-org/llama.cpp/blob/master/common/speculative.cpp).
- [SGLang-Omni: experimento y criterios go/no-go](https://github.com/sgl-project/sglang-omni/issues/3583).

La decisión de reutilización es **asimétrica**: el código C de Mastrapasqua se puede reutilizar directamente; SPADE es una **plantilla metodológica de entrenamiento**; NVIDIA y llama.cpp ofrecen **algoritmos y diseños para adaptar**, no componentes listos para pegar en el runtime C.

---

## 8. Hoja de ruta ejecutable y criterios de decisión

### Etapa 0 — Baseline (obligatoria)

**Objetivo:** establecer qué porción del RTF se puede ahorrar realmente.

- Congelar versión/commit, modelo 0.6B o 1.7B, CPU, compilador, opciones de compilación, hilos y formato de pesos.
- Construir un conjunto reproducible con español (incluyendo variaciones regionales), inglés y code-switching; textos cortos, largos, números, siglas y pausas; varias voces de referencia si se usa clonación.
- Medir RTF total, TTFA, CP ms/frame, Talker ms/frame, decodificador, consumo de memoria, códigos RVQ y WER. Repetir para ejecución fría y caliente.
- Utilizar instrumentación del repositorio y desactivarla para los benchmarks finales.

**Salida exigida:** una tabla de línea base por CPU/formato/voz/idioma y el desglose de tiempo en el equipo real.

### Etapa 1 — Optimización sin desarrollo estructural

**Objetivo:** explorar la frontera RTF/calidad con capacidades ya existentes.

- Comparar BF16, INT8 e INT4 según los formatos soportados, sin presuponer que INT4 siempre será superior.
- Experimentar con `QWEN_CP_PREC`, `QWEN_CP_LAYER_PREC`, `QWEN_CP_LMHEAD_PREC` y, si tiene sentido, las opciones Q2 FFN.
- Verificar la ruta real de `cp_prefill2`; comparar hilos y perfiles del CP.
- Retener configuraciones que mejoren RTF **sin degradación perceptual o funcional inaceptable**.

**Decisión:** si ya se obtiene el RTF objetivo, evitar complejidad adicional; si no, continuar.

### Etapa 2A — Factibilidad de poda/destilación (en paralelo)

**Objetivo:** determinar si la reducción 5→4→3 tiene margen de calidad y de velocidad.

- Ensayar ablaciones sin entrenamiento para seleccionar capas importantes; **no** tratar su calidad como resultado final.
- Producir un estudiante de cuatro capas y evaluar su RTF en C con pesos cargables.
- Construir entrenamiento teacher–student sobre códigos y estados del predictor, inicialmente con Talker/codec congelados.
- Solo si 4 capas resulta prometedor, estudiar 3 capas; comparar calidad española/inglesa y clonación de voz.
- Probar estudiante con BF16/INT8/INT4 en CPU, porque los efectos de poda y cuantización pueden interactuar.

**Go/no-go:** continuar si existe una mejora **clara** de RTF extremo a extremo y una recuperación de calidad suficiente respecto del teacher. Si el modelo de cuatro capas ofrece mejor Pareto que el de tres, conservar cuatro.

### Etapa 2B — Factibilidad de speculative decoding (en paralelo)

**Objetivo:** descartar pronto una vía de alto costo de implementación si la aceptación es insuficiente.

- Instrumentar propuestas sin cambiar la salida efectiva de producción.
- Medir aciertos de propuestas greedy por profundidad RVQ y ventana 2/3/4.
- Microbenchmarkear un verificador causal de 2–4 posiciones usando los formatos CPU del repositorio.
- Calcular costo por código **correctamente avanzado** —incluidos rechazos y rollback— frente al bucle original.

**Go/no-go:** implementar versión productiva solo si existe margen positivo suficientemente amplio para superar el overhead y reducir el RTF total, no solo las llamadas al Transformer.

### Etapa 3 — Localidad y paralelismo

**Objetivo:** mejorar las rutas de mayor gasto observadas en los perfiles. Comparar configuración actual y cambios propuestos con pruebas A/B; no rediseñar el pool ni el layout de pesos sin evidencia de problemas reales.

### Etapa 4 — Sparsity opcional y combinación

Solo investigar kernels sparse si los diagnósticos revelan abundantes activaciones aprovechables y el ahorro calculado supera los costos de selección. Combinar estrategias únicamente después de validar cada una, reevaluando calidad y speedup conjunto.

### Criterios de aceptación global

| Métrica | Cómo utilizarla |
|---|---|
| RTF extremo a extremo | **Métrica principal**; exige comparación en misma CPU y modo. |
| TTFA | Asegurar que la mejora sostenida no empeora demasiado la primera salida. |
| CP ms/frame y ms/paso | Localizar y validar el mecanismo de mejora. |
| Corrección | Igualdad de códigos cuando se promete equivalencia; evaluación funcional cuando se cambia precisión o modelo. |
| Calidad | Escucha ciega, WER en dos idiomas, identidad vocal y prosodia, acentos y code-switching. |
| Memoria | Peso residente y pico de RAM del modelo, no solo tamaño del archivo. |
| Portabilidad | Ejecutar ruta CPU genérica en al menos arquitecturas representativas; no exigir GPU ni ISA especial. |

---

## 9. Riesgos, incertidumbres y límites

**Riesgo 1 — confundir reducción de capas con reducción lineal del RTF.** La poda disminuye bloques, pero existe trabajo no podado. Es imprescindible medir `p` (participación real de bloques) y `f` (participación del predictor en el total).

**Riesgo 2 — perder calidad tras poda.** Un predictor de cinco capas ya es un modelo relativamente compacto. Es posible que tres capas no recuperen calidad de forma aceptable aun con destilación; cuatro pueden constituir un mejor compromiso. No hay garantía de que restringir entrenamiento a español/inglés resuelva esto.

**Riesgo 3 — speculative decoding más lento en CPU.** La predicción futura puede ser poco exacta y la verificación mat-mat pequeña puede costar demasiado. Hay antecedentes positivos bajo implementaciones específicas y negativos con otro draft; **ninguno sustituye pruebas en el motor destino**.

**Riesgo 4 — ajustes de precisión que degradan códigos.** INT4, Q2 y precisión mixta ya existen, pero menor tamaño no garantiza menor RTF ni fidelidad. Especificar tolerancias y conservar un control BF16/INT8 de calidad.

**Riesgo 5 — exceso de optimizaciones redundantes.** El motor ya está trabajado para CPU; localidad, hilos, caché y kernels deben investigarse a partir del perfil real, no mediante reimplementación preventiva.

**Riesgo 6 — confundir entrenamiento y despliegue.** El producto final puede ser estrictamente C/CPU aunque el trabajo offline de destilación requiera otro stack. Esta distinción mantiene la portabilidad de ejecución sin cerrar injustificadamente la línea de compresión.

---

## 10. Conclusiones definitivas y prioridades

1. **La reducción 5→3 sigue recomendada como investigación**, pero planteada en forma **5→4→3 con poda por importancia y destilación**. Es la opción más directamente relacionada con reducir pesos y trabajo por paso en cualquier CPU; no es una mejora asegurada y requiere entrenamiento.
2. **No conviene comenzar escribiendo un port completo de speculative decoding.** La oportunidad sigue abierta y NVIDIA ofrece un diseño específico para Qwen3-TTS, pero una prueba temprana de aceptación y costo de verificación debe determinar si merece desarrollarse en C.
3. **La primera acción ejecutable es aprovechar capacidades que ya existen.** En particular, el motor permite ajustar precisión de Code Predictor, capas y cabezas de salida por separado; esto se había subestimado en el reporte anterior.
4. **Localidad de pesos y paralelismo adaptativo** son líneas razonables de mejora incremental, pero el margen es incierto porque el motor ya tiene optimizaciones importantes.
5. **Sparsity dinámica** queda en último lugar por su baja evidencia de ganancia neta y mayor riesgo de cambios de salida.
6. **Especializar español e inglés es una hipótesis de calidad/destilación, no una explicación automática de velocidad.** Lo que puede acelerar es el modelo resultante, si realmente requiere menos trabajo.

### Orden sintético propuesto

**Si se permite entrenamiento offline:** `baseline → precisión y tuning existentes → (poda/destilación 5→4→3 || prueba go/no-go especulativa) → localidad/paralelismo → integración y validación → sparsity opcional`.

**Si no se permite ningún reentrenamiento:** `baseline → precisión y tuning existentes → localidad/paralelismo → prueba go/no-go especulativa → integración solo si mejora → sparsity opcional`.

La prioridad final no es demostrar que una técnica «debería» ser rápida, sino alcanzar un **RTF inferior y reproducible en CPU convencional, con español e inglés de calidad**, manteniendo como motor principal la implementación en C de Mastrapasqua.

---

## 11. Referencias y material técnico

Las fuentes siguientes documentan los hechos, pero el texto anterior incorpora el contexto, interpretación, supuestos, riesgos y criterios necesarios para entender el análisis sin consultarlas.

- **[R1]** Gabriele Mastrapasqua, *qwen3-tts*, motor puro C: https://github.com/gabriele-mastrapasqua/qwen3-tts
- **[R2]** Mastrapasqua, *Performance: benchmarks y optimizaciones CPU*: https://github.com/gabriele-mastrapasqua/qwen3-tts/blob/main/docs/performance.md
- **[R3]** Mastrapasqua, código del predictor (`qwen_tts_code_predictor.c`): https://github.com/gabriele-mastrapasqua/qwen3-tts/blob/main/qwen_tts_code_predictor.c
- **[R4]** Mastrapasqua, documentación general y opciones de compilación: https://github.com/gabriele-mastrapasqua/qwen3-tts#readme
- **[R5]** Qwen, configuración oficial del Code Predictor: https://github.com/QwenLM/Qwen3-TTS/blob/main/qwen_tts/core/models/configuration_qwen3_tts.py
- **[R6]** Qwen3-TTS Technical Report: https://arxiv.org/abs/2601.15621
- **[R7]** Qwen3-TTS, guía oficial de fine-tuning: https://github.com/QwenLM/Qwen3-TTS/tree/main/finetuning
- **[R8]** Nguyen et al., *SPADE: Structured Pruning and Adaptive Distillation for Efficient LLM-TTS*, ICASSP 2026: https://arxiv.org/abs/2509.20802 ; código: https://github.com/ml-inory/SPADE
- **[R9]** NVIDIA TensorRT Edge-LLM, *CodePredictor Speculative Decoding*: https://nvidia.github.io/TensorRT-Edge-LLM/latest/user_guide/features/codepredictor-speculative-decoding.html
- **[R10]** `darkautism/qwen3-tts`, proyecto distribuido Rust, referencia de memoria: https://github.com/darkautism/qwen3-tts
- **[R11]** SGLang-Omni, issue #3583, resultados *no-go* de speculative decoding en Qwen3-TTS 1.7B: https://github.com/sgl-project/sglang-omni/issues/3583
- **[R12]** Durmus et al., *RVQ Position Aware Speculative Decoding for On Device Text to Speech*, septiembre 2026: https://arxiv.org/abs/2609.37007
- **[R13]** `ggml-org/llama.cpp`, lógica especulativa CPU/C++: https://github.com/ggml-org/llama.cpp/blob/master/common/speculative.cpp
- **[R14]** Implementaciones alternativas en GGML/C++: https://github.com/CHE3MZ/qwen3-tts.cpp y https://github.com/ServeurpersoCom/qwentts.cpp

**Nota metodológica final:** la investigación revisa fuentes públicas y código visible a fecha de corte. No sustituye la lectura del commit exacto usado en un fork ni los benchmarks experimentales en la CPU destino. Cualquier velocidad futura se debe contrastar contra el mismo binario y entorno de referencia.
