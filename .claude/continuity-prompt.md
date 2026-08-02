# Prompt de continuidad — Spike: clonación cross-lingual español→inglés neutro (4 candidatos)

## Objetivo activo
Implementar y ejecutar un spike (experimento desechable, en scratchpad, sin tocar el repo ni la caché HF real) que compare 4 modelos Chatterbox para el objetivo del usuario: **clonar el timbre/forma de habla de una narración en español y sintetizar inglés lo más NEUTRO posible (sin arrastrar acento españolizado) a partir de texto arbitrario**. Es clonación cross-lingual, NO un traductor (no requiere ASR/MT). Al terminar: reportar (Fase 5), ofrecer limpieza del scratchpad, y NO tocar el repo sin confirmación explícita.

## Progreso verificado
- **Requisito del usuario**: inglés neutro, sin acento español; timbre reconocible deseable; fuente = texto en inglés (no habla en tiempo real).
- **El repo hoy bloquea cross-lingual**: `synthesis.py:92` fija `language_id="es"` (única aparición en `src/`); solo carga el pack `es-mx-latam`. Habilitarlo = cambio de código.
- **`chatterbox-tts==0.1.7`** instalado en Python global `C:\Users\Cristian\AppData\Local\Programs\Python\Python313\Lib\site-packages\chatterbox` (el `python` del PATH lo tiene; hay `.venv/` pero no hace falta). Submódulos: `models, mtl_tts, tts, tts_turbo, vc`. **Turbo soportado en 0.1.7** — no subir versión.
- **es-mx-latam ya cacheado** (read-only) en `~/.cache/huggingface/hub/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam/snapshots/27e595bf2fe7be0533ca299d9afafcde08b7cca7/`. `ve.safetensors` del base en `models--ResembleAI--chatterbox/snapshots/5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18/ve.safetensors`.

## Especificación cerrada del experimento (sin decisiones abiertas)
**Pregunta única del spike**: con la voz de referencia en español, ¿cuál de los 4 candidatos genera inglés con acento más neutro conservando timbre reconocible, a nivel aceptable para el usuario? (Métrica perceptual → el usuario escucha los WAV; no hay métrica automática.)

**Variable independiente**: el modelo/checkpoint. **Dependiente**: neutralidad del acento + preservación de timbre (juicio del usuario). **Controladas** (idénticas en todas las condiciones): misma referencia (`ref` = `timbre-reference.wav`, ver abajo), mismo texto, `cfg_weight=0.0`, `temperature=0.7`, `torch.manual_seed(1234)` antes de cada `generate`, device `cpu`. **`exaggeration` NO es constante**: cada modelo corre en su punto neutral (0.5 en cand. 1-3; 0.0 en Turbo), ver «Decisiones y trade-offs cerrados». Resto de knobs (`repetition_penalty`/`min_p`/`top_p`/`top_k`) en su default por modelo.

**Referencia (aprobada por el usuario)**: voz `default` del repo. Rutas confirmadas: `src/tts_sidecar/voices/default/timbre-reference.wav` (10 MB, narración completa) + `speech-reference.wav` (1.9 MB, clip corto). **`audio_prompt_path` = `timbre-reference.wav` en los 4 candidatos** (variable `ref`). Razón: los candidatos 2/3/4 usan el `generate(audio_prompt_path=...)` vanilla de chatterbox, que hace TODO el conditioning (Voice Encoder + T3 + S3Gen) desde un único archivo; para que el modelo sea la única variable, los 4 reciben el mismo archivo. Se elige `timbre-reference.wav` (el más largo) porque el Voice Encoder usa el audio completo y T3/S3Gen toman sus primeros ~6-10 s → mejor captura de timbre. **Trade-off cerrado**: esto NO replica el conditioning de dos archivos del repo (`conditionals.py:32-33`: timbre→VE completo, speech→T3 6s + S3Gen 10s); es aceptable porque el spike aísla el MODELO, no el pipeline de conditioning, y así el baseline es comparable a 2/3/4. (Nota: el `_load_es_latam` carga `conds.pt` incorporado si existe, pero pasar `audio_prompt_path` explícito lo sobrescribe vía `prepare_conditionals`, así que el baseline clona de nuestra `ref`, no de la voz incorporada.)

