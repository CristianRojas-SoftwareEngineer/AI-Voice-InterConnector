# Makefile for Qwen3-TTS Pure C Inference Engine

UNAME_S := $(shell uname -s)
UNAME_M := $(shell uname -m)
CC = gcc

# Línea base de arquitectura / SIMD.
#   - macOS:      -mcpu=apple-m1 portable por defecto: M1 es el chip más antiguo
#                 que soporta la versión mínima de macOS del producto, y todo Mac
#                 con Apple Silicon posterior ejecuta su código. -march=native
#                 acoplaría el codegen al chip del runner (SIGILL en un M1);
#                 SIMD=native -> -march=native solo para compilar y ejecutar en la misma CPU
#   - Linux x86:  PORTABLE -mavx2 -mfma by default (Haswell 2013+). We deliberately
#                 do NOT use -march=native off-Mac: it locks codegen to the build
#                 host and SIGILLs on any older CPU in the field (the reported
#                 "AVX-512 Ryzen ran our scalar/illegal build" bug). Override with:
#                   SIMD=scalar  -> no AVX2 (pre-2013 CPUs; uses portable C fallback)
#                   SIMD=avx512  -> add AVX-512 (validate with Intel SDE / real HW)
#   - Linux ARM:  -march=armv8.2-a+dotprod por defecto (el motor exige dotprod: SDOT es
#                 la base del producto escalar entero); SIMD=native -> -march=native solo
#                 para compilar y ejecutar en la misma CPU
SIMD ?= auto
ifeq ($(UNAME_S),Darwin)
    ifeq ($(SIMD),native)
        ARCH_FLAGS = -march=native
    else
        ARCH_FLAGS = -mcpu=apple-m1
    endif
else ifneq (,$(filter x86_64 amd64,$(UNAME_M)))
    ifeq ($(SIMD),scalar)
        ARCH_FLAGS =
    else ifeq ($(SIMD),avx512bf16)
        # AVX-512 + VNNI + BF16 (producto escalar int8 nativo y producto bf16 nativo
        # VDPBF16PS). Zen4/Zen5 (EPYC 9xx4/9xx5, Ryzen 7xxx+), Cooper Lake, Sapphire
        # Rapids. Comprueba la línea de ejecución de `--caps` para avx512bf16 ANTES de
        # usar esta compilación.
        # -mavx512dq: gcc exige que el objetivo coincida con el de los intrinsics
        # always_inline de DQ (la compilación cruzada con clang no lo exige). Toda CPU
        # de la era VNNI tiene DQ.
        ARCH_FLAGS = -mavx512f -mavx512bw -mavx512vl -mavx512dq -mavx512vnni -mavx512bf16 -mavx2 -mfma
    else ifeq ($(SIMD),avx512vnni)
        # AVX-512 + VNNI (native int8 dot). Cascade Lake+/Ice Lake/Zen4+ (e.g. Ryzen 9950X3D).
        ARCH_FLAGS = -mavx512f -mavx512bw -mavx512vl -mavx512dq -mavx512vnni -mavx2 -mfma
    else ifeq ($(SIMD),avx512)
        ARCH_FLAGS = -mavx512f -mavx512bw -mavx512vl -mavx2 -mfma
    else
        ARCH_FLAGS = -mavx2 -mfma
    endif
else
    # ARM (aarch64): el motor exige el producto escalar entero (SDOT) de ARMv8.2,
    # porque todas las multiplicaciones cuantizadas usan el mismo algoritmo entero
    # en todas las plataformas; sin dotprod la compilación falla. -march=native
    # acoplaria a la microarquitectura del runner (SIGILL en CPUs de campo mas
    # viejas), el mismo fallo que off-Mac en x86. Honra SIMD como x86:
    #   SIMD=native -> -march=native (dev box; misma CPU build=run)
    #   (default)   -> -march=armv8.2-a+dotprod (portable a todo ARMv8.2 con dotprod)
    ifeq ($(SIMD),native)
        ARCH_FLAGS = -march=native
    else
        ARCH_FLAGS = -march=armv8.2-a+dotprod
    endif
endif

# F4 TTS-build-portable: variante determinista evaluada 2026-08-24.
# Se probaron: (a) -ffp-contract=off -fno-fast-math y (b) -ffast-math -ffp-contract=off.
# Ambas degradan WER de `dub` corto ("Hola, ¿cómo estás?") de ≤0.25 a 1.0–1.33
# (18s audio garble vs 2–3s normal, whisper transcribe "sonido de la máquina").
# `synthesize` largo sigue verde, pero `dub` corto es gate C1. Self-test sigue PASS.
# Veredicto: se mantiene -ffast-math (contrato validado) y se documenta la variante
# medida como no adoptada; no se cambia H1 por evidencia. Re-evaluar en F5 con
# corpus 3 frases ES y medición RTF para confirmar no-regresión.
CFLAGS_BASE = -Wall -Wextra -O3 $(ARCH_FLAGS) -ffast-math
LDLIBS = -lm -lpthread

# LZ4 (embedded in vendor/ — no external dependency needed)
CFLAGS_BASE += -Ivendor

# BLAS (Accelerate on macOS, OpenBLAS on Linux, OpenBLAS+shims on MSYS2/MinGW)
ifeq ($(UNAME_S),Darwin)
    CFLAGS_BASE += -DACCELERATE_NEW_LAPACK
    LDLIBS += -framework Accelerate
else ifeq ($(UNAME_S),Linux)
    # _GNU_SOURCE: glibc solo declara extensiones como strcasestr con ella. Sin
    # la declaración, gcc anterior a 14 asumía que devuelve int (y truncaba el
    # puntero a 32 bits en x86_64), y gcc 14 lo rechaza como error.
    CFLAGS_BASE += -D_GNU_SOURCE -I/usr/include/openblas
    # Destino de clang para comprobar en check-isa las rutas ARM desde un host x86.
    ISACHK_ARM_X = -target aarch64-linux-gnu
    # OpenBLAS se enlaza estático para que el motor distribuido no exija
    # libopenblas en el sistema. Debian/Ubuntu instalan libopenblas.a fuera de la
    # ruta del enlazador (en openblas-pthread/), y EL8 (la imagen manylinux_2_28
    # de la CI) llama libopenblasp.a a la variante con hilos y libopenblas.a a la
    # secuencial, así que se busca por ruta, con la variante con hilos primero, y
    # se puede fijar con OPENBLAS_A=<ruta>. Su LAPACK está compilado con gfortran:
    # libgfortran (y en x86_64 libquadmath, que libgfortran usa) también van
    # estáticas, de modo que el motor solo enlaza dinámicamente glibc. Sin la
    # librería estática el enlace del motor falla con un mensaje (regla
    # openblas-static-missing) en vez de enlazar la dinámica en silencio; los
    # objetivos que no enlazan el motor, como clean, no la exigen.
    MULTIARCH := $(shell $(CC) -print-multiarch 2>/dev/null)
    OPENBLAS_A ?= $(firstword $(wildcard \
        /usr/lib/$(MULTIARCH)/openblas-pthread/libopenblas.a \
        /usr/lib/$(MULTIARCH)/libopenblas.a \
        /usr/lib64/libopenblasp.a \
        /usr/lib64/libopenblas.a \
        /usr/lib/libopenblas.a))
    ifeq ($(OPENBLAS_A),)
        OPENBLAS_A = openblas-static-missing
    endif
    ifneq (,$(filter x86_64 amd64,$(UNAME_M)))
        FORTRAN_STATIC = -lgfortran -lquadmath
    else
        FORTRAN_STATIC = -lgfortran
    endif
    LDLIBS += $(OPENBLAS_A) -Wl,-Bstatic $(FORTRAN_STATIC) -Wl,-Bdynamic -lm -lpthread
else
# ——— MinGW-w64/UCRT64 (MSYS2): vendoring-local shims POSIX ———
#   sys/mman.h + unistd.h stubs live in third_party/ingot/mingw_shim/ (§2.4 plan).
#   openblas package: mingw-w64-ucrt-x86_64-openblas (headers en /ucrt64).
#   cygpath -m traduce /ucrt64/... → C:/msys64/ucrt64/...: gcc de MinGW (invocado por
#   mingw32-make, un binario Windows nativo) NO pasa por el runtime MSYS2 mount-table,
#   por lo que paths en estilo MSYS2 (/ucrt64) no se resuelven y cblas.h no se encuentra.
#   Ver memoria: "MinGW build path root-cause".
    UCRT64_INC := $(shell cygpath -m /ucrt64/include/openblas)
    # Destino de clang para comprobar en check-isa las rutas ARM desde un host x86;
    # las cabeceras de MinGW sirven también para el destino aarch64.
    ISACHK_ARM_X = -target aarch64-w64-mingw32 -isystem $(shell cygpath -m /ucrt64/include)
    UCRT64_LIB := $(shell cygpath -m /ucrt64/lib)
    CFLAGS_BASE += -D_GNU_SOURCE -std=gnu11 \
        -I$(UCRT64_INC) -Ithird_party/ingot/mingw_shim \
        -include third_party/ingot/mingw_shim/unistd.h \
        -include third_party/ingot/mingw_shim/sys/mman.h
    # -static: vincular OpenBLAS/libwinpthread/libgcc/libgfortran/libquadmath como
    # estaticas -> qwen_tts.exe autocontenido (solo DLLs del sistema Windows).
    # libopenblas.a + libwinpthread.a existen en /ucrt64/lib.
    # -static al inicio fuerza libgcc/libwinpthread/libgomp/libgfortran/libquadmath
    # como estaticas. libopenblas.a (estatica) referencia omp_* (libgomp), gfortran_*
    # (libgfortran) y libquadmath → deben ir DESPUES de -lopenblas (ld resuelve left-to-right)
    # para cubrir las referencias pendientes. -lm cubre referencias matemáticas.
    LDLIBS += -static -L$(UCRT64_LIB) -lopenblas -lgomp -lws2_32 -lwinpthread -lm
    # Recurso con el manifiesto de aplicación (activeCodePage UTF-8): sin él, argv y
    # las APIs ANSI llegan en cp1252 y corrompen --ref-text y las rutas no ASCII.
    WIN_RES_OBJ := qwen_tts_manifest.o
