# Qwen3-TTS Pure C Implementation

[![Build](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/build.yml/badge.svg)](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/build.yml)
[![CodeQL](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/codeql.yml/badge.svg)](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/codeql.yml)
[![Memory Safety](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/safety.yml/badge.svg)](https://github.com/gabriele-mastrapasqua/qwen3-tts/actions/workflows/safety.yml)

A lightweight, cross-platform C inference engine for [Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS) text-to-speech models (0.6B and 1.7B). No Python, no PyTorch, no ONNX runtime — just C, a BLAS library, and raw model weights.

The engine runs the complete TTS pipeline: BPE tokenization, a 28-layer causal transformer (Talker), a multi-pass code predictor, and a convolutional speech decoder. Weights are memory-mapped directly from safetensors files in BF16, so loading is near-instant and memory usage stays low.

## Audio Samples

All samples generated with the 0.6B model — sub-realtime on a 2020 Apple M1 CPU (**RTF 0.52 `--int4` / 0.69 `--int8`**, no GPU; see [Performance](#performance)):

| Language | Speaker | Sample | Text |
|----------|---------|--------|------|
| English | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/english_ryan.wav) | *Hello, this is a test of the text to speech system.* |
| Italian | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/italian_ryan.wav) | *Buongiorno a tutti, questa e una dimostrazione del sistema di sintesi vocale.* |
| Italian | vivian | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/italian_vivian.wav) | *Buongiorno a tutti, questa e una dimostrazione del sistema di sintesi vocale.* |
| Spanish | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/spanish_ryan.wav) | *Hola, esta es una demostracion del sistema de sintesis de voz.* |
| Portuguese | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/portuguese_ryan.wav) | *Ola, esta e uma demonstracao do sistema de sintese de voz.* |
| French | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/french_ryan.wav) | *Bonjour a tous, ceci est une demonstration du systeme de synthese vocale.* |
| German | ryan | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/german_ryan.wav) | *Guten Tag, dies ist eine Demonstration des Sprachsynthesesystems.* |
| Japanese | Ono_Anna | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/japanese_ono_anna.wav) | *こんにちは、私の名前はアンナです。今日はとても良い天気ですね。東京の桜がとても綺麗です。* |
| Japanese | Ono_Anna | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/ganbatte_andrea.wav) | *頑張れ、アンドレア！あなたならできるよ。毎日少しずつ前に進もう。夢を諦めないで。応援してるよ！* |

## Quick Start

```bash
# Clone and build
git clone https://github.com/gabriele-mastrapasqua/qwen3-tts.git
cd qwen3-tts
make blas

# Download a model (interactive: small, large, voice-design, base-small, base-large)
./download_model.sh

# Synthesize speech
./qwen_tts -d qwen3-tts-0.6b --text "Hello, how are you today?" -o hello.wav
```

> **Dependencies:** Only a C compiler and BLAS (Accelerate on macOS, OpenBLAS on Linux).
## Features