**Texto en inglés (fijo, <300 chars c/u)**:
- corto: `The weather is beautiful today.`
- medio: `Artificial intelligence is transforming the way we work, learn, and communicate across the entire world.`

**4 candidatos (aprobados por el usuario) — carga y llamada exactas**:
1. **es-mx-latam (baseline)** — proceso SIN redirección de HF (lee caché real read-only). Cargar con el loader de producción: `PYTHONPATH=src`, `from tts_sidecar.model_loader import ModelLoader; tts = ModelLoader().load(Path(snapshot_dir), "es-mx-latam", "cpu")` (`load` enruta por la cadena `"es-mx-latam"` en el path del snapshot; el arg `model_name` no afecta el routing). Generar: `tts.generate(text, language_id="en", audio_prompt_path=ref, exaggeration=0.5, cfg_weight=0.0, temperature=0.7)`. (Si el import de `tts_sidecar` da side-effects molestos, replicar inline las ~40 líneas de `_load_es_latam`.)
2. **Multilingüe general (v3, la ÚLTIMA)** — proceso CON `HF_HOME=<scratch>/hfcache`. El `from_pretrained` de 0.1.7 carga v2 (deprecado) → NO usarlo. Ensamblar la última versión manualmente, replicando el patrón de `_load_es_latam`: descargar de `ResembleAI/chatterbox` (vía `snapshot_download(..., allow_patterns=[...])` o `hf_hub_download` por archivo) `t3_mtl23ls_v3.safetensors` (2144 MB) + `s3gen_v3.safetensors` (1056 MB) + `ve.safetensors` (5.7 MB) + `grapheme_mtl_merged_expanded_v1.json` + `conds.pt` (~3.21 GB). Ensamblar: `from chatterbox.tts import T3, S3Gen, VoiceEncoder; from chatterbox.mtl_tts import ChatterboxMultilingualTTS, MTLTokenizer, T3Config, Conditionals; from safetensors.torch import load_file` → `t3=T3(T3Config.multilingual()); t3.load_state_dict(load_file(t3_v3)); s3gen=S3Gen(); s3gen.load_state_dict(load_file(s3gen_v3), strict=False); ve=VoiceEncoder(); ve.load_state_dict(load_file(ve)); tok=MTLTokenizer(grapheme_json); tts=ChatterboxMultilingualTTS(t3, s3gen, ve, tok, "cpu", conds=Conditionals.load(conds_pt).to("cpu"))` (mover t3/s3gen/ve a `cpu().eval()`). Generar: `tts.generate(text, language_id="en", audio_prompt_path=ref, exaggeration=0.5, cfg_weight=0.0, temperature=0.7)`. (v2 y v3 comparten arquitectura `T3Config.multilingual`/vocab 2454, así que v3 carga en la misma estructura.)
3. **Inglés monolingüe base** — proceso CON `HF_HOME=<scratch>/hfcache`. `from chatterbox.tts import ChatterboxTTS; tts = ChatterboxTTS.from_pretrained("cpu")`. Baja de `ResembleAI/chatterbox`: `ve.safetensors, t3_cfg.safetensors, s3gen.safetensors, tokenizer.json, conds.pt` (~3.19 GB). Generar SIN `language_id`: `tts.generate(text, audio_prompt_path=ref, exaggeration=0.5, cfg_weight=0.0, temperature=0.7)`.
4. **Chatterbox-Turbo** — proceso CON `HF_HOME=<scratch>/hfcache`. `from chatterbox.tts_turbo import ChatterboxTurboTTS; tts = ChatterboxTurboTTS.from_pretrained("cpu")`. Baja de `ResembleAI/chatterbox-turbo` (~4.04 GB, incluye un `s3gen.safetensors` que `from_local` no usa; usa `t3_turbo_v1` + `s3gen_meanflow` + tokenizer GPT-2). Generar SIN `language_id`: `tts.generate(text, audio_prompt_path=ref, exaggeration=0.0, cfg_weight=0.0, temperature=0.7)` (deja `top_k=1000`, `norm_loudness=True` por defecto).