endif

CFLAGS = $(CFLAGS_BASE) -I$(INGOT_DIR)/include $(EXTRA_CFLAGS)

# Source files
SRCS = main.c \
       qwen_tts.c \
       qwen_tts_talker.c \
       qwen_tts_code_predictor.c \
       qwen_tts_speech_decoder.c \
       qwen_tts_kernels.c \
       qwen_tts_thread.c \
       qwen_tts_audio.c \
       qwen_tts_emotion.c \
       qwen_tts_compose.c \
       qwen_tts_sampling.c \
       qwen_tts_tokenizer.c \
       qwen_tts_server.c \
       qwen_tts_voice_clone.c \
       qwen_tts_speech_encoder.c \
       vendor/lz4.c

OBJS = $(SRCS:.c=.o) $(WIN_RES_OBJ)
TARGET = qwen_tts

# Solo MinGW: compila el recurso de manifiesto a COFF para enlazarlo en el ejecutable.
qwen_tts_manifest.o: qwen_tts_manifest.rc qwen_tts.exe.manifest
	windres -O coff -i $< -o $@

# ── ingot: the GGUF/safetensors library, vendored as a git subtree. Built by
# its own Makefile so this one never learns how it is compiled.
# Under MinGW-w64/UCRT64 the ingot Makefile is platform-agnostic and does NOT
# know the POSIX shims, so src/gguf.c (which includes <sys/mman.h>) fails to
# resolve them. We hand the shims through as CFLAGS/INCLUDE overrides — only on
# MinGW — without touching the vendored subtree Makefile itself. ──────────────
INGOT_DIR := third_party/ingot
INGOT_LIB := $(INGOT_DIR)/libingot.a
MINGW_SHIM_DIR := third_party/ingot/mingw_shim
ifeq ($(UNAME_S),Darwin)
	INGOT_SHIM_FLAGS :=
else ifneq (,$(filter Linux,$(UNAME_S)))
	INGOT_SHIM_FLAGS :=
else
	# Ruta absoluta para el sub-make de ingot; cygpath solo existe en MSYS2.
	MINGW_SHIM_DIR_ABS := $(shell cygpath -m "$(CURDIR)/$(MINGW_SHIM_DIR)")
	# MinGW-w64/UCRT64: aplica los mismos shims POSIX (-O2 conserva la
	# optimización del sub-make).
	INGOT_SHIM_FLAGS := CFLAGS="-O2 -I$(MINGW_SHIM_DIR_ABS) -include $(MINGW_SHIM_DIR_ABS)/unistd.h -include $(MINGW_SHIM_DIR_ABS)/sys/mman.h" INCLUDE="-Iinclude -I$(MINGW_SHIM_DIR_ABS)"
endif
$(INGOT_LIB):
	$(MAKE) -C $(INGOT_DIR) lib $(INGOT_SHIM_FLAGS)

# Refresh the vendored subtree from upstream (clean working tree required).
update-ingot:
	git subtree pull --prefix $(INGOT_DIR) https://github.com/mynah-org/ingot.git main --squash
	@$(MAKE) -C $(INGOT_DIR) clean

.PHONY: update-ingot
MODEL_DIR = qwen3-tts-0.6b

# Default: show help
all: help

help:
	@echo "qwen_tts — Qwen3-TTS Pure C Inference - Build Targets"
	@echo ""
	@echo "Build:"
	@echo "  make blas      - Build with BLAS acceleration (Accelerate/OpenBLAS)"
	@echo "  make debug     - Debug build with AddressSanitizer"
	@echo "  make clean     - Remove build artifacts"
	@echo "  make info      - Show build configuration"
	@echo ""
	@echo "Test (requires models downloaded via ./download_model.sh):"
	@echo "  make test-small      - Run all 0.6B tests (English + Italian)"
	@echo "  make test-large      - Run all 1.7B tests (config + English + Italian)"
	@echo "  make test-large-int8 - Run 1.7B INT8 tests (Italian + English, seed 42)"
	@echo "  make test-large-int4 - Run 1.7B INT4 tests (Italian + English, seed 42)"
	@echo "  make test-large-quant - Run all 1.7B quantization tests (INT8 + INT4)"
	@echo "  make test-emotion-ft - Emotion fine-tune (.expr graft) smoke: CSP Italian on 1.7B (preset+clone, seed 42)"
	@echo "  make test-clone      - Voice clone e2e (generate ref → clone → stream)"
	@echo "  make demo-clone      - Voice clone demo (requires REF=<reference.wav>)"
	@echo "  make test-regression - Cross-model regression checks"
	@echo "  make test-all        - Run everything (0.6B + 1.7B + regression)"
	@echo ""
	@echo "Benchmark:"
	@echo "  make bench           - RTF benchmark (short+long, normal+stream)"
	@echo "  make bench-full      - Full benchmark (+ server, qvoice, instruct, INT8)"
	@echo "  make cp-microbench   - Build qwen_tts_cpbench (per-op Code Predictor breakdown)"
	@echo "  make test-decoder-tool - Build qwen_tts_decoder_tool (decode a QWEN_DUMP_CODES dump alone)"
	@echo ""
	@echo "Example: make blas && ./$(TARGET) -d $(MODEL_DIR) -t \"Hello world\" -o output.wav"

# Build
$(TARGET): $(OBJS) $(INGOT_LIB)
	$(CC) $(CFLAGS) -o $@ $(OBJS) $(INGOT_LIB) $(LDLIBS)

ifeq ($(UNAME_S),Linux)
# En Linux el motor enlaza libopenblas.a: es requisito del enlace, y si no se
# encontró la regla de abajo detiene el build con el remedio.
$(TARGET): $(OPENBLAS_A)

.PHONY: openblas-static-missing
openblas-static-missing:
	@echo "No se encontró libopenblas.a: instala la variante estática de OpenBLAS (en Debian/Ubuntu, libopenblas-dev) o indica su ruta con OPENBLAS_A=<ruta>" >&2
	@exit 1
endif

blas: $(TARGET)

# CP micro-benchmark: separate binary instrumented with -DCP_MICROBENCH.
# Partitions per-frame Code Predictor time among sub-ops (QKV/attn/FFN/norm/lm_head).
# Clean rebuild into qwen_tts_cpbench so instrumented and normal .o never mix.
cp-microbench:
	$(MAKE) clean
	$(MAKE) TARGET=qwen_tts_cpbench EXTRA_CFLAGS=-DCP_MICROBENCH qwen_tts_cpbench
	@echo ""
	@echo "Built ./qwen_tts_cpbench  (run a normal generation; CP breakdown prints in the summary)"

# Standalone speech-decoder tool: decode a QWEN_DUMP_CODES text dump (16 ints/line)
# through the decoder alone (fixed input, no sampling) — A/B the decoder across changes.
test-decoder-tool: $(filter-out main.o,$(OBJS)) test_decoder_standalone.o $(INGOT_LIB)
	$(CC) $(CFLAGS) -o qwen_tts_decoder_tool $^ $(LDLIBS)
	@echo "Built ./qwen_tts_decoder_tool  (usage: ./qwen_tts_decoder_tool codes.txt [model_dir] [out.wav])"

# Compile C sources. -MMD -MP emits a .d per object listing every header it
# actually included, so a header edit rebuilds exactly what depends on it. The
# hand-written list below used to do this and drifted: qwen_tts_compose.o,
# _audio.o, _emotion.o and _speech_encoder.o included qwen_tts.h without a rule,
# so growing a struct in it left them compiled against the OLD layout — silent
# heap corruption at runtime, far from the edit. Never hand-maintain this again.
%.o: %.c
	$(CC) $(CFLAGS) -MMD -MP -c -o $@ $<

# The RVQ nearest-neighbor encode (speech encoder) is numerically sensitive: -ffast-math
# miscompiles it and SIGSEGVs on 1.7B (a bad codegen in the NN distance search; harmless on
# 0.6B due to heap layout). It's an offline one-time path (voice creation), NOT a hot loop, so
# compile this single TU without -ffast-math. See PLAN (icl-encode root-cause TODO).
qwen_tts_speech_encoder.o: qwen_tts_speech_encoder.c
	$(CC) $(filter-out -ffast-math,$(CFLAGS)) -MMD -MP -c -o $@ $<

# Header dependencies: auto-generated by -MMD -MP into <obj>.d, pulled in here.
# (`-` so a first, .d-less build does not warn.)
-include $(OBJS:.o=.d)