- **Pure C, minimal dependencies** — Only requires a C compiler and BLAS. No Python runtime needed.
- **Runs on macOS, Linux and Windows/WSL2 (ARM/x86)** — the hot matvec/attention kernels have **NEON+SDOT (ARM, ARMv8.2 with dotprod required), AVX2 and AVX-512/VNNI/BF16 (x86)** twins with a scalar fallback + runtime ISA guard; every quantized (int8/q4_0) multiplication uses the same **integer dot product** on all platforms (AVX2 `maddubs`, VNNI `dpbusd`, ARM SDOT, an int32-accumulating scalar loop), and decode threading runs on a **cross-OS pool** (GCD on macOS, pthread elsewhere). Validated on Apple M1, Ryzen 7 6800H, Ryzen 7 5825U and EPYC 9555P (Zen5). Single-stream RTF is memory/cache-bound, so the chip's cache matters most (see [Performance](#performance)).
- **CPU only (`make blas`)** — a single build target that links against BLAS; the engine runs entirely on the CPU and requires BLAS. Server request-batching adds throughput for concurrent users. → [Performance](#performance)
- **Both model sizes** — Automatically detects 0.6B or 1.7B from weight files.
- **9 preset voices** — `ryan`, `vivian`, `serena`, `aiden`, `eric`, `dylan`, `uncle_fu`, `ono_anna`, `sohee`.
- **10 languages** — English, Chinese, Japanese, Korean, German, French, Russian, Portuguese, Spanish, Italian.
- **Memory-mapped weights** — BF16 safetensors mmap'd directly. 0.6B ~3 GB, 1.7B ~8 GB.
- **Voice cloning** — Clone any voice from a short WAV clip. Ship it as a compact **~25 MB graft `.qvoice`** (load it with `--icl-only`): keeps the CustomVoice weights so emotion levers (`--instruct`, `--expr`, `--ml-steer`) all work, with full prosody (sighs/pauses). An 8 KB `--xvector-only` `.bin` is the ultra-lean alternative (identity only). `--expr` and heavy WDELTA voices overwrite weights and require bf16 or `--int8`.- **Voice management** — List, inspect, delete `.qvoice` profiles (`--list-voices`, `--delete-voice`). No model required.
- **Style control** — `--instruct` for emotion/style on 1.7B: angry, whisper, cheerful, and more.
- **Emotion in one flag** (🧪 **beta**; paralinguistics `[laugh]`/`[sigh]` 🧪 **alpha**) — `--emotion <sad\|joy\|anger\|fear\|disgust\|surprise>` (1.7B) auto-applies the ear-validated recipe (per-language fine-tune `.expr` + steering vector + a default English instruct + temperature), on presets **and** cloned voices, in every Qwen language. **Plus 7 blended "dyads"** (`contempt`, `awe`, `nostalgia`, `disapproval`, `remorse`, `outrage`, `despair`) and **inline `[emotion]` switching** — many emotions from one prompt in a single generation. A vivid English `--instruct` and `-T` override. Pitch-preserving `--rate`/`--volume` and a `--roughness` grit knob are still available.- **The small 0.6B is expressive too — and stays sub-realtime** 🆕 — for a long time `--emotion` did nothing on the 0.6B: unlike the 1.7B it has no steerable emotion subspace. It does, however, clone voices very well — so on the small model **the emotion rides on the voice**. **All 9 presets ship ready** (240 KB of 4 KB voice assets) and **any cloned voice emotes with zero setup** via six shipped emotion directions — so the small model has the whole expressive stack out of the box: **6 emotions + inline `[tag]` paralinguistics + voice cloning, together, at RTF ≈ 0.8** under `--int8` on an M1.- **Inline markup for audiobooks** — write one text with ElevenLabs/Bark-style tags and get a multi-emotion take in one pass: `--text "I won! [joy] ...amazing! [pause:500ms] [sad] But it's over. [sigh]"`. Mid-text emotion switches, `[pause:400ms]`/`[break:1s]` pauses, and `[sigh]`/`[huff]` paralinguistic fillers — auto-detected in `--text` (no flag) or explicit via `--compose`. Spans are model-generated and concatenated seamlessly.- **VoiceDesign** — Create new voices from text descriptions.
- **HTTP server** — `/v1/tts`, `/v1/tts/stream`, OpenAI-compatible `/v1/audio/speech`; JSON body takes `emotion`/`instruct`/`volume`/`rate` (same recipe as the CLI). **Inline `[mood]` markup works over the API too** — one request can switch emotion sentence-by-sentence (`"text":"[joy] Great news! [sad] But I must go."`), auto-detected and streamed span-by-span.- **Streaming** — Real-time audio via `--stream` (WAV) or `--stdout` (raw PCM).
- **INT8 / INT4 quantization** — `--int8` / `--int4` quantize Talker + Code Predictor (the activation is quantized to int8 and multiplied with an integer dot product: SDOT on ARM, VNNI `dpbusd` or AVX2 `maddubs` on x86), near-bf16 quality, and work with presets **and** custom `.qvoice` voices. `--quant-mixed` is the Talker in q4_0 plus a Code Predictor with INT8 attention and output heads and Q4_0 FFN (log label `[INT8+Q4_0-FFN]`; quality validated on the 0.6B model). On cache-rich Apple Silicon **both go sub-realtime** (0.6B best **0.52 int4 / 0.69 int8**); on memory-starved x86, int8+VNNI wins the wall clock. See [Performance](#performance).
- **Configurable sampling** — Temperature, top-k, top-p, and repetition penalty.
- **24 kHz WAV output** — 16-bit PCM, mono.

## Usage

```
./qwen_tts [options]

Required:
  -d, --model-dir <path>     Model directory
  --text <string>            Text to synthesize

Optional:
  -o, --output <path>        Output WAV file (default: output.wav)
  -s, --speaker <name>       Speaker voice (default: ryan)
  -l, --language <lang>      Target language (default: English)
  -I, --instruct <text>      Style/emotion instruction (1.7B model only)
  --temperature <f>          Sampling temperature (default: 0.5)
  --top-k <n>                Top-k sampling (default: 50)
  --top-p <f>                Top-p nucleus sampling (default: 1.0)
  --rep-penalty <f>          Repetition penalty (default: 1.05)
  --max-tokens <n>           Max audio tokens (default: 8192)
  --max-duration <secs>      Max audio duration in seconds
  --seed <n>                 Random seed for reproducible output
  --ref-audio <path>         Reference audio for voice cloning (Base model)
  --save-voice <path>        Save voice profile (.qvoice = full, .bin = x-vector only)
  --load-voice <path>        Load voice profile (.qvoice or .bin)
  --xvector-only             Clone via speaker x-vector only — clean, 8KB .bin (recommended for expr/emotion)
  --icl-only                 Graft mode: keep CV weights, use the .qvoice ICL prefix (max timbre mimicry)
  --target-cv <dir>          CV model dir for delta encoding (bit-identical cross-model)
  --list-voices <dir>        List .qvoice files in directory (no model needed)
  --delete-voice <path>      Delete a .qvoice file
  --voice-name <name>        Name for the voice (stored in .qvoice metadata)
  --voice-design             VoiceDesign mode (create voice from --instruct)
  --stream                   Stream audio (decode chunks during generation)
  --stdout                   Output raw s16le PCM to stdout (implies --stream)
  --int8                     INT8 quantized (0.6B & 1.7B; faster, ~same quality) — recommended; integer dot product on every platform (VNNI on AVX-512 x86, AVX2 maddubs on other x86, SDOT on ARM; ARM needs ARMv8.2 with dotprod)
  --int4                     Q4_0 quantized (experimental; slower than --int8 on CPU)
  -j, --threads <n>          Worker threads (default: 4)
  --silent                   Suppress status output
  --debug                    Verbose diagnostics
  --serve <port>             Start HTTP server
  --host <ipv4>              Bind address for --serve (default 127.0.0.1; 0.0.0.0 exposes to the network)
```

### Examples

```bash
# Basic English
./qwen_tts -d qwen3-tts-0.6b --text "The quick brown fox jumps over the lazy dog." -o fox.wav

# Italian with a specific voice
./qwen_tts -d qwen3-tts-0.6b -s ryan -l Italian \
    --text "Ciao, questa e una prova del sistema di sintesi vocale." -o test_it.wav

# Style/emotion control (1.7B only)
./qwen_tts -d qwen3-tts-1.7b -s ryan -l English \
    --text "I cannot believe you did that to me." \
    --instruct "Speak in a very angry and aggressive tone" -o angry.wav

# Reproducible output with seed
./qwen_tts -d qwen3-tts-0.6b --text "Hello world" --seed 42 -o hello.wav
```

### Voice Cloning

Clone any voice from a reference audio clip. Requires a Base model.

```bash
# Clone a voice
./qwen_tts -d qwen3-tts-0.6b-base --ref-audio reference.wav \
    --text "Hello, this is my cloned voice." -o cloned.wav
```

**Ready-to-use reference voices (CC0 / Public Domain).** Four lite ~25 MB graft `.qvoice` clones of LibriVox
public-domain readers (Italian, Spanish, English, French) so the demos/tests run out of the box and you have
voices to listen to and reuse:
```bash
bash download_voices.sh    # fetch galatea(IT)/quijote(ES)/ohenry(EN)/hugo(FR) into voices/ (sha256-verified)
./qwen_tts -d qwen3-tts-1.7b --load-voice voices/galatea_graft.qvoice --icl-only -l Italian \
    --text "Buongiorno, questa è la mia voce clonata." -o out.wav
```
> 🎭 Want these clones to **emote** (`--emotion`)? See the **Emotion & expressivity** section below — it needs `bash download_assets.sh` first.
Hosted on Hugging Face → [**gabrione/qwen3-tts-voices**](https://huggingface.co/gabrione/qwen3-tts-voices) (CC0, LibriVox attribution).

### Custom Voices — small, portable, and *emotable* `.qvoice`

Clone a voice once, save it as a portable `.qvoice`, reuse it forever on the CustomVoice model — with
`--instruct`, `--emotion`, streaming, and the HTTP server.

**The default `.qvoice` is now a ~25 MB "graft"** — it keeps the CustomVoice weights, so it stays small,
carries full prosody, **and the emotion / instruct levers still work on your clone** (no more multi-GB
weight-delta files):

```bash
# Create — default = ~25 MB graft (one-time; needs the Base model)
./qwen_tts -d qwen3-tts-0.6b-base --ref-audio mario.wav -l Italian \
    --voice-name "Mario" --save-voice mario.qvoice

# Use it on CustomVoice — --icl-only keeps the CV weights (→ instruct/emotion work)
./qwen_tts -d qwen3-tts-0.6b --load-voice mario.qvoice --icl-only \
    --text "Ciao, come stai?" -o output.wav

# ...with an emotion, on your OWN cloned voice:
./qwen_tts -d qwen3-tts-0.6b --load-voice mario.qvoice --icl-only \
    --emotion joy -l Italian --text "Ce l'abbiamo fatta!" -o joy.wav

# Server / manage
./qwen_tts -d qwen3-tts-0.6b --load-voice mario.qvoice --icl-only --serve 8080
./qwen_tts --list-voices ./my_voices/
```

**Other formats:** **8 KB `.bin` x-vector** (`--xvector-only`,
tiniest & cleanest) · **heavy WDELTA** (`--target-cv`, ~0.8–3 GB, bit-identical — only if you need exact fidelity).
Loading a WDELTA voice (without `--icl-only`) or an `--expr` file overwrites Talker weights, so it requires bf16 or
`--int8`: with Q4_0 weights active (`--int4`, `--quant-mixed`) the engine stops with an error instead of ignoring the override.

**Voice clone samples** — cloned voices on 0.6B CustomVoice (25 MB grafts):

| Language | Voice | Source | Output | Text |
|----------|-------|--------|--------|------|
| Italian | Pirandello Reader | [LibriVox](https://librivox.org/) Public Domain | [input](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/ref_italian_pirandello.wav) → [clone](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/clone_italian_06b.wav) | *Buongiorno a tutti, questa e una dimostrazione della clonazione vocale.* |
| English | Sarac (F) | [LibriTTS-R](https://www.openslr.org/141/) CC-BY | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/clone_sarac_english_06b.wav) | *Good morning everyone, this is a demonstration of voice cloning using a custom voice profile.* |
| English | Peter (M) | [LibriTTS-R](https://www.openslr.org/141/) CC-BY | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/clone_peter_english_06b.wav) | *I love reading books aloud, there is something magical about bringing stories to life with your voice.* |
| French | Baudelaire Reader | [LibriVox](https://librivox.org/) Public Domain | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/clone_french_06b.wav) | *Bonjour a tous, ceci est une demonstration du clonage vocal avec un profil de voix personnalise.* |
| Spanish | Lu | [LibriVox](https://librivox.org/) Public Domain | [listen](https://github.com/gabriele-mastrapasqua/qwen3-tts/releases/download/v0.1-samples/clone_spanish_06b.wav) | *Buenos dias a todos, esta es una demostracion de la clonacion de voz con un perfil de voz personalizado.* |

### Emotion & expressivity (1.7B) · 🧪 Beta

> 🧪 **Beta quality.** Ear-validated after ~a month of tuning, but results vary by language/voice and can still
> be imprecise — expect rough edges (please gauge that before filing issues 🙏). Improvements will come, just
> not today.
>
> ⚙️ **Setup (once):** run **`bash download_assets.sh`** to fetch the emotion fine-tunes (`.expr`, ~200 MB for
> Italian) from Hugging Face → [**gabrione/qwen3-tts-italian-expr**](https://huggingface.co/gabrione/qwen3-tts-italian-expr).
> The steering vectors already ship in this repo. **`--emotion` then works on the 9 presets AND on your own
> cloned voices.** No clone yet? Grab ready-made **CC0 graft voices** with `bash download_voices.sh` →
> [**gabrione/qwen3-tts-voices**](https://huggingface.co/gabrione/qwen3-tts-voices) and emote them straight away.
>
> 🎧 **Hear it in one command** (after the two downloads): **`make emotion-demo`** renders a batch of emotion
> clips — every language × emotion, on presets *and* the galatea clone — so you can judge the current quality by
> ear. **`make emotion-para-demo`** adds the alpha `[laugh]`/`[sigh]`.

Emotion is **one flag**. Pick an emotion with `--emotion` and the engine auto-composes the validated
**COMBINE** stack for you — the per-language fine-tune (`.expr`) **plus** the steering vector for that
voice and emotion, at the ear-validated weights. No file paths, no layer ranges. A vivid **English or
Chinese** `--instruct` on top is optional but **recommended** — it drives the strongest, most natural result.

```bash
# emotion on a CLONED voice (galatea = a ready-made CC0 graft) — same one flag
./qwen_tts -d qwen3-tts-1.7b --load-voice voices/galatea_graft.qvoice --icl-only \
    -l Italian --emotion sad --text "Ho perso tutto, e adesso non so più cosa fare." -o sad.wav
```

```bash
# emotion in ONE flag — works on presets AND cloned voices
./qwen_tts -d qwen3-tts-1.7b -s ryan -l Italian -T 1.1 --emotion sad \
    --instruct "Speak softly, with quiet sadness." \
    --text "Allora, lascia che ti spieghi come stanno le cose." -o sad.wav
```

**🔊 Hear it** — click to download the examples:

| Language | Voice | Emotion | Text | Listen |
|----------|-------|---------|------|--------|
| Italian | ryan (preset) | 😢 sad | *Ho perso tutto quello che avevo, e adesso non so più cosa fare.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_it_sad.wav) |
| Italian | ryan (preset) | 😄 joy | *Non ci posso credere, è la notizia più bella della mia vita!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_it_joy.wav) |
| Italian | ryan (preset) | 🤢 disgust | *Ma che roba è questa? Fa davvero schifo, non riesco neanche a guardarla.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_it_disgust.wav) |
| Italian | ryan (preset) | 😲 surprise | *Cosa?! Non me lo aspettavo per niente, è incredibile!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_it_surprise.wav) |
| Italian | galatea (cloned voice) | 😢 sad | *Ho perso tutto quello che avevo, e adesso non so più cosa fare.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/galatea_it_sad.wav) |
| Italian | galatea (cloned voice) | 😠 anger | *Come ti permetti di parlarmi così? Questo non lo accetto!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/galatea_it_anger.wav) |
| English | ryan (preset) | 😢 sad | *I've lost everything I had, and now I don't know what to do anymore.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_en_sad.wav) |
| English | ryan (preset) | 😠 anger | *How dare you talk to me like that? I will not accept this!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_en_anger.wav) |
| English | ryan (preset) | 😨 fear | *There's someone in the house, I heard footsteps... I'm so scared, I don't know what to do.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ryan_en_fear.wav) |
| German | vivian | 😠 anger | *Also, lass mich dir in Ruhe erklären, wie die Dinge wirklich stehen.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/de_vivian_anger.wav) |
| French | vivian | 😢 sad | *Bon, laisse-moi t'expliquer calmement comment les choses se passent.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/fr_vivian_sad.wav) |
| French | vivian | 😲 surprise | *Quoi ? Je ne m'y attendais pas du tout, c'est incroyable.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/fr_vivian_surprise.wav) |
| Spanish | vivian | 😄 joy | *Bueno, déjame explicarte con calma cómo están realmente las cosas.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/es_vivian_joy.wav) |
| Spanish | vivian | 🤢 disgust | *¿Pero qué es esto? Es asqueroso, ni siquiera puedo mirarlo.* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/es_vivian_disgust.wav) |
| Portuguese | ryan | 😄 joy | *Não acredito, é a melhor notícia da minha vida!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/pt_ryan_joy.wav) |
| Chinese | vivian | 😄 joy | *我简直不敢相信，这是我一生中最好的消息，我太高兴了！* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/zh_vivian_joy.wav) |
| Russian | ryan | 😠 anger | *Как ты смеешь так со мной разговаривать? Это неприемлемо!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ru_ryan_anger.wav) |
| Japanese | ono_anna | 😢 sad | *私が持っていたものを全て失って、もうどうすればいいのか分からない。* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ja_ono_anna_sad.wav) |
| Japanese | ono_anna | 😨 fear | *家に誰かいる、足音が聞こえた……怖くてどうすればいいのか分からない。* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ja_ono_anna_fear.wav) |
| Korean | sohee | 😠 anger | *네가 어떻게 나한테 그렇게 말할 수 있어? 이건 절대 받아들일 수 없어!* | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_examples/ko_sohee_anger.wav) |

