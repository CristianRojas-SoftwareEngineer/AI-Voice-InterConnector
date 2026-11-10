> 🎯 **EMOTION RECIPE — the single source of truth is `resolve_emotion_recipe` in `main.c`.**
> ONE rule: **preset voice → pure STEER `ryan_<emo>` @ w12** (clean, every language); **cloned voice
> → COMBINE** (language `.expr` + steer). Use the **native preset per language** (JA `ono_anna`, KO `sohee`, ZH
> `vivian`, EN/Romance `ryan`). Exposed as the `--emotion <sad|joy|anger|fear|disgust|surprise>` flag.
> Paralinguistics = inline `[laugh]`/`[sigh]` tags.
>
> 🔬 **PARALINGUISTIC `[tag]` METHOD.**
> ONE method only: **INLINE native-trigger** (onomatopoeia inside the sentence, ONE generation → event in the
> voice's own timbre, mixed in). ❌ **NEVER the split-span / steering-span "splice"** (separate cold-prefill span
> = sounds like a different voice). The `[tag]` → onomatopoeia map lives in `qwen_tts_compose.c`
> (`para_pick` / `qwen_compose_para_substitute`).
> Shipped mapping: `[laugh]`→`哈哈哈` s7 (universal); `[sigh]`→`唉` s42 (ryan/clone) / `ahh` s7 (vivian); comma-
> delimited inline, one generation, seed pinned per-tag.

This file is the practical guide for agents working on this repository.
It is intentionally implementation-oriented: what to change, where, how to test,
and which behaviors are considered contractually stable.

## Project Scope

Pure C inference engine for Qwen3-TTS text-to-speech models:
- `Qwen3-TTS-12Hz-0.6B-CustomVoice`
- `Qwen3-TTS-12Hz-1.7B-CustomVoice`

CPU-only inference (BLAS is required, plus architecture-specific SIMD paths). The single build target is `make blas`.

## Current Status

- **All components verified correct** (Talker, Code Predictor, Speech Decoder — bit-identical to Python)
- **Performance**: RTF ~1.3–1.7 on Apple M1 8-core, 16 GB RAM (4 threads)
- **Dev hardware**: Apple M1 8-core, 16 GB RAM — all benchmarks reference this machine

## Active Branches

- **`main`**: Stable, production-ready code

## CI/CD (planned, not yet implemented)

After implementation, GitHub Actions will provide:
- **Build matrix**: Linux x86/ARM, macOS ARM/x86, Windows/WSL2 (manual trigger only)
- **CodeQL + clang-tidy**: Auto on PR to main (buffer overflow, UB, security checks)
- **ASan/UBSan**: Auto on PR to main (runtime memory safety with real model)
- **Benchmarks**: Per-runner CPU dump + ms/f breakdown as JSON artifacts
- **Releases**: Static binaries per platform on tag push (`v*`)

## Source Of Truth

When docs and code disagree, trust these files first:
- CLI behavior and options: `main.c`
- Public API and runtime state: `qwen_tts.h`
- Pipeline orchestration + prompt construction: `qwen_tts.c`
- Talker (LLM) forward pass + KV cache: `qwen_tts_talker.c`
- Code Predictor (MTP) forward pass: `qwen_tts_code_predictor.c`
- Speech tokenizer decoder (ConvNet): `qwen_tts_speech_decoder.c`
- Sampling strategies: `qwen_tts_sampling.c`
- WAV output: `qwen_tts_audio.c`
- Kernel dispatch and hot loops: `qwen_tts_kernels.c`
- Build targets: `Makefile`

Architecture/background references:
- `MODEL.md`

## User-Facing Behavior Contract (Do Not Break)

- Default output is a WAV file at 24 kHz, 16-bit PCM, mono.
- `--silent` must still write the WAV output file.
- `--silent` suppresses status/debug noise (stderr), not the audio output.
- Without `--debug`, stderr should be concise:
  - model loading info
  - final inference summary lines