clean:
	rm -f $(OBJS) $(OBJS:.o=.d) $(TARGET)
	rm -f vendor/lz4.d
	rm -f test_decoder_standalone.o test_decoder_standalone.d qwen_tts_decoder_tool

# Debug build
debug: CFLAGS = $(CFLAGS_BASE) -I$(INGOT_DIR)/include -g -O0 -DDEBUG -fsanitize=address -fsanitize=undefined
debug: LDLIBS += -fsanitize=address -fsanitize=undefined
debug: clean $(TARGET)

# Info
info:
	@echo "Platform: $(UNAME_S)"
	@echo "CC:       $(CC)"
	@echo "CFLAGS:   $(CFLAGS)"
	@echo "LDLIBS:   $(LDLIBS)"
	@echo "SRCS:     $(SRCS)"
	@echo "TARGET:   $(TARGET)"

# ── Test targets ──────────────────────────────────────────────────────────────
# Models must be downloaded first via ./download_model.sh
# Tests verify: model loading, config parsing, generation, WAV output, non-zero audio

MODEL_SMALL = qwen3-tts-0.6b
MODEL_LARGE = qwen3-tts-1.7b
MODEL_BASE_SMALL = qwen3-tts-0.6b-base
MODEL_VOICE_DESIGN = qwen3-tts-voice-design
TEST_DIR = /tmp/qwen_tts_tests

# Helper script for test validation
# Usage: validate_test <wav_file> <label>
define validate_wav
	@if [ ! -f $(1) ]; then echo "FAIL: $(1) not created"; exit 1; fi
	@WAV_SIZE=$$(stat -f%z $(1) 2>/dev/null || stat -c%s $(1) 2>/dev/null); \
	 if [ "$$WAV_SIZE" -le 44 ]; then echo "FAIL: $(1) is empty ($$WAV_SIZE bytes)"; exit 1; fi
	@if ! grep -q "Generated [1-9]" $(1).log; then echo "FAIL: no frames generated"; exit 1; fi
	@if grep -qi "nan" $(1).log; then echo "WARN: NaN detected in output"; fi
	@if grep -q "MISSING" $(1).log; then echo "FAIL: speech decoder weights MISSING"; exit 1; fi
	@echo "PASS: $(2)"
	@echo ""
endef

# ── Small model (0.6B) tests ──

test-small-en:
	@echo "--- 0.6B English ryan ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_SMALL) -s ryan -l English \
		--text "Hello, this is a test of the text to speech system." \
		-o $(TEST_DIR)/small_en.wav 2>&1 | tee $(TEST_DIR)/small_en.wav.log
	$(call validate_wav,$(TEST_DIR)/small_en.wav,0.6B English ryan)

test-small-it:
	@echo "--- 0.6B Italian ryan ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_SMALL) -s ryan -l Italian \
		--text "Ciao, questa è una prova del sistema di sintesi vocale." \
		-o $(TEST_DIR)/small_it.wav 2>&1 | tee $(TEST_DIR)/small_it.wav.log
	$(call validate_wav,$(TEST_DIR)/small_it.wav,0.6B Italian ryan)

test-small-vivian:
	@echo "--- 0.6B Italian vivian ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_SMALL) -s vivian -l Italian \
		--text "Buongiorno, come state oggi?" \
		-o $(TEST_DIR)/small_vivian.wav 2>&1 | tee $(TEST_DIR)/small_vivian.wav.log
	$(call validate_wav,$(TEST_DIR)/small_vivian.wav,0.6B Italian vivian)

test-small-stream:
	@echo "--- 0.6B Streaming WAV ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_SMALL) -s ryan -l English \
		--text "Hello, this is a streaming test of the system." \
		--stream -o $(TEST_DIR)/small_stream.wav 2>&1 | tee $(TEST_DIR)/small_stream.wav.log
	$(call validate_wav,$(TEST_DIR)/small_stream.wav,0.6B Streaming WAV)

test-small-stdout:
	@echo "--- 0.6B Raw PCM stdout ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_SMALL) -s ryan -l English \
		--text "Hello, this is a stdout test." \
		--stdout > $(TEST_DIR)/small_stdout.raw 2>$(TEST_DIR)/small_stdout.log
	@RAW_SIZE=$$(stat -f%z $(TEST_DIR)/small_stdout.raw 2>/dev/null || stat -c%s $(TEST_DIR)/small_stdout.raw 2>/dev/null); \
	 if [ "$$RAW_SIZE" -le 0 ]; then echo "FAIL: stdout produced no data"; exit 1; fi
	@echo "PASS: 0.6B Raw PCM stdout"
	@echo ""

test-small: test-small-en test-small-it test-small-vivian test-small-stream test-small-stdout
	@echo "=== All 0.6B tests passed ==="

# ── Large model (1.7B) tests ──

test-large-en:
	@echo "--- 1.7B English ryan ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l English \
		--text "Hello, this is a test of the text to speech system." \
		-o $(TEST_DIR)/large_en.wav 2>&1 | tee $(TEST_DIR)/large_en.wav.log
	$(call validate_wav,$(TEST_DIR)/large_en.wav,1.7B English ryan)

test-large-it:
	@echo "--- 1.7B Italian ryan ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l Italian \
		--text "Ciao, questa è una prova del sistema." \
		-o $(TEST_DIR)/large_it.wav 2>&1 | tee $(TEST_DIR)/large_it.wav.log
	$(call validate_wav,$(TEST_DIR)/large_it.wav,1.7B Italian ryan)

test-large-config:
	@echo "--- 1.7B config validation ---"
	@# Regression: config parser truncated nested objects, losing hidden_size=2048
	./$(TARGET) -d $(MODEL_LARGE) --text "Test." -o $(TEST_DIR)/large_cfg.wav 2>&1 | tee $(TEST_DIR)/large_cfg.log
	@if ! grep -q "hidden=2048" $(TEST_DIR)/large_cfg.log; then echo "FAIL: 1.7B hidden_size should be 2048"; exit 1; fi
	@if ! grep -q "inter=6144" $(TEST_DIR)/large_cfg.log; then echo "FAIL: 1.7B intermediate_size should be 6144"; exit 1; fi
	@if ! grep -q "MTP projection" $(TEST_DIR)/large_cfg.log; then echo "FAIL: 1.7B should have MTP projection"; exit 1; fi
	@if grep -q "MISSING" $(TEST_DIR)/large_cfg.log; then echo "FAIL: speech decoder weights MISSING"; exit 1; fi
	@echo "PASS: 1.7B config validation"
	@echo ""

test-large-instruct:
	@echo "--- 1.7B Instruct: angry ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l English \
		--text "I cannot believe you did that to me." \
		--instruct "Speak in a very angry and aggressive tone" \
		-o $(TEST_DIR)/large_angry.wav 2>&1 | tee $(TEST_DIR)/large_angry.wav.log
	$(call validate_wav,$(TEST_DIR)/large_angry.wav,1.7B Instruct angry)
	@echo "--- 1.7B Instruct: slow whisper ---"
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l English \
		--text "I cannot believe you did that to me." \
		--instruct "Speak very slowly and softly, in a sad whisper" \
		-o $(TEST_DIR)/large_whisper.wav 2>&1 | tee $(TEST_DIR)/large_whisper.wav.log
	$(call validate_wav,$(TEST_DIR)/large_whisper.wav,1.7B Instruct whisper)
	@echo "--- 1.7B Instruct: happy ---"
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l English \
		--text "I cannot believe you did that to me." \
		--instruct "Speak in a very happy, cheerful and excited tone" \
		-o $(TEST_DIR)/large_happy.wav 2>&1 | tee $(TEST_DIR)/large_happy.wav.log
	$(call validate_wav,$(TEST_DIR)/large_happy.wav,1.7B Instruct happy)

test-large-int8:
	@echo "--- 1.7B INT8 Italian ryan (seed 42) ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l Italian --seed 42 \
		--text "Ciao, come stai oggi? Spero tutto bene." \
		--int8 \
		-o $(TEST_DIR)/large_int8_it.wav 2>&1 | tee $(TEST_DIR)/large_int8_it.wav.log
	$(call validate_wav,$(TEST_DIR)/large_int8_it.wav,1.7B INT8 Italian ryan)
	@echo "--- 1.7B INT8 English ryan (seed 42) ---"
	./$(TARGET) -d $(MODEL_LARGE) -s ryan --seed 42 \
		--text "Hello, how are you doing today? I hope everything is going well." \
		--int8 \
		-o $(TEST_DIR)/large_int8_en.wav 2>&1 | tee $(TEST_DIR)/large_int8_en.wav.log
	$(call validate_wav,$(TEST_DIR)/large_int8_en.wav,1.7B INT8 English ryan)

test-large-int4:
	@echo "--- 1.7B INT4 Italian ryan (seed 42) ---"
	@mkdir -p $(TEST_DIR)
	./$(TARGET) -d $(MODEL_LARGE) -s ryan -l Italian --seed 42 \
		--text "Ciao, come stai oggi? Spero tutto bene." \
		--int4 \
		-o $(TEST_DIR)/large_int4_it.wav 2>&1 | tee $(TEST_DIR)/large_int4_it.wav.log
	$(call validate_wav,$(TEST_DIR)/large_int4_it.wav,1.7B INT4 Italian ryan)
	@echo "--- 1.7B INT4 English ryan (seed 42) ---"
	./$(TARGET) -d $(MODEL_LARGE) -s ryan --seed 42 \
		--text "Hello, how are you doing today? I hope everything is going well." \
		--int4 \
		-o $(TEST_DIR)/large_int4_en.wav 2>&1 | tee $(TEST_DIR)/large_int4_en.wav.log
	$(call validate_wav,$(TEST_DIR)/large_int4_en.wav,1.7B INT4 English ryan)