- **Emotions:** 6 primaries — `sad · joy · anger · fear · disgust · surprise` (synonyms like `happy`/`angry`
  work too) — **plus 7 blended "dyads"** (below).
- **The recipe:** a **preset voice** → pure **STEER** (the steering vector @ w12, clean in every language); a
  **cloned voice** → **COMBINE** (the language `.expr` + steer). Use the **native preset per language** (JA
  `ono_anna`, KO `sohee`, ZH `vivian`, EN/Romance `ryan`); the engine prints a hint.- **Works in every Qwen3-TTS language** (EN, IT, DE, ZH, RU, KO, JA, ES, FR, PT) — just set `-l <Language>`.

#### Blended emotions (dyads) · new

Emotion steering **directions add**: summing two primary vectors yields a coherent *new* emotion. Seven
ear-validated **Plutchik dyads** ship as first-class `--emotion` values — no new capture, no fine-tune:

| Dyad | = blend of | Reads as | Listen (English, ryan) |
|---|---|---|---|
| `contempt`    | anger + disgust    | sneering disdain | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_contempt.wav) |
| `awe`         | fear + surprise    | hushed wonder | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_awe.wav) |
| `nostalgia`   | joy + sad          | bittersweet fondness | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_nostalgia.wav) |
| `disapproval` | surprise + sad     | let-down reproach | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_disapproval.wav) |
| `remorse`     | sad + disgust      | guilty regret | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_remorse.wav) |
| `outrage`     | anger + surprise   | indignant shock | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_outrage.wav) |
| `despair`     | fear + sad         | hopeless dread | [▶ play](https://github.com/gabriele-mastrapasqua/qwen3-tts/raw/main/samples/emotion_dyads/dyad_despair.wav) |

```bash
./qwen_tts -d qwen3-tts-1.7b -s ryan -l English --emotion contempt \
    --text "Oh, sure, that's a truly brilliant idea." -o contempt.wav
```

#### Inline emotion switching — many emotions from ONE prompt · new

Write `[emotion]` tags **inside `--text`** — any primary or dyad — and the engine switches emotion **sentence by
sentence in a single generation**, clean at the seams, one output file. `[neutral]` resets; composes with a global
`--emotion` and with `[laugh]`/`[sigh]` tags.

```bash
./qwen_tts -d qwen3-tts-1.7b -s ryan -l English -T 1.1 --text \
  "[contempt] Oh, sure, that's a brilliant idea. [nostalgia] We used to spend every summer by the sea. [despair] And now there's nothing left." \
  -o switch.wav
```

- **Paralinguistics → inline `[tags]`, also automatic · 🧪 Alpha.** Write `[laugh]`, `[sigh]`, `[yawn]`, `[wow]`, `[giggle]` or `[scoff]` in
  `--text` and the engine performs the event (it picks the onomatopoeia anchor + the right seed per voice for
  you) — no flags. `[wow]`/`[yawn]`/`[scoff]` compose well with the matching `--emotion`; `[giggle]` is best
  **standalone** (stacking it with `--emotion joy` over-drives the laugh). *Alpha quality:* hit-or-miss across
  voices/languages (laughs land best); expect misses for now:
  ```bash
  ./qwen_tts -d qwen3-tts-1.7b -s ryan -l Italian -T 1.1 \
      --text "Che giornata... [sigh] non ce la faccio più. [laugh]" -o para.wav
  ```

#### Emotion + paralinguistics, and tuning the `--instruct` (experimental 🧪)

You can put a paralinguistic `[tag]` **inside an emotional sentence** (e.g. `--emotion joy` + `[laugh]`) and get both
at once — the engine switches to the **COMBINE** stack so the `.expr` language-correction keeps the event on-accent.
Still a bit unstable across some languages/voices; clearest on `[laugh]`/`[sigh]` with `ryan`/`vivian`. Reproduce with
`make emotion-para-demo`.

`--instruct` is an **optional** vivid English (or Chinese) line on top of the recipe (matters for cloned voices;
preset pure-emotion needs none): a stronger, more vivid instruct pushes emotion harder, and plain English
`"speak faster/slower"` shifts pacing (~±15 %). **Don't** use a slot template (`Tempo:+40%` comes out *slower* —
Qwen doesn't parse it); plain prose wins.

**Assets:** `bash download_assets.sh` fetches the `.expr` packs; `--verify` re-checks integrity. Full set ≈ 1.4 GB;
Italian-only emotion needs just `italian_csp_topk6.expr` (203 MB).

### Emotion & expressivity on the **small 0.6B** · 🆕

The 0.6B used to be the fast *neutral* voice: `--emotion` was a no-op there, and steering or
fine-tuning it never worked. The reason is structural — at half the width the small model has no
emotion subspace disjoint from language and timbre, so there is nothing to steer.

But it **clones voices very well**. So on the small model the emotion is not an inference-time lever,
it is a **property of the voice**: you clone from emotional audio and get an emotional voice.

**A cloned voice gets all six emotions for free.** Clone a voice the usual way and `--emotion` just
works on it — no asset to build, no 1.7B, no Base model:

```bash
./qwen_tts -d qwen3-tts-0.6b --load-voice myvoice.qvoice --icl-only --int8 \
    -l Italian --emotion anger --text "[sigh] ..." -o out.wav
# → Emotion 'ang' on 0.6B: generic direction @ 0.25 (no per-voice asset needed)
```

This works because the emotional offset in ECAPA speaker space turns out to be largely
**speaker-independent** — between two different speakers' deltas the cosine sits at 0.48-0.68 (random
1024-dim vectors would be ~0). Six averaged unit directions ship in this repo (4 KB each, 24 KB total)
and the engine adds one to whatever x-vector you loaded, preserving its norm. It costs nothing at
runtime. Dose it with `--emotion-strength` (default 0.25; 0.35 pushes harder).