**Descarga total nueva a scratchpad ≈ 10.4 GB** (candidatos 2+3+4); baseline 0. Todo aislado; se borra al cerrar.

**Estructura del código (scratchpad)**: un worker `spike.py <cand>` (unidad de trabajo, un candidato = un proceso) que el **orquestador escribe una sola vez** con las 4 recipes de «Especificación cerrada». Al arrancar fija `torch.set_num_threads(4)`; antes de cada `generate` hace `torch.manual_seed(1234)`; carga el candidato indicado, genera las 2 frases, guarda `out/<cand>__short.wav` y `out/<cand>__medium.wav`, y escribe `results/<cand>.json` (self-validación: `sample_rate` + duración de cada wav). **No hay driver bash único**: cada candidato lo corre su propio sub-agente (ver «Arquitectura de ejecución»), redirigiendo stdout+stderr a `logs/<cand>.log`.

**Mapeo resultado→conclusión**: inglés-base o Turbo neutro y con timbre reconocible → Camino «modelo nativo inglés». Solo multilingüe conserva timbre pero con leve acento → «multilingüe con cfg afinado». Ninguno preserva timbre aceptablemente → cross-lingual no viable con Chatterbox para la meta (respuesta valiosa igual, antes de tocar el repo).

## Arquitectura de ejecución (orquestador Opus 4.8 + sub-agentes Sonnet 5)
El experimento NO lo corre un único agente. El trabajo pesado se delega para no contaminar el contexto del orquestador con descargas de GB, warnings de torch y trazas de carga.

**Roles**
- **Orquestador = agente principal (Opus 4.8)**. NO carga modelos ni descarga pesos. Hace: pre-flight barato (escribir `spike.py`, crear carpetas), spawnear los 4 sub-agentes, esperar sus notificaciones, reconciliar leyendo artefactos de DISCO, reportar (Fase 5), ofrecer limpieza.
- **4 sub-agentes de trabajo (Sonnet 5)** — `subagent_type: general-purpose`, `model: "sonnet"`, uno por candidato. Cold-start con prompt autocontenido. Cada uno fija entorno, corre `spike.py <cand>`, self-valida, escribe artefactos a disco y devuelve un resumen ≤10 líneas. Diagnostica y reintenta SU propio fallo (máx. 1 vez) sin molestar al orquestador.

**Contrato de artefactos (a disco, NO al contexto)**
- Cada sub-agente produce: `out/<cand>__short.wav`, `out/<cand>__medium.wav`, `results/<cand>.json` (status, rutas, `sample_rate`, duración por wav en s, segundos de generación, warnings), `logs/<cand>.log` (stdout+stderr completo).
- Devuelve al orquestador SOLO: `status OK|FAIL`, las 2 duraciones, 1 nota. NUNCA vuelca logs ni trazas.
- **Regla dura del orquestador**: NO leer el `.output` del sub-agente (es symlink al transcript JSONL completo → desborda contexto). Reconciliar por `results/<cand>.json` de disco + el resumen devuelto. Leer `logs/<cand>.log` SOLO si ese candidato falló.