test-large-quant: test-large-int8 test-large-int4
	@echo "=== All 1.7B quantization tests passed ==="

test-large: test-large-config test-large-en test-large-it test-large-instruct
	@echo "=== All 1.7B tests passed ==="

# ── Cross-model regression tests ──

# Error-handling regression: bad invocations must FAIL cleanly (non-zero + clear message),
# never crash or silently succeed. No model needed -> fast + CI-friendly.
test-errors: $(TARGET)
	@echo "=== Error-handling test ==="
	@mkdir -p $(TEST_DIR)
	@if ./$(TARGET) -d $(MODEL_SMALL) >/dev/null 2>$(TEST_DIR)/err_notext.txt; then echo "FAIL: missing --text/--serve should error (exit 0)"; exit 1; fi
	@grep -qiE "text.*serve|--text" $(TEST_DIR)/err_notext.txt || { echo "FAIL: no clear message for missing --text"; cat $(TEST_DIR)/err_notext.txt; exit 1; }
	@echo "  PASS: missing --text/--serve errors cleanly"
	@if ./$(TARGET) -d /nonexistent_model_dir_xyz --text "x" -o /dev/null >/dev/null 2>$(TEST_DIR)/err_nomodel.txt; then echo "FAIL: nonexistent model dir should error (exit 0)"; exit 1; fi
	@echo "  PASS: nonexistent model dir errors cleanly"
	@if ./$(TARGET) --load-voice /nonexistent.qvoice -d $(MODEL_SMALL) --text "x" -o /dev/null >/dev/null 2>$(TEST_DIR)/err_novoice.txt; then echo "FAIL: missing .qvoice should error (exit 0)"; exit 1; fi
	@echo "  PASS: missing .qvoice errors cleanly"
	@echo "PASS: error-handling"
	@echo ""

test-emotion: $(TARGET)
	@echo "=== Expressivity / emotion (STEER) smoke test ==="
	@mkdir -p $(TEST_DIR)
	@# La emoción es una función qlsteer-STEER del modelo 1.7B; el peso de la receta (w12)
	@# es el que manda, y en el 0.6B la emoción queda neutra. La fuente de verdad es
	@# resolve_emotion_recipe en main.c.
	@if [ -d $(MODEL_LARGE) ]; then \
	   ./$(TARGET) -d $(MODEL_LARGE) -j1 -T 0 --seed 42 -s ryan -l Italian --emotion joy \
	     --text "La riunione inizia domani mattina." -o $(TEST_DIR)/em_joy.wav 2>$(TEST_DIR)/em_joy.log; \
	   grep -qi "Emotion 'joy': mode=STEER" $(TEST_DIR)/em_joy.log || { echo "FAIL: --emotion joy did not resolve to STEER"; cat $(TEST_DIR)/em_joy.log; exit 1; }; \
	   grep -qi "ryan_joy.qlsteer" $(TEST_DIR)/em_joy.log || { echo "FAIL: joy steer vector not loaded"; cat $(TEST_DIR)/em_joy.log; exit 1; }; \
	   test -s $(TEST_DIR)/em_joy.wav || { echo "FAIL: joy produced no audio"; exit 1; }; \
	   echo "  PASS: --emotion joy -> STEER ryan_joy.qlsteer + audio"; \
	   ./$(TARGET) -d $(MODEL_LARGE) -j1 -T 0 --seed 42 -s ryan -l Italian --emotion sad \
	     --text "La riunione inizia domani mattina." -o $(TEST_DIR)/em_sad.wav 2>$(TEST_DIR)/em_sad.log; \
	   grep -qi "Emotion 'sad': mode=STEER" $(TEST_DIR)/em_sad.log || { echo "FAIL: --emotion sad did not resolve to STEER"; cat $(TEST_DIR)/em_sad.log; exit 1; }; \
	   grep -qi "ryan_sad.qlsteer" $(TEST_DIR)/em_sad.log || { echo "FAIL: sad steer vector not loaded"; cat $(TEST_DIR)/em_sad.log; exit 1; }; \
	   echo "  PASS: --emotion sad -> STEER ryan_sad.qlsteer"; \
	 else echo "  SKIP: 1.7B model absent (emotion is a 1.7B STEER feature)"; fi
	@# 0.6B emotion is parked-neutral: --emotion must not crash, just produces audio.
	@./$(TARGET) -d $(MODEL_SMALL) -j1 -T 0 --seed 42 -s ryan -l Italian --emotion joy \
		--text "Ciao." -o $(TEST_DIR)/em_06b.wav 2>/dev/null; \
	 test -s $(TEST_DIR)/em_06b.wav || { echo "FAIL: 0.6B --emotion produced no audio"; exit 1; }
	@echo "  PASS: 0.6B --emotion parked-neutral (no crash, audio written)"
	@# Standalone --volume/--rate DSP (model-agnostic, no --emotion)
	@./$(TARGET) -d $(MODEL_SMALL) -j1 -T 0 --seed 42 -s ryan -l Italian --volume 1.2 --rate 0.9 \
		--text "Ciao." -o $(TEST_DIR)/em_vr.wav 2>$(TEST_DIR)/em_vr.log
	@grep -qi "Volume: 1.20" $(TEST_DIR)/em_vr.log && grep -qi "Rate: 0.90" $(TEST_DIR)/em_vr.log || { echo "FAIL: standalone --volume/--rate not applied"; cat $(TEST_DIR)/em_vr.log; exit 1; }
	@echo "  PASS: standalone --volume/--rate"
	@echo "PASS: expressivity/emotion smoke"
	@echo ""

# Emotion fine-tune (.expr graft) SMOKE — the CSP Italian emotion FT this branch ships.
# The WOW recipe: 1.7B + EN instruct + T1.1 + the dense .expr applied on a preset and on an
# --icl-only clone graft. 2 short renders (seed 42 = stable; some seeds glitch). Asserts the
# pack loads (N tensors applied) + non-empty audio. The .expr packs are local-only (large),
# so SKIP cleanly when absent instead of failing a fresh checkout. Pack override: EXPR_FT=...
# The shipped WIN pack (was italian_csp.expr — stale name, the test silently SKIPped;
# caught 2026-07-11 auditing a "too fast" test-all).
EXPR_FT ?= presets/expr/italian_csp_topk6.expr
EMO_FT_INSTR = Speak with warm, bright happiness, smiling through the words.
EMO_FT_TEXT  = Che bella notizia, sono davvero felicissimo oggi!
test-emotion-ft: $(TARGET)
	@echo "=== Emotion fine-tune (.expr graft) smoke — CSP Italian on 1.7B ==="
	@mkdir -p $(TEST_DIR)
	@# One shell block so a SKIP truly skips the whole target: in make each @-line is a
	@# separate shell, so `exit 0` on its own line only ends that line — the render below
	@# would still run (missing --expr pack) and FAIL. Keep the guards + body together.
	@if [ ! -f $(EXPR_FT) ]; then echo "  SKIP: $(EXPR_FT) not present (local-only emotion FT pack)"; exit 0; fi; \
	 if [ ! -d $(MODEL_LARGE) ]; then echo "  SKIP: $(MODEL_LARGE) not present"; exit 0; fi; \
	 ./$(TARGET) -d $(MODEL_LARGE) -j1 -T 1.1 --seed 42 -s ryan -l Italian \
		--expr $(EXPR_FT) --instruct "$(EMO_FT_INSTR)" \
		--text "$(EMO_FT_TEXT)" -o $(TEST_DIR)/ft_ryan.wav 2>$(TEST_DIR)/ft_ryan.log; \
	 grep -qiE "Expressivity: applied [1-9][0-9]*/" $(TEST_DIR)/ft_ryan.log || { echo "FAIL: .expr pack not applied (preset)"; cat $(TEST_DIR)/ft_ryan.log; exit 1; }; \
	 test -s $(TEST_DIR)/ft_ryan.wav || { echo "FAIL: preset+FT produced no audio"; exit 1; }; \
	 echo "  PASS: emotion FT pack applied on preset ryan -> audio"; \
	 if [ -f voices/galatea_graft.qvoice ]; then \
		./$(TARGET) -d $(MODEL_LARGE) -j1 -T 1.1 --seed 42 -l Italian \
			--load-voice voices/galatea_graft.qvoice --icl-only \
			--expr $(EXPR_FT) --instruct "$(EMO_FT_INSTR)" \
			--text "$(EMO_FT_TEXT)" -o $(TEST_DIR)/ft_clone.wav 2>$(TEST_DIR)/ft_clone.log; \
		grep -qiE "Expressivity: applied [1-9][0-9]*/" $(TEST_DIR)/ft_clone.log || { echo "FAIL: .expr pack not applied (clone graft)"; cat $(TEST_DIR)/ft_clone.log; exit 1; }; \
		test -s $(TEST_DIR)/ft_clone.wav || { echo "FAIL: clone+FT produced no audio"; exit 1; }; \
		echo "  PASS: emotion FT pack applied on galatea --icl-only graft -> audio"; \
	 else echo "  SKIP: voices/galatea_graft.qvoice not present (run: bash download_voices.sh)"; fi; \
	 echo "PASS: emotion fine-tune (.expr) smoke"
	@echo ""