**All 9 presets work out of the box** too — every one ships with its six dedicated assets (60 files,
240 KB total), each built from a donor in the language that voice speaks natively. So `--emotion` on
a preset needs no setup either:

```bash
# emotion on the SMALL model — nothing to install, the 1.7B is not in the path
./qwen_tts -d qwen3-tts-0.6b -s ryan -l Italian --int8 --emotion anger \
    --text "Non è possibile che succeda sempre la stessa cosa." -o anger.wav

# everything composes — emotion + inline paralinguistics, one generation
./qwen_tts -d qwen3-tts-0.6b -s ryan -l Italian --int8 --emotion sad \
    --text "[sigh] Non è possibile che succeda sempre la stessa cosa." -o sad_sigh.wav
```

**Resolution order** for `--emotion` on the 0.6B: dedicated asset → generic direction on the loaded
x-vector → an explicit error naming the missing piece. It never silently falls back to neutral.

**Speed** — M1, `-j4`, quiet machine. The full expressive stack costs ~0.09 RTF over the bare model:

| on the 0.6B | bf16 | **int8** | int4 |
|---|---|---|---|
| emotional voice (4 KB) | 1.16 | **0.73** | **0.56** |
| emotional voice (16.8 MB graft) | 1.14 | **0.72** | — |
| **emotional voice + `[tag]`** | 1.18 | **0.78** | — |
| bare 0.6B (reference) | 1.17 | 0.69 | — |