**Paralelización (anclada a la máquina: 16 CPUs lógicas, 31.4 GB RAM, ~17 libres)**
- Spawnear los 4 sub-agentes en background a la vez. RAM: 4×~3.2 GB ≈ 13 GB < 17 libres → caben.
- **Cada proceso DEBE capar hilos**: env `OMP_NUM_THREADS=4`, `MKL_NUM_THREADS=4` + `torch.set_num_threads(4)` en `spike.py`. Sin esto, torch-CPU toma los 16 cores por proceso y 4 procesos se sobresuscriben (64 hilos/16 cores) → cada inferencia más lenta. Con 4×4=16 hay paralelismo de cómputo real.
- Las descargas (I/O de 2/3/4) se solapan → principal ahorro de wall-clock.
- **Fallback** ante presión de memoria/OOM: 2 olas de 2 (p. ej. {1,3} luego {2,4}); el orquestador lanza la 2ª ola al cerrar la 1ª.

**Aislamiento de caché HF**: `HF_HOME` por candidato = `<scratch>/hfcache/<cand>` para 2/3/4 → evita carreras de escritura sobre los blobs compartidos de `ResembleAI/chatterbox` (cand 2 y 3 tiran del mismo repo). Cand 1 sin `HF_HOME` (lee la caché real read-only). Duplicar `ve.safetensors`/`conds.pt`/jsons entre 2 y 3 es despreciable (<10 MB).

**Plantilla de prompt del sub-agente (el orquestador la rellena, no la improvisa)**:
> Eres un sub-agente de trabajo. Ejecuta el experimento del candidato **{cand}** y NO devuelvas logs.
> 1. Entorno: `HF_HOME={hfcache/<cand>; vacío para cand 1}`, `OMP_NUM_THREADS=4`, `MKL_NUM_THREADS=4`.
> 2. Corre `PYTHONPATH=src python {scratch}/spike.py {cand} > logs/{cand}.log 2>&1` (la recipe de carga/params ya está en `spike.py`).
> 3. Verifica exit 0 y que existan `out/{cand}__short.wav` y `out/{cand}__medium.wav` no vacíos, y que `results/{cand}.json` tenga `sample_rate` y duración plausibles (>0.5 s).
> 4. Si falla: inspecciona `logs/{cand}.log`, corrige lo obvio (archivo faltante, typo de ruta), reintenta UNA vez. Si vuelve a fallar, marca FAIL.
> 5. Devuelve SOLO: `{cand}: OK|FAIL | short=<s>s medium=<s>s | <nota de una línea>`.

## Decisiones y trade-offs cerrados
- No hay pack de inglés (inglés = idioma base); modelos nativos de inglés = base (`t3_cfg`) y Turbo. Para inglés neutro son mejor apuesta que el multilingüe (no tienen fonología española que filtrar).
- **Timbre y prosodia van por caminos separados**: timbre = Voice Encoder + condicionamiento del S3Gen (siempre aplicado, ningún parámetro de muestreo lo degrada); prosodia/acento = generada por el T3 (ahí mandan `cfg_weight`/`temperature`/`exaggeration`). Por eso se puede optimizar contra el acento sin arriesgar el timbre.
- **`cfg_weight=0.0`** = palanca anti-accent principal (documentado por ResembleAI): la CFG amplifica la adherencia a la prosodia del prompt (español); a 0.0 el T3 usa su prior nativo (inglés) → ritmo nativo, no latino. Timbre intacto (va por el speaker embedding, no por la CFG). Fallback si suena demasiado plano: 0.3.
- **`temperature=0.7`** (bajado de 0.8): menos aleatoriedad en el muestreo del T3 → pronunciación inglesa más canónica y menos artefactos acentuados, sin caer en monótono/robótico.
- **Límite no controlable por parámetro**: el S3Gen condiciona ~10 s de la referencia española para el detalle acústico, así que algo de ritmo de origen puede filtrarse aun a `cfg_weight=0.0`. Se mitiga eligiendo modelo nativo de inglés (cand. 3/4), no ajustando muestreo.
- **Principio: el experimento prueba la ÚLTIMA versión de cada modelo, nunca un downgrade.** Multilingüe → **v3 ensamblada manualmente** (replicando `_load_es_latam`); el `from_pretrained` de 0.1.7 solo trae v2 (deprecado) y por eso NO se usa. Inglés base, Turbo y es-mx-latam ya cargan su última versión (`t3_cfg`/`t3_turbo_v1`+`s3gen_meanflow`/`t3_es_mx_latam`+`s3gen_v3`).
- **`exaggeration` por punto neutral de cada modelo** (0.5 en cand. 1-3; 0.0 en Turbo): es intensidad emocional, ortogonal al acento; usar el neutro de cada modelo evita handicapear a Turbo (su neutro es 0.0, el de la familia base/multi es 0.5). Trade-off asumido: introduce una segunda variable, justificada porque la pregunta es «cuál da mejor inglés nativo», no «cuál gana con parámetros idénticos artificiales». (Si se quisiera pureza de variable única, fijar 0.5 en los 4.)
- Aislamiento por proceso: baseline lee caché real read-only; 2/3/4 descargan a `HF_HOME` redirigido al scratchpad. `snapshot_download`/`hf_hub_download` respetan `HF_HOME`.
- En memoria todos ~3.2 GB (T3 ~2.1 + s3gen ~1.06 + ve); sin penalización de RAM.