test-batch: $(TARGET)
	@echo "=== Batched Talker step correctness (opt-in path vs single-stream) ==="
	@./$(TARGET) -d $(MODEL_SMALL) -j1 --batch-test 2>&1 | grep -E "probe|wiring|matmat path|batch-test"
	@./$(TARGET) -d $(MODEL_SMALL) -j1 --batch-test >/dev/null 2>&1 || { echo "FAIL: batched wiring not bit-exact vs single-stream"; exit 1; }
	@echo "  PASS: batched Talker step wiring is bit-exact; matmat path is a valid fp-order variant"
	@echo ""

# Real-kernel batched matmat throughput: qwen_matmat_{bf16,int8,q4_0} vs B*matvec,
# per precision, at the current thread count (override with QWEN_BATCH_B / -j via THREADS).
matmat-bench: $(TARGET)
	@echo "=== Batched matmat twins vs B*matvec (real kernels, 4 threads) ==="
	@./$(TARGET) --matmat-bench
	@echo "=== (single thread = compute-bound reference) ==="
	@./$(TARGET) --matmat-bench -j 1

# Compila las rutas de kernels de ISA que el host no activa (BFMMLA/SMMLA/VNNI/BF16 y
# las ARM desde x86) forzando su -march, para detectar errores de sintaxis sin
# hardware que las ejecute; no ejecuta nada. Solo comprueba la sintaxis: rápido, sin
# objetos. ISACHK hereda las definiciones e includes de la plataforma (OpenBLAS, shims
# MinGW, _GNU_SOURCE) y sustituye solo las banderas de ISA por las de cada comprobación.
ISACHK = $(filter-out $(ARCH_FLAGS),$(CFLAGS_BASE)) -fsyntax-only
check-isa:
	@echo "=== Compile-check newer-ISA paths (syntax only, not run) ==="
ifneq (,$(filter arm64 aarch64,$(UNAME_M)))
	@$(CC) $(ISACHK) -march=armv8.2-a+dotprod -DACCELERATE_NEW_LAPACK qwen_tts_kernels.c \
	  && echo "  arm armv8.2-a +dotprod    : OK (ARM64 baseline: M1, Graviton2)" || echo "  arm armv8.2-a +dotprod    : FAIL"
	@! $(CC) $(ISACHK) -march=armv8-a -DACCELERATE_NEW_LAPACK qwen_tts_kernels.c 2>/dev/null \
	  && echo "  arm armv8-a no dotprod   : rejected by #error (OK)" || echo "  arm armv8-a no dotprod   : FAIL (must be rejected)"
	@$(CC) $(ISACHK) -march=armv8.6-a+bf16+i8mm -DACCELERATE_NEW_LAPACK qwen_tts_kernels.c \
	  && echo "  arm armv8.6-a +bf16 +i8mm : OK (M2/M3/M4, Graviton3+)" || echo "  arm armv8.6-a +bf16 +i8mm : FAIL"
	@$(CC) $(ISACHK) -march=armv9-a+sme2 -DACCELERATE_NEW_LAPACK qwen_tts_kernels.c 2>/dev/null \
	  && echo "  arm armv9-a +sme2        : OK (M4/M5)" || echo "  arm armv9-a +sme2        : (toolchain lacks SME2 — skipped)"
	@# Cross-compile the x86 AVX-512-VNNI paths (incl. the q4 VNNI matvec) from the ARM
	@# Mac via clang -target, so x86 kernel breakage is caught here without a rented box.
	@clang -target x86_64-apple-macos13 $(ISACHK) -march=x86-64-v3 -mavx512f -mavx512bw -mavx512dq -mavx512vnni -mavx512bf16 \
	  qwen_tts_kernels.c 2>/dev/null \
	  && echo "  x86 avx512 +vnni (x-comp) : OK (Zen4/5, Ice Lake+; q4-VNNI)" || echo "  x86 avx512 +vnni (x-comp) : (clang cross lacks target — skipped)"
	@# talker.c carries the AVX-512 bf16<->f32 bulk-conversion paths (avx512-parity) —
	@# compile-check it for x86 too (needs the ingot include).
	@clang -target x86_64-apple-macos13 $(ISACHK) -Ithird_party/ingot/include -march=x86-64-v3 \
	  -mavx512f -mavx512bw -mavx512dq -mavx512vnni -mavx512bf16 qwen_tts_talker.c 2>/dev/null \
	  && echo "  x86 avx512 talker (x-comp): OK (bf16 conv paths)" || echo "  x86 avx512 talker (x-comp): (clang cross lacks target — skipped)"
else
	@$(CC) $(ISACHK) -march=x86-64-v3 -mavx512f -mavx512bw -mavx512dq -mavx512vnni -mavx512bf16 qwen_tts_kernels.c \
	  && echo "  x86 avx512 +vnni +bf16   : OK (Zen4/5, Ice Lake+)" || echo "  x86 avx512 +vnni +bf16   : FAIL"
	@$(CC) $(ISACHK) -march=sapphirerapids qwen_tts_kernels.c 2>/dev/null \
	  && echo "  x86 sapphirerapids (AMX) : OK" || echo "  x86 sapphirerapids (AMX) : (toolchain lacks AMX — skipped)"
	@# Rutas ARM (SDOT e i8mm) compiladas con clang cruzado desde el host x86.
	@clang $(ISACHK_ARM_X) $(ISACHK) -march=armv8.2-a+dotprod qwen_tts_kernels.c 2>/dev/null \
	  && echo "  arm armv8.2-a +dotprod (x-comp): OK" || echo "  arm armv8.2-a +dotprod (x-comp): (clang cross lacks target — skipped)"
	@clang $(ISACHK_ARM_X) $(ISACHK) -march=armv8.6-a+bf16+i8mm qwen_tts_kernels.c 2>/dev/null \
	  && echo "  arm armv8.6-a +bf16 +i8mm (x-comp): OK" || echo "  arm armv8.6-a +bf16 +i8mm (x-comp): (clang cross lacks target — skipped)"
endif
	@echo "  (la ruta escalar se verifica con --self-test en una compilación SIMD=scalar)"

test-compose: $(TARGET)
	@echo "=== Inline markup / --compose smoke test ==="
	@mkdir -p $(TEST_DIR)
	@# Inline markup auto-detected in --text -> multi-span compose
	@./$(TARGET) -d $(MODEL_SMALL) -j1 -T 0 --seed 42 -s ryan -l Italian \
		--text "Che bella notizia! [pause:400ms] [sad] Devo andare... [sigh] [neutral] Ciao." \
		-o $(TEST_DIR)/mk_inline.wav 2>$(TEST_DIR)/mk_inline.log
	@grep -qi "Inline markup detected" $(TEST_DIR)/mk_inline.log || { echo "FAIL: inline markup not auto-detected in --text"; cat $(TEST_DIR)/mk_inline.log; exit 1; }
	@# 3 spans: neutral / sad / neutral. [sigh] is an INLINE para tag -> folded as an
	@# onomatopoeia into the adjacent (sad) span's text (one generation, voice-native
	@# timbre), NOT a separate span. The old split/steering-span "splice" that made it a
	@# 4th span was rejected & deleted (CLAUDE.md para protocol) — so 3 is correct here.
	@grep -qi "composed 3 spans" $(TEST_DIR)/mk_inline.log || { echo "FAIL: expected 3 spans (neutral/sad+[sigh]/neutral)"; cat $(TEST_DIR)/mk_inline.log; exit 1; }
	@grep -qi "inline \[tag\]->onomatopoeia" $(TEST_DIR)/mk_inline.log || { echo "FAIL: [sigh] not folded inline as onomatopoeia"; cat $(TEST_DIR)/mk_inline.log; exit 1; }
	@grep -qi "pause 0.40s" $(TEST_DIR)/mk_inline.log || { echo "FAIL: [pause:400ms] not parsed"; cat $(TEST_DIR)/mk_inline.log; exit 1; }
	@test -s $(TEST_DIR)/mk_inline.wav || { echo "FAIL: no audio"; exit 1; }
	@echo "  PASS: inline [tag] markup in --text (3 spans, pause, [sigh] folded inline)"
	@# Plain text (no tags) must NOT trigger compose
	@./$(TARGET) -d $(MODEL_SMALL) -j1 -T 0 --seed 42 -s ryan -l Italian \
		--text "Frase normale senza tag." -o $(TEST_DIR)/mk_plain.wav 2>$(TEST_DIR)/mk_plain.log
	@if grep -qi "compose mode" $(TEST_DIR)/mk_plain.log; then echo "FAIL: plain text wrongly routed to compose"; cat $(TEST_DIR)/mk_plain.log; exit 1; fi
	@echo "  PASS: plain text stays on the normal path"
	@# Explicit --compose with English macros + pause
	@./$(TARGET) -d $(MODEL_SMALL) -j1 -T 0 --seed 42 -s ryan -l English \
		--compose "[excited] We won! | [pause:0.5] | [sad] But it is over. [sigh]" \
		-o $(TEST_DIR)/mk_compose.wav 2>$(TEST_DIR)/mk_compose.log
	@grep -qi "composed" $(TEST_DIR)/mk_compose.log || { echo "FAIL: --compose did not render"; cat $(TEST_DIR)/mk_compose.log; exit 1; }
	@echo "  PASS: explicit --compose"
	@echo "PASS: inline markup / compose smoke"
	@echo ""