**Notes.** Works on presets *and* cloned voices. Prefer the 16.8 MB graft for **anger** — with a bare
4 KB x-vector the high-arousal delivery compresses and can swallow a short word; the graft's prosody
scaffolding holds it together, at the same speed. The `[tag]` seeds differ from the 1.7B's (the engine
picks the right table per model automatically).

### HTTP Server

```bash
# Start server
./qwen_tts -d qwen3-tts-0.6b --serve 8080

# Serve many users at once — step their requests together (vLLM-style request batching)
./qwen_tts -d qwen3-tts-0.6b --serve 8080 --batch-size 4

# Generate speech
curl -s http://localhost:8080/v1/tts \
  -d '{"text":"Hello, how are you?"}' -o output.wav

# With emotion (same recipe as the CLI --emotion; joy/sad/angry/calm/…)
curl -s http://localhost:8080/v1/tts \
  -d '{"text":"What a wonderful day!","speaker":"ryan","language":"English","emotion":"joy"}' -o joy.wav

# Stream with real-time playback (emotion works on the streaming path too)
curl -sN http://localhost:8080/v1/tts/stream \
  -d '{"text":"Hello, how are you?","emotion":"sad"}' | \
  play -t raw -r 24000 -e signed -b 16 -c 1 -

# OpenAI-compatible endpoint
curl -s http://localhost:8080/v1/audio/speech \
  -d '{"input":"Hello world","voice":"ryan"}' -o output.wav
```