## Insights de sesión (API verificada de chatterbox 0.1.7)
- `ChatterboxTTS.from_local(ckpt_dir, device)`/`from_pretrained(device)`, `REPO_ID="ResembleAI/chatterbox"`. `generate(text, repetition_penalty=1.2, min_p=0.05, top_p=1.0, audio_prompt_path=None, exaggeration=0.5, cfg_weight=0.5, temperature=0.8)` — SIN `language_id`.
- `ChatterboxMultilingualTTS.from_pretrained` → `snapshot_download(repo_id="ResembleAI/chatterbox", allow_patterns=["ve.pt","t3_mtl23ls_v2.safetensors","s3gen.pt","grapheme_mtl_merged_expanded_v1.json","conds.pt","Cangjie5_TC.json"])` — ⚠ trae **v2 deprecado**; NO usar (para v3 ensamblar manualmente, ver Especificación candidato 2). `generate(text, language_id, audio_prompt_path=None, exaggeration=0.5, cfg_weight=0.5, temperature=0.8, repetition_penalty=2.0, min_p=0.05, top_p=1.0)` — `language_id` REQUERIDO.
- `ChatterboxTurboTTS.from_pretrained` → `snapshot_download(repo_id="ResembleAI/chatterbox-turbo", allow_patterns=["*.safetensors","*.json","*.txt","*.pt","*.model"])`. `from_local` usa `ve.safetensors`, `t3_turbo_v1.safetensors`, `s3gen_meanflow.safetensors`, tokenizer GPT-2, `conds.pt`. `generate(text, ..., audio_prompt_path=None, exaggeration=0.0, cfg_weight=0.0, temperature=0.8, top_k=1000, norm_loudness=True)`.
- Baseline es-mx-latam: el `from_local` de la librería NO aplica (el pack no trae `ve`/`t3_mtl23ls_v2`); usar `tts_sidecar.model_loader.ModelLoader.load` (arma T3(T3Config.multilingual())+s3gen_v3+ve compartido+MTLTokenizer).

## Estado en curso
- Sin código escrito aún. Scratchpad de sesión: `C:\Users\Cristian\AppData\Local\Temp\claude\C--Users-Cristian-Desktop-Proyectos-Voices-TTS-Sidecar\06d2a977-a048-4971-ae30-0f905d4e1199\scratchpad`
- Archivos del repo (solo lectura): `src/tts_sidecar/synthesis.py:92`, `src/tts_sidecar/model_loader.py` (`_load_es_latam`), `src/tts_sidecar/model_cache.py:14-43`, `src/tts_sidecar/engine.py:135-217`.
- El continuity-prompt fue commiteado en `64141ea`; esta regeneración (precisando el diseño) deja el working tree con cambios sin commitear.