test-regression:
	@echo "=== Regression tests ==="
	@echo ""
	@echo "--- Safetensors format (must load standard HF format, not custom .bin) ---"
	@# Both models must load from model.safetensors (no weights.bin)
	@if [ -f $(MODEL_SMALL)/weights.bin ]; then echo "WARN: weights.bin found in 0.6B dir (should use model.safetensors)"; fi
	@if [ -f $(MODEL_LARGE)/weights.bin ]; then echo "WARN: weights.bin found in 1.7B dir (should use model.safetensors)"; fi
	@if [ ! -f $(MODEL_SMALL)/model.safetensors ]; then echo "FAIL: 0.6B model.safetensors missing"; exit 1; fi
	@if [ ! -f $(MODEL_LARGE)/model.safetensors ]; then echo "FAIL: 1.7B model.safetensors missing"; exit 1; fi
	@if [ ! -f $(MODEL_SMALL)/speech_tokenizer/model.safetensors ]; then echo "FAIL: 0.6B speech_tokenizer missing"; exit 1; fi
	@if [ ! -f $(MODEL_LARGE)/speech_tokenizer/model.safetensors ]; then echo "FAIL: 1.7B speech_tokenizer missing"; exit 1; fi
	@echo "PASS: safetensors files present"
	@echo ""
	@echo "--- 0.6B vs 1.7B config divergence ---"
	./$(TARGET) -d $(MODEL_SMALL) --text "x" -o /dev/null 2>&1 | grep "^Config:" > $(TEST_DIR)/cfg_small.txt || true
	./$(TARGET) -d $(MODEL_LARGE) --text "x" -o /dev/null 2>&1 | grep "^Config:" > $(TEST_DIR)/cfg_large.txt || true
	@# 0.6B must have hidden=1024, 1.7B must have hidden=2048
	@if ! grep -q "hidden=1024" $(TEST_DIR)/cfg_small.txt; then echo "FAIL: 0.6B should have hidden=1024"; exit 1; fi
	@if ! grep -q "hidden=2048" $(TEST_DIR)/cfg_large.txt; then echo "FAIL: 1.7B should have hidden=2048"; exit 1; fi
	@# Both must have same head_dim=128 and same CP hidden=1024
	@if ! grep -q "head_dim=128" $(TEST_DIR)/cfg_small.txt; then echo "FAIL: 0.6B head_dim"; exit 1; fi
	@if ! grep -q "head_dim=128" $(TEST_DIR)/cfg_large.txt; then echo "FAIL: 1.7B head_dim"; exit 1; fi
	@echo "PASS: config divergence correct"
	@echo ""
	@echo "=== All regression tests passed ==="

# ── Combined ──

test-all: test-small test-large test-regression test-errors test-emotion test-emotion-ft test-compose test-caps test-selftest
	@echo ""
	@echo "========================================="
	@echo "  All tests passed (0.6B + 1.7B)"
	@echo "========================================="

# ── Capability self-report regression (catches "we thought AVX existed") ──
# Asserts the binary's --caps report is internally consistent with the build arch, so a
# false "we have AVX2/threading" belief can't survive: the binary states the truth and this
# test enforces it. On ARM it MUST report NEON; on x86 it MUST report AVX2 (default build) or
# scalar (SIMD=scalar) for matvec — a regression to the old un-wired SCALAR fails loudly. The
# threads line must report an active pool (GCD/pthread/Win32), never SINGLE-THREAD.
# Pure introspection, no model needed.

test-caps: $(TARGET)
	@echo "=== Capability report test ==="
	@mkdir -p $(TEST_DIR)
	@./$(TARGET) --caps | tee $(TEST_DIR)/caps.txt
	@grep -q "matvec + attn:" $(TEST_DIR)/caps.txt || { echo "FAIL: --caps missing matvec line"; exit 1; }
	@grep -q "matvec threads:" $(TEST_DIR)/caps.txt || { echo "FAIL: --caps missing threads line"; exit 1; }
	@grep -q "int8 dot:" $(TEST_DIR)/caps.txt || { echo "FAIL: --caps missing int8 dot line"; exit 1; }
	@if grep -q "arch:.*arm64" $(TEST_DIR)/caps.txt; then \
	   grep -q "matvec + attn:    NEON" $(TEST_DIR)/caps.txt || { echo "FAIL: arm64 build must report NEON matvec"; exit 1; }; \
	 elif grep -q "arch:.*x86-64" $(TEST_DIR)/caps.txt; then \
	   grep -qE "matvec \+ attn:    (AVX2|scalar)" $(TEST_DIR)/caps.txt || { echo "FAIL: x86 must report AVX2 (default) or scalar (SIMD=scalar) matvec"; exit 1; }; \
	   if grep -q "WARNING: built with AVX2 but this CPU lacks it" $(TEST_DIR)/caps.txt; then echo "FAIL: AVX2 build on a non-AVX2 CPU"; exit 1; fi; \
	 fi
	@grep -q "matvec threads:" $(TEST_DIR)/caps.txt && ! grep -q "SINGLE-THREAD" $(TEST_DIR)/caps.txt || { echo "FAIL: threads must report an active pool (GCD/pthread/Win32), not SINGLE-THREAD"; exit 1; }
	@echo "PASS: --caps report consistent with build arch"
	@echo ""

# ── Kernel numeric self-test (matvec correctness vs f32 reference) ──
# Prueba de corrección entre ISA de las multiplicaciones SIMD (bf16, int8, argmax, q4_0 y
# sus matmat). No depende de un audio de referencia completo, así que no le afecta la
# bifurcación de trayectoria de la decodificación voraz, que convierte la comparación de
# audio entre ISA en una falsa alarma. Ejecuta una sola pasada sobre la ruta compilada
# para la ISA de la compilación; en una máquina AVX-512 es la prueba de que el producto
# int8 VNNI y el matvec bf16 de __m512 son correctos. No necesita modelo: rápida.
test-selftest: $(TARGET)
	@echo "=== Kernel self-test ==="
	@./$(TARGET) --self-test || { echo "FAIL: kernel self-test"; exit 1; }
	@echo "PASS: kernel self-test (numerically correct)"
	@echo ""

GOLDEN_EN = The quick brown fox jumps over the lazy dog on a sunny afternoon.

# ── Mode matrix: quant × delivery (the combinations real usage hits) ──
# Each combination must RUN and produce coherent audio (non-empty + frames). Here we
# assert the CROSS-PRODUCT works: int8/bf16 × normal/stream. One shell so a failure
# stops cleanly. Reloads the model each run (natural gap → reliable, unlike rapid-fire).
test-modes: $(TARGET)
	@echo "=== Mode matrix (quant × delivery) 0.6B ==="
	@mkdir -p $(TEST_DIR)
	@chk() { sz=$$(stat -f%z "$$1" 2>/dev/null || stat -c%s "$$1" 2>/dev/null || echo 0); \
	   if [ "$$sz" -le 44 ] || ! grep -q "Generated [1-9]" "$$1.log"; then echo "FAIL: $$2"; exit 1; fi; \
	   if grep -qi "nan" "$$1.log"; then echo "FAIL: $$2 (NaN)"; exit 1; fi; \
	   echo "  PASS: $$2 ($$sz B)"; }; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 -s ryan -l English --text "$(GOLDEN_EN)" -o $(TEST_DIR)/m_bf.wav >$(TEST_DIR)/m_bf.wav.log 2>&1; chk $(TEST_DIR)/m_bf.wav "bf16 normal"; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 -s ryan -l English --stream --text "$(GOLDEN_EN)" -o $(TEST_DIR)/m_bfs.wav >$(TEST_DIR)/m_bfs.wav.log 2>&1; chk $(TEST_DIR)/m_bfs.wav "bf16 stream"; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 -s ryan -l English --int8 --text "$(GOLDEN_EN)" -o $(TEST_DIR)/m_i8.wav >$(TEST_DIR)/m_i8.wav.log 2>&1; chk $(TEST_DIR)/m_i8.wav "int8 normal"; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 -s ryan -l English --int8 --stream --text "$(GOLDEN_EN)" -o $(TEST_DIR)/m_i8s.wav >$(TEST_DIR)/m_i8s.wav.log 2>&1; chk $(TEST_DIR)/m_i8s.wav "int8 stream"; \
	 echo "PASS: mode matrix (4 combinations)"
	@echo ""