> The server listens only on `127.0.0.1` by default and has no authentication. Exposing it to the network requires an explicit `--host 0.0.0.0`.

### Streaming

```bash
# Stream to WAV file
./qwen_tts -d qwen3-tts-0.6b --text "Hello world" --stream -o hello.wav

# Pipe raw PCM to audio player
./qwen_tts -d qwen3-tts-0.6b --text "Hello world" --stdout | \
    play -t raw -r 24000 -e signed -b 16 -c 1 -
```

## How It Works

```
Text --> BPE Tokenizer --> Talker (LLM) --> Code Predictor --> Speech Decoder --> 24 kHz WAV
```

| Component | What it does |
|-----------|-------------|
| **Talker** | 28-layer Qwen3 transformer with GQA, RoPE, SwiGLU. Generates one audio frame token per step. |
| **Code Predictor** | 5-layer transformer running 15 sequential passes per frame. Predicts the remaining 15 codebook entries. |
| **Speech Decoder** | Causal ConvNet with 16-codebook RVQ dequantization and 480x upsampling. Converts codes to waveform. |

| | 0.6B | 1.7B |
|---|------|------|
| Talker hidden dim | 1024 | 2048 |
| Heads (Q/KV) | 16/8 | 16/8 |
| Layers | 28 | 28 |
| Code Predictor | 1024 hidden, 5 layers | 1024 hidden, 5 layers (+2048→1024 projection) |
| Memory | ~3 GB | ~8 GB |

