# Prompt de continuidad — Spike: clonación cross-lingual español→inglés neutro (4 candidatos)

## Objetivo activo
Diseñar, **implementar y ejecutar** un spike (experimento desechable, en scratchpad, sin tocar el repo ni la caché HF real) que compare 4 modelos Chatterbox para el objetivo del usuario: **clonar el timbre y la forma de habla de una narración en español y sintetizar inglés lo más NEUTRO posible (sin arrastrar acento españolizado) a partir de texto arbitrario**. Es clonación cross-lingual, NO un traductor (no requiere ASR/MT). Al terminar: reportar (Fase 5), ofrecer limpieza del scratchpad, y NO tocar el repo sin confirmación explícita.

## Progreso verificado
- **Requisito del usuario**: inglés neutro, sin acento español. Timbre reconocible deseable. Fuente = texto en inglés (no habla en tiempo real).
- **El repo hoy bloquea cross-lingual**: `synthesis.py:92` fija `language_id="es"` (única aparición de language_id en todo `src/`); solo carga el pack `es-mx-latam`. Habilitarlo = cambio de código, no config trivial.
- **Diseño experimental aprobado por el usuario** (vía AskUserQuestion):
  - Referencia (variable controlada) = **voz `default` empaquetada del repo**: `src/tts_sidecar/voices/default/timbre-reference.wav` + `speech-reference.wav`.
  - Candidatos = **los 4**: (1) `es-mx-latam` baseline [ya cacheado], (2) multilingüe general, (3) inglés monolingüe base, (4) Chatterbox-Turbo.
- **Tamaños reales (HF API, verificados)**:
  - `ResembleAI/Chatterbox-Multilingual-es-mx-latam` (repo 4.26 GB): `t3_es_mx_latam.safetensors` 2144 MB + `s3gen_v3.safetensors` 1056 MB (+ .pt dup) + `grapheme_mtl_merged_expanded_v1.json`. **NO trae ve.safetensors**. Cacheado en: `/c/Users/Cristian/.cache/huggingface/hub/models--ResembleAI--Chatterbox-Multilingual-es-mx-latam/snapshots/27e595bf2fe7be0533ca299d9afafcde08b7cca7/`
  - `ResembleAI/chatterbox-multilingual` → **HTTP 401, NO EXISTE**. El alias `model_cache.py:15` está muerto. Los pesos multilingües están en `ResembleAI/chatterbox`.
  - `ResembleAI/chatterbox` (repo 13.87 GB, contiene TODO): `t3_mtl23ls_v3.safetensors` 2144 MB (multi 23-idiomas v3), `t3_cfg.safetensors` 2129.7 MB (inglés base), `s3gen_v3.safetensors` 1056 MB, `s3gen.safetensors` 1056 MB (inglés), `ve.safetensors` 5.7 MB, `tokenizer.json`, `conds.pt`, etc. `ve` ya cacheado en `models--ResembleAI--chatterbox/snapshots/5bb1f6ee58e50c3b8d408bc82a6d3740c2db6e18/ve.safetensors`.
  - `ResembleAI/chatterbox-turbo` (repo 4.04 GB, HTTP 200): `t3_turbo_v1.safetensors` 1915.5 MB + `s3gen.safetensors` 1056 MB (+ `s3gen_meanflow` 1064.9 MB) + `ve.safetensors` 5.7 MB + tokenizer estilo GPT-2 (`vocab.json`,`merges.txt`,`added_tokens.json`,`tokenizer_config.json`,`special_tokens_map.json`) + `conds.pt` + `t3_turbo_v1.yaml`.
- **En memoria todos pesan ~3.2 GB** (T3 ~2.1 GB + s3gen ~1.06 GB + ve). No hay penalización de RAM al cambiar de modelo.