# ── Custom voice (.qvoice) — skip-if-absent (voices/ is gitignored / local-only) ──
test-qvoice: $(TARGET)
	@echo "=== Custom voice (.qvoice) test ==="
	@if [ ! -f voices/silvio_06b.qvoice ]; then echo "SKIP: voices/silvio_06b.qvoice not present (local-only)"; exit 0; fi; \
	 mkdir -p $(TEST_DIR); \
	 chk() { sz=$$(stat -f%z "$$1" 2>/dev/null || stat -c%s "$$1" 2>/dev/null || echo 0); \
	   if [ "$$sz" -le 44 ] || ! grep -q "Generated [1-9]" "$$1.log"; then echo "FAIL: $$2"; exit 1; fi; echo "  PASS: $$2 ($$sz B)"; }; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 -l Italian --load-voice voices/silvio_06b.qvoice --text "Buongiorno, questo e un test della voce." -o $(TEST_DIR)/qv.wav >$(TEST_DIR)/qv.wav.log 2>&1; chk $(TEST_DIR)/qv.wav "qvoice bf16"; \
	 ./$(TARGET) -d $(MODEL_SMALL) -j1 --seed 42 --int8 -l Italian --load-voice voices/silvio_06b.qvoice --text "Buongiorno, questo e un test della voce." -o $(TEST_DIR)/qvi.wav >$(TEST_DIR)/qvi.wav.log 2>&1; chk $(TEST_DIR)/qvi.wav "qvoice int8"; \
	 echo "PASS: custom voice (bf16 + int8)"
	@echo ""

# ── E2E: ONE command that runs EVERYTHING available (skips missing models/voices) ──
# This is the comprehensive regression: small/large/regression/errors/caps +
# quant (int8/int4) + mode matrix + custom voice + clone + voice-design + server suite.
e2e: $(TARGET)
	@echo "######################## E2E FULL REGRESSION ########################"
	@$(MAKE) --no-print-directory test-all
	@$(MAKE) --no-print-directory test-large-quant
	@$(MAKE) --no-print-directory test-modes
	@$(MAKE) --no-print-directory test-qvoice
	@$(MAKE) --no-print-directory test-clone
	@$(MAKE) --no-print-directory test-voice-design
	@$(MAKE) --no-print-directory test-serve-all
	@echo "######################## E2E COMPLETE — all green ########################"

# ── HTTP Server ──

serve: $(TARGET)
	./$(TARGET) -d $(MODEL_SMALL) --serve 8080