## Performance

> ### ⚡ Faster than real-time on Apple Silicon — both `--int8` and `--int4` go sub-1.0 RTF
> On a 2020 M1 (CPU, no GPU) the 0.6B model runs **~2× faster than real time** — CLI, streaming **and** server — with **no perceptible quality loss** by ear (cloned `.qvoice` voices included).

**Apple M1** (8-core, 16 GB, 4 threads), 0.6B — best RTF per precision. int4 is the fastest lever on Apple's
cache-rich SLC; int8 is the safest quality/speed pick — **both beat real time**:

| Precision | Best 0.6B RTF | vs real-time |
|---|---|---|
| bf16 | 1.3–1.8 | slower |
| **`--int8`** | **0.69** ⚡ | ~1.4× faster |
| **`--int4`** | **0.52** ⚡ | ~1.9× faster |

**Every delivery mode stays sub-realtime** — bf16 vs `--int8` across CLI / streaming / server / cloned voice:

| Mode | bf16 RTF | **`--int8` RTF** | First audio (TTFA) |
|---|---|---|---|
| CLI (short, ~4 s) | 1.5–1.8 | **0.90** ⚡ | 0.96 s |
| CLI (long, ~14 s) | ~1.3 | **0.80** ⚡ | — |
| **Streaming** (`--stream`, short) | 1.5–1.8 | **0.89** ⚡ | **0.46 s** |
| **Streaming** (long) | ~1.3 | **0.81** ⚡ | **0.50 s** |
| **HTTP server** (`--serve`, warm) | ~1.3 | **0.88** ⚡ | — |
| **Custom voice** `.qvoice` (streamed) | 1.34 | **0.93** ⚡ | 0.47 s |