## Bloqueadores y preguntas abiertas
Ninguna decisión de diseño abierta. Solo incógnitas de runtime (no bloqueantes, se resuelven al ejecutar):
- CPU-only: cada síntesis tarda minutos; 8 generaciones. Con hilos capados a 4/proceso y 4 sub-agentes en paralelo (16 cores) el cómputo se solapa; aun así usar timeout amplio.
- Turbo corre en su neutro (`exaggeration=0.0`). Si aun así aparecen artefactos, anotarlo; no re-tunear salvo evidencia.

## Próximos pasos (ordenados) — protocolo del orquestador
1. **Pre-flight (Opus, barato)**: confirmar chatterbox 0.1.7 y la ruta `ref`; crear `out/ results/ logs/ hfcache/` en el scratchpad; escribir `spike.py` (worker con las 4 recipes de «Especificación cerrada», con `torch.set_num_threads(4)` y `results/<cand>.json`).
2. **Delegar (spawn 4 sub-agentes Sonnet 5, background)**: uno por candidato, con la «Plantilla de prompt del sub-agente». Baseline sin `HF_HOME`; 2/3/4 con `HF_HOME=<scratch>/hfcache/<cand>`. Fallback a 2 olas de 2 si hay presión de RAM.
3. **Reconciliar (Opus)**: al recibir las notificaciones, leer los 4 `results/<cand>.json` de disco (NO los transcripts); verificar los 8 WAV válidos; re-spawnear UNA vez cualquier candidato en FAIL con el tail de su log como pista; no bloquear el resto.
4. **Reportar (Fase 5)**: tabla por condición, pedir al usuario que escuche los WAV, interpretación causal, limitaciones (juez único, un solo seed). Relayar lo relevante (el final report del sub-agente no lo ve el usuario).
5. **Ofrecer limpieza** del scratchpad (incl. `hfcache`, ~10 GB). No tocar el repo sin confirmación; si quedan decisiones abiertas tras oír los resultados, usar `resolve-open-decisions`.

## Fuentes a re-leer post-compactación
- `src/tts_sidecar/model_loader.py` — `_load_es_latam` (para el baseline).
- `src/tts_sidecar/synthesis.py:54-98` — ruta real de síntesis.
- `src/tts_sidecar/conditionals.py:28-53` — roles de timbre vs speech reference (contexto; el spike usa solo `timbre-reference.wav` como archivo único, ver Especificación).
- Librería (solo si algo falla): `.../site-packages/chatterbox/mtl_tts.py` y `.../tts_turbo.py` (`from_local`/`from_pretrained` ya inspeccionados; ver «Insights»).

## No repetir
- NO usar el alias `ResembleAI/chatterbox-multilingual` (HTTP 401, muerto). Los pesos multi están en `ResembleAI/chatterbox`.
- NO usar `from_pretrained` para el multilingüe (trae v2 deprecado): ensamblar **v3** manualmente. Regla general: probar la última versión de cada modelo, sin downgrades.
- NO descargar el repo `chatterbox` entero (13.87 GB): `from_pretrained` ya filtra por `allow_patterns`.
- NO escribir modelos nuevos en la caché HF real: `HF_HOME` por candidato al scratchpad (`hfcache/<cand>`, solo 2/3/4).
- NO cargar modelos ni descargar pesos en el agente orquestador: se delega a sub-agentes Sonnet 5; Opus solo orquesta y reconcilia.
- NO leer el `.output`/transcript de un sub-agente (symlink al JSONL completo → desborda el contexto del orquestador): reconciliar por `results/<cand>.json` y el resumen devuelto; el log solo si hubo FAIL.
- NO re-descargar es-mx-latam: leerlo read-only del snapshot cacheado (proceso sin redirección).
- NO usar `from_local` de la librería para es-mx-latam: usar `ModelLoader` del repo.
- Turbo YA soportado en 0.1.7 (`tts_turbo`) — no subir versión.
- NO empezar a implementar hasta completar `/compact` → `/continuity-prompt resume`.

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» sin reabrir decisiones ya cerradas salvo nueva evidencia.