- `--debug` enables verbose internal diagnostics.
- `--speaker` selects a preset speaker ID (0-8 for CustomVoice models).

## Model + Inference Facts

- Model variant is auto-detected from weights (0.6B vs 1.7B).
- Talker uses causal Qwen3 with KV cache, GQA, SwiGLU, RoPE.
- Code Predictor runs 15 sequential passes per audio frame.
- Speech decoder is a causal ConvNet with 480x upsampling.
- Large weights are bf16 mmapped and consumed via bf16 kernels.
- Output audio is 24 kHz, generated from 16-codebook discrete tokens at 12.5 Hz.

## Important Defaults

From `qwen_tts_load()` and CLI:
- Speaker ID: `0` (first preset voice)
- Temperature: `0.5`
- Top-k: `50`
- Top-p: `1.0`
- Repetition penalty: `1.05`
- Max new tokens: `8192`
- Code Predictor temperature: `0.0` (greedy)
- Code Predictor top-k: `50`
- Output file: `output.wav`

## Repository Map

- `main.c`
  - CLI parsing, defaults, reporting
- `qwen_tts.c`
  - high-level synthesis pipeline
  - prompt construction (ChatML format)
  - generation loop (Talker + Code Predictor + Speech Decoder)
- `qwen_tts_talker.c`
  - Talker LLM load + prefill + token step + KV cache
- `qwen_tts_code_predictor.c`
  - Code Predictor (MTP) load + forward (15 passes per frame)
- `qwen_tts_speech_decoder.c`
  - speech tokenizer decoder load + forward
  - codebook embedding lookup + RVQ sum
  - pre-transformer layers
  - ConvNet upsampling blocks: one network (`conv_decoder_forward`) for full decoding and
    one exact-streaming path; both use a two-pass ConvNeXt LayerNorm
  - decoder performance: a persistent per-context workspace (`qwen_sd_workspace_t`, slots that
    only grow, cleared in worker clones); transposed conv as a single GEMM plus a scatter in k
    order; causal conv as k accumulated GEMMs over the left-padded input (no im2col), with
    `W_k` reordered once at load and shared by workers. `QWEN_SD_INT8` (experimental) keeps its own im2col path
- `qwen_tts_audio.c`
  - WAV writer (24 kHz, 16-bit PCM, mono)
- `qwen_tts_sampling.c`
  - temperature, top-k, top-p, repetition penalty
- `qwen_tts_tokenizer.c`
  - tokenizer encode (text to token IDs)
- `qwen_tts_kernels.c`
  - ALL actual kernels live here (inline `#ifdef __ARM_NEON / __AVX2__ / else-scalar`),
    plus common math, threading (GCD, macOS-only), BLAS paths
  - the hot loops **do contain AVX2** (`_mm256_fmadd_ps`, `__AVX512BF16__`/`_mm512_dpbf16_ps`) in `qwen_rms_norm` (AVX2 16-wide), `bf16_matvec_fused` (AVX2 2-row fused with FMA) and `qwen_dot_f32_*` (AVX2/FMA 4 accumulators), plus `rms/bf16-conv` and `matvec+attn` (see `qwen_caps_report`).
  - every quantized multiplication (int8 and q4_0: matvec, argmax, QKV and matmat) uses the same integer dot product on every platform (see "Kernel/Optimization Rules").
- `download_model.sh`
  - interactive small/large model downloader

## Build + Run

Build:
```bash
make blas
```

Smoke run:
```bash
./qwen_tts -d qwen3-tts-0.6b --text "Hello, how are you?" -o output.wav
```

Play output (macOS):
```bash
afplay output.wav
```

## Streaming Output (Future)

Streaming mode will generate audio chunks progressively, writing WAV data
as frames are decoded. The causal ConvNet decoder enables this without
lookahead. Not implemented in phase 1.