**Expressive *and* sub-realtime** — the 0.6B's full expressive stack (emotional voice + inline `[tag]`
paralinguistics + cloning, see [Emotion on the small 0.6B](#emotion--expressivity-on-the-small-06b--)) costs
only **~0.09 RTF** over the bare model:

| 0.6B, everything on | bf16 | **`--int8`** | `--int4` |
|---|---|---|---|
| emotional voice (4 KB asset) | 1.16 | **0.73** ⚡ | **0.56** ⚡ |
| emotional voice (16.8 MB graft) | 1.14 | **0.72** ⚡ | — |
| **emotional voice + `[tag]`** | 1.18 | **0.78** ⚡ | — |
| bare 0.6B (reference) | 1.17 | 0.69 | — |

RTF = processing_time / audio_duration; **< 1.0 = faster than real-time**. Quantization reads fewer weight bytes
per frame and multiply them with an integer dot product (SDOT on ARM, VNNI or AVX2 `maddubs` on x86): **0.6B ~1.5 (bf16) → 0.69 (int8) → 0.52 (int4)**; **1.7B
~2.0 (bf16) → 1.79 (int8)** — no
perceptible quality loss, works with `.qvoice` voices.

### 📊 Benchmark *your* CPU

Want to know how this runs on **your** machine (Apple Silicon, AMD/Intel x86, ARM server)? The repo
ships a one-command per-box report — no setup beyond the model:

```bash
make bench              # quick RTF: short+long, normal+stream (both models)
make bench-full         # + server, instruct, INT8, .qvoice

# Per-CPU report (copy onto any rented ARM/x86 box):
./qwen_tts --caps       # what SIMD your CPU actually has (NEON/SDOT/bf16/i8mm/SVE • AVX2/AVX-512/VNNI/AMX)
./qwen_tts --self-test  # are the kernels numerically correct on this ISA?
```

**Cross-device CPU (single-stream 0.6B, this repo's best config):**

| Device | SIMD + threads | RAM | Best 0.6B RTF | Config |
|---|---|---|---|---|
| **Apple M1** 8-core | NEON + SDOT int8/int4, GCD 4-thread | 16 GB | **0.52 int4 / 0.69 int8** | `--int4 -j4` |
| **Neoverse-N1** (Ampere Altra Max, Scaleway) | NEON + SDOT, pthread 4-thread | 16 GB / 4 vCPU | **1.28** stream int4 + conv-int8 (**1.49** default) | `--int4 --stream` |
| **Graviton3** (Neoverse-V1, AWS c7g.2xlarge) | NEON + SDOT + **i8mm SMMLA + BFMMLA**, pthread 4-thread | 16 GB / 8 vCPU | **0.66** (1.7B int8: **0.95**, sub-RT!) | `--int8 -j4` |
| **Apple M4** (Mac mini, Scaleway) | NEON + SDOT + i8mm + bf16 + SME | 16 GB / 10-core | **0.32** | `--int4 -j4` |
| **Ryzen 7 6800H** (Zen3+, 16 MB L3, bare metal) | AVX2 + FMA, pthread 4-thread | 32 GB | **2.02** | `--int4 -j4` |
| **Ryzen 7 5825U** (Zen3, AVX2 integer dot product `maddubs`, Windows 11, cloned voice) | AVX2 + FMA, Win32 pool 4-thread | — | **0.66** idle; **0.84** with 1 thread loading memory; **1.10** with 2 threads loading memory (0.6B) | `--quant-mixed -j4 --stream` (Talker q4_0 + Code Predictor INT8 attention / Q4_0 FFN) |
| **EPYC 9555P** (Zen5, AVX-512+VNNI+BF16, Scaleway VM) | AVX-512 attention + VNNI + VDPBF16PS, pthread 4-thread | 16 GB / 4 vCPU | **0.95** (int4 = int8; -j1: int4 **1.05** beats int8 1.21) | `--int4 -j4`, `SIMD=avx512bf16` |

Numbers refreshed 2026-08-04 after the AVX-512 parity round (16-wide attention/rms/conversions,
native-bf16 `VDPBF16PS` matvec, q4-VNNI v3-default + fused-QKV VNNI twin — bf16 mode −21% single-thread
on Zen5, and **int4 now beats int8 single-thread on x86 0.6B** for the first time). Single-stream RTF is
**memory/cache-bound** (the Code Predictor re-reads its weights 16×/frame): SIMD width and thread count
matter less than fewer weight bytes (`--int8`/`--int4`) and a cache that fits the working set (Apple's
SLC, an X3D chip's V-cache). On cache-rich Apple Silicon **int4 is the fastest lever**; on x86 it now
depends on the model: **0.6B → `--int4`**, **1.7B → `--int8`** (still the 1.7B wall-clock king). Many-core servers are best for
**throughput** (concurrent requests), not single-stream latency. Check yours: `./qwen_tts --caps` (on
x86, build with `make blas SIMD=avx512bf16` on Zen4/5 / Cooper Lake+, or `SIMD=avx512vnni` if the CPU
lacks `avx512bf16` — the default build is portable AVX2).

**Concurrent serving — request batching (`--serve --batch-size N`).** For *N users at once*, the server
can step their requests **together** through the model (vLLM-style): weights are read from memory **once**
and reused across all in-flight requests, instead of re-read per user. A continuous scheduler keeps the
batch full (a finished request's slot is refilled immediately) and **streaming composes** — each user
still gets their own progressive audio stream. This trades a little per-request latency for much higher
total throughput on bandwidth-bound boxes.
**vs other implementations:**

| Hardware | 0.6B RTF | Notes |
|----------|----------|-------|
| **This project (C, Apple M1 CPU, `--int4`)** | **0.52** | Pure C, no GPU — **2× faster than real-time** (post-PR#17 decoder work) |
| This project (C, Apple M1 CPU, bf16) | 1.26–1.39 | Pure C, no GPU |
| Python + PyTorch (Ryzen 9 7950X CPU) | 4.5–5.8 | Official Python, CPU-only |
| NVIDIA RTX 3090 | 0.52–0.68 | Python + PyTorch + FlashAttention 2 |

5–7x faster than Python on CPU, and **faster than real-time with `--int8`** — on a 2020 laptop with no GPU.

**Throughput — server request-batching** (`--serve --batch-size N`, continuous batching + per-request
streaming). Batching is a **throughput / parallelism** lever, *not* a per-request speedup — it serves N
concurrent users in roughly the time of one by reading each weight once for all B sequences (matvec → matmat).
**The CPU path batches:**

| Backend | Batch speedup | Notes |
|---|---|---|
| **CPU x86** | ~N on bandwidth-bound servers | ~1× on cache-rich M1 (single-stream is already fast) |
| **CPU ARM** (Graviton3+, i8mm) | **int8 batch matmat 2.1×, int4 1.6×** (native SMMLA GEMM) | e2e batched server −19% wall @ B=4 vs the pre-SMMLA twin; bf16 1.5× (BFMMLA) |

## Credits & Acknowledgments

- **Salvatore Sanfilippo ([antirez](https://github.com/antirez))** — This project wouldn't exist without his [qwen-asr](https://github.com/antirez/qwen-asr), a pure C Qwen2-Audio ASR engine that proved you can do real neural inference in plain C with mmap'd safetensors, BF16 NEON kernels, and zero dependencies. The entire architecture of this TTS engine — the approach, the style, the philosophy of minimal C inference — is directly inspired by his work. If you like this project, go star qwen-asr first.
- **Michael Abrash** — His *[Graphics Programming Black Book](https://www.jagregory.com/abrash-black-book/)* (1997) shaped how we think about performance. The chapters on data alignment, struct layout, and cache-friendly access patterns for the 386/486 are still relevant today — we got a **24% speedup** from cache-line alignment (`posix_memalign(64)`), applying the same principles Abrash taught 30 years ago to modern SIMD and BLAS.
- **John Carmack** — His `.plan` files and QuakeCon talks on micro-optimization and cache friendliness were a constant reference. Where Abrash gave you the systematic rules and benchmarks, Carmack showed you the mindset: always think about how data flows through the CPU.
- **[Qwen3-TTS](https://github.com/QwenLM/Qwen3-TTS)** by the Qwen team at Alibaba — the model architecture, weights, and research. Models on [Hugging Face](https://huggingface.co/Qwen/Qwen3-TTS-12Hz-0.6B-CustomVoice). [Paper](https://arxiv.org/abs/2505.15894).
- **[Qwen2.5](https://github.com/QwenLM/Qwen2.5)** by the Qwen team — the base LLM architecture (GQA, RoPE, SwiGLU) used in the Talker and Code Predictor.

## License

MIT