## Decisiones y trade-offs cerrados
- **No hay pack de inglés**: el inglés es el idioma base de Chatterbox; los 6 language packs son idiomas no-ingleses. Para inglés hay 2 modelos nativos: base monolingüe (`t3_cfg`) y Turbo.
- **Para inglés NEUTRO, un modelo nativo de inglés (base/Turbo) es mejor apuesta que el multilingüe**: no tiene fonología española que filtrar. El multilingüe sirve pero requiere `cfg_weight=0` y aun así puede dejar bleed. El `es-mx-latam` es la peor apuesta (fine-tune español) → va como baseline para OÍR el problema.
- **Palanca anti-accent-bleed**: `cfg_weight=0.0` (documentado por ResembleAI). Trade-off: neutraliza acento pero sacrifica transferencia de estilo/prosodia. Timbre se conserva igual.
- **Aislamiento (clave del requisito «sin modificar estado»)**: cada condición en su propio proceso. es-mx-latam se lee READ-ONLY del snapshot real (ya cacheado, nada que descargar). Los 3 modelos nuevos descargan a una **caché HF en scratchpad** vía `HF_HOME`/`HF_HUB_CACHE` redirigido POR PROCESO → nunca tocan `~/.cache/huggingface`. Limpieza final = `rm` del scratchpad.

## Insights de sesión (API verificada de chatterbox 0.1.7)
- `chatterbox-tts==0.1.7` instalado en **Python global** `C:\Users\Cristian\AppData\Local\Programs\Python\Python313\Lib\site-packages\chatterbox` (NO en `.venv`). El `python` del PATH lo resuelve y tiene chatterbox. Hay un `.venv/` en el repo pero el global ya sirve.
- Submódulos: `models, mtl_tts, tts, tts_turbo, vc`. **Turbo SÍ está soportado en 0.1.7** (`tts_turbo.ChatterboxTurboTTS`) — no hace falta subir versión.
- Loaders y `generate` (firmas verificadas):
  - `tts.ChatterboxTTS.from_local(ckpt_dir, device)` / `.from_pretrained(device)`. `REPO_ID="ResembleAI/chatterbox"`. `from_pretrained` descarga SOLO 5 archivos: `ve.safetensors, t3_cfg.safetensors, s3gen.safetensors, tokenizer.json, conds.pt` (selectivo, NO el repo de 14 GB). `generate(text, repetition_penalty=1.2, min_p=0.05, top_p=1.0, audio_prompt_path=None, exaggeration=0.5, cfg_weight=0.5, temperature=0.8)` — **sin `language_id`** (inglés puro).
  - `mtl_tts.ChatterboxMultilingualTTS.from_local(ckpt_dir, device)` / `.from_pretrained`. `generate(text, language_id, audio_prompt_path=None, exaggeration=0.5, cfg_weight=0.5, temperature=0.8, repetition_penalty=2.0, min_p=0.05, top_p=1.0)` — `language_id` REQUERIDO (usar `"en"`). **VERIFICAR en `mtl_tts.py` qué T3 baja `from_pretrained` (v2 vs v3)** antes de correr.
  - `tts_turbo.ChatterboxTurboTTS.from_local(ckpt_dir, device)` / `.from_pretrained`. `generate(text, ..., audio_prompt_path=None, exaggeration=0.0, cfg_weight=0.0, temperature=0.8, top_k=1000, norm_loudness=True)` — inglés puro.
- **Baseline es-mx-latam**: reutilizar el loader de producción `from tts_sidecar.model_loader import ModelLoader; ModelLoader().load(snapshot_dir, "es-mx-latam", "cpu")` (arma T3(T3Config.multilingual())+s3gen_v3+ve+MTLTokenizer manualmente porque el pack no trae ve ni usa from_local estándar). Luego `generate(text, language_id="en", cfg_weight=0)`.

## Estado en curso
- Sin código escrito aún. Todo el diseño está en este prompt.
- Scratchpad de sesión: `C:\Users\Cristian\AppData\Local\Temp\claude\C--Users-Cristian-Desktop-Proyectos-Voices-TTS-Sidecar\06d2a977-a048-4971-ae30-0f905d4e1199\scratchpad`
- Archivos clave del repo (solo lectura, referencia): `src/tts_sidecar/synthesis.py:92`, `src/tts_sidecar/model_loader.py` (`_load_es_latam`), `src/tts_sidecar/model_cache.py:14-43`, `src/tts_sidecar/engine.py:135-217` (params: MAX_NEW_TOKENS=500, N_CFM_TIMESTEPS=4, EXAGGERATION=0.75, watermark bypass).