test-serve: $(TARGET)
	@echo "--- HTTP Server test ---"
	@mkdir -p $(TEST_DIR)
	@./$(TARGET) -d $(MODEL_SMALL) --serve 8090 &>/dev/null & SERVER_PID=$$!; \
	 sleep 4; \
	 echo "  Testing /v1/health..."; \
	 HEALTH=$$(curl -s http://localhost:8090/v1/health); \
	 if ! echo "$$HEALTH" | grep -q '"ok"'; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: health check"; exit 1; fi; \
	 echo "  Testing /v1/speakers..."; \
	 SPEAKERS=$$(curl -s http://localhost:8090/v1/speakers); \
	 if ! echo "$$SPEAKERS" | grep -q '"ryan"'; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: speakers"; exit 1; fi; \
	 echo "  Testing /v1/tts..."; \
	 curl -s -X POST http://localhost:8090/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"Test.","speaker":"ryan"}' \
	   -o $(TEST_DIR)/serve_test.wav; \
	 if [ ! -f $(TEST_DIR)/serve_test.wav ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: no WAV"; exit 1; fi; \
	 WAV_SIZE=$$(stat -f%z $(TEST_DIR)/serve_test.wav 2>/dev/null || stat -c%s $(TEST_DIR)/serve_test.wav 2>/dev/null); \
	 if [ "$$WAV_SIZE" -le 44 ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: empty WAV"; exit 1; fi; \
	 kill $$SERVER_PID 2>/dev/null; \
	 echo "PASS: HTTP Server test"
	@echo ""

# ── Server benchmark: 2 sequential runs, same seed (bit-identical output) ──

test-serve-bench: $(TARGET)
	@echo "=== Server Benchmark (seed=42, 2 runs) ==="
	@mkdir -p $(TEST_DIR)
	@./$(TARGET) -d $(MODEL_SMALL) --serve 8091 &>/dev/null & SERVER_PID=$$!; \
	 sleep 4; \
	 echo "--- Run 1 (cold) ---"; \
	 T1=$$(curl -s -w "%{time_total}" -X POST http://localhost:8091/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"The quick brown fox jumps over the lazy dog on a sunny afternoon.","speaker":"ryan","language":"English","seed":42}' \
	   -o $(TEST_DIR)/bench_run1.wav); \
	 S1=$$(stat -f%z $(TEST_DIR)/bench_run1.wav 2>/dev/null || stat -c%s $(TEST_DIR)/bench_run1.wav 2>/dev/null); \
	 echo "  $${T1}s, $$S1 bytes"; \
	 if [ "$$S1" -le 44 ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: empty WAV"; exit 1; fi; \
	 echo "--- Run 2 (warm) ---"; \
	 T2=$$(curl -s -w "%{time_total}" -X POST http://localhost:8091/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"The quick brown fox jumps over the lazy dog on a sunny afternoon.","speaker":"ryan","language":"English","seed":42}' \
	   -o $(TEST_DIR)/bench_run2.wav); \
	 S2=$$(stat -f%z $(TEST_DIR)/bench_run2.wav 2>/dev/null || stat -c%s $(TEST_DIR)/bench_run2.wav 2>/dev/null); \
	 echo "  $${T2}s, $$S2 bytes"; \
	 echo "--- Comparing outputs ---"; \
	 MD5_1=$$(md5sum $(TEST_DIR)/bench_run1.wav 2>/dev/null | cut -d' ' -f1 || md5 -q $(TEST_DIR)/bench_run1.wav 2>/dev/null); \
	 MD5_2=$$(md5sum $(TEST_DIR)/bench_run2.wav 2>/dev/null | cut -d' ' -f1 || md5 -q $(TEST_DIR)/bench_run2.wav 2>/dev/null); \
	 if [ "$$MD5_1" != "$$MD5_2" ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: outputs differ ($$MD5_1 vs $$MD5_2)"; exit 1; fi; \
	 kill $$SERVER_PID 2>/dev/null; \
	 echo "PASS: identical output ($$MD5_1)"
	@echo ""

# ── Server OpenAI-compatible API test ──

test-serve-openai: $(TARGET)
	@echo "=== Server OpenAI API test ==="
	@mkdir -p $(TEST_DIR)
	@./$(TARGET) -d $(MODEL_SMALL) --serve 8092 &>/dev/null & SERVER_PID=$$!; \
	 sleep 4; \
	 echo "--- /v1/audio/speech (OpenAI-compatible) ---"; \
	 HTTP_CODE=$$(curl -s -w "%{http_code}" -X POST http://localhost:8092/v1/audio/speech \
	   -H "Content-Type: application/json" \
	   -d '{"input":"Hello, this is a test of the OpenAI compatible endpoint.","voice":"ryan","seed":42}' \
	   -o $(TEST_DIR)/openai_test.wav); \
	 if [ "$$HTTP_CODE" != "200" ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: HTTP $$HTTP_CODE"; exit 1; fi; \
	 WAV_SIZE=$$(stat -f%z $(TEST_DIR)/openai_test.wav 2>/dev/null || stat -c%s $(TEST_DIR)/openai_test.wav 2>/dev/null); \
	 if [ "$$WAV_SIZE" -le 44 ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: empty WAV ($$WAV_SIZE bytes)"; exit 1; fi; \
	 echo "  HTTP 200, $$WAV_SIZE bytes"; \
	 echo "--- Verify same seed produces same output via /v1/tts ---"; \
	 curl -s -X POST http://localhost:8092/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"Hello, this is a test of the OpenAI compatible endpoint.","speaker":"ryan","seed":42}' \
	   -o $(TEST_DIR)/openai_ref.wav; \
	 MD5_OAI=$$(md5sum $(TEST_DIR)/openai_test.wav 2>/dev/null | cut -d' ' -f1 || md5 -q $(TEST_DIR)/openai_test.wav 2>/dev/null); \
	 MD5_REF=$$(md5sum $(TEST_DIR)/openai_ref.wav 2>/dev/null | cut -d' ' -f1 || md5 -q $(TEST_DIR)/openai_ref.wav 2>/dev/null); \
	 if [ "$$MD5_OAI" != "$$MD5_REF" ]; then kill $$SERVER_PID 2>/dev/null; echo "FAIL: OpenAI and TTS endpoints differ"; exit 1; fi; \
	 kill $$SERVER_PID 2>/dev/null; \
	 echo "PASS: OpenAI API (identical to /v1/tts)"
	@echo ""

# ── Server parallel requests test ──

test-serve-parallel: $(TARGET)
	@echo "=== Server Parallel Requests test ==="
	@mkdir -p $(TEST_DIR)
	@./$(TARGET) -d $(MODEL_SMALL) --serve 8093 &>/dev/null & SERVER_PID=$$!; \
	 sleep 4; \
	 echo "--- Sending 2 concurrent requests ---"; \
	 curl -s -w "Req1: HTTP %{http_code} in %{time_total}s\n" -X POST http://localhost:8093/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"Hello, this is request number one.","speaker":"ryan","seed":100}' \
	   -o $(TEST_DIR)/parallel_1.wav & PID1=$$!; \
	 curl -s -w "Req2: HTTP %{http_code} in %{time_total}s\n" -X POST http://localhost:8093/v1/tts \
	   -H "Content-Type: application/json" \
	   -d '{"text":"And this is request number two.","speaker":"vivian","seed":200}' \
	   -o $(TEST_DIR)/parallel_2.wav & PID2=$$!; \
	 wait $$PID1; wait $$PID2; \
	 echo "--- Validating outputs ---"; \
	 FAIL=0; \
	 for f in $(TEST_DIR)/parallel_1.wav $(TEST_DIR)/parallel_2.wav; do \
	   if [ ! -f "$$f" ]; then echo "FAIL: $$f not created"; FAIL=1; continue; fi; \
	   SZ=$$(stat -f%z "$$f" 2>/dev/null || stat -c%s "$$f" 2>/dev/null); \
	   if [ "$$SZ" -le 44 ]; then echo "FAIL: $$f empty ($$SZ bytes)"; FAIL=1; else echo "  $$f: $$SZ bytes"; fi; \
	 done; \
	 kill $$SERVER_PID 2>/dev/null; \
	 if [ "$$FAIL" -ne 0 ]; then echo "FAIL: parallel test"; exit 1; fi; \
	 echo "PASS: 2 parallel requests served"
	@echo ""

# ── Combined server tests ──

test-serve-all: test-serve test-serve-bench test-serve-openai test-serve-parallel
	@echo "=== All server tests passed ==="

# ── RTF Benchmarks ──
# Quick RTF measurements across configs. Auto-skips missing models/voices.

bench: $(TARGET)
	@./bench.sh --level basic --seed 42

bench-full: $(TARGET)
	@./bench.sh --level full --seed 42

# ── Voice Clone e2e test ──
# Step 1: Generate reference audio with CustomVoice model
# Step 2: Use that audio as voice clone reference with Base model (different text)
# Step 3: Also test streaming + voice clone

test-clone: $(TARGET)
	@echo "=== Voice Clone e2e test ==="
	@if [ ! -d $(MODEL_SMALL) ]; then echo "SKIP: $(MODEL_SMALL) not found (run ./download_model.sh --model small)"; exit 0; fi
	@if [ ! -d $(MODEL_BASE_SMALL) ]; then echo "SKIP: $(MODEL_BASE_SMALL) not found (run ./download_model.sh --model base-small)"; exit 0; fi
	@mkdir -p $(TEST_DIR)
	@echo ""
	@echo "--- Step 1: Generate reference audio (CustomVoice) ---"
	./$(TARGET) -d $(MODEL_SMALL) -s ryan -l English \
		--text "The weather is beautiful today, perfect for a walk in the park." \
		--seed 42 \
		-o $(TEST_DIR)/clone_ref.wav 2>&1 | tee $(TEST_DIR)/clone_ref.wav.log
	$(call validate_wav,$(TEST_DIR)/clone_ref.wav,Voice Clone: reference generation)
	@echo "--- Step 2: Clone voice with different text ---"
	./$(TARGET) -d $(MODEL_BASE_SMALL) \
		--text "I love programming in C, it gives you complete control over the machine." \
		--ref-audio $(TEST_DIR)/clone_ref.wav \
		--xvector-only \
		-o $(TEST_DIR)/clone_output.wav 2>&1 | tee $(TEST_DIR)/clone_output.wav.log
	$(call validate_wav,$(TEST_DIR)/clone_output.wav,Voice Clone: cloned output)
	@if ! grep -q "Voice clone:" $(TEST_DIR)/clone_output.wav.log; then echo "FAIL: voice clone not active"; exit 1; fi
	@if ! grep -q "speaker embedding" $(TEST_DIR)/clone_output.wav.log; then echo "FAIL: no speaker embedding extracted"; exit 1; fi
	@echo "--- Step 3: Clone voice + streaming ---"
	./$(TARGET) -d $(MODEL_BASE_SMALL) \
		--text "Streaming also works perfectly with voice cloning mode." \
		--ref-audio $(TEST_DIR)/clone_ref.wav \
		--xvector-only \
		--stream \
		-o $(TEST_DIR)/clone_stream.wav 2>&1 | tee $(TEST_DIR)/clone_stream.wav.log
	$(call validate_wav,$(TEST_DIR)/clone_stream.wav,Voice Clone: streaming)
	@if ! grep -q "streamed" $(TEST_DIR)/clone_stream.wav.log; then echo "FAIL: not streamed"; exit 1; fi
	@echo "=== Voice Clone e2e test passed ==="
	@echo "Listen:"
	@echo "  Reference:  afplay $(TEST_DIR)/clone_ref.wav"
	@echo "  Cloned:     afplay $(TEST_DIR)/clone_output.wav"
	@echo "  Streamed:   afplay $(TEST_DIR)/clone_stream.wav"

# ── VoiceDesign test ──

# NOTE: the whole body runs in ONE shell (\ continuations) so the SKIP `exit 0`
# actually stops the recipe — a per-line `@if ...; exit 0; fi` only exits its own
# sub-shell and the following model-run lines would still execute (the old bug).
test-voice-design: $(TARGET)
	@echo "=== VoiceDesign test ==="
	@if [ ! -f $(MODEL_VOICE_DESIGN)/model.safetensors ]; then \
	   echo "SKIP: $(MODEL_VOICE_DESIGN) not found or incomplete (run ./download_model.sh --model voice-design)"; \
	   exit 0; \
	 fi; \
	 mkdir -p $(TEST_DIR); \
	 echo "--- VoiceDesign: British male ---"; \
	 ./$(TARGET) -d $(MODEL_VOICE_DESIGN) -l English --voice-design \
	   --instruct "A deep male voice with a British accent, speaking slowly and calmly" \
	   --text "Good evening, welcome to the broadcast." \
	   -o $(TEST_DIR)/vd_british.wav 2>&1 | tee $(TEST_DIR)/vd_british.wav.log; \
	 if [ ! -s $(TEST_DIR)/vd_british.wav ] || ! grep -q "Generated [1-9]" $(TEST_DIR)/vd_british.wav.log; then echo "FAIL: VoiceDesign British male"; exit 1; fi; \
	 echo "PASS: VoiceDesign British male"; \
	 echo "--- VoiceDesign: energetic female ---"; \
	 ./$(TARGET) -d $(MODEL_VOICE_DESIGN) -l English --voice-design \
	   --instruct "Young energetic female, cheerful and fast-paced" \
	   --text "Oh my gosh, this is so exciting!" \
	   -o $(TEST_DIR)/vd_cheerful.wav 2>&1 | tee $(TEST_DIR)/vd_cheerful.wav.log; \
	 if [ ! -s $(TEST_DIR)/vd_cheerful.wav ] || ! grep -q "Generated [1-9]" $(TEST_DIR)/vd_cheerful.wav.log; then echo "FAIL: VoiceDesign energetic female"; exit 1; fi; \
	 echo "PASS: VoiceDesign energetic female"; \
	 echo "=== VoiceDesign test passed ==="

# ── Voice Clone Demo ──
# Uses an existing sample WAV as reference to clone a voice with new text.
# Requires: Base model (download with ./download_model.sh --model base-small)

# Voice Clone Demo
# Usage:
#   make demo-clone REF=my_voice.wav             (REF es obligatorio: audio de referencia)
#   make demo-clone REF=my_voice.wav TEXT="Hi!"  (custom text too)
# Output saved to samples/ for easy listening.

TEXT ?= I love programming in C, it gives you complete control over the machine.
TEXT_IT ?= Buongiorno, questa e una dimostrazione della clonazione vocale.

demo-clone: $(TARGET)
	@test -n "$(REF)" || { echo "demo-clone: indica el audio de referencia con REF=<ruta.wav>" >&2; exit 1; }
	@echo "=== Voice Clone Demo ==="
	@if [ ! -d $(MODEL_BASE_SMALL) ]; then \
		echo "Error: $(MODEL_BASE_SMALL) not found"; \
		echo "Download it with: ./download_model.sh --model base-small"; \
		exit 1; \
	fi
	@if [ ! -f "$(REF)" ]; then \
		echo "Error: $(REF) not found"; \
		echo "Usage: make demo-clone REF=your_audio.wav"; \
		exit 1; \
	fi
	@mkdir -p samples
	@echo ""
	@echo "Reference audio: $(REF)"
	@echo ""
	@echo "--- Cloning voice (English) ---"
	./$(TARGET) -d $(MODEL_BASE_SMALL) -l English \
		--text "$(TEXT)" \
		--ref-audio "$(REF)" \
		--xvector-only \
		-o samples/clone_output_en.wav
	@echo ""
	@echo "--- Cloning voice (Italian) ---"
	./$(TARGET) -d $(MODEL_BASE_SMALL) -l Italian \
		--text "$(TEXT_IT)" \
		--ref-audio "$(REF)" \
		--xvector-only \
		-o samples/clone_output_it.wav
	@echo ""
	@echo "=== Demo complete ==="
	@echo "Output saved to samples/"
	@echo ""
	@echo "Listen:"
	@echo "  Reference:  afplay $(REF)"
	@echo "  English:    afplay samples/clone_output_en.wav"
	@echo "  Italian:    afplay samples/clone_output_it.wav"

# Legacy aliases
test-en: test-small-en
test-it-ryan: test-small-it

.PHONY: all help blas clean debug info serve cp-microbench test-batch test-errors test-emotion test-emotion-ft test-compose test-caps test-selftest test-modes test-qvoice e2e \
        test-serve test-serve-bench test-serve-openai test-serve-parallel test-serve-all \
        test-clone test-voice-design \
        demo-clone \
        test-small test-small-en test-small-it test-small-vivian test-small-stream test-small-stdout \
        test-large test-large-en test-large-it test-large-config test-large-instruct \
        test-large-int8 test-large-int4 test-large-quant \
        test-regression test-all test-en test-it-ryan