## Kernel/Optimization Rules

- Architecture dispatch is done with `#ifdef` inside `qwen_tts_kernels.c`.
- Quantized (int8/q4_0) multiplications have ONE integer numerics: the activation is quantized by the
  single quantizers (`quantize_act_int8_ref`, `quantize_act_int8`, `quantize_act_int8_cols`: scale
  max|x|/127, round-half-even, saturate to [-128,127]) and multiplied with an integer dot product per ISA
  (AVX2 `maddubs`, VNNI `dpbusd`, ARM SDOT/i8mm, int32-accumulating scalar loop). There are no float
  fallbacks and no environment switches to select a quantized path; a batched `matmat` must equal B × `matvec`.
- Keep generic/NEON/AVX variants functionally equivalent.
- If you optimize one path, verify no regression on others.
- Favor meaningful speedups; avoid complexity for tiny wins.

## Cross-Hardware Performance Workflow (perf / SIMD / batching epics)

Dev box is **Apple M1** (the user's only fast-iteration device). M1 covers NEON + dotprod/SDOT
but **not** bf16/i8mm (M2+), SVE (server ARM), or AVX-512/VNNI/BF16/AMX (x86). So for any
performance/new-SIMD epic, the loop is:

1. **Develop on M1** here (fast iteration). Newer-ISA kernels are `#ifdef`-guarded so M1 keeps
   compiling the scalar/NEON fallback.
2. **Keep it compile-clean for other ISAs without leaving M1:** `make check-isa` forces the
   `-march` for the guarded BFMMLA/SMMLA/SME (ARM) and VNNI/BF16/AMX (x86) paths so syntax errors
   surface NOW, not on the rented box.
3. **Validate on real hardware** (we cannot fully test these on M1): rent/borrow the target, run
   the one-command harness, read the numbers, **fix, loop**. This is expected — perf/SIMD epics
   are *not* done until measured on real silicon.

**The testing/benchmark make targets** (document any new one here):
- `make matmat-bench` — batched matmat twins (`qwen_matmat_{bf16,int8,q4_0}`) vs B×matvec, per
  precision/threads (no model needed).
- `make check-isa` — compile-check the newer-ISA kernel paths (above).
- `./qwen_tts --caps` — runtime SIMD-extension detection ("does it fire?"): ARM dotprod/bf16/i8mm/
  SVE/SME, x86 AVX-512/VNNI/BF16/AMX. `./qwen_tts --self-test` — cross-ISA kernel correctness oracle.

Use **RTF + mel-corr** (not wall-clock, not md5) to compare across ISAs — fp-order differs benignly.

## Git Workflow

**New features MUST be developed on feature branches, NOT on main.**

- Branch naming: `feature/<short-name>` (e.g., `feature/streaming`, `feature/voice-clone`, `feature/http-server`)
- Open a PR to merge into main when ready
- Run `make test-all` before merging (both 0.6B and 1.7B must pass)
- Main branch must always build and pass tests

Roadmap and next steps are tracked in `PLAN.md`.

## Change Checklist For Agents

Before editing:
1. Identify behavioral contract impacted (CLI, output, speed, quality, memory).
2. Read corresponding source-of-truth file(s).
3. Create a feature branch if adding new functionality (NOT on main).

After editing:
1. Build: `make blas`
2. Run `make test-small` and `make test-large` (if model available).
3. Verify WAV output plays correctly and sounds reasonable.
4. Update `README.md` if CLI/runtime behavior changed.
5. Keep `PLAN.md` aligned if roadmap items completed or changed.

## Testing Rules (Comparative / A-B Tests)

When running comparative tests (normal vs streaming, before vs after optimization,
CLI vs server, etc.), **always use identical explicit parameters** across all runs:

- `--seed N` — same seed for every run
- `-s <speaker>` — explicit speaker name (not default)
- `-l <language>` — explicit language matching the text content
- Same `--text` across all runs
- Same model directory (`-d`)

**Default speaker for Italian and English**: `ryan` (`-s ryan`)

Example — comparing CLI normal vs streaming:
```bash
./qwen_tts -d qwen3-tts-0.6b --text "Test phrase here." --seed 42 -s ryan -l Italian -o normal.wav
./qwen_tts -d qwen3-tts-0.6b --text "Test phrase here." --seed 42 -s ryan -l Italian --stream -o stream.wav
```

For server tests, pass the same params in the JSON body:
```bash
curl -s http://localhost:8000/v1/tts -d '{"text":"Test phrase here.","seed":42,"speaker":"ryan","language":"Italian"}' -o server.wav
```

**Why**: Without explicit params, auto-detection or defaults may produce different
prompts (e.g. different codec token counts), making outputs non-comparable.

## Expressivity Plugins: Emotion vs Paralinguistics (Design Contract)

> ✅ **Emotion shipped: the recipe lives in `resolve_emotion_recipe` (`main.c`) — preset → STEER w12, clone →
> COMBINE, native preset per language, via `--emotion`.** The notes below are the para-vs-emo design contract
> (still relevant for the paralinguistics FT, future work); the emotion side is DONE — follow the recipe.

Emotion and paralinguistics (`[laugh]`/`[sigh]`/`[cough]`/`[breath]`…) are **SEPARATE, COMPOSABLE plugin fine-tunes**, NOT one model:

- The emotion plugin (CSP `.expr` FT band ~16-26 **+** `--ml-steer` L21-25 steering, applied TOGETHER) is the hard-won TOP WIN for both **presets AND cloned voices**. Adding paralinguistics MUST NOT regress, overwrite, or modify it.
- Paralinguistics is an **EXTRA training-data set augmenting a DIFFERENT characteristic** that *couples* with emotion — a user who wants both loads para + emo together and quality improves, never degrades.
- Therefore the para FT must target layers **DISJOINT from the emotion plugin's layers** (few layers, CSP-style) so the two `.expr` write different `talker.model.layers.N.` tensors and compose by summation with no interference.
- Do **NOT** use a para band that overlaps emotion (e.g. 0-27 or 16-26) — it clobbers emotion.

## Experiment Tracking (Senior Rule — Do Not Skip)

Every test/experiment gets its OWN dated folder + `README.md`, created with the run (not after):
`/tmp/qwen_tts_tests/YYYY-MM-DD_<short-name>/` (the `TEST_DIR` of the `Makefile`) containing the audio + a `README.md` that records:
1. **Date + short name**
2. **Hypothesis** — what we want to prove
3. **Why** — the prior result that motivates it
4. **Recipe** — data, trainer hparams, layers, engine flags (exact)
5. **Eval command** — the FULL `./qwen_tts ...` line (speaker, language, instruct/emotion/steer or NONE, expr, weight, `--no-compose`, seed). Show this next to the audio — full transparency, never hide whether emotion/instruct/steering was used. Default para eval = para-expr ONLY.
6. **What to validate** — the specific ear/objective check
7. **Result + verdict** — filled after the ear verdict

Why: across a long session we run many tests; the per-test README is the durable trail of WHY/HOW/what-it-proved, so we can decide to follow or abandon a path.

## Local-Only Artifacts (Do Not Depend On In Commits)

Common local directories/files are intentionally ignored:
- `qwen3-tts-0.6b/`, `qwen3-tts-1.7b/`
- `voices/`, `*.qvoice`, `TODO.md`, virtualenv folders

### Where audio test outputs go
Generated audio is NOT versioned (`*.wav` is gitignored). **Rule:**
- ALL generated audio for tests/experiments → **`/tmp/qwen_tts_tests/<YYYY-MM-DD_short-name>/`** (the `TEST_DIR` of the `Makefile`).
- The scratchpad dir (per-session, isolated) is also fine for throwaway audio.