## Bloqueadores y preguntas abiertas
- Verificar qué checkpoint T3 baja `ChatterboxMultilingualTTS.from_pretrained` (leer `mtl_tts.py`). Si no es el deseado, descargar `t3_mtl23ls_v3.safetensors` con `hf_hub_download(repo_id="ResembleAI/chatterbox", filename=..., cache_dir=scratchpad)` y armar dir para `from_local`.
- Confirmar que Turbo `from_pretrained` resuelve su tokenizer GPT-2 sin fricción.
- CPU-only: cada síntesis tarda minutos; 4 candidatos × ~2 frases = ~8 generaciones. Puede ser lento; considerar 1 frase corta + 1 media (<300 chars c/u por el límite del modelo).

## Próximos pasos (ordenados)
1. **Implementar y ejecutar el diseño experimental completo** (instrucción del usuario). Concretamente:
   a. Verificar en `mtl_tts.py`/`tts_turbo.py` los archivos que baja cada `from_pretrained` y ajustar la descarga selectiva.
   b. Escribir en scratchpad un script por condición (o uno parametrizado que corra cada condición **en su propio proceso**), con `HF_HOME` redirigido a `scratchpad/hfcache` para los 3 modelos nuevos y SIN redirigir para el baseline es-mx-latam (lee caché real read-only).
   c. Variables controladas idénticas: misma referencia (voz `default`), mismo set de texto en inglés (1 corta + 1 media), `cfg_weight=0`, `exaggeration=0.5`, `torch.manual_seed` fijo, device `cpu`. es-mx-latam y multilingüe usan `language_id="en"`.
   d. Salida: un `.wav` por candidato×frase en scratchpad, nombrados por condición.
   e. Ejecutar, capturar stdout/stderr, diagnosticar fallos antes de reintentar.
2. **Reportar (Fase 5)**: pregunta del spike + respuesta directa, tabla por condición, interpretación causal, limitaciones (la neutralidad es perceptual → el usuario debe escuchar los WAV).
3. **Ofrecer limpieza** del scratchpad (incl. `hfcache`). No implementar cambios en el repo sin confirmación; si quedan decisiones abiertas, usar `resolve-open-decisions`.

## Fuentes a re-leer post-compactación
- `src/tts_sidecar/model_loader.py` — método `_load_es_latam`: cómo arma el modelo es-mx-latam (para reutilizarlo en el baseline).
- `src/tts_sidecar/synthesis.py:54-98` — ruta real de síntesis (params a replicar).
- `C:\Users\...\site-packages\chatterbox\mtl_tts.py` — `from_pretrained`/`from_local` y qué T3 baja el multilingüe.
- `C:\Users\...\site-packages\chatterbox\tts_turbo.py` — loader y tokenizer de Turbo.
- `src/tts_sidecar/voices/default/` — confirmar nombres exactos de los WAV de referencia.

## No repetir
- NO usar el alias `ResembleAI/chatterbox-multilingual` (HTTP 401, muerto). Los pesos multi están en `ResembleAI/chatterbox`.
- NO descargar el repo `chatterbox` entero (13.87 GB): usar `from_pretrained` (baja 5 archivos) o `hf_hub_download` por archivo.
- NO escribir modelos nuevos en la caché HF real: redirigir `HF_HOME` a scratchpad por proceso.
- NO tocar el repo ni la caché real; NO re-descargar es-mx-latam (leerlo read-only del snapshot cacheado).
- Turbo YA está soportado en 0.1.7 (`tts_turbo`) — no subir versión de chatterbox-tts.
- NO empezar a implementar hasta completar `/compact` → `/continuity-prompt resume`.

---
Instrucción post-compactación: Ejecuta `/continuity-prompt resume` para leer este archivo desde disco. Revisa las fuentes listadas, valida el estado del workspace y continúa desde el paso 1 de «Próximos pasos» sin reabrir decisiones ya cerradas salvo nueva evidencia.
